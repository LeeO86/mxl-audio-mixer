//! Named down-mix settings (C8, 1.6.0): a gain matrix for every pair of
//! formats, as rows of the narrower format's channels and columns of the wider
//! one's. The matrix of a pair starts from the reference rules of Adrian's
//! `downmix.json` (`assets/downmix.json`) and may be edited cell by cell; the
//! edited matrices are stored, the others computed.
//!
//! The rules give each source channel a list of alternatives, each a set of
//! target channels with their gains: the first alternative whose channels the
//! target format all has is taken (a channel without one is dropped). The
//! mixer's 5.1 surrounds Ls / Rs are the rules' Lrs / Rrs, mono M is C.
//!
//! A set applies where a wider format meets one that lacks some of its
//! channels (not from mono, nor stereo into mono: the pan law and the
//! stereo-to-mono sum stay); a mix without sets keeps the parametric down-mix
//! of 1.4 ([`super::format::Downmix`]), so a show of 1.5 sounds as before.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use super::format::Format;
use super::model::Db;

/// The reference, verbatim.
pub const REFERENCE_JSON: &str = include_str!("../../assets/downmix.json");

/// Which rules of `downmix.json` a set starts from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Rules {
    /// `Downmix`.
    #[default]
    Reference,
    /// `Downmix` with `Downmix_Surround_To_Side` (the 5.1 surrounds to the sides).
    SurroundToSide,
    /// `Downmix` with `Downmix_Surround_To_SideAndRear` (to sides and rears).
    SurroundToSideAndRear,
}

impl Rules {
    pub const ALL: [Rules; 3] = [
        Rules::Reference,
        Rules::SurroundToSide,
        Rules::SurroundToSideAndRear,
    ];

    fn section(self) -> Option<&'static str> {
        match self {
            Rules::Reference => None,
            Rules::SurroundToSide => Some("Downmix_Surround_To_Side"),
            Rules::SurroundToSideAndRear => Some("Downmix_Surround_To_SideAndRear"),
        }
    }
}

/// One edited matrix of a set: `gains_db[row][column]`, rows the channels of
/// `to`, columns those of `from`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Matrix {
    pub from: Format,
    pub to: Format,
    pub gains_db: Vec<Vec<Db>>,
}

/// A named down-mix setting.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DownmixSet {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub rules: Rules,
    /// Scale a down-mix so that no output channel sums to more than unity.
    #[serde(default)]
    pub normalise: bool,
    /// The edited matrices; every other pair follows `rules`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub matrices: Vec<Matrix>,
}

/// Highest gain of a matrix cell.
pub const MAX_CELL_DB: f64 = 10.0;

impl DownmixSet {
    /// The set a new show starts with: the reference.
    pub fn reference() -> Self {
        Self {
            id: "reference".into(),
            name: "downmix.json".into(),
            rules: Rules::Reference,
            normalise: false,
            matrices: Vec::new(),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        let at = |e: String| format!("downmix {}: {e}", self.id);
        super::model::check_id(&self.id).map_err(at)?;
        if self.name.trim().is_empty() || self.name.len() > 64 {
            return Err(at("name: 1 to 64 characters".into()));
        }
        let mut pairs = std::collections::BTreeSet::new();
        for m in &self.matrices {
            let pair = (
                m.from.channels(),
                m.to.channels(),
                format!("{:?}{:?}", m.from, m.to),
            );
            if !pairs.insert(pair) {
                return Err(at(format!("two matrices for {:?} to {:?}", m.from, m.to)));
            }
            if !applies(m.from, m.to) {
                return Err(at(format!("{:?} to {:?} is no down-mix", m.from, m.to)));
            }
            if m.gains_db.len() != m.to.channels()
                || m.gains_db.iter().any(|r| r.len() != m.from.channels())
            {
                return Err(at(format!(
                    "the {:?} to {:?} matrix has {} rows of {} gains",
                    m.from,
                    m.to,
                    m.to.channels(),
                    m.from.channels()
                )));
            }
            for g in m.gains_db.iter().flatten() {
                if *g != Db::MINUS_INF && !(-60.0..=MAX_CELL_DB).contains(&g.0) {
                    return Err(at(format!("gains are -inf or -60 to {MAX_CELL_DB} dB")));
                }
            }
        }
        Ok(())
    }

    /// The matrix from `src` to `dst` (row-major, `dst` rows), linear gains:
    /// the edited one or the rules'.
    pub fn matrix(&self, src: Format, dst: Format) -> Vec<f64> {
        let mut m = match self.matrices.iter().find(|m| m.from == src && m.to == dst) {
            Some(m) => m.gains_db.iter().flatten().map(|g| gain(*g)).collect(),
            None => rules_matrix(self.rules, src, dst),
        };
        if self.normalise {
            let columns = src.channels();
            let most = m
                .chunks(columns)
                .map(|row| row.iter().map(|g| g.abs()).sum::<f64>())
                .fold(0f64, f64::max);
            if most > 1.0 {
                m.iter_mut().for_each(|g| *g /= most);
            }
        }
        m
    }
}

/// Whether a set maps `src` into `dst`: `dst` lacks a channel of `src`, and
/// neither the pan law (from mono) nor the stereo-to-mono sum does it.
pub fn applies(src: Format, dst: Format) -> bool {
    !matches!(
        (src, dst),
        (Format::Mono, _) | (Format::Stereo, Format::Mono)
    ) && src.labels().iter().any(|l| !dst.labels().contains(l))
}

/// −3 dB is 1/√2 exactly, as the parametric down-mix's (and BS.775's 0.7071).
fn gain(db: Db) -> f64 {
    if db.0 == -3.0 {
        std::f64::consts::FRAC_1_SQRT_2
    } else {
        db.gain()
    }
}

/// The rules' name of one of our channels.
fn rule_label(label: &str) -> &str {
    match label {
        "Ls" => "Lrs",
        "Rs" => "Rrs",
        "M" => "C",
        other => other,
    }
}

type Alternatives = Vec<BTreeMap<String, f64>>;

fn parsed() -> &'static BTreeMap<String, BTreeMap<String, Alternatives>> {
    static RULES: OnceLock<BTreeMap<String, BTreeMap<String, Alternatives>>> = OnceLock::new();
    RULES
        .get_or_init(|| serde_json::from_str(REFERENCE_JSON).expect("assets/downmix.json is valid"))
}

/// The alternatives of `label` under `rules`.
fn alternatives(rules: Rules, label: &str) -> &'static [BTreeMap<String, f64>] {
    let all = parsed();
    rules
        .section()
        .and_then(|s| all.get(s)?.get(label))
        .or_else(|| all.get("Downmix")?.get(label))
        .map_or(&[], Vec::as_slice)
}

/// The matrix of `rules` from `src` to `dst` (linear, row-major).
pub fn rules_matrix(rules: Rules, src: Format, dst: Format) -> Vec<f64> {
    let columns = src.channels();
    let index = |name: &str| dst.labels().iter().position(|l| rule_label(l) == name);
    let mut m = vec![0.0; dst.channels() * columns];
    for (s, label) in src.labels().iter().enumerate() {
        let take = alternatives(rules, rule_label(label))
            .iter()
            .find(|alt| alt.keys().all(|k| index(k).is_some()));
        for (k, db) in take.into_iter().flatten() {
            let d = index(k).expect("checked");
            m[d * columns + s] += gain(Db(*db));
        }
    }
    m
}

/// The set's matrix of a pair in dB, as the UI and the API show it.
pub fn matrix_db(set: &DownmixSet, src: Format, dst: Format) -> Vec<Vec<Db>> {
    let columns = src.channels();
    let m = DownmixSet {
        normalise: false,
        ..set.clone()
    }
    .matrix(src, dst);
    m.chunks(columns)
        .map(|row| {
            row.iter()
                .map(|g| {
                    if *g <= 0.0 {
                        Db::MINUS_INF
                    } else {
                        Db(20.0 * g.log10())
                    }
                })
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(m: &[f64], src: Format, dst: Format, from: &str, to: &str) -> f64 {
        let s = src.labels().iter().position(|l| *l == from).unwrap();
        let d = dst.labels().iter().position(|l| *l == to).unwrap();
        m[d * src.channels() + s]
    }

    fn db(g: f64) -> f64 {
        20.0 * g.log10()
    }

    #[test]
    fn the_reference_follows_downmix_json() {
        let r = Rules::Reference;
        let m = rules_matrix(r, Format::Surround51, Format::Stereo);
        let (s, d) = (Format::Surround51, Format::Stereo);
        assert_eq!(at(&m, s, d, "L", "L"), 1.0);
        assert!((at(&m, s, d, "C", "L") - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-12);
        assert!(
            (db(at(&m, s, d, "Ls", "L")) + 6.0206).abs() < 1e-3,
            "Lrs → L −6.02"
        );
        assert_eq!(at(&m, s, d, "Ls", "R"), 0.0);
        assert!((at(&m, s, d, "LFE", "L") - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-12);
        // 7.1.4 into 5.1: sides to L and the surround at −3, heights to the front.
        let (s, d) = (Format::Surround714, Format::Surround51);
        let m = rules_matrix(r, s, d);
        assert!((db(at(&m, s, d, "Lss", "L")) + 3.0103).abs() < 1e-3);
        assert!((db(at(&m, s, d, "Lss", "Ls")) + 3.0103).abs() < 1e-3);
        assert_eq!(at(&m, s, d, "Lrs", "Ls"), 1.0);
        assert!((db(at(&m, s, d, "Ltf", "L")) + 3.0103).abs() < 1e-3);
        assert!((db(at(&m, s, d, "Ltr", "Ls")) + 3.0103).abs() < 1e-3);
        // Mono is C: L and R at −6.02, C at 0, LFE at 0.
        let (s, d) = (Format::Surround51, Format::Mono);
        let m = rules_matrix(r, s, d);
        assert!((at(&m, s, d, "L", "M") - 0.5).abs() < 1e-9);
        assert_eq!(at(&m, s, d, "C", "M"), 1.0);
        assert_eq!(at(&m, s, d, "LFE", "M"), 1.0);
        // 5.1 into 7.1: to the rears, or to the sides.
        let (s, d) = (Format::Surround51, Format::Surround71);
        assert_eq!(at(&rules_matrix(r, s, d), s, d, "Ls", "Lrs"), 1.0);
        let side = rules_matrix(Rules::SurroundToSide, s, d);
        assert_eq!(at(&side, s, d, "Ls", "Lss"), 1.0);
        assert_eq!(at(&side, s, d, "Ls", "Lrs"), 0.0);
        let both = rules_matrix(Rules::SurroundToSideAndRear, s, d);
        assert!((at(&both, s, d, "Ls", "Lss") - 0.5).abs() < 1e-9);
        assert!((at(&both, s, d, "Ls", "Lrs") - 0.5).abs() < 1e-9);
    }

    #[test]
    fn edited_matrices_win_and_are_checked() {
        let mut set = DownmixSet::reference();
        assert_eq!(set.validate(), Ok(()));
        let mut g = matrix_db(&set, Format::Surround51, Format::Stereo);
        assert!((g[0][4].0 + 6.0206).abs() < 1e-3);
        g[0][4] = Db(-3.0);
        set.matrices.push(Matrix {
            from: Format::Surround51,
            to: Format::Stereo,
            gains_db: g.clone(),
        });
        assert_eq!(set.validate(), Ok(()));
        let m = set.matrix(Format::Surround51, Format::Stereo);
        assert!((m[4] - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-12);
        set.matrices[0].gains_db.pop();
        assert!(set.validate().is_err());
        set.matrices[0].gains_db = g;
        set.matrices[0].to = Format::Surround71;
        assert!(set.validate().is_err(), "rows of the wrong format");
        assert!(!applies(Format::Stereo, Format::Surround51));
        assert!(!applies(Format::Stereo, Format::Mono));
        assert!(applies(Format::Surround51, Format::Surround71));
    }
}
