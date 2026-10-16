//! Logic graphs and macros (§11, IMPLEMENTATION_PLAN M8b):
//! - `GET`, `POST /api/v1/logic`, `GET`, `PUT`, `PATCH`, `DELETE
//!   /api/v1/logic/{id}`: a graph's blocks and wires. Every answer adds
//!   `revision` (a digest of the graph), `ports` (each block's typed ports),
//!   `values` (each block's last value), `buttons` (its soft keys held down) and
//!   `actions` (its last 20 actions). `PUT` and `PATCH` take an optional
//!   `revision`: a graph changed since answers 409;
//! - `PUT /api/v1/logic/{id}/buttons/{block}` with `{"pressed": true}`: a soft
//!   key (not part of the show);
//! - `POST /api/v1/logic/{id}/trigger/{block}`: fire a `trigger` block on the
//!   next tick;
//! - `GET`, `POST /api/v1/logic-macros`, `GET`, `PUT`, `PATCH`, `DELETE
//!   /api/v1/logic-macros/{id}`: macros, as graphs (with `exposed`: the
//!   instance's ports);
//! - `POST /api/v1/logic-batch`: graphs and macros written and deleted as one
//!   change; `POST /api/v1/logic-validate`: the same body, only checked;
//! - `GET /api/v1/logic-blocks`: every block type with its parameters and ports.
//!
//! A refused change answers 400 `{"error", "issues": [{graph, macro, block,
//! port, wire, message}]}` (D162).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha1::{Digest, Sha1};

use crate::api::{apply_held, bad, body, conflict, not_found, ApiResult};
use crate::engine::FADE_SAMPLES;
use crate::mix::logic::{issues, Block, BlockKind, Graph, Issue, Macro, Wire};
use crate::mix::Mix;
use crate::ops::App;

pub fn routes() -> Router<App> {
    Router::new()
        .route("/api/v1/logic", get(list).post(create))
        .route(
            "/api/v1/logic/{id}",
            get(one).put(put_graph).patch(patch).delete(delete),
        )
        .route("/api/v1/logic/{id}/buttons/{block}", put(button))
        .route("/api/v1/logic/{id}/trigger/{block}", post(trigger))
        .route("/api/v1/logic-blocks", get(catalog))
        .route("/api/v1/logic-macros", get(macros).post(create_macro))
        .route(
            "/api/v1/logic-macros/{id}",
            get(one_macro)
                .put(put_macro)
                .patch(patch_macro)
                .delete(delete_macro),
        )
        .route("/api/v1/logic-batch", post(batch))
        .route("/api/v1/logic-validate", post(validate))
}

async fn run(f: impl std::future::Future<Output = ApiResult<Response>>) -> Response {
    f.await.unwrap_or_else(|r| *r)
}

/// A digest of a graph or macro: 16 hex digits.
pub fn revision<T: Serialize>(x: &T) -> String {
    let digest = Sha1::digest(serde_json::to_vec(x).unwrap_or_default());
    digest[..8].iter().map(|b| format!("{b:02x}")).collect()
}

fn block_ports(blocks: &[Block], macros: &[Macro]) -> Value {
    Value::Object(
        blocks
            .iter()
            .map(|b| (b.id.clone(), json!(b.kind.ports(macros))))
            .collect(),
    )
}

fn json_graph(app: &App, g: &Graph) -> Value {
    let c = app.control();
    let mut j = json!(g);
    j["revision"] = json!(revision(g));
    j["ports"] = block_ports(&g.blocks, &c.mix.logic_macros);
    j["values"] = c.logic.values(&g.id).unwrap_or(Value::Null);
    j["buttons"] = json!(c.logic.pressed(&g.id));
    j["actions"] = json!(c.logic.recent(&g.id));
    j
}

fn json_macro(macros: &[Macro], m: &Macro) -> Value {
    let mut j = json!(m);
    j["revision"] = json!(revision(m));
    j["ports"] = block_ports(&m.blocks, macros);
    j["exposed"] = json!(BlockKind::Macro {
        macro_id: m.id.clone()
    }
    .ports(macros));
    j
}

fn one_value(app: &App, id: &str) -> Value {
    let found = app.control().mix.logic_graph(id).cloned();
    found.map(|g| json_graph(app, &g)).unwrap_or(Value::Null)
}

fn macro_value(app: &App, id: &str) -> Value {
    let c = app.control();
    c.mix
        .logic_macro(id)
        .map(|m| json_macro(&c.mix.logic_macros, m))
        .unwrap_or(Value::Null)
}

/// 400 with every issue.
fn refused(issues: &[Issue]) -> Box<Response> {
    Box::new(
        (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": issues.first().map(|i| i.to_string()).unwrap_or_default(),
                "issues": issues,
            })),
        )
            .into_response(),
    )
}

/// Apply a change of the logic; refused with the issues if the logic is not
/// valid afterwards. The graphs are validated off the control lock, under the
/// change lock, and the stage then does not validate them again (D171).
async fn change(app: &App, f: impl FnOnce(&mut Mix) -> ApiResult<()>) -> ApiResult<()> {
    let _serial = app.changes.lock().await;
    let mut candidate = app.control().mix.clone();
    f(&mut candidate)?;
    let (candidate, found) = tokio::task::spawn_blocking(move || {
        let found = issues(&candidate);
        (candidate, found)
    })
    .await
    .map_err(|e| bad(e.to_string()))?;
    if !found.is_empty() {
        return Err(refused(&found));
    }
    // Only the graphs and macros are taken over: the rest of the mix may have
    // moved meanwhile (logic faders, the link). If what they name moved, the
    // stage validates them again.
    app.control().logic_validated(&candidate);
    apply_held(app, FADE_SAMPLES, move |mix| {
        mix.logic = candidate.logic;
        mix.logic_macros = candidate.logic_macros;
        Ok(())
    })
    .await
    .map(|_| ())
}

fn stale(id: &str, current: &str) -> Box<Response> {
    Box::new(
        (
            StatusCode::CONFLICT,
            Json(json!({"error": format!("{id} changed since revision was read"), "revision": current})),
        )
            .into_response(),
    )
}

/// Take `revision` out of a JSON body, and drop what only answers carry
/// (`ports`, `values`, `buttons`, `actions`, `exposed`), so a client may send
/// back what it got.
fn take_revision(v: &mut Value) -> ApiResult<Option<String>> {
    if let Some(o) = v.as_object_mut() {
        for k in ["ports", "values", "buttons", "actions", "exposed"] {
            o.remove(k);
        }
    }
    match v.as_object_mut().and_then(|o| o.remove("revision")) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(_) => Err(bad("revision must be a string")),
    }
}

/// A whole graph or macro from a JSON body; the id is the path's.
fn whole<T: serde::de::DeserializeOwned>(mut v: Value, id: &str) -> ApiResult<T> {
    let o = v
        .as_object_mut()
        .ok_or_else(|| bad("a JSON object is required"))?;
    match o.get("id") {
        Some(Value::String(given)) if given != id => {
            return Err(bad("the id in the body is not the one in the path"))
        }
        _ => {}
    }
    o.insert("id".into(), json!(id));
    serde_json::from_value(v).map_err(|e| bad(e.to_string()))
}

async fn list(State(app): State<App>) -> Json<Value> {
    let graphs = app.control().mix.logic.clone();
    Json(Value::Array(
        graphs.iter().map(|g| json_graph(&app, g)).collect(),
    ))
}

async fn one(State(app): State<App>, Path(id): Path<String>) -> Response {
    match one_value(&app, &id) {
        Value::Null => *not_found(format!("logic {id} does not exist")),
        v => Json(v).into_response(),
    }
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Body {
    id: Option<String>,
    name: Option<String>,
    description: Option<String>,
    enabled: Option<bool>,
    dry_run: Option<bool>,
    blocks: Option<Vec<Block>>,
    wires: Option<Vec<Wire>>,
    revision: Option<String>,
}

impl Body {
    fn apply(self, g: &mut Graph) {
        if let Some(v) = self.name {
            g.name = v;
        }
        if let Some(v) = self.description {
            g.description = v;
        }
        if let Some(v) = self.enabled {
            g.enabled = v;
        }
        if let Some(v) = self.dry_run {
            g.dry_run = v;
        }
        if let Some(v) = self.blocks {
            g.blocks = v;
        }
        if let Some(v) = self.wires {
            g.wires = v;
        }
    }
}

async fn create(State(app): State<App>, bytes: axum::body::Bytes) -> Response {
    run(async {
        let mut b: Body = body(&bytes)?;
        let id = match b.id.take() {
            Some(id) => id,
            None => {
                let c = app.control();
                (1..)
                    .map(|n| format!("logic-{n}"))
                    .find(|id| c.mix.logic_graph(id).is_none())
                    .expect("unbounded")
            }
        };
        let mut g = Graph {
            id: id.clone(),
            name: id.clone(),
            description: String::new(),
            enabled: true,
            dry_run: false,
            blocks: Vec::new(),
            wires: Vec::new(),
        };
        b.apply(&mut g);
        change(&app, |mix| {
            if mix.logic_graph(&g.id).is_some() {
                return Err(conflict(format!("logic {} exists", g.id)));
            }
            mix.logic.push(g);
            Ok(())
        })
        .await?;
        Ok((StatusCode::CREATED, Json(one_value(&app, &id))).into_response())
    })
    .await
}

/// The whole graph (D161); creates it if it does not exist.
async fn put_graph(
    State(app): State<App>,
    Path(id): Path<String>,
    bytes: axum::body::Bytes,
) -> Response {
    run(async {
        let mut v: Value = body(&bytes)?;
        let expected = take_revision(&mut v)?;
        let g: Graph = whole(v, &id)?;
        let mut created = false;
        change(&app, |mix| {
            match mix.logic.iter_mut().find(|x| x.id == id) {
                Some(current) => {
                    if expected.as_ref().is_some_and(|r| *r != revision(current)) {
                        return Err(stale(&format!("logic {id}"), &revision(current)));
                    }
                    *current = g;
                }
                None => {
                    if expected.is_some() {
                        return Err(not_found(format!("logic {id} does not exist")));
                    }
                    created = true;
                    mix.logic.push(g);
                }
            }
            Ok(())
        })
        .await?;
        let status = if created {
            StatusCode::CREATED
        } else {
            StatusCode::OK
        };
        Ok((status, Json(one_value(&app, &id))).into_response())
    })
    .await
}

async fn patch(
    State(app): State<App>,
    Path(id): Path<String>,
    bytes: axum::body::Bytes,
) -> Response {
    run(async {
        let mut b: Body = body(&bytes)?;
        if b.id.take().is_some_and(|new| new != id) {
            return Err(bad("a logic id cannot be changed"));
        }
        let expected = b.revision.take();
        change(&app, |mix| {
            let g = mix
                .logic
                .iter_mut()
                .find(|g| g.id == id)
                .ok_or_else(|| not_found(format!("logic {id} does not exist")))?;
            if expected.as_ref().is_some_and(|r| *r != revision(g)) {
                return Err(stale(&format!("logic {id}"), &revision(g)));
            }
            b.apply(g);
            Ok(())
        })
        .await?;
        Ok(Json(one_value(&app, &id)).into_response())
    })
    .await
}

async fn delete(State(app): State<App>, Path(id): Path<String>) -> Response {
    run(async {
        change(&app, |mix| {
            let before = mix.logic.len();
            mix.logic.retain(|g| g.id != id);
            if mix.logic.len() == before {
                return Err(not_found(format!("logic {id} does not exist")));
            }
            Ok(())
        })
        .await?;
        Ok(StatusCode::NO_CONTENT.into_response())
    })
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ButtonBody {
    pressed: bool,
}

async fn button(
    State(app): State<App>,
    Path((id, block)): Path<(String, String)>,
    bytes: axum::body::Bytes,
) -> Response {
    run(async {
        let b: ButtonBody = body(&bytes)?;
        let mut c = app.control();
        if c.mix.logic_graph(&id).is_none() {
            return Err(not_found(format!("logic {id} does not exist")));
        }
        if c.mix.logic_block_at(&id, &block) != Some(&BlockKind::Button) {
            return Err(not_found(format!("logic {id} has no button {block}")));
        }
        c.logic.set_button(&id, &block, b.pressed);
        drop(c);
        Ok(Json(one_value(&app, &id)).into_response())
    })
    .await
}

/// Fire a `trigger` block (D166). No body; the same access as the rest of the
/// API (X13).
async fn trigger(State(app): State<App>, Path((id, block)): Path<(String, String)>) -> Response {
    run(async {
        let mut c = app.control();
        let g = c
            .mix
            .logic_graph(&id)
            .ok_or_else(|| not_found(format!("logic {id} does not exist")))?;
        if !g.enabled {
            return Err(conflict(format!("logic {id} is disabled")));
        }
        if c.mix.logic_block_at(&id, &block) != Some(&BlockKind::Trigger) {
            return Err(not_found(format!("logic {id} has no trigger {block}")));
        }
        c.logic.trigger(&id, &block);
        Ok(StatusCode::NO_CONTENT.into_response())
    })
    .await
}

async fn macros(State(app): State<App>) -> Json<Value> {
    let c = app.control();
    let all = &c.mix.logic_macros;
    Json(Value::Array(
        all.iter().map(|m| json_macro(all, m)).collect(),
    ))
}

async fn one_macro(State(app): State<App>, Path(id): Path<String>) -> Response {
    match macro_value(&app, &id) {
        Value::Null => *not_found(format!("logic macro {id} does not exist")),
        v => Json(v).into_response(),
    }
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct MacroBody {
    id: Option<String>,
    name: Option<String>,
    description: Option<String>,
    blocks: Option<Vec<Block>>,
    wires: Option<Vec<Wire>>,
    revision: Option<String>,
}

impl MacroBody {
    fn apply(self, m: &mut Macro) {
        if let Some(v) = self.name {
            m.name = v;
        }
        if let Some(v) = self.description {
            m.description = v;
        }
        if let Some(v) = self.blocks {
            m.blocks = v;
        }
        if let Some(v) = self.wires {
            m.wires = v;
        }
    }
}

async fn create_macro(State(app): State<App>, bytes: axum::body::Bytes) -> Response {
    run(async {
        let mut b: MacroBody = body(&bytes)?;
        let id = match b.id.take() {
            Some(id) => id,
            None => {
                let c = app.control();
                (1..)
                    .map(|n| format!("macro-{n}"))
                    .find(|id| c.mix.logic_macro(id).is_none())
                    .expect("unbounded")
            }
        };
        let mut m = Macro {
            id: id.clone(),
            name: id.clone(),
            description: String::new(),
            blocks: Vec::new(),
            wires: Vec::new(),
        };
        b.apply(&mut m);
        change(&app, |mix| {
            if mix.logic_macro(&m.id).is_some() {
                return Err(conflict(format!("logic macro {} exists", m.id)));
            }
            mix.logic_macros.push(m);
            Ok(())
        })
        .await?;
        Ok((StatusCode::CREATED, Json(macro_value(&app, &id))).into_response())
    })
    .await
}

async fn put_macro(
    State(app): State<App>,
    Path(id): Path<String>,
    bytes: axum::body::Bytes,
) -> Response {
    run(async {
        let mut v: Value = body(&bytes)?;
        let expected = take_revision(&mut v)?;
        let m: Macro = whole(v, &id)?;
        let mut created = false;
        change(&app, |mix| {
            match mix.logic_macros.iter_mut().find(|x| x.id == id) {
                Some(current) => {
                    if expected.as_ref().is_some_and(|r| *r != revision(current)) {
                        return Err(stale(&format!("logic macro {id}"), &revision(current)));
                    }
                    *current = m;
                }
                None => {
                    if expected.is_some() {
                        return Err(not_found(format!("logic macro {id} does not exist")));
                    }
                    created = true;
                    mix.logic_macros.push(m);
                }
            }
            Ok(())
        })
        .await?;
        let status = if created {
            StatusCode::CREATED
        } else {
            StatusCode::OK
        };
        Ok((status, Json(macro_value(&app, &id))).into_response())
    })
    .await
}

async fn patch_macro(
    State(app): State<App>,
    Path(id): Path<String>,
    bytes: axum::body::Bytes,
) -> Response {
    run(async {
        let mut b: MacroBody = body(&bytes)?;
        if b.id.take().is_some_and(|new| new != id) {
            return Err(bad("a macro id cannot be changed"));
        }
        let expected = b.revision.take();
        change(&app, |mix| {
            let m = mix
                .logic_macros
                .iter_mut()
                .find(|m| m.id == id)
                .ok_or_else(|| not_found(format!("logic macro {id} does not exist")))?;
            if expected.as_ref().is_some_and(|r| *r != revision(m)) {
                return Err(stale(&format!("logic macro {id}"), &revision(m)));
            }
            b.apply(m);
            Ok(())
        })
        .await?;
        Ok(Json(macro_value(&app, &id)).into_response())
    })
    .await
}

async fn delete_macro(State(app): State<App>, Path(id): Path<String>) -> Response {
    run(async {
        change(&app, |mix| {
            let before = mix.logic_macros.len();
            mix.logic_macros.retain(|m| m.id != id);
            if mix.logic_macros.len() == before {
                return Err(not_found(format!("logic macro {id} does not exist")));
            }
            Ok(())
        })
        .await?;
        Ok(StatusCode::NO_CONTENT.into_response())
    })
    .await
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct BatchBody {
    #[serde(default)]
    graphs: Vec<Value>,
    #[serde(default)]
    macros: Vec<Value>,
    #[serde(default)]
    delete_graphs: Vec<String>,
    #[serde(default)]
    delete_macros: Vec<String>,
}

/// A batch, parsed: each graph and macro with its expected revision.
struct Batch {
    graphs: Vec<(Graph, Option<String>)>,
    macros: Vec<(Macro, Option<String>)>,
    delete_graphs: Vec<String>,
    delete_macros: Vec<String>,
}

impl Batch {
    fn parse(bytes: &[u8]) -> ApiResult<Self> {
        let b: BatchBody = body(bytes)?;
        let mut graphs = Vec::new();
        for mut v in b.graphs {
            let expected = take_revision(&mut v)?;
            let g: Graph = serde_json::from_value(v).map_err(|e| bad(format!("graphs: {e}")))?;
            graphs.push((g, expected));
        }
        let mut macros = Vec::new();
        for mut v in b.macros {
            let expected = take_revision(&mut v)?;
            let m: Macro = serde_json::from_value(v).map_err(|e| bad(format!("macros: {e}")))?;
            macros.push((m, expected));
        }
        Ok(Self {
            graphs,
            macros,
            delete_graphs: b.delete_graphs,
            delete_macros: b.delete_macros,
        })
    }

    /// 400 when a deletion names a graph or macro that does not exist.
    fn missing(&self, mix: &Mix) -> ApiResult<()> {
        for id in &self.delete_graphs {
            if mix.logic_graph(id).is_none() {
                return Err(bad(format!("logic {id} does not exist")));
            }
        }
        for id in &self.delete_macros {
            if mix.logic_macro(id).is_none() {
                return Err(bad(format!("logic macro {id} does not exist")));
            }
        }
        Ok(())
    }

    /// Apply to `mix`; the revisions that do not match.
    fn apply(self, mix: &mut Mix) -> Vec<Value> {
        let mut conflicts = Vec::new();
        for id in &self.delete_graphs {
            mix.logic.retain(|g| &g.id != id);
        }
        for id in &self.delete_macros {
            mix.logic_macros.retain(|m| &m.id != id);
        }
        for (m, expected) in self.macros {
            let current = mix.logic_macros.iter_mut().find(|x| x.id == m.id);
            let now = current.as_ref().map(|c| revision(&**c));
            if expected.is_some() && expected != now {
                conflicts.push(json!({"macro": m.id, "revision": now}));
                continue;
            }
            match current {
                Some(c) => *c = m,
                None => mix.logic_macros.push(m),
            }
        }
        for (g, expected) in self.graphs {
            let current = mix.logic.iter_mut().find(|x| x.id == g.id);
            let now = current.as_ref().map(|c| revision(&**c));
            if expected.is_some() && expected != now {
                conflicts.push(json!({"graph": g.id, "revision": now}));
                continue;
            }
            match current {
                Some(c) => *c = g,
                None => mix.logic.push(g),
            }
        }
        conflicts
    }
}

/// Graphs and macros as one change (D161).
async fn batch(State(app): State<App>, bytes: axum::body::Bytes) -> Response {
    run(async {
        let b = Batch::parse(&bytes)?;
        let graph_ids: Vec<String> = b.graphs.iter().map(|(g, _)| g.id.clone()).collect();
        let macro_ids: Vec<String> = b.macros.iter().map(|(m, _)| m.id.clone()).collect();
        change(&app, |mix| {
            b.missing(mix)?;
            conflicted(b.apply(mix))
        })
        .await?;
        let graphs: Vec<Value> = graph_ids.iter().map(|id| one_value(&app, id)).collect();
        let macros: Vec<Value> = macro_ids.iter().map(|id| macro_value(&app, id)).collect();
        Ok(Json(json!({"graphs": graphs, "macros": macros})).into_response())
    })
    .await
}

/// 409 when a revision did not match.
fn conflicted(conflicts: Vec<Value>) -> ApiResult<()> {
    if conflicts.is_empty() {
        return Ok(());
    }
    Err(Box::new(
        (
            StatusCode::CONFLICT,
            Json(json!({
                "error": "a graph or macro changed since its revision was read",
                "conflicts": conflicts,
            })),
        )
            .into_response(),
    ))
}

/// The proposed graphs and macros (the batch on the current mix) checked, not
/// applied: 200 `{"issues"}`, or 400 and 409 as the batch would answer.
async fn validate(State(app): State<App>, bytes: axum::body::Bytes) -> Response {
    run(async {
        let b = Batch::parse(&bytes)?;
        let mut mix = app.control().mix.clone();
        b.missing(&mix)?;
        conflicted(b.apply(&mut mix))?;
        let found = tokio::task::spawn_blocking(move || issues(&mix))
            .await
            .map_err(|e| bad(e.to_string()))?;
        Ok(Json(json!({"issues": found})).into_response())
    })
    .await
}

/// `GET /api/v1/logic-blocks`: every block type with its parameters and ports
/// (for `select` and `macro`, with their default parameters).
async fn catalog() -> Json<Value> {
    Json(catalog_json())
}

pub fn catalog_json() -> Value {
    use crate::mix::logic::{
        CompareOp, LoudnessAction, MathOp, Measure, PortType, MAX_MS, MAX_POINTS, MAX_SELECT,
        MAX_TEXT,
    };
    use crate::mix::model::{DEFAULT_RAMP_MS, MAX_RAMP_MS};
    let p = |name: &str, kind: &str| json!({"name": name, "kind": kind});
    let num = |name: &str, min: f64, max: f64, default: Option<f64>| json!({"name": name, "kind": "number", "min": min, "max": max, "default": default});
    let int = |name: &str, min: f64, max: f64| json!({"name": name, "kind": "integer", "min": min, "max": max});
    let options = |name: &str, opts: &[&str], default: &str| json!({"name": name, "kind": "enum", "options": opts, "default": default});
    let ramp = || num("ramp_ms", 0.0, MAX_RAMP_MS, Some(DEFAULT_RAMP_MS));
    let ms = || num("ms", 0.0, MAX_MS, None);
    let s = || "x".to_string();
    let types = [
        PortType::Boolean,
        PortType::Number,
        PortType::Level,
        PortType::String,
    ]
    .map(PortType::name);
    let all_types = [
        PortType::Boolean,
        PortType::Number,
        PortType::Level,
        PortType::Event,
        PortType::String,
    ]
    .map(PortType::name);
    let blocks: Vec<(BlockKind, &str, Vec<Value>)> = vec![
        (
            BlockKind::Const { value: 0.0 },
            "Number",
            vec![json!({"name": "value", "kind": "number", "default": 0})],
        ),
        (
            BlockKind::Text { value: s() },
            "Text",
            vec![json!({"name": "value", "kind": "string", "max_length": MAX_TEXT, "default": ""})],
        ),
        (BlockKind::Button, "Soft key", vec![]),
        (BlockKind::Trigger, "HTTP trigger", vec![]),
        (
            BlockKind::Mute { signal: s() },
            "Mute state",
            vec![p("signal", "signal")],
        ),
        (
            BlockKind::Pfl { signal: s() },
            "PFL state",
            vec![p("signal", "signal-or-vca")],
        ),
        (
            BlockKind::Fader { signal: s() },
            "Fader position",
            vec![p("signal", "signal")],
        ),
        (
            BlockKind::Vad { signal: s() },
            "Voice detected",
            vec![p("signal", "signal")],
        ),
        (
            BlockKind::Level { signal: s() },
            "Meter level",
            vec![p("signal", "signal")],
        ),
        (
            BlockKind::Above {
                signal: s(),
                threshold_db: -40.0,
                hysteresis_db: 6.0,
            },
            "Level above",
            vec![
                p("signal", "signal"),
                num("threshold_db", -120.0, 20.0, Some(-40.0)),
                num("hysteresis_db", 0.0, 60.0, Some(6.0)),
            ],
        ),
        (
            BlockKind::Loudness {
                signal: s(),
                measure: Measure::Momentary,
            },
            "Loudness",
            vec![
                p("signal", "signal"),
                options(
                    "measure",
                    &[
                        "momentary",
                        "short-term",
                        "integrated",
                        "range",
                        "true-peak",
                    ],
                    "momentary",
                ),
            ],
        ),
        (
            BlockKind::Tally { index: 0 },
            "Tally on air",
            vec![int("index", 0.0, 65535.0)],
        ),
        (
            BlockKind::Schedule {
                start_minute: 0,
                end_minute: 0,
            },
            "Schedule (UTC)",
            vec![
                int("start_minute", 0.0, 1439.0),
                int("end_minute", 0.0, 1439.0),
            ],
        ),
        (
            BlockKind::NmosReceiver { input: 1 },
            "NMOS receiver active",
            vec![int("input", 1.0, 1024.0)],
        ),
        (
            BlockKind::NmosSender { signal: s() },
            "NMOS sender active",
            vec![p("signal", "signal")],
        ),
        (
            BlockKind::Link { peer: s() },
            "Remote link up",
            vec![
                json!({"name": "peer", "kind": "peer", "options": ["master", "s1", "s2", "s3", "s4"]}),
            ],
        ),
        (BlockKind::And, "AND", vec![]),
        (BlockKind::Or, "OR", vec![]),
        (BlockKind::Xor, "XOR", vec![]),
        (BlockKind::Not, "NOT", vec![]),
        (
            BlockKind::Compare { op: CompareOp::Gt },
            "Compare",
            vec![options("op", &["gt", "lt", "eq", "ge", "le", "ne"], "gt")],
        ),
        (
            BlockKind::Math { op: MathOp::Add },
            "Math",
            vec![options(
                "op",
                &["add", "sub", "mul", "div", "min", "max"],
                "add",
            )],
        ),
        (
            BlockKind::Scale {
                factor: 1.0,
                offset: 0.0,
            },
            "Scale",
            vec![
                json!({"name": "factor", "kind": "number", "default": 1}),
                json!({"name": "offset", "kind": "number", "default": 0}),
            ],
        ),
        (
            BlockKind::Map {
                points: vec![[0.0, 0.0], [1.0, 1.0]],
            },
            "Map (curve)",
            vec![
                json!({"name": "points", "kind": "points", "min": 2, "max": MAX_POINTS, "default": [[0, 0], [1, 1]]}),
            ],
        ),
        (
            BlockKind::Select {
                inputs: 2,
                value_type: PortType::Number,
            },
            "Select",
            vec![
                json!({"name": "inputs", "kind": "integer", "min": 2, "max": MAX_SELECT, "default": 2}),
                json!({"name": "value_type", "kind": "port-type", "options": types, "default": "number"}),
            ],
        ),
        (BlockKind::Edge, "Edge (rise)", vec![]),
        (BlockKind::Latch, "Latch", vec![]),
        (BlockKind::Toggle, "Toggle", vec![]),
        (BlockKind::Counter, "Counter", vec![]),
        (BlockKind::Timer { ms: 0.0 }, "Timer (pulse)", vec![ms()]),
        (BlockKind::Delay { ms: 0.0 }, "Delay", vec![ms()]),
        (BlockKind::Debounce { ms: 0.0 }, "Debounce", vec![ms()]),
        (
            BlockKind::SetMute { signal: s() },
            "Set mute",
            vec![p("signal", "signal")],
        ),
        (
            BlockKind::SetFader {
                signal: s(),
                ramp_ms: 10.0,
            },
            "Set fader",
            vec![p("signal", "signal"), ramp()],
        ),
        (
            BlockKind::SetVca {
                vca: s(),
                ramp_ms: 10.0,
            },
            "Set VCA",
            vec![p("vca", "vca"), ramp()],
        ),
        (
            BlockKind::SetParam {
                signal: s(),
                node: s(),
                param: s(),
                ramp_ms: 10.0,
            },
            "Set node parameter",
            vec![
                p("signal", "signal"),
                p("node", "node"),
                p("param", "param"),
                ramp(),
            ],
        ),
        (
            BlockKind::SetPfl { signal: s() },
            "Set PFL",
            vec![p("signal", "signal-or-vca")],
        ),
        (
            BlockKind::SetTalk {
                mic: s(),
                destination: s(),
            },
            "Talkback",
            vec![p("mic", "mic"), p("destination", "destination")],
        ),
        (
            BlockKind::SetPublish { signal: s() },
            "Publish signal",
            vec![p("signal", "signal")],
        ),
        (
            BlockKind::SetMonitor {
                monitor: s(),
                source: None,
            },
            "Monitor source",
            vec![
                p("monitor", "monitor"),
                json!({"name": "source", "kind": "source", "default": null}),
            ],
        ),
        (
            BlockKind::LoudnessControl {
                signal: s(),
                action: LoudnessAction::Reset,
            },
            "Loudness measurement",
            vec![
                p("signal", "signal"),
                options("action", &["start", "stop", "reset"], "reset"),
            ],
        ),
        (
            BlockKind::Recall { snapshot: s() },
            "Recall snapshot",
            vec![p("snapshot", "snapshot")],
        ),
        (
            BlockKind::Webhook { url: s() },
            "HTTP webhook",
            vec![p("url", "url")],
        ),
        (
            BlockKind::TslOut { index: 0 },
            "TSL tally out",
            vec![int("index", 0.0, 65534.0)],
        ),
        (
            BlockKind::Log { message: s() },
            "Log / marker",
            vec![
                json!({"name": "message", "kind": "string", "max_length": MAX_TEXT, "default": ""}),
            ],
        ),
        (
            BlockKind::Macro { macro_id: s() },
            "Macro",
            vec![p("macro", "macro")],
        ),
        (
            BlockKind::MacroIn {
                name: s(),
                value_type: PortType::Boolean,
            },
            "Macro input",
            vec![
                p("name", "string"),
                json!({"name": "value_type", "kind": "port-type", "options": all_types, "default": "boolean"}),
            ],
        ),
        (
            BlockKind::MacroOut {
                name: s(),
                value_type: PortType::Boolean,
            },
            "Macro output",
            vec![
                p("name", "string"),
                json!({"name": "value_type", "kind": "port-type", "options": all_types, "default": "boolean"}),
            ],
        ),
    ];
    Value::Array(
        blocks
            .into_iter()
            .map(|(kind, label, params)| {
                let ports = kind.ports(&[]);
                json!({
                    "type": kind.type_name(),
                    "label": label,
                    "category": kind.category(),
                    "params": params,
                    "inputs": ports.inputs,
                    "outputs": ports.outputs,
                    "remembers": kind.remembers(),
                })
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalog_lists_every_type_once() {
        let c = catalog_json();
        let types: Vec<&str> = c
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["type"].as_str().unwrap())
            .collect();
        let unique: std::collections::BTreeSet<&str> = types.iter().copied().collect();
        assert_eq!(types.len(), unique.len());
        assert_eq!(types.len(), 48);
        let edge = c
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["type"] == "edge")
            .unwrap();
        assert_eq!(edge["outputs"], json!([{"name": "out", "type": "event"}]));
        assert_eq!(edge["remembers"], json!(true));
        assert_eq!(edge["category"], json!("logic"));
    }

    #[test]
    fn a_revision_follows_the_content() {
        let g = Graph {
            id: "g".into(),
            name: "G".into(),
            description: String::new(),
            enabled: true,
            dry_run: false,
            blocks: vec![],
            wires: vec![],
        };
        let mut h = g.clone();
        assert_eq!(revision(&g), revision(&h));
        assert_eq!(revision(&g).len(), 16);
        h.name = "H".into();
        assert_ne!(revision(&g), revision(&h));
    }
}
