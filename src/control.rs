//! The control side (§2): owns the engine handle, the inputs, the mix and the
//! published outputs. A change of the mix is compiled; when only parameter values
//! changed they go to the DSP thread as ramped messages, otherwise the nodes whose
//! wiring changes are faded out, and then the new plan is swapped in
//! ([`Control::stage`], wait [`FADE_WAIT`], [`Control::swap`]).

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::config::Config;
use crate::engine::{Command, Engine, NodeId, FADE_SAMPLES};
use crate::inputs::Inputs;
use crate::loudness::LoudnessMeters;
use crate::meters::Meters;
use crate::mix::compile::{Compiler, LayoutNode};
use crate::mix::model::{Kind, Mix, Patch, Strip, Tap};
use crate::outputs::{OutputSpec, Outputs};

/// Time between fading out the nodes a change removes or rewires and the swap:
/// the fade (10 ms) plus a few blocks for the message to arrive.
pub fn fade_wait(block: u32) -> Duration {
    Duration::from_micros(u64::from(FADE_SAMPLES + 3 * block) * 1_000_000 / 48_000)
}

/// What [`Control::stage`] left to do.
#[derive(Debug, PartialEq, Eq)]
pub enum Staged {
    /// Only values changed; they were sent.
    Done,
    /// Call [`Control::swap`], after [`fade_wait`] when `wait` (nodes are fading out).
    Swap { wait: bool },
}

/// A strip channel's patch state (§4.2: missing channels are flagged).
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PatchState {
    Unpatched,
    Ok,
    Missing,
}

pub struct Control {
    pub engine: Engine,
    pub inputs: Inputs,
    pub outputs: Outputs,
    pub meters: Meters,
    pub loudness: LoudnessMeters,
    pub mix: Mix,
    node: mxl_nmos::Node,
    compiler: Compiler,
    pub analyser: crate::analysis::Analyser,
    pub tally: crate::tally::Tally,
    /// Monitors heard in the browser (§10.1, B.1).
    pub streams: crate::preview::Streams,
    /// Most personal monitors.
    max_personal: usize,
    layout: HashMap<NodeId, LayoutNode>,
    procs: HashMap<NodeId, LayoutNode>,
    /// Processing latency of each signal's post-fader point, in samples.
    pub latency: HashMap<String, usize>,
    /// Counts changes of the mix; clients reload it when it moves.
    pub revision: u64,
    structure: Option<u64>,
    pending: Option<Mix>,
    block: u32,
    read_offset: u32,
    input_count: usize,
    default_commit: u32,
    mix_path: PathBuf,
    pub history: crate::history::History,
    /// Duration of the last plan compile (§15 `plan_compile_seconds`).
    pub compile_seconds: f64,
    /// Steps of the running plan by type (§15 `nodes{type}`).
    pub step_counts: BTreeMap<&'static str, usize>,
}

impl Control {
    pub fn new(
        engine: Engine,
        inputs: Inputs,
        outputs: Outputs,
        node: mxl_nmos::Node,
        config: &Config,
    ) -> Self {
        Self {
            engine,
            inputs,
            outputs,
            meters: Meters::default(),
            loudness: LoudnessMeters::default(),
            mix: Mix::default(),
            node,
            compiler: Compiler::default(),
            analyser: crate::analysis::Analyser::new(config.block_samples as usize),
            tally: crate::tally::Tally::default(),
            streams: crate::preview::Streams::new(config),
            max_personal: config.max_web_monitors,
            layout: HashMap::new(),
            procs: HashMap::new(),
            latency: HashMap::new(),
            revision: 0,
            structure: None,
            pending: None,
            block: config.block_samples,
            read_offset: config.read_offset_samples,
            input_count: config.input_flows,
            default_commit: config.output_commit_samples,
            mix_path: config.state_dir.join("mix.json"),
            history: crate::history::History::new(&config.state_dir),
            compile_seconds: 0.0,
            step_counts: BTreeMap::new(),
        }
    }

    /// The 100 ms tick: free what the DSP thread returned, run the inputs.
    pub fn tick(&mut self) {
        self.engine.collect();
        self.inputs.tick(&mut self.engine);
        self.history.tick(&self.mix);
    }

    /// The meter tick (§7, 25 Hz): meters, loudness, the analyser.
    pub fn meter_tick(&mut self) {
        self.meters.tick();
        let true_peaks = self.meters.true_peaks();
        self.loudness.tick(&true_peaks);
        self.analyser.tick(&mut self.engine.analysis);
    }

    /// The AFV gain of every strip with AFV on: 1 while its tally index is on air,
    /// its depth otherwise (also without a tally index or before any tally).
    fn afv_targets(&self, mix: &Mix) -> HashMap<String, f32> {
        mix.strips
            .iter()
            .filter_map(|s| {
                let a = s.afv.filter(|a| a.enabled)?;
                let on = s.tally_index.is_some_and(|i| self.tally.on_air(i));
                Some((
                    s.id.clone(),
                    if on { 1.0 } else { a.depth_db.gain() as f32 },
                ))
            })
            .collect()
    }

    /// One TSL packet; AFV strips whose tally changed fade open or closed.
    pub fn tally_packet(&mut self, config: &Config, packet: &[u8]) {
        if self.tally.packet(config, packet) {
            self.apply_afv();
        }
    }

    fn apply_afv(&mut self) {
        for (id, g) in self.afv_targets(&self.mix) {
            if self.compiler.afv.get(&id) == Some(&g) {
                continue;
            }
            self.compiler.afv.insert(id.clone(), g);
            let Some(a) = self.mix.strip(&id).and_then(|s| s.afv) else {
                continue;
            };
            let Some(node) = self.compiler.node_id(&format!("strip/{id}/afv")) else {
                continue;
            };
            let Some(l) = self.layout.get_mut(&node) else {
                continue;
            };
            let ms = if g >= 1.0 {
                a.fade_in_ms
            } else {
                a.fade_out_ms
            };
            let values = vec![g; l.values.len()];
            self.engine.param(node, &values, (ms * 48.0).round() as u32);
            l.values = values;
            tracing::info!(strip = %id, open = g >= 1.0, "afv");
        }
    }

    /// Per AFV strip: its tally index, on air, preview, open.
    pub fn tally_status(&self) -> serde_json::Value {
        let strips: serde_json::Map<String, serde_json::Value> = self
            .mix
            .strips
            .iter()
            .filter(|s| s.tally_index.is_some())
            .map(|s| {
                let i = s.tally_index.unwrap_or_default();
                (
                    s.id.clone(),
                    serde_json::json!({
                        "index": i,
                        "on_air": self.tally.on_air(i),
                        "preview": self.tally.preview(i),
                        "afv": s.afv.is_some_and(|a| a.enabled),
                        "afv_open": self.compiler.afv.get(&s.id).is_some_and(|g| *g >= 1.0),
                    }),
                )
            })
            .collect();
        serde_json::json!({
            "packets": self.tally.packets,
            "errors": self.tally.errors,
            "last_packet_age_s": self.tally.last_packet_age_s(),
            "strips": strips,
        })
    }

    /// Analyse `target` (signal and tap) or nothing; applied with the next
    /// change of the mix (`api::apply` with no change).
    pub fn set_analysis(&mut self, target: Option<(String, crate::mix::model::Tap)>) {
        self.compiler.analysis = target.clone();
        self.analyser.set_target(target);
    }

    fn specs(&self, mix: &Mix) -> Vec<OutputSpec> {
        mix.outputs
            .iter()
            .filter_map(|o| {
                let (name, _) = mix.signal(&o.signal)?;
                let format = match o.format {
                    Some(f) => f,
                    None => mix.tap_format(&o.signal, &Tap::PostFader)?,
                };
                Some(OutputSpec {
                    signal: o.signal.clone(),
                    name: name.to_string(),
                    format,
                    commit: o.commit_samples.unwrap_or(self.default_commit),
                })
            })
            .chain(
                mix.control_room
                    .monitors
                    .iter()
                    .filter(|m| m.publish)
                    .map(|m| OutputSpec {
                        signal: m.key(),
                        name: format!("Monitor {}", m.name),
                        format: m.format,
                        commit: self.default_commit,
                    }),
            )
            .collect()
    }

    /// Solo `signal` (PFL or AFL), or end its solo; interlock replaces the others.
    /// Applied with the next change of the mix (`api::apply` with no change).
    pub fn set_solo(&mut self, signal: &str, kind: Option<crate::mix::room::SoloKind>) {
        match kind {
            Some(k) => {
                if self.mix.control_room.solo.mode == crate::mix::room::SoloMode::Interlock {
                    self.compiler.solo.clear();
                }
                self.compiler.solo.insert(signal.to_string(), k);
            }
            None => {
                self.compiler.solo.remove(signal);
            }
        }
    }

    pub fn clear_solo(&mut self) {
        self.compiler.solo.clear();
    }

    pub fn solo(&self) -> &BTreeMap<String, crate::mix::room::SoloKind> {
        &self.compiler.solo
    }

    /// The personal monitors (runtime).
    pub fn personal(&self) -> &[crate::mix::compile::Personal] {
        &self.compiler.personal
    }

    /// Add a personal monitor on `source`; applied with the next change of the mix.
    pub fn add_personal(
        &mut self,
        source: Option<crate::mix::room::Source>,
    ) -> Result<String, String> {
        if self.compiler.personal.len() >= self.max_personal {
            return Err(format!(
                "at most {} personal monitors (MIXER_MAX_WEB_MONITORS)",
                self.max_personal
            ));
        }
        let id = (1..)
            .map(|n| format!("personal-{n}"))
            .find(|id| self.compiler.personal.iter().all(|p| &p.monitor.id != id))
            .expect("unbounded");
        let mut monitor: crate::mix::room::Monitor = serde_json::from_value(serde_json::json!({
            "id": id, "name": id, "kind": "headphones", "format": "stereo"
        }))
        .map_err(|e| e.to_string())?;
        monitor.source = source;
        self.compiler.personal.push(crate::mix::compile::Personal {
            monitor,
            solo: BTreeMap::new(),
            created: Instant::now(),
        });
        Ok(id)
    }

    /// Change personal monitor `id` with `f`.
    pub fn with_personal<T>(
        &mut self,
        id: &str,
        f: impl FnOnce(&mut crate::mix::compile::Personal) -> T,
    ) -> Option<T> {
        self.compiler
            .personal
            .iter_mut()
            .find(|p| p.monitor.id == id)
            .map(f)
    }

    pub fn remove_personal(&mut self, id: &str) -> bool {
        let before = self.compiler.personal.len();
        self.compiler.personal.retain(|p| p.monitor.id != id);
        self.compiler.personal.len() != before
    }

    /// Remove the personal monitors nobody listened to for `idle` (counted from
    /// their creation when never). True when any went.
    pub fn expire_personal(&mut self, idle: Duration) -> bool {
        let streams = &self.streams;
        let before = self.compiler.personal.len();
        self.compiler.personal.retain(|p| {
            let last = streams
                .last_heard(&p.monitor.id)
                .unwrap_or(p.created)
                .max(p.created);
            last.elapsed() < idle
        });
        self.compiler.personal.len() != before
    }

    /// A monitor or personal monitor with this id exists.
    pub fn has_monitor(&self, id: &str) -> bool {
        self.mix.control_room.monitors.iter().any(|m| m.id == id)
            || self.compiler.personal.iter().any(|p| p.monitor.id == id)
    }

    /// Every stream id: the monitors, then the personal monitors.
    pub fn stream_ids(&self) -> Vec<String> {
        self.mix
            .control_room
            .monitors
            .iter()
            .map(|m| m.id.clone())
            .chain(self.compiler.personal.iter().map(|p| p.monitor.id.clone()))
            .collect()
    }

    /// Start streaming `id` (someone listens). True when the plan needs it.
    pub fn listen(&mut self, id: &str) -> Result<bool, String> {
        if !self.has_monitor(id) {
            return Err(format!("monitor {id} does not exist"));
        }
        let c = &mut *self;
        c.streams.listen(&mut c.engine, id)
    }

    /// Stop unheard streams; true when the plan needs a change.
    pub fn expire_streams(&mut self) -> bool {
        let ids = self.stream_ids();
        let c = &mut *self;
        c.streams
            .expire(&mut c.engine, |id| ids.iter().any(|x| x == id))
    }

    /// Check `next` and start applying it. Parameter-only changes ramp over `ramp`
    /// samples and are done; anything else fades the affected nodes out and needs
    /// [`Control::swap`] after [`fade_wait`].
    pub fn stage(&mut self, next: Mix, ramp: u32) -> Result<Staged, String> {
        next.validate(self.input_count, self.block)?;
        self.compiler.afv = self.afv_targets(&next);
        self.compiler.solo.retain(|s, _| next.kind(s).is_some());
        self.compiler.streams = self.streams.slots();
        for p in &mut self.compiler.personal {
            p.solo.retain(|s, _| next.kind(s).is_some());
            if let Some(crate::mix::room::Source::Signal(id)) = &p.monitor.source {
                if next.kind(id).is_none() {
                    p.monitor.source = None;
                }
            }
        }
        let compiled = self.compiler.compile(
            &next,
            self.block as usize,
            FADE_SAMPLES,
            &self.outputs.slots(),
            &self.loudness.slots(),
        );
        let same_outputs = self
            .specs(&next)
            .iter()
            .map(|s| (&s.signal, s.format, s.commit))
            .eq(self
                .specs(&self.mix)
                .iter()
                .map(|s| (&s.signal, s.format, s.commit)));
        let same_loudness = LoudnessMeters::wanted(&next) == self.loudness.measured();
        if self.structure == Some(compiled.structure) && same_outputs && same_loudness {
            for (id, n) in &compiled.nodes {
                if self.layout.get(id).map(|o| &o.values) != Some(&n.values) {
                    self.engine.param(*id, &n.values, ramp);
                }
            }
            for (id, p) in &compiled.procs {
                let Some(old) = self.procs.get(id) else {
                    continue;
                };
                for (i, (v, o)) in p.values.iter().zip(old.values.iter()).enumerate() {
                    if v != o {
                        self.engine.proc_param(*id, i, *v, ramp);
                    }
                }
            }
            self.layout = compiled.nodes;
            self.procs = compiled.procs;
            let renamed = self.specs(&next) != self.specs(&self.mix);
            self.mix = next;
            if renamed {
                let specs = self.specs(&self.mix);
                self.outputs.sync(&mut self.engine, &self.node, &specs);
            }
            self.save()?;
            return Ok(Staged::Done);
        }
        let mut wait = false;
        for (id, old) in &self.layout {
            let keeps = compiled
                .nodes
                .get(id)
                .is_some_and(|n| n.signature == old.signature);
            if !keeps {
                self.engine
                    .param(*id, &vec![0.0; old.values.len()], FADE_SAMPLES);
                wait = true;
            }
        }
        self.pending = Some(next);
        Ok(Staged::Swap { wait })
    }

    /// Publish and unpublish outputs and swap in the plan of the staged mix.
    /// Returns publishing errors (the mix is applied without those outputs).
    pub fn swap(&mut self) -> Result<Vec<String>, String> {
        let Some(mut next) = self.pending.take() else {
            return Ok(Vec::new());
        };
        let specs = self.specs(&next);
        let errors = self.outputs.sync(&mut self.engine, &self.node, &specs);
        let slots = self.outputs.slots();
        next.outputs.retain(|o| slots.contains_key(&o.signal));
        self.loudness.sync(&mut self.engine, &next);
        let started = Instant::now();
        let mut compiled = self.compiler.compile(
            &next,
            self.block as usize,
            FADE_SAMPLES,
            &slots,
            &self.loudness.slots(),
        );
        self.compile_seconds = started.elapsed().as_secs_f64();
        let carry = |new: &HashMap<NodeId, LayoutNode>, old: &HashMap<NodeId, LayoutNode>| {
            new.iter()
                .filter_map(|(id, n)| {
                    let o = old.get(id)?;
                    (o.signature == n.signature).then_some((n.index, o.index))
                })
                .collect()
        };
        compiled.plan.carry = carry(&compiled.nodes, &self.layout);
        compiled.plan.proc_carry = carry(&compiled.procs, &self.procs);
        self.step_counts = BTreeMap::new();
        for step in &compiled.plan.steps {
            *self.step_counts.entry(step.kind()).or_default() += 1;
        }
        self.meters
            .set_plan(compiled.plan.meters.clone(), compiled.meters.clone());
        self.meters.set_nodes(
            compiled.plan.node_meters.clone(),
            compiled.node_meters.clone(),
        );
        self.engine.send(Command::SetPlan(Box::new(compiled.plan)));
        self.layout = compiled.nodes;
        self.procs = compiled.procs;
        self.latency = compiled.latency;
        self.structure = Some(compiled.structure);
        self.mix = next;
        self.save()?;
        Ok(errors)
    }

    fn save(&mut self) -> Result<(), String> {
        self.revision += 1;
        let body = serde_json::json!({"version": 1, "mix": self.mix});
        crate::state::write_atomic(
            &self.mix_path,
            &serde_json::to_vec_pretty(&body).unwrap_or_default(),
        )?;
        self.history.changed(&self.mix);
        Ok(())
    }

    /// The patch state of each channel of `strip`.
    pub fn patch_state(&self, strip: &Strip) -> Vec<PatchState> {
        strip
            .patch
            .iter()
            .map(|p| match p {
                None => PatchState::Unpatched,
                Some(Patch::Input(p)) if self.inputs.has_channels(p.input - 1, p.channel) => {
                    PatchState::Ok
                }
                Some(Patch::Talkback(t))
                    if self.mix.control_room.talkback.mics.iter().any(|m| {
                        m.id == t.talkback && self.inputs.has_channels(m.input - 1, m.channel)
                    }) || self.compiler.browser_mics.contains_key(&t.talkback) =>
                {
                    PatchState::Ok
                }
                Some(_) => PatchState::Missing,
            })
            .collect()
    }

    /// The published outputs with their latency.
    pub fn output_status(&self) -> Vec<crate::outputs::OutputStatus> {
        let mut status = self.outputs.status(&self.engine, &self.node);
        for o in &mut status {
            let processing = self.latency.get(&o.signal).copied().unwrap_or(0) as f64 / 48.0;
            o.processing_latency_ms = processing;
            o.latency_ms = f64::from(self.read_offset + self.block) / 48.0 + processing;
        }
        status
    }

    pub fn signals(&self) -> usize {
        self.mix.strips.len() + self.mix.buses.len()
    }

    pub fn is_bus(&self, id: &str) -> bool {
        self.mix.kind(id) == Some(Kind::Bus)
    }

    /// Stop the DSP thread (fading the outputs), then release readers and writers (G8).
    pub fn stop(&mut self) {
        self.engine.stop();
        self.inputs.release();
    }
}

/// The mix saved under `MIXER_STATE_DIR`. A file that cannot be read is moved
/// aside (`mix.json.bad`) so it is not overwritten.
pub fn load_mix(path: &std::path::Path) -> Mix {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Mix::default();
    };
    let parsed = serde_json::from_str::<serde_json::Value>(&text)
        .map_err(|e| e.to_string())
        .and_then(|v| serde_json::from_value::<Mix>(v["mix"].clone()).map_err(|e| e.to_string()));
    match parsed {
        Ok(mix) => mix,
        Err(e) => {
            let bad = path.with_extension("json.bad");
            tracing::error!(
                "{} is not a valid mix ({e}); moved to {} and starting with an empty mix",
                path.display(),
                bad.display()
            );
            let _ = std::fs::rename(path, &bad);
            Mix::default()
        }
    }
}
