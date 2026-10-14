//! The web port (G7, G10): probes, metrics, status, settings, export/import and
//! the inputs API. The mix API is in [`crate::api`].

use std::fmt::Write as _;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::config::{Config, SETTINGS};
use crate::control::Control;
use crate::engine::BLOCK_BUCKETS_US;
use crate::inputs::{ExtraDelay, InputConfig};
use crate::mix::Mix;
use crate::timing;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// The MXL commit the mixer is built against (the platform pin).
pub const MXL_REVISION: &str = "218ddaa0a08c12ffe75fc475ae65aa3d9eef16d7";

#[derive(Clone)]
pub struct App {
    pub config: Arc<Config>,
    pub node: mxl_nmos::Node,
    pub control: Arc<Mutex<Control>>,
    /// Serialises mix changes, which may wait for a fade between stage and swap.
    pub changes: Arc<tokio::sync::Mutex<()>>,
    pub tai_offset_s: i64,
    pub timing_trusted: bool,
    pub started: std::time::Instant,
    /// The built-in MediaMTX child (unused with a shared one).
    pub mediamtx: Arc<crate::preview::mediamtx::Child>,
}

impl App {
    pub fn control(&self) -> std::sync::MutexGuard<'_, Control> {
        self.control.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn ready(&self) -> (bool, Value) {
        let engine = self
            .control()
            .engine
            .counters
            .running
            .load(Ordering::Relaxed);
        let required = self.node.registration_required();
        let registered = self.node.is_registered();
        let ready = engine && (!required || registered);
        (
            ready,
            json!({
                "ready": ready,
                "engine_running": engine,
                "nmos_registration_required": required,
                "nmos_registered": registered,
                "timing_trusted": self.timing_trusted,
                "tai_utc_offset_s": self.tai_offset_s,
            }),
        )
    }
}

pub fn router(app: App) -> Router {
    Router::new()
        .route("/", get(ui))
        .route("/api/v1/events", get(crate::events::events))
        .route("/livez", get(|| async { "ok\n" }))
        .route("/readyz", get(readyz))
        .route("/metrics", get(metrics))
        .route("/api/v1/status", get(status))
        .route("/api/v1/config", get(config))
        .route("/api/v1/config/export", get(export))
        .route("/api/v1/config/import", post(import))
        .route("/api/v1/inputs", get(inputs))
        .route("/api/v1/inputs/{input}", get(input).patch(patch_input))
        .route("/api/v1/inputs/{input}/delay/confirm", post(confirm_delay))
        .merge(crate::api::routes())
        .merge(crate::show_api::routes())
        .merge(crate::room_api::routes())
        .merge(crate::preview_api::routes())
        .merge(crate::talkback_api::routes())
        .merge(crate::group_api::routes())
        .merge(crate::operations_api::routes())
        .merge(crate::auto_api::routes())
        .merge(crate::logic_api::routes())
        .merge(crate::widgets::routes())
        .layer(axum::middleware::from_fn_with_state(
            app.clone(),
            mix_revision,
        ))
        .with_state(app)
}

/// Every change answers with `x-mix-revision`, the mix revision after it: a client
/// keeps its own change over pushed mixes older than that (`web/src/store.js`).
async fn mix_revision(
    State(app): State<App>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let change = !matches!(
        *req.method(),
        axum::http::Method::GET | axum::http::Method::HEAD
    );
    let mut resp = next.run(req).await;
    if change {
        let revision = app.control().revision;
        resp.headers_mut()
            .insert("x-mix-revision", axum::http::HeaderValue::from(revision));
    }
    resp
}

/// The web UI: one self-contained page embedded at build time (`build.rs`).
pub const UI: &str = include_str!(env!("MIXER_UI_HTML"));

async fn ui() -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        UI,
    )
        .into_response()
}

pub fn error(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(json!({"error": message.into()}))).into_response()
}

async fn readyz(State(app): State<App>) -> Response {
    let (ready, details) = app.ready();
    let code = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (code, Json(details)).into_response()
}

async fn status(State(app): State<App>) -> Json<Value> {
    Json(status_json(&app))
}

/// `GET /api/v1/status`, also sent on the events WebSocket.
pub fn status_json(app: &App) -> Value {
    let (_, readiness) = app.ready();
    let control = app.control();
    let c = &control.engine.counters;
    let scheduling = control
        .engine
        .scheduling
        .lock()
        .map(|s| s.clone())
        .unwrap_or_default();
    json!({
        "application": "mxl-audio-mixer",
        "version": VERSION,
        "label": app.config.nmos_label,
        "mxl_revision": MXL_REVISION,
        "uptime_s": app.started.elapsed().as_secs(),
        "readiness": readiness,
        "nmos": {
            "node_id": app.node.node_id(),
            "host": app.node.host().to_string(),
            "port": app.config.nmos_port,
            "registry": app.node.registration_required(),
            "registered": app.node.is_registered(),
        },
        "mxl": {
            "scan_path": app.config.scan_path.display().to_string(),
            "output_domain_dir": app.config.output_domain_dir.display().to_string(),
            "output_domain_id": app.config.output_domain_id,
        },
        "dsp": {
            "block_samples": app.config.block_samples,
            "read_offset_ms": f64::from(app.config.read_offset_samples) / 48.0,
            "scheduling": scheduling,
            "blocks": c.blocks.load(Ordering::Relaxed),
            "deadline_misses": c.deadline_misses.load(Ordering::Relaxed),
            "load_ratio": c.load_ratio(),
            "plan_swaps": c.plan_swaps.load(Ordering::Relaxed),
        },
        "inputs": control.inputs.status(),
        "mix": {
            "strips": control.mix.strips.len(),
            "buses": control.mix.buses.len(),
            "sends": control.mix.sends.len(),
        },
        "outputs": control.output_status(),
        "tally": {
            "enabled": app.config.tsl_enable,
            "state": control.tally_status(),
        },
        "solo": control.solo(),
        "auto": {
            "vad_signals": control.auto.vads.len(),
            "speech": control.auto.vads.iter().filter(|(_, v)| v.state.speech_active).map(|(s, _)| s.clone()).collect::<Vec<_>>(),
            "automixes": control.mix.automixes.len(),
            "ducks": control.mix.ducks.len(),
            "autolevels": control.mix.autolevels.len(),
        },
        "talkback": {
            "talking": control.talking(),
            "listen_back": control.listen_back(),
            "browser_mics": control.talkers.slots().into_keys().collect::<Vec<_>>(),
        },
        "preview": {
            "mode": if app.config.preview_publish_url.is_none() { "own" } else { "shared" },
            "child_running": app.mediamtx.running.load(Ordering::Relaxed),
            "streams": control.streams.status(&control.stream_ids()).iter().filter(|s| s.state != "idle").count(),
        },
    })
}

async fn config(State(app): State<App>) -> Json<Value> {
    let settings: Vec<Value> = app
        .config
        .values
        .iter()
        .map(|(name, value, origin)| {
            let doc = SETTINGS.iter().find(|s| s.name == *name);
            json!({
                "key": name,
                "value": value,
                "source": origin,
                "default": doc.map(|d| d.default),
                "description": doc.map(|d| d.description),
            })
        })
        .collect();
    Json(json!({"settings": settings}))
}

/// The show file (G10). There are no secrets in it; settings are listed for the
/// record and are not applied on import (they come from the environment and file).
async fn export(State(app): State<App>) -> Json<Value> {
    let settings: serde_json::Map<String, Value> = app
        .config
        .values
        .iter()
        .map(|(name, value, _)| (name.to_string(), json!(value)))
        .collect();
    Json(json!({
        "format": "mxl-audio-mixer show",
        "version": 1,
        "application_version": VERSION,
        "settings": settings,
        "inputs": app.control().inputs.configs(),
        "mix": app.control().mix,
        "connections": app.node.connections(),
    }))
}

#[derive(Deserialize)]
struct ShowFile {
    version: u32,
    #[serde(default)]
    inputs: Vec<InputConfig>,
    #[serde(default)]
    mix: Option<Mix>,
    #[serde(default)]
    connections: Vec<mxl_nmos::SavedConnection>,
}

async fn import(State(app): State<App>, body: axum::body::Bytes) -> Response {
    let show: ShowFile = match serde_json::from_slice(&body) {
        Ok(show) => show,
        Err(e) => return error(StatusCode::BAD_REQUEST, format!("not a show file: {e}")),
    };
    if show.version != 1 {
        return error(
            StatusCode::BAD_REQUEST,
            format!("show file version {} is not supported", show.version),
        );
    }
    if let Some(mix) = &show.mix {
        let inputs = app.control().inputs.len();
        if let Err(e) = mix.validate(inputs, app.config.block_samples) {
            return error(StatusCode::BAD_REQUEST, format!("mix: {e}"));
        }
    }
    let result = {
        let mut control = app.control();
        let c = &mut *control;
        c.inputs.replace_configs(&mut c.engine, &show.inputs)
    };
    if let Err(e) = result {
        return error(StatusCode::BAD_REQUEST, e);
    }
    for input in &show.inputs {
        let _ = app.node.insert_receiver(crate::nmos::receiver(input));
    }
    let mut output_errors = Vec::new();
    if let Some(mix) = show.mix {
        {
            let control = app.control();
            if let Err(e) = control.history.record(&control.mix, None) {
                tracing::warn!("show history before import: {e}");
            }
        }
        match crate::api::apply(&app, crate::engine::FADE_SAMPLES, move |m| {
            *m = mix;
            Ok(())
        })
        .await
        {
            Ok(errors) => output_errors = errors,
            Err(r) => return *r,
        }
    }
    let failed = app.node.restore(show.connections).await;
    Json(json!({
        "imported": true,
        "rejected_connections": failed.iter().map(|(_, key, reason)| json!({"key": key, "reason": reason})).collect::<Vec<_>>(),
        "output_errors": output_errors,
    }))
    .into_response()
}

fn input_index(app: &App, input: usize) -> Result<usize, Box<Response>> {
    let count = app.control().inputs.len();
    if input == 0 || input > count {
        Err(Box::new(error(
            StatusCode::NOT_FOUND,
            format!("input {input} does not exist (1 to {count})"),
        )))
    } else {
        Ok(input - 1)
    }
}

async fn inputs(State(app): State<App>) -> Json<Value> {
    Json(json!(app.control().inputs.status()))
}

async fn input(State(app): State<App>, Path(input): Path<usize>) -> Response {
    match input_index(&app, input) {
        Ok(i) => Json(json!(app.control().inputs.status()[i])).into_response(),
        Err(r) => *r,
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InputPatch {
    label: Option<String>,
    extra_delay: Option<ExtraDelay>,
}

async fn patch_input(
    State(app): State<App>,
    Path(input): Path<usize>,
    body: axum::body::Bytes,
) -> Response {
    let i = match input_index(&app, input) {
        Ok(i) => i,
        Err(r) => return *r,
    };
    let patch: InputPatch = match serde_json::from_slice(&body) {
        Ok(p) => p,
        Err(e) => return error(StatusCode::BAD_REQUEST, e.to_string()),
    };
    let result = {
        let mut control = app.control();
        let c = &mut *control;
        c.inputs
            .update(&mut c.engine, i, patch.label, patch.extra_delay)
    };
    match result {
        Ok(config) => {
            let _ = app.node.insert_receiver(crate::nmos::receiver(&config));
            Json(json!(app.control().inputs.status()[i])).into_response()
        }
        Err(e) => error(StatusCode::BAD_REQUEST, e),
    }
}

async fn confirm_delay(State(app): State<App>, Path(input): Path<usize>) -> Response {
    let i = match input_index(&app, input) {
        Ok(i) => i,
        Err(r) => return *r,
    };
    let result = {
        let mut control = app.control();
        let c = &mut *control;
        c.inputs.confirm_delay(&mut c.engine, i)
    };
    match result {
        Ok(()) => Json(json!(app.control().inputs.status()[i])).into_response(),
        Err(e) => error(StatusCode::CONFLICT, e),
    }
}

/// Prometheus text, prefix `mxl_audio_mixer_` (G7, §15).
async fn metrics(State(app): State<App>) -> Response {
    let mut out = String::new();
    let p = "mxl_audio_mixer_";
    let mut metric = |name: &str, kind: &str, help: &str, lines: Vec<String>| {
        let _ = writeln!(out, "# HELP {p}{name} {help}\n# TYPE {p}{name} {kind}");
        for line in lines {
            let _ = writeln!(out, "{p}{name}{line}");
        }
    };
    let control = app.control();
    let inputs = &control.inputs;
    let c = inputs.counters();
    let period = app.config.block_period().as_secs_f64();
    let mut cumulative = 0u64;
    let mut lines = Vec::new();
    for (i, bound) in BLOCK_BUCKETS_US.iter().enumerate() {
        cumulative += c.block_time_buckets[i].load(Ordering::Relaxed);
        lines.push(format!(
            "_bucket{{le=\"{}\"}} {cumulative}",
            *bound as f64 / 1e6
        ));
    }
    cumulative += c.block_time_buckets[BLOCK_BUCKETS_US.len()].load(Ordering::Relaxed);
    lines.push(format!("_bucket{{le=\"+Inf\"}} {cumulative}"));
    lines.push(format!(
        "_sum {}",
        c.block_time_sum_ns.load(Ordering::Relaxed) as f64 / 1e9
    ));
    lines.push(format!("_count {cumulative}"));
    metric(
        "dsp_block_seconds",
        "histogram",
        "Time the DSP thread spends on one block.",
        lines,
    );
    metric(
        "dsp_deadline_misses_total",
        "counter",
        "Blocks woken later than one block period, plus blocks skipped after a late one.",
        vec![format!(" {}", c.deadline_misses.load(Ordering::Relaxed))],
    );
    metric(
        "dsp_load_ratio",
        "gauge",
        &format!("Block time / block period ({period} s), last block."),
        vec![format!(" {}", c.load_ratio())],
    );
    metric(
        "output_blocks_total",
        "counter",
        "Blocks written to MXL outputs (one per output and block).",
        vec![format!(" {}", c.output_blocks.load(Ordering::Relaxed))],
    );
    metric(
        "plan_swaps_total",
        "counter",
        "Execution plans swapped in by the DSP thread.",
        vec![format!(" {}", c.plan_swaps.load(Ordering::Relaxed))],
    );
    metric(
        "vad_active",
        "gauge",
        "Per analysed signal: 1 while its VAD detects speech.",
        control
            .auto
            .vads
            .iter()
            .map(|(s, v)| format!("{{signal=\"{s}\"}} {}", u8::from(v.state.speech_active)))
            .collect(),
    );
    metric(
        "meter_drops_total",
        "counter",
        "Metered blocks the meter thread did not get (its ring was full).",
        vec![format!(" {}", c.meter_drops.load(Ordering::Relaxed))],
    );
    metric(
        "vad_drops_total",
        "counter",
        "Tapped blocks the control engine did not get (the VAD ring was full).",
        vec![format!(" {}", c.vad_drops.load(Ordering::Relaxed))],
    );
    let streams = control.streams.status(&control.stream_ids());
    metric(
        "preview_state",
        "gauge",
        "Per stream: 0 idle, 1 connecting, 2 publishing.",
        streams
            .iter()
            .map(|s| {
                let v = match s.state {
                    "publishing" => 2,
                    "connecting" => 1,
                    _ => 0,
                };
                format!("{{path=\"{}\"}} {v}", s.path)
            })
            .collect(),
    );
    metric(
        "preview_readers",
        "gauge",
        "Per stream: readers on MediaMTX (with MEDIAMTX_API_URL).",
        streams
            .iter()
            .filter_map(|s| s.readers.map(|n| format!("{{path=\"{}\"}} {n}", s.path)))
            .collect(),
    );
    metric(
        "preview_child_restarts_total",
        "counter",
        "Restarts of the built-in MediaMTX.",
        vec![format!(
            " {}",
            app.mediamtx.restarts.load(Ordering::Relaxed)
        )],
    );
    metric(
        "tally_packets_total",
        "counter",
        "TSL UMD 5.0 packets received (UDP and TCP).",
        vec![format!(" {}", control.tally.packets)],
    );
    metric(
        "plan_compile_seconds",
        "gauge",
        "Duration of the last plan compile.",
        vec![format!(" {}", control.compile_seconds)],
    );
    metric(
        "signals",
        "gauge",
        "Signals of the mix (strips and buses).",
        vec![format!(" {}", control.signals())],
    );
    metric(
        "nodes",
        "gauge",
        "Steps of the running plan by type.",
        control
            .step_counts
            .iter()
            .map(|(kind, n)| format!("{{type=\"{kind}\"}} {n}"))
            .collect(),
    );
    metric(
        "output_commit_lag_ms",
        "gauge",
        "TAI time of an output's last commit minus the time of its last sample.",
        control
            .outputs
            .status(&control.engine, &app.node)
            .iter()
            .map(|o| format!("{{output=\"{}\"}} {}", o.signal, o.commit_lag_ms))
            .collect(),
    );
    // −∞ is not exported: a series without a value is absent.
    let finite = |db: crate::mix::model::Db| db.0.is_finite().then_some(db.0);
    let loudness = control.loudness.values(&control.mix.settings);
    let mut lines = Vec::new();
    for (signal, v) in &loudness {
        for (window, value) in [
            ("momentary", v.momentary_lufs),
            ("short-term", v.short_term_lufs),
            ("integrated", v.integrated_lufs),
        ] {
            if let Some(x) = finite(value) {
                lines.push(format!("{{signal=\"{signal}\",window=\"{window}\"}} {x}"));
            }
        }
    }
    metric(
        "loudness_lufs",
        "gauge",
        "Loudness (ITU-R BS.1770-4) of the measured signals by window.",
        lines,
    );
    let meters = control.meters.values();
    metric(
        "true_peak_dbtp",
        "gauge",
        "Largest true peak of a signal over its channels since the last meter update.",
        meters
            .iter()
            .filter_map(|(signal, m)| {
                let tp = m
                    .true_peak_dbtp
                    .iter()
                    .filter_map(|d| finite(*d))
                    .fold(f64::NEG_INFINITY, f64::max);
                tp.is_finite()
                    .then(|| format!("{{signal=\"{signal}\"}} {tp}"))
            })
            .collect(),
    );
    metric(
        "timing_trusted",
        "gauge",
        "1 when CLOCK_TAI - CLOCK_REALTIME is the TAI-UTC offset (37 s).",
        vec![format!(" {}", u8::from(app.timing_trusted))],
    );

    let status = inputs.status();
    let per = |f: &dyn Fn(&crate::inputs::InputStatus) -> String| -> Vec<String> {
        status
            .iter()
            .map(|s| format!("{{input=\"{}\"}} {}", s.input, f(s)))
            .collect()
    };
    metric(
        "input_late_samples_total",
        "counter",
        "Samples the source had not written by the read (too early, or the wait timed out).",
        per(&|s| s.late_samples.to_string()),
    );
    metric(
        "input_overrun_samples_total",
        "counter",
        "Samples the ring had already overwritten (too late): the reader fell behind.",
        per(&|s| s.overrun_samples.to_string()),
    );
    metric(
        "input_missing_samples_total",
        "counter",
        "Samples replaced by silence after the input had started.",
        per(&|s| s.missing_samples.to_string()),
    );
    metric(
        "input_lag_ms",
        "gauge",
        "Arrival lag at the last read: TAI now minus the flow's head.",
        per(&|s| s.lag_ms.to_string()),
    );
    let mut lag_lines = Vec::new();
    for s in &status {
        if let Some(lag) = s.arrival_lag_ms {
            for (q, v) in [("0.5", lag.p50), ("0.99", lag.p99), ("0.999", lag.p999)] {
                lag_lines.push(format!("{{input=\"{}\",quantile=\"{q}\"}} {v}", s.input));
            }
        }
    }
    metric(
        "input_arrival_lag_ms",
        "gauge",
        "Arrival lag over the sliding window, by quantile.",
        lag_lines,
    );
    metric(
        "input_effective_delay_ms",
        "gauge",
        "Read offset plus the input's extra delay.",
        per(&|s| s.effective_delay_ms.to_string()),
    );
    metric(
        "input_state",
        "gauge",
        "0 unrouted, 1 waiting, 2 running, 3 stalled.",
        per(&|s| (s.state as u8).to_string()),
    );
    metric(
        "input_reopens_total",
        "counter",
        "Times the input was opened again.",
        per(&|s| s.reopens.to_string()),
    );
    drop(control);
    metric(
        "process_cpu_seconds_total",
        "counter",
        "CPU time of the process.",
        vec![format!(" {}", timing::process_cpu_seconds())],
    );
    metric(
        "process_resident_memory_bytes",
        "gauge",
        "Resident memory of the process.",
        vec![format!(" {}", timing::process_resident_bytes())],
    );
    ([(header::CONTENT_TYPE, "text/plain; version=0.0.4")], out).into_response()
}
