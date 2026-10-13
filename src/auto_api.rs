//! The control engine's API (§8, §12.4):
//! - `GET /api/v1/vad`, `GET`, `PUT`, `DELETE /api/v1/vad/{signal}`: every
//!   analysed signal with its settings (`own`: set for it, else the defaults)
//!   and its state; `PUT` sets a signal's own settings (`threshold`, `hold_ms`,
//!   `gate_lufs`, `detector`: `silero` or `features`) and analyses it; `DELETE`
//!   removes them;
//! - `GET`, `POST /api/v1/automixes`, `GET`, `PATCH`, `DELETE
//!   /api/v1/automixes/{id}` (`name`, `enabled`, `last_mic_hold`,
//!   `vad_weighting`, `recall_safe`, `members`: strip ids or `{signal,
//!   weight_db, bypass}`; a
//!   bare id keeps that member's weight and bypass, an object sets them);
//!   GET adds `gains_db`;
//! - `GET`, `POST /api/v1/ducks`, `GET`, `PATCH`, `DELETE /api/v1/ducks/{id}`
//!   (`name`, `enabled`, `keys`, `targets`, `mode`: `fixed` or `ratio`;
//!   fixed: `depth_db`, `attack_ms`, `hold_ms`, `release_ms`; ratio: `ratio`
//!   {`target_lu`, `overvoice_lu`, `max_reduction_db`, `speed`, `bed_voice`};
//!   `channel_depth` {`front`, `center`, `lfe`, `surround`}; `recall_safe`;
//!   the fields of `ratio` and `channel_depth` given are set, the others
//!   kept); GET adds `active`, `gain_db` and for a ratio duck `control` (its
//!   state, the stages' reductions, the ratio aimed at and held, the
//!   loudness of dialogue and bed);
//! - `GET /api/v1/autolevels`, `GET`, `PUT`, `DELETE
//!   /api/v1/autolevels/{signal}` (`enabled`, `target_lufs`,
//!   `speech_offset_lu`, `max_gain_db`, `min_gain_db`, `attack_ms`,
//!   `release_ms`, `freeze_lufs`, `recall_safe`; `PUT` sets the fields given,
//!   the others kept or the defaults); GET adds `state` (the gain, the level,
//!   the target in force, frozen, speech).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::api::{apply, bad, body, conflict, not_found, ApiResult};
use crate::engine::FADE_SAMPLES;
use crate::mix::auto::{AutoLevel, Automix, AutomixMember, Duck, DuckMode, VadSettings};
use crate::mix::model::Db;
use crate::ops::App;

pub fn routes() -> Router<App> {
    Router::new()
        .route("/api/v1/vad", get(vads))
        .route(
            "/api/v1/vad/{signal}",
            get(vad).put(put_vad).delete(delete_vad),
        )
        .route("/api/v1/automixes", get(automixes).post(create_automix))
        .route(
            "/api/v1/automixes/{id}",
            get(automix).patch(patch_automix).delete(delete_automix),
        )
        .route("/api/v1/ducks", get(ducks).post(create_duck))
        .route(
            "/api/v1/ducks/{id}",
            get(duck).patch(patch_duck).delete(delete_duck),
        )
        .route("/api/v1/autolevels", get(autolevels))
        .route(
            "/api/v1/autolevels/{signal}",
            get(autolevel).put(put_autolevel).delete(delete_autolevel),
        )
}

async fn run(f: impl std::future::Future<Output = ApiResult<Response>>) -> Response {
    f.await.unwrap_or_else(|r| *r)
}

fn vad_json(app: &App, signal: &str) -> Option<Value> {
    let c = app.control();
    let settings = c.mix.vad_signals().remove(signal)?;
    let mut j = json!(settings);
    j["own"] = json!(c.mix.vad.iter().any(|v| v.signal == signal));
    j["state"] = json!(c.auto.vad_state(signal));
    Some(j)
}

async fn vads(State(app): State<App>) -> Json<Value> {
    let signals: Vec<String> = app.control().mix.vad_signals().into_keys().collect();
    Json(Value::Array(
        signals.iter().filter_map(|s| vad_json(&app, s)).collect(),
    ))
}

async fn vad(State(app): State<App>, Path(signal): Path<String>) -> Response {
    match vad_json(&app, &signal) {
        Some(v) => Json(v).into_response(),
        None => *not_found(format!("{signal} is not analysed")),
    }
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct VadBody {
    threshold: Option<f64>,
    hold_ms: Option<f64>,
    gate_lufs: Option<f64>,
    detector: Option<crate::mix::auto::VadDetector>,
}

async fn put_vad(
    State(app): State<App>,
    Path(signal): Path<String>,
    bytes: axum::body::Bytes,
) -> Response {
    run(async {
        let b: VadBody = if bytes.is_empty() {
            VadBody::default()
        } else {
            body(&bytes)?
        };
        let mut created = false;
        apply(&app, FADE_SAMPLES, |mix| {
            if mix.kind(&signal).is_none() {
                return Err(not_found(format!("signal {signal} does not exist")));
            }
            let current = mix
                .vad_signals()
                .remove(&signal)
                .unwrap_or_else(|| VadSettings::new(&signal));
            let v = VadSettings {
                signal: signal.clone(),
                threshold: b.threshold.unwrap_or(current.threshold),
                hold_ms: b.hold_ms.unwrap_or(current.hold_ms),
                gate_lufs: b.gate_lufs.unwrap_or(current.gate_lufs),
                detector: b.detector.unwrap_or(current.detector),
            };
            match mix.vad.iter_mut().find(|x| x.signal == signal) {
                Some(x) => *x = v,
                None => {
                    created = true;
                    mix.vad.push(v);
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
        Ok((status, Json(vad_json(&app, &signal).unwrap_or_default())).into_response())
    })
    .await
}

async fn delete_vad(State(app): State<App>, Path(signal): Path<String>) -> Response {
    run(async {
        apply(&app, FADE_SAMPLES, |mix| {
            let before = mix.vad.len();
            mix.vad.retain(|v| v.signal != signal);
            if mix.vad.len() == before {
                return Err(not_found(format!(
                    "{signal} has no VAD settings of its own"
                )));
            }
            Ok(())
        })
        .await?;
        Ok(StatusCode::NO_CONTENT.into_response())
    })
    .await
}

fn automix_json(app: &App, a: &Automix) -> Value {
    let mut j = json!(a);
    j["gains_db"] = json!(app
        .control()
        .auto
        .automix_status(&a.id)
        .map(|s| s.gains_db)
        .unwrap_or_default());
    j
}

fn one_automix(app: &App, id: &str) -> Option<Value> {
    let a = app.control().mix.automix(id).cloned()?;
    Some(automix_json(app, &a))
}

async fn automixes(State(app): State<App>) -> Json<Value> {
    let list = app.control().mix.automixes.clone();
    Json(Value::Array(
        list.iter().map(|a| automix_json(&app, a)).collect(),
    ))
}

async fn automix(State(app): State<App>, Path(id): Path<String>) -> Response {
    match one_automix(&app, &id) {
        Some(v) => Json(v).into_response(),
        None => *not_found(format!("auto-mix {id} does not exist")),
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum MemberBody {
    Id(String),
    Member(AutomixMember),
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct AutomixBody {
    id: Option<String>,
    name: Option<String>,
    enabled: Option<bool>,
    last_mic_hold: Option<bool>,
    vad_weighting: Option<bool>,
    members: Option<Vec<MemberBody>>,
    recall_safe: Option<bool>,
}

impl AutomixBody {
    fn apply(self, a: &mut Automix) {
        macro_rules! set {
            ($($f:ident),*) => { $(if let Some(x) = self.$f { a.$f = x; })* };
        }
        set!(name, enabled, last_mic_hold, vad_weighting, recall_safe);
        if let Some(members) = self.members {
            a.members = members
                .into_iter()
                .map(|m| match m {
                    MemberBody::Id(s) => a
                        .members
                        .iter()
                        .find(|x| x.signal == s)
                        .cloned()
                        .unwrap_or_else(|| AutomixMember::new(&s)),
                    MemberBody::Member(m) => m,
                })
                .collect();
        }
    }
}

async fn create_automix(State(app): State<App>, bytes: axum::body::Bytes) -> Response {
    run(async {
        let mut b: AutomixBody = body(&bytes)?;
        let id = match b.id.take() {
            Some(id) => id,
            None => {
                let c = app.control();
                (1..)
                    .map(|n| format!("automix-{n}"))
                    .find(|id| c.mix.automix(id).is_none())
                    .expect("unbounded")
            }
        };
        let mut a: Automix = serde_json::from_value(json!({"id": id, "name": id}))
            .map_err(|e| bad(e.to_string()))?;
        b.apply(&mut a);
        apply(&app, FADE_SAMPLES, |mix| {
            if mix.automix(&a.id).is_some() {
                return Err(conflict(format!("auto-mix {} exists", a.id)));
            }
            mix.automixes.push(a);
            Ok(())
        })
        .await?;
        Ok((
            StatusCode::CREATED,
            Json(one_automix(&app, &id).unwrap_or_default()),
        )
            .into_response())
    })
    .await
}

async fn patch_automix(
    State(app): State<App>,
    Path(id): Path<String>,
    bytes: axum::body::Bytes,
) -> Response {
    run(async {
        let mut b: AutomixBody = body(&bytes)?;
        if b.id.take().is_some_and(|new| new != id) {
            return Err(bad("an auto-mix id cannot be changed"));
        }
        apply(&app, FADE_SAMPLES, |mix| {
            let a = mix
                .automixes
                .iter_mut()
                .find(|a| a.id == id)
                .ok_or_else(|| not_found(format!("auto-mix {id} does not exist")))?;
            b.apply(a);
            Ok(())
        })
        .await?;
        Ok(Json(one_automix(&app, &id).unwrap_or_default()).into_response())
    })
    .await
}

async fn delete_automix(State(app): State<App>, Path(id): Path<String>) -> Response {
    run(async {
        apply(&app, FADE_SAMPLES, |mix| {
            let before = mix.automixes.len();
            mix.automixes.retain(|a| a.id != id);
            if mix.automixes.len() == before {
                return Err(not_found(format!("auto-mix {id} does not exist")));
            }
            Ok(())
        })
        .await?;
        Ok(StatusCode::NO_CONTENT.into_response())
    })
    .await
}

fn duck_json(app: &App, d: &Duck) -> Value {
    let mut j = json!(d);
    let s = app.control().auto.duck_status(&d.id);
    j["active"] = json!(s.active);
    j["gain_db"] = json!(s.gain_db);
    if let Some(r) = s.ratio {
        j["control"] = json!(r);
    }
    j
}

fn one_duck(app: &App, id: &str) -> Option<Value> {
    let d = app.control().mix.duck(id).cloned()?;
    Some(duck_json(app, &d))
}

async fn ducks(State(app): State<App>) -> Json<Value> {
    let list = app.control().mix.ducks.clone();
    Json(Value::Array(
        list.iter().map(|d| duck_json(&app, d)).collect(),
    ))
}

async fn duck(State(app): State<App>, Path(id): Path<String>) -> Response {
    match one_duck(&app, &id) {
        Some(v) => Json(v).into_response(),
        None => *not_found(format!("duck {id} does not exist")),
    }
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct DuckBody {
    id: Option<String>,
    name: Option<String>,
    enabled: Option<bool>,
    keys: Option<Vec<String>>,
    targets: Option<Vec<String>>,
    mode: Option<DuckMode>,
    depth_db: Option<Db>,
    attack_ms: Option<f64>,
    hold_ms: Option<f64>,
    release_ms: Option<f64>,
    ratio: Option<RatioBody>,
    channel_depth: Option<DepthBody>,
    recall_safe: Option<bool>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RatioBody {
    target_lu: Option<f64>,
    overvoice_lu: Option<f64>,
    max_reduction_db: Option<f64>,
    speed: Option<f64>,
    bed_voice: Option<bool>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct DepthBody {
    front: Option<f64>,
    center: Option<f64>,
    lfe: Option<f64>,
    surround: Option<f64>,
}

macro_rules! set {
    ($from:expr, $to:expr; $($f:ident),*) => { $(if let Some(x) = $from.$f { $to.$f = x; })* };
}

impl DuckBody {
    fn apply(self, d: &mut Duck) {
        set!(self, d; name, enabled, keys, targets, mode, depth_db, attack_ms, hold_ms, release_ms, recall_safe);
        if let Some(r) = self.ratio {
            set!(r, d.ratio; target_lu, overvoice_lu, max_reduction_db, speed, bed_voice);
        }
        if let Some(c) = self.channel_depth {
            set!(c, d.channel_depth; front, center, lfe, surround);
        }
    }
}

async fn create_duck(State(app): State<App>, bytes: axum::body::Bytes) -> Response {
    run(async {
        let mut b: DuckBody = body(&bytes)?;
        let id = match b.id.take() {
            Some(id) => id,
            None => {
                let c = app.control();
                (1..)
                    .map(|n| format!("duck-{n}"))
                    .find(|id| c.mix.duck(id).is_none())
                    .expect("unbounded")
            }
        };
        let mut d = Duck::new(&id);
        b.apply(&mut d);
        apply(&app, FADE_SAMPLES, |mix| {
            if mix.duck(&d.id).is_some() {
                return Err(conflict(format!("duck {} exists", d.id)));
            }
            mix.ducks.push(d);
            Ok(())
        })
        .await?;
        Ok((
            StatusCode::CREATED,
            Json(one_duck(&app, &id).unwrap_or_default()),
        )
            .into_response())
    })
    .await
}

async fn patch_duck(
    State(app): State<App>,
    Path(id): Path<String>,
    bytes: axum::body::Bytes,
) -> Response {
    run(async {
        let mut b: DuckBody = body(&bytes)?;
        if b.id.take().is_some_and(|new| new != id) {
            return Err(bad("a duck id cannot be changed"));
        }
        apply(&app, FADE_SAMPLES, |mix| {
            let d = mix
                .ducks
                .iter_mut()
                .find(|d| d.id == id)
                .ok_or_else(|| not_found(format!("duck {id} does not exist")))?;
            b.apply(d);
            Ok(())
        })
        .await?;
        Ok(Json(one_duck(&app, &id).unwrap_or_default()).into_response())
    })
    .await
}

fn autolevel_json(app: &App, signal: &str) -> Option<Value> {
    let c = app.control();
    let l = c.mix.autolevel(signal)?;
    let mut j = json!(l);
    j["state"] = json!(c.auto.autolevel_status(signal));
    Some(j)
}

async fn autolevels(State(app): State<App>) -> Json<Value> {
    let signals: Vec<String> = app
        .control()
        .mix
        .autolevels
        .iter()
        .map(|l| l.signal.clone())
        .collect();
    Json(Value::Array(
        signals
            .iter()
            .filter_map(|s| autolevel_json(&app, s))
            .collect(),
    ))
}

async fn autolevel(State(app): State<App>, Path(signal): Path<String>) -> Response {
    match autolevel_json(&app, &signal) {
        Some(v) => Json(v).into_response(),
        None => *not_found(format!("{signal} has no auto-level")),
    }
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct AutoLevelBody {
    enabled: Option<bool>,
    target_lufs: Option<f64>,
    speech_offset_lu: Option<f64>,
    max_gain_db: Option<f64>,
    min_gain_db: Option<f64>,
    attack_ms: Option<f64>,
    release_ms: Option<f64>,
    freeze_lufs: Option<f64>,
    recall_safe: Option<bool>,
}

async fn put_autolevel(
    State(app): State<App>,
    Path(signal): Path<String>,
    bytes: axum::body::Bytes,
) -> Response {
    run(async {
        let b: AutoLevelBody = if bytes.is_empty() {
            AutoLevelBody::default()
        } else {
            body(&bytes)?
        };
        let mut created = false;
        apply(&app, FADE_SAMPLES, |mix| {
            if mix.kind(&signal).is_none() {
                return Err(not_found(format!("signal {signal} does not exist")));
            }
            let mut l = mix
                .autolevel(&signal)
                .cloned()
                .unwrap_or_else(|| AutoLevel::new(&signal));
            set!(b, l; enabled, target_lufs, speech_offset_lu, max_gain_db, min_gain_db, attack_ms, release_ms, freeze_lufs, recall_safe);
            match mix.autolevels.iter_mut().find(|x| x.signal == signal) {
                Some(x) => *x = l,
                None => {
                    created = true;
                    mix.autolevels.push(l);
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
        Ok((
            status,
            Json(autolevel_json(&app, &signal).unwrap_or_default()),
        )
            .into_response())
    })
    .await
}

async fn delete_autolevel(State(app): State<App>, Path(signal): Path<String>) -> Response {
    run(async {
        apply(&app, FADE_SAMPLES, |mix| {
            let before = mix.autolevels.len();
            mix.autolevels.retain(|l| l.signal != signal);
            if mix.autolevels.len() == before {
                return Err(not_found(format!("{signal} has no auto-level")));
            }
            Ok(())
        })
        .await?;
        Ok(StatusCode::NO_CONTENT.into_response())
    })
    .await
}

async fn delete_duck(State(app): State<App>, Path(id): Path<String>) -> Response {
    run(async {
        apply(&app, FADE_SAMPLES, |mix| {
            let before = mix.ducks.len();
            mix.ducks.retain(|d| d.id != id);
            if mix.ducks.len() == before {
                return Err(not_found(format!("duck {id} does not exist")));
            }
            Ok(())
        })
        .await?;
        Ok(StatusCode::NO_CONTENT.into_response())
    })
    .await
}
