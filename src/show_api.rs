//! The show API (§12.1, §12.5): snapshots, layouts and the show history.
//!
//! - `GET /api/v1/snapshots`, `POST` `{id?, name?}` (take one)
//! - `GET|PUT|PATCH|DELETE /api/v1/snapshots/{id}` (PUT takes it again, PATCH
//!   `{name}` renames)
//! - `POST /api/v1/snapshots/{id}/recall` `{signals?, sections?, classes?,
//!   crossfade_ms?}`: recall-safe signals are left alone
//! - `GET|PUT /api/v1/layouts`
//! - `GET /api/v1/shows/history`, `POST` `{name}` (keep a named version)
//! - `GET|DELETE /api/v1/shows/history/{id}`, `POST …/{id}/restore`

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::api::{apply, bad, body, conflict, internal, not_found, ramp, with_errors, ApiResult};
use crate::history::now_ms;
use crate::mix::show::{capture, recall, Filter, Layout, Snapshot};
use crate::ops::App;

pub fn routes() -> Router<App> {
    Router::new()
        .route("/api/v1/snapshots", get(list).post(take))
        .route(
            "/api/v1/snapshots/{id}",
            get(snapshot).put(retake).patch(rename).delete(remove),
        )
        .route("/api/v1/snapshots/{id}/recall", post(recall_snapshot))
        .route("/api/v1/layouts", get(layouts).put(put_layouts))
        .route("/api/v1/shows/history", get(history).post(keep_version))
        .route(
            "/api/v1/shows/history/{id}",
            get(version).delete(delete_version),
        )
        .route("/api/v1/shows/history/{id}/restore", post(restore))
}

fn summary(s: &Snapshot) -> Value {
    json!({"id": s.id, "name": s.name, "time": s.time, "signals": s.signals.len(), "sends": s.sends.len()})
}

async fn run(f: impl std::future::Future<Output = ApiResult<Response>>) -> Response {
    f.await.unwrap_or_else(|r| *r)
}

async fn list(State(app): State<App>) -> Json<Value> {
    Json(Value::Array(
        app.control().mix.snapshots.iter().map(summary).collect(),
    ))
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct TakeBody {
    id: Option<String>,
    name: Option<String>,
}

fn unix_s() -> u64 {
    now_ms() / 1000
}

async fn take(State(app): State<App>, bytes: axum::body::Bytes) -> Response {
    run(async {
        let b: TakeBody = if bytes.is_empty() {
            TakeBody::default()
        } else {
            body(&bytes)?
        };
        let id = b.id.unwrap_or_else(|| {
            let m = &app.control().mix;
            (1..)
                .map(|n| format!("snapshot-{n}"))
                .find(|id| m.snapshots.iter().all(|s| &s.id != id))
                .expect("unbounded")
        });
        let name = b.name.unwrap_or_else(|| id.clone());
        let mut taken = None;
        apply(&app, 0, |m| {
            if m.snapshots.iter().any(|s| s.id == id) {
                return Err(conflict(format!("snapshot {id} exists")));
            }
            let s = capture(m, id.clone(), name, unix_s());
            taken = Some(summary(&s));
            m.snapshots.push(s);
            Ok(())
        })
        .await?;
        Ok((StatusCode::CREATED, Json(taken.unwrap_or_default())).into_response())
    })
    .await
}

async fn snapshot(State(app): State<App>, Path(id): Path<String>) -> Response {
    let found = app
        .control()
        .mix
        .snapshots
        .iter()
        .find(|s| s.id == id)
        .cloned();
    match found {
        Some(s) => Json(json!(s)).into_response(),
        None => *not_found(format!("snapshot {id} does not exist")),
    }
}

/// Change snapshot `id` with `f`.
async fn with_snapshot(
    app: &App,
    id: &str,
    f: impl FnOnce(&crate::mix::Mix, &mut Snapshot) -> ApiResult<()>,
) -> ApiResult<Value> {
    let mut out = Value::Null;
    apply(app, 0, |m| {
        let mut s = m
            .snapshots
            .iter()
            .find(|s| s.id == id)
            .cloned()
            .ok_or_else(|| not_found(format!("snapshot {id} does not exist")))?;
        f(m, &mut s)?;
        out = summary(&s);
        if let Some(slot) = m.snapshots.iter_mut().find(|x| x.id == id) {
            *slot = s;
        }
        Ok(())
    })
    .await?;
    Ok(out)
}

async fn retake(State(app): State<App>, Path(id): Path<String>) -> Response {
    run(async {
        let v = with_snapshot(&app, &id, |m, s| {
            *s = capture(m, s.id.clone(), s.name.clone(), unix_s());
            Ok(())
        })
        .await?;
        Ok(Json(v).into_response())
    })
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RenameBody {
    name: String,
}

async fn rename(
    State(app): State<App>,
    Path(id): Path<String>,
    bytes: axum::body::Bytes,
) -> Response {
    run(async {
        let b: RenameBody = body(&bytes)?;
        let v = with_snapshot(&app, &id, |_, s| {
            s.name = b.name;
            Ok(())
        })
        .await?;
        Ok(Json(v).into_response())
    })
    .await
}

async fn remove(State(app): State<App>, Path(id): Path<String>) -> Response {
    run(async {
        apply(&app, 0, |m| {
            let before = m.snapshots.len();
            m.snapshots.retain(|s| s.id != id);
            if m.snapshots.len() == before {
                return Err(not_found(format!("snapshot {id} does not exist")));
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
struct RecallBody {
    #[serde(flatten)]
    filter: Filter,
    crossfade_ms: Option<f64>,
}

async fn recall_snapshot(
    State(app): State<App>,
    Path(id): Path<String>,
    bytes: axum::body::Bytes,
) -> Response {
    run(async {
        let b: RecallBody = if bytes.is_empty() {
            RecallBody::default()
        } else {
            body(&bytes)?
        };
        let ramp = ramp(b.crossfade_ms)?;
        let mut touched = 0;
        let errors = apply(&app, ramp, |m| {
            let snap = m
                .snapshots
                .iter()
                .find(|s| s.id == id)
                .cloned()
                .ok_or_else(|| not_found(format!("snapshot {id} does not exist")))?;
            touched = recall(m, &snap, &b.filter);
            Ok(())
        })
        .await?;
        Ok(Json(with_errors(
            json!({"recalled": id, "signals": touched}),
            errors,
        ))
        .into_response())
    })
    .await
}

async fn layouts(State(app): State<App>) -> Json<Value> {
    Json(json!(app.control().mix.layouts))
}

async fn put_layouts(State(app): State<App>, bytes: axum::body::Bytes) -> Response {
    run(async {
        let layouts: Vec<Layout> = body(&bytes)?;
        apply(&app, 0, |m| {
            m.layouts = layouts;
            Ok(())
        })
        .await?;
        Ok(Json(json!(app.control().mix.layouts)).into_response())
    })
    .await
}

async fn history(State(app): State<App>) -> Json<Value> {
    Json(json!(app.control().history.list()))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VersionBody {
    name: String,
}

async fn keep_version(State(app): State<App>, bytes: axum::body::Bytes) -> Response {
    run(async {
        let b: VersionBody = body(&bytes)?;
        if b.name.trim().is_empty() || b.name.chars().count() > 64 {
            return Err(bad("a name must have 1 to 64 characters"));
        }
        let control = app.control();
        let id = control
            .history
            .record(&control.mix, Some(b.name))
            .map_err(internal)?;
        Ok((StatusCode::CREATED, Json(json!({"id": id}))).into_response())
    })
    .await
}

async fn version(State(app): State<App>, Path(id): Path<String>) -> Response {
    match app.control().history.get(&id) {
        Ok(mix) => Json(json!(mix)).into_response(),
        Err(e) => *not_found(e),
    }
}

async fn delete_version(State(app): State<App>, Path(id): Path<String>) -> Response {
    match app.control().history.delete(&id) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => *not_found(e),
    }
}

/// Load a version as the show. The show before it is kept as a version first.
async fn restore(State(app): State<App>, Path(id): Path<String>) -> Response {
    run(async {
        let mix = {
            let control = app.control();
            let mix = control.history.get(&id).map_err(not_found)?;
            control
                .history
                .record(&control.mix, None)
                .map_err(internal)?;
            mix
        };
        let errors = apply(&app, crate::engine::FADE_SAMPLES, |m| {
            *m = mix;
            Ok(())
        })
        .await?;
        Ok(Json(with_errors(json!({"restored": id}), errors)).into_response())
    })
    .await
}
