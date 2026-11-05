//! Console actions on many signals (§12.1, 1.6.0 B1, B2):
//!
//! - `PATCH /api/v1/signals`: one change of several strips and buses at once (a
//!   multi-selection, a copy of settings): their fields, their chain nodes and their
//!   solos, applied together as one mix revision. The UI computes the values (a
//!   relative fader move gives each selected strip its own level).
//! - `POST /api/v1/{strips,buses}/<id>/move` `{"position": n}`: the console order of
//!   the strips (or the buses) is the order of the mix's list; it is part of the
//!   show, so every operator sees the same order.

use std::collections::BTreeMap;

use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use axum::routing::{patch, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::api::{
    apply_held, bad, body, merge, not_found, ramp, take, with_errors, ApiResult, BusBody, StripBody,
};
use crate::mix::model::Kind;
use crate::mix::room::SoloKind;
use crate::mix::Mix;
use crate::ops::App;

pub fn routes() -> Router<App> {
    Router::new()
        .route("/api/v1/signals", patch(patch_signals))
        .route(
            "/api/v1/strips/{id}/move",
            post(|s, p, b| move_signal(s, p, b, Kind::Strip)),
        )
        .route(
            "/api/v1/buses/{id}/move",
            post(|s, p, b| move_signal(s, p, b, Kind::Bus)),
        )
}

/// `PATCH /api/v1/signals`.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Batch {
    /// Per strip or bus: the fields of its own PATCH (no `id`, no `ramp_ms`).
    #[serde(default)]
    pub signals: BTreeMap<String, Value>,
    /// Per strip or bus, per node id: a JSON merge patch of the node.
    #[serde(default)]
    pub nodes: BTreeMap<String, BTreeMap<String, Value>>,
    /// Per strip or bus: new nodes (each as for `POST …/chain`: `id` and
    /// `position` optional), after the node patches.
    #[serde(default)]
    pub add: BTreeMap<String, Vec<Value>>,
    /// Strips and buses to delete (with their references), last.
    #[serde(default)]
    pub delete: Vec<String>,
    /// Per strip or bus: `"pfl"`, `"afl"` or `null` (no solo).
    #[serde(default)]
    pub solo: BTreeMap<String, Option<SoloKind>>,
    pub ramp_ms: Option<f64>,
}

impl Batch {
    /// Nothing to change.
    pub fn is_empty(&self) -> bool {
        self.signals.is_empty()
            && self.nodes.is_empty()
            && self.add.is_empty()
            && self.delete.is_empty()
            && self.solo.is_empty()
    }
}

fn chain_of<'a>(mix: &'a mut Mix, id: &str) -> ApiResult<&'a mut Vec<crate::mix::chain::Proc>> {
    match mix.kind(id) {
        Some(Kind::Strip) => Ok(&mut mix
            .strips
            .iter_mut()
            .find(|s| s.id == id)
            .expect("exists")
            .chain),
        Some(Kind::Bus) => Ok(&mut mix
            .buses
            .iter_mut()
            .find(|b| b.id == id)
            .expect("exists")
            .chain),
        None => Err(not_found(format!("signal {id} does not exist"))),
    }
}

/// Apply the signal fields, node patches, new nodes and deletes of a batch to
/// `mix` (in that order): all or none. Returns the notes of format changes.
pub fn apply_batch(mix: &mut Mix, b: &mut Batch) -> ApiResult<Vec<String>> {
    let mut notes = Vec::new();
    for (id, v) in std::mem::take(&mut b.signals) {
        let kind = mix
            .kind(&id)
            .ok_or_else(|| not_found(format!("signal {id} does not exist")))?;
        let refuse = |b: &Value| {
            if b.get("id").is_some_and(|x| x.as_str() != Some(id.as_str())) {
                return Err(bad("a signal id cannot be changed"));
            }
            if b.get("ramp_ms").is_some() {
                return Err(bad(format!("{id}: ramp_ms goes at the top of the batch")));
            }
            Ok(())
        };
        refuse(&v)?;
        let at = |e: serde_json::Error| bad(format!("{id}: {e}"));
        match kind {
            Kind::Strip => {
                let mut b: StripBody = serde_json::from_value(v).map_err(at)?;
                b.id = None;
                if let Some(f) = b.format.take() {
                    notes.extend(crate::mix::reformat::change_format(mix, &id, f).map_err(bad)?);
                }
                let s = mix.strips.iter_mut().find(|s| s.id == id).expect("exists");
                b.apply(s);
            }
            Kind::Bus => {
                let mut b: BusBody = serde_json::from_value(v).map_err(at)?;
                b.id = None;
                if let Some(f) = b.format.take() {
                    notes.extend(crate::mix::reformat::change_format(mix, &id, f).map_err(bad)?);
                }
                let x = mix.buses.iter_mut().find(|x| x.id == id).expect("exists");
                b.apply(x);
            }
        }
    }
    for (id, patches) in std::mem::take(&mut b.nodes) {
        let chain = chain_of(mix, &id)?;
        for (node, mut patch) in patches {
            if take(&mut patch, "id").is_some_and(|v| v.as_str() != Some(node.as_str())) {
                return Err(bad("a node id cannot be changed"));
            }
            let p = chain
                .iter_mut()
                .find(|p| p.id == node)
                .ok_or_else(|| not_found(format!("{id} has no node {node}")))?;
            let mut v = json!(p);
            merge(&mut v, patch);
            *p = serde_json::from_value(v).map_err(|e| bad(format!("{id} {node}: {e}")))?;
        }
    }
    for (id, list) in std::mem::take(&mut b.add) {
        let chain = chain_of(mix, &id)?;
        for mut v in list {
            let position = take(&mut v, "position").and_then(|p| p.as_u64());
            let given = take(&mut v, "id").and_then(|i| i.as_str().map(str::to_string));
            if let Some(o) = v.as_object_mut() {
                o.insert("id".into(), json!("new"));
            }
            let mut node: crate::mix::chain::Proc =
                serde_json::from_value(v).map_err(|e| bad(format!("{id}: {e}")))?;
            node.id = given.unwrap_or_else(|| crate::mix::chain::next_id(chain, &node.kind));
            let at = position.map_or(chain.len(), |p| (p as usize).min(chain.len()));
            chain.insert(at, node);
        }
    }
    for id in std::mem::take(&mut b.delete) {
        if mix.kind(&id).is_none() {
            return Err(not_found(format!("signal {id} does not exist")));
        }
        mix.remove_signal(&id);
    }
    Ok(notes)
}

async fn patch_signals(State(app): State<App>, bytes: axum::body::Bytes) -> Response {
    let run = async {
        let mut b: Batch = body(&bytes)?;
        let ramp = ramp(b.ramp_ms)?;
        let mut changed: Vec<String> = b
            .signals
            .keys()
            .chain(b.nodes.keys())
            .chain(b.add.keys())
            .chain(b.delete.iter())
            .cloned()
            .collect();
        changed.sort();
        changed.dedup();
        // Nothing to change: no revision, no save.
        if b.is_empty() {
            let v = json!({"signals": changed, "soloed": app.control().solo()});
            return Ok(Json(v).into_response());
        }
        // A new node or a delete rewires: fade as `POST …/chain` and `DELETE` do.
        let ramp = if b.add.is_empty() && b.delete.is_empty() {
            ramp
        } else {
            ramp.max(crate::engine::FADE_SAMPLES)
        };
        let _serial = app.changes.lock().await;
        // The solos first (the plan is compiled with them); taken back if the
        // change is refused.
        let before = if b.solo.is_empty() {
            None
        } else {
            let mut c = app.control();
            if let Some(id) = b
                .solo
                .keys()
                .find(|s| c.mix.kind(s).is_none() && c.mix.vca(s).is_none())
            {
                return Err(not_found(format!("signal {id} does not exist")));
            }
            Some(c.set_solos(&b.solo))
        };
        let mut notes = Vec::new();
        let result = apply_held(&app, ramp, |m| {
            notes = apply_batch(m, &mut b)?;
            Ok(())
        })
        .await;
        let errors = match result {
            Ok(e) => e,
            Err(e) => {
                if let Some(before) = before {
                    app.control().restore_solo(before);
                }
                return Err(e);
            }
        };
        let mut v = json!({"signals": changed, "soloed": app.control().solo()});
        if !notes.is_empty() {
            v["warnings"] = json!(notes);
        }
        Ok(Json(with_errors(v, errors)).into_response())
    };
    run.await.unwrap_or_else(|r: Box<Response>| *r)
}

/// Move strip (or bus) `id` to `position` (0 = first) in the mix's list.
pub fn move_in(mix: &mut Mix, kind: Kind, id: &str, position: usize) -> ApiResult<()> {
    fn shift<T>(list: &mut Vec<T>, from: usize, to: usize) {
        let x = list.remove(from);
        list.insert(to.min(list.len()), x);
    }
    match kind {
        Kind::Strip => {
            let from = mix
                .strips
                .iter()
                .position(|s| s.id == id)
                .ok_or_else(|| not_found(format!("strip {id} does not exist")))?;
            shift(&mut mix.strips, from, position);
        }
        Kind::Bus => {
            let from = mix
                .buses
                .iter()
                .position(|b| b.id == id)
                .ok_or_else(|| not_found(format!("bus {id} does not exist")))?;
            shift(&mut mix.buses, from, position);
        }
    }
    Ok(())
}

async fn move_signal(
    State(app): State<App>,
    Path(id): Path<String>,
    bytes: axum::body::Bytes,
    kind: Kind,
) -> Response {
    let run = async {
        let mut v: Value = body(&bytes)?;
        let position = take(&mut v, "position")
            .and_then(|p| p.as_u64())
            .ok_or_else(|| bad("position is required"))? as usize;
        if v.as_object().is_some_and(|o| !o.is_empty()) {
            return Err(bad("only position is allowed"));
        }
        let errors = {
            let _serial = app.changes.lock().await;
            apply_held(&app, 0, |m| move_in(m, kind, &id, position)).await?
        };
        let c = app.control();
        let order: Vec<&str> = match kind {
            Kind::Strip => c.mix.strips.iter().map(|s| s.id.as_str()).collect(),
            Kind::Bus => c.mix.buses.iter().map(|b| b.id.as_str()).collect(),
        };
        Ok::<_, Box<Response>>(Json(with_errors(json!(order), errors)).into_response())
    };
    run.await.unwrap_or_else(|r| *r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mix::model::tests::{bus, strip};
    use crate::mix::model::Db;
    use crate::mix::Format;

    fn mix() -> Mix {
        let mut m = Mix::default();
        m.strips.push(strip("a", Format::Mono));
        m.strips.push(strip("b", Format::Mono));
        m.strips.push(strip("c", Format::Stereo));
        m.buses.push(bus("main", Format::Stereo));
        m.buses.push(bus("aux", Format::Mono));
        let node = |v| serde_json::from_value::<crate::mix::chain::Proc>(v).unwrap();
        for s in &mut m.strips {
            s.chain.push(node(json!({"id": "hpf", "type": "filter", "mode": "high-pass", "freq_hz": 80.0, "slope_db_per_oct": 24})));
        }
        m
    }
    fn map<T: serde::de::DeserializeOwned>(v: Value) -> T {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn a_batch_changes_every_signal_and_node_together() {
        let mut m = mix();
        let notes = apply_batch(
            &mut m,
            &mut map(json!({
                "signals": {"a": {"fader_db": -3.0, "mute": true}, "b": {"fader_db": "-inf"}, "main": {"fader_db": -6.0}},
                "nodes": {"a": {"hpf": {"freq_hz": 100.0}}, "c": {"hpf": {"bypass": true}}},
            })),
        )
        .unwrap();
        assert!(notes.is_empty());
        assert_eq!(m.strips[0].fader_db, Db(-3.0));
        assert!(m.strips[0].mute);
        assert_eq!(m.strips[1].fader_db, Db(f64::NEG_INFINITY));
        assert_eq!(m.buses[0].fader_db, Db(-6.0));
        assert_eq!(json!(m.strips[0].chain[0])["freq_hz"], json!(100.0));
        assert_eq!(json!(m.strips[2].chain[0])["bypass"], json!(true));
        assert_eq!(m.validate(4, 48), Ok(()));
    }

    #[test]
    fn a_batch_with_one_bad_entry_changes_nothing_for_the_caller() {
        // The mix given is a copy (apply_held): an error leaves the live one alone,
        // and every entry is checked.
        for (signals, nodes) in [
            (json!({"nope": {"mute": true}}), json!({})),
            (
                json!({"a": {"mute": true}}),
                json!({"b": {"nope": {"bypass": true}}}),
            ),
            (json!({"a": {"ramp_ms": 10}}), json!({})),
            (json!({"a": {"id": "x"}}), json!({})),
            (json!({"main": {"trim_db": 3}}), json!({})),
        ] {
            let mut m = mix();
            let mut b = map(json!({"signals": signals, "nodes": nodes}));
            assert!(apply_batch(&mut m, &mut b).is_err(), "{signals}");
        }
    }

    #[test]
    fn a_batch_changes_formats_like_a_patch() {
        let mut m = mix();
        apply_batch(
            &mut m,
            &mut map(json!({"signals": {"a": {"format": "stereo"}, "b": {"format": "stereo"}}})),
        )
        .unwrap();
        assert_eq!(
            (m.strips[0].format, m.strips[1].format),
            (Format::Stereo, Format::Stereo)
        );
        assert_eq!(m.strips[0].patch.len(), 2);
    }

    #[test]
    fn a_batch_adds_nodes_and_deletes_signals() {
        let mut m = mix();
        let comp = json!({"type": "compressor", "threshold_db": -20.0, "ratio": 3.0, "knee_db": 6.0, "attack_ms": 10.0, "release_ms": 150.0, "makeup_db": 0.0, "detector": "rms"});
        let mut first = comp.clone();
        first["position"] = json!(0);
        let mut b: Batch =
            map(json!({"add": {"a": [comp.clone()], "b": [first]}, "delete": ["c"]}));
        apply_batch(&mut m, &mut b).unwrap();
        assert_eq!(m.strips.len(), 2);
        assert_eq!(m.strips[0].chain[1].kind, m.strips[1].chain[0].kind);
        assert_eq!(m.strips[0].chain[1].id, m.strips[1].chain[0].id);
        assert_eq!(m.validate(4, 48), Ok(()));
        // One bad entry: an error (the caller's copy is dropped).
        let mut b: Batch = map(json!({"add": {"a": [comp]}, "delete": ["nope"]}));
        assert!(apply_batch(&mut mix(), &mut b).is_err());
        assert!(map::<Batch>(json!({})).is_empty());
    }

    #[test]
    fn moving_reorders_strips_and_buses() {
        let mut m = mix();
        move_in(&mut m, Kind::Strip, "c", 0).unwrap();
        let ids = |m: &Mix| m.strips.iter().map(|s| s.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(&m), ["c", "a", "b"]);
        move_in(&mut m, Kind::Strip, "c", 99).unwrap();
        assert_eq!(ids(&m), ["a", "b", "c"]);
        move_in(&mut m, Kind::Bus, "aux", 0).unwrap();
        assert_eq!(m.buses[0].id, "aux");
        assert!(
            move_in(&mut m, Kind::Strip, "main", 0).is_err(),
            "a bus is not a strip"
        );
        assert!(move_in(&mut m, Kind::Bus, "nope", 0).is_err());
    }
}
