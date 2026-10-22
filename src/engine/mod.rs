//! The DSP engine (§2): one real-time thread on the TAI block clock.
//!
//! Block n covers output samples [n·B, (n+1)·B). The thread sleeps until the TAI
//! time of sample (n+1)·B, then reads every routed input's block ending at
//! (n+1)·B − (read offset + the input's extra delay), runs the current plan and
//! writes the outputs' block ending at (n+1)·B. The control side talks to it only
//! through lock-free single-producer/single-consumer rings and atomics; the thread
//! never allocates, locks or does I/O other than MXL reads and writes.

mod dsp;
pub mod meter;
pub mod plan;
pub mod proc;
pub mod talk;

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use crate::dsp::loudness::SegmentMeter;
use crate::mxlio::writer::SamplesWriter;
use crate::mxlio::SamplesReader;
pub use plan::{NodeId, Plan, MAX_NODE_VALUES};

/// Loudness meter slots of the DSP thread.
pub const MAX_LOUDNESS: usize = MAX_OUTPUTS;
/// Monitors and personal monitors that can stream at once.
pub const MAX_STREAMS: usize = 96;
/// VAD tap slots (§8.1).
pub const MAX_VAD: usize = crate::mix::auto::MAX_VAD;
/// The VAD taps' ring (records as the meters'): 2^22 values, 0.68 s of 64
/// stereo signals.
pub const VAD_RING: usize = 1 << 22;

/// Largest channel count of an input flow (§4.2).
pub const MAX_CHANNELS: usize = 64;
/// Length of every fade in samples: the parameter ramp of §2 (10 ms).
pub const FADE_SAMPLES: u32 = 480;
/// Output slots of the DSP thread.
pub const MAX_OUTPUTS: usize = crate::mix::model::MAX_OUTPUTS;

/// A message from the control side to the DSP thread.
pub enum Command {
    /// Read `reader` for input `index` from the next block on.
    SetInput {
        index: usize,
        reader: Box<SamplesReader>,
        channels: usize,
    },
    /// Stop reading input `index` (master_enable false, unrouted, flow gone).
    ClearInput {
        index: usize,
    },
    /// Change input `index`'s extra delay, fading out and in.
    SetDelay {
        index: usize,
        samples: u32,
    },
    /// Run this plan from the next block on.
    SetPlan(Box<Plan>),
    /// Ramp node `node` to `values[..len]` over `ramp` samples (§2).
    Param {
        node: NodeId,
        values: [f32; MAX_NODE_VALUES],
        len: u8,
        ramp: u32,
    },
    /// Write output slot `slot` with this writer from the next block on.
    SetOutput {
        slot: usize,
        output: Box<OutputWriter>,
    },
    /// Stop writing output slot `slot`; the writer comes back to be released.
    ClearOutput {
        slot: usize,
    },
    /// Ramp parameter `index` of processor `node` to `value` over `ramp` samples.
    ProcParam {
        node: NodeId,
        index: u16,
        value: f32,
        ramp: u32,
    },
    /// Feed loudness meter slot `slot` with this meter from the next block on.
    SetLoudness {
        slot: usize,
        meter: Box<SegmentMeter>,
    },
    ClearLoudness {
        slot: usize,
    },
    /// Copy stream slot `slot` (a monitor's stereo, per block channel 1 then
    /// channel 2) into this ring from the next block on.
    SetStream {
        slot: usize,
        ring: Box<rtrb::Producer<f32>>,
    },
    ClearStream {
        slot: usize,
    },
    /// Play talkback slot `slot` (engine input `inputs + slot`) from this ring.
    SetTalk {
        slot: usize,
        ring: Box<rtrb::Consumer<f32>>,
    },
    ClearTalk {
        slot: usize,
    },
}

/// A message from the DSP thread back: things it must not free itself.
pub enum Returned {
    Reader(Box<SamplesReader>),
    Plan(Box<Plan>),
    Output(Box<OutputWriter>),
    Loudness(Box<SegmentMeter>),
    Stream(Box<rtrb::Producer<f32>>),
    Talk(Box<rtrb::Consumer<f32>>),
}

/// An output's writer with its commit staging (§4.3: the commit size is separate
/// from the block size) and its fade.
pub struct OutputWriter {
    pub writer: SamplesWriter,
    commit: usize,
    staging: Vec<f32>,
    filled: usize,
    staged: bool,
    next: u64,
    gain: f32,
    fading: i8,
}

impl OutputWriter {
    pub fn new(writer: SamplesWriter, commit: usize) -> Self {
        let channels = writer.channels;
        Self {
            writer,
            commit,
            staging: vec![0.0; channels * commit],
            filled: 0,
            staged: false,
            next: 0,
            gain: 0.0,
            fading: 1,
        }
    }

    pub fn channels(&self) -> usize {
        self.writer.channels
    }
}

/// Upper bucket bounds (µs) of the block-time histogram.
pub const BLOCK_BUCKETS_US: [u64; 10] =
    [50, 100, 200, 500, 1_000, 1_500, 2_000, 3_000, 5_000, 10_000];

/// Counters one input shares with the control side.
#[derive(Default)]
pub struct InputCounters {
    /// The flow's head index seen at the last read.
    pub head_index: AtomicU64,
    /// Samples per channel read into the input buffer.
    pub read_samples: AtomicU64,
    /// Samples the source had not written by the read (§4.2 "late"): too early, or the wait timed out.
    pub late_samples: AtomicU64,
    /// Samples the ring had already overwritten (too late): the reader fell behind.
    pub overrun_samples: AtomicU64,
    /// Samples replaced by silence after the input had started (late source, ring overrun, other errors).
    pub missing_samples: AtomicU64,
    /// The extra delay the DSP thread currently applies, in samples.
    pub delay_samples: AtomicU32,
    /// The last block was read.
    pub ok: AtomicBool,
    /// Channels of the current flow.
    pub channels: AtomicU32,
}

/// Counters of one output slot.
#[derive(Default)]
pub struct OutputCounters {
    /// Blocks staged for this output.
    pub blocks: AtomicU64,
    pub commits: AtomicU64,
    pub errors: AtomicU64,
    /// TAI time of the last commit minus the time of its last sample, in µs.
    pub commit_lag_us: AtomicU64,
}

/// Counters of the DSP thread.
pub struct EngineCounters {
    pub blocks: AtomicU64,
    pub deadline_misses: AtomicU64,
    pub block_time_buckets: [AtomicU64; BLOCK_BUCKETS_US.len() + 1],
    pub block_time_sum_ns: AtomicU64,
    /// Block time / block period of the last block, in parts per million.
    pub load_ppm: AtomicU32,
    pub output_blocks: AtomicU64,
    pub plan_swaps: AtomicU64,
    /// Meter records the meter ring had no room for.
    pub meter_drops: AtomicU64,
    /// VAD tap records the VAD ring had no room for.
    pub vad_drops: AtomicU64,
    pub running: AtomicBool,
    /// The DSP thread runs `SCHED_FIFO`.
    pub realtime: AtomicBool,
    pub stop: AtomicBool,
    pub inputs: Vec<InputCounters>,
    pub outputs: Vec<OutputCounters>,
    pub talk: Vec<talk::TalkCounters>,
}

impl EngineCounters {
    fn new(inputs: usize) -> Self {
        Self {
            blocks: AtomicU64::new(0),
            deadline_misses: AtomicU64::new(0),
            block_time_buckets: Default::default(),
            block_time_sum_ns: AtomicU64::new(0),
            load_ppm: AtomicU32::new(0),
            output_blocks: AtomicU64::new(0),
            plan_swaps: AtomicU64::new(0),
            meter_drops: AtomicU64::new(0),
            vad_drops: AtomicU64::new(0),
            running: AtomicBool::new(false),
            realtime: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            inputs: (0..inputs).map(|_| InputCounters::default()).collect(),
            outputs: (0..MAX_OUTPUTS)
                .map(|_| OutputCounters::default())
                .collect(),
            talk: (0..talk::MAX_TALK)
                .map(|_| talk::TalkCounters::default())
                .collect(),
        }
    }

    pub fn load_ratio(&self) -> f64 {
        f64::from(self.load_ppm.load(Ordering::Relaxed)) / 1e6
    }
}

pub struct EngineConfig {
    pub inputs: usize,
    pub block: u32,
    pub read_offset: u32,
    /// How long (samples) the arrival lag of blocks without a commit is held for
    /// the next commit; a head that stands still longer is a stall (§4.2).
    pub lag_window: u32,
    pub rt_priority: Option<i32>,
    pub cpu: Option<usize>,
}

/// The control side's handle on the engine.
pub struct Engine {
    pub counters: Arc<EngineCounters>,
    pub commands: rtrb::Producer<Command>,
    pub returned: rtrb::Consumer<Returned>,
    /// Per input: arrival lag samples (in 48 kHz samples), one per read block
    /// (see `dsp::ArrivalTracker`).
    pub lags: Vec<rtrb::Consumer<u32>>,
    /// The analysed signal: per block, channel 1 then channel 2 (a copy of
    /// channel 1 for mono).
    pub analysis: rtrb::Consumer<f32>,
    /// The VAD taps: every block of a tapped signal, all its channels, as a
    /// record (`meter::write`: plan generation, tap slot, channels, length).
    pub vad: rtrb::Consumer<f32>,
    pub scheduling: Arc<std::sync::Mutex<String>>,
    thread: Option<std::thread::JoinHandle<()>>,
    /// The meter thread (`meter`) and how it learns a new plan's meter bank.
    meter_banks: std::sync::mpsc::Sender<(u32, Arc<plan::MeterBank>)>,
    meter_stop: Arc<AtomicBool>,
    meter_thread: Option<std::thread::JoinHandle<()>>,
}

/// One second of two channels.
pub const ANALYSIS_RING: usize = 2 * 48_000;

/// Room of an input's arrival-lag ring: the held blocks of a whole window
/// (pushed at once on the next commit) plus two control ticks (100 ms) of blocks.
fn lag_ring(block: u32, lag_window: u32) -> usize {
    let block = block.max(1);
    (lag_window.div_ceil(block) + 2 * 4800u32.div_ceil(block)).max(1024) as usize
}

impl Engine {
    /// Start the DSP thread.
    pub fn start(config: EngineConfig) -> std::io::Result<Self> {
        let counters = Arc::new(EngineCounters::new(config.inputs));
        let (commands, command_rx) = rtrb::RingBuffer::new(4096);
        let (returned_tx, returned) = rtrb::RingBuffer::new(4096);
        let mut lag_tx = Vec::new();
        let mut lags = Vec::new();
        for _ in 0..config.inputs {
            let (tx, rx) = rtrb::RingBuffer::new(lag_ring(config.block, config.lag_window));
            lag_tx.push(tx);
            lags.push(rx);
        }
        let scheduling = Arc::new(std::sync::Mutex::new(String::new()));
        let (analysis_tx, analysis) = rtrb::RingBuffer::new(ANALYSIS_RING);
        let (vad_tx, vad) = rtrb::RingBuffer::new(VAD_RING);
        let (meter_tx, meter_rx) = rtrb::RingBuffer::new(meter::METER_RING);
        let (meter_banks, banks_rx) = std::sync::mpsc::channel();
        let meter_stop = Arc::new(AtomicBool::new(false));
        let meter_thread = meter::spawn(meter_rx, banks_rx, meter_stop.clone())?;
        let thread_counters = counters.clone();
        let thread_scheduling = scheduling.clone();
        let thread = std::thread::Builder::new()
            .name("dsp".into())
            .spawn(move || {
                dsp::run(
                    config,
                    thread_counters,
                    command_rx,
                    returned_tx,
                    lag_tx,
                    dsp::Taps {
                        meters: meter_tx,
                        analysis: analysis_tx,
                        vad: vad_tx,
                    },
                    thread_scheduling,
                )
            })?;
        Ok(Self {
            counters,
            commands,
            returned,
            lags,
            analysis,
            vad,
            scheduling,
            thread: Some(thread),
            meter_banks,
            meter_stop,
            meter_thread: Some(meter_thread),
        })
    }

    /// Tell the meter thread plan `generation`'s meter bank; before the plan is
    /// sent, so that its first records find it.
    pub fn meter_plan(&mut self, generation: u32, bank: Arc<plan::MeterBank>) {
        let _ = self.meter_banks.send((generation, bank));
    }

    /// Send a command; the ring is large enough that a full ring is a bug.
    pub fn send(&mut self, command: Command) {
        if self.commands.push(command).is_err() {
            tracing::error!("DSP command ring full; command dropped");
        }
    }

    /// Ramp node `node` to `values` over `ramp` samples.
    pub fn param(&mut self, node: NodeId, values: &[f32], ramp: u32) {
        let mut v = [0.0; MAX_NODE_VALUES];
        let len = values.len().min(MAX_NODE_VALUES);
        v[..len].copy_from_slice(&values[..len]);
        self.send(Command::Param {
            node,
            values: v,
            len: len as u8,
            ramp,
        });
    }

    /// Ramp parameter `index` of processor `node` to `value` over `ramp` samples.
    pub fn proc_param(&mut self, node: NodeId, index: usize, value: f32, ramp: u32) {
        self.send(Command::ProcParam {
            node,
            index: index as u16,
            value,
            ramp,
        });
    }

    /// Free what the DSP thread handed back.
    pub fn collect(&mut self) {
        while let Ok(item) = self.returned.pop() {
            drop(item);
        }
    }

    /// Fade the outputs out, stop the thread and wait for it (G8).
    pub fn stop(&mut self) {
        self.counters.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        self.meter_stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.meter_thread.take() {
            let _ = thread.join();
        }
        self.collect();
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::lag_ring;

    #[test]
    fn the_lag_ring_holds_a_window_and_two_ticks() {
        // 48-sample blocks, 2 + 3600 ms: 3602 held blocks + 2 × 100 ticks.
        assert_eq!(lag_ring(48, 96 + 172_800), 3602 + 200);
        // The defaults (96, 2 + 150 ms) fit the old 1024.
        assert_eq!(lag_ring(96, 96 + 7200), 1024);
        assert!(lag_ring(96, 96 + 7200) >= 76 + 100);
    }
}
