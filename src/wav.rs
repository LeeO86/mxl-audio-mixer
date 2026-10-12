//! A minimal WAV reader for the test tools (speech for the VAD tests): 16-bit
//! PCM or 32-bit float, any channel count, resampled to 48 kHz (linear
//! interpolation; good enough for test signals).

use std::path::Path;

pub struct Wav {
    pub rate: u32,
    pub channels: usize,
    /// Interleaved samples at `rate`.
    pub samples: Vec<f32>,
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

impl Wav {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.get(..4) != Some(b"RIFF") || bytes.get(8..12) != Some(b"WAVE") {
            return Err("not a RIFF/WAVE file".into());
        }
        let mut at = 12;
        let mut format = None;
        while at + 8 <= bytes.len() {
            let id = &bytes[at..at + 4];
            let len = u32_at(bytes, at + 4).unwrap_or(0) as usize;
            let body = at + 8;
            let end = (body + len).min(bytes.len());
            match id {
                b"fmt " => {
                    let tag = u16_at(bytes, body).ok_or("short fmt chunk")?;
                    let channels = u16_at(bytes, body + 2).ok_or("short fmt chunk")?;
                    let rate = u32_at(bytes, body + 4).ok_or("short fmt chunk")?;
                    let bits = u16_at(bytes, body + 14).ok_or("short fmt chunk")?;
                    let tag = if tag == 0xfffe {
                        u16_at(bytes, body + 24).ok_or("short extensible fmt chunk")?
                    } else {
                        tag
                    };
                    format = Some((tag, channels as usize, rate, bits));
                }
                b"data" => {
                    let (tag, channels, rate, bits) = format.ok_or("data before fmt")?;
                    let data = &bytes[body..end];
                    let samples: Vec<f32> = match (tag, bits) {
                        (1, 16) => data
                            .chunks_exact(2)
                            .map(|b| f32::from(i16::from_le_bytes([b[0], b[1]])) / 32_768.0)
                            .collect(),
                        (3, 32) => data
                            .chunks_exact(4)
                            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                            .collect(),
                        _ => {
                            return Err(format!(
                                "format {tag} with {bits} bits: only 16-bit PCM and 32-bit float"
                            ))
                        }
                    };
                    if channels == 0 || rate == 0 {
                        return Err("no channels or no sample rate".into());
                    }
                    return Ok(Self {
                        rate,
                        channels,
                        samples,
                    });
                }
                _ => {}
            }
            at = body + len + (len & 1);
        }
        Err("no data chunk".into())
    }

    pub fn read(path: &Path) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::parse(&bytes).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Channel `c` (the last one for higher `c`) at 48 kHz.
    pub fn channel_48k(&self, c: usize) -> Vec<f32> {
        let c = c.min(self.channels - 1);
        let x: Vec<f32> = self
            .samples
            .iter()
            .skip(c)
            .step_by(self.channels)
            .copied()
            .collect();
        if self.rate == 48_000 || x.is_empty() {
            return x;
        }
        let step = f64::from(self.rate) / 48_000.0;
        let len = ((x.len() - 1) as f64 / step) as usize + 1;
        (0..len)
            .map(|i| {
                let t = i as f64 * step;
                let k = t as usize;
                let f = (t - k as f64) as f32;
                let a = x[k];
                let b = x.get(k + 1).copied().unwrap_or(a);
                a + (b - a) * f
            })
            .collect()
    }

    /// The mean of the channels at 48 kHz.
    pub fn mono_48k(&self) -> Vec<f32> {
        let mut out = self.channel_48k(0);
        for c in 1..self.channels {
            for (o, v) in out.iter_mut().zip(self.channel_48k(c)) {
                *o += v;
            }
        }
        let n = self.channels as f32;
        out.iter_mut().for_each(|v| *v /= n);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(rate: u32, channels: u16, samples: &[i16]) -> Vec<u8> {
        let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + 12 + data.len() as u32).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&channels.to_le_bytes());
        b.extend_from_slice(&rate.to_le_bytes());
        b.extend_from_slice(&(rate * 2 * u32::from(channels)).to_le_bytes());
        b.extend_from_slice(&(2 * channels).to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"LIST");
        b.extend_from_slice(&3u32.to_le_bytes());
        b.extend_from_slice(b"abc\0");
        b.extend_from_slice(b"data");
        b.extend_from_slice(&(data.len() as u32).to_le_bytes());
        b.extend_from_slice(&data);
        b
    }

    #[test]
    fn chunks_channels_and_rate() {
        let w = Wav::parse(&wav(24_000, 2, &[16_384, -16_384, 0, 0, -16_384, 16_384])).unwrap();
        assert_eq!((w.rate, w.channels, w.samples.len()), (24_000, 2, 6));
        assert_eq!(w.channel_48k(0), [0.5, 0.25, 0.0, -0.25, -0.5]);
        assert_eq!(w.mono_48k(), [0.0; 5]);
        assert!(Wav::parse(b"RIFF....WAVEdata").is_err());
    }
}
