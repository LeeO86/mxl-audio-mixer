//! Voice activity detection, auto-level, automatic mixing and ducking (§8): the
//! settings in the mix. The detectors and controllers run on the control side
//! (`crate::auto`) and drive one gain node per strip or bus (`strip/<id>/auto`,
//! `bus/<id>/auto`): where its chain has an `auto` node, else after the chain,
//! before the fader; the operator's fader and the controllers' gain multiply.
//!
//! A signal is analysed (VAD) when it has VAD settings, is a member of an
//! auto-mix, a key of a duck, a target of a ratio duck that hears its targets,
//! or auto-levelled; without settings of its own it uses the defaults.

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
    #[serde(default)]
    pub detector: VadDetector,
}

/// Which detector a VAD runs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum VadDetector {
    /// Silero VAD, a small neural network (ONNX Runtime).
    #[default]
    Silero,
    /// The feature detector: energy, voicing, modulation, tone rejection.
    Features,
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
            detector: VadDetector::default(),
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
    #[serde(default)]
    pub mode: DuckMode,
    /// Fixed: the depth while a key speaks.
    #[serde(default = "default_depth")]
    pub depth_db: Db,
    /// Fixed: time from 0 dB to the depth (linear in dB).
    #[serde(default = "default_attack")]
    pub attack_ms: f64,
    /// Fixed: time the depth stays after the last speech.
    #[serde(default = "default_duck_hold")]
    pub hold_ms: f64,
    /// Fixed: time from the depth back to 0 dB.
    #[serde(default = "default_release")]
    pub release_ms: f64,
    #[serde(default)]
    pub ratio: Ratio,
    /// How much of the duck each channel group of a target takes.
    #[serde(default)]
    pub channel_depth: ChannelDepth,
    /// A snapshot's recall leaves it.
    #[serde(default)]
    pub recall_safe: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DuckMode {
    /// Down by `depth_db` while a key speaks.
    #[default]
    Fixed,
    /// Com-IS-Assist's ratio control: while a key speaks, as far down as keeps
    /// the keys' loudness `ratio.target_lu` above the targets'.
    Ratio,
}

/// A ratio duck's control (Com-IS-Assist's defaults and ranges).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ratio {
    /// How far the keys sit above the targets while a key speaks, LU.
    #[serde(default = "default_ratio_target")]
    pub target_lu: f64,
    /// The same while the targets speak too (over-voice), LU.
    #[serde(default = "default_overvoice")]
    pub overvoice_lu: f64,
    /// The deepest reduction, dB (positive).
    #[serde(default = "default_max_reduction")]
    pub max_reduction_db: f64,
    /// Scales every stage's attack and release (> 1 faster).
    #[serde(default = "one")]
    pub speed: f64,
    /// The targets' speech counts (their VAD): with it alone the duck comes back
    /// four times faster (interview passthrough), with a key's the over-voice
    /// target applies.
    #[serde(default = "yes")]
    pub bed_voice: bool,
}

impl Default for Ratio {
    fn default() -> Self {
        Self {
            target_lu: default_ratio_target(),
            overvoice_lu: default_overvoice(),
            max_reduction_db: default_max_reduction(),
            speed: 1.0,
            bed_voice: true,
        }
    }
}

pub const RATIO_TARGET_RANGE: (f64, f64) = (0.0, 12.0);
pub const OVERVOICE_RANGE: (f64, f64) = (0.0, 24.0);
pub const MAX_REDUCTION_RANGE: (f64, f64) = (0.0, 48.0);
pub const SPEED_RANGE: (f64, f64) = (0.25, 8.0);

/// The share of a duck's dB each channel group of a target takes, 0 to 1
/// (Com-IS-Assist's duck depth): L and R, the centre (and mono), the LFE, and
/// every other channel (surround, side, rear, height).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelDepth {
    #[serde(default = "one")]
    pub front: f64,
    #[serde(default = "one")]
    pub center: f64,
    #[serde(default = "one")]
    pub lfe: f64,
    #[serde(default = "one")]
    pub surround: f64,
}

impl Default for ChannelDepth {
    fn default() -> Self {
        Self {
            front: 1.0,
            center: 1.0,
            lfe: 1.0,
            surround: 1.0,
        }
    }
}

impl ChannelDepth {
    /// The share per channel of a signal with channels `labels`.
    pub fn per_channel(&self, labels: &[&str]) -> Vec<f64> {
        crate::auto::comis::engine::DuckDepth {
            front_lr: self.front,
            center: self.center,
            lfe: self.lfe,
            surround: self.surround,
        }
        .per_channel(labels)
    }
}

/// Speech-aware loudness (§8.2): the signal's gain rides its short-term
/// loudness to `target_lufs`, `speech_offset_lu` higher while it speaks (its
/// VAD), within `min_gain_db` … `max_gain_db`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AutoLevel {
    pub signal: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default = "default_level_target")]
    pub target_lufs: f64,
    #[serde(default = "default_speech_offset")]
    pub speech_offset_lu: f64,
    #[serde(default = "default_max_gain")]
    pub max_gain_db: f64,
    #[serde(default = "default_min_gain")]
    pub min_gain_db: f64,
    /// Time constant of the gain going down.
    #[serde(default = "default_level_attack")]
    pub attack_ms: f64,
    /// Time constant of the gain going up.
    #[serde(default = "default_level_release")]
    pub release_ms: f64,
    /// While the momentary loudness is below this, there is no signal: the gain
    /// stays and the measurement skips the frames.
    #[serde(default = "default_freeze")]
    pub freeze_lufs: f64,
    /// A snapshot's recall leaves it.
    #[serde(default)]
    pub recall_safe: bool,
}

pub const LEVEL_TARGET_RANGE: (f64, f64) = (-40.0, 0.0);
pub const SPEECH_OFFSET_RANGE: (f64, f64) = (-10.0, 10.0);
pub const LEVEL_GAIN_LIMIT_DB: f64 = 24.0;
pub const LEVEL_MAX_TIME_MS: f64 = 60_000.0;

impl AutoLevel {
    pub fn new(signal: &str) -> Self {
        Self {
            signal: signal.to_string(),
            enabled: true,
            target_lufs: default_level_target(),
            speech_offset_lu: default_speech_offset(),
            max_gain_db: default_max_gain(),
            min_gain_db: default_min_gain(),
            attack_ms: default_level_attack(),
            release_ms: default_level_release(),
            freeze_lufs: default_freeze(),
            recall_safe: false,
        }
    }
}

fn yes() -> bool {
    true
}
fn one() -> f64 {
    1.0
}
fn default_ratio_target() -> f64 {
    5.0
}
fn default_overvoice() -> f64 {
    13.0
}
fn default_max_reduction() -> f64 {
    24.0
}
fn default_level_target() -> f64 {
    -23.0
}
fn default_speech_offset() -> f64 {
    2.0
}
fn default_max_gain() -> f64 {
    12.0
}
fn default_min_gain() -> f64 {
    -12.0
}
fn default_level_attack() -> f64 {
    1000.0
}
fn default_level_release() -> f64 {
    3000.0
}
fn default_freeze() -> f64 {
    -50.0
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
            mode: DuckMode::Fixed,
            depth_db: default_depth(),
            attack_ms: default_attack(),
            hold_ms: default_duck_hold(),
            release_ms: default_release(),
            ratio: Ratio::default(),
            channel_depth: ChannelDepth::default(),
            recall_safe: false,
        }
    }

    /// A ratio duck that listens to its targets' speech.
    pub fn hears_targets(&self) -> bool {
        self.mode == DuckMode::Ratio && self.ratio.bed_voice
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

    pub fn autolevel(&self, signal: &str) -> Option<&AutoLevel> {
        self.autolevels.iter().find(|l| l.signal == signal)
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
            .chain(self.ducks.iter().flat_map(|d| d.keys.iter()))
            .chain(
                self.ducks
                    .iter()
                    .filter(|d| d.hears_targets())
                    .flat_map(|d| d.targets.iter()),
            )
            .chain(self.autolevels.iter().map(|l| &l.signal));
        for s in implied {
            out.entry(s.clone()).or_insert_with(|| VadSettings::new(s));
        }
        out
    }

    /// The signals with an auto gain node: auto-mix members, duck targets and
    /// auto-levelled signals.
    pub fn auto_signals(&self) -> BTreeSet<String> {
        self.automixes
            .iter()
            .flat_map(|a| a.members.iter().map(|m| m.signal.clone()))
            .chain(self.ducks.iter().flat_map(|d| d.targets.iter().cloned()))
            .chain(self.autolevels.iter().map(|l| l.signal.clone()))
            .collect()
    }

    /// Take `id` out of the VAD settings, auto-mixes, ducks and auto-levels.
    pub fn remove_from_auto(&mut self, id: &str) {
        self.vad.retain(|v| v.signal != id);
        self.autolevels.retain(|l| l.signal != id);
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
        swap(
            &mut self.autolevels,
            |l| &l.signal,
            |l, n| AutoLevel {
                signal: n.to_string(),
                ..l.clone()
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

fn check_range(what: &str, v: f64, (lo, hi): (f64, f64)) -> Result<(), String> {
    if (lo..=hi).contains(&v) {
        Ok(())
    } else {
        Err(format!("{what} must be {lo} to {hi}"))
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
        let r = &d.ratio;
        check_range(
            &format!("duck {}: ratio.target_lu", d.id),
            r.target_lu,
            RATIO_TARGET_RANGE,
        )?;
        check_range(
            &format!("duck {}: ratio.overvoice_lu", d.id),
            r.overvoice_lu,
            OVERVOICE_RANGE,
        )?;
        check_range(
            &format!("duck {}: ratio.max_reduction_db", d.id),
            r.max_reduction_db,
            MAX_REDUCTION_RANGE,
        )?;
        check_range(&format!("duck {}: ratio.speed", d.id), r.speed, SPEED_RANGE)?;
        let c = &d.channel_depth;
        for (name, v) in [
            ("front", c.front),
            ("center", c.center),
            ("lfe", c.lfe),
            ("surround", c.surround),
        ] {
            check_range(
                &format!("duck {}: channel_depth.{name}", d.id),
                v,
                (0.0, 1.0),
            )?;
        }
    }
    let mut seen = BTreeSet::new();
    for l in &mix.autolevels {
        let what = format!("auto-level {}", l.signal);
        if mix.kind(&l.signal).is_none() {
            return Err(format!("{what}: not a strip or bus"));
        }
        if !seen.insert(l.signal.as_str()) {
            return Err(format!("{what}: set twice"));
        }
        check_range(
            &format!("{what}: target_lufs"),
            l.target_lufs,
            LEVEL_TARGET_RANGE,
        )?;
        check_range(
            &format!("{what}: speech_offset_lu"),
            l.speech_offset_lu,
            SPEECH_OFFSET_RANGE,
        )?;
        check_range(
            &format!("{what}: max_gain_db"),
            l.max_gain_db,
            (0.0, LEVEL_GAIN_LIMIT_DB),
        )?;
        check_range(
            &format!("{what}: min_gain_db"),
            l.min_gain_db,
            (-LEVEL_GAIN_LIMIT_DB, 0.0),
        )?;
        check_range(
            &format!("{what}: attack_ms"),
            l.attack_ms,
            (0.0, LEVEL_MAX_TIME_MS),
        )?;
        check_range(
            &format!("{what}: release_ms"),
            l.release_ms,
            (0.0, LEVEL_MAX_TIME_MS),
        )?;
        check_range(&format!("{what}: freeze_lufs"), l.freeze_lufs, (-70.0, 0.0))?;
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

        // A ratio duck hears its targets (interview passthrough); an
        // auto-level analyses its signal and gets an auto node.
        m.ducks[0].mode = DuckMode::Ratio;
        m.autolevels.push(AutoLevel::new("main"));
        assert_eq!(
            m.vad_signals().keys().collect::<Vec<_>>(),
            ["a", "b", "c", "main", "music"]
        );
        assert!(m.auto_signals().contains("main"));
        m.ducks[0].ratio.bed_voice = false;
        assert!(!m.vad_signals().contains_key("music"));
        m.ducks[0].mode = DuckMode::Fixed;

        m.remove_signal("b");
        assert!(m.vad.is_empty());
        assert_eq!(m.automixes[0].members.len(), 1);
        m.remove_signal("music");
        assert!(m.ducks[0].targets.is_empty());
        m.remove_signal("main");
        assert!(m.autolevels.is_empty());
        assert_eq!(validate(&m), Ok(()));
    }

    #[test]
    fn older_ducks_read_as_fixed() {
        let d: Duck = serde_json::from_value(serde_json::json!({
            "id": "bed", "name": "bed", "keys": ["a"], "targets": ["music"]
        }))
        .unwrap();
        assert_eq!(d, {
            let mut x = Duck::new("bed");
            x.keys = vec!["a".into()];
            x.targets = vec!["music".into()];
            x
        });
        assert_eq!(
            ChannelDepth {
                lfe: 0.0,
                surround: 0.5,
                ..ChannelDepth::default()
            }
            .per_channel(Format::Surround51.labels()),
            [1.0, 1.0, 1.0, 0.0, 0.5, 0.5]
        );
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
        let mut l = AutoLevel::new("bc-l");
        l.target_lufs = -18.0;
        m.autolevels.push(l);
        m.replace_auto_refs(&["bc-l".into(), "bc-r".into()], &["bc".into()]);
        assert_eq!(
            m.autolevels
                .iter()
                .map(|l| (l.signal.as_str(), l.target_lufs))
                .collect::<Vec<_>>(),
            [("bc", -18.0)]
        );
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
        assert!(!bad(&|m| m.ducks[0].mode = DuckMode::Ratio));
        assert!(bad(&|m| m.ducks[0].ratio.target_lu = 13.0));
        assert!(bad(&|m| m.ducks[0].ratio.overvoice_lu = -1.0));
        assert!(bad(&|m| m.ducks[0].ratio.max_reduction_db = 49.0));
        assert!(bad(&|m| m.ducks[0].ratio.speed = 0.1));
        assert!(bad(&|m| m.ducks[0].channel_depth.lfe = 1.5));
        assert!(!bad(&|m| m.ducks[0].channel_depth.lfe = 0.0));
        assert!(!bad(&|m| m.autolevels.push(AutoLevel::new("main"))));
        assert!(bad(&|m| m.autolevels.push(AutoLevel::new("nope"))));
        assert!(bad(&|m| {
            m.autolevels.push(AutoLevel::new("a"));
            m.autolevels.push(AutoLevel::new("a"));
        }));
        for f in [
            |l: &mut AutoLevel| l.target_lufs = -41.0,
            |l: &mut AutoLevel| l.speech_offset_lu = 11.0,
            |l: &mut AutoLevel| l.max_gain_db = -1.0,
            |l: &mut AutoLevel| l.min_gain_db = 1.0,
            |l: &mut AutoLevel| l.max_gain_db = 25.0,
            |l: &mut AutoLevel| l.release_ms = 60_001.0,
            |l: &mut AutoLevel| l.freeze_lufs = -71.0,
        ] {
            assert!(bad(&|m| {
                let mut l = AutoLevel::new("a");
                f(&mut l);
                m.autolevels.push(l);
            }));
        }
        assert!(bad(&|m| {
            for k in 0..=MAX_VAD {
                let id = format!("s{k}");
                m.strips.push(strip(&id, Format::Mono));
                m.vad.push(VadSettings::new(&id));
            }
        }));
    }
}
