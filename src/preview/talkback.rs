//! Browser talkback (§10.2, Appendix B.1): a browser publishes its microphone by
//! WHIP to MediaMTX as `<PREVIEW_PATH_PREFIX>/talkback-<client>`; the mixer reads
//! it there over RTSP, decodes the Opus to mono and plays it in an engine
//! talkback slot as the mic `browser-<n>`. A browser repeats its registration
//! while its microphone is on; [`IDLE`] after the last, the mic is removed.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;

use super::rtsp;
use super::{CONNECTING, IDLE as STATE_IDLE, PUBLISHING as RECEIVING, RECONNECT};
use crate::config::Config;
use crate::engine::talk::MAX_TALK;
use crate::engine::{Command, Engine};
use crate::mix::talkback::BROWSER_PREFIX;

/// A browser mic without a registration for this long is removed.
pub const IDLE: Duration = Duration::from_secs(30);
/// Most concealed frames after a gap in the sequence numbers.
const MAX_CONCEALED: u16 = 5;

#[derive(Default)]
pub struct MicState {
    pub state: AtomicU8,
    pub packets: AtomicU64,
    pub lost: AtomicU64,
    pub connects: AtomicU64,
    pub error: Mutex<String>,
}

struct Active {
    client: String,
    slot: usize,
    path: String,
    stop: Arc<AtomicBool>,
    state: Arc<MicState>,
    seen: Instant,
}

impl Drop for Active {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct BrowserMicStatus {
    pub id: String,
    pub client: String,
    pub path: String,
    pub state: &'static str,
    pub packets: u64,
    pub lost: u64,
    pub connects: u64,
    pub error: String,
    /// The jitter buffer's delay, ms.
    pub delay_ms: f64,
    pub underruns: u64,
    pub seen_s_ago: f64,
}

pub struct Talkers {
    read_base: String,
    prefix: String,
    active: BTreeMap<String, Active>,
}

/// A client id: 1 to 32 characters a-z, 0-9 and '-'.
pub fn valid_client(c: &str) -> bool {
    !c.is_empty()
        && c.len() <= 32
        && c.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

impl Talkers {
    pub fn new(config: &Config) -> Self {
        let read_base = config
            .preview_publish_url
            .clone()
            .unwrap_or_else(|| format!("rtsp://127.0.0.1:{}", config.mediamtx_rtsp_port));
        Self {
            read_base,
            prefix: config.preview_path_prefix.clone(),
            active: BTreeMap::new(),
        }
    }

    /// The MediaMTX path of `client`'s microphone.
    pub fn path(&self, client: &str) -> String {
        format!("{}/talkback-{client}", self.prefix)
    }

    /// The engine slot of every browser mic, for the compiler.
    pub fn slots(&self) -> BTreeMap<String, usize> {
        self.active
            .iter()
            .map(|(id, a)| (id.clone(), a.slot))
            .collect()
    }

    /// `client` has its microphone on: its mic id, and true when it is new (the
    /// plan needs it).
    pub fn register(
        &mut self,
        engine: &mut Engine,
        client: &str,
    ) -> Result<(String, bool), String> {
        if !valid_client(client) {
            return Err("a client id has 1 to 32 characters a-z, 0-9 and '-'".into());
        }
        if let Some((id, a)) = self.active.iter_mut().find(|(_, a)| a.client == client) {
            a.seen = Instant::now();
            return Ok((id.clone(), false));
        }
        let used: Vec<usize> = self.active.values().map(|a| a.slot).collect();
        let slot = (0..MAX_TALK)
            .find(|s| !used.contains(s))
            .ok_or_else(|| format!("at most {MAX_TALK} browser microphones at once"))?;
        let id = (1..)
            .map(|n| format!("{BROWSER_PREFIX}{n}"))
            .find(|id| !self.active.contains_key(id))
            .expect("unbounded");
        let (producer, consumer) = rtrb::RingBuffer::new(crate::config::SAMPLE_RATE as usize);
        engine.send(Command::SetTalk {
            slot,
            ring: Box::new(consumer),
        });
        let stop = Arc::new(AtomicBool::new(false));
        let state = Arc::new(MicState::default());
        let path = self.path(client);
        let url = format!("{}/{path}", self.read_base);
        let (s, st) = (stop.clone(), state.clone());
        std::thread::Builder::new()
            .name(format!("talk-{id}"))
            .spawn(move || receive(producer, &url, &s, &st))
            .map_err(|e| e.to_string())?;
        tracing::info!(mic = %id, client, slot, "browser talkback mic added");
        self.active.insert(
            id.clone(),
            Active {
                client: client.to_string(),
                slot,
                path,
                stop,
                state,
                seen: Instant::now(),
            },
        );
        Ok((id, true))
    }

    /// Remove mic `id`; true when it existed.
    pub fn remove(&mut self, engine: &mut Engine, id: &str) -> bool {
        match self.active.remove(id) {
            Some(a) => {
                engine.send(Command::ClearTalk { slot: a.slot });
                tracing::info!(mic = %id, "browser talkback mic removed");
                true
            }
            None => false,
        }
    }

    /// Remove the mics not registered within [`IDLE`]; true when any went.
    pub fn expire(&mut self, engine: &mut Engine) -> bool {
        let gone: Vec<String> = self
            .active
            .iter()
            .filter(|(_, a)| a.seen.elapsed() > IDLE)
            .map(|(id, _)| id.clone())
            .collect();
        for id in &gone {
            self.remove(engine, id);
        }
        !gone.is_empty()
    }

    pub fn status(&self, engine: &Engine) -> Vec<BrowserMicStatus> {
        self.active
            .iter()
            .map(|(id, a)| {
                let c = &engine.counters.talk[a.slot];
                BrowserMicStatus {
                    id: id.clone(),
                    client: a.client.clone(),
                    path: a.path.clone(),
                    state: match a.state.state.load(Ordering::Relaxed) {
                        RECEIVING => "receiving",
                        CONNECTING => "connecting",
                        _ => "idle",
                    },
                    packets: a.state.packets.load(Ordering::Relaxed),
                    lost: a.state.lost.load(Ordering::Relaxed),
                    connects: a.state.connects.load(Ordering::Relaxed),
                    error: a.state.error.lock().map(|e| e.clone()).unwrap_or_default(),
                    delay_ms: c.delay_ms(),
                    underruns: c.underruns.load(Ordering::Relaxed),
                    seen_s_ago: a.seen.elapsed().as_secs_f64(),
                }
            })
            .collect()
    }
}

/// Read `url` until `stop`, decode, and push mono samples into `ring`;
/// reconnect after [`RECONNECT`] (the browser may not have published yet).
fn receive(mut ring: rtrb::Producer<f32>, url: &str, stop: &AtomicBool, state: &MicState) {
    let set_error = |e: String| {
        if let Ok(mut s) = state.error.lock() {
            *s = e;
        }
    };
    let mut buf = Vec::with_capacity(1500);
    // 120 ms of stereo, the longest Opus frame.
    let mut pcm = vec![0f32; 2 * 5760];
    while !stop.load(Ordering::Relaxed) {
        state.state.store(CONNECTING, Ordering::Relaxed);
        let mut reader = match rtsp::Reader::connect(url, Duration::from_secs(1)) {
            Ok(r) => r,
            Err(e) => {
                set_error(format!("{url}: {e}"));
                let until = Instant::now() + RECONNECT;
                while Instant::now() < until && !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(50));
                }
                continue;
            }
        };
        let channels = if reader.channels >= 2 {
            opus::Channels::Stereo
        } else {
            opus::Channels::Mono
        };
        let n = if reader.channels >= 2 { 2 } else { 1 };
        let mut decoder = match opus::Decoder::new(48_000, channels) {
            Ok(d) => d,
            Err(e) => {
                set_error(format!("opus: {e}"));
                return;
            }
        };
        state.connects.fetch_add(1, Ordering::Relaxed);
        state.state.store(RECEIVING, Ordering::Relaxed);
        set_error(String::new());
        let mut last: Option<u16> = None;
        let mut frame = 960;
        let push = |ring: &mut rtrb::Producer<f32>, pcm: &[f32], samples: usize| {
            if ring.slots() < samples {
                return;
            }
            if let Ok(chunk) = ring.write_chunk_uninit(samples) {
                chunk.fill_from_iter((0..samples).map(|k| {
                    if n == 2 {
                        0.5 * (pcm[2 * k] + pcm[2 * k + 1])
                    } else {
                        pcm[k]
                    }
                }));
            }
        };
        while !stop.load(Ordering::Relaxed) {
            let seq = match reader.next(&mut buf) {
                Ok(Some(seq)) => seq,
                Ok(None) => continue,
                Err(e) => {
                    set_error(format!("{url}: {e}"));
                    break;
                }
            };
            if let Some(prev) = last {
                let gap = seq.wrapping_sub(prev).wrapping_sub(1);
                if gap > 0 && gap < 1000 {
                    state.lost.fetch_add(u64::from(gap), Ordering::Relaxed);
                    for _ in 0..gap.min(MAX_CONCEALED) {
                        if let Ok(s) = decoder.decode_float(&[], &mut pcm[..n * frame], false) {
                            push(&mut ring, &pcm, s);
                        }
                    }
                }
            }
            last = Some(seq);
            match decoder.decode_float(&buf, &mut pcm, false) {
                Ok(s) => {
                    frame = s.max(1);
                    push(&mut ring, &pcm, s);
                    state.packets.fetch_add(1, Ordering::Relaxed);
                }
                Err(e) => set_error(format!("opus: {e}")),
            }
        }
    }
    state.state.store(STATE_IDLE, Ordering::Relaxed);
}
