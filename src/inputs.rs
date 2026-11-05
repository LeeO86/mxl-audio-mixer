//! The inputs' control side (§4.2): routing from IS-05, waiting for a flow,
//! re-opening re-created flows, stall detection, arrival lag and the automatic
//! extra delay. Runs on a 100 ms tick, never in the DSP thread.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config::{Config, SAMPLE_RATE};
use crate::engine::{Command, Engine, EngineCounters};
use crate::mxlio::{self, Api, MxlInstance};

/// Length of the sliding window of arrival lags.
pub const LAG_WINDOW: Duration = Duration::from_secs(10);
/// The automatic delay goes down only when the need is at least this much lower
/// (1 ms), for `MIXER_INPUT_DELAY_HOLD_S`.
const DELAY_LOWER_SAMPLES: u32 = 48;
/// The start-up phase of an input (from its first automatic decision after a
/// route or a start): the window and the hold are [`STARTUP_HOLD`], so the
/// delay settles fast; a second late block ends it.
const STARTUP: Duration = Duration::from_secs(60);
/// The window and the hold time in the start-up phase.
const STARTUP_HOLD: Duration = Duration::from_secs(5);
/// Lag samples needed before the first automatic setting: one second of blocks.
const MIN_LAG_SECONDS: u64 = 1;
const LAG_BUCKET_SAMPLES: u32 = 4; // 83 µs resolution

/// §15 `input_state`: 0 unrouted, 1 waiting, 2 running, 3 stalled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Unrouted = 0,
    Waiting = 1,
    Running = 2,
    Stalled = 3,
}

/// An input's extra delay: measured (`auto`, the default) or fixed in ms.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ExtraDelay {
    Auto(AutoTag),
    Ms(f64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AutoTag {
    Auto,
}

impl Default for ExtraDelay {
    fn default() -> Self {
        Self::Auto(AutoTag::Auto)
    }
}

/// Per-input settings kept in the show file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InputConfig {
    /// Input number, from 1.
    pub input: usize,
    pub label: String,
    #[serde(default)]
    pub extra_delay: ExtraDelay,
}

impl InputConfig {
    /// The defaults of the input at slot `index` (number `index + 1`).
    pub fn default_for(index: usize) -> Self {
        Self {
            input: index + 1,
            label: format!("Input {}", index + 1),
            extra_delay: ExtraDelay::default(),
        }
    }
}

/// Where an input reads from, from its IS-05 activation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    pub domain: PathBuf,
    pub flow: Uuid,
}

/// The settings of the automatic delay (`MIXER_INPUT_DELAY_*`), in samples.
#[derive(Debug, Clone, Copy)]
struct DelayPolicy {
    read_offset: u32,
    delay_max: u32,
    /// The delay goes down only after the need stayed lower this long, and
    /// changes at most once per this time going down.
    hold: Duration,
    /// How long the worst lag counts (`MIXER_INPUT_DELAY_WINDOW_S`).
    window: Duration,
    /// Added to the worst lag (`MIXER_INPUT_DELAY_MARGIN_MS`) ...
    margin: u32,
    /// ... and this share of the worst lag above the read offset
    /// (`MIXER_INPUT_DELAY_MARGIN_PCT` / 100): a long tail gets more, a local
    /// input with a short lag about the fixed margin only.
    margin_share: f64,
}

impl DelayPolicy {
    /// The delay that covers `lag`, with the margin, within the maximum.
    fn need_of(&self, lag: u32) -> u32 {
        let above = lag.saturating_sub(self.read_offset);
        let margin = self.margin + (f64::from(above) * self.margin_share).round() as u32;
        (above + margin).min(self.delay_max)
    }
}

/// The automatic delay (D-347, D310): it covers the worst arrival lag of the
/// last [`DelayPolicy::window`] (5 min by default) plus a margin. It goes up at
/// once when less than half the margin is left above a block's lag (or a block
/// was late); it goes down only when the worst lag of the window has been at
/// least [`DELAY_LOWER_SAMPLES`] lower for a hold time, so after a spike it
/// stays up for the window plus a hold time, and a tail that keeps coming
/// keeps it up. A local input with a short, steady lag keeps a small delay: its
/// worst lag is about its 99.9th percentile. Lags above the read offset plus
/// the maximum delay are no outliers but a writer catching up (a stall, D230)
/// and count for nothing.
///
/// In the start-up phase of an input ([`STARTUP`] from its first decision) the
/// window and the hold are [`STARTUP_HOLD`], so a backlog of the start does not
/// keep the delay up for minutes; a second late block in that phase ends it
/// (the tail is real). The last run's delay (D242) is a floor for one hold
/// time; a lag that needs about as much ends the start-up phase too.
#[derive(Default)]
struct DelayHold {
    /// Per second: the highest lag of its blocks (within the limit).
    peaks: VecDeque<(Instant, u32)>,
    last_change: Option<Instant>,
    /// Since when the need has been lower, and the highest need since then.
    lower: Option<(Instant, u32)>,
    /// The first decision: the start-up phase runs [`STARTUP`] from it.
    started: Option<Instant>,
    /// Late events (a tick with a late block after one without, after the
    /// first change) seen in the start-up phase (2: it ended), and whether the
    /// last tick had a late block.
    startup_late: u8,
    late_before: bool,
    /// The delay of the last run (D242): a floor for one hold time.
    seeded: Option<(Instant, u32)>,
}

impl DelayHold {
    /// Start from the delay of the last run.
    fn seed(&mut self, delay: u32, now: Instant) {
        self.seeded = Some((now, delay));
    }

    /// A hold past its start-up phase (tests of the steady state).
    #[cfg(test)]
    fn steady() -> Self {
        Self {
            started: Some(Instant::now() - 2 * STARTUP),
            startup_late: 2,
            ..Self::default()
        }
    }

    /// A stall interrupts a period of lower need: it starts again afterwards.
    fn interrupt(&mut self) {
        self.lower = None;
    }

    /// The worst lag of the window (samples), if any.
    fn worst(&self) -> Option<u32> {
        self.peaks.iter().map(|(_, l)| *l).max()
    }

    /// The new delay for an input with delay `current`, or `None` to keep it.
    /// `p999` is the arrival lag's 99.9th percentile (`None` before a second of
    /// lags), `max_lag` the highest lag of the blocks since the last call; one
    /// above `read_offset + current` was late.
    fn next(
        &mut self,
        p999: Option<u32>,
        max_lag: Option<u32>,
        current: u32,
        policy: &DelayPolicy,
        now: Instant,
    ) -> Option<u32> {
        let DelayPolicy {
            read_offset,
            delay_max,
            ..
        } = *policy;
        let need_of = |lag: u32| policy.need_of(lag);
        let started = *self.started.get_or_insert(now);
        let startup = now.duration_since(started) < STARTUP && self.startup_late < 2;
        let (hold, window) = if startup {
            (
                policy.hold.min(STARTUP_HOLD),
                policy.window.min(STARTUP_HOLD),
            )
        } else {
            (policy.hold, policy.window)
        };
        if self
            .seeded
            .is_some_and(|(t, _)| now.duration_since(t) >= policy.hold)
        {
            self.seeded = None;
        }
        let max_lag = max_lag.filter(|&l| l <= read_offset + delay_max);
        let late = max_lag.is_some_and(|l| l > read_offset + current);
        // A late block after the first decision is a start-up event; so is a
        // lag that needs about the last run's delay (it was no stale value).
        if late && !self.late_before && startup && self.last_change.is_some() {
            self.startup_late = self.startup_late.saturating_add(1);
        }
        self.late_before = late;
        if let (Some(lag), Some((_, seed))) = (max_lag, self.seeded) {
            if need_of(lag) + DELAY_LOWER_SAMPLES >= seed {
                self.startup_late = 2;
            }
        }
        // The percentile is below the highest lag of its window; it counts on
        // its own only where no lags are given (tests).
        if let Some(lag) = max_lag.max(p999) {
            match self.peaks.back_mut() {
                Some((t, peak)) if now.duration_since(*t) < Duration::from_secs(1) => {
                    *peak = (*peak).max(lag);
                }
                _ => self.peaks.push_back((now, lag)),
            }
        }
        while self
            .peaks
            .front()
            .is_some_and(|(t, _)| now.duration_since(*t) >= window)
        {
            self.peaks.pop_front();
        }
        // Before a second of lags only a late block moves the delay.
        let worst = match (p999, self.worst()) {
            (None, _) if !late => return None,
            (p, w) => p.max(w)?,
        };
        let floor = self.seeded.map_or(0, |(_, d)| d);
        let need = need_of(worst).max(floor);
        // Up when less than half the margin is left above the worst lag.
        let half = (need_of(worst) + read_offset).saturating_sub(worst) / 2;
        if need > current && (late || worst + half > read_offset + current) {
            self.lower = None;
            self.last_change = Some(now);
            return Some(need);
        }
        if need + DELAY_LOWER_SAMPLES > current {
            self.lower = None;
            return None;
        }
        let (since, highest) = match self.lower {
            Some((since, highest)) => (since, highest.max(need)),
            None => (now, need),
        };
        self.lower = Some((since, highest));
        let held = |t: Instant| now.duration_since(t) >= hold;
        if held(since) && self.last_change.is_none_or(held) {
            self.lower = None;
            self.last_change = Some(now);
            return Some(highest);
        }
        None
    }
}

/// A sliding window of arrival lags as per-second histograms.
struct LagWindow {
    seconds: VecDeque<(Instant, Vec<u32>, u64)>,
    buckets: usize,
}

impl LagWindow {
    fn new(max_lag_samples: u32) -> Self {
        Self {
            seconds: VecDeque::new(),
            buckets: (max_lag_samples / LAG_BUCKET_SAMPLES) as usize + 2,
        }
    }

    fn push(&mut self, lag: u32, now: Instant) {
        let fresh = self
            .seconds
            .back()
            .is_none_or(|(t, _, _)| now.duration_since(*t) >= Duration::from_secs(1));
        if fresh {
            self.seconds.push_back((now, vec![0; self.buckets], 0));
        }
        while self
            .seconds
            .front()
            .is_some_and(|(t, _, _)| now.duration_since(*t) > LAG_WINDOW)
        {
            self.seconds.pop_front();
        }
        if let Some((_, hist, count)) = self.seconds.back_mut() {
            let bucket = ((lag / LAG_BUCKET_SAMPLES) as usize).min(self.buckets - 1);
            hist[bucket] += 1;
            *count += 1;
        }
    }

    fn clear(&mut self) {
        self.seconds.clear();
    }

    fn span(&self) -> Duration {
        match (self.seconds.front(), self.seconds.back()) {
            // From the first to the last per-second bucket: one second of samples
            // at least before the first automatic setting.
            (Some((a, _, _)), Some((b, _, _))) => b.duration_since(*a),
            _ => Duration::ZERO,
        }
    }

    /// The upper edge (samples) of the bucket holding quantile `q`.
    /// For display: every lag; lags beyond the window's range share its top
    /// bucket, so a writer always later than that reads the edge (`lag_ms`, the
    /// last lag, is exact).
    fn quantile(&self, q: f64) -> Option<u32> {
        self.quantile_upto(q, u32::MAX)
    }

    /// Quantile `q` of the lags up to `limit` samples only. The automatic delay
    /// uses it with the read offset plus the maximum delay: a lag beyond that is
    /// a writer catching up (or one no delay can cover) and must not move it.
    fn quantile_upto(&self, q: f64, limit: u32) -> Option<u32> {
        let bucket_of = |b: usize| (b as u32 + 1) * LAG_BUCKET_SAMPLES;
        let count =
            |b: usize| -> u64 { self.seconds.iter().map(|(_, h, _)| u64::from(h[b])).sum() };
        let within = (0..self.buckets).take_while(|&b| b as u32 * LAG_BUCKET_SAMPLES <= limit);
        let total: u64 = within.clone().map(count).sum();
        if total == 0 {
            return None;
        }
        let target = ((total as f64) * q).ceil().max(1.0) as u64;
        let mut seen = 0;
        for b in within {
            seen += count(b);
            if seen >= target {
                return Some(bucket_of(b));
            }
        }
        None
    }
}

struct Slot {
    config: InputConfig,
    state: State,
    route: Option<Route>,
    inode: Option<u64>,
    last_head: u64,
    head_changed: Instant,
    reopens: u64,
    lag: LagWindow,
    last_lag: u32,
    /// Extra delay applied (samples).
    delay: u32,
    /// An automatic change waiting for confirmation.
    proposed: Option<u32>,
    hold: DelayHold,
    /// Automatic delay changes (each one a fade), and ticks with a late block
    /// after one without (D310).
    delay_changes: u64,
    late_events: u64,
    late_before: bool,
    /// What is connected (C2), from [`crate::sources`].
    connected: Connected,
}

/// What an input is connected to (C2): the IS-05 sender id and the name of the
/// source (the IS-04 sender label from the Query API, else the MXL flow's
/// label).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Connected {
    pub sender_id: Option<Uuid>,
    pub label: Option<String>,
}

/// An input's status for the API, the UI and the metrics.
#[derive(Debug, Clone, Serialize)]
pub struct InputStatus {
    /// Input number, from 1.
    pub input: usize,
    pub label: String,
    pub state: State,
    pub flow: Option<Uuid>,
    pub domain: Option<String>,
    /// The connected sender (IS-05 `sender_id`), if the controller gave one.
    pub sender_id: Option<Uuid>,
    /// The name of what is connected (C2): the IS-04 sender label, else the MXL
    /// flow label; `null` while unrouted or unknown. `label` is the alias.
    pub connected_label: Option<String>,
    pub channels: u32,
    pub extra_delay: ExtraDelay,
    pub extra_delay_ms: f64,
    pub effective_delay_ms: f64,
    pub proposed_delay_ms: Option<f64>,
    pub lag_ms: f64,
    pub arrival_lag_ms: Option<ArrivalLag>,
    /// The worst arrival lag of `MIXER_INPUT_DELAY_WINDOW_S` that the automatic
    /// delay covers (D310); `null` without one (unrouted, fixed delay).
    pub worst_lag_ms: Option<f64>,
    /// Automatic delay changes since the start (each one a fade).
    pub delay_changes: u64,
    /// Late events: control ticks (100 ms) with a late block after one without,
    /// about the dropouts heard.
    pub late_events: u64,
    pub late_samples: u64,
    /// Samples the ring had already overwritten.
    pub overrun_samples: u64,
    pub missing_samples: u64,
    pub reopens: u64,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct ArrivalLag {
    pub p50: f64,
    pub p99: f64,
    pub p999: f64,
}

fn ms(samples: u32) -> f64 {
    f64::from(samples) * 1000.0 / f64::from(SAMPLE_RATE)
}

fn samples(ms: f64) -> u32 {
    (ms * f64::from(SAMPLE_RATE) / 1000.0).round().max(0.0) as u32
}

/// The inputs' state. Methods that talk to the DSP thread take the [`Engine`].
pub struct Inputs {
    api: Api,
    counters: Arc<EngineCounters>,
    slots: Vec<Slot>,
    read_offset: u32,
    delay_max: u32,
    confirm: bool,
    policy: DelayPolicy,
    stall: Duration,
    show_path: PathBuf,
    /// The last automatic delay per flow (samples), kept in
    /// `MIXER_STATE_DIR/input-delays.json`: an input starts at it (D242).
    last_delays: HashMap<Uuid, u32>,
    last_delays_path: PathBuf,
    last_delays_dirty: bool,
    /// MXL instances by flow id and the inode of its data file (D240).
    instances: HashMap<(Uuid, u64), MxlInstance>,
}

impl Inputs {
    pub fn new(api: Api, engine: &mut Engine, config: &Config, saved: Vec<InputConfig>) -> Self {
        let counters = engine.counters.clone();
        let now = Instant::now();
        let slots = (0..config.input_flows)
            .map(|i| Slot {
                config: saved
                    .iter()
                    .find(|c| c.input == i + 1)
                    .cloned()
                    .unwrap_or_else(|| InputConfig::default_for(i)),
                state: State::Unrouted,
                route: None,
                inode: None,
                last_head: 0,
                head_changed: now,
                reopens: 0,
                lag: LagWindow::new(
                    config.read_offset_samples + config.input_delay_max_samples + 4800,
                ),
                last_lag: 0,
                delay: 0,
                proposed: None,
                hold: DelayHold::default(),
                delay_changes: 0,
                late_events: 0,
                late_before: false,
                connected: Connected::default(),
            })
            .collect();
        let mut inputs = Self {
            api,
            counters,
            slots,
            read_offset: config.read_offset_samples,
            delay_max: config.input_delay_max_samples,
            confirm: config.input_delay_confirm,
            policy: DelayPolicy {
                read_offset: config.read_offset_samples,
                delay_max: config.input_delay_max_samples,
                hold: config.input_delay_hold,
                window: config.input_delay_window,
                margin: config.input_delay_margin_samples,
                margin_share: config.input_delay_margin_pct / 100.0,
            },
            stall: config.input_stall,
            show_path: config.state_dir.join("inputs.json"),
            last_delays: load_last_delays(&config.state_dir.join("input-delays.json")),
            last_delays_path: config.state_dir.join("input-delays.json"),
            last_delays_dirty: false,
            instances: HashMap::new(),
        };
        for i in 0..inputs.slots.len() {
            if let ExtraDelay::Ms(value) = inputs.slots[i].config.extra_delay {
                inputs.set_delay(engine, i, samples(value).min(inputs.delay_max));
            }
        }
        inputs
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    pub fn configs(&self) -> Vec<InputConfig> {
        self.slots.iter().map(|s| s.config.clone()).collect()
    }

    pub fn counters(&self) -> &Arc<EngineCounters> {
        &self.counters
    }

    fn set_delay(&mut self, engine: &mut Engine, index: usize, delay: u32) {
        self.slots[index].delay = delay;
        let slot = &self.slots[index];
        if let Some(route) = slot
            .route
            .as_ref()
            .filter(|_| slot.config.extra_delay == ExtraDelay::default())
        {
            if self.last_delays.insert(route.flow, delay) != Some(delay) {
                self.last_delays_dirty = true;
            }
        }
        engine.send(Command::SetDelay {
            index,
            samples: delay,
        });
    }

    /// Route input `index` from an IS-05 activation (`None`: stop reading).
    pub fn route(&mut self, engine: &mut Engine, index: usize, route: Option<Route>) {
        let Some(slot) = self.slots.get_mut(index) else {
            return;
        };
        if slot.route == route && slot.state != State::Unrouted {
            return;
        }
        if slot.route.is_some() {
            engine.send(Command::ClearInput {
                index,
                forget: true,
            });
        }
        let slot = &mut self.slots[index];
        // Another flow on an automatic delay starts over: from 0, or from that
        // flow's last delay (D242), not from the old flow's delay.
        let other_flow = slot.route.as_ref().map(|r| r.flow) != route.as_ref().map(|r| r.flow);
        if other_flow && slot.config.extra_delay == ExtraDelay::default() && slot.delay != 0 {
            slot.delay = 0;
            slot.proposed = None;
            engine.send(Command::SetDelay { index, samples: 0 });
        }
        let slot = &mut self.slots[index];
        slot.route = route;
        slot.inode = None;
        slot.lag.clear();
        slot.hold = DelayHold::default();
        slot.state = if slot.route.is_some() {
            State::Waiting
        } else {
            State::Unrouted
        };
        tracing::info!(input = index + 1, state = ?slot.state, flow = ?slot.route.as_ref().map(|r| r.flow), "input routed");
        if slot.route.is_some() {
            self.try_open(engine, index);
        }
    }

    fn try_open(&mut self, engine: &mut Engine, index: usize) -> bool {
        let Some(route) = self.slots[index].route.clone() else {
            return false;
        };
        // No flow (yet): nothing to open, no instance made and dropped per tick.
        let Some(inode) = mxlio::flow_inode(&route.domain, route.flow) else {
            return false;
        };
        // One instance per flow generation (flow id and data-file inode): libmxl
        // caches one reader per flow id per instance, so an instance shared
        // across a re-created flow could hand a re-open the old reader, mapped
        // on the deleted files, while the DSP thread still holds it. Inputs on
        // the same generation share it (each instance is an inotify instance
        // and a watcher thread; `fs.inotify.max_user_instances` is per uid).
        // A reader keeps its instance alive; both go when the control side
        // drops the reader the DSP thread returns.
        let key = (route.flow, inode);
        let instance = match self.instances.get(&key) {
            Some(instance) => instance.clone(),
            None => {
                match MxlInstance::new(self.api.clone(), &route.domain.display().to_string(), "") {
                    Ok(instance) => {
                        self.instances.insert(key, instance.clone());
                        instance
                    }
                    Err(err) => {
                        tracing::error!(
                            input = index + 1,
                            "cannot create an MXL instance for {} ({err}); each flow generation \
                         read needs one (an inotify instance: check fs.inotify.max_user_instances)",
                            route.domain.display()
                        );
                        return false;
                    }
                }
            }
        };
        let opened = mxlio::open_reader(&instance, route.flow);
        drop(instance);
        self.prune_instances(Some(key));
        match opened {
            Ok((reader, channels)) => {
                let inode = Some(inode);
                engine.send(Command::SetInput {
                    index,
                    reader: Box::new(reader),
                    channels: channels as usize,
                });
                let head = self.counters.inputs[index]
                    .head_index
                    .load(Ordering::Relaxed);
                let slot = &mut self.slots[index];
                let was = slot.state;
                slot.inode = inode;
                if was == State::Stalled {
                    // Still stalled until the head moves again.
                    slot.last_head = head;
                } else {
                    slot.state = State::Running;
                    slot.head_changed = Instant::now();
                    slot.last_head = head;
                }
                // Start at the delay this flow had last (D242): no losses while
                // the measurement settles; the start-up phase lets it fall fast.
                let auto = slot.config.extra_delay == ExtraDelay::default();
                let last = self.last_delays.get(&route.flow).copied();
                if let Some(last) =
                    last.filter(|_| auto && was == State::Waiting && slot.delay == 0)
                {
                    let last = last.min(self.delay_max);
                    tracing::info!(
                        input = index + 1,
                        delay_ms = ms(last),
                        "input delay of the last run"
                    );
                    self.set_delay(engine, index, last);
                    self.slots[index].hold.seed(last, Instant::now());
                }
                let slot = &mut self.slots[index];
                if was != State::Waiting {
                    slot.reopens += 1;
                }
                tracing::info!(input = index + 1, flow = %route.flow, channels, "input running");
                true
            }
            Err(_) => false,
        }
    }

    /// Keep only the instances of the flow generations the inputs read (and
    /// `extra`, about to be read); readers still in the DSP thread keep theirs.
    fn prune_instances(&mut self, extra: Option<(Uuid, u64)>) {
        if self.instances.is_empty() {
            return;
        }
        let used: Vec<(Uuid, u64)> = self
            .slots
            .iter()
            .filter(|s| s.state != State::Waiting && s.state != State::Unrouted)
            .filter_map(|s| Some((s.route.as_ref()?.flow, s.inode?)))
            .chain(extra)
            .collect();
        self.instances.retain(|k, _| used.contains(k));
    }

    /// The 100 ms tick.
    pub fn tick(&mut self, engine: &mut Engine) {
        let now = Instant::now();
        for i in 0..self.slots.len() {
            // The highest lag of this tick: a block above the read offset plus
            // the delay was late (or saved only by the read's short wait).
            let mut max_lag = None;
            while let Ok(lag) = engine.lags[i].pop() {
                self.slots[i].last_lag = lag;
                self.slots[i].lag.push(lag, now);
                max_lag = max_lag.max(Some(lag));
            }
            let slot = &mut self.slots[i];
            let applied = self.counters.inputs[i]
                .delay_samples
                .load(Ordering::Relaxed);
            let late = slot.state == State::Running
                && max_lag.is_some_and(|l| {
                    l > self.read_offset + applied && l <= self.read_offset + self.delay_max
                });
            slot.late_events += u64::from(late && !slot.late_before);
            slot.late_before = late;
            match self.slots[i].state {
                State::Unrouted => {}
                State::Waiting => {
                    self.try_open(engine, i);
                }
                State::Running | State::Stalled => self.watch(engine, i, now),
            }
            self.auto_delay(engine, i, now, max_lag);
        }
        self.prune_instances(None);
        if self.last_delays_dirty {
            self.last_delays_dirty = false;
            let body = serde_json::json!({ "version": 1, "delays_ms": self
                .last_delays
                .iter()
                .map(|(flow, d)| (flow.to_string(), ms(*d)))
                .collect::<std::collections::BTreeMap<_, _>>() });
            if let Err(e) = crate::state::write_atomic(
                &self.last_delays_path,
                &serde_json::to_vec_pretty(&body).unwrap_or_default(),
            ) {
                tracing::warn!("saving the input delays: {e}");
            }
        }
    }

    fn watch(&mut self, engine: &mut Engine, i: usize, now: Instant) {
        let Some(route) = self.slots[i].route.clone() else {
            return;
        };
        let inode = mxlio::flow_inode(&route.domain, route.flow);
        if inode.is_none() {
            tracing::info!(input = i + 1, flow = %route.flow, "input_flow_gone: waiting for the flow");
            engine.send(Command::ClearInput {
                index: i,
                forget: false,
            });
            let slot = &mut self.slots[i];
            slot.state = State::Waiting;
            slot.reopens += 1;
            return;
        }
        if inode != self.slots[i].inode {
            tracing::info!(input = i + 1, flow = %route.flow, "input flow re-created: re-opening");
            self.try_open(engine, i);
            return;
        }
        let head = self.counters.inputs[i].head_index.load(Ordering::Relaxed);
        let slot = &mut self.slots[i];
        if head != slot.last_head {
            slot.last_head = head;
            slot.head_changed = now;
            if slot.state == State::Stalled {
                slot.state = State::Running;
                tracing::info!(input = i + 1, "input_resumed");
            }
        } else if slot.state == State::Running
            && now.duration_since(slot.head_changed) >= self.stall
        {
            slot.state = State::Stalled;
            tracing::warn!(input = i + 1, "input_stalled");
            // The flow may have been re-created under the same inode: open it again once.
            self.try_open(engine, i);
        }
    }

    fn auto_delay(&mut self, engine: &mut Engine, i: usize, now: Instant, max_lag: Option<u32>) {
        let (policy, confirm) = (self.policy, self.confirm);
        let (read_offset, delay_max) = (policy.read_offset, policy.delay_max);
        let slot = &mut self.slots[i];
        if slot.state == State::Stalled {
            slot.hold.interrupt();
        }
        if slot.config.extra_delay != ExtraDelay::default() || slot.state != State::Running {
            return;
        }
        // The percentile after a second of lags; a late block counts at once.
        let p999 = (slot.lag.span() >= Duration::from_secs(MIN_LAG_SECONDS))
            .then(|| slot.lag.quantile_upto(0.999, read_offset + delay_max))
            .flatten();
        let current = slot.proposed.unwrap_or(slot.delay);
        let Some(target) = slot.hold.next(p999, max_lag, current, &policy, now) else {
            return;
        };
        if confirm {
            if slot.proposed != Some(target) {
                tracing::info!(
                    input = i + 1,
                    from_ms = ms(slot.delay),
                    to_ms = ms(target),
                    "input delay change proposed"
                );
            }
            slot.proposed = Some(target);
        } else {
            tracing::info!(
                input = i + 1,
                from_ms = ms(slot.delay),
                to_ms = ms(target),
                worst_lag_ms = slot.hold.worst().map(ms),
                "input delay set"
            );
            slot.delay_changes += 1;
            self.set_delay(engine, i, target);
        }
    }

    /// Apply a proposed automatic delay (confirmation mode).
    pub fn confirm_delay(&mut self, engine: &mut Engine, index: usize) -> Result<(), String> {
        let slot = self.slots.get_mut(index).ok_or("no such input")?;
        let target = slot.proposed.take().ok_or("no delay change is pending")?;
        slot.delay_changes += 1;
        self.set_delay(engine, index, target);
        Ok(())
    }

    /// Change an input's label or extra delay; saved in the show file.
    pub fn update(
        &mut self,
        engine: &mut Engine,
        index: usize,
        label: Option<String>,
        extra_delay: Option<ExtraDelay>,
    ) -> Result<InputConfig, String> {
        let delay_max = self.delay_max;
        let slot = self.slots.get_mut(index).ok_or("no such input")?;
        if let Some(label) = label {
            if label.len() > 64 {
                return Err("label must have at most 64 characters".into());
            }
            // An empty alias is the default name again (C2).
            slot.config.label = if label.trim().is_empty() {
                InputConfig::default_for(index).label
            } else {
                label
            };
        }
        if let Some(delay) = extra_delay {
            if let ExtraDelay::Ms(value) = delay {
                if !(0.0..=ms(delay_max)).contains(&value) {
                    return Err(format!(
                        "extra_delay must be auto or 0 to {} ms",
                        ms(delay_max)
                    ));
                }
            }
            slot.config.extra_delay = delay;
            slot.proposed = None;
            match delay {
                ExtraDelay::Ms(value) => self.set_delay(engine, index, samples(value)),
                ExtraDelay::Auto(_) => {
                    self.slots[index].lag.clear();
                    self.slots[index].hold = DelayHold::default();
                }
            }
        }
        let config = self.slots[index].config.clone();
        self.save()?;
        Ok(config)
    }

    /// Replace every input's settings (import).
    pub fn replace_configs(
        &mut self,
        engine: &mut Engine,
        configs: &[InputConfig],
    ) -> Result<(), String> {
        for c in configs {
            if c.input == 0 || c.input > self.slots.len() {
                return Err(format!("input {} does not exist", c.input));
            }
        }
        for c in configs {
            self.update(
                engine,
                c.input - 1,
                Some(c.label.clone()),
                Some(c.extra_delay),
            )?;
        }
        Ok(())
    }

    fn save(&self) -> Result<(), String> {
        save_configs(&self.show_path, &self.configs())
    }

    pub fn status(&self) -> Vec<InputStatus> {
        self.slots
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let c = &self.counters.inputs[i];
                let applied = c.delay_samples.load(Ordering::Relaxed);
                let lag = match (
                    s.lag.quantile(0.5),
                    s.lag.quantile(0.99),
                    s.lag.quantile(0.999),
                ) {
                    (Some(a), Some(b), Some(c)) => Some(ArrivalLag {
                        p50: ms(a),
                        p99: ms(b),
                        p999: ms(c),
                    }),
                    _ => None,
                };
                InputStatus {
                    input: i + 1,
                    label: s.config.label.clone(),
                    state: s.state,
                    flow: s.route.as_ref().map(|r| r.flow),
                    domain: s.route.as_ref().map(|r| r.domain.display().to_string()),
                    sender_id: s.route.as_ref().and(s.connected.sender_id),
                    connected_label: s.route.as_ref().and(s.connected.label.clone()),
                    channels: if s.state == State::Unrouted {
                        0
                    } else {
                        c.channels.load(Ordering::Relaxed)
                    },
                    extra_delay: s.config.extra_delay,
                    extra_delay_ms: ms(applied),
                    effective_delay_ms: ms(self.read_offset + applied),
                    proposed_delay_ms: s.proposed.map(ms),
                    lag_ms: ms(s.last_lag),
                    arrival_lag_ms: lag,
                    worst_lag_ms: (s.config.extra_delay == ExtraDelay::default())
                        .then(|| s.hold.worst())
                        .flatten()
                        .map(ms),
                    delay_changes: s.delay_changes,
                    late_events: s.late_events,
                    late_samples: c.late_samples.load(Ordering::Relaxed),
                    overrun_samples: c.overrun_samples.load(Ordering::Relaxed),
                    missing_samples: c.missing_samples.load(Ordering::Relaxed),
                    reopens: s.reopens,
                }
            })
            .collect()
    }

    /// Every input's route (index from 0).
    pub fn routes(&self) -> Vec<Option<Route>> {
        self.slots.iter().map(|s| s.route.clone()).collect()
    }

    /// Set what input `index` (from 0) is connected to (C2).
    pub fn set_connected(&mut self, index: usize, connected: Connected) {
        if let Some(s) = self.slots.get_mut(index) {
            s.connected = connected;
        }
    }

    /// Whether input `index` (from 0) runs and has at least `channels` channels.
    pub fn has_channels(&self, index: usize, channels: usize) -> bool {
        self.slots
            .get(index)
            .is_some_and(|s| s.state == State::Running)
            && self.counters.inputs[index].channels.load(Ordering::Relaxed) as usize >= channels
    }
}

/// The last automatic delays per flow (D242); a missing or bad file is empty.
fn load_last_delays(path: &std::path::Path) -> HashMap<Uuid, u32> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v["delays_ms"].as_object().cloned())
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| Some((k.parse().ok()?, samples(v.as_f64()?))))
                .collect()
        })
        .unwrap_or_default()
}

pub fn load_configs(path: &std::path::Path) -> Vec<InputConfig> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| serde_json::from_value(v["inputs"].clone()).ok())
        .unwrap_or_default()
}

pub fn save_configs(path: &std::path::Path, configs: &[InputConfig]) -> Result<(), String> {
    let body = serde_json::json!({"version": 1, "inputs": configs});
    crate::state::write_atomic(path, &serde_json::to_vec_pretty(&body).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lag_quantiles() {
        let mut w = LagWindow::new(4800);
        let t = Instant::now();
        for i in 0..1000u32 {
            w.push(if i == 999 { 1000 } else { 48 }, t);
        }
        assert_eq!(w.quantile(0.5), Some(52));
        assert_eq!(w.quantile(0.999), Some(52));
        assert_eq!(w.quantile(1.0), Some(1004));
        w.clear();
        assert_eq!(w.quantile(0.5), None);
    }

    /// The policy of the tests: offset 2 ms, maximum 150 ms, hold 30 s,
    /// window 60 s, margin 1 ms.
    const P: DelayPolicy = DelayPolicy {
        read_offset: 96,
        delay_max: 7200,
        hold: Duration::from_secs(30),
        window: Duration::from_secs(60),
        margin: 48,
        margin_share: 0.0,
    };

    /// Ticks at 10 Hz from a steady hold at delay 0: per tick the p99.9 and the
    /// highest lag. Returns the changes (tick, delay) and the late ticks.
    fn drive(
        mut h: DelayHold,
        mut current: u32,
        policy: &DelayPolicy,
        ticks: u64,
        lag: impl Fn(u64) -> (Option<u32>, Option<u32>),
    ) -> (Vec<(u64, u32)>, u32) {
        let t0 = Instant::now();
        let (mut changes, mut late) = (Vec::new(), 0);
        for k in 0..ticks {
            let now = t0 + Duration::from_millis(100 * k);
            let (p, m) = lag(k);
            late += u32::from(
                k > 0 && m.is_some_and(|m| m > policy.read_offset + current && m <= 7296),
            );
            if let Some(d) = h.next(p, m, current, policy, now) {
                current = d;
                changes.push((k, d));
            }
        }
        (changes, late)
    }

    #[test]
    fn auto_delay_does_not_flap() {
        // host-03 fabrics mirrors (D-263): p99.9 8.08 <-> 10.08 ms every 0.5 s.
        // v1.3.0 changed the delay 7.083 <-> 9.083 ms at every flip (with a fade).
        let (changes, _) = drive(DelayHold::steady(), 0, &P, 1200, |k| {
            (Some(if (k / 5) % 2 == 0 { 388 } else { 484 }), None)
        });
        assert_eq!(changes.len(), 2, "{changes:?}");
        assert_eq!(changes[0], (0, 388 + 48 - 96));
        assert_eq!(changes[1].1, 484 + 48 - 96, "up at once to the higher need");
        assert!(changes[1].0 <= 5);
    }

    #[test]
    fn auto_delay_goes_down_after_the_window_and_a_hold() {
        // 10 ms for 20 s, then 4 ms: down once, when the 10 ms left the window
        // (60 s) and the need then stayed lower for the hold (30 s).
        let (changes, _) = drive(DelayHold::steady(), 0, &P, 1500, |k| {
            let l = if k < 200 { 480 } else { 192 };
            (Some(l), Some(l))
        });
        assert_eq!(changes[0], (0, 432));
        assert_eq!(changes.len(), 2, "{changes:?}");
        // (A second's peak leaves the window up to a second early.)
        assert!(
            (190 + 600 + 300..=200 + 600 + 310).contains(&changes[1].0),
            "{changes:?}"
        );
        assert_eq!(changes[1].1, 144);
        // A need only slightly lower (0.5 ms) keeps the delay.
        let (changes, _) = drive(DelayHold::steady(), 0, &P, 1500, |k| {
            let l = if k < 200 { 480 } else { 456 };
            (Some(l), Some(l))
        });
        assert_eq!(changes, vec![(0, 432)]);
    }

    #[test]
    fn auto_delay_rises_at_once_but_not_for_a_small_creep() {
        let (changes, _) = drive(DelayHold::steady(), 0, &P, 200, |k| {
            let l = if k < 50 {
                96
            } else if k < 100 {
                110
            } else {
                300
            };
            (Some(l), Some(l))
        });
        // 96 + 48 - 96 = 48; 110 stays within half the margin; 300 rises at once.
        assert_eq!(changes, vec![(0, 48), (100, 252)]);
    }

    /// The gateway pattern of test-all (1.4.0): a steady lag (p99.9 3 ms) and
    /// once every `period_s` one block `outlier` samples late, until `until_s`.
    fn gateway(
        period_s: u64,
        outlier: u32,
        until_s: u64,
        ticks: u64,
        policy: &DelayPolicy,
    ) -> (Vec<(u64, u32)>, u32) {
        drive(DelayHold::steady(), 0, policy, ticks, |k| {
            let lag = if k > 0 && k % (period_s * 10) == 0 && k < until_s * 10 {
                144 + outlier
            } else {
                144
            };
            (Some(144), Some(lag))
        })
    }

    #[test]
    fn a_recurring_outlier_costs_one_block() {
        // +6 ms once every 30 s for 10 min: one late block (the first outlier),
        // one rise to cover it, no fall while the outliers go on; after the last
        // one down after the window and a hold.
        let (changes, late) = gateway(30, 288, 600, 9000, &P);
        assert_eq!(late, 1);
        assert_eq!(changes[..2], [(0, 96), (300, 384)], "{changes:?}");
        assert_eq!(changes.len(), 3, "{changes:?}");
        assert!((5700 + 600 + 300..=5700 + 600 + 320).contains(&changes[2].0));
        assert_eq!(changes[2].1, 96);
        // Every 4 min with the default 5 min window: still one.
        let p = DelayPolicy {
            window: Duration::from_secs(300),
            ..P
        };
        let (_, late) = gateway(240, 288, 3600, 36_000, &p);
        assert_eq!(late, 1);
    }

    #[test]
    fn a_one_off_late_block_costs_the_window_and_a_hold() {
        let (changes, late) = gateway(30, 288, 31, 3000, &P);
        assert_eq!(late, 1);
        assert_eq!(changes.len(), 3, "{changes:?}");
        let up = changes[1].0;
        assert!((up + 900..=up + 910).contains(&changes[2].0), "{changes:?}");
    }

    #[test]
    fn a_tail_with_spikes_is_covered_by_the_worst_of_the_window() {
        // A mirror: 14 ms steady, a 20 ms spike every 20 s, and once a 24 ms
        // one: the first of each size is late, then nothing.
        let (changes, late) = drive(DelayHold::steady(), 0, &P, 6000, |k| {
            let l = match k {
                _ if k == 1500 => 1152,
                _ if k % 200 == 100 => 960,
                _ => 672,
            };
            (Some(672), Some(l))
        });
        assert_eq!(late, 2, "{changes:?}");
        assert_eq!(changes.last().unwrap().1, 960 + 48 - 96, "{changes:?}");
    }

    #[test]
    fn lags_beyond_the_limit_move_no_delay() {
        // After a stall the first lags are seconds (beyond offset + maximum):
        // a writer catching up, not an outlier (D230).
        let t0 = Instant::now();
        let mut h = DelayHold::steady();
        assert_eq!(h.next(Some(144), Some(144), 96, &P, t0), None);
        for lag in [96_000, 48_000, 7297] {
            assert_eq!(h.next(Some(144), Some(lag), 96, &P, t0), None);
        }
        assert_eq!(h.worst(), Some(144));
    }

    #[test]
    fn the_last_delays_load_by_flow() {
        let dir = std::env::temp_dir().join(format!("amx-delays-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("input-delays.json");
        let flow = Uuid::new_v4();
        std::fs::write(
            &path,
            format!(r#"{{"version": 1, "delays_ms": {{"{flow}": 14.0, "x": 3.0}}}}"#),
        )
        .unwrap();
        let delays = load_last_delays(&path);
        assert_eq!(delays.get(&flow), Some(&672));
        assert_eq!(delays.len(), 1);
        assert!(load_last_delays(&dir.join("missing.json")).is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A fresh hold (start-up phase), p99.9 14 ms; `seed`: the last run's
    /// delay. Returns the changes and the late ticks.
    fn start(ticks: u64, seed: Option<u32>, lag: impl Fn(u64) -> u32) -> (Vec<(u64, u32)>, u32) {
        let mut h = DelayHold::default();
        let current = seed.unwrap_or(0);
        if let Some(d) = seed {
            h.seed(d, Instant::now());
        }
        drive(h, current, &P, ticks, |k| (Some(672), Some(lag(k))))
    }

    #[test]
    fn the_start_up_phase_settles_fast() {
        // A fresh mirror input at 14 ms with a 75 ms outlier 2 s in (a backlog):
        // up at once, back to the steady need within about 10 s, not after the
        // window. A second late block (60 ms at 30 s) ends the start-up phase:
        // from then on the window keeps it.
        let steady = 672 + 48 - 96;
        let (changes, _) = start(1200, None, |k| match k {
            20 => 3600,
            300 => 2880,
            _ => 672,
        });
        assert_eq!(changes[0], (0, steady));
        assert_eq!(changes[1], (20, 3600 + 48 - 96));
        assert!(
            changes[2].0 <= 20 + 110 && changes[2].1 == steady,
            "{changes:?}"
        );
        assert_eq!(changes[3..], [(300, 2880 + 48 - 96)], "{changes:?}");
        // A single backlog alone: down within about 10 s and stays there.
        let (changes, _) = start(1200, None, |k| if k == 20 { 3600 } else { 672 });
        assert_eq!(changes.len(), 3, "{changes:?}");
        assert_eq!(changes.last().unwrap().1, steady, "{changes:?}");
    }

    #[test]
    fn the_last_runs_delay_covers_the_first_outliers() {
        // A gateway with +6 ms every 30 s, started at its last run's 6 ms delay:
        // nothing late at the start nor later (1.4.2 without the seed fell to the
        // steady need after 5 s and lost the next outlier).
        let gw = |k: u64| if k % 300 == 150 { 144 + 288 } else { 144 };
        let mut h = DelayHold::default();
        h.seed(384, Instant::now());
        let (changes, late) = drive(h, 384, &P, 6000, |k| (Some(144), Some(gw(k))));
        assert_eq!(late, 0);
        assert!(changes.iter().all(|c| c.1 == 384), "{changes:?}");
        // A stale high delay from the last run (100 ms, nothing that big now):
        // down after its hold time and a start-up hold, about 35 s.
        let (changes, late) = start(600, Some(4800), |_| 672);
        assert_eq!(late, 0);
        assert!(
            changes
                .first()
                .is_some_and(|&(k, d)| (300..=360).contains(&k) && d == 624),
            "{changes:?}"
        );
    }

    #[test]
    fn a_stall_restarts_the_lower_period() {
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let mut h = DelayHold::steady();
        assert_eq!(h.next(Some(144), Some(432), 96, &P, at(0)), Some(384));
        // The 9 ms lag leaves the window at 60 s; stalled at 70 s: the lower
        // period starts again after it, the fall comes 30 s later.
        assert_eq!(h.next(Some(144), Some(144), 384, &P, at(65)), None);
        h.interrupt();
        assert_eq!(h.next(Some(144), Some(144), 384, &P, at(80)), None);
        assert_eq!(h.next(Some(144), Some(144), 384, &P, at(109)), None);
        assert_eq!(h.next(Some(144), Some(144), 384, &P, at(110)), Some(96));
    }

    #[test]
    fn a_new_route_starts_over() {
        // `route` and `extra_delay: auto` start the hold over (a new DelayHold):
        // the old flow's worst lag is gone, the start-up hold applies.
        let t0 = Instant::now();
        let mut h = DelayHold::steady();
        assert_eq!(h.next(Some(144), Some(432), 96, &P, t0), Some(384));
        h = DelayHold::default();
        let at = |s: u64| t0 + Duration::from_secs(s);
        assert_eq!(h.next(Some(144), Some(144), 384, &P, at(1)), None);
        assert_eq!(h.next(Some(144), Some(144), 384, &P, at(6)), Some(96));
    }

    #[test]
    fn confirm_mode_proposes_once_for_a_recurring_outlier() {
        // With MIXER_INPUT_DELAY_CONFIRM the delay stays until confirmed; the
        // hold decides against the proposal, so the same outlier again proposes
        // nothing new.
        let t0 = Instant::now();
        let mut h = DelayHold::steady();
        let (applied, mut proposed) = (96u32, None);
        let mut proposals = Vec::new();
        for k in 0..3000u64 {
            let lag = if k % 300 == 150 { 432 } else { 144 };
            let current = proposed.unwrap_or(applied);
            let now = t0 + Duration::from_millis(100 * k);
            if let Some(t) = h.next(Some(144), Some(lag), current, &P, now) {
                proposed = Some(t);
                proposals.push((k, t));
            }
        }
        assert_eq!(proposals, vec![(150, 384)]);
    }

    #[test]
    fn a_late_block_counts_before_the_first_second() {
        let t0 = Instant::now();
        let mut h = DelayHold::steady();
        assert_eq!(h.next(None, Some(400), 0, &P, t0), Some(352));
        assert_eq!(h.next(None, None, 352, &P, t0), None);
        // Capped at the maximum.
        assert_eq!(h.next(None, Some(7296), 352, &P, t0), Some(7200));
    }

    #[test]
    fn lag_window_span_needs_a_second_of_samples() {
        let mut w = LagWindow::new(4800);
        let t = Instant::now();
        assert_eq!(w.span(), Duration::ZERO);
        w.push(48, t);
        assert!(w.span() < Duration::from_secs(MIN_LAG_SECONDS), "one tick");
        w.push(48, t + Duration::from_millis(900));
        assert!(w.span() < Duration::from_secs(MIN_LAG_SECONDS));
        w.push(48, t + Duration::from_millis(1000));
        assert!(w.span() >= Duration::from_secs(MIN_LAG_SECONDS));
    }

    #[test]
    fn extra_delay_json() {
        assert_eq!(
            serde_json::to_string(&ExtraDelay::default()).unwrap(),
            "\"auto\""
        );
        assert_eq!(
            serde_json::from_str::<ExtraDelay>("12.5").unwrap(),
            ExtraDelay::Ms(12.5)
        );
        assert_eq!(
            serde_json::from_str::<ExtraDelay>("\"auto\"").unwrap(),
            ExtraDelay::default()
        );
        assert!(serde_json::from_str::<ExtraDelay>("\"x\"").is_err());
    }

    /// A replicated flow (D-347): grains of `grain` samples, each committed
    /// `base` + a log-normal delay (median `median`, shape `sigma`, samples)
    /// after its last sample, and now and then (`spikes_per_h`) a stall of
    /// `spike` samples (uniform in the range); commits stay in order. Yields
    /// the arrival lag of every 2 ms block (block clock index − head).
    pub(super) struct Writer {
        rng: u64,
        grain: u64,
        median: f64,
        sigma: f64,
        spike_p: f64,
        spike: (f64, f64),
        next_grain: u64,
        next_commit: u64,
        head: u64,
        pending: std::collections::VecDeque<(u64, u64)>,
    }

    impl Writer {
        pub(super) fn new(
            seed: u64,
            grain: u64,
            median_ms: f64,
            sigma: f64,
            spikes_per_h: f64,
            spike_ms: (f64, f64),
        ) -> Self {
            let grains_per_h = 3600.0 * 48_000.0 / grain as f64;
            Self {
                rng: seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1,
                grain,
                median: median_ms * 48.0,
                sigma,
                spike_p: spikes_per_h / grains_per_h,
                spike: (spike_ms.0 * 48.0, spike_ms.1 * 48.0),
                next_grain: 0,
                next_commit: 0,
                head: 0,
                pending: Default::default(),
            }
        }

        fn uniform(&mut self) -> f64 {
            self.rng ^= self.rng << 13;
            self.rng ^= self.rng >> 7;
            self.rng ^= self.rng << 17;
            (self.rng >> 11) as f64 / (1u64 << 53) as f64
        }

        fn normal(&mut self) -> f64 {
            let (a, b) = (self.uniform().max(1e-300), self.uniform());
            (-2.0 * a.ln()).sqrt() * (std::f64::consts::TAU * b).cos()
        }

        /// The lag at block clock index `now` (samples).
        pub(super) fn lag(&mut self, now: u64) -> u32 {
            // Commit times of the grains up to a second ahead.
            while self.next_grain * self.grain < now + 48_000 {
                let end = (self.next_grain + 1) * self.grain;
                let mut d = self.median * (self.sigma * self.normal()).exp();
                if self.uniform() < self.spike_p {
                    let u = self.uniform();
                    d += self.spike.0 + u * (self.spike.1 - self.spike.0);
                }
                let at = (end + d as u64).max(self.next_commit);
                self.next_commit = at;
                self.pending.push_back((at, end));
                self.next_grain += 1;
            }
            while self.pending.front().is_some_and(|(at, _)| *at <= now) {
                self.head = self.pending.pop_front().unwrap().1;
            }
            now.saturating_sub(self.head).min(u64::from(u32::MAX)) as u32
        }
    }

    /// Late blocks, dropouts (runs of late blocks), the mean and the last
    /// delay (samples) of `policy` over `seconds` of `writer` (2 ms blocks,
    /// a 100 ms tick, read offset 96, maximum 7200), and the lag quantiles.
    pub(super) fn simulate(
        writer: &mut Writer,
        seconds: u64,
        mut policy: impl FnMut(Option<u32>, Option<u32>, u32, Instant) -> Option<u32>,
    ) -> (u64, u64, f64, u32, (f64, f64, f64)) {
        let t0 = Instant::now();
        let mut window = LagWindow::new(96 + 7200 + 4800);
        let mut all = vec![0u64; 2000];
        let (mut late, mut dropouts, mut was_late) = (0u64, 0u64, false);
        let (mut delay, mut delay_sum, mut ticks) = (0u32, 0f64, 0u64);
        let mut max_lag = None;
        for b in 1..=seconds * 500 {
            let now_index = b * 96;
            let lag = writer.lag(now_index);
            let now = t0 + Duration::from_micros(b * 2000);
            window.push(lag, now);
            all[((lag / 24) as usize).min(1999)] += 1;
            max_lag = max_lag.max(Some(lag));
            // The first 10 s settle (a start is late anyway).
            let is_late = lag > 96 + delay && b > 5000;
            late += u64::from(is_late);
            dropouts += u64::from(is_late && !was_late);
            was_late = is_late;
            if b % 50 == 0 {
                let p999 = (window.span() >= Duration::from_secs(1))
                    .then(|| window.quantile_upto(0.999, 96 + 7200))
                    .flatten();
                if let Some(d) = policy(p999, max_lag.take(), delay, now) {
                    delay = d;
                }
                if b > 5000 {
                    delay_sum += f64::from(delay);
                    ticks += 1;
                }
            }
        }
        let total: u64 = all.iter().sum();
        let q = |q: f64| {
            let target = (total as f64 * q).ceil() as u64;
            let mut seen = 0;
            for (i, c) in all.iter().enumerate() {
                seen += c;
                if seen >= target {
                    return ms((i as u32 + 1) * 24);
                }
            }
            0.0
        };
        (
            late,
            dropouts,
            ms((delay_sum / ticks.max(1) as f64) as u32),
            delay,
            (q(0.5), q(0.99), q(0.999)),
        )
    }

    /// D-347 in numbers: `cargo test --release --lib d347 -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn d347_replicated_flow() {
        let args = |name: &str, default: f64| {
            std::env::var(name)
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(default)
        };
        let hours = args("SIM_H", 1.0);
        for seed in 1..=args("SIM_SEEDS", 3.0) as u64 {
            let mut w = Writer::new(
                seed,
                args("SIM_GRAIN", 210.0) as u64,
                args("SIM_MEDIAN_MS", 8.5),
                args("SIM_SIGMA", 0.30),
                args("SIM_SPIKES_H", 30.0),
                (args("SIM_SPIKE_LO", 10.0), args("SIM_SPIKE_HI", 22.0)),
            );
            let mut hold = DelayHold::default();
            let policy = DelayPolicy {
                window: Duration::from_secs_f64(args("SIM_WINDOW_S", 300.0)),
                margin: samples(args("SIM_MARGIN_MS", 1.0)),
                margin_share: args("SIM_MARGIN_PCT", 20.0) / 100.0,
                ..P
            };
            let r = simulate(&mut w, (hours * 3600.0) as u64, |p, m, c, now| {
                hold.next(p, m, c, &policy, now)
            });
            println!("new seed {seed}: late blocks {} dropouts {} mean delay {:.1} ms last {:.1} ms lag p50/p99/p99.9 {:?}", r.0, r.1, r.2 + 2.0, ms(r.3) + 2.0, r.4);
        }
    }
}
