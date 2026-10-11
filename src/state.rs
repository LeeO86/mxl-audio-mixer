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
}
