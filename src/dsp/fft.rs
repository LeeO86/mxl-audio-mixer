//! Spectrum analysis on the control side (§7): an in-place radix-2 FFT and
//! fractional-octave bands.

use std::f64::consts::PI;

/// A complex FFT of a fixed power-of-two size, with its twiddle factors.
pub struct Fft {
    n: usize,
    twiddles: Vec<(f64, f64)>,
}

impl Fft {
    pub fn new(n: usize) -> Self {
        assert!(n.is_power_of_two() && n >= 2);
        let twiddles = (0..n / 2)
            .map(|k| {
                let a = -2.0 * PI * k as f64 / n as f64;
                (a.cos(), a.sin())
            })
            .collect();
        Self { n, twiddles }
    }

    pub fn len(&self) -> usize {
        self.n
    }

    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    /// Transform `re` and `im` (each of length n) in place.
    pub fn run(&self, re: &mut [f64], im: &mut [f64]) {
        let n = self.n;
        let bits = n.trailing_zeros();
        for i in 0..n {
            let j = i.reverse_bits() >> (usize::BITS - bits);
            if j > i {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let mut len = 2;
        while len <= n {
            let step = n / len;
            for start in (0..n).step_by(len) {
                for k in 0..len / 2 {
                    let (wr, wi) = self.twiddles[k * step];
                    let (a, b) = (start + k, start + k + len / 2);
                    let tr = re[b] * wr - im[b] * wi;
                    let ti = re[b] * wi + im[b] * wr;
                    re[b] = re[a] - tr;
                    im[b] = im[a] - ti;
                    re[a] += tr;
                    im[a] += ti;
                }
            }
            len *= 2;
        }
    }
}

/// Centre frequencies of 1/`per_octave`-octave bands from about 20 Hz to 20 kHz
/// (base-2 series around 1 kHz).
pub fn band_centres(per_octave: u32) -> Vec<f64> {
    let per = f64::from(per_octave);
    let lo = (per * (20f64 / 1000.0).log2()).ceil() as i32;
    let hi = (per * (20_000f64 / 1000.0).log2()).floor() as i32;
    (lo..=hi)
        .map(|k| 1000.0 * 2f64.powf(f64::from(k) / per))
        .collect()
}

/// The power spectrum of `samples` (Hann window) in fractional-octave bands, in
/// dBFS: a full-scale sine reads 0 dB in its band.
pub fn bands(fft: &Fft, samples: &[f64], rate: f64, centres: &[f64], per_octave: u32) -> Vec<f64> {
    let n = fft.len();
    let mut re: Vec<f64> = samples
        .iter()
        .take(n)
        .enumerate()
        .map(|(i, x)| x * 0.5 * (1.0 - (2.0 * PI * i as f64 / n as f64).cos()))
        .collect();
    re.resize(n, 0.0);
    let mut im = vec![0.0; n];
    fft.run(&mut re, &mut im);
    // Hann: coherent gain 0.5; a sine of amplitude A gives |X| = A·n/4 at its bin,
    // spread over neighbouring bins with total power A²·n²/16·1.5.
    let scale = 1.0 / ((n * n) as f64 / 16.0 * 1.5);
    let bin_hz = rate / n as f64;
    let half = 2f64.powf(0.5 / f64::from(per_octave));
    centres
        .iter()
        .map(|c| {
            let lo = ((c / half) / bin_hz).ceil() as usize;
            let hi = (((c * half) / bin_hz).floor() as usize).min(n / 2);
            let p: f64 = (lo.max(1)..=hi.max(lo.max(1)))
                .map(|k| re[k] * re[k] + im[k] * im[k])
                .sum::<f64>()
                * scale;
            if p > 0.0 {
                (10.0 * p.log10()).max(-200.0)
            } else {
                -200.0
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transform_matches_the_direct_dft() {
        let n = 64;
        let fft = Fft::new(n);
        let x: Vec<f64> = (0..n).map(|i| ((i * 7 % 13) as f64 - 6.0) / 6.0).collect();
        let (mut re, mut im) = (x.clone(), vec![0.0; n]);
        fft.run(&mut re, &mut im);
        for k in 0..n {
            let (mut r, mut i) = (0.0, 0.0);
            for (t, v) in x.iter().enumerate() {
                let a = -2.0 * PI * (k * t) as f64 / n as f64;
                r += v * a.cos();
                i += v * a.sin();
            }
            assert!((re[k] - r).abs() < 1e-9 && (im[k] - i).abs() < 1e-9);
        }
    }

    #[test]
    fn a_sine_reads_its_level_in_its_band() {
        let fft = Fft::new(4096);
        let centres = band_centres(6);
        assert_eq!(centres.len(), 59);
        assert!(centres[0] >= 20.0 && *centres.last().unwrap() <= 20_000.0);
        let k = centres
            .iter()
            .position(|c| (*c - 1000.0).abs() < 1e-6)
            .unwrap();
        let x: Vec<f64> = (0..4096)
            .map(|i| 0.5 * (2.0 * PI * 1000.0 * i as f64 / 48_000.0).sin())
            .collect();
        let b = bands(&fft, &x, 48_000.0, &centres, 6);
        assert!((b[k] + 6.02).abs() < 0.5, "{}", b[k]);
        assert!(
            b[k - 3] < -60.0 && b[k + 3] < -60.0,
            "{} {}",
            b[k - 3],
            b[k + 3]
        );
    }
}
