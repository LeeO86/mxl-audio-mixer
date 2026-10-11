//! Process start-up and shutdown (G1–G9). Exit codes: 0, 75 (a port cannot be
//! bound), 78 (invalid configuration), 143 (SIGTERM).

use std::net::SocketAddr;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mxl_audio_mixer::config::{self, Config};
use mxl_audio_mixer::control::{self, Control, Staged};
use mxl_audio_mixer::engine::{Engine, EngineConfig};
use mxl_audio_mixer::inputs::{self, Inputs};
use mxl_audio_mixer::meters::METER_TICK;
use mxl_audio_mixer::mxlio::{self, domain};
use mxl_audio_mixer::nmos::{self, RouteRequest};
use mxl_audio_mixer::ops::{self, App};
use mxl_audio_mixer::outputs::Outputs;
use mxl_audio_mixer::{state, timing};

const EX_CONFIG: u8 = 78;
const EX_UNAVAILABLE: u8 = 75;
const EX_TERM: u8 = 143;

fn fail(code: u8, message: impl std::fmt::Display) -> ExitCode {
    tracing::error!("{message}");
    eprintln!("mxl-audio-mixer: {message}");
    ExitCode::from(code)
}

fn main() -> ExitCode {
    let config = match config::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("mxl-audio-mixer: invalid configuration: {e}");
            return ExitCode::from(EX_CONFIG);
        }
    };
    tracing_subscriber::fmt()
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
        .with_writer(std::io::stderr)
        .with_env_filter(tracing_subscriber::EnvFilter::new(format!(
            "{},hyper=warn,reqwest=warn",
            config.log_level
        )))
        .init();
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(timing::worker_threads())
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => return fail(EX_UNAVAILABLE, format!("tokio runtime: {e}")),
    };
    runtime.block_on(run(config))
}

async fn run(config: Config) -> ExitCode {
    tracing::info!(
        "mxl-audio-mixer {} starting, {} inputs, {}-sample blocks",
        ops::VERSION,
        config.input_flows,
        config.block_samples
    );
    let (tai_offset_s, timing_trusted) = timing::check_tai_offset();
    if !timing_trusted {
        tracing::error!(
            "timing untrusted: CLOCK_TAI - CLOCK_REALTIME is {tai_offset_s} s, not {} s; every MXL time is off. Set the kernel TAI offset.",
            timing::TAI_UTC_OFFSET_S
        );
    }

    if let Err(e) = std::fs::create_dir_all(&config.state_dir) {
        return fail(
            EX_CONFIG,
            format!("MIXER_STATE_DIR {}: {e}", config.state_dir.display()),
        );
    }
    match domain::ensure_output_domain(
        &config.output_domain_dir,
        config.output_domain_id,
        &config.nmos_label,
        config.history_duration_ns,
    ) {
        Ok(true) => tracing::info!(
            "created output domain {} ({})",
            config.output_domain_dir.display(),
            config.output_domain_id
        ),
        Ok(false) => tracing::info!(
            "output domain {} ({})",
            config.output_domain_dir.display(),
            config.output_domain_id
        ),
        Err(e) => return fail(EX_CONFIG, format!("MXL output domain: {e}")),
    }
    let api = match mxlio::load() {
        Ok(api) => api,
        Err(e) => return fail(EX_CONFIG, e),
    };

    let mut engine = match Engine::start(EngineConfig {
        inputs: config.input_flows,
        block: config.block_samples,
        read_offset: config.read_offset_samples,
        rt_priority: config.rt_priority,
        cpu: timing::dsp_cpu(),
    }) {
        Ok(e) => e,
        Err(e) => return fail(EX_UNAVAILABLE, format!("DSP thread: {e}")),
    };
    let inputs_path = config.state_dir.join("inputs.json");
    let connections_path = config.state_dir.join("connections.json");
    let inputs = Inputs::new(
        api.clone(),
        &mut engine,
        &config,
        inputs::load_configs(&inputs_path),
    );
    let configs = inputs.configs();

    let (route_tx, mut route_rx) = tokio::sync::mpsc::unbounded_channel::<RouteRequest>();
    let node = match nmos::build(&config, &configs, route_tx, connections_path.clone()) {
        Ok(node) => node,
        Err(e) => return fail(EX_CONFIG, format!("NMOS: {e}")),
    };
    let outputs = match Outputs::new(
        api,
        &config.output_domain_dir,
        config.output_domain_id,
        &config.nmos_seed,
    ) {
        Ok(o) => o,
        Err(e) => return fail(EX_CONFIG, format!("MXL output domain: {e}")),
    };
    let mut control = Control::new(engine, inputs, outputs, node.clone(), &config);
    // The mix of the last run, with its published outputs, before the IS-05
    // connections (of their senders) are restored.
    let mix_path = config.state_dir.join("mix.json");
    let saved_mix = control::load_mix(&mix_path);
    let staged = match control.stage(saved_mix, 0) {
        Ok(s) => s,
        Err(e) => {
            let bad = mix_path.with_extension("json.bad");
            tracing::error!(
                "the saved mix is not valid ({e}); moved to {} and starting with an empty mix",
                bad.display()
            );
            let _ = std::fs::rename(&mix_path, &bad);
            control
                .stage(Default::default(), 0)
                .unwrap_or(Staged::Swap { wait: false })
        }
    };
    if staged != Staged::Done {
        match control.swap() {
            Ok(errors) => {
                for e in errors {
                    tracing::error!("{e}");
                }
            }
            Err(e) => tracing::error!("saving the mix: {e}"),
        }
    }
    let control = Arc::new(Mutex::new(control));

    let ticking = control.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_millis(100));
        let mut meter_tick = tokio::time::interval(METER_TICK);
        let lock = || ticking.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            tokio::select! {
                request = route_rx.recv() => match request {
                    Some(r) => {
                        let mut c = lock();
                        let c = &mut *c;
                        c.inputs.route(&mut c.engine, r.index, r.route);
                    }
                    None => break,
                },
                _ = tick.tick() => lock().tick(),
                _ = meter_tick.tick() => lock().meter_tick(),
            }
        }
    });

    // G9: the connections of the last run, re-applied through the callback.
    let saved = state::load_connections(&connections_path);
    if !saved.is_empty() {
        let failed = node.restore(saved).await;
        for (_, key, reason) in failed {
            tracing::warn!("saved connection of {key} not restored: {reason}");
        }
    }

    let nmos_addr: SocketAddr = ([0, 0, 0, 0], config.nmos_port).into();
    let nmos_server = match node.bind(nmos_addr).await {
        Ok(s) => s,
        Err(e) => return fail(EX_UNAVAILABLE, e),
    };
    let web_addr: SocketAddr = ([0, 0, 0, 0], config.web_port).into();
    let web_listener = match tokio::net::TcpListener::bind(web_addr).await {
        Ok(l) => l,
        Err(e) => {
            return fail(
                EX_UNAVAILABLE,
                format!("cannot bind WEB_PORT {web_addr}: {e}"),
            )
        }
    };
    let config = Arc::new(config);
    let app = App {
        config: config.clone(),
        node: node.clone(),
        control: control.clone(),
        changes: Arc::new(tokio::sync::Mutex::new(())),
        tai_offset_s,
        timing_trusted,
        started: Instant::now(),
    };
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let mut web_stop = stop_rx.clone();
    let web = tokio::spawn(async move {
        axum::serve(web_listener, ops::router(app))
            .with_graceful_shutdown(async move {
                let _ = web_stop.wait_for(|s| *s).await;
            })
            .await
    });
    let nmos_task = tokio::spawn(nmos_server.run());
    node.start_registration();
    tracing::info!(
        "web on {web_addr}, NMOS node {} on {}:{}",
        node.node_id(),
        node.host(),
        config.nmos_port
    );

    let term = wait_for_signal().await;
    tracing::info!("shutting down");
    let deadline = config.shutdown_timeout;
    let sequence = async {
        // G8 1: fade the outputs, stop media and release the MXL readers and writers.
        tokio::task::block_in_place(|| control.lock().unwrap_or_else(|e| e.into_inner()).stop());
        // G8 2: deregister every NMOS resource, the Node last.
        node.shutdown(
            deadline
                .saturating_sub(Duration::from_secs(1))
                .max(Duration::from_secs(1)),
        )
        .await;
        // G8 3: remove the own output domain.
        if config.cleanup_on_exit {
            match domain::remove_output_domain(&config.output_domain_dir, &config.scan_path) {
                Ok(()) => tracing::info!(
                    "removed output domain {}",
                    config.output_domain_dir.display()
                ),
                Err(e) => tracing::error!("output domain cleanup failed: {e}"),
            }
        }
        let _ = stop_tx.send(true);
        let _ = web.await;
        let _ = nmos_task.await;
    };
    if tokio::time::timeout(deadline, sequence).await.is_err() {
        tracing::error!(
            "shutdown did not finish within SHUTDOWN_TIMEOUT_S ({} s)",
            deadline.as_secs()
        );
    }
    if term {
        ExitCode::from(EX_TERM)
    } else {
        ExitCode::SUCCESS
    }
}

/// `true` for SIGTERM, `false` for Ctrl-C.
async fn wait_for_signal() -> bool {
    let mut term = match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
        Ok(s) => s,
        Err(_) => {
            let _ = tokio::signal::ctrl_c().await;
            return false;
        }
    };
    tokio::select! {
        _ = term.recv() => true,
        _ = tokio::signal::ctrl_c() => false,
    }
}
