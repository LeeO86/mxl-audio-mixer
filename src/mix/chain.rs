//! Processing chains (§6): the nodes a strip or bus runs between its trim (or
//! sum) and its fader. Every node has an id unique within its signal, a `type`,
//! `bypass`, and the parameters of its type. Numeric parameters are smoothed by
//! the DSP thread; the others (band types, slopes, modes, look-ahead, delay time,
//! band count) define the node's structure, and changing them swaps the plan.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use serde::{Deserialize, Serialize};

use super::model::Tap;

/// Most nodes in one chain.
pub const MAX_NODES: usize = 32;
/// Most bands of a (dynamic) EQ (§6: up to 8).
pub const MAX_BANDS: usize = 8;
/// Longest delay node, in ms.
pub const MAX_DELAY_MS: f64 = 2000.0;
/// EQ band gain range, ± dB.
pub const EQ_GAIN_DB: f64 = 24.0;

/// A key (sidechain) source: another signal at one of its taps.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Key {
    pub signal: String,
    pub tap: Tap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BandType {
    Bell,
    LowShelf,
    HighShelf,
    Notch,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EqBand {
    #[serde(rename = "type")]
    pub kind: BandType,
    pub freq_hz: f64,
    pub q: f64,
    #[serde(default)]
    pub gain_db: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FilterMode {
    HighPass,
    LowPass,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DynamicBand {
    pub freq_hz: f64,
    pub q: f64,
    pub gain_db: f64,
    pub threshold_db: f64,
    pub ratio: f64,
    pub attack_ms: f64,
    pub release_ms: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Detector {
    Peak,
    Rms,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeEsserMode {
    SplitBand,
    WideBand,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MultibandBand {
    pub threshold_db: f64,
    pub ratio: f64,
    pub attack_ms: f64,
    pub release_ms: f64,
    pub makeup_db: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DelayUnit {
    Samples,
    Ms,
    Frames,
}

/// What an oscillator plays (§10.2 line-up).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Waveform {
    Sine,
    /// The stereo ident of EBU R 49: the tone on every channel, the first (left)
    /// one interrupted for 0.25 s every 3 s.
    EbuIdent,
}

/// A node's type and parameters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ProcKind {
    /// Parametric EQ, up to 8 bands.
    Eq { bands: Vec<EqBand> },
    /// Butterworth high- or low-pass, 12–48 dB/oct.
    Filter {
        mode: FilterMode,
        freq_hz: f64,
        slope_db_per_oct: u32,
    },
    /// Bells whose gain follows a per-band compressor on the band's level.
    DynamicEq { bands: Vec<DynamicBand> },
    /// Feed-forward compressor with soft knee, sidechain filter and key.
    Compressor {
        threshold_db: f64,
        ratio: f64,
        knee_db: f64,
        attack_ms: f64,
        release_ms: f64,
        makeup_db: f64,
        detector: Detector,
        #[serde(default)]
        sidechain_hpf_hz: Option<f64>,
        #[serde(default)]
        key: Option<Key>,
    },
    /// Look-ahead peak limiter, optionally on true peak.
    Limiter {
        ceiling_db: f64,
        release_ms: f64,
        lookahead_ms: f64,
        true_peak: bool,
    },
    /// Downward expander.
    Expander {
        threshold_db: f64,
        ratio: f64,
        range_db: f64,
        attack_ms: f64,
        release_ms: f64,
    },
    /// Gate with hysteresis and hold.
    Gate {
        threshold_db: f64,
        hysteresis_db: f64,
        attack_ms: f64,
        hold_ms: f64,
        release_ms: f64,
        range_db: f64,
    },
    /// De-esser: split-band (reduces the band above `freq_hz`) or wide-band
    /// (reduces everything when the band above `freq_hz` is loud).
    DeEsser {
        mode: DeEsserMode,
        freq_hz: f64,
        threshold_db: f64,
        range_db: f64,
        attack_ms: f64,
        release_ms: f64,
        listen: bool,
    },
    /// Ducker keyed by another signal.
    Ducker {
        key: Key,
        threshold_db: f64,
        depth_db: f64,
        attack_ms: f64,
        hold_ms: f64,
        release_ms: f64,
    },
    /// Multiband compressor, 3–5 bands with Linkwitz-Riley crossovers.
    Multiband {
        crossovers_hz: Vec<f64>,
        bands: Vec<MultibandBand>,
    },
    /// A tone generator: replaces the signal with a sine at `level_dbfs` on every
    /// channel (bypass: the input again).
    Oscillator {
        waveform: Waveform,
        freq_hz: f64,
        level_dbfs: f64,
    },
    /// Delay in samples, ms or frames (at `frame_rate`).
    Delay {
        time: f64,
        unit: DelayUnit,
        #[serde(default)]
        frame_rate: Option<f64>,
    },
}

/// One node of a chain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Proc {
    pub id: String,
    #[serde(default)]
    pub bypass: bool,
    #[serde(flatten)]
    pub kind: ProcKind,
}

/// Which named taps a node delimits (§5: pre/post-EQ, pre/post-dynamics).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Eq,
    Dynamics,
    Other,
}

fn range(what: &str, v: f64, lo: f64, hi: f64) -> Result<(), String> {
    if v.is_finite() && (lo..=hi).contains(&v) {
        Ok(())
    } else {
        Err(format!("{what} must be {lo} to {hi}, not {v}"))
    }
}

fn freq(v: f64) -> Result<(), String> {
    range("freq_hz", v, 10.0, 20_000.0)
}

fn times(attack: f64, release: f64) -> Result<(), String> {
    range("attack_ms", attack, 0.0, 1000.0)?;
    range("release_ms", release, 0.0, 10_000.0)
}

fn ratio(v: f64) -> Result<(), String> {
    range("ratio", v, 1.0, 1000.0)
}

fn threshold(v: f64) -> Result<(), String> {
    range("threshold_db", v, -100.0, 0.0)
}

impl ProcKind {
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Eq { .. } => "eq",
            Self::Filter { .. } => "filter",
            Self::DynamicEq { .. } => "dynamic-eq",
            Self::Compressor { .. } => "compressor",
            Self::Limiter { .. } => "limiter",
            Self::Expander { .. } => "expander",
            Self::Oscillator { .. } => "oscillator",
            Self::Gate { .. } => "gate",
            Self::DeEsser { .. } => "de-esser",
            Self::Ducker { .. } => "ducker",
            Self::Multiband { .. } => "multiband",
            Self::Delay { .. } => "delay",
        }
    }

    pub fn category(&self) -> Category {
        match self {
            Self::Eq { .. } | Self::Filter { .. } | Self::DynamicEq { .. } => Category::Eq,
            Self::Delay { .. } | Self::Oscillator { .. } => Category::Other,
            _ => Category::Dynamics,
        }
    }

    pub fn key(&self) -> Option<&Key> {
        match self {
            Self::Compressor { key, .. } => key.as_ref(),
            Self::Ducker { key, .. } => Some(key),
            _ => None,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Eq { bands } => {
                if bands.is_empty() || bands.len() > MAX_BANDS {
                    return Err(format!("an EQ has 1 to {MAX_BANDS} bands"));
                }
                for b in bands {
                    freq(b.freq_hz)?;
                    range("q", b.q, 0.1, 40.0)?;
                    range("gain_db", b.gain_db, -EQ_GAIN_DB, EQ_GAIN_DB)?;
                }
            }
            Self::Filter {
                freq_hz,
                slope_db_per_oct,
                ..
            } => {
                freq(*freq_hz)?;
                if ![12, 24, 36, 48].contains(slope_db_per_oct) {
                    return Err("slope_db_per_oct must be 12, 24, 36 or 48".into());
                }
            }
            Self::DynamicEq { bands } => {
                if bands.is_empty() || bands.len() > MAX_BANDS {
                    return Err(format!("a dynamic EQ has 1 to {MAX_BANDS} bands"));
                }
                for b in bands {
                    freq(b.freq_hz)?;
                    range("q", b.q, 0.1, 40.0)?;
                    range("gain_db", b.gain_db, -EQ_GAIN_DB, EQ_GAIN_DB)?;
                    threshold(b.threshold_db)?;
                    ratio(b.ratio)?;
                    times(b.attack_ms, b.release_ms)?;
                }
            }
            Self::Compressor {
                threshold_db,
                ratio: r,
                knee_db,
                attack_ms,
                release_ms,
                makeup_db,
                sidechain_hpf_hz,
                ..
            } => {
                threshold(*threshold_db)?;
                ratio(*r)?;
                range("knee_db", *knee_db, 0.0, 24.0)?;
                times(*attack_ms, *release_ms)?;
                range("makeup_db", *makeup_db, -24.0, 24.0)?;
                if let Some(f) = sidechain_hpf_hz {
                    freq(*f)?;
                }
            }
            Self::Limiter {
                ceiling_db,
                release_ms,
                lookahead_ms,
                ..
            } => {
                range("ceiling_db", *ceiling_db, -40.0, 0.0)?;
                range("release_ms", *release_ms, 1.0, 5000.0)?;
                range("lookahead_ms", *lookahead_ms, 0.1, 20.0)?;
            }
            Self::Expander {
                threshold_db,
                ratio: r,
                range_db,
                attack_ms,
                release_ms,
            } => {
                threshold(*threshold_db)?;
                ratio(*r)?;
                range("range_db", *range_db, 0.0, 100.0)?;
                times(*attack_ms, *release_ms)?;
            }
            Self::Gate {
                threshold_db,
                hysteresis_db,
                attack_ms,
                hold_ms,
                release_ms,
                range_db,
            } => {
                threshold(*threshold_db)?;
                range("hysteresis_db", *hysteresis_db, 0.0, 24.0)?;
                times(*attack_ms, *release_ms)?;
                range("hold_ms", *hold_ms, 0.0, 5000.0)?;
                range("range_db", *range_db, 0.0, 100.0)?;
            }
            Self::DeEsser {
                freq_hz,
                threshold_db,
                range_db,
                attack_ms,
                release_ms,
                ..
            } => {
                range("freq_hz", *freq_hz, 2000.0, 12_000.0)?;
                threshold(*threshold_db)?;
                range("range_db", *range_db, 0.0, 40.0)?;
                times(*attack_ms, *release_ms)?;
            }
            Self::Ducker {
                threshold_db,
                depth_db,
                attack_ms,
                hold_ms,
                release_ms,
                ..
            } => {
                threshold(*threshold_db)?;
                range("depth_db", *depth_db, 0.0, 100.0)?;
                times(*attack_ms, *release_ms)?;
                range("hold_ms", *hold_ms, 0.0, 5000.0)?;
            }
            Self::Multiband {
                crossovers_hz,
                bands,
            } => {
                if !(2..=4).contains(&crossovers_hz.len()) || bands.len() != crossovers_hz.len() + 1
                {
                    return Err("a multiband has 3 to 5 bands and one crossover fewer".into());
                }
                for w in crossovers_hz.windows(2) {
                    if w[1] <= w[0] {
                        return Err("crossovers_hz must rise".into());
                    }
                }
                for f in crossovers_hz {
                    range("crossovers_hz", *f, 20.0, 20_000.0)?;
                }
                for b in bands {
                    threshold(b.threshold_db)?;
                    ratio(b.ratio)?;
                    times(b.attack_ms, b.release_ms)?;
                    range("makeup_db", b.makeup_db, -24.0, 24.0)?;
                }
            }
            Self::Oscillator {
                freq_hz,
                level_dbfs,
                ..
            } => {
                freq(*freq_hz)?;
                range("level_dbfs", *level_dbfs, -100.0, 0.0)?;
            }
            Self::Delay {
                time,
                unit,
                frame_rate,
            } => {
                if *unit == DelayUnit::Frames {
                    range(
                        "frame_rate",
                        frame_rate.ok_or("frames need frame_rate")?,
                        1.0,
                        120.0,
                    )?;
                }
                if !time.is_finite() || *time < 0.0 {
                    return Err("time must be 0 or more".into());
                }
                let ms = self.delay_samples() as f64 / 48.0;
                if ms > MAX_DELAY_MS {
                    return Err(format!("a delay is at most {MAX_DELAY_MS} ms"));
                }
            }
        }
        Ok(())
    }

    /// A delay node's length in samples.
    pub fn delay_samples(&self) -> usize {
        match self {
            Self::Delay {
                time,
                unit,
                frame_rate,
            } => {
                let samples = match unit {
                    DelayUnit::Samples => *time,
                    DelayUnit::Ms => time * 48.0,
                    DelayUnit::Frames => time * 48_000.0 / frame_rate.unwrap_or(25.0),
                };
                samples.round().max(0.0) as usize
            }
            _ => 0,
        }
    }

    /// Latency the node adds (and delay compensation offsets), in samples. An
    /// explicit delay node is an operator's choice, not latency.
    pub fn latency(&self) -> usize {
        match self {
            Self::Limiter { lookahead_ms, .. } => (lookahead_ms * 48.0).round() as usize,
            _ => 0,
        }
    }

    /// The smoothed parameters, in a fixed order per type.
    pub fn values(&self) -> Vec<f32> {
        let v: Vec<f64> = match self {
            Self::Eq { bands } => bands
                .iter()
                .flat_map(|b| [b.freq_hz, b.q, b.gain_db])
                .collect(),
            Self::Filter { freq_hz, .. } => vec![*freq_hz],
            Self::DynamicEq { bands } => bands
                .iter()
                .flat_map(|b| {
                    [
                        b.freq_hz,
                        b.q,
                        b.gain_db,
                        b.threshold_db,
                        b.ratio,
                        b.attack_ms,
                        b.release_ms,
                    ]
                })
                .collect(),
            Self::Compressor {
                threshold_db,
                ratio,
                knee_db,
                attack_ms,
                release_ms,
                makeup_db,
                sidechain_hpf_hz,
                ..
            } => vec![
                *threshold_db,
                *ratio,
                *knee_db,
                *attack_ms,
                *release_ms,
                *makeup_db,
                sidechain_hpf_hz.unwrap_or(0.0),
            ],
            Self::Limiter {
                ceiling_db,
                release_ms,
                ..
            } => vec![*ceiling_db, *release_ms],
            Self::Expander {
                threshold_db,
                ratio,
                range_db,
                attack_ms,
                release_ms,
            } => vec![*threshold_db, *ratio, *range_db, *attack_ms, *release_ms],
            Self::Gate {
                threshold_db,
                hysteresis_db,
                attack_ms,
                hold_ms,
                release_ms,
                range_db,
            } => vec![
                *threshold_db,
                *hysteresis_db,
                *attack_ms,
                *hold_ms,
                *release_ms,
                *range_db,
            ],
            Self::DeEsser {
                freq_hz,
                threshold_db,
                range_db,
                attack_ms,
                release_ms,
                ..
            } => vec![*freq_hz, *threshold_db, *range_db, *attack_ms, *release_ms],
            Self::Ducker {
                threshold_db,
                depth_db,
                attack_ms,
                hold_ms,
                release_ms,
                ..
            } => vec![*threshold_db, *depth_db, *attack_ms, *hold_ms, *release_ms],
            Self::Multiband {
                crossovers_hz,
                bands,
            } => crossovers_hz
                .iter()
                .copied()
                .chain(bands.iter().flat_map(|b| {
                    [
                        b.threshold_db,
                        b.ratio,
                        b.attack_ms,
                        b.release_ms,
                        b.makeup_db,
                    ]
                }))
                .collect(),
            Self::Oscillator {
                freq_hz,
                level_dbfs,
                ..
            } => vec![*freq_hz, *level_dbfs],
            Self::Delay { .. } => Vec::new(),
        };
        v.into_iter().map(|x| x as f32).collect()
    }

    /// A hash of everything that is not smoothed.
    pub fn signature(&self) -> u64 {
        let mut h = DefaultHasher::new();
        self.type_name().hash(&mut h);
        match self {
            Self::Eq { bands } => {
                for b in bands {
                    b.kind.hash(&mut h);
                }
            }
            Self::Filter {
                mode,
                slope_db_per_oct,
                ..
            } => (mode, slope_db_per_oct).hash(&mut h),
            Self::DynamicEq { bands } => bands.len().hash(&mut h),
            Self::Compressor {
                detector,
                sidechain_hpf_hz,
                key,
                ..
            } => {
                detector.hash(&mut h);
                sidechain_hpf_hz.is_some().hash(&mut h);
                key.as_ref()
                    .map(|k| (&k.signal, format!("{:?}", k.tap)))
                    .hash(&mut h);
            }
            Self::Limiter {
                lookahead_ms,
                true_peak,
                ..
            } => (self.latency(), lookahead_ms.to_bits(), true_peak).hash(&mut h),
            Self::DeEsser { mode, listen, .. } => (mode, listen).hash(&mut h),
            Self::Ducker { key, .. } => (&key.signal, format!("{:?}", key.tap)).hash(&mut h),
            Self::Multiband { bands, .. } => bands.len().hash(&mut h),
            Self::Delay { .. } => self.delay_samples().hash(&mut h),
            Self::Oscillator { waveform, .. } => waveform.hash(&mut h),
            Self::Expander { .. } | Self::Gate { .. } => {}
        }
        h.finish()
    }
}

impl Proc {
    /// Every value the DSP thread smooths: 0 = the wet share (bypass), then the
    /// type's parameters.
    pub fn values(&self) -> Vec<f32> {
        let mut v = vec![if self.bypass { 0.0 } else { 1.0 }];
        v.extend(self.kind.values());
        v
    }
}

/// Check a chain: ids, count, every node's parameters.
pub fn validate(chain: &[Proc]) -> Result<(), String> {
    if chain.len() > MAX_NODES {
        return Err(format!("a chain has at most {MAX_NODES} nodes"));
    }
    let mut ids = std::collections::BTreeSet::new();
    for p in chain {
        let ok = !p.id.is_empty()
            && p.id.len() <= 32
            && p.id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if !ok {
            return Err(format!(
                "node id {:?} must be 1 to 32 characters a-z, 0-9 and '-'",
                p.id
            ));
        }
        if !ids.insert(p.id.as_str()) {
            return Err(format!("node id {} is used twice", p.id));
        }
        p.kind
            .validate()
            .map_err(|e| format!("node {}: {e}", p.id))?;
    }
    Ok(())
}

/// A free node id `<type>-<n>` in `chain`.
pub fn next_id(chain: &[Proc], kind: &ProcKind) -> String {
    (1..)
        .map(|n| format!("{}-{n}", kind.type_name()))
        .find(|id| chain.iter().all(|p| &p.id != id))
        .expect("unbounded")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_shape_and_validation() {
        let p: Proc = serde_json::from_str(
            r#"{"id": "eq-1", "type": "eq", "bands": [{"type": "bell", "freq_hz": 1000, "q": 1, "gain_db": 3}]}"#,
        )
        .unwrap();
        assert!(!p.bypass);
        assert_eq!(p.kind.type_name(), "eq");
        assert_eq!(validate(std::slice::from_ref(&p)), Ok(()));
        assert_eq!(p.values(), vec![1.0, 1000.0, 1.0, 3.0]);
        let back = serde_json::to_value(&p).unwrap();
        assert_eq!(back["type"], "eq");
        assert_eq!(back["bands"][0]["type"], "bell");

        assert!(serde_json::from_str::<Proc>(
            r#"{"id": "x", "type": "eq", "bands": [], "extra": 1}"#
        )
        .is_err());
        let bad: Proc = serde_json::from_str(
            r#"{"id": "f", "type": "filter", "mode": "high-pass", "freq_hz": 80, "slope_db_per_oct": 18}"#,
        )
        .unwrap();
        assert!(validate(&[bad]).unwrap_err().contains("slope"));
        let de: Proc = serde_json::from_str(
            r#"{"id": "d", "type": "de-esser", "mode": "split-band", "freq_hz": 1500, "threshold_db": -30, "range_db": 6, "attack_ms": 1, "release_ms": 50, "listen": false}"#,
        )
        .unwrap();
        assert!(validate(&[de]).unwrap_err().contains("2000"));
    }

    #[test]
    fn structure_and_latency() {
        let lim = |look: f64, ceiling: f64| ProcKind::Limiter {
            ceiling_db: ceiling,
            release_ms: 50.0,
            lookahead_ms: look,
            true_peak: false,
        };
        assert_eq!(lim(1.5, -1.0).latency(), 72);
        assert_eq!(lim(1.5, -1.0).signature(), lim(1.5, -3.0).signature());
        assert_ne!(lim(1.5, -1.0).signature(), lim(2.0, -1.0).signature());
        let delay = ProcKind::Delay {
            time: 2.0,
            unit: DelayUnit::Frames,
            frame_rate: Some(25.0),
        };
        assert_eq!(delay.delay_samples(), 3840);
        assert_eq!(delay.latency(), 0);
        assert!(ProcKind::Delay {
            time: 3.0,
            unit: DelayUnit::Frames,
            frame_rate: None
        }
        .validate()
        .is_err());
        assert!(ProcKind::Delay {
            time: 2001.0,
            unit: DelayUnit::Ms,
            frame_rate: None
        }
        .validate()
        .is_err());
    }
}
