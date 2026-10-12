//! Voice activity detection, automatic mixing and ducking (§8.1, §8.3, §8.4): the
//! settings in the mix. The detectors and controllers run on the control side
//! (`crate::auto`) and drive one gain node per strip or bus (`strip/<id>/auto`,
//! `bus/<id>/auto`): where its chain has an `auto` node, else after the chain,
//! before the fader; the operator's fader and the controllers' gain multiply.
//!
//! A signal is analysed (VAD) when it has VAD settings, is a member of an
//! auto-mix or a key of a duck; without settings of its own it uses the defaults.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::model::{Db, Kind, Mix};
use super::vca::{check_id, check_name};

/// Signals the VAD analyses at once (the DSP thread's tap rings).
pub const MAX_VAD: usize = 64;
pub const MAX_AUTOMIXES: usize = 16;
pub const MAX_DUCKS: usize = 32;
/// Longest hold, attack and release, in ms.
pub const MAX_TIME_MS: f64 = 10_000.0;
pub const WEIGHT_RANGE_DB: (f64, f64) = (-30.0, 10.0);

/// A signal's voice activity detector (§8.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VadSettings {
    pub signal: String,
    /// Speech probability above which speech starts.
    #[serde(default = "default_threshold")]
    pub threshold: f64,
    /// How long speech lasts after the probability fell below the threshold.
    #[serde(default = "default_vad_hold")]
    pub hold_ms: f64,
    /// While the signal's momentary loudness (400 ms) is below this, nothing is
    /// speech: crosstalk of other talkers into a microphone is speech too, but
    /// quieter (the guard band of Com-IS-Assist).
    #[serde(default = "default_gate")]
    pub gate_lufs: f64,
}

fn default_threshold() -> f64 {
    0.5
}
fn default_vad_hold() -> f64 {
    800.0
}
fn default_gate() -> f64 {
    -32.0
}

impl VadSettings {
    pub fn new(signal: &str) -> Self {
        Self {
            signal: signal.to_string(),
            threshold: default_threshold(),
            hold_ms: default_vad_hold(),
            gate_lufs: default_gate(),
        }
    }
}

/// A gain-sharing auto-mixer (§8.3) over strips.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Automix {
    pub id: String,
    pub name: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// While nobody speaks (no member's VAD), the gains stay where they were.
    #[serde(default = "yes")]
    pub last_mic_hold: bool,
    /// A member's share also follows its speech probability.
    #[serde(default)]
    pub vad_weighting: bool,
    #[serde(default)]
    pub members: Vec<AutomixMember>,
    /// A snapshot's recall leaves it.
    #[serde(default)]
    pub recall_safe: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AutomixMember {
    pub signal: String,
    /// Added to the member's level in the sharing.
    #[serde(default)]
    pub weight_db: f64,
    /// Out of the sharing, at 0 dB.
    #[serde(default)]
    pub bypass: bool,
}

impl AutomixMember {
    pub fn new(signal: &str) -> Self {
        Self {
            signal: signal.to_string(),
            weight_db: 0.0,
            bypass: false,
        }
    }
}

/// Duck `targets` while one of `keys` speaks (§8.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Duck {
    pub id: String,
    pub name: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Strips and buses whose speech ducks.
    #[serde(default)]
    pub keys: Vec<String>,
    /// Strips and buses that are ducked.
    #[serde(default)]
    pub targets: Vec<String>,
    #[serde(default = "default_depth")]
    pub depth_db: Db,
    /// Time from 0 dB to the depth (linear in dB).
    #[serde(default = "default_attack")]
    pub attack_ms: f64,
    /// Time the depth stays after the last speech.
    #[serde(default = "default_duck_hold")]
    pub hold_ms: f64,
    /// Time from the depth back to 0 dB.
    #[serde(default = "default_release")]
    pub release_ms: f64,
    /// A snapshot's recall leaves it.
    #[serde(default)]
    pub recall_safe: bool,
}

fn yes() -> bool {
    true
}
fn default_depth() -> Db {
    Db(-12.0)
}
fn default_attack() -> f64 {
    50.0
}
fn default_duck_hold() -> f64 {
    800.0
}
fn default_release() -> f64 {
    1000.0
}

impl Duck {
    pub fn new(id: &str) -> Self {
        Self {
            id: id.to_string(),
            name: id.to_string(),
            enabled: true,
            keys: Vec::new(),
            targets: Vec::new(),
            depth_db: default_depth(),
            attack_ms: default_attack(),
            hold_ms: default_duck_hold(),
            release_ms: default_release(),
            recall_safe: false,
        }
    }
}

impl Mix {
    pub fn automix(&self, id: &str) -> Option<&Automix> {
        self.automixes.iter().find(|a| a.id == id)
    }

    pub fn duck(&self, id: &str) -> Option<&Duck> {
        self.ducks.iter().find(|d| d.id == id)
    }

    /// The auto-mix `signal` is a member of.
    pub fn automix_of(&self, signal: &str) -> Option<&Automix> {
        self.automixes
            .iter()
            .find(|a| a.members.iter().any(|m| m.signal == signal))
    }

    /// Every analysed signal with its VAD settings (its own or the defaults).
    pub fn vad_signals(&self) -> BTreeMap<String, VadSettings> {
        let mut out: BTreeMap<String, VadSettings> = self
            .vad
            .iter()
            .map(|v| (v.signal.clone(), v.clone()))
            .collect();
        let implied = self
            .automixes
            .iter()
            .flat_map(|a| a.members.iter().map(|m| &m.signal))
            .chain(self.ducks.iter().flat_map(|d| d.keys.iter()));
        for s in implied {
            out.entry(s.clone()).or_insert_with(|| VadSettings::new(s));
        }
        out
    }

    /// The signals with an auto gain node: auto-mix members and duck targets.
    pub fn auto_signals(&self) -> BTreeSet<String> {
        self.automixes
            .iter()
            .flat_map(|a| a.members.iter().map(|m| m.signal.clone()))
            .chain(self.ducks.iter().flat_map(|d| d.targets.iter().cloned()))
            .collect()
    }

    /// Take `id` out of the VAD settings, auto-mixes and ducks.
    pub fn remove_from_auto(&mut self, id: &str) {
        self.vad.retain(|v| v.signal != id);
        for a in &mut self.automixes {
            a.members.retain(|m| m.signal != id);
        }
        for d in &mut self.ducks {
            d.keys.retain(|k| k != id);
            d.targets.retain(|t| t != id);
        }
    }

    /// `old` signals became `new` ones (a link or unlink): every reference to
    /// one of `old` becomes references to all of `new`, with the settings of the
    /// first one found.
    pub fn replace_auto_refs(&mut self, old: &[String], new: &[String]) {
        fn swap<T: Clone>(
            list: &mut Vec<T>,
            id: impl Fn(&T) -> &str,
            make: impl Fn(&T, &str) -> T,
            old: &[String],
            new: &[String],
        ) {
            let Some(at) = list.iter().position(|x| old.iter().any(|o| o == id(x))) else {
                return;
            };
            let template = list[at].clone();
            list.retain(|x| !old.iter().any(|o| o == id(x)));
            let mut k = at.min(list.len());
            for n in new {
                if !list.iter().any(|x| id(x) == n) {
                    list.insert(k, make(&template, n));
                    k += 1;
                }
            }
        }
        swap(
            &mut self.vad,
            |v| &v.signal,
            |v, n| VadSettings {
                signal: n.to_string(),
                ..v.clone()
            },
            old,
            new,
        );
        for a in &mut self.automixes {
            swap(
                &mut a.members,
                |m| &m.signal,
                |m, n| AutomixMember {
                    signal: n.to_string(),
                    ..m.clone()
                },
                old,
                new,
            );
        }
        for d in &mut self.ducks {
            for list in [&mut d.keys, &mut d.targets] {
                swap(list, |s| s, |_, n| n.to_string(), old, new);
            }
        }
    }
}

fn check_time(what: &str, ms: f64) -> Result<(), String> {
    if (0.0..=MAX_TIME_MS).contains(&ms) {
        Ok(())
    } else {
        Err(format!("{what} must be 0 to {MAX_TIME_MS} ms"))
    }
}

fn check_signals(what: &str, list: &[String], mix: &Mix) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for s in list {
        if mix.kind(s).is_none() {
            return Err(format!("{what}: {s} is not a strip or bus"));
        }
        if !seen.insert(s.as_str()) {
            return Err(format!("{what}: {s} is listed twice"));
        }
    }
    Ok(())
}

/// Check the VAD settings, auto-mixes and ducks against `mix`.
pub fn validate(mix: &Mix) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for v in &mix.vad {
        if mix.kind(&v.signal).is_none() {
            return Err(format!("vad: {} is not a strip or bus", v.signal));
        }
        if !seen.insert(v.signal.as_str()) {
            return Err(format!("vad: {} has settings twice", v.signal));
        }
        if !(0.05..=0.95).contains(&v.threshold) {
            return Err(format!("vad {}: threshold must be 0.05 to 0.95", v.signal));
        }
        check_time(&format!("vad {}: hold_ms", v.signal), v.hold_ms)?;
        if !(-70.0..=0.0).contains(&v.gate_lufs) {
            return Err(format!("vad {}: gate_lufs must be -70 to 0", v.signal));
        }
    }
    if mix.automixes.len() > MAX_AUTOMIXES {
        return Err(format!("at most {MAX_AUTOMIXES} auto-mixes"));
    }
    let mut ids = BTreeSet::new();
    let mut members = BTreeSet::new();
    for a in &mix.automixes {
        check_id("auto-mix", &a.id)?;
        if !ids.insert(a.id.as_str()) {
            return Err(format!("auto-mix id {} is used twice", a.id));
        }
        check_name(&format!("auto-mix {}", a.id), &a.name)?;
        for m in &a.members {
            if mix.kind(&m.signal) != Some(Kind::Strip) {
                return Err(format!("auto-mix {}: {} is not a strip", a.id, m.signal));
            }
            if !members.insert(m.signal.as_str()) {
                return Err(format!(
                    "auto-mix {}: strip {} is already in an auto-mix",
                    a.id, m.signal
                ));
            }
            let (lo, hi) = WEIGHT_RANGE_DB;
            if !(lo..=hi).contains(&m.weight_db) {
                return Err(format!(
                    "auto-mix {}: {} weight_db must be {lo} to {hi}",
                    a.id, m.signal
                ));
            }
        }
    }
    if mix.ducks.len() > MAX_DUCKS {
        return Err(format!("at most {MAX_DUCKS} ducks"));
    }
    let mut ids = BTreeSet::new();
    for d in &mix.ducks {
        check_id("duck", &d.id)?;
        if !ids.insert(d.id.as_str()) {
            return Err(format!("duck id {} is used twice", d.id));
        }
        check_name(&format!("duck {}", d.id), &d.name)?;
        check_signals(&format!("duck {} keys", d.id), &d.keys, mix)?;
        check_signals(&format!("duck {} targets", d.id), &d.targets, mix)?;
        if let Some(t) = d.targets.iter().find(|t| d.keys.contains(t)) {
            return Err(format!("duck {}: {t} cannot duck itself", d.id));
        }
        if d.depth_db.0 > 0.0 {
            return Err(format!("duck {}: depth_db must be -inf to 0", d.id));
        }
        check_time(&format!("duck {}: attack_ms", d.id), d.attack_ms)?;
        check_time(&format!("duck {}: hold_ms", d.id), d.hold_ms)?;
        check_time(&format!("duck {}: release_ms", d.id), d.release_ms)?;
    }
    let analysed = mix.vad_signals().len();
    if analysed > MAX_VAD {
        return Err(format!(
            "at most {MAX_VAD} signals with voice activity detection ({analysed})"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mix::model::tests::{bus, strip};
    use crate::mix::Format;

    fn mix() -> Mix {
        let mut m = Mix::default();
        for id in ["a", "b", "c", "music"] {
            m.strips.push(strip(id, Format::Mono));
        }
        m.buses.push(bus("main", Format::Stereo));
        m
    }

    fn automix(id: &str, members: &[&str]) -> Automix {
        Automix {
            id: id.into(),
            name: id.into(),
            enabled: true,
            last_mic_hold: true,
            vad_weighting: false,
            members: members.iter().map(|m| AutomixMember::new(m)).collect(),
            recall_safe: false,
        }
    }

    #[test]
    fn analysed_signals_and_auto_nodes() {
        let mut m = mix();
        m.automixes.push(automix("panel", &["a", "b"]));
        let mut d = Duck::new("bed");
        d.keys = vec!["c".into()];
        d.targets = vec!["music".into()];
        m.ducks.push(d);
        let mut own = VadSettings::new("b");
        own.threshold = 0.7;
        m.vad.push(own);
        assert_eq!(validate(&m), Ok(()));
        let v = m.vad_signals();
        assert_eq!(v.keys().collect::<Vec<_>>(), ["a", "b", "c"]);
        assert_eq!(v["b"].threshold, 0.7);
        assert_eq!(v["a"], VadSettings::new("a"));
        assert_eq!(
            m.auto_signals(),
            BTreeSet::from(["a".into(), "b".into(), "music".into()])
        );
        assert_eq!(m.automix_of("b").map(|a| a.id.as_str()), Some("panel"));

        m.remove_signal("b");
        assert!(m.vad.is_empty());
        assert_eq!(m.automixes[0].members.len(), 1);
        m.remove_signal("music");
        assert!(m.ducks[0].targets.is_empty());
        assert_eq!(validate(&m), Ok(()));
    }

    #[test]
    fn link_and_unlink_carry_the_references() {
        let mut m = mix();
        let mut a = automix("panel", &["a", "b", "c"]);
        a.members[1].weight_db = -6.0;
        m.automixes.push(a);
        m.vad.push(VadSettings::new("b"));
        let mut d = Duck::new("bed");
        d.keys = vec!["b".into(), "c".into()];
        m.ducks.push(d);
        m.replace_auto_refs(&["b".into(), "c".into()], &["bc".into()]);
        let members: Vec<_> = m.automixes[0]
            .members
            .iter()
            .map(|x| (x.signal.as_str(), x.weight_db))
            .collect();
        assert_eq!(members, [("a", 0.0), ("bc", -6.0)]);
        assert_eq!(m.vad[0].signal, "bc");
        assert_eq!(m.ducks[0].keys, ["bc"]);
        m.replace_auto_refs(&["bc".into()], &["bc-l".into(), "bc-r".into()]);
        let members: Vec<_> = m.automixes[0]
            .members
            .iter()
            .map(|x| (x.signal.as_str(), x.weight_db))
            .collect();
        assert_eq!(members, [("a", 0.0), ("bc-l", -6.0), ("bc-r", -6.0)]);
        assert_eq!(m.ducks[0].keys, ["bc-l", "bc-r"]);
    }

    #[test]
    fn validation() {
        let mut m = mix();
        m.automixes.push(automix("panel", &["a", "b"]));
        let mut d = Duck::new("bed");
        d.keys = vec!["a".into()];
        d.targets = vec!["music".into(), "main".into()];
        m.ducks.push(d);
        assert_eq!(validate(&m), Ok(()));
        let bad = |f: &dyn Fn(&mut Mix)| {
            let mut x = m.clone();
            f(&mut x);
            validate(&x).is_err()
        };
        assert!(
            bad(&|m| m.automixes.push(automix("other", &["b"]))),
            "two auto-mixes"
        );
        assert!(
            bad(&|m| m.automixes[0].members.push(AutomixMember::new("main"))),
            "a bus"
        );
        assert!(bad(&|m| m.automixes[0].members[0].weight_db = 11.0));
        assert!(!bad(&|m| m.automixes[0].members[0].weight_db = -30.0));
        assert!(
            bad(&|m| m.automixes.push(automix("panel", &[]))),
            "id twice"
        );
        assert!(
            bad(&|m| m.ducks[0].targets.push("a".into())),
            "ducks its key"
        );
        assert!(bad(&|m| m.ducks[0].keys.push("nope".into())));
        assert!(bad(&|m| m.ducks[0].depth_db = Db(1.0)));
        assert!(!bad(&|m| m.ducks[0].depth_db = Db::MINUS_INF));
        assert!(bad(&|m| m.ducks[0].release_ms = 10_001.0));
        assert!(bad(&|m| m.vad.push(VadSettings::new("nope"))));
        assert!(bad(&|m| {
            m.vad.push(VadSettings::new("c"));
            m.vad.push(VadSettings::new("c"));
        }));
        assert!(bad(&|m| {
            let mut v = VadSettings::new("c");
            v.threshold = 0.99;
            m.vad.push(v);
        }));
        assert!(bad(&|m| {
            for k in 0..=MAX_VAD {
                let id = format!("s{k}");
                m.strips.push(strip(&id, Format::Mono));
                m.vad.push(VadSettings::new(&id));
            }
        }));
    }
}
