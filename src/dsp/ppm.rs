//! Quasi-peak programme meters (§7, IEC 60268-10): the detector of a PPM. Type I
//! (Nordic, DIN): integration time 5 ms, return 20 dB in 1.7 s. Type II (EBU,
//! BBC): integration time 10 ms, return 24 dB in 2.8 s. The scales (EBU, BBC,
//! Nordic, DIN) are drawn by the UI from the detector's level in dBFS.
//!
//! Integration time is the burst length that reads 2 dB below the steady tone. On
//! a rectified sine, with the return running between the peaks, this needs a
//! charge time constant of 1.355 ms (Type I) and 2.763 ms (Type II); bursts of 1
//! to 10 kHz then read −2.00 ± 0.04 dB.

use crate::config::SAMPLE_RATE;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ballistics {
    /// Charge per sample towards a higher level.
    pub attack: f32,
    /// Factor per sample while falling.
    pub release: f32,
    /// Reading = level × gain: a steady 1 kHz tone reads its peak (the detector
    /// settles about 0.2 dB below it).
    pub gain: f32,
}

impl Ballistics {
    fn from(charge_s: f64, fall_db: f64, fall_s: f64) -> Self {
        let fs = f64::from(SAMPLE_RATE);
        let mut b = Self {
            attack: (1.0 - (-1.0 / (charge_s * fs)).exp()) as f32,
            release: 10f64.powf(-fall_db / 20.0 / (fall_s * fs)) as f32,
            gain: 1.0,
        };
        let tone: Vec<f32> = (0..SAMPLE_RATE as usize / 4)
            .map(|i| (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / fs).sin() as f32)
            .collect();
        b.gain = 1.0 / b.run(0.0, &tone).1;
        b
    }

    pub fn type_i() -> Self {
        Self::from(1.3553e-3, 20.0, 1.7)
    }

    pub fn type_ii() -> Self {
        Self::from(2.7631e-3, 24.0, 2.8)
    }

    /// Run the detector at `level` over `samples`; returns the new level and the
    /// block's largest level.
    #[inline]
    pub fn run(&self, mut level: f32, samples: &[f32]) -> (f32, f32) {
        let mut max = level;
        for &x in samples {
            let a = x.abs();
            if a > level {
                level += self.attack * (a - level);
            } else {
                level *= self.release;
            }
            max = max.max(level);
        }
        (level, max)
    }
}

/// [`Ballistics::run`] of two detectors over the same samples in one pass: the
/// two independent chains overlap, which halves the time of two passes.
#[inline]
pub fn run_pair(b: &[Ballistics; 2], mut level: [f32; 2], samples: &[f32]) -> ([f32; 2], [f32; 2]) {
    let mut max = level;
    for &x in samples {
        let a = x.abs();
        for k in 0..2 {
            let up = level[k] + b[k].attack * (a - level[k]);
            let down = level[k] * b[k].release;
            level[k] = if a > level[k] { up } else { down };
            max[k] = max[k].max(level[k]);
        }
    }
    (level, max)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(f: f64, seconds: f64) -> Vec<f32> {
        let n = (seconds * f64::from(SAMPLE_RATE)) as usize;
        (0..n)
            .map(|i| {
                (2.0 * std::f64::consts::PI * f * i as f64 / f64::from(SAMPLE_RATE)).sin() as f32
            })
            .collect()
    }

    fn db(x: f32) -> f64 {
        20.0 * f64::from(x).log10()
    }

    #[test]
    fn tone_bursts_read_two_db_low() {
        // IEC 60268-10: the integration-time burst reads −2 dB (Type II ±0.5 dB).
        for (b, burst) in [
            (Ballistics::type_i(), 0.005),
            (Ballistics::type_ii(), 0.010),
        ] {
            for f in [1000.0, 5000.0, 10_000.0] {
                let steady = b.run(0.0, &tone(f, 0.3)).1 * b.gain;
                let short = b.run(0.0, &tone(f, burst)).1 * b.gain;
                let rel = db(short) - db(steady);
                assert!((rel + 2.0).abs() < 0.1, "{f} Hz {burst} s burst: {rel} dB");
                assert!(db(steady).abs() < 0.3, "{f} Hz steady: {} dB", db(steady));
                if f == 1000.0 {
                    assert!(db(steady).abs() < 0.01);
                }
            }
        }
    }

    #[test]
    fn a_pair_matches_two_runs() {
        let b = [Ballistics::type_i(), Ballistics::type_ii()];
        let x: Vec<f32> = tone(3000.0, 0.05).iter().enumerate().map(|(i, v)| v * (i % 7) as f32 / 7.0).collect();
        let (l, m) = run_pair(&b, [0.3, 0.1], &x);
        for k in 0..2 {
            assert_eq!((l[k], m[k]), b[k].run([0.3, 0.1][k], &x));
        }
    }

    #[test]
    fn return_times() {
        for (b, fall, seconds) in [
            (Ballistics::type_i(), 20.0, 1.7),
            (Ballistics::type_ii(), 24.0, 2.8),
        ] {
            let silence = vec![0f32; (seconds * f64::from(SAMPLE_RATE)) as usize];
            let (level, _) = b.run(1.0, &silence);
            // f32 over 1.7 to 2.8 s: within 0.1 dB (IEC allows ±0.3 s).
            assert!((db(level) + fall).abs() < 0.1, "{}", db(level));
        }
    }
}
