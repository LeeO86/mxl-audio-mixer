//! The built-in MediaMTX (Appendix B.1, G6): without `PREVIEW_PUBLISH_URL` the
//! mixer runs its own as a supervised child process on its own port block (RTSP
//! on 127.0.0.1, WHEP/WHIP, ICE UDP; API off), restarted with backoff, as
//! mxl-webrtc-monitor does. Also the reader counts of a shared MediaMTX's API.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;

use crate::config::Config;

/// The MediaMTX configuration of the built-in instance: only the mixer's paths
/// (`<PREVIEW_PATH_PREFIX>/…`); publishing only from this host (the encoders over
/// RTSP on 127.0.0.1), reading (WHEP) from anywhere.
pub fn render(config: &Config) -> String {
    let paths = config.preview_path_prefix.replace('.', "\\.");
    let hosts = config
        .nmos_host_address
        .map(|a| format!("[\"{a}\"]"))
        .unwrap_or_else(|| "[]".into());
    format!(
        "logLevel: warn\n\
         api: false\n\
         metrics: false\n\
         pprof: false\n\
         playback: false\n\
         rtsp: true\n\
         rtspAddress: 127.0.0.1:{rtsp}\n\
         rtspTransports: [tcp]\n\
         rtmp: false\n\
         hls: false\n\
         srt: false\n\
         moq: false\n\
         webrtc: true\n\
         webrtcAddress: :{whep}\n\
         webrtcAllowOrigins: [\"*\"]\n\
         webrtcLocalUDPAddress: :{ice}\n\
         webrtcLocalTCPAddress: \"\"\n\
         webrtcAdditionalHosts: {hosts}\n\
         webrtcICEServers2: []\n\
         authMethod: internal\n\
         authInternalUsers:\n\
         - user: any\n  ips: [\"127.0.0.1\", \"::1\"]\n  permissions:\n  - action: publish\n  - action: read\n\
         - user: any\n  permissions:\n  - action: read\n\
         pathDefaults:\n  source: publisher\n\
         paths:\n  '~^{paths}/':\n",
        rtsp = config.mediamtx_rtsp_port,
        whep = config.mediamtx_whep_port,
        ice = config.mediamtx_ice_udp_port,
    )
}

/// The child's state, for the status, the UI and the metrics.
#[derive(Default)]
pub struct Child {
    pub running: AtomicBool,
    pub pid: AtomicU32,
    pub restarts: AtomicU64,
    pub last_exit: std::sync::Mutex<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChildStatus {
    pub running: bool,
    pub pid: Option<u32>,
    pub restarts: u64,
    pub last_exit: String,
}

impl Child {
    pub fn status(&self) -> ChildStatus {
        let pid = self.pid.load(Ordering::Relaxed);
        ChildStatus {
            running: self.running.load(Ordering::Relaxed),
            pid: (pid != 0).then_some(pid),
            restarts: self.restarts.load(Ordering::Relaxed),
            last_exit: self.last_exit.lock().map(|s| s.clone()).unwrap_or_default(),
        }
    }
}

/// Check that the built-in MediaMTX's ports are free (G6: a bind failure exits 75).
pub fn check_ports(config: &Config) -> Result<(), String> {
    let tcp = [
        ("127.0.0.1", config.mediamtx_rtsp_port, "MEDIAMTX_RTSP_PORT"),
        ("0.0.0.0", config.mediamtx_whep_port, "MEDIAMTX_WHEP_PORT"),
    ];
    for (host, port, name) in tcp {
        std::net::TcpListener::bind((host, port))
            .map_err(|e| format!("cannot bind {name} {port}: {e}"))?;
    }
    std::net::UdpSocket::bind(("0.0.0.0", config.mediamtx_ice_udp_port)).map_err(|e| {
        format!(
            "cannot bind MEDIAMTX_ICE_UDP_PORT {}: {e}",
            config.mediamtx_ice_udp_port
        )
    })?;
    Ok(())
}

fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    if std::fs::read_to_string(path).is_ok_and(|t| t == text) {
        return Ok(());
    }
    let tmp = path.with_extension("yml.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

/// Run the built-in MediaMTX until `stop`: restart it when it exits, after 1 s,
/// then 2, 4 … 30 s (back to 1 s after a minute of running).
pub async fn supervise(
    config: Arc<Config>,
    child: Arc<Child>,
    mut stop: tokio::sync::watch::Receiver<bool>,
) {
    let path: PathBuf = config.state_dir.join("mediamtx.yml");
    if let Err(e) = write_atomic(&path, &render(&config)) {
        tracing::error!("MediaMTX configuration {}: {e}", path.display());
    }
    let mut backoff = Duration::from_secs(1);
    loop {
        let started = std::time::Instant::now();
        let spawned = tokio::process::Command::new(&config.mediamtx_bin)
            .arg(&path)
            .current_dir(&config.state_dir)
            .kill_on_drop(true)
            .stdin(std::process::Stdio::null())
            .spawn();
        match spawned {
            Ok(mut process) => {
                child
                    .pid
                    .store(process.id().unwrap_or(0), Ordering::Relaxed);
                child.running.store(true, Ordering::Relaxed);
                tracing::info!(pid = process.id(), "MediaMTX started");
                let exit = tokio::select! {
                    r = process.wait() => r.map(|s| s.to_string()).unwrap_or_else(|e| e.to_string()),
                    _ = async { let _ = stop.wait_for(|s| *s).await; } => {
                        let _ = process.kill().await;
                        child.running.store(false, Ordering::Relaxed);
                        return;
                    }
                };
                child.running.store(false, Ordering::Relaxed);
                child.pid.store(0, Ordering::Relaxed);
                tracing::warn!("MediaMTX exited: {exit}");
                if let Ok(mut s) = child.last_exit.lock() {
                    *s = exit;
                }
            }
            Err(e) => {
                tracing::error!("cannot start MediaMTX ({}): {e}", config.mediamtx_bin);
                if let Ok(mut s) = child.last_exit.lock() {
                    *s = format!("cannot start {}: {e}", config.mediamtx_bin);
                }
            }
        }
        child.restarts.fetch_add(1, Ordering::Relaxed);
        if started.elapsed() > Duration::from_secs(60) {
            backoff = Duration::from_secs(1);
        }
        tokio::select! {
            _ = tokio::time::sleep(backoff) => {}
            _ = async { let _ = stop.wait_for(|s| *s).await; } => return,
        }
        backoff = (backoff * 2).min(Duration::from_secs(30));
    }
}

/// Reader counts of the streams under `prefix` from a MediaMTX API
/// (`GET /v3/paths/list`): stream id → readers.
pub fn readers_from_list(body: &serde_json::Value, prefix: &str) -> BTreeMap<String, u32> {
    let head = format!("{prefix}/");
    body["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let name = item["name"].as_str()?;
            let id = name.strip_prefix(&head)?;
            let n = item["readers"].as_array().map_or(0, |r| r.len() as u32);
            Some((id.to_string(), n))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_and_readers() {
        let env = [("NMOS_HOST_ADDRESS", "192.0.2.7")]
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let c = crate::config::load(&env).unwrap();
        let yml = render(&c);
        assert!(yml.contains("rtspAddress: 127.0.0.1:8954\n"));
        assert!(yml.contains("webrtcAddress: :8599\n"));
        assert!(yml.contains("webrtcLocalUDPAddress: :8199\n"));
        assert!(yml.contains("webrtcAdditionalHosts: [\"192.0.2.7\"]\n"));
        assert!(yml.contains("api: false\n") && yml.contains("hls: false\n"));
        assert!(yml.contains("moq: false\n"));
        assert!(yml.contains("paths:\n  '~^mxl-audio-mixer/':\n"));
        let list = serde_json::json!({"items": [
            {"name": "mxl-audio-mixer/main", "readers": [{"type": "webRTCSession"}, {"type": "webRTCSession"}]},
            {"name": "mxl-audio-mixer/phones", "readers": []},
            {"name": "other/main", "readers": [{}]}
        ]});
        let r = readers_from_list(&list, "mxl-audio-mixer");
        assert_eq!(
            r,
            BTreeMap::from([("main".into(), 2), ("phones".into(), 0)])
        );
    }
}
