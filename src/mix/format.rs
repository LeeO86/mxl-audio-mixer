//! Channel formats (§4.1), pan laws and width mapping (§5).
//!
//! Channel order follows SMPTE ST 2110-30 / ITU-R BS.2051: L R C LFE, then the
//! surrounds (Ls Rs, or Lss Rss Lrs Rrs), then the heights (Ltf Rtf Ltr Rtr).

use std::f64::consts::{FRAC_1_SQRT_2, FRAC_PI_4};

use serde::{Deserialize, Serialize};

use super::model::Db;

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

/// The down-mix (§5, D1/D2): how channels a narrower format lacks fold into the
/// ones it has. ITU-R BS.775 style: the centre and the surrounds into the front
/// pair at their levels, the LFE dropped (or added to the front pair at its
/// level). The defaults are the fixed down-mix of 1.3: −3 dB, −3 dB, LFE off, no
/// normalisation.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Downmix {
    /// C into L and R.
    #[serde(default = "minus_3")]
    pub centre_db: Db,
    /// The surrounds into the front pair (Ls → L, Rs → R).
    #[serde(default = "minus_3")]
    pub surround_db: Db,
    /// LFE into L and R when the target has no LFE (`-inf`: dropped).
    #[serde(default = "lfe_off")]
    pub lfe_db: Db,
    /// Scale a down-mix from 3.0 or wider into fewer channels so that no output
    /// channel sums to more than unity (the largest row sum of the matrix).
    #[serde(default)]
    pub normalise: bool,
}

fn minus_3() -> Db {
    Db(-3.0)
}

fn lfe_off() -> Db {
    Db::MINUS_INF
}

impl Default for Downmix {
    fn default() -> Self {
        Self {
            centre_db: minus_3(),
            surround_db: minus_3(),
            lfe_db: lfe_off(),
            normalise: false,
        }
    }
}

/// Lowest finite down-mix level.
pub const DOWNMIX_MIN_DB: f64 = -40.0;
/// Highest LFE level in a down-mix.
pub const DOWNMIX_LFE_MAX_DB: f64 = 10.0;

impl Downmix {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    pub fn validate(&self) -> Result<(), String> {
        let check = |what: &str, db: Db, hi: f64| {
            if db == Db::MINUS_INF || (DOWNMIX_MIN_DB..=hi).contains(&db.0) {
                Ok(())
            } else {
                Err(format!(
                    "downmix {what} must be -inf or {DOWNMIX_MIN_DB} to {hi} dB"
                ))
            }
        };
        check("centre_db", self.centre_db, 0.0)?;
        check("surround_db", self.surround_db, 0.0)?;
        check("lfe_db", self.lfe_db, DOWNMIX_LFE_MAX_DB)
    }
}

/// The laws width mapping uses.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Laws {
    pub pan: PanLaw,
    pub mono_sum: MonoSum,
    pub downmix: Downmix,
}

impl Laws {
    /// These laws with `downmix` instead of their own, when there is one (a
    /// target's override).
    pub fn with_downmix(self, downmix: Option<Downmix>) -> Self {
        Self {
            downmix: downmix.unwrap_or(self.downmix),
            ..self
        }
    }
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
/// ones it has (ITU-R BS.775 style down-mix, [`Downmix`]): C → L + R at the
/// centre level; Ls → L at the surround level (or to Lss when `dst` has side
/// surrounds); Lss, Lrs → Ls at −3 dB; Ltf → L and Ltr → Lrs (or Ls) at −3 dB;
/// LFE → L + R at the LFE level (dropped by default); L, R → mono with the
/// stereo-to-mono sum; mono → C, or L + R with the pan law when `dst` has no C.
/// Channels `dst` has and `src` lacks stay silent. With normalisation, a
/// down-mix from 3.0 or wider into fewer channels is scaled so that its largest
/// row sum is unity.
pub fn width_matrix(src: Format, dst: Format, laws: Laws) -> Vec<f32> {
    let columns = src.channels();
    let mut m = vec![0f64; dst.channels() * columns];
    for (s, label) in src.labels().iter().enumerate() {
        fold(label, dst, 1.0, laws, &mut |d, g| m[d * columns + s] += g);
    }
    if laws.downmix.normalise && columns > 2 && dst.channels() < columns {
        let most = m
            .chunks(columns)
            .map(|row| row.iter().map(|g| g.abs()).sum::<f64>())
            .fold(0f64, f64::max);
        if most > 1.0 {
            m.iter_mut().for_each(|g| *g /= most);
        }
    }
    m.into_iter().map(|g| g as f32).collect()
}

/// A down-mix level as a gain: −3 dB is 1/√2 exactly (BS.775's 0.7071), as in 1.3.
fn level(db: Db) -> f64 {
    if db.0 == -3.0 {
        FRAC_1_SQRT_2
    } else {
        db.gain()
    }
}

fn fold(label: &str, dst: Format, gain: f64, laws: Laws, add: &mut dyn FnMut(usize, f64)) {
    if let Some(d) = dst.index(label) {
        add(d, gain);
        return;
    }
    let h = FRAC_1_SQRT_2;
    let mut to = |l: &str, g: f64| fold(l, dst, gain * g, laws, add);
    match label {
        "M" if dst.index("C").is_some() => to("C", 1.0),
        "M" => {
            let (l, r) = pan_gains(0.0, laws.pan);
            to("L", l);
            to("R", r);
        }
        "L" | "R" => to("M", db_to_gain(laws.mono_sum.into())),
        "C" => {
            let c = level(laws.downmix.centre_db);
            to("L", c);
            to("R", c);
        }
        "LFE" => {
            let g = level(laws.downmix.lfe_db);
            if g > 0.0 {
                to("L", g);
                to("R", g);
            }
        }
        "Ls" if dst.index("Lss").is_some() => to("Lss", 1.0),
        "Rs" if dst.index("Rss").is_some() => to("Rss", 1.0),
        "Ls" => to("L", level(laws.downmix.surround_db)),
        "Rs" => to("R", level(laws.downmix.surround_db)),
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
                ..Laws::default()
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
        // Mono into 3.0 and the surround formats goes to C.
        for dst in [Format::Lcr, Format::Surround51, Format::Surround714] {
            let m = width_matrix(Format::Mono, dst, laws);
            for (d, g) in m.iter().enumerate() {
                assert_eq!(*g, if d == 2 { 1.0 } else { 0.0 }, "{dst:?}");
            }
        }
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
    fn adjustable_down_mix() {
        let h = FRAC_1_SQRT_2;
        // C at −6 dB, surrounds at 0 dB, LFE at 0 dB into the front pair.
        let laws = Laws {
            downmix: Downmix {
                centre_db: Db(-6.0),
                surround_db: Db(0.0),
                lfe_db: Db(0.0),
                normalise: false,
            },
            ..Laws::default()
        };
        let m = width_matrix(Format::Surround51, Format::Stereo, laws);
        let c = Db(-6.0).gain();
        // L row: L, R, C, LFE, Ls, Rs
        let expected_l = [1.0, 0.0, c, 1.0, 1.0, 0.0];
        let expected_r = [0.0, 1.0, c, 1.0, 0.0, 1.0];
        for s in 0..6 {
            assert!(close(m[s].into(), expected_l[s]), "L {s}: {}", m[s]);
            assert!(close(m[6 + s].into(), expected_r[s]), "R {s}: {}", m[6 + s]);
        }
        // Normalised: the L row sums to 1 + 0.5 + 1 + 1 = 3.5 → every gain / 3.5.
        let normalised = Laws {
            downmix: Downmix {
                normalise: true,
                ..laws.downmix
            },
            ..laws
        };
        let n = width_matrix(Format::Surround51, Format::Stereo, normalised);
        let sum = 1.0 + c + 1.0 + 1.0;
        for (a, b) in n.iter().zip(&m) {
            assert!(close((*a).into(), f64::from(*b) / sum));
        }
        let row: f64 = n[..6].iter().map(|g| f64::from(*g)).sum();
        assert!(close(row, 1.0), "{row}");
        // The default BS.775 down-mix normalised: L + 0.707 C + 0.707 Ls → / 2.414.
        let d = Laws {
            downmix: Downmix {
                normalise: true,
                ..Downmix::default()
            },
            ..Laws::default()
        };
        let n = width_matrix(Format::Surround51, Format::Stereo, d);
        assert!(close(n[0].into(), 1.0 / (1.0 + 2.0 * h)));
        // −inf drops: no centre and no surrounds in the down-mix.
        let none = Laws {
            downmix: Downmix {
                centre_db: Db::MINUS_INF,
                surround_db: Db::MINUS_INF,
                ..Downmix::default()
            },
            ..Laws::default()
        };
        let m = width_matrix(Format::Surround51, Format::Stereo, none);
        assert_eq!(&m[..6], &[1.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        // Normalisation leaves up-mixes, same formats and stereo → mono alone.
        for (src, dst) in [
            (Format::Stereo, Format::Surround51),
            (Format::Surround51, Format::Surround51),
            (Format::Stereo, Format::Mono),
            (Format::Surround51, Format::Surround71),
        ] {
            assert_eq!(
                width_matrix(src, dst, d),
                width_matrix(src, dst, Laws::default()),
                "{src:?} → {dst:?}"
            );
        }
        // Validation and JSON.
        assert_eq!(Downmix::default().validate(), Ok(()));
        let bad = Downmix {
            centre_db: Db(1.0),
            ..Downmix::default()
        };
        assert!(bad.validate().unwrap_err().contains("centre_db"));
        let bad = Downmix {
            lfe_db: Db(12.0),
            ..Downmix::default()
        };
        assert!(bad.validate().is_err());
        let back: Downmix = serde_json::from_str("{}").unwrap();
        assert_eq!(back, Downmix::default());
        assert_eq!(
            serde_json::to_value(Downmix::default()).unwrap(),
            serde_json::json!({"centre_db": -3.0, "surround_db": -3.0, "lfe_db": "-inf", "normalise": false})
        );
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
