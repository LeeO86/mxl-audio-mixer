//! The measured round trip of an insert (C4, 1.6.0): how much later its return
//! brings back what its send sent, found on the meter thread by cross-
//! correlating the two (they are metered blocks anyway). Both are summed to
//! mono and decimated by [`DECIMATE`] (a box average; 6 kHz at 48 kHz); about
//! once a second the last [`WINDOW`] return samples are compared with the send
//! at every lag up to [`MAX_LAG_MS`], and the lag of the best normalised
//! correlation (with a parabolic refinement) is the round trip, when that
//! correlation is clear (≥ [`MIN_CORRELATION`]). Otherwise the last value stays.
//! No delay compensation follows from it (Adrian, 2026-11-05): it is shown.

use std::sync::atomic::{AtomicU32, Ordering};

/// Samples averaged into one.
pub const DECIMATE: usize = 8;
/// Longest round trip measured.
pub const MAX_LAG_MS: usize = 500;
const MAX_LAG: usize = MAX_LAG_MS * 48 / DECIMATE;
/// Return samples (decimated) compared per measurement (≈ 340 ms).
const WINDOW: usize = 2048;
const HISTORY: usize = MAX_LAG + WINDOW;
/// Decimated samples between two measurements (≈ 1 s).
const EVERY: usize = 6000;
/// Below this normalised correlation the lag is not trusted.
pub const MIN_CORRELATION: f64 = 0.5;

/// A probe of the meter bank: the meter slots of the send and the return and
/// the result (samples + 1; 0: not measured yet), written by the meter thread.
#[derive(Debug, Default)]
pub struct Probe {
    pub send: u32,
    pub ret: u32,
    pub lag: AtomicU32,
}

impl Probe {
    pub fn new(send: u32, ret: u32) -> Self {
        Self {
            send,
            ret,
            lag: AtomicU32::new(0),
        }
    }

    /// The measured round trip in samples, if any.
    pub fn samples(&self) -> Option<u32> {
        self.lag.load(Ordering::Relaxed).checked_sub(1)
    }
}

/// The meter thread's state of one probe.
pub struct Correlator {
    send: Vec<f32>,
    ret: Vec<f32>,
    /// Decimated samples written (each ring at `n % HISTORY`).
    sent: usize,
    returned: usize,
    acc: [(f32, usize); 2],
    since: usize,
}

impl Default for Correlator {
    fn default() -> Self {
        Self {
            send: vec![0.0; HISTORY],
            ret: vec![0.0; HISTORY],
            sent: 0,
            returned: 0,
            acc: [(0.0, 0); 2],
            since: 0,
        }
    }
}

impl Correlator {
    /// One block of the send (`ret` false) or the return, its channels.
    pub fn feed(&mut self, ret: bool, chans: &[&[f32]]) {
        let len = chans.first().map_or(0, |c| c.len());
        let k = usize::from(ret);
        for i in 0..len {
            let x: f32 = chans.iter().map(|c| c[i]).sum();
            let (sum, n) = &mut self.acc[k];
            *sum += x;
            *n += 1;
            if *n == DECIMATE {
                let v = *sum / DECIMATE as f32;
                *sum = 0.0;
                *n = 0;
                if ret {
                    self.ret[self.returned % HISTORY] = v;
                    self.returned += 1;
                    self.since += 1;
                } else {
                    self.send[self.sent % HISTORY] = v;
                    self.sent += 1;
                }
            }
        }
    }

    /// Measure when due; the round trip in samples when it is clear.
    pub fn measure(&mut self) -> Option<f64> {
        if self.since < EVERY || self.returned < HISTORY || self.sent < HISTORY {
            return None;
        }
        self.since = 0;
        // Newest first: the return's window and the send's history, from the
        // same block on.
        let end = self.returned.min(self.sent);
        let r: Vec<f32> = (0..WINDOW)
            .map(|i| self.ret[(end - 1 - i) % HISTORY])
            .collect();
        let s: Vec<f32> = (0..HISTORY)
            .map(|i| self.send[(end - 1 - i) % HISTORY])
            .collect();
        let sq = |x: &[f32]| x.iter().map(|v| f64::from(*v) * f64::from(*v)).sum::<f64>();
        let energy_r = sq(&r);
        if energy_r < 1e-12 {
            return None;
        }
        // The send's energy over the window at each lag, sliding.
        let mut energy_s = sq(&s[..WINDOW]);
        let mut best = (0usize, f64::MIN);
        let mut scores = vec![0.0; MAX_LAG];
        for (lag, score) in scores.iter_mut().enumerate() {
            if lag > 0 {
                let (out, new) = (f64::from(s[lag - 1]), f64::from(s[lag + WINDOW - 1]));
                energy_s += new * new - out * out;
            }
            let dot: f32 = r
                .iter()
                .zip(&s[lag..lag + WINDOW])
                .map(|(a, b)| a * b)
                .sum();
            *score = f64::from(dot) / (energy_r * energy_s.max(1e-24)).sqrt();
            if *score > best.1 {
                best = (lag, *score);
            }
        }
        if best.1 < MIN_CORRELATION {
            return None;
        }
        let (lag, peak) = best;
        let refine = if lag > 0 && lag + 1 < MAX_LAG {
            let (a, c) = (scores[lag - 1], scores[lag + 1]);
            let d = a - 2.0 * peak + c;
            if d.abs() > 1e-12 {
                (0.5 * (a - c) / d).clamp(-0.5, 0.5)
            } else {
                0.0
            }
        } else {
            0.0
        };
        Some((lag as f64 + refine) * DECIMATE as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Noise that repeats only after a long time.
    fn noise(seed: &mut u32) -> f32 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 17;
        *seed ^= *seed << 5;
        (*seed as f32 / u32::MAX as f32) - 0.5
    }

    #[test]
    fn finds_a_delayed_return_and_ignores_unrelated_audio() {
        for delay in [0usize, 96, 1000, 7200] {
            let mut c = Correlator::default();
            let mut seed = 1;
            let mut line = vec![0.0f32; delay];
            let mut got = None;
            for _ in 0..2000 {
                let block: Vec<f32> = (0..96).map(|_| noise(&mut seed)).collect();
                line.extend_from_slice(&block);
                let back: Vec<f32> = line.drain(..96).map(|x| 0.5 * x).collect();
                c.feed(false, &[&block]);
                c.feed(true, &[&back, &back]);
                if let Some(l) = c.measure() {
                    got = Some(l);
                }
            }
            let got = got.expect("measured");
            assert!((got - delay as f64).abs() <= 8.0, "{delay}: {got}");
        }
        // A return that is something else: nothing.
        let mut c = Correlator::default();
        let (mut a, mut b) = (1, 99);
        for _ in 0..2000 {
            let x: Vec<f32> = (0..96).map(|_| noise(&mut a)).collect();
            let y: Vec<f32> = (0..96).map(|_| noise(&mut b)).collect();
            c.feed(false, &[&x]);
            c.feed(true, &[&y]);
            assert_eq!(c.measure(), None);
        }
    }
}
