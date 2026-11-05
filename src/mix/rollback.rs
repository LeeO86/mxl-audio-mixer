//! Saved state that the release before can load (rollback 1.6 → 1.5.1, the
//! owner's rollback target, unchanged). 1.5.1 drops the chain nodes it does not
//! know (`fader`, `direct-out`, `insert`) and the fields it does not know (D279),
//! but a tap that names a dropped node makes its mix invalid, and a direct out
//! that is only a node would be gone (no MXL flow, no NMOS sender). So what is
//! written (`mix.json`, the show history, the export) is [`for_saving`]:
//!
//! - every strip's direct out `out` also as the pre-1.6 `outputs` entry, at the
//!   nearest point 1.5.1 has ([`legacy_at`]): same key, flow and sender;
//! - every tap (sends, outputs, chain keys, virtual outputs) that 1.5.1 cannot
//!   take or that sits after the fader as the nearest 1.5.1 tap in `tap`, and the
//!   exact one in `tap_exact` (next to it; for a virtual output beside `source`).
//!
//! [`restore`] undoes that on the JSON before it is parsed (load, history,
//! import): `tap_exact` back into `tap`, the legacy entries of `out` dropped.
//! In 1.5.1 a tap after the fader or a new node is approximate (the nodes after
//! the fader run before it there); the other direct outs and insert sends are
//! not published there.

use serde_json::Value;

use super::chain::{Category, Proc, ProcKind, MAIN_DIRECT_OUT};
use super::model::{Mix, Tap};

/// The field beside a rewritten tap with the exact one.
pub const TAP_EXACT: &str = "tap_exact";

/// A node 1.5.1 knows.
fn known(p: &Proc) -> bool {
    !matches!(
        p.kind,
        ProcKind::Fader {} | ProcKind::DirectOut { .. } | ProcKind::Insert { .. }
    )
}

/// The 1.5.1 tap nearest to the point before `chain[pos]` (`pos == len`: the
/// chain's end): post-fader after the fader node; pre-fader when only new nodes
/// lie between it and the fader; else after the nearest known node before it;
/// else the chain's start (pre-EQ or pre-dynamics where that is its input).
pub fn legacy_at(chain: &[Proc], pos: usize) -> Tap {
    let fader = chain.iter().position(|p| p.kind == ProcKind::Fader {});
    if fader.is_some_and(|f| pos > f) {
        return Tap::PostFader;
    }
    let end = fader.unwrap_or(chain.len());
    if chain[pos.min(end)..end].iter().all(|p| !known(p)) {
        return Tap::PreFader;
    }
    if let Some(p) = chain[..pos].iter().rev().find(|p| known(p)) {
        return Tap::After(p.id.clone());
    }
    let first = chain.iter().find(|p| known(p)).map(|p| p.kind.category());
    let has = |c: Category| chain.iter().any(|p| known(p) && p.kind.category() == c);
    match first {
        Some(Category::Dynamics) => Tap::PreDynamics,
        _ if !has(Category::Eq) || first == Some(Category::Eq) => Tap::PreEq,
        _ if !has(Category::Dynamics) => Tap::PreDynamics,
        _ => Tap::PreEq,
    }
}

/// The 1.5.1 tap for `tap` of `signal`, when it needs another one.
pub fn legacy_tap(mix: &Mix, signal: &str, tap: &Tap) -> Option<Tap> {
    let Tap::After(id) = tap else { return None };
    let chain = mix.chain(signal)?;
    let i = chain.iter().position(|p| &p.id == id)?;
    let fader = chain.iter().position(|p| p.kind == ProcKind::Fader {});
    if known(&chain[i]) && fader.is_none_or(|f| i < f) {
        return None;
    }
    Some(legacy_at(chain, i + 1))
}

/// Rewrite the tap at `obj[field]` of `signal` for 1.5.1, keeping the exact one.
fn rewrite(mix: &Mix, signal: &str, obj: &mut Value, field: &str, exact_into: Option<&mut Value>) {
    let Some(tap) = obj
        .get(field)
        .and_then(|t| serde_json::from_value::<Tap>(t.clone()).ok())
    else {
        return;
    };
    let Some(legacy) = legacy_tap(mix, signal, &tap) else {
        return;
    };
    obj[field] = serde_json::to_value(&legacy).unwrap_or_default();
    let exact = serde_json::to_value(&tap).unwrap_or_default();
    match exact_into {
        Some(o) => o[TAP_EXACT] = exact,
        None => obj[TAP_EXACT] = exact,
    }
}

/// Rewrite the keys of the chain nodes in `chain` (JSON).
fn rewrite_keys(mix: &Mix, chain: &mut Value) {
    for node in chain.as_array_mut().into_iter().flatten() {
        let Some(signal) = node
            .get("key")
            .and_then(|k| k.get("signal"))
            .and_then(Value::as_str)
            .map(str::to_string)
        else {
            continue;
        };
        rewrite(mix, &signal, &mut node["key"], "tap", None);
    }
}

/// The array at `v[key]`, if there is one (indexing would insert a null).
fn array<'a>(v: &'a mut Value, key: &str) -> impl Iterator<Item = &'a mut Value> {
    v.get_mut(key)
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
}

/// Every chain (JSON) of the strips, buses and snapshots in `v`.
fn chains(v: &mut Value, mut each: impl FnMut(&mut Value)) {
    for kind in ["strips", "buses"] {
        for s in array(v, kind) {
            if let Some(c) = s.get_mut("chain") {
                each(c);
            }
        }
    }
    for snap in array(v, "snapshots") {
        let states = snap.get_mut("signals").and_then(Value::as_object_mut);
        for (_, state) in states.into_iter().flatten() {
            if let Some(c) = state.get_mut("chain") {
                each(c);
            }
        }
    }
}

/// The mix as it is written: loadable by 1.5.1 with the same outputs.
pub fn for_saving(mix: &Mix) -> Value {
    let mut v = serde_json::to_value(mix).unwrap_or_default();
    for (k, s) in mix.sends.iter().enumerate() {
        rewrite(mix, &s.from, &mut v["sends"][k], "tap", None);
    }
    for (k, o) in mix.outputs.iter().enumerate() {
        rewrite(mix, &o.signal, &mut v["outputs"][k], "tap", None);
    }
    for (k, r) in mix.virtual_outputs.iter().enumerate() {
        if let super::model::OutputSource::Signal { signal, .. } = &r.source {
            let route = &mut v["virtual_outputs"][k];
            let mut source = route["source"].take();
            rewrite(mix, signal, &mut source, "tap", Some(route));
            route["source"] = source;
        }
    }
    chains(&mut v, |c| rewrite_keys(mix, c));
    // The direct out `out` as the strip's published output of 1.5.
    for s in &mix.strips {
        let Some((
            i,
            ProcKind::DirectOut {
                format,
                commit_samples,
            },
        )) = s
            .chain
            .iter()
            .enumerate()
            .find(|(_, p)| p.id == MAIN_DIRECT_OUT)
            .map(|(i, p)| (i, &p.kind))
        else {
            continue;
        };
        let tap = legacy_at(&s.chain, i);
        let mut o =
            serde_json::json!({"signal": s.id, "format": format, "commit_samples": commit_samples});
        if tap != Tap::PostFader {
            o["tap"] = serde_json::to_value(&tap).unwrap_or_default();
        }
        match v.get_mut("outputs").and_then(Value::as_array_mut) {
            Some(list) => list.push(o),
            None => v["outputs"] = Value::Array(vec![o]),
        }
    }
    v
}

/// Undo [`for_saving`] on saved JSON before it is parsed: the exact taps back,
/// the legacy entries of the direct outs `out` dropped.
pub fn restore(v: &mut Value) {
    fn take_exact(obj: &mut Value, into: &str) {
        if let Some(exact) = obj.as_object_mut().and_then(|o| o.remove(TAP_EXACT)) {
            obj[into] = exact;
        }
    }
    if !v.is_object() {
        return;
    }
    for s in array(v, "sends") {
        take_exact(s, "tap");
    }
    for o in array(v, "outputs") {
        take_exact(o, "tap");
    }
    for r in array(v, "virtual_outputs") {
        let exact = r.as_object_mut().and_then(|o| o.remove(TAP_EXACT));
        if let (Some(exact), Some(source)) = (exact, r.get_mut("source").filter(|s| s.is_object()))
        {
            source["tap"] = exact;
        }
    }
    chains(v, |c| {
        for node in c.as_array_mut().into_iter().flatten() {
            if let Some(key) = node.get_mut("key").filter(|k| k.is_object()) {
                take_exact(key, "tap");
            }
        }
    });
    let is_out = |p: &Value| {
        p.get("id").and_then(Value::as_str) == Some(MAIN_DIRECT_OUT)
            && p.get("type").and_then(Value::as_str) == Some("direct-out")
    };
    let with_out: Vec<String> = v
        .get("strips")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|s| {
            s.get("chain")
                .and_then(Value::as_array)
                .is_some_and(|c| c.iter().any(is_out))
        })
        .filter_map(|s| s.get("id").and_then(Value::as_str).map(str::to_string))
        .collect();
    if let Some(list) = v.get_mut("outputs").and_then(Value::as_array_mut) {
        list.retain(|o| {
            !o.get("signal")
                .and_then(Value::as_str)
                .is_some_and(|s| with_out.iter().any(|w| w == s))
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A 1.6 show with every new node and every kind of tap on them.
    pub fn show() -> Mix {
        let eq = json!({"id": "eq-1", "type": "eq", "bands": [{"type": "bell", "freq_hz": 1000.0, "q": 1.0, "gain_db": 3.0}]});
        let comp = json!({"id": "compressor-1", "type": "compressor", "threshold_db": -20.0, "ratio": 3.0, "knee_db": 6.0,
            "attack_ms": 10.0, "release_ms": 150.0, "makeup_db": 0.0, "detector": "rms", "key": {"signal": "vox", "tap": {"after": "insert-1"}}});
        let strip = |id: &str, chain: Value| {
            json!({"id": id, "name": id, "format": "mono", "patch": [{"input": 1, "channel": 1}],
            "trim_db": 0.0, "polarity": [false], "fader_db": -6.0, "mute": false, "pan": 0.0, "width": 1.0, "chain": chain})
        };
        let send = |from: &str, to: &str, tap: Value| json!({"from": from, "to": to, "tap": tap, "gain_db": 0.0, "mute": false, "phase": false});
        let v = json!({
            "strips": [
                strip("vox", json!([
                    {"id": "out-2", "type": "direct-out"},
                    eq,
                    {"id": "insert-1", "type": "insert", "send_db": -3.0, "return_input": 2},
                    {"id": "fader", "type": "fader"},
                    {"id": "eq-2", "type": "eq", "bands": [{"type": "bell", "freq_hz": 3000.0, "q": 1.0, "gain_db": -2.0}]},
                    {"id": "out", "type": "direct-out", "commit_samples": 480},
                    {"id": "auto-1", "type": "auto"}
                ])),
                strip("pre", json!([
                    eq,
                    {"id": "out", "type": "direct-out", "format": "stereo"},
                    {"id": "fader", "type": "fader"}
                ])),
                strip("bed", json!([comp]))
            ],
            "buses": [
                {"id": "main", "name": "Main", "format": "stereo", "fader_db": 0.0, "mute": false, "compensate": true,
                 "chain": [{"id": "insert-1", "type": "insert", "return_input": 1}]},
                {"id": "aux", "name": "Aux", "format": "mono", "fader_db": 0.0, "mute": false, "compensate": true}
            ],
            "sends": [
                send("vox", "main", json!("post-pan")),
                send("vox", "aux", json!({"after": "fader"})),
                send("pre", "aux", json!({"after": "out"})),
                send("bed", "aux", json!({"after": "compressor-1"})),
                send("main", "aux", json!({"after": "insert-1"}))
            ],
            "outputs": [
                {"signal": "main", "format": null, "commit_samples": null},
                {"signal": "aux", "format": null, "commit_samples": null, "tap": "pre-fader"}
            ],
            "virtual_outputs": [{"output": "1", "source": {"signal": "vox", "tap": {"after": "eq-2"}}}],
            "automixes": [{"id": "am", "name": "AM", "members": [{"signal": "vox"}]}]
        });
        let mut m: Mix = serde_json::from_value(v).unwrap();
        m.settings = Mix::fresh().settings;
        m.buses[1].downmix_set = Some("reference".into());
        let snap = super::super::show::capture(&m, "s1".into(), "S1".into(), 0);
        m.snapshots.push(snap);
        m
    }

    #[test]
    fn a_saved_1_6_show_is_a_1_5_1_show_and_back() {
        let m = show();
        assert_eq!(m.validate(2, 96), Ok(()));
        let saved = for_saving(&m);
        // The fixture is what 1.5.1 is checked against (tests/rollback/check-1.5.1.sh).
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/rollback-1.6.json"
        );
        if std::env::var_os("UPDATE_FIXTURES").is_some() {
            std::fs::write(path, serde_json::to_string_pretty(&saved).unwrap() + "\n").unwrap();
        }
        let fixture: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(
            saved, fixture,
            "the saved form changed: check it with 1.5.1 and update the fixture"
        );

        // Legacy taps where 1.5.1 lacks the node, the exact ones beside them.
        let s = &saved["sends"];
        assert_eq!(s[0]["tap"], "post-pan");
        assert!(s[0].get(TAP_EXACT).is_none());
        assert_eq!(
            (&s[1]["tap"], &s[1][TAP_EXACT]),
            (&json!("post-fader"), &json!({"after": "fader"}))
        );
        assert_eq!(
            (&s[2]["tap"], &s[2][TAP_EXACT]),
            (&json!("pre-fader"), &json!({"after": "out"}))
        );
        assert!(
            s[3].get(TAP_EXACT).is_none(),
            "a known node before the fader stays"
        );
        assert_eq!(
            (&s[4]["tap"], &s[4][TAP_EXACT]),
            (&json!("pre-fader"), &json!({"after": "insert-1"}))
        );
        assert_eq!(saved["virtual_outputs"][0]["source"]["tap"], "post-fader");
        assert_eq!(
            saved["virtual_outputs"][0][TAP_EXACT],
            json!({"after": "eq-2"})
        );
        let key = &saved["strips"][2]["chain"][0]["key"];
        assert_eq!(
            (&key["tap"], &key[TAP_EXACT]),
            (&json!("pre-fader"), &json!({"after": "insert-1"}))
        );
        // The direct outs `out` as 1.5 outputs: after the fader → post-fader;
        // right before the fader → pre-fader.
        let outs: Vec<&Value> = saved["outputs"].as_array().unwrap().iter().collect();
        assert_eq!(outs.len(), 4);
        assert_eq!(
            outs[2],
            &json!({"signal": "vox", "format": null, "commit_samples": 480})
        );
        assert_eq!(
            outs[3],
            &json!({"signal": "pre", "format": "stereo", "commit_samples": null, "tap": "pre-fader"})
        );

        // 1.6 loads it back exactly.
        let (mut back, dropped) = super::super::tolerant::mix(saved).unwrap();
        assert!(dropped.is_empty(), "{dropped:?}");
        back.lift_direct_outs();
        assert_eq!(back, m);
        // An export imported (strict parse after restore) is the same.
        let mut v = for_saving(&m);
        restore(&mut v);
        assert_eq!(serde_json::from_value::<Mix>(v).unwrap(), m);
    }

    /// What 1.5.1 made of the fixture (written by tests/rollback/check-1.5.1.sh
    /// with the released 1.5.1 code): valid, the same published outputs.
    #[test]
    fn the_1_5_1_load_of_the_fixture_keeps_the_outputs() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/rollback-1.6.loaded-by-1.5.1.json"
        );
        let loaded: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(loaded["valid"], true, "{}", loaded["error"]);
        let mut outputs: Vec<&str> = loaded["outputs"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|o| o["signal"].as_str())
            .collect();
        outputs.sort();
        let m = show();
        let mut want: Vec<String> = m
            .outputs
            .iter()
            .map(|o| o.signal.clone())
            .chain(
                m.chain_outputs()
                    .into_iter()
                    .filter(|o| o.key == o.signal)
                    .map(|o| o.key),
            )
            .collect();
        want.sort();
        assert_eq!(outputs, want);
    }
}
