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
    Setting { name: "WEB_PORT", default: "8190", description: "Port of the web UI, the API, /livez, /readyz and /metrics." },
    Setting { name: "NMOS_PORT", default: "3342", description: "Port of the IS-04 Node API and the IS-05 Connection API." },
    Setting { name: "NMOS_SEED", default: "<HOST_ID>-mixer", description: "Seed of every NMOS id, the output domain id and the MXL flow ids (UUIDv5)." },
    Setting { name: "NMOS_LABEL", default: "<HOST_ID>", description: "Node label; the Device is \"<label> Audio Mixer\"." },
    Setting { name: "NMOS_TAGS", default: "{}", description: "JSON object of tag name to string arrays, on the Node and the Device." },
    Setting { name: "NMOS_HOST_ADDRESS", default: "first non-loopback IPv4", description: "IPv4 literal announced in NMOS (the pod IP on the platform)." },
    Setting { name: "NMOS_REGISTRY_ADDRESS", default: "unset", description: "IPv4 address or DNS name of the IS-04 Registration API. Unset: no registration." },
    Setting { name: "NMOS_REGISTRY_PORT", default: "unset", description: "Port of the Registration API (set together with the address)." },
    Setting { name: "NMOS_QUERY_ADDRESS", default: "registry address", description: "IPv4 address or DNS name of the Query API, used for readiness." },
    Setting { name: "NMOS_QUERY_PORT", default: "registration port + 1", description: "Query API port." },
    Setting { name: "NMOS_DNS_SD", default: "false", description: "mDNS/DNS-SD browsing and advertisement (needs a build with the dns-sd feature)." },
    Setting { name: "MXL_DOMAIN_SCAN_PATH", default: "/Volumes/mxl", description: "Parent of the MXL domain directories (an existing absolute directory, else exit 78); input domains (mirrors included) are found below it." },
    Setting { name: "MXL_OUTPUT_DOMAIN_DIR", default: "<scan path>/mixer-<first 8 hex of the domain id>", description: "The mixer's own output domain, a directory directly below the scan path (else exit 78). Created if missing; the only directory MXL_CLEANUP_ON_EXIT removes." },
    Setting { name: "MXL_OUTPUT_DOMAIN_ID", default: "UUIDv5 of NMOS_SEED", description: "Id of the own output domain. An existing domain_def.json with another id is kept (a warning), never overwritten." },
    Setting { name: "MXL_HISTORY_DURATION_NS", default: "200000000", description: "history_duration written into a new output domain." },
    Setting { name: "MXL_CLEANUP_ON_EXIT", default: "false", description: "Remove the own output domain directory on SIGTERM." },
    Setting { name: "SHUTDOWN_TIMEOUT_S", default: "10", description: "Time for the SIGTERM sequence (1–120)." },
    Setting { name: "MIXER_CONFIG_FILE", default: "unset", description: "JSON file with any of these settings (environment wins)." },
    Setting { name: "MIXER_STATE_DIR", default: "/config", description: "Directory of everything the mixer writes itself (show file, connections); must be writable, else exit 78." },
    Setting { name: "MIXER_LOG_LEVEL", default: "info", description: "Log level: error, warn, info, debug, trace." },
    Setting { name: "MIXER_INPUT_FLOWS", default: "16", description: "Number of MXL audio inputs (NMOS receivers), 1–256." },
    Setting { name: "MIXER_BLOCK_SAMPLES", default: "96", description: "DSP block size in samples at 48 kHz: 48, 96, 144 or 240." },
    Setting { name: "MIXER_READ_OFFSET", default: "2", description: "Read offset in ms: every input is read at t_out − (offset + its extra delay)." },
    Setting { name: "MIXER_OUTPUT_COMMIT_SAMPLES", default: "one block", description: "Samples per MXL commit of an output (a multiple of the block size)." },
    Setting { name: "MIXER_RT_PRIORITY", default: "off", description: "SCHED_FIFO priority of the DSP thread, 1–49 (needs CAP_SYS_NICE)." },
    Setting { name: "MIXER_INPUT_STALL_S", default: "2", description: "An input whose head index does not move for this long is stalled and re-opened." },
    Setting { name: "MIXER_INPUT_DELAY_MAX_MS", default: "150", description: "Upper limit of an input's automatic extra delay." },
    Setting { name: "MIXER_INPUT_DELAY_CONFIRM", default: "false", description: "true: an automatic delay change waits for operator confirmation instead of applying with a fade." },
    Setting { name: "WIDGET_FRAME_ANCESTORS", default: "'self'", description: "CSP frame-ancestors of the /widget pages; /widgets answers these origins with CORS." },
    Setting { name: "TSL_ENABLE", default: "false", description: "Listen for TSL UMD 5.0 tally (audio-follows-video)." },
    Setting { name: "TSL_UDP_PORT", default: "8910", description: "TSL UMD 5.0 over UDP." },
    Setting { name: "TSL_TCP_PORT", default: "8911", description: "TSL UMD 5.0 over TCP (DLE/STX framing)." },
    Setting { name: "TSL_SCREEN", default: "-1", description: "Only this TSL screen; -1 is every screen." },
    Setting { name: "TSL_MAP", default: "empty", description: "display:index pairs, e.g. 0:1,1:2: the tally index a TSL display index drives; empty: the same number." },
    Setting { name: "PREVIEW_PUBLISH_URL", default: "unset", description: "RTSP base URL of the shared MediaMTX. Unset: start a built-in MediaMTX and publish there." },
    Setting { name: "PREVIEW_PATH_PREFIX", default: "mxl-audio-mixer", description: "Path prefix of every stream: <prefix>/<monitor id>." },
    Setting { name: "PREVIEW_WHEP_URL", default: "unset", description: "Public base URL of WHEP and WHIP for the browser. Unset with the built-in MediaMTX: the page's host and MEDIAMTX_WHEP_PORT." },
    Setting { name: "PREVIEW_HLS_URL", default: "unset", description: "Public base URL of HLS (listed for the platform; the mixer plays by WHEP)." },
    Setting { name: "MEDIAMTX_API_URL", default: "unset", description: "API of the shared MediaMTX, for reader counts." },
    Setting { name: "MEDIAMTX_RTSP_PORT", default: "8954", description: "RTSP port of the built-in MediaMTX (127.0.0.1)." },
    Setting { name: "MEDIAMTX_WHEP_PORT", default: "8599", description: "WHEP and WHIP port of the built-in MediaMTX." },
    Setting { name: "MEDIAMTX_ICE_UDP_PORT", default: "8199", description: "WebRTC ICE UDP port of the built-in MediaMTX." },
    Setting { name: "MEDIAMTX_BIN", default: "mediamtx", description: "The built-in MediaMTX executable (the image has it)." },
    Setting { name: "MIXER_MAX_WEB_MONITORS", default: "8", description: "Most personal monitor paths of browser clients at once." },
    Setting { name: "MIXER_LINK_ROLE", default: "off", description: "Remote link (§13): off, master or slave. A master connects to MIXER_LINK_PEERS; a slave accepts them on /api/v1/link." },
    Setting { name: "MIXER_LINK_TOKEN", default: "unset", description: "Pre-shared token of the remote link. Required when the role is not off. Never shown by GET /api/v1/config." },
    Setting { name: "MIXER_LINK_PEERS", default: "unset", description: "Master only: up to 4 WebSocket URLs of slaves, comma-separated (ws:// or wss://)." },
    Setting { name: "MIXER_LINK_TAKEOVER", default: "allow", description: "Slave: allow or deny local changes after the master has disconnected. The last mix is kept either way." },
];

pub const ALLOWED_BLOCKS: [u32; 4] = [48, 96, 144, 240];

/// Remote-link role (§13).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkRole {
    Off,
    Master,
    Slave,
}

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
    /// CSP source list for the widget pages.
    pub widget_frame_ancestors: String,
    pub tsl_enable: bool,
    pub tsl_udp_port: u16,
    pub tsl_tcp_port: u16,
    /// Only this TSL screen; `None`: every screen.
    pub tsl_screen: Option<u16>,
    /// TSL display index → tally index; empty: the same number.
    pub tsl_map: BTreeMap<u16, u16>,
    /// RTSP base of the shared MediaMTX; `None`: the built-in one.
    pub preview_publish_url: Option<String>,
    pub preview_path_prefix: String,
    pub preview_whep_url: Option<String>,
    pub preview_hls_url: Option<String>,
    pub mediamtx_api_url: Option<String>,
    pub mediamtx_rtsp_port: u16,
    pub mediamtx_whep_port: u16,
    pub mediamtx_ice_udp_port: u16,
    pub mediamtx_bin: String,
    pub max_web_monitors: usize,
    pub link_role: LinkRole,
    /// Empty when the role is off. Not included in `values`.
    pub link_token: String,
    pub link_peers: Vec<String>,
    /// Slave rejects local mix changes after the master has disconnected.
    pub link_takeover_deny: bool,
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
        let web_port: u16 = self.int("WEB_PORT", 8190, 1, 65535)?;
        let nmos_port: u16 = self.int("NMOS_PORT", 3342, 1, 65535)?;
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
        if !scan_path.is_absolute() {
            return err(format!(
                "MXL_DOMAIN_SCAN_PATH must be an absolute path, got {}",
                scan_path.display()
            ));
        }
        // The only directory the mixer writes as its own, and removes on exit.
        if !crate::mxlio::domain::is_child_of(&output_domain_dir, &scan_path) {
            return err(format!(
                "MXL_OUTPUT_DOMAIN_DIR must be an absolute path directly below MXL_DOMAIN_SCAN_PATH ({}), got {}",
                scan_path.display(),
                output_domain_dir.display()
            ));
        }
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
        let widget_frame_ancestors = self
            .get("WIDGET_FRAME_ANCESTORS")
            .map(|(v, _)| v)
            .unwrap_or_else(|| "'self'".into());
        // One CSP directive's source list: a ';' or ',' would start another directive or policy.
        if widget_frame_ancestors
            .chars()
            .any(|c| c.is_control() || c == ';' || c == ',')
        {
            return err("WIDGET_FRAME_ANCESTORS must be a CSP source list, e.g. 'self' https://designer.example");
        }
        let tsl_enable = self.boolean("TSL_ENABLE", false)?;
        let tsl_udp_port: u16 = self.int("TSL_UDP_PORT", 8910, 1, 65535)?;
        let tsl_tcp_port: u16 = self.int("TSL_TCP_PORT", 8911, 1, 65535)?;
        if tsl_enable {
            for p in [tsl_udp_port, tsl_tcp_port] {
                if p == web_port || p == nmos_port {
                    return err(format!("TSL port {p} is WEB_PORT or NMOS_PORT"));
                }
            }
        }
        let tsl_screen = match self.int::<i64>("TSL_SCREEN", -1, -1, 65535)? {
            -1 => None,
            s => Some(s as u16),
        };
        let tsl_map = match self.get("TSL_MAP") {
            None => BTreeMap::new(),
            Some((raw, _)) => parse_tsl_map(&raw)?,
        };

        let url = |name: &str, schemes: &[&str]| -> Result<Option<String>, ConfigError> {
            match self.get(name) {
                None => Ok(None),
                Some((raw, _)) => {
                    let ok = schemes
                        .iter()
                        .any(|s| raw.starts_with(&format!("{s}://")) && raw.len() > s.len() + 3);
                    if ok && !raw.contains(char::is_whitespace) {
                        Ok(Some(raw.trim_end_matches('/').to_string()))
                    } else {
                        err(format!(
                            "{name} must be a {} URL, got {raw}",
                            schemes.join(" or ")
                        ))
                    }
                }
            }
        };
        let preview_publish_url = url("PREVIEW_PUBLISH_URL", &["rtsp"])?;
        let preview_whep_url = url("PREVIEW_WHEP_URL", &["http", "https"])?;
        let preview_hls_url = url("PREVIEW_HLS_URL", &["http", "https"])?;
        let mediamtx_api_url = url("MEDIAMTX_API_URL", &["http", "https"])?;
        let preview_path_prefix = self
            .get("PREVIEW_PATH_PREFIX")
            .map(|(v, _)| v.trim_matches('/').to_string())
            .unwrap_or_else(|| "mxl-audio-mixer".into());
        if preview_path_prefix.is_empty()
            || !preview_path_prefix
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_./".contains(&b))
            || preview_path_prefix.contains("//")
        {
            return err("PREVIEW_PATH_PREFIX must be path segments of a-z, A-Z, 0-9, '-', '_', '.' separated by '/'");
        }
        let mediamtx_rtsp_port: u16 = self.int("MEDIAMTX_RTSP_PORT", 8954, 1, 65535)?;
        let mediamtx_whep_port: u16 = self.int("MEDIAMTX_WHEP_PORT", 8599, 1, 65535)?;
        let mediamtx_ice_udp_port: u16 = self.int("MEDIAMTX_ICE_UDP_PORT", 8199, 1, 65535)?;
        if preview_publish_url.is_none() {
            let tcp = [web_port, nmos_port, mediamtx_rtsp_port, mediamtx_whep_port];
            if (1..tcp.len()).any(|i| tcp[i..].contains(&tcp[i - 1])) {
                return err(
                    "WEB_PORT, NMOS_PORT, MEDIAMTX_RTSP_PORT and MEDIAMTX_WHEP_PORT must differ",
                );
            }
        }
        let mediamtx_bin = self
            .get("MEDIAMTX_BIN")
            .map(|(v, _)| v)
            .unwrap_or_else(|| "mediamtx".into());
        let max_web_monitors: usize = self.int("MIXER_MAX_WEB_MONITORS", 8, 0, 64)?;
        let link_role = match self
            .get("MIXER_LINK_ROLE")
            .map(|(v, _)| v)
            .unwrap_or_else(|| "off".into())
            .as_str()
        {
            "off" => LinkRole::Off,
            "master" => LinkRole::Master,
            "slave" => LinkRole::Slave,
            other => {
                return err(format!(
                    "MIXER_LINK_ROLE must be off, master or slave, got {other}"
                ))
            }
        };
        let link_token = self
            .get("MIXER_LINK_TOKEN")
            .map(|(v, _)| v)
            .unwrap_or_default();
        if link_role != LinkRole::Off && (link_token.len() < 8 || link_token.len() > 200) {
            return err(
                "MIXER_LINK_TOKEN must have 8 to 200 characters when MIXER_LINK_ROLE is not off",
            );
        }
        let link_peers = self
            .get("MIXER_LINK_PEERS")
            .map(|(v, _)| v)
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();
        if link_role == LinkRole::Master && (link_peers.is_empty() || link_peers.len() > 4) {
            return err("MIXER_LINK_PEERS must list 1 to 4 ws:// or wss:// URLs when MIXER_LINK_ROLE is master");
        }
        if link_peers
            .iter()
            .any(|u| !(u.starts_with("ws://") || u.starts_with("wss://")))
        {
            return err("MIXER_LINK_PEERS entries must be ws:// or wss:// URLs");
        }
        let takeover = self
            .get("MIXER_LINK_TAKEOVER")
            .map(|(v, _)| v)
            .unwrap_or_else(|| "allow".into());
        let link_takeover_deny = match takeover.as_str() {
            "allow" => false,
            "deny" => true,
            other => {
                return err(format!(
                    "MIXER_LINK_TAKEOVER must be allow or deny, got {other}"
                ))
            }
        };

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
            widget_frame_ancestors,
            tsl_enable,
            tsl_udp_port,
            tsl_tcp_port,
            tsl_screen,
            tsl_map,
            preview_publish_url,
            preview_path_prefix,
            preview_whep_url,
            preview_hls_url,
            mediamtx_api_url,
            mediamtx_rtsp_port,
            mediamtx_whep_port,
            mediamtx_ice_udp_port,
            mediamtx_bin,
            max_web_monitors,
            link_role,
            link_token,
            link_peers,
            link_takeover_deny,
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
    /// The id in force when an existing `domain_def.json` has another one (G2):
    /// it is kept, and `GET /api/v1/config` shows it.
    pub fn set_output_domain_id(&mut self, id: Uuid) {
        self.output_domain_id = id;
        let text = self.value_text("MXL_OUTPUT_DOMAIN_ID");
        for (name, value, _) in &mut self.values {
            if *name == "MXL_OUTPUT_DOMAIN_ID" {
                *value = text.clone();
            }
        }
    }

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
            "WIDGET_FRAME_ANCESTORS" => self.widget_frame_ancestors.clone(),
            "TSL_ENABLE" => self.tsl_enable.to_string(),
            "TSL_UDP_PORT" => self.tsl_udp_port.to_string(),
            "TSL_TCP_PORT" => self.tsl_tcp_port.to_string(),
            "TSL_SCREEN" => self.tsl_screen.map_or("-1".into(), |s| s.to_string()),
            "TSL_MAP" => self
                .tsl_map
                .iter()
                .map(|(d, i)| format!("{d}:{i}"))
                .collect::<Vec<_>>()
                .join(","),
            "PREVIEW_PUBLISH_URL" => opt(self.preview_publish_url.clone()),
            "PREVIEW_PATH_PREFIX" => self.preview_path_prefix.clone(),
            "PREVIEW_WHEP_URL" => opt(self.preview_whep_url.clone()),
            "PREVIEW_HLS_URL" => opt(self.preview_hls_url.clone()),
            "MEDIAMTX_API_URL" => opt(self.mediamtx_api_url.clone()),
            "MEDIAMTX_RTSP_PORT" => self.mediamtx_rtsp_port.to_string(),
            "MEDIAMTX_WHEP_PORT" => self.mediamtx_whep_port.to_string(),
            "MEDIAMTX_ICE_UDP_PORT" => self.mediamtx_ice_udp_port.to_string(),
            "MEDIAMTX_BIN" => self.mediamtx_bin.clone(),
            "MIXER_MAX_WEB_MONITORS" => self.max_web_monitors.to_string(),
            "MIXER_LINK_ROLE" => match self.link_role {
                LinkRole::Off => "off",
                LinkRole::Master => "master",
                LinkRole::Slave => "slave",
            }
            .into(),
            "MIXER_LINK_TOKEN" => {
                if self.link_token.is_empty() {
                    String::new()
                } else {
                    "set".into()
                }
            }
            "MIXER_LINK_PEERS" => self.link_peers.join(","),
            "MIXER_LINK_TAKEOVER" => {
                if self.link_takeover_deny {
                    "deny"
                } else {
                    "allow"
                }
            }
            .into(),
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

/// `TSL_MAP`: comma-separated `display:index` pairs, both 0–65535, each display once.
fn parse_tsl_map(raw: &str) -> Result<BTreeMap<u16, u16>, ConfigError> {
    let mut map = BTreeMap::new();
    for item in raw.split(',').map(str::trim).filter(|i| !i.is_empty()) {
        let parsed = item.split_once(':').and_then(|(d, i)| {
            Some((d.trim().parse::<u16>().ok()?, i.trim().parse::<u16>().ok()?))
        });
        let Some((display, index)) = parsed else {
            return err(format!(
                "TSL_MAP entries must look like display:index (0-65535), got {item}"
            ));
        };
        if map.insert(display, index).is_some() {
            return err(format!("TSL_MAP maps display {display} twice"));
        }
    }
    Ok(map)
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
        assert_eq!(c.web_port, 8190);
        assert_eq!(c.nmos_port, 3342);
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
        assert_eq!(c.widget_frame_ancestors, "'self'");
        assert!(!c.tsl_enable);
        assert_eq!(
            (c.tsl_udp_port, c.tsl_tcp_port, c.tsl_screen),
            (8910, 8911, None)
        );
        assert!(c.tsl_map.is_empty());
        assert_eq!(c.preview_publish_url, None);
        assert_eq!(c.preview_path_prefix, "mxl-audio-mixer");
        assert_eq!(
            (
                c.mediamtx_rtsp_port,
                c.mediamtx_whep_port,
                c.mediamtx_ice_udp_port
            ),
            (8954, 8599, 8199)
        );
        assert_eq!(c.max_web_monitors, 8);
    }

    #[test]
    fn preview() {
        let c = load_vars(&[
            ("PREVIEW_PUBLISH_URL", "rtsp://mediamtx.ns.svc:8554/"),
            ("PREVIEW_PATH_PREFIX", "/prod/mix1/"),
            ("PREVIEW_WHEP_URL", "https://media.example"),
        ])
        .unwrap();
        assert_eq!(
            c.preview_publish_url.as_deref(),
            Some("rtsp://mediamtx.ns.svc:8554")
        );
        assert_eq!(c.preview_path_prefix, "prod/mix1");
        for (k, v) in [
            ("PREVIEW_PUBLISH_URL", "http://x"),
            ("PREVIEW_WHEP_URL", "media.example"),
            ("PREVIEW_PATH_PREFIX", "a b"),
            ("PREVIEW_PATH_PREFIX", "a//b"),
            ("MEDIAMTX_WHEP_PORT", "8190"),
            ("MIXER_MAX_WEB_MONITORS", "65"),
        ] {
            assert!(load_vars(&[(k, v)]).is_err(), "{k}={v}");
        }
    }

    #[test]
    fn widgets_and_tally() {
        let c = load_vars(&[
            ("WIDGET_FRAME_ANCESTORS", "'self' https://designer.example"),
            ("TSL_ENABLE", "true"),
            ("TSL_SCREEN", "3"),
            ("TSL_MAP", "0:1, 1:2,7:7"),
        ])
        .unwrap();
        assert_eq!(c.widget_frame_ancestors, "'self' https://designer.example");
        assert_eq!(c.tsl_screen, Some(3));
        assert_eq!(c.tsl_map, BTreeMap::from([(0, 1), (1, 2), (7, 7)]));
        for (k, v) in [
            ("WIDGET_FRAME_ANCESTORS", "'self'; script-src *"),
            ("WIDGET_FRAME_ANCESTORS", "a, b"),
            ("TSL_MAP", "0-1"),
            ("TSL_MAP", "0:1,0:2"),
            ("TSL_MAP", "70000:1"),
            ("TSL_SCREEN", "-2"),
        ] {
            assert!(load_vars(&[(k, v)]).is_err(), "{k}={v}");
        }
        assert!(load_vars(&[("TSL_ENABLE", "true"), ("TSL_UDP_PORT", "8190")]).is_err());
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
            vec![("WEB_PORT", "3342")],
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

    #[test]
    fn the_output_domain_is_directly_below_the_scan_path() {
        let c = load_vars(&[
            ("MXL_DOMAIN_SCAN_PATH", "/dev/shm/mxl/"),
            ("MXL_OUTPUT_DOMAIN_DIR", "/dev/shm/mxl/mix1"),
        ])
        .unwrap();
        assert_eq!(c.output_domain_dir, PathBuf::from("/dev/shm/mxl/mix1"));
        for dir in [
            "/Volumes",
            "/Volumes/mxl",
            "/Volumes/mxl/a/b",
            "/Volumes/mxl/..",
            "/tmp/mix1",
            "mix1",
        ] {
            let e = load_vars(&[("MXL_OUTPUT_DOMAIN_DIR", dir)]).unwrap_err();
            assert!(
                e.0.contains("directly below MXL_DOMAIN_SCAN_PATH"),
                "{dir}: {e}"
            );
        }
        assert!(load_vars(&[("MXL_DOMAIN_SCAN_PATH", "mxl")]).is_err());
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
