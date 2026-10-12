//! The talkback API (§10.2):
//! - `GET`, `PATCH /api/v1/talkback`: the mics (with the browsers'), the
//!   destinations, who talks, listen-back; PATCH `{"browser_priority"}`;
//! - `POST /api/v1/talkback/mics`, `GET`, `PATCH`, `DELETE …/mics/{id}`;
//! - `POST /api/v1/talkback/destinations`, `GET`, `PATCH`, `DELETE …/destinations/{id}`;
//! - `PUT`, `DELETE /api/v1/talkback/talk/{mic}/{destination}` (PUT
//!   `{"momentary": true}`: repeat it at least every [`MOMENTARY_HOLD`]),
//!   `DELETE /api/v1/talkback/talk/{mic}`, `DELETE /api/v1/talkback/talk`;
//! - `POST /api/v1/talkback/browser` `{"client"}`: a browser's microphone is on
//!   (repeat it while on); answers `{id, path, whip_url, whip_port}`; `DELETE
//!   …/browser/{id}`;
//! - `GET`, `PUT`, `DELETE /api/v1/listen-back` (`{"source"}` or
//!   `{"destination"}`: its return).
//!
//! [`MOMENTARY_HOLD`]: crate::control::MOMENTARY_HOLD

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::api::{apply, bad, body, conflict, not_found, ApiResult};
use crate::engine::FADE_SAMPLES;
use crate::mix::model::Db;
use crate::mix::room::Source;
use crate::mix::talkback::{Destination, Mic, Mode};
use crate::ops::App;

pub fn routes() -> Router<App> {
    Router::new()
        .route("/api/v1/talkback", get(talkback).patch(patch_talkback))
        .route("/api/v1/talkback/mics", get(mics).post(create_mic))
        .route(
            "/api/v1/talkback/mics/{id}",
            get(mic).patch(patch_mic).delete(delete_mic),
        )
        .route(
            "/api/v1/talkback/destinations",
            get(destinations).post(create_destination),
        )
        .route(
            "/api/v1/talkback/destinations/{id}",
            get(destination)
                .patch(patch_destination)
                .delete(delete_destination),
        )
        .route("/api/v1/talkback/talk", delete(stop_all))
        .route("/api/v1/talkback/talk/{mic}", delete(stop_mic))
        .route(
            "/api/v1/talkback/talk/{mic}/{destination}",
            put(talk).delete(untalk),
        )
        .route("/api/v1/talkback/browser", post(browser))
        .route("/api/v1/talkback/browser/{id}", delete(remove_browser))
        .route(
            "/api/v1/listen-back",
            get(listen_back)
                .put(put_listen_back)
                .delete(end_listen_back),
        )
}

async fn run(f: impl std::future::Future<Output = ApiResult<Response>>) -> Response {
    f.await.unwrap_or_else(|r| *r)
}

/// The talkback summary (also in the events' status).
pub fn talkback_json(app: &App) -> Value {
    let c = app.control();
    let t = &c.mix.control_room.talkback;
    json!({
        "mics": t.mics,
        "destinations": t.destinations,
        "browser_priority": t.browser_priority,
        "browser_mics": c.talkers.status(&c.engine),
        "talking": c.talking(),
        "listen_back": c.listen_back(),
    })
}

async fn talkback(State(app): State<App>) -> Json<Value> {
    Json(talkback_json(&app))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TalkbackBody {
    browser_priority: Option<u8>,
}

async fn patch_talkback(State(app): State<App>, bytes: axum::body::Bytes) -> Response {
    run(async {
        let b: TalkbackBody = body(&bytes)?;
        apply(&app, FADE_SAMPLES, |m| {
            if let Some(v) = b.browser_priority {
                m.control_room.talkback.browser_priority = v;
            }
            Ok(())
        })
        .await?;
        Ok(Json(talkback_json(&app)).into_response())
    })
    .await
}

async fn mics(State(app): State<App>) -> Json<Value> {
    Json(json!(app.control().mix.control_room.talkback.mics))
}

fn find_mic(app: &App, id: &str) -> Option<Mic> {
    let c = app.control();
    c.mix
        .control_room
        .talkback
        .mics
        .iter()
        .find(|m| m.id == id)
        .cloned()
}

async fn mic(State(app): State<App>, Path(id): Path<String>) -> Response {
    match find_mic(&app, &id) {
        Some(m) => Json(json!(m)).into_response(),
        None => *not_found(format!("talkback mic {id} does not exist")),
    }
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct MicBody {
    id: Option<String>,
    name: Option<String>,
    input: Option<usize>,
    channel: Option<usize>,
    level_db: Option<f64>,
    priority: Option<u8>,
    dims_control_room: Option<bool>,
}

impl MicBody {
    fn apply(self, m: &mut Mic) {
        macro_rules! set {
            ($($f:ident),*) => { $(if let Some(v) = self.$f { m.$f = v; })* };
        }
        set!(name, input, channel, level_db, priority, dims_control_room);
    }
}

async fn create_mic(State(app): State<App>, bytes: axum::body::Bytes) -> Response {
    run(async {
        let mut b: MicBody = body(&bytes)?;
        let input = b.input.ok_or_else(|| bad("input is required"))?;
        let channel = b.channel.ok_or_else(|| bad("channel is required"))?;
        let id = match b.id.take() {
            Some(id) => id,
            None => {
                let c = app.control();
                let t = &c.mix.control_room.talkback;
                (1..)
                    .map(|n| format!("mic-{n}"))
                    .find(|id| t.mics.iter().all(|m| &m.id != id))
                    .expect("unbounded")
            }
        };
        let mut m: Mic = serde_json::from_value(
            json!({"id": id, "name": id, "input": input, "channel": channel}),
        )
        .map_err(|e| bad(e.to_string()))?;
        b.apply(&mut m);
        apply(&app, 0, |mix| {
            let t = &mut mix.control_room.talkback;
            if t.mics.iter().any(|x| x.id == m.id) {
                return Err(conflict(format!("talkback mic {} exists", m.id)));
            }
            t.mics.push(m);
            Ok(())
        })
        .await?;
        let v = find_mic(&app, &id).map(|m| json!(m)).unwrap_or_default();
        Ok((StatusCode::CREATED, Json(v)).into_response())
    })
    .await
}

async fn patch_mic(
    State(app): State<App>,
    Path(id): Path<String>,
    bytes: axum::body::Bytes,
) -> Response {
    run(async {
        let mut b: MicBody = body(&bytes)?;
        if b.id.take().is_some_and(|new| new != id) {
            return Err(bad("a mic id cannot be changed"));
        }
        apply(&app, FADE_SAMPLES, |mix| {
            let m = mix
                .control_room
                .talkback
                .mics
                .iter_mut()
                .find(|m| m.id == id)
                .ok_or_else(|| not_found(format!("talkback mic {id} does not exist")))?;
            b.apply(m);
            Ok(())
        })
        .await?;
        Ok(Json(find_mic(&app, &id).map(|m| json!(m)).unwrap_or_default()).into_response())
    })
    .await
}

async fn delete_mic(State(app): State<App>, Path(id): Path<String>) -> Response {
    run(async {
        apply(&app, 0, |mix| {
            let t = &mut mix.control_room.talkback;
            let before = t.mics.len();
            t.mics.retain(|m| m.id != id);
            if t.mics.len() == before {
                return Err(not_found(format!("talkback mic {id} does not exist")));
            }
            Ok(())
        })
        .await?;
        Ok(StatusCode::NO_CONTENT.into_response())
    })
    .await
}

async fn destinations(State(app): State<App>) -> Json<Value> {
    Json(json!(app.control().mix.control_room.talkback.destinations))
}

fn find_destination(app: &App, id: &str) -> Option<Destination> {
    let c = app.control();
    c.mix
        .control_room
        .talkback
        .destinations
        .iter()
        .find(|d| d.id == id)
        .cloned()
}

async fn destination(State(app): State<App>, Path(id): Path<String>) -> Response {
    match find_destination(&app, &id) {
        Some(d) => Json(json!(d)).into_response(),
        None => *not_found(format!("talkback destination {id} does not exist")),
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
struct DestinationBody {
    id: Option<String>,
    name: Option<String>,
    bus: Option<String>,
    mode: Option<Mode>,
    interrupt_db: Option<Db>,
    level_db: Option<f64>,
    #[serde(default, rename = "return", deserialize_with = "present")]
    return_feed: Option<Option<Source>>,
}

impl DestinationBody {
    fn apply(self, d: &mut Destination) {
        macro_rules! set {
            ($($f:ident),*) => { $(if let Some(v) = self.$f { d.$f = v; })* };
        }
        set!(name, bus, mode, interrupt_db, level_db, return_feed);
    }
}

async fn create_destination(State(app): State<App>, bytes: axum::body::Bytes) -> Response {
    run(async {
        let mut b: DestinationBody = body(&bytes)?;
        let bus = b.bus.take().ok_or_else(|| bad("bus is required"))?;
        let id = b.id.take().unwrap_or_else(|| bus.clone());
        let mut d: Destination = serde_json::from_value(json!({"id": id, "name": id, "bus": bus}))
            .map_err(|e| bad(e.to_string()))?;
        b.apply(&mut d);
        apply(&app, 0, |mix| {
            let t = &mut mix.control_room.talkback;
            if t.destinations.iter().any(|x| x.id == d.id) {
                return Err(conflict(format!("talkback destination {} exists", d.id)));
            }
            t.destinations.push(d);
            Ok(())
        })
        .await?;
        let v = find_destination(&app, &id)
            .map(|d| json!(d))
            .unwrap_or_default();
        Ok((StatusCode::CREATED, Json(v)).into_response())
    })
    .await
}

async fn patch_destination(
    State(app): State<App>,
    Path(id): Path<String>,
    bytes: axum::body::Bytes,
) -> Response {
    run(async {
        let mut b: DestinationBody = body(&bytes)?;
        if b.id.take().is_some_and(|new| new != id) {
            return Err(bad("a destination id cannot be changed"));
        }
        apply(&app, FADE_SAMPLES, |mix| {
            let d = mix
                .control_room
                .talkback
                .destinations
                .iter_mut()
                .find(|d| d.id == id)
                .ok_or_else(|| not_found(format!("talkback destination {id} does not exist")))?;
            b.apply(d);
            Ok(())
        })
        .await?;
        let v = find_destination(&app, &id)
            .map(|d| json!(d))
            .unwrap_or_default();
        Ok(Json(v).into_response())
    })
    .await
}

async fn delete_destination(State(app): State<App>, Path(id): Path<String>) -> Response {
    run(async {
        apply(&app, 0, |mix| {
            let t = &mut mix.control_room.talkback;
            let before = t.destinations.len();
            t.destinations.retain(|d| d.id != id);
            if t.destinations.len() == before {
                return Err(not_found(format!(
                    "talkback destination {id} does not exist"
                )));
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
struct TalkBody {
    #[serde(default)]
    momentary: bool,
}

async fn talk(
    State(app): State<App>,
    Path((mic, dest)): Path<(String, String)>,
    bytes: axum::body::Bytes,
) -> Response {
    run(async {
        let b: TalkBody = if bytes.is_empty() {
            TalkBody::default()
        } else {
            body(&bytes)?
        };
        app.control()
            .set_talk(&mic, &dest, true, b.momentary)
            .map_err(not_found)?;
        apply(&app, FADE_SAMPLES, |_| Ok(())).await?;
        Ok(Json(json!(app.control().talking())).into_response())
    })
    .await
}

async fn untalk(State(app): State<App>, Path((mic, dest)): Path<(String, String)>) -> Response {
    run(async {
        app.control()
            .set_talk(&mic, &dest, false, false)
            .map_err(not_found)?;
        apply(&app, FADE_SAMPLES, |_| Ok(())).await?;
        Ok(Json(json!(app.control().talking())).into_response())
    })
    .await
}

async fn stop_mic(State(app): State<App>, Path(mic): Path<String>) -> Response {
    run(async {
        app.control().clear_talk(Some(&mic));
        apply(&app, FADE_SAMPLES, |_| Ok(())).await?;
        Ok(Json(json!(app.control().talking())).into_response())
    })
    .await
}

async fn stop_all(State(app): State<App>) -> Response {
    run(async {
        app.control().clear_talk(None);
        apply(&app, FADE_SAMPLES, |_| Ok(())).await?;
        Ok(Json(json!(app.control().talking())).into_response())
    })
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BrowserBody {
    client: String,
}

async fn browser(State(app): State<App>, bytes: axum::body::Bytes) -> Response {
    run(async {
        let b: BrowserBody = body(&bytes)?;
        let (id, new) = app
            .control()
            .register_browser_mic(&b.client)
            .map_err(|e| Box::new(crate::ops::error(StatusCode::CONFLICT, e)))?;
        if new {
            apply(&app, 0, |_| Ok(())).await?;
        }
        let path = app.control().talkers.path(&b.client);
        let whip = app
            .config
            .preview_whep_url
            .as_ref()
            .map(|base| format!("{base}/{path}/whip"));
        Ok(Json(json!({
            "id": id,
            "path": path,
            "whip_url": whip,
            "whip_port": app.config.mediamtx_whep_port,
        }))
        .into_response())
    })
    .await
}

async fn remove_browser(State(app): State<App>, Path(id): Path<String>) -> Response {
    run(async {
        if !app.control().remove_browser_mic(&id) {
            return Err(not_found(format!("browser mic {id} does not exist")));
        }
        apply(&app, 0, |_| Ok(())).await?;
        Ok(StatusCode::NO_CONTENT.into_response())
    })
    .await
}

async fn listen_back(State(app): State<App>) -> Json<Value> {
    Json(json!(app.control().listen_back()))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListenBackBody {
    source: Option<Source>,
    destination: Option<String>,
}

async fn put_listen_back(State(app): State<App>, bytes: axum::body::Bytes) -> Response {
    run(async {
        let b: ListenBackBody = body(&bytes)?;
        let source = match (b.source, b.destination) {
            (Some(s), None) => s,
            (None, Some(d)) => find_destination(&app, &d)
                .ok_or_else(|| not_found(format!("talkback destination {d} does not exist")))?
                .return_feed
                .ok_or_else(|| bad(format!("talkback destination {d} has no return")))?,
            _ => return Err(bad("give a source or a destination")),
        };
        {
            let c = app.control();
            match &source {
                Source::Signal(s) if c.mix.kind(s).is_none() => {
                    return Err(not_found(format!("signal {s} does not exist")))
                }
                Source::Input(n) if *n == 0 || *n > app.config.input_flows => {
                    return Err(not_found(format!("input {n} does not exist")))
                }
                _ => {}
            }
        }
        app.control().set_listen_back(Some(source));
        apply(&app, 0, |_| Ok(())).await?;
        Ok(Json(json!(app.control().listen_back())).into_response())
    })
    .await
}

async fn end_listen_back(State(app): State<App>) -> Response {
    run(async {
        app.control().set_listen_back(None);
        apply(&app, 0, |_| Ok(())).await?;
        Ok(Json(json!(app.control().listen_back())).into_response())
    })
    .await
}
