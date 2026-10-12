//! Talkback slots (§10.2): mono audio a decoder thread pushes into a ring (a
//! browser's microphone, received by WHIP), played by the DSP thread with a
//! jitter buffer. A slot is engine input `inputs + slot`, so a plan reads it like
//! an input channel.
//!
//! The buffer starts once `target` samples are queued beyond the block; an
//! underrun plays silence, raises the target by 10 ms (at most 100 ms) and starts
//! again; more than 50 ms beyond the target is dropped. The queued samples are
//! the slot's measured delay.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

/// Talkback slots of the DSP thread.
pub const MAX_TALK: usize = 16;
/// The jitter buffer: start, step, most, and the excess that is dropped.
pub const TARGET_START: usize = 960;
pub const TARGET_STEP: usize = 480;
pub const TARGET_MAX: usize = 4_800;
pub const EXCESS: usize = 2_400;
/// The fade-in after a (re)start.
const FADE: f32 = 1.0 / 96.0;

#[derive(Default)]
pub struct TalkCounters {
    pub playing: AtomicBool,
    /// Samples queued after the last block (a running mean ×16).
    pub fill_x16: AtomicU32,
    pub target: AtomicU32,
    pub underruns: AtomicU64,
    pub dropped: AtomicU64,
    pub blocks: AtomicU64,
}

impl TalkCounters {
    /// The jitter buffer's mean delay in ms.
    pub fn delay_ms(&self) -> f64 {
        f64::from(self.fill_x16.load(Ordering::Relaxed)) / 16.0 / 48.0
    }
}

pub(crate) struct TalkIn {
    pub ring: Option<Box<rtrb::Consumer<f32>>>,
    pub buffer: Vec<f32>,
    primed: bool,
    target: usize,
    gain: f32,
    fill_x16: u32,
}

impl TalkIn {
    pub fn new(block: usize) -> Self {
        Self {
            ring: None,
            buffer: vec![0.0; block],
            primed: false,
            target: TARGET_START,
            gain: 0.0,
            fill_x16: 0,
        }
    }

    pub fn reset(&mut self) {
        self.primed = false;
        self.target = TARGET_START;
        self.gain = 0.0;
        self.fill_x16 = 0;
        self.buffer.fill(0.0);
    }

    /// Fill the block buffer from the ring.
    pub fn read(&mut self, block: usize, c: &TalkCounters) {
        let Some(ring) = self.ring.as_mut() else {
            self.buffer.fill(0.0);
            c.playing.store(false, Ordering::Relaxed);
            return;
        };
        let avail = ring.slots();
        if !self.primed {
            if avail < self.target + block {
                self.buffer.fill(0.0);
                c.playing.store(false, Ordering::Relaxed);
                return;
            }
            self.primed = true;
            self.gain = 0.0;
        }
        if avail < block {
            self.buffer.fill(0.0);
            self.primed = false;
            self.target = (self.target + TARGET_STEP).min(TARGET_MAX);
            c.underruns.fetch_add(1, Ordering::Relaxed);
            c.target.store(self.target as u32, Ordering::Relaxed);
            c.playing.store(false, Ordering::Relaxed);
            return;
        }
        let keep = self.target + block + EXCESS;
        if avail > keep {
            let n = avail - (self.target + block);
            if let Ok(chunk) = ring.read_chunk(n) {
                chunk.commit_all();
                c.dropped.fetch_add(n as u64, Ordering::Relaxed);
            }
        }
        if let Ok(chunk) = ring.read_chunk(block) {
            let (a, b) = chunk.as_slices();
            self.buffer[..a.len()].copy_from_slice(a);
            self.buffer[a.len()..a.len() + b.len()].copy_from_slice(b);
            chunk.commit_all();
        }
        if self.gain < 1.0 {
            for x in &mut self.buffer {
                self.gain = (self.gain + FADE).min(1.0);
                *x *= self.gain;
            }
        }
        let queued = ring.slots() as u32;
        self.fill_x16 = self.fill_x16 - self.fill_x16 / 16 + queued;
        c.fill_x16.store(self.fill_x16, Ordering::Relaxed);
        c.target.store(self.target as u32, Ordering::Relaxed);
        c.blocks.fetch_add(1, Ordering::Relaxed);
        c.playing.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jitter_buffer() {
        let (mut tx, rx) = rtrb::RingBuffer::new(48_000);
        let mut t = TalkIn::new(96);
        t.ring = Some(Box::new(rx));
        let c = TalkCounters::default();
        let push = |tx: &mut rtrb::Producer<f32>, n: usize| {
            for _ in 0..n {
                tx.push(0.5).unwrap();
            }
        };
        // Silent until the target and a block are queued.
        push(&mut tx, 960);
        t.read(96, &c);
        assert!(!c.playing.load(Ordering::Relaxed) && t.buffer[0] == 0.0);
        push(&mut tx, 96);
        t.read(96, &c);
        assert!(c.playing.load(Ordering::Relaxed));
        assert!(t.buffer[0] < 0.01 && t.buffer[95] == 0.5, "faded in");
        // Run dry (1056 queued: 11 blocks): an underrun, a longer target.
        for _ in 0..11 {
            t.read(96, &c);
        }
        assert_eq!(c.underruns.load(Ordering::Relaxed), 1);
        assert_eq!(c.target.load(Ordering::Relaxed), 1440);
        // Far too much queued: dropped down to the target.
        push(&mut tx, 1440 + 96);
        t.read(96, &c);
        push(&mut tx, 20_000);
        t.read(96, &c);
        assert!(c.dropped.load(Ordering::Relaxed) > 15_000);
        assert_eq!(t.ring.as_ref().unwrap().slots(), 1440);
    }
}
