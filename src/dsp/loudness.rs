//! Loudness (§7): ITU-R BS.1770-4 and EBU R 128 / Tech 3341 / Tech 3342.
//!
//! The DSP side ([`SegmentMeter`]) K-weights every channel and sends the energy of
//! each 10 ms segment (480 samples, aligned to the sample index). The control side
//! ([`Loudness`]) derives:
//! - momentary loudness: the last 400 ms, ungated, every 10 ms;
//! - short-term loudness: the last 3 s, ungated, every 10 ms;
//! - integrated loudness: 400 ms blocks every 100 ms (75 % overlap), absolute
//!   gate −70 LUFS, relative gate −10 LU;
//! - loudness range (Tech 3342): short-term values every 100 ms, absolute gate
//!   −70 LUFS, relative gate −20 LU, 95th minus 10th percentile.
//!
//! The 10 ms step lets the maximum of momentary loudness be found within 10 ms
//! (EBU Tech 3341 cases 13 and 14).
//!
//! The blocks and short-term values are kept in histograms of 0.1 LU bins above
//! the absolute gate ([`Histogram`]), with their count, power sum and loudness
//! sum per bin: constant memory and time, however long a measurement runs. Only
//! the bin holding a relative gate is decided as a whole (by its mean), and the
//! range's percentiles take the mean of their bin.

use crate::dsp::biquad::{Coefficients, State};

/// Samples per segment (10 ms at 48 kHz).
pub const SEGMENT: u64 = 480;
/// Segments per 100 ms step, per 400 ms block and per 3 s window.
const STEP: usize = 10;
const MOMENTARY: usize = 40;
const SHORT_TERM: usize = 300;
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
    /// Segment number: the segment covers samples [n·480, (n+1)·480).
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
        // 10 s of segments, far more than the control side's tick needs.
        let (out, rx) = rtrb::RingBuffer::new(1024);
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
            self.current = start.is_multiple_of(SEGMENT).then_some(seg);
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
    blocks: Histogram,
    short_terms: Histogram,
    pub max_true_peak: f32,
}

/// Bins of [`BIN_LU`] from the absolute gate (−70 LUFS) up to +30 LUFS; louder
/// values share the top bin.
const BIN_LU: f64 = 0.1;
const BINS: usize = 1000;
const ABSOLUTE_GATE: f64 = -70.0;

/// The values above the absolute gate: per bin their count, power sum and
/// loudness sum.
#[derive(Debug, Clone)]
struct Histogram {
    count: Box<[u64]>,
    power: Box<[f64]>,
    lufs: Box<[f64]>,
    n: u64,
    total: f64,
}

impl Default for Histogram {
    fn default() -> Self {
        Self {
            count: vec![0; BINS].into(),
            power: vec![0.0; BINS].into(),
            lufs: vec![0.0; BINS].into(),
            n: 0,
            total: 0.0,
        }
    }
}

impl Histogram {
    fn push(&mut self, p: f64) {
        let l = lufs(p);
        if l <= ABSOLUTE_GATE {
            return;
        }
        let bin = (((l - ABSOLUTE_GATE) / BIN_LU) as usize).min(BINS - 1);
        self.count[bin] += 1;
        self.power[bin] += p;
        self.lufs[bin] += l;
        self.n += 1;
        self.total += p;
    }

    fn clear(&mut self) {
        *self = Self::default();
    }

    /// The bins above the relative gate `offset` LU under the mean power of
    /// everything above the absolute gate: (bin, count); a bin counts when its
    /// mean loudness is above the gate.
    fn gated(&self, offset: f64) -> impl Iterator<Item = (usize, u64)> + '_ {
        let gate = if self.n == 0 {
            f64::INFINITY
        } else {
            lufs(self.total / self.n as f64) + offset
        };
        (0..BINS)
            .filter(move |&b| self.count[b] > 0 && self.lufs[b] / self.count[b] as f64 > gate)
            .map(|b| (b, self.count[b]))
    }

    /// Gated integrated loudness (BS.1770-4: relative gate −10 LU).
    fn integrated(&self) -> f64 {
        let (n, p) = self
            .gated(-10.0)
            .fold((0, 0.0), |(n, p), (b, c)| (n + c, p + self.power[b]));
        if n == 0 {
            return f64::NEG_INFINITY;
        }
        lufs(p / n as f64)
    }

    /// Loudness range (EBU Tech 3342: relative gate −20 LU, 95th minus 10th
    /// percentile, interpolated between ranks).
    fn range(&self) -> f64 {
        let n: u64 = self.gated(-20.0).map(|(_, c)| c).sum();
        if n < 2 {
            return 0.0;
        }
        // The value of rank `k` (0-based): the mean loudness of its bin.
        let at = |k: u64| {
            let mut seen = 0;
            for (b, c) in self.gated(-20.0) {
                seen += c;
                if k < seen {
                    return self.lufs[b] / c as f64;
                }
            }
            f64::NAN
        };
        let pct = |q: f64| {
            let pos = q * (n - 1) as f64;
            let (lo, hi) = (pos.floor() as u64, pos.ceil() as u64);
            at(lo) + (at(hi) - at(lo)) * (pos - lo as f64)
        };
        pct(0.95) - pct(0.10)
    }
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

    /// Take one segment. Missing segments up to 3 s count as silence (the DSP
    /// thread zero-fills the blocks it misses); a longer gap, or one backwards,
    /// restarts the windows.
    pub fn push(&mut self, s: &Segment) {
        match self.last {
            Some(l) if s.index > l + 1 && s.index - l - 1 <= SHORT_TERM as u64 => {
                for index in l + 1..s.index {
                    self.push_power(index, 0.0);
                }
            }
            Some(l) if s.index != l + 1 => self.recent.clear(),
            _ => {}
        }
        let p: f64 = self
            .weights
            .iter()
            .zip(s.energy.iter())
            .map(|(g, e)| g * e / SEGMENT as f64)
            .sum();
        self.push_power(s.index, p);
    }

    fn push_power(&mut self, index: u64, p: f64) {
        self.last = Some(index);
        self.recent.push_back(p);
        while self.recent.len() > SHORT_TERM {
            self.recent.pop_front();
        }
        // Gating blocks and short-term values for the range every 100 ms.
        if !self.running || !(index + 1).is_multiple_of(STEP as u64) {
            return;
        }
        if self.recent.len() >= MOMENTARY {
            self.blocks.push(self.mean(MOMENTARY));
        }
        if self.recent.len() >= SHORT_TERM {
            self.short_terms.push(self.mean(SHORT_TERM));
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
            momentary: lufs(self.mean(MOMENTARY)),
            short_term: lufs(self.mean(SHORT_TERM)),
            integrated: self.blocks.integrated(),
            range: self.short_terms.range(),
        }
    }
}

/// The power of `lufs`, for tests and targets.
pub fn lufs_power(l: f64) -> f64 {
    power(l)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    // The list-based gating of 1.4.3 and before, the reference.
    /// Gated integrated loudness of 400 ms block powers.
    fn integrated(blocks: &[f64]) -> f64 {
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
    fn range(short_terms: &[f64]) -> f64 {
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

    #[test]
    fn short_gaps_are_silence_long_ones_restart() {
        let mut l = Loudness::new(weights(&["M"]));
        let seg = |index: u64| {
            let mut energy = [0.0; MAX_CHANNELS];
            energy[0] = 0.01 * SEGMENT as f64;
            Segment { index, energy }
        };
        for i in 0..400 {
            l.push(&seg(i));
        }
        let full = l.values().momentary;
        // Two segments (20 ms) missing: 2 of the 40 momentary segments are silent.
        l.push(&seg(402));
        let m = l.values().momentary;
        assert!(
            close(m, full + 10.0 * (38.0f64 / 40.0).log10(), 1e-9),
            "{m} vs {full}"
        );
        assert!(l.values().short_term.is_finite());
        // More than 3 s missing: the windows restart.
        l.push(&seg(402 + 400));
        assert_eq!(l.values().momentary, f64::NEG_INFINITY);
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

    /// A deterministic generator: uniform in [0, 1).
    fn uniform(seed: &mut u64) -> f64 {
        *seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (*seed >> 11) as f64 / (1u64 << 53) as f64
    }

    /// The histogram against the list-based reference, on block (or short-term)
    /// loudness sequences: the largest differences of I and LRA.
    fn compare(levels: &[f64]) -> (f64, f64) {
        let powers: Vec<f64> = levels.iter().map(|l| power(*l)).collect();
        let mut h = Histogram::default();
        for p in &powers {
            h.push(*p);
        }
        let d = |a: f64, b: f64| if a == b { 0.0 } else { (a - b).abs() };
        (
            d(h.integrated(), integrated(&powers)),
            d(h.range(), range(&powers)),
        )
    }

    #[test]
    fn histogram_gating_matches_the_lists() {
        let mut seed = 7;
        let mut cases: Vec<(&str, Vec<f64>)> = vec![
            ("tone", vec![-23.0; 600]),
            (
                "steps",
                [-36.0, -23.0, -36.0, -72.0, -20.0, -26.0]
                    .iter()
                    .flat_map(|l| std::iter::repeat_n(*l, 200))
                    .collect(),
            ),
            ("silence", vec![-120.0; 100]),
            ("one value", vec![-30.0]),
        ];
        cases.push((
            "uniform −80…0",
            (0..20_000)
                .map(|_| -80.0 + 80.0 * uniform(&mut seed))
                .collect(),
        ));
        cases.push((
            "speech-like",
            (0..36_000)
                .map(|_| {
                    // About normal (sum of 4 uniforms) around −24 LUFS, σ ≈ 6 LU, with pauses.
                    let n: f64 = (0..4).map(|_| uniform(&mut seed)).sum::<f64>() - 2.0;
                    if uniform(&mut seed) < 0.2 {
                        -90.0
                    } else {
                        -24.0 + 10.4 * n
                    }
                })
                .collect(),
        ));
        cases.push((
            "ramp",
            (0..10_000)
                .map(|k| -60.0 + 50.0 * k as f64 / 10_000.0)
                .collect(),
        ));
        for (what, levels) in &cases {
            let (di, dr) = compare(levels);
            eprintln!("{what}: I differs by {di:.4}, LRA by {dr:.4}");
            assert!(
                di <= 0.05 && dr <= 0.05,
                "{what}: I differs by {di}, LRA by {dr}"
            );
        }
    }

    /// Signals through the whole pipeline, against the lists of the same blocks
    /// and short-term values.
    #[test]
    fn signals_match_the_lists() {
        let mut seed = 3;
        let noise: Vec<f32> = (0..48_000 * 40)
            .map(|k| {
                // Pink-ish enough: white noise whose level changes every second.
                let level = 0.05 + 0.2 * ((k / 48_000) % 7) as f64 / 7.0;
                (level * (uniform(&mut seed) - 0.5)) as f32
            })
            .collect();
        for (what, signal) in [
            ("tone", steps(2, &[(30.0, -23.0)])),
            (
                "steps",
                steps(
                    2,
                    &[(10.0, -36.0), (20.0, -23.0), (10.0, -50.0), (10.0, -18.0)],
                ),
            ),
            ("noise", vec![noise.clone(), noise]),
        ] {
            let l = measure(&signal, &["L", "R"]);
            // The reference lists, from the same segment powers.
            let (mut meter, mut rx) = SegmentMeter::new(2);
            let w = weights(&["L", "R"]);
            let mut recent: Vec<f64> = Vec::new();
            let (mut blocks, mut shorts) = (Vec::new(), Vec::new());
            let mut start = 0;
            while start + 96 <= signal[0].len() {
                for (c, ch) in signal.iter().enumerate() {
                    meter.feed(start as u64, c, &ch[start..start + 96]);
                }
                while let Ok(s) = rx.pop() {
                    recent.push(
                        w.iter()
                            .zip(s.energy.iter())
                            .map(|(g, e)| g * e / SEGMENT as f64)
                            .sum(),
                    );
                    if (s.index + 1).is_multiple_of(STEP as u64) {
                        let mean =
                            |n: usize| recent[recent.len() - n..].iter().sum::<f64>() / n as f64;
                        if recent.len() >= MOMENTARY {
                            blocks.push(mean(MOMENTARY));
                        }
                        if recent.len() >= SHORT_TERM {
                            shorts.push(mean(SHORT_TERM));
                        }
                    }
                }
                start += 96;
            }
            let v = l.values();
            let (i, r) = (integrated(&blocks), range(&shorts));
            eprintln!(
                "{what}: I {:.4} / {i:.4}, LRA {:.4} / {r:.4}",
                v.integrated, v.range
            );
            assert!(
                (v.integrated - i).abs() <= 0.05,
                "{what}: I {} vs {i}",
                v.integrated
            );
            assert!(
                (v.range - r).abs() <= 0.05,
                "{what}: LRA {} vs {r}",
                v.range
            );
        }
    }

    #[test]
    fn segments_are_aligned_and_partial_ones_skipped() {
        let (mut meter, mut rx) = SegmentMeter::new(1);
        let block = vec![0.1f32; 144];
        // Start mid-segment 1: the first segment sent is #2.
        let mut start = SEGMENT + 100;
        while start < 4 * SEGMENT + 50 {
            meter.feed(start, 0, &block);
            start += 144;
        }
        let first = rx.pop().unwrap();
        assert_eq!(first.index, 2);
        assert_eq!(rx.pop().unwrap().index, 3);
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
