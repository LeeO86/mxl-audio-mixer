//! Meters (§7) on the control side: the DSP thread accumulates peak and energy
//! per signal channel; every [`METER_TICK`] this takes the peak since the last
//! tick and the RMS over [`RMS_WINDOW`]. The peak hold keeps the highest peak per
//! signal channel until it is cleared through the API; it survives plan swaps.

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;

use crate::config::SAMPLE_RATE;
use crate::engine::plan::{MeterBank, NodeMeter};
use crate::mix::model::Db;

/// Meter update period (§7: about 25 Hz).
pub const METER_TICK: Duration = Duration::from_millis(40);
/// Integration time of the RMS value.
pub const RMS_WINDOW: Duration = Duration::from_millis(300);

#[derive(Default, Clone)]
struct Channel {
    energy: f64,
    cross: f64,
    samples: u64,
    /// Per tick: energy, samples and (first channel) cross sum.
    window: VecDeque<(f64, u64, f64)>,
    peak: f32,
    true_peak: f32,
    ppm: [f32; 2],
    rms: f64,
    /// Window sums of energy and cross.
    sums: (f64, f64),
}

/// One processing node's meters: gain reduction (dB, ≥ 0) and the peaks at its
/// input and output since the last update.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct NodeValues {
    pub gain_reduction_db: f64,
    pub in_peak_dbfs: Db,
    pub out_peak_dbfs: Db,
}

/// One signal's meter values per channel, in dBFS (true peak in dBTP).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct MeterValues {
    pub peak_dbfs: Vec<Db>,
    pub peak_hold_dbfs: Vec<Db>,
    pub true_peak_dbtp: Vec<Db>,
    pub rms_dbfs: Vec<Db>,
    /// Quasi-peak readings (IEC 60268-10 Type I and Type II) in dBFS; a steady
    /// 1 kHz tone reads its peak.
    pub ppm_type1_dbfs: Vec<Db>,
    pub ppm_type2_dbfs: Vec<Db>,
    /// Phase correlation of the first two channels over the RMS window (−1 to
    /// +1); none for mono or silence.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation: Option<f64>,
    pub nodes: BTreeMap<String, NodeValues>,
}

pub struct Meters {
    bank: Arc<MeterBank>,
    signals: Vec<(String, u32, usize)>,
    channels: Vec<Channel>,
    holds: BTreeMap<String, Vec<f32>>,
    node_bank: Arc<[NodeMeter]>,
    nodes: Vec<(String, String, usize)>,
    node_values: Vec<(f32, f32, f32)>,
}

impl Default for Meters {
    fn default() -> Self {
        Self {
            bank: Arc::new(MeterBank::new(0)),
            signals: Vec::new(),
            channels: Vec::new(),
            holds: BTreeMap::new(),
            node_bank: Arc::new([]),
            nodes: Vec::new(),
            node_values: Vec::new(),
        }
    }
}

fn dbfs(level: f64) -> Db {
    if level > 0.0 {
        Db((20.0 * level.log10()).max(-200.0))
    } else {
        Db::MINUS_INF
    }
}

impl Meters {
    /// Follow a new plan's meter bank; `signals` is `(signal, first channel, channels)`.
    pub fn set_plan(&mut self, bank: Arc<MeterBank>, signals: Vec<(String, u32, usize)>) {
        self.channels = vec![Channel::default(); bank.channels.len()];
        self.bank = bank;
        self.holds.retain(|s, hold| {
            signals
                .iter()
                .any(|(signal, _, n)| signal == s && *n == hold.len())
        });
        for (signal, _, n) in &signals {
            self.holds
                .entry(signal.clone())
                .or_insert_with(|| vec![0.0; *n]);
        }
        self.signals = signals;
    }

    /// Follow a new plan's node meters; `nodes` is `(signal, node id, slot)`.
    pub fn set_nodes(&mut self, bank: Arc<[NodeMeter]>, nodes: Vec<(String, String, usize)>) {
        self.node_values = vec![(0.0, 0.0, 0.0); bank.len()];
        self.node_bank = bank;
        self.nodes = nodes;
    }

    /// Each signal's largest true peak (linear) since the last tick.
    pub fn true_peaks(&self) -> BTreeMap<String, f32> {
        self.signals
            .iter()
            .map(|(s, first, n)| {
                let range = *first as usize..*first as usize + n;
                let tp = self.channels[range]
                    .iter()
                    .fold(0f32, |m, c| m.max(c.true_peak));
                (s.clone(), tp)
            })
            .collect()
    }

    /// Clear the peak hold of `signal`, or of every signal; `false` when there is
    /// no such signal.
    pub fn clear_hold(&mut self, signal: Option<&str>) -> bool {
        match signal {
            Some(s) => match self.holds.get_mut(s) {
                Some(hold) => {
                    hold.fill(0.0);
                    true
                }
                None => false,
            },
            None => {
                self.holds.values_mut().for_each(|h| h.fill(0.0));
                true
            }
        }
    }

    pub fn tick(&mut self) {
        let window = u64::from(SAMPLE_RATE) * RMS_WINDOW.as_millis() as u64 / 1000;
        for (i, m) in self.bank.channels.iter().enumerate() {
            let c = &mut self.channels[i];
            c.peak = f32::from_bits(m.peak.swap(0, Ordering::Relaxed));
            c.true_peak = f32::from_bits(m.true_peak.swap(0, Ordering::Relaxed));
            for (k, b) in self.bank.ppm.iter().enumerate() {
                c.ppm[k] = f32::from_bits(m.ppm_max[k].swap(0, Ordering::Relaxed)) * b.gain;
            }
            let energy = f64::from_bits(m.energy.load(Ordering::Relaxed));
            let cross = f64::from_bits(m.cross.load(Ordering::Relaxed));
            let samples = m.samples.load(Ordering::Relaxed);
            c.window.push_back((
                energy - c.energy,
                samples.saturating_sub(c.samples),
                cross - c.cross,
            ));
            c.energy = energy;
            c.cross = cross;
            c.samples = samples;
            let mut total = 0;
            let mut keep = 0;
            for (k, (_, n, _)) in c.window.iter().enumerate().rev() {
                total += n;
                keep = c.window.len() - k;
                if total >= window {
                    break;
                }
            }
            while c.window.len() > keep {
                c.window.pop_front();
            }
            let (e, n, x) = c
                .window
                .iter()
                .fold((0.0, 0u64, 0.0), |(e, n, x), (de, dn, dx)| {
                    (e + de, n + dn, x + dx)
                });
            c.sums = (e.max(0.0), x);
            c.rms = if n > 0 {
                (e.max(0.0) / n as f64).sqrt()
            } else {
                0.0
            };
        }
        for (v, m) in self.node_values.iter_mut().zip(self.node_bank.iter()) {
            *v = (
                f32::from_bits(m.reduction.swap(0, Ordering::Relaxed)),
                f32::from_bits(m.in_peak.swap(0, Ordering::Relaxed)),
                f32::from_bits(m.out_peak.swap(0, Ordering::Relaxed)),
            );
        }
        for (signal, first, n) in &self.signals {
            if let Some(hold) = self.holds.get_mut(signal) {
                let peaks = &self.channels[*first as usize..*first as usize + n];
                for (h, c) in hold.iter_mut().zip(peaks) {
                    *h = h.max(c.peak);
                }
            }
        }
    }

    pub fn values(&self) -> BTreeMap<String, MeterValues> {
        self.signals
            .iter()
            .map(|(signal, first, n)| {
                let range = *first as usize..*first as usize + n;
                let chans = &self.channels[range];
                let hold = self.holds.get(signal);
                (
                    signal.clone(),
                    MeterValues {
                        peak_dbfs: chans.iter().map(|c| dbfs(f64::from(c.peak))).collect(),
                        peak_hold_dbfs: (0..*n)
                            .map(|i| {
                                dbfs(f64::from(
                                    hold.and_then(|h| h.get(i)).copied().unwrap_or(0.0),
                                ))
                            })
                            .collect(),
                        true_peak_dbtp: chans
                            .iter()
                            .map(|c| dbfs(f64::from(c.true_peak)))
                            .collect(),
                        rms_dbfs: chans.iter().map(|c| dbfs(c.rms)).collect(),
                        ppm_type1_dbfs: chans.iter().map(|c| dbfs(f64::from(c.ppm[0]))).collect(),
                        ppm_type2_dbfs: chans.iter().map(|c| dbfs(f64::from(c.ppm[1]))).collect(),
                        correlation: match chans {
                            [a, b, ..] => {
                                let d = (a.sums.0 * b.sums.0).sqrt();
                                (d > 1e-12).then(|| (a.sums.1 / d).clamp(-1.0, 1.0))
                            }
                            _ => None,
                        },
                        nodes: self
                            .nodes
                            .iter()
                            .filter(|(s, _, _)| s == signal)
                            .filter_map(|(_, node, slot)| {
                                let (gr, i, o) = *self.node_values.get(*slot)?;
                                Some((
                                    node.clone(),
                                    NodeValues {
                                        gain_reduction_db: f64::from(gr),
                                        in_peak_dbfs: dbfs(f64::from(i)),
                                        out_peak_dbfs: dbfs(f64::from(o)),
                                    },
                                ))
                            })
                            .collect(),
                    },
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peak_per_tick_and_rms_over_the_window() {
        let bank = Arc::new(MeterBank::new(1));
        let mut meters = Meters::default();
        meters.set_plan(bank.clone(), vec![("main".into(), 0, 1)]);
        let m = &bank.channels[0];
        // 40 ms of a 0.5 square wave (RMS 0.5), peak 0.5.
        let feed = |level: f32, samples: u64| {
            m.peak.fetch_max(level.to_bits(), Ordering::Relaxed);
            let e = f64::from_bits(m.energy.load(Ordering::Relaxed))
                + f64::from(level * level) * samples as f64;
            m.energy.store(e.to_bits(), Ordering::Relaxed);
            m.samples.fetch_add(samples, Ordering::Relaxed);
        };
        for _ in 0..10 {
            feed(0.5, 1920);
            meters.tick();
        }
        let v = &meters.values()["main"];
        assert!((v.peak_dbfs[0].0 + 6.0206).abs() < 1e-3);
        assert!((v.rms_dbfs[0].0 + 6.0206).abs() < 1e-3);
        // Silence: the peak drops at once, the RMS after the window.
        feed(0.0, 1920);
        meters.tick();
        let v = &meters.values()["main"];
        assert_eq!(v.peak_dbfs[0], Db::MINUS_INF);
        assert!(v.rms_dbfs[0].0 > -7.0);
        for _ in 0..8 {
            feed(0.0, 1920);
            meters.tick();
        }
        assert_eq!(meters.values()["main"].rms_dbfs[0], Db::MINUS_INF);
        // The hold keeps the −6 dB peak through silence and a plan swap, until cleared.
        assert!((meters.values()["main"].peak_hold_dbfs[0].0 + 6.0206).abs() < 1e-3);
        let next = Arc::new(MeterBank::new(3));
        meters.set_plan(next, vec![("other".into(), 0, 2), ("main".into(), 2, 1)]);
        meters.tick();
        assert!((meters.values()["main"].peak_hold_dbfs[0].0 + 6.0206).abs() < 1e-3);
        assert!(!meters.clear_hold(Some("gone")));
        assert!(meters.clear_hold(Some("main")));
        assert_eq!(meters.values()["main"].peak_hold_dbfs[0], Db::MINUS_INF);
    }

    #[test]
    fn correlation_of_two_channels() {
        let bank = Arc::new(MeterBank::new(2));
        let mut meters = Meters::default();
        meters.set_plan(bank.clone(), vec![("st".into(), 0, 2)]);
        let add = |a: &std::sync::atomic::AtomicU64, v: f64| {
            a.store(
                (f64::from_bits(a.load(Ordering::Relaxed)) + v).to_bits(),
                Ordering::Relaxed,
            );
        };
        // L = R (in phase), then L = −R (inverted): 1920 samples of ±0.5 each tick.
        for sign in [1.0, -1.0] {
            for _ in 0..10 {
                for c in &bank.channels {
                    add(&c.energy, 0.25 * 1920.0);
                    c.samples.fetch_add(1920, Ordering::Relaxed);
                }
                add(&bank.channels[0].cross, sign * 0.25 * 1920.0);
                meters.tick();
            }
            let r = meters.values()["st"].correlation.unwrap();
            assert!((r - sign).abs() < 1e-9, "{r}");
        }
        for _ in 0..10 {
            for c in &bank.channels {
                c.samples.fetch_add(1920, Ordering::Relaxed);
            }
            meters.tick();
        }
        assert_eq!(meters.values()["st"].correlation, None);
    }
}
