//! Speech-aware loudness (§8.2): a signal's gain rides its loudness to a target.
//!
//! Per 10 ms frame, from the signal's tap (before its auto gain, feed-forward):
//! - while the momentary loudness is below `freeze_lufs` there is no signal:
//!   the gain stays (`frozen`), the frames are not measured, and neither are
//!   the 400 ms that brought the momentary loudness below it (the pause);
//! - the level is the short-term loudness (BS.1770, the mean of the last 3 s of
//!   frames with signal: pauses shorter than 400 ms count, as in short-term
//!   loudness);
//! - the target is `target_lufs`, plus `speech_offset_lu` while the signal's
//!   VAD reports speech;
//! - the gain goes towards target − level, within `min_gain_db` …
//!   `max_gain_db`, as a one-pole in dB: with the time constant `attack_ms`
//!   down, `release_ms` up.

use std::collections::VecDeque;

use serde::Serialize;

use super::level::{lufs, Reading};
use crate::mix::auto::AutoLevel;

const FRAME_MS: f64 = 10.0;
const SHORT_TERM: usize = 300;
const MOMENTARY: usize = 40;

#[derive(Debug, Clone, Default)]
pub struct State {
    pub gain_db: f64,
    powers: VecDeque<f64>,
    pub status: LevelStatus,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct LevelStatus {
    pub gain_db: f64,
    /// The short-term loudness of the frames with signal, LUFS.
    pub level_lufs: Option<f64>,
    /// The target in force.
    pub target_lufs: f64,
    pub frozen: bool,
    pub speech: bool,
}

fn one_pole(ms: f64) -> f64 {
    if ms <= 0.0 {
        1.0
    } else {
        1.0 - (-FRAME_MS / ms).exp()
    }
}

impl State {
    /// One frame of the signal's `reading`; `speech`: its VAD.
    pub fn frame(&mut self, l: &AutoLevel, reading: &Reading, speech: bool) {
        let target = l.target_lufs + if speech { l.speech_offset_lu } else { 0.0 };
        if !l.enabled {
            *self = Self::default();
            self.status.target_lufs = target;
            return;
        }
        let frozen = reading.momentary < l.freeze_lufs;
        if frozen && !self.status.frozen {
            // The momentary window that fell below the freeze level was the
            // pause: out of the measurement.
            let keep = self.powers.len().saturating_sub(MOMENTARY);
            self.powers.truncate(keep);
        }
        if !frozen {
            if self.powers.len() == SHORT_TERM {
                self.powers.pop_front();
            }
            self.powers.push_back(reading.power);
        }
        let level = (!self.powers.is_empty())
            .then(|| lufs(self.powers.iter().sum::<f64>() / self.powers.len() as f64));
        if let (false, Some(level)) = (frozen, level) {
            let want = (target - level).clamp(l.min_gain_db, l.max_gain_db);
            let a = if want < self.gain_db {
                one_pole(l.attack_ms)
            } else {
                one_pole(l.release_ms)
            };
            self.gain_db += a * (want - self.gain_db);
        }
        self.gain_db = self.gain_db.clamp(l.min_gain_db, l.max_gain_db);
        self.status = LevelStatus {
            gain_db: (self.gain_db * 100.0).round() / 100.0 + 0.0,
            level_lufs: level.map(|x| (x * 10.0).round() / 10.0),
            target_lufs: target,
            frozen,
            speech,
        };
    }

    pub fn gain(&self) -> f64 {
        10f64.powf(self.gain_db / 20.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auto::level::Level;
    use crate::dsp::loudness::weights;

    /// A 1 kHz tone at `lufs` (mono), one frame.
    fn tone(level: &mut Level, lufs: f64, k: usize) -> Reading {
        let peak = 10f64.powf((lufs + 3.01) / 20.0);
        let x: Vec<f32> = (0..480)
            .map(|n| {
                let t = (k * 480 + n) as f64 / 48_000.0;
                (peak * (2.0 * std::f64::consts::PI * 1000.0 * t).sin()) as f32
            })
            .collect();
        level.frame(&[&x])
    }

    #[test]
    fn rides_to_the_target() {
        let l = AutoLevel::new("vox");
        let mut s = State::default();
        let mut level = Level::new(weights(&["M"]));
        // −30 LUFS, no speech: +7 dB to −23.
        for k in 0..3000 {
            s.frame(&l, &tone(&mut level, -30.0, k), false);
        }
        assert!((s.gain_db - 7.0).abs() < 0.05, "{:?}", s.status);
        assert_eq!(s.status.level_lufs, Some(-30.0));
        // Speech: +2 LU, 9 dB; up with the 3 s release.
        let mut trace = Vec::new();
        for k in 0..3000 {
            s.frame(&l, &tone(&mut level, -30.0, k), true);
            trace.push(s.gain_db);
        }
        assert!(
            (trace[299] - (7.0 + 2.0 * 0.632)).abs() < 0.05,
            "{}",
            trace[299]
        );
        assert!((s.gain_db - 9.0).abs() < 0.05);
        // Louder than the range allows: −45 needs +22, at most +12.
        for k in 0..3000 {
            s.frame(&l, &tone(&mut level, -45.0, k), false);
        }
        assert!((s.gain_db - 12.0).abs() < 0.01 && s.gain_db <= 12.0);
        // Down with the 1 s attack: −10 LUFS needs −13, at least −12.
        let mut trace = Vec::new();
        for k in 0..2000 {
            s.frame(&l, &tone(&mut level, -10.0, k), false);
            trace.push(s.gain_db);
        }
        assert!(trace[100] < 5.0, "{}", trace[100]);
        assert!((s.gain_db + 12.0).abs() < 0.01 && s.gain_db >= -12.0);
    }

    #[test]
    fn freezes_without_signal() {
        let l = AutoLevel::new("vox");
        let mut s = State::default();
        let mut level = Level::new(weights(&["M"]));
        for k in 0..1000 {
            s.frame(&l, &tone(&mut level, -30.0, k), false);
        }
        let before = s.gain_db;
        // −60 LUFS (below the freeze level): the gain keeps working until the
        // momentary loudness falls below −50 (the first 400 ms of the pause,
        // hardly: 3 s up), then stays.
        let mut frozen_at = None;
        for k in 0..1000 {
            s.frame(&l, &tone(&mut level, -60.0, k), false);
            if s.status.frozen && frozen_at.is_none() {
                frozen_at = Some((k, s.gain_db));
            }
        }
        let (k, g) = frozen_at.expect("frozen");
        assert!(k < 40, "{k}");
        assert!((g - before).abs() < 0.1, "{g} {before}");
        assert_eq!(s.gain_db, g);
        // The level after the pause is still that of the signal.
        for k in 0..41 {
            s.frame(&l, &tone(&mut level, -30.0, k), false);
        }
        assert_eq!(s.status.level_lufs, Some(-30.0));
        // Disabled: 0 dB.
        let mut off = l.clone();
        off.enabled = false;
        s.frame(&off, &tone(&mut level, -30.0, 0), false);
        assert_eq!(s.gain(), 1.0);
    }
}
