//! Files under `MIXER_STATE_DIR` (G1): `inputs.json` (per-input settings) and
//! `connections.json` (the active IS-05 connections, G9).

use std::path::Path;

/// Write `bytes` to `path` through a temporary file, so a crash never leaves a
/// half-written file.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("rename to {}: {e}", path.display()))
}

/// When to write a file that changes often (`mix.json`, D168): at most once per
/// `delay`, `delay` after the first change since the last write. A crash loses at
/// most that much.
#[derive(Debug, Default)]
pub struct Coalesce {
    due: Option<std::time::Instant>,
}

impl Coalesce {
    /// Something changed at `now`.
    pub fn changed(&mut self, now: std::time::Instant, delay: std::time::Duration) {
        self.due.get_or_insert(now + delay);
    }

    /// A write is waiting.
    pub fn pending(&self) -> bool {
        self.due.is_some()
    }

    /// It is time to write; the caller writes now.
    pub fn take_due(&mut self, now: std::time::Instant) -> bool {
        if self.due.is_some_and(|t| now >= t) {
            self.due = None;
            true
        } else {
            false
        }
    }

    /// Write now whatever is waiting (shutdown, an operator's explicit action).
    pub fn take(&mut self) -> bool {
        self.due.take().is_some()
    }
}

/// `MIXER_STATE_DIR` (G1): create it and prove it is writable, so a read-only
/// mount fails the start instead of losing every change later.
pub fn check_state_dir(dir: &Path) -> Result<(), String> {
    let probe = dir.join(".write-probe");
    std::fs::create_dir_all(dir)
        .and_then(|()| std::fs::write(&probe, b"ok"))
        .and_then(|()| std::fs::remove_file(&probe))
        .map_err(|e| format!("MIXER_STATE_DIR {} is not writable: {e}", dir.display()))
}

pub fn load_connections(path: &Path) -> Vec<mxl_nmos::SavedConnection> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn save_connections(
    path: &Path,
    connections: &[mxl_nmos::SavedConnection],
) -> Result<(), String> {
    write_atomic(
        path,
        &serde_json::to_vec_pretty(connections).unwrap_or_default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_state_dir_must_be_writable() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("config");
        check_state_dir(&dir).unwrap();
        assert!(dir.is_dir() && !dir.join(".write-probe").exists());
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        // root ignores the mode bits; the check only means something for others.
        let root_user = unsafe { libc::geteuid() } == 0;
        assert_eq!(check_state_dir(&dir).is_err(), !root_user);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        let file = root.path().join("file");
        std::fs::write(&file, "").unwrap();
        assert!(check_state_dir(&file).is_err());
    }

    #[test]
    fn connections_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/connections.json");
        assert!(load_connections(&path).is_empty());
        let saved = vec![mxl_nmos::SavedConnection {
            role: mxl_nmos::Role::Receiver,
            key: "in-1".into(),
            active: mxl_nmos::ActiveConnection {
                master_enable: true,
                ..Default::default()
            },
        }];
        save_connections(&path, &saved).unwrap();
        assert_eq!(load_connections(&path), saved);
    }

    #[test]
    fn writes_are_coalesced() {
        use std::time::{Duration, Instant};
        let delay = Duration::from_millis(500);
        let start = Instant::now();
        let mut c = Coalesce::default();
        let mut writes = Vec::new();
        // A fader moved every 10 ms for 2 s.
        for tick in 0..200u64 {
            let now = start + Duration::from_millis(tick * 10);
            c.changed(now, delay);
            if c.take_due(now) {
                writes.push(tick);
            }
        }
        assert_eq!(writes, vec![50, 101, 152], "every 500 ms, not 100 a second");
        // What changed after the last write (from 1530 ms) is written 500 ms
        // after its first change, though the fader stopped at 1990 ms.
        assert!(c.pending());
        assert!(!c.take_due(start + Duration::from_millis(2029)));
        assert!(c.take_due(start + Duration::from_millis(2030)));
        assert!(!c.pending());
        // Shutdown writes at once; nothing waiting, nothing to write.
        c.changed(start, delay);
        assert!(c.take());
        assert!(!c.take());
    }
}
