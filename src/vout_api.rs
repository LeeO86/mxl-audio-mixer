//! The virtual output API (C7): fixed MXL flows and NMOS senders from
//! `MIXER_OUTPUT_FLOWS` / `MIXER_OUTPUTS` that the designer wires; the operator
//! (or a logic action) picks what feeds each.
//! - `GET /api/v1/virtual-outputs`, `GET /api/v1/virtual-outputs/{output}`;
//! - `PUT /api/v1/virtual-outputs/{output}` with `{"source": <source> | null}`;
//! - `DELETE /api/v1/virtual-outputs/{output}`: no source (silence; also when it had none).
//!
//! `{output}` is the slot number (from 1) or the output's configured `key`.
//!
//! A source is `{"signal": "<strip or bus>", "tap"?: <tap>}`, `{"monitor":
//! "<monitor id>"}` or `{"input": <n>, "format"?: "<format>"}` (see
//! [`OutputSource`]). The flow's format never changes; another source format is
//! up- or down-mixed like a send.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::api::{apply, body, not_found, ApiResult};
use crate::mix::compile::vout_key;
use crate::mix::model::OutputSource;
use crate::ops::App;

pub fn routes() -> Router<App> {
    Router::new()
        .route("/api/v1/virtual-outputs", get(list))
        .route(
            "/api/v1/virtual-outputs/{output}",
            get(one).put(put_source).delete(clear_source),
        )
}

async fn run(f: impl std::future::Future<Output = ApiResult<Response>>) -> Response {
    f.await.unwrap_or_else(|r| *r)
}

/// Every virtual output: its slot, id, key, label, format, source, ids and state.
pub fn status(control: &crate::control::Control) -> Vec<Value> {
    let outputs = control.output_status();
    control
        .virtual_outputs
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let key = vout_key(&v.id);
            let o = outputs.iter().find(|o| o.signal == key);
            json!({
                "slot": i + 1,
                "id": v.id,
                "key": v.key,
                "signal": key,
                "label": v.label,
                "format": v.format,
                "source": control.mix.virtual_source(&v.id),
                "nmos_sender_id": o.map(|o| o.nmos_sender_id),
                "mxl_flow_id": o.map(|o| o.mxl_flow_id),
                "running": o.is_some(),
                "errors": o.map_or(0, |o| o.errors),
                "commit_lag_ms": o.map(|o| o.commit_lag_ms),
            })
        })
        .collect()
}

/// The slot index (from 0) and id of `output`: a slot number or a key.
fn find(app: &App, output: &str) -> ApiResult<(usize, String)> {
    let control = app.control();
    let list = &control.virtual_outputs;
    let at = match output.parse::<usize>() {
        Ok(n) if (1..=list.len()).contains(&n) => Some(n - 1),
        Ok(_) => None,
        Err(_) => list.iter().position(|v| v.key.as_deref() == Some(output)),
    };
    at.map(|i| (i, list[i].id.clone())).ok_or_else(|| {
        not_found(format!(
            "virtual output {output} does not exist (MIXER_OUTPUT_FLOWS is {})",
            list.len()
        ))
    })
}

async fn list(State(app): State<App>) -> Json<Value> {
    Json(json!(status(&app.control())))
}

async fn one(State(app): State<App>, Path(output): Path<String>) -> Response {
    run(async {
        let (i, _) = find(&app, &output)?;
        Ok(Json(status(&app.control()).swap_remove(i)).into_response())
    })
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceBody {
    source: Option<OutputSource>,
}

async fn put_source(
    State(app): State<App>,
    Path(output): Path<String>,
    bytes: axum::body::Bytes,
) -> Response {
    run(async {
        let (i, id) = find(&app, &output)?;
        let b: SourceBody = body(&bytes)?;
        apply(&app, 0, |m| {
            m.set_virtual_source(&id, b.source);
            Ok(())
        })
        .await?;
        Ok(Json(status(&app.control()).swap_remove(i)).into_response())
    })
    .await
}

async fn clear_source(State(app): State<App>, Path(output): Path<String>) -> Response {
    run(async {
        let (_, id) = find(&app, &output)?;
        apply(&app, 0, |m| {
            m.set_virtual_source(&id, None);
            Ok(())
        })
        .await?;
        Ok(StatusCode::NO_CONTENT.into_response())
    })
    .await
}
