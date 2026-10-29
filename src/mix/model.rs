//! The mix (§5): strips, buses, sends and published outputs, as kept in the show
//! file. Parameters are addressed by signal id and field (§12.4), e.g.
//! `strips/vox-1/fader_db` or `sends/vox-1/main/gain_db`.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::chain::{Category, Proc};
use super::format::{Downmix, Format, Laws, MonoSum, PanLaw};

/// Largest send gain (§5: −∞ … +10 dB).
pub const MAX_GAIN_DB: f64 = 10.0;
/// Largest fader (strips, buses, VCAs), and the most a fader and its VCAs add up to.
pub const MAX_FADER_DB: f64 = 15.0;
/// Trim range, ± dB.
pub const TRIM_RANGE_DB: f64 = 42.0;
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

/// Channel `channel` (from 1) of input `input` (from 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchPoint {
    pub input: usize,
    pub channel: usize,
}

/// A talkback mic (§10.2), configured or a browser's.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TalkbackPoint {
    pub talkback: String,
}

/// One strip channel's source: an input channel or a talkback mic.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Patch {
    Input(PatchPoint),
    Talkback(TalkbackPoint),
}

impl From<PatchPoint> for Patch {
    fn from(p: PatchPoint) -> Self {
        Patch::Input(p)
    }
}

impl Patch {
    pub fn input(&self) -> Option<PatchPoint> {
        match self {
            Patch::Input(p) => Some(*p),
            Patch::Talkback(_) => None,
        }
    }
}

/// An input strip: patch → trim and polarity → fader and mute → pan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Strip {
    pub id: String,
    pub name: String,
    pub format: Format,
    /// Per strip channel, the input channel or talkback mic it reads (`null`:
    /// unpatched).
    pub patch: Vec<Option<Patch>>,
    pub trim_db: f64,
    /// Per strip channel, `true` inverts the polarity.
    pub polarity: Vec<bool>,
    pub fader_db: Db,
    pub mute: bool,
    /// Mono: pan position; stereo: balance. −1 … +1. Wider formats: unused.
    pub pan: f64,
    /// Stereo width, 0 (mono) … 1 (unchanged) … 2. Other formats: unused.
    pub width: f64,
    /// Processing between the trim and the fader (§6).
    #[serde(default)]
    pub chain: Vec<Proc>,
    /// Measure loudness (§7) at the post-fader point.
    #[serde(default)]
    pub loudness: bool,
    /// The strip's colour on the console, `#rrggbb`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colour: Option<String>,
    /// Snapshot recalls leave this signal alone (§12.5).
    #[serde(default)]
    pub recall_safe: bool,
    /// Solo-in-place never mutes this signal (§10.1).
    #[serde(default)]
    pub solo_safe: bool,
    /// Mute groups never mute this signal (§9 isolate).
    #[serde(default)]
    pub mute_safe: bool,
    /// The tally index (§10.3) this strip follows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tally_index: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub afv: Option<Afv>,
}

/// Audio-follows-video (§10.3): with `enabled`, the strip is open while its tally
/// index is on air (LH) and at `depth_db` otherwise, after its fader.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Afv {
    pub enabled: bool,
    pub depth_db: Db,
    pub fade_in_ms: f64,
    pub fade_out_ms: f64,
}

/// Longest AFV fade.
pub const MAX_AFV_FADE_MS: f64 = 10_000.0;

fn yes() -> bool {
    true
}

/// A summing bus: the sum of its sends → processing → fader and mute.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bus {
    pub id: String,
    pub name: String,
    pub format: Format,
    pub fader_db: Db,
    pub mute: bool,
    #[serde(default)]
    pub chain: Vec<Proc>,
    #[serde(default)]
    pub loudness: bool,
    /// Delay compensation (§4.4): sends with less latency are delayed to the
    /// largest. Off for minimum-latency paths (talkback, IFB).
    #[serde(default = "yes")]
    pub compensate: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colour: Option<String>,
    #[serde(default)]
    pub recall_safe: bool,
    #[serde(default)]
    pub solo_safe: bool,
    #[serde(default)]
    pub mute_safe: bool,
    /// A mix-minus (§5, N-1): the bus also takes every send into `of`, as it is,
    /// except those from `without`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minus: Option<Minus>,
    /// A subgroup (C12): every send into it is taken post-fader (a strip's
    /// post-pan) at 0 dB, whatever the send says; its fader is the group level.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub subgroup: bool,
    /// The down-mix of everything arriving here from a wider format (D2);
    /// `None`: the mix's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub downmix: Option<Downmix>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Minus {
    pub of: String,
    #[serde(default)]
    pub without: Vec<String>,
}

impl Strip {
    /// The layout of its surround panner (D5), when one is on and can run.
    pub fn surround_layout(&self) -> Option<Format> {
        self.chain.iter().find_map(|p| match &p.kind {
            super::chain::ProcKind::SurroundPan { layout, .. }
                if !p.bypass && p.kind.supports(self.format, true).is_ok() =>
            {
                Some(*layout)
            }
            _ => None,
        })
    }

    /// A strip with the defaults of §5: unpatched, 0 dB, unmuted, centred.
    pub fn new(id: &str, format: Format) -> Self {
        let n = format.channels();
        Self {
            id: id.to_string(),
            name: id.to_string(),
            format,
            patch: vec![None; n],
            trim_db: 0.0,
            polarity: vec![false; n],
            fader_db: Db(0.0),
            mute: false,
            pan: 0.0,
            width: 1.0,
            chain: Vec::new(),
            loudness: false,
            colour: None,
            recall_safe: false,
            solo_safe: false,
            mute_safe: false,
            tally_index: None,
            afv: None,
        }
    }
}

impl Bus {
    /// A bus with the defaults of §5: 0 dB, unmuted, delay-compensated.
    pub fn new(id: &str, format: Format) -> Self {
        Self {
            id: id.to_string(),
            name: id.to_string(),
            format,
            fader_db: Db(0.0),
            mute: false,
            chain: Vec::new(),
            loudness: false,
            compensate: true,
            colour: None,
            recall_safe: false,
            solo_safe: false,
            mute_safe: false,
            minus: None,
            subgroup: false,
            downmix: None,
        }
    }
}

/// Where a send or key taps its source (§5). Pre/post-EQ and pre/post-dynamics
/// are the input of the first and the output of the last node of that kind in the
/// chain (the chain's input when it has none). Buses have no `input` or
/// `post-pan`. `{"after": "<node id>"}` taps after any node.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Tap {
    Input,
    PreEq,
    PostEq,
    PreDynamics,
    PostDynamics,
    PreFader,
    PostFader,
    PostPan,
    After(String),
}

const TAP_NAMES: [(&str, Tap); 8] = [
    ("input", Tap::Input),
    ("pre-eq", Tap::PreEq),
    ("post-eq", Tap::PostEq),
    ("pre-dynamics", Tap::PreDynamics),
    ("post-dynamics", Tap::PostDynamics),
    ("pre-fader", Tap::PreFader),
    ("post-fader", Tap::PostFader),
    ("post-pan", Tap::PostPan),
];

impl Serialize for Tap {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Tap::After(id) => {
                use serde::ser::SerializeMap;
                let mut m = s.serialize_map(Some(1))?;
                m.serialize_entry("after", id)?;
                m.end()
            }
            named => {
                let name = TAP_NAMES
                    .iter()
                    .find(|(_, t)| t == named)
                    .map(|(n, _)| *n)
                    .unwrap_or_default();
                s.serialize_str(name)
            }
        }
    }
}

impl<'de> Deserialize<'de> for Tap {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Name(String),
            After { after: String },
        }
        match Raw::deserialize(d)? {
            Raw::Name(n) => TAP_NAMES
                .iter()
                .find(|(name, _)| *name == n)
                .map(|(_, t)| t.clone())
                .ok_or_else(|| serde::de::Error::custom(format!("unknown tap {n}"))),
            Raw::After { after } => Ok(Tap::After(after)),
        }
    }
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
    /// Read the source one block late: the explicit block delay that lets a
    /// routing loop close through this send (§4.4).
    #[serde(default)]
    pub block_delay: bool,
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

/// What feeds a virtual output (C7). In JSON exactly one of `signal`,
/// `monitor` or `input`, with `tap` only beside `signal` and `format` only
/// beside `input`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged, try_from = "RawSource")]
pub enum OutputSource {
    /// A strip (its direct out) or a bus at `tap` (default: post-fader).
    Signal {
        signal: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tap: Option<Tap>,
    },
    /// A monitor of the control room (its output, after its level).
    Monitor { monitor: String },
    /// An input straight through, no strip. `format`: what the input carries
    /// (default: the output's format, channel for channel); another format is
    /// up- or down-mixed like a send.
    Input {
        input: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        format: Option<Format>,
    },
}

/// Every field a source may have, checked by [`OutputSource`]'s `TryFrom`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSource {
    signal: Option<String>,
    tap: Option<Tap>,
    monitor: Option<String>,
    input: Option<usize>,
    format: Option<Format>,
}

impl TryFrom<RawSource> for OutputSource {
    type Error = String;

    fn try_from(r: RawSource) -> Result<Self, String> {
        match (r.signal, r.monitor, r.input) {
            (Some(signal), None, None) if r.format.is_none() => {
                Ok(Self::Signal { signal, tap: r.tap })
            }
            (None, Some(monitor), None) if r.tap.is_none() && r.format.is_none() => {
                Ok(Self::Monitor { monitor })
            }
            (None, None, Some(input)) if r.tap.is_none() => Ok(Self::Input {
                input,
                format: r.format,
            }),
            _ => Err(
                "a source has exactly one of signal (with tap), monitor or input (with format)"
                    .into(),
            ),
        }
    }
}

/// The source of virtual output `output`: its configured `key`, else its slot
/// number (from 1) as text. An output without a route is silent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VirtualRoute {
    pub output: String,
    pub source: OutputSource,
}

/// Mix-wide laws and the loudness target.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MixSettings {
    #[serde(default)]
    pub pan_law_db: PanLaw,
    #[serde(default)]
    pub mono_sum_db: MonoSum,
    /// EBU R 128 target (§7).
    #[serde(default = "default_target")]
    pub loudness_target_lufs: f64,
    /// Integrated loudness within ± this of the target is in tolerance.
    #[serde(default = "default_tolerance")]
    pub loudness_tolerance_lu: f64,
    /// The down-mix wherever a wider format meets a narrower one (D1): sends,
    /// monitors, solo, published layouts. A bus or monitor can override it.
    /// Left out of the JSON while it is the default, so an unchanged mix still
    /// loads in 1.3 (a rollback).
    #[serde(default, skip_serializing_if = "Downmix::is_default")]
    pub downmix: Downmix,
}

fn default_target() -> f64 {
    -23.0
}

fn default_tolerance() -> f64 {
    1.0
}

impl Default for MixSettings {
    fn default() -> Self {
        Self {
            pan_law_db: PanLaw::default(),
            mono_sum_db: MonoSum::default(),
            loudness_target_lufs: default_target(),
            loudness_tolerance_lu: default_tolerance(),
            downmix: Downmix::default(),
        }
    }
}

impl MixSettings {
    pub fn laws(&self) -> Laws {
        Laws {
            pan: self.pan_law_db,
            mono_sum: self.mono_sum_db,
            downmix: self.downmix,
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
    /// What feeds the virtual outputs (C7); left out while empty (a rollback).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub virtual_outputs: Vec<VirtualRoute>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub layouts: Vec<super::show::Layout>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub snapshots: Vec<super::show::Snapshot>,
    #[serde(default)]
    pub control_room: super::room::ControlRoom,
    /// VCA (DCA) groups (§9).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vcas: Vec<super::vca::Vca>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mute_groups: Vec<super::vca::MuteGroup>,
    /// Voice activity detector settings (§8.1).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vad: Vec<super::auto::VadSettings>,
    /// Gain-sharing auto-mixers (§8.3).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub automixes: Vec<super::auto::Automix>,
    /// Ducking by speech (§8.4).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ducks: Vec<super::auto::Duck>,
    /// Speech-aware loudness (§8.2).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub autolevels: Vec<super::auto::AutoLevel>,
    /// Logic graphs (§11).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub logic: Vec<super::logic::Graph>,
    /// Logic macros: sub-graphs used as blocks (§11.3).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub logic_macros: Vec<super::logic::Macro>,
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

    /// The width of `id` at `tap`: a mono strip is stereo after its panner, a
    /// strip with a surround panner (D5) its layout.
    pub fn tap_format(&self, id: &str, tap: &Tap) -> Option<Format> {
        let (_, format) = self.signal(id)?;
        if let (Some(s), Tap::PostPan) = (self.strip(id), tap) {
            if let Some(layout) = s.surround_layout() {
                return Some(layout);
            }
        }
        Some(match (self.kind(id)?, tap, format) {
            (Kind::Strip, Tap::PostPan, Format::Mono) => Format::Stereo,
            _ => format,
        })
    }

    /// The laws for what arrives at bus `to` (its down-mix override, D2).
    pub fn laws_into(&self, to: &str) -> Laws {
        self.settings
            .laws()
            .with_downmix(self.bus(to).and_then(|b| b.downmix))
    }

    /// The nodes of `id`'s chain that cannot run on its format (C10): they pass
    /// the signal through. `(node id, why)`.
    pub fn chain_warnings(&self, id: &str) -> Vec<(String, String)> {
        let Some((_, format)) = self.signal(id) else {
            return Vec::new();
        };
        let strip = self.kind(id) == Some(Kind::Strip);
        self.chain(id)
            .unwrap_or_default()
            .iter()
            .filter_map(|p| {
                p.kind
                    .supports(format, strip)
                    .err()
                    .map(|e| (p.id.clone(), e))
            })
            .collect()
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
            .find(|id| self.kind(id).is_none() && self.vca(id).is_none())
            .expect("unbounded")
    }

    /// Check everything the compiler and the API rely on. `inputs` is the number
    /// of inputs, `block` the block size.
    pub fn validate(&self, inputs: usize, block: u32) -> Result<(), String> {
        self.validate_except_logic(inputs, block)?;
        super::logic::validate(self)
    }

    /// [`Mix::validate`] without the logic graphs and macros, which the
    /// control side checks only when they or what they name change (D171).
    pub fn validate_except_logic(&self, inputs: usize, block: u32) -> Result<(), String> {
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
        for (id, colour) in self
            .strips
            .iter()
            .map(|s| (&s.id, &s.colour))
            .chain(self.buses.iter().map(|b| (&b.id, &b.colour)))
            .chain(self.vcas.iter().map(|v| (&v.id, &v.colour)))
        {
            if let Some(c) = colour {
                let hex = c.strip_prefix('#').unwrap_or("");
                if hex.len() != 6 || !hex.chars().all(|x| x.is_ascii_hexdigit()) {
                    return Err(format!("{id}: colour {c:?} is not #rrggbb"));
                }
            }
        }
        super::show::validate(self)?;
        super::room::validate(self, inputs)?;
        super::vca::validate(self)?;
        super::auto::validate(self)?;
        for s in &self.strips {
            check_name(&s.name)?;
            if let Some(a) = &s.afv {
                if a.depth_db.0 > 0.0 {
                    return Err(format!("strip {}: afv depth_db must be -inf to 0", s.id));
                }
                for (what, ms) in [("fade_in_ms", a.fade_in_ms), ("fade_out_ms", a.fade_out_ms)] {
                    if !(0.0..=MAX_AFV_FADE_MS).contains(&ms) {
                        return Err(format!(
                            "strip {}: afv {what} must be 0 to {MAX_AFV_FADE_MS}",
                            s.id
                        ));
                    }
                }
            }
            let n = s.format.channels();
            if s.patch.len() != n || s.polarity.len() != n {
                return Err(format!(
                    "strip {}: patch and polarity need {n} entries for {}",
                    s.id,
                    serde_json::to_string(&s.format).unwrap_or_default()
                ));
            }
            for p in s.patch.iter().flatten() {
                let p = match p {
                    Patch::Input(p) => p,
                    Patch::Talkback(t) => {
                        let known = t.talkback.starts_with(super::talkback::BROWSER_PREFIX)
                            || self
                                .control_room
                                .talkback
                                .mics
                                .iter()
                                .any(|m| m.id == t.talkback);
                        if !known {
                            return Err(format!(
                                "strip {}: talkback mic {} does not exist",
                                s.id, t.talkback
                            ));
                        }
                        continue;
                    }
                };
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
            check_fader(&format!("strip {}: fader_db", s.id), s.fader_db)?;
            if !(-1.0..=1.0).contains(&s.pan) {
                return Err(format!("strip {}: pan must be -1 to 1", s.id));
            }
            if !(0.0..=2.0).contains(&s.width) {
                return Err(format!("strip {}: width must be 0 to 2", s.id));
            }
        }
        self.settings.downmix.validate()?;
        for b in &self.buses {
            check_name(&b.name)?;
            check_fader(&format!("bus {}: fader_db", b.id), b.fader_db)?;
            if let Some(d) = &b.downmix {
                d.validate().map_err(|e| format!("bus {}: {e}", b.id))?;
            }
            if let Some(m) = &b.minus {
                if self.kind(&m.of) != Some(Kind::Bus) || m.of == b.id {
                    return Err(format!(
                        "bus {}: minus of {} is not another bus",
                        b.id, m.of
                    ));
                }
                if let Some(w) = m.without.iter().find(|w| self.kind(w).is_none()) {
                    return Err(format!("bus {}: minus without {w}: no such signal", b.id));
                }
            }
        }
        let mut pairs = BTreeSet::new();
        for send in &self.sends {
            if self.kind(&send.from).is_none() {
                return Err(format!("send source {} does not exist", send.from));
            }
            if self.kind(&send.to) != Some(Kind::Bus) {
                return Err(format!("send destination {} is not a bus", send.to));
            }
            if send.from == send.to {
                return Err(format!("bus {} cannot send to itself", send.to));
            }
            self.check_tap(&send.from, &send.tap)
                .map_err(|e| format!("send {} → {}: {e}", send.from, send.to))?;
            check_gain(
                &format!("send {} → {}: gain_db", send.from, send.to),
                send.gain_db,
            )?;
            if !pairs.insert((send.from.as_str(), send.to.as_str())) {
                return Err(format!("send {} → {} exists twice", send.from, send.to));
            }
        }
        for (id, chain) in self.chains() {
            super::chain::validate(chain).map_err(|e| format!("{id}: {e}"))?;
            for p in chain {
                // An empty key (a ducker whose key was deleted) hears nothing.
                let Some(key) = p.kind.key().filter(|k| !k.signal.is_empty()) else {
                    continue;
                };
                if key.signal == id {
                    return Err(format!(
                        "{id}: node {} cannot be keyed by its own signal",
                        p.id
                    ));
                }
                if self.kind(&key.signal).is_none() {
                    return Err(format!(
                        "{id}: the key {} of node {} does not exist",
                        key.signal, p.id
                    ));
                }
                self.check_tap(&key.signal, &key.tap)
                    .map_err(|e| format!("{id}: node {} key: {e}", p.id))?;
            }
        }
        self.signal_order()?;
        let mut published = BTreeSet::new();
        if self.outputs.len() > MAX_OUTPUTS {
            return Err(format!("at most {MAX_OUTPUTS} outputs"));
        }
        for o in &self.outputs {
            // A bus, or a strip (its post-fader direct out).
            if self.kind(&o.signal).is_none() {
                return Err(format!("output signal {} does not exist", o.signal));
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
        self.validate_virtual_outputs(inputs)?;
        Ok(())
    }

    /// The virtual outputs' sources (C7): one per output, each naming what
    /// exists. Outputs this host does not have (another `MIXER_OUTPUTS`) are kept
    /// and stay silent.
    fn validate_virtual_outputs(&self, inputs: usize) -> Result<(), String> {
        let mut outputs = BTreeSet::new();
        for r in &self.virtual_outputs {
            if r.output.is_empty() || r.output.len() > 32 {
                return Err(format!("virtual output {:?}: 1 to 32 characters", r.output));
            }
            if !outputs.insert(r.output.as_str()) {
                return Err(format!("virtual output {} has two sources", r.output));
            }
            let at = |e: String| format!("virtual output {}: {e}", r.output);
            match &r.source {
                OutputSource::Signal { signal, tap } => {
                    if self.kind(signal).is_none() {
                        return Err(at(format!("signal {signal} does not exist")));
                    }
                    if let Some(t) = tap {
                        self.check_tap(signal, t).map_err(at)?;
                    }
                }
                OutputSource::Monitor { monitor } => {
                    if !self.control_room.monitors.iter().any(|m| &m.id == monitor) {
                        return Err(at(format!("monitor {monitor} does not exist")));
                    }
                }
                OutputSource::Input { input, .. } => {
                    if *input == 0 || *input > inputs {
                        return Err(at(format!("input {input} does not exist")));
                    }
                }
            }
        }
        Ok(())
    }

    /// C2: input `input` (from 1) was renamed from `old` to `new`: every strip
    /// that reads only that input and is still named `old` is named `new`.
    pub fn inherit_input_label(&mut self, input: usize, old: &str, new: &str) {
        for s in &mut self.strips {
            let inputs: BTreeSet<usize> = s
                .patch
                .iter()
                .flatten()
                .filter_map(|p| p.input().map(|p| p.input))
                .collect();
            if s.name == old && inputs.len() == 1 && inputs.contains(&input) {
                s.name = new.to_string();
            }
        }
    }

    /// The source of virtual output `output` (its key, else its slot number as
    /// text), if it has one.
    pub fn virtual_source(&self, output: &str) -> Option<&OutputSource> {
        self.virtual_outputs
            .iter()
            .find(|r| r.output == output)
            .map(|r| &r.source)
    }

    /// Set (or with `None` clear) the source of virtual output `output`; the
    /// caller validates the mix after (the logic's route action, D4, too).
    pub fn set_virtual_source(&mut self, output: &str, source: Option<OutputSource>) {
        self.virtual_outputs.retain(|r| r.output != output);
        if let Some(source) = source {
            self.virtual_outputs.push(VirtualRoute {
                output: output.to_string(),
                source,
            });
        }
    }

    /// Every signal's chain.
    pub fn chains(&self) -> impl Iterator<Item = (&str, &[Proc])> {
        self.strips
            .iter()
            .map(|s| (s.id.as_str(), s.chain.as_slice()))
            .chain(
                self.buses
                    .iter()
                    .map(|b| (b.id.as_str(), b.chain.as_slice())),
            )
    }

    pub fn chain(&self, id: &str) -> Option<&[Proc]> {
        self.chains().find(|(s, _)| *s == id).map(|(_, c)| c)
    }

    /// Every tap `id` has (those [`Mix::check_tap`] takes): the named taps in
    /// signal order, then after each node of its chain. Empty for an unknown id.
    pub fn taps(&self, id: &str) -> Vec<Tap> {
        let after = self
            .chain(id)
            .unwrap_or_default()
            .iter()
            .map(|p| Tap::After(p.id.clone()));
        TAP_NAMES
            .iter()
            .map(|(_, t)| t.clone())
            .chain(after)
            .filter(|t| self.check_tap(id, t).is_ok())
            .collect()
    }

    /// Whether `id` has `tap`.
    pub fn check_tap(&self, id: &str, tap: &Tap) -> Result<(), String> {
        match (self.kind(id), tap) {
            (None, _) => Err(format!("signal {id} does not exist")),
            (Some(Kind::Bus), Tap::Input | Tap::PostPan) => {
                Err(format!("bus {id} has no input or post-pan tap"))
            }
            (Some(_), Tap::After(node)) => {
                if self
                    .chain(id)
                    .is_some_and(|c| c.iter().any(|p| &p.id == node))
                {
                    Ok(())
                } else {
                    Err(format!("{id} has no node {node}"))
                }
            }
            _ => Ok(()),
        }
    }

    /// The chain positions of a named tap: the index of the first node of
    /// `category` and one past the last (both 0 without such a node).
    pub fn category_span(chain: &[Proc], category: Category) -> (usize, usize) {
        let first = chain.iter().position(|p| p.kind.category() == category);
        let last = chain.iter().rposition(|p| p.kind.category() == category);
        match (first, last) {
            (Some(f), Some(l)) => (f, l + 1),
            _ => (0, 0),
        }
    }

    /// Signals in processing order: a signal comes after every signal it reads
    /// (sends without a block delay, keys). A loop is an error (§4.4).
    /// The sends a mix-minus bus mirrors: (the bus, the send into its `of`).
    fn mirrored(&self) -> impl Iterator<Item = (&str, &Send)> {
        self.buses.iter().flat_map(move |b| {
            b.minus
                .iter()
                .flat_map(move |m| {
                    self.sends.iter().filter(move |s| {
                        s.to == m.of
                            && s.from != b.id
                            && !m.without.contains(&s.from)
                            && !self.sends.iter().any(|o| o.from == s.from && o.to == b.id)
                    })
                })
                .map(move |s| (b.id.as_str(), s))
        })
    }

    /// Every send the plan has: the sends, and those mix-minus buses mirror (a
    /// bus's own send from a source wins over the mirrored one).
    pub fn all_sends(&self) -> Vec<Send> {
        let mut all: Vec<Send> = self.sends.iter().map(|s| self.locked(s.clone())).collect();
        all.extend(self.mirrored().map(|(bus, s)| {
            self.locked(Send {
                to: bus.to_string(),
                ..self.locked(s.clone())
            })
        }));
        all
    }

    /// Whether `id` is a subgroup bus (C12).
    pub fn is_subgroup(&self, id: &str) -> bool {
        self.bus(id).is_some_and(|b| b.subgroup)
    }

    /// Store every send into a subgroup as the plan takes it (C12, D186).
    pub fn lock_subgroup_sends(&mut self) {
        if !self.buses.iter().any(|b| b.subgroup) {
            return;
        }
        let locked: Vec<Send> = self.sends.iter().map(|s| self.locked(s.clone())).collect();
        self.sends = locked;
    }

    /// A send as the plan takes it: into a subgroup, from the source's output
    /// tap (a strip's post-pan, a bus's post-fader) at 0 dB (C12).
    pub fn locked(&self, mut send: Send) -> Send {
        if self.is_subgroup(&send.to) {
            send.tap = self.default_tap(&send.from);
            send.gain_db = Db(0.0);
        }
        send
    }

    pub fn signal_order(&self) -> Result<Vec<&str>, String> {
        let mut feeds: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for s in self.sends.iter().filter(|s| !s.block_delay) {
            feeds
                .entry(s.to.as_str())
                .or_default()
                .push(s.from.as_str());
        }
        for (bus, s) in self.mirrored() {
            if !s.block_delay {
                feeds.entry(bus).or_default().push(s.from.as_str());
            }
        }
        for (id, chain) in self.chains() {
            for key in chain
                .iter()
                .filter_map(|p| p.kind.key())
                .filter(|k| !k.signal.is_empty())
            {
                feeds.entry(id).or_default().push(key.signal.as_str());
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
        let ids: Vec<&str> = self
            .strips
            .iter()
            .map(|s| s.id.as_str())
            .chain(self.buses.iter().map(|b| b.id.as_str()))
            .collect();
        for id in ids {
            visit(id, &feeds, &mut state, &mut order, &mut Vec::new())?;
        }
        Ok(order)
    }

    /// Remove a signal with its sends and its output, and every reference to
    /// it: mix-minus, talkback destinations and returns, monitor sources, VCAs,
    /// mute groups, the auto setup and layouts; logic blocks that name it stay
    /// with an empty target (they never act, C9).
    pub fn remove_signal(&mut self, id: &str) {
        let destinations: Vec<String> = self
            .control_room
            .talkback
            .destinations
            .iter()
            .filter(|d| d.bus == id)
            .map(|d| d.id.clone())
            .collect();
        self.strips.retain(|s| s.id != id);
        self.buses.retain(|b| b.id != id);
        self.sends.retain(|s| s.from != id && s.to != id);
        self.outputs.retain(|o| o.signal != id);
        self.virtual_outputs
            .retain(|r| !matches!(&r.source, OutputSource::Signal { signal, .. } if signal == id));
        self.control_room
            .talkback
            .destinations
            .retain(|d| d.bus != id);
        for b in &mut self.buses {
            if b.minus.as_ref().is_some_and(|m| m.of == id) {
                b.minus = None;
            }
            if let Some(m) = &mut b.minus {
                m.without.retain(|w| w != id);
            }
        }
        for v in &mut self.vcas {
            v.members.retain(|m| m != id);
        }
        for g in &mut self.mute_groups {
            g.members.retain(|m| m != id);
        }
        // What listened to it listens to nothing (C9).
        let gone = |s: &Option<super::room::Source>| matches!(s, Some(super::room::Source::Signal(x)) if x == id);
        for m in &mut self.control_room.monitors {
            if gone(&m.source) {
                m.source = None;
            }
        }
        self.control_room
            .sources
            .retain(|s| !matches!(s, super::room::Source::Signal(x) if x == id));
        for d in &mut self.control_room.talkback.destinations {
            if gone(&d.return_feed) {
                d.return_feed = None;
            }
        }
        // A key on it is cleared: a compressor keys itself again, a ducker hears
        // nothing (an empty key).
        let chains = self
            .strips
            .iter_mut()
            .map(|s| &mut s.chain)
            .chain(self.buses.iter_mut().map(|b| &mut b.chain));
        for p in chains.flatten() {
            match &mut p.kind {
                super::chain::ProcKind::Compressor { key, .. }
                    if key.as_ref().is_some_and(|k| k.signal == id) =>
                {
                    *key = None;
                }
                super::chain::ProcKind::Ducker { key, .. } if key.signal == id => {
                    key.signal.clear();
                }
                _ => {}
            }
        }
        self.remove_from_auto(id);
        self.remove_from_logic(id, &destinations);
        self.remove_from_layouts(id);
    }

    /// Take `id` (a signal or VCA) out of every layout page.
    pub fn remove_from_layouts(&mut self, id: &str) {
        for p in self.layouts.iter_mut().flat_map(|l| l.pages.iter_mut()) {
            p.signals.retain(|s| s != id);
        }
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

pub(crate) fn check_fader(what: &str, db: Db) -> Result<(), String> {
    if db.0 <= MAX_FADER_DB {
        Ok(())
    } else {
        Err(format!("{what} must be -inf to +{MAX_FADER_DB} dB"))
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
pub(crate) mod tests {
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
            chain: Vec::new(),
            loudness: false,
            colour: None,
            recall_safe: false,
            solo_safe: false,
            mute_safe: false,
            tally_index: None,
            afv: None,
        }
    }

    pub fn bus(id: &str, format: Format) -> Bus {
        Bus {
            id: id.into(),
            name: id.into(),
            format,
            fader_db: Db(0.0),
            mute: false,
            chain: Vec::new(),
            loudness: false,
            compensate: true,
            colour: None,
            recall_safe: false,
            solo_safe: false,
            mute_safe: false,
            minus: None,
            subgroup: false,
            downmix: None,
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
            block_delay: false,
        }
    }

    #[test]
    fn default_down_mix_is_left_out_for_a_rollback() {
        // An unchanged mix has no `downmix` in its settings: 1.3 (deny_unknown_fields) loads it.
        let mix = Mix::default();
        let v = serde_json::to_value(&mix).unwrap();
        assert!(v["settings"].get("downmix").is_none(), "{v}");
        let back: Mix = serde_json::from_value(v).unwrap();
        assert_eq!(back, mix);
        // A changed one is written and read back.
        let mut changed = Mix::default();
        changed.settings.downmix.centre_db = Db(-6.0);
        let v = serde_json::to_value(&changed).unwrap();
        assert_eq!(v["settings"]["downmix"]["centre_db"], -6.0);
        let back: Mix = serde_json::from_value(v).unwrap();
        assert_eq!(back, changed);
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
        bad.strips[0].patch[0] = Some(
            PatchPoint {
                input: 5,
                channel: 1,
            }
            .into(),
        );
        assert!(bad.validate(4, 96).unwrap_err().contains("input 5"));
        let mut bad = mix.clone();
        bad.strips[0].fader_db = Db(15.5);
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
            signal: "nope".into(),
            format: None,
            commit_samples: None,
        });
        assert!(bad.validate(4, 96).unwrap_err().contains("does not exist"));
        // A strip is published as its post-fader direct out.
        let mut direct = mix.clone();
        direct.outputs.push(Output {
            signal: "vox".into(),
            format: None,
            commit_samples: None,
        });
        assert_eq!(direct.validate(4, 96), Ok(()));
    }

    #[test]
    fn feedback_without_delay_is_rejected() {
        let mut mix = Mix::default();
        for id in ["a", "b", "c"] {
            mix.buses.push(bus(id, Format::Stereo));
        }
        mix.sends.push(send("a", "b"));
        mix.sends.push(send("b", "c"));
        assert_eq!(mix.signal_order().unwrap(), ["a", "b", "c"]);
        mix.sends.push(send("c", "a"));
        let err = mix.validate(1, 96).unwrap_err();
        assert!(
            err.contains("feedback loop") && err.contains("a → b → c → a"),
            "{err}"
        );
        // An explicit block delay on one send of the loop makes it valid.
        mix.sends.last_mut().unwrap().block_delay = true;
        assert_eq!(mix.validate(1, 96), Ok(()));
    }

    #[test]
    fn taps_and_keys() {
        let mut mix = Mix::default();
        mix.strips.push(strip("vox", Format::Mono));
        mix.strips.push(strip("music", Format::Stereo));
        mix.buses.push(bus("main", Format::Stereo));
        mix.strips[0].chain = serde_json::from_str(
            r#"[{"id": "eq", "type": "eq", "bands": [{"type": "bell", "freq_hz": 1000, "q": 1, "gain_db": 3}]}]"#,
        )
        .unwrap();
        let mut s = send("vox", "main");
        s.tap = Tap::After("eq".into());
        mix.sends.push(s.clone());
        assert_eq!(mix.validate(2, 96), Ok(()));
        let mut bad = mix.clone();
        bad.sends[0].tap = Tap::After("nope".into());
        assert!(bad.validate(2, 96).unwrap_err().contains("no node nope"));
        let mut bad = mix.clone();
        bad.sends.push(Send {
            from: "main".into(),
            to: "main".into(),
            ..s
        });
        assert!(bad.validate(2, 96).is_err());
        // Music ducked under vox: vox comes first in the order.
        mix.strips[1].chain = serde_json::from_str(
            r#"[{"id": "duck", "type": "ducker", "key": {"signal": "vox", "tap": "post-fader"}, "threshold_db": -40, "depth_db": 10, "attack_ms": 5, "hold_ms": 100, "release_ms": 300}]"#,
        )
        .unwrap();
        mix.strips.swap(0, 1);
        assert_eq!(mix.validate(2, 96), Ok(()));
        let order = mix.signal_order().unwrap();
        assert!(order.iter().position(|s| *s == "vox") < order.iter().position(|s| *s == "music"));
        let mut bad = mix.clone();
        bad.strips[0].chain[0] = serde_json::from_str(
            r#"{"id": "duck", "type": "ducker", "key": {"signal": "music", "tap": "post-fader"}, "threshold_db": -40, "depth_db": 10, "attack_ms": 5, "hold_ms": 100, "release_ms": 300}"#,
        )
        .unwrap();
        assert!(bad.validate(2, 96).unwrap_err().contains("own signal"));
        assert_eq!(
            serde_json::to_value(Tap::After("eq".into())).unwrap(),
            serde_json::json!({"after": "eq"})
        );
        assert_eq!(
            serde_json::from_str::<Tap>("\"pre-eq\"").unwrap(),
            Tap::PreEq
        );
    }

    #[test]
    fn tap_formats_and_ids() {
        let mut mix = Mix::default();
        mix.strips.push(strip("strip-1", Format::Mono));
        mix.buses.push(bus("bus-1", Format::Surround51));
        assert_eq!(
            mix.tap_format("strip-1", &Tap::PostFader),
            Some(Format::Mono)
        );
        assert_eq!(
            mix.tap_format("strip-1", &Tap::PostPan),
            Some(Format::Stereo)
        );
        assert_eq!(
            mix.tap_format("bus-1", &Tap::PostFader),
            Some(Format::Surround51)
        );
        assert_eq!(mix.next_id("strip"), "strip-2");
        assert_eq!(mix.next_id("bus"), "bus-2");
        // The taps a source offers: a bus has no input or post-pan.
        let names = |id: &str| -> Vec<String> {
            mix.taps(id)
                .iter()
                .map(|t| serde_json::to_value(t).unwrap().to_string())
                .collect()
        };
        assert_eq!(names("strip-1").len(), 8);
        assert!(!names("bus-1")
            .iter()
            .any(|t| t.contains("input") || t.contains("post-pan")));
        assert_eq!(names("bus-1").len(), 6);
        assert!(names("nope").is_empty());
        mix.sends.push(send("strip-1", "bus-1"));
        mix.remove_signal("bus-1");
        assert!(mix.sends.is_empty() && mix.buses.is_empty());
    }

    /// C9: deleting a signal takes it out of everything that names it.
    #[test]
    fn deleting_a_signal_removes_its_references() {
        use serde_json::json;
        let mut mix = Mix::default();
        mix.strips.push(strip("vox", Format::Mono));
        mix.strips.push(strip("gtr", Format::Mono));
        mix.buses.push(bus("main", Format::Stereo));
        mix.sends.push(send("vox", "main"));
        mix.sends.push(send("gtr", "main"));
        mix.vcas.push(
            serde_json::from_value(json!({"id": "v", "name": "V", "members": ["vox", "gtr"]}))
                .unwrap(),
        );
        mix.mute_groups.push(
            serde_json::from_value(json!({"id": "g", "name": "G", "members": ["vox"]})).unwrap(),
        );
        mix.layouts.push(
            serde_json::from_value(
                json!({"name": "L", "pages": [{"name": "P", "signals": ["vox", "main"]}]}),
            )
            .unwrap(),
        );
        mix.control_room.monitors.push(
            serde_json::from_value(
                json!({"id": "pfl", "name": "PFL", "format": "stereo", "source": {"signal": "vox"}}),
            )
            .unwrap(),
        );
        mix.control_room.sources = vec![
            serde_json::from_value(json!({"signal": "vox"})).unwrap(),
            serde_json::from_value(json!({"signal": "main"})).unwrap(),
        ];
        let t = &mut mix.control_room.talkback;
        t.mics.push(
            serde_json::from_value(json!({"id": "tb", "name": "TB", "input": 1, "channel": 1}))
                .unwrap(),
        );
        t.destinations.push(
            serde_json::from_value(
                json!({"id": "ifb", "name": "IFB", "bus": "main", "return": {"signal": "vox"}}),
            )
            .unwrap(),
        );
        // Keys on vox: gtr's compressor and ducker.
        let node = |v| serde_json::from_value::<crate::mix::chain::Proc>(v).unwrap();
        mix.strips[1].chain.push(node(json!({
            "id": "comp", "type": "compressor", "threshold_db": -20.0, "ratio": 4.0,
            "knee_db": 0.0, "attack_ms": 5.0, "release_ms": 100.0, "makeup_db": 0.0,
            "detector": "peak", "key": {"signal": "vox", "tap": "post-fader"}})));
        mix.strips[1].chain.push(node(json!({
            "id": "duck", "type": "ducker", "key": {"signal": "vox", "tap": "post-fader"},
            "threshold_db": -40.0, "depth_db": 10.0, "attack_ms": 5.0, "hold_ms": 100.0,
            "release_ms": 200.0})));
        assert_eq!(mix.validate(1, 48), Ok(()));
        mix.remove_signal("vox");
        assert_eq!(mix.validate(1, 48), Ok(()));
        assert_eq!(mix.sends.len(), 1);
        assert_eq!(mix.vcas[0].members, ["gtr"]);
        assert!(mix.mute_groups[0].members.is_empty());
        assert_eq!(mix.layouts[0].pages[0].signals, ["main"]);
        assert_eq!(mix.control_room.monitors[0].source, None);
        assert_eq!(mix.control_room.sources.len(), 1, "main stays");
        assert_eq!(mix.control_room.talkback.destinations[0].return_feed, None);
        assert_eq!(
            mix.strips[0].chain[0].kind.key(),
            None,
            "the compressor keys itself"
        );
        assert_eq!(
            mix.strips[0].chain[1].kind.key().unwrap().signal,
            "",
            "the ducker hears nothing"
        );
        // Deleting main takes its talkback destination.
        mix.remove_signal("main");
        assert!(mix.control_room.talkback.destinations.is_empty());
        assert_eq!(mix.validate(1, 48), Ok(()));
    }

    /// C12: sends into a subgroup are taken post-pan at 0 dB.
    #[test]
    fn subgroup_sends_are_locked() {
        let mut mix = Mix::default();
        mix.strips.push(strip("vox", Format::Mono));
        mix.buses.push(bus("sub", Format::Stereo));
        mix.buses.push(bus("main", Format::Stereo));
        mix.sends.push(Send {
            tap: Tap::PreFader,
            gain_db: Db(-12.0),
            ..send("vox", "sub")
        });
        mix.sends.push(Send {
            gain_db: Db(-3.0),
            ..send("sub", "main")
        });
        assert_eq!(mix.all_sends(), mix.sends);
        mix.buses[0].subgroup = true;
        let all = mix.all_sends();
        assert_eq!((&all[0].tap, all[0].gain_db), (&Tap::PostPan, Db(0.0)));
        assert_eq!(all[1], mix.sends[1], "sends out of a subgroup are free");
        // Stored as heard, and kept so when the flag is cleared.
        mix.lock_subgroup_sends();
        assert_eq!(mix.sends, all);
        mix.buses[0].subgroup = false;
        mix.lock_subgroup_sends();
        assert_eq!(mix.all_sends(), all);
        let json = serde_json::to_value(&mix.buses[1]).unwrap();
        assert!(json.get("subgroup").is_none(), "not written while off");
    }
}
