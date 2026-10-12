//! Show data besides the signal graph (§12.1, §12.5): layouts (user-defined strip
//! order and pages) and snapshots (parameter states with recall filters and safes).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::model::{Db, Mix};

/// A console layout: named pages of signals in the operator's order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Layout {
    pub name: String,
    pub pages: Vec<Page>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Page {
    pub name: String,
    pub signals: Vec<String>,
}

/// One signal's parameters in a snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignalState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trim_db: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub polarity: Option<Vec<bool>>,
    pub fader_db: Db,
    pub mute: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pan: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<f64>,
    /// Every chain node as JSON (its id, type, bypass and parameters).
    #[serde(default)]
    pub chain: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SendState {
    pub from: String,
    pub to: String,
    pub gain_db: Db,
    pub mute: bool,
    pub phase: bool,
}

/// Parameter states within a show.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub id: String,
    pub name: String,
    /// Seconds since 1970 (UTC) when it was taken.
    pub time: u64,
    pub signals: BTreeMap<String, SignalState>,
    pub sends: Vec<SendState>,
}

/// Parameter classes a recall can be limited to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Class {
    /// Trim and polarity.
    Trim,
    Fader,
    Mute,
    /// Pan, balance and width.
    Pan,
    /// The parameters (and bypass) of chain nodes.
    Chain,
    Sends,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Section {
    Strips,
    Buses,
}

/// What a recall touches; empty fields mean everything.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Filter {
    #[serde(default)]
    pub signals: Option<Vec<String>>,
    #[serde(default)]
    pub sections: Option<Vec<Section>>,
    #[serde(default)]
    pub classes: Option<Vec<Class>>,
}

impl Filter {
    fn class(&self, c: Class) -> bool {
        self.classes.as_ref().is_none_or(|cs| cs.contains(&c))
    }
}

fn node_json(p: &super::chain::Proc) -> Value {
    serde_json::to_value(p).unwrap_or(Value::Null)
}

/// The current parameter state of `mix`.
pub fn capture(mix: &Mix, id: String, name: String, time: u64) -> Snapshot {
    let mut signals = BTreeMap::new();
    for s in &mix.strips {
        signals.insert(
            s.id.clone(),
            SignalState {
                trim_db: Some(s.trim_db),
                polarity: Some(s.polarity.clone()),
                fader_db: s.fader_db,
                mute: s.mute,
                pan: Some(s.pan),
                width: Some(s.width),
                chain: s.chain.iter().map(node_json).collect(),
            },
        );
    }
    for b in &mix.buses {
        signals.insert(
            b.id.clone(),
            SignalState {
                trim_db: None,
                polarity: None,
                fader_db: b.fader_db,
                mute: b.mute,
                pan: None,
                width: None,
                chain: b.chain.iter().map(node_json).collect(),
            },
        );
    }
    let sends = mix
        .sends
        .iter()
        .map(|s| SendState {
            from: s.from.clone(),
            to: s.to.clone(),
            gain_db: s.gain_db,
            mute: s.mute,
            phase: s.phase,
        })
        .collect();
    Snapshot {
        id,
        name,
        time,
        signals,
        sends,
    }
}

/// Apply `snap` to `mix` within `filter`, leaving recall-safe signals alone. Only
/// parameters change: signals, sends and nodes that no longer exist (or whose
/// node type differs) are skipped. Returns the number of signals touched.
pub fn recall(mix: &mut Mix, snap: &Snapshot, filter: &Filter) -> usize {
    let wanted = |id: &str, bus: bool, safe: bool| {
        !safe
            && filter
                .signals
                .as_ref()
                .is_none_or(|s| s.iter().any(|x| x == id))
            && filter.sections.as_ref().is_none_or(|ss| {
                ss.contains(if bus {
                    &Section::Buses
                } else {
                    &Section::Strips
                })
            })
    };
    let restore_chain = |chain: &mut Vec<super::chain::Proc>, saved: &[Value]| {
        for v in saved {
            let Ok(p) = serde_json::from_value::<super::chain::Proc>(v.clone()) else {
                continue;
            };
            if let Some(cur) = chain
                .iter_mut()
                .find(|c| c.id == p.id && c.kind.type_name() == p.kind.type_name())
            {
                *cur = p;
            }
        }
    };
    let mut touched = Vec::new();
    for s in &mut mix.strips {
        let Some(st) = snap.signals.get(&s.id) else {
            continue;
        };
        if !wanted(&s.id, false, s.recall_safe) {
            continue;
        }
        if filter.class(Class::Trim) {
            if let Some(t) = st.trim_db {
                s.trim_db = t;
            }
            if let Some(p) = &st.polarity {
                if p.len() == s.polarity.len() {
                    s.polarity = p.clone();
                }
            }
        }
        if filter.class(Class::Fader) {
            s.fader_db = st.fader_db;
        }
        if filter.class(Class::Mute) {
            s.mute = st.mute;
        }
        if filter.class(Class::Pan) {
            if let Some(p) = st.pan {
                s.pan = p;
            }
            if let Some(w) = st.width {
                s.width = w;
            }
        }
        if filter.class(Class::Chain) {
            restore_chain(&mut s.chain, &st.chain);
        }
        touched.push(s.id.clone());
    }
    for b in &mut mix.buses {
        let Some(st) = snap.signals.get(&b.id) else {
            continue;
        };
        if !wanted(&b.id, true, b.recall_safe) {
            continue;
        }
        if filter.class(Class::Fader) {
            b.fader_db = st.fader_db;
        }
        if filter.class(Class::Mute) {
            b.mute = st.mute;
        }
        if filter.class(Class::Chain) {
            restore_chain(&mut b.chain, &st.chain);
        }
        touched.push(b.id.clone());
    }
    if filter.class(Class::Sends) {
        for s in &mut mix.sends {
            if !touched.contains(&s.from) {
                continue;
            }
            if let Some(st) = snap.sends.iter().find(|x| x.from == s.from && x.to == s.to) {
                s.gain_db = st.gain_db;
                s.mute = st.mute;
                s.phase = st.phase;
            }
        }
    }
    touched.len()
}

/// Check the show data against `mix`'s signals.
pub fn validate(mix: &Mix) -> Result<(), String> {
    let name_ok = |n: &str| !n.trim().is_empty() && n.chars().count() <= 64;
    let mut names = std::collections::BTreeSet::new();
    for l in &mix.layouts {
        if !name_ok(&l.name) || !names.insert(l.name.as_str()) {
            return Err(format!(
                "layout name {:?}: 1 to 64 characters, unique",
                l.name
            ));
        }
        for p in &l.pages {
            if !name_ok(&p.name) {
                return Err(format!(
                    "layout {}: page names have 1 to 64 characters",
                    l.name
                ));
            }
            for s in &p.signals {
                if mix.kind(s).is_none() {
                    return Err(format!(
                        "layout {} page {}: signal {s} does not exist",
                        l.name, p.name
                    ));
                }
            }
        }
    }
    let mut ids = std::collections::BTreeSet::new();
    for s in &mix.snapshots {
        let id_ok = !s.id.is_empty()
            && s.id.len() <= 32
            && !s.id.starts_with('-')
            && s.id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if !id_ok {
            return Err(format!(
                "snapshot id {:?} must be 1 to 32 characters a-z, 0-9 and '-', not starting with '-'",
                s.id
            ));
        }
        if !ids.insert(s.id.as_str()) {
            return Err(format!("snapshot id {} is used twice", s.id));
        }
        if !name_ok(&s.name) {
            return Err(format!("snapshot {}: a name has 1 to 64 characters", s.id));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mix::model::{Bus, Db, Send, Strip, Tap};
    use crate::mix::Format;

    fn mix() -> Mix {
        let strip = |id: &str| {
            Strip {
            id: id.into(),
            name: id.into(),
            format: Format::Mono,
            patch: vec![None],
            trim_db: 0.0,
            polarity: vec![false],
            fader_db: Db(0.0),
            mute: false,
            pan: 0.0,
            width: 1.0,
            chain: serde_json::from_str(
                r#"[{"id": "eq", "type": "eq", "bands": [{"type": "bell", "freq_hz": 1000, "q": 1, "gain_db": 3}]}]"#,
            )
            .unwrap(),
            loudness: false,
            colour: None,
            recall_safe: false,
            tally_index: None,
            afv: None,
        }
        };
        let mut m = Mix::default();
        m.strips.push(strip("a"));
        m.strips.push(strip("b"));
        m.buses.push(Bus {
            id: "main".into(),
            name: "Main".into(),
            format: Format::Stereo,
            fader_db: Db(0.0),
            mute: false,
            chain: Vec::new(),
            loudness: false,
            compensate: true,
            colour: None,
            recall_safe: false,
        });
        for from in ["a", "b"] {
            m.sends.push(Send {
                from: from.into(),
                to: "main".into(),
                tap: Tap::PostPan,
                gain_db: Db(0.0),
                mute: false,
                phase: false,
                block_delay: false,
            });
        }
        m
    }

    #[test]
    fn capture_and_recall_with_filters_and_safes() {
        let mut m = mix();
        let snap = capture(&m, "s1".into(), "Opening".into(), 1);
        m.strips[0].fader_db = Db(-10.0);
        m.strips[0].mute = true;
        m.strips[1].fader_db = Db(-20.0);
        m.strips[1].recall_safe = true;
        m.sends[0].gain_db = Db(-6.0);
        m.strips[0].chain[0] = serde_json::from_str(
            r#"{"id": "eq", "type": "eq", "bands": [{"type": "bell", "freq_hz": 2000, "q": 2, "gain_db": -4}]}"#,
        )
        .unwrap();
        m.buses[0].mute = true;

        // Faders of strips only: a's fader back, a's mute kept, b safe, bus untouched.
        let mut r = m.clone();
        let n = recall(
            &mut r,
            &snap,
            &Filter {
                classes: Some(vec![Class::Fader]),
                sections: Some(vec![Section::Strips]),
                ..Default::default()
            },
        );
        assert_eq!(n, 1);
        assert_eq!(r.strips[0].fader_db, Db(0.0));
        assert!(r.strips[0].mute);
        assert_eq!(r.strips[1].fader_db, Db(-20.0));
        assert!(r.buses[0].mute);
        assert_eq!(r.sends[0].gain_db, Db(-6.0));

        // Everything: chain, sends and the bus come back too.
        let mut r = m.clone();
        recall(&mut r, &snap, &Filter::default());
        assert_eq!(r.strips[0].chain, mix().strips[0].chain);
        assert_eq!(r.sends[0].gain_db, Db(0.0));
        assert!(!r.buses[0].mute && !r.strips[0].mute);
        assert_eq!(r.strips[1].fader_db, Db(-20.0), "recall-safe");

        // Nodes that changed type are skipped.
        let mut r = m.clone();
        r.strips[0].chain[0] = serde_json::from_str(
            r#"{"id": "eq", "type": "filter", "mode": "high-pass", "freq_hz": 80, "slope_db_per_oct": 12}"#,
        )
        .unwrap();
        let before = r.strips[0].chain.clone();
        recall(&mut r, &snap, &Filter::default());
        assert_eq!(r.strips[0].chain, before);
    }

    #[test]
    fn layouts_are_checked() {
        let mut m = mix();
        m.layouts.push(Layout {
            name: "Main".into(),
            pages: vec![Page {
                name: "Talent".into(),
                signals: vec!["b".into(), "a".into(), "main".into()],
            }],
        });
        assert_eq!(validate(&m), Ok(()));
        m.layouts[0].pages[0].signals.push("nope".into());
        assert!(validate(&m).unwrap_err().contains("nope"));
    }
}
