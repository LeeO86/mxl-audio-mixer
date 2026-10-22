//! High-level operations (§5) that generate ordinary routing underneath (§1: "the
//! operator sees a console; the engine sees a graph"):
//! - **group:** strips → a new subgroup bus (flagged `subgroup`); their
//!   post-fader sends move to it, and it sends where they sent;
//! - **N-1:** per participant (a bus each with the sum of all others) or one of
//!   the selection; from a bus (a mix-minus that follows it) or from the
//!   selection's strips (sends of their own);
//! - **mix-minus:** a bus that takes every send into another bus except some;
//! - **link:** mono strips → one stereo or surround strip (shared processing and
//!   fader); **unlink:** a stereo strip → two mono strips.
//!
//! Each returns what it made; the caller validates and applies the result.

use serde::{Deserialize, Serialize};

use super::model::{Bus, Db, Kind, Minus, Mix, Send, Strip, Tap};
use super::room::Source;
use super::Format;

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Made {
    pub buses: Vec<String>,
    pub strips: Vec<String>,
    pub removed: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum N1Mode {
    /// An N-1 bus per participant: everything but that participant.
    #[default]
    PerParticipant,
    /// One N-1 bus: everything but the selection.
    One,
}

fn strips_of<'a>(mix: &'a Mix, ids: &[String]) -> Result<Vec<&'a Strip>, String> {
    if ids.is_empty() {
        return Err("select at least one strip".into());
    }
    let mut seen = std::collections::BTreeSet::new();
    ids.iter()
        .map(|id| {
            if !seen.insert(id) {
                return Err(format!("strip {id} is selected twice"));
            }
            mix.strip(id)
                .ok_or_else(|| format!("strip {id} does not exist"))
        })
        .collect()
}

fn free_id(mix: &Mix, wanted: Option<String>, prefix: &str) -> Result<String, String> {
    match wanted {
        Some(id) if mix.kind(&id).is_some() || mix.vca(&id).is_some() => {
            Err(format!("signal {id} exists"))
        }
        Some(id) => Ok(id),
        None => Ok(mix.next_id(prefix)),
    }
}

fn send(from: &str, to: &str, tap: Tap) -> Send {
    Send {
        from: from.to_string(),
        to: to.to_string(),
        tap,
        gain_db: Db(0.0),
        mute: false,
        phase: false,
        block_delay: false,
    }
}

fn after_fader(tap: &Tap) -> bool {
    matches!(tap, Tap::PostFader | Tap::PostPan)
}

/// Group `strips` into a new bus: their post-fader (and post-pan) sends move to
/// the group, which sends to each of those buses; pre-fader sends (auxes) stay.
/// Refused when a mix-minus of one of those buses leaves out one of the strips:
/// through the group it would be back in.
pub fn group(
    mix: &mut Mix,
    strips: &[String],
    id: Option<String>,
    name: Option<String>,
    format: Option<Format>,
) -> Result<Made, String> {
    strips_of(mix, strips)?;
    let moving: Vec<&str> = mix
        .sends
        .iter()
        .filter(|s| strips.contains(&s.from) && after_fader(&s.tap))
        .map(|s| s.to.as_str())
        .collect();
    let broken: Vec<&str> = mix
        .buses
        .iter()
        .filter(|b| {
            b.minus.as_ref().is_some_and(|m| {
                moving.contains(&m.of.as_str()) && m.without.iter().any(|w| strips.contains(w))
            })
        })
        .map(|b| b.id.as_str())
        .collect();
    if !broken.is_empty() {
        return Err(format!(
            "the mix-minus {} leaves out a strip of the group; through the group it would be back in",
            broken.join(", ")
        ));
    }
    let id = free_id(mix, id, "group")?;
    let mut bus = Bus::new(&id, format.unwrap_or(Format::Stereo));
    if let Some(n) = name {
        bus.name = n;
    }
    // Its sends are post-pan at 0 dB: a subgroup (C12).
    bus.subgroup = true;
    let mut targets: Vec<String> = Vec::new();
    mix.sends.retain(|s| {
        let moved = strips.contains(&s.from) && after_fader(&s.tap);
        if moved && !targets.contains(&s.to) {
            targets.push(s.to.clone());
        }
        !moved
    });
    mix.buses.push(bus);
    for s in strips {
        mix.sends.push(send(s, &id, Tap::PostPan));
    }
    for t in targets {
        mix.sends.push(send(&id, &t, Tap::PostFader));
    }
    Ok(Made {
        buses: vec![id],
        ..Made::default()
    })
}

/// N-1 buses of `strips`. With `of` a bus, they are mix-minus buses of it (they
/// follow its sends); without, they get sends from the selection's other strips.
pub fn n_minus_one(
    mix: &mut Mix,
    strips: &[String],
    of: Option<String>,
    mode: N1Mode,
    format: Option<Format>,
) -> Result<Made, String> {
    let names: Vec<(String, String)> = strips_of(mix, strips)?
        .iter()
        .map(|s| (s.id.clone(), s.name.clone()))
        .collect();
    if let Some(b) = &of {
        if mix.kind(b) != Some(Kind::Bus) {
            return Err(format!("{b} is not a bus"));
        }
    }
    let format = format
        .or_else(|| of.as_ref().and_then(|b| mix.bus(b)).map(|b| b.format))
        .unwrap_or(Format::Mono);
    let mut made = Made::default();
    match mode {
        N1Mode::PerParticipant => {
            for (p, name) in &names {
                let id = free_id(mix, Some(format!("n1-{p}")), "n1")
                    .or_else(|_| free_id(mix, None, "n1"))?;
                let mut bus = Bus::new(&id, format);
                bus.name = format!("N-1 {name}");
                match &of {
                    Some(b) => {
                        bus.minus = Some(Minus {
                            of: b.clone(),
                            without: vec![p.clone()],
                        })
                    }
                    None => {
                        for (q, _) in names.iter().filter(|(q, _)| q != p) {
                            mix.sends.push(send(q, &id, Tap::PostPan));
                        }
                    }
                }
                mix.buses.push(bus);
                made.buses.push(id);
            }
        }
        N1Mode::One => {
            let b = of.ok_or("one N-1 of the selection needs the bus it is taken from (of)")?;
            let id = free_id(mix, None, "n1")?;
            let mut bus = Bus::new(&id, format);
            bus.name = format!(
                "{} minus selection",
                mix.bus(&b).map(|x| x.name.as_str()).unwrap_or(&b)
            );
            bus.minus = Some(Minus {
                of: b,
                without: strips.to_vec(),
            });
            mix.buses.push(bus);
            made.buses.push(id);
        }
    }
    Ok(made)
}

/// A new bus that takes every send into `of` except those from `without`.
pub fn mix_minus(
    mix: &mut Mix,
    of: &str,
    without: &[String],
    id: Option<String>,
    name: Option<String>,
) -> Result<Made, String> {
    let base = mix
        .bus(of)
        .ok_or_else(|| format!("bus {of} does not exist"))?;
    let format = base.format;
    let base_name = base.name.clone();
    if let Some(w) = without.iter().find(|w| mix.kind(w).is_none()) {
        return Err(format!("signal {w} does not exist"));
    }
    let id = free_id(mix, id, "minus")?;
    let mut bus = Bus::new(&id, format);
    bus.name = name.unwrap_or_else(|| {
        let w: Vec<&str> = without
            .iter()
            .map(|w| mix.signal(w).map(|(n, _)| n).unwrap_or(w.as_str()))
            .collect();
        format!("{base_name} minus {}", w.join(", "))
    });
    bus.minus = Some(Minus {
        of: of.to_string(),
        without: without.to_vec(),
    });
    mix.buses.push(bus);
    Ok(Made {
        buses: vec![id],
        ..Made::default()
    })
}

/// The format of `n` linked mono strips.
pub fn link_format(n: usize) -> Option<Format> {
    [
        Format::Stereo,
        Format::Lcr,
        Format::Surround51,
        Format::Surround71,
        Format::Surround514,
        Format::Surround714,
    ]
    .into_iter()
    .find(|f| f.channels() == n)
}

/// Replace `old` by `new` (all of `new`, in its place) wherever the show names a
/// signal: layouts, VCAs, mute groups, monitor sources, talkback returns.
fn replace_refs(mix: &mut Mix, old: &[String], new: &[String]) {
    let swap = |list: &mut Vec<String>| {
        if let Some(at) = list.iter().position(|x| old.contains(x)) {
            list.retain(|x| !old.contains(x));
            for (k, n) in new.iter().enumerate() {
                if !list.contains(n) {
                    list.insert((at + k).min(list.len()), n.clone());
                }
            }
        }
    };
    for p in mix.layouts.iter_mut().flat_map(|l| l.pages.iter_mut()) {
        swap(&mut p.signals);
    }
    for v in &mut mix.vcas {
        swap(&mut v.members);
    }
    for g in &mut mix.mute_groups {
        swap(&mut g.members);
    }
    mix.replace_auto_refs(old, new);
    let first = new.first().cloned();
    let fix = |s: &mut Option<Source>| {
        if let (Some(Source::Signal(id)), Some(n)) = (s.as_ref(), &first) {
            if old.contains(id) {
                *s = Some(Source::Signal(n.clone()));
            }
        }
    };
    for m in &mut mix.control_room.monitors {
        fix(&mut m.source);
    }
    for d in &mut mix.control_room.talkback.destinations {
        fix(&mut d.return_feed);
    }
    for s in &mut mix.control_room.sources {
        if let (Source::Signal(id), Some(n)) = (&*s, &first) {
            if old.contains(id) {
                *s = Source::Signal(n.clone());
            }
        }
    }
    mix.control_room.sources.dedup();
}

/// Link mono `strips` into one strip of `format` (by default the format of that
/// many channels): their patches in order, the first strip's processing, fader,
/// trim and settings, and a send to every bus any of them sent to (the first
/// one's level). The mono strips are removed.
pub fn link(
    mix: &mut Mix,
    strips: &[String],
    format: Option<Format>,
    id: Option<String>,
    name: Option<String>,
) -> Result<Made, String> {
    let parts: Vec<Strip> = strips_of(mix, strips)?.into_iter().cloned().collect();
    if let Some(s) = parts.iter().find(|s| s.format != Format::Mono) {
        return Err(format!("strip {} is not mono", s.id));
    }
    let format = match format {
        Some(f) if f.channels() == parts.len() => f,
        Some(f) => {
            return Err(format!(
                "{} strips do not make {}",
                parts.len(),
                serde_json::to_string(&f).unwrap_or_default()
            ))
        }
        None => link_format(parts.len())
            .ok_or_else(|| format!("{} strips make no channel format", parts.len()))?,
    };
    let first = &parts[0];
    let id = match id {
        Some(i) if strips.contains(&i) => i,
        other => free_id(mix, other, "link")?,
    };
    let mut linked = first.clone();
    linked.id = id.clone();
    linked.name = name.unwrap_or_else(|| first.name.clone());
    linked.format = format;
    linked.patch = parts.iter().map(|s| s.patch[0].clone()).collect();
    linked.polarity = parts.iter().map(|s| s.polarity[0]).collect();
    linked.pan = 0.0;
    linked.width = 1.0;
    let mut sends: Vec<Send> = Vec::new();
    for s in parts.iter() {
        for x in mix.sends.iter().filter(|x| x.from == s.id) {
            if !sends.iter().any(|y| y.to == x.to) {
                let mut y = x.clone();
                y.from = id.clone();
                sends.push(y);
            }
        }
    }
    mix.sends.retain(|x| !strips.contains(&x.from));
    let at = mix
        .strips
        .iter()
        .position(|s| s.id == first.id)
        .unwrap_or(0);
    mix.strips.retain(|s| !strips.contains(&s.id));
    mix.strips.insert(at.min(mix.strips.len()), linked);
    mix.sends.extend(sends);
    replace_refs(mix, strips, std::slice::from_ref(&id));
    for b in &mut mix.buses {
        if let Some(m) = &mut b.minus {
            if m.without.iter().any(|w| strips.contains(w)) {
                m.without.retain(|w| !strips.contains(w));
                m.without.push(id.clone());
            }
        }
    }
    Ok(Made {
        strips: vec![id.clone()],
        removed: strips.iter().filter(|s| **s != id).cloned().collect(),
        ..Made::default()
    })
}

/// Split stereo `strip` into two mono strips `<id>-l` and `<id>-r` (hard left and
/// right), each with its processing, fader and sends.
pub fn unlink(mix: &mut Mix, strip: &str) -> Result<Made, String> {
    let s = mix
        .strip(strip)
        .ok_or_else(|| format!("strip {strip} does not exist"))?
        .clone();
    if s.format != Format::Stereo {
        return Err(format!("strip {strip} is not stereo"));
    }
    let ids = [format!("{strip}-l"), format!("{strip}-r")];
    for id in &ids {
        if mix.kind(id).is_some() || mix.vca(id).is_some() {
            return Err(format!("signal {id} exists"));
        }
    }
    let at = mix.strips.iter().position(|x| x.id == strip).unwrap_or(0);
    let sends: Vec<Send> = mix
        .sends
        .iter()
        .filter(|x| x.from == strip)
        .cloned()
        .collect();
    mix.sends.retain(|x| x.from != strip);
    mix.strips.retain(|x| x.id != strip);
    for (k, (id, pan)) in ids.iter().zip([-1.0, 1.0]).enumerate() {
        let mut m = s.clone();
        m.id = id.clone();
        m.name = format!("{} {}", s.name, ["L", "R"][k]);
        m.format = Format::Mono;
        m.patch = vec![s.patch[k].clone()];
        m.polarity = vec![s.polarity[k]];
        m.pan = pan;
        m.width = 1.0;
        mix.strips.insert((at + k).min(mix.strips.len()), m);
        for x in &sends {
            let mut y = x.clone();
            y.from = id.clone();
            mix.sends.push(y);
        }
    }
    replace_refs(mix, &[strip.to_string()], &ids);
    for b in &mut mix.buses {
        if let Some(m) = &mut b.minus {
            if m.without.iter().any(|w| w == strip) {
                m.without.retain(|w| w != strip);
                m.without.extend(ids.iter().cloned());
            }
        }
    }
    Ok(Made {
        strips: ids.to_vec(),
        removed: vec![strip.to_string()],
        ..Made::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mix::model::Patch;

    fn patched(input: usize, channel: usize) -> Option<Patch> {
        Some(crate::mix::model::PatchPoint { input, channel }.into())
    }

    fn mix() -> Mix {
        let mut m = Mix::default();
        for (i, id) in ["host", "guest", "phone"].iter().enumerate() {
            let mut s = Strip::new(id, Format::Mono);
            s.patch[0] = patched(1, i + 1);
            m.strips.push(s);
        }
        m.buses.push(Bus::new("main", Format::Stereo));
        m.buses.push(Bus::new("aux", Format::Mono));
        for id in ["host", "guest", "phone"] {
            m.sends.push(send(id, "main", Tap::PostPan));
            m.sends.push(send(id, "aux", Tap::PreFader));
        }
        m
    }

    fn ids(v: &[String]) -> Vec<&str> {
        v.iter().map(String::as_str).collect()
    }

    #[test]
    fn group_moves_the_post_fader_sends() {
        let mut m = mix();
        let made = group(
            &mut m,
            &["host".into(), "guest".into()],
            None,
            Some("Talent".into()),
            None,
        )
        .unwrap();
        assert_eq!(made.buses, ["group-1"]);
        assert_eq!(m.validate(4, 96), Ok(()));
        assert!(m.send("host", "main").is_none(), "moved");
        assert!(m.send("host", "aux").is_some(), "the pre-fader aux stays");
        assert!(m.send("host", "group-1").is_some());
        assert!(m.is_subgroup("group-1"));
        assert!(m.send("group-1", "main").is_some());
        assert!(m.send("phone", "main").is_some());
        // A mix-minus of main without phone: grouping phone is refused.
        mix_minus(&mut m, "main", &["phone".into()], None, None).unwrap();
        let err = group(&mut m, &["phone".into()], None, None, None).unwrap_err();
        assert!(err.contains("minus-1"), "{err}");
    }

    #[test]
    fn n1_per_participant_and_one() {
        let mut m = mix();
        let sel: Vec<String> = vec!["host".into(), "guest".into(), "phone".into()];
        let made = n_minus_one(
            &mut m,
            &sel,
            Some("main".into()),
            N1Mode::PerParticipant,
            Some(Format::Mono),
        )
        .unwrap();
        assert_eq!(ids(&made.buses), ["n1-host", "n1-guest", "n1-phone"]);
        assert_eq!(m.validate(4, 96), Ok(()));
        let all = m.all_sends();
        let into = |b: &str| {
            let mut v: Vec<&str> = all
                .iter()
                .filter(|s| s.to == b)
                .map(|s| s.from.as_str())
                .collect();
            v.sort();
            v
        };
        assert_eq!(into("n1-host"), ["guest", "phone"]);
        assert_eq!(into("n1-phone"), ["guest", "host"]);
        // Without a bus: sends of their own from the others.
        let made = n_minus_one(&mut m, &sel[..2], None, N1Mode::PerParticipant, None).unwrap();
        assert_eq!(ids(&made.buses), ["n1-1", "n1-2"]);
        assert!(m.send("guest", "n1-1").is_some() && m.send("host", "n1-1").is_none());
        let made = n_minus_one(&mut m, &sel[..1], Some("main".into()), N1Mode::One, None).unwrap();
        assert_eq!(made.buses, ["n1-3"]);
        assert_eq!(m.validate(4, 96), Ok(()));
        let all = m.all_sends();
        assert!(all.iter().any(|s| s.from == "guest" && s.to == "n1-3"));
        assert!(!all.iter().any(|s| s.from == "host" && s.to == "n1-3"));
        assert!(n_minus_one(&mut m, &sel, None, N1Mode::One, None).is_err());
    }

    #[test]
    fn mix_minus_follows_its_bus() {
        let mut m = mix();
        mix_minus(
            &mut m,
            "main",
            &["phone".into()],
            Some("remote".into()),
            None,
        )
        .unwrap();
        assert_eq!(m.bus("remote").unwrap().name, "main minus phone");
        let g = |m: &Mix| {
            m.all_sends()
                .into_iter()
                .find(|s| s.from == "host" && s.to == "remote")
                .map(|s| s.gain_db)
        };
        assert_eq!(g(&m), Some(Db(0.0)));
        m.sends
            .iter_mut()
            .find(|s| s.from == "host" && s.to == "main")
            .unwrap()
            .gain_db = Db(-6.0);
        assert_eq!(g(&m), Some(Db(-6.0)), "it follows the bus's send");
        // A send of its own wins.
        m.sends.push(Send {
            gain_db: Db(-20.0),
            ..send("host", "remote", Tap::PostPan)
        });
        assert_eq!(g(&m), Some(Db(-20.0)));
        assert_eq!(
            m.all_sends()
                .iter()
                .filter(|s| s.to == "remote" && s.from == "host")
                .count(),
            1
        );
        // Feeding main from remote is no loop (remote reads main's sources, not
        // main); through another bus it is: remote → fx → main, mirrored as fx → remote.
        let mut ok = m.clone();
        ok.sends.push(send("remote", "main", Tap::PostFader));
        assert_eq!(ok.validate(4, 96), Ok(()));
        let mut bad = m.clone();
        bad.buses.push(Bus::new("fx", Format::Stereo));
        bad.sends.push(send("remote", "fx", Tap::PostFader));
        bad.sends.push(send("fx", "main", Tap::PostFader));
        assert!(bad.validate(4, 96).unwrap_err().contains("loop"));
        // Deleting the bus it is taken from leaves a plain bus.
        m.remove_signal("main");
        assert_eq!(m.bus("remote").unwrap().minus, None);
    }

    #[test]
    fn link_and_unlink() {
        let mut m = mix();
        m.vcas.push(crate::mix::vca::Vca {
            id: "talent".into(),
            name: "Talent".into(),
            fader_db: Db(0.0),
            mute: false,
            members: vec!["host".into(), "guest".into()],
            colour: None,
            recall_safe: false,
        });
        let made = link(
            &mut m,
            &["host".into(), "guest".into()],
            None,
            Some("pair".into()),
            Some("Pair".into()),
        )
        .unwrap();
        assert_eq!(
            (made.strips.as_slice(), made.removed.len()),
            (&["pair".to_string()][..], 2)
        );
        assert_eq!(m.validate(4, 96), Ok(()));
        let p = m.strip("pair").unwrap();
        assert_eq!(p.format, Format::Stereo);
        assert_eq!(p.patch, vec![patched(1, 1), patched(1, 2)]);
        assert_eq!(ids(&m.vcas[0].members), ["pair"]);
        assert!(m.send("pair", "main").is_some() && m.send("pair", "aux").is_some());
        assert!(m.strip("host").is_none() && m.send("host", "main").is_none());
        assert!(link(&mut m, &["phone".into()], None, None, None).is_err());

        let made = unlink(&mut m, "pair").unwrap();
        assert_eq!(ids(&made.strips), ["pair-l", "pair-r"]);
        assert_eq!(m.validate(4, 96), Ok(()));
        let (l, r) = (m.strip("pair-l").unwrap(), m.strip("pair-r").unwrap());
        assert_eq!(
            (l.pan, r.pan, l.patch.clone()),
            (-1.0, 1.0, vec![patched(1, 1)])
        );
        assert_eq!(ids(&m.vcas[0].members), ["pair-l", "pair-r"]);
        assert!(m.send("pair-r", "aux").is_some());
    }
}
