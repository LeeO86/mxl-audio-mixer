//! Second-order sections (§6 filter design).
//!
//! Designs from R. Bristow-Johnson, "Cookbook formulae for audio EQ biquad filter
//! coefficients" (the W3C Audio EQ Cookbook): bilinear transforms of the analog
//! prototypes, pre-warped at the centre or corner frequency. Butterworth high- and
//! low-pass filters of order 2k are cascades of k sections with
//! Q_i = 1 / (2 cos((2i − 1)π / 4k)) (12, 24, 36, 48 dB/oct = order 2, 4, 6, 8).
//! Sections run in transposed direct form II in f64.

use std::f64::consts::PI;

use crate::config::SAMPLE_RATE;

/// Normalised coefficients (a0 = 1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Coefficients {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
}

impl Coefficients {
    pub const IDENTITY: Self = Self {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    };

    fn normalise(b: [f64; 3], a: [f64; 3]) -> Self {
        Self {
            b0: b[0] / a[0],
            b1: b[1] / a[0],
            b2: b[2] / a[0],
            a1: a[1] / a[0],
            a2: a[2] / a[0],
        }
    }

    /// Magnitude response at `freq` Hz.
    pub fn magnitude(&self, freq: f64) -> f64 {
        let w = 2.0 * PI * freq / f64::from(SAMPLE_RATE);
        let (c1, s1, c2, s2) = (w.cos(), w.sin(), (2.0 * w).cos(), (2.0 * w).sin());
        let num = (
            self.b0 + self.b1 * c1 + self.b2 * c2,
            -(self.b1 * s1 + self.b2 * s2),
        );
        let den = (
            1.0 + self.a1 * c1 + self.a2 * c2,
            -(self.a1 * s1 + self.a2 * s2),
        );
        (num.0.hypot(num.1)) / (den.0.hypot(den.1))
    }
}

fn w0(freq: f64) -> f64 {
    2.0 * PI * freq.clamp(1.0, 0.49 * f64::from(SAMPLE_RATE)) / f64::from(SAMPLE_RATE)
}

/// Bell (peaking) at `freq` with quality `q` and `gain_db`.
pub fn bell(freq: f64, q: f64, gain_db: f64) -> Coefficients {
    let a = 10f64.powf(gain_db / 40.0);
    let w = w0(freq);
    let alpha = w.sin() / (2.0 * q);
    let c = w.cos();
    Coefficients::normalise(
        [1.0 + alpha * a, -2.0 * c, 1.0 - alpha * a],
        [1.0 + alpha / a, -2.0 * c, 1.0 - alpha / a],
    )
}

/// Low shelf with corner `freq`, quality `q` (0.7071: no overshoot) and `gain_db`.
pub fn low_shelf(freq: f64, q: f64, gain_db: f64) -> Coefficients {
    let a = 10f64.powf(gain_db / 40.0);
    let w = w0(freq);
    let alpha = w.sin() / (2.0 * q);
    let c = w.cos();
    let s = 2.0 * a.sqrt() * alpha;
    Coefficients::normalise(
        [
            a * ((a + 1.0) - (a - 1.0) * c + s),
            2.0 * a * ((a - 1.0) - (a + 1.0) * c),
            a * ((a + 1.0) - (a - 1.0) * c - s),
        ],
        [
            (a + 1.0) + (a - 1.0) * c + s,
            -2.0 * ((a - 1.0) + (a + 1.0) * c),
            (a + 1.0) + (a - 1.0) * c - s,
        ],
    )
}

/// High shelf with corner `freq`, quality `q` and `gain_db`.
pub fn high_shelf(freq: f64, q: f64, gain_db: f64) -> Coefficients {
    let a = 10f64.powf(gain_db / 40.0);
    let w = w0(freq);
    let alpha = w.sin() / (2.0 * q);
    let c = w.cos();
    let s = 2.0 * a.sqrt() * alpha;
    Coefficients::normalise(
        [
            a * ((a + 1.0) + (a - 1.0) * c + s),
            -2.0 * a * ((a - 1.0) + (a + 1.0) * c),
            a * ((a + 1.0) + (a - 1.0) * c - s),
        ],
        [
            (a + 1.0) - (a - 1.0) * c + s,
            2.0 * ((a - 1.0) - (a + 1.0) * c),
            (a + 1.0) - (a - 1.0) * c - s,
        ],
    )
}

/// Notch at `freq` with quality `q`.
pub fn notch(freq: f64, q: f64) -> Coefficients {
    let w = w0(freq);
    let alpha = w.sin() / (2.0 * q);
    let c = w.cos();
    Coefficients::normalise([1.0, -2.0 * c, 1.0], [1.0 + alpha, -2.0 * c, 1.0 - alpha])
}

/// Second-order low-pass.
pub fn low_pass(freq: f64, q: f64) -> Coefficients {
    let w = w0(freq);
    let alpha = w.sin() / (2.0 * q);
    let c = w.cos();
    Coefficients::normalise(
        [(1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0],
        [1.0 + alpha, -2.0 * c, 1.0 - alpha],
    )
}

/// Second-order high-pass.
pub fn high_pass(freq: f64, q: f64) -> Coefficients {
    let w = w0(freq);
    let alpha = w.sin() / (2.0 * q);
    let c = w.cos();
    Coefficients::normalise(
        [(1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0],
        [1.0 + alpha, -2.0 * c, 1.0 - alpha],
    )
}

/// Band-pass with 0 dB at the centre.
pub fn band_pass(freq: f64, q: f64) -> Coefficients {
    let w = w0(freq);
    let alpha = w.sin() / (2.0 * q);
    let c = w.cos();
    Coefficients::normalise([alpha, 0.0, -alpha], [1.0 + alpha, -2.0 * c, 1.0 - alpha])
}

/// The Q of each section of a Butterworth filter of `order` (even, 2–8).
pub fn butterworth_q(order: usize) -> impl Iterator<Item = f64> {
    let n = order as f64;
    (1..=order / 2).map(move |i| 1.0 / (2.0 * ((2.0 * i as f64 - 1.0) * PI / (2.0 * n)).cos()))
}

/// The state of one section for one channel.
#[derive(Debug, Clone, Copy, Default)]
pub struct State {
    z1: f64,
    z2: f64,
}

impl State {
    #[inline]
    pub fn tick(&mut self, c: &Coefficients, x: f64) -> f64 {
        let y = c.b0 * x + self.z1;
        self.z1 = c.b1 * x - c.a1 * y + self.z2;
        self.z2 = c.b2 * x - c.a2 * y;
        y
    }

    /// Filter `samples` in place.
    pub fn run(&mut self, c: &Coefficients, samples: &mut [f32]) {
        for s in samples {
            *s = self.tick(c, f64::from(*s)) as f32;
        }
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db(x: f64) -> f64 {
        20.0 * x.log10()
    }

    /// |H(jΩ)| of an analog prototype `num(s) / den(s)` (second order, real
    /// coefficients, highest power first).
    fn analog(num: [f64; 3], den: [f64; 3], omega: f64) -> f64 {
        let eval = |p: [f64; 3]| {
            let re = p[2] - p[0] * omega * omega;
            let im = p[1] * omega;
            re.hypot(im)
        };
        eval(num) / eval(den)
    }

    /// The frequency `f` mapped onto the prototype's Ω axis (Ω = 1 at `f0`).
    fn warp(f: f64, f0: f64) -> f64 {
        let fs = f64::from(SAMPLE_RATE);
        (PI * f / fs).tan() / (PI * f0 / fs).tan()
    }

    const FREQS: [f64; 9] = [
        20.0, 63.0, 250.0, 700.0, 1000.0, 3100.0, 8000.0, 15000.0, 21000.0,
    ];

    /// Every design matches its analog prototype on the warped axis (tolerance
    /// 1e-6 dB), the reference the cookbook derives them from.
    #[test]
    fn designs_match_their_analog_prototypes() {
        for &(f0, q, g) in &[(1000.0, 0.7, 6.0), (120.0, 2.0, -12.0), (9000.0, 4.0, 3.0)] {
            let a = 10f64.powf(g / 40.0);
            let sa = a.sqrt() / q;
            let cases: [(Coefficients, [f64; 3], [f64; 3]); 7] = [
                (bell(f0, q, g), [1.0, a / q, 1.0], [1.0, 1.0 / (a * q), 1.0]),
                (low_shelf(f0, q, g), [a, a * sa, a * a], [a, sa, 1.0]),
                (high_shelf(f0, q, g), [a * a, a * sa, a], [1.0, sa, a]),
                (notch(f0, q), [1.0, 0.0, 1.0], [1.0, 1.0 / q, 1.0]),
                (low_pass(f0, q), [0.0, 0.0, 1.0], [1.0, 1.0 / q, 1.0]),
                (high_pass(f0, q), [1.0, 0.0, 0.0], [1.0, 1.0 / q, 1.0]),
                (band_pass(f0, q), [0.0, 1.0 / q, 0.0], [1.0, 1.0 / q, 1.0]),
            ];
            for (i, (c, num, den)) in cases.iter().enumerate() {
                for f in FREQS {
                    if i == 3 && (f - f0).abs() < 1.0 {
                        continue;
                    }
                    let d = db(c.magnitude(f));
                    let r = db(analog(*num, *den, warp(f, f0)));
                    assert!(
                        (d - r).abs() < 1e-6,
                        "case {i} f0 {f0} at {f} Hz: {d} vs {r}"
                    );
                }
            }
        }
    }

    #[test]
    fn design_intent() {
        assert!((db(bell(1000.0, 1.0, 6.0).magnitude(1000.0)) - 6.0).abs() < 1e-9);
        assert!((db(bell(1000.0, 1.0, -9.0).magnitude(1000.0)) + 9.0).abs() < 1e-9);
        assert!(db(notch(2000.0, 4.0).magnitude(2000.0)) < -100.0);
        let q = std::f64::consts::FRAC_1_SQRT_2;
        assert!((db(low_shelf(200.0, q, 6.0).magnitude(10.0)) - 6.0).abs() < 0.05);
        assert!((db(high_shelf(5000.0, q, -6.0).magnitude(22000.0)) + 6.0).abs() < 0.05);
        assert!((db(band_pass(3000.0, 2.0).magnitude(3000.0))).abs() < 1e-9);
    }

    #[test]
    fn butterworth_cascades() {
        let qs: Vec<f64> = butterworth_q(4).collect();
        assert!((qs[0] - 0.541_196).abs() < 1e-6 && (qs[1] - 1.306_563).abs() < 1e-6);
        for (order, slope) in [(2, 12.0), (4, 24.0), (6, 36.0), (8, 48.0)] {
            let fc = 1000.0;
            let hp: Vec<Coefficients> = butterworth_q(order).map(|q| high_pass(fc, q)).collect();
            let lp: Vec<Coefficients> = butterworth_q(order).map(|q| low_pass(fc, q)).collect();
            let mag =
                |cs: &[Coefficients], f: f64| cs.iter().map(|c| db(c.magnitude(f))).sum::<f64>();
            // −3.01 dB at the corner, flat in the pass band, the slope an octave
            // further than two octaves out.
            assert!((mag(&hp, fc) + 3.0103).abs() < 1e-3, "hp {order}");
            assert!((mag(&lp, fc) + 3.0103).abs() < 1e-3, "lp {order}");
            assert!(mag(&hp, 16000.0).abs() < 0.01 && mag(&lp, 60.0).abs() < 0.01);
            let octave = mag(&hp, fc / 8.0) - mag(&hp, fc / 4.0);
            assert!((octave + slope).abs() < 0.3, "hp {order}: {octave} dB/oct");
            // Low-pass corner at 250 Hz, so the octave stays far from Nyquist, where
            // the bilinear transform steepens the slope.
            let lp: Vec<Coefficients> = butterworth_q(order).map(|q| low_pass(100.0, q)).collect();
            let octave = mag(&lp, 3200.0) - mag(&lp, 1600.0);
            assert!(
                (octave + slope).abs() < 0.03 * slope,
                "lp {order}: {octave} dB/oct"
            );
        }
    }

    #[test]
    fn running_a_section_matches_its_response() {
        let c = bell(1000.0, 1.0, 6.0);
        let mut s = State::default();
        let mut peak = 0f32;
        for n in 0..48_000 {
            let x = (2.0 * PI * 1000.0 * n as f64 / 48_000.0).sin() as f32;
            let mut v = [x];
            s.run(&c, &mut v);
            if n > 24_000 {
                peak = peak.max(v[0].abs());
            }
        }
        assert!((db(f64::from(peak)) - 6.0).abs() < 0.01);
    }
}
