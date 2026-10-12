//! Talkback (§10.2): talkback microphones and the destinations they talk to.
//!
//! A mic is an MXL input channel (configured in the show) or a browser's
//! microphone (runtime, `browser-<n>`, received by WHIP). A destination is a bus
//! (studio, IFB/N-1, remote, aux): while a mic talks to it, the mic is mixed into
//! the bus after its fader; an `interrupt` destination also takes its programme
//! down to `interrupt_db`, and only its highest-priority talking mics are heard.
//! A talking mic that `dims_control_room` dims every monitor by its
//! `talkback_dim_db`. Who talks to what is runtime state, not part of the show.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::model::{Db, Kind, Mix};
use super::room::Source;

pub const MAX_MICS: usize = 16;
pub const MAX_DESTINATIONS: usize = 32;
/// Ids of browser microphones; configured mics cannot use it.
pub const BROWSER_PREFIX: &str = "browser-";
/// Highest priority.
pub const MAX_PRIORITY: u8 = 9;
/// Mic and destination level range, dB.
pub const LEVEL_RANGE_DB: (f64, f64) = (-40.0, 20.0);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Talkback {
    #[serde(default)]
    pub mics: Vec<Mic>,
    #[serde(default)]
    pub destinations: Vec<Destination>,
    /// The priority of browser microphones (§10.2 per-feed priority).
    #[serde(default)]
    pub browser_priority: u8,
}

/// A talkback microphone on an MXL input channel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mic {
    pub id: String,
    pub name: String,
    pub input: usize,
    pub channel: usize,
    #[serde(default)]
    pub level_db: f64,
    /// On an interrupting destination only the highest-priority talking mics
    /// are heard (0 … 9).
    #[serde(default)]
    pub priority: u8,
    /// While it talks, every monitor is dimmed by its `talkback_dim_db`
    /// (the mic is in the control room).
    #[serde(default = "yes")]
    pub dims_control_room: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    /// Talk is mixed over the programme.
    #[default]
    Mix,
    /// Talk interrupts the programme (IFB): the programme goes to
    /// `interrupt_db`, the highest priority wins.
    Interrupt,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Destination {
    pub id: String,
    pub name: String,
    /// The bus talk goes into (after its fader).
    pub bus: String,
    #[serde(default)]
    pub mode: Mode,
    /// The programme while interrupted (`-inf`: cut).
    #[serde(default = "cut")]
    pub interrupt_db: Db,
    #[serde(default)]
    pub level_db: f64,
    /// What comes back from it (e.g. a remote's return), for listen-back.
    #[serde(default, rename = "return", skip_serializing_if = "Option::is_none")]
    pub return_feed: Option<Source>,
}

fn cut() -> Db {
    Db::MINUS_INF
}

/// A mic as the compiler sees it: configured or browser.
#[derive(Debug, Clone, PartialEq)]
pub struct MicInfo {
    pub id: String,
    pub priority: u8,
    pub level_db: f64,
    pub dims_control_room: bool,
}

/// Who talks to what: mic id → destination ids.
pub type Talking = BTreeMap<String, BTreeSet<String>>;

/// The programme gain of `dest` and the gain of each of `mics` into it.
pub fn destination_gains(
    dest: &Destination,
    mics: &[MicInfo],
    talking: &Talking,
) -> (f32, Vec<f32>) {
    let talks = |m: &MicInfo| talking.get(&m.id).is_some_and(|d| d.contains(&dest.id));
    let top = mics.iter().filter(|m| talks(m)).map(|m| m.priority).max();
    let gains = mics
        .iter()
        .map(|m| {
            let heard = talks(m) && (dest.mode == Mode::Mix || Some(m.priority) == top);
            if heard {
                Db(m.level_db + dest.level_db).gain() as f32
            } else {
                0.0
            }
        })
        .collect();
    let programme = match (dest.mode, top) {
        (Mode::Interrupt, Some(_)) => dest.interrupt_db.gain() as f32,
        _ => 1.0,
    };
    (programme, gains)
}

/// True while a mic that dims the control room talks to anything.
pub fn dims_control_room(mics: &[MicInfo], talking: &Talking) -> bool {
    mics.iter()
        .any(|m| m.dims_control_room && talking.get(&m.id).is_some_and(|d| !d.is_empty()))
}

fn check_id(what: &str, id: &str) -> Result<(), String> {
    let ok = !id.is_empty()
        && id.len() <= 32
        && !id.starts_with('-')
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if ok {
        Ok(())
    } else {
        Err(format!(
            "{what} id {id:?} must be 1 to 32 characters a-z, 0-9 and '-'"
        ))
    }
}

fn check_level(what: &str, v: f64) -> Result<(), String> {
    let (lo, hi) = LEVEL_RANGE_DB;
    if v.is_finite() && (lo..=hi).contains(&v) {
        Ok(())
    } else {
        Err(format!("{what} level_db must be {lo} to {hi}"))
    }
}

/// Check the talkback settings against `mix`.
pub fn validate(mix: &Mix, inputs: usize) -> Result<(), String> {
    let t = &mix.control_room.talkback;
    if t.mics.len() > MAX_MICS {
        return Err(format!("at most {MAX_MICS} talkback mics"));
    }
    if t.destinations.len() > MAX_DESTINATIONS {
        return Err(format!("at most {MAX_DESTINATIONS} talkback destinations"));
    }
    if t.browser_priority > MAX_PRIORITY {
        return Err(format!("browser_priority must be 0 to {MAX_PRIORITY}"));
    }
    let mut ids = BTreeSet::new();
    for m in &t.mics {
        check_id("talkback mic", &m.id)?;
        if m.id.starts_with(BROWSER_PREFIX) {
            return Err(format!(
                "talkback mic {}: {BROWSER_PREFIX}… is kept for browser microphones",
                m.id
            ));
        }
        if !ids.insert(m.id.as_str()) {
            return Err(format!("talkback mic id {} is used twice", m.id));
        }
        if m.name.trim().is_empty() || m.name.chars().count() > 64 {
            return Err(format!(
                "talkback mic {}: a name has 1 to 64 characters",
                m.id
            ));
        }
        if m.input == 0 || m.input > inputs {
            return Err(format!(
                "talkback mic {}: input {} does not exist (1 to {inputs})",
                m.id, m.input
            ));
        }
        if m.channel == 0 || m.channel > crate::engine::MAX_CHANNELS {
            return Err(format!(
                "talkback mic {}: channel {} is not 1 to {}",
                m.id,
                m.channel,
                crate::engine::MAX_CHANNELS
            ));
        }
        if m.priority > MAX_PRIORITY {
            return Err(format!(
                "talkback mic {}: priority must be 0 to {MAX_PRIORITY}",
                m.id
            ));
        }
        check_level(&format!("talkback mic {}:", m.id), m.level_db)?;
    }
    let mut ids = BTreeSet::new();
    let mut buses = BTreeSet::new();
    for d in &t.destinations {
        check_id("talkback destination", &d.id)?;
        if !ids.insert(d.id.as_str()) {
            return Err(format!("talkback destination id {} is used twice", d.id));
        }
        if d.name.trim().is_empty() || d.name.chars().count() > 64 {
            return Err(format!(
                "talkback destination {}: a name has 1 to 64 characters",
                d.id
            ));
        }
        if mix.kind(&d.bus) != Some(Kind::Bus) {
            return Err(format!(
                "talkback destination {}: bus {} does not exist",
                d.id, d.bus
            ));
        }
        if !buses.insert(d.bus.as_str()) {
            return Err(format!(
                "talkback destination {}: bus {} already has a destination",
                d.id, d.bus
            ));
        }
        if d.interrupt_db.0 > 0.0 {
            return Err(format!(
                "talkback destination {}: interrupt_db must be -inf to 0",
                d.id
            ));
        }
        check_level(&format!("talkback destination {}:", d.id), d.level_db)?;
        match &d.return_feed {
            Some(Source::Signal(s)) if mix.kind(s).is_none() => {
                return Err(format!(
                    "talkback destination {}: return {s} does not exist",
                    d.id
                ))
            }
            Some(Source::Input(n)) if *n == 0 || *n > inputs => {
                return Err(format!(
                    "talkback destination {}: return input {n} does not exist",
                    d.id
                ))
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dest(mode: Mode) -> Destination {
        serde_json::from_value(serde_json::json!({
            "id": "ifb", "name": "IFB", "bus": "ifb", "mode": mode
        }))
        .unwrap()
    }

    fn mic(id: &str, priority: u8, dims: bool) -> MicInfo {
        MicInfo {
            id: id.into(),
            priority,
            level_db: 0.0,
            dims_control_room: dims,
        }
    }

    fn talking(pairs: &[(&str, &str)]) -> Talking {
        let mut t = Talking::new();
        for (m, d) in pairs {
            t.entry(m.to_string()).or_default().insert(d.to_string());
        }
        t
    }

    #[test]
    fn mix_interrupt_and_priority() {
        let mics = [mic("dir", 5, true), mic("prod", 2, false)];
        let none = talking(&[]);
        let both = talking(&[("dir", "ifb"), ("prod", "ifb")]);
        let prod = talking(&[("prod", "ifb")]);
        let elsewhere = talking(&[("dir", "studio")]);

        let mix = dest(Mode::Mix);
        assert_eq!(destination_gains(&mix, &mics, &none), (1.0, vec![0.0, 0.0]));
        assert_eq!(destination_gains(&mix, &mics, &both), (1.0, vec![1.0, 1.0]));
        assert_eq!(
            destination_gains(&mix, &mics, &elsewhere),
            (1.0, vec![0.0, 0.0])
        );

        let ifb = dest(Mode::Interrupt);
        assert_eq!(ifb.interrupt_db, Db::MINUS_INF);
        assert_eq!(destination_gains(&ifb, &mics, &none), (1.0, vec![0.0, 0.0]));
        // The director outranks the producer; alone, the producer is heard.
        assert_eq!(destination_gains(&ifb, &mics, &both), (0.0, vec![1.0, 0.0]));
        assert_eq!(destination_gains(&ifb, &mics, &prod), (0.0, vec![0.0, 1.0]));
        let mut dimmed = dest(Mode::Interrupt);
        dimmed.interrupt_db = Db(-20.0);
        dimmed.level_db = -6.0;
        let (p, g) = destination_gains(&dimmed, &mics, &prod);
        assert!((p - 0.1).abs() < 1e-6 && (g[1] - 0.501_187).abs() < 1e-5);

        assert!(dims_control_room(&mics, &both));
        assert!(!dims_control_room(&mics, &prod));
        assert!(!dims_control_room(&mics, &none));
    }

    #[test]
    fn validation() {
        use crate::mix::model::tests::bus;
        use crate::mix::Format;
        let mut mix = Mix::default();
        mix.buses.push(bus("ifb", Format::Mono));
        mix.buses.push(bus("studio", Format::Stereo));
        mix.control_room.talkback.mics.push(Mic {
            id: "dir".into(),
            name: "Director".into(),
            input: 1,
            channel: 1,
            level_db: 0.0,
            priority: 1,
            dims_control_room: true,
        });
        mix.control_room
            .talkback
            .destinations
            .push(dest(Mode::Interrupt));
        assert_eq!(validate(&mix, 4), Ok(()));
        let bad = |f: &dyn Fn(&mut Talkback)| {
            let mut m = mix.clone();
            f(&mut m.control_room.talkback);
            validate(&m, 4).is_err()
        };
        assert!(bad(&|t| t.mics[0].id = "browser-1".into()));
        assert!(bad(&|t| t.mics[0].input = 5));
        assert!(bad(&|t| t.mics[0].priority = 10));
        assert!(bad(&|t| t.mics[0].level_db = 30.0));
        assert!(bad(&|t| t.mics.push(t.mics[0].clone())));
        assert!(bad(&|t| t.destinations[0].bus = "nope".into()));
        assert!(bad(&|t| {
            let mut d = t.destinations[0].clone();
            d.id = "again".into();
            t.destinations.push(d);
        }));
        assert!(bad(&|t| t.destinations[0].interrupt_db = Db(3.0)));
        assert!(bad(
            &|t| t.destinations[0].return_feed = Some(Source::Input(9))
        ));
        assert!(!bad(
            &|t| t.destinations[0].return_feed = Some(Source::Signal("studio".into()))
        ));
    }
}
