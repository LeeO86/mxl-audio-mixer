//! The logic runtime (§11.2): every 10 ms (100 Hz, the control engine's tick)
//! each enabled graph is evaluated in order. A block that remembers (`edge`,
//! `latch`, `timer`) publishes the value it computed last tick, which is what
//! lets a loop through one be deterministic. Actions run when their input changed,
//! and not at all while the graph is in dry-run.

use std::collections::{BTreeMap, BTreeSet};

use crate::mix::logic::{Block, BlockKind, CompareOp, Graph};
use crate::mix::model::Db;
use crate::mix::Mix;

/// What the blocks read, gathered once per tick.
pub struct Readings<'a> {
    pub mix: &'a Mix,
    pub speech: &'a BTreeMap<String, bool>,
    pub level_db: &'a BTreeMap<String, f64>,
    /// Tally indexes whose left lamp is on. Empty before any tally.
    pub on_air: &'a BTreeSet<u16>,
    /// UTC minute of the day, 0..1440.
    pub minute_of_day: u16,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Action {
    pub signal: String,
    pub mute: Option<bool>,
    pub fader_db: Option<f64>,
    /// Recall this snapshot. Set on a rising edge only.
    pub recall: Option<String>,
    /// POST this URL. Set on a rising edge only.
    pub webhook: Option<String>,
}

#[derive(Default)]
pub struct Logic {
    /// Last value of the blocks that remember, per graph.
    memory: BTreeMap<String, BTreeMap<String, f64>>,
    /// What each action last applied, so it runs only when its input changes.
    applied: BTreeMap<String, BTreeMap<String, f64>>,
    /// Soft keys, per graph and block.
    buttons: BTreeMap<String, BTreeMap<String, bool>>,
    /// The last tick's value of every block, per graph.
    pub values: BTreeMap<String, BTreeMap<String, f64>>,
    /// Ticks evaluated (`logic_ticks_total`).
    pub ticks: u64,
    /// Actions returned, per graph (`logic_actions_total{graph}`).
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

    /// One tick. Returns the actions whose input changed.
    pub fn tick(&mut self, readings: &Readings<'_>) -> Vec<Action> {
        let graphs: Vec<Graph> = readings
            .mix
            .logic
            .iter()
            .filter(|g| g.enabled)
            .cloned()
            .collect();
        self.values
            .retain(|id, _| graphs.iter().any(|g| &g.id == id));
        self.memory
            .retain(|id, _| graphs.iter().any(|g| &g.id == id));
        self.applied
            .retain(|id, _| graphs.iter().any(|g| &g.id == id));
        self.ticks += 1;
        let mut actions = Vec::new();
        for g in &graphs {
            let a = self.graph(g, readings);
            if !a.is_empty() {
                *self.actions.entry(g.id.clone()).or_default() += a.len() as u64;
            }
            actions.extend(a);
        }
        actions
    }

    fn graph(&mut self, g: &Graph, readings: &Readings<'_>) -> Vec<Action> {
        let order = order(g);
        let mut out: BTreeMap<String, f64> = BTreeMap::new();
        let memory = self.memory.entry(g.id.clone()).or_default();
        for b in &g.blocks {
            if b.kind.remembers() {
                out.insert(b.id.clone(), memory.get(&b.id).copied().unwrap_or(0.0));
            }
        }
        let pressed = self.buttons.get(&g.id).cloned().unwrap_or_default();
        let input = |b: &Block, port: &str, out: &BTreeMap<String, f64>| -> f64 {
            g.wires
                .iter()
                .find(|w| w.to_block == b.id && w.to_port == port)
                .and_then(|w| out.get(&w.from_block))
                .copied()
                .unwrap_or(0.0)
        };
        for id in &order {
            let b = g.blocks.iter().find(|b| &b.id == id).unwrap();
            if b.kind.remembers() {
                continue;
            }
            let v = match &b.kind {
                BlockKind::Const { value } => *value,
                BlockKind::Button => {
                    f64::from(u8::from(pressed.get(&b.id).copied().unwrap_or(false)))
                }
                BlockKind::Mute { signal } => f64::from(u8::from(muted(readings.mix, signal))),
                BlockKind::Fader { signal } => fader(readings.mix, signal),
                BlockKind::Vad { signal } => f64::from(u8::from(
                    readings.speech.get(signal).copied().unwrap_or(false),
                )),
                BlockKind::Level { signal } => {
                    readings.level_db.get(signal).copied().unwrap_or(-200.0)
                }
                BlockKind::Tally { index } => f64::from(u8::from(readings.on_air.contains(index))),
                BlockKind::Above {
                    signal,
                    threshold_db,
                    hysteresis_db,
                } => {
                    let level = readings.level_db.get(signal).copied().unwrap_or(-200.0);
                    let key = format!("above:{}", b.id);
                    let mut held = memory.get(&key).copied().unwrap_or(0.0);
                    if held < 0.5 {
                        if level >= *threshold_db {
                            held = 1.0;
                        }
                    } else if level < *threshold_db - *hysteresis_db {
                        held = 0.0;
                    }
                    memory.insert(key, held);
                    held
                }
                BlockKind::And => {
                    f64::from(u8::from(on(input(b, "a", &out)) && on(input(b, "b", &out))))
                }
                BlockKind::Or => {
                    f64::from(u8::from(on(input(b, "a", &out)) || on(input(b, "b", &out))))
                }
                BlockKind::Xor => {
                    f64::from(u8::from(on(input(b, "a", &out)) != on(input(b, "b", &out))))
                }
                BlockKind::Schedule {
                    start_minute,
                    end_minute,
                } => f64::from(u8::from(in_schedule(
                    readings.minute_of_day,
                    *start_minute,
                    *end_minute,
                ))),
                BlockKind::Not => f64::from(u8::from(!on(input(b, "in", &out)))),
                BlockKind::Compare { op } => {
                    let (a, b2) = (input(b, "a", &out), input(b, "b", &out));
                    let r = match op {
                        CompareOp::Gt => a > b2,
                        CompareOp::Lt => a < b2,
                        CompareOp::Eq => (a - b2).abs() < 1e-6,
                    };
                    f64::from(u8::from(r))
                }
                BlockKind::SetMute { .. }
                | BlockKind::SetFader { .. }
                | BlockKind::Recall { .. }
                | BlockKind::Webhook { .. } => {
                    input(b, b.kind.ports().1.first().copied().unwrap_or("in"), &out)
                }
                BlockKind::Edge
                | BlockKind::Latch
                | BlockKind::Timer { .. }
                | BlockKind::Delay { .. }
                | BlockKind::Debounce { .. } => 0.0,
            };
            out.insert(b.id.clone(), v);
        }
        // The blocks that remember publish last tick's value (what the blocks
        // after them read above). They all take this tick's value together, so
        // one of them reads another's previous tick: the delay a loop needs.
        let mut taken = Vec::new();
        for b in g.blocks.iter().filter(|b| b.kind.remembers()) {
            let now = match &b.kind {
                BlockKind::Edge => {
                    let key = format!("in:{}", b.id);
                    let was = memory.get(&key).copied().unwrap_or(0.0);
                    let cur = input(b, "in", &out);
                    memory.insert(key, cur);
                    f64::from(u8::from(on(cur) && !on(was)))
                }
                BlockKind::Latch => {
                    let mut held = memory.get(&b.id).copied().unwrap_or(0.0);
                    if on(input(b, "reset", &out)) {
                        held = 0.0;
                    } else if on(input(b, "set", &out)) {
                        held = 1.0;
                    }
                    held
                }
                BlockKind::Timer { ms } => {
                    let key_in = format!("in:{}", b.id);
                    let key_left = format!("left:{}", b.id);
                    let was = memory.get(&key_in).copied().unwrap_or(0.0);
                    let cur = input(b, "in", &out);
                    memory.insert(key_in, cur);
                    let mut left = if on(cur) && !on(was) {
                        pulse_ticks(*ms)
                    } else {
                        memory.get(&key_left).copied().unwrap_or(0.0)
                    };
                    let running = left > 0.0;
                    if left > 0.0 {
                        left -= 1.0;
                    }
                    memory.insert(key_left, left);
                    f64::from(u8::from(running))
                }
                BlockKind::Delay { ms } => {
                    let key_out = format!("delay-out:{}", b.id);
                    let key_wait = format!("delay-wait:{}", b.id);
                    let cur = on(input(b, "in", &out));
                    let mut held = memory.get(&key_out).copied().unwrap_or(0.0) >= 0.5;
                    let mut wait = memory.get(&key_wait).copied().unwrap_or(0.0);
                    if cur == held {
                        wait = 0.0;
                    } else {
                        wait += 1.0;
                        if wait >= pulse_ticks(*ms).max(1.0) {
                            held = cur;
                            wait = 0.0;
                        }
                    }
                    memory.insert(key_out, f64::from(u8::from(held)));
                    memory.insert(key_wait, wait);
                    f64::from(u8::from(held))
                }
                BlockKind::Debounce { ms } => {
                    let key_out = format!("deb-out:{}", b.id);
                    let key_in = format!("deb-in:{}", b.id);
                    let key_n = format!("deb-n:{}", b.id);
                    let cur = on(input(b, "in", &out));
                    let prev = memory.get(&key_in).copied().unwrap_or(0.0) >= 0.5;
                    let mut n = if cur == prev {
                        memory.get(&key_n).copied().unwrap_or(0.0) + 1.0
                    } else {
                        1.0
                    };
                    let mut held = memory.get(&key_out).copied().unwrap_or(0.0) >= 0.5;
                    if n >= pulse_ticks(*ms).max(1.0) {
                        held = cur;
                        n = 0.0;
                    }
                    memory.insert(key_in, f64::from(u8::from(cur)));
                    memory.insert(key_n, n);
                    memory.insert(key_out, f64::from(u8::from(held)));
                    f64::from(u8::from(held))
                }
                _ => 0.0,
            };
            taken.push((b.id.clone(), now));
        }
        for (id, now) in taken {
            memory.insert(id.clone(), now);
            out.insert(id, now);
        }
        let mut actions = Vec::new();
        if !g.dry_run {
            let applied = self.applied.entry(g.id.clone()).or_default();
            for b in &g.blocks {
                let v = out.get(&b.id).copied().unwrap_or(0.0);
                let last = applied.get(&b.id).copied();
                match &b.kind {
                    BlockKind::SetMute { signal } if last.is_none_or(|p| on(p) != on(v)) => {
                        applied.insert(b.id.clone(), v);
                        actions.push(Action {
                            signal: signal.clone(),
                            mute: Some(on(v)),
                            fader_db: None,
                            recall: None,
                            webhook: None,
                        });
                    }
                    BlockKind::SetFader { signal } if last.is_none_or(|p| (p - v).abs() > 0.05) => {
                        applied.insert(b.id.clone(), v);
                        actions.push(Action {
                            signal: signal.clone(),
                            mute: None,
                            fader_db: Some(v),
                            recall: None,
                            webhook: None,
                        });
                    }
                    BlockKind::Webhook { url } => {
                        let was = last.unwrap_or(v);
                        if last.is_none() {
                            applied.insert(b.id.clone(), v);
                        } else if on(v) && !on(was) {
                            applied.insert(b.id.clone(), v);
                            actions.push(Action {
                                signal: String::new(),
                                mute: None,
                                fader_db: None,
                                recall: None,
                                webhook: Some(url.clone()),
                            });
                        } else if on(v) != on(was) {
                            applied.insert(b.id.clone(), v);
                        }
                    }
                    BlockKind::Recall { snapshot } => {
                        let was = last.unwrap_or(v);
                        if last.is_none() {
                            applied.insert(b.id.clone(), v);
                        } else if on(v) && !on(was) {
                            applied.insert(b.id.clone(), v);
                            actions.push(Action {
                                signal: String::new(),
                                mute: None,
                                fader_db: None,
                                recall: Some(snapshot.clone()),
                                webhook: None,
                            });
                        } else if on(v) != on(was) {
                            applied.insert(b.id.clone(), v);
                        }
                    }
                    _ => {}
                }
            }
        }
        self.values.insert(g.id.clone(), out);
        actions
    }
}

fn on(v: f64) -> bool {
    v >= 0.5
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

fn fader(mix: &Mix, signal: &str) -> f64 {
    mix.strip(signal)
        .map(|s| s.fader_db.0)
        .or_else(|| mix.bus(signal).map(|b| b.fader_db.0))
        .unwrap_or(0.0)
}

/// Combinational order: a block after the blocks it reads, except a block that
/// remembers, whose output is already known.
fn order(g: &Graph) -> Vec<String> {
    let mut incoming: BTreeMap<&str, usize> = g.blocks.iter().map(|b| (b.id.as_str(), 0)).collect();
    let mut next: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for w in &g.wires {
        let to = g.blocks.iter().find(|b| b.id == w.to_block);
        if to.is_some_and(|b| b.kind.remembers()) {
            continue;
        }
        *incoming.get_mut(w.to_block.as_str()).unwrap() += 1;
        next.entry(w.from_block.as_str())
            .or_default()
            .push(&w.to_block);
    }
    let mut ready: Vec<&str> = incoming
        .iter()
        .filter(|(_, n)| **n == 0)
        .map(|(id, _)| *id)
        .collect();
    let mut out = Vec::new();
    while let Some(id) = ready.pop() {
        out.push(id.to_string());
        for n in next.get(id).into_iter().flatten() {
            let c = incoming.get_mut(n).unwrap();
            *c -= 1;
            if *c == 0 {
                ready.push(n);
            }
        }
    }
    out
}

/// Clamp a fader a logic graph asked for.
pub fn clamp_fader(db: f64) -> Db {
    Db(if db.is_finite() {
        db.min(crate::mix::model::MAX_FADER_DB)
    } else {
        f64::NEG_INFINITY
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mix::logic::{Block, BlockKind, CompareOp, Graph, Wire};
    use crate::mix::model::tests::strip;
    use crate::mix::model::Db;
    use crate::mix::Format;

    fn block(id: &str, kind: BlockKind) -> Block {
        Block {
            id: id.into(),
            kind,
            x: 0.0,
            y: 0.0,
        }
    }

    fn wire(from: &str, to: &str, port: &str) -> Wire {
        Wire {
            from_block: from.into(),
            from_port: "out".into(),
            to_block: to.into(),
            to_port: port.into(),
        }
    }

    fn readings<'a>(
        mix: &'a Mix,
        speech: &'a BTreeMap<String, bool>,
        level: &'a BTreeMap<String, f64>,
        on_air: &'a BTreeSet<u16>,
    ) -> Readings<'a> {
        Readings {
            mix,
            speech,
            level_db: level,
            on_air,
            minute_of_day: 0,
        }
    }

    fn readings_at<'a>(
        mix: &'a Mix,
        speech: &'a BTreeMap<String, bool>,
        level: &'a BTreeMap<String, f64>,
        on_air: &'a BTreeSet<u16>,
        minute_of_day: u16,
    ) -> Readings<'a> {
        Readings {
            minute_of_day,
            ..readings(mix, speech, level, on_air)
        }
    }

    #[test]
    fn speech_mutes_the_music_one_tick_after_it_starts() {
        let mut mix = Mix::default();
        mix.strips.push(strip("vox", Format::Mono));
        mix.strips.push(strip("music", Format::Mono));
        mix.logic.push(Graph {
            id: "g".into(),
            name: "G".into(),
            description: String::new(),
            enabled: true,
            dry_run: false,
            blocks: vec![
                block(
                    "v",
                    BlockKind::Vad {
                        signal: "vox".into(),
                    },
                ),
                block(
                    "m",
                    BlockKind::SetMute {
                        signal: "music".into(),
                    },
                ),
            ],
            wires: vec![wire("v", "m", "in")],
        });
        let mut logic = Logic::default();
        let level = BTreeMap::new();
        let on_air = BTreeSet::new();
        let mut speech = BTreeMap::from([("vox".to_string(), false)]);
        // The first tick applies what the graph says now.
        assert_eq!(
            logic.tick(&readings(&mix, &speech, &level, &on_air)),
            vec![Action {
                signal: "music".into(),
                mute: Some(false),
                fader_db: None,
                recall: None,
                webhook: None
            }]
        );
        assert!(
            logic
                .tick(&readings(&mix, &speech, &level, &on_air))
                .is_empty(),
            "not again"
        );
        speech.insert("vox".into(), true);
        assert_eq!(
            logic.tick(&readings(&mix, &speech, &level, &on_air)),
            vec![Action {
                signal: "music".into(),
                mute: Some(true),
                fader_db: None,
                recall: None,
                webhook: None
            }]
        );
        assert!(logic
            .tick(&readings(&mix, &speech, &level, &on_air))
            .is_empty());
        // logic_ticks_total and logic_actions_total{graph}.
        assert_eq!(logic.ticks, 4);
        assert_eq!(logic.actions, BTreeMap::from([("g".to_string(), 2)]));
    }

    #[test]
    fn an_edge_through_a_latch_toggles() {
        let mut mix = Mix::default();
        mix.logic.push(Graph {
            id: "g".into(),
            name: "G".into(),
            description: String::new(),
            enabled: true,
            dry_run: true,
            blocks: vec![
                block("b", BlockKind::Button),
                block("e", BlockKind::Edge),
                block("l", BlockKind::Latch),
            ],
            wires: vec![wire("b", "e", "in"), wire("e", "l", "set")],
        });
        let mut logic = Logic::default();
        let speech = BTreeMap::new();
        let level = BTreeMap::new();
        let on_air = BTreeSet::new();
        logic.tick(&readings(&mix, &speech, &level, &on_air));
        logic.set_button("g", "b", true);
        logic.tick(&readings(&mix, &speech, &level, &on_air));
        // The edge publishes one tick later; the latch takes it the tick after.
        assert_eq!(logic.values["g"]["e"], 1.0);
        logic.tick(&readings(&mix, &speech, &level, &on_air));
        assert_eq!(logic.values["g"]["l"], 1.0);
        assert_eq!(logic.values["g"]["e"], 0.0, "the edge lasts one tick");
        logic.set_button("g", "b", false);
        logic.tick(&readings(&mix, &speech, &level, &on_air));
        assert_eq!(logic.values["g"]["l"], 1.0, "the latch holds");
    }

    #[test]
    fn compare_and_fader() {
        let mut mix = Mix::default();
        mix.strips.push(strip("vox", Format::Mono));
        mix.strips[0].fader_db = Db(-20.0);
        mix.logic.push(Graph {
            id: "g".into(),
            name: "G".into(),
            description: String::new(),
            enabled: true,
            dry_run: false,
            blocks: vec![
                block(
                    "f",
                    BlockKind::Fader {
                        signal: "vox".into(),
                    },
                ),
                block("c", BlockKind::Const { value: -10.0 }),
                block("cmp", BlockKind::Compare { op: CompareOp::Lt }),
                block(
                    "set",
                    BlockKind::SetFader {
                        signal: "vox".into(),
                    },
                ),
            ],
            wires: vec![
                wire("f", "cmp", "a"),
                wire("c", "cmp", "b"),
                wire("cmp", "set", "level"),
            ],
        });
        // compare out is 1 (fader -20 < -10), and set-fader sets the fader to
        // that 1: the wiring is what the graph asked.
        let mut logic = Logic::default();
        let speech = BTreeMap::new();
        let level = BTreeMap::new();
        let on_air = BTreeSet::new();
        let a = logic.tick(&readings(&mix, &speech, &level, &on_air));
        assert_eq!(logic.values["g"]["cmp"], 1.0);
        assert_eq!(a[0].fader_db, Some(1.0));
        assert_eq!(clamp_fader(f64::NEG_INFINITY).0, f64::NEG_INFINITY);
        assert_eq!(clamp_fader(30.0).0, crate::mix::model::MAX_FADER_DB);
    }

    #[test]
    fn tally_on_air_mutes_when_the_lamp_is_lit() {
        let mut mix = Mix::default();
        mix.strips.push(strip("music", Format::Mono));
        mix.logic.push(Graph {
            id: "g".into(),
            name: "G".into(),
            description: String::new(),
            enabled: true,
            dry_run: false,
            blocks: vec![
                block("t", BlockKind::Tally { index: 5 }),
                block(
                    "m",
                    BlockKind::SetMute {
                        signal: "music".into(),
                    },
                ),
            ],
            wires: vec![wire("t", "m", "in")],
        });
        let mut logic = Logic::default();
        let speech = BTreeMap::new();
        let level = BTreeMap::new();
        let off = BTreeSet::new();
        let on = BTreeSet::from([5]);
        assert_eq!(logic.values.get("g"), None);
        let first = logic.tick(&readings(&mix, &speech, &level, &off));
        assert_eq!(logic.values["g"]["t"], 0.0);
        assert_eq!(first[0].mute, Some(false));
        let lit = logic.tick(&readings(&mix, &speech, &level, &on));
        assert_eq!(logic.values["g"]["t"], 1.0);
        assert_eq!(lit[0].mute, Some(true));
        assert!(logic.tick(&readings(&mix, &speech, &level, &on)).is_empty());
        let dark = logic.tick(&readings(&mix, &speech, &level, &off));
        assert_eq!(logic.values["g"]["t"], 0.0);
        assert_eq!(dark[0].mute, Some(false));
    }

    #[test]
    fn a_timer_holds_after_its_input_rises_and_a_new_rise_restarts_it() {
        let mut mix = Mix::default();
        mix.logic.push(Graph {
            id: "g".into(),
            name: "G".into(),
            description: String::new(),
            enabled: true,
            dry_run: true,
            blocks: vec![
                block("b", BlockKind::Button),
                block("t", BlockKind::Timer { ms: 30.0 }),
            ],
            wires: vec![wire("b", "t", "in")],
        });
        let mut logic = Logic::default();
        let speech = BTreeMap::new();
        let level = BTreeMap::new();
        let on_air = BTreeSet::new();
        let tick = |logic: &mut Logic, mix: &Mix| {
            logic.tick(&readings(mix, &speech, &level, &on_air));
            logic.values["g"]["t"]
        };
        assert_eq!(tick(&mut logic, &mix), 0.0);
        logic.set_button("g", "b", true);
        assert_eq!(tick(&mut logic, &mix), 1.0);
        assert_eq!(tick(&mut logic, &mix), 1.0);
        logic.set_button("g", "b", false);
        assert_eq!(tick(&mut logic, &mix), 1.0, "the pulse outlasts the input");
        assert_eq!(tick(&mut logic, &mix), 0.0);
        logic.set_button("g", "b", true);
        assert_eq!(tick(&mut logic, &mix), 1.0);
        logic.set_button("g", "b", false);
        assert_eq!(tick(&mut logic, &mix), 1.0);
        logic.set_button("g", "b", true);
        assert_eq!(tick(&mut logic, &mix), 1.0, "a rise starts the 30 ms again");
        assert_eq!(tick(&mut logic, &mix), 1.0);
        assert_eq!(tick(&mut logic, &mix), 1.0);
        assert_eq!(tick(&mut logic, &mix), 0.0);
        assert_eq!(pulse_ticks(0.0), 0.0);
        assert_eq!(pulse_ticks(11.0), 2.0);
    }

    #[test]
    fn above_holds_until_the_level_falls_by_the_hysteresis() {
        let mut mix = Mix::default();
        mix.strips.push(strip("vox", Format::Mono));
        mix.logic.push(Graph {
            id: "g".into(),
            name: "G".into(),
            description: String::new(),
            enabled: true,
            dry_run: true,
            blocks: vec![block(
                "a",
                BlockKind::Above {
                    signal: "vox".into(),
                    threshold_db: -40.0,
                    hysteresis_db: 6.0,
                },
            )],
            wires: vec![],
        });
        let mut logic = Logic::default();
        let speech = BTreeMap::new();
        let on_air = BTreeSet::new();
        let tick = |logic: &mut Logic, db: f64| {
            let level = BTreeMap::from([("vox".into(), db)]);
            logic.tick(&readings(&mix, &speech, &level, &on_air));
            logic.values["g"]["a"]
        };
        assert_eq!(tick(&mut logic, -50.0), 0.0);
        assert_eq!(tick(&mut logic, -40.0), 1.0);
        assert_eq!(tick(&mut logic, -45.0), 1.0, "still inside the hysteresis");
        assert_eq!(tick(&mut logic, -46.0), 1.0, "the floor still holds");
        assert_eq!(tick(&mut logic, -47.0), 0.0);
        assert_eq!(tick(&mut logic, -40.0), 1.0);
    }

    #[test]
    fn a_recall_fires_when_its_input_rises() {
        let mut mix = Mix::default();
        mix.logic.push(Graph {
            id: "g".into(),
            name: "G".into(),
            description: String::new(),
            enabled: true,
            dry_run: false,
            blocks: vec![
                block("b", BlockKind::Button),
                block(
                    "r",
                    BlockKind::Recall {
                        snapshot: "show".into(),
                    },
                ),
            ],
            wires: vec![wire("b", "r", "in")],
        });
        let mut logic = Logic::default();
        let speech = BTreeMap::new();
        let level = BTreeMap::new();
        let on_air = BTreeSet::new();
        let tick = |logic: &mut Logic| logic.tick(&readings(&mix, &speech, &level, &on_air));
        assert!(
            tick(&mut logic).is_empty(),
            "off at the start does not recall"
        );
        logic.set_button("g", "b", true);
        let rose = tick(&mut logic);
        assert_eq!(rose.len(), 1);
        assert_eq!(rose[0].recall.as_deref(), Some("show"));
        assert!(tick(&mut logic).is_empty(), "holding does not recall again");
        logic.set_button("g", "b", false);
        assert!(tick(&mut logic).is_empty());
        logic.set_button("g", "b", true);
        assert_eq!(tick(&mut logic)[0].recall.as_deref(), Some("show"));
    }

    #[test]
    fn xor_schedule_and_a_webhook_on_the_rise() {
        let mut mix = Mix::default();
        mix.logic.push(Graph {
            id: "g".into(),
            name: "G".into(),
            description: String::new(),
            enabled: true,
            dry_run: false,
            blocks: vec![
                block("a", BlockKind::Const { value: 1.0 }),
                block("b", BlockKind::Const { value: 1.0 }),
                block("x", BlockKind::Xor),
                block(
                    "s",
                    BlockKind::Schedule {
                        start_minute: 22 * 60,
                        end_minute: 6 * 60,
                    },
                ),
                block("btn", BlockKind::Button),
                block(
                    "w",
                    BlockKind::Webhook {
                        url: "https://example.test/hook".into(),
                    },
                ),
            ],
            wires: vec![
                wire("a", "x", "a"),
                wire("b", "x", "b"),
                wire("btn", "w", "in"),
            ],
        });
        let mut logic = Logic::default();
        let speech = BTreeMap::new();
        let level = BTreeMap::new();
        let on_air = BTreeSet::new();
        logic.tick(&readings_at(&mix, &speech, &level, &on_air, 23 * 60));
        assert_eq!(logic.values["g"]["x"], 0.0, "both on is not xor");
        assert_eq!(logic.values["g"]["s"], 1.0, "23:00 is inside 22:00–06:00");
        logic.tick(&readings_at(&mix, &speech, &level, &on_air, 12 * 60));
        assert_eq!(logic.values["g"]["s"], 0.0);
        assert!(logic
            .tick(&readings(&mix, &speech, &level, &on_air))
            .iter()
            .all(|a| a.webhook.is_none()));
        logic.set_button("g", "btn", true);
        let rose = logic.tick(&readings(&mix, &speech, &level, &on_air));
        assert_eq!(
            rose[0].webhook.as_deref(),
            Some("https://example.test/hook")
        );
        assert!(in_schedule(0, 22 * 60, 6 * 60));
        assert!(in_schedule(10 * 60, 9 * 60, 17 * 60));
        assert!(!in_schedule(8 * 60, 9 * 60, 17 * 60));
    }
}
