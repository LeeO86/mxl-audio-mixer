//! What the inputs read (C2, C3):
//! - [`resolve_task`]: the name of what each input is connected to — the IS-04
//!   sender label from the Query API (`NMOS_QUERY_ADDRESS`) for the IS-05
//!   `sender_id`, else the `label` of the MXL flow's `flow_def.json`. Resolved
//!   off the control lock, once per change of sender or flow (a failed lookup
//!   again after [`RETRY`]); the DSP and control paths never wait for it.
//! - `GET /api/v1/flows`: the audio flows in the visible MXL domains.
//! - `PUT /api/v1/inputs/{input}/route` with `{"flow_id", "domain_id",
//!   "sender_id"?}` (or `{"flow_id": null}` to disconnect): routes the input
//!   through its own IS-05 receiver, as a controller would, so the registry, the
//!   saved connections and the UI agree.

use std::collections::HashMap;
use std::path::Path as FsPath;
use std::time::{Duration, Instant};

use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use axum::{Json, Router};
use mxl_nmos::{ResourceType, Role};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::api::{bad, body, internal, not_found};
use crate::inputs::Connected;
use crate::mxlio::domain;
use crate::nmos::receiver_key;
use crate::ops::App;

/// How often the inputs are looked at.
pub const TICK: Duration = Duration::from_secs(2);
/// A lookup that found no name is tried again after this.
pub const RETRY: Duration = Duration::from_secs(30);

pub fn routes() -> Router<App> {
    Router::new()
        .route("/api/v1/flows", get(flows))
        .route("/api/v1/inputs/{input}/route", put(route_input))
}

/// The client for the Query API (the environment's proxy settings apply).
fn client() -> Option<&'static reqwest::Client> {
    static CLIENT: std::sync::OnceLock<Option<reqwest::Client>> = std::sync::OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .timeout(Duration::from_secs(2))
                .build()
                .ok()
        })
        .as_ref()
}

/// The client for the own IS-05 API on the loopback: never through a proxy.
fn loopback() -> Option<&'static reqwest::Client> {
    static CLIENT: std::sync::OnceLock<Option<reqwest::Client>> = std::sync::OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .no_proxy()
                .build()
                .ok()
        })
        .as_ref()
}

/// The `label` of a flow's `flow_def.json` in `domain`.
fn flow_label(domain: &FsPath, flow: Uuid) -> Option<String> {
    let path = crate::mxlio::flow_dir(domain, flow).join("flow_def.json");
    let def: Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    def["label"]
        .as_str()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
}

/// The label of sender `id` from the Query API at `query`.
async fn sender_label(query: &str, id: Uuid) -> Option<String> {
    let url = format!("{query}/x-nmos/query/v1.3/senders/{id}");
    let v: Value = client()?
        .get(url)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json()
        .await
        .ok()?;
    v["label"]
        .as_str()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
}

/// What a lookup was for: the sender and the flow.
type Key = (Option<Uuid>, Option<Uuid>);

/// Keep every input's [`Connected`] up to date until `stop`.
pub async fn resolve_task(app: App, mut stop: tokio::sync::watch::Receiver<bool>) {
    let query = app.node.settings().query.clone();
    // Per input: what was looked up, and when (a miss is tried again).
    let mut done: HashMap<usize, (Key, Option<Instant>)> = HashMap::new();
    let mut tick = tokio::time::interval(TICK);
    loop {
        tokio::select! {
            _ = tick.tick() => {}
            _ = async { let _ = stop.wait_for(|s| *s).await; } => return,
        }
        let routes = app.control().inputs.routes();
        for (i, route) in routes.into_iter().enumerate() {
            let sender = route.as_ref().and_then(|_| {
                app.node
                    .active(Role::Receiver, &receiver_key(i))
                    .and_then(|a| a.peer_id)
            });
            let key = (sender, route.as_ref().map(|r| r.flow));
            let retry = match done.get(&i) {
                Some((k, None)) if *k == key => false,
                Some((k, Some(at))) if *k == key => at.elapsed() >= RETRY,
                _ => true,
            };
            if !retry {
                continue;
            }
            let mut label = None;
            if let (Some(q), Some(id)) = (&query, sender) {
                label = sender_label(q, id).await;
            }
            if label.is_none() {
                if let Some(r) = &route {
                    let (dir, flow) = (r.domain.clone(), r.flow);
                    label = tokio::task::spawn_blocking(move || flow_label(&dir, flow))
                        .await
                        .ok()
                        .flatten();
                }
            }
            let missed = route.is_some() && label.is_none();
            done.insert(i, (key, missed.then(Instant::now)));
            app.control().inputs.set_connected(
                i,
                Connected {
                    sender_id: sender,
                    label,
                },
            );
        }
    }
}

/// Every audio flow in the visible domains (`own`: the mixer's output domain).
fn list_flows(scan: &FsPath, own: &FsPath) -> Vec<Value> {
    let mut out = Vec::new();
    for d in domain::scan(scan, own) {
        let Ok(entries) = std::fs::read_dir(&d.path) else {
            continue;
        };
        let mut flows: Vec<Value> = entries
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                let id: Uuid = name.strip_suffix(".mxl-flow")?.parse().ok()?;
                let text = std::fs::read_to_string(e.path().join("flow_def.json")).ok()?;
                let def: Value = serde_json::from_str(&text).ok()?;
                if def["format"].as_str() != Some("urn:x-nmos:format:audio") {
                    return None;
                }
                Some(json!({
                    "domain_id": d.id,
                    "domain_label": d.label,
                    "own": d.path == own,
                    "flow_id": id,
                    "label": def["label"].as_str().unwrap_or_default(),
                    "channels": def["channel_count"],
                }))
            })
            .collect();
        flows.sort_by(|a, b| a["label"].as_str().cmp(&b["label"].as_str()));
        out.extend(flows);
    }
    out
}

async fn flows(State(app): State<App>) -> Response {
    let (scan, own) = (
        app.config.scan_path.clone(),
        app.config.output_domain_dir.clone(),
    );
    match tokio::task::spawn_blocking(move || list_flows(&scan, &own)).await {
        Ok(list) => Json(json!(list)).into_response(),
        Err(e) => *internal(e.to_string()),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RouteBody {
    flow_id: Option<Uuid>,
    #[serde(default)]
    domain_id: Option<Uuid>,
    #[serde(default)]
    sender_id: Option<Uuid>,
}

/// The IS-05 staged PATCH for a route (`None`: disconnect).
fn staged_patch(route: Option<(Uuid, Uuid)>, sender: Option<Uuid>) -> Value {
    match route {
        Some((domain, flow)) => json!({
            "master_enable": true,
            "sender_id": sender,
            "activation": {"mode": "activate_immediate"},
            "transport_params": [{"mxl_domain_id": domain, "mxl_flow_id": flow}],
        }),
        None => json!({
            "master_enable": false,
            "sender_id": null,
            "activation": {"mode": "activate_immediate"},
        }),
    }
}

async fn route_input(
    State(app): State<App>,
    Path(input): Path<usize>,
    bytes: axum::body::Bytes,
) -> Response {
    let run = async {
        let count = app.control().inputs.len();
        if input == 0 || input > count {
            return Err(not_found(format!("input {input} does not exist")));
        }
        let b: RouteBody = body(&bytes)?;
        let route = match (b.flow_id, b.domain_id) {
            (Some(f), Some(d)) => Some((d, f)),
            (Some(_), None) => return Err(bad("a flow_id needs its domain_id")),
            (None, _) => None,
        };
        let id = app
            .node
            .resource_id(ResourceType::Receiver, &receiver_key(input - 1));
        let url = format!(
            "http://127.0.0.1:{}/x-nmos/connection/v1.2/single/receivers/{id}/staged",
            app.config.nmos_port
        );
        let client = loopback().ok_or_else(|| internal("no HTTP client"))?;
        let resp = client
            .patch(url)
            .json(&staged_patch(route, b.sender_id))
            .send()
            .await
            .map_err(|e| internal(format!("IS-05: {e}")))?;
        let status = resp.status();
        let answer: Value = resp.json().await.unwrap_or_default();
        if !status.is_success() {
            let why = answer["debug"]
                .as_str()
                .or(answer["error"].as_str())
                .unwrap_or("refused")
                .to_string();
            return Err(bad(format!("IS-05 {}: {why}", status.as_u16())));
        }
        Ok(Json(json!(app.control().inputs.status()[input - 1])).into_response())
    };
    run.await.unwrap_or_else(|r| *r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flows_and_their_labels() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().join("cams");
        let flow = Uuid::new_v4();
        let fd = crate::mxlio::flow_dir(&d, flow);
        std::fs::create_dir_all(&fd).unwrap();
        std::fs::write(
            d.join("domain_def.json"),
            format!(r#"{{"id": "{}", "label": "Cameras"}}"#, Uuid::new_v4()),
        )
        .unwrap();
        std::fs::write(
            fd.join("flow_def.json"),
            r#"{"label": " SDI 3 ", "format": "urn:x-nmos:format:audio", "channel_count": 2}"#,
        )
        .unwrap();
        let video = crate::mxlio::flow_dir(&d, Uuid::new_v4());
        std::fs::create_dir_all(&video).unwrap();
        std::fs::write(
            video.join("flow_def.json"),
            r#"{"label": "Cam", "format": "urn:x-nmos:format:video"}"#,
        )
        .unwrap();
        assert_eq!(flow_label(&d, flow).as_deref(), Some("SDI 3"));
        assert_eq!(flow_label(&d, Uuid::new_v4()), None);
        let list = list_flows(dir.path(), &dir.path().join("own"));
        assert_eq!(list.len(), 1, "audio flows only: {list:?}");
        assert_eq!(list[0]["flow_id"], json!(flow));
        assert_eq!(list[0]["domain_label"], "Cameras");
        assert_eq!(list[0]["own"], false);
    }

    #[tokio::test]
    async fn sender_labels_from_the_query_api() {
        let id = Uuid::new_v4();
        let app = Router::new().route(
            "/x-nmos/query/v1.3/senders/{id}",
            get(move |Path(asked): Path<Uuid>| async move {
                if asked == id {
                    Json(json!({"id": id, "label": "SDI 4 audio"})).into_response()
                } else {
                    axum::http::StatusCode::NOT_FOUND.into_response()
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await });
        assert_eq!(
            sender_label(&base, id).await.as_deref(),
            Some("SDI 4 audio")
        );
        assert_eq!(sender_label(&base, Uuid::new_v4()).await, None);
    }

    #[test]
    fn staged_patches() {
        let (d, f, s) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let p = staged_patch(Some((d, f)), Some(s));
        assert_eq!(p["master_enable"], true);
        assert_eq!(p["sender_id"], json!(s));
        assert_eq!(p["transport_params"][0]["mxl_flow_id"], json!(f));
        let off = staged_patch(None, None);
        assert_eq!(off["master_enable"], false);
        assert!(off.get("transport_params").is_none());
    }
}
