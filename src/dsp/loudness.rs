//! Loudness (§7): ITU-R BS.1770-4 and EBU R 128 / Tech 3341 / Tech 3342.
//!
//! The DSP side ([`SegmentMeter`]) K-weights every channel and sends the energy of
//! each 100 ms segment (4800 samples, aligned to the sample index). The control
//! side ([`Loudness`]) derives:
//! - momentary loudness: the last 4 segments (400 ms), ungated;
//! - short-term loudness: the last 30 segments (3 s), ungated;
//! - integrated loudness: 400 ms blocks every 100 ms (75 % overlap), absolute
//!   gate −70 LUFS, relative gate −10 LU;
//! - loudness range (Tech 3342): short-term values every 100 ms, absolute gate
//!   −70 LUFS, relative gate −20 LU, 95th minus 10th percentile.

use crate::dsp::biquad::{Coefficients, State};

/// Samples per segment (100 ms at 48 kHz).
pub const SEGMENT: u64 = 4800;
pub const MAX_CHANNELS: usize = crate::mix::format::MAX_FORMAT_CHANNELS;

/// BS.1770-4 K-weighting at 48 kHz: the shelving pre-filter and the RLB high-pass.
pub const K_SHELF: Coefficients = Coefficients {
    b0: 1.535_124_859_586_97,
    b1: -2.691_696_189_406_38,
    b2: 1.198_392_810_852_85,
    a1: -1.690_659_293_182_41,
    a2: 0.732_480_774_215_85,
};
pub const K_HIGH_PASS: Coefficients = Coefficients {
    b0: 1.0,
    b1: -2.0,
    b2: 1.0,
    a1: -1.990_047_454_833_98,
    a2: 0.990_072_250_366_21,
};

/// The energy (sum of squared K-weighted samples) of one segment per channel.
#[derive(Debug, Clone, Copy)]
pub struct Segment {
    /// Segment number: the segment covers samples [n·4800, (n+1)·4800).
    pub index: u64,
    pub energy: [f64; MAX_CHANNELS],
}

/// The DSP-side part: K-weighting and segment energies for one signal.
pub struct SegmentMeter {
    channels: usize,
    filters: Vec<[State; 2]>,
    acc: [f64; MAX_CHANNELS],
    done: [f64; MAX_CHANNELS],
    /// The segment being summed, when its first sample was seen.
    current: Option<u64>,
    /// The sample index the next block should start at.
    next: Option<u64>,
    out: rtrb::Producer<Segment>,
}

impl SegmentMeter {
    /// A meter for `channels` channels and the consumer the control side reads.
    pub fn new(channels: usize) -> (Self, rtrb::Consumer<Segment>) {
        let (out, rx) = rtrb::RingBuffer::new(256);
        (
            Self {
                channels: channels.min(MAX_CHANNELS),
                filters: vec![[State::default(); 2]; channels.min(MAX_CHANNELS)],
                acc: [0.0; MAX_CHANNELS],
                done: [0.0; MAX_CHANNELS],
                current: None,
                next: None,
                out,
            },
            rx,
        )
    }

    /// Feed channel `channel` of the block starting at sample index `start`. A
    /// segment is sent after its last sample of the last channel arrived; the
    /// partial first segment after a (re)start is not sent.
    pub fn feed(&mut self, start: u64, channel: usize, samples: &[f32]) {
        if channel >= self.channels {
            return;
        }
        let seg = start / SEGMENT;
        if channel == 0 && self.next != Some(start) {
            // A (re)start or a gap: the segment counts only from its first sample.
            self.acc = [0.0; MAX_CHANNELS];
            self.current = (start % SEGMENT == 0).then_some(seg);
        }
        let boundary = ((seg + 1) * SEGMENT - start) as usize;
        let [shelf, hp] = &mut self.filters[channel];
        let mut acc = self.acc[channel];
        for (k, &x) in samples.iter().enumerate() {
            if k == boundary {
                self.done[channel] = acc;
                acc = 0.0;
            }
            let y = hp.tick(&K_HIGH_PASS, shelf.tick(&K_SHELF, f64::from(x)));
            acc += y * y;
        }
        if boundary == samples.len() {
            self.done[channel] = acc;
            acc = 0.0;
        }
        self.acc[channel] = acc;
        if channel + 1 < self.channels {
            return;
        }
        self.next = Some(start + samples.len() as u64);
        if boundary <= samples.len() {
            if self.current == Some(seg) {
                let _ = self.out.push(Segment {
                    index: seg,
                    energy: self.done,
                });
            }
            self.current = Some(seg + 1);
        }
    }
}

/// BS.1770-4 channel weights: 1.41 for the surround channels at ±60°…±120°
/// (Ls, Rs, Lss, Rss), 0 for LFE, 1.0 for the rest.
pub fn weights(labels: &[&str]) -> Vec<f64> {
    labels
        .iter()
        .map(|l| match *l {
            "LFE" => 0.0,
            "Ls" | "Rs" | "Lss" | "Rss" => 1.41,
            _ => 1.0,
        })
        .collect()
}

fn lufs(power: f64) -> f64 {
    if power > 0.0 {
        -0.691 + 10.0 * power.log10()
    } else {
        f64::NEG_INFINITY
    }
}

fn power(lufs: f64) -> f64 {
    10f64.powf((lufs + 0.691) / 10.0)
}

/// The control-side measurement of one signal.
#[derive(Debug, Clone, Default)]
pub struct Loudness {
    weights: Vec<f64>,
    recent: std::collections::VecDeque<f64>,
    last: Option<u64>,
    pub running: bool,
    blocks: Vec<f64>,
    short_terms: Vec<f64>,
    pub max_true_peak: f32,
}

/// The values of a measurement, in LUFS (LRA in LU, true peak in dBTP).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Values {
    pub momentary: f64,
    pub short_term: f64,
    pub integrated: f64,
    pub range: f64,
}

impl Loudness {
    pub fn new(weights: Vec<f64>) -> Self {
        Self {
            weights,
            running: true,
            ..Default::default()
        }
    }

    pub fn reset(&mut self) {
        self.blocks.clear();
        self.short_terms.clear();
        self.max_true_peak = 0.0;
    }

    /// Take one segment. A gap in the segment numbers restarts the windows.
    pub fn push(&mut self, s: &Segment) {
        if self.last.is_some_and(|l| s.index != l + 1) {
            self.recent.clear();
        }
        self.last = Some(s.index);
        let p: f64 = self
            .weights
            .iter()
            .zip(s.energy.iter())
            .map(|(g, e)| g * e / SEGMENT as f64)
            .sum();
        self.recent.push_back(p);
        while self.recent.len() > 30 {
            self.recent.pop_front();
        }
        if !self.running {
            return;
        }
        if self.recent.len() >= 4 {
            self.blocks.push(self.mean(4));
        }
        if self.recent.len() >= 30 {
            self.short_terms.push(self.mean(30));
        }
    }

    fn mean(&self, n: usize) -> f64 {
        let take = n.min(self.recent.len());
        if take < n {
            return 0.0;
        }
        self.recent.iter().rev().take(take).sum::<f64>() / take as f64
    }

    pub fn values(&self) -> Values {
        Values {
            momentary: lufs(self.mean(4)),
            short_term: lufs(self.mean(30)),
            integrated: integrated(&self.blocks),
            range: range(&self.short_terms),
        }
    }
}

/// Gated integrated loudness of 400 ms block powers.
pub fn integrated(blocks: &[f64]) -> f64 {
    let abs: Vec<f64> = blocks
        .iter()
        .copied()
        .filter(|p| lufs(*p) > -70.0)
        .collect();
    if abs.is_empty() {
        return f64::NEG_INFINITY;
    }
    let gate = lufs(abs.iter().sum::<f64>() / abs.len() as f64) - 10.0;
    let rel: Vec<f64> = abs.into_iter().filter(|p| lufs(*p) > gate).collect();
    if rel.is_empty() {
        return f64::NEG_INFINITY;
    }
    lufs(rel.iter().sum::<f64>() / rel.len() as f64)
}

/// Loudness range (EBU Tech 3342) of short-term powers.
pub fn range(short_terms: &[f64]) -> f64 {
    let abs: Vec<f64> = short_terms
        .iter()
        .copied()
        .filter(|p| lufs(*p) > -70.0)
        .collect();
    if abs.is_empty() {
        return 0.0;
    }
    let gate = lufs(abs.iter().sum::<f64>() / abs.len() as f64) - 20.0;
    let mut rel: Vec<f64> = abs.into_iter().map(lufs).filter(|l| *l > gate).collect();
    if rel.len() < 2 {
        return 0.0;
    }
    rel.sort_by(f64::total_cmp);
    let pct = |q: f64| {
        let pos = q * (rel.len() - 1) as f64;
        let (lo, hi) = (pos.floor() as usize, pos.ceil() as usize);
        rel[lo] + (rel[hi] - rel[lo]) * (pos - lo as f64)
    };
    pct(0.95) - pct(0.10)
}

/// The power of `lufs`, for tests and targets.
pub fn lufs_power(l: f64) -> f64 {
    power(l)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    /// Feed `signal` (per channel) through a segment meter in 96-sample blocks,
    /// starting at sample index 0, into a measurement.
    fn measure(signal: &[Vec<f32>], labels: &[&str]) -> Loudness {
        let (mut meter, mut rx) = SegmentMeter::new(signal.len());
        let mut l = Loudness::new(weights(labels));
        let n = signal[0].len();
        let mut start = 0;
        while start + 96 <= n {
            for (c, ch) in signal.iter().enumerate() {
                meter.feed(start as u64, c, &ch[start..start + 96]);
            }
            while let Ok(s) = rx.pop() {
                l.push(&s);
            }
            start += 96;
        }
        l
    }

    /// A 1 kHz sine of `secs` seconds at each (seconds, dBFS) step, on every channel.
    fn steps(channels: usize, steps: &[(f64, f64)]) -> Vec<Vec<f32>> {
        let mut out = vec![Vec::new(); channels];
        let mut k = 0usize;
        for &(secs, dbfs) in steps {
            let a = 10f64.powf(dbfs / 20.0);
            for _ in 0..(secs * 48_000.0) as usize {
                let v = (a * (2.0 * PI * 1000.0 * k as f64 / 48_000.0).sin()) as f32;
                for ch in out.iter_mut() {
                    ch.push(v);
                }
                k += 1;
            }
        }
        out
    }

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    /// EBU Tech 3341 minimum requirements, the synthetic cases (tolerance ±0.1 LU).
    #[test]
    fn ebu_3341() {
        let st = ["L", "R"];
        // Case 1: stereo 1 kHz at −23 dBFS: M, S, I = −23.0.
        let v = measure(&steps(2, &[(20.0, -23.0)]), &st).values();
        assert!(
            close(v.momentary, -23.0, 0.1) && close(v.short_term, -23.0, 0.1),
            "{v:?}"
        );
        assert!(close(v.integrated, -23.0, 0.1), "case 1: {v:?}");
        // Case 2: −33 dBFS → −33.0.
        let v = measure(&steps(2, &[(20.0, -33.0)]), &st).values();
        assert!(close(v.integrated, -33.0, 0.1), "case 2: {v:?}");
        // Case 3: −36 / −23 / −36 dBFS, 10 / 60 / 10 s → I = −23.0 (relative gate).
        let v = measure(
            &steps(2, &[(10.0, -36.0), (60.0, -23.0), (10.0, -36.0)]),
            &st,
        )
        .values();
        assert!(close(v.integrated, -23.0, 0.1), "case 3: {v:?}");
        // Case 4: −72 / −36 / −23 / −36 / −72 → I = −23.0 (absolute gate).
        let v = measure(
            &steps(
                2,
                &[
                    (10.0, -72.0),
                    (10.0, -36.0),
                    (60.0, -23.0),
                    (10.0, -36.0),
                    (10.0, -72.0),
                ],
            ),
            &st,
        )
        .values();
        assert!(close(v.integrated, -23.0, 0.1), "case 4: {v:?}");
        // Case 5: −26 / −20 / −26, 20 / 20.1 / 20 s → I = −23.0.
        let v = measure(
            &steps(2, &[(20.0, -26.0), (20.1, -20.0), (20.0, -26.0)]),
            &st,
        )
        .values();
        assert!(close(v.integrated, -23.0, 0.1), "case 5: {v:?}");
    }

    /// Channel weights: five channels at −30 dBFS (1 kHz) read
    /// −30 − 3.01 + 10·log10(3 + 2 × 1.41); LFE does not count.
    #[test]
    fn surround_weights() {
        let five = steps(5, &[(10.0, -30.0)]);
        let v = measure(&five, &["L", "R", "C", "Ls", "Rs"]).values();
        let expected = -30.0 - 3.0103 + 10.0 * (3.0f64 + 2.0 * 1.41).log10();
        assert!(close(v.integrated, expected, 0.05), "{v:?} vs {expected}");
        let mut with_lfe = steps(2, &[(10.0, -23.0)]);
        with_lfe.extend(steps(1, &[(10.0, 0.0)]));
        let v = measure(&with_lfe, &["L", "R", "LFE"]).values();
        assert!(close(v.integrated, -23.0, 0.1), "{v:?}");
    }

    /// EBU Tech 3342 cases 1–4 (tolerance ±1 LU).
    #[test]
    fn ebu_3342() {
        let st = ["L", "R"];
        for (steps_, lra) in [
            (vec![(20.0, -20.0), (20.0, -30.0)], 10.0),
            (vec![(20.0, -20.0), (20.0, -15.0)], 5.0),
            (vec![(20.0, -40.0), (20.0, -20.0)], 20.0),
            (
                vec![
                    (20.0, -50.0),
                    (20.0, -35.0),
                    (20.0, -20.0),
                    (20.0, -35.0),
                    (20.0, -50.0),
                ],
                15.0,
            ),
        ] {
            let v = measure(&steps(2, &steps_), &st).values();
            assert!(
                close(v.range, lra, 1.0),
                "LRA {} not {lra}: {steps_:?}",
                v.range
            );
        }
    }

    #[test]
    fn segments_are_aligned_and_partial_ones_skipped() {
        let (mut meter, mut rx) = SegmentMeter::new(1);
        let block = vec![0.1f32; 144];
        // Start mid-segment at index 1000: the first segment sent is #1.
        let mut start = 1000u64;
        while start < 3 * SEGMENT + 500 {
            meter.feed(start, 0, &block);
            start += 144;
        }
        let first = rx.pop().unwrap();
        assert_eq!(first.index, 1);
        assert_eq!(rx.pop().unwrap().index, 2);
        assert!(rx.pop().is_err());
        assert!(first.energy[0] > 0.0);
    }

    #[test]
    fn stop_and_reset() {
        let mut l = measure(&steps(2, &[(5.0, -23.0)]), &["L", "R"]);
        l.running = false;
        l.reset();
        assert_eq!(l.values().integrated, f64::NEG_INFINITY);
        assert!(close(l.values().momentary, -23.0, 0.1));
    }
}
