//! The mix (§5): strips, buses, sends and published outputs, as kept in the show
//! file. Parameters are addressed by signal id and field (§12.4), e.g.
//! `strips/vox-1/fader_db` or `sends/vox-1/main/gain_db`.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::format::{Format, Laws, MonoSum, PanLaw};

/// Largest fader and send gain (§5: −∞ … +10 dB).
pub const MAX_GAIN_DB: f64 = 10.0;
/// Trim range, ± dB.
pub const TRIM_RANGE_DB: f64 = 24.0;
/// Default parameter ramp (§2).
pub const DEFAULT_RAMP_MS: f64 = 10.0;
/// Longest parameter ramp accepted.
pub const MAX_RAMP_MS: f64 = 10_000.0;
/// Largest number of published outputs.
pub const MAX_OUTPUTS: usize = 128;

/// A gain in dB, or −∞ (`"-inf"` in JSON).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Db(pub f64);

impl Db {
    pub const MINUS_INF: Db = Db(f64::NEG_INFINITY);

    pub fn gain(self) -> f64 {
        if self.0 == f64::NEG_INFINITY {
            0.0
        } else {
            10f64.powf(self.0 / 20.0)
        }
    }
}

impl Serialize for Db {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if self.0 == f64::NEG_INFINITY {
            s.serialize_str("-inf")
        } else {
            s.serialize_f64(self.0)
        }
    }
}

impl<'de> Deserialize<'de> for Db {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Number(f64),
            Text(String),
        }
        match Raw::deserialize(d)? {
            Raw::Number(v) if v.is_finite() => Ok(Db(v)),
            Raw::Text(t) if t == "-inf" => Ok(Db::MINUS_INF),
            _ => Err(serde::de::Error::custom(
                "a gain is a number of dB or \"-inf\"",
            )),
        }
    }
}

/// One strip channel's source: channel `channel` (from 1) of input `input` (from 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchPoint {
    pub input: usize,
    pub channel: usize,
}

/// An input strip: patch → trim and polarity → fader and mute → pan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Strip {
    pub id: String,
    pub name: String,
    pub format: Format,
    /// Per strip channel, the input channel it reads (`null`: unpatched).
    pub patch: Vec<Option<PatchPoint>>,
    pub trim_db: f64,
    /// Per strip channel, `true` inverts the polarity.
    pub polarity: Vec<bool>,
    pub fader_db: Db,
    pub mute: bool,
    /// Mono: pan position; stereo: balance. −1 … +1. Wider formats: unused.
    pub pan: f64,
    /// Stereo width, 0 (mono) … 1 (unchanged) … 2. Other formats: unused.
    pub width: f64,
}

/// A summing bus: the sum of its sends → fader and mute.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bus {
    pub id: String,
    pub name: String,
    pub format: Format,
    pub fader_db: Db,
    pub mute: bool,
}

/// Where a send taps its source (§5). Buses have `pre-fader` and `post-fader`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Tap {
    Input,
    PreFader,
    PostFader,
    PostPan,
}

/// A matrix point: signal `from` into bus `to`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Send {
    pub from: String,
    pub to: String,
    pub tap: Tap,
    pub gain_db: Db,
    pub mute: bool,
    /// Polarity inversion of the send.
    pub phase: bool,
}

/// A signal published as an MXL flow and an NMOS sender (§4.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Output {
    pub signal: String,
    /// Layout of the flow; `null`: the signal's format.
    #[serde(default)]
    pub format: Option<Format>,
    /// Samples per MXL commit; `null`: `MIXER_OUTPUT_COMMIT_SAMPLES`.
    #[serde(default)]
    pub commit_samples: Option<u32>,
}

/// Mix-wide laws.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct MixSettings {
    #[serde(default)]
    pub pan_law_db: PanLaw,
    #[serde(default)]
    pub mono_sum_db: MonoSum,
}

impl MixSettings {
    pub fn laws(&self) -> Laws {
        Laws {
            pan: self.pan_law_db,
            mono_sum: self.mono_sum_db,
        }
    }
}

/// The whole mix.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Mix {
    #[serde(default)]
    pub settings: MixSettings,
    #[serde(default)]
    pub strips: Vec<Strip>,
    #[serde(default)]
    pub buses: Vec<Bus>,
    #[serde(default)]
    pub sends: Vec<Send>,
    #[serde(default)]
    pub outputs: Vec<Output>,
}

/// What a signal id names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Strip,
    Bus,
}

impl Mix {
    pub fn kind(&self, id: &str) -> Option<Kind> {
        if self.strips.iter().any(|s| s.id == id) {
            Some(Kind::Strip)
        } else if self.buses.iter().any(|b| b.id == id) {
            Some(Kind::Bus)
        } else {
            None
        }
    }

    pub fn strip(&self, id: &str) -> Option<&Strip> {
        self.strips.iter().find(|s| s.id == id)
    }

    pub fn bus(&self, id: &str) -> Option<&Bus> {
        self.buses.iter().find(|b| b.id == id)
    }

    pub fn send(&self, from: &str, to: &str) -> Option<&Send> {
        self.sends.iter().find(|s| s.from == from && s.to == to)
    }

    pub fn output(&self, signal: &str) -> Option<&Output> {
        self.outputs.iter().find(|o| o.signal == signal)
    }

    /// A signal's name and format.
    pub fn signal(&self, id: &str) -> Option<(&str, Format)> {
        self.strip(id)
            .map(|s| (s.name.as_str(), s.format))
            .or_else(|| self.bus(id).map(|b| (b.name.as_str(), b.format)))
    }

    /// The width of `id` at `tap`: a mono strip is stereo after its panner.
    pub fn tap_format(&self, id: &str, tap: Tap) -> Option<Format> {
        let (_, format) = self.signal(id)?;
        Some(match (self.kind(id)?, tap, format) {
            (Kind::Strip, Tap::PostPan, Format::Mono) => Format::Stereo,
            _ => format,
        })
    }

    /// A signal's default tap for a new send: a strip's post-pan, a bus's post-fader.
    pub fn default_tap(&self, id: &str) -> Tap {
        match self.kind(id) {
            Some(Kind::Strip) => Tap::PostPan,
            _ => Tap::PostFader,
        }
    }

    /// A free id `<prefix>-<n>`.
    pub fn next_id(&self, prefix: &str) -> String {
        (1..)
            .map(|n| format!("{prefix}-{n}"))
            .find(|id| self.kind(id).is_none())
            .expect("unbounded")
    }

    /// Check everything the compiler and the API rely on. `inputs` is the number
    /// of inputs, `block` the block size.
    pub fn validate(&self, inputs: usize, block: u32) -> Result<(), String> {
        let mut ids = BTreeSet::new();
        for id in self
            .strips
            .iter()
            .map(|s| &s.id)
            .chain(self.buses.iter().map(|b| &b.id))
        {
            check_id(id)?;
            if !ids.insert(id.as_str()) {
                return Err(format!("signal id {id} is used twice"));
            }
        }
        for s in &self.strips {
            check_name(&s.name)?;
            let n = s.format.channels();
            if s.patch.len() != n || s.polarity.len() != n {
                return Err(format!(
                    "strip {}: patch and polarity need {n} entries for {}",
                    s.id,
                    serde_json::to_string(&s.format).unwrap_or_default()
                ));
            }
            for p in s.patch.iter().flatten() {
                if p.input == 0 || p.input > inputs {
                    return Err(format!(
                        "strip {}: input {} does not exist (1 to {inputs})",
                        s.id, p.input
                    ));
                }
                if p.channel == 0 || p.channel > crate::engine::MAX_CHANNELS {
                    return Err(format!(
                        "strip {}: channel {} is not 1 to {}",
                        s.id,
                        p.channel,
                        crate::engine::MAX_CHANNELS
                    ));
                }
            }
            if !(-TRIM_RANGE_DB..=TRIM_RANGE_DB).contains(&s.trim_db) {
                return Err(format!(
                    "strip {}: trim_db must be -{TRIM_RANGE_DB} to {TRIM_RANGE_DB}",
                    s.id
                ));
            }
            check_gain(&format!("strip {}: fader_db", s.id), s.fader_db)?;
            if !(-1.0..=1.0).contains(&s.pan) {
                return Err(format!("strip {}: pan must be -1 to 1", s.id));
            }
            if !(0.0..=2.0).contains(&s.width) {
                return Err(format!("strip {}: width must be 0 to 2", s.id));
            }
        }
        for b in &self.buses {
            check_name(&b.name)?;
            check_gain(&format!("bus {}: fader_db", b.id), b.fader_db)?;
        }
        let mut pairs = BTreeSet::new();
        for send in &self.sends {
            let Some(kind) = self.kind(&send.from) else {
                return Err(format!("send source {} does not exist", send.from));
            };
            if self.kind(&send.to) != Some(Kind::Bus) {
                return Err(format!("send destination {} is not a bus", send.to));
            }
            if send.from == send.to {
                return Err(format!("bus {} cannot send to itself", send.to));
            }
            if kind == Kind::Bus && matches!(send.tap, Tap::Input | Tap::PostPan) {
                return Err(format!(
                    "send {} → {}: a bus has the taps pre-fader and post-fader",
                    send.from, send.to
                ));
            }
            check_gain(
                &format!("send {} → {}: gain_db", send.from, send.to),
                send.gain_db,
            )?;
            if !pairs.insert((send.from.as_str(), send.to.as_str())) {
                return Err(format!("send {} → {} exists twice", send.from, send.to));
            }
        }
        self.bus_order()?;
        let mut published = BTreeSet::new();
        if self.outputs.len() > MAX_OUTPUTS {
            return Err(format!("at most {MAX_OUTPUTS} outputs"));
        }
        for o in &self.outputs {
            if self.kind(&o.signal) != Some(Kind::Bus) {
                return Err(format!("output signal {} is not a bus", o.signal));
            }
            if !published.insert(o.signal.as_str()) {
                return Err(format!("signal {} is published twice", o.signal));
            }
            if let Some(c) = o.commit_samples {
                if c == 0 || c % block != 0 || c > 48_000 {
                    return Err(format!(
                        "output {}: commit_samples must be a multiple of the block size {block} up to 48000",
                        o.signal
                    ));
                }
            }
        }
        Ok(())
    }

    /// Buses in processing order: a bus comes after every bus that sends to it.
    /// A cycle is an error (§4.4: feedback needs an explicit block delay, which
    /// the mix does not have yet).
    pub fn bus_order(&self) -> Result<Vec<&Bus>, String> {
        let mut feeds: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for s in &self.sends {
            if self.kind(&s.from) == Some(Kind::Bus) {
                feeds
                    .entry(s.to.as_str())
                    .or_default()
                    .push(s.from.as_str());
            }
        }
        let mut order = Vec::new();
        let mut state: BTreeMap<&str, u8> = BTreeMap::new(); // 1 visiting, 2 done
        fn visit<'a>(
            id: &'a str,
            feeds: &BTreeMap<&'a str, Vec<&'a str>>,
            state: &mut BTreeMap<&'a str, u8>,
            order: &mut Vec<&'a str>,
            path: &mut Vec<&'a str>,
        ) -> Result<(), String> {
            match state.get(id) {
                Some(2) => return Ok(()),
                Some(_) => {
                    let start = path.iter().position(|p| *p == id).unwrap_or(0);
                    let mut cycle: Vec<&str> = path[start..].to_vec();
                    cycle.push(id);
                    cycle.reverse();
                    return Err(format!(
                        "the routing has a feedback loop without a block delay: {}",
                        cycle.join(" → ")
                    ));
                }
                None => {}
            }
            state.insert(id, 1);
            path.push(id);
            for from in feeds.get(id).into_iter().flatten() {
                visit(from, feeds, state, order, path)?;
            }
            path.pop();
            state.insert(id, 2);
            order.push(id);
            Ok(())
        }
        for b in &self.buses {
            visit(&b.id, &feeds, &mut state, &mut order, &mut Vec::new())?;
        }
        Ok(order.into_iter().filter_map(|id| self.bus(id)).collect())
    }

    /// Remove a signal with its sends and its output.
    pub fn remove_signal(&mut self, id: &str) {
        self.strips.retain(|s| s.id != id);
        self.buses.retain(|b| b.id != id);
        self.sends.retain(|s| s.from != id && s.to != id);
        self.outputs.retain(|o| o.signal != id);
    }
}

fn check_id(id: &str) -> Result<(), String> {
    let ok = !id.is_empty()
        && id.len() <= 32
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !id.starts_with('-');
    if ok {
        Ok(())
    } else {
        Err(format!(
            "signal id {id:?} must be 1 to 32 characters a-z, 0-9 and '-', not starting with '-'"
        ))
    }
}

fn check_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() || name.chars().count() > 64 {
        Err("a name must have 1 to 64 characters".into())
    } else {
        Ok(())
    }
}

fn check_gain(what: &str, db: Db) -> Result<(), String> {
    if db.0 <= MAX_GAIN_DB {
        Ok(())
    } else {
        Err(format!("{what} must be -inf to +{MAX_GAIN_DB} dB"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn strip(id: &str, format: Format) -> Strip {
        let n = format.channels();
        Strip {
            id: id.into(),
            name: id.into(),
            format,
            patch: vec![None; n],
            trim_db: 0.0,
            polarity: vec![false; n],
            fader_db: Db(0.0),
            mute: false,
            pan: 0.0,
            width: 1.0,
        }
    }

    pub fn bus(id: &str, format: Format) -> Bus {
        Bus {
            id: id.into(),
            name: id.into(),
            format,
            fader_db: Db(0.0),
            mute: false,
        }
    }

    pub fn send(from: &str, to: &str) -> Send {
        Send {
            from: from.into(),
            to: to.into(),
            tap: Tap::PostFader,
            gain_db: Db(0.0),
            mute: false,
            phase: false,
        }
    }

    #[test]
    fn db_json() {
        assert_eq!(serde_json::to_string(&Db::MINUS_INF).unwrap(), "\"-inf\"");
        assert_eq!(serde_json::from_str::<Db>("-6.5").unwrap(), Db(-6.5));
        assert_eq!(
            serde_json::from_str::<Db>("\"-inf\"").unwrap(),
            Db::MINUS_INF
        );
        assert!(serde_json::from_str::<Db>("\"x\"").is_err());
        assert_eq!(Db::MINUS_INF.gain(), 0.0);
        assert!((Db(-6.0206).gain() - 0.5).abs() < 1e-4);
    }

    #[test]
    fn validation() {
        let mut mix = Mix::default();
        mix.strips.push(strip("vox", Format::Mono));
        mix.buses.push(bus("main", Format::Stereo));
        mix.sends.push(send("vox", "main"));
        assert_eq!(mix.validate(4, 96), Ok(()));

        let mut bad = mix.clone();
        bad.strips[0].patch[0] = Some(PatchPoint {
            input: 5,
            channel: 1,
        });
        assert!(bad.validate(4, 96).unwrap_err().contains("input 5"));
        let mut bad = mix.clone();
        bad.strips[0].fader_db = Db(10.5);
        assert!(bad.validate(4, 96).is_err());
        let mut bad = mix.clone();
        bad.buses[0].id = "vox".into();
        assert!(bad.validate(4, 96).unwrap_err().contains("twice"));
        let mut bad = mix.clone();
        bad.sends.push(send("main", "vox"));
        assert!(bad.validate(4, 96).unwrap_err().contains("not a bus"));
        let mut bad = mix.clone();
        bad.strips[0].id = "Vox".into();
        assert!(bad.validate(4, 96).is_err());
        let mut bad = mix.clone();
        bad.outputs.push(Output {
            signal: "main".into(),
            format: None,
            commit_samples: Some(100),
        });
        assert!(bad.validate(4, 96).unwrap_err().contains("multiple"));
        let mut bad = mix.clone();
        bad.outputs.push(Output {
            signal: "vox".into(),
            format: None,
            commit_samples: None,
        });
        assert!(bad.validate(4, 96).unwrap_err().contains("not a bus"));
    }

    #[test]
    fn feedback_without_delay_is_rejected() {
        let mut mix = Mix::default();
        for id in ["a", "b", "c"] {
            mix.buses.push(bus(id, Format::Stereo));
        }
        mix.sends.push(send("a", "b"));
        mix.sends.push(send("b", "c"));
        let order: Vec<&str> = mix
            .bus_order()
            .unwrap()
            .iter()
            .map(|b| b.id.as_str())
            .collect();
        assert_eq!(order, ["a", "b", "c"]);
        mix.sends.push(send("c", "a"));
        let err = mix.validate(1, 96).unwrap_err();
        assert!(
            err.contains("feedback loop") && err.contains("a → b → c → a"),
            "{err}"
        );
    }

    #[test]
    fn tap_formats_and_ids() {
        let mut mix = Mix::default();
        mix.strips.push(strip("strip-1", Format::Mono));
        mix.buses.push(bus("bus-1", Format::Surround51));
        assert_eq!(
            mix.tap_format("strip-1", Tap::PostFader),
            Some(Format::Mono)
        );
        assert_eq!(
            mix.tap_format("strip-1", Tap::PostPan),
            Some(Format::Stereo)
        );
        assert_eq!(
            mix.tap_format("bus-1", Tap::PostFader),
            Some(Format::Surround51)
        );
        assert_eq!(mix.next_id("strip"), "strip-2");
        assert_eq!(mix.next_id("bus"), "bus-2");
        mix.sends.push(send("strip-1", "bus-1"));
        mix.remove_signal("bus-1");
        assert!(mix.sends.is_empty() && mix.buses.is_empty());
    }
}
