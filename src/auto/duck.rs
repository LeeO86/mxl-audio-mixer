//! Ducking by speech (§8.4): a duck's targets go down by its depth while one of
//! its keys speaks (their VADs), and come back after the hold.
//!
//! Per 10 ms frame the gain moves linearly in dB: from 0 dB to the depth in
//! `attack_ms`, back in `release_ms`, after `hold_ms` without speech. A depth of
//! −∞ ramps as −120 dB and is silence at its end. A target of several ducks
//! takes the deepest. The gain goes to the target's auto node (in its chain, by
//! default before the fader), ramped over the 10 ms to the next frame.

use crate::mix::auto::Duck;

const FRAME_MS: f64 = 10.0;
/// −∞ as a ramp's end.
pub const FLOOR_DB: f64 = -120.0;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct State {
    /// The current gain, dB (0 or down to the depth).
    pub gain_db: f64,
    /// A key spoke in the last frame.
    pub active: bool,
    /// Hold left, ms.
    hold_ms: f64,
}

impl State {
    /// One frame; `speech`: a key speaks.
    pub fn frame(&mut self, d: &Duck, speech: bool) {
        self.active = d.enabled && speech;
        if !d.enabled {
            *self = Self::default();
            return;
        }
        let depth = d.depth_db.0.max(FLOOR_DB);
        let target = if speech {
            self.hold_ms = d.hold_ms;
            depth
        } else if self.hold_ms > 0.0 {
            self.hold_ms -= FRAME_MS;
            depth
        } else {
            0.0
        };
        let span = -depth;
        let step = |ms: f64| {
            if ms <= 0.0 {
                f64::INFINITY
            } else {
                span * FRAME_MS / ms
            }
        };
        if target < self.gain_db {
            self.gain_db = (self.gain_db - step(d.attack_ms)).max(target);
        } else if target > self.gain_db {
            self.gain_db = (self.gain_db + step(d.release_ms)).min(target);
        }
    }

    /// The gain as a factor.
    pub fn gain(&self) -> f64 {
        if self.gain_db <= FLOOR_DB {
            0.0
        } else {
            10f64.powf(self.gain_db / 20.0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mix::model::Db;

    #[test]
    fn attack_hold_release() {
        let mut d = Duck::new("d");
        d.depth_db = Db(-20.0);
        d.attack_ms = 100.0;
        d.hold_ms = 200.0;
        d.release_ms = 400.0;
        let mut s = State::default();
        let mut trace = Vec::new();
        for k in 0..100 {
            s.frame(&d, k < 30);
            trace.push(s.gain_db);
        }
        // Attack: −2 dB per frame, at the depth after 10 frames.
        assert!((trace[0] + 2.0).abs() < 1e-9 && trace[9] == -20.0);
        // Speech ends after frame 29; the hold keeps the depth 20 frames.
        assert_eq!(trace[49], -20.0);
        // Release: +0.5 dB per frame for 40 frames.
        assert!((trace[50] + 19.5).abs() < 1e-9);
        assert_eq!(trace[89], 0.0);
        assert!(trace[88] < 0.0);
        // −∞: down to silence, immediately with an attack of 0.
        d.depth_db = Db::MINUS_INF;
        d.attack_ms = 0.0;
        let mut s = State::default();
        s.frame(&d, true);
        assert_eq!(s.gain(), 0.0);
        // Disabled: back to 0 dB at once.
        d.enabled = false;
        s.frame(&d, true);
        assert_eq!((s.gain(), s.active), (1.0, false));
    }
}
