//! Widgets for operator screens (§12.6, Appendix B.2), as in the sibling media
//! functions:
//! - `GET /widgets`: the widgets with a JSON schema of their query; CORS only for
//!   the origins listed in `WIDGET_FRAME_ANCESTORS` (`Vary: Origin`), no other CORS;
//! - `GET /widget/<id>?<params>[&theme=dark|light|transparent]`: the UI page, which
//!   shows the widget alone; `Content-Security-Policy: frame-ancestors
//!   <WIDGET_FRAME_ANCESTORS>`, no `X-Frame-Options`. A bad parameter is 400, an
//!   unknown id 404, another method than GET 405.
//!
//! Widgets: `strip`, `fader-bank`, `loudness`, `monitor`, `talkback`, `vca`. The
//! `talkback` widget's browser microphone needs a frame with `allow="microphone"`
//! on an HTTPS page.

use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::mix::Mix;
use crate::ops::{error, App, UI, VERSION};

/// Most strips of a fader bank given as a list.
pub const MAX_BANK: usize = 32;

/// The page of `layout` that `page` names: its 1-based number or its name.
pub fn layout_page<'a>(
    mix: &'a Mix,
    layout: &str,
    page: &str,
) -> Option<&'a crate::mix::show::Page> {
    let l = mix.layouts.iter().find(|l| l.name == layout)?;
    match page.parse::<usize>() {
        Ok(n) if n >= 1 => l.pages.get(n - 1),
        _ => l.pages.iter().find(|p| p.name == page),
    }
}

pub fn routes() -> Router<App> {
    Router::new()
        .route("/widgets", any(list))
        .route("/widget/{id}", any(page))
        .route("/widget/{id}/", any(page))
}

fn not_get(method: &Method) -> Option<Response> {
    (method != Method::GET).then(|| {
        let mut r = error(StatusCode::METHOD_NOT_ALLOWED, "widgets answer GET only");
        r.headers_mut()
            .insert(header::ALLOW, header::HeaderValue::from_static("GET"));
        r
    })
}

/// True when the CSP source list names `origin` (or holds `*`).
pub fn listed_origin(sources: &str, origin: &str) -> bool {
    let normal = |t: &str| t.trim_end_matches('/').to_ascii_lowercase();
    let origin = normal(origin);
    if origin.is_empty() || origin == "null" {
        return false;
    }
    sources
        .split_whitespace()
        .any(|t| t == "*" || normal(t) == origin)
}

fn catalogue(mix: &Mix) -> Value {
    let signals: Vec<&str> = mix
        .strips
        .iter()
        .map(|s| s.id.as_str())
        .chain(mix.buses.iter().map(|b| b.id.as_str()))
        .collect();
    let signal = |title: &str| json!({"type": "string", "title": title, "enum": signals});
    let layouts: Vec<&str> = mix.layouts.iter().map(|l| l.name.as_str()).collect();
    let monitors: Vec<&str> = mix
        .control_room
        .monitors
        .iter()
        .map(|m| m.id.as_str())
        .collect();
    let talkback = &mix.control_room.talkback;
    let destinations: Vec<&str> = talkback
        .destinations
        .iter()
        .map(|d| d.id.as_str())
        .collect();
    let mics: Vec<&str> = talkback.mics.iter().map(|m| m.id.as_str()).collect();
    let vcas: Vec<&str> = mix.vcas.iter().map(|v| v.id.as_str()).collect();
    let pages: Vec<String> = mix
        .layouts
        .iter()
        .flat_map(|l| l.pages.iter().map(|p| p.name.clone()))
        .collect();
    let item = |id: &str, title: &str, params: Value, w: u32, h: u32| json!({"id": id, "title": title, "params": params, "min_size": {"w": w, "h": h}, "version": VERSION});
    json!([
        item(
            "strip",
            "Channel strip",
            json!({"type": "object", "properties": {"strip": signal("Strip or bus")}, "required": ["strip"]}),
            120,
            480
        ),
        item(
            "fader-bank",
            "Fader bank",
            json!({
                "type": "object",
                "properties": {
                    "layout": {"type": "string", "title": "Layout", "enum": layouts},
                    "page": {"type": "string", "title": "Page of the layout (its name or 1-based number)", "examples": pages},
                    "strips": {"type": "string", "title": "Strips and buses, comma-separated (instead of a layout page)"}
                },
                "oneOf": [{"required": ["layout", "page"]}, {"required": ["strips"]}]
            }),
            480,
            480
        ),
        item(
            "loudness",
            "Loudness",
            json!({"type": "object", "properties": {"signal": signal("Strip or bus")}, "required": ["signal"]}),
            320,
            200
        ),
        item(
            "monitor",
            "Monitor",
            json!({"type": "object", "properties": {"monitor": {"type": "string", "title": "Monitor", "enum": monitors}}, "required": ["monitor"]}),
            360,
            200
        ),
        item(
            "talkback",
            "Talkback",
            json!({
                "type": "object",
                "properties": {
                    "destinations": {"type": "string", "title": "Destinations, comma-separated (default: all)", "examples": destinations},
                    "mic": {"type": "string", "title": "Talkback mic (default: the browser's microphone)", "enum": mics}
                }
            }),
            260,
            110
        ),
        item(
            "vca",
            "VCA",
            json!({"type": "object", "properties": {"vca": {"type": "string", "title": "VCA", "enum": vcas}}, "required": ["vca"]}),
            120,
            480
        ),
    ])
}

async fn list(State(app): State<App>, method: Method, headers: HeaderMap) -> Response {
    if let Some(r) = not_get(&method) {
        return r;
    }
    let body = catalogue(&app.control().mix);
    let mut r = Json(body).into_response();
    let origin = headers
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if listed_origin(&app.config.widget_frame_ancestors, origin) {
        if let Ok(v) = header::HeaderValue::from_str(origin) {
            r.headers_mut()
                .insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, v);
        }
    }
    r.headers_mut()
        .insert(header::VARY, header::HeaderValue::from_static("Origin"));
    r
}

/// Check a widget's query against the mix.
pub fn check(mix: &Mix, id: &str, q: &HashMap<String, String>) -> Result<(), (StatusCode, String)> {
    let bad = |m: String| Err((StatusCode::BAD_REQUEST, m));
    if let Some(t) = q.get("theme") {
        if !["dark", "light", "transparent"].contains(&t.as_str()) {
            return bad("theme must be dark, light or transparent".into());
        }
    }
    let signal = |key: &str| -> Result<(), (StatusCode, String)> {
        match q.get(key) {
            Some(s) if mix.kind(s).is_some() => Ok(()),
            Some(s) => bad(format!("{key} {s} is not a strip or bus")),
            None => bad(format!("{key} is required")),
        }
    };
    match id {
        "strip" => signal("strip"),
        "loudness" => signal("signal"),
        "talkback" => {
            let t = &mix.control_room.talkback;
            if let Some(list) = q.get("destinations") {
                let ids: Vec<&str> = list
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .collect();
                if ids.is_empty() {
                    return bad("destinations lists at least one destination".into());
                }
                if let Some(d) = ids
                    .iter()
                    .find(|d| !t.destinations.iter().any(|x| x.id == **d))
                {
                    return bad(format!("talkback destination {d} does not exist"));
                }
            }
            match q.get("mic") {
                Some(m) if !t.mics.iter().any(|x| &x.id == m) => {
                    bad(format!("talkback mic {m} does not exist"))
                }
                _ => Ok(()),
            }
        }
        "vca" => match q.get("vca") {
            Some(v) if mix.vca(v).is_some() => Ok(()),
            Some(v) => bad(format!("VCA {v} does not exist")),
            None => bad("vca is required".into()),
        },
        "monitor" => match q.get("monitor") {
            Some(m) if mix.control_room.monitors.iter().any(|x| &x.id == m) => Ok(()),
            Some(m) => bad(format!("monitor {m} does not exist")),
            None => bad("monitor is required".into()),
        },
        "fader-bank" => {
            if let Some(list) = q.get("strips") {
                let ids: Vec<&str> = list
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .collect();
                if ids.is_empty() || ids.len() > MAX_BANK {
                    return bad(format!("strips lists 1 to {MAX_BANK} strips or buses"));
                }
                if let Some(s) = ids.iter().find(|s| mix.kind(s).is_none()) {
                    return bad(format!("{s} is not a strip or bus"));
                }
                return Ok(());
            }
            match (q.get("layout"), q.get("page")) {
                (Some(l), Some(p)) if layout_page(mix, l, p).is_some() => Ok(()),
                (Some(l), Some(p)) => bad(format!("layout {l} has no page {p}")),
                _ => bad("layout and page, or strips, are required".into()),
            }
        }
        _ => Err((StatusCode::NOT_FOUND, format!("widget {id} does not exist"))),
    }
}

async fn page(
    State(app): State<App>,
    method: Method,
    Path(id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    if let Some(r) = not_get(&method) {
        return r;
    }
    if let Err((status, message)) = check(&app.control().mix, &id, &q) {
        return error(status, message);
    }
    let csp = format!("frame-ancestors {}", app.config.widget_frame_ancestors);
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8".to_string()),
            (header::CACHE_CONTROL, "no-cache".to_string()),
            (header::CONTENT_SECURITY_POLICY, csp),
        ],
        UI,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins() {
        let s = "'self' https://designer.example";
        assert!(listed_origin(s, "https://designer.example"));
        assert!(listed_origin(s, "https://Designer.example/"));
        assert!(!listed_origin(s, "https://evil.example"));
        assert!(!listed_origin(s, "null") && !listed_origin(s, ""));
        assert!(listed_origin("*", "https://any.example"));
    }

    #[test]
    fn params() {
        let mut mix = Mix::default();
        for i in 0..10 {
            mix.strips.push(crate::mix::model::tests::strip(
                &format!("s{i}"),
                crate::mix::Format::Mono,
            ));
        }
        mix.layouts.push(crate::mix::show::Layout {
            name: "Show".into(),
            pages: vec![
                crate::mix::show::Page {
                    name: "Talent".into(),
                    signals: vec!["s1".into(), "s2".into()],
                },
                crate::mix::show::Page {
                    name: "Band".into(),
                    signals: vec!["s3".into()],
                },
            ],
        });
        let q = |pairs: &[(&str, &str)]| {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        let status =
            |id: &str, p: &[(&str, &str)]| check(&mix, id, &q(p)).err().map(|e| e.0.as_u16());
        assert_eq!(status("strip", &[("strip", "s1")]), None);
        assert_eq!(status("strip", &[("strip", "nope")]), Some(400));
        assert_eq!(status("strip", &[]), Some(400));
        assert_eq!(
            status("strip", &[("strip", "s1"), ("theme", "pink")]),
            Some(400)
        );
        assert_eq!(
            status("loudness", &[("signal", "s2"), ("theme", "transparent")]),
            None
        );
        assert_eq!(
            status("fader-bank", &[("layout", "Show"), ("page", "2")]),
            None
        );
        assert_eq!(
            status("fader-bank", &[("layout", "Show"), ("page", "Talent")]),
            None
        );
        assert_eq!(
            status("fader-bank", &[("layout", "Show"), ("page", "3")]),
            Some(400)
        );
        assert_eq!(
            status("fader-bank", &[("layout", "Nope"), ("page", "1")]),
            Some(400)
        );
        assert_eq!(status("fader-bank", &[("layout", "Show")]), Some(400));
        assert_eq!(
            layout_page(&mix, "Show", "Band").map(|p| p.signals.len()),
            Some(1)
        );
        assert_eq!(status("fader-bank", &[("strips", "s1, s3")]), None);
        assert_eq!(status("fader-bank", &[("strips", "s1,x")]), Some(400));
        assert_eq!(status("fader-bank", &[]), Some(400));
        assert_eq!(status("monitor", &[("monitor", "main")]), Some(400));
        assert_eq!(status("vca", &[]), Some(400));
        assert_eq!(status("vca", &[("vca", "band")]), Some(400));
        assert_eq!(status("nope", &[]), Some(404));
        assert_eq!(status("talkback", &[]), None);
        assert_eq!(status("talkback", &[("destinations", "ifb")]), Some(400));
        assert_eq!(status("talkback", &[("mic", "dir")]), Some(400));
    }
}
