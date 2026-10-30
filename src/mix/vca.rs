//! VCA (DCA) groups and mute groups (§9).
//!
//! A VCA sums no audio: each member's fader gets the VCA's fader added (dB), and a
//! muted VCA mutes its members without changing their own mute. VCAs nest (a VCA
//! can be a member of another); a member gets the offset of every VCA above it,
//! each counted once; cycles are rejected. A VCA at −∞ silences its members; a
//! fader and its VCAs add up to at most [`MAX_FADER_DB`].
//!
//! A mute group mutes its members while it is active. A `mute_safe` (isolated)
//! strip or bus is never muted by a mute group or a VCA. Neither changes the
//! plan: only fader values move.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::model::{check_fader, Db, Mix, MAX_FADER_DB};

pub const MAX_VCAS: usize = 64;
pub const MAX_MUTE_GROUPS: usize = 32;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Vca {
    pub id: String,
    pub name: String,
    #[serde(default = "zero")]
    pub fader_db: Db,
    #[serde(default)]
    pub mute: bool,
    /// Strips, buses and other VCAs.
    #[serde(default)]
    pub members: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colour: Option<String>,
    #[serde(default)]
    pub recall_safe: bool,
}

fn zero() -> Db {
    Db(0.0)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MuteGroup {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub active: bool,
    /// Strips and buses.
    #[serde(default)]
    pub members: Vec<String>,
}

/// What the VCAs do to one signal: the sum of their offsets (−∞ when one is at
/// −∞) and whether one of them is muted.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Effect {
    pub offset_db: f64,
    pub muted: bool,
}

impl Mix {
    pub fn vca(&self, id: &str) -> Option<&Vca> {
        self.vcas.iter().find(|v| v.id == id)
    }

    /// Every VCA above `member` (directly or through nested VCAs), each once.
    pub fn vcas_of(&self, member: &str) -> BTreeSet<String> {
        let mut found = BTreeSet::new();
        let mut todo = vec![member.to_string()];
        while let Some(m) = todo.pop() {
            for v in self.vcas.iter().filter(|v| v.members.contains(&m)) {
                if found.insert(v.id.clone()) {
                    todo.push(v.id.clone());
                }
            }
        }
        found
    }

    /// The strips and buses under VCA `id` (through nested VCAs).
    pub fn vca_members(&self, id: &str) -> BTreeSet<String> {
        let mut signals = BTreeSet::new();
        let mut seen = BTreeSet::new();
        let mut todo = vec![id.to_string()];
        while let Some(v) = todo.pop() {
            if !seen.insert(v.clone()) {
                continue;
            }
            let Some(vca) = self.vca(&v) else { continue };
            for m in &vca.members {
                if self.vca(m).is_some() {
                    todo.push(m.clone());
                } else if self.kind(m).is_some() {
                    signals.insert(m.clone());
                }
            }
        }
        signals
    }

    /// The VCAs' effect on every signal that has one.
    pub fn vca_effects(&self) -> BTreeMap<String, Effect> {
        let mut out = BTreeMap::new();
        let ids = self
            .strips
            .iter()
            .map(|s| &s.id)
            .chain(self.buses.iter().map(|b| &b.id));
        for id in ids {
            let above = self.vcas_of(id);
            if above.is_empty() {
                continue;
            }
            let mut offset = 0.0;
            let mut muted = false;
            for v in above.iter().filter_map(|v| self.vca(v)) {
                offset += v.fader_db.0;
                muted |= v.mute;
            }
            out.insert(
                id.clone(),
                Effect {
                    offset_db: offset,
                    muted,
                },
            );
        }
        out
    }

    /// The signals that active mute groups mute (members that are not mute-safe).
    pub fn group_muted(&self) -> BTreeSet<String> {
        self.mute_groups
            .iter()
            .filter(|g| g.active)
            .flat_map(|g| g.members.iter())
            .filter(|m| !self.mute_safe(m))
            .cloned()
            .collect()
    }

    /// The gain of a signal's fader node: its fader, mute, the VCAs and the mute
    /// groups (a mute-safe signal is muted only by its own mute).
    pub fn fader_gain(
        &self,
        id: &str,
        fader: Db,
        mute: bool,
        effects: &BTreeMap<String, Effect>,
        grouped: &BTreeSet<String>,
    ) -> f32 {
        if self.silenced(id, mute, effects, grouped) {
            return 0.0;
        }
        self.fader_level(id, fader, effects)
    }

    /// A signal's fader and its VCAs' offsets as a gain, without the mutes.
    pub fn fader_level(&self, id: &str, fader: Db, effects: &BTreeMap<String, Effect>) -> f32 {
        let offset = effects.get(id).map_or(0.0, |e| e.offset_db);
        Db((fader.0 + offset).min(MAX_FADER_DB)).gain() as f32
    }

    /// Whether a signal is muted: by its own mute, a muted VCA (unless it is
    /// mute-safe) or an active mute group.
    pub fn silenced(
        &self,
        id: &str,
        mute: bool,
        effects: &BTreeMap<String, Effect>,
        grouped: &BTreeSet<String>,
    ) -> bool {
        let vca_muted = effects.get(id).is_some_and(|e| e.muted);
        mute || (vca_muted && !self.mute_safe(id)) || grouped.contains(id)
    }

    fn mute_safe(&self, id: &str) -> bool {
        self.strip(id).is_some_and(|s| s.mute_safe) || self.bus(id).is_some_and(|b| b.mute_safe)
    }
}

pub(crate) fn check_id(what: &str, id: &str) -> Result<(), String> {
    let ok = !id.is_empty()
        && id.len() <= 32
        && !id.starts_with('-')
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if ok {
        Ok(())
    } else {
        Err(format!(
            "{what} id {id:?} must be 1 to 32 characters a-z, 0-9 and '-'"
        ))
    }
}

pub(crate) fn check_name(what: &str, name: &str) -> Result<(), String> {
    if name.trim().is_empty() || name.chars().count() > 64 {
        Err(format!("{what}: a name has 1 to 64 characters"))
    } else {
        Ok(())
    }
}

/// Check the VCAs and mute groups against `mix`.
pub fn validate(mix: &Mix) -> Result<(), String> {
    if mix.vcas.len() > MAX_VCAS {
        return Err(format!("at most {MAX_VCAS} VCAs"));
    }
    if mix.mute_groups.len() > MAX_MUTE_GROUPS {
        return Err(format!("at most {MAX_MUTE_GROUPS} mute groups"));
    }
    let mut ids = BTreeSet::new();
    for v in &mix.vcas {
        check_id("VCA", &v.id)?;
        if mix.kind(&v.id).is_some() {
            return Err(format!("VCA id {} is a strip or bus id", v.id));
        }
        if !ids.insert(v.id.as_str()) {
            return Err(format!("VCA id {} is used twice", v.id));
        }
        check_name(&format!("VCA {}", v.id), &v.name)?;
        check_fader(&format!("VCA {}: fader_db", v.id), v.fader_db)?;
        let mut seen = BTreeSet::new();
        for m in &v.members {
            if !seen.insert(m.as_str()) {
                return Err(format!("VCA {}: member {m} is listed twice", v.id));
            }
            if m == &v.id {
                return Err(format!("VCA {} cannot be its own member", v.id));
            }
            if mix.kind(m).is_none() && !mix.vcas.iter().any(|x| &x.id == m) {
                return Err(format!(
                    "VCA {}: member {m} is not a strip, bus or VCA",
                    v.id
                ));
            }
        }
    }
    // Nesting must not loop: a VCA above itself.
    for v in &mix.vcas {
        if mix.vcas_of(&v.id).contains(&v.id) {
            return Err(format!("VCA {} is nested in itself", v.id));
        }
    }
    let mut ids = BTreeSet::new();
    for g in &mix.mute_groups {
        check_id("mute group", &g.id)?;
        if !ids.insert(g.id.as_str()) {
            return Err(format!("mute group id {} is used twice", g.id));
        }
        check_name(&format!("mute group {}", g.id), &g.name)?;
        let mut seen = BTreeSet::new();
        for m in &g.members {
            if !seen.insert(m.as_str()) {
                return Err(format!("mute group {}: member {m} is listed twice", g.id));
            }
            if mix.kind(m).is_none() {
                return Err(format!(
                    "mute group {}: member {m} is not a strip or bus",
                    g.id
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mix::model::tests::{bus, strip};
    use crate::mix::Format;

    fn vca(id: &str, db: f64, members: &[&str]) -> Vca {
        Vca {
            id: id.into(),
            name: id.into(),
            fader_db: Db(db),
            mute: false,
            members: members.iter().map(|m| m.to_string()).collect(),
            colour: None,
            recall_safe: false,
        }
    }

    fn mix() -> Mix {
        let mut m = Mix::default();
        for id in ["vox", "gtr", "keys"] {
            m.strips.push(strip(id, Format::Mono));
        }
        m.buses.push(bus("main", Format::Stereo));
        m
    }

    #[test]
    fn offsets_nesting_and_mutes() {
        let mut m = mix();
        // band (−6): gtr, keys; all (−4): band, vox, and keys again (counted once).
        m.vcas.push(vca("band", -6.0, &["gtr", "keys"]));
        m.vcas.push(vca("all", -4.0, &["band", "vox", "keys"]));
        assert_eq!(validate(&m), Ok(()));
        let e = m.vca_effects();
        assert_eq!(e["vox"].offset_db, -4.0);
        assert_eq!(e["gtr"].offset_db, -10.0);
        assert_eq!(e["keys"].offset_db, -10.0, "each VCA once");
        assert!(!e.contains_key("main"));
        assert_eq!(
            m.vca_members("all"),
            BTreeSet::from(["gtr".into(), "keys".into(), "vox".into()])
        );
        let g = m.group_muted();
        let gain = |m: &Mix, id: &str| m.fader_gain(id, Db(0.0), false, &m.vca_effects(), &g);
        assert!((gain(&m, "gtr") - 0.316_227_8).abs() < 1e-6);
        m.vcas[1].mute = true;
        assert_eq!(gain(&m, "vox"), 0.0);
        assert_eq!(
            gain(&m, "gtr"),
            0.0,
            "a muted outer VCA mutes nested members"
        );
        m.vcas[1].mute = false;
        m.vcas[0].fader_db = Db::MINUS_INF;
        assert_eq!(gain(&m, "keys"), 0.0);
        assert!((gain(&m, "vox") - 0.630_957_3).abs() < 1e-6);

        m.mute_groups.push(MuteGroup {
            id: "band".into(),
            name: "Band".into(),
            active: true,
            members: vec!["gtr".into(), "keys".into()],
        });
        m.strips[2].mute_safe = true;
        assert_eq!(m.group_muted(), BTreeSet::from(["gtr".into()]));
        // Mute-safe also holds against a VCA's mute.
        m.mute_groups.clear();
        m.vcas[0].fader_db = Db(0.0);
        m.vcas[0].mute = true;
        let g = m.group_muted();
        assert_eq!(gain(&m, "gtr"), 0.0);
        assert!(gain(&m, "keys") > 0.0, "keys is mute-safe");
        // A fader and its VCAs add up to at most +15 dB.
        m.vcas[0].mute = false;
        m.vcas[0].fader_db = Db(15.0);
        m.vcas[1].fader_db = Db(15.0);
        let capped = m.fader_gain("gtr", Db(15.0), false, &m.vca_effects(), &g);
        assert!((capped - 5.623_413).abs() < 1e-5, "{capped}");
    }

    #[test]
    fn validation() {
        let mut m = mix();
        m.vcas.push(vca("a", 0.0, &["vox"]));
        m.vcas.push(vca("b", 0.0, &["a"]));
        assert_eq!(validate(&m), Ok(()));
        let bad = |f: &dyn Fn(&mut Mix)| {
            let mut x = m.clone();
            f(&mut x);
            validate(&x).is_err()
        };
        assert!(bad(&|m| m.vcas[0].members.push("b".into())), "a cycle");
        assert!(bad(&|m| m.vcas[0].members.push("a".into())), "itself");
        assert!(bad(&|m| m.vcas[0].members.push("nope".into())));
        assert!(bad(&|m| m.vcas[0].members.push("vox".into())), "twice");
        assert!(bad(&|m| m.vcas[0].id = "vox".into()), "a signal's id");
        assert!(bad(&|m| m.vcas[1].id = "a".into()));
        assert!(!bad(&|m| m.vcas[0].fader_db = Db(15.0)));
        assert!(bad(&|m| m.vcas[0].fader_db = Db(15.5)));
        assert!(
            bad(&|m| m.mute_groups.push(MuteGroup {
                id: "g".into(),
                name: "G".into(),
                active: false,
                members: vec!["a".into()],
            })),
            "a VCA in a mute group"
        );
    }
}
