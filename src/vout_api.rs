//! The virtual output API (C7): fixed MXL flows and NMOS senders from
//! `MIXER_OUTPUT_FLOWS` / `MIXER_OUTPUTS` that the designer wires; the operator
//! (or a logic action) picks what feeds each.
//! - `GET /api/v1/virtual-outputs`, `GET /api/v1/virtual-outputs/{slot}`;
//! - `PUT /api/v1/virtual-outputs/{slot}` with `{"source": <source> | null}`;
//! - `DELETE /api/v1/virtual-outputs/{slot}`: no source (silence; also when it had none).
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
            "/api/v1/virtual-outputs/{slot}",
            get(one).put(put_source).delete(clear_source),
        )
}

async fn run(f: impl std::future::Future<Output = ApiResult<Response>>) -> Response {
    f.await.unwrap_or_else(|r| *r)
}

/// Every virtual output: its slot, label, format, source, ids and state.
pub fn status(control: &crate::control::Control) -> Vec<Value> {
    let outputs = control.output_status();
    control
        .virtual_outputs
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let slot = i + 1;
            let key = vout_key(slot);
            let o = outputs.iter().find(|o| o.signal == key);
            json!({
                "slot": slot,
                "signal": key,
                "label": v.label,
                "format": v.format,
                "source": control.mix.virtual_source(slot),
                "nmos_sender_id": o.map(|o| o.nmos_sender_id),
                "mxl_flow_id": o.map(|o| o.mxl_flow_id),
                "running": o.is_some(),
                "errors": o.map_or(0, |o| o.errors),
                "commit_lag_ms": o.map(|o| o.commit_lag_ms),
            })
        })
        .collect()
}

fn slot_of(app: &App, slot: usize) -> ApiResult<usize> {
    let n = app.control().virtual_outputs.len();
    if slot == 0 || slot > n {
        return Err(not_found(format!(
            "virtual output {slot} does not exist (MIXER_OUTPUT_FLOWS is {n})"
        )));
    }
    Ok(slot)
}

async fn list(State(app): State<App>) -> Json<Value> {
    Json(json!(status(&app.control())))
}

async fn one(State(app): State<App>, Path(slot): Path<usize>) -> Response {
    run(async {
        let slot = slot_of(&app, slot)?;
        Ok(Json(status(&app.control()).swap_remove(slot - 1)).into_response())
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
    Path(slot): Path<usize>,
    bytes: axum::body::Bytes,
) -> Response {
    run(async {
        let slot = slot_of(&app, slot)?;
        let b: SourceBody = body(&bytes)?;
        apply(&app, 0, |m| {
            m.set_virtual_source(slot, b.source);
            Ok(())
        })
        .await?;
        Ok(Json(status(&app.control()).swap_remove(slot - 1)).into_response())
    })
    .await
}

async fn clear_source(State(app): State<App>, Path(slot): Path<usize>) -> Response {
    run(async {
        let slot = slot_of(&app, slot)?;
        apply(&app, 0, |m| {
            m.set_virtual_source(slot, None);
            Ok(())
        })
        .await?;
        Ok(StatusCode::NO_CONTENT.into_response())
    })
    .await
}
