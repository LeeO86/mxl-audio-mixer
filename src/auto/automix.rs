//! The gain-sharing auto-mixer (§8.3), after Dan Dugan's (US patent 3,814,856;
//! JAES 23(6), 1975): each member's gain follows its share of the total level.
//!
//! Per 10 ms frame, a member's level L is the mean square of its frame (the VAD
//! tap: before the fader), rising at once and falling with a time constant of
//! 100 ms; a member takes part from its first measured frame on. With the weight w (10^(weight_db/10)) and, with VAD weighting, its
//! speech probability p (counting p + 0.1 × (1 − p), so a member without speech
//! weighs 10 dB less), the member's gain is
//!
//! g_i = √(w_i L_i / Σ_j w_j L_j)
//!
//! so the gains' powers add up to 1: one talker is at 0 dB, two equal ones at
//! −3 dB each, and N members with equal noise at −10·log10(N) dB: the NOM
//! attenuation is part of the sharing, the total noise stays that of one open
//! microphone. A bypassed member is at 0 dB and out of the sharing; so is a
//! muted one (its strip mute, a VCA, a mute group, the fader at −∞), whose gain
//! stays where it was. With last-mic-hold, while no member speaks (its VAD), the
//! gains stay where they were: the room keeps the last talker's microphone.
//!
//! The gains go to each member's auto node (after its fader), ramped over the
//! 10 ms to the next frame; the operator's fader multiplies.

use std::collections::{BTreeMap, BTreeSet};

use crate::mix::auto::Automix;

/// Fall of a member's level per 10 ms frame: a time constant of 100 ms.
pub const LEVEL_FALL: f64 = 0.904_837_418_035_959_6; // e^(−10/100)
/// What a member without speech counts with VAD weighting (−10 dB).
pub const VAD_FLOOR: f64 = 0.1;

/// One member's frame as the auto-mixer sees it.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Input {
    /// Mean square of the frame.
    pub power: f64,
    pub probability: f64,
    pub active: bool,
}

/// The state of one auto-mix: each member's level and gain.
#[derive(Debug, Default, Clone)]
pub struct State {
    pub levels: BTreeMap<String, f64>,
    pub gains: BTreeMap<String, f64>,
}

impl State {
    /// One frame: `inputs` per member (members without one keep their level),
    /// `muted` the signals that are off. Updates and returns the gains.
    pub fn frame(
        &mut self,
        a: &Automix,
        inputs: &BTreeMap<String, Input>,
        muted: &BTreeSet<String>,
    ) -> &BTreeMap<String, f64> {
        self.levels
            .retain(|s, _| a.members.iter().any(|m| &m.signal == s));
        self.gains
            .retain(|s, _| a.members.iter().any(|m| &m.signal == s));
        for m in &a.members {
            if let Some(i) = inputs.get(&m.signal) {
                let l = self.levels.entry(m.signal.clone()).or_insert(0.0);
                *l = i.power.max(*l * LEVEL_FALL);
            }
        }
        if !a.enabled {
            for m in &a.members {
                self.gains.insert(m.signal.clone(), 1.0);
            }
            return &self.gains;
        }
        // A member shares once its level was measured (a new member's VAD tap
        // starts with the next plan).
        let sharing: Vec<_> = a
            .members
            .iter()
            .filter(|m| !m.bypass && !muted.contains(&m.signal))
            .filter(|m| self.levels.contains_key(&m.signal))
            .collect();
        for m in a.members.iter().filter(|m| m.bypass) {
            self.gains.insert(m.signal.clone(), 1.0);
        }
        let speaking = sharing
            .iter()
            .any(|m| inputs.get(&m.signal).is_some_and(|i| i.active));
        let known = sharing.iter().all(|m| self.gains.contains_key(&m.signal));
        if a.last_mic_hold && !speaking && known {
            return &self.gains;
        }
        let weighted: Vec<(String, f64)> = sharing
            .iter()
            .map(|m| {
                let mut w = 10f64.powf(m.weight_db / 10.0);
                if a.vad_weighting {
                    let p = inputs.get(&m.signal).map_or(0.0, |i| i.probability);
                    w *= p + VAD_FLOOR * (1.0 - p);
                }
                (m.signal.clone(), w * self.levels[&m.signal])
            })
            .collect();
        let total: f64 = weighted.iter().map(|(_, x)| x).sum();
        let n = weighted.len().max(1) as f64;
        for (s, x) in weighted {
            let g = if total > 1e-20 {
                (x / total).sqrt()
            } else {
                (1.0 / n).sqrt()
            };
            self.gains.insert(s, g);
        }
        &self.gains
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mix::auto::AutomixMember;

    fn automix(members: &[&str]) -> Automix {
        Automix {
            id: "am".into(),
            name: "am".into(),
            enabled: true,
            last_mic_hold: false,
            vad_weighting: false,
            members: members.iter().map(|m| AutomixMember::new(m)).collect(),
        }
    }

    fn inputs(list: &[(&str, f64, bool)]) -> BTreeMap<String, Input> {
        list.iter()
            .map(|(s, db, active)| {
                (
                    s.to_string(),
                    Input {
                        power: if db.is_finite() {
                            10f64.powf(db / 10.0)
                        } else {
                            0.0
                        },
                        probability: if *active { 1.0 } else { 0.0 },
                        active: *active,
                    },
                )
            })
            .collect()
    }

    fn db(g: f64) -> f64 {
        20.0 * g.log10()
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 0.01
    }

    #[test]
    fn sharing_nom_weights_bypass_and_mutes() {
        let mut a = automix(&["a", "b", "c", "d"]);
        let none = BTreeSet::new();
        let mut st = State::default();
        // Equal noise on all four: −6.02 dB each, the powers add up to 1.
        let g = st
            .frame(
                &a,
                &inputs(&[
                    ("a", -60.0, false),
                    ("b", -60.0, false),
                    ("c", -60.0, false),
                    ("d", -60.0, false),
                ]),
                &none,
            )
            .clone();
        assert!(g.values().all(|g| close(db(*g), -6.02)), "{g:?}");
        // One talker 30 dB above the others: him near 0 dB, the others 30 dB lower.
        let g = st
            .frame(
                &a,
                &inputs(&[
                    ("a", -30.0, true),
                    ("b", -60.0, false),
                    ("c", -60.0, false),
                    ("d", -60.0, false),
                ]),
                &none,
            )
            .clone();
        assert!(db(g["a"]) > -0.02 && close(db(g["b"]), -30.01), "{g:?}");
        assert!(close(g.values().map(|g| g * g).sum::<f64>(), 1.0));
        // Two equal talkers: −3 dB each.
        let mut st = State::default();
        let g = st
            .frame(
                &a,
                &inputs(&[
                    ("a", -30.0, true),
                    ("b", -30.0, true),
                    ("c", f64::NEG_INFINITY, false),
                    ("d", f64::NEG_INFINITY, false),
                ]),
                &none,
            )
            .clone();
        assert!(
            close(db(g["a"]), -3.01) && close(db(g["b"]), -3.01) && g["c"] == 0.0,
            "{g:?}"
        );
        // A weight of +6 dB on a: 3.98 : 1 of the power.
        a.members[0].weight_db = 6.0;
        let g = st
            .frame(
                &a,
                &inputs(&[
                    ("a", -30.0, true),
                    ("b", -30.0, true),
                    ("c", f64::NEG_INFINITY, false),
                    ("d", f64::NEG_INFINITY, false),
                ]),
                &none,
            )
            .clone();
        let wa = 10f64.powf(0.6);
        assert!(
            close(g["a"], (wa / (wa + 1.0)).sqrt()) && close(g["b"], (1.0 / (wa + 1.0)).sqrt()),
            "{g:?}"
        );
        a.members[0].weight_db = 0.0;
        // Bypass b: 0 dB, and a shares alone.
        a.members[1].bypass = true;
        let g = st
            .frame(
                &a,
                &inputs(&[
                    ("a", -30.0, true),
                    ("b", -30.0, true),
                    ("c", f64::NEG_INFINITY, false),
                    ("d", f64::NEG_INFINITY, false),
                ]),
                &none,
            )
            .clone();
        assert_eq!((g["a"], g["b"]), (1.0, 1.0));
        a.members[1].bypass = false;
        // Muted c and d: out of the sharing, their gains unchanged.
        let muted = BTreeSet::from(["c".to_string(), "d".to_string()]);
        let mut st = State::default();
        let g = st
            .frame(
                &a,
                &inputs(&[
                    ("a", -40.0, false),
                    ("b", -40.0, false),
                    ("c", -40.0, false),
                    ("d", -40.0, false),
                ]),
                &muted,
            )
            .clone();
        assert!(close(db(g["a"]), -3.01) && !g.contains_key("c"), "{g:?}");
        // Disabled: every member at 0 dB.
        a.enabled = false;
        let g = st.frame(&a, &inputs(&[]), &none).clone();
        assert!(g.values().all(|g| *g == 1.0) && g.len() == 4);
    }

    #[test]
    fn level_falls_with_100_ms_and_last_mic_hold() {
        let mut a = automix(&["a", "b"]);
        let none = BTreeSet::new();
        let mut st = State::default();
        st.frame(
            &a,
            &inputs(&[("a", -20.0, true), ("b", -60.0, false)]),
            &none,
        );
        // a stops: its level falls by e^(−1) in 100 ms (4.34 dB), so its gain
        // stays above b's for a while instead of swapping at once.
        for _ in 0..10 {
            st.frame(
                &a,
                &inputs(&[("a", -80.0, false), ("b", -60.0, false)]),
                &none,
            );
        }
        assert!((10.0 * (st.levels["a"] / 0.01).log10() + 4.34).abs() < 0.01);
        assert!(st.gains["a"] > st.gains["b"]);
        // With last-mic-hold, nobody speaking keeps the gains.
        a.last_mic_hold = true;
        let mut st = State::default();
        let talk = st
            .frame(
                &a,
                &inputs(&[("a", -20.0, true), ("b", -60.0, false)]),
                &none,
            )
            .clone();
        for _ in 0..200 {
            st.frame(
                &a,
                &inputs(&[("a", -60.0, false), ("b", -60.0, false)]),
                &none,
            );
        }
        assert_eq!(st.gains, talk, "held");
        let g = st
            .frame(
                &a,
                &inputs(&[("a", -60.0, false), ("b", -20.0, true)]),
                &none,
            )
            .clone();
        assert!(g["b"] > 0.99, "{g:?}");
    }

    #[test]
    fn a_new_member_shares_from_its_first_frame() {
        let mut a = automix(&["a"]);
        a.last_mic_hold = true;
        let none = BTreeSet::new();
        let mut st = State::default();
        st.frame(&a, &inputs(&[("a", -30.0, false)]), &none);
        a.members.push(AutomixMember::new("b"));
        // b's tap has no frame yet: a keeps its gain, b gets none, nothing is held.
        let g = st.frame(&a, &inputs(&[("a", -30.0, false)]), &none).clone();
        assert_eq!((g.get("a"), g.get("b")), (Some(&1.0), None));
        let g = st
            .frame(
                &a,
                &inputs(&[("a", -30.0, false), ("b", -30.0, false)]),
                &none,
            )
            .clone();
        assert!(
            close(db(g["a"]), -3.01) && close(db(g["b"]), -3.01),
            "{g:?}"
        );
    }

    #[test]
    fn vad_weighting() {
        let mut a = automix(&["a", "b"]);
        a.vad_weighting = true;
        let mut st = State::default();
        // Equal levels, a speaks and b does not: b counts 10 dB less.
        let g = st
            .frame(
                &a,
                &inputs(&[("a", -30.0, true), ("b", -30.0, false)]),
                &BTreeSet::new(),
            )
            .clone();
        assert!(
            close(g["a"], (1.0f64 / 1.1).sqrt()) && close(g["b"], (0.1f64 / 1.1).sqrt()),
            "{g:?}"
        );
    }
}
