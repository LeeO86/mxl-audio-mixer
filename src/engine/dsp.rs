//! The DSP thread's loop. Everything it touches is allocated before the loop or
//! on the control side.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use mxl::Error as MxlError;

use super::plan::{InputSource, OutputSink};
use super::{
    Command, EngineConfig, EngineCounters, OutputWriter, Plan, Returned, BLOCK_BUCKETS_US,
    FADE_SAMPLES, MAX_CHANNELS, MAX_LOUDNESS, MAX_OUTPUTS,
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
    /// Head index at the previous block; arrival lags count only while it moves.
    last_head: u64,
}

impl Input {
    fn new(block: usize) -> Self {
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
            last_head: 0,
        }
    }

    /// Silent: no reader and the fade-out is over.
    fn silent(&self) -> bool {
        self.reader.is_none() && self.gain <= 0.0
    }
}

struct Inputs<'a> {
    inputs: &'a [Input],
    block: usize,
}

impl InputSource for Inputs<'_> {
    fn channel(&self, input: usize, channel: usize) -> Option<&[f32]> {
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
    block: usize,
}

impl OutputSink for Outputs<'_> {
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
    scheduling: Arc<std::sync::Mutex<String>>,
) {
    let notes = crate::timing::set_up_dsp_thread(config.rt_priority, config.cpu);
    tracing::info!("DSP thread: {notes}");
    if let Ok(mut s) = scheduling.lock() {
        *s = notes;
    }
    let block = config.block as usize;
    let b = u64::from(config.block);
    let period_ns = time_of(b);
    // Bounded wait for samples that are almost there: a quarter block.
    let read_wait = Duration::from_nanos(period_ns / 4);
    let mut inputs: Vec<Input> = (0..config.inputs).map(|_| Input::new(block)).collect();
    let mut outputs: Vec<Option<Box<OutputWriter>>> = (0..MAX_OUTPUTS).map(|_| None).collect();
    let mut loudness: Vec<Option<Box<SegmentMeter>>> = (0..MAX_LOUDNESS).map(|_| None).collect();
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
                            let _ = returned.push(Returned::Reader(old));
                        }
                        input.channels = channels.min(MAX_CHANNELS);
                        input.started = false;
                        input.gain = 0.0;
                        input.fading = 1;
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
            let now_index = index_at(tai_ns());
            if let Ok(runtime) = reader.get_runtime_info() {
                c.head_index.store(runtime.headIndex, Ordering::Relaxed);
                // A head that does not move is a stall, not a late arrival.
                if runtime.headIndex != input.last_head {
                    input.last_head = runtime.headIndex;
                    let lag = now_index
                        .saturating_sub(runtime.headIndex)
                        .min(u64::from(u32::MAX)) as u32;
                    let _ = lags[i].push(lag);
                }
            }
            let channels = input.channels;
            let result = reader.get_samples(head, block, read_wait).map(|data| {
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
                        c.missing_samples.fetch_add(b, Ordering::Relaxed);
                        if matches!(error, MxlError::OutOfRangeTooEarly | MxlError::Timeout) {
                            c.late_samples.fetch_add(b, Ordering::Relaxed);
                        }
                    }
                    apply_fades(input, block, c, false);
                }
            }
        }

        for o in outputs.iter_mut().flatten() {
            if o.next != start {
                o.filled = 0;
            }
            o.next = start + b;
            o.staged = false;
            for c in 0..o.channels() {
                let at = c * o.commit + o.filled;
                o.staging[at..at + block].fill(0.0);
            }
        }
        if let Some(p) = plan.as_mut() {
            p.process(
                start,
                &Inputs {
                    inputs: &inputs,
                    block,
                },
                &mut Outputs {
                    slots: &mut outputs,
                    loudness: &mut loudness,
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
        // After a miss, continue with the first block that has not ended yet.
        n = (n + 1).max(index_at(done) / b);
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
