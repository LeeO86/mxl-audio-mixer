//! Loudness measurement on the control side (§7): one DSP segment meter per
//! signal with `loudness: true`, in its own slot (independent of plans, so the
//! integration continues over plan swaps); momentary, short-term, integrated,
//! loudness range and maximum true peak; start, stop and reset.

use std::collections::{BTreeMap, HashMap};

use serde::Serialize;

use crate::dsp::loudness::{weights, Loudness, Segment, SegmentMeter};
use crate::engine::{Command, Engine, MAX_LOUDNESS};
use crate::mix::model::{Db, MixSettings};
use crate::mix::{Format, Mix};

struct Entry {
    slot: usize,
    format: Format,
    segments: rtrb::Consumer<Segment>,
    measurement: Loudness,
}

/// A signal's loudness for the API and the metrics.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct LoudnessValues {
    pub momentary_lufs: Db,
    pub short_term_lufs: Db,
    pub integrated_lufs: Db,
    pub range_lu: f64,
    pub max_true_peak_dbtp: Db,
    pub running: bool,
    /// The integrated loudness is within the tolerance of the target.
    pub in_tolerance: Option<bool>,
}

fn db(v: f64) -> Db {
    if v.is_finite() {
        Db(v)
    } else {
        Db::MINUS_INF
    }
}

#[derive(Default)]
pub struct LoudnessMeters {
    entries: HashMap<String, Entry>,
}

impl LoudnessMeters {
    /// Signal → loudness meter slot.
    pub fn slots(&self) -> HashMap<String, usize> {
        self.entries
            .iter()
            .map(|(s, e)| (s.clone(), e.slot))
            .collect()
    }

    /// The (signal, format) pairs `mix` measures.
    pub fn wanted(mix: &Mix) -> Vec<(String, Format)> {
        let mut v: Vec<(String, Format)> = mix
            .strips
            .iter()
            .filter(|s| s.loudness)
            .map(|s| (s.id.clone(), s.format))
            .chain(
                mix.buses
                    .iter()
                    .filter(|b| b.loudness)
                    .map(|b| (b.id.clone(), b.format)),
            )
            .collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    }

    pub fn measured(&self) -> Vec<(String, Format)> {
        let mut v: Vec<(String, Format)> = self
            .entries
            .iter()
            .map(|(s, e)| (s.clone(), e.format))
            .collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    }

    /// Start meters for signals that are new or changed format, stop the others.
    pub fn sync(&mut self, engine: &mut Engine, mix: &Mix) {
        let wanted = Self::wanted(mix);
        let gone: Vec<String> = self
            .entries
            .iter()
            .filter(|(s, e)| !wanted.iter().any(|(w, f)| w == *s && *f == e.format))
            .map(|(s, _)| s.clone())
            .collect();
        for s in gone {
            if let Some(e) = self.entries.remove(&s) {
                engine.send(Command::ClearLoudness { slot: e.slot });
            }
        }
        for (signal, format) in wanted {
            if self.entries.contains_key(&signal) {
                continue;
            }
            let used: Vec<usize> = self.entries.values().map(|e| e.slot).collect();
            let Some(slot) = (0..MAX_LOUDNESS).find(|s| !used.contains(s)) else {
                tracing::error!("no free loudness meter for {signal}");
                continue;
            };
            let (meter, segments) = SegmentMeter::new(format.channels());
            engine.send(Command::SetLoudness {
                slot,
                meter: Box::new(meter),
            });
            self.entries.insert(
                signal,
                Entry {
                    slot,
                    format,
                    segments,
                    measurement: Loudness::new(weights(format.labels())),
                },
            );
        }
    }

    /// Take the segments the DSP thread sent; `true_peaks` is each signal's true
    /// peak since the last tick (linear).
    pub fn tick(&mut self, true_peaks: &BTreeMap<String, f32>) {
        for (signal, e) in self.entries.iter_mut() {
            while let Ok(s) = e.segments.pop() {
                e.measurement.push(&s);
            }
            if e.measurement.running {
                if let Some(tp) = true_peaks.get(signal) {
                    e.measurement.max_true_peak = e.measurement.max_true_peak.max(*tp);
                }
            }
        }
    }

    /// `start`, `stop` or `reset` the integration of `signal`.
    pub fn control(&mut self, signal: &str, action: &str) -> Result<(), String> {
        let e = self
            .entries
            .get_mut(signal)
            .ok_or_else(|| format!("{signal} has no loudness measurement"))?;
        match action {
            "start" => e.measurement.running = true,
            "stop" => e.measurement.running = false,
            "reset" => e.measurement.reset(),
            _ => return Err(format!("unknown action {action}: start, stop or reset")),
        }
        Ok(())
    }

    pub fn values(&self, settings: &MixSettings) -> BTreeMap<String, LoudnessValues> {
        self.entries
            .iter()
            .map(|(s, e)| {
                let v = e.measurement.values();
                let tp = e.measurement.max_true_peak;
                (
                    s.clone(),
                    LoudnessValues {
                        momentary_lufs: db(v.momentary),
                        short_term_lufs: db(v.short_term),
                        integrated_lufs: db(v.integrated),
                        range_lu: v.range,
                        max_true_peak_dbtp: db(if tp > 0.0 {
                            20.0 * f64::from(tp).log10()
                        } else {
                            f64::NEG_INFINITY
                        }),
                        running: e.measurement.running,
                        in_tolerance: v.integrated.is_finite().then(|| {
                            (v.integrated - settings.loudness_target_lufs).abs()
                                <= settings.loudness_tolerance_lu
                        }),
                    },
                )
            })
            .collect()
    }
}
