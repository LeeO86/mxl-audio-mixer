//! The inputs' control side (§4.2): routing from IS-05, waiting for a flow,
//! re-opening re-created flows, stall detection, arrival lag and the automatic
//! extra delay. Runs on a 100 ms tick, never in the DSP thread.

use std::collections::VecDeque;
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
/// outlier of about that size (within 1 ms) comes again within
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
}

impl DelayHold {
    /// The higher of the two late-block floors (0 without one).
    fn floor(&self) -> u32 {
        let n = |f: Option<(Instant, u32)>| f.map_or(0, |(_, n)| n);
        n(self.short).max(n(self.long))
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
        let memory = hold * LATE_MEMORY_HOLDS;
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
            // An outlier needs at least 1 ms more than the 99.9th percentile (not
            // known in the first second: a late block then is no outlier).
            let outlier = p999.is_some_and(|p| need > need_of(p) + DELAY_LOWER_SAMPLES);
            // It recurs when it needs about as much (within 1 ms) as the
            // recurring floor or the last outlier: another size, a smaller tail
            // or a single higher spike, renews nothing and does not ratchet.
            let near = |f: Option<(Instant, u32)>| {
                f.map(|(_, n)| n)
                    .filter(|&n| need.abs_diff(n) <= DELAY_LOWER_SAMPLES)
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

    fn count(&self) -> u64 {
        self.seconds.iter().map(|(_, _, c)| *c).sum()
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
    fn quantile(&self, q: f64) -> Option<u32> {
        let total = self.count();
        if total == 0 {
            return None;
        }
        let target = ((total as f64) * q).ceil().max(1.0) as u64;
        let mut seen = 0;
        for b in 0..self.buckets {
            seen += self
                .seconds
                .iter()
                .map(|(_, h, _)| u64::from(h[b]))
                .sum::<u64>();
            if seen >= target {
                return Some((b as u32 + 1) * LAG_BUCKET_SAMPLES);
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
        // An instance of its own for every open: libmxl caches one reader per
        // flow id per instance, so a shared instance could hand a re-open (a
        // flow deleted and re-created) the old reader, mapped on the deleted
        // files, while the DSP thread still holds it. The reader keeps its
        // instance; both go when the control side drops the returned reader.
        let instance =
            match MxlInstance::new(self.api.clone(), &route.domain.display().to_string(), "") {
                Ok(instance) => instance,
                Err(err) => {
                    tracing::warn!(
                        input = index + 1,
                        "MXL instance for {}: {err}",
                        route.domain.display()
                    );
                    return false;
                }
            };
        match mxlio::open_reader(&instance, route.flow) {
            Ok((reader, channels)) => {
                let inode = mxlio::flow_inode(&route.domain, route.flow);
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
                if was != State::Waiting {
                    slot.reopens += 1;
                }
                tracing::info!(input = index + 1, flow = %route.flow, channels, "input running");
                true
            }
            Err(_) => false,
        }
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
            .then(|| slot.lag.quantile(0.999))
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
        let mut hold = DelayHold::default();
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
            (DelayHold::default(), 0, 0, Vec::new());
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
        let (mut h, mut current, mut misses, mut changes) = (DelayHold::default(), 0, 0, 0);
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
        let (mut h, mut current, mut out) = (DelayHold::default(), 96, Vec::new());
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
    fn a_writer_catching_up_raises_nothing() {
        // After a stall the first lags are seconds (beyond offset + maximum):
        // a writer catching up, not an outlier (D230).
        let t0 = Instant::now();
        let hold = Duration::from_secs(30);
        let mut h = DelayHold::default();
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
        let mut h = DelayHold::default();
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
        let mut h = DelayHold::default();
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
        let mut h = DelayHold::default();
        assert_eq!(
            h.next(Some(144), Some(432), 96, 96, 7200, hold, t0),
            Some(384)
        );
        h = DelayHold::default();
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
        let mut h = DelayHold::default();
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
        let mut h = DelayHold::default();
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
        let mut h = DelayHold::default();
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
