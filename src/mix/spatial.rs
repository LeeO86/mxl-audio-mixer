//! The matrices of the M/S / width node (D3) and the surround panner (D5). Both
//! run as gain matrices in the plan (`Step::Matrix`): the DSP thread ramps every
//! coefficient per sample when a parameter moves, and holds no other state.

use std::f64::consts::FRAC_PI_2;

use super::chain::{StereoMode, LFE_OFF_DB};
use super::format::Format;

/// The 2 × 2 matrix (row-major: out 1 ← in 1, out 1 ← in 2, out 2 ← in 1,
/// out 2 ← in 2) of the M/S / width node: the input to mid and side (L/R:
/// M = (L + R) / 2, S = (L − R) / 2; M/S: as it is), the side × `width_pct` / 100,
/// then mid and side to the output (L/R: L = M + S, R = M − S; M/S: as it is).
/// L/R in and out at 100 % is the identity, at 0 % mono (L + R) / 2 on both.
pub fn width_matrix(width_pct: f64, input: StereoMode, output: StereoMode) -> [f64; 4] {
    let w = width_pct / 100.0;
    // Into mid/side.
    let a = match input {
        StereoMode::Lr => [0.5, 0.5, 0.5, -0.5],
        StereoMode::Ms => [1.0, 0.0, 0.0, 1.0],
    };
    // The side scaled.
    let a = [a[0], a[1], a[2] * w, a[3] * w];
    // Out of mid/side.
    let b = match output {
        StereoMode::Lr => [1.0, 1.0, 1.0, -1.0],
        StereoMode::Ms => [1.0, 0.0, 0.0, 1.0],
    };
    [
        b[0] * a[0] + b[1] * a[2],
        b[0] * a[1] + b[1] * a[3],
        b[2] * a[0] + b[3] * a[2],
        b[2] * a[1] + b[3] * a[3],
    ]
}

/// The speaker angles of `layout` (degrees, 0 front centre, negative left), in
/// channel order; `None` for the LFE. ITU-R BS.775 (5.1) and BS.2051 (7.1)
/// nominal positions; the height layouts (5.1.4, 7.1.4) add four top speakers
/// at ±45° and ±135° after the bed (see [`top`]).
fn speakers(layout: Format) -> &'static [Option<f64>] {
    match layout {
        Format::Surround71 => &[
            Some(-30.0),
            Some(30.0),
            Some(0.0),
            None,
            Some(-90.0),
            Some(90.0),
            Some(-150.0),
            Some(150.0),
        ],
        Format::Surround514 => &[
            Some(-30.0),
            Some(30.0),
            Some(0.0),
            None,
            Some(-110.0),
            Some(110.0),
            Some(-45.0),
            Some(45.0),
            Some(-135.0),
            Some(135.0),
        ],
        Format::Surround714 => &[
            Some(-30.0),
            Some(30.0),
            Some(0.0),
            None,
            Some(-90.0),
            Some(90.0),
            Some(-150.0),
            Some(150.0),
            Some(-45.0),
            Some(45.0),
            Some(-135.0),
            Some(135.0),
        ],
        _ => &[
            Some(-30.0),
            Some(30.0),
            Some(0.0),
            None,
            Some(-110.0),
            Some(110.0),
        ],
    }
}

/// The first top (height) channel of `layout`: the speakers from there on are
/// the upper layer. The channel count for a layout without height.
fn top(layout: Format) -> usize {
    match layout {
        Format::Surround514 => 6,
        Format::Surround714 => 8,
        _ => speakers(layout).len(),
    }
}

/// Powers (squared gains, summing to 1) of a point source at `azimuth`
/// (degrees) on the ring of speakers `spk` (one layer): constant-power panning
/// between the two neighbouring speakers (pairwise, as 2-D VBAP with a
/// sine/cosine law). `out` has a value per entry of `spk`.
fn point(spk: &[Option<f64>], azimuth: f64, out: &mut [f64]) {
    // The speakers sorted by angle, as (angle, channel).
    let mut ring: [(f64, usize); 8] = [(0.0, 0); 8];
    let mut n = 0;
    for (c, a) in spk.iter().enumerate() {
        if let Some(a) = a {
            ring[n] = (*a, c);
            n += 1;
        }
    }
    let ring = &mut ring[..n];
    ring.sort_by(|a, b| a.0.total_cmp(&b.0));
    let az = wrap(azimuth);
    out.iter_mut().for_each(|p| *p = 0.0);
    for k in 0..n {
        let (a1, c1) = ring[k];
        let (mut a2, c2) = ring[(k + 1) % n];
        if k + 1 == n {
            a2 += 360.0;
        }
        let mut x = az;
        if x < a1 {
            x += 360.0;
        }
        if x >= a1 && x <= a2 {
            let t = (x - a1) / (a2 - a1);
            out[c1] += (t * FRAC_PI_2).cos().powi(2);
            out[c2] += (t * FRAC_PI_2).sin().powi(2);
            return;
        }
    }
}

/// An angle in degrees into −180 … 180.
fn wrap(a: f64) -> f64 {
    let mut a = (a + 180.0).rem_euclid(360.0) - 180.0;
    if a == -180.0 {
        a = 180.0;
    }
    a
}

/// The surround panner's matrix: `layout.channels()` rows × `inputs` (1 or 2)
/// columns, row-major. A mono input sits at `azimuth_deg`; a stereo input's L
/// and R at `azimuth_deg` ∓ 30°. On a height layout (5.1.4, 7.1.4)
/// `elevation_deg` (0 … 90) lifts the source: the bed keeps cos² of its power,
/// the top layer gets sin², panned there at the same azimuth (A10); on the other
/// layouts it has no effect. `spread_pct` blends each source's powers with
/// an equal share on every speaker (100 %: all alike), `divergence_pct` moves
/// that share of the centre's power into L and R; both keep the power (the sum
/// of the squared gains of a column, without the LFE) at 1. The LFE gets each
/// input at `lfe_db` (a stereo input's channels at half that gain each); at
/// [`LFE_OFF_DB`] or below, nothing.
pub fn surround_pan(
    layout: Format,
    inputs: usize,
    azimuth_deg: f64,
    spread_pct: f64,
    divergence_pct: f64,
    lfe_db: f64,
    elevation_deg: f64,
) -> Vec<f32> {
    let spk = speakers(layout);
    let rows = spk.len();
    let top = top(layout);
    let lift = (elevation_deg.clamp(0.0, 90.0)).to_radians();
    let (bed_share, top_share) = if top < rows {
        (lift.cos().powi(2), lift.sin().powi(2))
    } else {
        (1.0, 0.0)
    };
    let spread = (spread_pct / 100.0).clamp(0.0, 1.0);
    let divergence = (divergence_pct / 100.0).clamp(0.0, 1.0);
    let mains = spk.iter().filter(|a| a.is_some()).count() as f64;
    let lfe = if lfe_db <= LFE_OFF_DB {
        0.0
    } else {
        10f64.powf(lfe_db / 20.0)
    };
    let lfe_each = if inputs == 2 { lfe * 0.5 } else { lfe };
    let mut m = vec![0f32; rows * inputs];
    for i in 0..inputs {
        let az = match (inputs, i) {
            (2, 0) => azimuth_deg - 30.0,
            (2, _) => azimuth_deg + 30.0,
            _ => azimuth_deg,
        };
        let mut p = [0f64; 12];
        point(&spk[..top], az, &mut p[..top]);
        if top < rows {
            point(&spk[top..], az, &mut p[top..rows]);
        }
        for (r, v) in p[..rows].iter_mut().enumerate() {
            *v *= if r < top { bed_share } else { top_share };
        }
        // Centre divergence: the share into L and R, half each.
        let c = p[2] * divergence;
        p[2] -= c;
        p[0] += c / 2.0;
        p[1] += c / 2.0;
        for (r, a) in spk.iter().enumerate() {
            let power = match a {
                Some(_) => (1.0 - spread) * p[r] + spread / mains,
                None => 0.0,
            };
            let g = if a.is_some() { power.sqrt() } else { lfe_each };
            m[r * inputs + i] = g as f32;
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    fn apply(m: [f64; 4], l: f64, r: f64) -> (f64, f64) {
        (m[0] * l + m[1] * r, m[2] * l + m[3] * r)
    }

    #[test]
    fn width_identity_mono_and_wider() {
        use StereoMode::{Lr, Ms};
        assert_eq!(width_matrix(100.0, Lr, Lr), [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(width_matrix(0.0, Lr, Lr), [0.5, 0.5, 0.5, 0.5]);
        // The same as the stereo strip's width (format::stereo_matrix).
        for w in [0.0, 0.3, 1.0, 1.7, 2.0] {
            let a = width_matrix(w * 100.0, Lr, Lr);
            let b = super::super::format::stereo_matrix(w, 0.0);
            for k in 0..4 {
                assert!(close(a[k], b[k]), "{w}: {a:?} {b:?}");
            }
        }
        // 200 %: double side. L only → M 0.5, S 0.5 → L 1.5, R −0.5.
        let (l, r) = apply(width_matrix(200.0, Lr, Lr), 1.0, 0.0);
        assert!(close(l, 1.5) && close(r, -0.5));
        // Mono in: unchanged at any width (no side).
        for w in [0.0, 50.0, 200.0] {
            let (l, r) = apply(width_matrix(w, Lr, Lr), 0.4, 0.4);
            assert!(close(l, 0.4) && close(r, 0.4));
        }
        // Encode then decode is the identity; M/S of L only.
        let enc = width_matrix(100.0, Lr, Ms);
        let dec = width_matrix(100.0, Ms, Lr);
        let (m, s) = apply(enc, 1.0, 0.0);
        assert!(close(m, 0.5) && close(s, 0.5));
        let (l, r) = apply(dec, m, s);
        assert!(close(l, 1.0) && close(r, 0.0));
        // An M/S microphone pair (M front, S figure-8) at 0 % is its mid alone.
        let (l, r) = apply(width_matrix(0.0, Ms, Lr), 0.7, 0.2);
        assert!(close(l, 0.7) && close(r, 0.7));
        // M/S through, width on the side only.
        assert_eq!(width_matrix(50.0, Ms, Ms), [1.0, 0.0, 0.0, 0.5]);
    }

    fn power(m: &[f32], rows: usize, inputs: usize, col: usize, lfe: usize) -> f64 {
        (0..rows)
            .filter(|r| *r != lfe)
            .map(|r| f64::from(m[r * inputs + col]).powi(2))
            .sum()
    }

    #[test]
    fn panner_keeps_the_power() {
        for layout in [
            Format::Surround51,
            Format::Surround71,
            Format::Surround514,
            Format::Surround714,
        ] {
            let rows = layout.channels();
            for az in (-180..=180).step_by(5) {
                for spread in [0.0, 25.0, 100.0] {
                    for div in [0.0, 40.0, 100.0] {
                        for inputs in [1, 2] {
                            let m = surround_pan(
                                layout, inputs, az as f64, spread, div, LFE_OFF_DB, 0.0,
                            );
                            assert_eq!(m.len(), rows * inputs);
                            for col in 0..inputs {
                                let p = power(&m, rows, inputs, col, 3);
                                assert!(
                                    (p - 1.0).abs() < 1e-6,
                                    "{layout:?} az {az} spread {spread} div {div} in {inputs}: {p}"
                                );
                            }
                            assert!(m.iter().all(|g| (0.0..=1.0).contains(g)));
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn panner_positions() {
        let one = |layout, az: f64| surround_pan(layout, 1, az, 0.0, 0.0, LFE_OFF_DB, 0.0);
        // On a speaker: that speaker alone.
        for (az, ch) in [(0.0, 2), (-30.0, 0), (30.0, 1), (-110.0, 4), (110.0, 5)] {
            let m = one(Format::Surround51, az);
            for (r, g) in m.iter().enumerate() {
                assert!(
                    close((*g).into(), if r == ch { 1.0 } else { 0.0 }),
                    "{az}: {m:?}"
                );
            }
        }
        for (az, ch) in [(-90.0, 4), (90.0, 5), (-150.0, 6), (150.0, 7), (180.0, 99)] {
            let m = one(Format::Surround71, az);
            if ch == 99 {
                // Straight behind: between the rear surrounds, −3 dB each.
                assert!(close(m[6].into(), std::f64::consts::FRAC_1_SQRT_2));
                assert!(close(m[7].into(), std::f64::consts::FRAC_1_SQRT_2));
                continue;
            }
            assert!(close(m[ch].into(), 1.0), "{az}: {m:?}");
        }
        // Halfway between C and R: −3 dB in each.
        let m = one(Format::Surround51, 15.0);
        assert!(close(m[2].into(), std::f64::consts::FRAC_1_SQRT_2));
        assert!(close(m[1].into(), std::f64::consts::FRAC_1_SQRT_2));
        // Behind on 5.1 (between Rs at 110 and Ls at −110): both.
        let m = one(Format::Surround51, 180.0);
        assert!(close(m[4].into(), m[5].into()) && m[4] > 0.7);
        // −180 and 180 are the same place.
        assert_eq!(
            one(Format::Surround51, -180.0),
            one(Format::Surround51, 180.0)
        );
        // Full divergence: the centre's power into L and R.
        let m = surround_pan(Format::Surround51, 1, 0.0, 0.0, 100.0, LFE_OFF_DB, 0.0);
        assert!(close(m[2].into(), 0.0));
        assert!(close(m[0].into(), std::f64::consts::FRAC_1_SQRT_2));
        // Full spread: every main speaker alike.
        let m = surround_pan(Format::Surround71, 1, 40.0, 100.0, 0.0, LFE_OFF_DB, 0.0);
        for r in [0, 1, 2, 4, 5, 6, 7] {
            assert!(close(m[r].into(), (1.0f64 / 7.0).sqrt()));
        }
        // LFE: off, at 0 dB, at −10 dB; a stereo input's channels at half each.
        assert_eq!(one(Format::Surround51, 0.0)[3], 0.0);
        let m = surround_pan(Format::Surround51, 1, 0.0, 0.0, 0.0, 0.0, 0.0);
        assert!(close(m[3].into(), 1.0));
        let m = surround_pan(Format::Surround51, 2, 0.0, 0.0, 0.0, -10.0, 0.0);
        assert!(close(m[6].into(), 0.5 * 10f64.powf(-0.5)));
        assert!(close(m[7].into(), 0.5 * 10f64.powf(-0.5)));
        // A stereo input at 0°: L on the L speaker, R on the R speaker.
        let m = surround_pan(Format::Surround51, 2, 0.0, 0.0, 0.0, LFE_OFF_DB, 0.0);
        assert!(close(m[0].into(), 1.0) && close(m[3].into(), 1.0));
        assert!(close(m[1].into(), 0.0) && close(m[2].into(), 0.0));
    }

    #[test]
    fn panner_lifts_into_the_height_layer() {
        let at = |layout, az: f64, el: f64| surround_pan(layout, 1, az, 0.0, 0.0, LFE_OFF_DB, el);
        // At 0° elevation a height layout pans like its bed.
        let m = at(Format::Surround514, -110.0, 0.0);
        assert!(close(m[4].into(), 1.0) && m[6..].iter().all(|g| *g == 0.0));
        // All the way up at −45°: the top front left speaker alone.
        let m = at(Format::Surround714, -45.0, 90.0);
        assert!(close(m[8].into(), 1.0), "{m:?}");
        assert!(m[..8].iter().all(|g| f64::from(*g).abs() < 1e-6));
        // The power stays 1 at every height.
        for layout in [Format::Surround514, Format::Surround714] {
            for el in [0.0, 20.0, 45.0, 70.0, 90.0] {
                for az in (-180..=180).step_by(15) {
                    let m = surround_pan(layout, 2, az as f64, 30.0, 50.0, LFE_OFF_DB, el);
                    for col in 0..2 {
                        let p = power(&m, layout.channels(), 2, col, 3);
                        assert!((p - 1.0).abs() < 1e-6, "{layout:?} az {az} el {el}: {p}");
                    }
                }
            }
        }
        // 45° elevation: half the power in each layer.
        let m = at(Format::Surround714, 0.0, 45.0);
        assert!(close(m[2].into(), std::f64::consts::FRAC_1_SQRT_2));
        assert!(close(m[8].into(), 0.5) && close(m[9].into(), 0.5));
        // Behind and up on 5.1.4: the top rear pair.
        let m = at(Format::Surround514, 180.0, 90.0);
        assert!(close(m[8].into(), m[9].into()) && m[8] > 0.7);
        // No height layer: elevation changes nothing.
        assert_eq!(
            at(Format::Surround51, 30.0, 60.0),
            at(Format::Surround51, 30.0, 0.0)
        );
        // Full spread on 7.1.4: all 11 main speakers alike.
        let m = surround_pan(Format::Surround714, 1, 10.0, 100.0, 0.0, LFE_OFF_DB, 30.0);
        for r in (0..12).filter(|r| *r != 3) {
            assert!(close(m[r].into(), (1.0f64 / 11.0).sqrt()));
        }
    }
}
