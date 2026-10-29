//! The runtime, block by block (D158–D174).

use super::*;
use crate::mix::logic::tests::{block, graph, wire};
use crate::mix::logic::{Block, BlockKind as K, CompareOp, Macro, MathOp, Measure, PortType, Wire};
use crate::mix::model::tests::strip;
use crate::mix::Format;

fn mix_of(blocks: Vec<Block>, wires: Vec<Wire>) -> Mix {
    let mut mix = Mix::default();
    mix.strips.push(strip("vox", Format::Mono));
    mix.strips.push(strip("music", Format::Mono));
    mix.logic.push(graph(blocks, wires));
    assert_eq!(
        crate::mix::logic::issues(&mix),
        vec![],
        "the test graph is valid"
    );
    mix
}

fn out(from: &str, to: &str, port: &str) -> Wire {
    wire(from, "out", to, port)
}

/// One tick with nothing measured; the acts.
fn tick(logic: &mut Logic, mix: &Mix) -> Vec<Act> {
    logic
        .tick(&Readings::quiet(mix))
        .into_iter()
        .map(|a| a.act)
        .collect()
}

fn value(logic: &Logic, block: &str) -> Json {
    logic.values("g").unwrap()[block].clone()
}

#[test]
fn speech_mutes_the_music_one_tick_after_it_starts() {
    let mix = mix_of(
        vec![
            block(
                "v",
                K::Vad {
                    signal: "vox".into(),
                },
            ),
            block(
                "m",
                K::SetMute {
                    signal: "music".into(),
                },
            ),
        ],
        vec![out("v", "m", "in")],
    );
    let mut logic = Logic::default();
    let mut speech = BTreeMap::from([("vox".to_string(), false)]);
    let at = |logic: &mut Logic, speech: &BTreeMap<String, bool>| {
        logic.tick(&Readings {
            speech,
            ..Readings::quiet(&mix)
        })
    };
    // The first tick applies what the graph says now.
    let first = at(&mut logic, &speech);
    assert_eq!(
        first,
        vec![Action {
            graph: "g".into(),
            block: "m".into(),
            dry_run: false,
            retry: false,
            act: Act::Mute {
                signal: "music".into(),
                on: false
            }
        }]
    );
    assert!(at(&mut logic, &speech).is_empty(), "not again");
    speech.insert("vox".into(), true);
    assert_eq!(
        at(&mut logic, &speech)[0].act,
        Act::Mute {
            signal: "music".into(),
            on: true
        }
    );
    assert!(at(&mut logic, &speech).is_empty());
    // logic_ticks_total and logic_actions_total{graph}.
    assert_eq!(logic.ticks, 4);
    assert_eq!(logic.actions, BTreeMap::from([("g".to_string(), 2)]));
    assert_eq!(logic.recent("g").len(), 2);
    assert_eq!(logic.recent("g")[1].action, "mute music on");
}

#[test]
fn an_action_without_a_wire_never_runs() {
    let mix = mix_of(
        vec![
            block(
                "m",
                K::SetMute {
                    signal: "music".into(),
                },
            ),
            block(
                "f",
                K::SetFader {
                    signal: "music".into(),
                    ramp_ms: 10.0,
                },
            ),
            block(
                "r",
                K::Recall {
                    snapshot: "s".into(),
                },
            ),
            block("t", K::TslOut { index: 1 }),
        ],
        vec![],
    );
    let mut logic = Logic::default();
    for _ in 0..3 {
        assert!(tick(&mut logic, &mix).is_empty());
    }
}

#[test]
fn dry_run_lists_the_actions_and_applies_them_when_it_ends() {
    let mut mix = mix_of(
        vec![
            block("c", K::Const { value: 1.0 }),
            block(
                "m",
                K::SetMute {
                    signal: "music".into(),
                },
            ),
        ],
        vec![out("c", "m", "in")],
    );
    mix.logic[0].dry_run = true;
    let mut logic = Logic::default();
    let a = logic.tick(&Readings::quiet(&mix));
    assert_eq!(a.len(), 1);
    assert!(a[0].dry_run);
    assert!(logic.actions.is_empty(), "dry run is not counted as run");
    assert!(logic.recent("g")[0].dry_run);
    assert!(tick(&mut logic, &mix).is_empty());
    mix.logic[0].dry_run = false;
    let a = logic.tick(&Readings::quiet(&mix));
    assert_eq!(a.len(), 1);
    assert!(!a[0].dry_run);
}

#[test]
fn an_edge_through_a_latch_toggles() {
    let mut mix = mix_of(
        vec![
            block("b", K::Button),
            block("e", K::Edge),
            block("l", K::Latch),
        ],
        vec![out("b", "e", "in"), out("e", "l", "set")],
    );
    mix.logic[0].dry_run = true;
    let mut logic = Logic::default();
    tick(&mut logic, &mix);
    logic.set_button("g", "b", true);
    tick(&mut logic, &mix);
    // The edge publishes one tick later; the latch takes it the tick after.
    assert_eq!(value(&logic, "e"), json!(1), "fired once");
    tick(&mut logic, &mix);
    assert_eq!(value(&logic, "l"), json!(true));
    assert_eq!(value(&logic, "e"), json!(1), "the edge lasts one tick");
    logic.set_button("g", "b", false);
    tick(&mut logic, &mix);
    assert_eq!(value(&logic, "l"), json!(true), "the latch holds");
    assert_eq!(logic.pressed("g"), Vec::<String>::new());
}

#[test]
fn toggle_flips_on_each_event_and_counter_counts() {
    let mix = mix_of(
        vec![
            block("b", K::Button),
            block("t", K::Toggle),
            block("u", K::Button),
            block("d", K::Button),
            block("z", K::Trigger),
            block("c", K::Counter),
        ],
        vec![
            out("b", "t", "in"),
            out("u", "c", "up"),
            out("d", "c", "down"),
            out("z", "c", "reset"),
        ],
    );
    let mut logic = Logic::default();
    let press = |logic: &mut Logic, b: &str| {
        logic.set_button("g", b, true);
        tick(logic, &mix);
        logic.set_button("g", b, false);
        tick(logic, &mix);
    };
    tick(&mut logic, &mix);
    assert_eq!(value(&logic, "t"), json!(false));
    press(&mut logic, "b");
    assert_eq!(value(&logic, "t"), json!(true));
    press(&mut logic, "b");
    assert_eq!(value(&logic, "t"), json!(false));
    press(&mut logic, "u");
    press(&mut logic, "u");
    press(&mut logic, "d");
    assert_eq!(value(&logic, "c"), json!(1.0));
    logic.trigger("g", "z");
    tick(&mut logic, &mix);
    assert_eq!(value(&logic, "c"), json!(0.0));
    assert_eq!(value(&logic, "z"), json!(1), "the trigger fired once");
    tick(&mut logic, &mix);
    assert_eq!(value(&logic, "z"), json!(1), "and not again");
}

#[test]
fn a_value_into_an_event_fires_on_the_rise_but_not_at_the_start() {
    let mix = mix_of(
        vec![
            block("b", K::Button),
            block(
                "r",
                K::Recall {
                    snapshot: "show".into(),
                },
            ),
        ],
        vec![out("b", "r", "in")],
    );
    let mut logic = Logic::default();
    let mut on = Logic::default();
    on.set_button("g", "b", true);
    assert!(tick(&mut on, &mix).is_empty(), "already on at the start");
    assert!(tick(&mut logic, &mix).is_empty());
    logic.set_button("g", "b", true);
    assert_eq!(
        tick(&mut logic, &mix),
        vec![Act::Recall {
            snapshot: "show".into()
        }]
    );
    assert!(
        tick(&mut logic, &mix).is_empty(),
        "holding does not recall again"
    );
    logic.set_button("g", "b", false);
    assert!(tick(&mut logic, &mix).is_empty());
    logic.set_button("g", "b", true);
    assert_eq!(tick(&mut logic, &mix).len(), 1);
}

#[test]
fn compare_math_scale_map_and_select() {
    let mut mix = mix_of(
        vec![
            block(
                "f",
                K::Fader {
                    signal: "vox".into(),
                    threshold_db: 0.0,
                },
            ),
            block("c", K::Const { value: -10.0 }),
            block("lt", K::Compare { op: CompareOp::Lt }),
            block("ge", K::Compare { op: CompareOp::Ge }),
            block("ne", K::Compare { op: CompareOp::Ne }),
            block("add", K::Math { op: MathOp::Add }),
            block("div", K::Math { op: MathOp::Div }),
            block("min", K::Math { op: MathOp::Min }),
            block(
                "sc",
                K::Scale {
                    factor: 2.0,
                    offset: 1.0,
                },
            ),
            block(
                "map",
                K::Map {
                    points: vec![[-60.0, 0.0], [-20.0, 1.0], [0.0, 3.0]],
                },
            ),
            block(
                "sel",
                K::Select {
                    inputs: 3,
                    value_type: PortType::String,
                },
            ),
            block(
                "t0",
                K::Text {
                    value: "zero".into(),
                },
            ),
            block(
                "t1",
                K::Text {
                    value: "one".into(),
                },
            ),
        ],
        vec![
            out("f", "lt", "a"),
            out("c", "lt", "b"),
            out("f", "ge", "a"),
            out("c", "ge", "b"),
            out("c", "ne", "a"),
            out("c", "ne", "b"),
            out("f", "add", "a"),
            out("c", "add", "b"),
            out("f", "div", "a"),
            out("f", "min", "a"),
            out("c", "min", "b"),
            out("c", "sc", "in"),
            out("f", "map", "in"),
            out("lt", "sel", "sel"),
            out("t0", "sel", "in0"),
            out("t1", "sel", "in1"),
            out("f", "sel", "in2"),
        ],
    );
    mix.strips[0].fader_db = Db(-30.0);
    let mut logic = Logic::default();
    tick(&mut logic, &mix);
    assert_eq!(value(&logic, "lt"), json!(true));
    assert_eq!(value(&logic, "ge"), json!(false));
    assert_eq!(value(&logic, "ne"), json!(false));
    assert_eq!(value(&logic, "add"), json!(-40.0));
    assert_eq!(value(&logic, "div"), json!(0.0), "÷0 is 0");
    assert_eq!(value(&logic, "min"), json!(-30.0));
    assert_eq!(value(&logic, "sc"), json!(-19.0));
    assert_eq!(value(&logic, "map"), json!(0.75));
    assert_eq!(value(&logic, "sel"), json!("one"), "true selects in1");
    mix.strips[0].fader_db = Db(5.0);
    tick(&mut logic, &mix);
    assert_eq!(
        value(&logic, "map"),
        json!(3.0),
        "flat after the last point"
    );
    assert_eq!(value(&logic, "sel"), json!("zero"));
    assert_eq!(map(&[[0.0, 0.0], [1.0, 1.0]], -5.0), 0.0);
}

#[test]
fn select_clamps_and_numbers_become_text() {
    let mix = mix_of(
        vec![
            block("k", K::Const { value: 7.0 }),
            block("a", K::Const { value: -12.345 }),
            block("b", K::Const { value: 2.0 }),
            block(
                "sel",
                K::Select {
                    inputs: 2,
                    value_type: PortType::String,
                },
            ),
        ],
        vec![
            out("k", "sel", "sel"),
            out("a", "sel", "in0"),
            out("b", "sel", "in1"),
        ],
    );
    let mut logic = Logic::default();
    tick(&mut logic, &mix);
    assert_eq!(value(&logic, "sel"), json!("2"));
    assert_eq!(format_number(-12.345), "-12.35");
    assert_eq!(format_number(f64::NEG_INFINITY), "-inf");
    assert_eq!(format_number(-0.001), "0");
}

#[test]
fn a_fader_is_on_from_its_threshold_where_it_feeds_on_off() {
    let mut mix = mix_of(
        vec![
            block(
                "f",
                K::Fader {
                    signal: "vox".into(),
                    threshold_db: -20.0,
                },
            ),
            block(
                "m",
                K::SetMute {
                    signal: "music".into(),
                },
            ),
            block("e", K::Edge),
            block(
                "r",
                K::Recall {
                    snapshot: "s".into(),
                },
            ),
            block(
                "sc",
                K::Scale {
                    factor: 1.0,
                    offset: 0.0,
                },
            ),
            block(
                "none",
                K::Fader {
                    signal: String::new(),
                    threshold_db: -120.0,
                },
            ),
            block(
                "m2",
                K::SetMute {
                    signal: "vox".into(),
                },
            ),
        ],
        vec![
            out("f", "m", "in"),
            out("f", "e", "in"),
            out("e", "r", "in"),
            out("f", "sc", "in"),
            out("none", "m2", "in"),
        ],
    );
    let mute = |on| Act::Mute {
        signal: "music".into(),
        on,
    };
    let mut logic = Logic::default();
    mix.strips[0].fader_db = Db(-30.0);
    // Below the threshold: off; a fader without a signal is off whatever its threshold.
    let a = tick(&mut logic, &mix);
    assert!(a.contains(&mute(false)));
    assert!(a.contains(&Act::Mute {
        signal: "vox".into(),
        on: false
    }));
    // The level still goes out as it is.
    assert_eq!(value(&logic, "sc"), json!(-30.0));
    // At the threshold: on, and an event input sees the rise.
    mix.strips[0].fader_db = Db(-20.0);
    // (Two ticks: a mute runs at most every 20 ms, D173.)
    let mut a = tick(&mut logic, &mix);
    a.extend(tick(&mut logic, &mix));
    assert!(a.contains(&mute(true)), "{a:?}");
    assert!(a.iter().any(|x| matches!(x, Act::Recall { .. })), "{a:?}");
    // A closed fader is off.
    mix.strips[0].fader_db = Db(f64::NEG_INFINITY);
    let mut a = tick(&mut logic, &mix);
    a.extend(tick(&mut logic, &mix));
    assert!(a.contains(&mute(false)), "{a:?}");
}

#[test]
fn a_fader_block_of_an_old_show_is_on_from_0_db() {
    let k: K = serde_json::from_value(json!({"type": "fader", "signal": "vox"})).unwrap();
    assert_eq!(
        k,
        K::Fader {
            signal: "vox".into(),
            threshold_db: 0.0
        }
    );
}

#[test]
fn compare_and_fader_with_its_ramp() {
    let mut mix = mix_of(
        vec![
            block(
                "f",
                K::Fader {
                    signal: "vox".into(),
                    threshold_db: 0.0,
                },
            ),
            block("c", K::Const { value: -10.0 }),
            block("cmp", K::Compare { op: CompareOp::Lt }),
            block(
                "set",
                K::SetFader {
                    signal: "vox".into(),
                    ramp_ms: 200.0,
                },
            ),
        ],
        vec![
            out("f", "cmp", "a"),
            out("c", "cmp", "b"),
            out("cmp", "set", "level"),
        ],
    );
    mix.strips[0].fader_db = Db(-20.0);
    // compare is on (−20 < −10), and set-fader sets the fader to that 1 dB:
    // the wiring is what the graph asked.
    let mut logic = Logic::default();
    let a = tick(&mut logic, &mix);
    assert_eq!(
        a,
        vec![Act::Fader {
            signal: "vox".into(),
            db: 1.0,
            ramp_ms: 200.0
        }]
    );
    assert_eq!(clamp_fader(f64::NEG_INFINITY).0, f64::NEG_INFINITY);
    assert_eq!(clamp_fader(30.0).0, crate::mix::model::MAX_FADER_DB);
}

#[test]
fn a_fader_follows_only_changes_above_005_db() {
    let mut mix = mix_of(vec![], vec![]);
    mix.logic[0] = graph(
        vec![
            block(
                "l",
                K::Level {
                    signal: "vox".into(),
                },
            ),
            block(
                "v",
                K::SetVca {
                    vca: "v1".into(),
                    ramp_ms: 10.0,
                },
            ),
        ],
        vec![out("l", "v", "level")],
    );
    mix.vcas.push(crate::mix::vca::Vca {
        id: "v1".into(),
        name: "V".into(),
        fader_db: Db(0.0),
        mute: false,
        members: vec![],
        colour: None,
        recall_safe: false,
    });
    assert_eq!(crate::mix::logic::issues(&mix), vec![]);
    let mut logic = Logic::default();
    let at = |logic: &mut Logic, db: f64| {
        let level = BTreeMap::from([("vox".to_string(), db)]);
        logic
            .tick(&Readings {
                level_db: &level,
                ..Readings::quiet(&mix)
            })
            .len()
    };
    assert_eq!(at(&mut logic, -20.0), 1);
    assert_eq!(at(&mut logic, -20.04), 0);
    assert_eq!(at(&mut logic, -20.1), 1);
}

#[test]
fn tally_on_air_mutes_when_the_lamp_is_lit() {
    let mix = mix_of(
        vec![
            block("t", K::Tally { index: 5 }),
            block(
                "m",
                K::SetMute {
                    signal: "music".into(),
                },
            ),
        ],
        vec![out("t", "m", "in")],
    );
    let mut logic = Logic::default();
    let on = BTreeSet::from([5]);
    assert_eq!(logic.values("g"), None);
    let first = tick(&mut logic, &mix);
    assert_eq!(value(&logic, "t"), json!(false));
    assert_eq!(
        first,
        vec![Act::Mute {
            signal: "music".into(),
            on: false
        }]
    );
    tick(&mut logic, &mix); // an action runs at most every 20 ms
    let lit = logic.tick(&Readings {
        on_air: &on,
        ..Readings::quiet(&mix)
    });
    assert_eq!(value(&logic, "t"), json!(true));
    assert_eq!(
        lit[0].act,
        Act::Mute {
            signal: "music".into(),
            on: true
        }
    );
}

#[test]
fn a_timer_holds_after_its_input_rises_and_a_new_rise_restarts_it() {
    let mix = mix_of(
        vec![block("b", K::Button), block("t", K::Timer { ms: 30.0 })],
        vec![out("b", "t", "in")],
    );
    let mut logic = Logic::default();
    let t = |logic: &mut Logic| {
        tick(logic, &mix);
        value(logic, "t")
    };
    assert_eq!(t(&mut logic), json!(false));
    logic.set_button("g", "b", true);
    assert_eq!(t(&mut logic), json!(true));
    assert_eq!(t(&mut logic), json!(true));
    logic.set_button("g", "b", false);
    assert_eq!(t(&mut logic), json!(true), "the pulse outlasts the input");
    assert_eq!(t(&mut logic), json!(false));
    logic.set_button("g", "b", true);
    assert_eq!(t(&mut logic), json!(true));
    logic.set_button("g", "b", false);
    assert_eq!(t(&mut logic), json!(true));
    logic.set_button("g", "b", true);
    assert_eq!(t(&mut logic), json!(true), "a rise starts the 30 ms again");
    assert_eq!(t(&mut logic), json!(true));
    assert_eq!(t(&mut logic), json!(true));
    assert_eq!(t(&mut logic), json!(false));
    assert_eq!(pulse_ticks(0.0), 0.0);
    assert_eq!(pulse_ticks(11.0), 2.0);
}

#[test]
fn delay_and_debounce() {
    let mix = mix_of(
        vec![
            block("b", K::Button),
            block("d", K::Delay { ms: 20.0 }),
            block("db", K::Debounce { ms: 30.0 }),
        ],
        vec![out("b", "d", "in"), out("b", "db", "in")],
    );
    let mut logic = Logic::default();
    let t = |logic: &mut Logic| {
        tick(logic, &mix);
        (value(logic, "d"), value(logic, "db"))
    };
    t(&mut logic);
    logic.set_button("g", "b", true);
    assert_eq!(t(&mut logic), (json!(false), json!(false)));
    assert_eq!(t(&mut logic), (json!(true), json!(false)));
    assert_eq!(t(&mut logic), (json!(true), json!(true)));
    logic.set_button("g", "b", false);
    t(&mut logic);
    logic.set_button("g", "b", true);
    assert_eq!(
        t(&mut logic).1,
        json!(true),
        "a short dip does not pass the debounce"
    );
}

#[test]
fn above_holds_until_the_level_falls_by_the_hysteresis() {
    let mix = mix_of(
        vec![block(
            "a",
            K::Above {
                signal: "vox".into(),
                threshold_db: -40.0,
                hysteresis_db: 6.0,
            },
        )],
        vec![],
    );
    let mut logic = Logic::default();
    let t = |logic: &mut Logic, db: f64| {
        let level = BTreeMap::from([("vox".into(), db)]);
        logic.tick(&Readings {
            level_db: &level,
            ..Readings::quiet(&mix)
        });
        value(logic, "a")
    };
    assert_eq!(t(&mut logic, -50.0), json!(false));
    assert_eq!(t(&mut logic, -40.0), json!(true));
    assert_eq!(
        t(&mut logic, -45.0),
        json!(true),
        "still inside the hysteresis"
    );
    assert_eq!(t(&mut logic, -46.0), json!(true), "the floor still holds");
    assert_eq!(t(&mut logic, -47.0), json!(false));
}

#[test]
fn inputs_read_pfl_loudness_nmos_and_the_link() {
    let mix = mix_of(
        vec![
            block(
                "p",
                K::Pfl {
                    signal: "vox".into(),
                },
            ),
            block(
                "li",
                K::Loudness {
                    signal: "vox".into(),
                    measure: Measure::Integrated,
                },
            ),
            block(
                "lm",
                K::Loudness {
                    signal: "music".into(),
                    measure: Measure::Momentary,
                },
            ),
            block("rx", K::NmosReceiver { input: 3 }),
            block(
                "tx",
                K::NmosSender {
                    signal: "vox".into(),
                },
            ),
            block("s1", K::Link { peer: "s1".into() }),
            block("s2", K::Link { peer: "s2".into() }),
            block(
                "s",
                K::Schedule {
                    start_minute: 22 * 60,
                    end_minute: 6 * 60,
                },
            ),
        ],
        vec![],
    );
    let pfl = BTreeSet::from(["vox".to_string()]);
    let loud = |i: f64| LoudnessValues {
        momentary_lufs: Db(-20.0),
        short_term_lufs: Db(-21.0),
        integrated_lufs: Db(i),
        range_lu: 5.0,
        max_true_peak_dbtp: Db(-3.0),
        running: true,
        in_tolerance: None,
    };
    let loudness = BTreeMap::from([("vox".to_string(), loud(-23.0))]);
    let nmos = BTreeSet::from(["in-3".to_string(), "out-vox".to_string()]);
    let link = BTreeSet::from(["s1".to_string()]);
    let mut logic = Logic::default();
    logic.tick(&Readings {
        pfl: &pfl,
        loudness: &loudness,
        nmos: &nmos,
        link: &link,
        minute_of_day: 23 * 60,
        ..Readings::quiet(&mix)
    });
    assert_eq!(value(&logic, "p"), json!(true));
    assert_eq!(value(&logic, "li"), json!(-23.0));
    assert_eq!(value(&logic, "lm"), json!(FLOOR_DB), "not measured");
    assert_eq!(value(&logic, "rx"), json!(true));
    assert_eq!(value(&logic, "tx"), json!(true));
    assert_eq!(value(&logic, "s1"), json!(true));
    assert_eq!(value(&logic, "s2"), json!(false));
    assert_eq!(
        value(&logic, "s"),
        json!(true),
        "23:00 is inside 22:00–06:00"
    );
    let unmeasured = BTreeMap::from([("vox".to_string(), loud(f64::NEG_INFINITY))]);
    logic.tick(&Readings {
        loudness: &unmeasured,
        ..Readings::quiet(&mix)
    });
    assert_eq!(value(&logic, "li"), json!(FLOOR_DB));
    assert_eq!(value(&logic, "p"), json!(false));
    assert!(in_schedule(10 * 60, 9 * 60, 17 * 60));
    assert!(!in_schedule(8 * 60, 9 * 60, 17 * 60));
}

#[test]
fn every_action_says_what_it_asks() {
    let mut mix = mix_of(vec![], vec![]);
    mix.control_room.monitors.push(
        serde_json::from_value(json!({"id": "main", "name": "Main", "format": "stereo"})).unwrap(),
    );
    mix.control_room.talkback = serde_json::from_value(json!({
        "mics": [{"id": "pd", "name": "PD", "input": 1, "channel": 1}],
        "destinations": [{"id": "studio", "name": "Studio", "bus": "music"}]
    }))
    .unwrap();
    mix.strips[0].chain = vec![serde_json::from_value(json!({
        "id": "comp", "type": "compressor", "threshold_db": -20.0, "ratio": 4.0,
        "knee_db": 6.0, "attack_ms": 10.0, "release_ms": 100.0, "makeup_db": 0.0,
        "detector": "peak"
    }))
    .unwrap()];
    mix.logic[0] = graph(
        vec![
            block("b", K::Button),
            block("k", K::Const { value: -6.0 }),
            block(
                "txt",
                K::Text {
                    value: "ON AIR".into(),
                },
            ),
            block("e", K::Edge),
            block(
                "pfl",
                K::SetPfl {
                    signal: "vox".into(),
                },
            ),
            block(
                "talk",
                K::SetTalk {
                    mic: "pd".into(),
                    destination: "studio".into(),
                },
            ),
            block(
                "pub",
                K::SetPublish {
                    signal: "music".into(),
                },
            ),
            block(
                "mon",
                K::SetMonitor {
                    monitor: "main".into(),
                    source: Some(Source::Signal("vox".into())),
                },
            ),
            block(
                "loud",
                K::LoudnessControl {
                    signal: "vox".into(),
                    action: LoudnessAction::Reset,
                },
            ),
            block(
                "par",
                K::SetParam {
                    signal: "vox".into(),
                    node: "comp".into(),
                    param: "threshold_db".into(),
                    ramp_ms: 50.0,
                },
            ),
            block(
                "hook",
                K::Webhook {
                    url: "https://user:pw@example.test/x?k=1".into(),
                },
            ),
            block("tsl", K::TslOut { index: 7 }),
            block(
                "log",
                K::Log {
                    message: "pressed".into(),
                },
            ),
        ],
        vec![
            out("b", "e", "in"),
            out("b", "pfl", "in"),
            out("b", "talk", "in"),
            out("b", "pub", "in"),
            out("e", "mon", "in"),
            out("e", "loud", "in"),
            out("k", "par", "value"),
            out("e", "hook", "in"),
            out("b", "tsl", "left"),
            out("txt", "tsl", "label"),
            out("e", "log", "in"),
            out("k", "log", "value"),
        ],
    );
    assert_eq!(crate::mix::logic::issues(&mix), vec![]);
    let mut logic = Logic::default();
    let first = tick(&mut logic, &mix);
    assert_eq!(
        first,
        vec![
            Act::Pfl {
                signal: "vox".into(),
                on: false
            },
            Act::Talk {
                mic: "pd".into(),
                destination: "studio".into(),
                on: false
            },
            Act::Publish {
                signal: "music".into(),
                on: false
            },
            Act::Param {
                signal: "vox".into(),
                node: "comp".into(),
                param: "threshold_db".into(),
                value: -6.0,
                ramp_ms: 50.0
            },
            Act::Tsl {
                index: 7,
                left: false,
                right: false,
                label: "ON AIR".into()
            },
        ]
    );
    for _ in 0..4 {
        tick(&mut logic, &mix); // TSL runs at most every 50 ms
    }
    logic.set_button("g", "b", true);
    assert_eq!(tick(&mut logic, &mix).len(), 4, "pfl, talk, publish, tsl");
    // The edge fires one tick later: the event actions.
    let fired = tick(&mut logic, &mix);
    assert_eq!(
        fired,
        vec![
            Act::Monitor {
                monitor: "main".into(),
                source: Some(Source::Signal("vox".into()))
            },
            Act::Loudness {
                signal: "vox".into(),
                action: LoudnessAction::Reset
            },
            Act::Webhook {
                url: "https://user:pw@example.test/x?k=1".into()
            },
            Act::Log {
                text: "pressed -6".into()
            },
        ]
    );
    let described: Vec<String> = logic.recent("g").iter().map(|r| r.action.clone()).collect();
    assert!(
        described.contains(&"webhook https://example.test/x".to_string()),
        "{described:?}"
    );
    assert!(
        described.contains(&"TSL 7 left on right off \"ON AIR\"".to_string()),
        "{described:?}"
    );
    assert!(described.contains(&"monitor main source vox".to_string()));
    assert_eq!(logic.recent("g").len(), 13);
    for _ in 0..10 {
        logic.set_button("g", "b", false);
        tick(&mut logic, &mix);
        logic.set_button("g", "b", true);
        tick(&mut logic, &mix);
    }
    assert_eq!(
        logic.recent("g").len(),
        RECENT,
        "the list keeps the last 20"
    );
}

fn inverter() -> Macro {
    Macro {
        id: "inv".into(),
        name: "Invert".into(),
        description: String::new(),
        blocks: vec![
            block(
                "i",
                K::MacroIn {
                    name: "x".into(),
                    value_type: PortType::Boolean,
                },
            ),
            block("n", K::Not),
            block("l", K::Latch),
            block(
                "o",
                K::MacroOut {
                    name: "y".into(),
                    value_type: PortType::Boolean,
                },
            ),
        ],
        wires: vec![
            out("i", "n", "in"),
            out("n", "o", "in"),
            out("i", "l", "set"),
        ],
    }
}

#[test]
fn macros_run_inlined_with_state_per_instance() {
    let mut mix = Mix::default();
    mix.strips.push(strip("music", Format::Mono));
    mix.logic_macros.push(inverter());
    mix.logic.push(graph(
        vec![
            block("b", K::Button),
            block(
                "m1",
                K::Macro {
                    macro_id: "inv".into(),
                },
            ),
            block(
                "m2",
                K::Macro {
                    macro_id: "inv".into(),
                },
            ),
            block(
                "set",
                K::SetMute {
                    signal: "music".into(),
                },
            ),
        ],
        vec![out("b", "m1", "x"), wire("m1", "y", "set", "in")],
    ));
    assert_eq!(crate::mix::logic::issues(&mix), vec![]);
    let mut logic = Logic::default();
    assert_eq!(
        tick(&mut logic, &mix),
        vec![Act::Mute {
            signal: "music".into(),
            on: true
        }],
        "not(off) mutes, in the same tick: inlined, no delay"
    );
    tick(&mut logic, &mix); // an action runs at most every 20 ms
    logic.set_button("g", "b", true);
    assert_eq!(
        tick(&mut logic, &mix),
        vec![Act::Mute {
            signal: "music".into(),
            on: false
        }]
    );
    tick(&mut logic, &mix);
    assert_eq!(value(&logic, "m1.l"), json!(true));
    assert_eq!(
        value(&logic, "m2.l"),
        json!(false),
        "each instance has its own state"
    );
    // A changed macro recompiles; the latch keeps its state by id.
    mix.logic_macros[0].description = "changed".into();
    tick(&mut logic, &mix);
    assert_eq!(value(&logic, "m1.l"), json!(true));
    // A button inside a macro is addressed by its path.
    let mut with_button = inverter();
    with_button.blocks.push(block("k", K::Button));
    mix.logic_macros[0] = with_button;
    logic.set_button("g", "m2.k", true);
    tick(&mut logic, &mix);
    assert_eq!(value(&logic, "m2.k"), json!(true));
    assert_eq!(mix.logic_block_at("g", "m2.k"), Some(&K::Button));
}

#[test]
fn evaluation_order_is_deterministic() {
    // A long chain given in reverse block order still settles in one tick.
    let mut blocks = vec![block(
        "set",
        K::SetMute {
            signal: "music".into(),
        },
    )];
    let mut wires = vec![];
    for n in 0..20 {
        blocks.push(block(&format!("n{n}"), K::Not));
        if n > 0 {
            wires.push(out(&format!("n{n}"), &format!("n{}", n - 1), "in"));
        }
    }
    blocks.push(block("b", K::Button));
    wires.push(out("b", "n19", "in"));
    wires.push(out("n0", "set", "in"));
    let mix = mix_of(blocks, wires);
    let mut logic = Logic::default();
    // 20 nots of off: off.
    assert_eq!(
        tick(&mut logic, &mix),
        vec![Act::Mute {
            signal: "music".into(),
            on: false
        }]
    );
    tick(&mut logic, &mix); // an action runs at most every 20 ms
    logic.set_button("g", "b", true);
    assert_eq!(
        tick(&mut logic, &mix),
        vec![Act::Mute {
            signal: "music".into(),
            on: true
        }]
    );
}

#[test]
fn a_disabled_graph_does_not_run() {
    let mut mix = mix_of(
        vec![
            block("c", K::Const { value: 1.0 }),
            block(
                "m",
                K::SetMute {
                    signal: "music".into(),
                },
            ),
        ],
        vec![out("c", "m", "in")],
    );
    mix.logic[0].enabled = false;
    let mut logic = Logic::default();
    assert!(tick(&mut logic, &mix).is_empty());
    assert_eq!(logic.values("g"), None);
}

#[test]
fn the_tick_stays_cheap_at_the_limits() {
    // 32 graphs of 256 inlined blocks (4 instances of a 64-block macro).
    let mut m = Macro {
        id: "chain".into(),
        name: "Chain".into(),
        description: String::new(),
        blocks: vec![block(
            "i",
            K::MacroIn {
                name: "x".into(),
                value_type: PortType::Boolean,
            },
        )],
        wires: vec![],
    };
    for n in 0..62 {
        m.blocks.push(block(&format!("n{n}"), K::Not));
        let from = if n == 0 {
            "i".to_string()
        } else {
            format!("n{}", n - 1)
        };
        m.wires.push(out(&from, &format!("n{n}"), "in"));
    }
    m.blocks.push(block(
        "o",
        K::MacroOut {
            name: "y".into(),
            value_type: PortType::Boolean,
        },
    ));
    m.wires.push(out("n61", "o", "in"));
    let mut mix = Mix::default();
    mix.logic_macros.push(m);
    for g in 0..32 {
        let mut gr = graph(
            (0..4)
                .map(|n| {
                    block(
                        &format!("m{n}"),
                        K::Macro {
                            macro_id: "chain".into(),
                        },
                    )
                })
                .collect(),
            (1..4)
                .map(|n| wire(&format!("m{}", n - 1), "y", &format!("m{n}"), "x"))
                .collect(),
        );
        gr.id = format!("g{g}");
        mix.logic.push(gr);
    }
    assert_eq!(crate::mix::logic::issues(&mix), vec![]);
    let mut logic = Logic::default();
    tick(&mut logic, &mix);
    let start = std::time::Instant::now();
    for _ in 0..100 {
        tick(&mut logic, &mix);
    }
    let per_tick = start.elapsed() / 100;
    // A debug build; release is several times faster. The tick is 10 ms.
    assert!(
        per_tick < std::time::Duration::from_millis(10),
        "{per_tick:?}"
    );
    eprintln!("logic tick, 32 × 256 blocks: {per_tick:?}");
}

#[test]
fn a_rewiring_change_waits_for_the_change_lock() {
    use std::time::{Duration, Instant};
    let lock = std::sync::Arc::new(tokio::sync::Mutex::new(()));
    let mut w = Waiting::default();
    let t0 = Instant::now();
    assert!(matches!(w.take(&lock, t0), Taken::Idle));
    let publish = |on| Action {
        graph: "g".into(),
        block: "pub".into(),
        dry_run: false,
        retry: false,
        act: Act::Publish {
            signal: "aux".into(),
            on,
        },
    };
    w.push(publish(true));
    // An API change holds the lock: the action waits, then goes once it is free.
    let held = lock.clone().try_lock_owned().unwrap();
    for _ in 0..10 {
        assert!(matches!(w.take(&lock, t0), Taken::Wait));
    }
    drop(held);
    let Taken::Go(guard, acts) = w.take(&lock, t0) else {
        panic!("the lock is free")
    };
    assert_eq!(acts, vec![publish(true)]);
    // The logic holds the lock until its swap: the API waits meanwhile.
    assert!(lock.try_lock().is_err());
    drop(guard);
    assert!(lock.try_lock().is_ok());
    // At most one rewiring change per 250 ms; the newest of a block wins.
    w.push(publish(false));
    w.push(publish(true));
    assert!(matches!(
        w.take(&lock, t0 + Duration::from_millis(100)),
        Taken::Wait
    ));
    let Taken::Go(_, acts) = w.take(&lock, t0 + Duration::from_millis(250)) else {
        panic!("250 ms later")
    };
    assert_eq!(acts, vec![publish(true)]);
    // A lock held for a second is reported once; the action keeps waiting.
    w.push(publish(false));
    let later = t0 + Duration::from_secs(1);
    let held = lock.clone().try_lock_owned().unwrap();
    for _ in 1..MAX_WAIT_TICKS {
        assert!(matches!(w.take(&lock, later), Taken::Wait));
    }
    assert!(matches!(w.take(&lock, later), Taken::Late(a) if a == vec![publish(false)]));
    assert!(matches!(w.take(&lock, later), Taken::Wait));
    drop(held);
    assert!(matches!(w.take(&lock, later), Taken::Go(_, a) if a == vec![publish(false)]));
    assert!(matches!(w.take(&lock, later), Taken::Idle));
}

#[test]
fn an_unconnected_macro_input_drives_no_action() {
    // B1 (D159): the macro's actions hang off its exposed inputs only.
    let mut mix = mix_of(vec![], vec![]);
    mix.logic_macros.push(Macro {
        id: "act".into(),
        name: "Act".into(),
        description: String::new(),
        blocks: vec![
            block(
                "lvl",
                K::MacroIn {
                    name: "lvl".into(),
                    value_type: PortType::Level,
                },
            ),
            block(
                "on",
                K::MacroIn {
                    name: "on".into(),
                    value_type: PortType::Boolean,
                },
            ),
            block(
                "fader",
                K::SetFader {
                    signal: "vox".into(),
                    ramp_ms: 10.0,
                },
            ),
            block(
                "mute",
                K::SetMute {
                    signal: "vox".into(),
                },
            ),
        ],
        wires: vec![out("lvl", "fader", "level"), out("on", "mute", "in")],
    });
    mix.logic[0] = graph(
        vec![block(
            "m",
            K::Macro {
                macro_id: "act".into(),
            },
        )],
        vec![],
    );
    assert_eq!(crate::mix::logic::issues(&mix), vec![]);
    let mut logic = Logic::default();
    for _ in 0..3 {
        assert!(
            tick(&mut logic, &mix).is_empty(),
            "no fader 0 dB, no unmute"
        );
    }
    // Wired outside, it acts.
    mix.logic[0]
        .blocks
        .push(block("k", K::Const { value: -6.0 }));
    mix.logic[0].wires.push(wire("k", "out", "m", "lvl"));
    assert_eq!(
        tick(&mut logic, &mix),
        vec![Act::Fader {
            signal: "vox".into(),
            db: -6.0,
            ramp_ms: 10.0
        }]
    );
}

#[test]
fn a_changed_action_acts_on_its_new_target_at_once() {
    // B2 (D172): state is kept only for the same type and parameters.
    let mut mix = mix_of(
        vec![
            block("c", K::Const { value: 1.0 }),
            block(
                "m",
                K::SetMute {
                    signal: "vox".into(),
                },
            ),
        ],
        vec![out("c", "m", "in")],
    );
    let mut logic = Logic::default();
    assert_eq!(
        tick(&mut logic, &mix),
        vec![Act::Mute {
            signal: "vox".into(),
            on: true
        }]
    );
    tick(&mut logic, &mix);
    mix.logic[0].blocks[1].kind = K::SetMute {
        signal: "music".into(),
    };
    assert_eq!(
        tick(&mut logic, &mix),
        vec![Act::Mute {
            signal: "music".into(),
            on: true
        }],
        "music mutes; vox stays as it is"
    );
    // Another type with the same id starts over too.
    mix.logic[0].blocks[1].kind = K::SetPfl {
        signal: "music".into(),
    };
    tick(&mut logic, &mix);
    assert_eq!(logic.recent("g").last().unwrap().action, "PFL music on");
    // A TSL block moved to another index sends there.
    mix.logic[0].blocks[1].kind = K::TslOut { index: 3 };
    mix.logic[0].wires[0] = out("c", "m", "left");
    assert_eq!(
        tick(&mut logic, &mix),
        vec![Act::Tsl {
            index: 3,
            left: true,
            right: false,
            label: String::new()
        }]
    );
}

#[test]
fn edges_need_a_rise_they_saw() {
    // An input already on at the start, or a wire moved to a source that is
    // on, is not a rise: no recall after a restart or an edit (D172).
    let mut mix = mix_of(
        vec![
            block("off", K::Const { value: 0.0 }),
            block("on", K::Const { value: 1.0 }),
            block("e", K::Edge),
            block(
                "r",
                K::Recall {
                    snapshot: "s".into(),
                },
            ),
        ],
        vec![out("on", "e", "in"), out("off", "r", "in")],
    );
    let mut logic = Logic::default();
    for _ in 0..3 {
        tick(&mut logic, &mix);
    }
    assert_eq!(value(&logic, "e"), json!(0), "on from the start: no edge");
    mix.logic[0].wires[1] = out("on", "r", "in");
    assert!(
        (0..3).all(|_| tick(&mut logic, &mix).is_empty()),
        "a wire moved from off to on does not fire the recall"
    );
}

#[test]
fn not_a_number_is_no_change_and_infinity_is_clamped() {
    // B3 (D170).
    let mut mix = mix_of(
        vec![
            block(
                "l",
                K::Level {
                    signal: "vox".into(),
                },
            ),
            block(
                "f",
                K::SetFader {
                    signal: "vox".into(),
                    ramp_ms: 10.0,
                },
            ),
        ],
        vec![out("l", "f", "level")],
    );
    mix.logic[0]
        .blocks
        .push(block("z", K::Const { value: 0.0 }));
    let mut logic = Logic::default();
    let at = |logic: &mut Logic, db: f64| {
        let level = BTreeMap::from([("vox".to_string(), db)]);
        let acts: Vec<Act> = logic
            .tick(&Readings {
                level_db: &level,
                ..Readings::quiet(&mix)
            })
            .into_iter()
            .map(|a| a.act)
            .collect();
        acts
    };
    assert!(at(&mut logic, f64::NAN).is_empty(), "NaN does nothing");
    assert!(at(&mut logic, f64::NAN).is_empty());
    assert_eq!(at(&mut logic, -10.0).len(), 1);
    for _ in 0..5 {
        assert!(at(&mut logic, f64::NAN).is_empty(), "and is not a change");
    }
    let inf = at(&mut logic, f64::INFINITY);
    assert_eq!(inf.len(), 1);
    assert!(at(&mut logic, f64::INFINITY).is_empty());
    assert_eq!(
        clamp_fader(f64::INFINITY).0,
        crate::mix::model::MAX_FADER_DB
    );
    assert_eq!(clamp_fader(f64::NEG_INFINITY).0, f64::NEG_INFINITY);
}

#[test]
fn webhooks_and_faders_are_rate_limited() {
    // D173: a webhook at most every 100 ms, a fader at most every 20 ms; the
    // newest fader value goes out when it may.
    let mix = mix_of(
        vec![
            block("b", K::Button),
            block(
                "w",
                K::Webhook {
                    url: "http://h/x".into(),
                },
            ),
            block(
                "l",
                K::Level {
                    signal: "vox".into(),
                },
            ),
            block(
                "f",
                K::SetFader {
                    signal: "vox".into(),
                    ramp_ms: 10.0,
                },
            ),
        ],
        vec![out("b", "w", "in"), out("l", "f", "level")],
    );
    let mut logic = Logic::default();
    let mut posts = 0;
    let mut faders = Vec::new();
    for n in 0..40 {
        logic.set_button("g", "b", n % 2 == 1);
        let level = BTreeMap::from([("vox".to_string(), -f64::from(n))]);
        for a in logic.tick(&Readings {
            level_db: &level,
            ..Readings::quiet(&mix)
        }) {
            match a.act {
                Act::Webhook { .. } => posts += 1,
                Act::Fader { db, .. } => faders.push(db),
                _ => {}
            }
        }
    }
    assert_eq!(posts, 4, "20 rises in 400 ms: one post per 100 ms");
    assert_eq!(faders.len(), 20, "40 changes in 400 ms: one per 20 ms");
    assert_eq!(faders[1], -2.0, "the newest value");
}

#[test]
fn an_action_without_a_target_is_valid_and_never_acts() {
    // A new block from the palette has no target yet (D164).
    let mix = mix_of(
        vec![
            block("c", K::Const { value: 1.0 }),
            block(
                "m",
                K::SetMute {
                    signal: String::new(),
                },
            ),
            block(
                "f",
                K::SetFader {
                    signal: String::new(),
                    ramp_ms: 10.0,
                },
            ),
            block(
                "v",
                K::SetVca {
                    vca: String::new(),
                    ramp_ms: 10.0,
                },
            ),
            block(
                "p",
                K::SetParam {
                    signal: "vox".into(),
                    node: String::new(),
                    param: String::new(),
                    ramp_ms: 10.0,
                },
            ),
            block(
                "t",
                K::SetTalk {
                    mic: String::new(),
                    destination: String::new(),
                },
            ),
            block(
                "mon",
                K::SetMonitor {
                    monitor: String::new(),
                    source: None,
                },
            ),
            block(
                "r",
                K::Recall {
                    snapshot: String::new(),
                },
            ),
            block(
                "in",
                K::Mute {
                    signal: String::new(),
                },
            ),
        ],
        vec![
            out("c", "m", "in"),
            out("c", "f", "level"),
            out("c", "v", "level"),
            out("c", "p", "value"),
            out("c", "t", "in"),
            out("in", "mon", "in"),
            out("in", "r", "in"),
        ],
    );
    let w = crate::mix::logic::warnings(&mix.logic[0].blocks);
    let marked: Vec<&str> = w.iter().filter_map(|i| i.block.as_deref()).collect();
    assert_eq!(marked, ["m", "f", "v", "p", "t", "mon", "r", "in"]);
    assert!(w.iter().all(|i| i.message == "choose a target"));
    let mut logic = Logic::default();
    for _ in 0..3 {
        assert!(tick(&mut logic, &mix).is_empty());
    }
    assert_eq!(value(&logic, "in"), json!(false));
}

#[test]
fn a_forgotten_action_runs_again_as_a_quiet_retry() {
    // D167: a rewiring change that failed is retried, without a new entry.
    let mix = mix_of(
        vec![
            block("c", K::Const { value: 1.0 }),
            block(
                "p",
                K::SetPublish {
                    signal: "music".into(),
                },
            ),
        ],
        vec![out("c", "p", "in")],
    );
    let mut logic = Logic::default();
    let first = logic.tick(&Readings::quiet(&mix));
    assert_eq!(first.len(), 1);
    assert!(!first[0].retry);
    assert!(logic.tick(&Readings::quiet(&mix)).is_empty());
    logic.forget("g", "p");
    let again = logic.tick(&Readings::quiet(&mix));
    assert_eq!(again.len(), 1);
    assert!(again[0].retry);
    assert_eq!(logic.recent("g").len(), 1, "listed once");
    assert!(logic.tick(&Readings::quiet(&mix)).is_empty());
}
