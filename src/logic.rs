//! The logic runtime (§11.2): every 10 ms (100 Hz, the control engine's tick)
//! each enabled graph is evaluated. A graph is compiled once (macros inlined,
//! [`crate::mix::logic::inline`]) into an index-based plan and recompiled when
//! it or a macro changes; the blocks run in topological order (D160). A block
//! that remembers publishes the value it computed last tick, which is what lets
//! a loop through one be deterministic.
//!
//! Value actions (mute, fader, VCA, parameter, PFL, talk, publish, TSL out)
//! follow their input: on the first tick and whenever it changes. Event
//! actions (monitor source, loudness, recall, webhook, log) run when their
//! input fires. An action whose input has no wire never runs. In dry run the
//! actions are returned marked `dry_run` and not executed (D164).

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde_json::{json, Value as Json};

use crate::loudness::LoudnessValues;
use crate::mix::logic::{
    inline, BlockKind, CompareOp, FlatBlock, Graph, LoudnessAction, Macro, MathOp, Measure,
    PortType,
};
use crate::mix::model::Db;
use crate::mix::room::Source;
use crate::mix::Mix;

/// Silence, and a signal with no meter or loudness yet.
pub const FLOOR_DB: f64 = -200.0;
/// The actions a graph keeps for the API.
pub const RECENT: usize = 20;

/// What the blocks read, gathered once per tick.
pub struct Readings<'a> {
    pub mix: &'a Mix,
    pub speech: &'a BTreeMap<String, bool>,
    pub level_db: &'a BTreeMap<String, f64>,
    /// Tally indexes whose left lamp is on. Empty before any tally.
    pub on_air: &'a BTreeSet<u16>,
    /// UTC minute of the day, 0..1440.
    pub minute_of_day: u16,
    /// Signals in PFL.
    pub pfl: &'a BTreeSet<String>,
    pub loudness: &'a BTreeMap<String, LoudnessValues>,
    /// NMOS sender and receiver keys (`in-3`, `out-pgm`) that are active.
    pub nmos: &'a BTreeSet<String>,
    /// Remote-link peers that are up (`master`, `s1` …).
    pub link: &'a BTreeSet<String>,
    /// Unix time, for the action list.
    pub now_ms: u64,
}

static NO_BOOLS: BTreeMap<String, bool> = BTreeMap::new();
static NO_LEVELS: BTreeMap<String, f64> = BTreeMap::new();
static NO_INDEXES: BTreeSet<u16> = BTreeSet::new();
static NO_STRINGS: BTreeSet<String> = BTreeSet::new();
static NO_LOUDNESS: BTreeMap<String, LoudnessValues> = BTreeMap::new();

impl<'a> Readings<'a> {
    /// Nothing measured, nothing on air, midnight.
    pub fn quiet(mix: &'a Mix) -> Self {
        Self {
            mix,
            speech: &NO_BOOLS,
            level_db: &NO_LEVELS,
            on_air: &NO_INDEXES,
            minute_of_day: 0,
            pfl: &NO_STRINGS,
            loudness: &NO_LOUDNESS,
            nmos: &NO_STRINGS,
            link: &NO_STRINGS,
            now_ms: 0,
        }
    }
}

/// A value on a wire.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Bool(bool),
    /// A number or a level.
    Num(f64),
    /// Fired this tick.
    Event(bool),
    Str(String),
}

impl Value {
    fn truthy(&self) -> bool {
        match self {
            Self::Bool(b) | Self::Event(b) => *b,
            Self::Num(x) => *x >= 0.5,
            Self::Str(s) => !s.is_empty(),
        }
    }

    fn num(&self) -> f64 {
        match self {
            Self::Num(x) => *x,
            other => f64::from(u8::from(other.truthy())),
        }
    }

    fn text(&self) -> String {
        match self {
            Self::Str(s) => s.clone(),
            Self::Bool(b) => b.to_string(),
            Self::Num(x) => format_number(*x),
            Self::Event(_) => String::new(),
        }
    }

    fn default_of(ty: PortType) -> Self {
        match ty {
            PortType::Boolean => Self::Bool(false),
            PortType::Number | PortType::Level => Self::Num(0.0),
            PortType::Event => Self::Event(false),
            PortType::String => Self::Str(String::new()),
        }
    }
}

/// A number as text: at most two decimals, `inf` and `-inf`.
pub fn format_number(x: f64) -> String {
    if x.is_nan() {
        return "nan".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "inf".into() } else { "-inf".into() };
    }
    let s = format!("{x:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" {
        "0".into()
    } else {
        s.to_string()
    }
}

/// Convert at an input (D158). `prev` is the input's last on/off, for a value
/// that becomes an event (it does not fire on the first tick seen).
fn convert(v: &Value, want: PortType, prev: &mut Option<bool>) -> Value {
    match want {
        PortType::Boolean => Value::Bool(v.truthy()),
        PortType::Number | PortType::Level => Value::Num(v.num()),
        PortType::String => Value::Str(v.text()),
        PortType::Event => match v {
            Value::Event(f) => Value::Event(*f),
            other => {
                let cur = other.truthy();
                let fired = *prev == Some(false) && cur;
                *prev = Some(cur);
                Value::Event(fired)
            }
        },
    }
}

/// What a logic action asks for.
#[derive(Debug, Clone, PartialEq)]
pub enum Act {
    Mute {
        signal: String,
        on: bool,
    },
    Fader {
        signal: String,
        db: f64,
        ramp_ms: f64,
    },
    Vca {
        vca: String,
        db: f64,
        ramp_ms: f64,
    },
    Param {
        signal: String,
        node: String,
        param: String,
        value: f64,
        ramp_ms: f64,
    },
    Pfl {
        signal: String,
        on: bool,
    },
    Talk {
        mic: String,
        destination: String,
        on: bool,
    },
    Publish {
        signal: String,
        on: bool,
    },
    Monitor {
        monitor: String,
        source: Option<Source>,
    },
    Loudness {
        signal: String,
        action: LoudnessAction,
    },
    Recall {
        snapshot: String,
    },
    Webhook {
        url: String,
    },
    Tsl {
        index: u16,
        left: bool,
        right: bool,
        label: String,
    },
    Log {
        text: String,
    },
}

fn on_off(on: bool) -> &'static str {
    if on {
        "on"
    } else {
        "off"
    }
}

impl Act {
    /// For the action list and the events socket.
    pub fn describe(&self) -> String {
        match self {
            Self::Mute { signal, on } => format!("mute {signal} {}", on_off(*on)),
            Self::Fader { signal, db, .. } => format!("fader {signal} {} dB", format_number(*db)),
            Self::Vca { vca, db, .. } => format!("VCA {vca} {} dB", format_number(*db)),
            Self::Param {
                signal,
                node,
                param,
                value,
                ..
            } => format!("{signal} {node} {param} {}", format_number(*value)),
            Self::Pfl { signal, on } => format!("PFL {signal} {}", on_off(*on)),
            Self::Talk {
                mic,
                destination,
                on,
            } => format!("talk {mic} to {destination} {}", on_off(*on)),
            Self::Publish { signal, on } => {
                format!("{} {signal}", if *on { "publish" } else { "unpublish" })
            }
            Self::Monitor { monitor, source } => format!(
                "monitor {monitor} source {}",
                match source {
                    Some(Source::Signal(s)) => s.clone(),
                    Some(Source::Input(i)) => format!("input {i}"),
                    None => "default".into(),
                }
            ),
            Self::Loudness { signal, action } => format!("loudness {signal} {}", action.name()),
            Self::Recall { snapshot } => format!("recall {snapshot}"),
            Self::Webhook { url } => format!("webhook {}", crate::control::redact_url(url)),
            Self::Tsl {
                index,
                left,
                right,
                label,
            } => format!(
                "TSL {index} left {} right {} {label:?}",
                on_off(*left),
                on_off(*right)
            ),
            Self::Log { text } => format!("log {text}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Action {
    pub graph: String,
    /// The block (`<instance>.<block>` inside a macro).
    pub block: String,
    pub dry_run: bool,
    /// A retry of a change that failed (D167): not listed again.
    pub retry: bool,
    pub act: Act,
}

/// One entry of a graph's action list (`actions` in the API).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Recent {
    pub t_ms: u64,
    pub block: String,
    pub action: String,
    pub dry_run: bool,
}

/// What a value action last applied.
#[derive(Debug, Clone, PartialEq)]
enum Applied {
    Bool(bool),
    Num(f64),
    Tsl(bool, bool, String),
}

/// A block's memory, kept by block id across recompiles.
#[derive(Debug, Clone, Default)]
struct State {
    /// Per input: its last on/off, for a value converted into an event.
    conv: Vec<Option<bool>>,
    held: bool,
    /// Last input (edge, timer, debounce).
    prev: bool,
    /// Ticks left (timer), waited (delay), stable (debounce).
    n: f64,
    count: f64,
    applied: Option<Applied>,
    /// What it applied was forgotten after a failure: its next action is a
    /// retry.
    retry: bool,
    /// Edge and timer: the input has been seen once (the first tick is not a
    /// rise, D172).
    seen: bool,
    /// The tick this action last ran (the rate limit, D173).
    last_run: Option<u64>,
}

/// Fewest ticks between two runs of one action block (D173): value actions
/// 20 ms, TSL 50 ms, webhooks 100 ms. A value held back is the newest one
/// when the block may run again.
fn min_ticks(kind: &BlockKind) -> u64 {
    match kind {
        BlockKind::Webhook { .. } => 10,
        BlockKind::TslOut { .. } => 5,
        BlockKind::SetMute { .. }
        | BlockKind::SetFader { .. }
        | BlockKind::SetVca { .. }
        | BlockKind::SetParam { .. } => 2,
        _ => 0,
    }
}

struct Plan {
    blocks: Vec<FlatBlock>,
    /// Per block and input: the block that feeds it.
    inputs: Vec<Vec<Option<usize>>>,
    /// Per block and input: a value reaches it. An input wired to a macro's
    /// exposed input that is itself unwired outside is not driven (D159).
    driven: Vec<Vec<bool>>,
    /// Per block and input: the fader block that feeds it, also through
    /// macro ports (its on/off goes to an on/off input, D244).
    fader: Vec<Vec<Option<usize>>>,
    /// The blocks that do not remember, in order.
    order: Vec<usize>,
    /// The blocks that remember.
    stateful: Vec<usize>,
}

impl Plan {
    fn new(g: &Graph, macros: &[Macro]) -> Result<Self, String> {
        let flat = inline(&g.blocks, &g.wires, macros)?;
        let order = flat
            .order()
            .map_err(|i| format!("a combinational loop through {}", flat.blocks[i].id))?;
        let mut inputs: Vec<Vec<Option<usize>>> = flat
            .blocks
            .iter()
            .map(|b| vec![None; b.ports.inputs.len()])
            .collect();
        for w in &flat.wires {
            inputs[w.to][w.to_port] = Some(w.from);
        }
        let stateful = (0..flat.blocks.len())
            .filter(|i| flat.blocks[*i].kind.remembers())
            .collect();
        // Follow pass-throughs (macro-in, macro-out) back to a real source.
        let is_pass =
            |k: &BlockKind| matches!(k, BlockKind::MacroIn { .. } | BlockKind::MacroOut { .. });
        let source = |mut src: usize| loop {
            if !is_pass(&flat.blocks[src].kind) {
                return Some(src);
            }
            src = inputs[src][0]?;
        };
        let driven = inputs
            .iter()
            .map(|ins| ins.iter().map(|s| s.and_then(source).is_some()).collect())
            .collect();
        let fader = inputs
            .iter()
            .map(|ins| {
                ins.iter()
                    .map(|s| {
                        s.and_then(source)
                            .filter(|&f| matches!(flat.blocks[f].kind, BlockKind::Fader { .. }))
                    })
                    .collect()
            })
            .collect();
        Ok(Self {
            driven,
            fader,
            order: order
                .into_iter()
                .filter(|i| !flat.blocks[*i].kind.remembers())
                .collect(),
            stateful,
            inputs,
            blocks: flat.blocks,
        })
    }
}

/// One enabled graph.
struct Run {
    graph: Graph,
    /// Shared with each tick, so the blocks are not copied to be read.
    plan: std::sync::Arc<Plan>,
    states: Vec<State>,
    /// Each block's value after the last tick (an action's: its input).
    outs: Vec<Value>,
    /// Times each block's value fired as an event.
    fired: Vec<u64>,
    dry_run: bool,
    /// Ticks run, for the rate limits (D173).
    tick: u64,
}

impl Run {
    fn new(g: &Graph, macros: &[Macro], old: Option<Run>) -> Result<Self, String> {
        let plan = Plan::new(g, macros)?;
        // A change of dry run is noticed by the caller (it resets what was applied).
        let dry_run = old.as_ref().map_or(g.dry_run, |o| o.dry_run);
        let tick = old.as_ref().map_or(0, |o| o.tick);
        // A block keeps its memory only while its type and parameters stay
        // the same (D172): a set-mute moved to another signal acts on it at
        // once; the old target stays as it is.
        type Kept = (BlockKind, Vec<Option<String>>, State, Value, u64);
        let mut kept: BTreeMap<String, Kept> = BTreeMap::new();
        if let Some(old) = old {
            let sources = |i: usize| -> Vec<Option<String>> {
                old.plan.inputs[i]
                    .iter()
                    .map(|s| s.map(|s| old.plan.blocks[s].id.clone()))
                    .collect()
            };
            let from: Vec<Vec<Option<String>>> = (0..old.plan.blocks.len()).map(sources).collect();
            for ((((b, s), v), f), src) in old
                .plan
                .blocks
                .iter()
                .zip(old.states)
                .zip(old.outs)
                .zip(old.fired)
                .zip(from)
            {
                kept.insert(b.id.clone(), (b.kind.clone(), src, s, v, f));
            }
        }
        let mut states = Vec::new();
        let mut outs = Vec::new();
        let mut fired = Vec::new();
        for (i, b) in plan.blocks.iter().enumerate() {
            let ty = b
                .ports
                .outputs
                .first()
                .or(b.ports.inputs.first())
                .map_or(PortType::Number, |p| p.ty);
            let fresh = || (State::default(), Value::default_of(ty), 0);
            let (mut s, v, f) = match kept.remove(&b.id) {
                Some((kind, src, s, v, f)) if kind == b.kind => {
                    let mut s = s;
                    // An input wired to another block starts over: its new
                    // value is not an edge (D172).
                    for (k, now) in plan.inputs[i].iter().enumerate() {
                        let now = now.map(|n| &plan.blocks[n].id);
                        if src.get(k).cloned().flatten().as_ref() != now {
                            if let Some(c) = s.conv.get_mut(k) {
                                *c = None;
                            }
                            s.seen = false;
                        }
                    }
                    (s, v, f)
                }
                _ => fresh(),
            };
            s.conv.resize(plan.inputs[i].len(), None);
            states.push(s);
            outs.push(v);
            fired.push(f);
        }
        Ok(Self {
            graph: g.clone(),
            plan: std::sync::Arc::new(plan),
            states,
            outs,
            fired,
            dry_run,
            tick,
        })
    }

    /// Input `k` of block `i`, converted to the input's type. A fader feeding
    /// an on/off input (directly or through macro ports) is on from its
    /// threshold (D244), not from 0.5.
    fn read(&mut self, i: usize, k: usize) -> Value {
        let want = self.plan.blocks[i].ports.inputs[k].ty;
        match self.plan.inputs[i][k] {
            None => Value::default_of(want),
            Some(src) => {
                let on_off = matches!(want, PortType::Boolean | PortType::Event);
                if let Some(f) = self.plan.fader[i][k].filter(|_| on_off) {
                    let high = Value::Bool(self.states[f].held);
                    convert(&high, want, &mut self.states[i].conv[k])
                } else {
                    convert(&self.outs[src], want, &mut self.states[i].conv[k])
                }
            }
        }
    }

    /// A value reaches input `k` of block `i` (also through macro ports).
    fn wired(&self, i: usize, k: usize) -> bool {
        self.plan.driven[i].get(k).copied().unwrap_or(false)
    }

    fn tick(
        &mut self,
        r: &Readings<'_>,
        pressed: &BTreeMap<String, bool>,
        triggered: &BTreeSet<String>,
    ) -> Vec<(String, Act, bool)> {
        let mut acts = Vec::new();
        let mut retries = Vec::new();
        self.tick += 1;
        let plan = self.plan.clone();
        for &i in &plan.order {
            let b = &plan.blocks[i];
            let before = acts.len();
            let v = self.evaluate(i, &b.id, &b.kind, r, pressed, triggered, &mut acts);
            if acts.len() > before {
                let retry = std::mem::take(&mut self.states[i].retry);
                retries.resize(acts.len(), retry);
            }
            self.outs[i] = v;
        }
        // The blocks that remember take this tick's value together, so one of
        // them reads another's previous tick: the delay a loop needs.
        let mut taken = Vec::with_capacity(plan.stateful.len());
        for &i in &plan.stateful {
            taken.push((i, self.remember(i, &plan.blocks[i].kind)));
        }
        for (i, v) in taken {
            self.outs[i] = v;
        }
        for (v, f) in self.outs.iter().zip(self.fired.iter_mut()) {
            if *v == Value::Event(true) {
                *f += 1;
            }
        }
        acts.into_iter()
            .zip(retries)
            .map(|((b, a), r)| (b, a, r))
            .collect()
    }

    #[allow(clippy::too_many_arguments)]
    fn evaluate(
        &mut self,
        i: usize,
        id: &str,
        kind: &BlockKind,
        r: &Readings<'_>,
        pressed: &BTreeMap<String, bool>,
        triggered: &BTreeSet<String>,
        acts: &mut Vec<(String, Act)>,
    ) -> Value {
        let b = Value::Bool;
        let level = |s: &str| r.level_db.get(s).copied().unwrap_or(FLOOR_DB);
        match kind {
            BlockKind::Const { value } => Value::Num(*value),
            BlockKind::Text { value } => Value::Str(value.clone()),
            BlockKind::Button => b(pressed.get(id).copied().unwrap_or(false)),
            BlockKind::Trigger => Value::Event(triggered.contains(id)),
            BlockKind::Mute { signal } => b(muted(r.mix, signal)),
            BlockKind::Pfl { signal } => b(r.pfl.contains(signal)),
            BlockKind::Fader {
                signal,
                threshold_db,
            } => {
                let db = fader(r.mix, signal);
                // On/off for the inputs it feeds (read): a missing or unset
                // signal is off.
                self.states[i].held = db.is_some_and(|db| db >= *threshold_db);
                Value::Num(db.unwrap_or(0.0))
            }
            BlockKind::Vad { signal } => b(r.speech.get(signal).copied().unwrap_or(false)),
            BlockKind::Level { signal } => Value::Num(level(signal)),
            BlockKind::Above {
                signal,
                threshold_db,
                hysteresis_db,
            } => {
                let l = level(signal);
                let s = &mut self.states[i];
                if !s.held {
                    s.held = l >= *threshold_db;
                } else if l < *threshold_db - *hysteresis_db {
                    s.held = false;
                }
                b(s.held)
            }
            BlockKind::Loudness { signal, measure } => {
                let v = r.loudness.get(signal).map_or(FLOOR_DB, |l| {
                    let x = match measure {
                        Measure::Momentary => l.momentary_lufs.0,
                        Measure::ShortTerm => l.short_term_lufs.0,
                        Measure::Integrated => l.integrated_lufs.0,
                        Measure::Range => l.range_lu,
                        Measure::TruePeak => l.max_true_peak_dbtp.0,
                    };
                    if x.is_finite() {
                        x
                    } else {
                        FLOOR_DB
                    }
                });
                Value::Num(v)
            }
            BlockKind::Tally { index } => b(r.on_air.contains(index)),
            BlockKind::Schedule {
                start_minute,
                end_minute,
            } => b(in_schedule(r.minute_of_day, *start_minute, *end_minute)),
            BlockKind::NmosReceiver { input } => b(r.nmos.contains(&format!("in-{input}"))),
            BlockKind::NmosSender { signal } => {
                b(r.nmos.contains(&crate::outputs::nmos_key(signal)))
            }
            BlockKind::Link { peer } => b(r.link.contains(peer)),
            BlockKind::And => {
                let (x, y) = (self.read(i, 0).truthy(), self.read(i, 1).truthy());
                b(x && y)
            }
            BlockKind::Or => {
                let (x, y) = (self.read(i, 0).truthy(), self.read(i, 1).truthy());
                b(x || y)
            }
            BlockKind::Xor => {
                let (x, y) = (self.read(i, 0).truthy(), self.read(i, 1).truthy());
                b(x != y)
            }
            BlockKind::Not => b(!self.read(i, 0).truthy()),
            BlockKind::Compare { op } => {
                let (x, y) = (self.read(i, 0).num(), self.read(i, 1).num());
                let eq = (x - y).abs() < 1e-6 || x == y;
                b(match op {
                    CompareOp::Gt => x > y,
                    CompareOp::Lt => x < y,
                    CompareOp::Eq => eq,
                    CompareOp::Ge => x > y || eq,
                    CompareOp::Le => x < y || eq,
                    CompareOp::Ne => !eq,
                })
            }
            BlockKind::Math { op } => {
                let (x, y) = (self.read(i, 0).num(), self.read(i, 1).num());
                Value::Num(match op {
                    MathOp::Add => x + y,
                    MathOp::Sub => x - y,
                    MathOp::Mul => x * y,
                    MathOp::Div if y == 0.0 => 0.0,
                    MathOp::Div => x / y,
                    MathOp::Min => x.min(y),
                    MathOp::Max => x.max(y),
                })
            }
            BlockKind::Scale { factor, offset } => {
                Value::Num(self.read(i, 0).num() * factor + offset)
            }
            BlockKind::Map { points } => Value::Num(map(points, self.read(i, 0).num())),
            BlockKind::Select { inputs, .. } => {
                let sel = self.read(i, 0).num();
                let last = f64::from(*inputs) - 1.0;
                let k = if sel.is_nan() {
                    0
                } else {
                    sel.round().clamp(0.0, last) as usize
                };
                // Every input is read, so their conversions keep their state.
                let values: Vec<Value> = (1..=usize::from(*inputs))
                    .map(|k| self.read(i, k))
                    .collect();
                values[k].clone()
            }
            BlockKind::MacroIn { .. } | BlockKind::MacroOut { .. } => self.read(i, 0),
            BlockKind::Edge
            | BlockKind::Latch
            | BlockKind::Toggle
            | BlockKind::Counter
            | BlockKind::Timer { .. }
            | BlockKind::Delay { .. }
            | BlockKind::Debounce { .. }
            | BlockKind::Macro { .. } => Value::Bool(false),
            _ => self.action(i, id, kind, acts),
        }
    }

    /// An action block: what it asks for (into `acts`), and its input's value.
    fn action(
        &mut self,
        i: usize,
        id: &str,
        kind: &BlockKind,
        acts: &mut Vec<(String, Act)>,
    ) -> Value {
        let tick = self.tick;
        let ready = self.states[i]
            .last_run
            .is_none_or(|t| tick >= t + min_ticks(kind));
        if let BlockKind::TslOut { index } = kind {
            // No index chosen yet: it never acts (D164).
            let Some(index) = index else {
                return Value::Bool(false);
            };
            if !(0..3).any(|k| self.wired(i, k)) {
                return Value::Bool(false);
            }
            let left = self.read(i, 0).truthy();
            let right = self.read(i, 1).truthy();
            let label = self.read(i, 2).text();
            let now = Applied::Tsl(left, right, label.clone());
            if ready && self.states[i].applied.as_ref() != Some(&now) {
                self.states[i].applied = Some(now);
                self.states[i].last_run = Some(tick);
                acts.push((
                    id.to_string(),
                    Act::Tsl {
                        index: *index,
                        left,
                        right,
                        label,
                    },
                ));
            }
            return Value::Bool(left);
        }
        if !self.wired(i, 0) || kind.target_unset() {
            // No input, or no target chosen yet: it never acts (D164).
            return Value::default_of(self.plan.blocks[i].ports.inputs[0].ty);
        }
        let v = self.read(i, 0);
        let mut follow_bool = |s: &mut State, act: &dyn Fn(bool) -> Act| {
            let on = v.truthy();
            if ready && s.applied != Some(Applied::Bool(on)) {
                s.applied = Some(Applied::Bool(on));
                s.last_run = Some(tick);
                acts.push((id.to_string(), act(on)));
            }
        };
        let s = &mut self.states[i];
        match kind {
            BlockKind::SetMute { signal } => follow_bool(s, &|on| Act::Mute {
                signal: signal.clone(),
                on,
            }),
            BlockKind::SetPfl { signal } => follow_bool(s, &|on| Act::Pfl {
                signal: signal.clone(),
                on,
            }),
            BlockKind::SetPublish { signal } => follow_bool(s, &|on| Act::Publish {
                signal: signal.clone(),
                on,
            }),
            BlockKind::SetTalk { mic, destination } => follow_bool(s, &|on| Act::Talk {
                mic: mic.clone(),
                destination: destination.clone(),
                on,
            }),
            BlockKind::SetFader { .. } | BlockKind::SetVca { .. } | BlockKind::SetParam { .. } => {
                let x = v.num();
                // Not a number is no change (D170); +∞ and −∞ are clamped.
                let changed = !x.is_nan()
                    && ready
                    && match s.applied {
                        Some(Applied::Num(p)) => {
                            let tolerance = if matches!(kind, BlockKind::SetParam { .. }) {
                                1e-6 * p.abs().max(1.0)
                            } else {
                                0.05
                            };
                            !(p == x || (p - x).abs() <= tolerance)
                        }
                        _ => true,
                    };
                if changed {
                    s.applied = Some(Applied::Num(x));
                    s.last_run = Some(tick);
                    acts.push((
                        id.to_string(),
                        match kind {
                            BlockKind::SetFader { signal, ramp_ms } => Act::Fader {
                                signal: signal.clone(),
                                db: x,
                                ramp_ms: *ramp_ms,
                            },
                            BlockKind::SetVca { vca, ramp_ms } => Act::Vca {
                                vca: vca.clone(),
                                db: x,
                                ramp_ms: *ramp_ms,
                            },
                            BlockKind::SetParam {
                                signal,
                                node,
                                param,
                                ramp_ms,
                            } => Act::Param {
                                signal: signal.clone(),
                                node: node.clone(),
                                param: param.clone(),
                                value: x,
                                ramp_ms: *ramp_ms,
                            },
                            _ => unreachable!(),
                        },
                    ));
                }
            }
            BlockKind::Webhook { .. } if !ready => {
                // At most one post per 100 ms per block (D173).
            }
            _ if v.truthy() => {
                s.last_run = Some(tick);
                let act = match kind {
                    BlockKind::SetMonitor { monitor, source } => Act::Monitor {
                        monitor: monitor.clone(),
                        source: source.clone(),
                    },
                    BlockKind::LoudnessControl { signal, action } => Act::Loudness {
                        signal: signal.clone(),
                        action: *action,
                    },
                    BlockKind::Recall { snapshot } => Act::Recall {
                        snapshot: snapshot.clone(),
                    },
                    BlockKind::Webhook { url } => Act::Webhook { url: url.clone() },
                    BlockKind::Log { message } => {
                        let value = self.wired(i, 1).then(|| self.read(i, 1).text());
                        Act::Log {
                            text: match value {
                                Some(x) if message.is_empty() => x,
                                Some(x) => format!("{message} {x}"),
                                None => message.clone(),
                            },
                        }
                    }
                    _ => return v,
                };
                acts.push((id.to_string(), act));
            }
            _ => {}
        }
        v
    }

    /// A block that remembers: its value for the next tick.
    fn remember(&mut self, i: usize, kind: &BlockKind) -> Value {
        match kind {
            BlockKind::Edge => {
                let cur = self.read(i, 0).truthy();
                let s = &mut self.states[i];
                let rose = s.seen && cur && !s.prev;
                s.prev = cur;
                s.seen = true;
                Value::Event(rose)
            }
            BlockKind::Latch => {
                let (set, reset) = (self.read(i, 0).truthy(), self.read(i, 1).truthy());
                let s = &mut self.states[i];
                if reset {
                    s.held = false;
                } else if set {
                    s.held = true;
                }
                Value::Bool(s.held)
            }
            BlockKind::Toggle => {
                let fire = self.read(i, 0).truthy();
                let s = &mut self.states[i];
                if fire {
                    s.held = !s.held;
                }
                Value::Bool(s.held)
            }
            BlockKind::Counter => {
                let up = self.read(i, 0).truthy();
                let down = self.read(i, 1).truthy();
                let reset = self.read(i, 2).truthy();
                let s = &mut self.states[i];
                if reset {
                    s.count = 0.0;
                } else {
                    s.count += f64::from(u8::from(up)) - f64::from(u8::from(down));
                }
                Value::Num(s.count)
            }
            BlockKind::Timer { ms } => {
                let cur = self.read(i, 0).truthy();
                let s = &mut self.states[i];
                if s.seen && cur && !s.prev {
                    s.n = pulse_ticks(*ms);
                }
                s.prev = cur;
                s.seen = true;
                let running = s.n > 0.0;
                if running {
                    s.n -= 1.0;
                }
                Value::Bool(running)
            }
            BlockKind::Delay { ms } => {
                let cur = self.read(i, 0).truthy();
                let s = &mut self.states[i];
                if cur == s.held {
                    s.n = 0.0;
                } else {
                    s.n += 1.0;
                    if s.n >= pulse_ticks(*ms).max(1.0) {
                        s.held = cur;
                        s.n = 0.0;
                    }
                }
                Value::Bool(s.held)
            }
            BlockKind::Debounce { ms } => {
                let cur = self.read(i, 0).truthy();
                let s = &mut self.states[i];
                s.n = if cur == s.prev { s.n + 1.0 } else { 1.0 };
                s.prev = cur;
                if s.n >= pulse_ticks(*ms).max(1.0) {
                    s.held = cur;
                    s.n = 0.0;
                }
                Value::Bool(s.held)
            }
            _ => Value::Bool(false),
        }
    }

    fn values(&self) -> Json {
        let mut m = serde_json::Map::new();
        for (i, b) in self.plan.blocks.iter().enumerate() {
            let v = match &self.outs[i] {
                Value::Bool(x) => json!(x),
                Value::Num(x) => json!(x),
                Value::Str(s) => json!(s),
                Value::Event(_) => json!(self.fired[i]),
            };
            m.insert(b.id.clone(), v);
        }
        Json::Object(m)
    }
}

#[derive(Default)]
pub struct Logic {
    runs: BTreeMap<String, Run>,
    /// The macros the runs were compiled with.
    macros: Vec<Macro>,
    /// Soft keys, per graph and block.
    buttons: BTreeMap<String, BTreeMap<String, bool>>,
    /// Triggers posted since the last tick, per graph.
    triggers: BTreeMap<String, BTreeSet<String>>,
    recent: BTreeMap<String, VecDeque<Recent>>,
    /// Graphs that do not compile (validation should have refused them).
    broken: BTreeSet<String>,
    /// Ticks evaluated (`logic_ticks_total`).
    pub ticks: u64,
    /// Actions run, per graph (`logic_actions_total{graph}`).
    pub actions: BTreeMap<String, u64>,
}

impl Logic {
    pub fn set_button(&mut self, graph: &str, block: &str, pressed: bool) {
        self.buttons
            .entry(graph.to_string())
            .or_default()
            .insert(block.to_string(), pressed);
    }

    pub fn button(&self, graph: &str, block: &str) -> bool {
        self.buttons
            .get(graph)
            .and_then(|g| g.get(block))
            .copied()
            .unwrap_or(false)
    }

    /// The soft keys held down in `graph`.
    pub fn pressed(&self, graph: &str) -> Vec<String> {
        self.buttons
            .get(graph)
            .map(|g| {
                g.iter()
                    .filter(|(_, p)| **p)
                    .map(|(b, _)| b.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Fire trigger `block` of `graph` on the next tick (D166).
    pub fn trigger(&mut self, graph: &str, block: &str) {
        self.triggers
            .entry(graph.to_string())
            .or_default()
            .insert(block.to_string());
    }

    /// Forget what action `block` of `graph` applied, so it runs again with
    /// its current input (a change that could not be applied).
    pub fn forget(&mut self, graph: &str, block: &str) {
        if let Some(run) = self.runs.get_mut(graph) {
            if let Some(i) = run.plan.blocks.iter().position(|b| b.id == block) {
                run.states[i].applied = None;
                run.states[i].retry = true;
            }
        }
    }

    /// Each block's value after the last tick, as JSON (`values` in the API).
    pub fn values(&self, graph: &str) -> Option<Json> {
        self.runs.get(graph).map(Run::values)
    }

    /// The last [`RECENT`] actions of `graph`, oldest first.
    pub fn recent(&self, graph: &str) -> Vec<Recent> {
        self.recent
            .get(graph)
            .map(|r| r.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// One tick. Returns the actions to run (and the dry-run ones, marked).
    pub fn tick(&mut self, r: &Readings<'_>) -> Vec<Action> {
        let mix = r.mix;
        let macros_changed = self.macros != mix.logic_macros;
        if macros_changed {
            self.macros = mix.logic_macros.clone();
        }
        let enabled: BTreeSet<&str> = mix
            .logic
            .iter()
            .filter(|g| g.enabled)
            .map(|g| g.id.as_str())
            .collect();
        self.runs.retain(|id, _| enabled.contains(id.as_str()));
        self.recent
            .retain(|id, _| mix.logic.iter().any(|g| &g.id == id));
        self.buttons
            .retain(|id, _| mix.logic.iter().any(|g| &g.id == id));
        self.ticks += 1;
        let mut out = Vec::new();
        for g in mix.logic.iter().filter(|g| g.enabled) {
            let stale = self
                .runs
                .get(&g.id)
                .is_none_or(|run| macros_changed || run.graph != *g);
            if stale {
                let old = self.runs.remove(&g.id);
                match Run::new(g, &self.macros, old) {
                    Ok(run) => {
                        self.broken.remove(&g.id);
                        self.runs.insert(g.id.clone(), run);
                    }
                    Err(e) => {
                        if self.broken.insert(g.id.clone()) {
                            tracing::warn!("logic {}: {e}", g.id);
                        }
                        continue;
                    }
                }
            }
            let run = self.runs.get_mut(&g.id).expect("compiled");
            if run.dry_run != g.dry_run {
                // What dry run did not apply is applied once it ends.
                run.dry_run = g.dry_run;
                for s in &mut run.states {
                    s.applied = None;
                }
            }
            let pressed = self.buttons.get(&g.id).cloned().unwrap_or_default();
            let triggered = self.triggers.remove(&g.id).unwrap_or_default();
            let acts = run.tick(r, &pressed, &triggered);
            if acts.is_empty() {
                continue;
            }
            if !g.dry_run {
                *self.actions.entry(g.id.clone()).or_default() += acts.len() as u64;
            }
            let recent = self.recent.entry(g.id.clone()).or_default();
            for (block, act, retry) in acts {
                if !retry {
                    recent.push_back(Recent {
                        t_ms: r.now_ms,
                        block: block.clone(),
                        action: act.describe(),
                        dry_run: g.dry_run,
                    });
                    if recent.len() > RECENT {
                        recent.pop_front();
                    }
                }
                out.push(Action {
                    graph: g.id.clone(),
                    block,
                    dry_run: g.dry_run,
                    retry,
                    act,
                });
            }
        }
        self.triggers.clear();
        out
    }
}

/// Ticks a logic change that rewires the plan may wait for the change lock
/// before it is reported (1 s); it keeps waiting.
pub const MAX_WAIT_TICKS: u32 = 100;
/// Least time between two logic changes that rewire the plan (D173).
pub const REWIRE_INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);

/// Logic changes that rewire the plan (PFL, talk, publish, monitor source,
/// recall), waiting for the API's change lock, so an API change and a logic
/// change never overwrite each other's staged plan (D167). The newest action
/// of a block replaces an older one still waiting.
#[derive(Default)]
pub struct Waiting {
    actions: Vec<Action>,
    ticks: u32,
    last_go: Option<std::time::Instant>,
}

/// What [`Waiting::take`] found.
pub enum Taken {
    /// Nothing waits.
    Idle,
    /// The lock is held, or the last change was less than
    /// [`REWIRE_INTERVAL`] ago; try again next tick.
    Wait,
    /// The lock has been held for [`MAX_WAIT_TICKS`]: these still wait (the
    /// caller warns once).
    Late(Vec<Action>),
    /// The lock, held until the change is swapped in, and the actions.
    Go(tokio::sync::OwnedMutexGuard<()>, Vec<Action>),
}

impl Waiting {
    pub fn push(&mut self, a: Action) {
        self.actions
            .retain(|x| !(x.graph == a.graph && x.block == a.block));
        self.actions.push(a);
    }

    /// One try per tick.
    pub fn take(
        &mut self,
        lock: &std::sync::Arc<tokio::sync::Mutex<()>>,
        now: std::time::Instant,
    ) -> Taken {
        if self.actions.is_empty() {
            return Taken::Idle;
        }
        if self.last_go.is_some_and(|t| now < t + REWIRE_INTERVAL) {
            return Taken::Wait;
        }
        match lock.clone().try_lock_owned() {
            Ok(guard) => {
                self.ticks = 0;
                self.last_go = Some(now);
                Taken::Go(guard, std::mem::take(&mut self.actions))
            }
            Err(_) => {
                self.ticks += 1;
                if self.ticks == MAX_WAIT_TICKS {
                    Taken::Late(self.actions.clone())
                } else {
                    Taken::Wait
                }
            }
        }
    }
}

/// The curve through `points` at `x`: linear between them, flat outside.
fn map(points: &[[f64; 2]], x: f64) -> f64 {
    let (Some(first), Some(last)) = (points.first(), points.last()) else {
        return x;
    };
    if x <= first[0] || x.is_nan() {
        return first[1];
    }
    if x >= last[0] {
        return last[1];
    }
    for w in points.windows(2) {
        let ([x0, y0], [x1, y1]) = (w[0], w[1]);
        if x <= x1 {
            return y0 + (y1 - y0) * (x - x0) / (x1 - x0);
        }
    }
    last[1]
}

fn in_schedule(minute: u16, start: u16, end: u16) -> bool {
    if start == end {
        false
    } else if start < end {
        minute >= start && minute < end
    } else {
        minute >= start || minute < end
    }
}

/// Ticks of 10 ms a pulse lasts. Rounded up, so it is at least `ms` and at
/// most one tick longer. Zero stays zero. One day is the longest.
fn pulse_ticks(ms: f64) -> f64 {
    if !ms.is_finite() || ms <= 0.0 {
        0.0
    } else {
        (ms / 10.0).ceil().min(8_640_000.0)
    }
}

fn muted(mix: &Mix, signal: &str) -> bool {
    mix.strip(signal)
        .map(|s| s.mute)
        .or_else(|| mix.bus(signal).map(|b| b.mute))
        .unwrap_or(false)
}

fn fader(mix: &Mix, signal: &str) -> Option<f64> {
    mix.strip(signal)
        .map(|s| s.fader_db.0)
        .or_else(|| mix.bus(signal).map(|b| b.fader_db.0))
}

/// Clamp a fader a logic graph asked for: at most `MAX_FADER_DB` (also +∞),
/// −∞ closes it. Not a number never gets here (D170).
pub fn clamp_fader(db: f64) -> Db {
    Db(if db == f64::NEG_INFINITY || db.is_nan() {
        f64::NEG_INFINITY
    } else {
        db.min(crate::mix::model::MAX_FADER_DB)
    })
}

#[cfg(test)]
mod tests;
