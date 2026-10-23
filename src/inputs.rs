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
/// Margin added to the 99.9th percentile (1 ms).
pub const DELAY_MARGIN_SAMPLES: u32 = 48;
/// The automatic delay goes up once less than this is left above the 99.9th
/// percentile (0.5 ms), so a slowly growing lag does not fade at every step.
const DELAY_RAISE_HEADROOM_SAMPLES: u32 = DELAY_MARGIN_SAMPLES / 2;
/// The automatic delay goes down only when the need is at least this much lower
/// (1 ms), for `MIXER_INPUT_DELAY_HOLD_S`.
const DELAY_LOWER_SAMPLES: u32 = 48;
/// The start-up phase of an input (from its first automatic decision after a
/// route or a start): the delay settles fast, a late block's short floor and
/// the period of lower need last [`STARTUP_HOLD`] instead of the hold time. An
/// outlier that recurs (also one the size of the last run's delay) still gets
/// its recurring floor.
const STARTUP: Duration = Duration::from_secs(60);
/// The hold time in the start-up phase.
const STARTUP_HOLD: Duration = Duration::from_secs(5);
/// How many hold times the need of a late block keeps the automatic delay up.
const LATE_MEMORY_HOLDS: u32 = 10;
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

/// Hysteresis and hold of the automatic delay (F2): up at once when the lag
/// comes close to the delay, or when a block was late (its lag above the read
/// offset plus the delay: a rare outlier the 99.9th percentile does not show);
/// down only after the need stayed lower by [`DELAY_LOWER_SAMPLES`] for the
/// whole hold time, to the highest need seen in that time, and at most once per
/// hold time. The need of a late block stays a floor for one hold time. When an
/// outlier of about that size (within 1 ms and a block) comes again within
/// [`LATE_MEMORY_HOLDS`] hold times of the last one (late again, or now
/// covered), it recurs: its floor then lasts that long (5 min by default),
/// renewed by every recurrence, so a rare outlier, e.g. once a minute, is lost
/// once instead of once per hold time. A one-off late block, or a single spike
/// above a recurring outlier, costs its latency for about two hold times.
/// Lags above the read offset plus the maximum delay are no outliers but a
/// writer catching up (a stall, D230) and count for nothing here.
#[derive(Default)]
struct DelayHold {
    last_change: Option<Instant>,
    /// Since when the need has been lower, and the highest need since then.
    lower: Option<(Instant, u32)>,
    /// The floor of a late block that does not recur: when, and its need (one
    /// hold time).
    short: Option<(Instant, u32)>,
    /// The floor of a recurring outlier: when it last came, and its need (the
    /// memory time, renewed by each recurrence of about that size).
    long: Option<(Instant, u32)>,
    /// The last outlier that was late or recurred, within the memory time: when,
    /// and its need.
    last_late: Option<(Instant, u32)>,
    /// The DSP block (samples): lags are measured at block ends, so the same
    /// outlier reads up to a block apart.
    block: u32,
    /// The first decision: the start-up phase runs [`STARTUP`] from it.
    started: Option<Instant>,
    /// Outlier events seen in the start-up phase (late or covered after a late
    /// one), and whether the last tick had an outlier.
    startup_outliers: u8,
    outlier_before: bool,
    /// The delay of the last run (D242): a floor for one (normal) hold time
    /// from the start, and an outlier of that size recurs at once.
    seeded: Option<(Instant, u32)>,
}

impl DelayHold {
    fn new(block: u32) -> Self {
        Self {
            block,
            ..Self::default()
        }
    }

    /// Start from the delay of the last run.
    fn seed(&mut self, delay: u32, now: Instant) {
        self.seeded = Some((now, delay));
        self.last_late = Some((now, delay));
    }

    /// A hold past its start-up phase (tests of the steady state).
    #[cfg(test)]
    fn steady(block: u32) -> Self {
        Self {
            started: Some(Instant::now() - 2 * STARTUP),
            ..Self::new(block)
        }
    }

    /// The higher of the two late-block floors (0 without one).
    fn floor(&self) -> u32 {
        let n = |f: Option<(Instant, u32)>| f.map_or(0, |(_, n)| n);
        n(self.short).max(n(self.long)).max(n(self.seeded))
    }

    /// A stall interrupts a period of lower need: it starts again afterwards.
    fn interrupt(&mut self) {
        self.lower = None;
    }

    /// The new delay for an input with delay `current`, or `None` to keep it.
    /// `p999` is the arrival lag's 99.9th percentile (`None` before a second of
    /// lags), `max_lag` the highest lag of the blocks since the last call; one
    /// above `read_offset + current` was late.
    #[allow(clippy::too_many_arguments)]
    fn next(
        &mut self,
        p999: Option<u32>,
        max_lag: Option<u32>,
        current: u32,
        read_offset: u32,
        delay_max: u32,
        hold: Duration,
        now: Instant,
    ) -> Option<u32> {
        let need_of = |lag: u32| {
            (lag + DELAY_MARGIN_SAMPLES)
                .saturating_sub(read_offset)
                .min(delay_max)
        };
        if self
            .seeded
            .is_some_and(|(t, _)| now.duration_since(t) >= hold)
        {
            self.seeded = None;
        }
        let started = *self.started.get_or_insert(now);
        let memory = hold * LATE_MEMORY_HOLDS;
        // The start-up hold only until a second outlier: outliers that come again,
        // also of other sizes, are no backlog, the normal hold keeps them covered.
        let startup = now.duration_since(started) < STARTUP && self.startup_outliers < 2;
        let hold = if startup {
            hold.min(STARTUP_HOLD)
        } else {
            hold
        };
        let expire = |f: &mut Option<(Instant, u32)>, after: Duration| {
            if f.is_some_and(|(t, _)| now.duration_since(t) >= after) {
                *f = None;
            }
        };
        expire(&mut self.short, hold);
        expire(&mut self.long, memory);
        expire(&mut self.last_late, memory);
        if let Some(lag) = max_lag.filter(|&l| l <= read_offset + delay_max) {
            let need = need_of(lag);
            let late = lag > read_offset + current;
            // An outlier needs at least 1 ms more than the 99.9th percentile.
            // Before the first second (no percentile yet) a late block counts as one.
            let outlier = p999.map_or(late, |p| need > need_of(p) + DELAY_LOWER_SAMPLES);
            // An outlier event starts on a tick after one without: a backlog
            // draining over many ticks is one event, rare outliers are each one.
            // Not in the first second: a backlog then is no repeated outlier.
            let event = outlier && !self.outlier_before && p999.is_some();
            self.outlier_before = outlier;
            if event && (late || self.last_late.is_some()) && now.duration_since(started) < STARTUP
            {
                self.startup_outliers = self.startup_outliers.saturating_add(1);
            }
            // It recurs when it needs about as much (within 1 ms and a block) as the
            // recurring floor or the last outlier: another size, a smaller tail
            // or a single higher spike, renews nothing and does not ratchet.
            let near = |f: Option<(Instant, u32)>| {
                f.map(|(_, n)| n)
                    .filter(|&n| need.abs_diff(n) <= DELAY_LOWER_SAMPLES + self.block)
            };
            let matched = near(self.long).max(near(self.last_late));
            let recurs = outlier && matched.is_some();
            if recurs {
                self.long = Some((now, need.max(matched.unwrap_or(0))));
            } else if late {
                let before = self.short.map_or(0, |(_, n)| n);
                self.short = Some((now, need.max(before)));
            }
            if outlier && (late || recurs) {
                self.last_late = Some((now, need));
            }
            if late {
                let floor = need.max(self.floor());
                if floor > current {
                    self.lower = None;
                    self.last_change = Some(now);
                    return Some(floor);
                }
            }
        }
        let p999 = p999?;
        let need = need_of(p999).max(self.floor());
        if p999 + DELAY_RAISE_HEADROOM_SAMPLES > read_offset + current && need > current {
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
    pub channels: u32,
    pub extra_delay: ExtraDelay,
    pub extra_delay_ms: f64,
    pub effective_delay_ms: f64,
    pub proposed_delay_ms: Option<f64>,
    pub lag_ms: f64,
    pub arrival_lag_ms: Option<ArrivalLag>,
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
    hold: Duration,
    stall: Duration,
    show_path: PathBuf,
    /// The last automatic delay per flow (samples), kept in
    /// `MIXER_STATE_DIR/input-delays.json`: an input starts at it (D242).
    last_delays: HashMap<Uuid, u32>,
    last_delays_path: PathBuf,
    last_delays_dirty: bool,
    /// The DSP block in samples.
    block: u32,
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
                hold: DelayHold::new(config.block_samples),
            })
            .collect();
        let mut inputs = Self {
            api,
            counters,
            slots,
            read_offset: config.read_offset_samples,
            delay_max: config.input_delay_max_samples,
            confirm: config.input_delay_confirm,
            hold: config.input_delay_hold,
            stall: config.input_stall,
            show_path: config.state_dir.join("inputs.json"),
            last_delays: load_last_delays(&config.state_dir.join("input-delays.json")),
            last_delays_path: config.state_dir.join("input-delays.json"),
            last_delays_dirty: false,
            instances: HashMap::new(),
            block: config.block_samples,
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
            engine.send(Command::ClearInput { index });
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
        slot.hold = DelayHold::new(self.block);
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
            engine.send(Command::ClearInput { index: i });
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
        let (read_offset, delay_max, hold, confirm) =
            (self.read_offset, self.delay_max, self.hold, self.confirm);
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
        let Some(target) =
            slot.hold
                .next(p999, max_lag, current, read_offset, delay_max, hold, now)
        else {
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
                "input delay set"
            );
            self.set_delay(engine, i, target);
        }
    }

    /// Apply a proposed automatic delay (confirmation mode).
    pub fn confirm_delay(&mut self, engine: &mut Engine, index: usize) -> Result<(), String> {
        let slot = self.slots.get_mut(index).ok_or("no such input")?;
        let target = slot.proposed.take().ok_or("no delay change is pending")?;
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
            if label.trim().is_empty() || label.len() > 64 {
                return Err("label must have 1 to 64 characters".into());
            }
            slot.config.label = label;
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
                    self.slots[index].hold = DelayHold::new(self.block);
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
                    late_samples: c.late_samples.load(Ordering::Relaxed),
                    overrun_samples: c.overrun_samples.load(Ordering::Relaxed),
                    missing_samples: c.missing_samples.load(Ordering::Relaxed),
                    reopens: s.reopens,
                }
            })
            .collect()
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

    /// The delays an input takes when its p99.9 (samples) follows `p999` at 10 Hz.
    fn delays(p999: impl Fn(u64) -> u32, ticks: u64, hold_s: u64) -> Vec<(u64, u32)> {
        let t0 = Instant::now();
        let mut hold = DelayHold::steady(0);
        let mut current = 0;
        let mut changes = Vec::new();
        for k in 0..ticks {
            let now = t0 + Duration::from_millis(100 * k);
            if let Some(d) = hold.next(
                Some(p999(k)),
                None,
                current,
                96,
                7200,
                Duration::from_secs(hold_s),
                now,
            ) {
                current = d;
                changes.push((k, d));
            }
        }
        changes
    }

    #[test]
    fn auto_delay_does_not_flap() {
        // host-03 fabrics mirrors (D-263): p99.9 8.08 <-> 10.08 ms every 0.5 s.
        // v1.3.0 changed the delay 7.083 <-> 9.083 ms at every flip (with a fade).
        let changes = delays(|k| if (k / 5) % 2 == 0 { 388 } else { 484 }, 1200, 30);
        assert_eq!(changes.len(), 2, "{changes:?}");
        assert_eq!(changes[0], (0, 388 + 48 - 96));
        assert_eq!(changes[1].1, 484 + 48 - 96, "up at once to the higher need");
        assert!(changes[1].0 <= 5);
    }

    #[test]
    fn auto_delay_goes_down_after_the_hold_by_a_margin() {
        // 10 ms for 20 s, then 4 ms: down once, 30 s after the need went down.
        let changes = delays(|k| if k < 200 { 480 } else { 192 }, 1200, 30);
        assert_eq!(changes, vec![(0, 432), (500, 144)]);
        // A need only slightly lower (0.5 ms) keeps the delay.
        let changes = delays(|k| if k < 200 { 480 } else { 456 }, 1200, 30);
        assert_eq!(changes, vec![(0, 432)]);
        // Lower in steps: the delay follows the highest need of the hold time,
        // and changes at most once per hold time.
        let changes = delays(|k| 480u32.saturating_sub(k as u32 / 5), 1200, 30);
        assert_eq!(changes.len(), 4, "{changes:?}");
        for w in changes.windows(2) {
            assert!(w[1].0 - w[0].0 >= 300, "{changes:?}");
        }
    }

    #[test]
    fn auto_delay_rises_at_once_but_not_for_a_small_creep() {
        let changes = delays(
            |k| {
                if k < 50 {
                    96
                } else if k < 100 {
                    110
                } else {
                    300
                }
            },
            200,
            30,
        );
        // 96 + 48 - 96 = 48; 110 stays within the 0.5 ms headroom; 300 rises at once.
        assert_eq!(changes, vec![(0, 48), (100, 252)]);
    }

    /// The gateway pattern of test-all (1.4.0): a steady lag (p99.9 3 ms) and
    /// once every `period_s` one block `outlier` samples late, until `until_s`.
    /// Returns the late blocks and the delay changes (tick, delay) at 10 Hz.
    fn gateway(period_s: u64, outlier: u32, until_s: u64, ticks: u64) -> (u32, Vec<(u64, u32)>) {
        let t0 = Instant::now();
        let (mut hold, mut current, mut misses, mut changes) =
            (DelayHold::steady(0), 0, 0, Vec::new());
        for k in 0..ticks {
            let now = t0 + Duration::from_millis(100 * k);
            let lag = if k > 0 && k % (period_s * 10) == 0 && k < until_s * 10 {
                144 + outlier
            } else {
                144
            };
            // The first tick sets the delay (the start is late anyway).
            misses += u32::from(k > 0 && lag > 96 + current);
            if let Some(d) = hold.next(
                Some(144),
                Some(lag),
                current,
                96,
                7200,
                Duration::from_secs(30),
                now,
            ) {
                current = d;
                changes.push((k, d));
            }
        }
        (misses, changes)
    }

    #[test]
    fn a_late_block_raises_the_delay_at_once_and_it_stays() {
        // +6 ms once every 30 s for 10 min: one late block (the first outlier),
        // one rise to cover it, no fall while the outliers go on.
        let (misses, changes) = gateway(30, 288, 600, 6000);
        assert_eq!(misses, 1, "{changes:?}");
        assert_eq!(changes, vec![(0, 96), (300, 144 + 288 + 48 - 96)]);
        // Every 50 s: the same (it comes again before the one-off floor and the
        // following hold are over, so it recurs).
        let (misses, changes) = gateway(50, 288, 600, 6000);
        assert_eq!(misses, 1, "{changes:?}");
        assert_eq!(changes.len(), 2, "{changes:?}");
        // Every 4 min: the first is a one-off (down after 2 hold times); the
        // second, within the memory, recurs and costs one more block; then it
        // stays covered.
        let (misses, changes) = gateway(240, 288, 1500, 15000);
        assert_eq!(misses, 2, "{changes:?}");
        assert_eq!(changes, vec![(0, 96), (2400, 384), (3000, 96), (4800, 384)]);
    }

    #[test]
    fn a_one_off_late_block_costs_two_hold_times() {
        // One outlier at 30 s: up at once, down 2 hold times later.
        let (misses, changes) = gateway(30, 288, 31, 3000);
        assert_eq!(misses, 1);
        assert_eq!(changes, vec![(0, 96), (300, 384), (900, 96)]);
    }

    #[test]
    fn after_the_last_late_block_the_delay_falls_after_the_memory_and_a_hold() {
        // Outliers until 100 s (the last at 90 s): down once, 5 min + 30 s after
        // the last one, to the steady need.
        let (misses, changes) = gateway(30, 288, 100, 6000);
        assert_eq!(misses, 1);
        assert_eq!(changes.len(), 3, "{changes:?}");
        assert_eq!(changes[2], (900 + 3000 + 300, 96));
    }

    #[test]
    fn a_mirror_with_rare_outliers_above_the_offset_is_covered() {
        // test-all (1.4.0, offset 20 ms): replay audio over an RDMA mirror, a
        // steady 8-12 ms with rare outliers of 20-30 ms; 1.4.0 lost them and
        // changed the delay 9 times in 10 min.
        let t0 = Instant::now();
        let (offset, hold) = (960, Duration::from_secs(30));
        let (mut h, mut current, mut misses, mut changes) = (DelayHold::steady(0), 0, 0, 0);
        for k in 0..6000u64 {
            let now = t0 + Duration::from_millis(100 * k);
            let p999 = 576; // 12 ms
            let lag = match k % 470 {
                100 => 960 + 400, // 28 ms
                300 => 960 + 100, // 22 ms
                _ => 480 + (k % 7) as u32 * 20,
            };
            misses += u32::from(k > 10 && lag > offset + current);
            if let Some(d) = h.next(Some(p999), Some(lag), current, offset, 7200, hold, now) {
                current = d;
                changes += 1;
            }
        }
        assert_eq!(misses, 1, "only the first 28 ms outlier");
        assert_eq!(current, 960 + 400 + 48 - 960);
        assert!(changes <= 2, "{changes} changes");
    }

    /// Runs the hold over `ticks` ticks (10 Hz) with p99.9 3 ms, offset 2 ms;
    /// `lag(k)` is the highest lag of tick k. Returns the delay per tick.
    fn run(ticks: u64, lag: impl Fn(u64) -> u32) -> Vec<u32> {
        let t0 = Instant::now();
        let (mut h, mut current, mut out) = (DelayHold::steady(0), 96, Vec::new());
        for k in 0..ticks {
            let now = t0 + Duration::from_millis(100 * k);
            if let Some(d) = h.next(
                Some(144),
                Some(lag(k)),
                current,
                96,
                7200,
                Duration::from_secs(30),
                now,
            ) {
                current = d;
            }
            out.push(current);
        }
        out
    }

    #[test]
    fn one_spike_then_small_outliers_does_not_ratchet() {
        // Review probe A: one 6 ms spike at 30 s, then outliers 1.25 ms above the
        // p99.9 every 20 s for 20 min. The spike's floor goes after ~2 hold times;
        // the small outliers keep only their own need.
        let d = run(12_000, |k| match k {
            300 => 144 + 288,
            k if k > 300 && k % 200 == 0 => 144 + 60,
            _ => 144,
        });
        assert_eq!(d[301], 384, "up at once for the spike");
        let back = d.iter().skip(301).position(|&x| x < 384).unwrap() + 301;
        assert!(
            back <= 301 + 650,
            "down after ~2 hold times, at tick {back}"
        );
        assert!(d[11_999] <= 156, "{}", d[11_999]);
    }

    #[test]
    fn a_single_high_spike_over_a_recurring_outlier_does_not_ratchet() {
        // Review probe B: 6 ms every 30 s (a recurring floor of 8 ms) and one
        // 100 ms spike at 300 s: up to it at once, back to the recurring floor
        // after its own short floor and a hold.
        let d = run(12_000, |k| match k {
            3000 => 144 + 4800,
            k if k > 0 && k % 300 == 0 => 144 + 288,
            _ => 144,
        });
        assert_eq!(d[2999], 384);
        assert_eq!(d[3000], 4800 + 144 + 48 - 96);
        let back = d.iter().skip(3000).position(|&x| x < 4896).unwrap() + 3000;
        assert!(
            back <= 3000 + 650,
            "down after ~2 hold times, at tick {back}"
        );
        assert_eq!(d[11_999], 384, "the recurring 6 ms stays covered");
    }

    #[test]
    fn lags_beyond_the_limit_move_no_delay() {
        // 10 s steady at 3 ms, then 40 lags of a writer seconds behind (more
        // than 4 in 10 s: the whole p99.9 for display) within the window: the
        // percentile the delay uses ignores them, the displayed one shows them.
        let mut w = LagWindow::new(96 + 7200 + 4800);
        let t0 = Instant::now();
        for k in 0..5000u32 {
            w.push(144, t0 + Duration::from_millis(u64::from(k) * 2));
        }
        for k in 0..40u32 {
            w.push(
                144_000 - k * 48,
                t0 + Duration::from_millis(9_000 + u64::from(k)),
            );
        }
        assert_eq!(w.quantile_upto(0.999, 96 + 7200), Some(148));
        assert!(w.quantile(0.999).unwrap() > 7296);
        // Through the hold: no change.
        let mut h = DelayHold::steady(0);
        let p999 = w.quantile_upto(0.999, 96 + 7200);
        assert_eq!(
            h.next(
                p999,
                Some(100_000),
                100,
                96,
                7200,
                Duration::from_secs(30),
                t0
            ),
            None
        );
        // A lag at the limit still counts.
        w.push(7296, t0 + Duration::from_millis(9_500));
        w.push(7296, t0 + Duration::from_millis(9_501));
        assert_eq!(w.quantile_upto(1.0, 96 + 7200), Some(7300));
    }

    #[test]
    fn an_outlier_read_a_block_apart_still_recurs() {
        // The same 6 ms outlier reads 6 or 7.5 ms at 2 ms block ends: it recurs
        // (within 1 ms and a block), so the delay does not fall between them.
        let t0 = Instant::now();
        let hold = Duration::from_secs(30);
        let mut h = DelayHold::steady(96);
        let (mut current, mut misses) = (96, 0);
        for k in 0..6000u64 {
            let now = t0 + Duration::from_millis(100 * k);
            let lag = match k % 400 {
                100 if (k / 400) % 2 == 0 => 144 + 288,
                100 => 144 + 360,
                _ => 144,
            };
            misses += u32::from(lag > 96 + current);
            if let Some(d) = h.next(Some(144), Some(lag), current, 96, 7200, hold, now) {
                current = d;
            }
        }
        assert!(misses <= 2, "{misses} late blocks");
        assert!(current >= 384, "{current}");
    }

    /// A fresh hold, 10 Hz for `ticks`, p99.9 14 ms, offset 2 ms; `seed`: the
    /// last run's delay. Returns the changes and the late blocks after tick 10.
    fn start(ticks: u64, seed: Option<u32>, lag: impl Fn(u64) -> u32) -> (Vec<(u64, u32)>, u32) {
        let t0 = Instant::now();
        let hold = Duration::from_secs(30);
        let mut h = DelayHold::new(96);
        let mut current = 0;
        if let Some(d) = seed {
            h.seed(d, t0);
            current = d;
        }
        let (mut changes, mut late) = (Vec::new(), 0);
        for k in 0..ticks {
            let now = t0 + Duration::from_millis(100 * k);
            late += u32::from(k > 10 && lag(k) > 96 + current);
            if let Some(d) = h.next(Some(672), Some(lag(k)), current, 96, 7200, hold, now) {
                current = d;
                changes.push((k, d));
            }
        }
        (changes, late)
    }

    #[test]
    fn the_start_up_phase_settles_fast() {
        // A fresh mirror input at 14 ms with a 75 ms outlier 2 s in (a backlog):
        // up at once, back to the steady need within about 10 s (two start-up
        // holds), not after 1 min. A second outlier (60 ms at 30 s) ends the
        // start-up hold: from then on the normal hold keeps it.
        let (changes, _) = start(900, None, |k| match k {
            20 => 3600,
            300 => 2880,
            _ => 672,
        });
        let steady = 672 + 48 - 96;
        assert_eq!(changes[0], (0, steady));
        assert_eq!(changes[1], (20, 3600 + 48 - 96));
        assert!(
            changes[2].0 <= 20 + 110 && changes[2].1 == steady,
            "{changes:?}"
        );
        assert_eq!(changes[3..], [(300, 2880 + 48 - 96)], "{changes:?}");
        // A single backlog alone: down within about 10 s and stays there.
        let (changes, _) = start(900, None, |k| if k == 20 { 3600 } else { 672 });
        assert_eq!(changes.last().unwrap().1, steady, "{changes:?}");
        // The same outlier again within the start-up phase recurs: from then on
        // it stays covered.
        let (changes, _) = start(900, None, |k| if k == 20 || k == 300 { 3600 } else { 672 });
        assert_eq!(changes.last().unwrap(), &(300, 3552), "{changes:?}");
    }

    #[test]
    fn the_last_runs_delay_covers_the_first_outliers() {
        // A gateway with +6 ms every 30 s, started at its last run's 6 ms delay:
        // nothing late at the start nor later (1.4.2 without the seed fell to the
        // steady need after 5 s and lost the next outlier).
        let gw = |k: u64| if k % 300 == 150 { 144 + 288 } else { 144 };
        let t0 = Instant::now();
        let hold = Duration::from_secs(30);
        let mut h = DelayHold::new(96);
        h.seed(384, t0);
        let (mut current, mut late) = (384, 0);
        for k in 0..6000u64 {
            let now = t0 + Duration::from_millis(100 * k);
            late += u32::from(gw(k) > 96 + current);
            if let Some(d) = h.next(Some(144), Some(gw(k)), current, 96, 7200, hold, now) {
                current = d;
            }
        }
        assert_eq!((late, current), (0, 384));
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

    #[test]
    fn a_writer_catching_up_raises_nothing() {
        // After a stall the first lags are seconds (beyond offset + maximum):
        // a writer catching up, not an outlier (D230).
        let t0 = Instant::now();
        let hold = Duration::from_secs(30);
        let mut h = DelayHold::steady(0);
        assert_eq!(h.next(Some(144), Some(144), 96, 96, 7200, hold, t0), None);
        for lag in [96_000, 48_000, 7297] {
            assert_eq!(h.next(Some(144), Some(lag), 96, 96, 7200, hold, t0), None);
        }
        assert!(h.floor() == 0 && h.last_late.is_none());
    }

    #[test]
    fn a_stall_keeps_the_floor_and_restarts_the_lower_period() {
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let hold = Duration::from_secs(30);
        let mut h = DelayHold::steady(0);
        assert_eq!(
            h.next(Some(144), Some(432), 96, 96, 7200, hold, at(0)),
            Some(384)
        );
        // Stalled at 40 s (after the one-off floor of 30 s, during the lower
        // period): the period starts again after it, the fall comes 30 s later.
        assert_eq!(
            h.next(Some(144), Some(144), 384, 96, 7200, hold, at(35)),
            None
        );
        h.interrupt();
        assert_eq!(
            h.next(Some(144), Some(144), 384, 96, 7200, hold, at(50)),
            None
        );
        assert_eq!(
            h.next(Some(144), Some(144), 384, 96, 7200, hold, at(79)),
            None
        );
        assert_eq!(
            h.next(Some(144), Some(144), 384, 96, 7200, hold, at(80)),
            Some(96)
        );
        // A stall during the floor does not shorten it.
        let mut h = DelayHold::steady(0);
        assert_eq!(
            h.next(Some(144), Some(432), 96, 96, 7200, hold, at(0)),
            Some(384)
        );
        h.interrupt();
        assert_eq!(
            h.next(Some(144), Some(144), 384, 96, 7200, hold, at(29)),
            None
        );
        assert!(h.floor() > 0);
    }

    #[test]
    fn a_new_route_forgets_the_floor() {
        // `route` and `extra_delay: auto` start the hold over (DelayHold::default).
        let t0 = Instant::now();
        let hold = Duration::from_secs(30);
        let mut h = DelayHold::steady(0);
        assert_eq!(
            h.next(Some(144), Some(432), 96, 96, 7200, hold, t0),
            Some(384)
        );
        h = DelayHold::steady(0);
        let at = |s: u64| t0 + Duration::from_secs(s);
        assert_eq!(
            h.next(Some(144), Some(144), 384, 96, 7200, hold, at(1)),
            None
        );
        assert_eq!(
            h.next(Some(144), Some(144), 384, 96, 7200, hold, at(31)),
            Some(96)
        );
    }

    #[test]
    fn confirm_mode_proposes_once_for_a_recurring_outlier() {
        // With MIXER_INPUT_DELAY_CONFIRM the delay stays until confirmed; the
        // hold decides against the proposal, so the same outlier again proposes
        // nothing new.
        let t0 = Instant::now();
        let hold = Duration::from_secs(30);
        let mut h = DelayHold::steady(0);
        let (applied, mut proposed) = (96u32, None);
        let mut proposals = Vec::new();
        for k in 0..3000u64 {
            let lag = if k % 300 == 150 { 432 } else { 144 };
            let current = proposed.unwrap_or(applied);
            let now = t0 + Duration::from_millis(100 * k);
            if let Some(t) = h.next(Some(144), Some(lag), current, 96, 7200, hold, now) {
                proposed = Some(t);
                proposals.push((k, t));
            }
        }
        assert_eq!(proposals, vec![(150, 384)]);
    }

    #[test]
    fn a_late_block_counts_before_the_first_second() {
        let t0 = Instant::now();
        let mut h = DelayHold::steady(0);
        let hold = Duration::from_secs(30);
        assert_eq!(h.next(None, Some(400), 0, 96, 7200, hold, t0), Some(352));
        assert_eq!(h.next(None, None, 352, 96, 7200, hold, t0), None);
        // Capped at the maximum.
        assert_eq!(
            h.next(None, Some(7296), 352, 96, 7200, hold, t0),
            Some(7200)
        );
    }

    #[test]
    fn a_stall_restarts_the_hold() {
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let hold = Duration::from_secs(30);
        let mut h = DelayHold::steady(0);
        assert_eq!(h.next(Some(480), None, 0, 96, 7200, hold, at(0)), Some(432));
        assert_eq!(h.next(Some(192), None, 432, 96, 7200, hold, at(1)), None);
        h.interrupt(); // stalled at 20 s
        assert_eq!(h.next(Some(192), None, 432, 96, 7200, hold, at(25)), None);
        assert_eq!(
            h.next(Some(192), None, 432, 96, 7200, hold, at(40)),
            None,
            "lower since 25 s only"
        );
        assert_eq!(
            h.next(Some(192), None, 432, 96, 7200, hold, at(55)),
            Some(144)
        );
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
}
