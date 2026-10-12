//! EBU Tech 3341 and Tech 3342 against the EBU loudness test set v5, through the
//! mixer's own pipeline: the DSP thread's segment meter in 96-sample blocks, the
//! control side's gating, the true-peak interpolator.
//!
//! The files are not in the repository. Set `EBU_TEST_SET` to the directory of
//! the unpacked `ebu-loudness-test-setv05.zip` (SHA-256 9cc500b4…b062; CI
//! downloads and checks it); without it the tests are skipped. Expected values and
//! tolerances are the ones of Tech 3341 (2016) and Tech 3342 (2016): ±0.1 LU for
//! loudness, +0.2 / −0.4 dB for true peak, ±1 LU for the loudness range.

use std::path::PathBuf;

use mxl_audio_mixer::dsp::loudness::{weights, Loudness, SegmentMeter, Values};
use mxl_audio_mixer::dsp::truepeak::{taps, TruePeak};

const BLOCK: usize = 96;

fn dir() -> Option<PathBuf> {
    let d = PathBuf::from(std::env::var("EBU_TEST_SET").ok()?);
    d.is_dir().then_some(d)
}

/// The file's channels as f32.
fn read(name: &str) -> Vec<Vec<f32>> {
    let path = dir().expect("EBU_TEST_SET").join(name);
    let mut r = hound::WavReader::open(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let spec = r.spec();
    assert_eq!(spec.sample_rate, 48_000, "{name}");
    let n = spec.channels as usize;
    let scale = 1.0 / (1u64 << (spec.bits_per_sample - 1)) as f32;
    let mut out = vec![Vec::new(); n];
    for (i, s) in r.samples::<i32>().enumerate() {
        out[i % n].push(s.expect("sample") as f32 * scale);
    }
    out
}

/// Run `name` through the segment meter and the measurement; `each` sees the
/// values after every 10 ms segment (segment `i` ends at (i + 1) × 10 ms).
fn measure(name: &str, labels: &[&str], mut each: impl FnMut(usize, Values)) -> Values {
    let signal = read(name);
    assert_eq!(signal.len(), labels.len(), "{name}: channels");
    let (mut meter, mut rx) = SegmentMeter::new(signal.len());
    let mut l = Loudness::new(weights(labels));
    let mut segment = 0;
    let len = signal[0].len();
    let mut start = 0;
    while start < len {
        let end = (start + BLOCK).min(len);
        for (c, ch) in signal.iter().enumerate() {
            let mut block = [0f32; BLOCK];
            block[..end - start].copy_from_slice(&ch[start..end]);
            meter.feed(start as u64, c, &block);
        }
        while let Ok(s) = rx.pop() {
            l.push(&s);
            each(segment, l.values());
            segment += 1;
        }
        start += BLOCK;
    }
    l.values()
}

fn close(what: &str, value: f64, expected: f64, tolerance: f64) {
    assert!(
        (value - expected).abs() <= tolerance,
        "{what}: {value:.3}, expected {expected} ± {tolerance}"
    );
}

const STEREO: [&str; 2] = ["L", "R"];

macro_rules! needs_files {
    () => {
        if dir().is_none() {
            eprintln!("EBU_TEST_SET not set: skipped");
            return;
        }
    };
}

#[test]
fn integrated_cases_1_to_8() {
    needs_files!();
    for (name, labels, expected) in [
        ("seq-3341-1-16bit.wav", &STEREO[..], -23.0),
        ("seq-3341-2-16bit.wav", &STEREO, -33.0),
        ("seq-3341-3-16bit-v02.wav", &STEREO, -23.0),
        ("seq-3341-4-16bit-v02.wav", &STEREO, -23.0),
        ("seq-3341-5-16bit-v02.wav", &STEREO, -23.0),
        (
            "seq-3341-6-5channels-16bit.wav",
            &["L", "R", "C", "Ls", "Rs"],
            -23.0,
        ),
        (
            "seq-3341-6-6channels-WAVEEX-16bit.wav",
            &["L", "R", "C", "LFE", "Ls", "Rs"],
            -23.0,
        ),
        ("seq-3341-7_seq-3342-5-24bit.wav", &STEREO, -23.0),
        ("seq-3341-2011-8_seq-3342-6-24bit-v02.wav", &STEREO, -23.0),
    ] {
        let v = measure(name, labels, |_, _| {});
        close(name, v.integrated, expected, 0.1);
    }
}

#[test]
fn momentary_and_short_term_constant_cases_1_2_9_12() {
    needs_files!();
    for (name, level) in [
        ("seq-3341-1-16bit.wav", -23.0),
        ("seq-3341-2-16bit.wav", -33.0),
    ] {
        // Constant after 1 s (momentary) and 3 s (short-term).
        measure(name, &STEREO, |i, v| {
            if i >= 99 {
                close(&format!("{name} M at {i}"), v.momentary, level, 0.1);
            }
            if i >= 299 {
                close(&format!("{name} S at {i}"), v.short_term, level, 0.1);
            }
        });
    }
    measure("seq-3341-9-24bit.wav", &STEREO, |i, v| {
        if i >= 299 {
            close(&format!("case 9 S at {i}"), v.short_term, -23.0, 0.1);
        }
    });
    measure("seq-3341-12-24bit.wav", &STEREO, |i, v| {
        if i >= 99 {
            close(&format!("case 12 M at {i}"), v.momentary, -23.0, 0.1);
        }
    });
}

#[test]
fn short_term_maximum_cases_10_11() {
    needs_files!();
    for n in 1..=20 {
        let name = format!("seq-3341-10-{n}-24bit.wav");
        let mut max = f64::MIN;
        measure(&name, &STEREO, |_, v| max = max.max(v.short_term));
        close(&name, max, -23.0, 0.1);
    }
    // Every 6 s the maximum so far rises by 1 LU, from −38 LUFS.
    let mut max = f64::MIN;
    measure("seq-3341-11-24bit.wav", &STEREO, |i, v| {
        max = max.max(v.short_term);
        if (i + 1) % 600 == 0 {
            let expected = -38.0 + ((i + 1) / 600 - 1) as f64;
            close(
                &format!("case 11 window {}", (i + 1) / 600),
                max,
                expected,
                0.1,
            );
        }
    });
}

#[test]
fn momentary_maximum_cases_13_14() {
    needs_files!();
    for n in 1..=20 {
        let name = if n <= 2 {
            format!("seq-3341-13-{n}-24bit.wav")
        } else {
            format!("seq-3341-13-{n}-24bit.wav.wav")
        };
        let mut max = f64::MIN;
        measure(&name, &STEREO, |_, v| max = max.max(v.momentary));
        close(&name, max, -23.0, 0.1);
    }
    // Every 800 ms the maximum so far rises by 1 LU, from −38 LUFS.
    let mut max = f64::MIN;
    measure("seq-3341-14-24bit.wav.wav", &STEREO, |i, v| {
        max = max.max(v.momentary);
        if (i + 1) % 80 == 0 {
            let expected = -38.0 + ((i + 1) / 80 - 1) as f64;
            close(
                &format!("case 14 window {}", (i + 1) / 80),
                max,
                expected,
                0.1,
            );
        }
    });
}

#[test]
fn true_peak_cases_15_to_23() {
    needs_files!();
    let t = taps();
    for (n, expected) in [
        (15, -6.0),
        (16, -6.0),
        (17, -6.0),
        (18, -6.0),
        (19, 3.0),
        (20, 0.0),
        (21, 0.0),
        (22, 0.0),
        (23, 0.0),
    ] {
        let name = format!("seq-3341-{n}-24bit.wav.wav");
        let peak = read(&name)
            .iter()
            .map(|ch| TruePeak::default().run(&t, ch))
            .fold(0f32, f32::max);
        let db = 20.0 * f64::from(peak).log10();
        assert!(
            (expected - 0.4..=expected + 0.2).contains(&db),
            "{name}: {db:.3} dBTP, expected {expected} +0.2/−0.4"
        );
    }
}

#[test]
fn loudness_range_tech_3342() {
    needs_files!();
    for (name, expected) in [
        ("seq-3342-1-16bit.wav", 10.0),
        ("seq-3342-2-16bit.wav", 5.0),
        ("seq-3342-3-16bit.wav", 20.0),
        ("seq-3342-4-16bit.wav", 15.0),
        ("seq-3341-7_seq-3342-5-24bit.wav", 5.0),
        ("seq-3341-2011-8_seq-3342-6-24bit-v02.wav", 15.0),
    ] {
        let v = measure(name, &STEREO, |_, _| {});
        close(name, v.range, expected, 1.0);
    }
}
