//! The analyser (§7): spectrum and goniometer of one tap point, computed on the
//! control side from the blocks the DSP thread copies into a ring.
//!
//! Every meter tick it takes the new blocks and, with an analysed signal,
//! computes a 4096-point spectrum (Hann) in 1/6-octave bands (the mean power of
//! the first two channels), their correlation, and the last 256 sample pairs for
//! the goniometer.

use serde::Serialize;

use crate::dsp::fft::{band_centres, bands, Fft};
use crate::mix::model::Tap;

pub const FFT_SIZE: usize = 4096;
pub const BANDS_PER_OCTAVE: u32 = 6;
pub const XY_POINTS: usize = 256;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct AnalysisValues {
    pub signal: String,
    pub tap: Tap,
    pub bands_hz: Vec<f64>,
    pub spectrum_db: Vec<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation: Option<f64>,
    /// Channel 1 and channel 2 (or the mono channel twice), oldest first.
    pub xy: Vec<[f32; 2]>,
}

pub struct Analyser {
    pub target: Option<(String, Tap)>,
    block: usize,
    history: [Vec<f32>; 2],
    pos: usize,
    filled: usize,
    fft: Fft,
    centres: Vec<f64>,
    scratch: Vec<f32>,
    values: Option<AnalysisValues>,
}

impl Analyser {
    pub fn new(block: usize) -> Self {
        Self {
            target: None,
            block,
            history: [vec![0.0; FFT_SIZE], vec![0.0; FFT_SIZE]],
            pos: 0,
            filled: 0,
            fft: Fft::new(FFT_SIZE),
            centres: band_centres(BANDS_PER_OCTAVE),
            scratch: vec![0.0; 2 * block],
            values: None,
        }
    }

    pub fn set_target(&mut self, target: Option<(String, Tap)>) {
        self.target = target;
        self.filled = 0;
        self.values = None;
    }

    fn push(&mut self, ch: usize, samples: &[f32]) {
        let mut p = self.pos;
        for &x in samples {
            self.history[ch][p] = x;
            p = (p + 1) % FFT_SIZE;
        }
    }

    /// Take the blocks in `ring`; recompute when there were any.
    pub fn tick(&mut self, ring: &mut rtrb::Consumer<f32>) {
        let mut fresh = false;
        while ring.slots() >= 2 * self.block {
            let Ok(chunk) = ring.read_chunk(2 * self.block) else {
                break;
            };
            let (a, b) = chunk.as_slices();
            let n = a.len();
            self.scratch[..n].copy_from_slice(a);
            self.scratch[n..].copy_from_slice(b);
            chunk.commit_all();
            if self.target.is_none() {
                continue;
            }
            let (l, r) = self.scratch.split_at(self.block);
            let (l, r) = (l.to_vec(), r.to_vec());
            self.push(0, &l);
            self.push(1, &r);
            self.pos = (self.pos + self.block) % FFT_SIZE;
            self.filled = (self.filled + self.block).min(FFT_SIZE);
            fresh = true;
        }
        if fresh {
            self.compute();
        }
    }

    fn ordered(&self, ch: usize) -> impl Iterator<Item = f32> + '_ {
        (0..FFT_SIZE).map(move |i| self.history[ch][(self.pos + i) % FFT_SIZE])
    }

    fn compute(&mut self) {
        let Some((signal, tap)) = self.target.clone() else {
            return;
        };
        let rate = f64::from(crate::config::SAMPLE_RATE);
        let mut power = vec![0.0; self.centres.len()];
        for ch in 0..2 {
            let x: Vec<f64> = self.ordered(ch).map(f64::from).collect();
            for (p, db) in
                power
                    .iter_mut()
                    .zip(bands(&self.fft, &x, rate, &self.centres, BANDS_PER_OCTAVE))
            {
                *p += 10f64.powf(db / 10.0) / 2.0;
            }
        }
        let spectrum_db = power
            .iter()
            .map(|p| {
                if *p > 0.0 {
                    (10.0 * p.log10()).max(-200.0)
                } else {
                    -200.0
                }
            })
            .map(|d| (d * 10.0).round() / 10.0)
            .collect();
        let n = self.filled.min(FFT_SIZE);
        let (mut ll, mut rr, mut lr) = (0f64, 0f64, 0f64);
        for (l, r) in self.ordered(0).zip(self.ordered(1)).skip(FFT_SIZE - n) {
            ll += f64::from(l) * f64::from(l);
            rr += f64::from(r) * f64::from(r);
            lr += f64::from(l) * f64::from(r);
        }
        let d = (ll * rr).sqrt();
        let xy = self
            .ordered(0)
            .zip(self.ordered(1))
            .skip(FFT_SIZE - XY_POINTS)
            .map(|(l, r)| [l, r])
            .collect();
        self.values = Some(AnalysisValues {
            signal,
            tap,
            bands_hz: self
                .centres
                .iter()
                .map(|c| (c * 10.0).round() / 10.0)
                .collect(),
            spectrum_db,
            correlation: (d > 1e-12).then(|| (lr / d).clamp(-1.0, 1.0)),
            xy,
        });
    }

    pub fn values(&self) -> Option<&AnalysisValues> {
        self.values.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tone_in_one_channel_and_its_inverse() {
        let block = 96;
        let (mut tx, mut rx) = rtrb::RingBuffer::new(crate::engine::ANALYSIS_RING);
        let mut a = Analyser::new(block);
        a.set_target(Some(("st".into(), Tap::PostFader)));
        let mut t = 0usize;
        for _ in 0..(FFT_SIZE / block + 2) {
            let l: Vec<f32> = (0..block)
                .map(|i| {
                    (0.5 * (2.0 * std::f64::consts::PI * 1000.0 * (t + i) as f64 / 48_000.0).sin())
                        as f32
                })
                .collect();
            t += block;
            for x in &l {
                tx.push(*x).unwrap();
            }
            for x in &l {
                tx.push(-*x).unwrap();
            }
        }
        a.tick(&mut rx);
        let v = a.values().unwrap();
        let k = v
            .bands_hz
            .iter()
            .position(|f| (*f - 1000.0).abs() < 0.1)
            .unwrap();
        assert!((v.spectrum_db[k] + 6.0).abs() < 0.5, "{}", v.spectrum_db[k]);
        assert!(v.spectrum_db[k + 4] < -60.0);
        assert!((v.correlation.unwrap() + 1.0).abs() < 1e-6);
        assert_eq!(v.xy.len(), XY_POINTS);
        assert!(v.xy.iter().all(|[l, r]| (l + r).abs() < 1e-6));
    }
}
