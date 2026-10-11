//! Show history (§12.5): versions of the mix in `<state dir>/history/`.
//!
//! After a change, a version is kept when the last automatic one is older than
//! [`AUTO_INTERVAL`]; the newest [`AUTO_KEEP`] automatic versions stay. Named
//! versions (saved by an operator) are never removed automatically.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::mix::Mix;

pub const AUTO_INTERVAL: Duration = Duration::from_secs(60);
pub const AUTO_KEEP: usize = 100;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct File {
    version: u32,
    /// Milliseconds since 1970 (UTC).
    time_ms: u64,
    #[serde(default)]
    name: Option<String>,
    mix: Mix,
}

/// One version, without its mix.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Entry {
    pub id: String,
    pub time_ms: u64,
    pub name: Option<String>,
    pub strips: usize,
    pub buses: usize,
    pub sends: usize,
}

pub struct History {
    dir: PathBuf,
    last_auto: Option<Instant>,
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 24 && id.bytes().all(|b| b.is_ascii_digit())
}

impl History {
    pub fn new(state_dir: &Path) -> Self {
        Self {
            dir: state_dir.join("history"),
            last_auto: None,
        }
    }

    /// Keep a version of `mix`; returns its id.
    pub fn record(&self, mix: &Mix, name: Option<String>) -> Result<String, String> {
        std::fs::create_dir_all(&self.dir).map_err(|e| format!("{}: {e}", self.dir.display()))?;
        let mut time_ms = now_ms();
        while self.dir.join(format!("{time_ms}.json")).exists() {
            time_ms += 1;
        }
        let file = File {
            version: 1,
            time_ms,
            name,
            mix: mix.clone(),
        };
        let id = time_ms.to_string();
        crate::state::write_atomic(
            &self.dir.join(format!("{id}.json")),
            &serde_json::to_vec_pretty(&file).unwrap_or_default(),
        )?;
        Ok(id)
    }

    /// After a change: keep an automatic version if the last one is old enough.
    pub fn changed(&mut self, mix: &Mix) {
        if self.last_auto.is_some_and(|t| t.elapsed() < AUTO_INTERVAL) {
            return;
        }
        match self.record(mix, None) {
            Ok(_) => {
                self.last_auto = Some(Instant::now());
                self.prune();
            }
            Err(e) => tracing::warn!("show history: {e}"),
        }
    }

    fn read(&self, id: &str) -> Result<File, String> {
        if !valid_id(id) {
            return Err(format!("version {id} does not exist"));
        }
        let path = self.dir.join(format!("{id}.json"));
        let bytes = std::fs::read(&path).map_err(|_| format!("version {id} does not exist"))?;
        serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Every version, newest first.
    pub fn list(&self) -> Vec<Entry> {
        let mut out: Vec<Entry> = std::fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                let id = name.strip_suffix(".json")?;
                let f = self.read(id).ok()?;
                Some(Entry {
                    id: id.to_string(),
                    time_ms: f.time_ms,
                    name: f.name,
                    strips: f.mix.strips.len(),
                    buses: f.mix.buses.len(),
                    sends: f.mix.sends.len(),
                })
            })
            .collect();
        out.sort_by_key(|e| std::cmp::Reverse(e.time_ms));
        out
    }

    pub fn get(&self, id: &str) -> Result<Mix, String> {
        self.read(id).map(|f| f.mix)
    }

    pub fn delete(&self, id: &str) -> Result<(), String> {
        self.read(id)?;
        std::fs::remove_file(self.dir.join(format!("{id}.json"))).map_err(|e| e.to_string())
    }

    fn prune(&self) {
        let autos: Vec<Entry> = self
            .list()
            .into_iter()
            .filter(|e| e.name.is_none())
            .collect();
        for e in autos.iter().skip(AUTO_KEEP) {
            let _ = std::fs::remove_file(self.dir.join(format!("{}.json", e.id)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_are_kept_listed_and_pruned() {
        let dir = std::env::temp_dir().join(format!("mixer-history-{}", now_ms()));
        let mut h = History::new(&dir);
        let mut mix = Mix::default();
        h.changed(&mix);
        // Within the interval: no second automatic version.
        mix.settings.loudness_target_lufs = -24.0;
        h.changed(&mix);
        assert_eq!(h.list().len(), 1);
        let named = h.record(&mix, Some("before show".into())).unwrap();
        let list = h.list();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, named);
        assert_eq!(list[0].name.as_deref(), Some("before show"));
        assert_eq!(h.get(&named).unwrap().settings.loudness_target_lufs, -24.0);
        for _ in 0..AUTO_KEEP + 3 {
            h.record(&mix, None).unwrap();
        }
        h.prune();
        let list = h.list();
        assert_eq!(list.iter().filter(|e| e.name.is_none()).count(), AUTO_KEEP);
        assert!(list.iter().any(|e| e.id == named));
        assert!(h.get("../mix").is_err());
        h.delete(&named).unwrap();
        assert!(h.get(&named).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
