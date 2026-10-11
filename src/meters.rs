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
use crate::engine::plan::MeterBank;
use crate::mix::model::Db;

/// Meter update period (§7: about 25 Hz).
pub const METER_TICK: Duration = Duration::from_millis(40);
/// Integration time of the RMS value.
pub const RMS_WINDOW: Duration = Duration::from_millis(300);

#[derive(Default, Clone)]
struct Channel {
    energy: f64,
    samples: u64,
    window: VecDeque<(f64, u64)>,
    peak: f32,
    rms: f64,
}

/// One signal's meter values per channel, in dBFS.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct MeterValues {
    pub peak_dbfs: Vec<Db>,
    pub peak_hold_dbfs: Vec<Db>,
    pub rms_dbfs: Vec<Db>,
}

pub struct Meters {
    bank: Arc<MeterBank>,
    signals: Vec<(String, u32, usize)>,
    channels: Vec<Channel>,
    holds: BTreeMap<String, Vec<f32>>,
}

impl Default for Meters {
    fn default() -> Self {
        Self {
            bank: Arc::new(MeterBank::new(0)),
            signals: Vec::new(),
            channels: Vec::new(),
            holds: BTreeMap::new(),
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
            let energy = f64::from_bits(m.energy.load(Ordering::Relaxed));
            let samples = m.samples.load(Ordering::Relaxed);
            c.window
                .push_back((energy - c.energy, samples.saturating_sub(c.samples)));
            c.energy = energy;
            c.samples = samples;
            let mut total = 0;
            let mut keep = 0;
            for (k, (_, n)) in c.window.iter().enumerate().rev() {
                total += n;
                keep = c.window.len() - k;
                if total >= window {
                    break;
                }
            }
            while c.window.len() > keep {
                c.window.pop_front();
            }
            let (e, n) = c
                .window
                .iter()
                .fold((0.0, 0u64), |(e, n), (de, dn)| (e + de, n + dn));
            c.rms = if n > 0 {
                (e.max(0.0) / n as f64).sqrt()
            } else {
                0.0
            };
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
                        rms_dbfs: chans.iter().map(|c| dbfs(c.rms)).collect(),
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
}
