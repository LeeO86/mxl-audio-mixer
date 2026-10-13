//! The plan compiler (§2): mix → execution plan.
//!
//! Signals are emitted in dependency order (a signal after every signal it reads
//! through a send or a key; block-delay sends excepted). A strip is patch → trim →
//! chain → fader → panner; a bus is the sum of its sends → chain → fader. Sends
//! into a bus are delay-compensated (§4.4) unless the bus turns it off; a
//! block-delay send reads its source as it was one block earlier. Then the
//! published outputs (width mapping → output slot).
//!
//! Every gain node and processor has a key (`strip/<id>/fader`, `proc/<id>/<node>`,
//! `send/<from>/<to>`, …) that maps to a stable [`NodeId`], and a signature of its
//! wiring: the control side carries values and state over a swap only when the
//! signature is the same.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use super::chain::{Category, Proc, ProcKind};
use super::format::{pan_gains, stereo_matrix, width_matrix, Format};
use super::model::{Db, Kind, Mix, Patch, Tap};
use super::room::{check_matrix, monitor_gain, Monitor, SoloKind, Source};
use super::talkback::{destination_gains, dims_control_room, MicInfo, Talking};
use crate::engine::plan::{NodeId, Plan, PlanBuilder, Processor, Span, Step};

/// A gain node of a compiled plan, as the control side remembers it.
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutNode {
    pub index: u32,
    pub signature: u64,
    pub values: Vec<f32>,
}

/// A compiled plan and what the control side needs to know about it.
pub struct Compiled {
    pub plan: Plan,
    pub nodes: HashMap<NodeId, LayoutNode>,
    pub procs: HashMap<NodeId, LayoutNode>,
    /// `(signal id, first meter channel, channels)`.
    pub meters: Vec<(String, u32, usize)>,
    /// `(signal id, node id, node meter slot)`.
    pub node_meters: Vec<(String, String, usize)>,
    /// Processing latency of each signal's post-fader point, in samples.
    pub latency: HashMap<String, usize>,
    /// Equal for two compiles exactly when only values differ.
    pub structure: u64,
}

#[derive(Default)]
pub struct Compiler {
    ids: HashMap<String, NodeId>,
    /// The signal and tap the analyser (spectrum, goniometer) reads, if any.
    pub analysis: Option<(String, Tap)>,
    /// The gain of each AFV strip's AFV node (1 open, its depth closed).
    pub afv: HashMap<String, f32>,
    /// Who is soloed, and how (the control room's runtime state).
    pub solo: std::collections::BTreeMap<String, SoloKind>,
    /// The stream slot of each monitor (by id) someone listens to.
    pub streams: HashMap<String, usize>,
    /// The personal monitors of browser clients (runtime).
    pub personal: Vec<Personal>,
    /// Who talks to what (talkback, runtime).
    pub talking: Talking,
    /// The engine's talkback slot of each browser microphone (runtime).
    pub browser_mics: std::collections::BTreeMap<String, usize>,
    /// The engine input index of talkback slot 0 (after the MXL inputs).
    pub talk_base: usize,
    /// What the monitors that follow solo play instead of their source
    /// (listen-back, runtime).
    pub listen_back: Option<Source>,
}

/// A talkback mic with where the engine reads it (input index, channel).
struct MicSource {
    info: MicInfo,
    input: u32,
    channel: u32,
}

impl Compiler {
    fn mic_sources(&self, mix: &Mix) -> Vec<MicSource> {
        let t = &mix.control_room.talkback;
        let configured = t.mics.iter().map(|m| MicSource {
            info: MicInfo {
                id: m.id.clone(),
                priority: m.priority,
                level_db: m.level_db,
                dims_control_room: m.dims_control_room,
            },
            input: (m.input - 1) as u32,
            channel: (m.channel - 1) as u32,
        });
        let browser = self.browser_mics.iter().map(|(id, slot)| MicSource {
            info: MicInfo {
                id: id.clone(),
                priority: t.browser_priority,
                level_db: 0.0,
                dims_control_room: false,
            },
            input: (self.talk_base + slot) as u32,
            channel: 0,
        });
        configured.chain(browser).collect()
    }
}

/// A browser client's own monitor path (§10.1 personal monitoring): stereo, its
/// own source and its own solo.
#[derive(Debug, Clone, PartialEq)]
pub struct Personal {
    pub monitor: Monitor,
    pub solo: std::collections::BTreeMap<String, SoloKind>,
    pub created: std::time::Instant,
}

/// A stereo solo bus of `soloed` (PFL from the pre-fader tap with `pfl_trim_db`,
/// AFL from the output tap); none without solos. Node keys start with `key`.
fn build_solo_bus(
    c: &mut Build,
    mix: &Mix,
    taps: &HashMap<String, Taps>,
    soloed: &std::collections::BTreeMap<String, SoloKind>,
    pfl_trim_db: f64,
    key: &str,
) -> Option<Span> {
    if soloed.is_empty() {
        return None;
    }
    let soloed = &expand_solo(mix, soloed);
    let laws = mix.settings.laws();
    let bus = c.b.buffers(2);
    c.b.step(Step::Clear { dst: bus });
    for (id, kind) in soloed {
        let tap = match kind {
            SoloKind::Pfl => Tap::PreFader,
            SoloKind::Afl => mix.default_tap(id),
        };
        let (Some(t), Some(from)) = (taps.get(id), mix.tap_format(id, &tap)) else {
            continue;
        };
        let src = t.get(&tap).0;
        let coefficients = c.b.coefficients(&width_matrix(from, Format::Stereo, laws));
        let gain = match kind {
            SoloKind::Pfl => Db(pfl_trim_db).gain() as f32,
            SoloKind::Afl => 1.0,
        };
        let node = c.node(
            format!("{key}/{id}"),
            (
                format_name(from),
                *kind,
                f64::from(laws.pan).to_bits(),
                f64::from(laws.mono_sum).to_bits(),
                t.chain,
            ),
            vec![gain],
        );
        c.b.step(Step::Send {
            node,
            src,
            dst: bus,
            coefficients,
        });
    }
    Some(bus)
}

/// Solos with every soloed VCA replaced by its members (a member's own solo wins).
fn expand_solo(
    mix: &Mix,
    soloed: &std::collections::BTreeMap<String, SoloKind>,
) -> std::collections::BTreeMap<String, SoloKind> {
    let mut out: std::collections::BTreeMap<String, SoloKind> = soloed
        .iter()
        .filter(|(id, _)| mix.vca(id).is_none())
        .map(|(id, k)| (id.clone(), *k))
        .collect();
    for (id, kind) in soloed.iter().filter(|(id, _)| mix.vca(id).is_some()) {
        for m in mix.vca_members(id) {
            out.entry(m).or_insert(*kind);
        }
    }
    out
}

impl Compiler {
    /// A compiler whose talkback slots start at engine input `talk_base`.
    pub fn with_talk_base(talk_base: usize) -> Self {
        Self {
            talk_base,
            ..Self::default()
        }
    }

    /// The node id of `key` (e.g. `strip/vox/afv`), once it has been compiled.
    pub fn node_id(&self, key: &str) -> Option<NodeId> {
        self.ids.get(key).copied()
    }
}

/// A signal's tap points and their latencies.
#[derive(Clone)]
struct Taps {
    input: Span,
    chain_in: Span,
    after: Vec<(String, Span, usize)>,
    pre_eq: (Span, usize),
    post_eq: (Span, usize),
    pre_dynamics: (Span, usize),
    post_dynamics: (Span, usize),
    pre_fader: (Span, usize),
    post: Span,
    pan: Span,
    /// Hash of the chain's structure: a change rewires the signal's outputs.
    chain: u64,
}

impl Taps {
    fn get(&self, tap: &Tap) -> (Span, usize) {
        let total = self.pre_fader.1;
        match tap {
            Tap::Input => (self.input, 0),
            Tap::PreEq => self.pre_eq,
            Tap::PostEq => self.post_eq,
            Tap::PreDynamics => self.pre_dynamics,
            Tap::PostDynamics => self.post_dynamics,
            Tap::PreFader => self.pre_fader,
            Tap::PostFader => (self.post, total),
            Tap::PostPan => (self.pan, total),
            Tap::After(id) => self
                .after
                .iter()
                .find(|(n, _, _)| n == id)
                .map(|(_, s, l)| (*s, *l))
                .unwrap_or((self.chain_in, 0)),
        }
    }
}

fn hash(value: impl Hash) -> u64 {
    let mut h = DefaultHasher::new();
    value.hash(&mut h);
    h.finish()
}

fn format_name(f: Format) -> &'static str {
    match f {
        Format::Mono => "mono",
        Format::Stereo => "stereo",
        Format::Lcr => "3.0",
        Format::Surround51 => "5.1",
        Format::Surround71 => "7.1",
        Format::Surround514 => "5.1.4",
        Format::Surround714 => "7.1.4",
    }
}

struct Build<'a> {
    b: PlanBuilder,
    compiler: &'a mut Compiler,
    ramp: u32,
    nodes: HashMap<NodeId, LayoutNode>,
    procs: HashMap<NodeId, LayoutNode>,
    node_meters: Vec<(String, String, usize)>,
    structure: DefaultHasher,
}

impl Build<'_> {
    fn id(&mut self, key: &str) -> NodeId {
        let next = self.compiler.ids.len() as NodeId;
        *self.compiler.ids.entry(key.to_string()).or_insert(next)
    }

    fn node(&mut self, key: String, signature: impl Hash, values: Vec<f32>) -> u32 {
        let id = self.id(&key);
        let signature = hash((&key, signature));
        let index = self.b.node(id, &values, self.ramp);
        signature.hash(&mut self.structure);
        self.nodes.insert(
            id,
            LayoutNode {
                index,
                signature,
                values,
            },
        );
        index
    }

    /// Add a processor of `kind` keyed `key`; returns its index.
    fn processor(
        &mut self,
        key: String,
        kind: &ProcKind,
        values: Vec<f32>,
        channels: usize,
        meter: usize,
    ) -> u32 {
        let id = self.id(&key);
        let signature = hash((&key, kind.signature(), channels));
        signature.hash(&mut self.structure);
        let block = self.b.block();
        let index = self
            .b
            .processor(Processor::new(id, kind, &values, channels, block, meter));
        self.procs.insert(
            id,
            LayoutNode {
                index,
                signature,
                values,
            },
        );
        index
    }

    fn state(
        &mut self,
        key: String,
        channels: usize,
        make: impl FnOnce(NodeId) -> Processor,
    ) -> u32 {
        let id = self.id(&key);
        let signature = hash((&key, channels));
        signature.hash(&mut self.structure);
        let index = self.b.processor(make(id));
        self.procs.insert(
            id,
            LayoutNode {
                index,
                signature,
                values: Vec::new(),
            },
        );
        index
    }

    fn single(&self, span: Span, i: usize) -> Span {
        Span {
            start: span.start + i as u32,
            len: 1,
        }
    }

    /// Emit a signal's chain on `input` (`n` channels); returns the taps it
    /// defines: (after each node, the named taps, the output with its latency).
    #[allow(clippy::type_complexity)]
    fn chain(
        &mut self,
        signal: &str,
        chain: &[Proc],
        input: Span,
        n: usize,
        taps: &HashMap<String, Taps>,
    ) -> (
        Vec<(String, Span, usize)>,
        [(Span, usize); 4],
        (Span, usize),
    ) {
        let mut cur = input;
        let mut latency = 0;
        let mut after = Vec::new();
        for p in chain {
            let key = p
                .kind
                .key()
                .and_then(|k| taps.get(&k.signal).map(|t| t.get(&k.tap).0))
                .unwrap_or_default();
            let meter = self.b.node_meter();
            self.node_meters
                .push((signal.to_string(), p.id.clone(), meter));
            let proc = self.processor(
                format!("proc/{signal}/{}", p.id),
                &p.kind,
                p.values(),
                n,
                meter,
            );
            let dst = self.b.buffers(n);
            self.b.step(Step::Process {
                proc,
                src: cur,
                dst,
                key,
            });
            latency += p.kind.latency();
            after.push((p.id.clone(), dst, latency));
            cur = dst;
        }
        let at = |pos: usize| -> (Span, usize) {
            if pos == 0 {
                (input, 0)
            } else {
                (after[pos - 1].1, after[pos - 1].2)
            }
        };
        let (fe, le) = Mix::category_span(chain, Category::Eq);
        let (fd, ld) = Mix::category_span(chain, Category::Dynamics);
        let named = [at(fe), at(le), at(fd), at(ld)];
        (after, named, (cur, latency))
    }
}

fn chain_hash(chain: &[Proc]) -> u64 {
    hash(
        chain
            .iter()
            .map(|p| (&p.id, p.kind.signature()))
            .collect::<Vec<_>>(),
    )
}

impl Compiler {
    /// Compile `mix`. `outputs` and `loudness` map a signal to its output and
    /// loudness meter slot; signals without one are left out. New nodes ramp in
    /// over `ramp` samples.
    pub fn compile(
        &mut self,
        mix: &Mix,
        block: usize,
        ramp: u32,
        outputs: &HashMap<String, usize>,
        loudness: &HashMap<String, usize>,
    ) -> Compiled {
        let laws = mix.settings.laws();
        let mut c = Build {
            b: PlanBuilder::new(block),
            compiler: self,
            ramp,
            nodes: HashMap::new(),
            procs: HashMap::new(),
            node_meters: Vec::new(),
            structure: DefaultHasher::new(),
        };
        let mut taps: HashMap<String, Taps> = HashMap::new();
        let mut meters = Vec::new();
        let mut latency = HashMap::new();

        // Talkback mics: one mono buffer each, metered as `talkback:<id>`.
        let mics = c.compiler.mic_sources(mix);
        let talking = c.compiler.talking.clone();
        let mut mic_bufs: HashMap<String, Span> = HashMap::new();
        for m in &mics {
            let dst = c.b.buffers(1);
            let buffer = c.b.buffer(dst, 0);
            c.b.step(Step::Patch {
                input: m.input,
                channel: m.channel,
                dst: buffer,
            });
            (&m.info.id, m.input, m.channel).hash(&mut c.structure);
            let key = format!("talkback:{}", m.info.id);
            let slot = c.b.meter(1);
            c.b.step(Step::Meter { src: dst, slot });
            meters.push((key, slot, 1));
            mic_bufs.insert(m.info.id.clone(), dst);
        }
        let mic_infos: Vec<MicInfo> = mics.iter().map(|m| m.info.clone()).collect();
        // VCAs and mute groups (§9) act on the fader nodes' values only.
        let vca_effects = mix.vca_effects();
        let grouped = mix.group_muted();
        let soloed_signals = expand_solo(mix, &c.compiler.solo);

        // Every send, the mix-minus buses' mirrored ones too (§5).
        let all_sends = mix.all_sends();
        // Block-delay sends read last block's source, held across the block.
        let mut held: HashMap<(String, String), (u32, Span)> = HashMap::new();
        for s in all_sends.iter().filter(|s| s.block_delay) {
            let Some(f) = mix.tap_format(&s.from, &s.tap) else {
                continue;
            };
            let n = f.channels();
            let proc = c.state(format!("hold/{}/{}", s.from, s.to), n, |id| {
                Processor::hold_block(id, n, block)
            });
            let dst = c.b.buffers(n);
            c.b.step(Step::HoldOut { proc, dst });
            held.insert((s.from.clone(), s.to.clone()), (proc, dst));
        }

        let order: Vec<String> = mix
            .signal_order()
            .unwrap_or_default()
            .into_iter()
            .map(str::to_string)
            .collect();
        for id in &order {
            let t = match mix.kind(id) {
                Some(Kind::Strip) => {
                    let Some(s) = mix.strip(id) else { continue };
                    let n = s.format.channels();
                    let input = c.b.buffers(n);
                    let mut resolved = Vec::with_capacity(n);
                    for (ch, point) in s.patch.iter().enumerate() {
                        let at = match point {
                            Some(Patch::Input(p)) => {
                                Some(((p.input - 1) as u32, (p.channel - 1) as u32))
                            }
                            Some(Patch::Talkback(t)) => mics
                                .iter()
                                .find(|m| m.info.id == t.talkback)
                                .map(|m| (m.input, m.channel)),
                            None => None,
                        };
                        resolved.push(at);
                        match at {
                            Some((input_index, channel)) => {
                                let dst = c.b.buffer(input, ch);
                                c.b.step(Step::Patch {
                                    input: input_index,
                                    channel,
                                    dst,
                                });
                            }
                            None => {
                                let dst = c.single(input, ch);
                                c.b.step(Step::Clear { dst });
                            }
                        }
                    }
                    let trim = Db(s.trim_db).gain() as f32;
                    let values = s
                        .polarity
                        .iter()
                        .map(|p| if *p { -trim } else { trim })
                        .collect();
                    let node = c.node(
                        format!("strip/{}/trim", s.id),
                        (format_name(s.format), &resolved),
                        values,
                    );
                    let chain_in = c.b.buffers(n);
                    c.b.step(Step::Gain {
                        node,
                        src: input,
                        dst: chain_in,
                    });
                    let chain_sig = chain_hash(&s.chain);
                    let (after, named, pre) = c.chain(&s.id, &s.chain, chain_in, n, &taps);
                    let sip_muted = mix.control_room.solo.sip
                        && !soloed_signals.is_empty()
                        && !soloed_signals.contains_key(&s.id)
                        && !s.solo_safe;
                    let fader = if sip_muted {
                        0.0
                    } else {
                        mix.fader_gain(&s.id, s.fader_db, s.mute, &vca_effects, &grouped)
                    };
                    let node = c.node(
                        format!("strip/{}/fader", s.id),
                        (format_name(s.format), chain_sig),
                        vec![fader; n],
                    );
                    let mut post = c.b.buffers(n);
                    c.b.step(Step::Gain {
                        node,
                        src: pre.0,
                        dst: post,
                    });
                    if s.afv.is_some_and(|a| a.enabled) {
                        let g = c.compiler.afv.get(&s.id).copied().unwrap_or(0.0);
                        let node = c.node(
                            format!("strip/{}/afv", s.id),
                            (format_name(s.format), chain_sig),
                            vec![g; n],
                        );
                        let after = c.b.buffers(n);
                        c.b.step(Step::Gain {
                            node,
                            src: post,
                            dst: after,
                        });
                        post = after;
                    }
                    let pan_values = match s.format {
                        Format::Mono => {
                            let (l, r) = pan_gains(s.pan, laws.pan);
                            Some(vec![l as f32, r as f32])
                        }
                        Format::Stereo => Some(
                            stereo_matrix(s.width, s.pan)
                                .iter()
                                .map(|v| *v as f32)
                                .collect(),
                        ),
                        _ => None,
                    };
                    let pan = match pan_values {
                        Some(values) => {
                            let node = c.node(
                                format!("strip/{}/pan", s.id),
                                (format_name(s.format), chain_sig),
                                values,
                            );
                            let pan = c.b.buffers(2);
                            c.b.step(Step::Matrix {
                                node,
                                src: post,
                                dst: pan,
                            });
                            pan
                        }
                        None => post,
                    };
                    Taps {
                        input,
                        chain_in,
                        after,
                        pre_eq: named[0],
                        post_eq: named[1],
                        pre_dynamics: named[2],
                        post_dynamics: named[3],
                        pre_fader: pre,
                        post,
                        pan,
                        chain: chain_sig,
                    }
                }
                Some(Kind::Bus) => {
                    let Some(bus) = mix.bus(id) else { continue };
                    let n = bus.format.channels();
                    let sum = c.b.buffers(n);
                    c.b.step(Step::Clear { dst: sum });
                    let into: Vec<_> = all_sends.iter().filter(|s| s.to == bus.id).collect();
                    let source = |s: &super::model::Send| -> Option<(Span, usize, u64)> {
                        if s.block_delay {
                            return held
                                .get(&(s.from.clone(), s.to.clone()))
                                .map(|(_, span)| (*span, 0, 0));
                        }
                        let t = taps.get(&s.from)?;
                        let (span, lat) = t.get(&s.tap);
                        Some((span, lat, t.chain))
                    };
                    let most = into
                        .iter()
                        .filter(|s| !s.block_delay)
                        .filter_map(|s| source(s).map(|x| x.1))
                        .max()
                        .unwrap_or(0);
                    for send in into {
                        let (Some((mut src, lat, src_chain)), Some(from)) =
                            (source(send), mix.tap_format(&send.from, &send.tap))
                        else {
                            continue;
                        };
                        let comp = if bus.compensate && !send.block_delay {
                            most - lat
                        } else {
                            0
                        };
                        if comp > 0 {
                            let kind = ProcKind::Delay {
                                time: comp as f64,
                                unit: super::chain::DelayUnit::Samples,
                                frame_rate: None,
                            };
                            let fc = from.channels();
                            let proc = c.processor(
                                format!("comp/{}/{}", send.from, send.to),
                                &kind,
                                vec![1.0],
                                fc,
                                usize::MAX,
                            );
                            let dst = c.b.buffers(fc);
                            c.b.step(Step::Process {
                                proc,
                                src,
                                dst,
                                key: Span::default(),
                            });
                            src = dst;
                        }
                        let w = width_matrix(from, bus.format, laws);
                        let coefficients = c.b.coefficients(&w);
                        let mut gain = if send.mute {
                            0.0
                        } else {
                            send.gain_db.gain() as f32
                        };
                        if send.phase {
                            gain = -gain;
                        }
                        let node = c.node(
                            format!("send/{}/{}", send.from, send.to),
                            (
                                format_name(from),
                                format_name(bus.format),
                                f64::from(laws.pan).to_bits(),
                                f64::from(laws.mono_sum).to_bits(),
                                &send.tap,
                                send.block_delay,
                                comp,
                                src_chain,
                            ),
                            vec![gain],
                        );
                        c.b.step(Step::Send {
                            node,
                            src,
                            dst: sum,
                            coefficients,
                        });
                    }
                    let chain_sig = chain_hash(&bus.chain);
                    let (after, named, pre) = c.chain(&bus.id, &bus.chain, sum, n, &taps);
                    let pre = (pre.0, pre.1 + most);
                    let shift = |(s, l): (Span, usize)| (s, l + most);
                    let fader =
                        mix.fader_gain(&bus.id, bus.fader_db, bus.mute, &vca_effects, &grouped);
                    let node = c.node(
                        format!("bus/{}/fader", bus.id),
                        (format_name(bus.format), chain_sig),
                        vec![fader; n],
                    );
                    let mut post = c.b.buffers(n);
                    c.b.step(Step::Gain {
                        node,
                        src: pre.0,
                        dst: post,
                    });
                    // Talkback into this bus (§10.2): the programme (down while an
                    // interrupt talks), plus every mic at its gain.
                    let dest = mix
                        .control_room
                        .talkback
                        .destinations
                        .iter()
                        .find(|d| d.bus == bus.id);
                    if let Some(d) = dest {
                        let (programme, gains) = destination_gains(d, &mic_infos, &talking);
                        let out = c.b.buffers(n);
                        let node = c.node(
                            format!("talkback/{}/programme", d.id),
                            (format_name(bus.format), chain_sig),
                            vec![programme; n],
                        );
                        c.b.step(Step::Gain {
                            node,
                            src: post,
                            dst: out,
                        });
                        let coefficients =
                            c.b.coefficients(&width_matrix(Format::Mono, bus.format, laws));
                        for (m, g) in mics.iter().zip(gains) {
                            let node = c.node(
                                format!("talkback/{}/{}", d.id, m.info.id),
                                (
                                    format_name(bus.format),
                                    m.input,
                                    m.channel,
                                    f64::from(laws.pan).to_bits(),
                                ),
                                vec![g],
                            );
                            c.b.step(Step::Send {
                                node,
                                src: mic_bufs[&m.info.id],
                                dst: out,
                                coefficients,
                            });
                        }
                        post = out;
                    }
                    Taps {
                        input: sum,
                        chain_in: sum,
                        after: after
                            .into_iter()
                            .map(|(i, s, l)| (i, s, l + most))
                            .collect(),
                        pre_eq: shift(named[0]),
                        post_eq: shift(named[1]),
                        pre_dynamics: shift(named[2]),
                        post_dynamics: shift(named[3]),
                        pre_fader: pre,
                        post,
                        pan: post,
                        chain: chain_sig,
                    }
                }
                None => continue,
            };
            let (_, format) = mix.signal(id).expect("signal exists");
            let n = format.channels();
            let slot = c.b.meter(n);
            c.b.step(Step::Meter { src: t.post, slot });
            meters.push((id.clone(), slot, n));
            if let Some(&slot) = loudness.get(id) {
                c.b.step(Step::Loudness {
                    src: t.post,
                    slot: slot as u32,
                });
            }
            latency.insert(id.clone(), t.pre_fader.1);
            taps.insert(id.clone(), t);
        }

        // The control room: the solo bus, then every monitor path.
        let room = &mix.control_room;
        let soloed = c.compiler.solo.clone();
        let solo_bus = build_solo_bus(&mut c, mix, &taps, &soloed, room.solo.pfl_trim_db, "solo");
        if let Some(bus) = solo_bus {
            let slot = c.b.meter(2);
            c.b.step(Step::Meter { src: bus, slot });
            meters.push(("solo".into(), slot, 2));
        }
        // Monitor paths: the control room's, then the personal ones (each with its
        // own solo), each with its stream slot if someone listens.
        let listen_back = c.compiler.listen_back.clone();
        let talk_dim = dims_control_room(&mic_infos, &talking);
        let mut jobs: Vec<(Monitor, Option<Span>, Option<usize>)> = room
            .monitors
            .iter()
            .map(|m| {
                let mut m = m.clone();
                if let (Some(s), true) = (&listen_back, m.follows_solo) {
                    m.source = Some(s.clone());
                }
                let slot = c.compiler.streams.get(&m.id).copied();
                (m, solo_bus, slot)
            })
            .collect();
        let control_room_monitors = jobs.len();
        for w in c.compiler.personal.clone() {
            let bus = build_solo_bus(
                &mut c,
                mix,
                &taps,
                &w.solo,
                room.solo.pfl_trim_db,
                &format!("{}/solo", w.monitor.id),
            );
            let slot = c.compiler.streams.get(&w.monitor.id).copied();
            jobs.push((w.monitor, bus, slot));
        }
        let mono_sum = Db(f64::from(laws.mono_sum)).gain() as f32;
        for (j, (m, solo_bus, stream)) in jobs.iter().enumerate() {
            let solo_bus = *solo_bus;
            let talk_dimmed = talk_dim && j < control_room_monitors;
            let n = m.format.channels();
            let key = m.key();
            let mon = c.b.buffers(n);
            c.b.step(Step::Clear { dst: mon });
            let under_solo = solo_bus.is_some() && m.follows_solo;
            let programme = if under_solo {
                room.solo.programme_db.gain() as f32
            } else {
                1.0
            };
            let source = match &m.source {
                Some(Source::Signal(id)) => {
                    let tap = mix.default_tap(id);
                    match (taps.get(id), mix.tap_format(id, &tap)) {
                        (Some(t), Some(from)) => Some((t.get(&tap).0, from, id.clone(), t.chain)),
                        _ => None,
                    }
                }
                Some(Source::Input(i)) => {
                    let pin = c.b.buffers(2);
                    for ch in 0..2 {
                        let dst = c.b.buffer(pin, ch);
                        c.b.step(Step::Patch {
                            input: (*i - 1) as u32,
                            channel: ch as u32,
                            dst,
                        });
                    }
                    Some((pin, Format::Stereo, format!("input {i}"), 0))
                }
                None => None,
            };
            if let Some((src, from, name, chain)) = source {
                let coefficients = c.b.coefficients(&width_matrix(from, m.format, laws));
                let node = c.node(
                    format!("{key}/source"),
                    (
                        format_name(from),
                        format_name(m.format),
                        name,
                        f64::from(laws.pan).to_bits(),
                        f64::from(laws.mono_sum).to_bits(),
                        chain,
                        under_solo,
                    ),
                    vec![programme],
                );
                c.b.step(Step::Send {
                    node,
                    src,
                    dst: mon,
                    coefficients,
                });
            }
            if let (Some(bus), true) = (solo_bus, m.follows_solo) {
                let coefficients =
                    c.b.coefficients(&width_matrix(Format::Stereo, m.format, laws));
                let node = c.node(format!("{key}/solo"), format_name(m.format), vec![1.0]);
                c.b.step(Step::Send {
                    node,
                    src: bus,
                    dst: mon,
                    coefficients,
                });
            }
            let mut cur = mon;
            if n >= 2 {
                let out = c.b.buffers(n);
                let node = c.node(
                    format!("{key}/check"),
                    format_name(m.format),
                    check_matrix(m.check, mono_sum).to_vec(),
                );
                c.b.step(Step::Matrix {
                    node,
                    src: c.b.first(cur, 2),
                    dst: c.b.first(out, 2),
                });
                if n > 2 {
                    let node = c.node(
                        format!("{key}/through"),
                        format_name(m.format),
                        vec![1.0; n - 2],
                    );
                    c.b.step(Step::Gain {
                        node,
                        src: Span {
                            start: cur.start + 2,
                            len: (n - 2) as u32,
                        },
                        dst: Span {
                            start: out.start + 2,
                            len: (n - 2) as u32,
                        },
                    });
                }
                cur = out;
            }
            let level = c.b.buffers(n);
            let node = c.node(
                format!("{key}/level"),
                format_name(m.format),
                vec![monitor_gain(m, talk_dimmed); n],
            );
            c.b.step(Step::Gain {
                node,
                src: cur,
                dst: level,
            });
            let (_, _, out) = c.chain(&key, &m.chain, level, n, &taps);
            cur = out.0;
            if let (Some(b), Some(lfe)) = (m.bass_management.filter(|b| b.enabled), m.lfe()) {
                let kind = ProcKind::BassManagement {
                    crossover_hz: b.crossover_hz,
                    lfe_gain_db: b.lfe_gain_db,
                    lfe,
                };
                let mut values = vec![1.0];
                values.extend(kind.values());
                let proc = c.processor(format!("{key}/bass"), &kind, values, n, usize::MAX);
                let dst = c.b.buffers(n);
                c.b.step(Step::Process {
                    proc,
                    src: cur,
                    dst,
                    key: Span::default(),
                });
                cur = dst;
            }
            if m.delay_ms > 0.0 {
                let kind = ProcKind::Delay {
                    time: m.delay_ms,
                    unit: super::chain::DelayUnit::Ms,
                    frame_rate: None,
                };
                let proc = c.processor(format!("{key}/delay"), &kind, vec![1.0], n, usize::MAX);
                let dst = c.b.buffers(n);
                c.b.step(Step::Process {
                    proc,
                    src: cur,
                    dst,
                    key: Span::default(),
                });
                cur = dst;
            }
            let slot = c.b.meter(n);
            c.b.step(Step::Meter { src: cur, slot });
            meters.push((key.clone(), slot, n));
            if let Some(slot) = stream {
                let src = if m.format == Format::Stereo {
                    cur
                } else {
                    let st = c.b.buffers(2);
                    c.b.step(Step::Clear { dst: st });
                    let coefficients =
                        c.b.coefficients(&width_matrix(m.format, Format::Stereo, laws));
                    let node = c.node(format!("{key}/stream"), format_name(m.format), vec![1.0]);
                    c.b.step(Step::Send {
                        node,
                        src: cur,
                        dst: st,
                        coefficients,
                    });
                    st
                };
                c.b.step(Step::Stream {
                    src,
                    slot: *slot as u32,
                });
                (&m.id, *slot).hash(&mut c.structure);
            }
            if let (true, Some(&slot)) = (m.publish, outputs.get(&key)) {
                c.b.step(Step::Output {
                    src: cur,
                    slot: slot as u32,
                });
            }
        }

        for o in &mix.outputs {
            let (Some(&slot), Some(t)) = (outputs.get(&o.signal), taps.get(&o.signal)) else {
                continue;
            };
            let Some(from) = mix.tap_format(&o.signal, &Tap::PostFader) else {
                continue;
            };
            let to = o.format.unwrap_or(from);
            let out = c.b.buffers(to.channels());
            c.b.step(Step::Clear { dst: out });
            let w = width_matrix(from, to, laws);
            let coefficients = c.b.coefficients(&w);
            let node = c.node(
                format!("output/{}", o.signal),
                (
                    format_name(from),
                    format_name(to),
                    slot,
                    f64::from(laws.pan).to_bits(),
                    f64::from(laws.mono_sum).to_bits(),
                    t.chain,
                ),
                vec![1.0],
            );
            c.b.step(Step::Send {
                node,
                src: t.post,
                dst: out,
                coefficients,
            });
            c.b.step(Step::Output {
                src: out,
                slot: slot as u32,
            });
        }

        for s in all_sends.iter().filter(|s| s.block_delay) {
            let (Some((proc, _)), Some(t)) =
                (held.get(&(s.from.clone(), s.to.clone())), taps.get(&s.from))
            else {
                continue;
            };
            let src = t.get(&s.tap).0;
            c.b.step(Step::HoldIn { proc: *proc, src });
        }

        if let Some((signal, tap)) = c.compiler.analysis.clone() {
            if let Some(t) = taps.get(&signal) {
                let span = t.get(&tap).0;
                let src = c.b.first(span, 2);
                c.b.step(Step::Analyse { src });
                (signal, &tap).hash(&mut c.structure);
            }
        }

        let structure = c.structure.finish();
        Compiled {
            plan: c.b.finish(),
            nodes: c.nodes,
            procs: c.procs,
            meters,
            node_meters: c.node_meters,
            latency,
            structure,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::plan::{InputSource, OutputSink};
    use crate::mix::model::{Bus, Output, PatchPoint, Send, Strip};

    struct Inputs(Vec<Vec<Vec<f32>>>);
    impl InputSource for Inputs {
        fn channel(&self, input: usize, channel: usize) -> Option<&[f32]> {
            self.0.get(input)?.get(channel).map(|c| c.as_slice())
        }
    }
    #[derive(Default)]
    struct Outputs(HashMap<usize, Vec<Vec<f32>>>);
    impl OutputSink for Outputs {
        fn write(&mut self, slot: usize, channel: usize, channels: usize, samples: &[f32]) {
            let out = self
                .0
                .entry(slot)
                .or_insert_with(|| vec![Vec::new(); channels]);
            out[channel].extend_from_slice(samples);
        }
    }

    fn strip(id: &str, format: Format, input: usize) -> Strip {
        let n = format.channels();
        Strip {
            id: id.into(),
            name: id.into(),
            format,
            patch: (0..n)
                .map(|c| {
                    Some(
                        PatchPoint {
                            input,
                            channel: c + 1,
                        }
                        .into(),
                    )
                })
                .collect(),
            trim_db: 0.0,
            polarity: vec![false; n],
            fader_db: Db(0.0),
            mute: false,
            pan: 0.0,
            width: 1.0,
            chain: Vec::new(),
            loudness: false,
            colour: None,
            recall_safe: false,
            solo_safe: false,
            mute_safe: false,
            tally_index: None,
            afv: None,
        }
    }

    fn bus(id: &str, format: Format) -> Bus {
        Bus {
            id: id.into(),
            name: id.into(),
            format,
            fader_db: Db(0.0),
            mute: false,
            chain: Vec::new(),
            loudness: false,
            compensate: true,
            colour: None,
            recall_safe: false,
            solo_safe: false,
            mute_safe: false,
            minus: None,
        }
    }

    fn send(from: &str, to: &str, tap: Tap) -> Send {
        Send {
            from: from.into(),
            to: to.into(),
            tap,
            gain_db: Db(0.0),
            mute: false,
            phase: false,
            block_delay: false,
        }
    }

    fn mix() -> Mix {
        let mut mix = Mix::default();
        mix.strips.push(strip("vox", Format::Mono, 1));
        mix.strips.push(strip("music", Format::Stereo, 2));
        mix.buses.push(Bus {
            name: "Main".into(),
            ..bus("main", Format::Stereo)
        });
        mix.sends.push(send("vox", "main", Tap::PostPan));
        mix.sends.push(send("music", "main", Tap::PostFader));
        mix.outputs.push(Output {
            signal: "main".into(),
            format: None,
            commit_samples: None,
        });
        mix
    }

    fn run(compiled: &mut Compiled, inputs: &Inputs, blocks: usize) -> Outputs {
        let mut out = Outputs::default();
        for k in 0..blocks {
            compiled
                .plan
                .process((k * compiled.plan.block) as u64, inputs, &mut out);
        }
        out
    }

    fn slots() -> HashMap<String, usize> {
        HashMap::from([("main".to_string(), 3)])
    }

    fn compile(compiler: &mut Compiler, mix: &Mix, block: usize) -> Compiled {
        compiler.compile(mix, block, 0, &slots(), &HashMap::new())
    }

    #[test]
    fn strips_into_a_bus_into_an_output() {
        let mut compiler = Compiler::default();
        let mut c = compile(&mut compiler, &mix(), 48);
        let inputs = Inputs(vec![
            vec![vec![0.5; 48]],
            vec![vec![0.1; 48], vec![0.2; 48]],
        ]);
        let out = run(&mut c, &inputs, 1);
        let main = &out.0[&3];
        // vox centred at −3 dB: 0.5·0.7071; music: L 0.1, R 0.2.
        let h = std::f32::consts::FRAC_1_SQRT_2;
        assert!(
            (main[0][0] - (0.5 * h + 0.1)).abs() < 1e-6,
            "{}",
            main[0][0]
        );
        assert!((main[1][0] - (0.5 * h + 0.2)).abs() < 1e-6);
        assert_eq!(c.meters.len(), 3);
        let kinds: Vec<&str> = c.plan.steps.iter().map(|s| s.kind()).collect();
        assert_eq!(kinds.iter().filter(|k| **k == "send").count(), 3);
    }

    #[test]
    fn talkback_into_a_bus_with_interrupt_and_priority() {
        use crate::mix::model::{Patch, TalkbackPoint};
        use crate::mix::talkback::{Destination, Mic, Mode};
        let mut m = Mix::default();
        m.strips.push(strip("vox", Format::Mono, 1));
        m.buses.push(bus("ifb", Format::Mono));
        m.sends.push(send("vox", "ifb", Tap::PostFader));
        m.outputs.push(Output {
            signal: "ifb".into(),
            format: None,
            commit_samples: None,
        });
        // A strip on the browser microphone, into its own bus.
        let mut remote = strip("remote", Format::Mono, 1);
        remote.patch[0] = Some(Patch::Talkback(TalkbackPoint {
            talkback: "browser-1".into(),
        }));
        m.strips.push(remote);
        m.buses.push(bus("rec", Format::Mono));
        m.sends.push(send("remote", "rec", Tap::PostFader));
        m.outputs.push(Output {
            signal: "rec".into(),
            format: None,
            commit_samples: None,
        });
        let mic = |id: &str, channel, priority| Mic {
            id: id.into(),
            name: id.into(),
            input: 2,
            channel,
            level_db: 0.0,
            priority,
            dims_control_room: true,
        };
        let t = &mut m.control_room.talkback;
        t.mics.push(mic("dir", 1, 5));
        t.mics.push(mic("prod", 2, 1));
        t.destinations.push(Destination {
            id: "ifb".into(),
            name: "IFB".into(),
            bus: "ifb".into(),
            mode: Mode::Interrupt,
            interrupt_db: Db::MINUS_INF,
            level_db: 0.0,
            return_feed: None,
        });
        assert_eq!(m.validate(2, 48), Ok(()));
        let slots = HashMap::from([("ifb".to_string(), 3), ("rec".to_string(), 4)]);
        let mut compiler = Compiler::with_talk_base(2);
        compiler.browser_mics.insert("browser-1".into(), 0);
        // vox 0.5, dir 0.3, prod 0.2, the browser 0.05 (talkback slot 0).
        let inputs = Inputs(vec![
            vec![vec![0.5; 48]],
            vec![vec![0.3; 48], vec![0.2; 48]],
            vec![vec![0.05; 48]],
        ]);
        let level = |compiler: &mut Compiler, talking: &[(&str, &str)], mix: &Mix| {
            compiler.talking.clear();
            for (mic, dest) in talking {
                compiler
                    .talking
                    .entry(mic.to_string())
                    .or_default()
                    .insert(dest.to_string());
            }
            let mut c = compiler.compile(mix, 48, 0, &slots, &HashMap::new());
            let out = run(&mut c, &inputs, 1);
            (out.0[&3][0][0], out.0[&4][0][0], c.structure)
        };
        let (idle, rec, structure) = level(&mut compiler, &[], &m);
        assert_eq!((idle, rec), (0.5, 0.05));
        let (dir, _, s) = level(&mut compiler, &[("dir", "ifb")], &m);
        assert_eq!(dir, 0.3, "interrupt: the programme cut");
        assert_eq!(s, structure, "talk is a value change, not a new plan");
        let (both, _, _) = level(&mut compiler, &[("dir", "ifb"), ("prod", "ifb")], &m);
        assert_eq!(both, 0.3, "the director outranks the producer");
        let (prod, _, _) = level(&mut compiler, &[("prod", "ifb")], &m);
        assert_eq!(prod, 0.2);
        let mut mixed = m.clone();
        mixed.control_room.talkback.destinations[0].mode = Mode::Mix;
        let (sum, _, _) = level(&mut compiler, &[("dir", "ifb"), ("prod", "ifb")], &mixed);
        assert!(
            (sum - 1.0).abs() < 1e-6,
            "mix: programme and both mics: {sum}"
        );
    }

    #[test]
    fn vcas_and_mute_groups_change_values_not_the_plan() {
        let mut compiler = Compiler::default();
        let base = mix();
        let inputs = Inputs(vec![
            vec![vec![0.5; 48]],
            vec![vec![0.1; 48], vec![0.2; 48]],
        ]);
        let mut level = |m: &Mix| {
            let mut c = compile(&mut compiler, m, 48);
            let out = run(&mut c, &inputs, 1);
            (out.0[&3][0][0], c.structure)
        };
        let h = std::f32::consts::FRAC_1_SQRT_2;
        let (plain, structure) = level(&base);
        assert!((plain - (0.5 * h + 0.1)).abs() < 1e-6);
        let mut with = base.clone();
        with.vcas.push(crate::mix::vca::Vca {
            id: "talent".into(),
            name: "Talent".into(),
            fader_db: Db(-6.0),
            mute: false,
            members: vec!["vox".into()],
            colour: None,
            recall_safe: false,
        });
        let (vca, s) = level(&with);
        assert!((vca - (0.5 * h * 0.501_187_2 + 0.1)).abs() < 1e-6, "{vca}");
        assert_eq!(s, structure, "a VCA is a value change");
        with.vcas[0].mute = true;
        let (muted, _) = level(&with);
        assert!((muted - 0.1).abs() < 1e-6);
        with.vcas[0].mute = false;
        with.mute_groups.push(crate::mix::vca::MuteGroup {
            id: "playout".into(),
            name: "Playout".into(),
            active: true,
            members: vec!["music".into()],
        });
        let (grouped, s) = level(&with);
        assert!((grouped - 0.5 * h * 0.501_187_2).abs() < 1e-6);
        assert_eq!(s, structure);
        with.strips[1].mute_safe = true;
        let (safe, _) = level(&with);
        assert!((safe - vca).abs() < 1e-6, "mute-safe");
    }

    #[test]
    fn afv_adds_a_gain_after_the_fader() {
        let mut compiler = Compiler::default();
        let base = mix();
        let a = compile(&mut compiler, &base, 48);
        assert!(compiler.node_id("strip/vox/afv").is_none());
        let mut with = base.clone();
        with.strips[0].afv = Some(crate::mix::model::Afv {
            enabled: true,
            depth_db: Db(-20.0),
            fade_in_ms: 100.0,
            fade_out_ms: 500.0,
        });
        compiler.afv.insert("vox".into(), 0.1);
        let b = compile(&mut compiler, &with, 48);
        assert_ne!(a.structure, b.structure);
        let id = compiler.node_id("strip/vox/afv").expect("afv node");
        assert_eq!(b.nodes[&id].values, vec![0.1]);
        let gains = |c: &Compiled| c.plan.steps.iter().filter(|s| s.kind() == "gain").count();
        assert_eq!(gains(&b), gains(&a) + 1);
        // Off again: the node goes.
        with.strips[0].afv.as_mut().unwrap().enabled = false;
        let c = compile(&mut compiler, &with, 48);
        assert_eq!(gains(&c), gains(&a));
    }

    #[test]
    fn parameter_changes_keep_the_structure_and_rewiring_does_not() {
        let mut compiler = Compiler::default();
        let base = mix();
        let a = compile(&mut compiler, &base, 48);
        let mut faded = base.clone();
        faded.strips[0].fader_db = Db(-6.0);
        faded.strips[1].pan = 0.5;
        faded.sends[0].mute = true;
        let b = compile(&mut compiler, &faded, 48);
        assert_eq!(a.structure, b.structure);
        let changed: Vec<_> = b
            .nodes
            .iter()
            .filter(|(id, n)| a.nodes[id].values != n.values)
            .collect();
        assert_eq!(changed.len(), 3);
        assert!(b
            .nodes
            .iter()
            .all(|(id, n)| a.nodes[id].signature == n.signature));

        let mut rewired = base.clone();
        rewired.sends[0].tap = Tap::PreFader;
        let c = compile(&mut compiler, &rewired, 48);
        assert_ne!(a.structure, c.structure);
        let differing = c
            .nodes
            .iter()
            .filter(|(id, n)| a.nodes[id].signature != n.signature)
            .count();
        assert_eq!(differing, 1, "only the send changed its wiring");

        let mut repatched = base;
        repatched.strips[0].patch[0] = Some(
            PatchPoint {
                input: 2,
                channel: 2,
            }
            .into(),
        );
        let d = compile(&mut compiler, &repatched, 48);
        assert_ne!(a.structure, d.structure);
    }

    #[test]
    fn polarity_trim_mute_and_nested_buses() {
        let mut m = mix();
        m.strips[0].polarity = vec![true];
        m.strips[0].trim_db = 6.0206;
        m.strips[1].mute = true;
        m.buses.push(Bus {
            fader_db: Db(-6.0206),
            ..bus("sub", Format::Mono)
        });
        m.sends.push(send("main", "sub", Tap::PostFader));
        m.outputs.push(Output {
            signal: "sub".into(),
            format: Some(Format::Stereo),
            commit_samples: None,
        });
        assert_eq!(m.validate(2, 48), Ok(()));
        let mut compiler = Compiler::default();
        let slots = HashMap::from([("main".to_string(), 0), ("sub".to_string(), 1)]);
        let mut c = compiler.compile(&m, 48, 0, &slots, &HashMap::new());
        let inputs = Inputs(vec![
            vec![vec![0.25; 48]],
            vec![vec![0.5; 48], vec![0.5; 48]],
        ]);
        let out = run(&mut c, &inputs, 1);
        let h = std::f32::consts::FRAC_1_SQRT_2;
        // vox: −0.25 · 2 = −0.5, centred → −0.5·h per side; music muted.
        let l = -0.5 * h;
        assert!((out.0[&0][0][0] - l).abs() < 1e-5, "{}", out.0[&0][0][0]);
        // sub (mono) = (L + R) · −3 dB sum · −6 dB fader; published as stereo (C).
        let sub = 2.0 * l * 0.707_945_8 * 0.5;
        assert!((out.0[&1][0][0] - sub * h).abs() < 1e-5);
        assert_eq!(out.0[&1].len(), 2);
    }

    fn limiter() -> Proc {
        serde_json::from_str(
            r#"{"id": "lim", "type": "limiter", "ceiling_db": -1, "release_ms": 50, "lookahead_ms": 1.5, "true_peak": false}"#,
        )
        .unwrap()
    }

    /// §17: delay compensation keeps parallel paths phase-coherent. One input on
    /// two strips, one with a 72-sample look-ahead limiter (not limiting), summed
    /// with opposite polarity: the bus nulls; without compensation it does not.
    #[test]
    fn delay_compensation_null_test() {
        let mut m = Mix::default();
        m.strips.push(strip("a", Format::Mono, 1));
        m.strips.push(strip("b", Format::Mono, 1));
        m.strips[0].chain.push(limiter());
        m.buses.push(bus("main", Format::Mono));
        m.sends.push(send("a", "main", Tap::PostFader));
        m.sends.push(Send {
            phase: true,
            ..send("b", "main", Tap::PostFader)
        });
        m.outputs.push(Output {
            signal: "main".into(),
            format: None,
            commit_samples: None,
        });
        assert_eq!(m.validate(1, 96), Ok(()));
        let signal: Vec<f32> = (0..96 * 50)
            .map(|k| (k as f32 * 0.01).sin() * 0.3)
            .collect();
        let feed = |c: &mut Compiled| {
            let mut out = Outputs::default();
            for (k, block) in signal.chunks(96).enumerate() {
                let inputs = Inputs(vec![vec![block.to_vec()]]);
                c.plan.process((k * 96) as u64, &inputs, &mut out);
            }
            out.0[&3][0][960..].iter().fold(0f32, |m, v| m.max(v.abs()))
        };
        let mut compiler = Compiler::default();
        let mut c = compile(&mut compiler, &m, 96);
        assert_eq!(c.latency["main"], 72);
        assert!(feed(&mut c) < 1e-6, "compensated paths must null");
        m.buses[0].compensate = false;
        let mut c = compile(&mut compiler, &m, 96);
        assert!(feed(&mut c) > 0.01, "uncompensated paths do not null");
    }

    /// §4.4: a loop closes through an explicit block delay; the signal comes back
    /// one block later at the send's gain.
    #[test]
    fn block_delay_feedback() {
        let mut m = Mix::default();
        m.strips.push(strip("s", Format::Mono, 1));
        m.buses.push(bus("main", Format::Mono));
        m.buses.push(bus("echo", Format::Mono));
        m.sends.push(send("s", "main", Tap::PostFader));
        m.sends.push(send("main", "echo", Tap::PostFader));
        m.sends.push(Send {
            gain_db: Db(-6.0206),
            block_delay: true,
            ..send("echo", "main", Tap::PostFader)
        });
        m.outputs.push(Output {
            signal: "main".into(),
            format: None,
            commit_samples: None,
        });
        assert_eq!(m.validate(1, 48), Ok(()));
        let mut compiler = Compiler::default();
        let mut c = compile(&mut compiler, &m, 48);
        let mut out = Outputs::default();
        for k in 0..4u64 {
            let x = if k == 0 { 1.0 } else { 0.0 };
            c.plan
                .process(k * 48, &Inputs(vec![vec![vec![x; 48]]]), &mut out);
        }
        let main = &out.0[&3][0];
        for (k, expected) in [1.0f32, 0.5, 0.25, 0.125].iter().enumerate() {
            assert!(
                (main[k * 48 + 10] - expected).abs() < 1e-4,
                "block {k}: {}",
                main[k * 48 + 10]
            );
        }
    }

    #[test]
    fn chain_taps_and_keys() {
        let mut m = mix();
        // vox → +6 dB in a dynamic-free chain (a delay node, then a compressor that
        // never acts); music ducked by vox.
        m.strips[0].chain = serde_json::from_str(
            r#"[{"id": "d", "type": "delay", "time": 10, "unit": "samples"},
                {"id": "c", "type": "compressor", "threshold_db": 0, "ratio": 2, "knee_db": 0, "attack_ms": 1, "release_ms": 50, "makeup_db": 6.0206, "detector": "peak"}]"#,
        )
        .unwrap();
        m.strips[1].chain = serde_json::from_str(
            r#"[{"id": "duck", "type": "ducker", "key": {"signal": "vox", "tap": "post-fader"}, "threshold_db": -40, "depth_db": 20, "attack_ms": 0, "hold_ms": 0, "release_ms": 0}]"#,
        )
        .unwrap();
        m.buses.push(bus("pre", Format::Mono));
        m.sends.push(send("vox", "pre", Tap::After("d".into())));
        m.outputs.push(Output {
            signal: "pre".into(),
            format: None,
            commit_samples: None,
        });
        assert_eq!(m.validate(2, 48), Ok(()));
        let mut compiler = Compiler::default();
        let slots = HashMap::from([("main".to_string(), 0), ("pre".to_string(), 1)]);
        let mut c = compiler.compile(&m, 48, 0, &slots, &HashMap::new());
        assert_eq!(c.node_meters.len(), 3);
        let inputs = Inputs(vec![
            vec![vec![0.25; 48]],
            vec![vec![0.1; 48], vec![0.1; 48]],
        ]);
        let out = run(&mut c, &inputs, 3);
        let h = std::f32::consts::FRAC_1_SQRT_2;
        // pre: vox after the delay node (10 samples late), before the +6 dB compressor.
        assert_eq!(out.0[&1][0][5], 0.0);
        assert!((out.0[&1][0][100] - 0.25).abs() < 1e-6);
        // main: vox × 2 centred, music −20 dB (ducked).
        assert!(
            (out.0[&0][0][120] - (0.5 * h + 0.01)).abs() < 1e-4,
            "{}",
            out.0[&0][0][120]
        );
    }
}
