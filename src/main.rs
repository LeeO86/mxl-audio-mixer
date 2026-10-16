//! Process start-up and shutdown (G1–G9). Exit codes: 0, 75 (a port cannot be
//! bound, libmxl cannot be loaded), 78 (invalid configuration), 143 (SIGTERM).

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
use mxl_audio_mixer::{caps, state, timing};

const EX_CONFIG: u8 = 78;
const EX_UNAVAILABLE: u8 = 75;
const EX_TERM: u8 = 143;

fn fail(code: u8, message: impl std::fmt::Display) -> ExitCode {
    tracing::error!("{message}");
    eprintln!("mxl-audio-mixer: {message}");
    ExitCode::from(code)
}

/// Every second: the reader counts of a shared MediaMTX (with `MEDIAMTX_API_URL`),
/// then stop the streams nobody listens to and drop idle personal monitors.
async fn preview_task(app: App, mut stop: tokio::sync::watch::Receiver<bool>) {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .ok();
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            _ = tick.tick() => {}
            _ = async { let _ = stop.wait_for(|s| *s).await; } => return,
        }
        if let (Some(url), Some(client)) = (&app.config.mediamtx_api_url, &client) {
            let list = async {
                client
                    .get(format!("{url}/v3/paths/list"))
                    .send()
                    .await?
                    .json::<serde_json::Value>()
                    .await
            };
            if let Ok(v) = list.await {
                let readers = mxl_audio_mixer::preview::mediamtx::readers_from_list(
                    &v,
                    &app.config.preview_path_prefix,
                );
                app.control().streams.set_readers(readers);
            }
        }
        let changed = {
            let mut c = app.control();
            let personal = c.expire_personal(mxl_audio_mixer::preview::PERSONAL_IDLE);
            let mics = c.expire_browser_mics();
            let talk = c.expire_talk();
            c.expire_streams() || personal || mics || talk
        };
        if changed {
            let ramp = mxl_audio_mixer::engine::FADE_SAMPLES;
            if let Err(r) = mxl_audio_mixer::api::apply(&app, ramp, |_| Ok(())).await {
                tracing::warn!("preview: re-staging the plan failed ({})", r.status());
            }
        }
    }
}

/// G6: every port this start will listen on is free, checked before the output
/// domain is created, so a busy port (exit 75) leaves nothing behind.
fn check_ports(config: &Config) -> Result<(), String> {
    let mut tcp = vec![
        ("WEB_PORT", config.web_port),
        ("NMOS_PORT", config.nmos_port),
    ];
    if config.tsl_enable {
        tcp.push(("TSL_TCP_PORT", config.tsl_tcp_port));
        std::net::UdpSocket::bind(("0.0.0.0", config.tsl_udp_port))
            .map_err(|e| format!("cannot bind TSL_UDP_PORT {}: {e}", config.tsl_udp_port))?;
    }
    for (name, port) in tcp {
        std::net::TcpListener::bind(("0.0.0.0", port))
            .map_err(|e| format!("cannot bind {name} {port}: {e}"))?;
    }
    if config.preview_publish_url.is_none() {
        mxl_audio_mixer::preview::mediamtx::check_ports(config)?;
    }
    Ok(())
}

/// A failed start after the output domain was created: remove it again when this
/// start created it (never a domain that was there before).
fn abort(created: bool, config: &Config, code: u8, message: impl std::fmt::Display) -> ExitCode {
    if created {
        if let Err(e) = domain::remove_output_domain(&config.output_domain_dir, &config.scan_path) {
            tracing::error!("output domain cleanup failed: {e}");
        }
    }
    fail(code, message)
}

fn main() -> ExitCode {
    // Before any thread starts (threads inherit both): the permitted SYS_NICE of
    // the image's file capability into the effective set, and the open-files limit.
    let raised = caps::raise_permitted();
    let open_files = caps::raise_open_files();
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
    tracing::info!("effective capabilities: {}", caps::effective());
    if let Err(e) = raised {
        tracing::warn!("raising the permitted capabilities into the effective set failed: {e}");
    }
    match open_files {
        Ok(n) => tracing::info!("open files limit {n}"),
        Err(e) => tracing::warn!("open files limit not raised: {e}"),
    }
    let workers = timing::worker_threads();
    timing::reserve_dsp_core();
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(workers)
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => return fail(EX_UNAVAILABLE, format!("tokio runtime: {e}")),
    };
    runtime.block_on(run(config))
}

async fn run(mut config: Config) -> ExitCode {
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

    if let Err(e) = domain::check_scan_path(&config.scan_path) {
        return fail(EX_CONFIG, e);
    }
    if let Err(e) = state::check_state_dir(&config.state_dir) {
        return fail(EX_CONFIG, e);
    }
    let api = match mxlio::load() {
        Ok(api) => api,
        Err(e) => return fail(EX_UNAVAILABLE, e),
    };
    if let Err(e) = check_ports(&config) {
        return fail(EX_UNAVAILABLE, e);
    }
    // A failed ensure may leave a directory or a domain_def.json it made;
    // remove the domain only then.
    let existed = config.output_domain_dir.exists();
    let (created, foreign) = match domain::ensure_output_domain(
        &config.output_domain_dir,
        config.output_domain_id,
        &config.nmos_label,
        config.history_duration_ns,
    ) {
        Ok(d) => {
            tracing::info!(
                "{} output domain {} ({})",
                if d.created { "created" } else { "using" },
                config.output_domain_dir.display(),
                d.id
            );
            config.set_output_domain_id(d.id);
            (d.created, d.foreign)
        }
        Err(e) => {
            return abort(
                !existed || e.created,
                &config,
                EX_CONFIG,
                format!("MXL output domain: {e}"),
            )
        }
    };
    // G2: a domain with another id was there before; it is not ours to remove.
    let cleanup_on_exit = config.cleanup_on_exit && !foreign;
    if config.cleanup_on_exit && foreign {
        tracing::warn!(
            "MXL_CLEANUP_ON_EXIT: {} keeps its own id, so it is not removed on exit",
            config.output_domain_dir.display()
        );
    }

    let mut engine = match Engine::start(EngineConfig {
        inputs: config.input_flows,
        block: config.block_samples,
        read_offset: config.read_offset_samples,
        rt_priority: config.rt_priority,
        cpu: timing::dsp_cpu(),
    }) {
        Ok(e) => e,
        Err(e) => return abort(created, &config, EX_UNAVAILABLE, format!("DSP thread: {e}")),
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
        Err(e) => return abort(created, &config, EX_CONFIG, format!("NMOS: {e}")),
    };
    let outputs = match Outputs::new(
        api,
        &config.output_domain_dir,
        config.output_domain_id,
        &config.nmos_seed,
    ) {
        Ok(o) => o,
        Err(e) => {
            return abort(
                created,
                &config,
                EX_CONFIG,
                format!("MXL output domain: {e}"),
            )
        }
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
    if let Some(target) = config.tsl_out_target.clone() {
        let (tx, rx) = tokio::sync::watch::channel(Default::default());
        control.tsl_out = Some(tx);
        mxl_audio_mixer::tally::spawn_sender(target.clone(), config.tsl_out_screen, rx);
        tracing::info!(
            "TSL UMD 5.0 output to udp {target}, screen {}",
            config.tsl_out_screen
        );
    }
    let control = Arc::new(Mutex::new(control));
    let link_peers = Arc::new(tokio::sync::Mutex::new(
        mxl_audio_mixer::link::Peers::default(),
    ));

    let ticking = control.clone();
    let ticking_link = link_peers.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_millis(100));
        let mut meter_tick = tokio::time::interval(METER_TICK);
        let mut auto_tick = tokio::time::interval(mxl_audio_mixer::auto::TICK);
        auto_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
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
                _ = auto_tick.tick() => {
                    // The link's peers for the logic `link` block; a busy lock
                    // keeps the last state.
                    let up = ticking_link.try_lock().ok().map(|p| {
                        p.list.iter().filter(|p| p.up).map(|p| p.id.clone()).collect()
                    });
                    let mut c = lock();
                    if let Some(up) = up {
                        c.link_up = up;
                    }
                    c.auto_tick();
                    c.logic_tick();
                }
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
        Err(e) => return abort(created, &config, EX_UNAVAILABLE, e),
    };
    let web_addr: SocketAddr = ([0, 0, 0, 0], config.web_port).into();
    let web_listener = match tokio::net::TcpListener::bind(web_addr).await {
        Ok(l) => l,
        Err(e) => {
            return abort(
                created,
                &config,
                EX_UNAVAILABLE,
                format!("cannot bind WEB_PORT {web_addr}: {e}"),
            )
        }
    };
    if config.tsl_enable {
        let sockets = match mxl_audio_mixer::tally::bind(&config).await {
            Ok(s) => s,
            Err(e) => {
                return abort(
                    created,
                    &config,
                    EX_UNAVAILABLE,
                    format!(
                        "cannot bind TSL_UDP_PORT {} / TSL_TCP_PORT {}: {e}",
                        config.tsl_udp_port, config.tsl_tcp_port
                    ),
                )
            }
        };
        let tally_control = control.clone();
        let tally_config = config.clone();
        mxl_audio_mixer::tally::spawn(
            sockets,
            Arc::new(move |packet: &[u8]| {
                tally_control
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .tally_packet(&tally_config, packet)
            }),
        );
        tracing::info!(
            "TSL UMD 5.0 on udp/{} and tcp/{}",
            config.tsl_udp_port,
            config.tsl_tcp_port
        );
    }
    let own_mediamtx = config.preview_publish_url.is_none();
    let config = Arc::new(config);
    let mediamtx = Arc::new(mxl_audio_mixer::preview::mediamtx::Child::default());
    let app = App {
        config: config.clone(),
        node: node.clone(),
        control: control.clone(),
        changes: Arc::new(tokio::sync::Mutex::new(())),
        tai_offset_s,
        timing_trusted,
        started: Instant::now(),
        mediamtx: mediamtx.clone(),
        link: link_peers,
    };
    if config.link_role == mxl_audio_mixer::config::LinkRole::Master {
        for url in config.link_peers.clone() {
            let app = app.clone();
            tokio::spawn(async move { mxl_audio_mixer::link::master(app, url).await });
        }
    }
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let app_for_preview = app.clone();
    let mut web_stop = stop_rx.clone();
    let web = tokio::spawn(async move {
        axum::serve(web_listener, ops::router(app))
            .with_graceful_shutdown(async move {
                let _ = web_stop.wait_for(|s| *s).await;
            })
            .await
    });
    let nmos_task = tokio::spawn(nmos_server.run());
    let mediamtx_task = own_mediamtx.then(|| {
        tokio::spawn(mxl_audio_mixer::preview::mediamtx::supervise(
            config.clone(),
            mediamtx.clone(),
            stop_rx.clone(),
        ))
    });
    tokio::spawn(preview_task(app_for_preview, stop_rx.clone()));
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
        if cleanup_on_exit {
            match domain::remove_output_domain(&config.output_domain_dir, &config.scan_path) {
                Ok(()) => tracing::info!(
                    "removed output domain {}",
                    config.output_domain_dir.display()
                ),
                Err(e) => tracing::error!("output domain cleanup failed: {e}"),
            }
        }
        let _ = stop_tx.send(true);
        // The child is killed and reaped before the exit: its ports are free for the next start.
        if let Some(t) = mediamtx_task {
            let _ = t.await;
        }
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
