//! MXL domains (G2): the domains visible below the scan path (mirrors
//! included) and the mixer's own output domain.

use std::path::{Path, PathBuf};

use uuid::Uuid;

/// A domain with a readable `domain_def.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Domain {
    pub id: Uuid,
    pub label: String,
    pub path: PathBuf,
}

fn read_def(dir: &Path) -> Option<Domain> {
    let text = std::fs::read_to_string(dir.join("domain_def.json")).ok()?;
    let def: serde_json::Value = serde_json::from_str(&text).ok()?;
    Some(Domain {
        id: def["id"].as_str()?.trim().parse().ok()?,
        label: def["label"].as_str().unwrap_or_default().to_string(),
        path: dir.to_path_buf(),
    })
}

/// Every child of `scan_path` with a `domain_def.json`, plus `own`. A domain id
/// seen twice keeps its first directory (sorted by path).
pub fn scan(scan_path: &Path, own: &Path) -> Vec<Domain> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(scan_path)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect()
        })
        .unwrap_or_default();
    if !dirs.iter().any(|d| d == own) {
        dirs.push(own.to_path_buf());
    }
    dirs.sort();
    let mut found: Vec<Domain> = Vec::new();
    for dir in dirs {
        if let Some(domain) = read_def(&dir) {
            if !found.iter().any(|d| d.id == domain.id) {
                found.push(domain);
            }
        }
    }
    found
}

/// Create the own output domain once. An existing `domain_def.json` with another
/// id is an error and is not overwritten; `options.json` is written only when missing.
pub fn ensure_output_domain(
    dir: &Path,
    id: Uuid,
    label: &str,
    history_ns: u64,
) -> Result<bool, String> {
    if dir.parent().is_none() || dir == Path::new("/Volumes/mxl") {
        return Err(format!("{} cannot be an output domain", dir.display()));
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let def = dir.join("domain_def.json");
    let created = match read_def(dir) {
        Some(existing) if existing.id == id => false,
        Some(existing) => {
            return Err(format!(
                "{} already has id {}, not {id}; not overwriting it",
                def.display(),
                existing.id
            ))
        }
        None if def.exists() => {
            return Err(format!(
                "{} is not a valid domain definition",
                def.display()
            ))
        }
        None => {
            let body = serde_json::json!({
                "id": id.to_string(),
                "label": label,
                "description": format!("{label} MXL audio mixer output domain"),
                "tags": {}
            });
            std::fs::write(
                &def,
                serde_json::to_string_pretty(&body).unwrap_or_default(),
            )
            .map_err(|e| format!("write {}: {e}", def.display()))?;
            true
        }
    };
    let options = dir.join("options.json");
    if !options.exists() {
        let body = serde_json::json!({"urn:x-mxl:option:history_duration/v1.0": history_ns});
        std::fs::write(&options, body.to_string())
            .map_err(|e| format!("write {}: {e}", options.display()))?;
    }
    Ok(created)
}

/// Remove the own output domain (G8 with `MXL_CLEANUP_ON_EXIT`). Refuses the scan
/// root itself.
pub fn remove_output_domain(dir: &Path, scan_path: &Path) -> Result<(), String> {
    if dir == scan_path || dir.parent().is_none() {
        return Err(format!("refusing to remove {}", dir.display()));
    }
    match std::fs::remove_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("remove {}: {e}", dir.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_domain_is_created_once_and_never_overwritten() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("mixer-1");
        let id = Uuid::new_v4();
        assert!(ensure_output_domain(&dir, id, "Mixer", 200_000_000).unwrap());
        let def: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("domain_def.json")).unwrap())
                .unwrap();
        for key in ["id", "label", "description", "tags"] {
            assert!(def.get(key).is_some(), "{key}");
        }
        assert!(!ensure_output_domain(&dir, id, "Mixer", 1).unwrap());
        assert!(std::fs::read_to_string(dir.join("options.json"))
            .unwrap()
            .contains("200000000"));
        assert!(ensure_output_domain(&dir, Uuid::new_v4(), "Mixer", 1).is_err());
        assert!(std::fs::read_to_string(dir.join("domain_def.json"))
            .unwrap()
            .contains(&id.to_string()));
    }

    #[test]
    fn scan_includes_mirrors_and_own() {
        let root = tempfile::tempdir().unwrap();
        let mirror = root.path().join("mirror-studio");
        std::fs::create_dir_all(&mirror).unwrap();
        let mid = Uuid::new_v4();
        std::fs::write(
            mirror.join("domain_def.json"),
            format!(r#"{{"id":"{mid}","label":"m","description":"d","tags":{{}}}}"#),
        )
        .unwrap();
        std::fs::create_dir_all(root.path().join("junk")).unwrap();
        let own = root.path().join("mixer-x");
        let oid = Uuid::new_v4();
        ensure_output_domain(&own, oid, "Mixer", 1).unwrap();
        let found = scan(root.path(), &own);
        assert_eq!(found.len(), 2);
        assert!(found.iter().any(|d| d.id == mid) && found.iter().any(|d| d.id == oid));
        remove_output_domain(&own, root.path()).unwrap();
        assert!(!own.exists());
        assert!(remove_output_domain(root.path(), root.path()).is_err());
    }
}
