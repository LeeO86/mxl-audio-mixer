//! Remote link (§13): one master and up to four slaves.
//!
//! The gallery works the far-side desk (strips, auxiliaries, in-ears) over this
//! link. The far side mixes its own voices, so those in-ears do not wait for the
//! trip home and back. The gallery builds one mix-minus of its programme — the
//! other sources, without the far-side contribution — publishes it, and tells
//! the far side to add that return to each in-ear. The audio of both the
//! contribution and the return goes by MXL, not by this socket.
//!
//! The slave holds its mix when the master disconnects. With
//! `MIXER_LINK_TAKEOVER=deny`, local changes are then refused until the master
//! is back.
//!
//! The slave takes one master at a time. Each side pings every second; a side
//! that hears nothing for [`DEAD_AFTER`] closes the link (the master then
//! reconnects). Commands are sent at most once: one that is not sent when the
//! link drops is dropped, and the master pushes its levels again if it wants.
//! A message of a type this version does not know is ignored.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::time::Instant;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite;

use crate::config::LinkRole;
use crate::engine::FADE_SAMPLES;
use crate::mix::model::{
    Bus, Db, Kind, Minus, Mix, Output, Patch, Send, Tap, MAX_GAIN_DB, MAX_OUTPUTS,
};
use crate::mix::Format;
use crate::ops::App;

/// A side that hears nothing for this long closes the link. Both sides ping
/// every [`PING_EVERY`], and the slave sends meters ten times a second.
pub const DEAD_AFTER: Duration = Duration::from_secs(5);
const PING_EVERY: Duration = Duration::from_secs(1);
/// A connection that has not sent its hello by then is closed.
const HELLO_WITHIN: Duration = Duration::from_secs(5);
/// The master gives up a connection attempt after this long and retries.
const CONNECT_WITHIN: Duration = Duration::from_secs(10);
/// The largest link message, both ways (axum's default is 64 MiB). A desk or a
/// push is a part of the mix, which the API takes up to 2 MB.
pub const MAX_MESSAGE: usize = 4 << 20;

/// The slave's side of the link (§13): the master connection that holds it,
/// and whether local changes are refused.
#[derive(Debug, Default)]
pub struct Takeover {
    /// A slave with `MIXER_LINK_TAKEOVER=deny`.
    pub deny: bool,
    /// A master has connected at least once.
    pub seen: bool,
    /// The connection that holds the link: one master at a time.
    owner: Option<u64>,
    /// The master's own change is being staged.
    pub remote_write: bool,
}

impl Takeover {
    pub fn new(deny: bool) -> Self {
        Self {
            deny,
            ..Self::default()
        }
    }

    /// A master holds the link.
    pub fn up(&self) -> bool {
        self.owner.is_some()
    }

    /// Connection `conn` takes the link; false while another master holds it.
    pub fn claim(&mut self, conn: u64) -> bool {
        if self.owner.is_some() {
            return false;
        }
        self.owner = Some(conn);
        self.seen = true;
        true
    }

    /// Connection `conn` ended. Only the connection that holds the link
    /// brings it down.
    pub fn release(&mut self, conn: u64) {
        if self.owner == Some(conn) {
            self.owner = None;
        }
    }

    /// Takeover deny refuses this change: a master had connected, none holds
    /// the link now, and the change is not the master's own. Start-up (no
    /// master yet) still loads the show.
    pub fn refuses(&self) -> bool {
        self.deny && self.seen && self.owner.is_none() && !self.remote_write
    }
}

/// What the master asks the slave to do.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Command {
    Mute {
        signal: String,
        mute: bool,
    },
    Fader {
        signal: String,
        db: Db,
    },
    /// An auxiliary send. A send that is not there yet is created (a strip
    /// post-pan, a bus post-fader).
    Send {
        from: String,
        to: String,
        db: Db,
        mute: bool,
    },
    Recall {
        snapshot: String,
    },
    /// Far-side in-ears: one bus per local voice (the other voices, plus the
    /// gallery return). The return is a strip on the far side, patched to the
    /// flow the gallery published.
    Ears {
        return_strip: String,
        voices: Vec<String>,
    },
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
        #[serde(default)]
        desk: Option<Desk>,
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
    /// Master → slave: apply the faders, mutes and sends the master last saw.
    Push {
        #[serde(default)]
        seq: u64,
        levels: Vec<Level>,
        #[serde(default)]
        sends: Vec<DeskSend>,
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

/// The far-side desk, as the gallery draws it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Desk {
    pub strips: Vec<DeskStrip>,
    pub buses: Vec<DeskBus>,
    pub sends: Vec<DeskSend>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeskStrip {
    pub id: String,
    pub name: String,
    pub format: Format,
    pub fader_db: Db,
    pub mute: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colour: Option<String>,
    /// One label per channel: `1.2` (input 1 channel 2), `tb:<id>`, or empty.
    #[serde(default)]
    pub patch: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeskBus {
    pub id: String,
    pub name: String,
    pub format: Format,
    pub fader_db: Db,
    pub mute: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colour: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minus: Option<Minus>,
    /// Delay compensation is off: an in-ear or IFB, mixed at the far side's latency.
    pub ears: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeskSend {
    pub from: String,
    pub to: String,
    pub gain_db: Db,
    pub mute: bool,
    /// Tapped before the fader.
    pub pre: bool,
}

/// One slave, as the master sees it, or the master, as the slave sees it.
/// `id` is a short name (`s1` … `s4`, or `master`); `url` is the socket.
#[derive(Debug, Clone, Serialize)]
pub struct Peer {
    pub id: String,
    pub url: String,
    pub label: String,
    pub up: bool,
    pub rtt_ms: Option<f64>,
    pub revision: u64,
    #[serde(default)]
    pub last_error: String,
}

/// A push of the levels and sends the master last saw.
struct PushBody {
    levels: Vec<Level>,
    sends: Vec<DeskSend>,
}

#[derive(Default)]
pub struct Peers {
    pub list: Vec<Peer>,
    /// Last faders and mutes the master saw, per peer, for a push after reconnect.
    levels: BTreeMap<String, Vec<Level>>,
    desks: BTreeMap<String, Desk>,
    peaks: BTreeMap<String, BTreeMap<String, f64>>,
    /// The gallery's published mix-minus bus for this far side.
    returns: BTreeMap<String, String>,
    /// Commands waiting for the connection task that owns the socket.
    pending: BTreeMap<String, Vec<Command>>,
    pending_push: BTreeMap<String, PushBody>,
}

pub fn routes() -> Router<App> {
    Router::new()
        .route("/api/v1/link", get(socket))
        .route("/api/v1/remotes", get(remotes))
        .route("/api/v1/remotes/{id}/command", post(command))
        .route("/api/v1/remotes/{id}/push", post(push))
        .route("/api/v1/remotes/{id}/return", post(gallery_return))
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
            let found = set_fader(mix, signal, db.0);
            if !found {
                return Err(format!("no signal {signal}"));
            }
            Ok(())
        }
        Command::Send { from, to, db, mute } => set_send(mix, from, to, *db, *mute),
        Command::Ears {
            return_strip,
            voices,
        } => apply_ears(mix, return_strip, voices),
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

/// Copy faders, mutes and sends onto signals that exist.
pub(crate) fn push_levels(dst: &mut Mix, levels: &[Level], sends: &[DeskSend]) {
    for level in levels {
        set_fader(dst, &level.signal, level.fader_db);
        set_mute(dst, &level.signal, level.mute);
    }
    for send in sends {
        let _ = set_send(dst, &send.from, &send.to, send.gain_db, send.mute);
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

/// The desk the gallery draws: strips, buses and sends, without the processing.
pub fn desk_of(mix: &Mix) -> Desk {
    Desk {
        strips: mix
            .strips
            .iter()
            .map(|s| DeskStrip {
                id: s.id.clone(),
                name: s.name.clone(),
                format: s.format,
                fader_db: s.fader_db,
                mute: s.mute,
                colour: s.colour.clone(),
                patch: s.patch.iter().map(patch_label).collect(),
            })
            .collect(),
        buses: mix
            .buses
            .iter()
            .map(|b| DeskBus {
                id: b.id.clone(),
                name: b.name.clone(),
                format: b.format,
                fader_db: b.fader_db,
                mute: b.mute,
                colour: b.colour.clone(),
                minus: b.minus.clone(),
                ears: !b.compensate,
            })
            .collect(),
        sends: mix
            .sends
            .iter()
            .map(|s| DeskSend {
                from: s.from.clone(),
                to: s.to.clone(),
                gain_db: s.gain_db,
                mute: s.mute,
                pre: !matches!(s.tap, Tap::PostFader | Tap::PostPan),
            })
            .collect(),
    }
}

fn patch_label(patch: &Option<Patch>) -> String {
    match patch {
        Some(Patch::Input(p)) => format!("{}.{}", p.input, p.channel),
        Some(Patch::Talkback(t)) => format!("tb:{}", t.talkback),
        None => String::new(),
    }
}

/// Create or update the gallery's mix-minus of `of` without the far-side
/// contribution, and publish it so it can be routed to the far side.
pub fn ensure_return(
    mix: &mut Mix,
    peer: &str,
    of: &str,
    without: &[String],
) -> Result<String, String> {
    if mix.kind(of) != Some(Kind::Bus) {
        return Err(format!("{of} is not a bus"));
    }
    for w in without {
        if mix.kind(w).is_none() {
            return Err(format!("signal {w} does not exist"));
        }
    }
    let id = return_id(mix, peer)?;
    let format = mix.bus(of).map(|b| b.format).unwrap_or(Format::Stereo);
    let minus = Minus {
        of: of.to_string(),
        without: without.to_vec(),
    };
    if let Some(bus) = mix.buses.iter_mut().find(|b| b.id == id) {
        bus.minus = Some(minus);
    } else {
        let mut bus = Bus::new(&id, format);
        bus.name = format!("Return {id}");
        bus.minus = Some(minus);
        mix.buses.push(bus);
    }
    if mix.output(&id).is_none() {
        if mix.outputs.len() >= MAX_OUTPUTS {
            return Err("no free published output for the gallery return".into());
        }
        mix.outputs.push(Output {
            signal: id.clone(),
            format: None,
            commit_samples: None,
        });
    }
    Ok(id)
}

/// One in-ear per local voice: the other far-side voices, plus the gallery return.
pub fn apply_ears(mix: &mut Mix, return_strip: &str, voices: &[String]) -> Result<(), String> {
    if mix.kind(return_strip) != Some(Kind::Strip) {
        return Err(format!("{return_strip} is not a strip"));
    }
    if voices.is_empty() {
        return Err("name the far-side voices".into());
    }
    if voices.iter().any(|v| v == return_strip) {
        return Err("the gallery return is not one of the local voices".into());
    }
    let mut seen = BTreeSet::new();
    for v in voices {
        if !seen.insert(v.as_str()) {
            return Err(format!("voice {v} is named twice"));
        }
        if mix.kind(v) != Some(Kind::Strip) {
            return Err(format!("{v} is not a strip"));
        }
    }
    let names: Vec<(String, String)> = voices
        .iter()
        .map(|v| {
            let name = mix
                .strip(v)
                .map(|s| s.name.clone())
                .unwrap_or_else(|| v.clone());
            (v.clone(), name)
        })
        .collect();
    for (voice, name) in &names {
        let id = iem_id(mix, voice);
        if mix.kind(&id).is_none() {
            let mut bus = Bus::new(&id, Format::Mono);
            bus.name = format!("IEM {name}");
            bus.compensate = false;
            mix.buses.push(bus);
        }
        for (other, _) in names.iter().filter(|(other, _)| other != voice) {
            ensure_send(mix, other, &id, Tap::PostFader);
        }
        ensure_send(mix, return_strip, &id, Tap::PostFader);
    }
    Ok(())
}

/// `ret-<peer>` (`ret-s1`), so a rebuild finds the same bus after a restart.
/// Refused while another signal has that id: a fresh id each time would leave
/// a new bus and output behind on every rebuild.
fn return_id(mix: &Mix, peer: &str) -> Result<String, String> {
    let wanted = format!("ret-{peer}");
    let kept = mix.bus(&wanted).and_then(|b| b.minus.as_ref()).is_some();
    let free = mix.kind(&wanted).is_none() && mix.vca(&wanted).is_none();
    if kept || free {
        Ok(wanted)
    } else {
        Err(format!(
            "{wanted} is another signal: rename it to build the gallery return"
        ))
    }
}

fn iem_id(mix: &Mix, voice: &str) -> String {
    let wanted = format!("iem-{voice}");
    let ok = (1..=32).contains(&wanted.len())
        && !wanted.starts_with('-')
        && wanted
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if ok {
        match mix.kind(&wanted) {
            None if mix.vca(&wanted).is_none() => return wanted,
            Some(Kind::Bus) => return wanted,
            _ => {}
        }
    }
    mix.next_id("iem")
}

/// Add a 0 dB send when it is missing. An existing send keeps its level.
fn ensure_send(mix: &mut Mix, from: &str, to: &str, tap: Tap) {
    if mix.send(from, to).is_some() {
        return;
    }
    mix.sends.push(Send {
        from: from.to_string(),
        to: to.to_string(),
        tap,
        gain_db: Db(0.0),
        mute: false,
        phase: false,
        block_delay: false,
    });
}

fn set_send(mix: &mut Mix, from: &str, to: &str, db: Db, mute: bool) -> Result<(), String> {
    if mix.kind(from).is_none() {
        return Err(format!("no signal {from}"));
    }
    if mix.kind(to) != Some(Kind::Bus) {
        return Err(format!("{to} is not a bus"));
    }
    let db = Db(if db.0.is_finite() {
        db.0.min(MAX_GAIN_DB)
    } else {
        f64::NEG_INFINITY
    });
    if let Some(send) = mix.sends.iter_mut().find(|s| s.from == from && s.to == to) {
        send.gain_db = db;
        send.mute = mute;
        return Ok(());
    }
    let tap = mix.default_tap(from);
    mix.sends.push(Send {
        from: from.to_string(),
        to: to.to_string(),
        tap,
        gain_db: db,
        mute,
        phase: false,
        block_delay: false,
    });
    Ok(())
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
    ws.max_message_size(MAX_MESSAGE)
        .max_frame_size(MAX_MESSAGE)
        .on_upgrade(move |socket| slave(socket, app))
}

/// Numbers the master connections, so that only the one holding the link ends it.
static CONNECTIONS: AtomicU64 = AtomicU64::new(1);

/// The link, held by one master connection. Dropping it brings the link down.
struct Held {
    app: App,
    conn: u64,
}

impl Drop for Held {
    fn drop(&mut self) {
        self.app.control().link.release(self.conn);
    }
}

async fn slave(mut socket: WebSocket, app: App) {
    let hello = match tokio::time::timeout(HELLO_WITHIN, socket.recv()).await {
        Ok(Some(Ok(Message::Text(text)))) => serde_json::from_str::<Msg>(&text).ok(),
        _ => None,
    };
    let Some(Msg::Hello { token, label, .. }) = hello else {
        return;
    };
    if !token_ok(&token, &app.config.link_token) {
        refuse(&mut socket, "token").await;
        return;
    }
    let conn = CONNECTIONS.fetch_add(1, Ordering::Relaxed);
    if !app.control().link.claim(conn) {
        tracing::warn!(master = %label, "remote link: refused a second master");
        refuse(&mut socket, "another master is connected").await;
        return;
    }
    let held = Held {
        app: app.clone(),
        conn,
    };
    app.link.lock().await.master_seen(Some(&label));
    if let Err(e) = slave_session(&app, &mut socket).await {
        tracing::warn!(master = %label, "remote link: {e}");
    }
    // Down in the peer list before the link is free for the next master.
    app.link.lock().await.master_seen(None);
    drop(held);
}

async fn refuse(socket: &mut WebSocket, error: &str) {
    let ack = Msg::Ack {
        seq: 0,
        ok: false,
        error: error.into(),
    };
    let _ = send(socket, text_msg(&ack)).await;
}

/// Returns only when the link has ended.
async fn slave_session(app: &App, socket: &mut WebSocket) -> Result<(), String> {
    let hello = Msg::Hello {
        role: "slave".into(),
        token: String::new(),
        label: app.config.host_id.clone(),
    };
    send(socket, text_msg(&hello)).await?;
    send_state(app, socket).await?;
    let mut sent = app.control().revision;
    let mut meters = tokio::time::interval(Duration::from_millis(100));
    let mut ping = tokio::time::interval(PING_EVERY);
    let mut heard = Instant::now();
    let mut logged = false;
    loop {
        tokio::select! {
            msg = next_within(socket, heard) => {
                match msg? {
                    Some(Ok(Message::Text(text))) => {
                        heard = Instant::now();
                        handle_slave(app, &text, socket, &mut logged).await?;
                    }
                    Some(Ok(Message::Close(_))) | None => return Err("the master closed the link".into()),
                    Some(Ok(_)) => heard = Instant::now(),
                    Some(Err(e)) => return Err(e.to_string()),
                }
            }
            _ = meters.tick() => {
                let rev = app.control().revision;
                if rev != sent {
                    send_state(app, socket).await?;
                }
                sent = app.control().revision;
                let peaks = peaks_of(app);
                send(socket, text_msg(&Msg::Meters { peaks })).await?;
            }
            _ = ping.tick() => {
                send(socket, text_msg(&Msg::Ping { t_ms: now_ms() })).await?;
            }
        }
    }
}

/// The next frame, or an error when nothing has come for [`DEAD_AFTER`] since
/// `heard`: a peer that is gone without closing the socket.
async fn next_within<S>(socket: &mut S, heard: Instant) -> Result<Option<S::Item>, String>
where
    S: futures_util::Stream + Unpin,
{
    tokio::time::timeout_at(heard + DEAD_AFTER, socket.next())
        .await
        .map_err(|_| format!("nothing heard for {} s", DEAD_AFTER.as_secs()))
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

async fn send_state(app: &App, socket: &mut WebSocket) -> Result<(), String> {
    let (revision, levels, desk) = {
        let c = app.control();
        (c.revision, levels_of(&c.mix), desk_of(&c.mix))
    };
    let state = Msg::State {
        revision,
        levels,
        desk: Some(desk),
    };
    send(socket, text_msg(&state)).await
}

/// A message this version does not know: its `type`, and its `seq` (a command's).
#[derive(Debug, PartialEq)]
struct Unknown {
    kind: String,
    seq: u64,
}

fn parse(text: &str) -> Result<Msg, Unknown> {
    serde_json::from_str(text).map_err(|_| {
        let v: Value = serde_json::from_str(text).unwrap_or_default();
        Unknown {
            kind: v["type"].as_str().unwrap_or_default().to_string(),
            seq: v["seq"].as_u64().unwrap_or(0),
        }
    })
}

/// A peer of another version may send what this one does not know: ignore it,
/// and say so once per connection.
fn note_unknown(logged: &mut bool, kind: &str) {
    if !*logged {
        *logged = true;
        tracing::warn!(
            kind,
            "remote link: ignoring a message this version does not know (logged once per connection)"
        );
    }
}

async fn handle_slave(
    app: &App,
    text: &str,
    socket: &mut WebSocket,
    logged: &mut bool,
) -> Result<(), String> {
    let msg = match parse(text) {
        Ok(msg) => msg,
        Err(unknown) if unknown.kind == "command" => {
            // A command this version does not understand: refuse it, keep the link.
            let ack = Msg::Ack {
                seq: unknown.seq,
                ok: false,
                error: "command not understood".into(),
            };
            return send(socket, text_msg(&ack)).await;
        }
        Err(unknown) => {
            note_unknown(logged, &unknown.kind);
            return Ok(());
        }
    };
    match msg {
        Msg::Command { seq, command } => {
            let result = apply_remote(app, &command).await;
            let ok = result.is_ok();
            let ack = Msg::Ack {
                seq,
                ok,
                error: result.err().unwrap_or_default(),
            };
            send(socket, text_msg(&ack)).await?;
            if ok {
                send_state(app, socket).await?;
            }
        }
        Msg::Push { seq, levels, sends } => {
            let result = apply_push(app, &levels, &sends).await;
            let ok = result.is_ok();
            let ack = Msg::Ack {
                seq,
                ok,
                error: result.err().unwrap_or_default(),
            };
            send(socket, text_msg(&ack)).await?;
            if ok {
                send_state(app, socket).await?;
            }
        }
        Msg::Pong { t_ms } => app.link.lock().await.pong("master", t_ms, now_ms()),
        Msg::Ping { t_ms } => send(socket, text_msg(&Msg::Pong { t_ms })).await?,
        _ => {}
    }
    Ok(())
}

/// Stage the master's change: takeover deny never refuses it (§13).
fn stage_remote(
    control: &mut crate::control::Control,
    next: Mix,
) -> Result<crate::control::Staged, String> {
    control.link.remote_write = true;
    let staged = control.stage(next, FADE_SAMPLES);
    control.link.remote_write = false;
    staged
}

async fn apply_remote(app: &App, command: &Command) -> Result<(), String> {
    let command = command.clone();
    let _serial = app.changes.lock().await;
    let staged = {
        let mut control = app.control();
        let mut next = control.mix.clone();
        apply_command(&mut next, &command)?;
        stage_remote(&mut control, next)
    };
    finish(app, staged).await
}

async fn apply_push(app: &App, levels: &[Level], sends: &[DeskSend]) -> Result<(), String> {
    let levels = levels.to_vec();
    let sends = sends.to_vec();
    let _serial = app.changes.lock().await;
    let staged = {
        let mut control = app.control();
        let mut next = control.mix.clone();
        push_levels(&mut next, &levels, &sends);
        stage_remote(&mut control, next)
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

fn ws_text(msg: &Msg) -> tungstenite::Message {
    tungstenite::Message::Text(serde_json::to_string(msg).unwrap_or_default().into())
}

/// Send one message. A peer that does not take it within [`DEAD_AFTER`] is gone.
async fn send<S, M>(socket: &mut S, message: M) -> Result<(), String>
where
    S: futures_util::Sink<M> + Unpin,
    S::Error: std::fmt::Display,
{
    match tokio::time::timeout(DEAD_AFTER, socket.send(message)).await {
        Ok(result) => result.map_err(|e| e.to_string()),
        Err(_) => Err(format!("a send waited {} s", DEAD_AFTER.as_secs())),
    }
}

/// Milliseconds on this process's monotonic clock. A ping's stamp comes back
/// in its pong, so the round trip reads one host's clock only.
fn now_ms() -> u64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_millis() as u64
}

/// A slave's id is its place in `MIXER_LINK_PEERS` (`s1` … `s4`), so it stays
/// the same across restarts whichever slave answers first.
fn peer_id(urls: &[String], url: &str) -> String {
    let n = urls.iter().position(|u| u == url).unwrap_or(0);
    format!("s{}", n + 1)
}

impl Peers {
    fn peer_mut(&mut self, id: &str) -> Option<&mut Peer> {
        self.list.iter_mut().find(|p| p.id == id)
    }

    /// Slave: the master connected (with its label), or went away (`None`).
    fn master_seen(&mut self, label: Option<&str>) {
        let up = label.is_some();
        if let Some(p) = self.peer_mut("master") {
            p.up = up;
            if let Some(label) = label {
                p.label = label.to_string();
            }
            return;
        }
        self.list.push(Peer {
            id: "master".into(),
            url: String::new(),
            label: label.unwrap_or("master").to_string(),
            up,
            rtt_ms: None,
            revision: 0,
            last_error: String::new(),
        });
    }

    /// A pong to this side's own ping: the round trip on this host's clock.
    fn pong(&mut self, id: &str, t_ms: u64, now: u64) {
        if let Some(p) = self.peer_mut(id) {
            p.rtt_ms = Some(now.saturating_sub(t_ms) as f64);
        }
    }

    /// Master: a socket to slave `id` is open. It is up once the slave says hello.
    fn connected(&mut self, id: &str, url: &str) {
        if let Some(p) = self.peer_mut(id) {
            p.url = url.to_string();
            p.up = false;
            return;
        }
        self.list.push(Peer {
            id: id.to_string(),
            url: url.to_string(),
            label: String::new(),
            up: false,
            rtt_ms: None,
            revision: 0,
            last_error: String::new(),
        });
    }

    /// Master: the link to slave `id` ended. What was not sent is dropped
    /// (at most once).
    fn ended(&mut self, id: &str) {
        if let Some(p) = self.peer_mut(id) {
            p.up = false;
        }
        let dropped = self.pending.remove(id).map_or(0, |c| c.len())
            + usize::from(self.pending_push.remove(id).is_some());
        if dropped > 0 {
            tracing::warn!(
                peer = id,
                dropped,
                "remote link: commands not sent before the link ended were dropped"
            );
        }
    }

    /// Master: one message from slave `id`; returns the reply to send, if any.
    fn on_slave_msg(&mut self, id: &str, msg: Msg, now: u64) -> Option<Msg> {
        match msg {
            Msg::Hello { label, .. } => {
                if let Some(p) = self.peer_mut(id) {
                    p.label = label;
                    p.up = true;
                    p.last_error.clear();
                }
            }
            Msg::State {
                revision,
                levels,
                desk,
            } => {
                if !levels.is_empty() {
                    self.levels.insert(id.to_string(), levels);
                }
                if let Some(desk) = desk {
                    self.desks.insert(id.to_string(), desk);
                }
                if let Some(p) = self.peer_mut(id) {
                    p.revision = revision;
                    p.up = true;
                }
            }
            Msg::Ack { seq, ok, error } => {
                if !ok {
                    tracing::warn!(peer = id, seq, "remote link: the slave refused: {error}");
                }
                if let Some(p) = self.peer_mut(id) {
                    p.last_error = if ok { String::new() } else { error };
                }
            }
            // The slave's own round trip; the master measures with its own pings.
            Msg::Ping { t_ms } => return Some(Msg::Pong { t_ms }),
            Msg::Pong { t_ms } => self.pong(id, t_ms, now),
            Msg::Meters { peaks } => {
                self.peaks.insert(id.to_string(), peaks);
            }
            Msg::Command { .. } | Msg::Push { .. } => {}
        }
        None
    }
}

type MasterSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Master: keep a connection to one slave and remember its desk.
pub async fn master(app: App, url: String) {
    let id = peer_id(&app.config.link_peers, &url);
    loop {
        if let Err(e) = master_once(&app, &id, &url).await {
            tracing::warn!(peer = %url, "remote link: {e}");
        }
        app.link.lock().await.ended(&id);
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

/// Returns only when the link has ended.
async fn master_once(app: &App, id: &str, url: &str) -> Result<(), String> {
    let config = tungstenite::protocol::WebSocketConfig::default()
        .max_message_size(Some(MAX_MESSAGE))
        .max_frame_size(Some(MAX_MESSAGE));
    let connect = tokio_tungstenite::connect_async_with_config(url, Some(config), false);
    let (mut socket, _) = tokio::time::timeout(CONNECT_WITHIN, connect)
        .await
        .map_err(|_| format!("no connection within {} s", CONNECT_WITHIN.as_secs()))?
        .map_err(|e| e.to_string())?;
    app.link.lock().await.connected(id, url);
    let hello = Msg::Hello {
        role: "master".into(),
        token: app.config.link_token.clone(),
        label: app.config.host_id.clone(),
    };
    send(&mut socket, ws_text(&hello)).await?;
    let mut flush = tokio::time::interval(Duration::from_millis(50));
    let mut ping = tokio::time::interval(PING_EVERY);
    let mut heard = Instant::now();
    let mut seq = 0;
    let mut logged = false;
    loop {
        tokio::select! {
            msg = next_within(&mut socket, heard) => {
                match msg? {
                    Some(Ok(tungstenite::Message::Text(text))) => {
                        heard = Instant::now();
                        let reply = match parse(&text) {
                            Ok(msg) => app.link.lock().await.on_slave_msg(id, msg, now_ms()),
                            Err(unknown) => {
                                note_unknown(&mut logged, &unknown.kind);
                                None
                            }
                        };
                        if let Some(reply) = reply {
                            send(&mut socket, ws_text(&reply)).await?;
                        }
                    }
                    Some(Ok(tungstenite::Message::Close(_))) | None => return Err("the slave closed the link".into()),
                    Some(Ok(_)) => heard = Instant::now(),
                    Some(Err(e)) => return Err(e.to_string()),
                }
            }
            _ = flush.tick() => flush_master(app, id, &mut socket, &mut seq).await?,
            _ = ping.tick() => send(&mut socket, ws_text(&Msg::Ping { t_ms: now_ms() })).await?,
        }
    }
}

/// Send the queued commands and push, numbered from 1 on each connection.
async fn flush_master(
    app: &App,
    id: &str,
    socket: &mut MasterSocket,
    seq: &mut u64,
) -> Result<(), String> {
    let (commands, push) = {
        let mut peers = app.link.lock().await;
        (
            peers.pending.remove(id).unwrap_or_default(),
            peers.pending_push.remove(id),
        )
    };
    for command in commands {
        *seq += 1;
        send(socket, ws_text(&Msg::Command { seq: *seq, command })).await?;
    }
    if let Some(push) = push {
        *seq += 1;
        let push = Msg::Push {
            seq: *seq,
            levels: push.levels,
            sends: push.sends,
        };
        send(socket, ws_text(&push)).await?;
    }
    Ok(())
}

async fn remotes(State(app): State<App>) -> Json<Value> {
    let peers = app.link.lock().await;
    let list: Vec<Value> = peers
        .list
        .iter()
        .map(|p| {
            json!({
                "id": p.id,
                "url": p.url,
                "label": p.label,
                "up": p.up,
                "rtt_ms": p.rtt_ms,
                "revision": p.revision,
                "last_error": p.last_error,
                "desk": peers.desks.get(&p.id),
                "peaks": peers.peaks.get(&p.id),
                "return_bus": peers.returns.get(&p.id),
            })
        })
        .collect();
    Json(json!({ "peers": list }))
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
    if !peers.list.iter().any(|p| p.id == id && p.up) {
        return Err(axum::http::StatusCode::CONFLICT);
    }
    let Some(levels) = peers.levels.get(&id).cloned() else {
        return Err(axum::http::StatusCode::CONFLICT);
    };
    let sends = peers
        .desks
        .get(&id)
        .map(|d| d.sends.clone())
        .unwrap_or_default();
    peers.pending_push.insert(id, PushBody { levels, sends });
    Ok(Json(json!({"queued": true})))
}

#[derive(Deserialize)]
struct ReturnBody {
    of: String,
    #[serde(default)]
    without: Vec<String>,
    return_strip: String,
    voices: Vec<String>,
}

/// Build the gallery mix-minus, publish it, and ask the far side to put it in
/// each local in-ear.
async fn gallery_return(
    State(app): State<App>,
    Path(id): Path<String>,
    bytes: axum::body::Bytes,
) -> Response {
    if app.config.link_role != LinkRole::Master {
        return StatusCode::NOT_FOUND.into_response();
    }
    let body: ReturnBody = match serde_json::from_slice(&bytes) {
        Ok(b) => b,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": e.to_string()})),
            )
                .into_response()
        }
    };
    {
        let peers = app.link.lock().await;
        if !peers.list.iter().any(|p| p.id == id && p.up) {
            return (
                StatusCode::CONFLICT,
                Json(json!({"error": "far side is not linked"})),
            )
                .into_response();
        }
    }
    let _serial = app.changes.lock().await;
    let (bus, staged) = {
        let mut control = app.control();
        let mut next = control.mix.clone();
        let bus = match ensure_return(&mut next, &id, &body.of, &body.without) {
            Ok(bus) => bus,
            Err(e) => return (StatusCode::BAD_REQUEST, Json(json!({"error": e}))).into_response(),
        };
        let staged = control.stage(next, FADE_SAMPLES);
        (bus, staged)
    };
    if let Err(e) = finish(&app, staged).await {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": e}))).into_response();
    }
    {
        let mut peers = app.link.lock().await;
        peers.returns.insert(id.clone(), bus.clone());
        peers.pending.entry(id).or_default().push(Command::Ears {
            return_strip: body.return_strip,
            voices: body.voices,
        });
    }
    (
        StatusCode::CREATED,
        Json(json!({"bus": bus, "queued": true})),
    )
        .into_response()
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
                db: Db(-12.0),
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
        push_levels(&mut other, &levels, &[]);
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

    #[test]
    fn in_ears_mix_the_other_voices_locally_and_the_gallery_sends_a_mix_minus() {
        let mut far = Mix::default();
        for id in ["presenter", "guest-a", "guest-b", "gallery"] {
            let mut s = strip(id);
            s.name = id.into();
            far.strips.push(s);
        }
        let ears = Command::Ears {
            return_strip: "gallery".into(),
            voices: vec!["presenter".into(), "guest-a".into(), "guest-b".into()],
        };
        apply_command(&mut far, &ears).unwrap();
        apply_command(&mut far, &ears).unwrap();
        for (voice, others) in [
            ("presenter", ["guest-a", "guest-b"]),
            ("guest-a", ["presenter", "guest-b"]),
            ("guest-b", ["presenter", "guest-a"]),
        ] {
            let bus = far.bus(&format!("iem-{voice}")).unwrap();
            assert!(!bus.compensate, "an in-ear is not delay-compensated");
            assert!(bus.name.starts_with("IEM "));
            let from: Vec<&str> = far
                .sends
                .iter()
                .filter(|s| s.to == format!("iem-{voice}"))
                .map(|s| s.from.as_str())
                .collect();
            assert_eq!(from.len(), 3, "{voice}");
            assert!(!from.contains(&voice));
            assert!(from.contains(&"gallery"));
            for other in others {
                assert!(from.contains(&other), "{voice} hears {other}");
            }
        }
        apply_command(
            &mut far,
            &Command::Send {
                from: "guest-a".into(),
                to: "iem-presenter".into(),
                db: Db(-6.0),
                mute: true,
            },
        )
        .unwrap();
        let send = far.send("guest-a", "iem-presenter").unwrap();
        assert_eq!(send.gain_db, Db(-6.0));
        assert!(send.mute);
        // Building the ears again leaves the level the operator set.
        apply_command(&mut far, &ears).unwrap();
        assert_eq!(
            far.send("guest-a", "iem-presenter").unwrap().gain_db,
            Db(-6.0)
        );
        let desk = desk_of(&far);
        assert!(desk.buses.iter().any(|b| b.id == "iem-presenter" && b.ears));
        assert!(desk
            .sends
            .iter()
            .any(|s| s.from == "gallery" && s.to == "iem-guest-a" && !s.pre));

        let mut home = Mix::default();
        home.buses.push(Bus::new("main", Format::Stereo));
        home.strips.push(strip("far-mix"));
        home.strips.push(strip("host"));
        let id = ensure_return(&mut home, "s1", "main", &["far-mix".into()]).unwrap();
        assert_eq!(id, "ret-s1");
        assert_eq!(
            home.bus(&id).unwrap().minus.as_ref().unwrap().without,
            vec!["far-mix".to_string()]
        );
        assert!(home.output(&id).is_some());
        let again =
            ensure_return(&mut home, "s1", "main", &["far-mix".into(), "host".into()]).unwrap();
        assert_eq!(again, id);
        assert_eq!(home.buses.iter().filter(|b| b.id == id).count(), 1);
        assert_eq!(home.outputs.len(), 1);
        assert_eq!(
            home.bus(&id).unwrap().minus.as_ref().unwrap().without.len(),
            2
        );
        assert!(ensure_return(&mut home, "s1", "missing", &[]).is_err());
        assert!(apply_command(
            &mut far,
            &Command::Ears {
                return_strip: "presenter".into(),
                voices: vec!["presenter".into()],
            },
        )
        .is_err());
        let parsed: Command =
            serde_json::from_str(r#"{"op":"fader","signal":"vox","db":"-inf"}"#).unwrap();
        assert_eq!(
            parsed,
            Command::Fader {
                signal: "vox".into(),
                db: Db::MINUS_INF,
            }
        );
    }
    #[test]
    fn one_master_holds_the_link_and_takeover_deny_spares_its_changes() {
        let mut t = Takeover::new(true);
        assert!(!t.refuses(), "start-up loads the show before any master");
        assert!(t.claim(1));
        assert!(t.up() && t.seen && !t.refuses());
        assert!(!t.claim(2), "a second master is refused");
        // A connection that never held the link does not bring it down.
        t.release(2);
        assert!(t.up());
        t.release(1);
        assert!(!t.up());
        assert!(t.refuses(), "the master is gone: local changes are refused");
        t.remote_write = true;
        assert!(!t.refuses(), "the master's own change passes");
        t.remote_write = false;
        assert!(t.claim(3), "the master is back");
        // The old connection ending late (a stale release) leaves the new one up.
        t.release(1);
        assert!(t.up() && !t.refuses());
        let mut allow = Takeover::new(false);
        assert!(allow.claim(1));
        allow.release(1);
        assert!(!allow.refuses());
    }

    #[test]
    fn a_message_of_another_version_is_named_not_fatal() {
        assert!(matches!(
            parse(r#"{"type":"ping","t_ms":5,"extra":1}"#),
            Ok(Msg::Ping { t_ms: 5 })
        ));
        assert_eq!(
            parse(r#"{"type":"tally","on":true}"#).unwrap_err(),
            Unknown {
                kind: "tally".into(),
                seq: 0
            }
        );
        assert_eq!(
            parse(r#"{"type":"command","seq":7,"command":{"op":"pan","signal":"vox"}}"#)
                .unwrap_err(),
            Unknown {
                kind: "command".into(),
                seq: 7
            }
        );
        assert_eq!(parse("not json").unwrap_err().kind, "");
        // A push from an older master has no seq.
        assert!(matches!(
            parse(r#"{"type":"push","levels":[]}"#),
            Ok(Msg::Push { seq: 0, .. })
        ));
        let mut logged = false;
        note_unknown(&mut logged, "tally");
        assert!(logged);
    }

    #[test]
    fn the_master_measures_the_round_trip_with_its_own_clock() {
        let mut peers = Peers::default();
        peers.connected("s1", "ws://far/api/v1/link");
        assert!(!peers.list[0].up, "up once the slave says hello");
        let hello = Msg::Hello {
            role: "slave".into(),
            token: String::new(),
            label: "far".into(),
        };
        assert!(peers.on_slave_msg("s1", hello, 0).is_none());
        assert!(peers.list[0].up);
        assert_eq!(peers.list[0].label, "far");
        // The slave's ping carries the slave's clock: answered, not measured.
        let reply = peers.on_slave_msg("s1", Msg::Ping { t_ms: 9_000_000 }, 1_000);
        assert!(matches!(reply, Some(Msg::Pong { t_ms: 9_000_000 })));
        assert_eq!(peers.list[0].rtt_ms, None);
        // The pong to the master's ping sent at 1 000 ms, back at 1 012 ms.
        assert!(peers
            .on_slave_msg("s1", Msg::Pong { t_ms: 1_000 }, 1_012)
            .is_none());
        assert_eq!(peers.list[0].rtt_ms, Some(12.0));
        let refused = Msg::Ack {
            seq: 3,
            ok: false,
            error: "no signal x".into(),
        };
        peers.on_slave_msg("s1", refused, 0);
        assert_eq!(peers.list[0].last_error, "no signal x");
        assert!(now_ms() <= now_ms(), "monotonic");
    }

    #[test]
    fn link_state_follows_the_connection_and_unsent_commands_are_dropped() {
        let mut slave = Peers::default();
        slave.master_seen(Some("gallery"));
        assert!(metrics(&slave).contains(&"remote_link_up{peer=\"master\"} 1".to_string()));
        slave.pong("master", 100, 104);
        assert_eq!(slave.list[0].rtt_ms, Some(4.0));
        slave.master_seen(None);
        assert!(metrics(&slave).contains(&"remote_link_up{peer=\"master\"} 0".to_string()));
        assert_eq!(slave.list.len(), 1);
        assert_eq!(slave.list[0].label, "gallery");

        let mut master = Peers::default();
        master.connected("s2", "ws://b/api/v1/link");
        master.on_slave_msg(
            "s2",
            Msg::State {
                revision: 4,
                levels: Vec::new(),
                desk: None,
            },
            0,
        );
        assert!(master.list[0].up);
        master
            .pending
            .entry("s2".into())
            .or_default()
            .push(Command::Recall {
                snapshot: "a".into(),
            });
        master.pending_push.insert(
            "s2".into(),
            PushBody {
                levels: Vec::new(),
                sends: Vec::new(),
            },
        );
        master.ended("s2");
        assert!(!master.list[0].up);
        assert!(master.pending.is_empty() && master.pending_push.is_empty());
        assert!(metrics(&master).contains(&"remote_link_up{peer=\"s2\"} 0".to_string()));
    }

    #[test]
    fn a_slave_keeps_its_place_in_the_peer_list_as_its_id() {
        let urls: Vec<String> = ["ws://a/l", "ws://b/l", "ws://c/l"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(peer_id(&urls, "ws://c/l"), "s3");
        assert_eq!(peer_id(&urls, "ws://a/l"), "s1");
    }

    #[test]
    fn a_taken_return_id_is_refused_instead_of_leaking_a_bus_per_rebuild() {
        let mut home = Mix::default();
        home.buses.push(Bus::new("main", Format::Stereo));
        home.strips.push(strip("ret-s1"));
        for _ in 0..3 {
            assert!(ensure_return(&mut home, "s1", "main", &[]).is_err());
        }
        assert_eq!(home.buses.len(), 1);
        assert!(home.outputs.is_empty());
        assert_eq!(
            ensure_return(&mut home, "s2", "main", &[]).unwrap(),
            "ret-s2"
        );
        assert_eq!(
            ensure_return(&mut home, "s2", "main", &[]).unwrap(),
            "ret-s2"
        );
        assert_eq!(home.buses.len(), 2);
        assert_eq!(home.outputs.len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_silent_peer_is_gone_after_the_deadline() {
        let mut silent = futures_util::stream::pending::<u8>();
        let heard = Instant::now();
        let err = next_within(&mut silent, heard).await.unwrap_err();
        assert!(err.contains("nothing heard"), "{err}");
        assert!(Instant::now() - heard >= DEAD_AFTER);
        let mut talking = futures_util::stream::iter([1u8]);
        assert_eq!(next_within(&mut talking, Instant::now()).await, Ok(Some(1)));

        /// A peer whose socket takes nothing more.
        struct Stuck;
        impl futures_util::Sink<u8> for Stuck {
            type Error = String;
            fn poll_ready(
                self: std::pin::Pin<&mut Self>,
                _: &mut std::task::Context<'_>,
            ) -> std::task::Poll<Result<(), String>> {
                std::task::Poll::Pending
            }
            fn start_send(self: std::pin::Pin<&mut Self>, _: u8) -> Result<(), String> {
                Ok(())
            }
            fn poll_flush(
                self: std::pin::Pin<&mut Self>,
                _: &mut std::task::Context<'_>,
            ) -> std::task::Poll<Result<(), String>> {
                std::task::Poll::Pending
            }
            fn poll_close(
                self: std::pin::Pin<&mut Self>,
                _: &mut std::task::Context<'_>,
            ) -> std::task::Poll<Result<(), String>> {
                std::task::Poll::Pending
            }
        }
        assert!(send(&mut Stuck, 1).await.is_err());
    }

    #[tokio::test]
    async fn wss_is_built_in() {
        // A listener that closes at once: with TLS built in, the handshake
        // fails on the socket instead of "TLS support not compiled in".
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                drop(socket);
            }
        });
        let url = format!("wss://127.0.0.1:{port}/api/v1/link");
        let err = tokio_tungstenite::connect_async(url).await.unwrap_err();
        assert!(
            !matches!(
                err,
                tungstenite::Error::Url(tungstenite::error::UrlError::TlsFeatureNotEnabled)
            ),
            "{err}"
        );
    }
}
