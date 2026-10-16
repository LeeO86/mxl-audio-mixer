//! Logic graphs (§11.1): blocks with typed ports, wired together, saved in the
//! show; macros (§11.3) are sub-graphs used as blocks. The runtime
//! (`crate::logic`) evaluates them at 100 Hz.
//!
//! Every port has a [`PortType`]. A wire goes from an output to an input, an
//! input takes one wire and converts what it receives ([`PortType::accepts`],
//! D158). A loop is allowed only through a block that remembers
//! ([`BlockKind::remembers`]): those publish last tick's value, so the loop is
//! not combinational.
//!
//! A macro's exposed ports are its `macro-in` and `macro-out` blocks. For
//! evaluation every `macro` block (an instance) is inlined ([`inline`]): its
//! blocks become `<instance>.<block>`, and its `macro-in` and `macro-out`
//! blocks pass their value through.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::chain::Proc;
use super::model::{Mix, MAX_RAMP_MS};
use super::room::Source;
use super::vca::{check_id, check_name};

pub const MAX_GRAPHS: usize = 32;
pub const MAX_BLOCKS: usize = 64;
pub const MAX_MACROS: usize = 32;
/// Macros inside macros, counting the outermost.
pub const MAX_DEPTH: usize = 4;
/// Blocks of one graph once its macros are inlined.
pub const MAX_INLINED: usize = 256;
pub const MAX_SELECT: u8 = 8;
pub const MAX_POINTS: usize = 16;
/// Characters of a `text` value or a `log` message.
pub const MAX_TEXT: usize = 200;
/// Characters of a graph's or macro's description.
pub const MAX_DESCRIPTION: usize = 1000;
/// Blocks and macro instances one inlining visits (D169): bounds the work of
/// nested instances that hold no blocks of their own.
pub const MAX_EXPANDED: usize = 2 * MAX_INLINED;
/// Longest timer, delay or debounce: one day.
pub const MAX_MS: f64 = 86_400_000.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Graph {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Values are computed; actions are listed, not run.
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub blocks: Vec<Block>,
    #[serde(default)]
    pub wires: Vec<Wire>,
}

fn yes() -> bool {
    true
}

/// A sub-graph used as a block (§11.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Macro {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub blocks: Vec<Block>,
    #[serde(default)]
    pub wires: Vec<Wire>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Block {
    pub id: String,
    #[serde(flatten)]
    pub kind: BlockKind,
    /// Where the canvas draws it.
    #[serde(default)]
    pub x: f64,
    #[serde(default)]
    pub y: f64,
}

/// What a port carries (§11.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PortType {
    Boolean,
    Number,
    /// A number in dB.
    Level,
    /// True for the one tick it fires.
    Event,
    String,
}

impl PortType {
    /// An input of this type takes a wire from an output of type `from` (D158):
    /// booleans, numbers and levels convert into each other, anything but an
    /// event into a string, anything but a string into an event.
    pub fn accepts(self, from: PortType) -> bool {
        match (from, self) {
            (a, b) if a == b => true,
            (PortType::String, _) | (PortType::Event, PortType::String) => false,
            _ => true,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Boolean => "boolean",
            Self::Number => "number",
            Self::Level => "level",
            Self::Event => "event",
            Self::String => "string",
        }
    }
}

fn number_type() -> PortType {
    PortType::Number
}

fn two() -> u8 {
    2
}

fn ramp10() -> f64 {
    super::model::DEFAULT_RAMP_MS
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum BlockKind {
    // Inputs.
    /// A fixed number.
    Const {
        value: f64,
    },
    /// A fixed string.
    Text {
        value: String,
    },
    /// A soft key: on while pressed (runtime, not in the show).
    Button,
    /// Fires on the tick after `POST /api/v1/logic/<graph>/trigger/<block>`.
    Trigger,
    /// The signal's mute.
    Mute {
        signal: String,
    },
    /// The signal (strip, bus or VCA) is in PFL.
    Pfl {
        signal: String,
    },
    /// The signal's fader, dB.
    Fader {
        signal: String,
    },
    /// The signal's voice detection.
    Vad {
        signal: String,
    },
    /// The signal's peak level, dBFS.
    Level {
        signal: String,
    },
    /// On once the signal's peak reaches `threshold_db`, until it falls below
    /// that by `hysteresis_db`.
    Above {
        signal: String,
        threshold_db: f64,
        hysteresis_db: f64,
    },
    /// One of the signal's loudness values (LUFS, LU or dBTP).
    Loudness {
        signal: String,
        measure: Measure,
    },
    /// TSL tally `index` is on air (the left lamp).
    Tally {
        index: u16,
    },
    /// On while the UTC minute of the day is inside `[start_minute, end_minute)`.
    /// If `end_minute` is earlier, the window crosses midnight.
    Schedule {
        start_minute: u16,
        end_minute: u16,
    },
    /// The NMOS receiver of input `input` (1 is the first) is active.
    NmosReceiver {
        input: u32,
    },
    /// The NMOS sender of the signal's published output is active.
    NmosSender {
        signal: String,
    },
    /// The remote link to `peer` (`master`, or `s1` to `s4`) is up.
    Link {
        peer: String,
    },

    // Logic.
    And,
    Or,
    /// On when exactly one of `a` and `b` is on.
    Xor,
    Not,
    Compare {
        op: CompareOp,
    },
    /// `a` and `b` combined by `op`.
    Math {
        op: MathOp,
    },
    /// `in` × `factor` + `offset`.
    Scale {
        factor: f64,
        #[serde(default)]
        offset: f64,
    },
    /// The curve through `points` (`[x, y]`, x rising), flat outside them.
    Map {
        points: Vec<[f64; 2]>,
    },
    /// The input `sel` names (rounded, clamped), of `inputs` inputs.
    Select {
        #[serde(default = "two")]
        inputs: u8,
        #[serde(default = "number_type")]
        value_type: PortType,
    },
    /// Fires on the tick its input rose.
    Edge,
    /// Stays on after `set`, off after `reset` (reset wins).
    Latch,
    /// Flips on each event.
    Toggle,
    /// Counts `up` and `down` events; `reset` sets it to 0.
    Counter,
    /// On for `ms` milliseconds after `in` rises, rounded up to the 10 ms tick.
    /// A rise while it is still on starts the duration again.
    Timer {
        ms: f64,
    },
    /// The input, `ms` later (rounded up to the 10 ms tick).
    Delay {
        ms: f64,
    },
    /// Follows the input only after it has stayed the same for `ms`.
    Debounce {
        ms: f64,
    },

    // Actions.
    /// Sets the signal's mute from `in`.
    SetMute {
        signal: String,
    },
    /// Sets the signal's fader (dB) from `level`.
    SetFader {
        signal: String,
        #[serde(default = "ramp10")]
        ramp_ms: f64,
    },
    /// Sets the VCA's fader (dB) from `level`.
    SetVca {
        vca: String,
        #[serde(default = "ramp10")]
        ramp_ms: f64,
    },
    /// Sets a parameter the DSP smooths, of node `node` in the signal's chain,
    /// from `value`. `param` is a field (`threshold_db`), a path
    /// (`bands/1/gain_db`) or `bypass`.
    SetParam {
        signal: String,
        node: String,
        param: String,
        #[serde(default = "ramp10")]
        ramp_ms: f64,
    },
    /// PFL of the signal (strip, bus or VCA) from `in`.
    SetPfl {
        signal: String,
    },
    /// Talkback mic `mic` talks to `destination` while `in` is on.
    SetTalk {
        mic: String,
        destination: String,
    },
    /// The signal is published while `in` is on.
    SetPublish {
        signal: String,
    },
    /// Monitor `monitor` listens to `source` when `in` fires.
    SetMonitor {
        monitor: String,
        source: Option<Source>,
    },
    /// Starts, stops or resets the signal's loudness integration when `in` fires.
    LoudnessControl {
        signal: String,
        action: LoudnessAction,
    },
    /// Recalls snapshot `snapshot` when `in` fires.
    Recall {
        snapshot: String,
    },
    /// POST `url` when `in` fires. The body is `{"graph", "block", "value": 1}`.
    Webhook {
        url: String,
    },
    /// TSL UMD 5.0 display `index` on `TSL_OUT_TARGET`: `left` lamp (red),
    /// `right` lamp (green), `label`.
    TslOut {
        index: u16,
    },
    /// A log line and an event on the events socket when `in` fires: `message`,
    /// then `value` if wired.
    Log {
        message: String,
    },

    // Macros.
    /// An instance of macro `macro`.
    Macro {
        #[serde(rename = "macro")]
        macro_id: String,
    },
    /// Inside a macro: the instance's input `name`.
    MacroIn {
        name: String,
        value_type: PortType,
    },
    /// Inside a macro: the instance's output `name`.
    MacroOut {
        name: String,
        value_type: PortType,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CompareOp {
    Gt,
    Lt,
    Eq,
    Ge,
    Le,
    Ne,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MathOp {
    Add,
    Sub,
    Mul,
    Div,
    Min,
    Max,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Measure {
    Momentary,
    ShortTerm,
    Integrated,
    Range,
    TruePeak,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LoudnessAction {
    Start,
    Stop,
    Reset,
}

impl LoudnessAction {
    pub fn name(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Reset => "reset",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Wire {
    pub from_block: String,
    pub from_port: String,
    pub to_block: String,
    pub to_port: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Port {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: PortType,
}

/// A block's ports.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Ports {
    pub inputs: Vec<Port>,
    pub outputs: Vec<Port>,
}

impl Ports {
    pub fn input(&self, name: &str) -> Option<usize> {
        self.inputs.iter().position(|p| p.name == name)
    }
}

fn port(name: &str, ty: PortType) -> Port {
    Port {
        name: name.into(),
        ty,
    }
}

impl BlockKind {
    /// The ports; a macro instance's come from its definition in `macros`
    /// (none if it is missing).
    pub fn ports(&self, macros: &[Macro]) -> Ports {
        use PortType::{Boolean as B, Event as E, Level as L, Number as N, String as S};
        let out = |t| vec![port("out", t)];
        let (inputs, outputs) = match self {
            Self::Const { .. } => (vec![], out(N)),
            Self::Text { .. } => (vec![], out(S)),
            Self::Trigger => (vec![], out(E)),
            Self::Button
            | Self::Mute { .. }
            | Self::Pfl { .. }
            | Self::Vad { .. }
            | Self::Above { .. }
            | Self::Tally { .. }
            | Self::Schedule { .. }
            | Self::NmosReceiver { .. }
            | Self::NmosSender { .. }
            | Self::Link { .. } => (vec![], out(B)),
            Self::Fader { .. } | Self::Level { .. } | Self::Loudness { .. } => (vec![], out(L)),
            Self::And | Self::Or | Self::Xor => (vec![port("a", B), port("b", B)], out(B)),
            Self::Not => (vec![port("in", B)], out(B)),
            Self::Compare { .. } => (vec![port("a", N), port("b", N)], out(B)),
            Self::Math { .. } => (vec![port("a", N), port("b", N)], out(N)),
            Self::Scale { .. } | Self::Map { .. } => (vec![port("in", N)], out(N)),
            Self::Select { inputs, value_type } => {
                let mut v = vec![port("sel", N)];
                v.extend((0..*inputs).map(|i| port(&format!("in{i}"), *value_type)));
                (v, out(*value_type))
            }
            Self::Edge => (vec![port("in", B)], out(E)),
            Self::Latch => (vec![port("set", B), port("reset", B)], out(B)),
            Self::Toggle => (vec![port("in", E)], out(B)),
            Self::Counter => (
                vec![port("up", E), port("down", E), port("reset", E)],
                out(N),
            ),
            Self::Timer { .. } | Self::Delay { .. } | Self::Debounce { .. } => {
                (vec![port("in", B)], out(B))
            }
            Self::SetMute { .. }
            | Self::SetPfl { .. }
            | Self::SetTalk { .. }
            | Self::SetPublish { .. } => (vec![port("in", B)], vec![]),
            Self::SetFader { .. } | Self::SetVca { .. } => (vec![port("level", L)], vec![]),
            Self::SetParam { .. } => (vec![port("value", N)], vec![]),
            Self::SetMonitor { .. }
            | Self::LoudnessControl { .. }
            | Self::Recall { .. }
            | Self::Webhook { .. } => (vec![port("in", E)], vec![]),
            Self::TslOut { .. } => (
                vec![port("left", B), port("right", B), port("label", S)],
                vec![],
            ),
            Self::Log { .. } => (vec![port("in", E), port("value", S)], vec![]),
            Self::Macro { macro_id } => {
                let Some(m) = macros.iter().find(|m| &m.id == macro_id) else {
                    return Ports::default();
                };
                let mut p = Ports::default();
                for b in &m.blocks {
                    match &b.kind {
                        Self::MacroIn { name, value_type } => {
                            p.inputs.push(port(name, *value_type))
                        }
                        Self::MacroOut { name, value_type } => {
                            p.outputs.push(port(name, *value_type))
                        }
                        _ => {}
                    }
                }
                return p;
            }
            // Seen from inside the macro.
            Self::MacroIn { value_type, .. } => (vec![], out(*value_type)),
            Self::MacroOut { value_type, .. } => (vec![port("in", *value_type)], vec![]),
        };
        Ports { inputs, outputs }
    }

    /// Remembers, so a loop through it is not combinational. It publishes the
    /// value it computed last tick.
    pub fn remembers(&self) -> bool {
        matches!(
            self,
            Self::Edge
                | Self::Latch
                | Self::Toggle
                | Self::Counter
                | Self::Timer { .. }
                | Self::Delay { .. }
                | Self::Debounce { .. }
        )
    }

    pub fn is_action(&self) -> bool {
        self.category() == "action"
    }

    /// `input`, `logic`, `action` or `macro`.
    pub fn category(&self) -> &'static str {
        match self {
            Self::Const { .. }
            | Self::Text { .. }
            | Self::Button
            | Self::Trigger
            | Self::Mute { .. }
            | Self::Pfl { .. }
            | Self::Fader { .. }
            | Self::Vad { .. }
            | Self::Level { .. }
            | Self::Above { .. }
            | Self::Loudness { .. }
            | Self::Tally { .. }
            | Self::Schedule { .. }
            | Self::NmosReceiver { .. }
            | Self::NmosSender { .. }
            | Self::Link { .. } => "input",
            Self::And
            | Self::Or
            | Self::Xor
            | Self::Not
            | Self::Compare { .. }
            | Self::Math { .. }
            | Self::Scale { .. }
            | Self::Map { .. }
            | Self::Select { .. }
            | Self::Edge
            | Self::Latch
            | Self::Toggle
            | Self::Counter
            | Self::Timer { .. }
            | Self::Delay { .. }
            | Self::Debounce { .. } => "logic",
            Self::Macro { .. } | Self::MacroIn { .. } | Self::MacroOut { .. } => "macro",
            _ => "action",
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Const { .. } => "const",
            Self::Text { .. } => "text",
            Self::Button => "button",
            Self::Trigger => "trigger",
            Self::Mute { .. } => "mute",
            Self::Pfl { .. } => "pfl",
            Self::Fader { .. } => "fader",
            Self::Vad { .. } => "vad",
            Self::Level { .. } => "level",
            Self::Above { .. } => "above",
            Self::Loudness { .. } => "loudness",
            Self::Tally { .. } => "tally",
            Self::Schedule { .. } => "schedule",
            Self::NmosReceiver { .. } => "nmos-receiver",
            Self::NmosSender { .. } => "nmos-sender",
            Self::Link { .. } => "link",
            Self::And => "and",
            Self::Or => "or",
            Self::Xor => "xor",
            Self::Not => "not",
            Self::Compare { .. } => "compare",
            Self::Math { .. } => "math",
            Self::Scale { .. } => "scale",
            Self::Map { .. } => "map",
            Self::Select { .. } => "select",
            Self::Edge => "edge",
            Self::Latch => "latch",
            Self::Toggle => "toggle",
            Self::Counter => "counter",
            Self::Timer { .. } => "timer",
            Self::Delay { .. } => "delay",
            Self::Debounce { .. } => "debounce",
            Self::SetMute { .. } => "set-mute",
            Self::SetFader { .. } => "set-fader",
            Self::SetVca { .. } => "set-vca",
            Self::SetParam { .. } => "set-param",
            Self::SetPfl { .. } => "set-pfl",
            Self::SetTalk { .. } => "set-talk",
            Self::SetPublish { .. } => "set-publish",
            Self::SetMonitor { .. } => "set-monitor",
            Self::LoudnessControl { .. } => "loudness-control",
            Self::Recall { .. } => "recall",
            Self::Webhook { .. } => "webhook",
            Self::TslOut { .. } => "tsl-out",
            Self::Log { .. } => "log",
            Self::Macro { .. } => "macro",
            Self::MacroIn { .. } => "macro-in",
            Self::MacroOut { .. } => "macro-out",
        }
    }

    /// The strip, bus or VCA the block names.
    pub fn signal(&self) -> Option<&str> {
        match self {
            Self::Mute { signal }
            | Self::Pfl { signal }
            | Self::Fader { signal }
            | Self::Vad { signal }
            | Self::Level { signal }
            | Self::Above { signal, .. }
            | Self::Loudness { signal, .. }
            | Self::NmosSender { signal }
            | Self::SetMute { signal }
            | Self::SetFader { signal, .. }
            | Self::SetParam { signal, .. }
            | Self::SetPfl { signal }
            | Self::SetPublish { signal }
            | Self::LoudnessControl { signal, .. } => Some(signal),
            Self::SetVca { vca, .. } => Some(vca),
            _ => None,
        }
    }
}

impl Mix {
    pub fn logic_graph(&self, id: &str) -> Option<&Graph> {
        self.logic.iter().find(|g| g.id == id)
    }

    pub fn logic_macro(&self, id: &str) -> Option<&Macro> {
        self.logic_macros.iter().find(|m| m.id == id)
    }

    /// Every block of every graph and macro.
    pub fn logic_blocks(&self) -> impl Iterator<Item = &Block> {
        self.logic
            .iter()
            .flat_map(|g| &g.blocks)
            .chain(self.logic_macros.iter().flat_map(|m| &m.blocks))
    }

    /// Signals a logic graph reads with voice detection.
    pub fn logic_vad_signals(&self) -> impl Iterator<Item = &str> {
        self.logic_blocks().filter_map(|b| match &b.kind {
            BlockKind::Vad { signal } => Some(signal.as_str()),
            _ => None,
        })
    }

    /// The block at `path` of graph `graph`: a block id, or
    /// `<instance>.<block>` inside a macro instance (nested: `a.b.c`).
    pub fn logic_block_at(&self, graph: &str, path: &str) -> Option<&BlockKind> {
        let mut blocks = &self.logic_graph(graph)?.blocks;
        let mut parts = path.split('.').peekable();
        while let Some(part) = parts.next() {
            let b = blocks.iter().find(|b| b.id == part)?;
            if parts.peek().is_none() {
                return Some(&b.kind);
            }
            let BlockKind::Macro { macro_id } = &b.kind else {
                return None;
            };
            blocks = &self.logic_macro(macro_id)?.blocks;
        }
        None
    }

    /// Remove the blocks (and their wires) that name signal `id`.
    pub fn remove_from_logic(&mut self, id: &str) {
        let strip = |blocks: &mut Vec<Block>, wires: &mut Vec<Wire>| {
            let gone: Vec<String> = blocks
                .iter()
                .filter(|b| b.kind.signal() == Some(id))
                .map(|b| b.id.clone())
                .collect();
            blocks.retain(|b| b.kind.signal() != Some(id));
            wires.retain(|w| !gone.contains(&w.from_block) && !gone.contains(&w.to_block));
        };
        for g in &mut self.logic {
            strip(&mut g.blocks, &mut g.wires);
        }
        for m in &mut self.logic_macros {
            strip(&mut m.blocks, &mut m.wires);
        }
    }
}

/// One validation problem, where the UI puts its marker (D162).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Issue {
    pub graph: Option<String>,
    #[serde(rename = "macro")]
    pub macro_id: Option<String>,
    pub block: Option<String>,
    pub port: Option<String>,
    /// Index in `wires`.
    pub wire: Option<usize>,
    pub message: String,
}

impl std::fmt::Display for Issue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (&self.graph, &self.macro_id) {
            (Some(g), _) => write!(f, "logic {g}: ")?,
            (None, Some(m)) => write!(f, "logic macro {m}: ")?,
            (None, None) => write!(f, "logic: ")?,
        }
        if let Some(b) = &self.block {
            write!(f, "block {b}: ")?;
        }
        if let Some(p) = &self.port {
            write!(f, "port {p}: ")?;
        }
        f.write_str(&self.message)
    }
}

/// Where issues are found: a graph or a macro.
#[derive(Clone, Copy)]
enum At<'a> {
    Top,
    Graph(&'a str),
    Macro(&'a str),
}

impl At<'_> {
    fn issue(
        self,
        block: Option<&str>,
        port: Option<&str>,
        wire: Option<usize>,
        message: String,
    ) -> Issue {
        let (graph, macro_id) = match self {
            At::Top => (None, None),
            At::Graph(g) => (Some(g.to_string()), None),
            At::Macro(m) => (None, Some(m.to_string())),
        };
        Issue {
            graph,
            macro_id,
            block: block.map(str::to_string),
            port: port.map(str::to_string),
            wire,
            message,
        }
    }
}

/// What logic validation reads of the mix besides the graphs and macros: the
/// strips and buses with their chains' structure, VCAs, talkback mics and
/// destinations, monitors (D171). Equal contexts and equal logic validate the
/// same.
/// A chain node as logic validation sees it: id, type, structure.
type NodeShape = (String, &'static str, u64);

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Context {
    signals: Vec<(String, Vec<NodeShape>)>,
    vcas: Vec<String>,
    mics: Vec<String>,
    destinations: Vec<String>,
    monitors: Vec<String>,
}

impl Context {
    pub fn of(mix: &Mix) -> Self {
        let chain = |c: &[Proc]| {
            c.iter()
                .map(|p| (p.id.clone(), p.kind.type_name(), p.kind.signature()))
                .collect()
        };
        Self {
            signals: mix
                .strips
                .iter()
                .map(|s| (s.id.clone(), chain(&s.chain)))
                .chain(mix.buses.iter().map(|b| (b.id.clone(), chain(&b.chain))))
                .collect(),
            vcas: mix.vcas.iter().map(|v| v.id.clone()).collect(),
            mics: mix
                .control_room
                .talkback
                .mics
                .iter()
                .map(|m| m.id.clone())
                .collect(),
            destinations: mix
                .control_room
                .talkback
                .destinations
                .iter()
                .map(|d| d.id.clone())
                .collect(),
            monitors: mix
                .control_room
                .monitors
                .iter()
                .map(|m| m.id.clone())
                .collect(),
        }
    }
}

/// The first problem, as text, for [`Mix::validate`].
pub fn validate(mix: &Mix) -> Result<(), String> {
    match issues(mix).first() {
        Some(i) => Err(i.to_string()),
        None => Ok(()),
    }
}

/// Every problem of the logic graphs and macros of `mix`.
pub fn issues(mix: &Mix) -> Vec<Issue> {
    let mut out = Vec::new();
    if mix.logic.len() > MAX_GRAPHS {
        out.push(At::Top.issue(
            None,
            None,
            None,
            format!("at most {MAX_GRAPHS} logic graphs"),
        ));
    }
    if mix.logic_macros.len() > MAX_MACROS {
        out.push(At::Top.issue(
            None,
            None,
            None,
            format!("at most {MAX_MACROS} logic macros"),
        ));
    }
    let macros = &mix.logic_macros;
    let depths = depths(macros);
    let mut bad_macros = BTreeSet::new();
    let mut ids = BTreeSet::new();
    for m in macros {
        let at = At::Macro(&m.id);
        let before = out.len();
        if let Err(e) = check_id("logic macro", &m.id) {
            out.push(at.issue(None, None, None, e));
        }
        if !ids.insert(m.id.as_str()) {
            out.push(at.issue(None, None, None, format!("macro id {} is used twice", m.id)));
        }
        if let Err(e) = check_name(&format!("logic macro {}", m.id), &m.name) {
            out.push(at.issue(None, None, None, e));
        }
        check_level(mix, at, &m.blocks, &m.wires, true, &mut out);
        if m.description.chars().count() > MAX_DESCRIPTION {
            out.push(at.issue(
                None,
                None,
                None,
                format!("a description has at most {MAX_DESCRIPTION} characters"),
            ));
        }
        if let Some((block, message)) = recursion(&depths, m) {
            out.push(at.issue(Some(&block), None, None, message));
        }
        if out.len() > before {
            bad_macros.insert(m.id.as_str());
        }
    }
    let bad_macros = spoiled(macros, &bad_macros);
    // A macro on its own: loops inside it, and its size once inlined.
    for m in macros {
        if bad_macros.contains(m.id.as_str()) {
            continue;
        }
        if let Some(i) = check_inlined(At::Macro(&m.id), &m.blocks, &m.wires, macros) {
            out.push(i);
        }
    }
    let mut ids = BTreeSet::new();
    for g in &mix.logic {
        let at = At::Graph(&g.id);
        let before = out.len();
        if let Err(e) = check_id("logic", &g.id) {
            out.push(at.issue(None, None, None, e));
        }
        if !ids.insert(g.id.as_str()) {
            out.push(at.issue(None, None, None, format!("logic id {} is used twice", g.id)));
        }
        if let Err(e) = check_name(&format!("logic {}", g.id), &g.name) {
            out.push(at.issue(None, None, None, e));
        }
        if g.description.chars().count() > MAX_DESCRIPTION {
            out.push(at.issue(
                None,
                None,
                None,
                format!("a description has at most {MAX_DESCRIPTION} characters"),
            ));
        }
        check_level(mix, at, &g.blocks, &g.wires, false, &mut out);
        if out.len() == before && !uses_bad(macros, &g.blocks, &bad_macros) {
            if let Some(i) = check_inlined(at, &g.blocks, &g.wires, macros) {
                out.push(i);
            }
        }
    }
    out
}

/// Every macro's depth (itself and the macros inside it), or why it has none:
/// it uses itself. Computed once per validation, each macro once (D169).
fn depths(macros: &[Macro]) -> BTreeMap<&str, Result<usize, String>> {
    fn visit<'a>(
        id: &'a str,
        macros: &'a [Macro],
        memo: &mut BTreeMap<&'a str, Result<usize, String>>,
        stack: &mut Vec<&'a str>,
    ) -> Result<usize, String> {
        if let Some(r) = memo.get(id) {
            return r.clone();
        }
        if let Some(start) = stack.iter().position(|s| *s == id) {
            return Err(format!(
                "macro {id} uses itself ({} → {id})",
                stack[start..].join(" → ")
            ));
        }
        let Some(m) = macros.iter().find(|m| m.id == id) else {
            return Ok(0);
        };
        stack.push(id);
        let mut result = Ok(0);
        for b in &m.blocks {
            if let BlockKind::Macro { macro_id } = &b.kind {
                match visit(macro_id, macros, memo, stack) {
                    Ok(d) => result = result.map(|r: usize| r.max(d)),
                    Err(e) => {
                        result = Err(e);
                        break;
                    }
                }
            }
        }
        stack.pop();
        let r = result.map(|d| d + 1);
        memo.insert(id, r.clone());
        r
    }
    let mut memo = BTreeMap::new();
    for m in macros {
        let _ = visit(&m.id, macros, &mut memo, &mut Vec::new());
    }
    memo
}

/// A macro that uses itself (directly or through others), or nests deeper than
/// [`MAX_DEPTH`]: the instance block of `m` where it starts, and why.
fn recursion(
    depths: &BTreeMap<&str, Result<usize, String>>,
    m: &Macro,
) -> Option<(String, String)> {
    for b in &m.blocks {
        let BlockKind::Macro { macro_id } = &b.kind else {
            continue;
        };
        match depths.get(macro_id.as_str()) {
            Some(Err(e)) => return Some((b.id.clone(), e.clone())),
            Some(Ok(d)) if d + 1 > MAX_DEPTH => {
                return Some((
                    b.id.clone(),
                    format!("macros nest at most {MAX_DEPTH} deep"),
                ))
            }
            _ => {}
        }
    }
    None
}

/// The macros that are bad or use a bad or missing macro, at any depth: a
/// fixed point over the macros, not a walk of every instance (D169).
fn spoiled<'a>(macros: &'a [Macro], bad: &BTreeSet<&'a str>) -> BTreeSet<&'a str> {
    let mut out = bad.clone();
    loop {
        let before = out.len();
        for m in macros {
            if !out.contains(m.id.as_str()) && uses_bad(macros, &m.blocks, &out) {
                out.insert(&m.id);
            }
        }
        if out.len() == before {
            return out;
        }
    }
}

/// `blocks` use a macro in `bad` or a missing one (directly; see [`spoiled`]).
fn uses_bad(macros: &[Macro], blocks: &[Block], bad: &BTreeSet<&str>) -> bool {
    blocks.iter().any(|b| match &b.kind {
        BlockKind::Macro { macro_id } => {
            bad.contains(macro_id.as_str()) || !macros.iter().any(|m| &m.id == macro_id)
        }
        _ => false,
    })
}

/// Inline and check loops and size; the issue names the top-level block.
fn check_inlined(at: At<'_>, blocks: &[Block], wires: &[Wire], macros: &[Macro]) -> Option<Issue> {
    let flat = match inline(blocks, wires, macros) {
        Ok(f) => f,
        Err(e) => return Some(at.issue(None, None, None, e)),
    };
    flat.order().err().map(|i| {
        at.issue(
            Some(&flat.blocks[i].top),
            None,
            None,
            "a loop needs an edge, a latch, a toggle, a counter, a timer, a delay or a debounce in it"
                .into(),
        )
    })
}

/// The blocks and wires of one graph or macro (not inlined).
fn check_level(
    mix: &Mix,
    at: At<'_>,
    blocks: &[Block],
    wires: &[Wire],
    in_macro: bool,
    out: &mut Vec<Issue>,
) {
    if blocks.len() > MAX_BLOCKS {
        out.push(at.issue(None, None, None, format!("at most {MAX_BLOCKS} blocks")));
    }
    let mut ids = BTreeSet::new();
    let mut names_in = BTreeSet::new();
    let mut names_out = BTreeSet::new();
    let mut ports: BTreeMap<&str, Ports> = BTreeMap::new();
    for b in blocks {
        if let Err(e) = check_id("logic block", &b.id) {
            out.push(at.issue(Some(&b.id), None, None, e));
        }
        if !ids.insert(b.id.as_str()) {
            out.push(at.issue(Some(&b.id), None, None, format!("block {} twice", b.id)));
        }
        if let Err((port, e)) = check_block(mix, &b.kind, in_macro) {
            out.push(at.issue(Some(&b.id), port, None, e));
        }
        let dup = match &b.kind {
            BlockKind::MacroIn { name, .. } => !names_in.insert(name.as_str()),
            BlockKind::MacroOut { name, .. } => !names_out.insert(name.as_str()),
            _ => false,
        };
        if dup {
            out.push(at.issue(
                Some(&b.id),
                None,
                None,
                "two exposed ports of one direction have this name".into(),
            ));
        }
        ports.insert(&b.id, b.kind.ports(&mix.logic_macros));
    }
    let mut taken = BTreeSet::new();
    for (i, w) in wires.iter().enumerate() {
        let to_port = Some(w.to_port.as_str());
        let Some(from) = ports.get(w.from_block.as_str()) else {
            out.push(at.issue(
                Some(&w.to_block),
                to_port,
                Some(i),
                format!("no block {}", w.from_block),
            ));
            continue;
        };
        let Some(to) = ports.get(w.to_block.as_str()) else {
            out.push(at.issue(
                Some(&w.from_block),
                Some(&w.from_port),
                Some(i),
                format!("no block {}", w.to_block),
            ));
            continue;
        };
        let Some(source) = from.outputs.iter().find(|p| p.name == w.from_port) else {
            out.push(at.issue(
                Some(&w.from_block),
                Some(&w.from_port),
                Some(i),
                format!("{} has no output {}", w.from_block, w.from_port),
            ));
            continue;
        };
        let Some(target) = to.inputs.iter().find(|p| p.name == w.to_port) else {
            out.push(at.issue(
                Some(&w.to_block),
                to_port,
                Some(i),
                format!("{} has no input {}", w.to_block, w.to_port),
            ));
            continue;
        };
        if !target.ty.accepts(source.ty) {
            out.push(at.issue(
                Some(&w.to_block),
                to_port,
                Some(i),
                format!(
                    "a {} output cannot feed a {} input",
                    source.ty.name(),
                    target.ty.name()
                ),
            ));
        }
        if !taken.insert((w.to_block.as_str(), w.to_port.as_str())) {
            out.push(at.issue(
                Some(&w.to_block),
                to_port,
                Some(i),
                format!("{}.{} has two wires", w.to_block, w.to_port),
            ));
        }
    }
}

fn range(what: &str, v: f64, lo: f64, hi: f64) -> Result<(), String> {
    if v.is_finite() && (lo..=hi).contains(&v) {
        Ok(())
    } else {
        Err(format!("{what} must be {lo} to {hi}"))
    }
}

fn text(what: &str, s: &str) -> Result<(), String> {
    if s.chars().count() > MAX_TEXT {
        Err(format!("{what} has at most {MAX_TEXT} characters"))
    } else {
        Ok(())
    }
}

type BlockCheck = Result<(), (Option<&'static str>, String)>;

/// A block's parameters against the mix; the error may name a port.
fn check_block(mix: &Mix, kind: &BlockKind, in_macro: bool) -> BlockCheck {
    let plain = |r: Result<(), String>| r.map_err(|e| (None, e));
    if let Some(s) = kind.signal() {
        let vca_ok = matches!(
            kind,
            BlockKind::Pfl { .. } | BlockKind::SetPfl { .. } | BlockKind::SetVca { .. }
        );
        let found = if matches!(kind, BlockKind::SetVca { .. }) {
            mix.vca(s).is_some()
        } else {
            mix.kind(s).is_some() || (vca_ok && mix.vca(s).is_some())
        };
        if !found {
            return Err((
                None,
                if matches!(kind, BlockKind::SetVca { .. }) {
                    format!("VCA {s} does not exist")
                } else {
                    format!("{s} is not a strip or bus")
                },
            ));
        }
    }
    match kind {
        BlockKind::Const { value } if !value.is_finite() => {
            Err((None, "value is not a finite number".into()))
        }
        BlockKind::Text { value } => plain(text("value", value)),
        BlockKind::Timer { ms } | BlockKind::Delay { ms } | BlockKind::Debounce { ms } => {
            plain(range("ms", *ms, 0.0, MAX_MS))
        }
        BlockKind::Schedule {
            start_minute,
            end_minute,
        } if *start_minute >= 1440 || *end_minute >= 1440 => {
            Err((None, "minutes must be 0 to 1439".into()))
        }
        BlockKind::Webhook { url } => {
            if !(url.starts_with("https://") || url.starts_with("http://")) || url.len() > 2000 {
                Err((None, "url must be an http(s) URL".into()))
            } else {
                Ok(())
            }
        }
        BlockKind::Above {
            threshold_db,
            hysteresis_db,
            ..
        } => {
            plain(range("threshold_db", *threshold_db, -120.0, 20.0))?;
            plain(range("hysteresis_db", *hysteresis_db, 0.0, 60.0))
        }
        BlockKind::Recall { snapshot } => plain(check_id("snapshot", snapshot)),
        BlockKind::NmosReceiver { input } if !(1..=1024).contains(input) => {
            Err((None, "input must be 1 to 1024".into()))
        }
        BlockKind::Link { peer } => {
            if peer == "master" || matches!(peer.as_str(), "s1" | "s2" | "s3" | "s4") {
                Ok(())
            } else {
                Err((None, "peer must be master or s1 to s4".into()))
            }
        }
        BlockKind::Scale { factor, offset } if !factor.is_finite() || !offset.is_finite() => {
            Err((None, "factor and offset must be finite numbers".into()))
        }
        BlockKind::Map { points } => {
            if points.len() < 2 || points.len() > MAX_POINTS {
                return Err((None, format!("a map has 2 to {MAX_POINTS} points")));
            }
            if points.iter().flatten().any(|v| !v.is_finite()) {
                return Err((None, "points must be finite numbers".into()));
            }
            if points.windows(2).any(|w| w[1][0] <= w[0][0]) {
                return Err((None, "the points' x must rise".into()));
            }
            Ok(())
        }
        BlockKind::Select { inputs, value_type } => {
            if !(2..=MAX_SELECT).contains(inputs) {
                Err((None, format!("a select has 2 to {MAX_SELECT} inputs")))
            } else if *value_type == PortType::Event {
                Err((None, "a select cannot pass events".into()))
            } else {
                Ok(())
            }
        }
        BlockKind::SetFader { ramp_ms, .. } | BlockKind::SetVca { ramp_ms, .. } => {
            plain(range("ramp_ms", *ramp_ms, 0.0, MAX_RAMP_MS))
        }
        BlockKind::SetParam {
            signal,
            node,
            param,
            ramp_ms,
        } => {
            plain(range("ramp_ms", *ramp_ms, 0.0, MAX_RAMP_MS))?;
            let chain = mix
                .strip(signal)
                .map(|s| &s.chain)
                .or_else(|| mix.bus(signal).map(|b| &b.chain))
                .ok_or((None, format!("{signal} is not a strip or bus")))?;
            let p = chain
                .iter()
                .find(|p| &p.id == node)
                .ok_or((None, format!("{signal} has no node {node}")))?;
            plain(check_param(p, param))
        }
        BlockKind::SetTalk { mic, destination } => {
            let t = &mix.control_room.talkback;
            if !t.mics.iter().any(|m| &m.id == mic) {
                Err((None, format!("talkback mic {mic} does not exist")))
            } else if !t.destinations.iter().any(|d| &d.id == destination) {
                Err((
                    None,
                    format!("talkback destination {destination} does not exist"),
                ))
            } else {
                Ok(())
            }
        }
        BlockKind::SetMonitor { monitor, source } => {
            if !mix.control_room.monitors.iter().any(|m| &m.id == monitor) {
                return Err((None, format!("monitor {monitor} does not exist")));
            }
            match source {
                Some(Source::Signal(s)) if mix.kind(s).is_none() => {
                    Err((None, format!("{s} is not a strip or bus")))
                }
                Some(Source::Input(0)) => Err((None, "inputs count from 1".into())),
                _ => Ok(()),
            }
        }
        BlockKind::TslOut { index } if *index > 65534 => {
            Err((None, "index must be 0 to 65534".into()))
        }
        BlockKind::Log { message } => plain(text("message", message)),
        BlockKind::Macro { macro_id } => {
            if mix.logic_macro(macro_id).is_none() {
                Err((None, format!("macro {macro_id} does not exist")))
            } else {
                Ok(())
            }
        }
        BlockKind::MacroIn { name, .. } | BlockKind::MacroOut { name, .. } => {
            if !in_macro {
                Err((None, "exposed ports belong in a macro".into()))
            } else {
                plain(check_id("port", name))
            }
        }
        _ => Ok(()),
    }
}

fn param_pointer(param: &str) -> Result<String, String> {
    let ok = !param.is_empty()
        && param.len() <= 64
        && param.split('/').all(|s| {
            !s.is_empty()
                && s.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        })
        && !matches!(param, "id" | "type");
    if ok {
        Ok(format!("/{param}"))
    } else {
        Err(format!("{param:?} is not a node parameter"))
    }
}

/// The node with `param` set to `value` (not range-checked).
pub fn with_param(p: &Proc, param: &str, value: f64) -> Result<Proc, String> {
    let mut v = serde_json::to_value(p).map_err(|e| e.to_string())?;
    let slot = v
        .pointer_mut(&param_pointer(param)?)
        .ok_or_else(|| format!("node {} has no parameter {param}", p.id))?;
    if param == "bypass" {
        *slot = serde_json::Value::Bool(value >= 0.5);
    } else {
        if !slot.is_number() {
            return Err(format!("{param} is not a number"));
        }
        *slot = serde_json::json!(value);
    }
    serde_json::from_value(v).map_err(|e| e.to_string())
}

/// `param` of node `p` is a number (or `bypass`) the DSP smooths: changing it
/// does not change the node's structure.
pub fn check_param(p: &Proc, param: &str) -> Result<(), String> {
    if param == "bypass" {
        return Ok(());
    }
    let v = serde_json::to_value(p).map_err(|e| e.to_string())?;
    let current = v
        .pointer(&param_pointer(param)?)
        .ok_or_else(|| format!("node {} has no parameter {param}", p.id))?
        .as_f64()
        .ok_or_else(|| format!("{param} is not a number"))?;
    // Small and large steps, relative and absolute: a delay of 0 ms or 20
    // samples moves its structure only by a whole sample or more.
    let steps = [
        (current.abs() * 0.01).max(0.01),
        (current.abs() * 0.5).max(1.0),
        (current.abs() * 2.0).max(10.0),
        1000.0,
    ];
    for d in steps {
        for candidate in [current + d, current - d] {
            let Ok(q) = with_param(p, param, candidate) else {
                continue;
            };
            if q.kind.validate().is_ok() && q.kind.signature() != p.kind.signature() {
                return Err(format!(
                    "{param} changes the node's structure; logic sets only smoothed parameters"
                ));
            }
        }
    }
    Ok(())
}

/// Set `param` of `p` to `value`, or as close to it as the parameter's range
/// allows (a bisection from the current value). Structural changes are refused.
pub fn set_param_clamped(p: &mut Proc, param: &str, value: f64) -> Result<(), String> {
    if value.is_nan() {
        // Not a number: no change (D170).
        return Ok(());
    }
    if param == "bypass" {
        p.bypass = value >= 0.5;
        return Ok(());
    }
    let sig = p.kind.signature();
    let fits = |v: f64| {
        with_param(p, param, v)
            .ok()
            .filter(|q| q.kind.validate().is_ok() && q.kind.signature() == sig)
    };
    if value.is_finite() {
        if let Some(q) = fits(value) {
            *p = q;
            return Ok(());
        }
    }
    let v = serde_json::to_value(&*p).map_err(|e| e.to_string())?;
    let current = v
        .pointer(&param_pointer(param)?)
        .and_then(|x| x.as_f64())
        .ok_or_else(|| format!("node {} has no number {param}", p.id))?;
    let (mut good, mut bad) = (current, value.clamp(-1e12, 1e12));
    let mut best = None;
    for _ in 0..48 {
        let mid = (good + bad) / 2.0;
        if let Some(q) = fits(mid) {
            best = Some(q);
            good = mid;
        } else {
            bad = mid;
        }
    }
    if let Some(q) = best {
        *p = q;
    }
    Ok(())
}

/// A block of an inlined graph.
#[derive(Debug, Clone, PartialEq)]
pub struct FlatBlock {
    /// `<instance>.<block>` inside macros.
    pub id: String,
    /// The graph's own block it came from (the instance, for a block in a macro).
    pub top: String,
    /// `macro-in` and `macro-out` pass their input through.
    pub kind: BlockKind,
    pub ports: Ports,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlatWire {
    pub from: usize,
    pub to: usize,
    /// Input index of `to`.
    pub to_port: usize,
}

/// A graph with every macro instance inlined: every block has at most one
/// output.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Flat {
    pub blocks: Vec<FlatBlock>,
    pub wires: Vec<FlatWire>,
    /// Macro instances visited, for the [`MAX_EXPANDED`] budget.
    instances: usize,
}

fn pass_ports(kind: &BlockKind) -> Option<Ports> {
    match kind {
        BlockKind::MacroIn { value_type, .. } | BlockKind::MacroOut { value_type, .. } => {
            Some(Ports {
                inputs: vec![port("in", *value_type)],
                outputs: vec![port("out", *value_type)],
            })
        }
        _ => None,
    }
}

/// Inline every macro instance of `blocks` (D163).
pub fn inline(blocks: &[Block], wires: &[Wire], macros: &[Macro]) -> Result<Flat, String> {
    let mut flat = Flat::default();
    expand("", None, blocks, wires, macros, 1, &mut flat)?;
    Ok(flat)
}

/// The exposed inputs and outputs of one level: name → flat block.
type Exposed = (BTreeMap<String, usize>, BTreeMap<String, usize>);

fn expand(
    prefix: &str,
    top: Option<&str>,
    blocks: &[Block],
    wires: &[Wire],
    macros: &[Macro],
    depth: usize,
    flat: &mut Flat,
) -> Result<Exposed, String> {
    enum End {
        Block(usize),
        Instance(Exposed),
    }
    let mut ends: BTreeMap<&str, End> = BTreeMap::new();
    let mut exposed: Exposed = Default::default();
    for b in blocks {
        let id = if prefix.is_empty() {
            b.id.clone()
        } else {
            format!("{prefix}.{}", b.id)
        };
        let top_id = top.unwrap_or(&b.id);
        if let BlockKind::Macro { macro_id } = &b.kind {
            if depth > MAX_DEPTH {
                return Err(format!("macros nest at most {MAX_DEPTH} deep"));
            }
            flat.instances += 1;
            if flat.instances + flat.blocks.len() > MAX_EXPANDED {
                return Err(format!(
                    "at most {MAX_EXPANDED} blocks and macro instances with the macros inlined"
                ));
            }
            let m = macros
                .iter()
                .find(|m| &m.id == macro_id)
                .ok_or_else(|| format!("macro {macro_id} does not exist"))?;
            let inner = expand(
                &id,
                Some(top_id),
                &m.blocks,
                &m.wires,
                macros,
                depth + 1,
                flat,
            )?;
            ends.insert(&b.id, End::Instance(inner));
            continue;
        }
        let ports = pass_ports(&b.kind).unwrap_or_else(|| b.kind.ports(macros));
        let index = flat.blocks.len();
        match &b.kind {
            BlockKind::MacroIn { name, .. } => {
                exposed.0.insert(name.clone(), index);
            }
            BlockKind::MacroOut { name, .. } => {
                exposed.1.insert(name.clone(), index);
            }
            _ => {}
        }
        flat.blocks.push(FlatBlock {
            id,
            top: top_id.to_string(),
            kind: b.kind.clone(),
            ports,
        });
        if flat.blocks.len() > MAX_INLINED {
            return Err(format!(
                "at most {MAX_INLINED} blocks with the macros inlined"
            ));
        }
        ends.insert(&b.id, End::Block(index));
    }
    for w in wires {
        let missing = || format!("a wire names a missing block or port ({w:?})");
        let from = match ends.get(w.from_block.as_str()).ok_or_else(missing)? {
            End::Block(i) => *i,
            End::Instance((_, outs)) => *outs.get(&w.from_port).ok_or_else(missing)?,
        };
        let (to, to_port) = match ends.get(w.to_block.as_str()).ok_or_else(missing)? {
            End::Block(i) => (
                *i,
                flat.blocks[*i]
                    .ports
                    .input(&w.to_port)
                    .ok_or_else(missing)?,
            ),
            End::Instance((ins, _)) => (*ins.get(&w.to_port).ok_or_else(missing)?, 0),
        };
        flat.wires.push(FlatWire { from, to, to_port });
    }
    Ok(exposed)
}

impl Flat {
    /// Evaluation order (D160): a block after the blocks it reads, except that
    /// a block that remembers is read as last tick's value. Ties go by block
    /// order. `Err` names a block on a combinational loop.
    pub fn order(&self) -> Result<Vec<usize>, usize> {
        let n = self.blocks.len();
        let mut incoming = vec![0usize; n];
        let mut next: Vec<Vec<usize>> = vec![Vec::new(); n];
        let mut prev: Vec<Vec<usize>> = vec![Vec::new(); n];
        for w in &self.wires {
            if self.blocks[w.to].kind.remembers() {
                continue;
            }
            incoming[w.to] += 1;
            next[w.from].push(w.to);
            prev[w.to].push(w.from);
        }
        let mut ready: BTreeSet<usize> = (0..n).filter(|i| incoming[*i] == 0).collect();
        let mut order = Vec::with_capacity(n);
        while let Some(i) = ready.pop_first() {
            order.push(i);
            for &j in &next[i] {
                incoming[j] -= 1;
                if incoming[j] == 0 {
                    ready.insert(j);
                }
            }
        }
        if order.len() == n {
            return Ok(order);
        }
        // Walk back through blocks left over until one repeats: it is on a loop.
        let mut seen = BTreeSet::new();
        let mut i = (0..n).find(|i| incoming[*i] > 0).unwrap_or(0);
        while seen.insert(i) {
            match prev[i].iter().find(|p| incoming[**p] > 0) {
                Some(p) => i = *p,
                None => break,
            }
        }
        Err(i)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::mix::model::tests::strip;
    use crate::mix::Format;

    pub fn graph(blocks: Vec<Block>, wires: Vec<Wire>) -> Graph {
        Graph {
            id: "g".into(),
            name: "G".into(),
            description: String::new(),
            enabled: true,
            dry_run: false,
            blocks,
            wires,
        }
    }

    pub fn block(id: &str, kind: BlockKind) -> Block {
        Block {
            id: id.into(),
            kind,
            x: 0.0,
            y: 0.0,
        }
    }

    pub fn wire(from: &str, from_port: &str, to: &str, to_port: &str) -> Wire {
        Wire {
            from_block: from.into(),
            from_port: from_port.into(),
            to_block: to.into(),
            to_port: to_port.into(),
        }
    }

    fn mix_with(blocks: Vec<Block>, wires: Vec<Wire>) -> Mix {
        let mut m = Mix::default();
        m.strips.push(strip("vox", Format::Mono));
        m.logic.push(graph(blocks, wires));
        m
    }

    #[test]
    fn a_loop_needs_memory() {
        let blocks = vec![block("a", BlockKind::And), block("b", BlockKind::Or)];
        let wires = vec![wire("a", "out", "b", "a"), wire("b", "out", "a", "a")];
        let issues = issues(&mix_with(blocks.clone(), wires));
        assert_eq!(issues.len(), 1);
        assert!(issues[0].message.contains("loop"), "{issues:?}");
        assert!(issues[0].block.is_some());
        let mut with = blocks.clone();
        with.push(block("e", BlockKind::Edge));
        let through = vec![
            wire("a", "out", "e", "in"),
            wire("e", "out", "b", "a"),
            wire("b", "out", "a", "a"),
        ];
        assert_eq!(issues_of(with, through), vec![]);
        // A toggle and a counter remember too.
        let toggled = vec![block("t", BlockKind::Toggle), block("n", BlockKind::Not)];
        assert_eq!(
            issues_of(
                toggled,
                vec![wire("t", "out", "n", "in"), wire("n", "out", "t", "in")]
            ),
            vec![]
        );
    }

    fn issues_of(blocks: Vec<Block>, wires: Vec<Wire>) -> Vec<Issue> {
        issues(&mix_with(blocks, wires))
    }

    #[test]
    fn validation() {
        let mut m = Mix::default();
        m.strips.push(strip("vox", Format::Mono));
        m.logic.push(graph(
            vec![
                block(
                    "v",
                    BlockKind::Vad {
                        signal: "vox".into(),
                    },
                ),
                block(
                    "m",
                    BlockKind::SetMute {
                        signal: "vox".into(),
                    },
                ),
            ],
            vec![wire("v", "out", "m", "in")],
        ));
        assert_eq!(validate(&m), Ok(()));
        assert_eq!(m.logic_vad_signals().collect::<Vec<_>>(), ["vox"]);
        m.remove_signal("vox");
        assert!(m.logic[0].blocks.is_empty());
        let bad = |kind: BlockKind| !issues_of(vec![block("x", kind)], vec![]).is_empty();
        assert!(bad(BlockKind::Mute {
            signal: "nope".into()
        }));
        assert!(bad(BlockKind::Timer { ms: -1.0 }));
        assert!(bad(BlockKind::Above {
            signal: "vox".into(),
            threshold_db: 40.0,
            hysteresis_db: 6.0,
        }));
        assert!(bad(BlockKind::Recall {
            snapshot: "Show".into()
        }));
        assert!(bad(BlockKind::Const { value: f64::NAN }));
        assert!(bad(BlockKind::Text {
            value: "x".repeat(201)
        }));
        assert!(bad(BlockKind::Link { peer: "s5".into() }));
        assert!(!bad(BlockKind::Link { peer: "s4".into() }));
        assert!(bad(BlockKind::NmosReceiver { input: 0 }));
        assert!(bad(BlockKind::Map {
            points: vec![[0.0, 0.0]]
        }));
        assert!(bad(BlockKind::Map {
            points: vec![[1.0, 0.0], [0.0, 1.0]]
        }));
        assert!(!bad(BlockKind::Map {
            points: vec![[0.0, 0.0], [1.0, 1.0]]
        }));
        assert!(bad(BlockKind::Select {
            inputs: 9,
            value_type: PortType::Number
        }));
        assert!(bad(BlockKind::Select {
            inputs: 2,
            value_type: PortType::Event
        }));
        assert!(bad(BlockKind::SetVca {
            vca: "nope".into(),
            ramp_ms: 10.0
        }));
        assert!(bad(BlockKind::SetFader {
            signal: "vox".into(),
            ramp_ms: 20_000.0
        }));
        assert!(bad(BlockKind::SetTalk {
            mic: "m".into(),
            destination: "d".into()
        }));
        assert!(bad(BlockKind::SetMonitor {
            monitor: "nope".into(),
            source: None
        }));
        assert!(bad(BlockKind::TslOut { index: 65535 }));
        assert!(bad(BlockKind::Macro {
            macro_id: "nope".into()
        }));
        assert!(bad(BlockKind::MacroIn {
            name: "a".into(),
            value_type: PortType::Number
        }));
        assert!(!bad(BlockKind::Pfl {
            signal: "vox".into()
        }));
        assert!(!bad(BlockKind::Loudness {
            signal: "vox".into(),
            measure: Measure::Integrated
        }));
        // A timer in a loop is fine.
        let timed = vec![
            block("t", BlockKind::Timer { ms: 10.0 }),
            block("n", BlockKind::Not),
        ];
        assert_eq!(
            issues_of(
                timed,
                vec![wire("t", "out", "n", "in"), wire("n", "out", "t", "in")]
            ),
            vec![]
        );
    }

    #[test]
    fn wires_are_type_checked_with_the_port_named() {
        let blocks = vec![
            block("txt", BlockKind::Text { value: "hi".into() }),
            block("e", BlockKind::Edge),
            block("not", BlockKind::Not),
            block(
                "log",
                BlockKind::Log {
                    message: "m".into(),
                },
            ),
            block(
                "lvl",
                BlockKind::Level {
                    signal: "vox".into(),
                },
            ),
            block(
                "r",
                BlockKind::Recall {
                    snapshot: "s".into(),
                },
            ),
        ];
        // Strings feed only strings; events do not feed strings.
        let i = issues_of(blocks.clone(), vec![wire("txt", "out", "not", "in")]);
        assert_eq!(i.len(), 1);
        assert_eq!(i[0].block.as_deref(), Some("not"));
        assert_eq!(i[0].port.as_deref(), Some("in"));
        assert_eq!(i[0].wire, Some(0));
        assert_eq!(i[0].graph.as_deref(), Some("g"));
        assert!(i[0].message.contains("string output cannot feed a boolean"));
        assert_eq!(
            issues_of(blocks.clone(), vec![wire("e", "out", "log", "value")]).len(),
            1
        );
        // The conversions D158 allows.
        assert_eq!(
            issues_of(
                blocks.clone(),
                vec![
                    wire("lvl", "out", "not", "in"),
                    wire("lvl", "out", "r", "in"),
                    wire("e", "out", "log", "in"),
                    wire("not", "out", "log", "value"),
                ]
            ),
            vec![]
        );
        // Every issue is listed, not only the first.
        let i = issues_of(
            blocks,
            vec![
                wire("txt", "out", "not", "in"),
                wire("nope", "out", "log", "in"),
                wire("e", "nope", "r", "in"),
            ],
        );
        assert_eq!(i.len(), 3, "{i:?}");
        assert_eq!(
            i[0].to_string(),
            "logic g: block not: port in: a string output cannot feed a boolean input"
        );
    }

    #[test]
    fn port_types_accept() {
        use PortType::*;
        for from in [Boolean, Number, Level, Event, String] {
            for to in [Boolean, Number, Level, Event, String] {
                let ok = to.accepts(from);
                let want = from == to || (from != String && !(from == Event && to == String));
                assert_eq!(ok, want, "{from:?} → {to:?}");
            }
        }
    }

    fn duck_macro() -> Macro {
        // in (boolean) → not → out (boolean), and a latch kept per instance.
        Macro {
            id: "inv".into(),
            name: "Invert".into(),
            description: String::new(),
            blocks: vec![
                block(
                    "i",
                    BlockKind::MacroIn {
                        name: "x".into(),
                        value_type: PortType::Boolean,
                    },
                ),
                block("n", BlockKind::Not),
                block(
                    "o",
                    BlockKind::MacroOut {
                        name: "y".into(),
                        value_type: PortType::Boolean,
                    },
                ),
            ],
            wires: vec![wire("i", "out", "n", "in"), wire("n", "out", "o", "in")],
        }
    }

    #[test]
    fn macros_inline_with_their_ports() {
        let mut m = mix_with(
            vec![
                block("b", BlockKind::Button),
                block(
                    "m1",
                    BlockKind::Macro {
                        macro_id: "inv".into(),
                    },
                ),
                block(
                    "set",
                    BlockKind::SetMute {
                        signal: "vox".into(),
                    },
                ),
            ],
            vec![wire("b", "out", "m1", "x"), wire("m1", "y", "set", "in")],
        );
        m.logic_macros.push(duck_macro());
        assert_eq!(issues(&m), vec![]);
        let ports = BlockKind::Macro {
            macro_id: "inv".into(),
        }
        .ports(&m.logic_macros);
        assert_eq!(ports.inputs, vec![port("x", PortType::Boolean)]);
        assert_eq!(ports.outputs, vec![port("y", PortType::Boolean)]);
        let flat = inline(&m.logic[0].blocks, &m.logic[0].wires, &m.logic_macros).unwrap();
        let ids: Vec<&str> = flat.blocks.iter().map(|b| b.id.as_str()).collect();
        assert_eq!(ids, ["b", "m1.i", "m1.n", "m1.o", "set"]);
        assert!(flat.blocks[1..4].iter().all(|b| b.top == "m1"));
        assert_eq!(flat.wires.len(), 4);
        let order = flat.order().unwrap();
        let pos = |i| order.iter().position(|x| *x == i).unwrap();
        assert!(pos(0) < pos(1) && pos(1) < pos(2) && pos(2) < pos(3) && pos(3) < pos(4));
        assert_eq!(m.logic_block_at("g", "m1.n"), Some(&BlockKind::Not));
        assert_eq!(m.logic_block_at("g", "b"), Some(&BlockKind::Button));
        assert_eq!(m.logic_block_at("g", "b.n"), None);
        // A wire into the instance is type-checked against the macro's port.
        m.logic[0]
            .blocks
            .push(block("t", BlockKind::Text { value: "x".into() }));
        m.logic[0].wires[0] = wire("t", "out", "m1", "x");
        assert_eq!(issues(&m)[0].block.as_deref(), Some("m1"));
    }

    #[test]
    fn a_loop_through_a_macro_is_found_on_the_inlined_graph() {
        let mut m = mix_with(
            vec![
                block(
                    "m1",
                    BlockKind::Macro {
                        macro_id: "inv".into(),
                    },
                ),
                block("a", BlockKind::And),
            ],
            vec![wire("m1", "y", "a", "a"), wire("a", "out", "m1", "x")],
        );
        m.logic_macros.push(duck_macro());
        let i = issues(&m);
        assert_eq!(i.len(), 1, "{i:?}");
        assert!(i[0].message.contains("loop"));
        assert!(matches!(i[0].block.as_deref(), Some("m1" | "a")));
    }

    #[test]
    fn recursive_and_deep_macros_are_refused() {
        let mut m = Mix::default();
        let user = |id: &str, uses: &str| Macro {
            id: id.into(),
            name: id.into(),
            description: String::new(),
            blocks: vec![block(
                "u",
                BlockKind::Macro {
                    macro_id: uses.into(),
                },
            )],
            wires: vec![],
        };
        m.logic_macros = vec![user("a", "b"), user("b", "a")];
        let i = issues(&m);
        assert!(
            i.iter().any(|i| i.macro_id.as_deref() == Some("a")
                && i.block.as_deref() == Some("u")
                && i.message.contains("uses itself")),
            "{i:?}"
        );
        m.logic_macros = vec![user("a", "a")];
        assert!(issues(&m)[0].message.contains("uses itself"));
        // a → b → c → d is 4 deep: fine; one more is not.
        m.logic_macros = vec![user("a", "b"), user("b", "c"), user("c", "d"), duck_macro()];
        m.logic_macros[2] = user("c", "inv");
        assert_eq!(issues(&m), vec![]);
        m.logic_macros.push(user("z", "a"));
        let i = issues(&m);
        assert!(
            i.iter()
                .any(|i| i.macro_id.as_deref() == Some("z") && i.message.contains("deep")),
            "{i:?}"
        );
    }

    #[test]
    fn the_inlined_size_is_limited() {
        let mut big = duck_macro();
        big.id = "big".into();
        for n in 0..60 {
            big.blocks
                .push(block(&format!("k{n}"), BlockKind::Const { value: 0.0 }));
        }
        let mut m = Mix::default();
        m.logic_macros.push(big);
        m.logic.push(graph(
            (0..5)
                .map(|n| {
                    block(
                        &format!("m{n}"),
                        BlockKind::Macro {
                            macro_id: "big".into(),
                        },
                    )
                })
                .collect(),
            vec![],
        ));
        let i = issues(&m);
        assert!(i[0].message.contains("256"), "{i:?}");
    }

    #[test]
    fn macro_ports_live_only_in_macros_and_have_unique_names() {
        let mut inv = duck_macro();
        inv.blocks.push(block(
            "i2",
            BlockKind::MacroIn {
                name: "x".into(),
                value_type: PortType::Number,
            },
        ));
        let mut m = Mix::default();
        m.logic_macros.push(inv);
        let i = issues(&m);
        assert_eq!(i.len(), 1);
        assert_eq!(i[0].macro_id.as_deref(), Some("inv"));
        assert_eq!(i[0].block.as_deref(), Some("i2"));
    }

    #[test]
    fn set_param_takes_smoothed_parameters_and_clamps() {
        let mut p: Proc = serde_json::from_value(serde_json::json!({
            "id": "comp", "type": "compressor", "threshold_db": -20.0, "ratio": 4.0,
            "knee_db": 6.0, "attack_ms": 10.0, "release_ms": 100.0, "makeup_db": 0.0,
            "detector": "peak"
        }))
        .unwrap();
        assert_eq!(check_param(&p, "threshold_db"), Ok(()));
        assert_eq!(check_param(&p, "bypass"), Ok(()));
        assert!(check_param(&p, "detector").is_err(), "not a number");
        assert!(check_param(&p, "nope").is_err());
        assert!(check_param(&p, "id").is_err());
        set_param_clamped(&mut p, "threshold_db", -30.0).unwrap();
        assert_eq!(serde_json::to_value(&p).unwrap()["threshold_db"], -30.0);
        // Above the range (−100 … 0): as close as it goes.
        set_param_clamped(&mut p, "threshold_db", 12.0).unwrap();
        let t = serde_json::to_value(&p).unwrap()["threshold_db"]
            .as_f64()
            .unwrap();
        assert!((-1e-6..=0.0).contains(&t), "{t}");
        set_param_clamped(&mut p, "bypass", 1.0).unwrap();
        assert!(p.bypass);
        let lim: Proc = serde_json::from_value(serde_json::json!({
            "id": "lim", "type": "limiter", "ceiling_db": -1.0, "release_ms": 50.0,
            "lookahead_ms": 1.0, "true_peak": false
        }))
        .unwrap();
        assert_eq!(check_param(&lim, "ceiling_db"), Ok(()));
        assert!(check_param(&lim, "lookahead_ms")
            .unwrap_err()
            .contains("structure"));
        let eq: Proc = serde_json::from_value(serde_json::json!({
            "id": "eq", "type": "eq", "bands": [{"type": "bell", "freq_hz": 1000, "q": 1, "gain_db": 0}]
        }))
        .unwrap();
        assert_eq!(check_param(&eq, "bands/0/gain_db"), Ok(()));
        assert!(check_param(&eq, "bands/1/gain_db").is_err());
    }

    #[test]
    fn a_1_2_show_loads_and_validates() {
        // Every 1.2.0 block type, wired the way 1.2 allowed (all numbers).
        let show = serde_json::json!({
            "strips": [],
            "logic": [{
                "id": "old", "name": "Old",
                "blocks": [
                    {"id": "c", "type": "const", "value": 1},
                    {"id": "b", "type": "button"},
                    {"id": "t", "type": "tally", "index": 3},
                    {"id": "tm", "type": "timer", "ms": 100},
                    {"id": "and", "type": "and"},
                    {"id": "or", "type": "or"},
                    {"id": "x", "type": "xor"},
                    {"id": "n", "type": "not"},
                    {"id": "cmp", "type": "compare", "op": "gt"},
                    {"id": "e", "type": "edge"},
                    {"id": "l", "type": "latch"},
                    {"id": "d", "type": "delay", "ms": 50},
                    {"id": "db", "type": "debounce", "ms": 50},
                    {"id": "s", "type": "schedule", "start_minute": 0, "end_minute": 60},
                    {"id": "r", "type": "recall", "snapshot": "a"},
                    {"id": "w", "type": "webhook", "url": "http://h/x"}
                ],
                "wires": [
                    {"from_block": "c", "from_port": "out", "to_block": "and", "to_port": "a"},
                    {"from_block": "b", "from_port": "out", "to_block": "and", "to_port": "b"},
                    {"from_block": "and", "from_port": "out", "to_block": "cmp", "to_port": "a"},
                    {"from_block": "cmp", "from_port": "out", "to_block": "e", "to_port": "in"},
                    {"from_block": "e", "from_port": "out", "to_block": "l", "to_port": "set"},
                    {"from_block": "l", "from_port": "out", "to_block": "r", "to_port": "in"},
                    {"from_block": "tm", "from_port": "out", "to_block": "w", "to_port": "in"},
                    {"from_block": "c", "from_port": "out", "to_block": "tm", "to_port": "in"},
                    {"from_block": "t", "from_port": "out", "to_block": "x", "to_port": "a"},
                    {"from_block": "x", "from_port": "out", "to_block": "d", "to_port": "in"},
                    {"from_block": "d", "from_port": "out", "to_block": "db", "to_port": "in"},
                    {"from_block": "db", "from_port": "out", "to_block": "n", "to_port": "in"},
                    {"from_block": "s", "from_port": "out", "to_block": "or", "to_port": "a"},
                    {"from_block": "e", "from_port": "out", "to_block": "or", "to_port": "b"}
                ]
            }]
        });
        let mix: Mix = serde_json::from_value(show).unwrap();
        assert_eq!(issues(&mix), vec![]);
        // Unchanged when written again, except set-fader's ramp default.
        let back: Mix = serde_json::from_value(serde_json::to_value(&mix).unwrap()).unwrap();
        assert_eq!(back, mix);
        assert!(serde_json::to_value(&mix)
            .unwrap()
            .get("logic_macros")
            .is_none());
        let old_fader: BlockKind =
            serde_json::from_value(serde_json::json!({"type": "set-fader", "signal": "vox"}))
                .unwrap();
        assert_eq!(
            old_fader,
            BlockKind::SetFader {
                signal: "vox".into(),
                ramp_ms: 10.0
            }
        );
    }

    #[test]
    fn nested_macros_validate_quickly_within_a_budget() {
        // B4 (D169): 4 levels of 64 instances each, no blocks of their own,
        // would be 64^4 instances; the budget stops the inlining early, and
        // each macro's depth is computed once.
        let level = |id: &str, inner: Option<&str>| Macro {
            id: id.into(),
            name: id.into(),
            description: String::new(),
            blocks: match inner {
                Some(inner) => (0..64)
                    .map(|n| {
                        block(
                            &format!("i{n}"),
                            BlockKind::Macro {
                                macro_id: inner.into(),
                            },
                        )
                    })
                    .collect(),
                None => vec![block("n", BlockKind::Not)],
            },
            wires: vec![],
        };
        let mut m = Mix {
            logic_macros: vec![
                level("a", Some("b")),
                level("b", Some("c")),
                level("c", Some("d")),
                level("d", None),
            ],
            ..Mix::default()
        };
        m.logic.push(graph(
            (0..64)
                .map(|n| {
                    block(
                        &format!("g{n}"),
                        BlockKind::Macro {
                            macro_id: "a".into(),
                        },
                    )
                })
                .collect(),
            vec![],
        ));
        let start = std::time::Instant::now();
        let found = issues(&m);
        assert!(
            start.elapsed() < std::time::Duration::from_secs(1),
            "{:?}",
            start.elapsed()
        );
        assert!(
            found.iter().any(|i| i.message.contains("inlined")),
            "{found:?}"
        );
    }

    #[test]
    fn small_structural_values_are_found() {
        // A delay of 0 ms, 0.5 ms or 20 samples changes its structure by a
        // whole sample or more (X14).
        for (time, unit) in [(0.0, "ms"), (0.5, "ms"), (20.0, "samples")] {
            let p: Proc = serde_json::from_value(serde_json::json!({
                "id": "d", "type": "delay", "time": time, "unit": unit
            }))
            .unwrap();
            assert!(check_param(&p, "time").is_err(), "{time} {unit}");
        }
        let mut p: Proc = serde_json::from_value(serde_json::json!({
            "id": "c", "type": "compressor", "threshold_db": -20.0, "ratio": 4.0,
            "knee_db": 6.0, "attack_ms": 10.0, "release_ms": 100.0, "makeup_db": 0.0,
            "detector": "peak"
        }))
        .unwrap();
        let before = p.clone();
        set_param_clamped(&mut p, "threshold_db", f64::NAN).unwrap();
        assert_eq!(p, before, "NaN changes nothing");
    }

    #[test]
    fn descriptions_are_limited() {
        let mut m = mix_with(vec![], vec![]);
        m.logic[0].description = "x".repeat(MAX_DESCRIPTION + 1);
        assert!(issues(&m)[0].message.contains("description"));
    }
}
