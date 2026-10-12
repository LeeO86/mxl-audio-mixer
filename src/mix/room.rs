//! The control room (§10.1): monitor paths with their source, controls and room
//! alignment, the source buttons, and the solo settings. Part of the show; the
//! solo state itself (who is soloed) is not.

use serde::{Deserialize, Serialize};

use super::chain::{self, Proc, MAX_DELAY_MS};
use super::model::{Db, Mix};
use super::Format;

/// Most monitor paths.
pub const MAX_MONITORS: usize = 16;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ControlRoom {
    #[serde(default)]
    pub monitors: Vec<Monitor>,
    /// The source buttons, in order.
    #[serde(default)]
    pub sources: Vec<Source>,
    #[serde(default)]
    pub solo: SoloSettings,
    /// SPL presets (dB SPL) for monitors with a calibration.
    #[serde(default = "reference_presets")]
    pub reference_spl: Vec<f64>,
}

fn reference_presets() -> Vec<f64> {
    vec![79.0, 82.0, 85.0]
}

/// What a monitor listens to: a strip or bus (at its output: a strip post-pan, a
/// bus post-fader), or an input's first two channels.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum Source {
    Signal(String),
    Input(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum MonitorKind {
    #[default]
    Speakers,
    Headphones,
}

/// The L/R check of a monitor: stereo as it is, mono (L + R with the mix's
/// stereo-to-mono sum), the difference (L − R, the same gain), swapped, or one
/// side on both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Check {
    #[default]
    Stereo,
    Mono,
    Difference,
    Swap,
    Left,
    Right,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Monitor {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub kind: MonitorKind,
    pub format: Format,
    #[serde(default)]
    pub source: Option<Source>,
    #[serde(default)]
    pub level_db: f64,
    #[serde(default)]
    pub dim: bool,
    #[serde(default = "dim_depth")]
    pub dim_db: f64,
    #[serde(default)]
    pub cut: bool,
    #[serde(default)]
    pub check: Check,
    /// Plays the solo bus while anything is soloed (the programme then at
    /// `solo.programme_db`).
    #[serde(default = "yes")]
    pub follows_solo: bool,
    /// Room alignment: trim, delay, and a processing chain (EQ, filters for bass
    /// management, …).
    #[serde(default)]
    pub trim_db: f64,
    #[serde(default)]
    pub delay_ms: f64,
    #[serde(default)]
    pub chain: Vec<Proc>,
    /// Bass management (formats with an LFE channel): switched on and off in the
    /// control room.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bass_management: Option<BassManagement>,
    /// dB SPL at a level of 0 dB, for the reference presets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calibration_spl: Option<f64>,
    /// Published as an MXL flow with an NMOS sender (to physical speakers).
    #[serde(default)]
    pub publish: bool,
}

/// Bass management: the lows of every main channel to the LFE (sub) channel.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BassManagement {
    pub enabled: bool,
    /// Linkwitz-Riley 24 dB/oct crossover, 40–250 Hz.
    pub crossover_hz: f64,
    /// Gain of the LFE input on the sub, −20 to +20 dB.
    pub lfe_gain_db: f64,
}

impl Monitor {
    /// The LFE channel of its format, if it has one.
    pub fn lfe(&self) -> Option<usize> {
        self.format.labels().iter().position(|l| *l == "LFE")
    }
}

fn dim_depth() -> f64 {
    -21.0
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum SoloMode {
    /// Every solo adds to the solo bus.
    #[default]
    Additive,
    /// A new solo replaces the others.
    Interlock,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SoloSettings {
    #[serde(default)]
    pub mode: SoloMode,
    /// Gain of PFL on the solo bus.
    #[serde(default)]
    pub pfl_trim_db: f64,
    /// The programme on a monitor that follows solo, while anything is soloed
    /// (the automatic PFL dim, default −21 dB; `-inf`: solo replaces the
    /// programme).
    #[serde(default = "programme_under_solo")]
    pub programme_db: Db,
    /// Solo-in-place: while anything is soloed, every other strip that is not
    /// solo-safe is muted in the mix. Destructive; off unless enabled.
    #[serde(default)]
    pub sip: bool,
}

fn programme_under_solo() -> Db {
    Db(-21.0)
}

impl Default for SoloSettings {
    fn default() -> Self {
        Self {
            mode: SoloMode::default(),
            pfl_trim_db: 0.0,
            programme_db: programme_under_solo(),
            sip: false,
        }
    }
}

/// Pre-fader listen or after-fader listen (strips post-pan, buses post-fader).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
pub enum SoloKind {
    Pfl,
    Afl,
}

impl Monitor {
    /// The key of its published output and its meter (`monitor:<id>`; a colon
    /// never appears in a signal id).
    pub fn key(&self) -> String {
        format!("monitor:{}", self.id)
    }
}

fn range(what: &str, v: f64, lo: f64, hi: f64) -> Result<(), String> {
    if v.is_finite() && (lo..=hi).contains(&v) {
        Ok(())
    } else {
        Err(format!("{what} must be {lo} to {hi}, not {v}"))
    }
}

/// Check the control room against `mix`.
pub fn validate(mix: &Mix, inputs: usize) -> Result<(), String> {
    let room = &mix.control_room;
    if room.monitors.len() > MAX_MONITORS {
        return Err(format!("at most {MAX_MONITORS} monitors"));
    }
    let source = |s: &Source| match s {
        Source::Signal(id) if mix.kind(id).is_none() => Err(format!("source {id} does not exist")),
        Source::Input(n) if *n == 0 || *n > inputs => {
            Err(format!("input {n} does not exist (1 to {inputs})"))
        }
        _ => Ok(()),
    };
    let mut ids = std::collections::BTreeSet::new();
    for m in &room.monitors {
        let id_ok = !m.id.is_empty()
            && m.id.len() <= 32
            && !m.id.starts_with('-')
            && m.id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if !id_ok {
            return Err(format!(
                "monitor id {:?} must be 1 to 32 characters a-z, 0-9 and '-'",
                m.id
            ));
        }
        if !ids.insert(m.id.as_str()) {
            return Err(format!("monitor id {} is used twice", m.id));
        }
        if m.name.trim().is_empty() || m.name.chars().count() > 64 {
            return Err(format!("monitor {}: a name has 1 to 64 characters", m.id));
        }
        let at = |e: String| format!("monitor {}: {e}", m.id);
        if let Some(s) = &m.source {
            source(s).map_err(at)?;
        }
        range("level_db", m.level_db, -80.0, 12.0).map_err(at)?;
        range("dim_db", m.dim_db, -60.0, 0.0).map_err(at)?;
        range("trim_db", m.trim_db, -24.0, 24.0).map_err(at)?;
        range("delay_ms", m.delay_ms, 0.0, MAX_DELAY_MS).map_err(at)?;
        if let Some(c) = m.calibration_spl {
            range("calibration_spl", c, 40.0, 130.0).map_err(at)?;
        }
        chain::validate(&m.chain).map_err(at)?;
        if let Some(b) = &m.bass_management {
            if b.enabled && m.lfe().is_none() {
                return Err(at(format!(
                    "bass management needs a format with an LFE channel, not {}",
                    serde_json::to_string(&m.format).unwrap_or_default()
                )));
            }
            range("crossover_hz", b.crossover_hz, 40.0, 250.0).map_err(at)?;
            range("lfe_gain_db", b.lfe_gain_db, -20.0, 20.0).map_err(at)?;
        }
        if m.chain.iter().any(|p| p.kind.key().is_some()) {
            return Err(at("a monitor's chain takes no keyed nodes".into()));
        }
    }
    for s in &room.sources {
        source(s).map_err(|e| format!("source button: {e}"))?;
    }
    range("pfl_trim_db", room.solo.pfl_trim_db, -24.0, 24.0)?;
    if room.solo.programme_db.0 > 0.0 {
        return Err("programme_db must be -inf to 0".into());
    }
    for r in &room.reference_spl {
        range("reference_spl", *r, 40.0, 130.0)?;
    }
    Ok(())
}

/// The L/R matrix of a check, row-major (L′ from L, R; R′ from L, R); `sum` is
/// the stereo-to-mono gain.
pub fn check_matrix(check: Check, sum: f32) -> [f32; 4] {
    match check {
        Check::Stereo => [1.0, 0.0, 0.0, 1.0],
        Check::Mono => [sum, sum, sum, sum],
        Check::Difference => [sum, -sum, sum, -sum],
        Check::Swap => [0.0, 1.0, 1.0, 0.0],
        Check::Left => [1.0, 0.0, 1.0, 0.0],
        Check::Right => [0.0, 1.0, 0.0, 1.0],
    }
}

/// The monitor's gain after the check: level, dim, cut, trim.
pub fn monitor_gain(m: &Monitor) -> f32 {
    if m.cut {
        return 0.0;
    }
    let dim = if m.dim { m.dim_db } else { 0.0 };
    Db(m.level_db + dim + m.trim_db).gain() as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mix::model::tests::{bus, strip};

    fn monitor(id: &str) -> Monitor {
        serde_json::from_value(serde_json::json!({"id": id, "name": "Main", "format": "stereo"}))
            .unwrap()
    }

    #[test]
    fn defaults_and_validation() {
        let m = monitor("main");
        assert_eq!(
            (m.level_db, m.dim_db, m.check, m.follows_solo, m.publish),
            (0.0, -21.0, Check::Stereo, true, false)
        );
        let room = ControlRoom::default();
        assert_eq!(room.solo.programme_db, Db(-21.0));
        assert_eq!(room.solo.mode, SoloMode::Additive);
        let mut mix = Mix::default();
        mix.strips.push(strip("vox", Format::Mono));
        mix.buses.push(bus("pgm", Format::Stereo));
        mix.control_room.monitors.push(Monitor {
            source: Some(Source::Signal("pgm".into())),
            ..monitor("main")
        });
        assert_eq!(validate(&mix, 4), Ok(()));
        let bad = |f: &dyn Fn(&mut Mix)| {
            let mut m = mix.clone();
            f(&mut m);
            validate(&m, 4).is_err()
        };
        assert!(bad(
            &|m| m.control_room.monitors[0].source = Some(Source::Signal("nope".into()))
        ));
        assert!(bad(
            &|m| m.control_room.monitors[0].source = Some(Source::Input(5))
        ));
        assert!(bad(&|m| m.control_room.monitors.push(monitor("main"))));
        assert!(bad(&|m| m.control_room.monitors[0].level_db = 20.0));
        assert!(bad(&|m| m.control_room.monitors[0].id = "Main".into()));
        assert!(bad(&|m| m.control_room.solo.programme_db = Db(3.0)));
    }

    #[test]
    fn checks_and_gain() {
        let apply = |m: [f32; 4], l: f32, r: f32| (m[0] * l + m[1] * r, m[2] * l + m[3] * r);
        assert_eq!(apply(check_matrix(Check::Swap, 0.5), 1.0, 2.0), (2.0, 1.0));
        assert_eq!(apply(check_matrix(Check::Left, 0.5), 1.0, 2.0), (1.0, 1.0));
        assert_eq!(apply(check_matrix(Check::Right, 0.5), 1.0, 2.0), (2.0, 2.0));
        assert_eq!(apply(check_matrix(Check::Mono, 0.5), 1.0, 2.0), (1.5, 1.5));
        assert_eq!(
            apply(check_matrix(Check::Difference, 0.5), 1.0, 1.0),
            (0.0, 0.0)
        );
        let mut m = monitor("main");
        m.level_db = -10.0;
        m.dim = true;
        assert!((monitor_gain(&m) - 0.031_622_78).abs() < 1e-6);
        m.cut = true;
        assert_eq!(monitor_gain(&m), 0.0);
    }
}
