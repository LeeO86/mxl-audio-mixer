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

/// One channel's interpolator history.
#[derive(Debug, Clone, Default)]
pub struct TruePeak {
    history: [f32; TAPS],
    at: usize,
}

impl TruePeak {
    /// The largest |value| of the oversampled `samples`.
    pub fn run(&mut self, taps: &[[f32; TAPS]; PHASES], samples: &[f32]) -> f32 {
        let mut peak = 0f32;
        for &x in samples {
            self.at = (self.at + TAPS - 1) % TAPS;
            self.history[self.at] = x;
            for phase in taps {
                let mut y = 0f32;
                for (m, h) in phase.iter().enumerate() {
                    y += h * self.history[(self.at + m) % TAPS];
                }
                peak = peak.max(y.abs());
            }
        }
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
