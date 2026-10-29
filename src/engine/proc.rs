//! Processing nodes as the DSP thread runs them (§6). A [`Processor`] is built on
//! the control side with all of its storage; the DSP thread only runs it. Numeric
//! parameters ramp per block (coefficients are recomputed while they move); the
//! wet share (bypass) crossfades over the block, with the dry path delayed by the
//! node's latency.

use std::sync::atomic::{AtomicU32, Ordering};

use crate::dsp::biquad::{self, Coefficients, State};
use crate::dsp::dynamics::{
    compressor_gain, expander_gain, from_db, power_to_db, to_db, Ballistics, LookaheadGain,
};
use crate::dsp::truepeak::{self, TruePeak, PHASES, TAPS};
use crate::mix::chain::{BandType, DeEsserMode, Detector, FilterMode, ProcKind, MAX_BANDS};
use crate::mix::format::MAX_FORMAT_CHANNELS;

use super::plan::NodeId;

/// Channels a processor handles at most.
pub const MAX_CH: usize = MAX_FORMAT_CHANNELS;

type Taps = [[f32; TAPS]; PHASES];

/// Per-node meters, written by the DSP thread only.
pub struct NodeMeter {
    /// Largest gain reduction since the control side last took it (dB ≥ 0, f32 bits).
    pub reduction: AtomicU32,
    pub in_peak: AtomicU32,
    pub out_peak: AtomicU32,
    /// Per band of a multiband or dynamic EQ: the largest gain reduction since
    /// the control side last took it (dB ≥ 0, f32 bits); unused slots stay 0.
    pub bands: [AtomicU32; MAX_BANDS],
    /// A leveler's gain after its last block (dB, signed, f32 bits); NaN for
    /// other nodes.
    pub level: AtomicU32,
}

impl Default for NodeMeter {
    fn default() -> Self {
        Self {
            reduction: AtomicU32::new(0),
            in_peak: AtomicU32::new(0),
            out_peak: AtomicU32::new(0),
            bands: Default::default(),
            level: AtomicU32::new(f32::NAN.to_bits()),
        }
    }
}

fn max_into(a: &AtomicU32, v: f32) {
    a.fetch_max(v.max(0.0).to_bits(), Ordering::Relaxed);
}

/// A run of delay lines, one per channel.
#[derive(Debug, Clone)]
pub struct Delay {
    lines: Vec<Vec<f32>>,
    at: usize,
}

impl Delay {
    pub fn new(channels: usize, samples: usize) -> Self {
        Self {
            lines: vec![vec![0.0; samples]; channels],
            at: 0,
        }
    }

    fn len(&self) -> usize {
        self.lines.first().map_or(0, |l| l.len())
    }

    /// Delay every channel of `io` in place.
    pub fn run(&mut self, io: &mut [&mut [f32]]) {
        let n = self.len();
        if n == 0 {
            return;
        }
        let block = io.first().map_or(0, |c| c.len());
        for (line, ch) in self.lines.iter_mut().zip(io.iter_mut()) {
            let mut at = self.at;
            for s in ch.iter_mut() {
                std::mem::swap(&mut line[at], s);
                at += 1;
                if at == n {
                    at = 0;
                }
            }
        }
        self.at = (self.at + block) % n;
    }
}

struct Smoothing {
    cur: Vec<f32>,
    target: Vec<f32>,
    step: Vec<f32>,
    left: Vec<u32>,
}

impl Smoothing {
    fn new(values: &[f32]) -> Self {
        Self {
            cur: values.to_vec(),
            target: values.to_vec(),
            step: vec![0.0; values.len()],
            left: vec![0; values.len()],
        }
    }

    fn set(&mut self, i: usize, value: f32, ramp: u32) {
        if i >= self.cur.len() {
            return;
        }
        self.target[i] = value;
        if ramp == 0 {
            self.cur[i] = value;
            self.left[i] = 0;
        } else {
            self.step[i] = (value - self.cur[i]) / ramp as f32;
            self.left[i] = ramp;
        }
    }

    /// Advance by a block; `true` when a value moved.
    fn advance(&mut self, block: u32) -> bool {
        let mut moved = false;
        for i in 0..self.cur.len() {
            if self.left[i] == 0 {
                continue;
            }
            moved = true;
            let n = block.min(self.left[i]);
            self.left[i] -= n;
            self.cur[i] = if self.left[i] == 0 {
                self.target[i]
            } else {
                self.cur[i] + self.step[i] * n as f32
            };
        }
        moved
    }
}

/// EBU R 49 ident: the left channel is off for 0.25 s every 3 s, with 2 ms ramps.
const IDENT_PERIOD: u64 = 3 * 48_000;
const IDENT_GAP: u64 = 12_000;
const IDENT_RAMP: u64 = 96;

fn ident_gain(played: u64) -> f32 {
    let t = played % IDENT_PERIOD;
    if t < IDENT_GAP {
        0.0
    } else if t < IDENT_GAP + IDENT_RAMP {
        (t - IDENT_GAP) as f32 / IDENT_RAMP as f32
    } else if t >= IDENT_PERIOD - IDENT_RAMP {
        (IDENT_PERIOD - t) as f32 / IDENT_RAMP as f32
    } else {
        1.0
    }
}

/// Uniform white noise with an RMS of 1 (xorshift32).
fn white(state: &mut u32) -> f32 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *state = x;
    // Uniform in [−1, 1): RMS 1/√3.
    let u = (x as f32 / u32::MAX as f32) * 2.0 - 1.0;
    u * 1.732_050_8
}

/// Paul Kellet's refined pink filter on white noise of RMS 1, scaled back to an
/// RMS of 1 (the filter's gain on unit white noise, measured over 10 s: 3.043).
fn pink_filter(b: &mut [f32; 7], w: f32) -> f32 {
    b[0] = 0.99886 * b[0] + w * 0.055_517_9;
    b[1] = 0.99332 * b[1] + w * 0.075_075_9;
    b[2] = 0.96900 * b[2] + w * 0.153_852;
    b[3] = 0.86650 * b[3] + w * 0.310_485_6;
    b[4] = 0.55000 * b[4] + w * 0.532_952_2;
    b[5] = -0.7616 * b[5] - w * 0.016_898;
    let out = b[0] + b[1] + b[2] + b[3] + b[4] + b[5] + b[6] + w * 0.5362;
    b[6] = w * 0.115_926;
    out / PINK_GAIN
}

const PINK_GAIN: f32 = 3.043;

/// The latency test signal: the tone during the first 20 ms of every second of
/// the TAI sample index, starting at phase 0 (at 1 kHz exactly 20 cycles).
const PULSE: u64 = 960;

fn pulse_gain(index: u64) -> f32 {
    if index % u64::from(crate::config::SAMPLE_RATE) < PULSE {
        1.0
    } else {
        0.0
    }
}

/// The type-specific part: coefficients and state.
pub enum Kind {
    /// Per band its type and order (2 for bells, shelves and notches; a pass
    /// band's Butterworth order); `order / 2` sections each, in band order.
    Eq {
        types: Vec<(BandType, usize)>,
        coefficients: Vec<Coefficients>,
        states: Vec<State>,
    },
    Filter {
        mode: FilterMode,
        qs: Vec<f64>,
        coefficients: Vec<Coefficients>,
        states: Vec<State>,
    },
    DynamicEq {
        bands: usize,
        detect: Vec<Coefficients>,
        detect_states: Vec<State>,
        bells: Vec<Coefficients>,
        bell_states: Vec<State>,
        envelopes: Vec<f64>,
        ballistics: Vec<Ballistics>,
    },
    Compressor {
        detector: Detector,
        sidechain: Option<(Coefficients, Vec<State>)>,
        ballistics: Ballistics,
        envelope: f64,
        mean_square: f64,
        /// One block of levels, gains in dB and gain factors, in turn.
        scratch: Vec<f64>,
    },
    Limiter {
        delay: Delay,
        gain: LookaheadGain,
        true_peak: Option<(Box<Taps>, Vec<TruePeak>)>,
    },
    Expander {
        ballistics: Ballistics,
        envelope: f64,
    },
    /// Linkwitz-Riley 24 dB/oct: two Butterworth sections each way.
    BassManagement {
        lfe: usize,
        low: Coefficients,
        high: Coefficients,
        /// Two high-pass sections per channel (unused on the LFE).
        high_states: Vec<State>,
        low_states: [State; 2],
        sum: Vec<f32>,
    },
    /// Phase (cycles, 0–1) and the samples played (for the ident's period and
    /// the sweep); for noise, a generator and the pink filter per channel.
    Oscillator {
        waveform: crate::mix::chain::Waveform,
        phase: f64,
        played: u64,
        noise: Vec<u32>,
        pink: Vec<[f32; 7]>,
    },
    Gate {
        attack: f64,
        release: f64,
        envelope: f64,
        open: bool,
        held: u32,
    },
    DeEsser {
        mode: DeEsserMode,
        listen: bool,
        low: Vec<Coefficients>,
        high: Vec<Coefficients>,
        low_states: Vec<State>,
        high_states: Vec<State>,
        band: Vec<f32>,
        ballistics: Ballistics,
        envelope: f64,
        scratch: Vec<f64>,
    },
    Ducker {
        attack: f64,
        release: f64,
        envelope: f64,
        held: u32,
    },
    Multiband {
        bands: usize,
        low: Vec<Coefficients>,
        high: Vec<Coefficients>,
        allpass: Vec<Coefficients>,
        /// Per crossover and channel: two low-pass and two high-pass sections.
        split_states: Vec<[State; 4]>,
        /// Per band, later crossover and channel.
        allpass_states: Vec<State>,
        scratch: Vec<f32>,
        ballistics: Vec<Ballistics>,
        envelopes: Vec<f64>,
    },
    /// The leveler (B3): K-weighted energy in 100 ms segments, the last 3 s of
    /// them, the gain it heads for and the gain it has, and the safety limiter.
    Leveler {
        k_states: Vec<[State; 2]>,
        weights: Vec<f64>,
        energy: f64,
        samples: usize,
        segments: Vec<f64>,
        at: usize,
        filled: usize,
        desired: f64,
        gain: f64,
        limit: Option<(Delay, LookaheadGain)>,
    },
    Delay(Delay),
    /// One block of a signal, kept for the next block (a feedback send's block delay).
    Hold(Vec<f32>),
}

/// A processing node in a plan.
pub struct Processor {
    pub id: NodeId,
    channels: usize,
    params: Smoothing,
    dirty: bool,
    pub latency: usize,
    dry: Vec<f32>,
    dry_delay: Delay,
    wet: f32,
    pub meter: usize,
    pub kind: Kind,
    /// The absolute sample index (TAI) of the block being processed.
    pub start: u64,
    /// The gain reduction of each band in the last block (multiband, dynamic EQ).
    band_reduction: [f32; MAX_BANDS],
}

fn allpass(freq: f64) -> Coefficients {
    let w = 2.0 * std::f64::consts::PI * freq / 48_000.0;
    let alpha = w.sin() / (2.0 * std::f64::consts::FRAC_1_SQRT_2);
    let c = w.cos();
    let a0 = 1.0 + alpha;
    Coefficients {
        b0: (1.0 - alpha) / a0,
        b1: -2.0 * c / a0,
        b2: 1.0,
        a1: -2.0 * c / a0,
        a2: (1.0 - alpha) / a0,
    }
}

const Q_BUTTERWORTH: f64 = std::f64::consts::FRAC_1_SQRT_2;

/// The leveler's loudness segment (100 ms) and window (3 s: short-term loudness).
const LEVELER_SEGMENT: usize = 4800;
const LEVELER_SEGMENTS: usize = 30;
/// Segments the leveler waits for after a start before it moves (400 ms).
const LEVELER_FIRST: usize = 4;
/// Release of the leveler's safety limiter, ms.
const LEVELER_RELEASE_MS: f64 = 50.0;

/// BS.1770 channel weights of a signal with `channels` channels (its format
/// follows from the count): LFE 0, the surrounds 1.41, the rest 1.
fn loudness_weights(channels: usize) -> Vec<f64> {
    use crate::mix::format::Format;
    let format = [
        Format::Mono,
        Format::Stereo,
        Format::Lcr,
        Format::Surround51,
        Format::Surround71,
        Format::Surround514,
        Format::Surround714,
    ]
    .into_iter()
    .find(|f| f.channels() == channels);
    match format {
        Some(f) => crate::dsp::loudness::weights(f.labels()),
        None => vec![1.0; channels],
    }
}

/// Run a look-ahead peak limiter (`delay` holds the look-ahead) at linear
/// `ceiling` over a block; returns the largest gain reduction in dB.
fn lookahead_limit(
    io: &mut [&mut [f32]],
    delay: &mut Delay,
    gain: &mut LookaheadGain,
    mut true_peak: Option<&mut (Box<Taps>, Vec<TruePeak>)>,
    ceiling: f64,
    block: usize,
) -> f64 {
    let lines = &mut delay.lines;
    let len = lines.first().map_or(0, |l| l.len()).max(1);
    let mut at = delay.at;
    let mut least = 1f64;
    for k in 0..block {
        let mut peak = 0f64;
        for (c, ch) in io.iter().enumerate() {
            let x = ch[k];
            let v = match true_peak.as_deref_mut() {
                Some((taps, tp)) => tp[c].step(taps, x),
                None => x.abs(),
            };
            peak = peak.max(f64::from(v));
        }
        let g = gain.next(peak, ceiling);
        least = least.min(g);
        for (c, ch) in io.iter_mut().enumerate() {
            let out = lines[c][at];
            lines[c][at] = ch[k];
            ch[k] = (f64::from(out) * g) as f32;
        }
        at = (at + 1) % len;
    }
    delay.at = at;
    -to_db(least)
}

impl Processor {
    /// Build a node of `kind` for `channels` channels and blocks of `block`.
    pub fn new(
        id: NodeId,
        kind: &ProcKind,
        values: &[f32],
        channels: usize,
        block: usize,
        meter: usize,
    ) -> Self {
        let channels = channels.min(MAX_CH);
        let latency = kind.latency();
        let k = match kind {
            ProcKind::Eq { bands } => {
                let sections: usize = bands.iter().map(|b| b.sections()).sum();
                Kind::Eq {
                    types: bands.iter().map(|b| (b.kind, 2 * b.sections())).collect(),
                    coefficients: vec![Coefficients::IDENTITY; sections],
                    states: vec![State::default(); sections * channels],
                }
            }
            ProcKind::Filter {
                mode,
                slope_db_per_oct,
                ..
            } => {
                let qs: Vec<f64> = biquad::butterworth_q(*slope_db_per_oct as usize / 6).collect();
                Kind::Filter {
                    mode: *mode,
                    coefficients: vec![Coefficients::IDENTITY; qs.len()],
                    states: vec![State::default(); qs.len() * channels],
                    qs,
                }
            }
            ProcKind::DynamicEq { bands } => Kind::DynamicEq {
                bands: bands.len(),
                detect: vec![Coefficients::IDENTITY; bands.len()],
                detect_states: vec![State::default(); bands.len() * channels],
                bells: vec![Coefficients::IDENTITY; bands.len()],
                bell_states: vec![State::default(); bands.len() * channels],
                envelopes: vec![0.0; bands.len()],
                ballistics: vec![Ballistics::new(1.0, 1.0); bands.len()],
            },
            ProcKind::Compressor {
                detector,
                sidechain_hpf_hz,
                ..
            } => Kind::Compressor {
                detector: *detector,
                sidechain: sidechain_hpf_hz
                    .map(|_| (Coefficients::IDENTITY, vec![State::default(); MAX_CH])),
                ballistics: Ballistics::new(1.0, 1.0),
                envelope: 0.0,
                mean_square: 0.0,
                scratch: vec![0.0; block],
            },
            ProcKind::Limiter { true_peak, .. } => Kind::Limiter {
                delay: Delay::new(channels, latency),
                gain: LookaheadGain::new(latency),
                true_peak: true_peak.then(|| {
                    (
                        Box::new(truepeak::taps()),
                        vec![TruePeak::default(); channels],
                    )
                }),
            },
            ProcKind::Expander { .. } => Kind::Expander {
                ballistics: Ballistics::new(1.0, 1.0),
                envelope: 0.0,
            },
            ProcKind::BassManagement { lfe, .. } => Kind::BassManagement {
                lfe: *lfe,
                low: Coefficients::IDENTITY,
                high: Coefficients::IDENTITY,
                high_states: vec![State::default(); 2 * channels],
                low_states: [State::default(); 2],
                sum: vec![0.0; block],
            },
            ProcKind::Oscillator { waveform, .. } => Kind::Oscillator {
                waveform: *waveform,
                phase: 0.0,
                played: 0,
                noise: (0..channels)
                    .map(|c| 0x9e37_79b9u32.wrapping_mul(c as u32 + 1) | 1)
                    .collect(),
                pink: vec![[0.0; 7]; channels],
            },
            ProcKind::Gate { .. } => Kind::Gate {
                attack: 0.0,
                release: 0.0,
                envelope: 0.0,
                open: false,
                held: 0,
            },
            ProcKind::DeEsser { mode, listen, .. } => Kind::DeEsser {
                mode: *mode,
                listen: *listen,
                low: vec![Coefficients::IDENTITY; 2],
                high: vec![Coefficients::IDENTITY; 2],
                low_states: vec![State::default(); 2 * channels],
                high_states: vec![State::default(); 2 * channels],
                band: vec![0.0; channels * block],
                ballistics: Ballistics::new(1.0, 1.0),
                envelope: 0.0,
                scratch: vec![0.0; block],
            },
            ProcKind::Ducker { .. } => Kind::Ducker {
                attack: 0.0,
                release: 0.0,
                envelope: 0.0,
                held: 0,
            },
            ProcKind::Multiband { bands, .. } => {
                let n = bands.len();
                Kind::Multiband {
                    bands: n,
                    low: vec![Coefficients::IDENTITY; n - 1],
                    high: vec![Coefficients::IDENTITY; n - 1],
                    allpass: vec![Coefficients::IDENTITY; n - 1],
                    split_states: vec![[State::default(); 4]; (n - 1) * channels],
                    allpass_states: vec![State::default(); n * (n - 1) * channels],
                    scratch: vec![0.0; (n + 1) * channels * block],
                    ballistics: vec![Ballistics::new(1.0, 1.0); n],
                    envelopes: vec![0.0; n],
                }
            }
            ProcKind::Leveler { limiter, .. } => Kind::Leveler {
                k_states: vec![[State::default(); 2]; channels],
                weights: loudness_weights(channels),
                energy: 0.0,
                samples: 0,
                segments: vec![0.0; LEVELER_SEGMENTS],
                at: 0,
                filled: 0,
                desired: 0.0,
                gain: 0.0,
                limit: limiter.then(|| {
                    let mut gain = LookaheadGain::new(latency);
                    gain.set_release(LEVELER_RELEASE_MS);
                    (Delay::new(channels, latency), gain)
                }),
            },
            ProcKind::Delay { .. } => Kind::Delay(Delay::new(channels, kind.delay_samples())),
            // The compiler makes the auto node a gain step and the width and the
            // surround panner gain matrices; a processor of them passes through.
            ProcKind::Auto {} | ProcKind::Width { .. } | ProcKind::SurroundPan { .. } => {
                Kind::Delay(Delay::new(channels, 0))
            }
        };
        Self {
            id,
            channels,
            params: Smoothing::new(values),
            dirty: true,
            latency,
            dry: vec![0.0; channels * block],
            dry_delay: Delay::new(channels, latency),
            wet: values.first().copied().unwrap_or(1.0),
            meter,
            kind: k,
            start: 0,
            band_reduction: [0.0; MAX_BANDS],
        }
    }

    /// A one-block hold for `channels` channels.
    pub fn hold_block(id: NodeId, channels: usize, block: usize) -> Self {
        Self {
            id,
            channels,
            params: Smoothing::new(&[]),
            dirty: false,
            latency: 0,
            dry: Vec::new(),
            dry_delay: Delay::new(0, 0),
            wet: 1.0,
            meter: usize::MAX,
            kind: Kind::Hold(vec![0.0; channels * block]),
            start: 0,
            band_reduction: [0.0; MAX_BANDS],
        }
    }

    /// Copy the held block of channel `ch` into `out`.
    pub fn held(&self, ch: usize, out: &mut [f32]) {
        match &self.kind {
            Kind::Hold(data) if (ch + 1) * out.len() <= data.len() => {
                out.copy_from_slice(&data[ch * out.len()..(ch + 1) * out.len()])
            }
            _ => out.fill(0.0),
        }
    }

    /// Keep `samples` as channel `ch`'s block for next time.
    pub fn hold(&mut self, ch: usize, samples: &[f32]) {
        if let Kind::Hold(data) = &mut self.kind {
            let n = samples.len();
            if (ch + 1) * n <= data.len() {
                data[ch * n..(ch + 1) * n].copy_from_slice(samples);
            }
        }
    }

    pub fn set_param(&mut self, index: usize, value: f32, ramp: u32) {
        self.params.set(index, value, ramp);
    }

    /// Take over `old`'s state and current values (same structure); then ramp to
    /// this node's targets over `ramp` samples.
    pub fn adopt(&mut self, old: &mut Processor, ramp: u32) {
        std::mem::swap(&mut self.kind, &mut old.kind);
        std::mem::swap(&mut self.dry_delay, &mut old.dry_delay);
        self.wet = old.wet;
        if old.params.cur.len() == self.params.cur.len() {
            self.params.cur.copy_from_slice(&old.params.cur);
            for i in 0..self.params.cur.len() {
                let target = self.params.target[i];
                self.params.set(i, target, ramp);
            }
        }
        self.dirty = true;
    }

    fn update(&mut self) {
        let p = |i: usize| f64::from(self.params.cur[i]);
        match &mut self.kind {
            Kind::Eq {
                types,
                coefficients,
                ..
            } => {
                let mut at = 0;
                for (b, (t, order)) in types.iter().enumerate() {
                    let (f, q, g) = (p(1 + 3 * b), p(2 + 3 * b), p(3 + 3 * b));
                    let single = match t {
                        BandType::Bell => biquad::bell(f, q, g),
                        BandType::LowShelf => biquad::low_shelf(f, q, g),
                        BandType::HighShelf => biquad::high_shelf(f, q, g),
                        BandType::Notch => biquad::notch(f, q),
                        BandType::HighPass | BandType::LowPass => {
                            for (k, bq) in biquad::butterworth_q(*order).enumerate() {
                                coefficients[at + k] = if *t == BandType::HighPass {
                                    biquad::high_pass(f, bq)
                                } else {
                                    biquad::low_pass(f, bq)
                                };
                            }
                            at += order / 2;
                            continue;
                        }
                    };
                    coefficients[at] = single;
                    at += 1;
                }
            }
            Kind::Filter {
                mode,
                qs,
                coefficients,
                ..
            } => {
                for (c, q) in coefficients.iter_mut().zip(qs.iter()) {
                    *c = match mode {
                        FilterMode::HighPass => biquad::high_pass(p(1), *q),
                        FilterMode::LowPass => biquad::low_pass(p(1), *q),
                    };
                }
            }
            Kind::DynamicEq {
                bands,
                detect,
                ballistics,
                ..
            } => {
                for b in 0..*bands {
                    let o = 1 + 7 * b;
                    detect[b] = biquad::band_pass(p(o), p(o + 1));
                    ballistics[b] = Ballistics::new(p(o + 5), p(o + 6));
                }
            }
            Kind::Compressor {
                sidechain,
                ballistics,
                ..
            } => {
                *ballistics = Ballistics::new(p(4), p(5));
                if let Some((c, _)) = sidechain {
                    *c = biquad::high_pass(p(7), Q_BUTTERWORTH);
                }
            }
            Kind::Limiter { gain, .. } => gain.set_release(p(2)),
            Kind::Expander { ballistics, .. } => *ballistics = Ballistics::new(p(4), p(5)),
            Kind::Gate {
                attack, release, ..
            } => {
                *attack = crate::dsp::dynamics::coefficient(p(3));
                *release = crate::dsp::dynamics::coefficient(p(5));
            }
            Kind::DeEsser {
                low,
                high,
                ballistics,
                ..
            } => {
                for c in low.iter_mut() {
                    *c = biquad::low_pass(p(1), Q_BUTTERWORTH);
                }
                for c in high.iter_mut() {
                    *c = biquad::high_pass(p(1), Q_BUTTERWORTH);
                }
                *ballistics = Ballistics::new(p(4), p(5));
            }
            Kind::Ducker {
                attack, release, ..
            } => {
                *attack = crate::dsp::dynamics::coefficient(p(3));
                *release = crate::dsp::dynamics::coefficient(p(5));
            }
            Kind::Multiband {
                bands,
                low,
                high,
                allpass: ap,
                ballistics,
                ..
            } => {
                let crossovers = *bands - 1;
                for x in 0..crossovers {
                    let f = p(1 + x);
                    low[x] = biquad::low_pass(f, Q_BUTTERWORTH);
                    high[x] = biquad::high_pass(f, Q_BUTTERWORTH);
                    ap[x] = allpass(f);
                }
                for (b, ballistic) in ballistics.iter_mut().enumerate() {
                    let o = 1 + crossovers + 5 * b;
                    *ballistic = Ballistics::new(p(o + 2), p(o + 3));
                }
            }
            Kind::BassManagement { low, high, .. } => {
                *low = biquad::low_pass(p(1), Q_BUTTERWORTH);
                *high = biquad::high_pass(p(1), Q_BUTTERWORTH);
            }
            Kind::Delay(_) | Kind::Oscillator { .. } | Kind::Hold(_) | Kind::Leveler { .. } => {}
        }
    }

    /// Run one block in place on `io` (one slice per channel). `key` holds the
    /// key signal's channels for keyed nodes.
    pub fn process(
        &mut self,
        io: &mut [&mut [f32]],
        key: &[&[f32]],
        block: usize,
        meters: &[NodeMeter],
    ) {
        let n = io.len().min(self.channels);
        let io = &mut io[..n];
        if self.params.advance(block as u32) {
            self.dirty = true;
        }
        if self.dirty {
            self.update();
            self.dirty = false;
        }
        let wet_target = self.params.cur.first().copied().unwrap_or(1.0);
        let crossfade = self.wet != wet_target || wet_target < 1.0;
        if crossfade {
            for (c, ch) in io.iter().enumerate() {
                self.dry[c * block..(c + 1) * block].copy_from_slice(ch);
            }
        }
        let meter = meters.get(self.meter);
        if let Some(m) = meter {
            let peak = io
                .iter()
                .fold(0f32, |a, c| a.max(crate::dsp::reduce::peak(c)));
            max_into(&m.in_peak, peak);
        }
        let reduction = self.run(io, key, block);
        if let Some(m) = meter {
            max_into(&m.reduction, reduction as f32);
            if matches!(self.kind, Kind::Multiband { .. } | Kind::DynamicEq { .. }) {
                for (slot, gr) in m.bands.iter().zip(self.band_reduction.iter()) {
                    max_into(slot, *gr);
                }
            }
            if let Kind::Leveler { gain, .. } = &self.kind {
                m.level.store((*gain as f32).to_bits(), Ordering::Relaxed);
            }
        }
        if crossfade {
            let mut dry: [&mut [f32]; MAX_CH] = std::array::from_fn(|_| <&mut [f32]>::default());
            for (c, chunk) in self.dry.chunks_mut(block).take(n).enumerate() {
                dry[c] = chunk;
            }
            self.dry_delay.run(&mut dry[..n]);
            let (from, to) = (self.wet, wet_target);
            for (c, ch) in io.iter_mut().enumerate() {
                for (k, s) in ch.iter_mut().enumerate() {
                    let w = from + (to - from) * (k + 1) as f32 / block as f32;
                    *s = *s * w + dry[c][k] * (1.0 - w);
                }
            }
            self.wet = wet_target;
        } else if self.latency > 0 {
            // Keep the dry delay line current for a later bypass.
            for (c, ch) in io.iter().enumerate() {
                self.dry[c * block..(c + 1) * block].copy_from_slice(ch);
            }
        }
        if let Some(m) = meter {
            let peak = io
                .iter()
                .fold(0f32, |a, c| a.max(crate::dsp::reduce::peak(c)));
            max_into(&m.out_peak, peak);
        }
    }

    /// The type's processing; returns the largest gain reduction in dB.
    fn run(&mut self, io: &mut [&mut [f32]], key: &[&[f32]], block: usize) -> f64 {
        let start = self.start;
        let cur = &self.params.cur;
        let p = |i: usize| f64::from(cur[i]);
        let n = io.len();
        let chans = self.channels;
        let band_reduction = &mut self.band_reduction;
        match &mut self.kind {
            Kind::Eq {
                coefficients,
                states,
                ..
            }
            | Kind::Filter {
                coefficients,
                states,
                ..
            } => {
                let sections = coefficients.len();
                if sections <= biquad::CASCADE {
                    for (c, ch) in io.iter_mut().enumerate() {
                        let mut st = [State::default(); biquad::CASCADE];
                        for s in 0..sections {
                            st[s] = states[s * chans + c];
                        }
                        biquad::cascade(coefficients, &mut st[..sections], ch);
                        for s in 0..sections {
                            states[s * chans + c] = st[s];
                        }
                    }
                } else {
                    for (c, ch) in io.iter_mut().enumerate() {
                        for (s, co) in coefficients.iter().enumerate() {
                            states[s * chans + c].run(co, ch);
                        }
                    }
                }
                0.0
            }
            Kind::DynamicEq {
                bands,
                detect,
                detect_states,
                bells,
                bell_states,
                envelopes,
                ballistics,
            } => {
                let per = chans;
                let mut most = 0f64;
                for b in 0..*bands {
                    let o = 1 + 7 * b;
                    let (gain, threshold, ratio) = (p(o + 2), p(o + 3), p(o + 4));
                    let mut env = envelopes[b];
                    for k in 0..block {
                        let mut level = 0f64;
                        for (c, ch) in io.iter().enumerate() {
                            let y = detect_states[b * per + c].tick(&detect[b], f64::from(ch[k]));
                            level = level.max(y.abs());
                        }
                        let target = compressor_gain(to_db(level), threshold, ratio, 0.0);
                        env = ballistics[b].gain(env, target);
                    }
                    envelopes[b] = env;
                    most = most.max(-env);
                    band_reduction[b] = (-env).max(0.0) as f32;
                    bells[b] = biquad::bell(p(o), p(o + 1), gain + env);
                    for (c, ch) in io.iter_mut().enumerate() {
                        bell_states[b * per + c].run(&bells[b], ch);
                    }
                }
                most
            }
            Kind::Compressor {
                detector,
                sidechain,
                ballistics,
                envelope,
                mean_square,
                scratch,
            } => {
                // In passes over the block: only the ballistics run sample by
                // sample; the conversions and the curve vectorise. Detection reads
                // every sample before its gain is applied, as per sample.
                let (threshold, ratio, knee, makeup) = (p(1), p(2), p(3), p(6));
                let s = &mut scratch[..block];
                s.fill(0.0);
                let keyed = !key.is_empty();
                for c in 0..if keyed { key.len() } else { n } {
                    let src: &[f32] = if keyed {
                        &key[c][..block]
                    } else {
                        &io[c][..block]
                    };
                    match sidechain {
                        Some((co, st)) => {
                            let st = &mut st[c.min(MAX_CH - 1)];
                            for (v, x) in s.iter_mut().zip(src) {
                                *v = v.max(st.tick(co, f64::from(*x)).abs());
                            }
                        }
                        None => {
                            for (v, x) in s.iter_mut().zip(src) {
                                *v = v.max(f64::from(*x).abs());
                            }
                        }
                    }
                }
                let rms = matches!(detector, Detector::Rms);
                if rms {
                    let mut ms = *mean_square;
                    for v in s.iter_mut() {
                        ms = ballistics.level(ms, *v * *v);
                        *v = ms;
                    }
                    *mean_square = ms;
                    for v in s.iter_mut() {
                        *v = power_to_db(*v);
                    }
                } else {
                    for v in s.iter_mut() {
                        *v = to_db(*v);
                    }
                }
                for v in s.iter_mut() {
                    *v = compressor_gain(*v, threshold, ratio, knee);
                }
                if rms {
                    *envelope = s.last().copied().unwrap_or(*envelope);
                } else {
                    let mut env = *envelope;
                    for v in s.iter_mut() {
                        env = ballistics.gain(env, *v);
                        *v = env;
                    }
                    *envelope = env;
                }
                let most = s.iter().fold(0f64, |m, v| m.max(-v));
                for v in s.iter_mut() {
                    *v = from_db(*v + makeup);
                }
                for ch in io.iter_mut() {
                    for (x, g) in ch.iter_mut().zip(s.iter()) {
                        *x *= *g as f32;
                    }
                }
                most
            }
            Kind::Limiter {
                delay,
                gain,
                true_peak,
            } => lookahead_limit(io, delay, gain, true_peak.as_mut(), from_db(p(1)), block),
            Kind::Leveler {
                k_states,
                weights,
                energy,
                samples,
                segments,
                at,
                filled,
                desired,
                gain,
                limit,
            } => {
                use crate::dsp::loudness::{K_HIGH_PASS, K_SHELF};
                let (target, max_gain, max_cut, speed, gate) = (p(1), p(2), p(3), p(4), p(5));
                // The block's K-weighted, channel-weighted energy.
                let mut e = 0f64;
                for (c, ch) in io.iter().enumerate() {
                    let [shelf, hp] = &mut k_states[c];
                    let mut acc = 0f64;
                    for &x in ch.iter() {
                        let y = hp.tick(&K_HIGH_PASS, shelf.tick(&K_SHELF, f64::from(x)));
                        acc += y * y;
                    }
                    e += weights[c] * acc;
                }
                *energy += e;
                *samples += block;
                // Every 100 ms: the loudness of the last 3 s (less after a start),
                // and, above the gate, the gain that meets the target.
                if *samples >= LEVELER_SEGMENT {
                    segments[*at] = *energy / *samples as f64;
                    *at = (*at + 1) % LEVELER_SEGMENTS;
                    *filled = (*filled + 1).min(LEVELER_SEGMENTS);
                    *energy = 0.0;
                    *samples = 0;
                    if *filled >= LEVELER_FIRST {
                        let power = segments.iter().sum::<f64>() / *filled as f64;
                        let lufs = -0.691 + 10.0 * power.max(1e-20).log10();
                        if lufs > gate {
                            *desired = target - lufs;
                        }
                    }
                }
                // Toward the desired gain (within the limits) at the speed.
                let want = desired.clamp(-max_cut, max_gain);
                let step = speed * block as f64 / f64::from(crate::config::SAMPLE_RATE);
                let next = *gain + (want - *gain).clamp(-step, step);
                let (g0, g1) = (from_db(*gain), from_db(next));
                *gain = next;
                let inv = 1.0 / block as f64;
                for ch in io.iter_mut() {
                    for (k, x) in ch.iter_mut().enumerate() {
                        let g = g0 + (g1 - g0) * (k + 1) as f64 * inv;
                        *x = (f64::from(*x) * g) as f32;
                    }
                }
                match limit {
                    Some((delay, lg)) => lookahead_limit(io, delay, lg, None, from_db(p(6)), block),
                    None => 0.0,
                }
            }
            Kind::Expander {
                ballistics,
                envelope,
            } => {
                let (threshold, ratio, range) = (p(1), p(2), p(3));
                // The level detector smooths (attack while rising, release while
                // falling); a sine's zero crossings must not close the expander.
                let mut most = 0f64;
                for k in 0..block {
                    let peak = io.iter().fold(0f64, |a, ch| a.max(f64::from(ch[k]).abs()));
                    *envelope = ballistics.level(*envelope, peak);
                    let gain = expander_gain(to_db(*envelope), threshold, ratio, range);
                    most = most.max(-gain);
                    let g = from_db(gain) as f32;
                    for ch in io.iter_mut() {
                        ch[k] *= g;
                    }
                }
                most
            }
            Kind::Gate {
                attack,
                release,
                envelope,
                open,
                held,
            } => {
                let (threshold, hysteresis, hold, range) = (p(1), p(2), p(4), p(6));
                let hold_samples = (hold * 48.0) as u32;
                let mut most = 0f64;
                for k in 0..block {
                    let level = to_db(io.iter().fold(0f64, |a, ch| a.max(f64::from(ch[k]).abs())));
                    if level > threshold {
                        *open = true;
                        *held = 0;
                    } else if *open && level < threshold - hysteresis {
                        *held += 1;
                        if *held > hold_samples {
                            *open = false;
                        }
                    }
                    let target = if *open { 0.0 } else { -range };
                    let a = if target > *envelope {
                        *attack
                    } else {
                        *release
                    };
                    *envelope = a * *envelope + (1.0 - a) * target;
                    most = most.max(-*envelope);
                    let g = from_db(*envelope) as f32;
                    for ch in io.iter_mut() {
                        ch[k] *= g;
                    }
                }
                most
            }
            Kind::DeEsser {
                mode,
                listen,
                low,
                high,
                low_states,
                high_states,
                band,
                ballistics,
                envelope,
                scratch,
            } => {
                let (threshold, range) = (p(2), p(3));
                // The band above the frequency (split-band: LR4 high part; wide-band:
                // the second-order high-pass sidechain).
                for (c, ch) in io.iter_mut().enumerate() {
                    let b = &mut band[c * block..(c + 1) * block];
                    b.copy_from_slice(ch);
                    let sections = if *mode == DeEsserMode::SplitBand {
                        2
                    } else {
                        1
                    };
                    biquad::cascade(
                        &high[..sections],
                        &mut high_states[2 * c..2 * c + sections],
                        b,
                    );
                    if *mode == DeEsserMode::SplitBand {
                        biquad::cascade(&low[..2], &mut low_states[2 * c..2 * c + 2], ch);
                    }
                }
                // The level, its dB, the target and the gain vectorise; the
                // ballistics run sample by sample.
                let s = &mut scratch[..block];
                s.fill(0.0);
                for c in 0..n {
                    for (v, x) in s.iter_mut().zip(&band[c * block..(c + 1) * block]) {
                        *v = v.max(f64::from(*x).abs());
                    }
                }
                for v in s.iter_mut() {
                    let level = to_db(*v);
                    *v = if level > threshold {
                        (threshold - level).max(-range)
                    } else {
                        0.0
                    };
                }
                let mut env = *envelope;
                for v in s.iter_mut() {
                    env = ballistics.gain(env, *v);
                    *v = env;
                }
                *envelope = env;
                let most = s.iter().fold(0f64, |m, v| m.max(-v));
                for v in s.iter_mut() {
                    *v = from_db(*v);
                }
                for (c, ch) in io.iter_mut().enumerate() {
                    let hb = &band[c * block..(c + 1) * block];
                    match (*listen, *mode) {
                        (true, _) => ch.copy_from_slice(hb),
                        (false, DeEsserMode::SplitBand) => {
                            for ((x, h), g) in ch.iter_mut().zip(hb).zip(s.iter()) {
                                *x += h * *g as f32;
                            }
                        }
                        (false, DeEsserMode::WideBand) => {
                            for (x, g) in ch.iter_mut().zip(s.iter()) {
                                *x *= *g as f32;
                            }
                        }
                    }
                }
                most
            }
            Kind::Ducker {
                attack,
                release,
                envelope,
                held,
            } => {
                let (threshold, depth, hold) = (p(1), p(2), p(4));
                let hold_samples = (hold * 48.0) as u32;
                let mut most = 0f64;
                for k in 0..block {
                    let level = to_db(key.iter().fold(0f64, |a, ch| a.max(f64::from(ch[k]).abs())));
                    let target = if level > threshold {
                        *held = 0;
                        -depth
                    } else if *held < hold_samples {
                        *held += 1;
                        -depth
                    } else {
                        0.0
                    };
                    let a = if target < *envelope {
                        *attack
                    } else {
                        *release
                    };
                    *envelope = a * *envelope + (1.0 - a) * target;
                    most = most.max(-*envelope);
                    let g = from_db(*envelope) as f32;
                    for ch in io.iter_mut() {
                        ch[k] *= g;
                    }
                }
                most
            }
            Kind::Multiband {
                bands,
                low,
                high,
                allpass: ap,
                split_states,
                allpass_states,
                scratch,
                ballistics,
                envelopes,
            } => {
                let nb = *bands;
                let crossovers = nb - 1;
                // scratch: band b, channel c at ((b * n) + c) * block; the last slot
                // holds the running remainder.
                for (c, ch) in io.iter().enumerate() {
                    let rest_at = (nb * n + c) * block;
                    scratch[rest_at..rest_at + block].copy_from_slice(ch);
                    for x in 0..crossovers {
                        let st = &mut split_states[x * chans + c];
                        for k in 0..block {
                            let v = f64::from(scratch[rest_at + k]);
                            let lo = st[0].tick(&low[x], v);
                            let lo = st[1].tick(&low[x], lo);
                            let hi = st[2].tick(&high[x], v);
                            let hi = st[3].tick(&high[x], hi);
                            scratch[(x * n + c) * block + k] = lo as f32;
                            scratch[rest_at + k] = hi as f32;
                        }
                        // Lower bands pass the all-pass of every higher crossover.
                        for b in 0..x {
                            let st = &mut allpass_states[(b * crossovers + x) * chans + c];
                            let at = (b * n + c) * block;
                            st.run(&ap[x], &mut scratch[at..at + block]);
                        }
                    }
                    let last = ((nb - 1) * n + c) * block;
                    scratch.copy_within(rest_at..rest_at + block, last);
                }
                let mut most = 0f64;
                for b in 0..nb {
                    let o = 1 + crossovers + 5 * b;
                    let (threshold, ratio, makeup) = (p(o), p(o + 1), p(o + 4));
                    let mut env = envelopes[b];
                    for k in 0..block {
                        let peak = (0..n).fold(0f64, |a, c| {
                            a.max(f64::from(scratch[(b * n + c) * block + k]).abs())
                        });
                        let target = compressor_gain(to_db(peak), threshold, ratio, 0.0);
                        env = ballistics[b].gain(env, target);
                        let g = from_db(env + makeup) as f32;
                        for c in 0..n {
                            scratch[(b * n + c) * block + k] *= g;
                        }
                    }
                    envelopes[b] = env;
                    most = most.max(-env);
                    band_reduction[b] = (-env).max(0.0) as f32;
                }
                for (c, ch) in io.iter_mut().enumerate() {
                    for (k, s) in ch.iter_mut().enumerate() {
                        *s = (0..nb).map(|b| scratch[(b * n + c) * block + k]).sum();
                    }
                }
                most
            }
            Kind::BassManagement {
                lfe,
                low,
                high,
                high_states,
                low_states,
                sum,
            } => {
                let sum = &mut sum[..block];
                sum.fill(0.0);
                for (c, ch) in io.iter_mut().enumerate() {
                    if c == *lfe {
                        continue;
                    }
                    for (s, x) in sum.iter_mut().zip(ch.iter()) {
                        *s += *x;
                    }
                    high_states[2 * c].run(high, ch);
                    high_states[2 * c + 1].run(high, ch);
                }
                low_states[0].run(low, sum);
                low_states[1].run(low, sum);
                let g = from_db(p(2)) as f32;
                if let Some(ch) = io.get_mut(*lfe) {
                    for (y, s) in ch.iter_mut().zip(sum.iter()) {
                        *y = *y * g + *s;
                    }
                }
                0.0
            }
            Kind::Oscillator {
                waveform,
                phase,
                played,
                noise,
                pink,
            } => {
                use crate::mix::chain::Waveform;
                let rate = f64::from(crate::config::SAMPLE_RATE);
                let amplitude = from_db(p(2));
                match waveform {
                    Waveform::White | Waveform::Pink => {
                        let rms = amplitude as f32;
                        let is_pink = *waveform == Waveform::Pink;
                        for (c, ch) in io.iter_mut().enumerate() {
                            let (Some(state), Some(filter)) = (noise.get_mut(c), pink.get_mut(c))
                            else {
                                continue;
                            };
                            for y in ch.iter_mut().take(block) {
                                let w = white(state);
                                *y = rms * if is_pink { pink_filter(filter, w) } else { w };
                            }
                        }
                        *played = played.wrapping_add(block as u64);
                        return 0.0;
                    }
                    Waveform::Sweep => {
                        let (from, to, secs) = (p(1), p(3), p(4));
                        let period = (secs * rate).max(1.0) as u64;
                        for k in 0..block {
                            let t = (*played % period) as f64 / period as f64;
                            let f = from * (to / from).powf(t);
                            let s =
                                ((2.0 * std::f64::consts::PI * *phase).sin() * amplitude) as f32;
                            *phase = (*phase + f / rate).fract();
                            *played = played.wrapping_add(1);
                            for ch in io.iter_mut() {
                                ch[k] = s;
                            }
                        }
                        return 0.0;
                    }
                    _ => {}
                }
                let step = p(1) / rate;
                for k in 0..block {
                    let (left, all) = match waveform {
                        Waveform::EbuIdent => (ident_gain(*played), 1.0),
                        Waveform::SecondPulse => {
                            let g = pulse_gain(start + k as u64);
                            (g, g)
                        }
                        _ => (1.0, 1.0),
                    };
                    let s = ((2.0 * std::f64::consts::PI * *phase).sin() * amplitude) as f32;
                    *phase = if all == 0.0 {
                        0.0
                    } else {
                        (*phase + step).fract()
                    };
                    *played = played.wrapping_add(1);
                    for (c, ch) in io.iter_mut().enumerate() {
                        ch[k] = s * if c == 0 { left } else { all };
                    }
                }
                0.0
            }
            Kind::Delay(d) => {
                d.run(io);
                0.0
            }
            Kind::Hold(_) => 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mix::chain::{DynamicBand, EqBand, Key, MultibandBand};
    use crate::mix::model::Tap;
    use std::f64::consts::PI;

    const B: usize = 96;

    fn proc(kind: ProcKind, channels: usize) -> Processor {
        let mut values = vec![1.0];
        values.extend(kind.values());
        Processor::new(0, &kind, &values, channels, B, usize::MAX)
    }

    /// Not a check: the DSP thread's hot spots per 96-sample block (the §16 load's
    /// nodes, the meters), in ns:
    /// `cargo test --release --lib dsp_costs -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn dsp_costs() {
        let tone: Vec<f32> = (0..B * 1000)
            .map(|i| (0.3 * (2.0 * PI * 997.0 * i as f64 / 48_000.0).sin()) as f32)
            .collect();
        type Run = Box<dyn FnMut(&mut [f32])>;
        let time = |what: &str, mut f: Run| {
            let mut buf = vec![0f32; B];
            let started = std::time::Instant::now();
            for b in tone.chunks_exact(B) {
                buf.copy_from_slice(b);
                f(&mut buf);
            }
            let ns = started.elapsed().as_nanos() as f64 / 1000.0;
            eprintln!("{what:28} {ns:8.0} ns per block");
        };
        let node = |kind: ProcKind| {
            let mut p = proc(kind, 1);
            Box::new(move |b: &mut [f32]| {
                let mut io = [b];
                p.process(&mut io, &[], B, &[]);
            }) as Box<dyn FnMut(&mut [f32])>
        };
        let comp = |detector| ProcKind::Compressor {
            threshold_db: -26.0,
            ratio: 3.0,
            knee_db: 6.0,
            attack_ms: 5.0,
            release_ms: 150.0,
            makeup_db: 2.0,
            detector,
            sidechain_hpf_hz: None,
            key: None,
        };
        time(
            "compressor (peak)",
            node(comp(crate::mix::chain::Detector::Peak)),
        );
        time(
            "compressor (rms)",
            node(comp(crate::mix::chain::Detector::Rms)),
        );
        time(
            "de-esser (split-band)",
            node(ProcKind::DeEsser {
                mode: DeEsserMode::SplitBand,
                freq_hz: 6000.0,
                threshold_db: -30.0,
                range_db: 6.0,
                attack_ms: 1.0,
                release_ms: 80.0,
                listen: false,
            }),
        );
        time(
            "eq (4 bands)",
            node(ProcKind::Eq {
                bands: (0..4)
                    .map(|k| EqBand {
                        kind: crate::mix::chain::BandType::Bell,
                        freq_hz: 200.0 * (k + 1) as f64,
                        q: 1.0,
                        gain_db: 3.0,
                        slope_db_per_oct: None,
                    })
                    .collect(),
            }),
        );
        let pass = |kind, f, slope| EqBand {
            kind,
            freq_hz: f,
            q: 0.707,
            gain_db: 0.0,
            slope_db_per_oct: Some(slope),
        };
        time(
            "eq (hp 24, 4 bells, lp 12)",
            node(ProcKind::Eq {
                bands: std::iter::once(pass(BandType::HighPass, 80.0, 24))
                    .chain((0..4).map(|k| EqBand {
                        kind: BandType::Bell,
                        freq_hz: 200.0 * (k + 1) as f64,
                        q: 1.0,
                        gain_db: 3.0,
                        slope_db_per_oct: None,
                    }))
                    .chain(std::iter::once(pass(BandType::LowPass, 16_000.0, 12)))
                    .collect(),
            }),
        );
        time(
            "filter (hp 24)",
            node(ProcKind::Filter {
                mode: FilterMode::HighPass,
                freq_hz: 80.0,
                slope_db_per_oct: 24,
            }),
        );
        // Gain-matrix steps (the width node, the surround panner, a send's
        // down-mix), on the plan: steady and while the values ramp.
        use super::super::plan::{InputSource, OutputSink, PlanBuilder, Step};
        struct Tone<'a>(&'a [f32]);
        impl InputSource for Tone<'_> {
            fn channel(&self, _: usize, _: usize) -> Option<&[f32]> {
                Some(self.0)
            }
        }
        struct Null;
        impl OutputSink for Null {
            fn write(&mut self, _: usize, _: usize, _: usize, _: &[f32]) {}
        }
        let matrix = |what: &str, inputs: usize, values: Vec<f32>, send: bool| {
            let outputs = values.len() / inputs;
            for ramping in [false, true] {
                let mut b = PlanBuilder::new(B);
                let src = b.buffers(inputs);
                for c in 0..inputs {
                    let dst = b.buffer(src, c);
                    b.step(Step::Patch {
                        input: 0,
                        channel: c as u32,
                        dst,
                    });
                }
                let dst = b.buffers(outputs);
                let node = if send {
                    b.node(1, &[1.0], 0)
                } else {
                    b.node(1, &values, 0)
                };
                let coefficients = b.coefficients(&values);
                b.step(if send {
                    Step::Send {
                        node,
                        src,
                        dst,
                        coefficients,
                    }
                } else {
                    Step::Matrix { node, src, dst }
                });
                let mut plan = b.finish();
                // Without the matrix: the patch steps alone, taken off.
                let mut bare = PlanBuilder::new(B);
                let s2 = bare.buffers(inputs);
                for c in 0..inputs {
                    let dst = bare.buffer(s2, c);
                    bare.step(Step::Patch {
                        input: 0,
                        channel: c as u32,
                        dst,
                    });
                }
                let mut bare = bare.finish();
                let blocks = 20_000u64;
                let t = |p: &mut super::super::plan::Plan, ramp: bool| {
                    let started = std::time::Instant::now();
                    for k in 0..blocks {
                        if ramp && k % 8 == 0 {
                            p.set_param(1, &values, 8 * B as u32);
                        }
                        p.process(k * B as u64, &Tone(&tone[..B]), &mut Null);
                    }
                    started.elapsed().as_nanos() as f64 / blocks as f64
                };
                let base = t(&mut bare, false);
                let ns = t(&mut plan, ramping) - base;
                let label = format!("{what}{}", if ramping { " (ramping)" } else { "" });
                eprintln!("{label:28} {ns:8.0} ns per block");
            }
        };
        let width: Vec<f32> = crate::mix::spatial::width_matrix(
            150.0,
            crate::mix::chain::StereoMode::Lr,
            crate::mix::chain::StereoMode::Lr,
        )
        .iter()
        .map(|v| *v as f32)
        .collect();
        matrix("width (2 × 2)", 2, width, false);
        use crate::mix::format::Format;
        let pan = |layout, inputs| {
            crate::mix::spatial::surround_pan(layout, inputs, 40.0, 20.0, 30.0, -10.0)
        };
        matrix(
            "surround pan mono → 5.1",
            1,
            pan(Format::Surround51, 1),
            false,
        );
        matrix(
            "surround pan stereo → 7.1",
            2,
            pan(Format::Surround71, 2),
            false,
        );
        let laws = crate::mix::format::Laws::default();
        matrix(
            "send 5.1 → stereo (down-mix)",
            6,
            crate::mix::format::width_matrix(Format::Surround51, Format::Stereo, laws),
            true,
        );
        // A metered channel on the meter thread: peak, energy, true peak, PPM.
        let mut meterer = crate::engine::meter::Meterer::default();
        meterer.plan(
            1,
            std::sync::Arc::new(super::super::plan::MeterBank::new(1)),
        );
        time(
            "meter channel (all)",
            Box::new(move |b: &mut [f32]| meterer.measure(1, 0, &[b])),
        );
        let ppm = [
            crate::dsp::ppm::Ballistics::type_i(),
            crate::dsp::ppm::Ballistics::type_ii(),
        ];
        let mut level = [0f32; 2];
        time(
            "ppm (both types)",
            Box::new(move |b: &mut [f32]| {
                let (l, m) = crate::dsp::ppm::run_pair(&ppm, level, b);
                level = l;
                std::hint::black_box(m);
            }),
        );
    }

    /// Run `p` over `input` (per channel) with an optional key; returns the output.
    #[test]
    fn bass_management_sends_the_lows_to_the_lfe() {
        let kind = ProcKind::BassManagement {
            crossover_hz: 80.0,
            lfe_gain_db: 10.0,
            lfe: 3,
        };
        let sine = |f: f64| -> Vec<f32> {
            (0..48_000)
                .map(|i| (0.5 * (2.0 * PI * f * i as f64 / 48_000.0).sin()) as f32)
                .collect()
        };
        let db = |c: &[f32]| {
            20.0 * f64::from(c[24_000..].iter().fold(0f32, |a, x| a.max(x.abs()))).log10()
        };
        for (f, l_db, lfe_db) in [(1000.0, -6.02, -80.0), (30.0, -40.0, -6.0)] {
            // 5.1: the tone on L only.
            let mut input = vec![vec![0f32; 48_000]; 6];
            input[0] = sine(f);
            let mut p = proc(kind.clone(), 6);
            let out = run(&mut p, &input, None);
            if f > 100.0 {
                assert!(
                    (db(&out[0]) - l_db).abs() < 0.1,
                    "{f} Hz on L: {}",
                    db(&out[0])
                );
                assert!(db(&out[3]) < lfe_db, "{f} Hz on the LFE: {}", db(&out[3]));
            } else {
                assert!(db(&out[0]) < l_db, "{f} Hz on L: {}", db(&out[0]));
                assert!(
                    (db(&out[3]) - lfe_db).abs() < 0.3,
                    "{f} Hz on the LFE: {}",
                    db(&out[3])
                );
            }
            assert!(db(&out[1]) < -100.0 && db(&out[2]) < -100.0);
        }
        // At the crossover the high-passed L and the low-passed sub add up to the input (LR4).
        let mut input = vec![vec![0f32; 48_000]; 6];
        input[0] = sine(80.0);
        let out = run(&mut proc(kind.clone(), 6), &input, None);
        let sum: Vec<f32> = out[0].iter().zip(&out[3]).map(|(a, b)| a + b).collect();
        assert!((db(&sum) + 6.02).abs() < 0.05, "{}", db(&sum));
        // The LFE input at +10 dB.
        let mut input = vec![vec![0f32; 48_000]; 6];
        input[3] = sine(30.0).iter().map(|x| x * 0.1).collect();
        let out = run(&mut proc(kind, 6), &input, None);
        assert!(
            (db(&out[3]) - (-26.02 + 10.0)).abs() < 0.05,
            "{}",
            db(&out[3])
        );
    }

    #[test]
    fn oscillator_sine_and_ebu_ident() {
        let sine = ProcKind::Oscillator {
            waveform: crate::mix::chain::Waveform::Sine,
            freq_hz: 1000.0,
            level_dbfs: -18.0,
            sweep_to_hz: 20_000.0,
            sweep_s: 10.0,
        };
        let mut p = proc(sine, 2);
        let out = run(&mut p, &vec![vec![0.5f32; 48_000]; 2], None);
        let peak = out[0].iter().fold(0f32, |a, x| a.max(x.abs()));
        assert!(
            (20.0 * f64::from(peak).log10() + 18.0).abs() < 0.01,
            "{peak}"
        );
        assert_eq!(out[0], out[1]);
        // 1 kHz: 2000 zero crossings a second.
        let crossings = out[0]
            .windows(2)
            .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
            .count();
        assert!((1998..=2001).contains(&crossings), "{crossings}");

        let ident = ProcKind::Oscillator {
            waveform: crate::mix::chain::Waveform::EbuIdent,
            freq_hz: 1000.0,
            level_dbfs: -18.0,
            sweep_to_hz: 20_000.0,
            sweep_s: 10.0,
        };
        let mut p = proc(ident, 2);
        let out = run(&mut p, &vec![vec![0.0f32; 4 * 48_000]; 2], None);
        let peak = |c: &[f32]| c.iter().fold(0f32, |a, x| a.max(x.abs()));
        assert_eq!(peak(&out[0][..12_000]), 0.0, "left off for 0.25 s");
        assert!(peak(&out[1][..12_000]) > 0.12, "right on");
        assert!(peak(&out[0][12_200..143_000]) > 0.12, "left back");
        assert_eq!(peak(&out[0][144_000..156_000]), 0.0, "again after 3 s");
    }

    #[test]
    fn noise_and_sweep() {
        let rms = |x: &[f32]| {
            (x.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
        };
        let db = |r: f64| 20.0 * r.log10();
        let osc = |waveform| ProcKind::Oscillator {
            waveform,
            freq_hz: 20.0,
            level_dbfs: -18.0,
            sweep_to_hz: 20_000.0,
            sweep_s: 1.0,
        };
        use crate::mix::chain::Waveform;
        for w in [Waveform::White, Waveform::Pink] {
            let out = run(
                &mut proc(osc(w), 2),
                &vec![vec![0f32; 10 * 48_000]; 2],
                None,
            );
            for c in &out {
                assert!(
                    (db(rms(&c[48_000..])) + 18.0).abs() < 0.2,
                    "{w:?} at {} dBFS RMS",
                    db(rms(&c[48_000..]))
                );
            }
            // Independent channels: almost no correlation.
            let dot: f64 = out[0]
                .iter()
                .zip(&out[1])
                .map(|(a, b)| f64::from(*a) * f64::from(*b))
                .sum();
            let corr = dot / (rms(&out[0]) * rms(&out[1]) * out[0].len() as f64);
            assert!(corr.abs() < 0.02, "{w:?} channels correlate {corr}");
        }
        // Pink: −3 dB per octave, compared over two octaves of bands.
        let out = run(
            &mut proc(osc(Waveform::Pink), 1),
            &[vec![0f32; 20 * 48_000]],
            None,
        );
        let band = |lo: f64| {
            let k = |f: f64| ProcKind::Filter {
                mode: crate::mix::chain::FilterMode::HighPass,
                freq_hz: f,
                slope_db_per_oct: 48,
            };
            let lp = |f: f64| ProcKind::Filter {
                mode: crate::mix::chain::FilterMode::LowPass,
                freq_hz: f,
                slope_db_per_oct: 48,
            };
            let x = run(&mut proc(k(lo), 1), &out, None);
            let x = run(&mut proc(lp(lo * 2.0), 1), &x, None);
            db(rms(&x[0][48_000..]))
        };
        let (b250, b1k, b4k) = (band(250.0), band(1000.0), band(4000.0));
        // Equal energy per octave: within a dB from octave to octave.
        assert!(
            (b250 - b1k).abs() < 1.0 && (b1k - b4k).abs() < 1.0,
            "{b250} {b1k} {b4k}"
        );
        // Sweep: peak at the level, from 20 Hz up to 20 kHz in 1 s, again.
        let out = run(
            &mut proc(osc(Waveform::Sweep), 1),
            &[vec![0f32; 2 * 48_000]],
            None,
        );
        let peak = out[0].iter().fold(0f32, |a, x| a.max(x.abs()));
        assert!((20.0 * f64::from(peak).log10() + 18.0).abs() < 0.05);
        let crossings = |x: &[f32]| {
            x.windows(2)
                .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
                .count()
        };
        // The first 100 ms of a sweep run 20 Hz to ~40 Hz; the last 100 ms ~10–20 kHz.
        assert!(crossings(&out[0][..4_800]) < 10);
        assert!(crossings(&out[0][43_200..48_000]) > 2_000);
        assert!(crossings(&out[0][48_000..52_800]) < 10, "it starts again");
    }

    #[test]
    fn second_pulse_follows_the_sample_index() {
        let pulse = ProcKind::Oscillator {
            waveform: crate::mix::chain::Waveform::SecondPulse,
            freq_hz: 1000.0,
            level_dbfs: -18.0,
            sweep_to_hz: 20_000.0,
            sweep_s: 10.0,
        };
        let mut p = proc(pulse, 2);
        // Start 0.5 s into a second: the first pulse is at 24 000.
        let first = 48_000 * 1_000_000 + 24_000;
        let mut out = vec![vec![0f32; 2 * 48_000]; 2];
        for (b, start) in (0..out[0].len()).step_by(B).enumerate() {
            p.start = first + (b * B) as u64;
            let mut io: Vec<&mut [f32]> =
                out.iter_mut().map(|c| &mut c[start..start + B]).collect();
            p.process(&mut io, &[], B, &[]);
        }
        let peak = |c: &[f32]| c.iter().fold(0f32, |a, x| a.max(x.abs()));
        assert_eq!(peak(&out[0][..24_000]), 0.0);
        assert!(peak(&out[0][24_000..24_960]) > 0.12);
        assert_eq!(out[0][24_000], 0.0, "starts at phase 0");
        assert_eq!(peak(&out[0][24_960..72_000]), 0.0);
        assert!(peak(&out[1][72_000..72_960]) > 0.12);
        assert_eq!(out[0], out[1]);
    }

    fn run(p: &mut Processor, input: &[Vec<f32>], key: Option<&[Vec<f32>]>) -> Vec<Vec<f32>> {
        let mut out = input.to_vec();
        let n = input[0].len() / B * B;
        for start in (0..n).step_by(B) {
            let mut io: Vec<&mut [f32]> =
                out.iter_mut().map(|c| &mut c[start..start + B]).collect();
            let k: Vec<&[f32]> = key
                .map(|k| k.iter().map(|c| &c[start..start + B]).collect())
                .unwrap_or_default();
            p.process(&mut io, &k, B, &[]);
        }
        out
    }

    fn sine(freq: f64, dbfs: f64, secs: f64) -> Vec<f32> {
        let a = 10f64.powf(dbfs / 20.0);
        (0..(secs * 48_000.0) as usize)
            .map(|k| (a * (2.0 * PI * freq * k as f64 / 48_000.0).sin()) as f32)
            .collect()
    }

    /// Peak in dBFS of the last `secs` seconds.
    fn tail_peak(x: &[f32], secs: f64) -> f64 {
        let from = x.len() - (secs * 48_000.0) as usize;
        20.0 * f64::from(x[from..].iter().fold(0f32, |m, v| m.max(v.abs()))).log10()
    }

    fn leveler(target: f64, max_gain: f64, speed: f64, gate: f64, limiter: bool) -> ProcKind {
        ProcKind::Leveler {
            target_lufs: target,
            max_gain_db: max_gain,
            max_cut_db: 12.0,
            speed_db_per_s: speed,
            gate_lufs: gate,
            limiter,
            ceiling_db: -1.0,
        }
    }

    /// BS.1770 loudness (one channel, weight 1) of the last `secs` seconds.
    fn tail_lufs(x: &[f32], secs: f64) -> f64 {
        use crate::dsp::loudness::{K_HIGH_PASS, K_SHELF};
        let (mut a, mut b) = (State::default(), State::default());
        let from = x.len() - (secs * 48_000.0) as usize;
        let mut e = 0.0;
        for (k, v) in x.iter().enumerate() {
            let y = b.tick(&K_HIGH_PASS, a.tick(&K_SHELF, f64::from(*v)));
            if k >= from {
                e += y * y;
            }
        }
        -0.691 + 10.0 * (e / (x.len() - from) as f64).log10()
    }

    #[test]
    fn leveler_meets_the_target_within_its_limits() {
        // −33 LUFS-ish into a −23 target: about +10 dB after a few seconds.
        let input = sine(997.0, -30.0, 12.0);
        let before = tail_lufs(&input, 2.0);
        let mut p = proc(leveler(-23.0, 15.0, 5.0, -60.0, false), 1);
        let out = run(&mut p, std::slice::from_ref(&input), None);
        let after = tail_lufs(&out[0], 2.0);
        assert!((after + 23.0).abs() < 0.5, "{before} → {after}");
        // The gain is limited: at most +6 dB.
        let mut p = proc(leveler(-23.0, 6.0, 5.0, -60.0, false), 1);
        let out = run(&mut p, std::slice::from_ref(&input), None);
        let gain = tail_lufs(&out[0], 2.0) - before;
        assert!((gain - 6.0).abs() < 0.2, "{gain}");
        // It moves at its speed: after 1 s at 2 dB/s, about 2 dB (the first
        // 400 ms it waits for a measurement).
        let short = sine(997.0, -30.0, 1.4);
        let mut p = proc(leveler(-23.0, 15.0, 2.0, -60.0, false), 1);
        run(&mut p, std::slice::from_ref(&short), None);
        match &p.kind {
            Kind::Leveler { gain, .. } => assert!((gain - 2.0).abs() < 0.2, "{gain}"),
            _ => unreachable!(),
        }
    }

    #[test]
    fn leveler_holds_below_the_gate_and_limits() {
        // −63 LUFS-ish is below a −50 gate: the gain stays at 0.
        let quiet = sine(997.0, -60.0, 6.0);
        let mut p = proc(leveler(-23.0, 15.0, 5.0, -50.0, false), 1);
        let out = run(&mut p, std::slice::from_ref(&quiet), None);
        assert!((tail_peak(&out[0], 1.0) - tail_peak(&quiet, 1.0)).abs() < 0.01);
        // Levelled to −10 LUFS (a sine's peak about −7 dBFS): the safety
        // limiter at −10 dBFS holds it, with 1 ms latency.
        let loud = sine(997.0, -20.0, 8.0);
        let mut kind = leveler(-10.0, 15.0, 10.0, -60.0, true);
        if let ProcKind::Leveler { ceiling_db, .. } = &mut kind {
            *ceiling_db = -10.0;
        }
        let mut p = proc(kind, 1);
        assert_eq!(p.latency, 48);
        let out = run(&mut p, std::slice::from_ref(&loud), None);
        let peak = tail_peak(&out[0], 2.0);
        assert!(peak <= -9.99 && peak > -10.5, "{peak}");
    }

    /// Not a check: the leveler's cost per 96-sample block, stereo and 5.1:
    /// `cargo test --release --lib leveler_cost -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn leveler_cost() {
        for channels in [2, 6] {
            for limiter in [false, true] {
                let mut p = proc(leveler(-23.0, 12.0, 2.0, -50.0, limiter), channels);
                let tone = sine(997.0, -20.0, 1.0);
                let mut bufs = vec![vec![0f32; B]; channels];
                let blocks = 20_000;
                let started = std::time::Instant::now();
                for b in 0..blocks {
                    let at = (b * B) % (tone.len() - B);
                    for c in bufs.iter_mut() {
                        c.copy_from_slice(&tone[at..at + B]);
                    }
                    let mut io: Vec<&mut [f32]> = bufs.iter_mut().map(|c| &mut c[..]).collect();
                    p.process(&mut io, &[], B, &[]);
                }
                let ns = started.elapsed().as_nanos() as f64 / blocks as f64;
                eprintln!("leveler {channels} ch, limiter {limiter}: {ns:6.0} ns per block");
            }
        }
    }

    #[test]
    fn eq_and_filter() {
        let mut eq = proc(
            ProcKind::Eq {
                bands: vec![EqBand {
                    kind: BandType::Bell,
                    freq_hz: 1000.0,
                    q: 1.0,
                    gain_db: 6.0,
                    slope_db_per_oct: None,
                }],
            },
            1,
        );
        let out = run(&mut eq, &[sine(1000.0, -20.0, 1.0)], None);
        assert!((tail_peak(&out[0], 0.5) + 14.0).abs() < 0.05);
        let mut hp = proc(
            ProcKind::Filter {
                mode: FilterMode::HighPass,
                freq_hz: 1000.0,
                slope_db_per_oct: 24,
            },
            1,
        );
        let out = run(&mut hp, &[sine(250.0, 0.0, 1.0)], None);
        assert!(
            (tail_peak(&out[0], 0.5) + 48.2).abs() < 1.0,
            "{}",
            tail_peak(&out[0], 0.5)
        );
    }

    #[test]
    fn eq_pass_bands_match_the_filter_node() {
        let band = |kind, f, slope| EqBand {
            kind,
            freq_hz: f,
            q: 0.707,
            gain_db: 0.0,
            slope_db_per_oct: Some(slope),
        };
        for (kind, mode) in [
            (BandType::HighPass, FilterMode::HighPass),
            (BandType::LowPass, FilterMode::LowPass),
        ] {
            for slope in [12, 24, 36, 48] {
                let x = sine(
                    if kind == BandType::HighPass {
                        250.0
                    } else {
                        4000.0
                    },
                    0.0,
                    1.0,
                );
                let mut eq = proc(
                    ProcKind::Eq {
                        bands: vec![band(kind, 1000.0, slope)],
                    },
                    1,
                );
                let mut filter = proc(
                    ProcKind::Filter {
                        mode,
                        freq_hz: 1000.0,
                        slope_db_per_oct: slope,
                    },
                    1,
                );
                let a = run(&mut eq, std::slice::from_ref(&x), None);
                let b = run(&mut filter, std::slice::from_ref(&x), None);
                assert_eq!(a, b, "{kind:?} {slope}");
                // Two octaves away: about slope × 2 down (the bilinear transform
                // adds a little towards Nyquist).
                let p = tail_peak(&a[0], 0.5);
                assert!(
                    (p + 2.0 * slope as f64).abs() < 3.0,
                    "{kind:?} {slope} dB/oct: {p}"
                );
            }
        }
        // All in one EQ: high-pass 24, a bell, low-pass 12 (4 sections).
        let mut eq = proc(
            ProcKind::Eq {
                bands: vec![
                    band(BandType::HighPass, 80.0, 24),
                    EqBand {
                        kind: BandType::Bell,
                        freq_hz: 1000.0,
                        q: 1.0,
                        gain_db: 6.0,
                        slope_db_per_oct: None,
                    },
                    band(BandType::LowPass, 15_000.0, 12),
                ],
            },
            2,
        );
        let x = sine(1000.0, -20.0, 1.0);
        let out = run(&mut eq, &[x.clone(), x], None);
        assert!(
            (tail_peak(&out[0], 0.5) + 14.0).abs() < 0.1,
            "{}",
            tail_peak(&out[0], 0.5)
        );
        assert_eq!(out[0], out[1]);
        let low = run(
            &mut eq,
            &[sine(20.0, -20.0, 1.0), sine(20.0, -20.0, 1.0)],
            None,
        );
        assert!(tail_peak(&low[0], 0.5) < -65.0);
        // The frequency of a pass band ramps like any parameter.
        eq.set_param(1, 160.0, 4800);
        let out = run(
            &mut eq,
            &[sine(100.0, -20.0, 1.0), sine(100.0, -20.0, 1.0)],
            None,
        );
        assert!(tail_peak(&out[0], 0.5) < -30.0);
    }

    fn compressor(threshold: f64, ratio: f64, attack: f64, release: f64) -> ProcKind {
        ProcKind::Compressor {
            threshold_db: threshold,
            ratio,
            knee_db: 0.0,
            attack_ms: attack,
            release_ms: release,
            makeup_db: 0.0,
            detector: Detector::Peak,
            sidechain_hpf_hz: None,
            key: None,
        }
    }

    #[test]
    fn compressor_static_curve_and_attack_time() {
        // −10 dBFS sine, threshold −20, 4:1: the peak settles at −17.5 dBFS.
        let mut c = proc(compressor(-20.0, 4.0, 0.1, 1000.0), 2);
        let x = sine(1000.0, -10.0, 2.0);
        let out = run(&mut c, &[x.clone(), x], None);
        assert!(
            (tail_peak(&out[0], 0.5) + 17.5).abs() < 0.2,
            "{}",
            tail_peak(&out[0], 0.5)
        );
        // Attack 10 ms: a step from silence to 0 dBFS (DC) reaches 63 % of the
        // final −15 dB reduction after 10 ms.
        let mut c = proc(compressor(-20.0, 4.0, 10.0, 100.0), 1);
        let mut step = vec![0.0f32; 4800];
        step.extend(vec![1.0f32; 9600]);
        let out = run(&mut c, &[step], None);
        let gr = 20.0 * f64::from(out[0][4800 + 480]).log10();
        assert!((gr + 15.0 * (1.0 - (-1f64).exp())).abs() < 0.3, "{gr}");
        assert!((20.0 * f64::from(out[0][14_000]).log10() + 15.0).abs() < 0.05);
    }

    #[test]
    fn limiter_holds_the_ceiling_with_its_latency() {
        let kind = ProcKind::Limiter {
            ceiling_db: -1.0,
            release_ms: 50.0,
            lookahead_ms: 1.5,
            true_peak: false,
        };
        let mut l = proc(kind.clone(), 1);
        assert_eq!(l.latency, 72);
        let x: Vec<f32> = (0..48_000)
            .map(|k| ((k as f64 * 0.37).sin() * if k % 4800 < 2400 { 0.2 } else { 3.0 }) as f32)
            .collect();
        let out = run(&mut l, &[x], None);
        let max = out[0].iter().fold(0f32, |m, v| m.max(v.abs()));
        assert!(f64::from(max) <= 10f64.powf(-1.0 / 20.0) + 1e-6, "{max}");
        // A quiet impulse passes unchanged, 72 samples later.
        let mut l = proc(kind, 1);
        let mut imp = vec![0.0f32; 960];
        imp[100] = 0.5;
        let out = run(&mut l, &[imp], None);
        assert_eq!(out[0][172], 0.5);
    }

    #[test]
    fn gate_expander_and_ducker() {
        let gate = ProcKind::Gate {
            threshold_db: -40.0,
            hysteresis_db: 6.0,
            attack_ms: 0.5,
            hold_ms: 10.0,
            release_ms: 20.0,
            range_db: 60.0,
        };
        let mut g = proc(gate.clone(), 1);
        let out = run(&mut g, &[sine(500.0, -60.0, 1.0)], None);
        assert!((tail_peak(&out[0], 0.3) + 120.0).abs() < 0.5);
        let mut g = proc(gate, 1);
        let out = run(&mut g, &[sine(500.0, -20.0, 1.0)], None);
        assert!((tail_peak(&out[0], 0.3) + 20.0).abs() < 0.05);

        let mut e = proc(
            ProcKind::Expander {
                threshold_db: -30.0,
                ratio: 2.0,
                range_db: 40.0,
                attack_ms: 0.1,
                release_ms: 1000.0,
            },
            1,
        );
        let out = run(&mut e, &[sine(500.0, -40.0, 2.0)], None);
        assert!(
            (tail_peak(&out[0], 0.3) + 50.0).abs() < 0.5,
            "{}",
            tail_peak(&out[0], 0.3)
        );

        let ducker = ProcKind::Ducker {
            key: Key {
                signal: "vox".into(),
                tap: Tap::PostFader,
            },
            threshold_db: -30.0,
            depth_db: 12.0,
            attack_ms: 5.0,
            hold_ms: 100.0,
            release_ms: 200.0,
        };
        let mut d = proc(ducker, 1);
        let music = sine(300.0, -10.0, 2.0);
        let voice = sine(1000.0, -20.0, 2.0);
        let out = run(&mut d, &[music], Some(&[voice]));
        assert!((tail_peak(&out[0], 0.5) + 22.0).abs() < 0.1);
    }

    #[test]
    fn de_esser_reduces_only_the_sibilant_band() {
        for mode in [DeEsserMode::SplitBand, DeEsserMode::WideBand] {
            let kind = ProcKind::DeEsser {
                mode,
                freq_hz: 5000.0,
                threshold_db: -30.0,
                range_db: 6.0,
                attack_ms: 0.5,
                release_ms: 100.0,
                listen: false,
            };
            let mut d = proc(kind.clone(), 1);
            let out = run(&mut d, &[sine(9000.0, -10.0, 1.0)], None);
            let p = tail_peak(&out[0], 0.3);
            assert!((p + 16.0).abs() < 0.6, "{mode:?} sibilance: {p}");
            let mut d = proc(kind, 1);
            let out = run(&mut d, &[sine(300.0, -10.0, 1.0)], None);
            assert!((tail_peak(&out[0], 0.3) + 10.0).abs() < 0.1, "{mode:?} low");
        }
    }

    #[test]
    fn multiband_is_flat_at_unity_and_compresses_one_band() {
        let band = |t: f64| MultibandBand {
            threshold_db: t,
            ratio: 4.0,
            attack_ms: 0.1,
            release_ms: 1000.0,
            makeup_db: 0.0,
        };
        let kind = |t: f64| ProcKind::Multiband {
            crossovers_hz: vec![200.0, 2000.0, 8000.0],
            bands: vec![band(0.0), band(t), band(0.0), band(0.0)],
        };
        // RMS, not the sampled peak: the all-pass phase moves the peak between samples.
        let rms_db = |x: &[f32]| {
            let t = &x[x.len() - 14_400..];
            10.0 * (t.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / t.len() as f64).log10()
        };
        for f in [50.0, 200.0, 700.0, 2000.0, 5000.0, 12_000.0] {
            let mut m = proc(kind(0.0), 1);
            let x = sine(f, -20.0, 1.0);
            let out = run(&mut m, std::slice::from_ref(&x), None);
            let diff = rms_db(&out[0]) - rms_db(&x);
            assert!(diff.abs() < 0.01, "{f} Hz: {diff} dB");
        }
        // Band 2 (200–2000 Hz) at threshold −40: a −20 dBFS 700 Hz sine drops by
        // 15 dB in that band. The neighbouring bands pass their share of it (−37 dB
        // and less, in phase) uncompressed, which adds about 0.7 dB.
        let mut m = proc(kind(-40.0), 1);
        let out = run(&mut m, &[sine(700.0, -20.0, 2.0)], None);
        assert!(
            (tail_peak(&out[0], 0.3) + 34.3).abs() < 0.5,
            "{}",
            tail_peak(&out[0], 0.3)
        );
        // Every band's gain reduction goes to the node meter (B2): band 2 about
        // 15 dB, the others none.
        let mut m = proc(kind(-40.0), 1);
        m.meter = 0;
        let meters = [NodeMeter::default()];
        let x = sine(700.0, -20.0, 1.0);
        let mut io = x.clone();
        for block in io.chunks_exact_mut(B) {
            p_block(&mut m, block, &meters);
        }
        let gr: Vec<f32> = meters[0]
            .bands
            .iter()
            .map(|a| f32::from_bits(a.load(Ordering::Relaxed)))
            .collect();
        assert!((gr[1] - 15.0).abs() < 1.0, "{gr:?}");
        assert!(gr[0] < 0.5 && gr[2] < 0.5 && gr[3] < 0.5, "{gr:?}");
        assert!(gr[4..].iter().all(|g| *g == 0.0));
    }

    fn p_block(p: &mut Processor, block: &mut [f32], meters: &[NodeMeter]) {
        let mut io = [block];
        p.process(&mut io, &[], B, meters);
    }

    #[test]
    fn dynamic_eq_cuts_a_loud_band() {
        let kind = ProcKind::DynamicEq {
            bands: vec![DynamicBand {
                freq_hz: 3000.0,
                q: 2.0,
                gain_db: 0.0,
                threshold_db: -30.0,
                ratio: 4.0,
                attack_ms: 1.0,
                release_ms: 200.0,
            }],
        };
        let mut d = proc(kind.clone(), 1);
        let out = run(&mut d, &[sine(3000.0, -10.0, 1.0)], None);
        assert!(
            (tail_peak(&out[0], 0.3) + 25.0).abs() < 1.0,
            "{}",
            tail_peak(&out[0], 0.3)
        );
        let mut d = proc(kind, 1);
        let out = run(&mut d, &[sine(3000.0, -40.0, 1.0)], None);
        assert!((tail_peak(&out[0], 0.3) + 40.0).abs() < 0.05);
    }

    #[test]
    fn delay_and_bypass() {
        let mut d = proc(
            ProcKind::Delay {
                time: 250.0,
                unit: crate::mix::chain::DelayUnit::Samples,
                frame_rate: None,
            },
            1,
        );
        let mut imp = vec![0.0f32; 960];
        imp[10] = 1.0;
        let out = run(&mut d, &[imp], None);
        assert_eq!(out[0][260], 1.0);
        // A bypassed limiter passes the input, delayed by its latency.
        let kind = ProcKind::Limiter {
            ceiling_db: -20.0,
            release_ms: 50.0,
            lookahead_ms: 1.0,
            true_peak: true,
        };
        let mut values = vec![0.0];
        values.extend(kind.values());
        let mut l = Processor::new(0, &kind, &values, 1, B, usize::MAX);
        let x = sine(1000.0, -3.0, 0.5);
        let out = run(&mut l, std::slice::from_ref(&x), None);
        for k in 1000..20_000 {
            assert!((out[0][k] - x[k - 48]).abs() < 1e-6);
        }
        // Switching it in crossfades within one block, then limits.
        l.set_param(0, 1.0, 0);
        let out = run(&mut l, &[x], None);
        assert!(tail_peak(&out[0], 0.2) < -19.9);
    }

    #[test]
    fn state_moves_to_the_next_plan() {
        let kind = compressor(-20.0, 4.0, 0.1, 1000.0);
        let mut old = proc(kind.clone(), 1);
        let x = sine(1000.0, -10.0, 1.0);
        run(&mut old, std::slice::from_ref(&x), None);
        let mut new = proc(kind, 1);
        new.adopt(&mut old, 480);
        // Already compressed from the first sample: no overshoot after the swap.
        let out = run(&mut new, &[x], None);
        assert!(
            20.0 * f64::from(out[0][..960].iter().fold(0f32, |m, v| m.max(v.abs()))).log10()
                < -17.3
        );
    }
}
