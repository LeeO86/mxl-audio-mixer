//! Live updates (§7, §12): the WebSocket `/api/v1/events`.
//!
//! The server sends JSON messages:
//! - `{"type": "status", "status": …}` five times a second (as `GET /api/v1/status`);
//! - `{"type": "mix", "revision": n, "mix": …, "latency_ms": {signal: ms}}` on connect
//!   and within 40 ms of every change of the mix, from any client;
//! - `{"type": "meters", "meters": …, "loudness": …, "vad": {signal: {"active",
//!   "probability"}}, "auto_db": {signal: dB}}` at the meter rate (25 Hz, as
//!   `GET /api/v1/meters` and `GET /api/v1/loudness`; `vad` and `auto_db` are the
//!   analysed signals and the auto nodes' gains, §8).
//!
//! - `{"type": "analysis", …}` with the meters, for clients that subscribed to it
//!   (as `values` of `GET /api/v1/analysis`).
//!
//! A client may send `{"type": "subscribe", "meters": ["vox", …] | null,
//! "meter_hz": 25, "analysis": false}`: only those signals' meters (null: all), at
//! most that often, and the analyser or not. Changes go through the REST API.

use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::meters::METER_TICK;
use crate::ops::{status_json, App};

const STATUS_PERIOD: Duration = Duration::from_millis(200);

pub async fn events(ws: WebSocketUpgrade, State(app): State<App>) -> Response {
    ws.on_upgrade(move |socket| run(socket, app))
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
enum ClientMessage {
    Subscribe {
        #[serde(default)]
        meters: Option<Vec<String>>,
        #[serde(default)]
        meter_hz: Option<f64>,
        #[serde(default)]
        analysis: bool,
    },
}

fn mix_json(app: &App) -> (u64, Value) {
    let control = app.control();
    let latency: serde_json::Map<String, Value> = control
        .latency
        .iter()
        .map(|(s, l)| (s.clone(), json!(*l as f64 / 48.0)))
        .collect();
    (
        control.revision,
        json!({"type": "mix", "revision": control.revision, "mix": control.mix, "latency_ms": latency}),
    )
}

fn meters_json(app: &App, only: Option<&[String]>) -> Value {
    let control = app.control();
    let mut meters = control.meters.values();
    let mut loudness = control.loudness.values(&control.mix.settings);
    let mut vad: serde_json::Map<String, Value> = control
        .auto
        .vads
        .iter()
        .map(|(s, v)| {
            let p = (v.state.speech_probability * 100.0).round() / 100.0;
            (
                s.clone(),
                json!({"active": v.state.speech_active, "probability": p}),
            )
        })
        .collect();
    let mut auto_db = control.auto.gains_db();
    if let Some(only) = only {
        meters.retain(|s, _| only.contains(s));
        loudness.retain(|s, _| only.contains(s));
        vad.retain(|s, _| only.contains(s));
        auto_db.retain(|s, _| only.contains(s));
    }
    json!({"type": "meters", "meters": meters, "loudness": loudness, "vad": vad, "auto_db": auto_db})
}

async fn send(socket: &mut WebSocket, value: &Value) -> bool {
    socket
        .send(Message::Text(value.to_string().into()))
        .await
        .is_ok()
}

async fn run(mut socket: WebSocket, app: App) {
    let mut status = tokio::time::interval(STATUS_PERIOD);
    let mut meter = tokio::time::interval(METER_TICK);
    let mut revision = None;
    let mut only: Option<Vec<String>> = None;
    let mut every = 1u64;
    let mut ticks = 0u64;
    let mut analysis = false;
    loop {
        tokio::select! {
            _ = status.tick() => {
                let s = json!({"type": "status", "status": status_json(&app)});
                if !send(&mut socket, &s).await {
                    break;
                }
            }
            _ = meter.tick() => {
                // A changed mix goes out within one meter tick.
                if revision != Some(app.control().revision) {
                    let (rev, mix) = mix_json(&app);
                    revision = Some(rev);
                    if !send(&mut socket, &mix).await {
                        break;
                    }
                }
                ticks += 1;
                if ticks.is_multiple_of(every) {
                    let m = meters_json(&app, only.as_deref());
                    if !send(&mut socket, &m).await {
                        break;
                    }
                    if analysis {
                        let a = app.control().analyser.values().map(|v| {
                            let mut a = json!(v);
                            a["type"] = json!("analysis");
                            a
                        });
                        if let Some(a) = a {
                            if !send(&mut socket, &a).await {
                                break;
                            }
                        }
                    }
                }
            }
            message = socket.recv() => match message {
                Some(Ok(Message::Text(text))) => {
                    if let Ok(ClientMessage::Subscribe { meters, meter_hz, analysis: a }) = serde_json::from_str(&text) {
                        only = meters;
                        analysis = a;
                        let base = 1000.0 / METER_TICK.as_millis() as f64;
                        every = meter_hz
                            .filter(|hz| *hz > 0.0)
                            .map_or(1, |hz| (base / hz).round().max(1.0) as u64);
                    }
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                _ => {}
            },
        }
    }
}
