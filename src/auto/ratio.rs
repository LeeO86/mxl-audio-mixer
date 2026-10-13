//! Ratio ducks (§8.4, `mode: "ratio"`): Com-IS-Assist's control (`comis`). While
//! a key speaks, the targets (the bed) go as far down as keeps the keys'
//! (the dialogue's) loudness `target_lu` above theirs; `overvoice_lu` while a
//! target speaks too. Without a key's speech they come back; with a target's
//! speech alone four times faster and without the ease-in (interview
//! passthrough).
//!
//! Every reading is of the dry signals (feed-forward, the VAD taps: before the
//! auto gains, the auto-level's gain added): reducing the bed by 1 dB raises the
//! ratio by 1 LU, so each stage's reduction is `target − (dialogue − bed)` at
//! its time scale: the slow stage from the bed's short-term loudness (3 s), the
//! mid stage from its momentary loudness (400 ms, ±6 dB on the slow one), the
//! fast stage from a 50 ms detector (up to 4 dB more), all against the
//! dialogue's short-term loudness. Several keys count as one dialogue (their
//! powers add), several targets as one bed.
//!
//! Each 10 ms frame steps every stage once (Com-IS-Assist steps the slow and
//! mid stages every 100 ms and the fast one every 64 samples; its envelopes are
//! one-pole filters over the elapsed time, so the step changes nothing but the
//! fast stage's resolution, which the 10 ms gain ramp has anyway).

use serde::Serialize;

use super::comis::engine::{AutomixEngine, AutomixEngineConfig, LoudnessSnapshot};
use super::comis::mix_state::MixState;
use super::comis::SILENCE_LUFS;
use crate::mix::auto::Duck;

const FRAME_S: f64 = 0.01;

/// What one frame of a ratio duck works from, LUFS.
#[derive(Debug, Clone, Copy)]
pub struct Inputs {
    pub key_voice: bool,
    pub target_voice: bool,
    pub dialogue_short_term: f64,
    pub bed_short_term: f64,
    pub bed_momentary: f64,
    pub bed_fast: f64,
}

impl Inputs {
    pub fn silent() -> Self {
        Self {
            key_voice: false,
            target_voice: false,
            dialogue_short_term: SILENCE_LUFS,
            bed_short_term: SILENCE_LUFS,
            bed_momentary: SILENCE_LUFS,
            bed_fast: SILENCE_LUFS,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct RatioStatus {
    /// `release`, `duck`, `interview` or `overvoice`.
    pub state: &'static str,
    pub reduction_db: f64,
    pub slow_db: f64,
    pub mid_db: f64,
    pub fast_db: f64,
    /// The ratio aimed at, LU.
    pub target_lu: f64,
    /// The dialogue's loudness over the ducked bed's (short-term), LU.
    pub ratio_lu: Option<f64>,
    pub dialogue_lufs: Option<f64>,
    pub bed_lufs: Option<f64>,
}

fn state_name(s: MixState) -> &'static str {
    match s {
        MixState::ReleaseToUnity => "release",
        MixState::DuckToTarget => "duck",
        MixState::InterviewPassthrough => "interview",
        MixState::DuckToOvervoice => "overvoice",
    }
}

fn reading(l: f64) -> Option<f64> {
    (l > SILENCE_LUFS).then(|| (l * 10.0).round() / 10.0)
}

fn round(x: f64) -> f64 {
    (x * 100.0).round() / 100.0 + 0.0
}

fn config(d: &Duck) -> AutomixEngineConfig {
    AutomixEngineConfig {
        target_ratio_lu: d.ratio.target_lu,
        overvoice_ratio_lu: d.ratio.overvoice_lu,
        max_gain_reduction_db: d.ratio.max_reduction_db,
        speed: d.ratio.speed,
        ..AutomixEngineConfig::default()
    }
}

pub struct State {
    engine: AutomixEngine,
    ratio: crate::mix::auto::Ratio,
    /// The last frame's reduction, dB (positive).
    pub reduction_db: f64,
    pub status: RatioStatus,
}

impl State {
    pub fn new(d: &Duck) -> Self {
        Self {
            engine: AutomixEngine::new(config(d)),
            ratio: d.ratio,
            reduction_db: 0.0,
            status: RatioStatus {
                state: "release",
                target_lu: d.ratio.target_lu,
                ..RatioStatus::default()
            },
        }
    }

    /// One frame; returns the reduction, dB (positive).
    pub fn frame(&mut self, d: &Duck, i: &Inputs) -> f64 {
        if d.ratio != self.ratio {
            self.ratio = d.ratio;
            self.engine.set_config(config(d));
        }
        if !d.enabled {
            self.engine.reset();
            self.reduction_db = 0.0;
            self.status = RatioStatus {
                state: "release",
                target_lu: d.ratio.target_lu,
                ..RatioStatus::default()
            };
            return 0.0;
        }
        let r = self.engine.process_tick(
            LoudnessSnapshot {
                bed_short_term_lufs: i.bed_short_term,
                bed_momentary_lufs: i.bed_momentary,
                dialogue_short_term_lufs: i.dialogue_short_term,
            },
            i.key_voice,
            i.target_voice && d.ratio.bed_voice,
            FRAME_S,
        );
        let reduction = self
            .engine
            .process_fast(i.bed_fast, i.dialogue_short_term, FRAME_S);
        let c = self.engine.contributions();
        let dry = (i.dialogue_short_term > SILENCE_LUFS && i.bed_short_term > SILENCE_LUFS)
            .then_some(i.dialogue_short_term - i.bed_short_term);
        self.status = RatioStatus {
            state: state_name(r.state),
            reduction_db: round(reduction),
            slow_db: round(c.slow_db),
            mid_db: round(c.mid_db),
            fast_db: round(c.fast_db),
            target_lu: self.engine.active_target_ratio_lu(),
            ratio_lu: dry.map(|x| (round(x + reduction) * 10.0).round() / 10.0),
            dialogue_lufs: reading(i.dialogue_short_term),
            bed_lufs: reading(i.bed_short_term),
        };
        self.reduction_db = reduction;
        reduction
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mix::auto::DuckMode;

    fn duck() -> Duck {
        let mut d = Duck::new("bed");
        d.mode = DuckMode::Ratio;
        d
    }

    fn inputs(key: bool, target: bool, dialogue: f64, bed: f64) -> Inputs {
        Inputs {
            key_voice: key,
            target_voice: target,
            dialogue_short_term: dialogue,
            bed_short_term: bed,
            bed_momentary: bed,
            bed_fast: bed,
        }
    }

    /// Frames until the reduction is within 0.5 dB of `want`.
    fn settle(s: &mut State, d: &Duck, i: &Inputs, want: f64) -> usize {
        for k in 0..2000 {
            if (s.frame(d, i) - want).abs() < 0.5 {
                return k;
            }
        }
        panic!("{:?}", s.status);
    }

    #[test]
    fn holds_the_ratio_and_lets_go() {
        let d = duck();
        let mut s = State::new(&d);
        // Dialogue −23, bed −20: 3 LU under the bed; 5 LU above needs 8 dB.
        let talk = inputs(true, false, -23.0, -20.0);
        let k = settle(&mut s, &d, &talk, 8.0);
        assert!(k < 100, "{k} frames");
        // The slow stage (2.5 s) takes over from the mid one in about 15 s.
        for _ in 0..1500 {
            s.frame(&d, &talk);
        }
        assert!((s.status.reduction_db - 8.0).abs() < 0.1, "{:?}", s.status);
        assert_eq!(s.status.state, "duck");
        assert_eq!(s.status.ratio_lu, Some(5.0));
        // The bed talks too (over-voice): 13 LU, 16 dB.
        let both = inputs(true, true, -23.0, -20.0);
        settle(&mut s, &d, &both, 16.0);
        assert_eq!((s.status.state, s.status.target_lu), ("overvoice", 13.0));
        // At most max_reduction_db.
        let loud = inputs(true, false, -23.0, 0.0);
        settle(&mut s, &d, &loud, 24.0);
        for _ in 0..300 {
            assert!(s.frame(&d, &loud) <= 24.0);
        }
        // Nobody talks: back to 0 dB, slowly at first (the ease-in).
        let quiet = inputs(false, false, -23.0, -20.0);
        let after_100ms: f64 = (0..10).map(|_| s.frame(&d, &quiet)).last().unwrap();
        assert!(after_100ms > 22.0, "{after_100ms}");
        let normal = settle(&mut s, &d, &quiet, 0.0);
        // The bed talks alone (interview): back faster.
        let mut s = State::new(&d);
        settle(&mut s, &d, &loud, 24.0);
        let interview = settle(&mut s, &d, &inputs(false, true, -23.0, -20.0), 0.0);
        assert_eq!(s.status.state, "interview");
        assert!(interview * 3 < normal, "{interview} vs {normal} frames");
        // Without bed_voice the bed's speech does not count.
        let mut d2 = duck();
        d2.ratio.bed_voice = false;
        let mut s = State::new(&d2);
        settle(&mut s, &d2, &loud, 24.0);
        s.frame(&d2, &inputs(false, true, -23.0, -20.0));
        assert_eq!(s.status.state, "release");
    }

    #[test]
    fn silence_and_disabled() {
        let mut d = duck();
        let mut s = State::new(&d);
        // A silent bed or dialogue asks for nothing.
        for _ in 0..300 {
            assert_eq!(s.frame(&d, &inputs(true, false, SILENCE_LUFS, -20.0)), 0.0);
        }
        assert_eq!(s.status.ratio_lu, None);
        settle(&mut s, &d, &inputs(true, false, -23.0, -20.0), 8.0);
        d.enabled = false;
        assert_eq!(s.frame(&d, &inputs(true, false, -23.0, -20.0)), 0.0);
        // Speed 4: there four times sooner.
        let mut d = duck();
        let mut slow = State::new(&d);
        let a = settle(&mut slow, &d, &inputs(true, false, -23.0, -20.0), 8.0);
        d.ratio.speed = 4.0;
        let mut fast = State::new(&d);
        let b = settle(&mut fast, &d, &inputs(true, false, -23.0, -20.0), 8.0);
        assert!(b * 2 < a, "{b} vs {a}");
    }
}
