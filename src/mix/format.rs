//! Channel formats (§4.1), pan laws and width mapping (§5).
//!
//! Channel order follows SMPTE ST 2110-30 / ITU-R BS.2051: L R C LFE, then the
//! surrounds (Ls Rs, or Lss Rss Lrs Rrs), then the heights (Ltf Rtf Ltr Rtr).

use std::f64::consts::{FRAC_1_SQRT_2, FRAC_PI_4};

use serde::{Deserialize, Serialize};

/// The widest format's channel count (7.1.4).
pub const MAX_FORMAT_CHANNELS: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Format {
    #[serde(rename = "mono")]
    Mono,
    #[serde(rename = "stereo")]
    Stereo,
    #[serde(rename = "3.0")]
    Lcr,
    #[serde(rename = "5.1")]
    Surround51,
    #[serde(rename = "7.1")]
    Surround71,
    #[serde(rename = "5.1.4")]
    Surround514,
    #[serde(rename = "7.1.4")]
    Surround714,
}

impl Format {
    pub fn labels(self) -> &'static [&'static str] {
        match self {
            Self::Mono => &["M"],
            Self::Stereo => &["L", "R"],
            Self::Lcr => &["L", "R", "C"],
            Self::Surround51 => &["L", "R", "C", "LFE", "Ls", "Rs"],
            Self::Surround71 => &["L", "R", "C", "LFE", "Lss", "Rss", "Lrs", "Rrs"],
            Self::Surround514 => &["L", "R", "C", "LFE", "Ls", "Rs", "Ltf", "Rtf", "Ltr", "Rtr"],
            Self::Surround714 => &[
                "L", "R", "C", "LFE", "Lss", "Rss", "Lrs", "Rrs", "Ltf", "Rtf", "Ltr", "Rtr",
            ],
        }
    }

    pub fn channels(self) -> usize {
        self.labels().len()
    }

    fn index(self, label: &str) -> Option<usize> {
        self.labels().iter().position(|l| *l == label)
    }

    /// The VSF TR-03 symbol of a channel label, where TR-03 has one.
    pub fn symbol(label: &str) -> Option<&'static str> {
        match label {
            "M" => Some("M1"),
            "L" => Some("L"),
            "R" => Some("R"),
            "C" => Some("C"),
            "LFE" => Some("LFE"),
            "Ls" => Some("Ls"),
            "Rs" => Some("Rs"),
            "Lss" => Some("Lss"),
            "Rss" => Some("Rss"),
            "Lrs" => Some("Lrs"),
            "Rrs" => Some("Rrs"),
            _ => None,
        }
    }
}

/// Mono-to-stereo pan law: the level of a centred mono signal in each side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(try_from = "f64", into = "f64")]
pub enum PanLaw {
    Zero,
    #[default]
    Minus3,
    Minus4_5,
    Minus6,
}

impl TryFrom<f64> for PanLaw {
    type Error = String;
    fn try_from(db: f64) -> Result<Self, String> {
        match db {
            0.0 => Ok(Self::Zero),
            -3.0 => Ok(Self::Minus3),
            -4.5 => Ok(Self::Minus4_5),
            -6.0 => Ok(Self::Minus6),
            _ => Err(format!("pan law must be 0, -3, -4.5 or -6 dB, not {db}")),
        }
    }
}

impl From<PanLaw> for f64 {
    fn from(law: PanLaw) -> f64 {
        match law {
            PanLaw::Zero => 0.0,
            PanLaw::Minus3 => -3.0,
            PanLaw::Minus4_5 => -4.5,
            PanLaw::Minus6 => -6.0,
        }
    }
}

/// Stereo-to-mono sum: the gain applied to L + R.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(try_from = "f64", into = "f64")]
pub enum MonoSum {
    #[default]
    Minus3,
    Minus6,
}

impl TryFrom<f64> for MonoSum {
    type Error = String;
    fn try_from(db: f64) -> Result<Self, String> {
        match db {
            -3.0 => Ok(Self::Minus3),
            -6.0 => Ok(Self::Minus6),
            _ => Err(format!("stereo-to-mono sum must be -3 or -6 dB, not {db}")),
        }
    }
}

impl From<MonoSum> for f64 {
    fn from(sum: MonoSum) -> f64 {
        match sum {
            MonoSum::Minus3 => -3.0,
            MonoSum::Minus6 => -6.0,
        }
    }
}

/// The laws width mapping uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Laws {
    pub pan: PanLaw,
    pub mono_sum: MonoSum,
}

/// Gains (left, right) of a mono signal at pan position `p` (−1 left, 0 centre,
/// +1 right). −3 dB is constant power (sin/cos), −6 dB linear, −4.5 dB their
/// geometric mean, 0 dB linear with both sides at unity in the centre.
pub fn pan_gains(p: f64, law: PanLaw) -> (f64, f64) {
    let p = p.clamp(-1.0, 1.0);
    let theta = (p + 1.0) * FRAC_PI_4;
    let power = (theta.cos(), theta.sin());
    let linear = ((1.0 - p) / 2.0, (1.0 + p) / 2.0);
    match law {
        PanLaw::Zero => ((1.0 - p).min(1.0), (1.0 + p).min(1.0)),
        PanLaw::Minus3 => power,
        PanLaw::Minus4_5 => ((power.0 * linear.0).sqrt(), (power.1 * linear.1).sqrt()),
        PanLaw::Minus6 => linear,
    }
}

/// Gains (left, right) of a stereo balance `b` (−1 left only … +1 right only).
pub fn balance_gains(b: f64) -> (f64, f64) {
    let b = b.clamp(-1.0, 1.0);
    ((1.0 - b).min(1.0), (1.0 + b).min(1.0))
}

/// The 2 × 2 matrix (row-major: L←L, L←R, R←L, R←R) of a stereo width `w`
/// (0 mono, 1 unchanged, 2 double side signal) followed by balance `b`.
pub fn stereo_matrix(w: f64, b: f64) -> [f64; 4] {
    let (gl, gr) = balance_gains(b);
    let same = (1.0 + w) / 2.0;
    let cross = (1.0 - w) / 2.0;
    [gl * same, gl * cross, gr * cross, gr * same]
}

fn db_to_gain(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

/// The width mapping from `src` to `dst` as a row-major matrix of
/// `dst.channels()` rows and `src.channels()` columns.
///
/// Channels with the same label map 1:1. A channel `dst` lacks is folded into the
/// ones it has (ITU-R BS.775 down-mix at −3 dB per step, LFE dropped):
/// C → L + R; Ls → L (or Lss when `dst` has side surrounds); Lss, Lrs → Ls;
/// Ltf → L; Ltr → Lrs (or Ls); L, R → mono with the stereo-to-mono sum; mono →
/// L + R with the pan law. Channels `dst` has and `src` lacks stay silent.
pub fn width_matrix(src: Format, dst: Format, laws: Laws) -> Vec<f32> {
    let columns = src.channels();
    let mut m = vec![0f64; dst.channels() * columns];
    for (s, label) in src.labels().iter().enumerate() {
        fold(label, dst, 1.0, laws, &mut |d, g| m[d * columns + s] += g);
    }
    m.into_iter().map(|g| g as f32).collect()
}

fn fold(label: &str, dst: Format, gain: f64, laws: Laws, add: &mut dyn FnMut(usize, f64)) {
    if let Some(d) = dst.index(label) {
        add(d, gain);
        return;
    }
    let h = FRAC_1_SQRT_2;
    let mut to = |l: &str, g: f64| fold(l, dst, gain * g, laws, add);
    match label {
        "M" => {
            let (l, r) = pan_gains(0.0, laws.pan);
            to("L", l);
            to("R", r);
        }
        "L" | "R" => to("M", db_to_gain(laws.mono_sum.into())),
        "C" => {
            to("L", h);
            to("R", h);
        }
        "Ls" if dst.index("Lss").is_some() => to("Lss", 1.0),
        "Rs" if dst.index("Rss").is_some() => to("Rss", 1.0),
        "Ls" => to("L", h),
        "Rs" => to("R", h),
        "Lss" | "Lrs" => to("Ls", h),
        "Rss" | "Rrs" => to("Rs", h),
        "Ltf" => to("L", h),
        "Rtf" => to("R", h),
        "Ltr" if dst.index("Lrs").is_some() => to("Lrs", h),
        "Rtr" if dst.index("Rrs").is_some() => to("Rrs", h),
        "Ltr" => to("Ls", h),
        "Rtr" => to("Rs", h),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Format; 7] = [
        Format::Mono,
        Format::Stereo,
        Format::Lcr,
        Format::Surround51,
        Format::Surround71,
        Format::Surround514,
        Format::Surround714,
    ];

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn channel_counts_and_names() {
        let counts: Vec<usize> = ALL.iter().map(|f| f.channels()).collect();
        assert_eq!(counts, [1, 2, 3, 6, 8, 10, 12]);
        assert!(ALL.iter().all(|f| f.channels() <= MAX_FORMAT_CHANNELS));
        assert_eq!(
            serde_json::to_string(&Format::Surround514).unwrap(),
            "\"5.1.4\""
        );
        assert_eq!(
            serde_json::from_str::<Format>("\"3.0\"").unwrap(),
            Format::Lcr
        );
    }

    #[test]
    fn pan_laws_in_the_centre_and_at_the_sides() {
        for (law, centre_db) in [
            (PanLaw::Zero, 0.0),
            (PanLaw::Minus3, -3.0103),
            (PanLaw::Minus4_5, -4.515),
            (PanLaw::Minus6, -6.0206),
        ] {
            let (l, r) = pan_gains(0.0, law);
            assert!(close(l, r));
            assert!((20.0 * l.log10() - centre_db).abs() < 0.01, "{law:?} {l}");
            let (l, r) = pan_gains(-1.0, law);
            assert!(close(l, 1.0) && close(r, 0.0), "{law:?}");
            let (l, r) = pan_gains(1.0, law);
            assert!(close(l, 0.0) && close(r, 1.0), "{law:?}");
        }
        // Constant power across the range.
        for p in [-0.7, -0.2, 0.3, 0.9] {
            let (l, r) = pan_gains(p, PanLaw::Minus3);
            assert!(close(l * l + r * r, 1.0));
        }
        assert!(PanLaw::try_from(-4.0).is_err());
        assert_eq!(serde_json::to_string(&PanLaw::Minus4_5).unwrap(), "-4.5");
    }

    proptest::proptest! {
        #[test]
        fn width_mapping_coefficients_are_bounded(
            src in 0usize..7,
            dst in 0usize..7,
            pan in 0usize..4,
            sum in 0usize..2,
        ) {
            let laws = Laws {
                pan: [PanLaw::Zero, PanLaw::Minus3, PanLaw::Minus4_5, PanLaw::Minus6][pan],
                mono_sum: [MonoSum::Minus3, MonoSum::Minus6][sum],
            };
            let m = width_matrix(ALL[src], ALL[dst], laws);
            proptest::prop_assert_eq!(m.len(), ALL[src].channels() * ALL[dst].channels());
            // At most +0.01 dB: C reaches mono as 2 × 1/√2 × the −3 dB (0.7079) sum.
            for g in m {
                proptest::prop_assert!(g.is_finite() && (0.0..=1.0012).contains(&g), "{}", g);
            }
        }

        #[test]
        fn constant_power_pan_law(p in -1.0f64..=1.0) {
            let (l, r) = pan_gains(p, PanLaw::Minus3);
            proptest::prop_assert!((l * l + r * r - 1.0).abs() < 1e-9);
            for law in [PanLaw::Zero, PanLaw::Minus4_5, PanLaw::Minus6] {
                let (l, r) = pan_gains(p, law);
                proptest::prop_assert!((0.0..=1.0).contains(&l) && (0.0..=1.0).contains(&r));
            }
        }
    }

    #[test]
    fn width_and_balance() {
        let unchanged = stereo_matrix(1.0, 0.0);
        assert_eq!(unchanged, [1.0, 0.0, 0.0, 1.0]);
        let mono = stereo_matrix(0.0, 0.0);
        assert_eq!(mono, [0.5, 0.5, 0.5, 0.5]);
        let left = stereo_matrix(1.0, -1.0);
        assert_eq!(left, [1.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn same_format_is_identity() {
        for f in ALL {
            let m = width_matrix(f, f, Laws::default());
            let n = f.channels();
            for d in 0..n {
                for s in 0..n {
                    assert_eq!(m[d * n + s], if d == s { 1.0 } else { 0.0 });
                }
            }
        }
    }

    #[test]
    fn mono_and_stereo() {
        let laws = Laws::default();
        let m = width_matrix(Format::Mono, Format::Stereo, laws);
        assert!(close(m[0].into(), FRAC_1_SQRT_2) && close(m[1].into(), FRAC_1_SQRT_2));
        let m = width_matrix(Format::Stereo, Format::Mono, laws);
        assert!(close(m[0].into(), 0.707_945_8) && close(m[1].into(), 0.707_945_8));
        let six = Laws {
            mono_sum: MonoSum::Minus6,
            ..laws
        };
        let m = width_matrix(Format::Stereo, Format::Mono, six);
        assert!(close(m[0].into(), 0.501_187_2));
        // Mono into 5.1 goes to the front pair.
        let m = width_matrix(Format::Mono, Format::Surround51, laws);
        assert!(m[0] > 0.7 && m[1] > 0.7 && m[2..].iter().all(|g| *g == 0.0));
    }

    #[test]
    fn bs775_down_mix_of_5_1() {
        let m = width_matrix(Format::Surround51, Format::Stereo, Laws::default());
        let h = FRAC_1_SQRT_2;
        // L row: L, R, C, LFE, Ls, Rs
        let expected_l = [1.0, 0.0, h, 0.0, h, 0.0];
        let expected_r = [0.0, 1.0, h, 0.0, 0.0, h];
        for s in 0..6 {
            assert!(close(m[s].into(), expected_l[s]), "L {s}: {}", m[s]);
            assert!(close(m[6 + s].into(), expected_r[s]), "R {s}");
        }
    }

    #[test]
    fn surround_up_and_down() {
        let laws = Laws::default();
        let h = FRAC_1_SQRT_2;
        // 5.1 → 7.1: Ls to the side surround, rear surrounds silent.
        let m = width_matrix(Format::Surround51, Format::Surround71, laws);
        let lss = 4;
        assert_eq!(m[lss * 6 + 4], 1.0);
        assert!(m[6 * 6..].iter().all(|g| *g == 0.0));
        // 7.1 → 5.1: both left surrounds into Ls at −3 dB.
        let m = width_matrix(Format::Surround71, Format::Surround51, laws);
        assert!(close(m[4 * 8 + 4].into(), h) && close(m[4 * 8 + 6].into(), h));
        // 7.1.4 → 7.1: top rear into the rear surround, top front into the front.
        let m = width_matrix(Format::Surround714, Format::Surround71, laws);
        assert!(close(m[6 * 12 + 10].into(), h));
        assert!(close(m[8].into(), h));
        // 7.1.4 → stereo: a top rear reaches L through Ls at −6 dB.
        let m = width_matrix(Format::Surround714, Format::Stereo, laws);
        assert!(close(m[10].into(), h * h));
        // Every source channel except LFE reaches every narrower format.
        for src in ALL {
            for dst in ALL {
                let m = width_matrix(src, dst, laws);
                for (s, label) in src.labels().iter().enumerate() {
                    let reaches = (0..dst.channels()).any(|d| m[d * src.channels() + s] != 0.0);
                    assert_eq!(
                        reaches,
                        *label != "LFE" || dst.channels() > 3,
                        "{src:?}→{dst:?} {label}"
                    );
                }
            }
        }
    }
}
