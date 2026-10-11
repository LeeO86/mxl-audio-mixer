//! The test signal: a sine per channel whose value depends only on the absolute
//! sample index, so a reader can check every sample it gets.

const RATE: f64 = 48_000.0;

/// Channel `c` carries 1 kHz × (c + 1) at −20 dBFS.
pub fn sample(channel: usize, index: u64) -> f32 {
    let freq = 1_000.0 * (channel as f64 + 1.0);
    let period = (RATE / 1_000.0) as u64; // 48 samples: a whole number of cycles for every channel
    let phase = (index % period) as f64 / RATE;
    (0.1 * (2.0 * std::f64::consts::PI * freq * phase).sin()) as f32
}

#[cfg(test)]
mod tests {
    #[test]
    fn deterministic_and_periodic() {
        assert_eq!(super::sample(0, 5), super::sample(0, 5 + 48 * 1000));
        assert!(super::sample(1, 3).abs() <= 0.1);
        assert_ne!(super::sample(0, 1), super::sample(1, 1));
    }
}
