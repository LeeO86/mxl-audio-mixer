//! Logic graphs (§11):
//! - `GET`, `POST /api/v1/logic`, `GET`, `PATCH`, `DELETE /api/v1/logic/{id}`:
//!   a graph's blocks and wires; GET adds `values` (each block's last value)
//!   and `buttons` (its soft keys);
//! - `PUT /api/v1/logic/{id}/buttons/{block}` with `{"pressed": true}`: a soft
//!   key. It is not part of the show.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::api::{apply, bad, body, conflict, not_found, ApiResult};
use crate::engine::FADE_SAMPLES;
use crate::mix::logic::{Block, Graph, Wire};
use crate::ops::App;

pub fn routes() -> Router<App> {
    Router::new()
        .route("/api/v1/logic", get(list).post(create))
        .route("/api/v1/logic/{id}", get(one).patch(patch).delete(delete))
        .route(
            "/api/v1/logic/{id}/buttons/{block}",
            axum::routing::put(button),
        )
}

async fn run(f: impl std::future::Future<Output = ApiResult<Response>>) -> Response {
    f.await.unwrap_or_else(|r| *r)
}

fn json_graph(app: &App, g: &Graph) -> Value {
    let c = app.control();
    let mut j = json!(g);
    j["values"] = json!(c.logic.values.get(&g.id));
    let buttons: Vec<String> = g
        .blocks
        .iter()
        .filter(|b| {
            matches!(b.kind, crate::mix::logic::BlockKind::Button) && c.logic.button(&g.id, &b.id)
        })
        .map(|b| b.id.clone())
        .collect();
    j["buttons"] = json!(buttons);
    j
}

async fn list(State(app): State<App>) -> Json<Value> {
    let graphs = app.control().mix.logic.clone();
    Json(Value::Array(
        graphs.iter().map(|g| json_graph(&app, g)).collect(),
    ))
}

async fn one(State(app): State<App>, Path(id): Path<String>) -> Response {
    let found = app.control().mix.logic_graph(&id).cloned();
    match found {
        Some(g) => Json(json_graph(&app, &g)).into_response(),
        None => *not_found(format!("logic {id} does not exist")),
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
        apply(&app, FADE_SAMPLES, |mix| {
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

fn one_value(app: &App, id: &str) -> Value {
    let found = app.control().mix.logic_graph(id).cloned();
    found.map(|g| json_graph(app, &g)).unwrap_or(Value::Null)
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
        apply(&app, FADE_SAMPLES, |mix| {
            let g = mix
                .logic
                .iter_mut()
                .find(|g| g.id == id)
                .ok_or_else(|| not_found(format!("logic {id} does not exist")))?;
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
        apply(&app, FADE_SAMPLES, |mix| {
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
        let g = c
            .mix
            .logic_graph(&id)
            .ok_or_else(|| not_found(format!("logic {id} does not exist")))?;
        if !g
            .blocks
            .iter()
            .any(|x| x.id == block && matches!(x.kind, crate::mix::logic::BlockKind::Button))
        {
            return Err(not_found(format!("logic {id} has no button {block}")));
        }
        c.logic.set_button(&id, &block, b.pressed);
        drop(c);
        Ok(Json(one_value(&app, &id)).into_response())
    })
    .await
}
