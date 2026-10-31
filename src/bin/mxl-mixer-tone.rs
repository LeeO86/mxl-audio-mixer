//! Test writer: a deterministic tone into an MXL audio flow on the TAI clock.
//!
//! ```sh
//! mxl-mixer-tone --domain /Volumes/mxl/tones --flow <uuid> [--channels 2] [--delay-ms 0] [--block 96]
//! ```
//!
//! Channel c carries 1 kHz × (c + 1) at −20 dBFS; every sample depends only on its
//! index, so a reader can check what it gets. `--delay-ms` writes every block that
//! much late (a mirrored or batched input), `--lead-ms` that much early (a writer
//! ahead of its timestamps, like an AV-sync generator). The domain directory and its
//! `domain_def.json` are created if missing (`--domain-id` sets the id).
//!
//! Commit timing like real writers: `--block 48` commits per packet like the
//! ST 2110 gateway, `--block 960` a 50 Hz video frame of audio at once like the
//! browser source. `--batch N` hands N blocks over together, when the last of
//! them is due (a receiver that delivers a burst of packets). `--late-ms X
//! --every-s P --for-s D` writes X ms later still for D seconds out of every P
//! (on the TAI clock): lag outliers that come and go. `--start-behind-ms X
//! --catch-up R` starts X ms behind and catches up at R times real time (a
//! mirror filling at the start).
//!
//! `--ramp` writes a ramp instead, the same on every channel: sample index i
//! carries (i mod 65536) / 131072 (exact in f32), so a reader can tell from a
//! sample which index it was written at (`mxl-mixer-probe --ramp`).
//!
//! `--wav FILE` writes a recording instead (speech for the VAD tests; the mean of
//! its channels, at 48 kHz, on every channel), followed by `--gap-s` seconds of
//! silence, repeated; sample index i plays position (i + `--offset-s` · 48000)
//! modulo that period, so the recording starts on every multiple of the period
//! in TAI and two writers with different offsets take turns.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use mxl_audio_mixer::mxlio::{self, domain, writer::WriterInstance};
use mxl_audio_mixer::timing::{index_at, sleep_until_tai, tai_ns, time_of};
use uuid::Uuid;

pub fn tone(channel: usize, index: u64) -> f32 {
    let freq = 1_000.0 * (channel as f64 + 1.0);
    let phase = (index % 48) as f64 / 48_000.0;
    (0.1 * (2.0 * std::f64::consts::PI * freq * phase).sin()) as f32
}

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let usage = || -> ! {
        eprintln!("usage: mxl-mixer-tone --domain DIR --flow UUID [--domain-id UUID] [--channels N] [--delay-ms D] [--lead-ms L] [--block B] [--commit C] [--batch N] [--late-ms X --every-s P --for-s D] [--start-behind-ms X --catch-up R] [--wav FILE [--gap-s G] [--offset-s O]] [--ramp]");
        std::process::exit(2)
    };
    let domain_dir = PathBuf::from(arg(&args, "--domain").unwrap_or_else(|| usage()));
    let flow: Uuid = arg(&args, "--flow")
        .and_then(|f| f.parse().ok())
        .unwrap_or_else(|| usage());
    let domain_id: Uuid = arg(&args, "--domain-id")
        .and_then(|f| f.parse().ok())
        .unwrap_or_else(|| Uuid::new_v5(&flow, b"domain"));
    let channels: usize = arg(&args, "--channels")
        .and_then(|v| v.parse().ok())
        .unwrap_or(2);
    let delay_ms: f64 = arg(&args, "--delay-ms")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.0);
    let block: u64 = arg(&args, "--block")
        .and_then(|v| v.parse().ok())
        .unwrap_or(96);
    let commit: u32 = arg(&args, "--commit")
        .and_then(|v| v.parse().ok())
        .unwrap_or(block as u32);
    let batch: u64 = arg(&args, "--batch")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1)
        .max(1);
    let nanos = |name: &str| -> u64 {
        arg(&args, name)
            .and_then(|v| v.parse::<f64>().ok())
            .map_or(0, |s| (s * 1e9).max(0.0) as u64)
    };
    let late_ns = nanos("--late-ms") / 1000;
    let mut behind_ns = nanos("--start-behind-ms") / 1000;
    let catch_up: f64 = arg(&args, "--catch-up")
        .and_then(|v| v.parse().ok())
        .unwrap_or(2.0_f64)
        .max(1.001);
    let (every_ns, for_ns) = (nanos("--every-s"), nanos("--for-s"));
    let seconds = |name: &str| -> u64 {
        arg(&args, name)
            .and_then(|v| v.parse::<f64>().ok())
            .map_or(0, |s| (s * 48_000.0).round().max(0.0) as u64)
    };
    let (gap, shift) = (seconds("--gap-s"), seconds("--offset-s"));
    let recording: Option<Vec<f32>> = arg(&args, "--wav").map(|path| {
        mxl_audio_mixer::wav::Wav::read(std::path::Path::new(&path))
            .map(|w| w.mono_48k())
            .unwrap_or_else(|e| {
                eprintln!("{e}");
                std::process::exit(1)
            })
    });
    let period = recording
        .as_ref()
        .map_or(1, |r| r.len() as u64 + gap)
        .max(1);
    let ramp = args.iter().any(|a| a == "--ramp");
    let sample = |c: usize, index: u64| -> f32 {
        if ramp {
            return mxl_audio_mixer::timing::ramp(index);
        }
        match &recording {
            Some(r) => r
                .get(((index + shift) % period) as usize)
                .copied()
                .unwrap_or(0.0),
            None => tone(c, index),
        }
    };

    if let Err(e) = domain::ensure_output_domain(&domain_dir, domain_id, "tones", 200_000_000) {
        eprintln!("{e}");
        std::process::exit(1);
    }
    let api = mxlio::load().unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1)
    });
    let instance = Arc::new(WriterInstance::new(api, &domain_dir).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1)
    }));
    let def = serde_json::json!({
        "id": flow.to_string(),
        "source_id": Uuid::new_v5(&flow, b"source").to_string(),
        "device_id": Uuid::new_v5(&flow, b"device").to_string(),
        "label": "Test tone",
        "description": "mxl-mixer-tone test signal",
        "format": "urn:x-nmos:format:audio",
        "tags": {"urn:x-nmos:tag:grouphint/v1.0": ["Test tone:Audio"]},
        "version": "0:0",
        "parents": [],
        "media_type": "audio/float32",
        "sample_rate": {"numerator": 48000, "denominator": 1},
        "channel_count": channels,
        "bit_depth": 32
    });
    let mut writer = instance
        .create_samples_writer(&def.to_string(), commit)
        .unwrap_or_else(|e| {
            eprintln!("{e}");
            std::process::exit(1)
        });
    eprintln!(
        "writing flow {flow} ({channels} ch, created {}, commit hint {}, buffer {}) into {}, {delay_ms} ms late",
        writer.created,
        writer.commit_hint,
        writer.buffer_length,
        domain_dir.display()
    );

    let stop = Arc::new(AtomicBool::new(false));
    for signal in [libc::SIGTERM, libc::SIGINT] {
        let flag = stop.clone();
        let _ = unsafe { signal_hook(signal, flag) };
    }
    let delay_ns = (delay_ms * 1e6) as u64;
    let lead_ns = nanos("--lead-ms") / 1000;
    let mut n = index_at(tai_ns()) / block + 1;
    while !stop.load(Ordering::Relaxed) {
        let last = (n + batch) * block;
        let due = time_of(last);
        let late = if every_ns > 0 && due % every_ns < for_ns {
            late_ns
        } else {
            0
        };
        sleep_until_tai((due + delay_ns + late + behind_ns).saturating_sub(lead_ns));
        // Behind by `behind_ns`, catching up at `catch_up` times real time.
        behind_ns =
            behind_ns.saturating_sub((time_of(batch * block) as f64 * (catch_up - 1.0)) as u64);
        for k in 1..=batch {
            let head = (n + k) * block;
            let start = head - block;
            let _ = writer.write(head, block as usize, |c, offset, samples| {
                for (k, s) in samples.iter_mut().enumerate() {
                    *s = sample(c, start + (offset + k) as u64);
                }
            });
        }
        // A late writer catches up block by block instead of skipping: a skipped
        // range would keep the ring's data of one buffer length earlier, which a
        // reader cannot tell from fresh samples.
        n += batch;
    }
}

static mut FLAGS: Vec<Arc<AtomicBool>> = Vec::new();

extern "C" fn on_signal(_: libc::c_int) {
    // SAFETY: the vector is written before the handlers are installed and only read here.
    #[allow(static_mut_refs)]
    unsafe {
        for flag in FLAGS.iter() {
            flag.store(true, Ordering::Relaxed);
        }
    }
}

/// Install a handler that sets `flag`.
unsafe fn signal_hook(signal: libc::c_int, flag: Arc<AtomicBool>) -> libc::sighandler_t {
    #[allow(static_mut_refs)]
    FLAGS.push(flag);
    libc::signal(
        signal,
        on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t,
    )
}
