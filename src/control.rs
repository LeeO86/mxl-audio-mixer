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
    pub mix: Mix,
    node: mxl_nmos::Node,
    compiler: Compiler,
    layout: HashMap<NodeId, LayoutNode>,
    structure: Option<u64>,
    pending: Option<Mix>,
    block: u32,
    input_count: usize,
    default_commit: u32,
    mix_path: PathBuf,
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
            mix: Mix::default(),
            node,
            compiler: Compiler::default(),
            layout: HashMap::new(),
            structure: None,
            pending: None,
            block: config.block_samples,
            input_count: config.input_flows,
            default_commit: config.output_commit_samples,
            mix_path: config.state_dir.join("mix.json"),
            compile_seconds: 0.0,
            step_counts: BTreeMap::new(),
        }
    }

    /// The 100 ms tick: free what the DSP thread returned, run the inputs.
    pub fn tick(&mut self) {
        self.engine.collect();
        self.inputs.tick(&mut self.engine);
    }

    fn specs(&self, mix: &Mix) -> Vec<OutputSpec> {
        mix.outputs
            .iter()
            .filter_map(|o| {
                let (name, _) = mix.signal(&o.signal)?;
                let format = match o.format {
                    Some(f) => f,
                    None => mix.tap_format(&o.signal, Tap::PostFader)?,
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
        );
        let same_outputs = self
            .specs(&next)
            .iter()
            .map(|s| (&s.signal, s.format, s.commit))
            .eq(self
                .specs(&self.mix)
                .iter()
                .map(|s| (&s.signal, s.format, s.commit)));
        if self.structure == Some(compiled.structure) && same_outputs {
            for (id, n) in &compiled.nodes {
                if self.layout.get(id).map(|o| &o.values) != Some(&n.values) {
                    self.engine.param(*id, &n.values, ramp);
                }
            }
            self.layout = compiled.nodes;
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
        let started = Instant::now();
        let mut compiled = self
            .compiler
            .compile(&next, self.block as usize, FADE_SAMPLES, &slots);
        self.compile_seconds = started.elapsed().as_secs_f64();
        compiled.plan.carry = compiled
            .nodes
            .iter()
            .filter_map(|(id, n)| {
                let old = self.layout.get(id)?;
                (old.signature == n.signature).then_some((n.index, old.index))
            })
            .collect();
        self.step_counts = BTreeMap::new();
        for step in &compiled.plan.steps {
            *self.step_counts.entry(step.kind()).or_default() += 1;
        }
        self.meters
            .set_plan(compiled.plan.meters.clone(), compiled.meters.clone());
        self.engine.send(Command::SetPlan(Box::new(compiled.plan)));
        self.layout = compiled.nodes;
        self.structure = Some(compiled.structure);
        self.mix = next;
        self.save()?;
        Ok(errors)
    }

    fn save(&self) -> Result<(), String> {
        let body = serde_json::json!({"version": 1, "mix": self.mix});
        crate::state::write_atomic(
            &self.mix_path,
            &serde_json::to_vec_pretty(&body).unwrap_or_default(),
        )
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
