//! The control engine (§8): voice activity detection, auto-level, the
//! auto-mixer and ducking.
//!
//! Every analysed signal gets a VAD tap slot: the DSP thread copies the
//! signal's channels before its auto gain, block by block, into the VAD ring
//! (records as the meters': plan generation, slot, channels). Every 10 ms the
//! control side ([`Auto::tick`]) takes the ring and cuts each signal into 10 ms
//! frames: its loudness (BS.1770 over its channels, `level`), its VAD (on the
//! mean of its channels), then frame by frame the auto-levels, auto-mixes and
//! ducks; it returns the auto nodes' gains that changed, per channel:
//! auto-level × auto-mix × the deepest duck of each channel.

pub mod autolevel;
pub mod automix;
pub mod comis;
pub mod duck;
pub mod level;
pub mod ratio;
pub mod silero;
pub mod vad;

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::Serialize;

use crate::engine::proc::MAX_CH;
use crate::engine::MAX_VAD;
use crate::mix::auto::DuckMode;
use crate::mix::Mix;
use comis::SILENCE_LUFS;
use level::{Level, Reading};

/// The tick of the control engine.
pub const TICK: std::time::Duration = std::time::Duration::from_millis(10);

/// One 10 ms frame of an analysed signal.
#[derive(Debug, Clone, Copy, Default)]
pub struct Frame {
    pub vad: automix::Input,
    pub reading: Reading,
}

/// A tapped signal: what it has of its next frame, per channel, and its loudness.
#[derive(Default)]
struct Tapped {
    level: Level,
    partial: Vec<Vec<f32>>,
    mean: Vec<f32>,
}

#[derive(Default)]
pub struct Auto {
    /// The signal of each VAD tap slot.
    slots: Vec<Option<String>>,
    /// Per slot: the first plan whose records are its signal's (`u32::MAX`
    /// until that plan is sent).
    valid_from: Vec<u32>,
    /// Slots given to another signal since the last plan was sent.
    fresh: BTreeSet<usize>,
    tapped: BTreeMap<String, Tapped>,
    pub vads: BTreeMap<String, vad::Vad>,
    pub automixes: BTreeMap<String, automix::State>,
    pub ducks: BTreeMap<String, duck::State>,
    ratios: BTreeMap<String, ratio::State>,
    autolevels: BTreeMap<String, autolevel::State>,
    /// The gain of each auto node per channel, as last sent.
    pub gains: BTreeMap<String, Vec<f32>>,
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
    /// A ratio duck's control.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ratio: Option<ratio::RatioStatus>,
}

fn to_db(g: f64) -> Option<f64> {
    // + 0.0: no "-0.0".
    (g > 0.0).then(|| (20.0 * g.log10() * 100.0).round() / 100.0 + 0.0)
}

fn power(lufs: f64) -> f64 {
    if lufs <= SILENCE_LUFS {
        0.0
    } else {
        10f64.powf((lufs + 0.691) / 10.0)
    }
}

impl Auto {
    pub fn new() -> Self {
        Self {
            slots: vec![None; MAX_VAD],
            valid_from: vec![u32::MAX; MAX_VAD],
            ..Self::default()
        }
    }

    /// Give every signal `mix` analyses a slot (keeping the ones it has) and
    /// return them, for the compiler.
    pub fn assign(&mut self, mix: &Mix) -> HashMap<String, usize> {
        if self.slots.len() != MAX_VAD {
            self.slots = vec![None; MAX_VAD];
            self.valid_from = vec![u32::MAX; MAX_VAD];
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
                self.valid_from[k] = u32::MAX;
                self.fresh.insert(k);
                self.tapped.remove(signal);
                if let Some(v) = self.vads.get_mut(signal) {
                    v.reset();
                }
            }
        }
        self.vads.retain(|s, _| wanted.contains_key(s));
        self.tapped.retain(|s, _| wanted.contains_key(s));
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(k, s)| s.clone().map(|s| (s, k)))
            .collect()
    }

    /// The plan `generation` with the current slots is sent: the fresh slots'
    /// records count from it on.
    pub fn planned(&mut self, generation: u32) {
        for k in std::mem::take(&mut self.fresh) {
            if let Some(v) = self.valid_from.get_mut(k) {
                *v = generation;
            }
        }
    }

    /// The gain each auto node of `mix` should be compiled with, per channel.
    pub fn compile_gains(&self, mix: &Mix) -> HashMap<String, Vec<f32>> {
        mix.auto_signals()
            .into_iter()
            .filter_map(|s| {
                let g = self.gains.get(&s)?.clone();
                Some((s, g))
            })
            .collect()
    }

    /// Take the records into the signals' frames.
    fn take(&mut self, ring: &mut rtrb::Consumer<f32>, mix: &Mix) {
        let (slots, valid_from, tapped) = (&self.slots, &self.valid_from, &mut self.tapped);
        crate::engine::meter::read(ring, &mut self.scratch, |generation, slot, chans| {
            let slot = slot as usize;
            let Some(Some(signal)) = slots.get(slot) else {
                return;
            };
            if generation < valid_from[slot] || chans.is_empty() {
                return;
            }
            if !tapped.contains_key(signal) {
                tapped.insert(signal.clone(), Tapped::default());
            }
            let Some(t) = tapped.get_mut(signal) else {
                return;
            };
            if t.partial.len() != chans.len() {
                let weights = mix
                    .signal(signal)
                    .map(|(_, f)| f.labels())
                    .filter(|l| l.len() == chans.len())
                    .map_or_else(|| vec![1.0; chans.len()], crate::dsp::loudness::weights);
                *t = Tapped {
                    level: Level::new(weights),
                    partial: vec![Vec::with_capacity(4 * vad::FRAME); chans.len()],
                    mean: Vec::with_capacity(vad::FRAME),
                };
            }
            for (p, c) in t.partial.iter_mut().zip(chans) {
                p.extend_from_slice(c);
            }
        });
    }

    /// Cut the signals into frames: loudness and VAD.
    fn frames(&mut self, mix: &Mix) -> BTreeMap<String, Vec<Frame>> {
        let settings = mix.vad_signals();
        let mut frames: BTreeMap<String, Vec<Frame>> = BTreeMap::new();
        for (signal, t) in &mut self.tapped {
            let Some(s) = settings.get(signal) else {
                continue;
            };
            let v = self
                .vads
                .entry(signal.clone())
                .or_insert_with(|| vad::Vad::with(s.detector));
            if v.wanted != s.detector {
                *v = vad::Vad::with(s.detector);
            }
            let list = frames.entry(signal.clone()).or_default();
            while t.partial.first().is_some_and(|p| p.len() >= vad::FRAME) {
                let n = t.partial.len().min(MAX_CH);
                let mut chans: [&[f32]; MAX_CH] = [&[]; MAX_CH];
                for (c, p) in chans.iter_mut().zip(&t.partial) {
                    *c = &p[..vad::FRAME];
                }
                let r = t.level.frame(&chans[..n]);
                t.mean.clear();
                t.mean.resize(vad::FRAME, 0.0);
                let scale = 1.0 / n as f32;
                for c in &chans[..n] {
                    for (m, x) in t.mean.iter_mut().zip(c.iter()) {
                        *m += x * scale;
                    }
                }
                v.frame(&t.mean, r.momentary, s);
                v.state.short_term_lufs = (r.short_term * 10.0).round() / 10.0;
                let st = &v.state;
                list.push(Frame {
                    vad: automix::Input {
                        power: 10f64.powf(st.level_dbfs / 10.0),
                        probability: st.speech_probability,
                        active: st.speech_active,
                    },
                    reading: r,
                });
                for p in &mut t.partial {
                    p.drain(..vad::FRAME);
                }
            }
        }
        frames
    }

    fn level_gain(&self, signal: &str) -> f64 {
        self.autolevels
            .get(signal)
            .map_or(1.0, autolevel::State::gain)
    }

    /// Take the ring, run the VADs, auto-levels, auto-mixes and ducks; returns
    /// the auto nodes whose gain changed, with the new gain per channel.
    pub fn tick(
        &mut self,
        ring: &mut rtrb::Consumer<f32>,
        mix: &Mix,
    ) -> BTreeMap<String, Vec<f32>> {
        self.take(ring, mix);
        let frames = self.frames(mix);
        let count = frames.values().map(Vec::len).max().unwrap_or(0);
        let own = |signal: &str, k: usize| frames.get(signal).and_then(|f| f.get(k));
        let at = |signal: &str, k: usize| {
            let f = frames.get(signal)?;
            f.get(k).or(f.last())
        };

        let effects = mix.vca_effects();
        let grouped = mix.group_muted();
        let muted: BTreeSet<String> = mix
            .strips
            .iter()
            .filter(|s| mix.fader_gain(&s.id, s.fader_db, s.mute, &effects, &grouped) == 0.0)
            .map(|s| s.id.clone())
            .collect();
        self.autolevels.retain(|s, _| mix.autolevel(s).is_some());
        self.automixes.retain(|id, _| mix.automix(id).is_some());
        self.ducks
            .retain(|id, _| mix.duck(id).is_some_and(|d| d.mode == DuckMode::Fixed));
        self.ratios
            .retain(|id, _| mix.duck(id).is_some_and(|d| d.mode == DuckMode::Ratio));
        for l in mix.autolevels.iter().filter(|l| !l.enabled) {
            self.autolevels.entry(l.signal.clone()).or_default().frame(
                l,
                &Reading::default(),
                false,
            );
        }
        for a in mix.automixes.iter().filter(|a| !a.enabled) {
            self.automixes
                .entry(a.id.clone())
                .or_default()
                .frame(a, &BTreeMap::new(), &muted);
        }
        for d in mix.ducks.iter().filter(|d| !d.enabled) {
            match d.mode {
                DuckMode::Fixed => self.ducks.entry(d.id.clone()).or_default().frame(d, false),
                DuckMode::Ratio => {
                    let r = self
                        .ratios
                        .entry(d.id.clone())
                        .or_insert_with(|| ratio::State::new(d));
                    r.frame(d, &ratio::Inputs::silent());
                }
            }
        }

        for k in 0..count {
            for l in mix.autolevels.iter().filter(|l| l.enabled) {
                if let Some(f) = own(&l.signal, k) {
                    self.autolevels.entry(l.signal.clone()).or_default().frame(
                        l,
                        &f.reading,
                        f.vad.active,
                    );
                }
            }
            for a in mix.automixes.iter().filter(|a| a.enabled) {
                let inputs = a
                    .members
                    .iter()
                    .filter_map(|m| {
                        let f = at(&m.signal, k)?;
                        let g = self.level_gain(&m.signal);
                        let input = automix::Input {
                            power: f.vad.power * g * g,
                            ..f.vad
                        };
                        Some((m.signal.clone(), input))
                    })
                    .collect();
                self.automixes
                    .entry(a.id.clone())
                    .or_default()
                    .frame(a, &inputs, &muted);
            }
            for d in mix.ducks.iter().filter(|d| d.enabled) {
                let speech =
                    |list: &[String]| list.iter().any(|s| at(s, k).is_some_and(|f| f.vad.active));
                match d.mode {
                    DuckMode::Fixed => {
                        let key = speech(&d.keys);
                        self.ducks.entry(d.id.clone()).or_default().frame(d, key);
                    }
                    DuckMode::Ratio => {
                        let sum = |list: &[String], pick: fn(&Reading) -> f64| {
                            let p: f64 = list
                                .iter()
                                .filter_map(|s| {
                                    let f = at(s, k)?;
                                    let g = self.level_gain(s);
                                    Some(power(pick(&f.reading)) * g * g)
                                })
                                .sum();
                            level::lufs(p)
                        };
                        let inputs = ratio::Inputs {
                            key_voice: speech(&d.keys),
                            target_voice: speech(&d.targets),
                            dialogue_short_term: sum(&d.keys, |r| r.short_term),
                            bed_short_term: sum(&d.targets, |r| r.short_term),
                            bed_momentary: sum(&d.targets, |r| r.momentary),
                            bed_fast: sum(&d.targets, |r| r.fast),
                        };
                        self.ratios
                            .entry(d.id.clone())
                            .or_insert_with(|| ratio::State::new(d))
                            .frame(d, &inputs);
                    }
                }
            }
        }

        let mut changed = BTreeMap::new();
        let targets = mix.auto_signals();
        self.gains.retain(|s, _| targets.contains(s));
        for s in targets {
            let g = self.gain(mix, &s);
            let old = self.gains.get(&s);
            let differs = match old {
                Some(o) => {
                    o.len() != g.len() || o.iter().zip(&g).any(|(a, b)| (a - b).abs() > 1e-6)
                }
                None => g.iter().any(|x| (x - 1.0).abs() > 1e-6),
            };
            if differs {
                changed.insert(s.clone(), g.clone());
            }
            self.gains.insert(s, g);
        }
        changed
    }

    /// `signal`'s auto gain per channel: auto-level × auto-mix × each channel's
    /// deepest duck (its share of each duck's dB).
    fn gain(&self, mix: &Mix, signal: &str) -> Vec<f32> {
        let labels = mix.signal(signal).map_or(&["M"][..], |(_, f)| f.labels());
        let am = mix
            .automix_of(signal)
            .and_then(|a| self.automixes.get(&a.id))
            .and_then(|st| st.gains.get(signal))
            .copied()
            .unwrap_or(1.0);
        let base = self.level_gain(signal) * am;
        let mut duck = vec![1.0f64; labels.len()];
        for d in mix
            .ducks
            .iter()
            .filter(|d| d.targets.iter().any(|t| t == signal))
        {
            let (db, gain) = match d.mode {
                DuckMode::Fixed => {
                    let st = self.ducks.get(&d.id).copied().unwrap_or_default();
                    (st.gain_db, st.gain())
                }
                DuckMode::Ratio => {
                    let r = self.ratios.get(&d.id).map_or(0.0, |r| r.reduction_db);
                    (-r, 10f64.powf(-r / 20.0))
                }
            };
            let shares = d.channel_depth.per_channel(labels);
            for (g, share) in duck.iter_mut().zip(shares) {
                let x = if share == 1.0 {
                    gain
                } else {
                    10f64.powf(db * share / 20.0)
                };
                *g = g.min(x);
            }
        }
        duck.into_iter().map(|d| (base * d) as f32).collect()
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
        if let Some(r) = self.ratios.get(id) {
            return DuckStatus {
                active: matches!(r.status.state, "duck" | "overvoice"),
                gain_db: to_db(10f64.powf(-r.reduction_db / 20.0)),
                ratio: Some(r.status),
            };
        }
        let st = self.ducks.get(id).copied().unwrap_or_default();
        DuckStatus {
            active: st.active,
            gain_db: to_db(st.gain()),
            ratio: None,
        }
    }

    pub fn autolevel_status(&self, signal: &str) -> Option<autolevel::LevelStatus> {
        self.autolevels.get(signal).map(|st| st.status)
    }

    /// Each auto node's gain in dB, its deepest channel's (−∞ as `null`), for
    /// the meters message.
    pub fn gains_db(&self) -> BTreeMap<String, Option<f64>> {
        self.gains
            .iter()
            .map(|(s, g)| {
                let min = g.iter().copied().fold(f32::INFINITY, f32::min);
                (
                    s.clone(),
                    to_db(f64::from(if min.is_finite() { min } else { 1.0 })),
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::meter;
    use crate::mix::auto::{AutoLevel, Automix, AutomixMember, Duck, VadSettings};
    use crate::mix::model::tests::strip;
    use crate::mix::model::Db;
    use crate::mix::Format;
    use std::sync::atomic::AtomicU64;

    fn mix() -> Mix {
        let mut m = Mix::default();
        for id in ["a", "b", "music"] {
            m.strips.push(strip(id, Format::Mono));
        }
        m
    }

    /// The DSP thread's side: `chans` of slot `slot` of plan `generation`, in
    /// 96-sample blocks.
    fn feed(tx: &mut rtrb::Producer<f32>, generation: u32, slot: usize, chans: &[&[f32]]) {
        let drops = AtomicU64::new(0);
        let len = chans[0].len();
        for start in (0..len).step_by(96) {
            let end = (start + 96).min(len);
            let block: Vec<&[f32]> = chans.iter().map(|c| &c[start..end]).collect();
            meter::write(tx, &drops, generation, slot as u32, &block);
        }
        assert_eq!(drops.into_inner(), 0);
    }

    #[test]
    fn slots_are_kept_freed_and_fresh() {
        let mut auto = Auto::new();
        let mut m = mix();
        m.vad.push(VadSettings::new("b"));
        let s = auto.assign(&m);
        assert_eq!(s, HashMap::from([("b".to_string(), 0)]));
        auto.planned(1);
        m.vad.insert(0, VadSettings::new("a"));
        let s = auto.assign(&m);
        assert_eq!((s["a"], s["b"]), (1, 0), "b keeps its slot");
        m.vad.retain(|v| v.signal != "b");
        m.vad.push(VadSettings::new("music"));
        let s = auto.assign(&m);
        assert_eq!((s["a"], s["music"]), (1, 0), "b's slot reused");
        assert!(auto.fresh.contains(&0));
        let (mut tx, mut rx) = rtrb::RingBuffer::new(100_000);
        // Plan 1 still runs (b in slot 0); plan 2 has music there.
        let x = vec![0.5f32; 960];
        feed(&mut tx, 1, 0, &[&x]);
        auto.tick(&mut rx, &m);
        assert!(
            !auto.tapped.contains_key("music"),
            "before the plan: not music's"
        );
        auto.planned(2);
        feed(&mut tx, 1, 0, &[&x]);
        feed(&mut tx, 2, 0, &[&x]);
        auto.tick(&mut rx, &m);
        assert_eq!(
            auto.vad_state("music").map(|v| v.level_dbfs.round()),
            Some(-6.0),
            "plan 2's records are music's"
        );
        assert!(auto.fresh.is_empty());
        assert!(rx.is_empty());
    }

    fn ring() -> (rtrb::Producer<f32>, rtrb::Consumer<f32>) {
        rtrb::RingBuffer::new(crate::engine::VAD_RING)
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
            recall_safe: false,
        });
        let mut d = Duck::new("bed");
        d.keys = vec!["a".into()];
        d.targets = vec!["music".into()];
        d.depth_db = Db(-20.0);
        d.attack_ms = 0.0;
        m.ducks.push(d);
        for v in ["a", "b"] {
            m.vad.push(VadSettings {
                detector: crate::mix::auto::VadDetector::Features,
                ..VadSettings::new(v)
            });
        }
        let slots = auto.assign(&m);
        auto.planned(1);
        let (mut tx, mut rx) = ring();
        assert!(auto.compile_gains(&m).is_empty());
        // a speaks, b has quiet noise; 2 s, 10 ms per tick.
        let (speech, _) = vad::tests::speech(2.0, 0.06, 1);
        let noise = vad::tests::pink(2.0, 0.0005, 2);
        let mut last = BTreeMap::new();
        let mut leads = 0;
        for k in 0..200 {
            let r = k * vad::FRAME..(k + 1) * vad::FRAME;
            feed(&mut tx, 1, slots["a"], &[&speech[r.clone()]]);
            feed(&mut tx, 1, slots["b"], &[&noise[r]]);
            for (s, g) in auto.tick(&mut rx, &m) {
                last.insert(s, g[0]);
            }
            let g = |s: &str| last.get(s).copied().unwrap_or(1.0);
            leads += usize::from(g("a") > 10.0 * g("b"));
        }
        assert!(auto.vad_state("a").unwrap().speech_onsets >= 1);
        assert_eq!(auto.duck_status("bed").gain_db, Some(-20.0));
        assert!((last["music"] - 0.1).abs() < 1e-6);
        // The talker's share is 20 dB above the noise most of the time (in his
        // pauses the shares move towards each other, as gain sharing does).
        assert!(leads > 120, "{leads} of 200 ticks; {last:?}");
        // The duck removed: the music comes back to 0 dB.
        m.ducks.clear();
        let changed = auto.tick(&mut rx, &m);
        assert!(!changed.contains_key("music") && !auto.gains.contains_key("music"));
    }

    /// A 1 kHz tone of `lufs` (as a mono signal), frame `k`.
    fn tone(lufs: f64, k: usize) -> Vec<f32> {
        let peak = 10f64.powf((lufs + 3.01) / 20.0);
        (k * vad::FRAME..(k + 1) * vad::FRAME)
            .map(|i| {
                let t = i as f64 / 48_000.0;
                (peak * (2.0 * std::f64::consts::PI * 1000.0 * t).sin()) as f32
            })
            .collect()
    }

    #[test]
    fn a_ratio_duck_on_a_surround_bed_and_an_auto_level() {
        let mut auto = Auto::new();
        let mut m = mix();
        m.strips.push(strip("bed", Format::Surround51));
        let mut d = Duck::new("com");
        d.mode = DuckMode::Ratio;
        d.keys = vec!["a".into()];
        d.targets = vec!["bed".into()];
        d.channel_depth.lfe = 0.0;
        d.channel_depth.surround = 0.5;
        m.ducks.push(d);
        m.autolevels.push(AutoLevel::new("b"));
        for v in ["a", "b", "bed"] {
            m.vad.push(VadSettings {
                detector: crate::mix::auto::VadDetector::Features,
                // The commentator talks from his first word to the end.
                hold_ms: if v == "a" { 10_000.0 } else { 800.0 },
                ..VadSettings::new(v)
            });
        }
        assert_eq!(m.validate(4, 48), Ok(()));
        let analysed: Vec<_> = m.vad_signals().into_keys().collect();
        assert_eq!(analysed, ["a", "b", "bed"], "the bed's VAD (interview)");
        let slots = auto.assign(&m);
        auto.planned(7);
        let (mut tx, mut rx) = ring();
        // The commentator speaks; the bed's five channels are tones of −30
        // LUFS each (−22.35 LUFS as a whole: the surrounds weigh 1.41, the LFE
        // nothing); b is a tone of −33 LUFS.
        let (speech, _) = vad::tests::speech(15.0, 0.05, 3);
        let mut last: BTreeMap<String, Vec<f32>> = BTreeMap::new();
        for k in 0..1500 {
            let ch = tone(-30.0, k);
            let lfe = vec![0.0f32; vad::FRAME];
            feed(
                &mut tx,
                7,
                slots["a"],
                &[&speech[k * vad::FRAME..(k + 1) * vad::FRAME]],
            );
            feed(&mut tx, 7, slots["bed"], &[&ch, &ch, &ch, &lfe, &ch, &ch]);
            feed(&mut tx, 7, slots["b"], &[&tone(-33.0, k)]);
            last.extend(auto.tick(&mut rx, &m));
        }
        let r = auto.duck_status("com").ratio.expect("a ratio duck");
        let bed = auto.vad_state("bed").unwrap().short_term_lufs;
        assert!(
            (bed - (-30.0 + 10.0 * (3.0 + 2.0 * 1.41f64).log10())).abs() < 0.1,
            "{bed}"
        );
        assert_eq!(r.state, "duck", "{r:?}");
        assert!(r.reduction_db > 3.0, "{r:?}");
        // The ratio held near 5 LU (the speech's short-term loudness moves).
        let ratio = r.ratio_lu.unwrap();
        assert!((ratio - 5.0).abs() < 1.5, "{r:?}");
        // The channels: all of the dB on the front and centre, half on the
        // surrounds, none on the LFE.
        let g = &last["bed"];
        let db = |x: f32| 20.0 * f64::from(x).log10();
        assert!((db(g[0]) + r.reduction_db).abs() < 0.02, "{g:?} {r:?}");
        assert_eq!((g[0], g[1], g[2]), (g[0], g[0], g[0]));
        assert_eq!(g[3], 1.0, "the LFE is not ducked");
        assert!((db(g[4]) - db(g[0]) / 2.0).abs() < 1e-3 && g[4] == g[5]);
        assert_eq!(auto.gains_db()["bed"], to_db(f64::from(g[0])));
        // b auto-levelled from −33 to −23 LUFS (the 3 s release: 15 s is 5 of
        // its time constants).
        let lv = auto.autolevel_status("b").unwrap();
        assert!((lv.gain_db - 10.0).abs() < 0.1, "{lv:?}");
        assert!((db(last["b"][0]) - lv.gain_db).abs() < 0.01);
    }
}
