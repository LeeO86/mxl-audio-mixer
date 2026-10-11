//! Settings (G1): environment, then the JSON file `MIXER_CONFIG_FILE`, then the
//! defaults. Unknown environment variables are ignored; unknown file keys and
//! invalid values are errors (exit code 78). Every setting is in [`SETTINGS`],
//! which also feeds the README table and `GET /api/v1/config`.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::time::Duration;

use uuid::Uuid;

/// One documented setting.
pub struct Setting {
    pub name: &'static str,
    pub default: &'static str,
    pub description: &'static str,
}

/// Every setting the mixer reads. Defaults shown as text; computed defaults are
/// described in words.
pub const SETTINGS: &[Setting] = &[
    Setting { name: "HOST_ID", default: "the host name", description: "Instance name; the default node label and the start of the default NMOS seed." },
    Setting { name: "WEB_PORT", default: "8160", description: "Port of the web UI, the API, /livez, /readyz and /metrics." },
    Setting { name: "NMOS_PORT", default: "3312", description: "Port of the IS-04 Node API and the IS-05 Connection API." },
    Setting { name: "NMOS_SEED", default: "<HOST_ID>-mixer", description: "Seed of every NMOS id, the output domain id and the MXL flow ids (UUIDv5)." },
    Setting { name: "NMOS_LABEL", default: "<HOST_ID>", description: "Node label; the Device is \"<label> Audio Mixer\"." },
    Setting { name: "NMOS_TAGS", default: "{}", description: "JSON object of tag name to string arrays, on the Node and the Device." },
    Setting { name: "NMOS_HOST_ADDRESS", default: "first non-loopback IPv4", description: "IPv4 literal announced in NMOS (the pod IP on the platform)." },
    Setting { name: "NMOS_REGISTRY_ADDRESS", default: "unset", description: "IPv4 address or DNS name of the IS-04 Registration API. Unset: no registration." },
    Setting { name: "NMOS_REGISTRY_PORT", default: "unset", description: "Port of the Registration API (set together with the address)." },
    Setting { name: "NMOS_QUERY_ADDRESS", default: "registry address", description: "IPv4 address or DNS name of the Query API, used for readiness." },
    Setting { name: "NMOS_QUERY_PORT", default: "registration port + 1", description: "Query API port." },
    Setting { name: "NMOS_DNS_SD", default: "false", description: "mDNS/DNS-SD browsing and advertisement (needs a build with the dns-sd feature)." },
    Setting { name: "MXL_DOMAIN_SCAN_PATH", default: "/Volumes/mxl", description: "Parent of the MXL domain directories; input domains (mirrors included) are found below it." },
    Setting { name: "MXL_OUTPUT_DOMAIN_DIR", default: "<scan path>/mixer-<first 8 hex of the domain id>", description: "The mixer's own output domain. Created if missing." },
    Setting { name: "MXL_OUTPUT_DOMAIN_ID", default: "UUIDv5 of NMOS_SEED", description: "Id of the own output domain." },
    Setting { name: "MXL_HISTORY_DURATION_NS", default: "200000000", description: "history_duration written into a new output domain." },
    Setting { name: "MXL_CLEANUP_ON_EXIT", default: "false", description: "Remove the own output domain directory on SIGTERM." },
    Setting { name: "SHUTDOWN_TIMEOUT_S", default: "10", description: "Time for the SIGTERM sequence (1–120)." },
    Setting { name: "MIXER_CONFIG_FILE", default: "unset", description: "JSON file with any of these settings (environment wins)." },
    Setting { name: "MIXER_STATE_DIR", default: "/config", description: "Directory of everything the mixer writes itself (show file, connections)." },
    Setting { name: "MIXER_LOG_LEVEL", default: "info", description: "Log level: error, warn, info, debug, trace." },
    Setting { name: "MIXER_INPUT_FLOWS", default: "16", description: "Number of MXL audio inputs (NMOS receivers), 1–256." },
    Setting { name: "MIXER_BLOCK_SAMPLES", default: "96", description: "DSP block size in samples at 48 kHz: 48, 96, 144 or 240." },
    Setting { name: "MIXER_READ_OFFSET", default: "2", description: "Read offset in ms: every input is read at t_out − (offset + its extra delay)." },
    Setting { name: "MIXER_OUTPUT_COMMIT_SAMPLES", default: "one block", description: "Samples per MXL commit of an output (a multiple of the block size)." },
    Setting { name: "MIXER_RT_PRIORITY", default: "off", description: "SCHED_FIFO priority of the DSP thread, 1–49 (needs CAP_SYS_NICE)." },
    Setting { name: "MIXER_INPUT_STALL_S", default: "2", description: "An input whose head index does not move for this long is stalled and re-opened." },
    Setting { name: "MIXER_INPUT_DELAY_MAX_MS", default: "150", description: "Upper limit of an input's automatic extra delay." },
    Setting { name: "MIXER_INPUT_DELAY_CONFIRM", default: "false", description: "true: an automatic delay change waits for operator confirmation instead of applying with a fade." },
];

pub const ALLOWED_BLOCKS: [u32; 4] = [48, 96, 144, 240];

/// A setting is invalid. The process exits with code 78.
#[derive(Debug)]
pub struct ConfigError(pub String);

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ConfigError {}

fn err<T>(message: impl Into<String>) -> Result<T, ConfigError> {
    Err(ConfigError(message.into()))
}

/// Where a value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Origin {
    Environment,
    File,
    Default,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub host_id: String,
    pub web_port: u16,
    pub nmos_port: u16,
    pub nmos_seed: String,
    pub nmos_label: String,
    pub nmos_tags: BTreeMap<String, Vec<String>>,
    pub nmos_host_address: Option<Ipv4Addr>,
    /// Registration API host (IPv4 literal or DNS name) and port.
    pub nmos_registry: Option<(String, u16)>,
    /// Query API host and port.
    pub nmos_query: Option<(String, u16)>,
    pub nmos_dns_sd: bool,
    pub scan_path: PathBuf,
    pub output_domain_dir: PathBuf,
    pub output_domain_id: Uuid,
    pub history_duration_ns: u64,
    pub cleanup_on_exit: bool,
    pub shutdown_timeout: Duration,
    pub config_file: Option<PathBuf>,
    pub state_dir: PathBuf,
    pub log_level: String,
    pub input_flows: usize,
    pub block_samples: u32,
    pub read_offset_samples: u32,
    pub output_commit_samples: u32,
    pub rt_priority: Option<i32>,
    pub input_stall: Duration,
    pub input_delay_max_samples: u32,
    pub input_delay_confirm: bool,
    /// Every setting's value as text and its origin, for `GET /api/v1/config`.
    pub values: Vec<(&'static str, String, Origin)>,
}

pub const SAMPLE_RATE: u32 = 48_000;

fn ms_to_samples(ms: f64) -> u32 {
    (ms * f64::from(SAMPLE_RATE) / 1000.0).round() as u32
}

/// Read the settings from the process environment (and the file it names).
pub fn from_env() -> Result<Config, ConfigError> {
    let env: HashMap<String, String> = std::env::vars().collect();
    load(&env)
}

/// Read the settings from `env` (and the file it names).
pub fn load(env: &HashMap<String, String>) -> Result<Config, ConfigError> {
    let env: HashMap<String, String> = env
        .iter()
        .filter(|(_, v)| !v.trim().is_empty())
        .map(|(k, v)| (k.clone(), v.trim().to_string()))
        .collect();
    let file = match env.get("MIXER_CONFIG_FILE") {
        Some(path) => read_file(path)?,
        None => HashMap::new(),
    };
    Parser {
        env: &env,
        file: &file,
    }
    .config()
}

fn read_file(path: &str) -> Result<HashMap<String, String>, ConfigError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| ConfigError(format!("cannot read MIXER_CONFIG_FILE {path}: {e}")))?;
    let doc: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| ConfigError(format!("MIXER_CONFIG_FILE {path} is not JSON: {e}")))?;
    let object = doc
        .as_object()
        .ok_or_else(|| ConfigError("MIXER_CONFIG_FILE must hold a JSON object".into()))?;
    let mut out = HashMap::new();
    for (key, value) in object {
        if !SETTINGS.iter().any(|s| s.name == key) {
            return err(format!("unknown setting {key} in MIXER_CONFIG_FILE"));
        }
        let text = match value {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Number(n) => n.to_string(),
            serde_json::Value::Bool(b) => b.to_string(),
            serde_json::Value::Object(_) if key == "NMOS_TAGS" => value.to_string(),
            _ => {
                return err(format!(
                    "{key} in MIXER_CONFIG_FILE must be a string, number or boolean"
                ))
            }
        };
        if key == "MIXER_CONFIG_FILE" {
            return err("MIXER_CONFIG_FILE cannot be set in the file itself");
        }
        out.insert(key.clone(), text);
    }
    Ok(out)
}

struct Parser<'a> {
    env: &'a HashMap<String, String>,
    file: &'a HashMap<String, String>,
}

impl Parser<'_> {
    fn get(&self, name: &str) -> Option<(String, Origin)> {
        if let Some(v) = self.env.get(name) {
            return Some((v.clone(), Origin::Environment));
        }
        self.file
            .get(name)
            .filter(|v| !v.trim().is_empty())
            .map(|v| (v.trim().to_string(), Origin::File))
    }

    fn origin(&self, name: &str) -> Origin {
        self.get(name).map(|(_, o)| o).unwrap_or(Origin::Default)
    }

    fn int<T: TryFrom<i64>>(
        &self,
        name: &str,
        default: i64,
        min: i64,
        max: i64,
    ) -> Result<T, ConfigError> {
        let value = match self.get(name) {
            None => default,
            Some((raw, _)) => match raw.parse::<i64>() {
                Ok(v) => v,
                Err(_) => return err(format!("{name} must be an integer, got {raw}")),
            },
        };
        if value < min || value > max {
            return err(format!("{name} must be from {min} to {max}, got {value}"));
        }
        T::try_from(value).map_err(|_| ConfigError(format!("{name} is out of range")))
    }

    fn float(&self, name: &str, default: f64, min: f64, max: f64) -> Result<f64, ConfigError> {
        let value = match self.get(name) {
            None => default,
            Some((raw, _)) => raw
                .parse::<f64>()
                .map_err(|_| ConfigError(format!("{name} must be a number, got {raw}")))?,
        };
        if !(min..=max).contains(&value) {
            return err(format!("{name} must be from {min} to {max}, got {value}"));
        }
        Ok(value)
    }

    fn boolean(&self, name: &str, default: bool) -> Result<bool, ConfigError> {
        match self.get(name) {
            None => Ok(default),
            Some((raw, _)) => match raw.to_ascii_lowercase().as_str() {
                "1" | "true" | "yes" | "on" => Ok(true),
                "0" | "false" | "no" | "off" => Ok(false),
                _ => err(format!("{name} must be true or false, got {raw}")),
            },
        }
    }

    /// An IPv4 literal or a DNS name (RFC 1123), for addresses the mixer connects
    /// to. Addresses it announces stay IPv4 literals (G5).
    fn host(&self, name: &str) -> Result<Option<String>, ConfigError> {
        match self.get(name) {
            None => Ok(None),
            Some((raw, _)) if is_host(&raw) => Ok(Some(raw.to_ascii_lowercase())),
            Some((raw, _)) => err(format!(
                "{name} must be an IPv4 address or a DNS name, got {raw}"
            )),
        }
    }

    fn config(&self) -> Result<Config, ConfigError> {
        let host_id = self
            .get("HOST_ID")
            .map(|(v, _)| v)
            .or_else(hostname)
            .unwrap_or_else(|| "mixer".to_string());
        let web_port: u16 = self.int("WEB_PORT", 8160, 1, 65535)?;
        let nmos_port: u16 = self.int("NMOS_PORT", 3312, 1, 65535)?;
        if web_port == nmos_port {
            return err("WEB_PORT and NMOS_PORT must differ");
        }
        let nmos_seed = self
            .get("NMOS_SEED")
            .map(|(v, _)| v)
            .unwrap_or_else(|| format!("{host_id}-mixer"));
        let nmos_label = self
            .get("NMOS_LABEL")
            .map(|(v, _)| v)
            .unwrap_or_else(|| host_id.clone());
        let nmos_tags = match self.get("NMOS_TAGS") {
            None => BTreeMap::new(),
            Some((raw, _)) => mxl_nmos::parse_tags(&raw).map_err(|e| ConfigError(e.to_string()))?,
        };
        let nmos_host_address = match self.get("NMOS_HOST_ADDRESS") {
            None => None,
            Some((raw, _)) => Some(
                mxl_nmos::require_announce_ipv4("NMOS_HOST_ADDRESS", &raw)
                    .map_err(|e| ConfigError(e.to_string()))?,
            ),
        };
        let registry_address = self.host("NMOS_REGISTRY_ADDRESS")?;
        let registry_port: Option<u16> = match self.get("NMOS_REGISTRY_PORT") {
            None => None,
            Some(_) => Some(self.int("NMOS_REGISTRY_PORT", 0, 1, 65535)?),
        };
        let nmos_registry = match (registry_address, registry_port) {
            (Some(a), Some(p)) => Some((a, p)),
            (None, None) => None,
            _ => return err("NMOS_REGISTRY_ADDRESS and NMOS_REGISTRY_PORT must be set together"),
        };
        let query_address = self.host("NMOS_QUERY_ADDRESS")?;
        let query_port: Option<u16> = match self.get("NMOS_QUERY_PORT") {
            None => None,
            Some(_) => Some(self.int("NMOS_QUERY_PORT", 0, 1, 65535)?),
        };
        let nmos_query = match (nmos_registry.clone(), query_address, query_port) {
            (_, Some(a), Some(p)) => Some((a, p)),
            (Some((ra, rp)), a, p) => Some((a.unwrap_or(ra), p.unwrap_or(rp.saturating_add(1)))),
            (None, None, None) => None,
            (None, _, _) => {
                return err("NMOS_QUERY_ADDRESS and NMOS_QUERY_PORT need a registry or each other")
            }
        };
        let nmos_dns_sd = self.boolean("NMOS_DNS_SD", false)?;
        let scan_path = PathBuf::from(
            self.get("MXL_DOMAIN_SCAN_PATH")
                .map(|(v, _)| v)
                .unwrap_or_else(|| "/Volumes/mxl".into()),
        );
        let output_domain_id = match self.get("MXL_OUTPUT_DOMAIN_ID") {
            None => mxl_nmos::ids::derive_id(&nmos_seed, "mxl-output-domain"),
            Some((raw, _)) => raw.parse().map_err(|_| {
                ConfigError(format!("MXL_OUTPUT_DOMAIN_ID must be a UUID, got {raw}"))
            })?,
        };
        let output_domain_dir = match self.get("MXL_OUTPUT_DOMAIN_DIR") {
            Some((raw, _)) => PathBuf::from(raw),
            None => scan_path.join(format!(
                "mixer-{}",
                &output_domain_id.simple().to_string()[..8]
            )),
        };
        let history_duration_ns: u64 =
            self.int("MXL_HISTORY_DURATION_NS", 200_000_000, 1, 60_000_000_000)?;
        let cleanup_on_exit = self.boolean("MXL_CLEANUP_ON_EXIT", false)?;
        let shutdown_timeout =
            Duration::from_secs(self.int::<u64>("SHUTDOWN_TIMEOUT_S", 10, 1, 120)?);
        let config_file = self.env.get("MIXER_CONFIG_FILE").map(PathBuf::from);
        let state_dir = PathBuf::from(
            self.get("MIXER_STATE_DIR")
                .map(|(v, _)| v)
                .unwrap_or_else(|| "/config".into()),
        );
        let log_level = self
            .get("MIXER_LOG_LEVEL")
            .map(|(v, _)| v.to_ascii_lowercase())
            .unwrap_or_else(|| "info".into());
        if !["error", "warn", "info", "debug", "trace"].contains(&log_level.as_str()) {
            return err(format!(
                "MIXER_LOG_LEVEL must be error, warn, info, debug or trace, got {log_level}"
            ));
        }
        let input_flows: usize = self.int("MIXER_INPUT_FLOWS", 16, 1, 256)?;
        let block_samples: u32 = self.int("MIXER_BLOCK_SAMPLES", 96, 1, 240)?;
        if !ALLOWED_BLOCKS.contains(&block_samples) {
            return err(format!(
                "MIXER_BLOCK_SAMPLES must be 48, 96, 144 or 240, got {block_samples}"
            ));
        }
        let history_ms = history_duration_ns as f64 / 1e6;
        let read_offset_ms = self.float("MIXER_READ_OFFSET", 2.0, 0.0, history_ms)?;
        let read_offset_samples = ms_to_samples(read_offset_ms);
        let output_commit_samples: u32 = self.int(
            "MIXER_OUTPUT_COMMIT_SAMPLES",
            i64::from(block_samples),
            i64::from(block_samples),
            4800,
        )?;
        if !output_commit_samples.is_multiple_of(block_samples) {
            return err(format!(
                "MIXER_OUTPUT_COMMIT_SAMPLES must be a multiple of the block size {block_samples}"
            ));
        }
        let rt_priority = match self.get("MIXER_RT_PRIORITY") {
            None => None,
            Some((raw, _)) if ["off", "false", "0"].contains(&raw.as_str()) => None,
            Some(_) => Some(self.int::<i32>("MIXER_RT_PRIORITY", 0, 1, 49)?),
        };
        let input_stall =
            Duration::from_secs_f64(self.float("MIXER_INPUT_STALL_S", 2.0, 0.1, 3600.0)?);
        let delay_max_ms = self.float("MIXER_INPUT_DELAY_MAX_MS", 150.0, 0.0, history_ms)?;
        if read_offset_ms + delay_max_ms > history_ms {
            return err(format!(
                "MIXER_READ_OFFSET + MIXER_INPUT_DELAY_MAX_MS ({} ms) exceeds the MXL history ({history_ms} ms)",
                read_offset_ms + delay_max_ms
            ));
        }
        let input_delay_confirm = self.boolean("MIXER_INPUT_DELAY_CONFIRM", false)?;

        let mut config = Config {
            host_id,
            web_port,
            nmos_port,
            nmos_seed,
            nmos_label,
            nmos_tags,
            nmos_host_address,
            nmos_registry,
            nmos_query,
            nmos_dns_sd,
            scan_path,
            output_domain_dir,
            output_domain_id,
            history_duration_ns,
            cleanup_on_exit,
            shutdown_timeout,
            config_file,
            state_dir,
            log_level,
            input_flows,
            block_samples,
            read_offset_samples,
            output_commit_samples,
            rt_priority,
            input_stall,
            input_delay_max_samples: ms_to_samples(delay_max_ms),
            input_delay_confirm,
            values: Vec::new(),
        };
        config.values = SETTINGS
            .iter()
            .map(|s| (s.name, config.value_text(s.name), self.origin(s.name)))
            .collect();
        Ok(config)
    }
}

impl Config {
    fn value_text(&self, name: &str) -> String {
        let opt = |v: Option<String>| v.unwrap_or_default();
        match name {
            "HOST_ID" => self.host_id.clone(),
            "WEB_PORT" => self.web_port.to_string(),
            "NMOS_PORT" => self.nmos_port.to_string(),
            "NMOS_SEED" => self.nmos_seed.clone(),
            "NMOS_LABEL" => self.nmos_label.clone(),
            "NMOS_TAGS" => serde_json::to_string(&self.nmos_tags).unwrap_or_default(),
            "NMOS_HOST_ADDRESS" => opt(self.nmos_host_address.map(|a| a.to_string())),
            "NMOS_REGISTRY_ADDRESS" => opt(self.nmos_registry.as_ref().map(|(a, _)| a.clone())),
            "NMOS_REGISTRY_PORT" => opt(self.nmos_registry.as_ref().map(|(_, p)| p.to_string())),
            "NMOS_QUERY_ADDRESS" => opt(self.nmos_query.as_ref().map(|(a, _)| a.clone())),
            "NMOS_QUERY_PORT" => opt(self.nmos_query.as_ref().map(|(_, p)| p.to_string())),
            "NMOS_DNS_SD" => self.nmos_dns_sd.to_string(),
            "MXL_DOMAIN_SCAN_PATH" => self.scan_path.display().to_string(),
            "MXL_OUTPUT_DOMAIN_DIR" => self.output_domain_dir.display().to_string(),
            "MXL_OUTPUT_DOMAIN_ID" => self.output_domain_id.to_string(),
            "MXL_HISTORY_DURATION_NS" => self.history_duration_ns.to_string(),
            "MXL_CLEANUP_ON_EXIT" => self.cleanup_on_exit.to_string(),
            "SHUTDOWN_TIMEOUT_S" => self.shutdown_timeout.as_secs().to_string(),
            "MIXER_CONFIG_FILE" => opt(self.config_file.as_ref().map(|p| p.display().to_string())),
            "MIXER_STATE_DIR" => self.state_dir.display().to_string(),
            "MIXER_LOG_LEVEL" => self.log_level.clone(),
            "MIXER_INPUT_FLOWS" => self.input_flows.to_string(),
            "MIXER_BLOCK_SAMPLES" => self.block_samples.to_string(),
            "MIXER_READ_OFFSET" => format!(
                "{}",
                f64::from(self.read_offset_samples) * 1000.0 / f64::from(SAMPLE_RATE)
            ),
            "MIXER_OUTPUT_COMMIT_SAMPLES" => self.output_commit_samples.to_string(),
            "MIXER_RT_PRIORITY" => opt(self.rt_priority.map(|p| p.to_string())),
            "MIXER_INPUT_STALL_S" => format!("{}", self.input_stall.as_secs_f64()),
            "MIXER_INPUT_DELAY_MAX_MS" => format!(
                "{}",
                f64::from(self.input_delay_max_samples) * 1000.0 / f64::from(SAMPLE_RATE)
            ),
            "MIXER_INPUT_DELAY_CONFIRM" => self.input_delay_confirm.to_string(),
            _ => String::new(),
        }
    }

    /// The settings for the NMOS node.
    pub fn nmos_settings(&self) -> mxl_nmos::Settings {
        let mut s = mxl_nmos::Settings::new(self.nmos_label.clone(), self.nmos_port);
        s.description = "MXL audio mixer".into();
        s.seed = Some(self.nmos_seed.clone());
        s.tags = self.nmos_tags.clone();
        s.host = self.nmos_host_address;
        s.registry = self
            .nmos_registry
            .as_ref()
            .map(|(a, p)| format!("http://{a}:{p}"));
        s.query = self
            .nmos_query
            .as_ref()
            .map(|(a, p)| format!("http://{a}:{p}"));
        s.dns_sd = self.nmos_dns_sd;
        s.service_name_prefix = "mxl-audio-mixer".into();
        s
    }

    pub fn block_period(&self) -> Duration {
        Duration::from_nanos(u64::from(self.block_samples) * 1_000_000_000 / u64::from(SAMPLE_RATE))
    }
}

/// An IPv4 literal or an RFC 1123 host name (letters, digits, hyphens, dots).
fn is_host(text: &str) -> bool {
    if text.parse::<Ipv4Addr>().is_ok() {
        return true;
    }
    let labels: Vec<&str> = text.trim_end_matches('.').split('.').collect();
    !text.is_empty()
        && text.len() <= 253
        && labels.iter().all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
        // All-numeric dotted text that is not a valid IPv4 address is a typo.
        && !labels.iter().all(|l| l.bytes().all(|b| b.is_ascii_digit()))
}

fn hostname() -> Option<String> {
    let mut buf = [0u8; 256];
    // SAFETY: the buffer is valid for its length.
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len() - 1) };
    if rc != 0 {
        return None;
    }
    let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
    std::str::from_utf8(&buf[..end])
        .ok()
        .map(str::to_string)
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load_vars(vars: &[(&str, &str)]) -> Result<Config, ConfigError> {
        let env: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        load(&env)
    }

    #[test]
    fn defaults() {
        let c = load_vars(&[("HOST_ID", "studio-a")]).unwrap();
        assert_eq!(c.web_port, 8160);
        assert_eq!(c.nmos_port, 3312);
        assert_eq!(c.nmos_seed, "studio-a-mixer");
        assert_eq!(c.nmos_label, "studio-a");
        assert_eq!(c.input_flows, 16);
        assert_eq!(c.block_samples, 96);
        assert_eq!(c.read_offset_samples, 96, "2 ms");
        assert_eq!(c.output_commit_samples, 96, "one block");
        assert_eq!(c.rt_priority, None);
        assert_eq!(c.state_dir, PathBuf::from("/config"));
        assert_eq!(c.scan_path, PathBuf::from("/Volumes/mxl"));
        assert_eq!(
            c.output_domain_id,
            mxl_nmos::ids::derive_id("studio-a-mixer", "mxl-output-domain")
        );
        assert!(c
            .output_domain_dir
            .to_str()
            .unwrap()
            .starts_with("/Volumes/mxl/mixer-"));
        assert_eq!(c.history_duration_ns, 200_000_000);
        assert_eq!(c.shutdown_timeout, Duration::from_secs(10));
        assert!(c.nmos_registry.is_none() && c.nmos_query.is_none() && !c.nmos_dns_sd);
        let explicit: Vec<_> = c
            .values
            .iter()
            .filter(|(_, _, o)| *o != Origin::Default)
            .map(|(n, _, _)| *n)
            .collect();
        assert_eq!(explicit, vec!["HOST_ID"]);
        assert_eq!(c.values.len(), SETTINGS.len());
    }

    #[test]
    fn environment_wins_over_file_and_origins_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config.json");
        std::fs::write(
            &file,
            r#"{"WEB_PORT": 9000, "MIXER_INPUT_FLOWS": "8", "NMOS_TAGS": {"a": ["b"]}}"#,
        )
        .unwrap();
        let c = load_vars(&[
            ("MIXER_CONFIG_FILE", file.to_str().unwrap()),
            ("WEB_PORT", "9100"),
        ])
        .unwrap();
        assert_eq!(c.web_port, 9100);
        assert_eq!(c.input_flows, 8);
        assert_eq!(c.nmos_tags["a"], vec!["b"]);
        let origin = |n: &str| c.values.iter().find(|(k, _, _)| *k == n).unwrap().2;
        assert_eq!(origin("WEB_PORT"), Origin::Environment);
        assert_eq!(origin("MIXER_INPUT_FLOWS"), Origin::File);
        assert_eq!(origin("MIXER_BLOCK_SAMPLES"), Origin::Default);
    }

    #[test]
    fn registry_and_query() {
        let c = load_vars(&[
            ("NMOS_REGISTRY_ADDRESS", "10.0.0.5"),
            ("NMOS_REGISTRY_PORT", "3210"),
        ])
        .unwrap();
        assert_eq!(c.nmos_registry, Some(("10.0.0.5".into(), 3210)));
        assert_eq!(c.nmos_query, Some(("10.0.0.5".into(), 3211)));
        let s = c.nmos_settings();
        assert_eq!(s.registry.as_deref(), Some("http://10.0.0.5:3210"));
        assert_eq!(s.query.as_deref(), Some("http://10.0.0.5:3211"));
    }

    #[test]
    fn registry_and_query_accept_dns_names() {
        let c = load_vars(&[
            (
                "NMOS_REGISTRY_ADDRESS",
                "nmos-registry.platform.svc.cluster.local",
            ),
            ("NMOS_REGISTRY_PORT", "3210"),
        ])
        .unwrap();
        let s = c.nmos_settings();
        assert_eq!(
            s.registry.as_deref(),
            Some("http://nmos-registry.platform.svc.cluster.local:3210")
        );
        assert_eq!(
            s.query.as_deref(),
            Some("http://nmos-registry.platform.svc.cluster.local:3211")
        );
        let c = load_vars(&[
            ("NMOS_REGISTRY_ADDRESS", "Registry"),
            ("NMOS_REGISTRY_PORT", "3210"),
            ("NMOS_QUERY_ADDRESS", "query"),
        ])
        .unwrap();
        assert_eq!(c.nmos_query, Some(("query".into(), 3211)));
        assert!(
            load_vars(&[("NMOS_HOST_ADDRESS", "mixer.platform.svc")]).is_err(),
            "announced addresses stay IPv4 (G5)"
        );
    }

    #[test]
    fn invalid_values_are_errors() {
        let dir = tempfile::tempdir().unwrap();
        let unknown = dir.path().join("u.json");
        std::fs::write(&unknown, r#"{"NOPE": 1}"#).unwrap();
        for vars in [
            vec![("WEB_PORT", "0")],
            vec![("WEB_PORT", "3312")],
            vec![("MIXER_BLOCK_SAMPLES", "100")],
            vec![("MIXER_INPUT_FLOWS", "0")],
            vec![("MIXER_RT_PRIORITY", "50")],
            vec![("MIXER_READ_OFFSET", "-1")],
            vec![("MIXER_OUTPUT_COMMIT_SAMPLES", "100")],
            vec![("MIXER_INPUT_DELAY_MAX_MS", "199")],
            vec![("NMOS_HOST_ADDRESS", "mixer.local")],
            vec![("NMOS_REGISTRY_ADDRESS", "10.0.0.5")],
            vec![
                ("NMOS_REGISTRY_ADDRESS", "nmos registry"),
                ("NMOS_REGISTRY_PORT", "3210"),
            ],
            vec![
                ("NMOS_REGISTRY_ADDRESS", "10.0.0.500"),
                ("NMOS_REGISTRY_PORT", "3210"),
            ],
            vec![
                ("NMOS_REGISTRY_ADDRESS", "-bad.example"),
                ("NMOS_REGISTRY_PORT", "3210"),
            ],
            vec![
                ("NMOS_QUERY_ADDRESS", "http://q"),
                ("NMOS_QUERY_PORT", "3211"),
            ],
            vec![("MXL_OUTPUT_DOMAIN_ID", "x")],
            vec![("MXL_CLEANUP_ON_EXIT", "maybe")],
            vec![("MIXER_LOG_LEVEL", "loud")],
            vec![("SHUTDOWN_TIMEOUT_S", "0")],
            vec![("NMOS_TAGS", "[]")],
            vec![("MIXER_CONFIG_FILE", "/nonexistent.json")],
            vec![("MIXER_CONFIG_FILE", unknown.to_str().unwrap())],
        ] {
            assert!(load_vars(&vars).is_err(), "{vars:?}");
        }
    }

    #[test]
    fn rt_priority_and_commit() {
        let c = load_vars(&[
            ("MIXER_RT_PRIORITY", "49"),
            ("MIXER_BLOCK_SAMPLES", "48"),
            ("MIXER_OUTPUT_COMMIT_SAMPLES", "480"),
            ("MIXER_READ_OFFSET", "0.5"),
        ])
        .unwrap();
        assert_eq!(c.rt_priority, Some(49));
        assert_eq!(c.output_commit_samples, 480);
        assert_eq!(c.read_offset_samples, 24);
        assert_eq!(c.block_period(), Duration::from_millis(1));
    }
}

#[cfg(test)]
mod readme {
    #[test]
    fn every_setting_is_documented_in_the_readme() {
        let readme = include_str!("../README.md");
        for s in super::SETTINGS {
            assert!(
                readme.contains(&format!("| `{}` |", s.name)),
                "{} is missing from README.md",
                s.name
            );
        }
    }
}
