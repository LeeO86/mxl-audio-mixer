//! The NMOS node (`mxl-nmos`, §3.1): one Device and `MIXER_INPUT_FLOWS` audio
//! receivers. The senders of published outputs are in [`crate::outputs`].

use std::path::PathBuf;

use mxl_nmos::{
    media_types, Device, Format, MxlDomain, Node, NodeImplementation, Receiver, ReceiverCaps, Role,
};
use serde_json::json;
use tokio::sync::mpsc::UnboundedSender;

use crate::config::Config;
use crate::inputs::{InputConfig, Route};
use crate::mxlio::domain;

pub const DEVICE_KEY: &str = "mixer";
pub const DEVICE_NAME: &str = "Audio Mixer";

/// A receiver activation for the input manager.
pub struct RouteRequest {
    pub index: usize,
    pub route: Option<Route>,
}

pub fn receiver_key(index: usize) -> String {
    format!("in-{}", index + 1)
}

pub fn input_index(key: &str) -> Option<usize> {
    key.strip_prefix("in-")?
        .parse::<usize>()
        .ok()?
        .checked_sub(1)
}

/// The receiver of an input (BCP-004-01 caps: float32, 1–64 channels, 48 kHz).
pub fn receiver(input: &InputConfig) -> Receiver {
    let mut r = Receiver::mxl(
        receiver_key(input.input - 1),
        DEVICE_KEY,
        Format::Audio,
        vec![media_types::AUDIO_FLOAT32.into()],
    );
    r.label = input.label.clone();
    r.description = format!("MXL audio input {}", input.input);
    r.caps = ReceiverCaps {
        media_types: vec![media_types::AUDIO_FLOAT32.into()],
        constraint_sets: vec![json!({
            "urn:x-nmos:cap:format:channel_count": {"minimum": 1, "maximum": 64},
            "urn:x-nmos:cap:format:sample_rate": {"enum": [{"numerator": 48000, "denominator": 1}]},
            "urn:x-nmos:cap:format:sample_depth": {"enum": [32]}
        })],
        version: None,
    };
    mxl_nmos::add_group_hint(&mut r.tags, &input.label, "Audio");
    r
}

/// Build the node. Activations of receivers go to `routes`; the callback accepts
/// at once and never waits for media (§3.1).
pub fn build(
    config: &Config,
    inputs: &[InputConfig],
    routes: UnboundedSender<RouteRequest>,
    connections_path: PathBuf,
) -> Result<Node, mxl_nmos::Error> {
    let scan = config.scan_path.clone();
    let own = config.output_domain_dir.clone();
    let implementation = NodeImplementation::new(move |activation| {
        let routes = routes.clone();
        Box::pin(async move {
            if activation.role == Role::Receiver {
                if let Some(index) = input_index(&activation.key) {
                    let route = match (
                        activation.master_enable,
                        activation.domain_path,
                        activation.mxl_flow_id,
                    ) {
                        (true, Some(domain), Some(flow)) => Some(Route { domain, flow }),
                        _ => None,
                    };
                    let _ = routes.send(RouteRequest { index, route });
                }
            }
            Ok(())
        })
    })
    .domains(move || {
        domain::scan(&scan, &own)
            .into_iter()
            .map(|d| MxlDomain {
                id: d.id,
                label: d.label,
                path: d.path,
            })
            .collect()
    })
    .on_connections_changed(move |all| {
        if let Err(err) = crate::state::save_connections(&connections_path, &all) {
            tracing::error!("saving the IS-05 connections failed: {err}");
        }
    });
    let node = Node::new(config.nmos_settings(), implementation)?;
    node.insert_device(Device::new(DEVICE_KEY, DEVICE_NAME).description("MXL audio mixer"))?;
    for input in inputs {
        node.insert_receiver(receiver(input))?;
    }
    Ok(node)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys() {
        assert_eq!(receiver_key(0), "in-1");
        assert_eq!(input_index("in-16"), Some(15));
        assert_eq!(input_index("in-0"), None);
        assert_eq!(input_index("out-1"), None);
    }
}
