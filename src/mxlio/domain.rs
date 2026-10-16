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
    match read_existing(dir) {
        Existing::Valid(d) => Some(d),
        _ => None,
    }
}

/// `domain_def.json` read once: missing, readable, or there but not a definition.
enum Existing {
    Missing,
    Valid(Domain),
    Invalid,
}

fn read_existing(dir: &Path) -> Existing {
    let text = match std::fs::read_to_string(dir.join("domain_def.json")) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Existing::Missing,
        Err(_) => return Existing::Invalid,
    };
    let parsed = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|def| {
            Some(Domain {
                id: def["id"].as_str()?.trim().parse().ok()?,
                label: def["label"].as_str().unwrap_or_default().to_string(),
                path: dir.to_path_buf(),
            })
        });
    parsed.map_or(Existing::Invalid, Existing::Valid)
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

/// The own output domain after [`ensure_output_domain`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputDomain {
    /// The id in force: the configured one, or the one an existing
    /// `domain_def.json` already has.
    pub id: Uuid,
    /// This call wrote `domain_def.json`.
    pub created: bool,
    /// `domain_def.json` had another id than the configured one: the domain is
    /// not this instance's to remove (`MXL_CLEANUP_ON_EXIT` leaves it).
    pub foreign: bool,
}

/// Create the own output domain once (G2). An existing `domain_def.json` is never
/// overwritten: with another id, that id is kept and a warning logged.
/// `options.json` is written only when missing. Both appear atomically and only
/// once, also when several processes create the same domain at the same time
/// (the test tone writers do).
pub fn ensure_output_domain(
    dir: &Path,
    id: Uuid,
    label: &str,
    history_ns: u64,
) -> Result<OutputDomain, String> {
    if dir.parent().is_none() {
        return Err(format!("{} cannot be an output domain", dir.display()));
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let def = dir.join("domain_def.json");
    // One read decides: a definition that appears between two checks (another
    // writer creating the domain) must not look invalid.
    let existing = match read_existing(dir) {
        Existing::Valid(existing) => Some(existing),
        Existing::Invalid => None,
        Existing::Missing => {
            let body = serde_json::json!({
                "id": id.to_string(),
                "label": label,
                "description": format!("{label} MXL audio mixer output domain"),
                "tags": {}
            });
            if write_new(&def, &serde_json::to_vec_pretty(&body).unwrap_or_default())? {
                return finish(
                    dir,
                    history_ns,
                    OutputDomain {
                        id,
                        created: true,
                        foreign: false,
                    },
                );
            }
            // Another process wrote it first.
            read_def(dir)
        }
    };
    let Some(existing) = existing else {
        return Err(format!(
            "{} is not a valid domain definition",
            def.display()
        ));
    };
    let foreign = existing.id != id;
    if foreign {
        tracing::warn!(
            "{} already has id {}, not {id}; keeping {} and not overwriting it",
            def.display(),
            existing.id,
            existing.id
        );
    }
    finish(
        dir,
        history_ns,
        OutputDomain {
            id: existing.id,
            created: false,
            foreign,
        },
    )
}

fn finish(dir: &Path, history_ns: u64, domain: OutputDomain) -> Result<OutputDomain, String> {
    let options = dir.join("options.json");
    if !options.exists() {
        let body = serde_json::json!({"urn:x-mxl:option:history_duration/v1.0": history_ns});
        write_new(&options, body.to_string().as_bytes())?;
    }
    Ok(domain)
}

/// Write `path` only if it does not exist: a private temporary file, then a hard
/// link (which never replaces a file). `false` when another writer was first.
fn write_new(path: &Path, bytes: &[u8]) -> Result<bool, String> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let tmp = path.with_file_name(format!(".{name}.{}.{n}.tmp", std::process::id()));
    let linked = std::fs::write(&tmp, bytes).and_then(|()| std::fs::hard_link(&tmp, path));
    let _ = std::fs::remove_file(&tmp);
    match linked {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(format!("write {}: {e}", path.display())),
    }
}

/// `dir` is a direct child of `scan_path` (G2, I5): the only kind of directory
/// the mixer creates as its domain or removes.
pub fn is_child_of(dir: &Path, scan_path: &Path) -> bool {
    dir.is_absolute()
        && matches!(
            dir.components().next_back(),
            Some(std::path::Component::Normal(_))
        )
        && dir.parent() == Some(scan_path)
}

/// `MXL_DOMAIN_SCAN_PATH` must be an existing directory: the platform mounts it
/// (hostPath); without it the domain would land in the container's own files.
pub fn check_scan_path(scan_path: &Path) -> Result<(), String> {
    match std::fs::metadata(scan_path) {
        Ok(m) if m.is_dir() => Ok(()),
        Ok(_) => Err(format!(
            "MXL_DOMAIN_SCAN_PATH {} is not a directory",
            scan_path.display()
        )),
        Err(e) => Err(format!("MXL_DOMAIN_SCAN_PATH {}: {e}", scan_path.display())),
    }
}

/// Remove the own output domain (G8 with `MXL_CLEANUP_ON_EXIT`). Refuses
/// anything but a direct child of the scan path.
pub fn remove_output_domain(dir: &Path, scan_path: &Path) -> Result<(), String> {
    if !is_child_of(dir, scan_path) {
        return Err(format!(
            "refusing to remove {}: not a directory directly below {}",
            dir.display(),
            scan_path.display()
        ));
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
        assert_eq!(
            ensure_output_domain(&dir, id, "Mixer", 200_000_000).unwrap(),
            OutputDomain {
                id,
                created: true,
                foreign: false
            }
        );
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .filter(|n| n.to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
        let def: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("domain_def.json")).unwrap())
                .unwrap();
        for key in ["id", "label", "description", "tags"] {
            assert!(def.get(key).is_some(), "{key}");
        }
        let again = ensure_output_domain(&dir, id, "Mixer", 1).unwrap();
        assert!(!again.created && !again.foreign);
        assert!(std::fs::read_to_string(dir.join("options.json"))
            .unwrap()
            .contains("200000000"));
        // Another configured id: the existing one is kept, the file untouched.
        let before = std::fs::read_to_string(dir.join("domain_def.json")).unwrap();
        assert_eq!(
            ensure_output_domain(&dir, Uuid::new_v4(), "Other", 1).unwrap(),
            OutputDomain {
                id,
                created: false,
                foreign: true
            }
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("domain_def.json")).unwrap(),
            before
        );
        // An unreadable definition is an error, not overwritten.
        let bad = root.path().join("mixer-bad");
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::write(bad.join("domain_def.json"), "{").unwrap();
        assert!(ensure_output_domain(&bad, id, "Mixer", 1).is_err());
        assert_eq!(
            std::fs::read_to_string(bad.join("domain_def.json")).unwrap(),
            "{"
        );
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
        // Only a direct child of the scan path is ever removed.
        let nested = root.path().join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();
        assert!(remove_output_domain(&nested, root.path()).is_err());
        assert!(remove_output_domain(root.path().parent().unwrap(), root.path()).is_err());
        assert!(remove_output_domain(Path::new("relative"), Path::new("")).is_err());
        assert!(nested.exists());
    }

    #[test]
    fn writers_creating_one_domain_at_once_all_succeed() {
        // The integration tests start several tone writers on one new domain;
        // one used to read the other's half-written domain_def.json and exit.
        for _ in 0..20 {
            let root = tempfile::tempdir().unwrap();
            let dir = root.path().join("tones");
            let id = Uuid::new_v4();
            let results: Vec<_> = std::thread::scope(|s| {
                let handles: Vec<_> = (0..8)
                    .map(|_| s.spawn(|| ensure_output_domain(&dir, id, "tones", 1)))
                    .collect();
                handles.into_iter().map(|h| h.join().unwrap()).collect()
            });
            assert!(results
                .iter()
                .all(|r| r.as_ref().is_ok_and(|d| d.id == id && !d.foreign)));
            assert_eq!(
                results
                    .iter()
                    .filter(|r| r.as_ref().unwrap().created)
                    .count(),
                1
            );
            assert_eq!(read_def(&dir).unwrap().id, id);
            assert!(dir.join("options.json").exists());
        }
    }

    #[test]
    fn the_scan_path_must_be_a_directory() {
        let root = tempfile::tempdir().unwrap();
        assert!(check_scan_path(root.path()).is_ok());
        assert!(check_scan_path(&root.path().join("missing")).is_err());
        let file = root.path().join("file");
        std::fs::write(&file, "").unwrap();
        assert!(check_scan_path(&file)
            .unwrap_err()
            .contains("not a directory"));
    }

    #[test]
    fn a_domain_is_a_direct_child_of_the_scan_path() {
        let scan = Path::new("/Volumes/mxl");
        assert!(is_child_of(Path::new("/Volumes/mxl/mix1"), scan));
        assert!(is_child_of(
            Path::new("/Volumes/mxl/mix1/"),
            Path::new("/Volumes/mxl/")
        ));
        for bad in [
            "/Volumes/mxl",
            "/Volumes",
            "/",
            "/Volumes/mxl/a/b",
            "/Volumes/mxl/..",
            "/Volumes/mxl/a/..",
            "/Volumes/other/mix1",
            "mix1",
            "Volumes/mxl/mix1",
        ] {
            assert!(!is_child_of(Path::new(bad), scan), "{bad}");
        }
    }
}
