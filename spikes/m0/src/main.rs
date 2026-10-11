//! Milestone 0 spike (SPECIFICATION.md §19): an `mxl-nmos` node with one MXL audio
//! receiver and one MXL audio sender on real flows, and MXL `audio/float32` read
//! and write on the TAI block clock with the block jitter measured.
//!
//! The sender writes a 2-channel test tone into the output domain, one block per
//! period. An IS-05 activation of the receiver hands a reader for the given flow
//! to the block thread, which reads at `t − read offset` and checks every sample
//! (the tone depends only on the sample index). On SIGTERM or after
//! `SPIKE_DURATION_S` it deregisters and prints a JSON report.
//!
//! Not the product: settings named `SPIKE_*` exist only here.

mod rt;
mod stats;
mod tone;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use mxl_nmos::{
    ids, media_types, ActiveConnection, Channel, Device, Flow, Format, MxlDomain, Node,
    NodeImplementation, Rational, Receiver, ReceiverCaps, Role, Sender, Settings, Source,
    StagedConnection,
};
use serde_json::json;
use tokio::sync::mpsc as tmpsc;
use uuid::Uuid;

const CHANNELS: usize = 2;

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

fn env_num<T: std::str::FromStr>(name: &str, default: T) -> T {
    match env(name) {
        None => default,
        Some(raw) => raw.trim().parse().unwrap_or_else(|_| {
            eprintln!("{name} is not a valid number: {raw}");
            std::process::exit(78);
        }),
    }
}

struct Paths {
    scan: PathBuf,
    output: PathBuf,
}

/// Every child of the scan path with a `domain_def.json`, mirrors included.
fn scan_domains(paths: &Paths) -> Vec<MxlDomain> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&paths.scan)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect()
        })
        .unwrap_or_default();
    if !dirs.contains(&paths.output) {
        dirs.push(paths.output.clone());
    }
    let mut found: Vec<MxlDomain> = dirs
        .into_iter()
        .filter_map(|dir| {
            let text = std::fs::read_to_string(dir.join("domain_def.json")).ok()?;
            let def: serde_json::Value = serde_json::from_str(&text).ok()?;
            Some(MxlDomain {
                id: def["id"].as_str()?.parse().ok()?,
                label: def["label"].as_str().unwrap_or_default().to_string(),
                path: dir,
            })
        })
        .collect();
    found.sort_by_key(|d| d.id);
    found.dedup_by_key(|d| d.id);
    found
}

/// Create the output domain once (G2): `domain_def.json` with id, label,
/// description and tags; `options.json` with the history duration.
fn ensure_output_domain(dir: &Path, id: Uuid, label: &str, history_ns: u64) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let def = dir.join("domain_def.json");
    if let Ok(text) = std::fs::read_to_string(&def) {
        let existing: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        if existing["id"].as_str() != Some(id.to_string().as_str()) {
            return Err(format!(
                "{} has id {}, not {id}; not overwriting",
                def.display(),
                existing["id"]
            ));
        }
    } else {
        let body = json!({"id": id.to_string(), "label": label, "description": "mxl-audio-mixer M0 spike output domain", "tags": {}});
        std::fs::write(&def, serde_json::to_string_pretty(&body).unwrap())
            .map_err(|e| e.to_string())?;
    }
    let options = dir.join("options.json");
    if !options.exists() {
        std::fs::write(
            &options,
            json!({"urn:x-mxl:option:history_duration/v1.0": history_ns}).to_string(),
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

enum ReaderRequest {
    Open { domain: PathBuf, flow: Uuid },
    Close,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let mut settings = Settings::new("mxl-audio-mixer M0 spike", 3312);
    settings.description = "mxl-audio-mixer milestone 0 spike".into();
    settings.service_name_prefix = "mxl-audio-mixer-m0".into();
    if let Err(err) = settings.apply_env() {
        eprintln!("invalid setting: {err}");
        std::process::exit(78);
    }
    let seed = settings.seed.clone().unwrap_or_else(|| "m0-spike".into());
    settings.seed = Some(seed.clone());
    let scan = PathBuf::from(env("MXL_DOMAIN_SCAN_PATH").unwrap_or_else(|| "/Volumes/mxl".into()));
    let paths = Paths {
        output: env("MXL_OUTPUT_DOMAIN_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| scan.join("m0-spike")),
        scan,
    };
    let domain_id: Uuid = match env("MXL_OUTPUT_DOMAIN_ID") {
        Some(raw) => raw.parse().unwrap_or_else(|_| {
            eprintln!("MXL_OUTPUT_DOMAIN_ID is not a UUID: {raw}");
            std::process::exit(78);
        }),
        None => ids::derive_id(&seed, "mxl-output-domain"),
    };
    let history_ns: u64 = env_num("MXL_HISTORY_DURATION_NS", 200_000_000);
    let block: u64 = env_num("SPIKE_BLOCK_SAMPLES", 96);
    let config = rt::Config {
        block,
        read_offset: env_num("SPIKE_READ_OFFSET_SAMPLES", 96),
        read_timeout: Duration::from_micros(env_num("SPIKE_READ_TIMEOUT_US", 500)),
        verify: env("SPIKE_VERIFY").map(|v| v != "false").unwrap_or(true),
        rt_priority: env("MIXER_RT_PRIORITY")
            .map(|v| v.parse().unwrap_or(0))
            .filter(|p| *p > 0),
        cpu: env("SPIKE_CPU").and_then(|v| v.parse().ok()),
    };
    let duration_s: u64 = env_num("SPIKE_DURATION_S", 0);
    if let Err(err) = ensure_output_domain(&paths.output, domain_id, &settings.label, history_ns) {
        eprintln!("MXL output domain: {err}");
        std::process::exit(78);
    }

    // MXL: the library is loaded at run time (built with -DMXL_ENABLE_FABRICS_OFI=OFF).
    let library = env("MXL_LIBRARY").unwrap_or_else(|| "/opt/mxl/lib/libmxl.so".into());
    let api = mxl::load_api(&library).unwrap_or_else(|err| {
        eprintln!("cannot load {library}: {err}");
        std::process::exit(78);
    });
    let output = mxl::MxlInstance::new(api.clone(), &paths.output.display().to_string(), "")
        .expect("MXL instance");
    let tai_index = (rt_tai_ns() as u128 * 48_000 / 1_000_000_000) as u64;
    tracing::info!(
        "MXL current index {} (CLOCK_TAI-derived {}), TAI-UTC offset {} s",
        output.get_current_index(&rt::RATE),
        tai_index,
        mxl_nmos::clock::tai_utc_offset_secs()
    );

    let node_id = ids::node_id_from_seed(&seed);
    let mxl_flow_id = ids::derive_id(&seed, "pgm/mxl-flow");
    let source_id = Uuid::new_v5(&node_id, b"source:pgm");
    let device_id = Uuid::new_v5(&node_id, b"device:mixer");
    let flow_def = json!({
        "id": mxl_flow_id.to_string(),
        "source_id": source_id.to_string(),
        "device_id": device_id.to_string(),
        "label": "PGM",
        "description": "M0 spike test tone",
        "format": "urn:x-nmos:format:audio",
        "tags": {"urn:x-nmos:tag:grouphint/v1.0": ["PGM:Audio"]},
        "version": "0:0",
        "parents": [],
        "media_type": "audio/float32",
        "sample_rate": {"numerator": 48000, "denominator": 1},
        "channel_count": CHANNELS,
        "bit_depth": 32
    });
    // No writer options: at 218ddaa `MxlInstance::create_flow_writer` frees the
    // options string before libmxl reads it (fixed upstream in aed4cac6, not on
    // release/v1.1), so libmxl's default commit/sync hint (10 ms) applies.
    let (writer, info, created) = output
        .create_flow_writer(&flow_def.to_string(), None)
        .expect("MXL flow writer");
    let continuous = info.continuous().map(|c| c.bufferLength).unwrap_or(0);
    tracing::info!("MXL flow {mxl_flow_id} (created: {created}, buffer {continuous} samples)");
    let writer = writer.to_samples_writer().expect("samples writer");

    // NMOS: one device, one audio sender (Source + Flow + Sender), one audio receiver.
    let (reader_tx, mut reader_rx) = tmpsc::unbounded_channel::<ReaderRequest>();
    let domain_paths = Paths {
        scan: paths.scan.clone(),
        output: paths.output.clone(),
    };
    let receiver_state = Arc::new(Mutex::new("unrouted".to_string()));
    let handler_tx = reader_tx.clone();
    let implementation = NodeImplementation::new(move |activation| {
        let tx = handler_tx.clone();
        Box::pin(async move {
            tracing::info!(
                "activation {:?} {} master_enable={} domain={:?} flow={:?}",
                activation.role,
                activation.key,
                activation.master_enable,
                activation.mxl_domain_id,
                activation.mxl_flow_id
            );
            if activation.role == Role::Receiver {
                let request = match (
                    activation.master_enable,
                    activation.domain_path,
                    activation.mxl_flow_id,
                ) {
                    (true, Some(domain), Some(flow)) => ReaderRequest::Open { domain, flow },
                    _ => ReaderRequest::Close,
                };
                let _ = tx.send(request);
            }
            Ok(())
        })
    })
    .domains(move || scan_domains(&domain_paths));
    let node = match Node::new(settings, implementation) {
        Ok(node) => node,
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(78);
        }
    };
    build_resources(&node, domain_id, mxl_flow_id).expect("NMOS resources");

    let (cmd_tx, cmd_rx) = mpsc::channel::<rt::Command>();
    let shared = rt::Shared::new();
    let (done_tx, done_rx) = mpsc::channel();
    let rt_shared = shared.clone();
    let rt_config = rt::Config { ..config };
    let block_thread = std::thread::Builder::new()
        .name("block".into())
        .spawn(move || rt::run(rt_config, writer, CHANNELS, cmd_rx, rt_shared, done_tx))
        .expect("block thread");

    // Readers are opened here, never in the block thread. A flow that does not
    // exist yet keeps the receiver "waiting" until it appears.
    let state = receiver_state.clone();
    let api_for_readers = api.clone();
    tokio::spawn(async move {
        let mut instances: HashMap<PathBuf, mxl::MxlInstance> = HashMap::new();
        let mut want: Option<(PathBuf, Uuid)> = None;
        loop {
            let request = if want.is_some() {
                tokio::time::timeout(Duration::from_millis(200), reader_rx.recv())
                    .await
                    .ok()
                    .flatten()
            } else {
                reader_rx.recv().await
            };
            match request {
                Some(ReaderRequest::Open { domain, flow }) => want = Some((domain, flow)),
                Some(ReaderRequest::Close) => {
                    want = None;
                    let _ = cmd_tx.send(rt::Command::NoInput);
                    *state.lock().unwrap() = "unrouted".into();
                    continue;
                }
                None if want.is_none() => break,
                None => {}
            }
            let Some((domain, flow)) = want.clone() else {
                continue;
            };
            let instance = instances.entry(domain.clone()).or_insert_with(|| {
                mxl::MxlInstance::new(api_for_readers.clone(), &domain.display().to_string(), "")
                    .expect("MXL instance")
            });
            match instance
                .create_flow_reader(&flow.to_string())
                .and_then(|r| r.to_samples_reader())
            {
                Ok(reader) => {
                    let _ = cmd_tx.send(rt::Command::Input(Box::new(reader)));
                    tracing::info!("receiver running: flow {flow} in {}", domain.display());
                    *state.lock().unwrap() = "running".into();
                    want = None;
                }
                Err(err) => {
                    let mut s = state.lock().unwrap();
                    if *s != "waiting" {
                        tracing::info!("receiver waiting for flow {flow}: {err}");
                    }
                    *s = "waiting".into();
                }
            }
        }
    });

    let port = node.settings().port;
    let server = match node.bind(([0, 0, 0, 0], port).into()).await {
        Ok(server) => server,
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(75);
        }
    };
    node.start_registration();
    let serving = tokio::spawn(server.run());
    tracing::info!("NMOS node {} on {}:{port}", node.node_id(), node.host());

    let started = Instant::now();
    let mut term =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("SIGTERM");
    let sleep = async {
        if duration_s > 0 {
            tokio::time::sleep(Duration::from_secs(duration_s)).await
        } else {
            std::future::pending::<()>().await
        }
    };
    tokio::select! {
        _ = term.recv() => {}
        _ = tokio::signal::ctrl_c() => {}
        _ = sleep => {}
    }
    shared.stop.store(true, Ordering::SeqCst);
    let _ = done_rx.recv_timeout(Duration::from_secs(2));
    let _ = block_thread.join();
    node.shutdown(Duration::from_secs(5)).await;
    let _ = serving.await;

    let report = shared.report.lock().unwrap();
    let summary = json!({
        "seconds": started.elapsed().as_secs_f64(),
        "block_samples": block,
        "read_offset_samples": config.read_offset,
        "read_timeout_us": config.read_timeout.as_micros() as u64,
        "scheduling": *shared.sched.lock().unwrap(),
        "tai_utc_offset_s": mxl_nmos::clock::tai_utc_offset_secs(),
        "mxl_buffer_samples": continuous,
        "blocks": report.blocks,
        "overruns": report.overruns,
        "written_samples": report.written_samples,
        "write_errors": report.write_errors,
        "read_samples": report.read_samples,
        "mismatched_samples": report.mismatched_samples,
        "too_early_blocks": report.too_early_blocks,
        "too_late_blocks": report.too_late_blocks,
        "timeouts": report.timeouts,
        "other_read_errors": report.other_read_errors,
        "receiver_state": *receiver_state.lock().unwrap(),
        "wake_jitter": shared.wake_jitter.lock().unwrap().summary(),
        "wake_jitter_over_500us": shared.wake_jitter.lock().unwrap().above_us(500),
        "wake_jitter_over_1ms": shared.wake_jitter.lock().unwrap().above_us(1000),
        "block_work": shared.block_work.lock().unwrap().summary(),
        "input_arrival_lag": shared.arrival_lag.lock().unwrap().summary(),
    });
    let text = serde_json::to_string_pretty(&summary).unwrap();
    println!("{text}");
    if let Some(path) = env("SPIKE_REPORT") {
        let _ = std::fs::write(path, text);
    }
}

fn rt_tai_ns() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: valid timespec.
    unsafe { libc::clock_gettime(libc::CLOCK_TAI, &mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

fn build_resources(node: &Node, domain_id: Uuid, mxl_flow_id: Uuid) -> Result<(), mxl_nmos::Error> {
    node.insert_device(
        Device::new("mixer", "Audio Mixer").description("mxl-audio-mixer M0 spike"),
    )?;
    let mut source = Source::audio("pgm", "mixer", vec![Channel::new("L"), Channel::new("R")]);
    source.label = "PGM".into();
    mxl_nmos::add_group_hint(&mut source.tags, "PGM", "Audio");
    node.insert_source(source)?;
    let mut flow = Flow::audio(
        "pgm",
        "pgm",
        "mixer",
        media_types::AUDIO_FLOAT32,
        Rational::new(48_000, 1),
        Some(32),
    );
    flow.label = "PGM".into();
    mxl_nmos::add_group_hint(&mut flow.tags, "PGM", "Audio");
    node.insert_flow(flow)?;
    let mut sender = Sender::mxl("pgm", "pgm", "mixer");
    sender.label = "PGM".into();
    sender.mxl_domain_id = Some(domain_id);
    sender.mxl_flow_id = Some(mxl_flow_id);
    mxl_nmos::add_group_hint(&mut sender.tags, "PGM", "Audio");
    node.insert_sender(sender)?;
    let own = ActiveConnection {
        master_enable: true,
        peer_id: None,
        mxl_domain_id: Some(domain_id),
        mxl_flow_id: Some(mxl_flow_id),
    };
    node.set_staged(
        Role::Sender,
        "pgm",
        StagedConnection {
            master_enable: true,
            peer_id: None,
            mxl_domain_id: Some(domain_id).into(),
            mxl_flow_id: Some(mxl_flow_id).into(),
        },
    )?;
    node.set_active(Role::Sender, "pgm", own)?;

    let mut receiver = Receiver::mxl(
        "in-1",
        "mixer",
        Format::Audio,
        vec![media_types::AUDIO_FLOAT32.into()],
    );
    receiver.label = "Input 1".into();
    receiver.caps = ReceiverCaps {
        media_types: vec![media_types::AUDIO_FLOAT32.into()],
        constraint_sets: vec![json!({
            "urn:x-nmos:cap:format:channel_count": {"minimum": 1, "maximum": 64},
            "urn:x-nmos:cap:format:sample_rate": {"enum": [{"numerator": 48000, "denominator": 1}]},
            "urn:x-nmos:cap:format:sample_depth": {"enum": [32]}
        })],
        version: None,
    };
    mxl_nmos::add_group_hint(&mut receiver.tags, "Input 1", "Audio");
    node.insert_receiver(receiver)?;
    Ok(())
}
