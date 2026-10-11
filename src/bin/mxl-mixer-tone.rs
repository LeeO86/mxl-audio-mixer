//! Test writer: a deterministic tone into an MXL audio flow on the TAI clock.
//!
//! ```sh
//! mxl-mixer-tone --domain /Volumes/mxl/tones --flow <uuid> [--channels 2] [--delay-ms 0] [--block 96]
//! ```
//!
//! Channel c carries 1 kHz × (c + 1) at −20 dBFS; every sample depends only on its
//! index, so a reader can check what it gets. `--delay-ms` writes every block that
//! much late (a mirrored or batched input). The domain directory and its
//! `domain_def.json` are created if missing (`--domain-id` sets the id).

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
        eprintln!("usage: mxl-mixer-tone --domain DIR --flow UUID [--domain-id UUID] [--channels N] [--delay-ms D] [--block B] [--commit C]");
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
    let mut n = index_at(tai_ns()) / block + 1;
    while !stop.load(Ordering::Relaxed) {
        let head = (n + 1) * block;
        sleep_until_tai(time_of(head) + delay_ns);
        let start = head - block;
        let _ = writer.write(head, block as usize, |c, offset, samples| {
            for (k, s) in samples.iter_mut().enumerate() {
                *s = tone(c, start + (offset + k) as u64);
            }
        });
        n = (n + 1).max(index_at(tai_ns().saturating_sub(delay_ns)) / block);
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
