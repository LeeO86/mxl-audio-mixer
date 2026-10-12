//! The control engine (§8): voice activity detection, the auto-mixer and ducking.
//!
//! Every analysed signal gets a VAD tap slot: the DSP thread copies the mean of
//! its channels before the fader into that slot's ring. Every 10 ms the control
//! side ([`Auto::tick`]) takes the rings, runs each signal's VAD frame by frame,
//! then the auto-mixes and ducks on those frames, and returns the auto nodes'
//! gains that changed (auto-mix gain × the deepest duck).

pub mod automix;
pub mod duck;
pub mod vad;

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::Serialize;

use crate::engine::MAX_VAD;
use crate::mix::Mix;

/// The tick of the control engine.
pub const TICK: std::time::Duration = std::time::Duration::from_millis(10);

#[derive(Default)]
pub struct Auto {
    /// The signal of each VAD tap slot.
    slots: Vec<Option<String>>,
    /// Slots given to another signal since the running plan was compiled: their
    /// rings may still hold the previous signal.
    fresh: BTreeSet<usize>,
    pub vads: BTreeMap<String, vad::Vad>,
    pub automixes: BTreeMap<String, automix::State>,
    pub ducks: BTreeMap<String, duck::State>,
    /// The gain of each auto node, as last sent.
    pub gains: BTreeMap<String, f32>,
    scratch: Vec<f32>,
}

/// An auto-mix's state for the API.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct AutomixStatus {
    /// Each member's gain, dB (−∞ as `null`).
    pub gains_db: BTreeMap<String, Option<f64>>,
}

/// A duck's state for the API.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DuckStatus {
    pub active: bool,
    pub gain_db: Option<f64>,
}

fn to_db(g: f64) -> Option<f64> {
    // + 0.0: no "-0.0".
    (g > 0.0).then(|| (20.0 * g.log10() * 100.0).round() / 100.0 + 0.0)
}

impl Auto {
    pub fn new() -> Self {
        Self {
            slots: vec![None; MAX_VAD],
            ..Self::default()
        }
    }

    /// Give every signal `mix` analyses a slot (keeping the ones it has) and
    /// return them, for the compiler.
    pub fn assign(&mut self, mix: &Mix) -> HashMap<String, usize> {
        if self.slots.len() != MAX_VAD {
            self.slots = vec![None; MAX_VAD];
        }
        let wanted = mix.vad_signals();
        for slot in self.slots.iter_mut() {
            if slot.as_ref().is_some_and(|s| !wanted.contains_key(s)) {
                *slot = None;
            }
        }
        for signal in wanted.keys() {
            if self.slots.iter().any(|s| s.as_ref() == Some(signal)) {
                continue;
            }
            if let Some(k) = self.slots.iter().position(Option::is_none) {
                self.slots[k] = Some(signal.clone());
                self.fresh.insert(k);
                if let Some(v) = self.vads.get_mut(signal) {
                    v.reset();
                }
            }
        }
        self.vads.retain(|s, _| wanted.contains_key(s));
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(k, s)| s.clone().map(|s| (s, k)))
            .collect()
    }

    /// The plan with the current slots runs: drop what the fresh slots' rings
    /// hold from before.
    pub fn swapped(&mut self, rings: &mut [rtrb::Consumer<f32>]) {
        for k in std::mem::take(&mut self.fresh) {
            if let Some(ring) = rings.get_mut(k) {
                let n = ring.slots();
                if let Ok(chunk) = ring.read_chunk(n) {
                    chunk.commit_all();
                }
            }
        }
    }

    /// The gain each auto node of `mix` should be compiled with.
    pub fn compile_gains(&self, mix: &Mix) -> HashMap<String, f32> {
        mix.auto_signals()
            .into_iter()
            .map(|s| {
                let g = self.gains.get(&s).copied().unwrap_or(1.0);
                (s, g)
            })
            .collect()
    }

    /// Take the rings, run the VADs, auto-mixes and ducks; returns the auto
    /// nodes whose gain changed, with the new gain.
    pub fn tick(&mut self, rings: &mut [rtrb::Consumer<f32>], mix: &Mix) -> BTreeMap<String, f32> {
        let settings = mix.vad_signals();
        let mut frames: BTreeMap<String, Vec<automix::Input>> = BTreeMap::new();
        for (k, slot) in self.slots.iter().enumerate() {
            let (Some(signal), Some(ring)) = (slot, rings.get_mut(k)) else {
                continue;
            };
            let n = ring.slots();
            if n == 0 {
                continue;
            }
            let Ok(chunk) = ring.read_chunk(n) else {
                continue;
            };
            let (a, b) = chunk.as_slices();
            self.scratch.clear();
            self.scratch.extend_from_slice(a);
            self.scratch.extend_from_slice(b);
            chunk.commit_all();
            if self.fresh.contains(&k) {
                continue;
            }
            let Some(s) = settings.get(signal) else {
                continue;
            };
            let v = self.vads.entry(signal.clone()).or_default();
            let list = frames.entry(signal.clone()).or_default();
            v.push(&self.scratch, s, |st| {
                list.push(automix::Input {
                    power: 10f64.powf(st.level_dbfs / 10.0),
                    probability: st.speech_probability,
                    active: st.speech_active,
                })
            });
        }
        let count = frames.values().map(Vec::len).max().unwrap_or(0);
        let at = |signal: &str, k: usize| -> Option<automix::Input> {
            let f = frames.get(signal)?;
            f.get(k).or(f.last()).copied()
        };

        let effects = mix.vca_effects();
        let grouped = mix.group_muted();
        let muted: BTreeSet<String> = mix
            .strips
            .iter()
            .filter(|s| mix.fader_gain(&s.id, s.fader_db, s.mute, &effects, &grouped) == 0.0)
            .map(|s| s.id.clone())
            .collect();
        self.automixes.retain(|id, _| mix.automix(id).is_some());
        for a in &mix.automixes {
            let st = self.automixes.entry(a.id.clone()).or_default();
            let n = a
                .members
                .iter()
                .map(|m| frames.get(&m.signal).map_or(0, Vec::len))
                .max()
                .unwrap_or(0);
            if !a.enabled {
                st.frame(a, &BTreeMap::new(), &muted);
            }
            for k in 0..n {
                let inputs = a
                    .members
                    .iter()
                    .filter_map(|m| Some((m.signal.clone(), at(&m.signal, k)?)))
                    .collect();
                st.frame(a, &inputs, &muted);
            }
        }
        self.ducks.retain(|id, _| mix.duck(id).is_some());
        for d in &mix.ducks {
            let st = self.ducks.entry(d.id.clone()).or_default();
            if !d.enabled {
                st.frame(d, false);
                continue;
            }
            for k in 0..count {
                let speech = d
                    .keys
                    .iter()
                    .any(|key| at(key, k).is_some_and(|f| f.active));
                st.frame(d, speech);
            }
        }

        let mut changed = BTreeMap::new();
        let targets = mix.auto_signals();
        self.gains.retain(|s, _| targets.contains(s));
        for s in targets {
            let am = mix
                .automix_of(&s)
                .and_then(|a| self.automixes.get(&a.id))
                .and_then(|st| st.gains.get(&s))
                .copied()
                .unwrap_or(1.0);
            let duck = mix
                .ducks
                .iter()
                .filter(|d| d.targets.contains(&s))
                .filter_map(|d| self.ducks.get(&d.id))
                .map(duck::State::gain)
                .fold(1.0, f64::min);
            let g = (am * duck) as f32;
            if (g - self.gains.get(&s).copied().unwrap_or(1.0)).abs() > 1e-6 {
                changed.insert(s.clone(), g);
            }
            self.gains.insert(s, g);
        }
        changed
    }

    pub fn vad_state(&self, signal: &str) -> Option<&vad::VadState> {
        self.vads.get(signal).map(|v| &v.state)
    }

    pub fn automix_status(&self, id: &str) -> Option<AutomixStatus> {
        self.automixes.get(id).map(|st| AutomixStatus {
            gains_db: st
                .gains
                .iter()
                .map(|(s, g)| (s.clone(), to_db(*g)))
                .collect(),
        })
    }

    pub fn duck_status(&self, id: &str) -> DuckStatus {
        let st = self.ducks.get(id).copied().unwrap_or_default();
        DuckStatus {
            active: st.active,
            gain_db: to_db(st.gain()),
        }
    }

    /// Each auto node's gain in dB (−∞ as `null`), for the meters message.
    pub fn gains_db(&self) -> BTreeMap<String, Option<f64>> {
        self.gains
            .iter()
            .map(|(s, g)| (s.clone(), to_db(f64::from(*g))))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mix::auto::{Automix, AutomixMember, Duck, VadSettings};
    use crate::mix::model::tests::strip;
    use crate::mix::model::Db;
    use crate::mix::Format;

    fn mix() -> Mix {
        let mut m = Mix::default();
        for id in ["a", "b", "music"] {
            m.strips.push(strip(id, Format::Mono));
        }
        m
    }

    #[test]
    fn slots_are_kept_freed_and_fresh() {
        let mut auto = Auto::new();
        let mut m = mix();
        m.vad.push(VadSettings::new("b"));
        let s = auto.assign(&m);
        assert_eq!(s, HashMap::from([("b".to_string(), 0)]));
        m.vad.insert(0, VadSettings::new("a"));
        let s = auto.assign(&m);
        assert_eq!((s["a"], s["b"]), (1, 0), "b keeps its slot");
        m.vad.retain(|v| v.signal != "b");
        m.vad.push(VadSettings::new("music"));
        let s = auto.assign(&m);
        assert_eq!((s["a"], s["music"]), (1, 0), "b's slot reused");
        assert!(auto.fresh.contains(&0));
        let (mut tx, rx) = rtrb::RingBuffer::new(1000);
        let mut rings = vec![rx];
        for _ in 0..10 {
            tx.push(0.5).unwrap();
        }
        auto.swapped(&mut rings);
        assert_eq!(rings[0].slots(), 0, "the old signal's samples dropped");
        assert!(auto.fresh.is_empty());
    }

    #[test]
    fn speech_ducks_the_music_and_drives_the_automix() {
        let mut auto = Auto::new();
        let mut m = mix();
        m.automixes.push(Automix {
            id: "am".into(),
            name: "am".into(),
            enabled: true,
            last_mic_hold: true,
            vad_weighting: false,
            members: vec![AutomixMember::new("a"), AutomixMember::new("b")],
        });
        let mut d = Duck::new("bed");
        d.keys = vec!["a".into()];
        d.targets = vec!["music".into()];
        d.depth_db = Db(-20.0);
        d.attack_ms = 0.0;
        m.ducks.push(d);
        let slots = auto.assign(&m);
        let mut txs = Vec::new();
        let mut rings = Vec::new();
        for _ in 0..MAX_VAD {
            let (tx, rx) = rtrb::RingBuffer::new(crate::engine::VAD_RING);
            txs.push(tx);
            rings.push(rx);
        }
        auto.swapped(&mut rings);
        assert_eq!(auto.compile_gains(&m)["music"], 1.0);
        // a speaks, b has quiet noise; 2 s, 10 ms per tick.
        let (speech, _) = vad::tests::speech(2.0, 0.06, 1);
        let noise = vad::tests::pink(2.0, 0.0005, 2);
        let mut last = BTreeMap::new();
        for k in 0..200 {
            let r = k * vad::FRAME..(k + 1) * vad::FRAME;
            for (sig, x) in [("a", &speech[r.clone()]), ("b", &noise[r])] {
                for v in x {
                    txs[slots[sig]].push(*v).unwrap();
                }
            }
            for (s, g) in auto.tick(&mut rings, &m) {
                last.insert(s, g);
            }
        }
        assert!(auto.vad_state("a").unwrap().speech_onsets >= 1);
        assert_eq!(auto.duck_status("bed").gain_db, Some(-20.0));
        assert!((last["music"] - 0.1).abs() < 1e-6);
        assert!(last["a"] > last["b"] * 10.0, "{last:?}");
        // The duck removed: the music comes back to 0 dB.
        m.ducks.clear();
        let changed = auto.tick(&mut rings, &m);
        assert!(!changed.contains_key("music") && !auto.gains.contains_key("music"));
    }
}
