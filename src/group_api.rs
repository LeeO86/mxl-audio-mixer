//! The VCA and mute group API (§9, §12.4):
//! - `GET`, `POST /api/v1/vcas`, `GET`, `PATCH`, `DELETE /api/v1/vcas/{id}`
//!   (`fader_db`, `mute`, `members`, `name`, `colour`, `recall_safe`, `ramp_ms`;
//!   GET adds `signals`: the strips and buses under it, for spill);
//! - `GET`, `POST /api/v1/mute-groups`, `GET`, `PATCH`, `DELETE
//!   /api/v1/mute-groups/{id}` (`active`, `members`, `name`).
//!
//! VCA PFL and AFL are `PUT /api/v1/solo/{vca}`.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::api::{apply, bad, body, conflict, not_found, ramp, ApiResult};
use crate::mix::model::Db;
use crate::mix::vca::{MuteGroup, Vca};
use crate::mix::Mix;
use crate::ops::App;

pub fn routes() -> Router<App> {
    Router::new()
        .route("/api/v1/vcas", get(vcas).post(create_vca))
        .route(
            "/api/v1/vcas/{id}",
            get(vca).patch(patch_vca).delete(delete_vca),
        )
        .route("/api/v1/mute-groups", get(groups).post(create_group))
        .route(
            "/api/v1/mute-groups/{id}",
            get(group).patch(patch_group).delete(delete_group),
        )
}

async fn run(f: impl std::future::Future<Output = ApiResult<Response>>) -> Response {
    f.await.unwrap_or_else(|r| *r)
}

fn vca_json(mix: &Mix, v: &Vca) -> Value {
    let mut j = json!(v);
    j["signals"] = json!(mix.vca_members(&v.id));
    j
}

async fn vcas(State(app): State<App>) -> Json<Value> {
    let c = app.control();
    Json(Value::Array(
        c.mix.vcas.iter().map(|v| vca_json(&c.mix, v)).collect(),
    ))
}

fn one_vca(app: &App, id: &str) -> Option<Value> {
    let c = app.control();
    c.mix.vca(id).map(|v| vca_json(&c.mix, v))
}

async fn vca(State(app): State<App>, Path(id): Path<String>) -> Response {
    match one_vca(&app, &id) {
        Some(v) => Json(v).into_response(),
        None => *not_found(format!("VCA {id} does not exist")),
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
struct VcaBody {
    id: Option<String>,
    name: Option<String>,
    fader_db: Option<Db>,
    mute: Option<bool>,
    members: Option<Vec<String>>,
    #[serde(default, deserialize_with = "present")]
    colour: Option<Option<String>>,
    recall_safe: Option<bool>,
    ramp_ms: Option<f64>,
}

impl VcaBody {
    fn apply(self, v: &mut Vca) {
        macro_rules! set {
            ($($f:ident),*) => { $(if let Some(x) = self.$f { v.$f = x; })* };
        }
        set!(name, fader_db, mute, members, recall_safe);
        if let Some(c) = self.colour {
            v.colour = c.filter(|c| !c.is_empty());
        }
    }
}

async fn create_vca(State(app): State<App>, bytes: axum::body::Bytes) -> Response {
    run(async {
        let mut b: VcaBody = body(&bytes)?;
        let id = match b.id.take() {
            Some(id) => id,
            None => {
                let c = app.control();
                (1..)
                    .map(|n| format!("vca-{n}"))
                    .find(|id| c.mix.vca(id).is_none() && c.mix.kind(id).is_none())
                    .expect("unbounded")
            }
        };
        let mut v: Vca = serde_json::from_value(json!({"id": id, "name": id}))
            .map_err(|e| bad(e.to_string()))?;
        b.ramp_ms = None;
        b.apply(&mut v);
        apply(&app, 0, |mix| {
            if mix.vca(&v.id).is_some() {
                return Err(conflict(format!("VCA {} exists", v.id)));
            }
            mix.vcas.push(v);
            Ok(())
        })
        .await?;
        Ok((
            StatusCode::CREATED,
            Json(one_vca(&app, &id).unwrap_or_default()),
        )
            .into_response())
    })
    .await
}

async fn patch_vca(
    State(app): State<App>,
    Path(id): Path<String>,
    bytes: axum::body::Bytes,
) -> Response {
    run(async {
        let mut b: VcaBody = body(&bytes)?;
        if b.id.take().is_some_and(|new| new != id) {
            return Err(bad("a VCA id cannot be changed"));
        }
        let ramp = ramp(b.ramp_ms.take())?;
        apply(&app, ramp, |mix| {
            let v = mix
                .vcas
                .iter_mut()
                .find(|v| v.id == id)
                .ok_or_else(|| not_found(format!("VCA {id} does not exist")))?;
            b.apply(v);
            Ok(())
        })
        .await?;
        Ok(Json(one_vca(&app, &id).unwrap_or_default()).into_response())
    })
    .await
}

async fn delete_vca(State(app): State<App>, Path(id): Path<String>) -> Response {
    run(async {
        apply(&app, crate::engine::FADE_SAMPLES, |mix| {
            let before = mix.vcas.len();
            mix.vcas.retain(|v| v.id != id);
            if mix.vcas.len() == before {
                return Err(not_found(format!("VCA {id} does not exist")));
            }
            for v in &mut mix.vcas {
                v.members.retain(|m| m != &id);
            }
            Ok(())
        })
        .await?;
        Ok(StatusCode::NO_CONTENT.into_response())
    })
    .await
}

async fn groups(State(app): State<App>) -> Json<Value> {
    Json(json!(app.control().mix.mute_groups))
}

fn one_group(app: &App, id: &str) -> Option<MuteGroup> {
    app.control()
        .mix
        .mute_groups
        .iter()
        .find(|g| g.id == id)
        .cloned()
}

async fn group(State(app): State<App>, Path(id): Path<String>) -> Response {
    match one_group(&app, &id) {
        Some(g) => Json(json!(g)).into_response(),
        None => *not_found(format!("mute group {id} does not exist")),
    }
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct GroupBody {
    id: Option<String>,
    name: Option<String>,
    active: Option<bool>,
    members: Option<Vec<String>>,
}

impl GroupBody {
    fn apply(self, g: &mut MuteGroup) {
        macro_rules! set {
            ($($f:ident),*) => { $(if let Some(x) = self.$f { g.$f = x; })* };
        }
        set!(name, active, members);
    }
}

async fn create_group(State(app): State<App>, bytes: axum::body::Bytes) -> Response {
    run(async {
        let mut b: GroupBody = body(&bytes)?;
        let id = match b.id.take() {
            Some(id) => id,
            None => {
                let c = app.control();
                (1..)
                    .map(|n| format!("mute-{n}"))
                    .find(|id| c.mix.mute_groups.iter().all(|g| &g.id != id))
                    .expect("unbounded")
            }
        };
        let mut g: MuteGroup = serde_json::from_value(json!({"id": id, "name": id}))
            .map_err(|e| bad(e.to_string()))?;
        b.apply(&mut g);
        apply(&app, crate::engine::FADE_SAMPLES, |mix| {
            if mix.mute_groups.iter().any(|x| x.id == g.id) {
                return Err(conflict(format!("mute group {} exists", g.id)));
            }
            mix.mute_groups.push(g);
            Ok(())
        })
        .await?;
        let v = one_group(&app, &id).map(|g| json!(g)).unwrap_or_default();
        Ok((StatusCode::CREATED, Json(v)).into_response())
    })
    .await
}

async fn patch_group(
    State(app): State<App>,
    Path(id): Path<String>,
    bytes: axum::body::Bytes,
) -> Response {
    run(async {
        let mut b: GroupBody = body(&bytes)?;
        if b.id.take().is_some_and(|new| new != id) {
            return Err(bad("a mute group id cannot be changed"));
        }
        apply(&app, crate::engine::FADE_SAMPLES, |mix| {
            let g = mix
                .mute_groups
                .iter_mut()
                .find(|g| g.id == id)
                .ok_or_else(|| not_found(format!("mute group {id} does not exist")))?;
            b.apply(g);
            Ok(())
        })
        .await?;
        Ok(Json(one_group(&app, &id).map(|g| json!(g)).unwrap_or_default()).into_response())
    })
    .await
}

async fn delete_group(State(app): State<App>, Path(id): Path<String>) -> Response {
    run(async {
        apply(&app, crate::engine::FADE_SAMPLES, |mix| {
            let before = mix.mute_groups.len();
            mix.mute_groups.retain(|g| g.id != id);
            if mix.mute_groups.len() == before {
                return Err(not_found(format!("mute group {id} does not exist")));
            }
            Ok(())
        })
        .await?;
        Ok(StatusCode::NO_CONTENT.into_response())
    })
    .await
}
