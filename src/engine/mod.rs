//! The DSP engine (§2): one real-time thread on the TAI block clock.
//!
//! Block n covers output samples [n·B, (n+1)·B). The thread sleeps until the TAI
//! time of sample (n+1)·B, then reads every routed input's block ending at
//! (n+1)·B − (read offset + the input's extra delay). The control side talks to it
//! only through lock-free single-producer/single-consumer rings and atomics; the
//! thread never allocates, locks or does I/O other than MXL reads.

mod dsp;

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use crate::mxlio::SamplesReader;

/// Largest channel count of an input flow (§4.2).
pub const MAX_CHANNELS: usize = 64;
/// Length of every fade in samples: the parameter ramp of §2 (10 ms).
pub const FADE_SAMPLES: u32 = 480;

/// A message from the control side to the DSP thread.
pub enum Command {
    /// Read `reader` for input `index` from the next block on.
    SetInput {
        index: usize,
        reader: Box<SamplesReader>,
        channels: usize,
    },
    /// Stop reading input `index` (master_enable false, unrouted, flow gone).
    ClearInput { index: usize },
    /// Change input `index`'s extra delay, fading out and in.
    SetDelay { index: usize, samples: u32 },
}

/// A message from the DSP thread back: things it must not free itself.
pub enum Returned {
    Reader(Box<SamplesReader>),
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
    /// Samples that were not there yet at the read deadline (§4.2 "late").
    pub late_samples: AtomicU64,
    /// Samples replaced by silence after the input had started (late, too old, errors).
    pub missing_samples: AtomicU64,
    /// The extra delay the DSP thread currently applies, in samples.
    pub delay_samples: AtomicU32,
    /// The last block was read.
    pub ok: AtomicBool,
    /// Channels of the current flow.
    pub channels: AtomicU32,
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
    pub running: AtomicBool,
    pub stop: AtomicBool,
    pub inputs: Vec<InputCounters>,
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
            running: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            inputs: (0..inputs).map(|_| InputCounters::default()).collect(),
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
    pub rt_priority: Option<i32>,
    pub cpu: Option<usize>,
}

/// The control side's handle on the engine.
pub struct Engine {
    pub counters: Arc<EngineCounters>,
    pub commands: rtrb::Producer<Command>,
    pub returned: rtrb::Consumer<Returned>,
    /// Per input: arrival lag samples (in 48 kHz samples), one per read block.
    pub lags: Vec<rtrb::Consumer<u32>>,
    pub scheduling: Arc<std::sync::Mutex<String>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Engine {
    /// Start the DSP thread.
    pub fn start(config: EngineConfig) -> std::io::Result<Self> {
        let counters = Arc::new(EngineCounters::new(config.inputs));
        let (commands, command_rx) = rtrb::RingBuffer::new(1024);
        let (returned_tx, returned) = rtrb::RingBuffer::new(1024);
        let mut lag_tx = Vec::new();
        let mut lags = Vec::new();
        for _ in 0..config.inputs {
            let (tx, rx) = rtrb::RingBuffer::new(1024);
            lag_tx.push(tx);
            lags.push(rx);
        }
        let scheduling = Arc::new(std::sync::Mutex::new(String::new()));
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
                    thread_scheduling,
                )
            })?;
        Ok(Self {
            counters,
            commands,
            returned,
            lags,
            scheduling,
            thread: Some(thread),
        })
    }

    /// Send a command; the ring is large enough that a full ring is a bug.
    pub fn send(&mut self, command: Command) {
        if self.commands.push(command).is_err() {
            tracing::error!("DSP command ring full; command dropped");
        }
    }

    /// Stop the thread and wait for it.
    pub fn stop(&mut self) {
        self.counters.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        while self.returned.pop().is_ok() {}
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stop();
    }
}
