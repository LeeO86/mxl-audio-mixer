//! Changing a strip's or bus's channel format (C10, P3). The signal keeps its
//! chain, sends, outputs and settings: the nodes that run on any width adapt to
//! the new channel count (the compiler builds them for it), the ones that
//! cannot are kept but bypassed and flagged, sends map through the width
//! mapping (down-mix or up-mix), and a strip's input patch is extended or
//! trimmed. The plan swap fades the signal out and in again (§2).

use super::format::Format;
use super::model::{Kind, Mix, Patch, PatchPoint};

/// Change `id`'s format to `format`; returns what the operator should know
/// (bypassed nodes, a pinned output). Nothing changes when the format is the
/// same.
pub fn change_format(mix: &mut Mix, id: &str, format: Format) -> Result<Vec<String>, String> {
    let kind = mix
        .kind(id)
        .ok_or_else(|| format!("signal {id} does not exist"))?;
    let (_, old) = mix.signal(id).expect("exists");
    if old == format {
        return Ok(Vec::new());
    }
    let mut notes = Vec::new();
    let strip = kind == Kind::Strip;
    match kind {
        Kind::Strip => {
            let s = mix.strips.iter_mut().find(|s| s.id == id).expect("exists");
            s.patch = adapt_patch(&s.patch, format.channels());
            s.polarity.resize(format.channels(), false);
            s.format = format;
        }
        Kind::Bus => {
            let b = mix.buses.iter_mut().find(|b| b.id == id).expect("exists");
            b.format = format;
        }
    }
    let chain = match kind {
        Kind::Strip => {
            &mut mix
                .strips
                .iter_mut()
                .find(|s| s.id == id)
                .expect("exists")
                .chain
        }
        Kind::Bus => {
            &mut mix
                .buses
                .iter_mut()
                .find(|b| b.id == id)
                .expect("exists")
                .chain
        }
    };
    for p in chain.iter_mut() {
        if let Err(why) = p.kind.supports(format, strip) {
            if !p.bypass {
                p.bypass = true;
                notes.push(format!("node {} is bypassed: {why}", p.id));
            }
        }
    }
    // A published signal keeps its flow's layout: downstream receivers go on
    // reading the same channels (the output maps the new format into it).
    if let Some(o) = mix.outputs.iter_mut().find(|o| o.signal == id) {
        if o.format.is_none() {
            o.format = Some(old);
            notes.push(format!(
                "the published flow keeps its {} layout; change it in the output when wanted",
                super::compile::format_name(old)
            ));
        }
    }
    Ok(notes)
}

/// A strip's patch for `n` channels: trimmed, or extended. A patch that runs
/// over consecutive channels of one input from the strip's first channel goes
/// on with the next channels of that input (as linked input selection does);
/// otherwise the new channels are unpatched.
pub fn adapt_patch(patch: &[Option<Patch>], n: usize) -> Vec<Option<Patch>> {
    let mut out: Vec<Option<Patch>> = patch.iter().take(n).cloned().collect();
    if out.len() >= n {
        return out;
    }
    let run = match patch.first() {
        Some(Some(Patch::Input(first))) => patch.iter().enumerate().all(|(k, p)| {
            matches!(p, Some(Patch::Input(q))
                if q.input == first.input && q.channel == first.channel + k)
        }),
        _ => false,
    };
    while out.len() < n {
        let k = out.len();
        let next = match (run, patch.first()) {
            (true, Some(Some(Patch::Input(first)))) => {
                let channel = first.channel + k;
                (channel <= crate::engine::MAX_CHANNELS).then_some(Patch::Input(PatchPoint {
                    input: first.input,
                    channel,
                }))
            }
            _ => None,
        };
        out.push(next);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mix::chain::{Proc, ProcKind, StereoMode};
    use crate::mix::model::tests::{bus, send, strip};
    use crate::mix::model::{Output, Tap};

    fn at(input: usize, channel: usize) -> Option<Patch> {
        Some(Patch::Input(PatchPoint { input, channel }))
    }

    fn node(id: &str, kind: ProcKind) -> Proc {
        Proc {
            id: id.into(),
            bypass: false,
            kind,
        }
    }

    #[test]
    fn patch_extends_a_run_and_trims() {
        assert_eq!(
            adapt_patch(&[at(2, 3), at(2, 4)], 6),
            vec![at(2, 3), at(2, 4), at(2, 5), at(2, 6), at(2, 7), at(2, 8)]
        );
        assert_eq!(adapt_patch(&[at(1, 1)], 2), vec![at(1, 1), at(1, 2)]);
        // Not a run: new channels unpatched.
        assert_eq!(
            adapt_patch(&[at(1, 2), at(1, 1)], 3),
            vec![at(1, 2), at(1, 1), None]
        );
        assert_eq!(
            adapt_patch(&[None, at(1, 2)], 3),
            vec![None, at(1, 2), None]
        );
        // Trimmed.
        assert_eq!(
            adapt_patch(&[at(1, 1), at(1, 2), at(1, 3)], 2),
            vec![at(1, 1), at(1, 2)]
        );
        // Not past the last channel an input can have.
        let last = crate::engine::MAX_CHANNELS;
        assert_eq!(adapt_patch(&[at(1, last)], 2), vec![at(1, last), None]);
    }

    #[test]
    fn chain_sends_and_output_are_kept() {
        let mut mix = Mix::default();
        let mut s = strip("music", Format::Stereo);
        s.patch = vec![at(1, 1), at(1, 2)];
        s.polarity = vec![false, true];
        s.chain = vec![
            node(
                "eq-1",
                serde_json::from_value(serde_json::json!({"type": "eq", "bands": [{"type": "bell", "freq_hz": 1000, "q": 1, "gain_db": 3}]})).unwrap(),
            ),
            node(
                "width-1",
                ProcKind::Width {
                    width_pct: 150.0,
                    input: StereoMode::Lr,
                    output: StereoMode::Lr,
                },
            ),
        ];
        mix.strips.push(s);
        mix.buses.push(bus("main", Format::Stereo));
        let mut sd = send("music", "main");
        sd.tap = Tap::After("width-1".into());
        mix.sends.push(sd);
        mix.outputs.push(Output {
            signal: "main".into(),
            format: None,
            commit_samples: None,
        });
        assert_eq!(mix.validate(2, 96), Ok(()));

        let notes = change_format(&mut mix, "music", Format::Surround51).unwrap();
        let s = mix.strip("music").unwrap();
        assert_eq!(s.format, Format::Surround51);
        assert_eq!(s.patch.len(), 6);
        assert_eq!(s.patch[5], at(1, 6));
        assert_eq!(s.polarity, vec![false, true, false, false, false, false]);
        assert_eq!(s.chain.len(), 2, "the chain stays");
        assert!(!s.chain[0].bypass, "an EQ adapts");
        assert!(s.chain[1].bypass, "M/S / width cannot run on 5.1");
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].contains("width-1"));
        assert_eq!(mix.sends.len(), 1, "the send stays");
        assert_eq!(
            mix.chain_warnings("music"),
            vec![("width-1".into(), "M/S / width needs a stereo signal".into())]
        );
        assert_eq!(mix.validate(2, 96), Ok(()));

        // The bus: its published flow keeps its stereo layout.
        let notes = change_format(&mut mix, "main", Format::Surround51).unwrap();
        assert_eq!(mix.output("main").unwrap().format, Some(Format::Stereo));
        assert!(notes[0].contains("stereo"), "{notes:?}");
        // Back to stereo: the node stays bypassed (an operator switches it on),
        // and no longer warns.
        let notes = change_format(&mut mix, "music", Format::Stereo).unwrap();
        assert!(notes.is_empty());
        assert!(mix.strip("music").unwrap().chain[1].bypass);
        assert!(mix.chain_warnings("music").is_empty());
        assert_eq!(mix.strip("music").unwrap().patch, vec![at(1, 1), at(1, 2)]);
        // The same format: nothing.
        assert!(change_format(&mut mix, "music", Format::Stereo)
            .unwrap()
            .is_empty());
        assert!(change_format(&mut mix, "nope", Format::Stereo).is_err());
    }
}
