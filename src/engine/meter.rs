//! The signal meters (§7) off the DSP thread. Its `Meter` steps write each
//! metered block into one ring as a record: the plan's generation, the first
//! meter slot, the channel count, the samples per channel (four header values,
//! as f32 bits), then the samples, channel after channel. A record is written
//! whole or, when the ring lacks room, not at all (`meter_drops`).
//!
//! The meter thread takes the records and computes what the DSP thread used to:
//! peak, energy, true peak (4× oversampling, BS.1770), the PPM detectors and the
//! cross sum for the correlation, into the plan's [`MeterBank`] atomics, which
//! the control side reads as before. True peak and PPM were about a third of the
//! DSP thread's time with every signal metered (the §16 load).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use super::plan::MeterBank;
use crate::dsp::reduce;
use crate::dsp::truepeak::{self, TruePeak, PHASES, TAPS};

/// Values before the samples of a record.
pub const HEADER: usize = 4;
/// The ring: 2^22 values, half a second of 160 metered channels.
pub const METER_RING: usize = 1 << 22;
/// How often the meter thread takes the ring.
const POLL: Duration = Duration::from_millis(2);

/// Plans the meter thread knows (the running one and the one before).
const KEPT: usize = 2;

pub struct Meterer {
    taps: [[f32; TAPS]; PHASES],
    bound: f32,
    plans: Vec<(u32, Arc<MeterBank>, Vec<TruePeak>)>,
}

impl Default for Meterer {
    fn default() -> Self {
        let taps = truepeak::taps();
        Self {
            bound: truepeak::bound(&taps),
            taps,
            plans: Vec::new(),
        }
    }
}

impl Meterer {
    /// The meter bank of plan `generation` (sent before the plan runs).
    pub fn plan(&mut self, generation: u32, bank: Arc<MeterBank>) {
        let n = bank.channels.len();
        self.plans
            .push((generation, bank, vec![TruePeak::default(); n]));
        if self.plans.len() > KEPT {
            self.plans.remove(0);
        }
    }

    pub fn knows(&self, generation: u32) -> bool {
        self.plans.iter().any(|(g, _, _)| *g == generation)
    }

    /// One block of meter `slot`'s channels of plan `generation`.
    pub fn measure(&mut self, generation: u32, slot: usize, chans: &[&[f32]]) {
        let Some((_, bank, tps)) = self.plans.iter_mut().find(|(g, _, _)| *g == generation) else {
            return;
        };
        for (c, x) in chans.iter().enumerate() {
            let (Some(m), Some(tp)) = (bank.channels.get(slot + c), tps.get_mut(slot + c)) else {
                continue;
            };
            let peak = reduce::peak(x);
            // A block that cannot exceed the true peak held since the control side
            // took it is not oversampled; taken in between, it counts with its
            // sample peak.
            let held = f32::from_bits(m.true_peak.load(Ordering::Relaxed));
            let true_peak = tp.run_above(&self.taps, self.bound, x, held);
            m.peak.fetch_max(peak.to_bits(), Ordering::Relaxed);
            m.true_peak
                .fetch_max(true_peak.to_bits(), Ordering::Relaxed);
            let total = f64::from_bits(m.energy.load(Ordering::Relaxed)) + reduce::energy(x);
            m.energy.store(total.to_bits(), Ordering::Relaxed);
            m.samples.fetch_add(x.len() as u64, Ordering::Relaxed);
            let level = [0, 1].map(|k| f32::from_bits(m.ppm[k].load(Ordering::Relaxed)));
            let (level, max) = crate::dsp::ppm::run_pair(&bank.ppm, level, x);
            for k in 0..2 {
                m.ppm[k].store(level[k].to_bits(), Ordering::Relaxed);
                m.ppm_max[k].fetch_max(max[k].to_bits(), Ordering::Relaxed);
            }
        }
        if let ([a, b, ..], Some(m)) = (chans, bank.channels.get(slot)) {
            let cross: f64 = a
                .iter()
                .zip(b.iter())
                .map(|(x, y)| f64::from(*x) * f64::from(*y))
                .sum();
            let total = f64::from_bits(m.cross.load(Ordering::Relaxed)) + cross;
            m.cross.store(total.to_bits(), Ordering::Relaxed);
        }
    }

    /// Take every record in `ring`; `banks` brings the banks of new plans.
    pub fn drain(
        &mut self,
        ring: &mut rtrb::Consumer<f32>,
        banks: &mpsc::Receiver<(u32, Arc<MeterBank>)>,
        scratch: &mut Vec<f32>,
    ) {
        while let Ok((g, bank)) = banks.try_recv() {
            self.plan(g, bank);
        }
        read(ring, scratch, |generation, slot, chans| {
            if !self.knows(generation) {
                while let Ok((g, bank)) = banks.try_recv() {
                    self.plan(g, bank);
                }
            }
            self.measure(generation, slot as usize, chans);
        });
    }
}

/// Take every record in `ring`: `each(generation, slot, channels)`. Allocates
/// nothing once `scratch` has grown to the largest record.
pub fn read(
    ring: &mut rtrb::Consumer<f32>,
    scratch: &mut Vec<f32>,
    mut each: impl FnMut(u32, u32, &[&[f32]]),
) {
    use crate::engine::proc::MAX_CH;
    while ring.slots() >= HEADER {
        let Ok(head) = ring.read_chunk(HEADER) else {
            break;
        };
        let (a, b) = head.as_slices();
        let mut h = [0u32; HEADER];
        for (d, v) in h.iter_mut().zip(a.iter().chain(b)) {
            *d = v.to_bits();
        }
        head.commit_all();
        let [generation, slot, channels, len] = h;
        let (channels, len) = (channels as usize, len as usize);
        // A record is written whole: its samples are there.
        let Ok(body) = ring.read_chunk(channels * len) else {
            break;
        };
        let (a, b) = body.as_slices();
        scratch.clear();
        scratch.extend_from_slice(a);
        scratch.extend_from_slice(b);
        body.commit_all();
        let mut chans: [&[f32]; MAX_CH] = [&[]; MAX_CH];
        let mut n = 0;
        for (c, x) in chans
            .iter_mut()
            .zip(scratch.chunks(len.max(1)).take(channels))
        {
            *c = x;
            n += 1;
        }
        each(generation, slot, &chans[..n]);
    }
}

/// The DSP thread's side: write one record, or count it dropped.
pub fn write(
    ring: &mut rtrb::Producer<f32>,
    drops: &AtomicU64,
    generation: u32,
    slot: u32,
    chans: &[&[f32]],
) {
    let len = chans.first().map_or(0, |c| c.len());
    let Ok(chunk) = ring.write_chunk_uninit(HEADER + chans.len() * len) else {
        drops.fetch_add(1, Ordering::Relaxed);
        return;
    };
    let header = [generation, slot, chans.len() as u32, len as u32].map(f32::from_bits);
    chunk.fill_from_iter(
        header
            .into_iter()
            .chain(chans.iter().flat_map(|c| c.iter().copied())),
    );
}

/// Start the meter thread; it stops when `stop` is set.
pub fn spawn(
    mut ring: rtrb::Consumer<f32>,
    banks: mpsc::Receiver<(u32, Arc<MeterBank>)>,
    stop: Arc<AtomicBool>,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name("meters".into())
        .spawn(move || {
            crate::timing::flush_denormals();
            let mut meterer = Meterer::default();
            let mut scratch = Vec::new();
            while !stop.load(Ordering::Relaxed) {
                meterer.drain(&mut ring, &banks, &mut scratch);
                std::thread::sleep(POLL);
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_through_the_ring() {
        let (mut tx, mut rx) = rtrb::RingBuffer::new(1000);
        let (banks_tx, banks) = mpsc::channel();
        let bank = Arc::new(MeterBank::new(3));
        banks_tx.send((7, bank.clone())).unwrap();
        let drops = AtomicU64::new(0);
        let l = vec![0.5f32; 96];
        let r = vec![-0.25f32; 96];
        write(&mut tx, &drops, 7, 1, &[&l, &r]);
        write(&mut tx, &drops, 9, 0, &[&l]);
        let mut m = Meterer::default();
        m.drain(&mut rx, &banks, &mut Vec::new());
        let c = &bank.channels;
        assert_eq!(f32::from_bits(c[1].peak.load(Ordering::Relaxed)), 0.5);
        assert_eq!(f32::from_bits(c[2].peak.load(Ordering::Relaxed)), 0.25);
        assert_eq!(c[0].samples.load(Ordering::Relaxed), 0, "plan 9 is unknown");
        assert!((f64::from_bits(c[1].cross.load(Ordering::Relaxed)) + 96.0 * 0.125).abs() < 1e-9);
        assert!(f32::from_bits(c[1].true_peak.load(Ordering::Relaxed)) >= 0.5);
        assert!(f32::from_bits(c[1].ppm[0].load(Ordering::Relaxed)) > 0.0);
        // A full ring drops the record and counts it.
        for _ in 0..20 {
            write(&mut tx, &drops, 7, 1, &[&l, &r]);
        }
        assert!(drops.load(Ordering::Relaxed) > 0);
    }
}
