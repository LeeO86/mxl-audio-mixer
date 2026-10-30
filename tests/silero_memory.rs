//! The Silero VAD keeps no memory per inference (the 1.3.0–1.4.3 leak: `ort`
//! 2.0.0-rc.14 leaked about 290 bytes per output tensor of every run, about
//! 16 kB/s per detector). A test binary of its own, so that no other test
//! allocates while the heap is measured.

use mxl_audio_mixer::auto::silero::SileroDetector;
use mxl_audio_mixer::auto::vad::{Detector, FRAME};

/// Bytes in use on the heap (glibc, every arena).
fn in_use() -> usize {
    unsafe { libc::mallinfo2() }.uordblks
}

#[test]
fn inference_keeps_no_memory() {
    let mut d = SileroDetector::new().expect("the model loads");
    // A 300 Hz tone with a little noise, one second of frames.
    let mut seed = 1u32;
    let x: Vec<f32> = (0..48_000)
        .map(|i| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = (seed >> 8) as f32 / (1 << 24) as f32 - 0.5;
            0.1 * (i as f32 * 300.0 * std::f32::consts::TAU / 48_000.0).sin() + 0.01 * noise
        })
        .collect();
    let mut run = |seconds: usize| {
        for _ in 0..seconds {
            for c in x.chunks_exact(FRAME) {
                d.frame(c);
            }
        }
    };
    run(5);
    let before = in_use();
    // 120 s: 3750 inferences, about 2.2 MB with the leak.
    run(120);
    let grown = in_use().saturating_sub(before);
    assert!(
        grown < 256 * 1024,
        "the heap grew by {grown} bytes in 3750 inferences"
    );
}
