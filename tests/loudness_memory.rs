//! A loudness measurement keeps constant memory however long it runs (up to
//! 1.4.3 it kept every 100 ms block and short-term value: about 160 B/s per
//! meter). A test binary of its own, so that no other test allocates while the
//! heap is measured.

use mxl_audio_mixer::dsp::loudness::{weights, Loudness, Segment, MAX_CHANNELS, SEGMENT};

/// Bytes in use on the heap (glibc: every arena and the mmapped chunks).
fn in_use() -> usize {
    let m = unsafe { libc::mallinfo2() };
    m.uordblks + m.hblkhd
}

#[test]
fn a_long_measurement_keeps_constant_memory() {
    let mut l = Loudness::new(weights(&["L", "R"]));
    let mut seed = 1u64;
    let mut segment = |index: u64| {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        // A level between −50 and −10 dBFS, changing every segment.
        let level = 10f64.powf(-(10.0 + 40.0 * (seed >> 11) as f64 / (1u64 << 53) as f64) / 10.0);
        let mut energy = [0.0; MAX_CHANNELS];
        energy[0] = level * SEGMENT as f64;
        energy[1] = level * SEGMENT as f64;
        Segment { index, energy }
    };
    // 10 minutes first, then 2 hours: 72 000 more blocks and short-term values.
    let ten_minutes = 60_000;
    for i in 0..ten_minutes {
        l.push(&segment(i));
    }
    let _ = l.values();
    let before = in_use();
    for i in ten_minutes..ten_minutes + 12 * ten_minutes {
        l.push(&segment(i));
    }
    let v = l.values();
    let grown = in_use().saturating_sub(before);
    assert!(v.integrated.is_finite() && v.range > 0.0, "{v:?}");
    assert!(
        grown < 16 * 1024,
        "the heap grew by {grown} bytes in 2 h of segments"
    );
}
