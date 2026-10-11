//! Dynamics building blocks (§6).
//!
//! Gain computers and ballistics follow D. Giannoulis, M. Massberg, J. D. Reiss,
//! "Digital Dynamic Range Compressor Design — A Tutorial and Analysis", JAES 60(6),
//! 2012: a feed-forward design whose gain computer works in dB with a quadratic soft
//! knee, followed by a smooth branching detector on the gain (log domain).
//!
//! Detector types:
//! - peak: the instantaneous |x| (linked: the largest over the channels);
//! - RMS: the mean square smoothed with the attack (rising) and release (falling)
//!   time constants.
//!
//! Attack and release times are time constants: a step reaches 1 − 1/e (63 %) of
//! its final value in dB after that time.

use crate::config::SAMPLE_RATE;

/// −∞ for practical purposes, in dB.
pub const FLOOR_DB: f64 = -200.0;

pub fn to_db(level: f64) -> f64 {
    if level > 1e-10 {
        20.0 * level.log10()
    } else {
        FLOOR_DB
    }
}

pub fn from_db(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

/// The per-sample coefficient of a one-pole smoother with time constant `ms`.
pub fn coefficient(ms: f64) -> f64 {
    if ms <= 0.0 {
        0.0
    } else {
        (-1.0 / (ms * 1e-3 * f64::from(SAMPLE_RATE))).exp()
    }
}

/// Static curve of a downward compressor: the gain in dB (≤ 0) for input level
/// `x` dB, threshold `t`, ratio `r` and knee width `w` dB.
pub fn compressor_gain(x: f64, t: f64, r: f64, w: f64) -> f64 {
    let over = x - t;
    let y = if 2.0 * over < -w {
        x
    } else if w > 0.0 && 2.0 * over.abs() <= w {
        x + (1.0 / r - 1.0) * (over + w / 2.0).powi(2) / (2.0 * w)
    } else {
        t + over / r
    };
    (y - x).min(0.0)
}

/// Static curve of a downward expander: below the threshold the level falls `r`
/// times as fast, down to `range` dB of attenuation.
pub fn expander_gain(x: f64, t: f64, r: f64, range: f64) -> f64 {
    if x >= t {
        0.0
    } else {
        ((x - t) * (r - 1.0)).max(-range)
    }
}

/// Smooth branching detector (attack when the value falls, as a gain does when it
/// reduces).
#[derive(Debug, Clone, Copy)]
pub struct Ballistics {
    pub attack: f64,
    pub release: f64,
}

impl Ballistics {
    pub fn new(attack_ms: f64, release_ms: f64) -> Self {
        Self {
            attack: coefficient(attack_ms),
            release: coefficient(release_ms),
        }
    }

    /// Move `state` (a gain in dB) toward `target`: attack while reducing,
    /// release while recovering.
    #[inline]
    pub fn gain(&self, state: f64, target: f64) -> f64 {
        let a = if target < state {
            self.attack
        } else {
            self.release
        };
        a * state + (1.0 - a) * target
    }

    /// Move `state` (a level or mean square) toward `target`: attack while rising.
    #[inline]
    pub fn level(&self, state: f64, target: f64) -> f64 {
        let a = if target > state {
            self.attack
        } else {
            self.release
        };
        a * state + (1.0 - a) * target
    }
}

/// Sliding minimum over the last `len` values, O(1) per value amortised, with
/// storage allocated up front.
#[derive(Debug, Clone)]
pub struct SlidingMin {
    values: Vec<f64>,
    stamps: Vec<u64>,
    head: usize,
    count: usize,
    len: u64,
    now: u64,
}

impl SlidingMin {
    pub fn new(len: usize) -> Self {
        let cap = len.max(1) + 1;
        Self {
            values: vec![0.0; cap],
            stamps: vec![0; cap],
            head: 0,
            count: 0,
            len: len.max(1) as u64,
            now: 0,
        }
    }

    /// Push `v` and return the minimum of the last `len` values.
    pub fn push(&mut self, v: f64) -> f64 {
        let cap = self.values.len();
        // Drop from the back while larger (monotonic deque).
        while self.count > 0 {
            let back = (self.head + self.count - 1) % cap;
            if self.values[back] >= v {
                self.count -= 1;
            } else {
                break;
            }
        }
        let at = (self.head + self.count) % cap;
        self.values[at] = v;
        self.stamps[at] = self.now;
        self.count += 1;
        while self.stamps[self.head] + self.len <= self.now {
            self.head = (self.head + 1) % cap;
            self.count -= 1;
        }
        self.now += 1;
        self.values[self.head]
    }
}

/// A look-ahead peak limiter's gain: the sliding minimum of the required gain over
/// the look-ahead plus one sample, averaged over the look-ahead (the attack), with
/// an exponential release. A sample delayed by the look-ahead and multiplied by
/// this gain never exceeds the ceiling.
#[derive(Debug, Clone)]
pub struct LookaheadGain {
    min: SlidingMin,
    window: Vec<f64>,
    at: usize,
    sum: f64,
    gain: f64,
    release: f64,
}

impl LookaheadGain {
    pub fn new(lookahead: usize) -> Self {
        let n = lookahead.max(1);
        Self {
            min: SlidingMin::new(n + 1),
            window: vec![1.0; n],
            at: 0,
            sum: n as f64,
            gain: 1.0,
            release: 0.0,
        }
    }

    pub fn set_release(&mut self, ms: f64) {
        self.release = coefficient(ms);
    }

    /// The gain for the sample leaving the delay, given the peak of the sample
    /// entering it.
    #[inline]
    pub fn next(&mut self, peak: f64, ceiling: f64) -> f64 {
        let required = if peak > ceiling { ceiling / peak } else { 1.0 };
        let held = self.min.push(required);
        self.sum += held - self.window[self.at];
        self.window[self.at] = held;
        self.at = (self.at + 1) % self.window.len();
        let smooth = self.sum / self.window.len() as f64;
        self.gain = if smooth < self.gain {
            smooth
        } else {
            self.release * self.gain + (1.0 - self.release) * smooth
        };
        self.gain
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compressor_static_curve() {
        // Below the knee: unity. Above: (x − t)/r. In the knee: the quadratic.
        assert_eq!(compressor_gain(-40.0, -20.0, 4.0, 6.0), 0.0);
        assert!((compressor_gain(-10.0, -20.0, 4.0, 0.0) + 7.5).abs() < 1e-12);
        assert!((compressor_gain(0.0, -20.0, 4.0, 6.0) + 15.0).abs() < 1e-12);
        let at_t = compressor_gain(-20.0, -20.0, 4.0, 6.0);
        assert!((at_t - (0.25 - 1.0) * 9.0 / 12.0).abs() < 1e-12);
        // Continuous at the knee edges.
        for edge in [-23.0, -17.0] {
            let below = compressor_gain(edge - 1e-9, -20.0, 4.0, 6.0);
            let above = compressor_gain(edge + 1e-9, -20.0, 4.0, 6.0);
            assert!((below - above).abs() < 1e-6);
        }
        // ∞:1 holds the output at the threshold.
        assert!((compressor_gain(-5.0, -20.0, f64::INFINITY, 0.0) + 15.0).abs() < 1e-12);
    }

    #[test]
    fn expander_static_curve() {
        assert_eq!(expander_gain(-10.0, -40.0, 2.0, 30.0), 0.0);
        assert!((expander_gain(-50.0, -40.0, 2.0, 30.0) + 10.0).abs() < 1e-12);
        assert!((expander_gain(-80.0, -40.0, 2.0, 30.0) + 30.0).abs() < 1e-12);
    }

    #[test]
    fn time_constants_reach_63_percent() {
        let b = Ballistics::new(10.0, 100.0);
        let mut g = 0.0;
        for _ in 0..480 {
            g = b.gain(g, -10.0);
        }
        assert!((g + 10.0 * (1.0 - (-1f64).exp())).abs() < 0.01, "{g}");
        let start = g;
        for _ in 0..4800 {
            g = b.gain(g, 0.0);
        }
        // After 100 ms of release, 63 % of the way back.
        assert!((g - start * (-1f64).exp()).abs() < 0.01, "{g}");
    }

    #[test]
    fn sliding_minimum() {
        let mut m = SlidingMin::new(3);
        let out: Vec<f64> = [5.0, 3.0, 4.0, 6.0, 7.0, 2.0, 8.0, 9.0, 9.0]
            .iter()
            .map(|v| m.push(*v))
            .collect();
        assert_eq!(out, [5.0, 3.0, 3.0, 3.0, 4.0, 2.0, 2.0, 2.0, 8.0]);
    }

    #[test]
    fn lookahead_limiter_never_exceeds_the_ceiling() {
        let n = 72;
        let mut g = LookaheadGain::new(n);
        g.set_release(50.0);
        let mut delay = vec![0.0f64; n];
        let mut max_out = 0f64;
        for k in 0..20_000usize {
            // Bursts up to +12 dB over the ceiling 0.5, with single-sample spikes.
            let x = if k % 997 == 0 {
                2.0
            } else {
                (k as f64 * 0.05).sin() * if (k / 3000) % 2 == 0 { 0.3 } else { 1.6 }
            };
            let out = delay[k % n] * g.next(x.abs(), 0.5);
            delay[k % n] = x;
            max_out = max_out.max(out.abs());
        }
        assert!(max_out <= 0.5 + 1e-9, "{max_out}");
        assert!(max_out > 0.49);
    }
}
