//! Published outputs (§4.3): an MXL writer in the own domain and an NMOS Source,
//! Flow and Sender per published signal. The MXL flow id derives from
//! `NMOS_SEED` and the signal id, so downstream readers find the same flow after a
//! restart. Writers stay open across plan swaps; unpublishing releases the writer,
//! which deletes the flow.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mxl_nmos::{
    ids, media_types, ActiveConnection, Channel, Flow, Node, Rational, ResourceType, Role, Sender,
    Source, StagedConnection,
};
use serde::Serialize;
use uuid::Uuid;

use crate::engine::{Command, Engine, OutputWriter, Returned, MAX_OUTPUTS};
use crate::mix::Format;
use crate::mxlio::writer::WriterInstance;
use crate::mxlio::Api;
use crate::nmos::DEVICE_KEY;

/// What a published signal is written as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputSpec {
    pub signal: String,
    pub name: String,
    pub format: Format,
    pub commit: u32,
    /// The slot (1 …) of a virtual output (C7, signal `vout:<key or slot>`); `None`: published.
    pub virtual_slot: Option<usize>,
}

struct Published {
    spec: OutputSpec,
    slot: usize,
    flow: Uuid,
}

/// An output's state for the API and the metrics.
#[derive(Debug, Clone, Serialize)]
pub struct OutputStatus {
    pub signal: String,
    pub name: String,
    pub format: Format,
    pub channels: usize,
    pub commit_samples: u32,
    pub mxl_flow_id: Uuid,
    pub nmos_sender_id: Uuid,
    /// The slot of a virtual output (C7, signal `vout:<key or slot>`); `null`: published.
    pub virtual_output: Option<usize>,
    pub blocks: u64,
    pub commits: u64,
    pub errors: u64,
    pub commit_lag_ms: f64,
    /// Processing latency of the bus (look-ahead, delay compensation).
    pub processing_latency_ms: f64,
    /// Input sample to output sample for local inputs (§4.4): read offset + one
    /// block + processing latency. Mirrored inputs add their extra delay.
    pub latency_ms: f64,
}

pub fn nmos_key(signal: &str) -> String {
    format!("out-{signal}")
}

/// The flow (`mxl`) or NMOS description of an output.
fn describe(spec: &OutputSpec, mxl: bool) -> String {
    let signal = &spec.signal;
    match (spec.virtual_slot, mxl) {
        (Some(n), _) => format!("mxl-audio-mixer virtual output {n}"),
        (None, true) => format!("mxl-audio-mixer output {signal}"),
        (None, false) => format!("MXL audio output {signal}"),
    }
}

pub struct Outputs {
    instance: Arc<WriterInstance>,
    domain_id: Uuid,
    seed: String,
    published: HashMap<String, Published>,
}

impl Outputs {
    pub fn new(api: Api, domain_dir: &Path, domain_id: Uuid, seed: &str) -> Result<Self, String> {
        Ok(Self {
            instance: Arc::new(WriterInstance::new(api, domain_dir)?),
            domain_id,
            seed: seed.to_string(),
            published: HashMap::new(),
        })
    }

    pub fn mxl_flow_id(&self, signal: &str) -> Uuid {
        ids::derive_id(&self.seed, &format!("output/{signal}/mxl-flow"))
    }

    /// Published signal → output slot.
    pub fn slots(&self) -> HashMap<String, usize> {
        self.published
            .iter()
            .map(|(s, p)| (s.clone(), p.slot))
            .collect()
    }

    /// Make the published set equal to `specs`: unpublish what is gone, publish
    /// what is new, re-create what changed its layout or commit size, and update
    /// the NMOS labels of renamed ones.
    pub fn sync(&mut self, engine: &mut Engine, node: &Node, specs: &[OutputSpec]) -> Vec<String> {
        let mut errors = Vec::new();
        let gone: Vec<String> = self
            .published
            .iter()
            .filter(|(s, p)| {
                !specs.iter().any(|o| {
                    &o.signal == *s && o.format == p.spec.format && o.commit == p.spec.commit
                })
            })
            .map(|(s, _)| s.clone())
            .collect();
        let released = gone.len();
        for signal in gone {
            self.unpublish(engine, node, &signal);
        }
        // Writers come back from the DSP thread and are freed (deleting their
        // flows) before a flow id is created again.
        reclaim(engine, released);
        for spec in specs {
            match self.published.get_mut(&spec.signal) {
                Some(p) if p.spec.name != spec.name => {
                    p.spec.name = spec.name.clone();
                    let (flow, spec) = (p.flow, p.spec.clone());
                    if let Err(e) = self.nmos(node, &spec, flow, false) {
                        errors.push(format!("output {}: NMOS: {e}", spec.signal));
                    }
                }
                Some(_) => {}
                None => {
                    if let Err(e) = self.publish(engine, node, spec) {
                        tracing::error!("output {}: {e}", spec.signal);
                        errors.push(format!("output {}: {e}", spec.signal));
                    }
                }
            }
        }
        errors
    }

    fn publish(
        &mut self,
        engine: &mut Engine,
        node: &Node,
        spec: &OutputSpec,
    ) -> Result<(), String> {
        let used: Vec<usize> = self.published.values().map(|p| p.slot).collect();
        let slot = (0..MAX_OUTPUTS)
            .find(|s| !used.contains(s))
            .ok_or("no free output slot")?;
        let flow = self.mxl_flow_id(&spec.signal);
        let key = nmos_key(&spec.signal);
        let def = serde_json::json!({
            "id": flow.to_string(),
            "source_id": node.resource_id(ResourceType::Source, &key).to_string(),
            "device_id": node.resource_id(ResourceType::Device, DEVICE_KEY).to_string(),
            "label": spec.name,
            "description": describe(spec, true),
            "format": "urn:x-nmos:format:audio",
            "tags": {"urn:x-nmos:tag:grouphint/v1.0": [mxl_nmos::group_hint(&spec.name, "Audio")]},
            "version": "0:0",
            "parents": [],
            "media_type": "audio/float32",
            "sample_rate": {"numerator": 48000, "denominator": 1},
            "channel_count": spec.format.channels(),
            "bit_depth": 32
        });
        let writer = self
            .instance
            .create_samples_writer(&def.to_string(), spec.commit)?;
        if writer.channels != spec.format.channels() {
            return Err(format!(
                "flow {flow} exists with {} channels, not {}",
                writer.channels,
                spec.format.channels()
            ));
        }
        engine.send(Command::SetOutput {
            slot,
            output: Box::new(OutputWriter::new(writer, spec.commit as usize)),
        });
        self.nmos(node, spec, flow, true)
            .map_err(|e| format!("NMOS: {e}"))?;
        tracing::info!(output = %spec.signal, %flow, channels = spec.format.channels(), commit = spec.commit, "output published");
        self.published.insert(
            spec.signal.clone(),
            Published {
                spec: spec.clone(),
                slot,
                flow,
            },
        );
        Ok(())
    }

    fn nmos(
        &self,
        node: &Node,
        spec: &OutputSpec,
        flow: Uuid,
        first: bool,
    ) -> Result<(), mxl_nmos::Error> {
        let key = nmos_key(&spec.signal);
        let channels = spec
            .format
            .labels()
            .iter()
            .map(|l| Channel {
                label: l.to_string(),
                symbol: Format::symbol(l).map(str::to_string),
            })
            .collect();
        let description = describe(spec, false);
        let mut source = Source::audio(&key, DEVICE_KEY, channels);
        source.label = spec.name.clone();
        source.description = description.clone();
        mxl_nmos::add_group_hint(&mut source.tags, &spec.name, "Audio");
        node.insert_source(source)?;
        let mut f = Flow::audio(
            &key,
            &key,
            DEVICE_KEY,
            media_types::AUDIO_FLOAT32,
            Rational::new(48_000, 1),
            Some(32),
        );
        f.label = spec.name.clone();
        f.description = description.clone();
        mxl_nmos::add_group_hint(&mut f.tags, &spec.name, "Audio");
        node.insert_flow(f)?;
        let mut sender = Sender::mxl(&key, &key, DEVICE_KEY);
        sender.label = spec.name.clone();
        sender.description = description;
        sender.mxl_domain_id = Some(self.domain_id);
        sender.mxl_flow_id = Some(flow);
        mxl_nmos::add_group_hint(&mut sender.tags, &spec.name, "Audio");
        node.insert_sender(sender)?;
        if first {
            node.set_staged(
                Role::Sender,
                &key,
                StagedConnection {
                    master_enable: true,
                    peer_id: None,
                    mxl_domain_id: Some(self.domain_id).into(),
                    mxl_flow_id: Some(flow).into(),
                },
            )?;
            node.set_active(
                Role::Sender,
                &key,
                ActiveConnection {
                    master_enable: true,
                    peer_id: None,
                    mxl_domain_id: Some(self.domain_id),
                    mxl_flow_id: Some(flow),
                },
            )?;
        }
        Ok(())
    }

    fn unpublish(&mut self, engine: &mut Engine, node: &Node, signal: &str) {
        let Some(p) = self.published.remove(signal) else {
            return;
        };
        engine.send(Command::ClearOutput { slot: p.slot });
        if let Err(e) = node.erase(ResourceType::Source, &nmos_key(signal)) {
            tracing::warn!(output = signal, "NMOS erase: {e}");
        }
        tracing::info!(output = signal, flow = %p.flow, "output unpublished");
    }

    pub fn status(&self, engine: &Engine, node: &Node) -> Vec<OutputStatus> {
        let mut out: Vec<OutputStatus> = self
            .published
            .values()
            .map(|p| {
                let c = &engine.counters.outputs[p.slot];
                use std::sync::atomic::Ordering::Relaxed;
                OutputStatus {
                    signal: p.spec.signal.clone(),
                    name: p.spec.name.clone(),
                    format: p.spec.format,
                    channels: p.spec.format.channels(),
                    commit_samples: p.spec.commit,
                    mxl_flow_id: p.flow,
                    nmos_sender_id: node
                        .resource_id(ResourceType::Sender, &nmos_key(&p.spec.signal)),
                    virtual_output: p.spec.virtual_slot,
                    blocks: c.blocks.load(Relaxed),
                    commits: c.commits.load(Relaxed),
                    errors: c.errors.load(Relaxed),
                    commit_lag_ms: c.commit_lag_us.load(Relaxed) as f64 / 1000.0,
                    processing_latency_ms: 0.0,
                    latency_ms: 0.0,
                }
            })
            .collect();
        out.sort_by(|a, b| a.signal.cmp(&b.signal));
        out
    }
}

/// Wait until the DSP thread has handed back `writers` released writers (at most
/// 200 ms) and free them.
fn reclaim(engine: &mut Engine, mut writers: usize) {
    let until = Instant::now() + Duration::from_millis(200);
    while writers > 0 && Instant::now() < until {
        match engine.returned.pop() {
            Ok(item) => {
                if matches!(item, Returned::Output(_)) {
                    writers -= 1;
                }
                drop(item);
            }
            Err(_) => std::thread::sleep(Duration::from_millis(1)),
        }
    }
}
