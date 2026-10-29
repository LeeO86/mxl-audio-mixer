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

/// A change of parameter values is written to `mix.json` at most this often,
/// this long after it (D168). Plan changes, shutdown and the operator's
/// explicit show actions write at once.
pub const SAVE_DELAY: Duration = Duration::from_millis(500);

/// `MIXER_TEST_LOGIC_SWAP_DELAY_MS` (test only, at most 5 s): a logic swap
/// waits this much longer, so `tests/integration/logic.sh` can put an API change
/// inside the window. Warned at start when set.
fn test_swap_delay() -> Duration {
    let ms = std::env::var("MIXER_TEST_LOGIC_SWAP_DELAY_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0)
        .min(5_000);
    if ms > 0 {
        tracing::warn!(
            "MIXER_TEST_LOGIC_SWAP_DELAY_MS={ms}: logic plan changes are delayed by {ms} ms; for tests only, never in production"
        );
    }
    Duration::from_millis(ms)
}

/// Logic webhook posts running at once.
pub const MAX_WEBHOOKS: usize = 8;

/// A momentary talk not repeated within this long ends (a client that went away).
pub const MOMENTARY_HOLD: std::time::Duration = std::time::Duration::from_secs(3);

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
    /// Browser microphones (talkback, §10.2).
    pub talkers: crate::preview::talkback::Talkers,
    /// Momentary talk (mic, destination): released unless repeated within
    /// [`MOMENTARY_HOLD`].
    momentary: BTreeMap<(String, String), std::time::Instant>,
    layout: HashMap<NodeId, LayoutNode>,
    procs: HashMap<NodeId, LayoutNode>,
    /// Processing latency of each signal's post-fader point, in samples.
    pub latency: HashMap<String, usize>,
    /// Counts changes of the mix; clients reload it when it moves.
    pub revision: u64,
    structure: Option<u64>,
    pending: Option<Mix>,
    /// When a logic change faded nodes out, swap the plan at this time.
    logic_swap_at: Option<Instant>,
    /// The API's change lock (`App::changes`): a logic change that rewires the
    /// plan takes it too, and holds it until its swap (D167).
    pub changes: std::sync::Arc<tokio::sync::Mutex<()>>,
    logic_guard: Option<tokio::sync::OwnedMutexGuard<()>>,
    /// Test only (`MIXER_TEST_LOGIC_SWAP_DELAY_MS`): a logic swap waits this
    /// much longer after its fade, so a test can put an API change inside the
    /// window (`tests/integration/logic.sh`). 0 otherwise.
    logic_swap_delay: Duration,
    logic_waiting: crate::logic::Waiting,
    /// Logic mutes, faders, VCAs and parameters waiting for the change lock
    /// (the newest per block).
    logic_values: Vec<crate::logic::Action>,
    /// Logic blocks whose rewiring change failed (warned once).
    logic_failing: std::collections::BTreeSet<(String, String)>,
    /// The logic last found valid, with what it named (D171).
    logic_valid: Option<(
        Vec<crate::mix::logic::Graph>,
        Vec<crate::mix::logic::Macro>,
        crate::mix::logic::Context,
    )>,
    /// `mix.json` is written at most every [`SAVE_DELAY`] (D168).
    mix_save: crate::state::Coalesce,
    block: u32,
    read_offset: u32,
    input_count: usize,
    /// The virtual outputs (C7), slot 1 first.
    pub virtual_outputs: Vec<crate::config::VirtualOutput>,
    default_commit: u32,
    mix_path: PathBuf,
    pub history: crate::history::History,
    /// Duration of the last plan compile (§15 `plan_compile_seconds`).
    pub compile_seconds: f64,
    /// Steps of the running plan by type (§15 `nodes{type}`).
    pub step_counts: BTreeMap<&'static str, usize>,
    /// VAD, auto-levels, auto-mixes and ducks (§8).
    pub auto: crate::auto::Auto,
    /// Logic graphs (§11).
    pub logic: crate::logic::Logic,
    /// Logic actions and log lines for the events socket (JSON text).
    pub logic_events: tokio::sync::broadcast::Sender<String>,
    /// Remote-link peers that are up, for the logic `link` block.
    pub link_up: std::collections::BTreeSet<String>,
    /// Loudness of the signals logic graphs read, refreshed at the meter rate.
    logic_loudness: BTreeMap<String, crate::loudness::LoudnessValues>,
    /// The logic editor's TSL output (`TSL_OUT_TARGET`), when configured.
    pub tsl_out: Option<tokio::sync::watch::Sender<BTreeMap<u16, crate::tally::OutDisplay>>>,
    /// Each `tsl-out` block's display, by graph and block.
    tsl_displays: BTreeMap<(String, String), (u16, crate::tally::OutDisplay)>,
    /// The slave's remote link (§13): the master that holds it, takeover deny.
    pub link: crate::link::Takeover,
}

impl Control {
    pub fn new(
        engine: Engine,
        inputs: Inputs,
        outputs: Outputs,
        node: mxl_nmos::Node,
        config: &Config,
    ) -> Self {
        let mut compiler = Compiler::with_talk_base(config.input_flows);
        compiler.vouts = config
            .virtual_outputs
            .iter()
            .map(|v| (v.id.clone(), v.format))
            .collect();
        Self {
            engine,
            inputs,
            outputs,
            meters: Meters::default(),
            loudness: LoudnessMeters::default(),
            mix: Mix::default(),
            node,
            compiler,
            analyser: crate::analysis::Analyser::new(config.block_samples as usize),
            tally: crate::tally::Tally::default(),
            streams: crate::preview::Streams::new(config),
            max_personal: config.max_web_monitors,
            talkers: crate::preview::talkback::Talkers::new(config),
            momentary: BTreeMap::new(),
            layout: HashMap::new(),
            procs: HashMap::new(),
            latency: HashMap::new(),
            revision: 0,
            structure: None,
            pending: None,
            logic_swap_at: None,
            changes: std::sync::Arc::new(tokio::sync::Mutex::new(())),
            logic_guard: None,
            logic_swap_delay: test_swap_delay(),
            logic_waiting: crate::logic::Waiting::default(),
            logic_values: Vec::new(),
            logic_failing: std::collections::BTreeSet::new(),
            mix_save: crate::state::Coalesce::default(),
            logic_valid: None,
            block: config.block_samples,
            read_offset: config.read_offset_samples,
            input_count: config.input_flows,
            virtual_outputs: config.virtual_outputs.clone(),
            default_commit: config.output_commit_samples,
            mix_path: config.state_dir.join("mix.json"),
            history: crate::history::History::new(&config.state_dir),
            compile_seconds: 0.0,
            step_counts: BTreeMap::new(),
            auto: crate::auto::Auto::new(),
            logic: crate::logic::Logic::default(),
            logic_events: tokio::sync::broadcast::channel(256).0,
            link_up: std::collections::BTreeSet::new(),
            logic_loudness: BTreeMap::new(),
            tsl_out: None,
            tsl_displays: BTreeMap::new(),
            link: crate::link::Takeover::new(
                config.link_role == crate::config::LinkRole::Slave && config.link_takeover_deny,
            ),
        }
    }

    fn minute_of_day() -> u16 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| ((d.as_secs() / 60) % 1440) as u16)
            .unwrap_or(0)
    }

    /// POST a logic webhook: one shared client (5 s timeout), at most
    /// [`MAX_WEBHOOKS`] posts at once; one more is dropped with a warning (D173).
    fn post_webhook(url: String, graph: String, block: String) {
        static CLIENT: std::sync::OnceLock<Option<reqwest::Client>> = std::sync::OnceLock::new();
        static RUNNING: std::sync::OnceLock<std::sync::Arc<tokio::sync::Semaphore>> =
            std::sync::OnceLock::new();
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let client = CLIENT.get_or_init(|| {
            reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(5))
                .build()
                .ok()
        });
        let Some(client) = client.clone() else {
            tracing::warn!("logic webhook: no HTTP client");
            return;
        };
        let running = RUNNING
            .get_or_init(|| std::sync::Arc::new(tokio::sync::Semaphore::new(MAX_WEBHOOKS)))
            .clone();
        let Ok(permit) = running.try_acquire_owned() else {
            tracing::warn!(
                "logic webhook {}: {MAX_WEBHOOKS} posts are still running; this one was dropped",
                redact_url(&url)
            );
            return;
        };
        handle.spawn(async move {
            let _permit = permit;
            let sent = client
                .post(&url)
                .json(&serde_json::json!({"graph": graph, "block": block, "value": 1}))
                .send()
                .await
                .and_then(|r| r.error_for_status())
                .map(|_| ());
            if let Err(e) = sent {
                tracing::warn!("logic webhook {}: {}", redact_url(&url), e.without_url());
            }
        });
    }

    /// What the logic blocks read this tick.
    fn logic_inputs(&self) -> LogicInputs {
        use crate::mix::logic::BlockKind;
        let speech = self
            .auto
            .vads
            .iter()
            .map(|(s, v)| (s.clone(), v.state.speech_active))
            .collect();
        let mut level_db = BTreeMap::new();
        let mut nmos = std::collections::BTreeSet::new();
        for b in self.mix.logic_blocks() {
            let key = match &b.kind {
                BlockKind::Level { signal } | BlockKind::Above { signal, .. } => {
                    if let Some(db) = self.meters.peak_dbfs(signal) {
                        level_db.insert(signal.clone(), db);
                    }
                    continue;
                }
                BlockKind::NmosReceiver { input } => {
                    (mxl_nmos::Role::Receiver, format!("in-{input}"))
                }
                BlockKind::NmosSender { signal } => {
                    (mxl_nmos::Role::Sender, crate::outputs::nmos_key(signal))
                }
                _ => continue,
            };
            if self
                .node
                .active(key.0, &key.1)
                .is_some_and(|a| a.master_enable)
            {
                nmos.insert(key.1);
            }
        }
        let pfl = self
            .compiler
            .solo
            .iter()
            .filter(|(_, k)| **k == crate::mix::room::SoloKind::Pfl)
            .map(|(s, _)| s.clone())
            .collect();
        (speech, level_db, pfl, nmos)
    }

    /// Logic graphs (§11.2), on the same 10 ms tick as the auto engine. The
    /// actions become the changes the API makes (D164): mutes, faders, VCAs and
    /// node parameters ramp (grouped by ramp); PFL, talk, publish, monitor
    /// source and recall are staged last, and swap the plan on a later tick if
    /// they rewire it.
    pub fn logic_tick(&mut self) {
        use crate::logic::Act;
        if self.logic_swap_at.is_some_and(|t| Instant::now() >= t) {
            self.logic_swap_at = None;
            if let Err(e) = self.swap() {
                tracing::warn!("logic: {e}");
            }
            // The API may change the mix again.
            self.logic_guard = None;
        }
        if self.pending.is_some() {
            return;
        }
        let (speech, level_db, pfl, nmos) = self.logic_inputs();
        let on_air = self.tally.on_air_indexes();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let actions = self.logic.tick(&crate::logic::Readings {
            mix: &self.mix,
            speech: &speech,
            level_db: &level_db,
            on_air: &on_air,
            minute_of_day: Self::minute_of_day(),
            pfl: &pfl,
            loudness: &self.logic_loudness,
            nmos: &nmos,
            link: &self.link_up,
            now_ms: now.as_millis() as u64,
        });
        self.update_tsl_out();
        for a in actions {
            let text = a.act.describe();
            let mut event = serde_json::json!({
                "type": "logic",
                "graph": a.graph,
                "block": a.block,
                "action": text,
                "dry_run": a.dry_run,
                "t_ms": now.as_millis() as u64,
            });
            if let Act::Log { text } = &a.act {
                event["log"] = serde_json::json!(text);
                tracing::info!(graph = %a.graph, block = %a.block, dry_run = a.dry_run, "logic: {text}");
            } else if a.dry_run {
                tracing::info!(graph = %a.graph, block = %a.block, "logic dry run: {text}");
            }
            // A retry of a failed change is not listed again (warned once).
            if !a.retry {
                let _ = self.logic_events.send(event.to_string());
            }
            if a.dry_run {
                continue;
            }
            if matches!(
                a.act,
                Act::Pfl { .. }
                    | Act::Talk { .. }
                    | Act::Publish { .. }
                    | Act::Monitor { .. }
                    | Act::Recall { .. }
            ) {
                self.logic_waiting.push(a);
                continue;
            }
            match a.act {
                Act::Webhook { url } => Self::post_webhook(url, a.graph, a.block),
                Act::Loudness { signal, action } => {
                    if let Err(e) = self.loudness.control(&signal, action.name()) {
                        tracing::warn!("logic {}: {e}", a.graph);
                    }
                }
                Act::Log { .. } => {}
                Act::Tsl {
                    index,
                    left,
                    right,
                    label,
                } => {
                    self.tsl_displays.insert(
                        (a.graph, a.block),
                        (index, crate::tally::OutDisplay { left, right, label }),
                    );
                }
                Act::Mute { .. } | Act::Fader { .. } | Act::Vca { .. } | Act::Param { .. } => {
                    // The newest value of a block waits for the change lock.
                    self.logic_values
                        .retain(|x| !(x.graph == a.graph && x.block == a.block));
                    self.logic_values.push(a);
                }
                _ => {}
            }
        }
        self.update_tsl_out();
        if !self.run_values() {
            return;
        }
        self.run_waiting();
    }

    /// Stage the waiting mutes, faders, VCAs and parameters under the change
    /// lock, grouped by ramp; they wait a tick while an API change holds it
    /// (D167). False when a swap is now pending.
    fn run_values(&mut self) -> bool {
        use crate::logic::Act;
        if self.logic_values.is_empty() {
            return true;
        }
        let Ok(guard) = self.changes.clone().try_lock_owned() else {
            return true;
        };
        let mut guard = Some(guard);
        let samples = |ms: f64| (ms * 48.0).round() as u32;
        let mut ramps: BTreeMap<u32, Vec<Act>> = BTreeMap::new();
        for a in std::mem::take(&mut self.logic_values) {
            let ramp = match &a.act {
                Act::Fader { ramp_ms, .. }
                | Act::Vca { ramp_ms, .. }
                | Act::Param { ramp_ms, .. } => samples(*ramp_ms),
                _ => FADE_SAMPLES,
            };
            ramps.entry(ramp).or_default().push(a.act);
        }
        for (ramp, acts) in ramps {
            let mut next = self.mix.clone();
            for act in acts {
                apply_value_act(&mut next, act);
            }
            if next != self.mix && !self.stage_logic(next, ramp, &mut guard) {
                tracing::warn!(
                    "logic: a change waits for the plan; later actions of this tick were dropped"
                );
                return false;
            }
        }
        true
    }

    /// The logic changes that rewire the plan, when the change lock is free:
    /// staged together; the lock is held until the swap (D167).
    fn run_waiting(&mut self) {
        use crate::logic::{Act, Taken};
        let (guard, actions) = match self.logic_waiting.take(&self.changes, Instant::now()) {
            Taken::Idle | Taken::Wait => return,
            Taken::Late(actions) => {
                for a in actions {
                    tracing::warn!(
                        graph = %a.graph,
                        block = %a.block,
                        "logic: {} waits: the mix has been changing for 1 s",
                        a.act.describe()
                    );
                }
                return;
            }
            Taken::Go(guard, actions) => (guard, actions),
        };
        let described: Vec<String> = actions
            .iter()
            .map(|a| format!("{} {}: {}", a.graph, a.block, a.act.describe()))
            .collect();
        let blocks: Vec<(String, String)> = actions
            .iter()
            .map(|a| (a.graph.clone(), a.block.clone()))
            .collect();
        let mut next = self.mix.clone();
        let solo_before = self.compiler.solo.clone();
        let talk_before = self.compiler.talking.clone();
        for act in actions.into_iter().map(|a| a.act) {
            match act {
                Act::Pfl { signal, on } => {
                    if on {
                        self.set_solo(&signal, Some(crate::mix::room::SoloKind::Pfl));
                    } else if self.compiler.solo.get(&signal)
                        == Some(&crate::mix::room::SoloKind::Pfl)
                    {
                        self.set_solo(&signal, None);
                    }
                }
                Act::Talk {
                    mic,
                    destination,
                    on,
                } => {
                    if let Err(e) = self.set_talk(&mic, &destination, on, false) {
                        tracing::warn!("logic: {e}");
                    }
                }
                Act::Publish { signal, on } => {
                    if !on {
                        next.outputs.retain(|o| o.signal != signal);
                    } else if next.output(&signal).is_none() && next.kind(&signal).is_some() {
                        next.outputs.push(crate::mix::model::Output {
                            signal,
                            format: None,
                            commit_samples: None,
                        });
                    }
                }
                Act::Monitor { monitor, source } => {
                    for m in next
                        .control_room
                        .monitors
                        .iter_mut()
                        .filter(|m| m.id == monitor)
                    {
                        m.source = source.clone();
                    }
                }
                Act::Recall { snapshot } => {
                    let Some(snap) = next.snapshots.iter().find(|s| s.id == snapshot).cloned()
                    else {
                        tracing::warn!("logic recall: snapshot {snapshot} does not exist");
                        continue;
                    };
                    crate::mix::show::recall(
                        &mut next,
                        &snap,
                        &crate::mix::show::Filter::default(),
                    );
                }
                _ => {}
            }
        }
        // Solo and talk live in the compiler: they need a stage too.
        if next == self.mix
            && self.compiler.solo == solo_before
            && self.compiler.talking == talk_before
        {
            return;
        }
        match self.stage(next, FADE_SAMPLES) {
            Ok(Staged::Done) => {}
            Ok(Staged::Swap { wait: true }) => {
                self.logic_swap_at =
                    Some(Instant::now() + fade_wait(self.block) + self.logic_swap_delay);
                self.logic_guard = Some(guard);
            }
            Ok(Staged::Swap { wait: false }) => {
                if let Err(e) = self.swap() {
                    tracing::warn!("logic: {e} ({})", described.join("; "));
                }
            }
            Err(e) => {
                // Not applied: the blocks act again with their current input
                // (at most every 250 ms); warned once until one succeeds.
                for (g, b) in &blocks {
                    self.logic.forget(g, b);
                }
                if blocks.iter().any(|k| self.logic_failing.insert(k.clone())) {
                    tracing::warn!("logic: {e}; not applied: {}", described.join("; "));
                }
                return;
            }
        }
        for k in &blocks {
            self.logic_failing.remove(k);
        }
    }

    /// Stage a logic value change under the change lock (`guard`); a swap
    /// happens now or after the fade, holding the lock until then. False when a
    /// swap is pending.
    fn stage_logic(
        &mut self,
        next: Mix,
        ramp: u32,
        guard: &mut Option<tokio::sync::OwnedMutexGuard<()>>,
    ) -> bool {
        match self.stage(next, ramp) {
            Ok(Staged::Done) => true,
            Ok(Staged::Swap { wait: true }) => {
                self.logic_swap_at =
                    Some(Instant::now() + fade_wait(self.block) + self.logic_swap_delay);
                self.logic_guard = guard.take();
                false
            }
            Ok(Staged::Swap { wait: false }) => {
                if let Err(e) = self.swap() {
                    tracing::warn!("logic: {e}");
                }
                true
            }
            Err(e) => {
                tracing::warn!("logic: {e}");
                true
            }
        }
    }

    /// Shutdown: the indexes the logic's TSL output has lit, emptied so the
    /// sender switches them off too.
    pub fn tsl_off(&mut self) -> Vec<u16> {
        let indexes = self.tsl_displays.values().map(|(i, _)| *i).collect();
        self.tsl_displays.clear();
        if let Some(tx) = &self.tsl_out {
            tx.send_replace(BTreeMap::new());
        }
        indexes
    }

    /// Hand the `tsl-out` displays of the running graphs to the TSL sender.
    fn update_tsl_out(&mut self) {
        let mix = &self.mix;
        self.tsl_displays.retain(|(g, b), (index, _)| {
            mix.logic_graph(g).is_some_and(|g| {
                g.enabled
                    && !g.dry_run
                    && matches!(
                        mix.logic_block_at(&g.id, b),
                        Some(crate::mix::logic::BlockKind::TslOut { index: i }) if i == index
                    )
            })
        });
        let Some(tx) = &self.tsl_out else {
            return;
        };
        let displays: BTreeMap<u16, crate::tally::OutDisplay> = self
            .tsl_displays
            .values()
            .map(|(i, d)| (*i, d.clone()))
            .collect();
        tx.send_if_modified(|current| {
            if *current == displays {
                false
            } else {
                *current = displays;
                true
            }
        });
    }

    /// The control engine's tick (§8, every [`crate::auto::TICK`]): VADs,
    /// auto-levels, auto-mixes and ducks; changed gains ramp over one frame.
    pub fn auto_tick(&mut self) {
        let c = &mut *self;
        let changed = c.auto.tick(&mut c.engine.vad, &c.mix);
        for (signal, g) in changed {
            let key = match c.mix.kind(&signal) {
                Some(Kind::Bus) => format!("bus/{signal}/auto"),
                _ => format!("strip/{signal}/auto"),
            };
            let node = c.compiler.node_id(&key);
            c.compiler.auto.insert(signal, g.clone());
            let Some(l) = node.and_then(|n| c.layout.get_mut(&n).map(|l| (n, l))) else {
                continue;
            };
            let (node, l) = l;
            if g.len() != l.values.len() {
                continue;
            }
            c.engine.param(node, &g, crate::auto::vad::FRAME as u32);
            l.values = g;
        }
    }

    /// The 100 ms tick: free what the DSP thread returned, run the inputs.
    pub fn tick(&mut self) {
        self.engine.collect();
        self.inputs.tick(&mut self.engine);
        self.history.tick(&self.mix);
        if self.mix_save.take_due(Instant::now()) {
            if let Err(e) = self.write_mix() {
                tracing::error!("saving the mix: {e}");
            }
        }
    }

    /// The meter tick (§7, 25 Hz): meters, loudness, the analyser.
    pub fn meter_tick(&mut self) {
        self.meters.tick();
        let true_peaks = self.meters.true_peaks();
        self.loudness.tick(&true_peaks);
        let read: std::collections::BTreeSet<&str> = self
            .mix
            .logic_blocks()
            .filter_map(|b| match &b.kind {
                crate::mix::logic::BlockKind::Loudness { signal, .. } => Some(signal.as_str()),
                _ => None,
            })
            .collect();
        self.logic_loudness = if read.is_empty() {
            BTreeMap::new()
        } else {
            let mut all = self.loudness.values(&self.mix.settings);
            all.retain(|s, _| read.contains(s.as_str()));
            all
        };
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

    /// Every output to publish: the virtual outputs first, so they take their
    /// slots before the published ones at a start (D267).
    fn specs(&self, mix: &Mix) -> Vec<OutputSpec> {
        let virtual_outputs = self.virtual_outputs.iter().map(|v| OutputSpec {
            signal: crate::mix::compile::vout_key(&v.id),
            name: v.label.clone(),
            format: v.format,
            commit: self.default_commit,
        });
        let published = mix.outputs.iter().filter_map(|o| {
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
        });
        let monitors = mix
            .control_room
            .monitors
            .iter()
            .filter(|m| m.publish)
            .map(|m| OutputSpec {
                signal: m.key(),
                name: format!("Monitor {}", m.name),
                format: m.format,
                commit: self.default_commit,
            });
        virtual_outputs.chain(published).chain(monitors).collect()
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

    /// Add a personal monitor on `source`, following the control room's solo or
    /// not; applied with the next change of the mix.
    pub fn add_personal(
        &mut self,
        source: Option<crate::mix::room::Source>,
        follows_solo: bool,
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
        // The control room's PFL/AFL only when asked for (`follows_solo`).
        monitor.follows_solo = follows_solo;
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

    /// Every talkback mic: configured, then the browsers'.
    pub fn mic_ids(&self) -> Vec<String> {
        self.mix
            .control_room
            .talkback
            .mics
            .iter()
            .map(|m| m.id.clone())
            .chain(self.talkers.slots().into_keys())
            .collect()
    }

    /// Who talks to what.
    pub fn talking(&self) -> &crate::mix::talkback::Talking {
        &self.compiler.talking
    }

    /// Mic `mic` starts (`on`) or stops talking to `dest`; a momentary talk ends
    /// unless repeated within [`MOMENTARY_HOLD`]. Applied with the next change.
    pub fn set_talk(
        &mut self,
        mic: &str,
        dest: &str,
        on: bool,
        momentary: bool,
    ) -> Result<(), String> {
        if !self.mic_ids().iter().any(|m| m == mic) {
            return Err(format!("talkback mic {mic} does not exist"));
        }
        if !self
            .mix
            .control_room
            .talkback
            .destinations
            .iter()
            .any(|d| d.id == dest)
        {
            return Err(format!("talkback destination {dest} does not exist"));
        }
        let key = (mic.to_string(), dest.to_string());
        if on {
            self.compiler
                .talking
                .entry(mic.to_string())
                .or_default()
                .insert(dest.to_string());
            if momentary {
                self.momentary.insert(key, std::time::Instant::now());
            } else {
                self.momentary.remove(&key);
            }
        } else {
            if let Some(d) = self.compiler.talking.get_mut(mic) {
                d.remove(dest);
                if d.is_empty() {
                    self.compiler.talking.remove(mic);
                }
            }
            self.momentary.remove(&key);
        }
        Ok(())
    }

    /// Stop `mic` (or every mic) talking.
    pub fn clear_talk(&mut self, mic: Option<&str>) {
        match mic {
            Some(m) => {
                self.compiler.talking.remove(m);
                self.momentary.retain(|(x, _), _| x != m);
            }
            None => {
                self.compiler.talking.clear();
                self.momentary.clear();
            }
        }
    }

    /// End momentary talk that was not repeated; true when any ended.
    pub fn expire_talk(&mut self) -> bool {
        let old: Vec<(String, String)> = self
            .momentary
            .iter()
            .filter(|(_, t)| t.elapsed() > MOMENTARY_HOLD)
            .map(|(k, _)| k.clone())
            .collect();
        for (m, d) in &old {
            let _ = self.set_talk(m, d, false, false);
        }
        !old.is_empty()
    }

    /// A browser's microphone is on: its mic id, and whether it is new.
    pub fn register_browser_mic(&mut self, client: &str) -> Result<(String, bool), String> {
        let c = &mut *self;
        c.talkers.register(&mut c.engine, client)
    }

    pub fn remove_browser_mic(&mut self, id: &str) -> bool {
        let c = &mut *self;
        let gone = c.talkers.remove(&mut c.engine, id);
        if gone {
            self.clear_talk(Some(id));
        }
        gone
    }

    /// Remove browser mics that stopped registering; true when any went.
    pub fn expire_browser_mics(&mut self) -> bool {
        let before = self.talkers.slots();
        let c = &mut *self;
        let changed = c.talkers.expire(&mut c.engine);
        if changed {
            let now = self.talkers.slots();
            for id in before.keys().filter(|id| !now.contains_key(*id)) {
                self.clear_talk(Some(id));
            }
        }
        changed
    }

    pub fn listen_back(&self) -> Option<&crate::mix::room::Source> {
        self.compiler.listen_back.as_ref()
    }

    /// The monitors that follow solo play `source` instead of theirs (or their
    /// own again with `None`). Applied with the next change.
    pub fn set_listen_back(&mut self, source: Option<crate::mix::room::Source>) {
        self.compiler.listen_back = source;
    }

    /// Stop unheard streams; true when the plan needs a change.
    pub fn expire_streams(&mut self) -> bool {
        let ids = self.stream_ids();
        let c = &mut *self;
        c.streams
            .expire(&mut c.engine, |id| ids.iter().any(|x| x == id))
    }

    /// Validate the logic of `next`, unless it and what it names are what was
    /// last found valid: a fader move does not check every graph again (D171).
    fn check_logic(&mut self, next: &Mix) -> Result<(), String> {
        let context = crate::mix::logic::Context::of(next);
        let same = self
            .logic_valid
            .as_ref()
            .is_some_and(|(g, m, c)| *g == next.logic && *m == next.logic_macros && *c == context);
        if !same {
            crate::mix::logic::validate(next)?;
            self.logic_valid = Some((next.logic.clone(), next.logic_macros.clone(), context));
        }
        Ok(())
    }

    /// The logic of `mix` was validated (off the control lock): the next stage
    /// need not do it again.
    pub fn logic_validated(&mut self, mix: &Mix) {
        self.logic_valid = Some((
            mix.logic.clone(),
            mix.logic_macros.clone(),
            crate::mix::logic::Context::of(mix),
        ));
    }

    /// Check `next` and start applying it. Parameter-only changes ramp over `ramp`
    /// samples and are done; anything else fades the affected nodes out and needs
    /// [`Control::swap`] after [`fade_wait`].
    pub fn stage(&mut self, mut next: Mix, ramp: u32) -> Result<Staged, String> {
        if self.link.refuses() {
            return Err("the master is disconnected and local takeover is denied".into());
        }
        // Whatever wrote the sends (API, recall, import, the link), a subgroup's
        // keep what is heard (C12, D186).
        next.lock_subgroup_sends();
        next.validate_except_logic(self.input_count, self.block)?;
        // The virtual outputs hold their slots: the published outputs and
        // monitors share the rest (D267).
        let free = crate::engine::MAX_OUTPUTS - self.virtual_outputs.len();
        let published = next.outputs.len()
            + next
                .control_room
                .monitors
                .iter()
                .filter(|m| m.publish)
                .count();
        if published > free {
            return Err(format!(
                "at most {free} published outputs and monitors (MIXER_OUTPUT_FLOWS takes {})",
                self.virtual_outputs.len()
            ));
        }
        self.check_logic(&next)?;
        self.compiler.afv = self.afv_targets(&next);
        self.compiler.auto = self.auto.compile_gains(&next);
        self.compiler.vad_slots = self.auto.assign(&next);
        self.compiler
            .solo
            .retain(|s, _| next.kind(s).is_some() || next.vca(s).is_some());
        self.compiler.streams = self.streams.slots();
        self.compiler.browser_mics = self.talkers.slots();
        let mics: Vec<String> = next
            .control_room
            .talkback
            .mics
            .iter()
            .map(|m| m.id.clone())
            .chain(self.compiler.browser_mics.keys().cloned())
            .collect();
        let dests: Vec<&str> = next
            .control_room
            .talkback
            .destinations
            .iter()
            .map(|d| d.id.as_str())
            .collect();
        self.compiler.talking.retain(|m, d| {
            d.retain(|x| dests.contains(&x.as_str()));
            mics.contains(m) && !d.is_empty()
        });
        self.momentary
            .retain(|(m, d), _| mics.contains(m) && dests.contains(&d.as_str()));
        if let Some(crate::mix::room::Source::Signal(id)) = &self.compiler.listen_back {
            if next.kind(id).is_none() {
                self.compiler.listen_back = None;
            }
        }
        for p in &mut self.compiler.personal {
            p.solo
                .retain(|s, _| next.kind(s).is_some() || next.vca(s).is_some());
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
            self.changed();
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
        // A monitor whose output could not be published is not shown as
        // published (its error is in `errors`).
        for m in &mut next.control_room.monitors {
            if m.publish && !slots.contains_key(&m.key()) {
                m.publish = false;
            }
        }
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
        self.engine
            .meter_plan(compiled.plan.generation, compiled.plan.meters.clone());
        self.auto.planned(compiled.plan.generation);
        self.engine.send(Command::SetPlan(Box::new(compiled.plan)));
        self.layout = compiled.nodes;
        self.procs = compiled.procs;
        self.latency = compiled.latency;
        self.structure = Some(compiled.structure);
        self.mix = next;
        self.save()?;
        Ok(errors)
    }

    /// A change of the mix: clients reload it; `mix.json` follows within
    /// [`SAVE_DELAY`] (D168).
    fn changed(&mut self) {
        self.revision += 1;
        self.history.changed(&self.mix);
        self.mix_save.changed(Instant::now(), SAVE_DELAY);
    }

    /// A change of the mix, written to `mix.json` at once.
    fn save(&mut self) -> Result<(), String> {
        self.changed();
        self.mix_save.take();
        self.write_mix()
    }

    fn write_mix(&self) -> Result<(), String> {
        let body = serde_json::json!({"version": 1, "mix": self.mix});
        crate::state::write_atomic(
            &self.mix_path,
            &serde_json::to_vec_pretty(&body).unwrap_or_default(),
        )
    }

    /// Write `mix.json` now if a change waits (shutdown, an operator's explicit
    /// show action).
    pub fn flush_mix(&mut self) {
        if self.mix_save.take() {
            if let Err(e) = self.write_mix() {
                tracing::error!("saving the mix: {e}");
            }
        }
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
        self.flush_mix();
        self.engine.stop();
    }
}

/// Speech, peak levels, signals in PFL and active NMOS keys, for the logic tick.
type LogicInputs = (
    BTreeMap<String, bool>,
    BTreeMap<String, f64>,
    std::collections::BTreeSet<String>,
    std::collections::BTreeSet<String>,
);

/// A logic mute, fader, VCA or node parameter on `mix`, clamped as the API
/// clamps it.
fn apply_value_act(mix: &mut Mix, act: crate::logic::Act) {
    use crate::logic::Act;
    match act {
        Act::Mute { signal, on } => {
            for s in mix.strips.iter_mut().filter(|s| s.id == signal) {
                s.mute = on;
            }
            for b in mix.buses.iter_mut().filter(|b| b.id == signal) {
                b.mute = on;
            }
        }
        Act::Fader { signal, db, .. } => {
            let db = crate::logic::clamp_fader(db);
            for s in mix.strips.iter_mut().filter(|s| s.id == signal) {
                s.fader_db = db;
            }
            for b in mix.buses.iter_mut().filter(|b| b.id == signal) {
                b.fader_db = db;
            }
        }
        Act::Vca { vca, db, .. } => {
            for v in mix.vcas.iter_mut().filter(|v| v.id == vca) {
                v.fader_db = crate::logic::clamp_fader(db);
            }
        }
        Act::Param {
            signal,
            node,
            param,
            value,
            ..
        } => {
            let chain = mix
                .strips
                .iter_mut()
                .find(|s| s.id == signal)
                .map(|s| &mut s.chain)
                .or_else(|| {
                    mix.buses
                        .iter_mut()
                        .find(|b| b.id == signal)
                        .map(|b| &mut b.chain)
                });
            if let Some(p) = chain.and_then(|c| c.iter_mut().find(|p| p.id == node)) {
                if let Err(e) = crate::mix::logic::set_param_clamped(p, &param, value) {
                    tracing::warn!("logic: {signal} {node}: {e}");
                }
            }
        }
        _ => {}
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

/// A URL for the log without what may be a secret: `user:password@` and the
/// query and fragment.
pub fn redact_url(url: &str) -> String {
    let url = url.split(['?', '#']).next().unwrap_or_default();
    match url.split_once("://") {
        Some((scheme, rest)) => {
            let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
            let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
            let slash = if rest.contains('/') { "/" } else { "" };
            format!("{scheme}://{host}{slash}{path}")
        }
        None => url.to_string(),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn webhook_urls_are_logged_without_secrets() {
        use super::redact_url;
        assert_eq!(
            redact_url("https://user:pass@hooks.example:8443/a/b?token=x#f"),
            "https://hooks.example:8443/a/b"
        );
        assert_eq!(redact_url("http://10.0.0.1/hook"), "http://10.0.0.1/hook");
        assert_eq!(redact_url("http://u@h"), "http://h");
        assert_eq!(redact_url("http://h/p@q"), "http://h/p@q");
    }
}
