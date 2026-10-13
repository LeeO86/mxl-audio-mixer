//! A tapped signal's loudness on the control side (BS.1770-4), per 10 ms frame:
//! every channel K-weighted, the channels weighted by their labels (LFE 0, the
//! surrounds 1.41), then
//! - momentary loudness: the last 400 ms (the VAD's gate, a ratio duck's mid
//!   stage);
//! - short-term loudness: the last 3 s (a ratio duck's slow stage and dialogue
//!   reference, the auto-level);
//! - fast loudness: an exponential mean square with a time constant of 50 ms
//!   (Com-IS-Assist's fast detector, for a ratio duck's fast stage: on the same
//!   scale as the others, so its trim is meaningful).
//!
//! Before 400 ms or 3 s of frames the missing ones count as silence.

use std::collections::VecDeque;

use crate::dsp::biquad::State;
use crate::dsp::loudness::{K_HIGH_PASS, K_SHELF};

use super::comis::SILENCE_LUFS;

const MOMENTARY: usize = 40;
const SHORT_TERM: usize = 300;
/// The fast detector's time constant, s.
const FAST_SECONDS: f64 = 0.05;

/// LUFS of a weighted mean square, at the silence floor for none.
pub fn lufs(power: f64) -> f64 {
    if power > 0.0 {
        (-0.691 + 10.0 * power.log10()).max(SILENCE_LUFS)
    } else {
        SILENCE_LUFS
    }
}

/// One frame's readings, LUFS.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reading {
    /// The frame's weighted mean square (K-weighted).
    pub power: f64,
    pub momentary: f64,
    pub short_term: f64,
    pub fast: f64,
}

impl Default for Reading {
    fn default() -> Self {
        Self {
            power: 0.0,
            momentary: SILENCE_LUFS,
            short_term: SILENCE_LUFS,
            fast: SILENCE_LUFS,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Level {
    weights: Vec<f64>,
    filters: Vec<[State; 2]>,
    fast: Vec<f64>,
    fast_coeff: f64,
    frames: VecDeque<f64>,
}

impl Level {
    /// A level for channels weighted `weights` (`dsp::loudness::weights`).
    pub fn new(weights: Vec<f64>) -> Self {
        let n = weights.len();
        Self {
            weights,
            filters: vec![[State::default(); 2]; n],
            fast: vec![0.0; n],
            fast_coeff: (-1.0 / (FAST_SECONDS * 48_000.0)).exp(),
            frames: VecDeque::with_capacity(SHORT_TERM),
        }
    }

    pub fn channels(&self) -> usize {
        self.weights.len()
    }

    /// One frame, per channel (as many as the weights; the rest is ignored).
    pub fn frame(&mut self, chans: &[&[f32]]) -> Reading {
        let a = self.fast_coeff;
        let mut power = 0.0;
        let mut fast = 0.0;
        for (c, x) in chans.iter().enumerate().take(self.weights.len()) {
            let [shelf, hp] = &mut self.filters[c];
            let mut ms = self.fast[c];
            let mut energy = 0.0;
            for &v in x.iter() {
                let v = f64::from(v);
                let y = hp.tick(
                    &K_HIGH_PASS,
                    shelf.tick(&K_SHELF, if v.is_finite() { v } else { 0.0 }),
                );
                let y2 = y * y;
                energy += y2;
                ms = ms * a + y2 * (1.0 - a);
            }
            self.fast[c] = ms;
            let w = self.weights[c];
            power += w * energy / x.len().max(1) as f64;
            fast += w * ms;
        }
        if self.frames.len() == SHORT_TERM {
            self.frames.pop_front();
        }
        self.frames.push_back(power);
        let mean = |n: usize| self.frames.iter().rev().take(n).sum::<f64>() / n as f64;
        Reading {
            power,
            momentary: lufs(mean(MOMENTARY)),
            short_term: lufs(mean(SHORT_TERM)),
            fast: lufs(fast),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::loudness::weights;
    use std::f64::consts::PI;

    fn sine(n: usize, freq: f64, peak: f64) -> Vec<f32> {
        (0..n)
            .map(|k| (peak * (2.0 * PI * freq * k as f64 / 48_000.0).sin()) as f32)
            .collect()
    }

    #[test]
    fn a_1k_sine_reads_as_bs1770_says() {
        // EBU Tech 3341 case 1: 1 kHz at −23 dBFS in both channels is −23 LUFS.
        let x = sine(48_000 * 4, 1000.0, 10f64.powf(-23.0 / 20.0));
        let mut l = Level::new(weights(&["L", "R"]));
        let mut r = Reading::default();
        for f in x.chunks_exact(480) {
            r = l.frame(&[f, f]);
        }
        assert!((r.short_term + 23.0).abs() < 0.05, "{r:?}");
        assert!((r.momentary + 23.0).abs() < 0.05, "{r:?}");
        assert!((r.fast + 23.0).abs() < 0.1, "{r:?}");
        // The LFE does not count, a surround counts 1.41 (+1.5 dB).
        let mut lfe = Level::new(weights(&["LFE"]));
        let mut ls = Level::new(weights(&["Ls"]));
        let (mut a, mut b) = (Reading::default(), Reading::default());
        for f in x.chunks_exact(480) {
            a = lfe.frame(&[f]);
            b = ls.frame(&[f]);
        }
        assert_eq!(a.short_term, SILENCE_LUFS);
        assert!(
            (b.short_term - (-26.01 + 10.0 * 1.41f64.log10())).abs() < 0.05,
            "{b:?}"
        );
    }

    /// `cargo test --release --lib level_cost -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn level_cost() {
        let x = sine(480, 1000.0, 0.1);
        let mut l = Level::new(weights(&["L", "R"]));
        let n = 20_000;
        let start = std::time::Instant::now();
        let mut sum = 0.0;
        for _ in 0..n {
            sum += l.frame(&[&x, &x]).short_term;
        }
        let per = start.elapsed().as_nanos() as f64 / f64::from(n);
        println!(
            "a stereo signal's loudness: {per:.0} ns per 10 ms frame, {:.2} % of a core ({sum:.0})",
            per / 1e5
        );
    }

    #[test]
    fn windows_and_the_fast_detector() {
        let tone = sine(480 * 400, 1000.0, 0.1);
        let silence = vec![0.0f32; 480 * 400];
        let mut l = Level::new(weights(&["M"]));
        let mut trace = Vec::new();
        for f in tone.chunks_exact(480).chain(silence.chunks_exact(480)) {
            trace.push(l.frame(&[f]));
        }
        let steady = trace[399];
        // 50 ms into the tone the fast detector is near the level, the
        // momentary 9 dB below it (50 of 400 ms), the short-term 18 dB.
        let early = trace[4];
        assert!(steady.short_term - early.fast < 3.0, "{early:?} {steady:?}");
        assert!((steady.momentary - early.momentary - 9.0).abs() < 0.3);
        assert!((steady.short_term - early.short_term - 18.0).abs() < 0.3);
        // After the tone: the momentary falls to the filters' tail after
        // 400 ms, the short-term after 3 s.
        assert!((trace[400 + 19].momentary - steady.momentary + 3.01).abs() < 0.1);
        assert!(trace[400 + 40].momentary < -90.0);
        assert!(trace[400 + 40].short_term > -40.0);
        assert!(trace[400 + 300].short_term < -90.0);
    }
}
