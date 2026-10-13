//! The default voice detector (§8.1): Silero VAD, a small recurrent network
//! (`assets/silero_vad.onnx`, MIT, Silero Team; see `assets/silero_vad.LICENSE`),
//! run with ONNX Runtime as Com-IS-Assist does.
//!
//! The 48 kHz frames are decimated to 16 kHz (a 95-tap windowed-sinc low-pass at
//! 7.2 kHz, Blackman), fed to the model in its 512-sample (32 ms) chunks with the
//! 64 samples of context before each and the recurrent state it carries; a frame's
//! probability is that of the last chunk completed. One session serves every
//! signal (the state is each detector's own), with one thread, so inference runs
//! on the control side's thread and never next to the DSP thread.

use std::sync::{Mutex, OnceLock};

use ort::session::Session;
use ort::value::Tensor;

use super::vad::{Detector, Frame, FRAME};

const MODEL: &[u8] = include_bytes!("../../assets/silero_vad.onnx");
const RATE: i64 = 16_000;
const CHUNK: usize = 512;
const CONTEXT: usize = 64;
const STATE: usize = 2 * 128;
const DECIMATE: usize = 3;
const TAPS: usize = 95;

struct Model {
    session: Mutex<Session>,
    _environment: ort::environment::Environment,
}

static MODEL_SESSION: OnceLock<Result<Model, String>> = OnceLock::new();

fn model() -> &'static Result<Model, String> {
    MODEL_SESSION.get_or_init(|| {
        let build = || -> ort::Result<Model> {
            let environment = ort::init().with_name("mxl-audio-mixer").build()?;
            let session = Session::builder(&environment)?
                .with_intra_threads(1)?
                .with_inter_threads(1)?
                .commit_from_memory(MODEL)?;
            Ok(Model {
                session: Mutex::new(session),
                _environment: environment,
            })
        };
        build().map_err(|e| e.to_string())
    })
}

/// Whether the model can run here (ONNX Runtime loaded it).
pub fn available() -> Result<(), String> {
    model().as_ref().map(|_| ()).map_err(Clone::clone)
}

fn low_pass() -> Vec<f32> {
    let fc = 7_200.0 / 48_000.0;
    let m = (TAPS - 1) as f64;
    let h: Vec<f64> = (0..TAPS)
        .map(|n| {
            let x = n as f64 - m / 2.0;
            let sinc = if x == 0.0 {
                2.0 * fc
            } else {
                (2.0 * std::f64::consts::PI * fc * x).sin() / (std::f64::consts::PI * x)
            };
            let t = 2.0 * std::f64::consts::PI * n as f64 / m;
            sinc * (0.42 - 0.5 * t.cos() + 0.08 * (2.0 * t).cos())
        })
        .collect();
    let sum: f64 = h.iter().sum();
    h.iter().map(|v| (v / sum) as f32).collect()
}

pub struct SileroDetector {
    taps: Vec<f32>,
    /// The last `TAPS` samples twice, so that they are one slice from `at`.
    history: Vec<f32>,
    at: usize,
    phase: usize,
    pending: Vec<f32>,
    context: Vec<f32>,
    state: Vec<f32>,
    probability: f32,
}

impl SileroDetector {
    /// A detector, or why the model cannot run.
    pub fn new() -> Result<Self, String> {
        available()?;
        Ok(Self {
            taps: low_pass(),
            history: vec![0.0; 2 * TAPS],
            at: 0,
            phase: 0,
            pending: Vec::with_capacity(CHUNK + FRAME / DECIMATE),
            context: vec![0.0; CONTEXT],
            state: vec![0.0; STATE],
            probability: 0.0,
        })
    }

    fn infer(&mut self) -> Result<f32, String> {
        let Ok(model) = model() else {
            return Err("no model".into());
        };
        let mut session = model.session.lock().unwrap_or_else(|e| e.into_inner());
        let mut input = Vec::with_capacity(CONTEXT + CHUNK);
        input.extend_from_slice(&self.context);
        input.extend_from_slice(&self.pending[..CHUNK]);
        self.context
            .copy_from_slice(&input[input.len() - CONTEXT..]);
        let run = |session: &mut Session, input: Vec<f32>, state: Vec<f32>| {
            let outputs = session.run(ort::inputs![
                "input" => Tensor::from_array(([1usize, CONTEXT + CHUNK], input))?,
                "state" => Tensor::from_array(([2usize, 1, 128], state))?,
                "sr" => Tensor::from_array(([1usize], vec![RATE]))?,
            ])?;
            let (_, p) = outputs[0].try_extract_tensor::<f32>()?;
            let (_, s) = outputs[1].try_extract_tensor::<f32>()?;
            Ok::<_, ort::Error>((p.first().copied().unwrap_or(0.0), s.to_vec()))
        };
        let (p, state) = run(&mut session, input, self.state.clone()).map_err(|e| e.to_string())?;
        if state.len() == STATE {
            self.state = state;
        }
        Ok(p)
    }
}

impl Detector for SileroDetector {
    fn frame(&mut self, x: &[f32]) -> Frame {
        let mut power = 0f64;
        for &v in x {
            power += f64::from(v) * f64::from(v);
            self.history[self.at] = v;
            self.history[self.at + TAPS] = v;
            self.at = (self.at + 1) % TAPS;
            self.phase += 1;
            if self.phase == DECIMATE {
                self.phase = 0;
                // The filter is symmetric: the order of the taps does not matter.
                let window = &self.history[self.at..self.at + TAPS];
                let y: f32 = window.iter().zip(&self.taps).map(|(a, b)| a * b).sum();
                self.pending.push(y);
            }
        }
        while self.pending.len() >= CHUNK {
            match self.infer() {
                Ok(p) => self.probability = p,
                Err(e) => {
                    tracing::warn!("Silero VAD: {e}");
                    self.probability = 0.0;
                }
            }
            self.pending.drain(..CHUNK);
        }
        let n = x.len().max(1) as f64;
        let level = power / n;
        Frame {
            probability: f64::from(self.probability),
            level_dbfs: if level > 0.0 {
                (10.0 * level.log10()).max(-120.0)
            } else {
                -120.0
            },
            features: [0.0; 4],
        }
    }

    fn reset(&mut self) {
        self.history.fill(0.0);
        self.at = 0;
        self.phase = 0;
        self.pending.clear();
        self.context.fill(0.0);
        self.state.fill(0.0);
        self.probability = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auto::vad::tests::{pink, sine};

    fn probabilities(x: &[f32]) -> Vec<f64> {
        let mut d = SileroDetector::new().expect("the model loads");
        x.chunks_exact(FRAME)
            .map(|c| d.frame(c).probability)
            .collect()
    }

    #[test]
    fn the_model_runs_and_hears_no_speech_in_silence_noise_or_tones() {
        for (what, x) in [
            ("silence", vec![0.0; 96_000]),
            ("pink −30 dBFS", pink(3.0, 0.0316, 3)),
            ("1 kHz", sine(3.0, 1_000.0, 0.18)),
            ("200 Hz", sine(3.0, 200.0, 0.18)),
        ] {
            let p = probabilities(&x);
            let max = p.iter().copied().fold(0.0, f64::max);
            assert!(max < 0.5, "{what}: probability up to {max:.2}");
        }
    }

    #[test]
    fn the_decimator_passes_speech_and_stops_aliases() {
        let h = low_pass();
        let gain = |f: f64| {
            let (mut re, mut im) = (0.0, 0.0);
            for (n, v) in h.iter().enumerate() {
                let w = 2.0 * std::f64::consts::PI * f / 48_000.0 * n as f64;
                re += f64::from(*v) * w.cos();
                im -= f64::from(*v) * w.sin();
            }
            20.0 * (re * re + im * im).sqrt().log10()
        };
        assert!(gain(1_000.0).abs() < 0.1 && gain(5_000.0).abs() < 0.5);
        assert!(
            gain(9_000.0) < -60.0 && gain(12_000.0) < -60.0,
            "{}",
            gain(9_000.0)
        );
    }
}
