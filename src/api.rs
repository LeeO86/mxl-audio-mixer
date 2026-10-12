//! The mix API (§5, §12.4): strips, buses, sends, published outputs, mix
//! settings and meters. Every parameter is addressed by its signal id and field
//! (`/api/v1/strips/{id}` `fader_db`, `/api/v1/sends/{from}/{to}` `gain_db`); a
//! PATCH may carry `ramp_ms` (default 10 ms) for the parameters it changes.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::control::{fade_wait, Staged};
use crate::mix::chain::Proc;
use crate::mix::model::{
    Afv, Bus, Db, Kind, MixSettings, Output, PatchPoint, Send, Strip, Tap, DEFAULT_RAMP_MS,
    MAX_RAMP_MS,
};
use crate::mix::{Format, Mix};
use crate::ops::{error, App};

pub fn routes() -> Router<App> {
    Router::new()
        .route("/api/v1/mix", get(mix))
        .route("/api/v1/mix/settings", get(settings).patch(patch_settings))
        .route("/api/v1/strips", get(strips).post(create_strip))
        .route(
            "/api/v1/strips/{id}",
            get(strip).patch(patch_strip).delete(delete_signal),
        )
        .route("/api/v1/buses", get(buses).post(create_bus))
        .route(
            "/api/v1/buses/{id}",
            get(bus).patch(patch_bus).delete(delete_signal),
        )
        .route("/api/v1/sends", get(sends))
        .route(
            "/api/v1/sends/{from}/{to}",
            get(send).put(put_send).delete(delete_send),
        )
        .route("/api/v1/outputs", get(outputs))
        .route(
            "/api/v1/outputs/{signal}",
            get(output).put(put_output).delete(delete_output),
        )
        .route(
            "/api/v1/strips/{id}/chain",
            get(|s, p| chain_list(s, p, Kind::Strip))
                .post(|s, p, b| chain_add(s, p, b, Kind::Strip)),
        )
        .route(
            "/api/v1/strips/{id}/chain/{node}",
            get(|s, p| chain_get(s, p, Kind::Strip))
                .patch(|s, p, b| chain_patch(s, p, b, Kind::Strip))
                .delete(|s, p| chain_delete(s, p, Kind::Strip)),
        )
        .route(
            "/api/v1/strips/{id}/chain/{node}/move",
            post(|s, p, b| chain_move(s, p, b, Kind::Strip)),
        )
        .route(
            "/api/v1/buses/{id}/chain",
            get(|s, p| chain_list(s, p, Kind::Bus)).post(|s, p, b| chain_add(s, p, b, Kind::Bus)),
        )
        .route(
            "/api/v1/buses/{id}/chain/{node}",
            get(|s, p| chain_get(s, p, Kind::Bus))
                .patch(|s, p, b| chain_patch(s, p, b, Kind::Bus))
                .delete(|s, p| chain_delete(s, p, Kind::Bus)),
        )
        .route(
            "/api/v1/buses/{id}/chain/{node}/move",
            post(|s, p, b| chain_move(s, p, b, Kind::Bus)),
        )
        .route("/api/v1/loudness", get(loudness))
        .route("/api/v1/loudness/{signal}/{action}", post(loudness_control))
        .route("/api/v1/meters", get(meters))
        .route("/api/v1/meters/clear", post(clear_holds))
        .route("/api/v1/meters/{signal}/clear", post(clear_hold))
        .route(
            "/api/v1/analysis",
            get(analysis).put(put_analysis).delete(delete_analysis),
        )
        .route("/api/v1/tally", get(tally))
}

/// The TSL receiver's settings, the lamps of every tally index seen, and the
/// strips that follow one.
async fn tally(State(app): State<App>) -> Json<Value> {
    let c = &app.config;
    let control = app.control();
    let mut v = control.tally_status();
    v["enabled"] = json!(c.tsl_enable);
    v["udp_port"] = json!(c.tsl_udp_port);
    v["tcp_port"] = json!(c.tsl_tcp_port);
    v["screen"] = json!(c.tsl_screen);
    v["map"] = json!(c.tsl_map);
    v["lamps"] = json!(control.tally.lamps());
    Json(v)
}

/// The analyser's signal and tap, and its last values (spectrum, correlation,
/// goniometer points).
async fn analysis(State(app): State<App>) -> Json<Value> {
    let control = app.control();
    let target = control
        .analyser
        .target
        .as_ref()
        .map(|(signal, tap)| json!({"signal": signal, "tap": tap}));
    Json(json!({"target": target, "values": control.analyser.values()}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AnalysisBody {
    signal: String,
    #[serde(default)]
    tap: Option<Tap>,
}

async fn put_analysis(State(app): State<App>, bytes: axum::body::Bytes) -> Response {
    let run = async {
        let b: AnalysisBody = body(&bytes)?;
        {
            let mut control = app.control();
            let mix = &control.mix;
            if mix.kind(&b.signal).is_none() {
                return Err(not_found(format!("signal {} does not exist", b.signal)));
            }
            let tap = b.tap.unwrap_or(Tap::PostFader);
            if mix.tap_format(&b.signal, &tap).is_none() {
                return Err(bad(format!("{} has no such tap", b.signal)));
            }
            control.set_analysis(Some((b.signal, tap)));
        }
        apply(&app, 0, |_| Ok(())).await?;
        Ok::<_, Box<Response>>(analysis(State(app)).await.into_response())
    };
    run.await.unwrap_or_else(|r| *r)
}

async fn delete_analysis(State(app): State<App>) -> Response {
    let run = async {
        app.control().set_analysis(None);
        apply(&app, 0, |_| Ok(())).await?;
        Ok::<_, Box<Response>>(StatusCode::NO_CONTENT.into_response())
    };
    run.await.unwrap_or_else(|r| *r)
}

pub type ApiResult<T> = Result<T, Box<Response>>;

pub(crate) fn bad(message: impl Into<String>) -> Box<Response> {
    Box::new(error(StatusCode::BAD_REQUEST, message))
}

pub(crate) fn not_found(message: impl Into<String>) -> Box<Response> {
    Box::new(error(StatusCode::NOT_FOUND, message))
}

pub(crate) fn conflict(message: impl Into<String>) -> Box<Response> {
    Box::new(error(StatusCode::CONFLICT, message))
}

pub(crate) fn internal(message: impl Into<String>) -> Box<Response> {
    Box::new(error(StatusCode::INTERNAL_SERVER_ERROR, message))
}

pub(crate) fn body<T: DeserializeOwned>(bytes: &[u8]) -> ApiResult<T> {
    serde_json::from_slice(bytes).map_err(|e| bad(e.to_string()))
}

pub(crate) fn ramp(ms: Option<f64>) -> ApiResult<u32> {
    let ms = ms.unwrap_or(DEFAULT_RAMP_MS);
    if !(0.0..=MAX_RAMP_MS).contains(&ms) {
        return Err(bad(format!("ramp_ms must be 0 to {MAX_RAMP_MS}")));
    }
    Ok((ms * 48.0).round() as u32)
}

/// Apply a change of the mix: stage it, wait for the fade when the change
/// rewires something, then swap. Returns output publishing errors.
pub async fn apply(
    app: &App,
    ramp: u32,
    change: impl FnOnce(&mut Mix) -> ApiResult<()>,
) -> ApiResult<Vec<String>> {
    let _serial = app.changes.lock().await;
    let staged = {
        let mut control = app.control();
        let mut next = control.mix.clone();
        change(&mut next)?;
        control.stage(next, ramp).map_err(bad)?
    };
    match staged {
        Staged::Done => Ok(Vec::new()),
        Staged::Swap { wait } => {
            if wait {
                tokio::time::sleep(fade_wait(app.config.block_samples)).await;
            }
            tokio::task::block_in_place(|| app.control().swap()).map_err(internal)
        }
    }
}

pub(crate) fn with_errors(mut value: Value, errors: Vec<String>) -> Value {
    if !errors.is_empty() {
        value["output_errors"] = json!(errors);
    }
    value
}

async fn mix(State(app): State<App>) -> Json<Value> {
    Json(json!(app.control().mix))
}

async fn settings(State(app): State<App>) -> Json<Value> {
    Json(json!(app.control().mix.settings))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SettingsBody {
    pan_law_db: Option<crate::mix::format::PanLaw>,
    mono_sum_db: Option<crate::mix::format::MonoSum>,
    loudness_target_lufs: Option<f64>,
    loudness_tolerance_lu: Option<f64>,
}

async fn patch_settings(State(app): State<App>, bytes: axum::body::Bytes) -> Response {
    let run = async {
        let b: SettingsBody = body(&bytes)?;
        let errors = apply(&app, crate::engine::FADE_SAMPLES, |m| {
            if let Some(law) = b.pan_law_db {
                m.settings.pan_law_db = law;
            }
            if let Some(sum) = b.mono_sum_db {
                m.settings.mono_sum_db = sum;
            }
            if let Some(t) = b.loudness_target_lufs {
                if !(-60.0..=0.0).contains(&t) {
                    return Err(bad("loudness_target_lufs must be -60 to 0"));
                }
                m.settings.loudness_target_lufs = t;
            }
            if let Some(t) = b.loudness_tolerance_lu {
                if !(0.0..=10.0).contains(&t) {
                    return Err(bad("loudness_tolerance_lu must be 0 to 10"));
                }
                m.settings.loudness_tolerance_lu = t;
            }
            Ok(())
        })
        .await?;
        let s: MixSettings = app.control().mix.settings;
        Ok::<_, Box<Response>>(Json(with_errors(json!(s), errors)).into_response())
    };
    run.await.unwrap_or_else(|r| *r)
}

fn strip_json(app: &App, s: &Strip) -> Value {
    let control = app.control();
    let mut v = json!(s);
    v["patch_state"] = json!(control.patch_state(s));
    v["published"] = json!(control.mix.output(&s.id).is_some());
    v
}

fn bus_json(app: &App, b: &Bus) -> Value {
    let mut v = json!(b);
    v["published"] = json!(app.control().mix.output(&b.id).is_some());
    v
}

async fn strips(State(app): State<App>) -> Json<Value> {
    let list = app.control().mix.strips.clone();
    Json(Value::Array(
        list.iter().map(|s| strip_json(&app, s)).collect(),
    ))
}

async fn strip(State(app): State<App>, Path(id): Path<String>) -> Response {
    let found = app.control().mix.strip(&id).cloned();
    match found {
        Some(s) => Json(strip_json(&app, &s)).into_response(),
        None => *not_found(format!("strip {id} does not exist")),
    }
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct StripBody {
    id: Option<String>,
    name: Option<String>,
    format: Option<Format>,
    patch: Option<Vec<Option<PatchPoint>>>,
    trim_db: Option<f64>,
    polarity: Option<Vec<bool>>,
    fader_db: Option<Db>,
    mute: Option<bool>,
    pan: Option<f64>,
    width: Option<f64>,
    chain: Option<Vec<Proc>>,
    loudness: Option<bool>,
    colour: Option<String>,
    recall_safe: Option<bool>,
    /// `null` clears it.
    #[serde(default, deserialize_with = "present")]
    tally_index: Option<Option<u16>>,
    #[serde(default, deserialize_with = "present")]
    afv: Option<Option<Afv>>,
    ramp_ms: Option<f64>,
}

/// A field that is present (`Some`, perhaps `Some(None)` for `null`) or absent.
fn present<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(d).map(Some)
}

/// `""` clears the colour.
fn colour(v: String) -> Option<String> {
    (!v.is_empty()).then_some(v)
}

impl StripBody {
    fn apply(self, s: &mut Strip) {
        if let Some(v) = self.tally_index {
            s.tally_index = v;
        }
        if let Some(v) = self.afv {
            s.afv = v;
        }
        if let Some(v) = self.colour {
            s.colour = colour(v);
        }
        if let Some(v) = self.recall_safe {
            s.recall_safe = v;
        }
        if let Some(format) = self.format {
            let n = format.channels();
            s.format = format;
            s.patch.resize(n, None);
            s.polarity.resize(n, false);
        }
        if let Some(v) = self.name {
            s.name = v;
        }
        if let Some(v) = self.patch {
            s.patch = v;
        }
        if let Some(v) = self.trim_db {
            s.trim_db = v;
        }
        if let Some(v) = self.polarity {
            s.polarity = v;
        }
        if let Some(v) = self.fader_db {
            s.fader_db = v;
        }
        if let Some(v) = self.mute {
            s.mute = v;
        }
        if let Some(v) = self.pan {
            s.pan = v;
        }
        if let Some(v) = self.width {
            s.width = v;
        }
        if let Some(v) = self.chain {
            s.chain = v;
        }
        if let Some(v) = self.loudness {
            s.loudness = v;
        }
    }
}

async fn create_strip(State(app): State<App>, bytes: axum::body::Bytes) -> Response {
    let run = async {
        let mut b: StripBody = body(&bytes)?;
        let format = b.format.ok_or_else(|| bad("format is required"))?;
        let id = match b.id.take() {
            Some(id) => id,
            None => app.control().mix.next_id("strip"),
        };
        let n = format.channels();
        let mut s = Strip {
            id: id.clone(),
            name: id.clone(),
            format,
            patch: vec![None; n],
            trim_db: 0.0,
            polarity: vec![false; n],
            fader_db: Db(0.0),
            mute: false,
            pan: 0.0,
            width: 1.0,
            chain: Vec::new(),
            loudness: false,
            colour: None,
            recall_safe: false,
            tally_index: None,
            afv: None,
        };
        b.apply(&mut s);
        let errors = apply(&app, 0, |m| {
            if m.kind(&id).is_some() {
                return Err(conflict(format!("signal {id} exists")));
            }
            m.strips.push(s);
            Ok(())
        })
        .await?;
        let s = app.control().mix.strip(&id).cloned();
        let v = s.map(|s| strip_json(&app, &s)).unwrap_or_default();
        Ok::<_, Box<Response>>((StatusCode::CREATED, Json(with_errors(v, errors))).into_response())
    };
    run.await.unwrap_or_else(|r| *r)
}

async fn patch_strip(
    State(app): State<App>,
    Path(id): Path<String>,
    bytes: axum::body::Bytes,
) -> Response {
    let run = async {
        let mut b: StripBody = body(&bytes)?;
        if b.id.take().is_some_and(|new| new != id) {
            return Err(bad("a signal id cannot be changed"));
        }
        let ramp = ramp(b.ramp_ms.take())?;
        let errors = apply(&app, ramp, |m| {
            let s = m
                .strips
                .iter_mut()
                .find(|s| s.id == id)
                .ok_or_else(|| not_found(format!("strip {id} does not exist")))?;
            b.apply(s);
            Ok(())
        })
        .await?;
        let s = app.control().mix.strip(&id).cloned();
        let v = s.map(|s| strip_json(&app, &s)).unwrap_or_default();
        Ok(Json(with_errors(v, errors)).into_response())
    };
    run.await.unwrap_or_else(|r| *r)
}

async fn delete_signal(State(app): State<App>, Path(id): Path<String>) -> Response {
    let run = async {
        apply(&app, 0, |m| {
            if m.kind(&id).is_none() {
                return Err(not_found(format!("signal {id} does not exist")));
            }
            m.remove_signal(&id);
            Ok(())
        })
        .await?;
        Ok::<_, Box<Response>>(StatusCode::NO_CONTENT.into_response())
    };
    run.await.unwrap_or_else(|r| *r)
}

async fn buses(State(app): State<App>) -> Json<Value> {
    let list = app.control().mix.buses.clone();
    Json(Value::Array(
        list.iter().map(|b| bus_json(&app, b)).collect(),
    ))
}

async fn bus(State(app): State<App>, Path(id): Path<String>) -> Response {
    let found = app.control().mix.bus(&id).cloned();
    match found {
        Some(b) => Json(bus_json(&app, &b)).into_response(),
        None => *not_found(format!("bus {id} does not exist")),
    }
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct BusBody {
    id: Option<String>,
    name: Option<String>,
    format: Option<Format>,
    fader_db: Option<Db>,
    mute: Option<bool>,
    chain: Option<Vec<Proc>>,
    loudness: Option<bool>,
    compensate: Option<bool>,
    colour: Option<String>,
    recall_safe: Option<bool>,
    ramp_ms: Option<f64>,
}

impl BusBody {
    fn apply(self, b: &mut Bus) {
        if let Some(v) = self.colour {
            b.colour = colour(v);
        }
        if let Some(v) = self.recall_safe {
            b.recall_safe = v;
        }
        if let Some(v) = self.chain {
            b.chain = v;
        }
        if let Some(v) = self.loudness {
            b.loudness = v;
        }
        if let Some(v) = self.compensate {
            b.compensate = v;
        }
        if let Some(v) = self.name {
            b.name = v;
        }
        if let Some(v) = self.format {
            b.format = v;
        }
        if let Some(v) = self.fader_db {
            b.fader_db = v;
        }
        if let Some(v) = self.mute {
            b.mute = v;
        }
    }
}

async fn create_bus(State(app): State<App>, bytes: axum::body::Bytes) -> Response {
    let run = async {
        let mut b: BusBody = body(&bytes)?;
        let format = b.format.ok_or_else(|| bad("format is required"))?;
        let id = match b.id.take() {
            Some(id) => id,
            None => app.control().mix.next_id("bus"),
        };
        let mut bus = Bus {
            id: id.clone(),
            name: id.clone(),
            format,
            fader_db: Db(0.0),
            mute: false,
            chain: Vec::new(),
            loudness: false,
            compensate: true,
            colour: None,
            recall_safe: false,
        };
        b.apply(&mut bus);
        let errors = apply(&app, 0, |m| {
            if m.kind(&id).is_some() {
                return Err(conflict(format!("signal {id} exists")));
            }
            m.buses.push(bus);
            Ok(())
        })
        .await?;
        let b = app.control().mix.bus(&id).cloned();
        let v = b.map(|b| bus_json(&app, &b)).unwrap_or_default();
        Ok::<_, Box<Response>>((StatusCode::CREATED, Json(with_errors(v, errors))).into_response())
    };
    run.await.unwrap_or_else(|r| *r)
}

async fn patch_bus(
    State(app): State<App>,
    Path(id): Path<String>,
    bytes: axum::body::Bytes,
) -> Response {
    let run = async {
        let mut b: BusBody = body(&bytes)?;
        if b.id.take().is_some_and(|new| new != id) {
            return Err(bad("a signal id cannot be changed"));
        }
        let ramp = ramp(b.ramp_ms.take())?;
        let errors = apply(&app, ramp, |m| {
            let bus = m
                .buses
                .iter_mut()
                .find(|x| x.id == id)
                .ok_or_else(|| not_found(format!("bus {id} does not exist")))?;
            b.apply(bus);
            Ok(())
        })
        .await?;
        let b = app.control().mix.bus(&id).cloned();
        let v = b.map(|b| bus_json(&app, &b)).unwrap_or_default();
        Ok(Json(with_errors(v, errors)).into_response())
    };
    run.await.unwrap_or_else(|r| *r)
}

async fn sends(State(app): State<App>) -> Json<Value> {
    Json(json!(app.control().mix.sends))
}

async fn send(State(app): State<App>, Path((from, to)): Path<(String, String)>) -> Response {
    match app.control().mix.send(&from, &to) {
        Some(s) => Json(json!(s)).into_response(),
        None => *not_found(format!("send {from} → {to} does not exist")),
    }
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct SendBody {
    tap: Option<Tap>,
    gain_db: Option<Db>,
    mute: Option<bool>,
    block_delay: Option<bool>,
    phase: Option<bool>,
    ramp_ms: Option<f64>,
}

/// Create or change a send. A new send defaults to 0 dB from the source's
/// output tap (a strip's post-pan, a bus's post-fader).
async fn put_send(
    State(app): State<App>,
    Path((from, to)): Path<(String, String)>,
    bytes: axum::body::Bytes,
) -> Response {
    let run = async {
        let mut b: SendBody = if bytes.is_empty() {
            SendBody::default()
        } else {
            body(&bytes)?
        };
        let ramp = ramp(b.ramp_ms.take())?;
        let created = app.control().mix.send(&from, &to).is_none();
        let errors = apply(&app, ramp, |m| {
            if m.kind(&from).is_none() {
                return Err(not_found(format!("signal {from} does not exist")));
            }
            if m.kind(&to) != Some(Kind::Bus) {
                return Err(not_found(format!("bus {to} does not exist")));
            }
            let tap = m.default_tap(&from);
            if m.send(&from, &to).is_none() {
                m.sends.push(Send {
                    from: from.clone(),
                    to: to.clone(),
                    tap,
                    gain_db: Db(0.0),
                    mute: false,
                    phase: false,
                    block_delay: false,
                });
            }
            let s = m
                .sends
                .iter_mut()
                .find(|s| s.from == from && s.to == to)
                .expect("just added");
            if let Some(v) = b.tap {
                s.tap = v;
            }
            if let Some(v) = b.gain_db {
                s.gain_db = v;
            }
            if let Some(v) = b.mute {
                s.mute = v;
            }
            if let Some(v) = b.phase {
                s.phase = v;
            }
            if let Some(v) = b.block_delay {
                s.block_delay = v;
            }
            Ok(())
        })
        .await?;
        let s = app.control().mix.send(&from, &to).cloned();
        let code = if created {
            StatusCode::CREATED
        } else {
            StatusCode::OK
        };
        Ok::<_, Box<Response>>((code, Json(with_errors(json!(s), errors))).into_response())
    };
    run.await.unwrap_or_else(|r| *r)
}

async fn delete_send(State(app): State<App>, Path((from, to)): Path<(String, String)>) -> Response {
    let run = async {
        apply(&app, 0, |m| {
            if m.send(&from, &to).is_none() {
                return Err(not_found(format!("send {from} → {to} does not exist")));
            }
            m.sends.retain(|s| !(s.from == from && s.to == to));
            Ok(())
        })
        .await?;
        Ok::<_, Box<Response>>(StatusCode::NO_CONTENT.into_response())
    };
    run.await.unwrap_or_else(|r| *r)
}

async fn outputs(State(app): State<App>) -> Json<Value> {
    let control = app.control();
    Json(json!(control.output_status()))
}

async fn output(State(app): State<App>, Path(signal): Path<String>) -> Response {
    let status = app.control().output_status();
    match status.into_iter().find(|o| o.signal == signal) {
        Some(o) => Json(json!(o)).into_response(),
        None => *not_found(format!("{signal} is not published")),
    }
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct OutputBody {
    #[serde(default)]
    format: Option<Format>,
    #[serde(default)]
    commit_samples: Option<u32>,
}

/// Publish a signal (or change its layout or commit size).
async fn put_output(
    State(app): State<App>,
    Path(signal): Path<String>,
    bytes: axum::body::Bytes,
) -> Response {
    let run = async {
        let b: OutputBody = if bytes.is_empty() {
            OutputBody::default()
        } else {
            body(&bytes)?
        };
        let errors = apply(&app, 0, |m| {
            if m.kind(&signal).is_none() {
                return Err(not_found(format!("signal {signal} does not exist")));
            }
            m.outputs.retain(|o| o.signal != signal);
            m.outputs.push(Output {
                signal: signal.clone(),
                format: b.format,
                commit_samples: b.commit_samples,
            });
            Ok(())
        })
        .await?;
        if !errors.is_empty() {
            return Err(internal(errors.join("; ")));
        }
        let status = app.control().output_status();
        let o = status.into_iter().find(|o| o.signal == signal);
        Ok::<_, Box<Response>>(Json(json!(o)).into_response())
    };
    run.await.unwrap_or_else(|r| *r)
}

async fn delete_output(State(app): State<App>, Path(signal): Path<String>) -> Response {
    let run = async {
        apply(&app, 0, |m| {
            if m.output(&signal).is_none() {
                return Err(not_found(format!("{signal} is not published")));
            }
            m.outputs.retain(|o| o.signal != signal);
            Ok(())
        })
        .await?;
        Ok::<_, Box<Response>>(StatusCode::NO_CONTENT.into_response())
    };
    run.await.unwrap_or_else(|r| *r)
}

async fn meters(State(app): State<App>) -> Json<Value> {
    Json(json!(app.control().meters.values()))
}

async fn loudness(State(app): State<App>) -> Json<Value> {
    let control = app.control();
    Json(json!(control.loudness.values(&control.mix.settings)))
}

/// `start`, `stop` or `reset` a signal's loudness integration (§7).
async fn loudness_control(
    State(app): State<App>,
    Path((signal, action)): Path<(String, String)>,
) -> Response {
    let result = app.control().loudness.control(&signal, &action);
    match result {
        Ok(()) => {
            let control = app.control();
            let values = control.loudness.values(&control.mix.settings);
            Json(json!(values.get(&signal))).into_response()
        }
        Err(e) if e.starts_with("unknown action") => *bad(e),
        Err(e) => *not_found(e),
    }
}

fn chain_mut<'a>(m: &'a mut Mix, kind: Kind, id: &str) -> ApiResult<&'a mut Vec<Proc>> {
    let found = match kind {
        Kind::Strip => m
            .strips
            .iter_mut()
            .find(|s| s.id == id)
            .map(|s| &mut s.chain),
        Kind::Bus => m
            .buses
            .iter_mut()
            .find(|b| b.id == id)
            .map(|b| &mut b.chain),
    };
    found.ok_or_else(|| not_found(format!("{id} does not exist")))
}

fn chain_of(app: &App, kind: Kind, id: &str) -> ApiResult<Vec<Proc>> {
    let mut mix = app.control().mix.clone();
    chain_mut(&mut mix, kind, id).map(|c| c.clone())
}

async fn chain_list(State(app): State<App>, Path(id): Path<String>, kind: Kind) -> Response {
    match chain_of(&app, kind, &id) {
        Ok(c) => Json(json!(c)).into_response(),
        Err(r) => *r,
    }
}

async fn chain_get(
    State(app): State<App>,
    Path((id, node)): Path<(String, String)>,
    kind: Kind,
) -> Response {
    match chain_of(&app, kind, &id) {
        Ok(c) => match c.into_iter().find(|p| p.id == node) {
            Some(p) => Json(json!(p)).into_response(),
            None => *not_found(format!("{id} has no node {node}")),
        },
        Err(r) => *r,
    }
}

/// Take `key` out of a JSON object body.
fn take(body: &mut Value, key: &str) -> Option<Value> {
    body.as_object_mut().and_then(|o| o.remove(key))
}

/// Add a node: the node's JSON (`id` optional), plus `position` (default: last).
async fn chain_add(
    State(app): State<App>,
    Path(id): Path<String>,
    bytes: axum::body::Bytes,
    kind: Kind,
) -> Response {
    let run = async {
        let mut v: Value = body(&bytes)?;
        let position = take(&mut v, "position").and_then(|p| p.as_u64());
        let given = take(&mut v, "id").and_then(|i| i.as_str().map(str::to_string));
        if let Some(o) = v.as_object_mut() {
            o.insert("id".into(), json!("new"));
        }
        let mut node: Proc = serde_json::from_value(v).map_err(|e| bad(e.to_string()))?;
        let node_id = {
            let chain = chain_of(&app, kind, &id)?;
            given.unwrap_or_else(|| crate::mix::chain::next_id(&chain, &node.kind))
        };
        node.id = node_id.clone();
        let errors = apply(&app, crate::engine::FADE_SAMPLES, |m| {
            let chain = chain_mut(m, kind, &id)?;
            let at = position.map_or(chain.len(), |p| (p as usize).min(chain.len()));
            chain.insert(at, node);
            Ok(())
        })
        .await?;
        let chain = chain_of(&app, kind, &id)?;
        let p = chain.into_iter().find(|p| p.id == node_id);
        Ok::<_, Box<Response>>(
            (StatusCode::CREATED, Json(with_errors(json!(p), errors))).into_response(),
        )
    };
    run.await.unwrap_or_else(|r| *r)
}

/// RFC 7396 merge patch.
fn merge(target: &mut Value, patch: Value) {
    match patch {
        Value::Object(p) => {
            if !target.is_object() {
                *target = json!({});
            }
            let t = target.as_object_mut().expect("object");
            for (k, v) in p {
                if v.is_null() {
                    t.remove(&k);
                } else {
                    merge(t.entry(k).or_insert(Value::Null), v);
                }
            }
        }
        other => *target = other,
    }
}

/// Change a node with a JSON merge patch of its parameters; `ramp_ms` ramps the
/// numeric ones (default 10 ms).
async fn chain_patch(
    State(app): State<App>,
    Path((id, node)): Path<(String, String)>,
    bytes: axum::body::Bytes,
    kind: Kind,
) -> Response {
    let run = async {
        let mut patch: Value = body(&bytes)?;
        let ramp = ramp(take(&mut patch, "ramp_ms").and_then(|v| v.as_f64()))?;
        if take(&mut patch, "id").is_some_and(|v| v.as_str() != Some(node.as_str())) {
            return Err(bad("a node id cannot be changed"));
        }
        let errors = apply(&app, ramp, |m| {
            let chain = chain_mut(m, kind, &id)?;
            let p = chain
                .iter_mut()
                .find(|p| p.id == node)
                .ok_or_else(|| not_found(format!("{id} has no node {node}")))?;
            let mut v = json!(p);
            merge(&mut v, patch);
            *p = serde_json::from_value(v).map_err(|e| bad(e.to_string()))?;
            Ok(())
        })
        .await?;
        let chain = chain_of(&app, kind, &id)?;
        let p = chain.into_iter().find(|p| p.id == node);
        Ok::<_, Box<Response>>(Json(with_errors(json!(p), errors)).into_response())
    };
    run.await.unwrap_or_else(|r| *r)
}

async fn chain_delete(
    State(app): State<App>,
    Path((id, node)): Path<(String, String)>,
    kind: Kind,
) -> Response {
    let run = async {
        apply(&app, crate::engine::FADE_SAMPLES, |m| {
            let chain = chain_mut(m, kind, &id)?;
            let before = chain.len();
            chain.retain(|p| p.id != node);
            if chain.len() == before {
                return Err(not_found(format!("{id} has no node {node}")));
            }
            Ok(())
        })
        .await?;
        Ok::<_, Box<Response>>(StatusCode::NO_CONTENT.into_response())
    };
    run.await.unwrap_or_else(|r| *r)
}

/// Move a node to `{"position": n}` (0 = first).
async fn chain_move(
    State(app): State<App>,
    Path((id, node)): Path<(String, String)>,
    bytes: axum::body::Bytes,
    kind: Kind,
) -> Response {
    let run = async {
        let mut v: Value = body(&bytes)?;
        let position = take(&mut v, "position")
            .and_then(|p| p.as_u64())
            .ok_or_else(|| bad("position is required"))? as usize;
        apply(&app, crate::engine::FADE_SAMPLES, |m| {
            let chain = chain_mut(m, kind, &id)?;
            let from = chain
                .iter()
                .position(|p| p.id == node)
                .ok_or_else(|| not_found(format!("{id} has no node {node}")))?;
            let p = chain.remove(from);
            chain.insert(position.min(chain.len()), p);
            Ok(())
        })
        .await?;
        let chain = chain_of(&app, kind, &id)?;
        Ok::<_, Box<Response>>(Json(json!(chain)).into_response())
    };
    run.await.unwrap_or_else(|r| *r)
}

/// Clear every peak hold.
async fn clear_holds(State(app): State<App>) -> StatusCode {
    app.control().meters.clear_hold(None);
    StatusCode::NO_CONTENT
}

/// Clear the peak hold of one strip or bus.
async fn clear_hold(State(app): State<App>, Path(signal): Path<String>) -> Response {
    if app.control().meters.clear_hold(Some(&signal)) {
        StatusCode::NO_CONTENT.into_response()
    } else {
        *not_found(format!("signal {signal} has no meter"))
    }
}
