//! The plan compiler (§2): mix → execution plan.
//!
//! Strips first (patch → trim → fader → panner), then the buses in dependency
//! order (sum of sends → fader), then the published outputs (width mapping →
//! output slot). Every node has a key (`strip/<id>/fader`, `send/<from>/<to>`, …)
//! that maps to a stable [`NodeId`], and a signature of its wiring: the control
//! side carries a node's values over a swap only when the signature is the same.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use super::format::{pan_gains, stereo_matrix, width_matrix, Format};
use super::model::{Db, Mix, Tap};
use crate::engine::plan::{NodeId, Plan, PlanBuilder, Span, Step};

/// A node of a compiled plan, as the control side remembers it.
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutNode {
    pub index: u32,
    pub signature: u64,
    pub values: Vec<f32>,
}

/// A compiled plan and what the control side needs to know about it.
pub struct Compiled {
    pub plan: Plan,
    pub nodes: HashMap<NodeId, LayoutNode>,
    /// `(signal id, first meter channel, channels)`.
    pub meters: Vec<(String, u32, usize)>,
    /// Equal for two compiles exactly when only node values differ.
    pub structure: u64,
}

#[derive(Default)]
pub struct Compiler {
    ids: HashMap<String, NodeId>,
}

#[derive(Clone, Copy)]
struct Taps {
    input: Span,
    pre: Span,
    post: Span,
    pan: Span,
}

fn hash(value: impl Hash) -> u64 {
    let mut h = DefaultHasher::new();
    value.hash(&mut h);
    h.finish()
}

fn format_name(f: Format) -> &'static str {
    match f {
        Format::Mono => "mono",
        Format::Stereo => "stereo",
        Format::Lcr => "3.0",
        Format::Surround51 => "5.1",
        Format::Surround71 => "7.1",
        Format::Surround514 => "5.1.4",
        Format::Surround714 => "7.1.4",
    }
}

struct Build<'a> {
    b: PlanBuilder,
    compiler: &'a mut Compiler,
    ramp: u32,
    nodes: HashMap<NodeId, LayoutNode>,
    structure: DefaultHasher,
}

impl Build<'_> {
    fn node(&mut self, key: String, signature: impl Hash, values: Vec<f32>) -> u32 {
        let next = self.compiler.ids.len() as NodeId;
        let id = *self.compiler.ids.entry(key.clone()).or_insert(next);
        let signature = hash((&key, signature));
        let index = self.b.node(id, &values, self.ramp);
        signature.hash(&mut self.structure);
        self.nodes.insert(
            id,
            LayoutNode {
                index,
                signature,
                values,
            },
        );
        index
    }

    fn single(&self, span: Span, i: usize) -> Span {
        Span {
            start: span.start + i as u32,
            len: 1,
        }
    }
}

impl Compiler {
    /// Compile `mix`. `outputs` maps a published signal to its output slot;
    /// outputs without a slot are left out. New nodes ramp in over `ramp` samples.
    pub fn compile(
        &mut self,
        mix: &Mix,
        block: usize,
        ramp: u32,
        outputs: &HashMap<String, usize>,
    ) -> Compiled {
        let laws = mix.settings.laws();
        let mut c = Build {
            b: PlanBuilder::new(block),
            compiler: self,
            ramp,
            nodes: HashMap::new(),
            structure: DefaultHasher::new(),
        };
        let mut taps: HashMap<&str, Taps> = HashMap::new();
        let mut meters = Vec::new();

        for s in &mix.strips {
            let n = s.format.channels();
            let input = c.b.buffers(n);
            for (ch, point) in s.patch.iter().enumerate() {
                match point {
                    Some(p) => {
                        let dst = c.b.buffer(input, ch);
                        c.b.step(Step::Patch {
                            input: (p.input - 1) as u32,
                            channel: (p.channel - 1) as u32,
                            dst,
                        });
                    }
                    None => {
                        let dst = c.single(input, ch);
                        c.b.step(Step::Clear { dst });
                    }
                }
            }
            let trim = Db(s.trim_db).gain() as f32;
            let values = s
                .polarity
                .iter()
                .map(|p| if *p { -trim } else { trim })
                .collect();
            let node = c.node(
                format!("strip/{}/trim", s.id),
                (format_name(s.format), &s.patch),
                values,
            );
            let pre = c.b.buffers(n);
            c.b.step(Step::Gain {
                node,
                src: input,
                dst: pre,
            });
            let fader = if s.mute {
                0.0
            } else {
                s.fader_db.gain() as f32
            };
            let node = c.node(
                format!("strip/{}/fader", s.id),
                format_name(s.format),
                vec![fader; n],
            );
            let post = c.b.buffers(n);
            c.b.step(Step::Gain {
                node,
                src: pre,
                dst: post,
            });
            let pan_values = match s.format {
                Format::Mono => {
                    let (l, r) = pan_gains(s.pan, laws.pan);
                    Some(vec![l as f32, r as f32])
                }
                Format::Stereo => Some(
                    stereo_matrix(s.width, s.pan)
                        .iter()
                        .map(|v| *v as f32)
                        .collect(),
                ),
                _ => None,
            };
            let pan = match pan_values {
                Some(values) => {
                    let node = c.node(format!("strip/{}/pan", s.id), format_name(s.format), values);
                    let pan = c.b.buffers(2);
                    c.b.step(Step::Matrix {
                        node,
                        src: post,
                        dst: pan,
                    });
                    pan
                }
                None => post,
            };
            let slot = c.b.meter(n);
            c.b.step(Step::Meter { src: post, slot });
            meters.push((s.id.clone(), slot, n));
            taps.insert(
                &s.id,
                Taps {
                    input,
                    pre,
                    post,
                    pan,
                },
            );
        }

        let order = mix.bus_order().unwrap_or_default();
        for bus in order {
            let n = bus.format.channels();
            let sum = c.b.buffers(n);
            c.b.step(Step::Clear { dst: sum });
            for send in mix.sends.iter().filter(|s| s.to == bus.id) {
                let (Some(t), Some(from)) = (
                    taps.get(send.from.as_str()).copied(),
                    mix.tap_format(&send.from, send.tap),
                ) else {
                    continue;
                };
                let src = match send.tap {
                    Tap::Input => t.input,
                    Tap::PreFader => t.pre,
                    Tap::PostFader => t.post,
                    Tap::PostPan => t.pan,
                };
                let w = width_matrix(from, bus.format, laws);
                let coefficients = c.b.coefficients(&w);
                let mut gain = if send.mute {
                    0.0
                } else {
                    send.gain_db.gain() as f32
                };
                if send.phase {
                    gain = -gain;
                }
                let node = c.node(
                    format!("send/{}/{}", send.from, send.to),
                    (
                        format_name(from),
                        format_name(bus.format),
                        f64::from(laws.pan).to_bits(),
                        f64::from(laws.mono_sum).to_bits(),
                        format!("{:?}", send.tap),
                    ),
                    vec![gain],
                );
                c.b.step(Step::Send {
                    node,
                    src,
                    dst: sum,
                    coefficients,
                });
            }
            let fader = if bus.mute {
                0.0
            } else {
                bus.fader_db.gain() as f32
            };
            let node = c.node(
                format!("bus/{}/fader", bus.id),
                format_name(bus.format),
                vec![fader; n],
            );
            let post = c.b.buffers(n);
            c.b.step(Step::Gain {
                node,
                src: sum,
                dst: post,
            });
            let slot = c.b.meter(n);
            c.b.step(Step::Meter { src: post, slot });
            meters.push((bus.id.clone(), slot, n));
            taps.insert(
                &bus.id,
                Taps {
                    input: sum,
                    pre: sum,
                    post,
                    pan: post,
                },
            );
        }

        for o in &mix.outputs {
            let (Some(&slot), Some(t)) = (outputs.get(&o.signal), taps.get(o.signal.as_str()))
            else {
                continue;
            };
            let Some(from) = mix.tap_format(&o.signal, Tap::PostFader) else {
                continue;
            };
            let to = o.format.unwrap_or(from);
            let src = t.post;
            let out = c.b.buffers(to.channels());
            c.b.step(Step::Clear { dst: out });
            let w = width_matrix(from, to, laws);
            let coefficients = c.b.coefficients(&w);
            let node = c.node(
                format!("output/{}", o.signal),
                (
                    format_name(from),
                    format_name(to),
                    slot,
                    f64::from(laws.pan).to_bits(),
                    f64::from(laws.mono_sum).to_bits(),
                ),
                vec![1.0],
            );
            c.b.step(Step::Send {
                node,
                src,
                dst: out,
                coefficients,
            });
            c.b.step(Step::Output {
                src: out,
                slot: slot as u32,
            });
        }

        let structure = c.structure.finish();
        Compiled {
            plan: c.b.finish(),
            nodes: c.nodes,
            meters,
            structure,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::plan::{InputSource, OutputSink};
    use crate::mix::model::{Bus, Output, PatchPoint, Send, Strip};

    struct Inputs(Vec<Vec<Vec<f32>>>);
    impl InputSource for Inputs {
        fn channel(&self, input: usize, channel: usize) -> Option<&[f32]> {
            self.0.get(input)?.get(channel).map(|c| c.as_slice())
        }
    }
    #[derive(Default)]
    struct Outputs(HashMap<usize, Vec<Vec<f32>>>);
    impl OutputSink for Outputs {
        fn write(&mut self, slot: usize, channel: usize, channels: usize, samples: &[f32]) {
            let out = self
                .0
                .entry(slot)
                .or_insert_with(|| vec![Vec::new(); channels]);
            out[channel].extend_from_slice(samples);
        }
    }

    fn strip(id: &str, format: Format, input: usize) -> Strip {
        let n = format.channels();
        Strip {
            id: id.into(),
            name: id.into(),
            format,
            patch: (0..n)
                .map(|c| {
                    Some(PatchPoint {
                        input,
                        channel: c + 1,
                    })
                })
                .collect(),
            trim_db: 0.0,
            polarity: vec![false; n],
            fader_db: Db(0.0),
            mute: false,
            pan: 0.0,
            width: 1.0,
        }
    }

    fn send(from: &str, to: &str, tap: Tap) -> Send {
        Send {
            from: from.into(),
            to: to.into(),
            tap,
            gain_db: Db(0.0),
            mute: false,
            phase: false,
        }
    }

    fn mix() -> Mix {
        let mut mix = Mix::default();
        mix.strips.push(strip("vox", Format::Mono, 1));
        mix.strips.push(strip("music", Format::Stereo, 2));
        mix.buses.push(Bus {
            id: "main".into(),
            name: "Main".into(),
            format: Format::Stereo,
            fader_db: Db(0.0),
            mute: false,
        });
        mix.sends.push(send("vox", "main", Tap::PostPan));
        mix.sends.push(send("music", "main", Tap::PostFader));
        mix.outputs.push(Output {
            signal: "main".into(),
            format: None,
            commit_samples: None,
        });
        mix
    }

    fn run(compiled: &mut Compiled, inputs: &Inputs, blocks: usize) -> Outputs {
        let mut out = Outputs::default();
        for _ in 0..blocks {
            compiled.plan.process(inputs, &mut out);
        }
        out
    }

    fn slots() -> HashMap<String, usize> {
        HashMap::from([("main".to_string(), 3)])
    }

    #[test]
    fn strips_into_a_bus_into_an_output() {
        let mut compiler = Compiler::default();
        let mut c = compiler.compile(&mix(), 48, 0, &slots());
        let inputs = Inputs(vec![
            vec![vec![0.5; 48]],
            vec![vec![0.1; 48], vec![0.2; 48]],
        ]);
        let out = run(&mut c, &inputs, 1);
        let main = &out.0[&3];
        // vox centred at −3 dB: 0.5·0.7071; music: L 0.1, R 0.2.
        let h = std::f32::consts::FRAC_1_SQRT_2;
        assert!(
            (main[0][0] - (0.5 * h + 0.1)).abs() < 1e-6,
            "{}",
            main[0][0]
        );
        assert!((main[1][0] - (0.5 * h + 0.2)).abs() < 1e-6);
        assert_eq!(c.meters.len(), 3);
        let kinds: Vec<&str> = c.plan.steps.iter().map(|s| s.kind()).collect();
        assert_eq!(kinds.iter().filter(|k| **k == "send").count(), 3);
    }

    #[test]
    fn parameter_changes_keep_the_structure_and_rewiring_does_not() {
        let mut compiler = Compiler::default();
        let base = mix();
        let a = compiler.compile(&base, 48, 0, &slots());
        let mut faded = base.clone();
        faded.strips[0].fader_db = Db(-6.0);
        faded.strips[1].pan = 0.5;
        faded.sends[0].mute = true;
        let b = compiler.compile(&faded, 48, 0, &slots());
        assert_eq!(a.structure, b.structure);
        let changed: Vec<_> = b
            .nodes
            .iter()
            .filter(|(id, n)| a.nodes[id].values != n.values)
            .collect();
        assert_eq!(changed.len(), 3);
        assert!(b
            .nodes
            .iter()
            .all(|(id, n)| a.nodes[id].signature == n.signature));

        let mut rewired = base.clone();
        rewired.sends[0].tap = Tap::PreFader;
        let c = compiler.compile(&rewired, 48, 0, &slots());
        assert_ne!(a.structure, c.structure);
        let differing = c
            .nodes
            .iter()
            .filter(|(id, n)| a.nodes[id].signature != n.signature)
            .count();
        assert_eq!(differing, 1, "only the send changed its wiring");

        let mut repatched = base;
        repatched.strips[0].patch[0] = Some(PatchPoint {
            input: 2,
            channel: 2,
        });
        let d = compiler.compile(&repatched, 48, 0, &slots());
        assert_ne!(a.structure, d.structure);
    }

    #[test]
    fn polarity_trim_mute_and_nested_buses() {
        let mut m = mix();
        m.strips[0].polarity = vec![true];
        m.strips[0].trim_db = 6.0206;
        m.strips[1].mute = true;
        m.buses.push(Bus {
            id: "sub".into(),
            name: "Sub".into(),
            format: Format::Mono,
            fader_db: Db(-6.0206),
            mute: false,
        });
        m.sends.push(send("main", "sub", Tap::PostFader));
        m.outputs.push(Output {
            signal: "sub".into(),
            format: Some(Format::Stereo),
            commit_samples: None,
        });
        assert_eq!(m.validate(2, 48), Ok(()));
        let mut compiler = Compiler::default();
        let slots = HashMap::from([("main".to_string(), 0), ("sub".to_string(), 1)]);
        let mut c = compiler.compile(&m, 48, 0, &slots);
        let inputs = Inputs(vec![
            vec![vec![0.25; 48]],
            vec![vec![0.5; 48], vec![0.5; 48]],
        ]);
        let out = run(&mut c, &inputs, 1);
        let h = std::f32::consts::FRAC_1_SQRT_2;
        // vox: −0.25 · 2 = −0.5, centred → −0.5·h per side; music muted.
        let l = -0.5 * h;
        assert!((out.0[&0][0][0] - l).abs() < 1e-5, "{}", out.0[&0][0][0]);
        // sub (mono) = (L + R) · −3 dB sum · −6 dB fader; published as stereo (centred).
        let sub = 2.0 * l * 0.707_945_8 * 0.5;
        assert!((out.0[&1][0][0] - sub * h).abs() < 1e-5);
        assert_eq!(out.0[&1].len(), 2);
    }
}
