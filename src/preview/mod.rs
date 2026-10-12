//! Monitors in the browser (§10.1, Appendix B.1): a monitor someone listens to is
//! copied by the DSP thread into a ring, encoded as Opus on a thread of its own
//! and published over RTSP to MediaMTX (the built-in one or the platform's), which
//! serves it by WHEP as `<PREVIEW_PATH_PREFIX>/<monitor id>`.
//!
//! Only streams someone listens to are encoded: a client's listen request (the
//! UI repeats it while playing) or, with `MEDIAMTX_API_URL`, readers on MediaMTX
//! keep a stream for [`GRACE`]; then it stops.

pub mod mediamtx;
pub mod rtsp;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::config::Config;
use crate::engine::{Command, Engine, MAX_STREAMS};

/// A stream without listen requests or readers stops after this long.
pub const GRACE: Duration = Duration::from_secs(10);
/// Opus: 10 ms frames, stereo, this bit rate.
pub const FRAME: usize = 480;
pub const BITRATE: i32 = 128_000;
/// A personal monitor nobody listened to for this long is removed.
pub const PERSONAL_IDLE: Duration = Duration::from_secs(600);
/// Reconnect to MediaMTX after this long (B.1: every ~2 s).
pub const RECONNECT: Duration = Duration::from_secs(2);

/// Publishing state of a stream.
pub const IDLE: u8 = 0;
pub const CONNECTING: u8 = 1;
pub const PUBLISHING: u8 = 2;

#[derive(Default)]
pub struct StreamState {
    pub state: AtomicU8,
    pub frames: AtomicU64,
    pub connects: AtomicU64,
    pub error: Mutex<String>,
}

struct Active {
    slot: usize,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    state: Arc<StreamState>,
}

impl Drop for Active {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// One stream's status for the API.
#[derive(Debug, Clone, Serialize)]
pub struct StreamStatus {
    pub id: String,
    pub path: String,
    pub state: &'static str,
    pub frames: u64,
    pub connects: u64,
    pub error: String,
    pub readers: Option<u32>,
    pub listened_s_ago: Option<f64>,
}

pub struct Streams {
    publish_base: String,
    prefix: String,
    block: usize,
    active: BTreeMap<String, Active>,
    listened: BTreeMap<String, Instant>,
    readers: BTreeMap<String, u32>,
}

impl Streams {
    pub fn new(config: &Config) -> Self {
        let publish_base = config
            .preview_publish_url
            .clone()
            .unwrap_or_else(|| format!("rtsp://127.0.0.1:{}", config.mediamtx_rtsp_port));
        Self {
            publish_base,
            prefix: config.preview_path_prefix.clone(),
            block: config.block_samples as usize,
            active: BTreeMap::new(),
            listened: BTreeMap::new(),
            readers: BTreeMap::new(),
        }
    }

    /// When `id` was last listened to.
    pub fn last_heard(&self, id: &str) -> Option<Instant> {
        self.listened.get(id).copied()
    }

    pub fn path(&self, id: &str) -> String {
        format!("{}/{id}", self.prefix)
    }

    /// Stream slot of every active stream, for the compiler.
    pub fn slots(&self) -> std::collections::HashMap<String, usize> {
        self.active
            .iter()
            .map(|(id, a)| (id.clone(), a.slot))
            .collect()
    }

    /// Someone listens to `id`. True when the stream is new (the plan needs it).
    pub fn listen(&mut self, engine: &mut Engine, id: &str) -> Result<bool, String> {
        self.listened.insert(id.to_string(), Instant::now());
        if self.active.contains_key(id) {
            return Ok(false);
        }
        let used: Vec<usize> = self.active.values().map(|a| a.slot).collect();
        let slot = (0..MAX_STREAMS)
            .find(|s| !used.contains(s))
            .ok_or_else(|| format!("at most {MAX_STREAMS} streams at once"))?;
        let (producer, consumer) = rtrb::RingBuffer::new(2 * crate::config::SAMPLE_RATE as usize);
        engine.send(Command::SetStream {
            slot,
            ring: Box::new(producer),
        });
        let stop = Arc::new(AtomicBool::new(false));
        let state = Arc::new(StreamState::default());
        let url = format!("{}/{}", self.publish_base, self.path(id));
        let (block, s, st) = (self.block, stop.clone(), state.clone());
        let thread = std::thread::Builder::new()
            .name(format!("stream-{id}"))
            .spawn(move || encode(consumer, block, &url, &s, &st))
            .map_err(|e| e.to_string())?;
        self.active.insert(
            id.to_string(),
            Active {
                slot,
                stop,
                thread: Some(thread),
                state,
            },
        );
        tracing::info!(stream = %id, slot, "stream started");
        Ok(true)
    }

    /// MediaMTX's reader counts per stream id (from its API).
    pub fn set_readers(&mut self, readers: BTreeMap<String, u32>) {
        self.readers = readers;
    }

    /// Stop the streams nobody listened to within [`GRACE`] and that have no
    /// readers, and those whose monitor is gone (`exists`). True when any stopped.
    pub fn expire(&mut self, engine: &mut Engine, exists: impl Fn(&str) -> bool) -> bool {
        let now = Instant::now();
        let gone: Vec<String> = self
            .active
            .keys()
            .filter(|id| {
                let heard = self.listened.get(*id).is_some_and(|t| now - *t < GRACE);
                let read = self.readers.get(*id).is_some_and(|n| *n > 0);
                !exists(id) || !(heard || read)
            })
            .cloned()
            .collect();
        for id in &gone {
            if let Some(a) = self.active.remove(id) {
                engine.send(Command::ClearStream { slot: a.slot });
                drop(a);
                tracing::info!(stream = %id, "stream stopped");
            }
        }
        !gone.is_empty()
    }

    pub fn status(&self, ids: &[String]) -> Vec<StreamStatus> {
        let now = Instant::now();
        ids.iter()
            .map(|id| {
                let a = self.active.get(id);
                let state = a.map_or(IDLE, |a| a.state.state.load(Ordering::Relaxed));
                StreamStatus {
                    id: id.clone(),
                    path: self.path(id),
                    state: match state {
                        PUBLISHING => "publishing",
                        CONNECTING => "connecting",
                        _ => "idle",
                    },
                    frames: a.map_or(0, |a| a.state.frames.load(Ordering::Relaxed)),
                    connects: a.map_or(0, |a| a.state.connects.load(Ordering::Relaxed)),
                    error: a
                        .map(|a| a.state.error.lock().map(|e| e.clone()).unwrap_or_default())
                        .unwrap_or_default(),
                    readers: self.readers.get(id).copied(),
                    listened_s_ago: self.listened.get(id).map(|t| (now - *t).as_secs_f64()),
                }
            })
            .collect()
    }
}

/// The encoder thread: blocks from `ring` (per block channel 1, channel 2) →
/// 10 ms Opus frames → RTP over RTSP to `url`, reconnecting every [`RECONNECT`].
fn encode(
    mut ring: rtrb::Consumer<f32>,
    block: usize,
    url: &str,
    stop: &AtomicBool,
    state: &StreamState,
) {
    let set_error = |e: String| {
        if let Ok(mut s) = state.error.lock() {
            *s = e;
        }
    };
    let mut encoder =
        match opus::Encoder::new(48_000, opus::Channels::Stereo, opus::Application::LowDelay) {
            Ok(e) => e,
            Err(e) => {
                set_error(format!("opus: {e}"));
                return;
            }
        };
    let _ = encoder.set_bitrate(opus::Bitrate::Bits(BITRATE));
    let mut publisher: Option<rtsp::Publisher> = None;
    let mut last_try: Option<Instant> = None;
    let mut pcm: Vec<f32> = Vec::with_capacity(4 * FRAME);
    let mut packet = vec![0u8; 4000];
    let mut timestamp: u32 = 0;
    while !stop.load(Ordering::Relaxed) {
        if publisher.is_none() && last_try.is_none_or(|t| t.elapsed() >= RECONNECT) {
            last_try = Some(Instant::now());
            state.state.store(CONNECTING, Ordering::Relaxed);
            match rtsp::Publisher::connect(url, Duration::from_secs(2)) {
                Ok(p) => {
                    publisher = Some(p);
                    state.connects.fetch_add(1, Ordering::Relaxed);
                    state.state.store(PUBLISHING, Ordering::Relaxed);
                    set_error(String::new());
                }
                Err(e) => set_error(format!("{url}: {e}")),
            }
        }
        let mut got = false;
        while ring.slots() >= 2 * block {
            let Ok(chunk) = ring.read_chunk(2 * block) else {
                break;
            };
            let (a, b) = chunk.as_slices();
            let both: Vec<f32> = a.iter().chain(b.iter()).copied().collect();
            chunk.commit_all();
            let (l, r) = both.split_at(block);
            for k in 0..block {
                pcm.push(l[k]);
                pcm.push(r[k]);
            }
            got = true;
        }
        while pcm.len() >= 2 * FRAME {
            match encoder.encode_float(&pcm[..2 * FRAME], &mut packet) {
                Ok(n) => {
                    if let Some(p) = publisher.as_mut() {
                        if let Err(e) = p.send(&packet[..n], timestamp) {
                            set_error(format!("{url}: {e}"));
                            publisher = None;
                            state.state.store(CONNECTING, Ordering::Relaxed);
                        } else {
                            state.frames.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
                Err(e) => set_error(format!("opus: {e}")),
            }
            timestamp = timestamp.wrapping_add(FRAME as u32);
            pcm.drain(..2 * FRAME);
        }
        if !got {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    state.state.store(IDLE, Ordering::Relaxed);
}
