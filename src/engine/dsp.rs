//! The DSP thread's loop. Everything it touches is allocated before the loop or
//! on the control side.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use mxl::Error as MxlError;

use super::plan::{InputSource, OutputSink};
use super::talk::{TalkIn, MAX_TALK};
use super::{
    Command, EngineConfig, EngineCounters, OutputWriter, Plan, Returned, BLOCK_BUCKETS_US,
    FADE_SAMPLES, MAX_CHANNELS, MAX_LOUDNESS, MAX_OUTPUTS, MAX_STREAMS,
};
use crate::dsp::loudness::SegmentMeter;
use crate::mxlio::SamplesReader;
use crate::timing::{index_at, sleep_until_tai, tai_ns, time_of};

struct Input {
    reader: Option<Box<SamplesReader>>,
    channels: usize,
    /// Extra delay in samples (beyond the read offset).
    delay: u32,
    /// A delay change waiting for the fade-out to finish.
    pending_delay: Option<u32>,
    /// Current gain of the fade, 0..=1.
    gain: f32,
    /// Fade direction: +1 in, −1 out, 0 steady.
    fading: i8,
    /// A block was read since the reader was set (start-up silence is not missing).
    started: bool,
    /// Samples of the current block, channel-major.
    buffer: Vec<f32>,
    /// Last sample of each channel, held while fading out over a gap.
    last: [f32; MAX_CHANNELS],
    arrival: ArrivalTracker,
}

impl Input {
    fn new(block: usize, lag_window_blocks: u32, block_samples: u32) -> Self {
        Self {
            reader: None,
            channels: 0,
            delay: 0,
            pending_delay: None,
            gain: 0.0,
            fading: 1,
            started: false,
            buffer: vec![0.0; MAX_CHANNELS * block],
            last: [0.0; MAX_CHANNELS],
            arrival: ArrivalTracker::new(lag_window_blocks, block_samples),
        }
    }

    /// Silent: no reader and the fade-out is over.
    fn silent(&self) -> bool {
        self.reader.is_none() && self.gain <= 0.0
    }
}

struct Inputs<'a> {
    inputs: &'a [Input],
    talks: &'a [TalkIn],
    block: usize,
}

impl InputSource for Inputs<'_> {
    fn channel(&self, input: usize, channel: usize) -> Option<&[f32]> {
        if input >= self.inputs.len() {
            let t = self.talks.get(input - self.inputs.len())?;
            return (channel == 0 && t.ring.is_some()).then_some(&t.buffer[..self.block]);
        }
        let i = self.inputs.get(input)?;
        if i.silent() || channel >= i.channels {
            return None;
        }
        Some(&i.buffer[channel * self.block..(channel + 1) * self.block])
    }
}

struct Outputs<'a> {
    slots: &'a mut [Option<Box<OutputWriter>>],
    loudness: &'a mut [Option<Box<SegmentMeter>>],
    analysis: &'a mut rtrb::Producer<f32>,
    vad: &'a mut rtrb::Producer<f32>,
    vad_drops: &'a std::sync::atomic::AtomicU64,
    meters: &'a mut rtrb::Producer<f32>,
    meter_drops: &'a std::sync::atomic::AtomicU64,
    streams: &'a mut [Option<Box<rtrb::Producer<f32>>>],
    /// The stream ring has room for this block (checked at channel 1).
    stream_room: bool,
    /// The analyser ring has room for this block (checked at channel 1).
    analysis_room: bool,
    block: usize,
}

impl Outputs<'_> {
    fn push_analysis(&mut self, samples: &[f32]) {
        if let Ok(chunk) = self.analysis.write_chunk_uninit(samples.len()) {
            chunk.fill_from_iter(samples.iter().copied());
        }
    }
}

impl OutputSink for Outputs<'_> {
    fn stream(&mut self, slot: usize, channel: usize, samples: &[f32]) {
        let block = self.block;
        let Some(Some(ring)) = self.streams.get_mut(slot) else {
            return;
        };
        if channel == 0 {
            self.stream_room = ring.slots() >= 2 * block;
        }
        if !self.stream_room {
            return;
        }
        if let Ok(chunk) = ring.write_chunk_uninit(block) {
            chunk.fill_from_iter(samples[..block].iter().copied());
        }
    }

    fn analyse(&mut self, channel: usize, channels: usize, samples: &[f32]) {
        let samples = &samples[..self.block];
        if channel == 0 {
            // Both channels of a block or neither: the reader stays aligned.
            self.analysis_room = self.analysis.slots() >= 2 * self.block;
        }
        if !self.analysis_room {
            return;
        }
        self.push_analysis(samples);
        if channels == 1 {
            self.push_analysis(samples);
        }
    }

    fn vad(&mut self, generation: u32, slot: usize, chans: &[&[f32]]) {
        super::meter::write(self.vad, self.vad_drops, generation, slot as u32, chans);
    }

    fn meter(&mut self, generation: u32, slot: usize, chans: &[&[f32]]) {
        super::meter::write(
            self.meters,
            self.meter_drops,
            generation,
            slot as u32,
            chans,
        );
    }

    fn loudness(&mut self, slot: usize, start: u64, channel: usize, samples: &[f32]) {
        if let Some(Some(m)) = self.loudness.get_mut(slot) {
            m.feed(start, channel, samples);
        }
    }

    fn write(&mut self, slot: usize, channel: usize, _channels: usize, samples: &[f32]) {
        let Some(Some(o)) = self.slots.get_mut(slot) else {
            return;
        };
        if channel >= o.channels() {
            return;
        }
        let at = channel * o.commit + o.filled;
        o.staging[at..at + self.block].copy_from_slice(&samples[..self.block]);
        o.staged = true;
    }
}

/// An input's arrival lag, one value per read block (§4.2): how far the flow's
/// head index is behind the end of the block being read. The read needs
/// `lag <= read_offset + delay`, so this is the quantity the automatic delay
/// must cover, and a block is late exactly when its lag is above that.
///
/// Every block counts, also the ones in which the head did not move: a writer
/// that commits in batches (a frame of audio, a burst of packets) is furthest
/// behind just before its next commit. The lags of blocks without a commit are
/// held until the head moves (they grow by one block each, so the first and a
/// count describe them); when it does not move within the window, the writer
/// stalled and they are dropped (a stall is not an arrival lag). After a new
/// reader, and after a stall, nothing counts until the writer has caught up: a
/// new flow's head is 0, a re-opened one may be stale, and a writer that resumes
/// is seconds behind and then commits faster than real time, also in bits when
/// it gets the CPU in bits. Caught up is back within a block of the lag from
/// before the stall, or a head that kept real time (at most 1/8 faster) over
/// 0.5 s: a new reader, a writer now later than before, one always later than
/// the window (shown; the delay ignores it). A catch-up slower than 1.125 times
/// real time counts.
struct ArrivalTracker {
    last_head: Option<u64>,
    /// Lag of the first block without a commit, and how many there are.
    first_waiting: u32,
    waiting: u32,
    window: u32,
    block: u32,
    stalled: bool,
    /// After a reset or a stall, until the writer has caught up.
    catching_up: Option<CatchUp>,
    /// The last lag reported: what "caught up" means after a stall.
    last_pushed: Option<u32>,
}

/// A writer catching up: where the current measuring span started (block end
/// and head index, at a head move), and the lag before the stall.
#[derive(Clone, Copy)]
struct CatchUp {
    since: Option<(u64, u64)>,
    target: Option<u32>,
}

/// The span over which the head's pace is measured: 0.5 s. Caught up is a head
/// that advanced at most 1/8 more than the time did (a batched writer moves at
/// real time on average; one catching up faster).
const CATCH_UP_SPAN_SAMPLES: u64 = 24_000;

impl ArrivalTracker {
    fn new(window_blocks: u32, block: u32) -> Self {
        Self {
            last_head: None,
            first_waiting: 0,
            waiting: 0,
            window: window_blocks,
            block,
            stalled: true,
            catching_up: None,
            last_pushed: None,
        }
        .catch_up()
    }

    fn catch_up(mut self) -> Self {
        self.start_catch_up();
        self
    }

    fn start_catch_up(&mut self) {
        self.catching_up = Some(CatchUp {
            since: None,
            target: self.last_pushed,
        });
    }

    fn reset(&mut self) {
        self.last_head = None;
        self.waiting = 0;
        self.stalled = true;
        self.start_catch_up();
    }

    /// The block ending at index `end` reads a flow whose head index is `head`.
    fn observe(&mut self, head: u64, end: u64, mut push: impl FnMut(u32)) {
        let lag = end.saturating_sub(head).min(u64::from(u32::MAX)) as u32;
        let Some(last) = self.last_head else {
            self.last_head = Some(head);
            return;
        };
        if head != last {
            self.last_head = Some(head);
            if let Some(mut c) = self.catching_up {
                self.waiting = 0;
                self.stalled = false;
                let back = c
                    .target
                    .is_some_and(|t| lag <= t.saturating_add(self.block));
                let mut paced = false;
                match c.since {
                    None => c.since = Some((end, head)),
                    Some((e0, h0)) if end - e0 >= CATCH_UP_SPAN_SAMPLES => {
                        let (time, advance) = (end - e0, head.saturating_sub(h0));
                        paced = advance <= time + time / 8;
                        c.since = Some((end, head));
                    }
                    Some(_) => {}
                }
                if back || paced {
                    self.catching_up = None;
                    self.last_pushed = Some(lag);
                    push(lag);
                } else {
                    self.catching_up = Some(c);
                }
                return;
            }
            if !self.stalled {
                for k in 0..self.waiting {
                    push(self.first_waiting.saturating_add(k * self.block));
                }
            }
            self.waiting = 0;
            self.stalled = false;
            self.last_pushed = Some(lag);
            push(lag);
        } else if !self.stalled {
            if self.waiting == 0 {
                self.first_waiting = lag;
            }
            if self.waiting < self.window {
                self.waiting += 1;
            } else {
                self.waiting = 0;
                self.stalled = true;
                self.start_catch_up();
            }
        }
    }
}

/// The rings the DSP thread copies audio into for the control side.
pub(super) struct Taps {
    /// Every metered block, as records (`meter`).
    pub meters: rtrb::Producer<f32>,
    /// The analysed signal (spectrum, goniometer).
    pub analysis: rtrb::Producer<f32>,
    /// Every block of the VAD taps, as records (`meter::write`).
    pub vad: rtrb::Producer<f32>,
}

const FADE_STEP: f32 = 1.0 / FADE_SAMPLES as f32;

fn record_block_time(counters: &EngineCounters, ns: u64, period_ns: u64) {
    let us = ns / 1_000;
    let bucket = BLOCK_BUCKETS_US
        .iter()
        .position(|b| us <= *b)
        .unwrap_or(BLOCK_BUCKETS_US.len());
    counters.block_time_buckets[bucket].fetch_add(1, Ordering::Relaxed);
    counters.block_time_sum_ns.fetch_add(ns, Ordering::Relaxed);
    counters.load_ppm.store(
        (ns.saturating_mul(1_000_000) / period_ns.max(1)) as u32,
        Ordering::Relaxed,
    );
}

pub(super) fn run(
    config: EngineConfig,
    counters: Arc<EngineCounters>,
    mut commands: rtrb::Consumer<Command>,
    mut returned: rtrb::Producer<Returned>,
    mut lags: Vec<rtrb::Producer<u32>>,
    taps: Taps,
    scheduling: Arc<std::sync::Mutex<String>>,
) {
    let Taps {
        mut meters,
        mut analysis,
        mut vad,
    } = taps;
    let (notes, realtime) = crate::timing::set_up_dsp_thread(config.rt_priority, config.cpu);
    counters.realtime.store(realtime, Ordering::Relaxed);
    tracing::info!("DSP thread: {notes}");
    if let Ok(mut s) = scheduling.lock() {
        *s = notes;
    }
    let block = config.block as usize;
    let b = u64::from(config.block);
    let period_ns = time_of(b);
    // Bounded wait for samples that are almost there: a quarter block per block,
    // shared by all inputs (see `read_timeout`).
    let read_wait_ns = period_ns / 4;
    let lag_window_blocks = config.lag_window.div_ceil(config.block.max(1));
    let mut inputs: Vec<Input> = (0..config.inputs)
        .map(|_| Input::new(block, lag_window_blocks, config.block))
        .collect();
    let mut outputs: Vec<Option<Box<OutputWriter>>> = (0..MAX_OUTPUTS).map(|_| None).collect();
    let mut loudness: Vec<Option<Box<SegmentMeter>>> = (0..MAX_LOUDNESS).map(|_| None).collect();
    let mut streams: Vec<Option<Box<rtrb::Producer<f32>>>> =
        (0..MAX_STREAMS).map(|_| None).collect();
    let mut talks: Vec<TalkIn> = (0..MAX_TALK).map(|_| TalkIn::new(block)).collect();
    let mut plan: Option<Box<Plan>> = None;
    let mut stopping = false;
    // The fade-out takes FADE_SAMPLES; a few blocks more cover a full commit.
    let mut stop_blocks = (FADE_SAMPLES as usize / block + 2) as u32 * 4;
    let mut n = index_at(tai_ns()) / b + 1;
    counters.running.store(true, Ordering::SeqCst);

    loop {
        if counters.stop.load(Ordering::Relaxed) && !stopping {
            // G8: fade every output out before the writers are released.
            stopping = true;
            for o in outputs.iter_mut().flatten() {
                o.fading = -1;
            }
        }
        if stopping {
            if outputs.iter().flatten().all(|o| o.gain <= 0.0) || stop_blocks == 0 {
                break;
            }
            stop_blocks -= 1;
        }
        let start = n * b;
        let deadline = time_of(start + b);
        sleep_until_tai(deadline);
        let woke = tai_ns();
        if woke < deadline {
            continue;
        }
        if woke - deadline > period_ns {
            counters.deadline_misses.fetch_add(1, Ordering::Relaxed);
        }

        while let Ok(command) = commands.pop() {
            match command {
                Command::SetInput {
                    index,
                    reader,
                    channels,
                } => {
                    if let Some(input) = inputs.get_mut(index) {
                        if let Some(old) = input.reader.replace(reader) {
                            // The control side drops returned readers (and with
                            // the last one an MXL instance and its watcher
                            // thread). Only a full ring (4096 returns not yet
                            // collected) drops one here, on the DSP thread.
                            let _ = returned.push(Returned::Reader(old));
                        }
                        input.channels = channels.min(MAX_CHANNELS);
                        input.started = false;
                        input.gain = 0.0;
                        input.fading = 1;
                        input.arrival.reset();
                        counters.inputs[index]
                            .channels
                            .store(input.channels as u32, Ordering::Relaxed);
                    }
                }
                Command::ClearInput { index } => {
                    if let Some(input) = inputs.get_mut(index) {
                        if let Some(old) = input.reader.take() {
                            let _ = returned.push(Returned::Reader(old));
                        }
                        input.started = false;
                        counters.inputs[index].ok.store(false, Ordering::Relaxed);
                    }
                }
                Command::SetDelay { index, samples } => {
                    if let Some(input) = inputs.get_mut(index) {
                        if input.started {
                            input.pending_delay = Some(samples);
                            input.fading = -1;
                        } else {
                            input.delay = samples;
                            counters.inputs[index]
                                .delay_samples
                                .store(samples, Ordering::Relaxed);
                        }
                    }
                }
                Command::SetPlan(mut next) => {
                    if let Some(mut old) = plan.take() {
                        next.adopt(&mut old, FADE_SAMPLES);
                        let _ = returned.push(Returned::Plan(old));
                    }
                    plan = Some(next);
                    counters.plan_swaps.fetch_add(1, Ordering::Relaxed);
                }
                Command::Param {
                    node,
                    values,
                    len,
                    ramp,
                } => {
                    if let Some(p) = plan.as_mut() {
                        p.set_param(node, &values[..len as usize], ramp);
                    }
                }
                Command::SetOutput { slot, mut output } => {
                    if let Some(s) = outputs.get_mut(slot) {
                        output.next = start;
                        if let Some(old) = s.replace(output) {
                            let _ = returned.push(Returned::Output(old));
                        }
                    }
                }
                Command::ClearOutput { slot } => {
                    if let Some(old) = outputs.get_mut(slot).and_then(|s| s.take()) {
                        let _ = returned.push(Returned::Output(old));
                    }
                }
                Command::ProcParam {
                    node,
                    index,
                    value,
                    ramp,
                } => {
                    if let Some(p) = plan.as_mut() {
                        p.set_proc_param(node, index as usize, value, ramp);
                    }
                }
                Command::SetLoudness { slot, meter } => {
                    if let Some(s) = loudness.get_mut(slot) {
                        if let Some(old) = s.replace(meter) {
                            let _ = returned.push(Returned::Loudness(old));
                        }
                    }
                }
                Command::ClearLoudness { slot } => {
                    if let Some(old) = loudness.get_mut(slot).and_then(|s| s.take()) {
                        let _ = returned.push(Returned::Loudness(old));
                    }
                }
                Command::SetStream { slot, ring } => {
                    if let Some(s) = streams.get_mut(slot) {
                        if let Some(old) = s.replace(ring) {
                            let _ = returned.push(Returned::Stream(old));
                        }
                    }
                }
                Command::ClearStream { slot } => {
                    if let Some(old) = streams.get_mut(slot).and_then(|s| s.take()) {
                        let _ = returned.push(Returned::Stream(old));
                    }
                }
                Command::SetTalk { slot, ring } => {
                    if let Some(t) = talks.get_mut(slot) {
                        if let Some(old) = t.ring.replace(ring) {
                            let _ = returned.push(Returned::Talk(old));
                        }
                        t.reset();
                    }
                }
                Command::ClearTalk { slot } => {
                    if let Some(t) = talks.get_mut(slot) {
                        if let Some(old) = t.ring.take() {
                            let _ = returned.push(Returned::Talk(old));
                        }
                        t.reset();
                    }
                }
            }
        }

        for (i, input) in inputs.iter_mut().enumerate() {
            let c = &counters.inputs[i];
            let Some(reader) = input.reader.as_ref() else {
                if input.gain > 0.0 {
                    apply_fades(input, block, c, false);
                }
                continue;
            };
            let total = u64::from(config.read_offset) + u64::from(input.delay);
            let head = (start + b).saturating_sub(total);
            if let Ok(runtime) = reader.get_runtime_info() {
                c.head_index.store(runtime.headIndex, Ordering::Relaxed);
                let ring = &mut lags[i];
                input.arrival.observe(runtime.headIndex, start + b, |lag| {
                    let _ = ring.push(lag);
                });
            }
            let channels = input.channels;
            let timeout = read_timeout(tai_ns(), woke + read_wait_ns);
            let mut result = match timeout {
                Some(t) => reader.get_samples(head, block, t),
                None => reader.get_samples_non_blocking(head, block),
            };
            if timeout.is_some() && matches!(result, Err(MxlError::OutOfRangeTooEarly)) {
                // MXL wakes a waiting reader only once per sync batch of the writer
                // (maxSyncBatchSizeHint) and, when its wait times out, returns the
                // result from before the wait: a commit during the wait is only
                // seen by looking once more.
                result = reader.get_samples_non_blocking(head, block);
            }
            let result = result.map(|data| {
                let count = data.num_of_channels().min(channels);
                for ch in 0..channels {
                    let out = &mut input.buffer[ch * block..(ch + 1) * block];
                    if ch >= count {
                        out.fill(0.0);
                        continue;
                    }
                    let Ok((a, bb)) = data.channel_data(ch) else {
                        out.fill(0.0);
                        continue;
                    };
                    for (dst, src) in out
                        .iter_mut()
                        .zip(a.chunks_exact(4).chain(bb.chunks_exact(4)))
                    {
                        *dst = f32::from_le_bytes([src[0], src[1], src[2], src[3]]);
                    }
                }
            });
            match result {
                Ok(()) => {
                    input.started = true;
                    c.ok.store(true, Ordering::Relaxed);
                    c.read_samples.fetch_add(b, Ordering::Relaxed);
                    apply_fades(input, block, c, true);
                }
                Err(error) => {
                    c.ok.store(false, Ordering::Relaxed);
                    if input.started {
                        note_missing(c, b, error);
                    }
                    apply_fades(input, block, c, false);
                }
            }
        }

        for (slot, o) in outputs.iter_mut().enumerate() {
            let Some(o) = o.as_mut() else {
                continue;
            };
            if o.next != start {
                if o.next < start {
                    fill_gap(o, start, &counters.outputs[slot]);
                }
                o.filled = 0;
            }
            o.next = start + b;
            o.staged = false;
            for c in 0..o.channels() {
                let at = c * o.commit + o.filled;
                o.staging[at..at + block].fill(0.0);
            }
        }
        for (t, c) in talks.iter_mut().zip(&counters.talk) {
            t.read(block, c);
        }
        if let Some(p) = plan.as_mut() {
            p.process(
                start,
                &Inputs {
                    inputs: &inputs,
                    talks: &talks,
                    block,
                },
                &mut Outputs {
                    slots: &mut outputs,
                    loudness: &mut loudness,
                    analysis: &mut analysis,
                    analysis_room: false,
                    vad: &mut vad,
                    vad_drops: &counters.vad_drops,
                    meters: &mut meters,
                    meter_drops: &counters.meter_drops,
                    streams: &mut streams,
                    stream_room: false,
                    block,
                },
            );
        }
        for (slot, o) in outputs.iter_mut().enumerate() {
            let Some(o) = o.as_mut() else {
                continue;
            };
            if !o.staged {
                o.filled = 0;
                if stopping {
                    o.gain = 0.0;
                }
                continue;
            }
            fade_output(o, block);
            o.filled += block;
            let oc = &counters.outputs[slot];
            oc.blocks.fetch_add(1, Ordering::Relaxed);
            counters.output_blocks.fetch_add(1, Ordering::Relaxed);
            let last = stopping && o.gain <= 0.0;
            if o.filled >= o.commit || last {
                commit(o, start + b, oc);
            }
        }

        let done = tai_ns();
        record_block_time(&counters, done - woke, period_ns);
        counters.blocks.fetch_add(1, Ordering::Relaxed);
        // After a miss, continue with the first block that has not ended yet; the
        // skipped blocks count as missed (their outputs get silence, fill_gap).
        let next = (n + 1).max(index_at(done) / b);
        if next > n + 1 {
            counters
                .deadline_misses
                .fetch_add(next - n - 1, Ordering::Relaxed);
        }
        n = next;
    }
    counters.running.store(false, Ordering::SeqCst);
    for input in &mut inputs {
        if let Some(reader) = input.reader.take() {
            let _ = returned.push(Returned::Reader(reader));
        }
    }
    for o in outputs.iter_mut() {
        if let Some(o) = o.take() {
            let _ = returned.push(Returned::Output(o));
        }
    }
    if let Some(p) = plan.take() {
        let _ = returned.push(Returned::Plan(p));
    }
    for m in loudness.iter_mut() {
        if let Some(m) = m.take() {
            let _ = returned.push(Returned::Loudness(m));
        }
    }
    for t in talks.iter_mut() {
        if let Some(r) = t.ring.take() {
            let _ = returned.push(Returned::Talk(r));
        }
    }
}

/// Apply the output's fade to the block just staged.
fn fade_output(o: &mut OutputWriter, block: usize) {
    if o.fading == 0 && o.gain >= 1.0 {
        return;
    }
    let channels = o.channels();
    for k in 0..block {
        match o.fading {
            1 => {
                o.gain = (o.gain + FADE_STEP).min(1.0);
                if o.gain >= 1.0 {
                    o.fading = 0;
                }
            }
            -1 => o.gain = (o.gain - FADE_STEP).max(0.0),
            _ => {}
        }
        for c in 0..channels {
            o.staging[c * o.commit + o.filled + k] *= o.gain;
        }
    }
}

/// After skipped blocks (a missed deadline): commit what was staged and write
/// silence up to `start`. Unwritten ranges would keep the ring's samples of one
/// buffer length earlier, which readers cannot tell from fresh ones.
fn fill_gap(o: &mut OutputWriter, start: u64, counters: &super::OutputCounters) {
    if o.filled > 0 {
        let end = o.next;
        commit(o, end, counters);
    }
    let len = u64::from(o.writer.buffer_length.max(1));
    let chunk = o.commit as u64;
    let mut from = o.next.max(start.saturating_sub(len.saturating_sub(chunk)));
    while from < start {
        let n = chunk.min(start - from);
        let result = o.writer.write(from + n, n as usize, |_, _, s| s.fill(0.0));
        if result.is_err() {
            counters.errors.fetch_add(1, Ordering::Relaxed);
        }
        from += n;
    }
}

/// Write the staged samples, which end at index `head`.
fn commit(o: &mut OutputWriter, head: u64, counters: &super::OutputCounters) {
    let count = o.filled;
    let commit = o.commit;
    let staging = &o.staging;
    let result = o.writer.write(head, count, |c, offset, samples| {
        let from = c * commit + offset;
        samples.copy_from_slice(&staging[from..from + samples.len()]);
    });
    o.filled = 0;
    match result {
        Ok(()) => {
            counters.commits.fetch_add(1, Ordering::Relaxed);
            let lag = tai_ns().saturating_sub(time_of(head)) / 1_000;
            counters.commit_lag_us.store(lag, Ordering::Relaxed);
        }
        Err(_) => {
            counters.errors.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// How long a read may wait for samples that are almost there: until `wait_end`,
/// a quarter block after the wake-up, for all inputs of the block together.
/// Each input waiting a quarter block of its own let a few late inputs (at the
/// start, before their delays are set, or stalled ones) take more than the whole
/// block: every block was then a deadline miss (1.4.0, 180 in its first second
/// with 7 late inputs). `None`: the window is over, read without waiting.
fn read_timeout(now: u64, wait_end: u64) -> Option<Duration> {
    (now < wait_end).then(|| Duration::from_nanos(wait_end - now))
}

/// A failed read after the input has started.
///
/// `missing` counts every failure. Late is the source not having written the
/// index yet. Overrun is the ring having already overwritten it. Other errors
/// stay in `missing` only: MXL has no third range code, and a late wake that
/// still finds the samples is a successful read.
fn note_missing(c: &super::InputCounters, samples: u64, error: MxlError) {
    c.missing_samples.fetch_add(samples, Ordering::Relaxed);
    match error {
        MxlError::OutOfRangeTooEarly | MxlError::Timeout => {
            c.late_samples.fetch_add(samples, Ordering::Relaxed);
        }
        MxlError::OutOfRangeTooLate => {
            c.overrun_samples.fetch_add(samples, Ordering::Relaxed);
        }
        _ => {}
    }
}

/// Fade in after a gap or a (re)start, fade out over a gap (holding the last
/// sample) and around a delay change, so nothing clicks.
fn apply_fades(input: &mut Input, block: usize, c: &super::InputCounters, have_data: bool) {
    let channels = input.channels;
    if !have_data && input.fading >= 0 && input.gain > 0.0 {
        input.fading = -1;
    }
    if have_data && input.fading == 0 && input.gain < 1.0 {
        input.fading = 1;
    }
    for k in 0..block {
        match input.fading {
            1 => {
                input.gain = (input.gain + FADE_STEP).min(1.0);
                if input.gain >= 1.0 {
                    input.fading = 0;
                }
            }
            -1 => {
                input.gain = (input.gain - FADE_STEP).max(0.0);
                if input.gain <= 0.0 {
                    if let Some(delay) = input.pending_delay.take() {
                        input.delay = delay;
                        c.delay_samples.store(delay, Ordering::Relaxed);
                    }
                    input.fading = if have_data { 1 } else { 0 };
                }
            }
            _ => {}
        }
        for ch in 0..channels {
            let s = &mut input.buffer[ch * block + k];
            if have_data {
                input.last[ch] = *s;
                *s *= input.gain;
            } else {
                *s = input.last[ch] * input.gain;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{note_missing, ArrivalTracker};
    use crate::engine::InputCounters;
    use mxl::Error as MxlError;
    use std::sync::atomic::Ordering;

    fn count(error: MxlError) -> (u64, u64, u64) {
        let c = InputCounters::default();
        note_missing(&c, 96, error);
        (
            c.missing_samples.load(Ordering::Relaxed),
            c.late_samples.load(Ordering::Relaxed),
            c.overrun_samples.load(Ordering::Relaxed),
        )
    }

    #[test]
    fn missing_samples_split_by_mxl_error() {
        assert_eq!(count(MxlError::OutOfRangeTooEarly), (96, 96, 0));
        assert_eq!(count(MxlError::Timeout), (96, 96, 0));
        assert_eq!(count(MxlError::OutOfRangeTooLate), (96, 0, 96));
        assert_eq!(count(MxlError::FlowNotFound), (96, 0, 0));
    }

    /// Lags of 2 ms blocks reading a writer whose commit of `batch` samples lands
    /// `late` samples after the batch's last sample, over `blocks` blocks.
    fn lags(batch: u64, late: u64, blocks: u64, stall_from: Option<u64>) -> Vec<u32> {
        let mut t = ArrivalTracker::new(76, 96);
        let mut out = Vec::new();
        for n in 0..=blocks {
            let end = 1_000_000 + n * 96;
            // The newest commit visible at `end`, frozen during a stall.
            let at = stall_from.map_or(end, |s| end.min(1_000_000 + s * 96));
            let head = (at - late) / batch * batch;
            t.observe(head, end, |l| out.push(l));
        }
        out
    }

    #[test]
    fn arrival_lag_counts_every_block_of_a_batched_writer() {
        // A writer committing 960 samples (a 50 Hz video frame of audio) on time:
        // the head is up to 960 samples behind just before each commit, and a
        // 2 ms read offset is late in 9 of 10 blocks. v1.3.0 counted only the
        // blocks after a commit (lag 0) and proposed no delay.
        let l = lags(960, 0, 1300, None);
        // Nothing counts in the first 0.5 s (a new reader, D239); the blocks
        // after the last commit are still held.
        assert!(l.len() >= 1000, "{} lags", l.len());
        let max = *l.iter().max().unwrap();
        assert!((864..=960).contains(&max), "max {max}");
        let late = l.iter().filter(|&&x| x > 96).count();
        assert!(
            late > 800,
            "{late} of {} blocks need more than 2 ms",
            l.len()
        );
    }

    #[test]
    fn arrival_lag_of_a_per_packet_writer_stays_small() {
        // 48-sample commits, 20 samples after the packet's end.
        let l = lags(48, 20, 1000, None);
        assert!(l.iter().all(|&x| x < 96), "{:?}", l.iter().max());
    }

    #[test]
    fn arrival_lag_ignores_a_stall() {
        // The writer stops after block 100 and never comes back: the blocks of the
        // stall add nothing beyond the window.
        let l = lags(48, 20, 1500, Some(400));
        assert!(l.iter().all(|&x| x < 96), "{:?}", l.iter().max());
        assert!((100..=401).contains(&l.len()), "{} lags", l.len());
    }

    #[test]
    fn arrival_lag_starts_at_the_first_head_move() {
        // A new flow (head 0) and a re-opened one on a stale head: neither the
        // head before the first commit nor the blocks spent waiting for it count.
        for stale in [0u64, 1_000_000 - 96 * 500] {
            let mut t = ArrivalTracker::new(76, 96);
            let mut out = Vec::new();
            for n in 0..600u64 {
                let end = 1_000_000 + n * 96;
                let head = if n < 30 { stale } else { end - 20 };
                t.observe(head, end, |l| out.push(l));
            }
            // A new reader counts once the lag has held for 0.5 s (250 blocks).
            assert!(
                (300..=330).contains(&out.len()),
                "stale head {stale}: {}",
                out.len()
            );
            assert!(
                out.iter().all(|&x| x == 20),
                "stale head {stale}: {:?}",
                out.iter().max()
            );
            // After a reset (a re-open) the stale head counts for nothing; the
            // first lag back at the one from before (within a block) counts.
            t.reset();
            let mut after = Vec::new();
            t.observe(stale, 2_000_000, |l| after.push(l));
            t.observe(stale + 96, 2_000_096, |l| after.push(l));
            t.observe(2_000_192 - 20, 2_000_192, |l| after.push(l));
            assert_eq!(after, vec![20]);
        }
    }

    #[test]
    fn held_lags_grow_by_one_block() {
        // One commit every 5 blocks: the held blocks come out in order. The
        // first period (before the first head move) does not count.
        let mut t = ArrivalTracker::new(76, 96);
        let mut out = Vec::new();
        for n in 0..=310u64 {
            let end = 96 * (100 + n);
            t.observe(96 * (100 + n / 5 * 5), end, |l| out.push(l));
        }
        assert_eq!(out[out.len() - 6..], [0, 96, 192, 288, 384, 0]);
    }

    #[test]
    fn the_read_wait_is_shared_by_all_inputs() {
        use super::read_timeout;
        use std::time::Duration;
        // Woken at 1000 ns with a 500 ns window: the first input may wait the
        // whole window, a later one only what is left, after it none.
        assert_eq!(read_timeout(1000, 1500), Some(Duration::from_nanos(500)));
        assert_eq!(read_timeout(1400, 1500), Some(Duration::from_nanos(100)));
        assert_eq!(read_timeout(1500, 1500), None);
        assert_eq!(read_timeout(9000, 1500), None);
    }

    #[test]
    fn a_writer_catching_up_after_a_stall_adds_no_lag() {
        // On time (lag 20) for 50 blocks, stopped for 300 blocks (a stall), then
        // it catches up 10 blocks per block and goes on on time. Neither the
        // stall nor the catch-up (lags of seconds, falling) counts.
        let mut t = ArrivalTracker::new(76, 96);
        let mut out = Vec::new();
        let mut head = 0u64;
        for n in 0..1000u64 {
            let end = 1_000_000 + n * 96;
            head = match n {
                0..400 => end - 20,
                400..700 => head,
                _ => (head + 960).min(end - 20),
            };
            t.observe(head, end, |l| out.push(l));
        }
        assert!(out.iter().all(|&l| l == 20), "{:?}", out.iter().max());
        assert!(out.len() > 150, "{} lags", out.len());
    }

    #[test]
    fn a_writer_always_later_than_the_window_is_still_shown() {
        // 300 ms late, on time otherwise: beyond the window (offset + maximum),
        // but its lag is reported (for the metrics; the delay ignores it).
        let mut t = ArrivalTracker::new(76, 96);
        let mut out = Vec::new();
        for n in 0..600u64 {
            let end = 1_000_000 + n * 96;
            t.observe(end - 14_400, end, |l| out.push(l));
        }
        assert!(out.len() >= 300, "{} lags", out.len());
        assert!(out.iter().all(|&l| l == 14_400));
    }

    #[test]
    fn a_slow_catch_up_adds_no_lag_either() {
        // Resumes 3 s behind and catches up at 1.5 times real time (the head
        // moves 144 samples per 96-sample block) to on time (lag 20).
        let mut t = ArrivalTracker::new(76, 96);
        let mut out = Vec::new();
        let mut head = 0u64;
        for n in 0..4000u64 {
            let end = 10_000_000 + n * 96;
            head = match n {
                0..400 => end - 20,
                400..700 => head,
                700 => end - 144_000,
                _ => (head + 144).min(end - 20),
            };
            t.observe(head, end, |l| out.push(l));
        }
        assert!(out.iter().all(|&l| l <= 20 + 96), "{:?}", out.iter().max());
        assert!(out.len() > 300, "{} lags", out.len());
    }

    #[test]
    fn a_catch_up_in_bits_adds_no_lag() {
        // Under load: after the stall the writer gets the CPU in bits, 30 blocks
        // of catch-up, then 20 blocks of nothing, so the lag falls in steps. It
        // counts only once back near its lag from before the stall.
        let mut t = ArrivalTracker::new(76, 96);
        let mut out = Vec::new();
        let (mut head, mut on_time) = (0u64, false);
        for n in 0..3000u64 {
            let end = 10_000_000 + n * 96;
            head = match n {
                0..400 => end - 20,
                400..700 => head,
                _ if on_time || (n - 700) % 50 < 30 => (head + 192).min(end - 20),
                _ => head,
            };
            on_time |= n >= 700 && head == end - 20;
            t.observe(head, end, |l| out.push(l));
        }
        assert!(out.iter().all(|&l| l <= 20 + 96), "{:?}", out.iter().max());
        assert!(out.len() > 1000, "{} lags", out.len());
    }

    #[test]
    fn a_writer_in_bursts_counts_after_half_a_second() {
        // Five 48-sample packets at once every 5 ms (not a multiple of the 2 ms
        // block): the lag at the head's moves goes up and down all the time,
        // but the head keeps real time, so it counts after 0.5 s.
        let l = lags(240, 20, 1000, None);
        assert!(l.len() > 600, "{} lags", l.len());
        assert!(
            l.iter().all(|&x| x <= 20 + 240 + 96),
            "{:?}",
            l.iter().max()
        );
    }
}
