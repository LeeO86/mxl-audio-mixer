//! Voice activity detection (§8.1), on the control side from a signal's tapped
//! audio (the mean of its channels before its auto gain, 48 kHz).
//!
//! A [`Detector`] turns each 10 ms frame into a speech probability; [`Vad`] adds
//! the smoothing, the threshold with hysteresis, the hold, and the outputs
//! (`speech_active`, `speech_probability`, `speech_level`, onsets and offsets).
//! [`FeatureDetector`] is v1's lightweight detector; a model-based one can take
//! its place behind the same trait.
//!
//! The features, per frame:
//! - **energy over the noise floor:** the power of the 250 Hz–4 kHz band (two
//!   second-order sections) in dB, against a floor that is the minimum of the
//!   smoothed band power over the last 2 s (minimum statistics: eight 250 ms
//!   windows). 4 dB above the floor counts 0, 14 dB counts 1;
//! - **voicing:** the largest normalised autocorrelation for pitch periods of
//!   80–400 Hz, on that band low-passed at 1 kHz (fourth-order Butterworth),
//!   decimated to 6 kHz, over the last 40 ms (the harmonics carry the period of
//!   a fundamental below 250 Hz). 0.4 counts 0, 0.75 counts 1;
//! - **modulation:** the standard deviation of the band power (dB) over the last
//!   400 ms: speech rises and falls with its syllables, noise and tones do not.
//!   2 dB counts 0, 6 dB counts 1;
//! - **a pure tone:** an autocorrelation above 0.9 that also reaches −0.9 at
//!   some lag in that range (a sine's half period) is a tone (line-up, idents,
//!   sweeps), never speech.
//!
//! Voicing and the tone count only when the voicing window's two halves are
//! within 10 dB: across an onset or an end any sound looks periodic.
//!
//! The frame's probability is energy × modulation × (0.3 + 0.7 × voicing), 0 for
//! a tone; unvoiced sounds (fricatives) thus count less than vowels and are
//! carried by the hold. Each of the counts is a smoothstep between its two
//! values.
//!
//! **The gate** (as Com-IS-Assist's guard band): while the signal's momentary
//! loudness (BS.1770 over its channels, 400 ms) is below `gate_lufs`, a frame counts as no
//! speech. A detector hears crosstalk of other talkers into a microphone as
//! speech, rightly; it is quieter than the microphone's own talker.
//!
//! **Detection latency:** a frame is analysed when its 10 ms are complete; the
//! voicing window adds about half its 40 ms; the probability rises with a time
//! constant of 14 ms; after silence the momentary loudness needs
//! 400 ms × 10^((gate − level)/10) to reach the gate (60 ms for a talker 8 dB
//! above it). A voiced onset is detected about 50–70 ms after it starts (unit
//! tests below; the delivery to the control side adds up to one 10 ms tick).

use std::collections::VecDeque;

use crate::dsp::biquad::{self, Coefficients, State};

/// 10 ms at 48 kHz.
pub const FRAME: usize = 480;
const DECIMATE: usize = 8;
/// The voicing window: 40 ms at 6 kHz.
const PITCH_WINDOW: usize = 240;
/// Pitch periods of 400 to 80 Hz at 6 kHz.
const LAG_MIN: usize = 15;
const LAG_MAX: usize = 75;
/// Minimum statistics: windows of 25 frames, 8 of them (2 s).
const FLOOR_WINDOW: usize = 25;
const FLOOR_WINDOWS: usize = 8;
/// Frames of the modulation measure (400 ms).
const MODULATION_FRAMES: usize = 40;
/// Below this the band power counts as this (dB).
const SILENCE_DB: f64 = -120.0;

fn smoothstep(x: f64, lo: f64, hi: f64) -> f64 {
    let t = ((x - lo) / (hi - lo)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn db(power: f64) -> f64 {
    if power > 0.0 {
        (10.0 * power.log10()).max(SILENCE_DB)
    } else {
        SILENCE_DB
    }
}

/// What a detector saw in one frame.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Frame {
    /// Speech probability of this frame, 0..1.
    pub probability: f64,
    /// The frame's RMS level, dBFS.
    pub level_dbfs: f64,
    /// The band power over the noise floor (dB), the modulation (dB), the
    /// voicing and the most negative autocorrelation (a tone near −1).
    pub features: [f64; 4],
}

/// Turns 10 ms frames into speech probabilities.
pub trait Detector: Send {
    fn frame(&mut self, x: &[f32]) -> Frame;
    fn reset(&mut self);
}

/// v1's detector: energy over the noise floor, voicing, modulation, tone
/// rejection (module documentation).
pub struct FeatureDetector {
    band: [Coefficients; 2],
    band_state: [State; 2],
    pitch: [Coefficients; 2],
    pitch_state: [State; 2],
    phase: usize,
    decimated: VecDeque<f32>,
    smoothed_db: Option<f64>,
    window_min: f64,
    window_frames: usize,
    minima: VecDeque<f64>,
    history: VecDeque<f64>,
}

impl Default for FeatureDetector {
    fn default() -> Self {
        let mut q = biquad::butterworth_q(4);
        let (q1, q2) = (q.next().unwrap_or(0.54), q.next().unwrap_or(1.31));
        Self {
            band: [
                biquad::high_pass(250.0, std::f64::consts::FRAC_1_SQRT_2),
                biquad::low_pass(4_000.0, std::f64::consts::FRAC_1_SQRT_2),
            ],
            band_state: Default::default(),
            pitch: [biquad::low_pass(1_000.0, q1), biquad::low_pass(1_000.0, q2)],
            pitch_state: Default::default(),
            phase: 0,
            decimated: VecDeque::with_capacity(PITCH_WINDOW + FRAME / DECIMATE),
            smoothed_db: None,
            window_min: f64::INFINITY,
            window_frames: 0,
            minima: VecDeque::with_capacity(FLOOR_WINDOWS),
            history: VecDeque::with_capacity(MODULATION_FRAMES),
        }
    }
}

impl FeatureDetector {
    /// The noise floor after this frame's band power `e` (dB).
    fn floor(&mut self, e: f64) -> f64 {
        let s = match self.smoothed_db {
            Some(s) => s + 0.3 * (e - s),
            None => e,
        };
        self.smoothed_db = Some(s);
        self.window_min = self.window_min.min(s);
        self.window_frames += 1;
        let floor = self.minima.iter().copied().fold(self.window_min, f64::min);
        if self.window_frames == FLOOR_WINDOW {
            if self.minima.len() == FLOOR_WINDOWS {
                self.minima.pop_front();
            }
            self.minima.push_back(self.window_min);
            self.window_min = f64::INFINITY;
            self.window_frames = 0;
        }
        floor
    }

    /// Standard deviation of the band power over the last 400 ms, with `e`.
    fn modulation(&mut self, e: f64) -> f64 {
        if self.history.len() == MODULATION_FRAMES {
            self.history.pop_front();
        }
        self.history.push_back(e);
        let n = self.history.len() as f64;
        let mean = self.history.iter().sum::<f64>() / n;
        (self.history.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n).sqrt()
    }

    /// The largest and smallest normalised autocorrelation for pitch lags; both
    /// 0 unless the window's halves are within 10 dB (an onset or an end in the
    /// window makes any sound look periodic).
    fn voicing(&self) -> (f64, f64) {
        if self.decimated.len() < PITCH_WINDOW {
            return (0.0, 0.0);
        }
        let skip = self.decimated.len() - PITCH_WINDOW;
        let x: Vec<f64> = self
            .decimated
            .iter()
            .skip(skip)
            .map(|v| f64::from(*v))
            .collect();
        let half = |h: &[f64]| h.iter().map(|v| v * v).sum::<f64>();
        let (first, second) = (half(&x[..PITCH_WINDOW / 2]), half(&x[PITCH_WINDOW / 2..]));
        if first.min(second) < 1e-12 || first.min(second) < 0.1 * first.max(second) {
            return (0.0, 0.0);
        }
        let (mut best, mut worst) = (0f64, 0f64);
        // Energies of x[..W−τ] and x[τ..], updated lag by lag.
        let mut head: f64 = x[..PITCH_WINDOW - LAG_MIN].iter().map(|v| v * v).sum();
        let mut tail: f64 = x[LAG_MIN..].iter().map(|v| v * v).sum();
        for lag in LAG_MIN..=LAG_MAX {
            if lag > LAG_MIN {
                head -= x[PITCH_WINDOW - lag] * x[PITCH_WINDOW - lag];
                tail -= x[lag - 1] * x[lag - 1];
            }
            let d = (head * tail).sqrt();
            if d < 1e-12 {
                continue;
            }
            let r: f64 = x[..PITCH_WINDOW - lag]
                .iter()
                .zip(&x[lag..])
                .map(|(a, b)| a * b)
                .sum::<f64>()
                / d;
            best = best.max(r);
            worst = worst.min(r);
        }
        (best, worst)
    }
}

impl Detector for FeatureDetector {
    fn frame(&mut self, x: &[f32]) -> Frame {
        let mut band = 0f64;
        let mut full = 0f64;
        for &s in x {
            let v = f64::from(s);
            full += v * v;
            let b = self.band_state[0].tick(&self.band[0], v);
            let b = self.band_state[1].tick(&self.band[1], b);
            band += b * b;
            let p = self.pitch_state[0].tick(&self.pitch[0], b);
            let p = self.pitch_state[1].tick(&self.pitch[1], p);
            self.phase += 1;
            if self.phase == DECIMATE {
                self.phase = 0;
                if self.decimated.len() == PITCH_WINDOW + FRAME / DECIMATE {
                    self.decimated.pop_front();
                }
                self.decimated.push_back(p as f32);
            }
        }
        let n = x.len().max(1) as f64;
        let level_dbfs = db(full / n);
        let e = db(band / n);
        let floor = self.floor(e);
        let modulation = self.modulation(e);
        let (voiced, anti) = self.voicing();
        let tone = anti < -0.93;
        let probability = if tone {
            0.0
        } else {
            smoothstep(e - floor, 4.0, 14.0)
                * smoothstep(modulation, 2.0, 6.0)
                * (0.3 + 0.7 * smoothstep(voiced, 0.4, 0.75))
        };
        Frame {
            probability,
            level_dbfs,
            features: [e - floor, modulation, voiced, anti],
        }
    }

    fn reset(&mut self) {
        *self = Self::default();
    }
}

/// A VAD's state as the API and the controllers see it.
#[derive(Debug, Clone, Copy, Default, PartialEq, serde::Serialize)]
pub struct VadState {
    pub speech_active: bool,
    pub speech_probability: f64,
    /// The power mean of the frames with speech (1 s time constant), dBFS.
    pub speech_level_dbfs: Option<f64>,
    /// The last frame's RMS level, dBFS.
    pub level_dbfs: f64,
    /// The momentary loudness (BS.1770 over the signal's channels, 400 ms),
    /// LUFS, and whether it is below the gate (nothing is speech then).
    pub momentary_lufs: f64,
    pub gated: bool,
    /// The short-term loudness (3 s), LUFS.
    pub short_term_lufs: f64,
    /// Onsets and offsets so far.
    pub speech_onsets: u64,
    pub speech_offsets: u64,
    /// The last frame's features (the feature detector's, for tuning).
    pub features: Features,
    /// The detector running: `silero` or `features`.
    pub detector: &'static str,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, serde::Serialize)]
pub struct Features {
    pub over_floor_db: f64,
    pub modulation_db: f64,
    pub voicing: f64,
    pub anti: f64,
}

impl From<[f64; 4]> for Features {
    fn from(f: [f64; 4]) -> Self {
        let r = |v: f64| (v * 100.0).round() / 100.0;
        Self {
            over_floor_db: r(f[0]),
            modulation_db: r(f[1]),
            voicing: r(f[2]),
            anti: r(f[3]),
        }
    }
}

/// One signal's VAD: a detector with the smoothing, threshold, hold and outputs.
pub struct Vad {
    detector: Box<dyn Detector>,
    /// The detector the settings ask for (the one running, unless it fell back).
    pub wanted: crate::mix::auto::VadDetector,
    pub state: VadState,
    /// Frames left in the hold.
    hold: u32,
    speech_power: Option<f64>,
    buffer: Vec<f32>,
    /// The loudness of a mono signal given to [`Vad::push`].
    level: super::level::Level,
}

impl Default for Vad {
    fn default() -> Self {
        Self::new(Box::<FeatureDetector>::default())
    }
}

impl Vad {
    /// A VAD with the detector `kind`; Silero falls back to the feature detector
    /// when ONNX Runtime cannot run the model.
    pub fn with(kind: crate::mix::auto::VadDetector) -> Self {
        use crate::mix::auto::VadDetector;
        let mut v = match kind {
            VadDetector::Silero => match super::silero::SileroDetector::new() {
                Ok(d) => {
                    let mut v = Self::new(Box::new(d));
                    v.state.detector = "silero";
                    v
                }
                Err(e) => {
                    static WARNED: std::sync::Once = std::sync::Once::new();
                    WARNED.call_once(|| {
                        tracing::warn!(
                            "Silero VAD unavailable ({e}); the feature detector runs instead"
                        )
                    });
                    Self::default()
                }
            },
            VadDetector::Features => Self::default(),
        };
        v.wanted = kind;
        v
    }

    pub fn new(detector: Box<dyn Detector>) -> Self {
        Self {
            detector,
            wanted: crate::mix::auto::VadDetector::Features,
            state: VadState {
                level_dbfs: SILENCE_DB,
                detector: "features",
                ..VadState::default()
            },
            hold: 0,
            speech_power: None,
            buffer: Vec::with_capacity(FRAME),
            level: super::level::Level::new(vec![1.0]),
        }
    }

    pub fn reset(&mut self) {
        self.detector.reset();
        self.hold = 0;
        self.buffer.clear();
        self.speech_power = None;
        self.level = super::level::Level::new(vec![1.0]);
        self.state = VadState {
            level_dbfs: SILENCE_DB,
            detector: self.state.detector,
            speech_onsets: self.state.speech_onsets,
            speech_offsets: self.state.speech_offsets + u64::from(self.state.speech_active),
            ..VadState::default()
        };
    }

    /// Analyse one complete frame (the mean of the signal's channels) with
    /// `settings`; `momentary` is the signal's momentary loudness, LUFS.
    pub fn frame(&mut self, x: &[f32], momentary: f64, settings: &crate::mix::auto::VadSettings) {
        let f = self.detector.frame(x);
        let gated = momentary < settings.gate_lufs;
        let probability = if gated { 0.0 } else { f.probability };
        let s = &mut self.state;
        s.momentary_lufs = (momentary * 10.0).round() / 10.0;
        s.gated = gated;
        let p = s.speech_probability;
        let a = if probability > p { 0.5 } else { 0.2 };
        s.speech_probability = p + a * (probability - p);
        s.level_dbfs = f.level_dbfs;
        s.features = f.features.into();
        let on = settings.threshold;
        let off = (on - 0.15).max(0.05);
        let hold_frames = (settings.hold_ms / 10.0).round() as u32;
        if s.speech_probability >= on {
            if !s.speech_active {
                s.speech_active = true;
                s.speech_onsets += 1;
            }
            self.hold = hold_frames;
        } else if s.speech_active && s.speech_probability < off {
            if self.hold == 0 {
                s.speech_active = false;
                s.speech_offsets += 1;
            } else {
                self.hold -= 1;
            }
        }
        if s.speech_active {
            let power = 10f64.powf(f.level_dbfs / 10.0);
            let mean = match self.speech_power {
                Some(m) => m + 0.01 * (power - m),
                None => power,
            };
            self.speech_power = Some(mean);
            s.speech_level_dbfs = Some((db(mean) * 10.0).round() / 10.0);
        }
    }

    /// Take `samples` of a mono signal (any length); analyse every frame they
    /// complete and call `each` after it.
    pub fn push(
        &mut self,
        samples: &[f32],
        settings: &crate::mix::auto::VadSettings,
        mut each: impl FnMut(&VadState),
    ) {
        let mut rest = samples;
        while !rest.is_empty() {
            let take = (FRAME - self.buffer.len()).min(rest.len());
            self.buffer.extend_from_slice(&rest[..take]);
            rest = &rest[take..];
            if self.buffer.len() == FRAME {
                let frame = std::mem::take(&mut self.buffer);
                let r = self.level.frame(&[&frame]);
                self.frame(&frame, r.momentary, settings);
                self.state.short_term_lufs = (r.short_term * 10.0).round() / 10.0;
                self.buffer = frame;
                self.buffer.clear();
                each(&self.state);
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::mix::auto::VadSettings;
    use std::f64::consts::PI;

    /// A deterministic noise generator (xorshift), uniform in −1..1.
    pub struct Noise(u32);

    impl Noise {
        pub fn new(seed: u32) -> Self {
            Self(seed.max(1))
        }
        pub fn next(&mut self) -> f64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            self.0 = x;
            f64::from(x) / f64::from(u32::MAX) * 2.0 - 1.0
        }
    }

    /// Pink noise (Paul Kellet's economy filter) at about `rms`.
    pub fn pink(seconds: f64, rms: f64, seed: u32) -> Vec<f32> {
        let mut n = Noise::new(seed);
        let (mut b0, mut b1, mut b2) = (0.0, 0.0, 0.0);
        let len = (seconds * 48_000.0) as usize;
        let raw: Vec<f64> = (0..len)
            .map(|_| {
                let w = n.next();
                b0 = 0.99765 * b0 + w * 0.0990460;
                b1 = 0.96300 * b1 + w * 0.2965164;
                b2 = 0.57000 * b2 + w * 1.0526913;
                b0 + b1 + b2 + w * 0.1848
            })
            .collect();
        let r = (raw.iter().map(|v| v * v).sum::<f64>() / len as f64).sqrt();
        raw.iter().map(|v| (v / r * rms) as f32).collect()
    }

    pub fn sine(seconds: f64, freq: f64, peak: f64) -> Vec<f32> {
        (0..(seconds * 48_000.0) as usize)
            .map(|i| (peak * (2.0 * PI * freq * i as f64 / 48_000.0).sin()) as f32)
            .collect()
    }

    /// Speech-like test signal: syllables of a voiced source (a glottal pulse
    /// train with a slowly moving pitch) through three formant resonances,
    /// 120–260 ms long with 40–120 ms gaps, some starting with a fricative
    /// (high-passed noise), in words of 3–5 syllables with 250 ms pauses.
    /// Returns the samples (vowels at `rms`, fricatives 12 dB lower) and the
    /// (start, end) sample of each word.
    pub fn speech(seconds: f64, rms: f64, seed: u32) -> (Vec<f32>, Vec<(usize, usize)>) {
        let len = (seconds * 48_000.0) as usize;
        let mut out = vec![0f64; len];
        let mut fric = vec![0f64; len];
        let mut rng = Noise::new(seed);
        let mut words = Vec::new();
        let vowels = [
            [(700.0, 1.0), (1_200.0, 0.5), (2_600.0, 0.2)],
            [(300.0, 1.0), (2_300.0, 0.4), (3_000.0, 0.2)],
            [(500.0, 1.0), (900.0, 0.6), (2_500.0, 0.2)],
            [(400.0, 1.0), (2_000.0, 0.5), (2_700.0, 0.2)],
        ];
        let mut t = 0usize;
        let mut phase = 0f64;
        while t + 48_000 / 2 < len {
            let start = t;
            let syllables = 3 + (rng.next().abs() * 3.0) as usize;
            for k in 0..syllables {
                let dur = (0.12 + 0.14 * rng.next().abs()) * 48_000.0;
                let dur = dur as usize;
                let v = vowels[(rng.next().abs() * 4.0) as usize % 4];
                let mut filters: Vec<(Coefficients, State, f64)> = v
                    .iter()
                    .map(|(f, g)| (biquad::band_pass(*f, 8.0), State::default(), *g))
                    .collect();
                let f0 = 110.0 + 40.0 * rng.next();
                if k > 0 && rng.next() > 0.3 {
                    let hp = biquad::high_pass(4_000.0, 0.7);
                    let mut st = State::default();
                    for i in 0..(0.06 * 48_000.0) as usize {
                        if t + i < len {
                            fric[t + i] = st.tick(&hp, rng.next());
                        }
                    }
                    t += (0.06 * 48_000.0) as usize;
                }
                for i in 0..dur {
                    if t + i >= len {
                        break;
                    }
                    let f = f0 * (1.0 + 0.1 * (2.0 * PI * 3.0 * i as f64 / 48_000.0).sin());
                    phase += f / 48_000.0;
                    let pulse = if phase >= 1.0 {
                        phase -= 1.0;
                        1.0
                    } else {
                        0.0
                    };
                    let env = (i.min(dur - i) as f64 / (0.02 * 48_000.0)).min(1.0);
                    let s: f64 = filters
                        .iter_mut()
                        .map(|(c, st, g)| *g * st.tick(c, pulse))
                        .sum();
                    out[t + i] += env * s;
                }
                t += dur + ((0.04 + 0.08 * rng.next().abs()) * 48_000.0) as usize;
            }
            words.push((start, t.min(len)));
            t += (0.25 * 48_000.0) as usize;
        }
        let rms_of = |x: &[f64]| {
            let v: Vec<f64> = x.iter().copied().filter(|v| v.abs() > 1e-9).collect();
            (v.iter().map(|v| v * v).sum::<f64>() / v.len().max(1) as f64).sqrt()
        };
        let (rv, rf) = (rms_of(&out), rms_of(&fric));
        let fric_rms = rms * 10f64.powf(-12.0 / 20.0);
        let x = out
            .iter()
            .zip(&fric)
            .map(|(v, f)| (v / rv * rms + if rf > 0.0 { f / rf * fric_rms } else { 0.0 }) as f32)
            .collect();
        (x, words)
    }

    /// The VAD's (active, probability) after each frame of `x`.
    pub fn run(x: &[f32], settings: &VadSettings) -> Vec<(bool, f64)> {
        let mut vad = Vad::with(settings.detector);
        let mut out = Vec::new();
        vad.push(x, settings, |s| {
            out.push((s.speech_active, s.speech_probability))
        });
        out
    }

    /// The feature detector's settings (the tests below are about it).
    fn settings() -> VadSettings {
        VadSettings {
            detector: crate::mix::auto::VadDetector::Features,
            ..VadSettings::new("x")
        }
    }

    /// The detector `VAD_DETECTOR` names (`silero` or `features`, default both).
    fn evaluated() -> Vec<VadSettings> {
        use crate::mix::auto::VadDetector;
        let all = [VadDetector::Features, VadDetector::Silero];
        all.into_iter()
            .filter(|d| {
                std::env::var("VAD_DETECTOR").map_or(true, |v| {
                    serde_json::to_value(d).is_ok_and(|j| j == v.as_str())
                })
            })
            .map(|detector| VadSettings {
                detector,
                ..VadSettings::new("x")
            })
            .collect()
    }

    fn db_gain(db: f64) -> f64 {
        10f64.powf(db / 20.0)
    }

    /// Spaced words of [`speech`]: 1 s of silence before each.
    fn spaced(seconds: f64, seed: u32) -> (Vec<f32>, Vec<(usize, usize)>) {
        let (x, words) = speech(seconds, db_gain(-24.0), seed);
        let mut out = Vec::new();
        let mut spans = Vec::new();
        for (a, b) in &words {
            out.extend(vec![0f32; 48_000]);
            let at = out.len();
            out.extend_from_slice(&x[*a..*b]);
            spans.push((at, out.len()));
        }
        (out, spans)
    }

    /// Evaluation, not a check: the features' 10/50/90 % quantiles on the test
    /// signals; per detector (`VAD_DETECTOR=silero|features`, default both) its
    /// cost and detection latency on synthetic words, and with
    /// `VAD_WAV=<file.wav>` that recording's speech, one character per 10 ms
    /// (`#` speech):
    /// `VAD_WAV=talk.wav cargo test --release --lib evaluate -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn evaluate() {
        let show = |what: &str, x: &[f32]| {
            let mut d = FeatureDetector::default();
            let frames: Vec<Frame> = x.chunks_exact(FRAME).map(|c| d.frame(c)).collect();
            let q = |i: usize| {
                let mut v: Vec<f64> = frames.iter().skip(50).map(|f| f.features[i]).collect();
                v.sort_by(|a, b| a.total_cmp(b));
                let p = |q: f64| v[((v.len() - 1) as f64 * q) as usize];
                format!("{:6.2} {:6.2} {:6.2}", p(0.1), p(0.5), p(0.9))
            };
            eprintln!(
                "{what:14} over floor {} | modulation {} | voicing {} | anti {}",
                q(0),
                q(1),
                q(2),
                q(3)
            );
        };
        show("pink", &pink(4.0, db_gain(-30.0), 7));
        show("sine 1 kHz", &sine(4.0, 1_000.0, 0.18));
        let (x, _) = speech(8.0, db_gain(-24.0), 3);
        show("speech", &x);
        let noise = pink(8.1, db_gain(-39.0), 11);
        let noisy: Vec<f32> = x.iter().zip(&noise).map(|(a, b)| a + b).collect();
        show("speech+noise", &noisy);
        for s in evaluated() {
            eprintln!("detector {:?}", s.detector);
            let (x, _) = speech(10.0, db_gain(-24.0), 4);
            let started = std::time::Instant::now();
            run(&x, &s);
            eprintln!(
                "  cost: {:.2} % of a core per analysed signal",
                started.elapsed().as_secs_f64() / 10.0 * 100.0
            );
            let mut all = Vec::new();
            for seed in 1..=10 {
                let (x, spans) = spaced(12.0, seed);
                all.extend(score(&x, &spans, &s).0);
            }
            all.sort_unstable();
            if !all.is_empty() {
                eprintln!(
                    "  latency of {} synthetic words (10 ms frames): median {}, 90 % {}, max {}",
                    all.len(),
                    all[all.len() / 2],
                    all[all.len() * 9 / 10],
                    all[all.len() - 1]
                );
            }
            if let Ok(path) = std::env::var("VAD_WAV") {
                let x = crate::wav::Wav::read(std::path::Path::new(&path))
                    .unwrap()
                    .mono_48k();
                let r = run(&x, &s);
                let line: String = r.iter().map(|f| if f.0 { '#' } else { '.' }).collect();
                for (k, c) in line.as_bytes().chunks(100).enumerate() {
                    eprintln!(
                        "{:5.1} s {}",
                        k as f64,
                        std::str::from_utf8(c).unwrap_or("")
                    );
                }
            }
        }
    }

    #[test]
    fn silence_noise_and_tones_are_not_speech() {
        let s = settings();
        assert!(run(&vec![0.0; 48_000], &s).iter().all(|f| !f.0));
        for (what, x) in [
            ("pink −30 dBFS", pink(4.0, db_gain(-30.0), 7)),
            (
                "1 kHz −18 dBFS",
                sine(4.0, 1_000.0, db_gain(-18.0) * 2f64.sqrt()),
            ),
            (
                "200 Hz −18 dBFS",
                sine(4.0, 200.0, db_gain(-18.0) * 2f64.sqrt()),
            ),
            (
                "3 kHz −18 dBFS",
                sine(4.0, 3_000.0, db_gain(-18.0) * 2f64.sqrt()),
            ),
        ] {
            let r = run(&x, &s);
            let active = r.iter().filter(|f| f.0).count();
            let max = r.iter().map(|f| f.1).fold(0.0, f64::max);
            assert_eq!(active, 0, "{what}: {active} frames speech, max p {max:.2}");
        }
        // A tone that starts and stops (an ident, beeps).
        let mut beeps = Vec::new();
        for _ in 0..8 {
            beeps.extend(sine(0.25, 1_000.0, 0.2));
            beeps.extend(vec![0.0; 12_000]);
        }
        assert!(run(&beeps, &s).iter().all(|f| !f.0), "beeps");
        // A logarithmic sweep 20 Hz–20 kHz in 4 s.
        let mut phase = 0f64;
        let sweep: Vec<f32> = (0..4 * 48_000)
            .map(|i| {
                let f = 20.0 * 1000f64.powf(i as f64 / (4.0 * 48_000.0));
                phase += f / 48_000.0;
                (0.2 * (2.0 * PI * phase).sin()) as f32
            })
            .collect();
        let r = run(&sweep, &s);
        assert!(
            r.iter().all(|f| !f.0),
            "sweep: {} frames",
            r.iter().filter(|f| f.0).count()
        );
    }

    /// Frames from each word's start to the detection, and the share of word
    /// frames with speech and of pause frames (beyond the hold) without.
    fn score(x: &[f32], words: &[(usize, usize)], s: &VadSettings) -> (Vec<usize>, f64, f64) {
        let r = run(x, s);
        let mut latencies = Vec::new();
        let (mut inside, mut inside_on) = (0, 0);
        let (mut outside, mut outside_off) = (0, 0);
        let hold = (s.hold_ms / 10.0) as usize + 30;
        for (start, end) in words {
            let (a, b) = (start / FRAME, end / FRAME);
            if let Some(k) = (a..(b.min(r.len()))).find(|&k| r[k].0) {
                latencies.push(k - a);
            }
            for f in &r[(a + 10).min(r.len())..b.min(r.len())] {
                inside += 1;
                inside_on += usize::from(f.0);
            }
        }
        for (k, f) in r.iter().enumerate() {
            let near = words
                .iter()
                .any(|(a, b)| k + 1 >= a / FRAME && k <= b / FRAME + hold);
            if !near {
                outside += 1;
                outside_off += usize::from(!f.0);
            }
        }
        (
            latencies,
            inside_on as f64 / inside.max(1) as f64,
            outside_off as f64 / outside.max(1) as f64,
        )
    }

    #[test]
    fn speech_is_detected_with_its_latency() {
        let s = settings();
        // Words with 1 s pauses, so that pauses longer than the hold exist.
        let (x, spans) = spaced(12.0, 3);
        let (lat, on, off) = score(&x, &spans, &s);
        assert_eq!(lat.len(), spans.len(), "every word detected");
        let worst = lat.iter().max().copied().unwrap_or(0);
        assert!(worst <= 8, "detection latency {lat:?} frames of 10 ms");
        assert!(on > 0.95, "speech frames detected: {on:.3}");
        assert!(off > 0.99, "pause frames free: {off:.3}");
    }

    #[test]
    fn speech_in_noise() {
        let s = settings();
        let (mut x, spans) = spaced(12.0, 5);
        // Pink noise 15 dB under the speech, from the start.
        let noise = pink(x.len() as f64 / 48_000.0 + 0.1, db_gain(-39.0), 11);
        for (v, n) in x.iter_mut().zip(&noise) {
            *v += n;
        }
        let (lat, on, off) = score(&x, &spans, &s);
        assert_eq!(lat.len(), spans.len(), "every word detected");
        assert!(lat.iter().max().copied().unwrap_or(0) <= 8, "{lat:?}");
        assert!(on > 0.9, "speech frames detected: {on:.3}");
        assert!(off > 0.97, "noise frames free: {off:.3}");
    }

    #[test]
    fn hold_threshold_and_level() {
        let mut s = settings();
        let (x, words) = speech(3.0, db_gain(-24.0), 9);
        let (a, b) = words[0];
        let mut sig = vec![0f32; 48_000];
        sig.extend_from_slice(&x[a..b]);
        sig.extend(vec![0f32; 48_000]);
        let end = (48_000 + b - a) / FRAME;
        let mut vad = Vad::default();
        let mut states = Vec::new();
        vad.push(&sig, &s, |st| states.push(*st));
        let last = states.iter().rposition(|st| st.speech_active).unwrap();
        // Off after the hold (80 frames) and the fall of the probability.
        assert!(
            last > end + 75 && last < end + 95,
            "last speech frame {last}, end {end}"
        );
        let st = states.last().unwrap();
        assert_eq!((st.speech_onsets, st.speech_offsets), (1, 1));
        let level = st.speech_level_dbfs.unwrap();
        assert!((level + 26.0).abs() < 3.0, "speech level {level}");
        assert!(
            (states[end - 5].momentary_lufs + 25.0).abs() < 4.0,
            "{:?}",
            states[end - 5]
        );
        // Below the gate nothing is speech (crosstalk is quieter than the talker).
        s.gate_lufs = -10.0;
        assert!(run(&sig, &s).iter().all(|f| !f.0));
    }
}
