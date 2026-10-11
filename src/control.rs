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
use crate::mix::model::{Kind, Mix, Strip, Tap};
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
    }

    /// The meter tick (§7, 25 Hz): meters, loudness, the analyser.
    pub fn meter_tick(&mut self) {
        self.meters.tick();
        let true_peaks = self.meters.true_peaks();
        self.loudness.tick(&true_peaks);
        self.analyser.tick(&mut self.engine.analysis);
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
            .collect()
    }

    /// Check `next` and start applying it. Parameter-only changes ramp over `ramp`
    /// samples and are done; anything else fades the affected nodes out and needs
    /// [`Control::swap`] after [`fade_wait`].
    pub fn stage(&mut self, next: Mix, ramp: u32) -> Result<Staged, String> {
        next.validate(self.input_count, self.block)?;
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
                Some(p) if self.inputs.has_channels(p.input - 1, p.channel) => PatchState::Ok,
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
