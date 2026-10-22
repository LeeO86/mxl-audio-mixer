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
/// comes close to the delay; down only after the need stayed lower by
/// [`DELAY_LOWER_SAMPLES`] for the whole hold time, to the highest need seen in
/// that time, and at most once per hold time.
#[derive(Default)]
struct DelayHold {
    last_change: Option<Instant>,
    /// Since when the need has been lower, and the highest need since then.
    lower: Option<(Instant, u32)>,
}

impl DelayHold {
    /// The new delay for an input with delay `current` and arrival lag `p999`
    /// (99.9th percentile), or `None` to keep it.
    fn next(
        &mut self,
        p999: u32,
        current: u32,
        read_offset: u32,
        delay_max: u32,
        hold: Duration,
        now: Instant,
    ) -> Option<u32> {
        let need = (p999 + DELAY_MARGIN_SAMPLES)
            .saturating_sub(read_offset)
            .min(delay_max);
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
            (Some((a, _, _)), Some((b, _, _))) => b.duration_since(*a) + Duration::from_secs(1),
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
    instances: HashMap<PathBuf, MxlInstance>,
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
            instances: HashMap::new(),
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
        let instance = match self.instances.get(&route.domain) {
            Some(instance) => instance.clone(),
            None => {
                match MxlInstance::new(self.api.clone(), &route.domain.display().to_string(), "") {
                    Ok(instance) => {
                        self.instances
                            .insert(route.domain.clone(), instance.clone());
                        instance
                    }
                    Err(err) => {
                        tracing::warn!(
                            input = index + 1,
                            "MXL instance for {}: {err}",
                            route.domain.display()
                        );
                        return false;
                    }
                }
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
            while let Ok(lag) = engine.lags[i].pop() {
                self.slots[i].last_lag = lag;
                self.slots[i].lag.push(lag, now);
            }
            match self.slots[i].state {
                State::Unrouted => {}
                State::Waiting => {
                    self.try_open(engine, i);
                }
                State::Running | State::Stalled => self.watch(engine, i, now),
            }
            self.auto_delay(engine, i, now);
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

    fn auto_delay(&mut self, engine: &mut Engine, i: usize, now: Instant) {
        let (read_offset, delay_max, hold, confirm) =
            (self.read_offset, self.delay_max, self.hold, self.confirm);
        let slot = &mut self.slots[i];
        if slot.config.extra_delay != ExtraDelay::default() || slot.state != State::Running {
            return;
        }
        if slot.lag.span() < Duration::from_secs(MIN_LAG_SECONDS) {
            return;
        }
        let Some(p999) = slot.lag.quantile(0.999) else {
            return;
        };
        let current = slot.proposed.unwrap_or(slot.delay);
        let Some(target) = slot
            .hold
            .next(p999, current, read_offset, delay_max, hold, now)
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

    /// Release the MXL instances once the DSP thread has stopped (G8).
    pub fn release(&mut self) {
        self.instances.clear();
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
            if let Some(d) = hold.next(p999(k), current, 96, 7200, Duration::from_secs(hold_s), now)
            {
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
