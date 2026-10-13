//! True peak (§7, ITU-R BS.1770-4 Annex 2): the largest |sample| after 4×
//! oversampling. The interpolator is a 48-tap Kaiser-windowed sinc (β = 6) in
//! four phases of 12 taps, each phase normalised to unity gain at DC. BS.1770-4
//! gives its filter as an example and allows others; this one is checked against
//! the EBU Tech 3341 true-peak cases.

use std::f64::consts::PI;

pub const PHASES: usize = 4;
pub const TAPS: usize = 12;

fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    for k in 1..50 {
        term *= (x / (2.0 * k as f64)).powi(2);
        sum += term;
        if term < 1e-12 * sum {
            break;
        }
    }
    sum
}

/// The polyphase taps: `taps()[p][m]` weights x(n − m) for output phase p.
pub fn taps() -> [[f32; TAPS]; PHASES] {
    let len = PHASES * TAPS;
    let centre = (len - 1) as f64 / 2.0;
    let beta = 6.0;
    let mut h = [0f64; PHASES * TAPS];
    for (n, v) in h.iter_mut().enumerate() {
        let t = (n as f64 - centre) / PHASES as f64;
        let sinc = if t.abs() < 1e-12 {
            1.0
        } else {
            (PI * t).sin() / (PI * t)
        };
        let r = (n as f64 - centre) / centre;
        *v = sinc * bessel_i0(beta * (1.0 - r * r).max(0.0).sqrt()) / bessel_i0(beta);
    }
    let mut out = [[0f32; TAPS]; PHASES];
    for (p, phase) in out.iter_mut().enumerate() {
        let sum: f64 = (0..TAPS).map(|m| h[m * PHASES + p]).sum();
        for (m, tap) in phase.iter_mut().enumerate() {
            *tap = (h[m * PHASES + p] / sum) as f32;
        }
    }
    out
}

/// The most an interpolator output can exceed the largest |sample| of its
/// window: the largest sum of |taps| over the phases (rounded up a little).
pub fn bound(taps: &[[f32; TAPS]; PHASES]) -> f32 {
    let most = taps
        .iter()
        .map(|p| p.iter().map(|h| h.abs()).sum::<f32>())
        .fold(0f32, f32::max);
    most * 1.0001
}

/// Samples per pass of [`TruePeak::run`] (a buffer on the stack).
const CHUNK: usize = 64;

/// The phases side by side: `transposed(t)[m][p]` = `t[p][m]`, so that one tap
/// of all four phases is one 4-wide multiply-add.
fn transposed(taps: &[[f32; TAPS]; PHASES]) -> [[f32; PHASES]; TAPS] {
    let mut out = [[0f32; PHASES]; TAPS];
    for (p, row) in taps.iter().enumerate() {
        for (m, h) in row.iter().enumerate() {
            out[m][p] = *h;
        }
    }
    out
}

/// One channel's interpolator history: the last `TAPS − 1` samples, oldest
/// first, and their largest |value|.
#[derive(Debug, Clone, Default)]
pub struct TruePeak {
    tail: [f32; TAPS - 1],
    tail_peak: f32,
}

impl TruePeak {
    /// The largest |value| of the oversampled `samples`: per sample the four
    /// phases at once, tap by tap (the same sums, in the same order, as phase
    /// by phase).
    pub fn run(&mut self, taps: &[[f32; TAPS]; PHASES], samples: &[f32]) -> f32 {
        let ht = transposed(taps);
        let mut lanes = [0f32; PHASES];
        let mut buf = [0f32; TAPS - 1 + CHUNK];
        for chunk in samples.chunks(CHUNK) {
            let len = chunk.len();
            buf[..TAPS - 1].copy_from_slice(&self.tail);
            buf[TAPS - 1..TAPS - 1 + len].copy_from_slice(chunk);
            for n in 0..len {
                let mut y = [0f32; PHASES];
                for (m, h) in ht.iter().enumerate() {
                    let v = buf[TAPS - 1 + n - m];
                    for k in 0..PHASES {
                        y[k] += h[k] * v;
                    }
                }
                for k in 0..PHASES {
                    lanes[k] = lanes[k].max(y[k].abs());
                }
            }
            self.tail.copy_from_slice(&buf[len..len + TAPS - 1]);
        }
        self.tail_peak = super::reduce::peak(&self.tail);
        lanes.iter().fold(0f32, |a, b| a.max(*b))
    }

    /// As [`run`](Self::run), but only when the block can raise `floor` (a peak
    /// already held): an interpolated value is at most `bound` × the largest
    /// |sample| of its window (the samples and the tail before them). Otherwise
    /// the history is kept up to date and the block's sample peak returned.
    pub fn run_above(
        &mut self,
        taps: &[[f32; TAPS]; PHASES],
        bound: f32,
        samples: &[f32],
        floor: f32,
    ) -> f32 {
        let sample_peak = super::reduce::peak(samples);
        if sample_peak.max(self.tail_peak) * bound > floor || samples.len() < TAPS - 1 {
            return self.run(taps, samples);
        }
        self.tail
            .copy_from_slice(&samples[samples.len() - (TAPS - 1)..]);
        self.tail_peak = super::reduce::peak(&self.tail);
        sample_peak
    }

    /// One sample (a limiter's detector): the largest |value| of its four
    /// interpolated values.
    pub fn step(&mut self, taps: &[[f32; TAPS]; PHASES], x: f32) -> f32 {
        let mut peak = 0f32;
        for phase in taps {
            let mut y = phase[0] * x;
            for (m, h) in phase.iter().enumerate().skip(1) {
                y += h * self.tail[TAPS - 1 - m];
            }
            peak = peak.max(y.abs());
        }
        self.tail.copy_within(1.., 0);
        self.tail[TAPS - 2] = x;
        peak
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db(x: f32) -> f64 {
        20.0 * f64::from(x).log10()
    }

    fn sine(freq: f64, amplitude: f64, phase_deg: f64, n: usize) -> Vec<f32> {
        (0..n)
            .map(|k| {
                (amplitude * (2.0 * PI * freq * k as f64 / 48_000.0 + phase_deg.to_radians()).sin())
                    as f32
            })
            .collect()
    }

    #[test]
    fn phases_have_unity_dc_gain() {
        for phase in taps() {
            let sum: f32 = phase.iter().sum();
            assert!((sum - 1.0).abs() < 1e-6);
        }
    }

    /// The EBU Tech 3341 true-peak signals: fs/4 sines at −6 dBFS with phases 0°,
    /// 45°, 60° and 67.5°, whose sample peaks lie up to 3 dB below the true peak;
    /// expected −6.0 dBTP within +0.2 / −0.4 dB. Plus a 1 kHz sine at +3 dBFS.
    #[test]
    fn ebu_3341_true_peak_cases() {
        let t = taps();
        let half = 10f64.powf(-6.0 / 20.0);
        let cases = [
            (12_000.0, half, 0.0, -6.0),
            (12_000.0, half, 45.0, -6.0),
            (12_000.0, half, 60.0, -6.0),
            (12_000.0, half, 67.5, -6.0),
            (1_000.0, 10f64.powf(3.0 / 20.0), 0.0, 3.0),
        ];
        for (i, (f, a, phase, expected)) in cases.into_iter().enumerate() {
            let x = sine(f, a, phase, 48_000);
            let mut tp = TruePeak::default();
            let peak = db(tp.run(&t, &x));
            assert!(
                peak <= expected + 0.2 && peak >= expected - 0.4,
                "signal {i}: {peak} dBTP, expected {expected}"
            );
        }
        // The sample peak of the 45° case is 3 dB lower: true peak is what finds it.
        let x = sine(12_000.0, half, 45.0, 4800);
        let sample = x.iter().fold(0f32, |m, v| m.max(v.abs()));
        assert!((db(sample) + 9.0).abs() < 0.1);
    }

    /// The interpolator as first written: per sample, the history read modulo.
    fn reference(taps: &[[f32; TAPS]; PHASES], x: &[f32]) -> Vec<f32> {
        let mut history = [0f32; TAPS];
        let mut at = 0;
        x.iter()
            .map(|&v| {
                at = (at + TAPS - 1) % TAPS;
                history[at] = v;
                taps.iter()
                    .map(|phase| {
                        let mut y = 0f32;
                        for (m, h) in phase.iter().enumerate() {
                            y += h * history[(at + m) % TAPS];
                        }
                        y.abs()
                    })
                    .fold(0f32, f32::max)
            })
            .collect()
    }

    fn noise(n: usize, seed: u32) -> Vec<f32> {
        let mut s = seed.max(1);
        (0..n)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                (f64::from(s) / f64::from(u32::MAX) * 2.0 - 1.0) as f32
            })
            .collect()
    }

    #[test]
    fn blocks_steps_and_the_skip_match_the_reference() {
        let t = taps();
        let x = noise(4800, 9);
        let want = reference(&t, &x);
        for block in [1, 7, 11, 12, 64, 96, 100, 480] {
            let mut tp = TruePeak::default();
            for (k, b) in x.chunks(block).enumerate() {
                let got = tp.run(&t, b);
                let w = want[k * block..k * block + b.len()]
                    .iter()
                    .fold(0f32, |a, v| a.max(*v));
                assert_eq!(got, w, "block {block}, chunk {k}");
            }
        }
        let mut tp = TruePeak::default();
        for (k, v) in x.iter().enumerate() {
            assert_eq!(tp.step(&t, *v), want[k], "step {k}");
        }
        // The held maximum over 20 blocks of 96 (a meter tick), with the skip,
        // equals the one computed on every block: on noise, on a loud burst
        // after quiet blocks, on the EBU fs/4 sines.
        let g = bound(&t);
        let mut burst = noise(1920, 3).iter().map(|v| v * 0.01).collect::<Vec<_>>();
        burst[1000..1100].copy_from_slice(&noise(100, 4));
        let half = 10f64.powf(-6.0 / 20.0);
        for signal in [x.clone(), burst, sine(12_000.0, half, 45.0, 1920)] {
            let (mut full, mut fast) = (TruePeak::default(), TruePeak::default());
            let (mut held_full, mut held_fast) = (0f32, 0f32);
            for b in signal.chunks(96) {
                held_full = held_full.max(full.run(&t, b));
                held_fast = held_fast.max(fast.run_above(&t, g, b, held_fast));
            }
            assert_eq!(held_fast, held_full);
        }
    }

    #[test]
    #[ignore]
    fn cost() {
        let t = taps();
        let x = noise(96 * 2000, 5);
        let mut tp = TruePeak::default();
        let started = std::time::Instant::now();
        let mut p = 0f32;
        for b in x.chunks(96) {
            p = p.max(tp.run(&t, std::hint::black_box(b)));
        }
        std::hint::black_box(p);
        eprintln!(
            "run: {:.0} ns per block",
            started.elapsed().as_nanos() as f64 / 2000.0
        );
    }

    #[test]
    fn in_band_sines_keep_their_level() {
        let t = taps();
        for f in [100.0, 997.0, 5000.0, 15_000.0] {
            let mut tp = TruePeak::default();
            let peak = db(tp.run(&t, &sine(f, 0.5, 13.0, 48_000)));
            assert!((peak + 6.0206).abs() < 0.3, "{f} Hz: {peak}");
        }
    }
}
