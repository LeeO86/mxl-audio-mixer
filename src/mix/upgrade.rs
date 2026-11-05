//! Saved show state of older versions brought to this one's model (1.6.0).
//!
//! The surround panner is no processing node any more (A7): a `surround-pan`
//! node of 1.5 becomes the strip's `surround` position (its layout is dropped:
//! the panner now pans into each surround bus in that bus's layout). A bypassed
//! node, a second one, or one on a bus (where it did nothing) is removed. In
//! snapshots the node's saved parameters become the snapshot's surround
//! position. A logic `set-param` block on a converted node has no target any
//! more: it is removed with its wires.

use super::chain::{Proc, ProcKind};
use super::model::{surround_entries, SurroundPos};
use super::Mix;

fn position(kind: &ProcKind) -> Option<SurroundPos> {
    match kind {
        ProcKind::SurroundPan {
            azimuth_deg,
            elevation_deg,
            spread_pct,
            divergence_pct,
            lfe_db,
            ..
        } => Some(SurroundPos {
            azimuth_deg: *azimuth_deg,
            elevation_deg: *elevation_deg,
            spread_pct: *spread_pct,
            divergence_pct: *divergence_pct,
            lfe_db: *lfe_db,
        }),
        _ => None,
    }
}

/// Take the surround panner nodes out of `chain`: the first active one's
/// position, and the ids of all removed.
fn take_panners(chain: &mut Vec<Proc>) -> (Option<SurroundPos>, Vec<String>) {
    let mut found = None;
    let mut removed = Vec::new();
    chain.retain(|p| {
        if !matches!(p.kind, ProcKind::SurroundPan { .. }) {
            return true;
        }
        if !p.bypass && found.is_none() {
            found = position(&p.kind);
        }
        removed.push(p.id.clone());
        false
    });
    (found, removed)
}

/// Bring `mix` to this version's model. Returns what changed, one line each
/// (for the log), and the paths of the logic blocks removed (as the tolerant
/// load lists what it dropped, D279).
pub fn upgrade(mix: &mut Mix) -> (Vec<String>, Vec<String>) {
    let mut notes = Vec::new();
    // (signal, node) of every removed panner.
    let mut gone: Vec<(String, String)> = Vec::new();
    for s in &mut mix.strips {
        let (pos, removed) = take_panners(&mut s.chain);
        let one = surround_entries(s.format) == 1;
        match pos {
            Some(p) if one && s.surround.is_empty() => {
                s.surround = vec![Some(p)];
                notes.push(format!(
                    "strip {}: surround panner node {} is the strip's surround pan now",
                    s.id, removed[0]
                ));
            }
            _ if !removed.is_empty() => notes.push(format!(
                "strip {}: surround panner node{} {} removed (bypassed or not used)",
                s.id,
                if removed.len() == 1 { "" } else { "s" },
                removed.join(", ")
            )),
            _ => {}
        }
        gone.extend(removed.into_iter().map(|n| (s.id.clone(), n)));
    }
    for b in &mut mix.buses {
        let (_, removed) = take_panners(&mut b.chain);
        if !removed.is_empty() {
            notes.push(format!(
                "bus {}: surround panner node {} removed (it did nothing on a bus)",
                b.id,
                removed.join(", ")
            ));
        }
        gone.extend(removed.into_iter().map(|n| (b.id.clone(), n)));
    }
    for snap in &mut mix.snapshots {
        for (id, st) in snap.signals.iter_mut() {
            let mut pos = None;
            st.chain.retain(|v| {
                let Ok(p) = serde_json::from_value::<Proc>(v.clone()) else {
                    return true;
                };
                if !matches!(p.kind, ProcKind::SurroundPan { .. }) {
                    return true;
                }
                if !p.bypass && pos.is_none() {
                    pos = position(&p.kind);
                }
                false
            });
            let one = mix
                .strips
                .iter()
                .find(|s| &s.id == id)
                .is_some_and(|s| surround_entries(s.format) == 1);
            if let (Some(p), true, true) = (pos, one, st.surround.is_none()) {
                st.surround = Some(vec![Some(p)]);
            }
        }
    }
    let mut dropped = Vec::new();
    if !gone.is_empty() {
        let names = |signal: &str, node: &str| gone.iter().any(|(s, n)| s == signal && n == node);
        let mut clean = |what: &str,
                         i: usize,
                         blocks: &mut Vec<super::logic::Block>,
                         wires: &mut Vec<super::logic::Wire>| {
            let mut ids = Vec::new();
            let mut k = 0;
            blocks.retain(|b| {
                let hit = matches!(&b.kind, super::logic::BlockKind::SetParam { signal, node, .. } if names(signal, node));
                if hit {
                    ids.push(b.id.clone());
                    dropped.push(format!("{what}[{i}].blocks[{k}]"));
                }
                k += 1;
                !hit
            });
            wires.retain(|w| !ids.contains(&w.from_block) && !ids.contains(&w.to_block));
        };
        for (i, g) in mix.logic.iter_mut().enumerate() {
            clean("logic", i, &mut g.blocks, &mut g.wires);
        }
        for (i, m) in mix.logic_macros.iter_mut().enumerate() {
            clean("logic_macros", i, &mut m.blocks, &mut m.wires);
        }
    }
    (notes, dropped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mix::model::{Bus, Strip};
    use crate::mix::Format;
    use serde_json::json;

    /// A show of 1.5.1 with surround panner nodes (as 1.5.1 wrote it).
    fn show_151() -> serde_json::Value {
        let mut m = Mix::default();
        m.strips.push(Strip::new("vox", Format::Mono));
        m.strips.push(Strip::new("fx", Format::Stereo));
        m.strips.push(Strip::new("off", Format::Mono));
        m.buses.push(Bus::new("sur", Format::Surround51));
        let mut v = serde_json::to_value(&m).unwrap();
        v["strips"][0]["chain"] = json!([
            {"id": "eq-1", "type": "eq", "bands": [{"type": "bell", "freq_hz": 1000, "q": 1, "gain_db": 3}]},
            {"id": "surround-pan-1", "type": "surround-pan", "layout": "5.1", "azimuth_deg": 110,
             "spread_pct": 20, "divergence_pct": 0, "lfe_db": -6}
        ]);
        v["strips"][1]["chain"] = json!([
            {"id": "surround-pan-1", "type": "surround-pan", "layout": "7.1.4", "azimuth_deg": -45,
             "elevation_deg": 30, "spread_pct": 0, "divergence_pct": 0, "lfe_db": -100}
        ]);
        v["strips"][2]["chain"] = json!([
            {"id": "p", "type": "surround-pan", "bypass": true, "layout": "5.1", "azimuth_deg": 90,
             "spread_pct": 0, "divergence_pct": 0, "lfe_db": -100}
        ]);
        v["logic"] = json!([{
            "id": "g", "name": "G",
            "blocks": [
                {"id": "f", "type": "fader", "signal": "vox"},
                {"id": "sp", "type": "set-param", "signal": "vox", "node": "surround-pan-1", "param": "azimuth_deg"},
                {"id": "sq", "type": "set-param", "signal": "vox", "node": "eq-1", "param": "bands/0/gain_db"}
            ],
            "wires": [
                {"from_block": "f", "from_port": "out", "to_block": "sp", "to_port": "value"},
                {"from_block": "f", "from_port": "out", "to_block": "sq", "to_port": "value"}
            ]
        }]);
        v["snapshots"] = json!([{
            "id": "s1", "name": "S1", "time": 0, "sends": [],
            "signals": {"vox": {"fader_db": 0, "mute": false, "chain": [
                {"id": "surround-pan-1", "type": "surround-pan", "layout": "5.1", "azimuth_deg": -30,
                 "spread_pct": 0, "divergence_pct": 0, "lfe_db": -100}
            ]}}
        }]);
        v
    }

    #[test]
    fn a_151_show_loads_with_the_panner_as_the_strip_surround() {
        let (mut m, dropped) = crate::mix::tolerant::mix(show_151()).unwrap();
        assert_eq!(dropped, vec!["logic[0].blocks[1]".to_string()]);
        assert_eq!(m.validate(1, 48), Ok(()));
        let vox = m.strip("vox").unwrap();
        assert_eq!(vox.chain.len(), 1, "the EQ stays");
        let p = vox.surround[0].unwrap();
        assert_eq!((p.azimuth_deg, p.spread_pct, p.lfe_db), (110.0, 20.0, -6.0));
        // The layout goes; the height stays.
        let fx = m.strip("fx").unwrap();
        assert!(fx.chain.is_empty());
        assert_eq!(fx.surround[0].unwrap().elevation_deg, 30.0);
        // A bypassed one is removed: the strip pans L/R as it sounded.
        let off = m.strip("off").unwrap();
        assert!(off.chain.is_empty() && off.surround.is_empty());
        // The logic block on the node and its wire go, the other stays.
        assert_eq!(m.logic[0].blocks.len(), 2);
        assert_eq!(m.logic[0].wires.len(), 1);
        // The snapshot's saved node becomes its surround position.
        let st = &m.snapshots[0].signals["vox"];
        assert!(st.chain.is_empty());
        assert_eq!(st.surround.as_ref().unwrap()[0].unwrap().azimuth_deg, -30.0);
        // Again: nothing more to do.
        let (notes, dropped) = upgrade(&mut m);
        assert!(notes.is_empty() && dropped.is_empty());
        // It saves without the node and loads strictly.
        let back: Mix = serde_json::from_value(serde_json::to_value(&m).unwrap()).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn the_api_refuses_a_panner_node() {
        let mut m = Mix::default();
        m.strips.push(Strip::new("vox", Format::Mono));
        m.strips[0].chain = serde_json::from_value(json!([
            {"id": "p", "type": "surround-pan", "layout": "5.1", "azimuth_deg": 0}
        ]))
        .unwrap();
        let e = m.validate(1, 48).unwrap_err();
        assert!(e.contains("surround"), "{e}");
    }
}
