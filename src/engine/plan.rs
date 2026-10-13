//! Execution plans (§2) and their executor.
//!
//! A plan is a list of steps over numbered block buffers, compiled on the control
//! side and swapped in at a block boundary. Its nodes hold the smoothed parameter
//! values (gains, panner matrices); a node that exists in the old and the new plan
//! with the same wiring keeps its current values (`carry`), a new node starts at
//! zero and ramps to its target, so a swap neither clicks nor disturbs unrelated
//! paths. Parameter messages address nodes by a stable [`NodeId`].

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

pub use super::proc::{NodeMeter, Processor, MAX_CH};

/// Values per node: the widest format's channels, or a 2 × 2 panner.
pub const MAX_NODE_VALUES: usize = 12;

/// Stable id of a node across plans (assigned by the compiler per node key).
pub type NodeId = u32;

/// A run of entries in `Plan::indices` or `Plan::coefficients`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub start: u32,
    pub len: u32,
}

impl Span {
    fn range(self) -> std::ops::Range<usize> {
        self.start as usize..(self.start + self.len) as usize
    }
}

/// Smoothed values of one node.
#[derive(Debug, Clone)]
pub struct Node {
    pub id: NodeId,
    len: usize,
    cur: [f32; MAX_NODE_VALUES],
    target: [f32; MAX_NODE_VALUES],
    step: [f32; MAX_NODE_VALUES],
    left: u32,
}

impl Node {
    /// A node at zero, ramping to `values` over `ramp` samples.
    pub fn new(id: NodeId, values: &[f32], ramp: u32) -> Self {
        let mut node = Self {
            id,
            len: values.len().min(MAX_NODE_VALUES),
            cur: [0.0; MAX_NODE_VALUES],
            target: [0.0; MAX_NODE_VALUES],
            step: [0.0; MAX_NODE_VALUES],
            left: 0,
        };
        node.set(values, ramp);
        node
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn target(&self) -> &[f32] {
        &self.target[..self.len]
    }

    pub fn current(&self) -> &[f32] {
        &self.cur[..self.len]
    }

    /// Ramp linearly to `values` over `ramp` samples (0: at once).
    pub fn set(&mut self, values: &[f32], ramp: u32) {
        let n = self.len.min(values.len());
        self.target[..n].copy_from_slice(&values[..n]);
        if ramp == 0 {
            self.cur = self.target;
            self.left = 0;
            return;
        }
        for i in 0..self.len {
            self.step[i] = (self.target[i] - self.cur[i]) / ramp as f32;
        }
        self.left = ramp;
    }

    /// Value `i` at sample `k` of the coming block.
    #[inline]
    fn at(&self, i: usize, k: usize) -> f32 {
        if (k as u32) < self.left {
            self.cur[i] + self.step[i] * (k + 1) as f32
        } else {
            self.target[i]
        }
    }

    fn ramping(&self) -> bool {
        self.left > 0
    }

    fn finish_block(&mut self, block: usize) {
        if self.left == 0 {
            return;
        }
        let n = (block as u32).min(self.left);
        self.left -= n;
        if self.left == 0 {
            self.cur = self.target;
        } else {
            for i in 0..self.len {
                self.cur[i] += self.step[i] * n as f32;
            }
        }
    }
}

/// One step of a plan. Buffer lists are spans of `Plan::indices`.
#[derive(Debug, Clone, Copy)]
pub enum Step {
    /// `dst` ← channel `channel` of input `input` (silence when it has none).
    Patch { input: u32, channel: u32, dst: u32 },
    /// `dst[c]` ← `src[c]` × node value `c`.
    Gain { node: u32, src: Span, dst: Span },
    /// `dst[d]` ← Σ `src[s]` × node value `d · src.len + s` (a panner).
    Matrix { node: u32, src: Span, dst: Span },
    /// `dst[d]` ← 0.
    Clear { dst: Span },
    /// `dst[d]` += node value 0 × Σ `src[s]` × coefficient `d · src.len + s`
    /// (a send with its width mapping).
    Send {
        node: u32,
        src: Span,
        dst: Span,
        coefficients: Span,
    },
    /// `src` to the meters (peak, true peak, energy, PPM, correlation of meter
    /// channels `slot …`), measured on the meter thread (`engine::meter`).
    Meter { src: Span, slot: u32 },
    /// `src` to output slot `slot`.
    Output { src: Span, slot: u32 },
    /// `dst` ← processor `proc` applied to `src`, keyed by `key` (empty: none).
    Process {
        proc: u32,
        src: Span,
        dst: Span,
        key: Span,
    },
    /// `dst` ← the block processor `proc` (a hold) took last time.
    HoldOut { proc: u32, dst: Span },
    /// Processor `proc` (a hold) takes `src` for the next block.
    HoldIn { proc: u32, src: Span },
    /// `src` to loudness meter `slot`.
    Loudness { src: Span, slot: u32 },
    /// The first two channels of `src` to the analyser (spectrum, goniometer).
    Analyse { src: Span },
    /// A stereo `src` to stream slot `slot` (a monitor heard in the browser).
    Stream { src: Span, slot: u32 },
}

impl Step {
    pub fn kind(&self) -> &'static str {
        match self {
            Step::Patch { .. } => "patch",
            Step::Gain { .. } => "gain",
            Step::Matrix { .. } => "pan",
            Step::Clear { .. } => "clear",
            Step::Send { .. } => "send",
            Step::Meter { .. } => "meter",
            Step::Output { .. } => "output",
            Step::Process { .. } => "process",
            Step::HoldOut { .. } | Step::HoldIn { .. } => "hold",
            Step::Loudness { .. } => "loudness",
            Step::Analyse { .. } => "analyse",
            Step::Stream { .. } => "stream",
        }
    }
}

/// One meter channel, written by the DSP thread only.
#[derive(Default)]
pub struct MeterChannel {
    /// Largest |sample| since the control side last took it (f32 bits).
    pub peak: AtomicU32,
    /// Running sum of squares (f64 bits) and its sample count.
    pub energy: AtomicU64,
    pub samples: AtomicU64,
    /// Largest true peak since the control side last took it (f32 bits).
    pub true_peak: AtomicU32,
    /// First channel of a signal with two or more: running sum of channel 1 ×
    /// channel 2 (f64 bits), for the phase correlation.
    pub cross: AtomicU64,
    /// PPM detectors (Type I, Type II): their level (f32 bits, the DSP thread's
    /// state) and the largest level since the control side last took it.
    pub ppm: [AtomicU32; 2],
    pub ppm_max: [AtomicU32; 2],
}

pub struct MeterBank {
    pub channels: Vec<MeterChannel>,
    pub ppm: [crate::dsp::ppm::Ballistics; 2],
}

impl MeterBank {
    pub fn new(channels: usize) -> Self {
        use crate::dsp::ppm::Ballistics;
        Self {
            channels: (0..channels).map(|_| MeterChannel::default()).collect(),
            ppm: [Ballistics::type_i(), Ballistics::type_ii()],
        }
    }
}

/// A compiled plan.
pub struct Plan {
    pub block: usize,
    /// Unique per plan: the meter thread finds its meter bank by it.
    pub generation: u32,
    buffers: Vec<f32>,
    indices: Vec<u32>,
    coefficients: Vec<f32>,
    pub steps: Vec<Step>,
    pub nodes: Vec<Node>,
    /// `(id, node index)` sorted by id.
    lookup: Vec<(NodeId, u32)>,
    /// `(new node, old node)`: values carried over from the previous plan.
    pub carry: Vec<(u32, u32)>,
    pub meters: Arc<MeterBank>,
    pub procs: Vec<Processor>,
    /// `(id, processor index)` sorted by id.
    proc_lookup: Vec<(NodeId, u32)>,
    /// `(new processor, old processor)`: state carried over from the previous plan.
    pub proc_carry: Vec<(u32, u32)>,
    pub node_meters: Arc<[NodeMeter]>,
}

/// Builds a plan on the control side.
pub struct PlanBuilder {
    plan: Plan,
    buffers: u32,
    meters: u32,
    node_meters: usize,
}

impl PlanBuilder {
    pub fn new(block: usize) -> Self {
        Self {
            plan: Plan {
                block,
                generation: 0,
                buffers: Vec::new(),
                indices: Vec::new(),
                coefficients: Vec::new(),
                steps: Vec::new(),
                nodes: Vec::new(),
                lookup: Vec::new(),
                carry: Vec::new(),
                meters: Arc::new(MeterBank::new(0)),
                procs: Vec::new(),
                proc_lookup: Vec::new(),
                proc_carry: Vec::new(),
                node_meters: Arc::new([]),
            },
            buffers: 0,
            meters: 0,
            node_meters: 0,
        }
    }

    /// Add a processor; returns its index.
    pub fn processor(&mut self, p: Processor) -> u32 {
        self.plan.procs.push(p);
        (self.plan.procs.len() - 1) as u32
    }

    /// A node meter slot.
    pub fn node_meter(&mut self) -> usize {
        self.node_meters += 1;
        self.node_meters - 1
    }

    pub fn block(&self) -> usize {
        self.plan.block
    }

    /// `n` new buffers.
    pub fn buffers(&mut self, n: usize) -> Span {
        let start = self.plan.indices.len() as u32;
        for _ in 0..n {
            self.plan.indices.push(self.buffers);
            self.buffers += 1;
        }
        Span {
            start,
            len: n as u32,
        }
    }

    /// The first `n` buffers of `span`.
    pub fn first(&self, span: Span, n: usize) -> Span {
        Span {
            start: span.start,
            len: (n as u32).min(span.len),
        }
    }

    /// A buffer id of `span`.
    pub fn buffer(&self, span: Span, i: usize) -> u32 {
        self.plan.indices[span.start as usize + i]
    }

    pub fn coefficients(&mut self, values: &[f32]) -> Span {
        let start = self.plan.coefficients.len() as u32;
        self.plan.coefficients.extend_from_slice(values);
        Span {
            start,
            len: values.len() as u32,
        }
    }

    /// A node with `values`, starting at zero (the swap carries values over).
    pub fn node(&mut self, id: NodeId, values: &[f32], ramp: u32) -> u32 {
        self.plan.nodes.push(Node::new(id, values, ramp));
        (self.plan.nodes.len() - 1) as u32
    }

    /// `n` meter channels.
    pub fn meter(&mut self, n: usize) -> u32 {
        let slot = self.meters;
        self.meters += n as u32;
        slot
    }

    pub fn step(&mut self, step: Step) {
        self.plan.steps.push(step);
    }

    pub fn finish(mut self) -> Plan {
        static GENERATIONS: AtomicU32 = AtomicU32::new(1);
        self.plan.generation = GENERATIONS.fetch_add(1, Ordering::Relaxed);
        self.plan.buffers = vec![0.0; self.buffers as usize * self.plan.block];
        self.plan.meters = Arc::new(MeterBank::new(self.meters as usize));
        self.plan.lookup = self
            .plan
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (n.id, i as u32))
            .collect();
        self.plan.lookup.sort_unstable();
        self.plan.proc_lookup = self
            .plan
            .procs
            .iter()
            .enumerate()
            .map(|(i, p)| (p.id, i as u32))
            .collect();
        self.plan.proc_lookup.sort_unstable();
        self.plan.node_meters = (0..self.node_meters)
            .map(|_| NodeMeter::default())
            .collect();
        self.plan
    }
}

/// What the executor reads inputs from.
pub trait InputSource {
    /// Channel `channel` of input `input` for this block, if it has one.
    fn channel(&self, input: usize, channel: usize) -> Option<&[f32]>;
}

/// Where the executor writes outputs and loudness meters.
pub trait OutputSink {
    /// Channel `channel` of `channels` for output slot `slot`.
    fn write(&mut self, slot: usize, channel: usize, channels: usize, samples: &[f32]);

    /// Channel `channel` of the block starting at sample `start` for loudness
    /// meter `slot`.
    fn loudness(&mut self, slot: usize, start: u64, channel: usize, samples: &[f32]) {
        let _ = (slot, start, channel, samples);
    }

    /// Channel `channel` (0 or 1) of `channels` (1 or 2) for the analyser.
    fn analyse(&mut self, channel: usize, channels: usize, samples: &[f32]) {
        let _ = (channel, channels, samples);
    }

    /// Channel `channel` (0 or 1) of stream slot `slot`.
    fn stream(&mut self, slot: usize, channel: usize, samples: &[f32]) {
        let _ = (slot, channel, samples);
    }

    /// A block of meter channels `slot …` of plan `generation`.
    fn meter(&mut self, generation: u32, slot: usize, chans: &[&[f32]]) {
        let _ = (generation, slot, chans);
    }
}

impl Plan {
    pub fn node_index(&self, id: NodeId) -> Option<usize> {
        self.lookup
            .binary_search_by_key(&id, |(i, _)| *i)
            .ok()
            .map(|k| self.lookup[k].1 as usize)
    }

    /// Apply a parameter message; `false` when this plan has no such node.
    pub fn set_param(&mut self, id: NodeId, values: &[f32], ramp: u32) -> bool {
        match self.node_index(id) {
            Some(i) => {
                self.nodes[i].set(values, ramp);
                true
            }
            None => false,
        }
    }

    /// Apply a processor parameter message; `false` when there is no such node.
    pub fn set_proc_param(&mut self, id: NodeId, index: usize, value: f32, ramp: u32) -> bool {
        match self.proc_lookup.binary_search_by_key(&id, |(i, _)| *i) {
            Ok(k) => {
                let i = self.proc_lookup[k].1 as usize;
                self.procs[i].set_param(index, value, ramp);
                true
            }
            Err(_) => false,
        }
    }

    /// Take over the current values of the nodes listed in `carry` and the state
    /// of the processors in `proc_carry` from `old`; values then ramp to this
    /// plan's targets over `ramp` samples.
    pub fn adopt(&mut self, old: &mut Plan, ramp: u32) {
        for &(new, prev) in &self.proc_carry {
            if let (Some(n), Some(o)) = (
                self.procs.get_mut(new as usize),
                old.procs.get_mut(prev as usize),
            ) {
                n.adopt(o, ramp);
            }
        }
        for &(new, prev) in &self.carry {
            let (Some(n), Some(o)) = (
                self.nodes.get_mut(new as usize),
                old.nodes.get(prev as usize),
            ) else {
                continue;
            };
            if n.len != o.len {
                continue;
            }
            n.cur = o.cur;
            let target = n.target;
            n.set(&target[..n.len], ramp);
        }
    }

    fn buf(&self, id: u32) -> std::ops::Range<usize> {
        let b = self.block;
        id as usize * b..(id as usize + 1) * b
    }

    /// Two different buffers, one to read and one to write.
    fn pair(&mut self, src: u32, dst: u32) -> (&[f32], &mut [f32]) {
        debug_assert_ne!(src, dst);
        let b = self.block;
        let (s, d) = (src as usize * b, dst as usize * b);
        if s < d {
            let (lo, hi) = self.buffers.split_at_mut(d);
            (&lo[s..s + b], &mut hi[..b])
        } else {
            let (lo, hi) = self.buffers.split_at_mut(s);
            (&hi[..b], &mut lo[d..d + b])
        }
    }

    /// Run every step for one block. Never allocates.
    pub fn process(&mut self, start: u64, inputs: &dyn InputSource, outputs: &mut dyn OutputSink) {
        let block = self.block;
        for i in 0..self.steps.len() {
            match self.steps[i] {
                Step::Patch {
                    input,
                    channel,
                    dst,
                } => {
                    let range = self.buf(dst);
                    match inputs.channel(input as usize, channel as usize) {
                        Some(samples) if samples.len() >= block => {
                            self.buffers[range].copy_from_slice(&samples[..block])
                        }
                        _ => self.buffers[range].fill(0.0),
                    }
                }
                Step::Gain { node, src, dst } => {
                    for c in 0..dst.len.min(src.len) as usize {
                        let (s, d) = (
                            self.indices[src.start as usize + c],
                            self.indices[dst.start as usize + c],
                        );
                        let n = &self.nodes[node as usize];
                        let ramping = n.ramping();
                        let g = n.target[c];
                        let node = n.clone();
                        let (s, d) = self.pair(s, d);
                        if ramping {
                            for k in 0..block {
                                d[k] = s[k] * node.at(c, k);
                            }
                        } else {
                            for k in 0..block {
                                d[k] = s[k] * g;
                            }
                        }
                    }
                    self.nodes[node as usize].finish_block(block);
                }
                Step::Matrix { node, src, dst } => {
                    let columns = src.len as usize;
                    for d in 0..dst.len as usize {
                        let out = self.indices[dst.start as usize + d];
                        let range = self.buf(out);
                        self.buffers[range].fill(0.0);
                        for s in 0..columns {
                            let from = self.indices[src.start as usize + s];
                            let n = &self.nodes[node as usize];
                            let v = d * columns + s;
                            let ramping = n.ramping();
                            let g = n.target[v];
                            if !ramping && g == 0.0 {
                                continue;
                            }
                            let node = n.clone();
                            let (x, y) = self.pair(from, out);
                            if ramping {
                                for k in 0..block {
                                    y[k] += x[k] * node.at(v, k);
                                }
                            } else {
                                for k in 0..block {
                                    y[k] += x[k] * g;
                                }
                            }
                        }
                    }
                    self.nodes[node as usize].finish_block(block);
                }
                Step::Clear { dst } => {
                    for d in dst.range() {
                        let range = self.buf(self.indices[d]);
                        self.buffers[range].fill(0.0);
                    }
                }
                Step::Send {
                    node,
                    src,
                    dst,
                    coefficients,
                } => {
                    let n = &self.nodes[node as usize];
                    if !n.ramping() && n.target[0] == 0.0 {
                        continue;
                    }
                    let node_copy = n.clone();
                    let columns = src.len as usize;
                    for d in 0..dst.len as usize {
                        let out = self.indices[dst.start as usize + d];
                        for s in 0..columns {
                            let w =
                                self.coefficients[coefficients.start as usize + d * columns + s];
                            if w == 0.0 {
                                continue;
                            }
                            let from = self.indices[src.start as usize + s];
                            let (x, y) = self.pair(from, out);
                            if node_copy.ramping() {
                                for k in 0..block {
                                    y[k] += x[k] * w * node_copy.at(0, k);
                                }
                            } else {
                                let g = w * node_copy.target[0];
                                for k in 0..block {
                                    y[k] += x[k] * g;
                                }
                            }
                        }
                    }
                    self.nodes[node as usize].finish_block(block);
                }
                Step::Meter { src, slot } => {
                    let n = (src.len as usize).min(MAX_CH);
                    let mut chans: [&[f32]; MAX_CH] = [&[]; MAX_CH];
                    for (c, ch) in chans.iter_mut().enumerate().take(n) {
                        *ch = &self.buffers[self.buf(self.indices[src.start as usize + c])];
                    }
                    outputs.meter(self.generation, slot as usize, &chans[..n]);
                }
                Step::Output { src, slot } => {
                    let channels = src.len as usize;
                    for c in 0..channels {
                        let range = self.buf(self.indices[src.start as usize + c]);
                        outputs.write(slot as usize, c, channels, &self.buffers[range]);
                    }
                }
                Step::Loudness { src, slot } => {
                    for c in 0..src.len as usize {
                        let range = self.buf(self.indices[src.start as usize + c]);
                        outputs.loudness(slot as usize, start, c, &self.buffers[range]);
                    }
                }
                Step::Stream { src, slot } => {
                    for c in 0..(src.len as usize).min(2) {
                        let range = self.buf(self.indices[src.start as usize + c]);
                        outputs.stream(slot as usize, c, &self.buffers[range]);
                    }
                }
                Step::Analyse { src } => {
                    let n = (src.len as usize).min(2);
                    for c in 0..n {
                        let range = self.buf(self.indices[src.start as usize + c]);
                        outputs.analyse(c, n, &self.buffers[range]);
                    }
                }
                Step::Process {
                    proc,
                    src,
                    dst,
                    key,
                } => {
                    let n = (dst.len.min(src.len) as usize).min(MAX_CH);
                    for c in 0..n {
                        let (s, d) = (
                            self.indices[src.start as usize + c],
                            self.indices[dst.start as usize + c],
                        );
                        let (s, d) = self.pair(s, d);
                        d.copy_from_slice(s);
                    }
                    let base = self.buffers.as_mut_ptr();
                    let mut io: [&mut [f32]; MAX_CH] =
                        std::array::from_fn(|_| <&mut [f32]>::default());
                    for (c, slot) in io.iter_mut().enumerate().take(n) {
                        let b = self.indices[dst.start as usize + c] as usize;
                        // SAFETY: the compiler gives every channel of a node its own
                        // buffer, distinct from the key's; all lie inside `buffers`.
                        *slot =
                            unsafe { std::slice::from_raw_parts_mut(base.add(b * block), block) };
                    }
                    let kn = (key.len as usize).min(MAX_CH);
                    let mut keys: [&[f32]; MAX_CH] = [&[]; MAX_CH];
                    for (c, slot) in keys.iter_mut().enumerate().take(kn) {
                        let b = self.indices[key.start as usize + c] as usize;
                        // SAFETY: as above; the key buffers are only read.
                        *slot = unsafe { std::slice::from_raw_parts(base.add(b * block), block) };
                    }
                    let meters = &self.node_meters;
                    if let Some(p) = self.procs.get_mut(proc as usize) {
                        p.start = start;
                        p.process(&mut io[..n], &keys[..kn], block, meters);
                    }
                }
                Step::HoldOut { proc, dst } => {
                    for c in 0..dst.len as usize {
                        let range = self.buf(self.indices[dst.start as usize + c]);
                        match self.procs.get(proc as usize) {
                            Some(p) => p.held(c, &mut self.buffers[range]),
                            None => self.buffers[range].fill(0.0),
                        }
                    }
                }
                Step::HoldIn { proc, src } => {
                    for c in 0..src.len as usize {
                        let range = self.buf(self.indices[src.start as usize + c]);
                        if let Some(p) = self.procs.get_mut(proc as usize) {
                            p.hold(c, &self.buffers[range]);
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub struct TestInputs(pub Vec<Vec<Vec<f32>>>);

    impl InputSource for TestInputs {
        fn channel(&self, input: usize, channel: usize) -> Option<&[f32]> {
            self.0.get(input)?.get(channel).map(|c| c.as_slice())
        }
    }

    #[derive(Default)]
    pub struct TestOutputs(pub Vec<Vec<Vec<f32>>>);

    impl OutputSink for TestOutputs {
        fn write(&mut self, slot: usize, channel: usize, channels: usize, samples: &[f32]) {
            if self.0.len() <= slot {
                self.0.resize(slot + 1, Vec::new());
            }
            let out = &mut self.0[slot];
            if out.len() < channels {
                out.resize(channels, Vec::new());
            }
            out[channel].extend_from_slice(samples);
        }
    }

    /// One input channel → gain node → matrix (mono to stereo) → output.
    fn simple(block: usize, gain: f32, ramp: u32) -> Plan {
        let mut b = PlanBuilder::new(block);
        let input = b.buffers(1);
        let post = b.buffers(1);
        let pan = b.buffers(2);
        let i0 = b.buffer(input, 0);
        b.step(Step::Patch {
            input: 0,
            channel: 0,
            dst: i0,
        });
        let g = b.node(1, &[gain], ramp);
        b.step(Step::Gain {
            node: g,
            src: input,
            dst: post,
        });
        let m = b.node(2, &[0.5, 0.25], ramp);
        b.step(Step::Matrix {
            node: m,
            src: post,
            dst: pan,
        });
        b.step(Step::Output { src: pan, slot: 0 });
        b.finish()
    }

    fn ones(block: usize) -> TestInputs {
        TestInputs(vec![vec![vec![1.0; block]]])
    }

    #[test]
    fn new_nodes_ramp_in_from_zero() {
        let mut plan = simple(4, 1.0, 8);
        let mut out = TestOutputs::default();
        plan.process(0, &ones(4), &mut out);
        plan.process(0, &ones(4), &mut out);
        plan.process(0, &ones(4), &mut out);
        let left = &out.0[0][0];
        // gain ramp k/8 times matrix ramp 0.5·k/8.
        let expected: Vec<f32> = (1..=12)
            .map(|k: usize| {
                let r = (k.min(8) as f32) / 8.0;
                r * 0.5 * r
            })
            .collect();
        for (a, b) in left.iter().zip(&expected) {
            assert!((a - b).abs() < 1e-6, "{left:?}");
        }
        assert!((out.0[0][1][11] - 0.25).abs() < 1e-6);
    }

    #[test]
    fn parameter_ramps_are_linear_and_continuous() {
        let mut plan = simple(48, 1.0, 0);
        let mut out = TestOutputs::default();
        plan.process(0, &ones(48), &mut out);
        assert!(plan.set_param(1, &[0.0], 480));
        assert!(!plan.set_param(99, &[0.0], 480));
        for _ in 0..12 {
            plan.process(0, &ones(48), &mut out);
        }
        let left = &out.0[0][0];
        assert!(left[..48].iter().all(|x| (*x - 0.5).abs() < 1e-6));
        let mut prev = 0.5;
        for x in &left[48..48 + 480] {
            assert!(
                *x <= prev && prev - *x < 0.5 / 480.0 + 1e-6,
                "step too large"
            );
            prev = *x;
        }
        assert!(left[48 + 480..].iter().all(|x| *x == 0.0));
    }

    proptest::proptest! {
        #[test]
        fn ramps_stay_between_old_and_new_and_land_exactly(
            from in -2.0f32..2.0,
            to in -2.0f32..2.0,
            ramp in 0u32..2000,
            block in proptest::sample::select(vec![48usize, 96, 144, 240]),
        ) {
            let mut plan = simple(block, from, 0);
            let mut out = TestOutputs::default();
            plan.set_param(1, &[to], ramp);
            let blocks = ramp as usize / block + 2;
            for _ in 0..blocks {
                plan.process(0, &ones(block), &mut out);
            }
            let (lo, hi) = (from.min(to) * 0.5, from.max(to) * 0.5);
            let left = &out.0[0][0];
            for x in left {
                proptest::prop_assert!(*x >= lo - 1e-5 && *x <= hi + 1e-5, "{} not in {}..{}", x, lo, hi);
            }
            for x in &left[ramp as usize..] {
                proptest::prop_assert_eq!(*x, to * 0.5);
            }
        }
    }

    #[test]
    fn a_swap_keeps_the_values_of_unchanged_nodes() {
        let block = 16;
        let mut old = simple(block, 1.0, 0);
        let mut out = TestOutputs::default();
        old.process(0, &ones(block), &mut out);
        // A new plan with the same nodes (now carried) plus an unrelated one.
        let mut b = PlanBuilder::new(block);
        let input = b.buffers(1);
        let post = b.buffers(1);
        let pan = b.buffers(2);
        let extra = b.buffers(1);
        let i0 = b.buffer(input, 0);
        b.step(Step::Patch {
            input: 0,
            channel: 0,
            dst: i0,
        });
        let g = b.node(1, &[1.0], 480);
        b.step(Step::Gain {
            node: g,
            src: input,
            dst: post,
        });
        let m = b.node(2, &[0.5, 0.25], 480);
        b.step(Step::Matrix {
            node: m,
            src: post,
            dst: pan,
        });
        let unrelated = b.node(3, &[1.0], 480);
        b.step(Step::Gain {
            node: unrelated,
            src: input,
            dst: extra,
        });
        b.step(Step::Output { src: pan, slot: 0 });
        let mut new = b.finish();
        new.carry = vec![(0, 0), (1, 1)];
        new.adopt(&mut old, 480);
        new.process(0, &ones(block), &mut out);
        let left = &out.0[0][0];
        // No discontinuity across the swap: every sample stays at 0.5.
        assert!(left.iter().all(|x| (*x - 0.5).abs() < 1e-6), "{left:?}");
        assert_eq!(new.nodes[2].current(), &[1.0 / 480.0 * block as f32]);
    }

    #[test]
    fn sends_sum_with_their_mapping() {
        let block = 8;
        let mut b = PlanBuilder::new(block);
        let a = b.buffers(1);
        let c = b.buffers(1);
        let bus = b.buffers(2);
        for (input, span) in [(0u32, a), (1, c)] {
            let dst = b.buffer(span, 0);
            b.step(Step::Patch {
                input,
                channel: 0,
                dst,
            });
        }
        b.step(Step::Clear { dst: bus });
        let w = b.coefficients(&[1.0, 0.0]);
        let s1 = b.node(10, &[0.5], 0);
        b.step(Step::Send {
            node: s1,
            src: a,
            dst: bus,
            coefficients: w,
        });
        let w2 = b.coefficients(&[0.5, 0.5]);
        let s2 = b.node(11, &[-1.0], 0);
        b.step(Step::Send {
            node: s2,
            src: c,
            dst: bus,
            coefficients: w2,
        });
        let off = b.node(12, &[0.0], 0);
        b.step(Step::Send {
            node: off,
            src: a,
            dst: bus,
            coefficients: w2,
        });
        b.step(Step::Output { src: bus, slot: 0 });
        let meter = b.meter(2);
        b.step(Step::Meter {
            src: bus,
            slot: meter,
        });
        let mut plan = b.finish();
        let inputs = TestInputs(vec![vec![vec![0.25; block]], vec![vec![0.5; block]]]);
        struct Metered(TestOutputs, crate::engine::meter::Meterer);
        impl OutputSink for Metered {
            fn write(&mut self, slot: usize, channel: usize, channels: usize, samples: &[f32]) {
                self.0.write(slot, channel, channels, samples);
            }
            fn meter(&mut self, generation: u32, slot: usize, chans: &[&[f32]]) {
                self.1.measure(generation, slot, chans);
            }
        }
        let mut meterer = crate::engine::meter::Meterer::default();
        meterer.plan(plan.generation, plan.meters.clone());
        let mut metered = Metered(TestOutputs::default(), meterer);
        plan.process(0, &inputs, &mut metered);
        let out = metered.0;
        // L = 0.25·0.5 − 0.5·0.5 = −0.125; R = −0.25.
        assert!(out.0[0][0].iter().all(|x| (*x + 0.125).abs() < 1e-7));
        assert!(out.0[0][1].iter().all(|x| (*x + 0.25).abs() < 1e-7));
        let m = &plan.meters.channels;
        assert_eq!(f32::from_bits(m[1].peak.load(Ordering::Relaxed)), 0.25);
        let energy = f64::from_bits(m[0].energy.load(Ordering::Relaxed));
        assert!((energy - 8.0 * 0.015625).abs() < 1e-9);
        assert_eq!(m[0].samples.load(Ordering::Relaxed), 8);
    }

    #[test]
    fn missing_inputs_and_denormals_are_silent_or_finite() {
        let block = 8;
        let mut plan = simple(block, 1.0, 0);
        let mut out = TestOutputs::default();
        plan.process(0, &TestInputs(vec![]), &mut out);
        assert!(out.0[0][0].iter().all(|x| *x == 0.0));
        let tiny = TestInputs(vec![vec![vec![f32::MIN_POSITIVE / 4.0; block]]]);
        plan.process(0, &tiny, &mut out);
        assert!(out.0[0][0].iter().all(|x| x.is_finite() && x.abs() < 1e-37));
    }
}
