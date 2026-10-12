//! The preview API (§10.1, Appendix B.1):
//! - `GET /api/v1/preview`: mode (`own`, `shared`), the built-in MediaMTX, the WHEP
//!   base, the streams;
//! - `POST /api/v1/preview/{id}/listen`: someone listens to monitor `id` (repeat
//!   it while listening; a stream stops [`crate::preview::GRACE`] after the last);
//!   answers `{path, whep_url}` (`whep_url` null: the page's host and `whep_port`);
//! - personal monitors: `GET`, `POST /api/v1/personal` (`{"source"?}`),
//!   `PATCH`, `DELETE /api/v1/personal/{id}`, `PUT`, `DELETE
//!   /api/v1/personal/{id}/solo/{signal}` (`{"kind"}`).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::api::{apply, bad, body, not_found, ApiResult};
use crate::mix::room::{Check, SoloKind, Source};
use crate::ops::App;

pub fn routes() -> Router<App> {
    Router::new()
        .route("/api/v1/preview", get(preview))
        .route("/api/v1/preview/{id}/listen", post(listen))
        .route("/api/v1/personal", get(personal).post(create_personal))
        .route(
            "/api/v1/personal/{id}",
            get(one_personal)
                .patch(patch_personal)
                .delete(delete_personal),
        )
        .route(
            "/api/v1/personal/{id}/solo/{signal}",
            put(put_solo).delete(end_solo),
        )
}

async fn run(f: impl std::future::Future<Output = ApiResult<Response>>) -> Response {
    f.await.unwrap_or_else(|r| *r)
}

/// The preview summary (also in the status).
pub fn preview_json(app: &App) -> Value {
    let c = &app.config;
    let control = app.control();
    let own = c.preview_publish_url.is_none();
    json!({
        "mode": if own { "own" } else { "shared" },
        "prefix": c.preview_path_prefix,
        "whep_url": c.preview_whep_url,
        "whep_port": own.then_some(c.mediamtx_whep_port),
        "hls_url": c.preview_hls_url,
        "child": own.then(|| app.mediamtx.status()),
        "streams": control.streams.status(&control.stream_ids()),
        "max_personal": c.max_web_monitors,
    })
}

async fn preview(State(app): State<App>) -> Json<Value> {
    Json(preview_json(&app))
}

async fn listen(State(app): State<App>, Path(id): Path<String>) -> Response {
    run(async {
        let new = app.control().listen(&id).map_err(not_found)?;
        if new {
            apply(&app, 0, |_| Ok(())).await?;
        }
        let path = app.control().streams.path(&id);
        let whep = app
            .config
            .preview_whep_url
            .as_ref()
            .map(|b| format!("{b}/{path}/whep"));
        Ok(Json(
            json!({"path": path, "whep_url": whep, "whep_port": app.config.mediamtx_whep_port}),
        )
        .into_response())
    })
    .await
}

fn personal_json(p: &crate::mix::compile::Personal) -> Value {
    let mut v = json!(p.monitor);
    v["solo"] = json!(p.solo);
    v
}

async fn personal(State(app): State<App>) -> Json<Value> {
    Json(Value::Array(
        app.control().personal().iter().map(personal_json).collect(),
    ))
}

async fn one_personal(State(app): State<App>, Path(id): Path<String>) -> Response {
    let c = app.control();
    match c.personal().iter().find(|p| p.monitor.id == id) {
        Some(p) => Json(personal_json(p)).into_response(),
        None => *not_found(format!("personal monitor {id} does not exist")),
    }
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct CreateBody {
    source: Option<Source>,
}

async fn create_personal(State(app): State<App>, bytes: axum::body::Bytes) -> Response {
    run(async {
        let b: CreateBody = if bytes.is_empty() {
            CreateBody::default()
        } else {
            body(&bytes)?
        };
        let id = {
            let mut c = app.control();
            if let Some(Source::Signal(s)) = &b.source {
                if c.mix.kind(s).is_none() {
                    return Err(bad(format!("source {s} does not exist")));
                }
            }
            c.add_personal(b.source)
                .map_err(|e| Box::new(crate::ops::error(StatusCode::CONFLICT, e)))?
        };
        apply(&app, 0, |_| Ok(())).await?;
        let c = app.control();
        let p = c
            .personal()
            .iter()
            .find(|p| p.monitor.id == id)
            .map(personal_json);
        Ok((StatusCode::CREATED, Json(p.unwrap_or_default())).into_response())
    })
    .await
}

/// A field that is present (`Some`, perhaps `Some(None)` for `null`) or absent.
fn present<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(d).map(Some)
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct PatchBody {
    #[serde(default, deserialize_with = "present")]
    source: Option<Option<Source>>,
    level_db: Option<f64>,
    dim: Option<bool>,
    cut: Option<bool>,
    check: Option<Check>,
}

async fn patch_personal(
    State(app): State<App>,
    Path(id): Path<String>,
    bytes: axum::body::Bytes,
) -> Response {
    run(async {
        let b: PatchBody = body(&bytes)?;
        {
            let mut c = app.control();
            if let Some(Some(Source::Signal(s))) = &b.source {
                if c.mix.kind(s).is_none() {
                    return Err(bad(format!("source {s} does not exist")));
                }
            }
            if let Some(l) = b.level_db {
                if !(-80.0..=12.0).contains(&l) {
                    return Err(bad("level_db must be -80 to 12"));
                }
            }
            c.with_personal(&id, |p| {
                let m = &mut p.monitor;
                if let Some(v) = b.source {
                    m.source = v;
                }
                if let Some(v) = b.level_db {
                    m.level_db = v;
                }
                if let Some(v) = b.dim {
                    m.dim = v;
                }
                if let Some(v) = b.cut {
                    m.cut = v;
                }
                if let Some(v) = b.check {
                    m.check = v;
                }
            })
            .ok_or_else(|| not_found(format!("personal monitor {id} does not exist")))?;
        }
        apply(&app, 0, |_| Ok(())).await?;
        Ok(one_personal(State(app), Path(id)).await)
    })
    .await
}

async fn delete_personal(State(app): State<App>, Path(id): Path<String>) -> Response {
    run(async {
        if !app.control().remove_personal(&id) {
            return Err(not_found(format!("personal monitor {id} does not exist")));
        }
        apply(&app, 0, |_| Ok(())).await?;
        Ok(StatusCode::NO_CONTENT.into_response())
    })
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SoloBody {
    kind: SoloKind,
}

async fn put_solo(
    State(app): State<App>,
    Path((id, signal)): Path<(String, String)>,
    bytes: axum::body::Bytes,
) -> Response {
    run(async {
        let b: SoloBody = body(&bytes)?;
        {
            let mut c = app.control();
            if c.mix.kind(&signal).is_none() {
                return Err(not_found(format!("signal {signal} does not exist")));
            }
            let interlock = c.mix.control_room.solo.mode == crate::mix::room::SoloMode::Interlock;
            c.with_personal(&id, |p| {
                if interlock {
                    p.solo.clear();
                }
                p.solo.insert(signal, b.kind);
            })
            .ok_or_else(|| not_found(format!("personal monitor {id} does not exist")))?;
        }
        apply(&app, 0, |_| Ok(())).await?;
        Ok(one_personal(State(app), Path(id)).await)
    })
    .await
}

async fn end_solo(State(app): State<App>, Path((id, signal)): Path<(String, String)>) -> Response {
    run(async {
        app.control()
            .with_personal(&id, |p| p.solo.remove(&signal))
            .ok_or_else(|| not_found(format!("personal monitor {id} does not exist")))?;
        apply(&app, 0, |_| Ok(())).await?;
        Ok(one_personal(State(app), Path(id)).await)
    })
    .await
}
