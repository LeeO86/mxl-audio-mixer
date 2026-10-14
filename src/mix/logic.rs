//! Logic graphs (§11.1): blocks with typed ports, wired together, saved in the
//! show. The runtime (`crate::logic`) evaluates them at 100 Hz.
//!
//! A value is a number; a boolean is 0 or 1. A wire's two ports must be the
//! same kind of port (an output into an input) and a port takes one wire.
//! A loop is allowed only through a block that remembers (`edge`, `latch`,
//! `timer`): those publish last tick's value, so the loop is not combinational.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::model::Mix;
use super::vca::{check_id, check_name};

pub const MAX_GRAPHS: usize = 32;
pub const MAX_BLOCKS: usize = 64;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Graph {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Values are computed; actions do not run.
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Block {
    pub id: String,
    #[serde(flatten)]
    pub kind: BlockKind,
    /// Where the canvas will draw it.
    #[serde(default)]
    pub x: f64,
    #[serde(default)]
    pub y: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum BlockKind {
    /// A fixed number (`out`).
    Const {
        value: f64,
    },
    /// A soft key (`out` is 1 while pressed).
    Button,
    /// The signal's mute (`out`).
    Mute {
        signal: String,
    },
    /// The signal's fader, dB (`out`).
    Fader {
        signal: String,
    },
    /// The signal's voice detection (`out` is 1 while it speaks).
    Vad {
        signal: String,
    },
    /// The signal's peak level, dBFS (`out`).
    Level {
        signal: String,
    },
    /// 1 while TSL tally `index` is on air (the left lamp).
    Tally {
        index: u16,
    },
    /// 1 for `ms` milliseconds after `in` rises, rounded up to the 10 ms tick.
    /// A rise while it is still on starts the duration again.
    Timer {
        ms: f64,
    },
    /// 1 once the signal's peak reaches `threshold_db`, until it falls below
    /// that by `hysteresis_db`.
    Above {
        signal: String,
        threshold_db: f64,
        hysteresis_db: f64,
    },
    /// Recalls snapshot `snapshot` when `in` rises.
    Recall {
        snapshot: String,
    },
    And,
    Or,
    Not,
    Compare {
        op: CompareOp,
    },
    /// 1 for one tick when its input rose to 1.
    Edge,
    /// Stays 1 after `set`, 0 after `reset` (reset wins).
    Latch,
    /// Sets the signal's mute from `in`.
    SetMute {
        signal: String,
    },
    /// Sets the signal's fader (dB) from `level`.
    SetFader {
        signal: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CompareOp {
    Gt,
    Lt,
    Eq,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Wire {
    pub from_block: String,
    pub from_port: String,
    pub to_block: String,
    pub to_port: String,
}

impl BlockKind {
    /// Output ports, then input ports.
    pub fn ports(&self) -> (&'static [&'static str], &'static [&'static str]) {
        match self {
            Self::Const { .. }
            | Self::Button
            | Self::Mute { .. }
            | Self::Fader { .. }
            | Self::Vad { .. }
            | Self::Level { .. }
            | Self::Tally { .. }
            | Self::Above { .. } => (&["out"], &[]),
            Self::And | Self::Or | Self::Compare { .. } => (&["out"], &["a", "b"]),
            Self::Not | Self::Edge | Self::Timer { .. } => (&["out"], &["in"]),
            Self::Latch => (&["out"], &["set", "reset"]),
            Self::SetMute { .. } | Self::Recall { .. } => (&[], &["in"]),
            Self::SetFader { .. } => (&[], &["level"]),
        }
    }

    /// Remembers, so a loop through it is not combinational.
    pub fn remembers(&self) -> bool {
        matches!(self, Self::Edge | Self::Latch | Self::Timer { .. })
    }

    pub fn signal(&self) -> Option<&str> {
        match self {
            Self::Mute { signal }
            | Self::Fader { signal }
            | Self::Vad { signal }
            | Self::Level { signal }
            | Self::SetMute { signal }
            | Self::SetFader { signal }
            | Self::Above { signal, .. } => Some(signal),
            _ => None,
        }
    }
}

impl Mix {
    pub fn logic_graph(&self, id: &str) -> Option<&Graph> {
        self.logic.iter().find(|g| g.id == id)
    }

    /// Signals a logic graph reads with voice detection.
    pub fn logic_vad_signals(&self) -> impl Iterator<Item = &str> {
        self.logic.iter().flat_map(|g| {
            g.blocks.iter().filter_map(|b| match &b.kind {
                BlockKind::Vad { signal } => Some(signal.as_str()),
                _ => None,
            })
        })
    }

    pub fn remove_from_logic(&mut self, id: &str) {
        for g in &mut self.logic {
            let gone: Vec<String> = g
                .blocks
                .iter()
                .filter(|b| b.kind.signal() == Some(id))
                .map(|b| b.id.clone())
                .collect();
            g.blocks.retain(|b| b.kind.signal() != Some(id));
            g.wires
                .retain(|w| !gone.contains(&w.from_block) && !gone.contains(&w.to_block));
        }
    }
}

/// `true` when the wires among the blocks that do not remember form a loop.
pub fn combinational_loop(blocks: &[Block], wires: &[Wire]) -> bool {
    let stateful: BTreeSet<&str> = blocks
        .iter()
        .filter(|b| b.kind.remembers())
        .map(|b| b.id.as_str())
        .collect();
    let mut out: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for w in wires {
        if stateful.contains(w.to_block.as_str()) {
            continue;
        }
        out.entry(w.from_block.as_str())
            .or_default()
            .push(&w.to_block);
    }
    let mut colour: BTreeMap<&str, u8> = BTreeMap::new();
    fn visit<'a>(
        id: &'a str,
        out: &BTreeMap<&str, Vec<&'a str>>,
        colour: &mut BTreeMap<&'a str, u8>,
    ) -> bool {
        match colour.get(id).copied().unwrap_or(0) {
            1 => return true,
            2 => return false,
            _ => {}
        }
        colour.insert(id, 1);
        for n in out.get(id).into_iter().flatten() {
            if visit(n, out, colour) {
                return true;
            }
        }
        colour.insert(id, 2);
        false
    }
    blocks.iter().any(|b| visit(&b.id, &out, &mut colour))
}

pub fn validate(mix: &Mix) -> Result<(), String> {
    if mix.logic.len() > MAX_GRAPHS {
        return Err(format!("at most {MAX_GRAPHS} logic graphs"));
    }
    let mut ids = BTreeSet::new();
    for g in &mix.logic {
        check_id("logic", &g.id)?;
        if !ids.insert(g.id.as_str()) {
            return Err(format!("logic id {} is used twice", g.id));
        }
        check_name(&format!("logic {}", g.id), &g.name)?;
        if g.blocks.len() > MAX_BLOCKS {
            return Err(format!("logic {}: at most {MAX_BLOCKS} blocks", g.id));
        }
        let mut blocks = BTreeSet::new();
        for b in &g.blocks {
            check_id(&format!("logic {} block", g.id), &b.id)?;
            if !blocks.insert(b.id.as_str()) {
                return Err(format!("logic {}: block {} twice", g.id, b.id));
            }
            if let Some(s) = b.kind.signal() {
                if mix.kind(s).is_none() {
                    return Err(format!("logic {}: {} is not a strip or bus", g.id, s));
                }
            }
            if let BlockKind::Const { value } = b.kind {
                if !value.is_finite() {
                    return Err(format!(
                        "logic {}: const {} is not a finite number",
                        g.id, b.id
                    ));
                }
            }
            if let BlockKind::Timer { ms } = b.kind {
                if !ms.is_finite() || !(0.0..=86_400_000.0).contains(&ms) {
                    return Err(format!(
                        "logic {}: timer {} must be 0 to 86400000 ms",
                        g.id, b.id
                    ));
                }
            }
            if let BlockKind::Above {
                threshold_db,
                hysteresis_db,
                ..
            } = b.kind
            {
                if !threshold_db.is_finite() || !(-120.0..=20.0).contains(&threshold_db) {
                    return Err(format!(
                        "logic {}: above {} threshold must be -120 to 20 dB",
                        g.id, b.id
                    ));
                }
                if !hysteresis_db.is_finite() || !(0.0..=60.0).contains(&hysteresis_db) {
                    return Err(format!(
                        "logic {}: above {} hysteresis must be 0 to 60 dB",
                        g.id, b.id
                    ));
                }
            }
            if let BlockKind::Recall { snapshot } = &b.kind {
                check_id(&format!("logic {} snapshot", g.id), snapshot)?;
            }
        }
        let mut taken = BTreeSet::new();
        for w in &g.wires {
            let from = g
                .blocks
                .iter()
                .find(|b| b.id == w.from_block)
                .ok_or_else(|| format!("logic {}: no block {}", g.id, w.from_block))?;
            let to = g
                .blocks
                .iter()
                .find(|b| b.id == w.to_block)
                .ok_or_else(|| format!("logic {}: no block {}", g.id, w.to_block))?;
            let (outs, _) = from.kind.ports();
            let (_, ins) = to.kind.ports();
            if !outs.contains(&w.from_port.as_str()) {
                return Err(format!(
                    "logic {}: {} has no output {}",
                    g.id, w.from_block, w.from_port
                ));
            }
            if !ins.contains(&w.to_port.as_str()) {
                return Err(format!(
                    "logic {}: {} has no input {}",
                    g.id, w.to_block, w.to_port
                ));
            }
            if !taken.insert((w.to_block.as_str(), w.to_port.as_str())) {
                return Err(format!(
                    "logic {}: {}.{} has two wires",
                    g.id, w.to_block, w.to_port
                ));
            }
        }
        if combinational_loop(&g.blocks, &g.wires) {
            return Err(format!(
                "logic {}: a loop needs an edge, a latch or a timer in it",
                g.id
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mix::model::tests::strip;
    use crate::mix::Format;

    fn graph(blocks: Vec<Block>, wires: Vec<Wire>) -> Graph {
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

    fn block(id: &str, kind: BlockKind) -> Block {
        Block {
            id: id.into(),
            kind,
            x: 0.0,
            y: 0.0,
        }
    }

    #[test]
    fn a_loop_needs_memory() {
        let blocks = vec![block("a", BlockKind::And), block("b", BlockKind::Or)];
        let wires = vec![
            Wire {
                from_block: "a".into(),
                from_port: "out".into(),
                to_block: "b".into(),
                to_port: "a".into(),
            },
            Wire {
                from_block: "b".into(),
                from_port: "out".into(),
                to_block: "a".into(),
                to_port: "a".into(),
            },
        ];
        assert!(combinational_loop(&blocks, &wires));
        let mut with = blocks.clone();
        with.push(block("e", BlockKind::Edge));
        let through = vec![
            Wire {
                from_block: "a".into(),
                from_port: "out".into(),
                to_block: "e".into(),
                to_port: "in".into(),
            },
            Wire {
                from_block: "e".into(),
                from_port: "out".into(),
                to_block: "b".into(),
                to_port: "a".into(),
            },
            Wire {
                from_block: "b".into(),
                from_port: "out".into(),
                to_block: "a".into(),
                to_port: "a".into(),
            },
        ];
        assert!(!combinational_loop(&with, &through));
        let _ = graph(blocks, wires);
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
            vec![Wire {
                from_block: "v".into(),
                from_port: "out".into(),
                to_block: "m".into(),
                to_port: "in".into(),
            }],
        ));
        assert_eq!(validate(&m), Ok(()));
        assert_eq!(m.logic_vad_signals().collect::<Vec<_>>(), ["vox"]);
        m.remove_signal("vox");
        assert!(m.logic[0].blocks.is_empty());
        let bad = |f: &dyn Fn(&mut Mix)| {
            let mut x = Mix::default();
            x.strips.push(strip("vox", Format::Mono));
            x.logic.push(graph(vec![], vec![]));
            f(&mut x);
            validate(&x).is_err()
        };
        assert!(bad(&|m| m.logic[0].blocks.push(block(
            "x",
            BlockKind::Mute {
                signal: "nope".into()
            }
        ))));
        assert!(bad(&|m| {
            m.logic[0].blocks.push(block("a", BlockKind::And));
            m.logic[0].wires.push(Wire {
                from_block: "a".into(),
                from_port: "out".into(),
                to_block: "a".into(),
                to_port: "a".into(),
            });
        }));
        assert!(bad(&|m| m.logic[0]
            .blocks
            .push(block("t", BlockKind::Timer { ms: -1.0 }))));
        assert!(bad(&|m| {
            m.logic[0].blocks.push(block(
                "a",
                BlockKind::Above {
                    signal: "vox".into(),
                    threshold_db: 40.0,
                    hysteresis_db: 6.0,
                },
            ));
        }));
        assert!(bad(&|m| {
            m.logic[0].blocks.push(block(
                "r",
                BlockKind::Recall {
                    snapshot: "Show".into(),
                },
            ));
        }));
        let mut timed = Mix::default();
        timed.logic.push(graph(
            vec![
                block("t", BlockKind::Timer { ms: 10.0 }),
                block("n", BlockKind::Not),
            ],
            vec![
                Wire {
                    from_block: "t".into(),
                    from_port: "out".into(),
                    to_block: "n".into(),
                    to_port: "in".into(),
                },
                Wire {
                    from_block: "n".into(),
                    from_port: "out".into(),
                    to_block: "t".into(),
                    to_port: "in".into(),
                },
            ],
        ));
        assert_eq!(validate(&timed), Ok(()));
    }
}
