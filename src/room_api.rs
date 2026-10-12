//! The control-room API (§10.1, §12.4):
//! - `GET`, `PATCH /api/v1/control-room`: source buttons, solo settings, SPL presets
//!   (and, in GET, the monitors and who is soloed);
//! - `GET`, `POST /api/v1/monitors`, `GET`, `PATCH`, `DELETE /api/v1/monitors/{id}`;
//! - `GET`, `DELETE /api/v1/solo` (everything), `PUT`, `DELETE /api/v1/solo/{signal}`
//!   with `{"kind": "pfl" | "afl"}`.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::api::{apply, bad, body, conflict, not_found, ramp, with_errors, ApiResult};
use crate::mix::chain::Proc;
use crate::mix::model::Db;
use crate::mix::room::{Check, Monitor, MonitorKind, SoloKind, SoloMode, Source};
use crate::mix::Format;
use crate::ops::App;

pub fn routes() -> Router<App> {
    Router::new()
        .route("/api/v1/control-room", get(room).patch(patch_room))
        .route("/api/v1/monitors", get(monitors).post(create_monitor))
        .route(
            "/api/v1/monitors/{id}",
            get(monitor).patch(patch_monitor).delete(delete_monitor),
        )
        .route("/api/v1/solo", get(solo).delete(clear_solo))
        .route("/api/v1/solo/{signal}", put(put_solo).delete(end_solo))
}

async fn run(f: impl std::future::Future<Output = ApiResult<Response>>) -> Response {
    f.await.unwrap_or_else(|r| *r)
}

fn room_json(app: &App) -> Value {
    let control = app.control();
    let mut v = json!(control.mix.control_room);
    v["soloed"] = json!(control.solo());
    v
}

async fn room(State(app): State<App>) -> Json<Value> {
    Json(room_json(&app))
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct SoloBody {
    mode: Option<SoloMode>,
    pfl_trim_db: Option<f64>,
    programme_db: Option<Db>,
    sip: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RoomBody {
    sources: Option<Vec<Source>>,
    solo: Option<SoloBody>,
    reference_spl: Option<Vec<f64>>,
}

async fn patch_room(State(app): State<App>, bytes: axum::body::Bytes) -> Response {
    run(async {
        let b: RoomBody = body(&bytes)?;
        let errors = apply(&app, crate::engine::FADE_SAMPLES, |m| {
            let r = &mut m.control_room;
            if let Some(v) = b.sources {
                r.sources = v;
            }
            if let Some(s) = b.solo {
                if let Some(v) = s.mode {
                    r.solo.mode = v;
                }
                if let Some(v) = s.pfl_trim_db {
                    r.solo.pfl_trim_db = v;
                }
                if let Some(v) = s.programme_db {
                    r.solo.programme_db = v;
                }
                if let Some(v) = s.sip {
                    r.solo.sip = v;
                }
            }
            if let Some(v) = b.reference_spl {
                r.reference_spl = v;
            }
            Ok(())
        })
        .await?;
        Ok(Json(with_errors(room_json(&app), errors)).into_response())
    })
    .await
}

async fn monitors(State(app): State<App>) -> Json<Value> {
    Json(json!(app.control().mix.control_room.monitors))
}

async fn monitor(State(app): State<App>, Path(id): Path<String>) -> Response {
    let found = app
        .control()
        .mix
        .control_room
        .monitors
        .iter()
        .find(|m| m.id == id)
        .cloned();
    match found {
        Some(m) => Json(json!(m)).into_response(),
        None => *not_found(format!("monitor {id} does not exist")),
    }
}

/// A field that is present (`Some`, perhaps `Some(None)` for `null`) or absent.
fn present<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(d).map(Some)
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct MonitorBody {
    id: Option<String>,
    name: Option<String>,
    kind: Option<MonitorKind>,
    format: Option<Format>,
    #[serde(default, deserialize_with = "present")]
    source: Option<Option<Source>>,
    level_db: Option<f64>,
    dim: Option<bool>,
    dim_db: Option<f64>,
    cut: Option<bool>,
    check: Option<Check>,
    follows_solo: Option<bool>,
    trim_db: Option<f64>,
    delay_ms: Option<f64>,
    chain: Option<Vec<Proc>>,
    #[serde(default, deserialize_with = "present")]
    calibration_spl: Option<Option<f64>>,
    publish: Option<bool>,
    ramp_ms: Option<f64>,
}

impl MonitorBody {
    fn apply(self, m: &mut Monitor) {
        macro_rules! set {
            ($($f:ident),*) => { $(if let Some(v) = self.$f { m.$f = v; })* };
        }
        set!(
            name,
            kind,
            format,
            source,
            level_db,
            dim,
            dim_db,
            cut,
            check,
            follows_solo,
            trim_db,
            delay_ms,
            chain,
            calibration_spl,
            publish
        );
    }
}

async fn create_monitor(State(app): State<App>, bytes: axum::body::Bytes) -> Response {
    run(async {
        let mut b: MonitorBody = body(&bytes)?;
        let format = b.format.ok_or_else(|| bad("format is required"))?;
        let id = match b.id.take() {
            Some(id) => id,
            None => {
                let c = app.control();
                let room = &c.mix.control_room;
                (1..)
                    .map(|n| format!("monitor-{n}"))
                    .find(|id| room.monitors.iter().all(|m| &m.id != id))
                    .expect("unbounded")
            }
        };
        let mut m: Monitor =
            serde_json::from_value(json!({"id": id, "name": id, "format": format}))
                .map_err(|e| bad(e.to_string()))?;
        b.ramp_ms = None;
        b.apply(&mut m);
        let errors = apply(&app, 0, |mix| {
            if mix.control_room.monitors.iter().any(|x| x.id == m.id) {
                return Err(conflict(format!("monitor {} exists", m.id)));
            }
            mix.control_room.monitors.push(m);
            Ok(())
        })
        .await?;
        let v = app
            .control()
            .mix
            .control_room
            .monitors
            .iter()
            .find(|x| x.id == id)
            .map(|m| json!(m))
            .unwrap_or_default();
        Ok((StatusCode::CREATED, Json(with_errors(v, errors))).into_response())
    })
    .await
}

async fn patch_monitor(
    State(app): State<App>,
    Path(id): Path<String>,
    bytes: axum::body::Bytes,
) -> Response {
    run(async {
        let mut b: MonitorBody = body(&bytes)?;
        if b.id.take().is_some_and(|new| new != id) {
            return Err(bad("a monitor id cannot be changed"));
        }
        let ramp = ramp(b.ramp_ms.take())?;
        let errors = apply(&app, ramp, |mix| {
            let m = mix
                .control_room
                .monitors
                .iter_mut()
                .find(|m| m.id == id)
                .ok_or_else(|| not_found(format!("monitor {id} does not exist")))?;
            b.apply(m);
            Ok(())
        })
        .await?;
        let v = app
            .control()
            .mix
            .control_room
            .monitors
            .iter()
            .find(|x| x.id == id)
            .map(|m| json!(m))
            .unwrap_or_default();
        Ok(Json(with_errors(v, errors)).into_response())
    })
    .await
}

async fn delete_monitor(State(app): State<App>, Path(id): Path<String>) -> Response {
    run(async {
        apply(&app, 0, |mix| {
            let before = mix.control_room.monitors.len();
            mix.control_room.monitors.retain(|m| m.id != id);
            if mix.control_room.monitors.len() == before {
                return Err(not_found(format!("monitor {id} does not exist")));
            }
            Ok(())
        })
        .await?;
        Ok(StatusCode::NO_CONTENT.into_response())
    })
    .await
}

async fn solo(State(app): State<App>) -> Json<Value> {
    Json(json!(app.control().solo()))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PutSolo {
    kind: SoloKind,
}

async fn put_solo(
    State(app): State<App>,
    Path(signal): Path<String>,
    bytes: axum::body::Bytes,
) -> Response {
    run(async {
        let b: PutSolo = body(&bytes)?;
        {
            let mut c = app.control();
            if c.mix.kind(&signal).is_none() {
                return Err(not_found(format!("signal {signal} does not exist")));
            }
            c.set_solo(&signal, Some(b.kind));
        }
        apply(&app, 0, |_| Ok(())).await?;
        Ok(Json(json!(app.control().solo())).into_response())
    })
    .await
}

async fn end_solo(State(app): State<App>, Path(signal): Path<String>) -> Response {
    run(async {
        app.control().set_solo(&signal, None);
        apply(&app, 0, |_| Ok(())).await?;
        Ok(Json(json!(app.control().solo())).into_response())
    })
    .await
}

async fn clear_solo(State(app): State<App>) -> Response {
    run(async {
        app.control().clear_solo();
        apply(&app, 0, |_| Ok(())).await?;
        Ok(Json(json!(app.control().solo())).into_response())
    })
    .await
}
