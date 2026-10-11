//! Test reader: reads an MXL audio flow on the TAI clock and reports what it got.
//!
//! ```sh
//! mxl-mixer-probe --domain DIR --flow UUID [--seconds 2] [--behind-ms 20] [--expect CH:SRC:GAIN ...]
//! ```
//!
//! Reads `--behind-ms` behind the TAI now (more for mirrored flows, which arrive
//! late). Prints one JSON line: samples read, failed reads, and per channel the peak and
//! RMS level. `--expect 1:2:0.5` checks that channel 1 carries 0.5 × the
//! `mxl-mixer-tone` signal of tone channel 2; the tone repeats every 48 samples,
//! so the probe finds the best of the 48 alignments and reports the largest error.

use std::path::PathBuf;
use std::time::Duration;

use mxl_audio_mixer::mxlio::{self, MxlInstance};
use mxl_audio_mixer::timing::{index_at, sleep_until_tai, tai_ns, time_of};
use uuid::Uuid;

const BLOCK: usize = 480;

fn tone(channel: usize, index: u64) -> f32 {
    let freq = 1_000.0 * (channel as f64 + 1.0);
    let phase = (index % 48) as f64 / 48_000.0;
    (0.1 * (2.0 * std::f64::consts::PI * freq * phase).sin()) as f32
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arg = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1).cloned())
    };
    let usage = || -> ! {
        eprintln!("usage: mxl-mixer-probe --domain DIR --flow UUID [--seconds S] [--behind-ms MS] [--expect CH:SRC:GAIN ...]");
        std::process::exit(2)
    };
    let domain = PathBuf::from(arg("--domain").unwrap_or_else(|| usage()));
    let flow: Uuid = arg("--flow")
        .and_then(|f| f.parse().ok())
        .unwrap_or_else(|| usage());
    let seconds: f64 = arg("--seconds").and_then(|s| s.parse().ok()).unwrap_or(2.0);
    let behind: u64 = arg("--behind-ms")
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(20)
        * 48;
    let expects: Vec<(usize, usize, f32)> = args
        .windows(2)
        .filter(|w| w[0] == "--expect")
        .map(|w| {
            let p: Vec<&str> = w[1].split(':').collect();
            match (p.first(), p.get(1), p.get(2)) {
                (Some(c), Some(s), Some(g)) => (
                    c.parse::<usize>().unwrap_or_else(|_| usage()) - 1,
                    s.parse::<usize>().unwrap_or_else(|_| usage()) - 1,
                    g.parse().unwrap_or_else(|_| usage()),
                ),
                _ => usage(),
            }
        })
        .collect();

    let api = mxlio::load().unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1)
    });
    let instance = MxlInstance::new(api, &domain.display().to_string(), "").unwrap_or_else(|e| {
        eprintln!("MXL instance: {e}");
        std::process::exit(1)
    });
    let (reader, channels) = mxlio::open_reader(&instance, flow).unwrap_or_else(|e| {
        eprintln!("open {flow}: {e}");
        std::process::exit(1)
    });
    let channels = channels as usize;
    let mut data: Vec<Vec<f32>> = vec![Vec::new(); channels];
    let mut first_index = None;
    let mut failed = 0u64;
    let blocks = (seconds * 48_000.0 / BLOCK as f64).ceil() as u64;
    // One block at a time, `behind` samples behind the TAI now.
    let mut head = index_at(tai_ns()) - behind;
    head -= head % BLOCK as u64;
    for _ in 0..blocks {
        head += BLOCK as u64;
        sleep_until_tai(time_of(head + behind));
        match reader.get_samples(head, BLOCK, Duration::from_millis(5)) {
            Ok(slice) => {
                first_index.get_or_insert(head - BLOCK as u64);
                for (c, out) in data.iter_mut().enumerate() {
                    if let Ok((a, b)) = slice.channel_data(c) {
                        out.extend(
                            a.chunks_exact(4)
                                .chain(b.chunks_exact(4))
                                .map(|s| f32::from_le_bytes([s[0], s[1], s[2], s[3]])),
                        );
                    }
                }
            }
            Err(_) => {
                failed += 1;
                if first_index.is_some() {
                    for out in data.iter_mut() {
                        out.extend(std::iter::repeat_n(f32::NAN, BLOCK));
                    }
                }
            }
        }
    }
    let first = first_index.unwrap_or(0);
    let level = |x: &[f32]| -> (f64, f64) {
        let valid: Vec<f64> = x
            .iter()
            .filter(|v| !v.is_nan())
            .map(|v| f64::from(*v))
            .collect();
        let peak = valid.iter().fold(0.0f64, |m, v| m.max(v.abs()));
        let rms = (valid.iter().map(|v| v * v).sum::<f64>() / valid.len().max(1) as f64).sqrt();
        let db = |l: f64| if l > 0.0 { 20.0 * l.log10() } else { -999.0 };
        (db(peak), db(rms))
    };
    let per_channel: Vec<serde_json::Value> = data
        .iter()
        .map(|x| {
            let (peak, rms) = level(x);
            serde_json::json!({"peak_dbfs": peak, "rms_dbfs": rms})
        })
        .collect();
    let checks: Vec<serde_json::Value> = expects
        .iter()
        .map(|&(ch, src, gain)| {
            let x = data.get(ch).cloned().unwrap_or_default();
            // Skip the first 100 ms (fades) and compare the rest.
            let skip = 4800.min(x.len());
            let best = (0..48u64)
                .map(|shift| {
                    let err = x[skip..]
                        .iter()
                        .enumerate()
                        .filter(|(_, v)| !v.is_nan())
                        .map(|(k, v)| {
                            let index = first + (skip + k) as u64 + shift;
                            (v - gain * tone(src, index)).abs()
                        })
                        .fold(0.0f32, f32::max);
                    (shift, err)
                })
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .unwrap_or((0, f32::INFINITY));
            serde_json::json!({
                "channel": ch + 1, "tone": src + 1, "gain": gain,
                "max_error": best.1, "shift_mod_48": best.0,
            })
        })
        .collect();
    let read: usize = data
        .first()
        .map(|d| d.iter().filter(|v| !v.is_nan()).count())
        .unwrap_or(0);
    println!(
        "{}",
        serde_json::json!({
            "flow": flow, "channels": channels, "samples": read,
            "failed_reads": failed, "levels": per_channel, "checks": checks,
        })
    );
}
