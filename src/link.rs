//! Remote link (§13): one master and up to four slaves. Audio stays on MXL.
//! The slave holds its mix when the master disconnects. With
//! `MIXER_LINK_TAKEOVER=deny`, local changes are then refused until the master
//! is back.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite;

use crate::config::LinkRole;
use crate::engine::FADE_SAMPLES;
use crate::mix::model::{Db, Mix};
use crate::ops::App;

/// What the master asks the slave to do.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Command {
    Mute { signal: String, mute: bool },
    Fader { signal: String, db: f64 },
    Recall { snapshot: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Msg {
    Hello {
        role: String,
        token: String,
        label: String,
    },
    State {
        revision: u64,
        #[serde(default)]
        levels: Vec<Level>,
    },
    Meters {
        peaks: BTreeMap<String, f64>,
    },
    Command {
        seq: u64,
        command: Command,
    },
    Ack {
        seq: u64,
        ok: bool,
        #[serde(default)]
        error: String,
    },
    /// Master → slave: apply the faders and mutes the master last saw.
    Push {
        levels: Vec<Level>,
    },
    Ping {
        t_ms: u64,
    },
    Pong {
        t_ms: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Level {
    signal: String,
    fader_db: f64,
    mute: bool,
}

/// One slave, as the master sees it, or the master, as the slave sees it.
#[derive(Debug, Clone, Serialize)]
pub struct Peer {
    pub id: String,
    pub label: String,
    pub up: bool,
    pub rtt_ms: Option<f64>,
    pub revision: u64,
}

#[derive(Default)]
pub struct Peers {
    pub list: Vec<Peer>,
    /// Last faders and mutes the master saw, per peer, for a push after reconnect.
    levels: BTreeMap<String, Vec<Level>>,
    /// Commands waiting for the connection task that owns the socket.
    pending: BTreeMap<String, Vec<Command>>,
    pending_push: BTreeMap<String, Vec<Level>>,
}

pub fn routes() -> Router<App> {
    Router::new()
        .route("/api/v1/link", get(socket))
        .route("/api/v1/remotes", get(remotes))
        .route("/api/v1/remotes/{id}/command", post(command))
        .route("/api/v1/remotes/{id}/push", post(push))
}

pub fn metrics(peers: &Peers) -> Vec<String> {
    let mut lines = Vec::new();
    for p in &peers.list {
        lines.push(format!(
            "remote_link_up{{peer=\"{}\"}} {}",
            p.id,
            u8::from(p.up)
        ));
        if let Some(rtt) = p.rtt_ms {
            lines.push(format!("remote_link_rtt_ms{{peer=\"{}\"}} {rtt}", p.id));
        }
    }
    lines
}

/// Apply one master command to the slave's mix. Recall uses the whole snapshot.
pub fn apply_command(mix: &mut Mix, command: &Command) -> Result<(), String> {
    match command {
        Command::Mute { signal, mute } => {
            let found = set_mute(mix, signal, *mute);
            if !found {
                return Err(format!("no signal {signal}"));
            }
            Ok(())
        }
        Command::Fader { signal, db } => {
            if !db.is_finite() && !db.is_infinite() {
                return Err("fader must be a number".into());
            }
            let found = set_fader(mix, signal, *db);
            if !found {
                return Err(format!("no signal {signal}"));
            }
            Ok(())
        }
        Command::Recall { snapshot } => {
            let snap = mix
                .snapshots
                .iter()
                .find(|s| s.id == *snapshot)
                .cloned()
                .ok_or_else(|| format!("no snapshot {snapshot}"))?;
            crate::mix::show::recall(mix, &snap, &crate::mix::show::Filter::default());
            Ok(())
        }
    }
}

/// Copy faders and mutes of signals that exist on both mixes.
pub(crate) fn push_levels(dst: &mut Mix, levels: &[Level]) {
    for level in levels {
        set_fader(dst, &level.signal, level.fader_db);
        set_mute(dst, &level.signal, level.mute);
    }
}

pub(crate) fn levels_of(mix: &Mix) -> Vec<Level> {
    let mut out = Vec::new();
    for s in &mix.strips {
        out.push(Level {
            signal: s.id.clone(),
            fader_db: s.fader_db.0,
            mute: s.mute,
        });
    }
    for b in &mix.buses {
        out.push(Level {
            signal: b.id.clone(),
            fader_db: b.fader_db.0,
            mute: b.mute,
        });
    }
    out
}

fn set_mute(mix: &mut Mix, signal: &str, mute: bool) -> bool {
    if let Some(s) = mix.strips.iter_mut().find(|s| s.id == signal) {
        s.mute = mute;
        return true;
    }
    if let Some(b) = mix.buses.iter_mut().find(|b| b.id == signal) {
        b.mute = mute;
        return true;
    }
    false
}

fn set_fader(mix: &mut Mix, signal: &str, db: f64) -> bool {
    let db = Db(if db.is_finite() {
        db.min(crate::mix::model::MAX_FADER_DB)
    } else {
        f64::NEG_INFINITY
    });
    if let Some(s) = mix.strips.iter_mut().find(|s| s.id == signal) {
        s.fader_db = db;
        return true;
    }
    if let Some(b) = mix.buses.iter_mut().find(|b| b.id == signal) {
        b.fader_db = db;
        return true;
    }
    false
}

fn token_ok(got: &str, want: &str) -> bool {
    if got.len() != want.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in got.bytes().zip(want.bytes()) {
        diff |= a ^ b;
    }
    diff == 0
}

async fn socket(ws: WebSocketUpgrade, State(app): State<App>) -> Response {
    if app.config.link_role != LinkRole::Slave {
        return axum::http::StatusCode::NOT_FOUND.into_response();
    }
    ws.on_upgrade(move |socket| slave(socket, app))
}

async fn slave(mut socket: WebSocket, app: App) {
    let Some(Ok(Message::Text(text))) = socket.recv().await else {
        return;
    };
    let Ok(Msg::Hello { token, .. }) = serde_json::from_str::<Msg>(&text) else {
        return;
    };
    if !token_ok(&token, &app.config.link_token) {
        let _ = socket
            .send(Message::Text(
                serde_json::to_string(&Msg::Ack {
                    seq: 0,
                    ok: false,
                    error: "token".into(),
                })
                .unwrap_or_default()
                .into(),
            ))
            .await;
        return;
    }
    {
        let mut c = app.control();
        c.link_up = true;
        c.link_seen = true;
    }
    let hello = Msg::Hello {
        role: "slave".into(),
        token: String::new(),
        label: app.config.host_id.clone(),
    };
    if socket.send(text_msg(&hello)).await.is_err() {
        mark_down(&app);
        return;
    }
    send_state(&app, &mut socket).await;
    let mut meters = tokio::time::interval(Duration::from_millis(100));
    let mut ping = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            msg = socket.recv() => {
                let Some(Ok(Message::Text(text))) = msg else {
                    mark_down(&app);
                    return;
                };
                if handle_slave(&app, &text, &mut socket).await.is_err() {
                    mark_down(&app);
                    return;
                }
            }
            _ = meters.tick() => {
                let peaks = peaks_of(&app);
                if socket.send(text_msg(&Msg::Meters { peaks })).await.is_err() {
                    mark_down(&app);
                    return;
                }
            }
            _ = ping.tick() => {
                let t_ms = now_ms();
                if socket.send(text_msg(&Msg::Ping { t_ms })).await.is_err() {
                    mark_down(&app);
                    return;
                }
            }
        }
    }
}

fn mark_down(app: &App) {
    let mut c = app.control();
    c.link_up = false;
    c.link_seen = true;
}

fn peaks_of(app: &App) -> BTreeMap<String, f64> {
    let c = app.control();
    let mut peaks = BTreeMap::new();
    for id in c
        .mix
        .strips
        .iter()
        .map(|s| s.id.as_str())
        .chain(c.mix.buses.iter().map(|b| b.id.as_str()))
    {
        if let Some(m) = c.meters.values().get(id) {
            let db = m
                .peak_dbfs
                .iter()
                .map(|d| d.0)
                .fold(f64::NEG_INFINITY, f64::max);
            if db.is_finite() {
                peaks.insert(id.to_string(), db);
            }
        }
    }
    peaks
}

async fn send_state(app: &App, socket: &mut WebSocket) {
    let (revision, levels) = {
        let c = app.control();
        (c.revision, levels_of(&c.mix))
    };
    let _ = socket
        .send(text_msg(&Msg::State { revision, levels }))
        .await;
}

async fn handle_slave(app: &App, text: &str, socket: &mut WebSocket) -> Result<(), ()> {
    let msg: Msg = serde_json::from_str(text).map_err(|_| ())?;
    match msg {
        Msg::Command { seq, command } => {
            let result = apply_remote(app, &command).await;
            let ack = Msg::Ack {
                seq,
                ok: result.is_ok(),
                error: result.err().unwrap_or_default(),
            };
            socket.send(text_msg(&ack)).await.map_err(|_| ())?;
        }
        Msg::Push { levels } => {
            let result = apply_push(app, &levels).await;
            let ack = Msg::Ack {
                seq: 0,
                ok: result.is_ok(),
                error: result.err().unwrap_or_default(),
            };
            socket.send(text_msg(&ack)).await.map_err(|_| ())?;
        }
        Msg::Pong { t_ms } => {
            let rtt = now_ms().saturating_sub(t_ms) as f64;
            let mut peers = app.link.lock().await;
            if let Some(p) = peers.list.iter_mut().find(|p| p.id == "master") {
                p.rtt_ms = Some(rtt);
                p.up = true;
            } else {
                peers.list.push(Peer {
                    id: "master".into(),
                    label: "master".into(),
                    up: true,
                    rtt_ms: Some(rtt),
                    revision: 0,
                });
            }
        }
        Msg::Ping { t_ms } => {
            socket
                .send(text_msg(&Msg::Pong { t_ms }))
                .await
                .map_err(|_| ())?;
        }
        _ => {}
    }
    Ok(())
}

async fn apply_remote(app: &App, command: &Command) -> Result<(), String> {
    let command = command.clone();
    let _serial = app.changes.lock().await;
    let staged = {
        let mut control = app.control();
        control.remote_write = true;
        let mut next = control.mix.clone();
        let result = apply_command(&mut next, &command);
        control.remote_write = false;
        result?;
        control.stage(next, FADE_SAMPLES)
    };
    finish(app, staged).await
}

async fn apply_push(app: &App, levels: &[Level]) -> Result<(), String> {
    let levels = levels.to_vec();
    let _serial = app.changes.lock().await;
    let staged = {
        let mut control = app.control();
        control.remote_write = true;
        let mut next = control.mix.clone();
        push_levels(&mut next, &levels);
        control.remote_write = false;
        control.stage(next, FADE_SAMPLES)
    };
    finish(app, staged).await
}

async fn finish(app: &App, staged: Result<crate::control::Staged, String>) -> Result<(), String> {
    match staged? {
        crate::control::Staged::Done => Ok(()),
        crate::control::Staged::Swap { wait } => {
            if wait {
                tokio::time::sleep(crate::control::fade_wait(app.config.block_samples)).await;
            }
            app.control().swap().map(|_| ())
        }
    }
}

fn text_msg(msg: &Msg) -> Message {
    Message::Text(serde_json::to_string(msg).unwrap_or_default().into())
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Master: keep a connection to one slave and remember its levels.
pub async fn master(app: App, url: String) {
    let id = url.clone();
    loop {
        match master_once(&app, &url, &id).await {
            Ok(()) => {}
            Err(e) => tracing::warn!(peer = %url, "remote link: {e}"),
        }
        {
            let mut peers = app.link.lock().await;
            if let Some(p) = peers.list.iter_mut().find(|p| p.id == id) {
                p.up = false;
            }
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

async fn master_once(app: &App, url: &str, id: &str) -> Result<(), String> {
    let (mut socket, _) = tokio_tungstenite::connect_async(url)
        .await
        .map_err(|e| e.to_string())?;
    let hello = Msg::Hello {
        role: "master".into(),
        token: app.config.link_token.clone(),
        label: app.config.host_id.clone(),
    };
    socket
        .send(tungstenite::Message::Text(
            serde_json::to_string(&hello).unwrap_or_default().into(),
        ))
        .await
        .map_err(|e| e.to_string())?;
    {
        let mut peers = app.link.lock().await;
        if let Some(p) = peers.list.iter_mut().find(|p| p.id == id) {
            p.up = true;
        } else {
            peers.list.push(Peer {
                id: id.into(),
                label: String::new(),
                up: true,
                rtt_ms: None,
                revision: 0,
            });
        }
    }
    let started = Instant::now();
    let mut flush = tokio::time::interval(Duration::from_millis(50));
    while started.elapsed() < Duration::from_secs(3600) {
        tokio::select! {
            msg = socket.next() => {
                let Some(msg) = msg else { return Err("closed".into()); };
                let msg = msg.map_err(|e| e.to_string())?;
                let tungstenite::Message::Text(text) = msg else { continue; };
                handle_master(app, id, &text, &mut socket).await?;
            }
            _ = flush.tick() => {
                flush_master(app, id, &mut socket).await?;
            }
        }
    }
    Ok(())
}

async fn handle_master(
    app: &App,
    id: &str,
    text: &str,
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> Result<(), String> {
    let Ok(parsed) = serde_json::from_str::<Msg>(text) else {
        return Ok(());
    };
    match parsed {
        Msg::Hello { label, .. } => {
            let mut peers = app.link.lock().await;
            if let Some(p) = peers.list.iter_mut().find(|p| p.id == id) {
                p.label = label;
            }
        }
        Msg::State { revision, levels } => {
            let mut peers = app.link.lock().await;
            if !levels.is_empty() {
                peers.levels.insert(id.to_string(), levels);
            }
            if let Some(p) = peers.list.iter_mut().find(|p| p.id == id) {
                p.revision = revision;
                p.up = true;
            }
        }
        Msg::Ping { t_ms } => {
            let rtt = now_ms().saturating_sub(t_ms) as f64;
            let mut peers = app.link.lock().await;
            if let Some(p) = peers.list.iter_mut().find(|p| p.id == id) {
                p.rtt_ms = Some(rtt);
            }
            socket
                .send(tungstenite::Message::Text(
                    serde_json::to_string(&Msg::Pong { t_ms })
                        .unwrap_or_default()
                        .into(),
                ))
                .await
                .map_err(|e| e.to_string())?;
        }
        Msg::Meters { peaks: _ } => {}
        _ => {}
    }
    Ok(())
}

async fn flush_master(
    app: &App,
    id: &str,
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> Result<(), String> {
    let (commands, push) = {
        let mut peers = app.link.lock().await;
        (
            peers.pending.remove(id).unwrap_or_default(),
            peers.pending_push.remove(id),
        )
    };
    for (seq, command) in commands.into_iter().enumerate() {
        socket
            .send(tungstenite::Message::Text(
                serde_json::to_string(&Msg::Command {
                    seq: seq as u64 + 1,
                    command,
                })
                .unwrap_or_default()
                .into(),
            ))
            .await
            .map_err(|e| e.to_string())?;
    }
    if let Some(levels) = push {
        socket
            .send(tungstenite::Message::Text(
                serde_json::to_string(&Msg::Push { levels })
                    .unwrap_or_default()
                    .into(),
            ))
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

async fn remotes(State(app): State<App>) -> Json<Value> {
    let peers = app.link.lock().await;
    Json(json!({ "peers": peers.list }))
}

async fn command(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(command): Json<Command>,
) -> Result<Json<Value>, axum::http::StatusCode> {
    if app.config.link_role != LinkRole::Master {
        return Err(axum::http::StatusCode::NOT_FOUND);
    }
    let peers = app.link.lock().await;
    if !peers.list.iter().any(|p| p.id == id && p.up) {
        return Err(axum::http::StatusCode::CONFLICT);
    }
    drop(peers);
    // The live socket is the master's; commands are applied by the slave when
    // it receives them. This process is the master, so the command is queued
    // on the peer for the connection task. A direct send needs the socket,
    // which the connection task owns. Store the command for that task.
    let mut peers = app.link.lock().await;
    peers.pending.entry(id).or_default().push(command);
    Ok(Json(json!({"queued": true})))
}

async fn push(
    State(app): State<App>,
    Path(id): Path<String>,
) -> Result<Json<Value>, axum::http::StatusCode> {
    if app.config.link_role != LinkRole::Master {
        return Err(axum::http::StatusCode::NOT_FOUND);
    }
    let mut peers = app.link.lock().await;
    let Some(levels) = peers.levels.get(&id).cloned() else {
        return Err(axum::http::StatusCode::CONFLICT);
    };
    peers.pending_push.insert(id, levels);
    Ok(Json(json!({"queued": true})))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mix::model::Db;

    fn strip(id: &str) -> crate::mix::model::Strip {
        crate::mix::model::tests::strip(id, crate::mix::Format::Mono)
    }

    #[test]
    fn commands_change_the_mix_and_a_push_copies_levels() {
        let mut mix = Mix::default();
        mix.strips.push(strip("vox"));
        apply_command(
            &mut mix,
            &Command::Fader {
                signal: "vox".into(),
                db: -12.0,
            },
        )
        .unwrap();
        apply_command(
            &mut mix,
            &Command::Mute {
                signal: "vox".into(),
                mute: true,
            },
        )
        .unwrap();
        assert_eq!(mix.strips[0].fader_db, Db(-12.0));
        assert!(mix.strips[0].mute);
        let levels = levels_of(&mix);
        let mut other = Mix::default();
        other.strips.push(strip("vox"));
        push_levels(&mut other, &levels);
        assert_eq!(other.strips[0].fader_db, Db(-12.0));
        assert!(other.strips[0].mute);
        assert!(apply_command(
            &mut mix,
            &Command::Mute {
                signal: "nope".into(),
                mute: true,
            },
        )
        .is_err());
        assert!(token_ok("secret-token", "secret-token"));
        assert!(!token_ok("secret-token", "secret-tokEn"));
    }
}
