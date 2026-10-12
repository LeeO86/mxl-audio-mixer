//! The high-level operations (§5), each `POST` with a JSON body; they answer the
//! signals they made (`{"buses", "strips", "removed"}`), applied as one change:
//! - `/api/v1/operations/group` `{"strips", "id"?, "name"?, "format"?}`;
//! - `/api/v1/operations/n-1` `{"strips", "of"?, "mode"?: "per-participant" |
//!   "one", "format"?}`;
//! - `/api/v1/operations/mix-minus` `{"of", "without", "id"?, "name"?}`;
//! - `/api/v1/operations/link` `{"strips", "format"?, "id"?, "name"?}`;
//! - `/api/v1/operations/unlink` `{"strip"}`.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::api::{apply, bad, body, with_errors, ApiResult};
use crate::mix::operations::{self, Made, N1Mode};
use crate::mix::{Format, Mix};
use crate::ops::App;

pub fn routes() -> Router<App> {
    Router::new()
        .route("/api/v1/operations/group", post(group))
        .route("/api/v1/operations/n-1", post(n1))
        .route("/api/v1/operations/mix-minus", post(mix_minus))
        .route("/api/v1/operations/link", post(link))
        .route("/api/v1/operations/unlink", post(unlink))
}

/// Run `op` on the mix as one change.
async fn run(app: &App, op: impl FnOnce(&mut Mix) -> Result<Made, String>) -> Response {
    let go = async {
        let mut made = None;
        let errors = apply(app, 0, |m| {
            made = Some(op(m).map_err(bad)?);
            Ok(())
        })
        .await?;
        let v = with_errors(json!(made.unwrap_or_default()), errors);
        Ok::<_, Box<Response>>((StatusCode::CREATED, Json(v)).into_response())
    };
    go.await.unwrap_or_else(|r| *r)
}

async fn parse<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> ApiResult<T> {
    body(bytes)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GroupBody {
    strips: Vec<String>,
    id: Option<String>,
    name: Option<String>,
    format: Option<Format>,
}

async fn group(State(app): State<App>, bytes: axum::body::Bytes) -> Response {
    match parse::<GroupBody>(&bytes).await {
        Ok(b) => {
            run(&app, |m| {
                operations::group(m, &b.strips, b.id, b.name, b.format)
            })
            .await
        }
        Err(r) => *r,
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct N1Body {
    strips: Vec<String>,
    of: Option<String>,
    #[serde(default)]
    mode: N1Mode,
    format: Option<Format>,
}

async fn n1(State(app): State<App>, bytes: axum::body::Bytes) -> Response {
    match parse::<N1Body>(&bytes).await {
        Ok(b) => {
            run(&app, |m| {
                operations::n_minus_one(m, &b.strips, b.of, b.mode, b.format)
            })
            .await
        }
        Err(r) => *r,
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MixMinusBody {
    of: String,
    without: Vec<String>,
    id: Option<String>,
    name: Option<String>,
}

async fn mix_minus(State(app): State<App>, bytes: axum::body::Bytes) -> Response {
    match parse::<MixMinusBody>(&bytes).await {
        Ok(b) => {
            run(&app, |m| {
                operations::mix_minus(m, &b.of, &b.without, b.id, b.name)
            })
            .await
        }
        Err(r) => *r,
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LinkBody {
    strips: Vec<String>,
    format: Option<Format>,
    id: Option<String>,
    name: Option<String>,
}

async fn link(State(app): State<App>, bytes: axum::body::Bytes) -> Response {
    match parse::<LinkBody>(&bytes).await {
        Ok(b) => {
            run(&app, |m| {
                operations::link(m, &b.strips, b.format, b.id, b.name)
            })
            .await
        }
        Err(r) => *r,
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UnlinkBody {
    strip: String,
}

async fn unlink(State(app): State<App>, bytes: axum::body::Bytes) -> Response {
    match parse::<UnlinkBody>(&bytes).await {
        Ok(b) => run(&app, |m| operations::unlink(m, &b.strip)).await,
        Err(r) => *r,
    }
}
