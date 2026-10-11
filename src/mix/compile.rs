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
use super::model::{Db, Kind, Mix, Tap};
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

        // Block-delay sends read last block's source, held across the block.
        let mut held: HashMap<(String, String), (u32, Span)> = HashMap::new();
        for s in mix.sends.iter().filter(|s| s.block_delay) {
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
                    for (ch, point) in s.patch.iter().enumerate() {
                        match point {
                            Some(p) => {
                                let dst = c.b.buffer(input, ch);
                                c.b.step(Step::Patch {
                                    input: (p.input - 1) as u32,
                                    channel: (p.channel - 1) as u32,
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
                        (format_name(s.format), &s.patch),
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
                    let fader = if s.mute {
                        0.0
                    } else {
                        s.fader_db.gain() as f32
                    };
                    let node = c.node(
                        format!("strip/{}/fader", s.id),
                        (format_name(s.format), chain_sig),
                        vec![fader; n],
                    );
                    let post = c.b.buffers(n);
                    c.b.step(Step::Gain {
                        node,
                        src: pre.0,
                        dst: post,
                    });
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
                    let into: Vec<_> = mix.sends.iter().filter(|s| s.to == bus.id).collect();
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
                    let fader = if bus.mute {
                        0.0
                    } else {
                        bus.fader_db.gain() as f32
                    };
                    let node = c.node(
                        format!("bus/{}/fader", bus.id),
                        (format_name(bus.format), chain_sig),
                        vec![fader; n],
                    );
                    let post = c.b.buffers(n);
                    c.b.step(Step::Gain {
                        node,
                        src: pre.0,
                        dst: post,
                    });
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
            let tp = c.state(format!("meter/{id}"), n, |pid| Processor::true_peak(pid, n));
            let slot = c.b.meter(n);
            c.b.step(Step::Meter {
                src: t.post,
                slot,
                proc: tp,
            });
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

        for s in mix.sends.iter().filter(|s| s.block_delay) {
            let (Some((proc, _)), Some(t)) =
                (held.get(&(s.from.clone(), s.to.clone())), taps.get(&s.from))
            else {
                continue;
            };
            let src = t.get(&s.tap).0;
            c.b.step(Step::HoldIn { proc: *proc, src });
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
                    Some(PatchPoint {
                        input,
                        channel: c + 1,
                    })
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
        repatched.strips[0].patch[0] = Some(PatchPoint {
            input: 2,
            channel: 2,
        });
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
