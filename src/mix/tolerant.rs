//! Tolerant loading of saved show state (D279): a `mix.json` or a show-history
//! version written by a newer version loads with what this version does not
//! know dropped, instead of being refused as a whole.
//!
//! The strict parse runs first and is all that runs for a mix this version
//! wrote. When it fails, the JSON is repaired step by step at the place the
//! error names: an unknown field is removed; an unknown variant (a new node
//! type, logic block kind, …) removes the array element it is in, or the field
//! when it is not in an array. Anything else (a wrong type, a missing field)
//! is not repaired. The API stays strict: this is only for files on disk.

use serde::de::DeserializeOwned;
use serde_json::Value;
use serde_path_to_error::Segment;

use super::Mix;

/// At most this many repairs, then the file counts as broken.
pub const MAX_REPAIRS: usize = 1000;

#[derive(Debug, Clone, PartialEq)]
enum Step {
    Key(String),
    Index(usize),
}

/// One thing a repair removed.
#[derive(Debug, Clone, PartialEq)]
struct Removed {
    at: Vec<Step>,
    value: Value,
}

fn path_text(at: &[Step]) -> String {
    let mut s = String::new();
    for step in at {
        match step {
            Step::Key(k) if s.is_empty() => s.push_str(k),
            Step::Key(k) => {
                s.push('.');
                s.push_str(k);
            }
            Step::Index(i) => s.push_str(&format!("[{i}]")),
        }
    }
    s
}

/// The JSON steps of an error's path, as far as they exist in `root`. A
/// buffered part (`flatten`, internally tagged and untagged enums) ends the
/// path where its buffering began.
fn steps(root: &Value, path: &serde_path_to_error::Path) -> Vec<Step> {
    let mut out = Vec::new();
    let mut at = root;
    for seg in path.iter() {
        let next = match seg {
            Segment::Seq { index } => at.get(*index).map(|v| (v, Step::Index(*index))),
            Segment::Map { key } | Segment::Enum { variant: key } => {
                at.get(key).map(|v| (v, Step::Key(key.clone())))
            }
            Segment::Unknown => None,
        };
        match next {
            Some((v, step)) => {
                at = v;
                out.push(step);
            }
            // An externally tagged variant name may not be a JSON key.
            None if matches!(seg, Segment::Enum { .. }) => {}
            None => break,
        }
    }
    out
}

fn get_mut<'a>(root: &'a mut Value, at: &[Step]) -> Option<&'a mut Value> {
    at.iter().try_fold(root, |v, step| match step {
        Step::Key(k) => v.get_mut(k),
        Step::Index(i) => v.get_mut(*i),
    })
}

/// Take `at` out of `root` (a key of an object or an element of an array).
fn take(root: &mut Value, at: &[Step]) -> Option<Removed> {
    let (last, parent) = at.split_last()?;
    let value = match (get_mut(root, parent)?, last) {
        (Value::Object(o), Step::Key(k)) => o.remove(k)?,
        (Value::Array(a), Step::Index(i)) if *i < a.len() => a.remove(*i),
        _ => return None,
    };
    Some(Removed {
        at: at.to_vec(),
        value,
    })
}

/// The path of the first object at or below `at` that has the key `name`.
fn find_key(v: &Value, at: &mut Vec<Step>, name: &str) -> bool {
    match v {
        Value::Object(o) => {
            if o.contains_key(name) {
                at.push(Step::Key(name.to_string()));
                return true;
            }
            for (k, child) in o {
                at.push(Step::Key(k.clone()));
                if find_key(child, at, name) {
                    return true;
                }
                at.pop();
            }
            false
        }
        Value::Array(a) => {
            for (i, child) in a.iter().enumerate() {
                at.push(Step::Index(i));
                if find_key(child, at, name) {
                    return true;
                }
                at.pop();
            }
            false
        }
        _ => false,
    }
}

/// Unknown field `name` at `at`: the field itself, or a key `name` of the
/// object there or below it (inside a buffered part the path stops early).
fn remove_field(root: &mut Value, at: &[Step], name: &str) -> Option<Removed> {
    if matches!(at.last(), Some(Step::Key(k)) if k == name) {
        return take(root, at);
    }
    let mut path = at.to_vec();
    let node = at.iter().try_fold(&*root, |v, step| match step {
        Step::Key(k) => v.get(k),
        Step::Index(i) => v.get(*i),
    })?;
    find_key(node, &mut path, name).then(|| take(root, &path))?
}

/// A logic block taken out of a graph or macro takes its wires with it.
fn drop_wires(root: &mut Value, block: &Removed) -> Vec<Removed> {
    let [.., Step::Key(k), Step::Index(_)] = block.at.as_slice() else {
        return Vec::new();
    };
    let Some(id) = block.value.get("id").and_then(Value::as_str) else {
        return Vec::new();
    };
    if k != "blocks" {
        return Vec::new();
    }
    let mut at = block.at[..block.at.len() - 2].to_vec();
    at.push(Step::Key("wires".into()));
    let Some(Value::Array(wires)) = get_mut(root, &at) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i < wires.len() {
        let names = |f: &str| wires[i].get(f).and_then(Value::as_str) == Some(id);
        if names("from_block") || names("to_block") {
            let mut w = at.clone();
            w.push(Step::Index(i + out.len()));
            out.push(Removed {
                at: w,
                value: wires.remove(i),
            });
        } else {
            i += 1;
        }
    }
    out
}

/// Parse `v`; on failure repair it (see the module) and parse again. The error
/// names the strict parse's and, after repairs, the one that stopped them.
fn parse<T: DeserializeOwned>(mut v: Value) -> Result<(T, Vec<Removed>), String> {
    let mut removed = Vec::new();
    let mut first = None;
    for _ in 0..=MAX_REPAIRS {
        let e = match serde_path_to_error::deserialize::<_, T>(v.clone()) {
            Ok(t) => return Ok((t, removed)),
            Err(e) => e,
        };
        let first = first.get_or_insert_with(|| e.to_string());
        let at = steps(&v, e.path());
        let msg = e.inner().to_string();
        let r = if let Some(name) = msg
            .strip_prefix("unknown field `")
            .and_then(|m| m.split('`').next())
        {
            remove_field(&mut v, &at, name)
        } else if msg.starts_with("unknown variant `") {
            take(&mut v, &at)
        } else {
            None
        };
        let Some(r) = r else {
            let last = e.to_string();
            return Err(if removed.is_empty() {
                last
            } else {
                format!("{first}; after dropping what a newer version may have added: {last}")
            });
        };
        let wires = drop_wires(&mut v, &r);
        removed.push(r);
        removed.extend(wires);
    }
    Err(first.unwrap_or_default())
}

/// A saved mix: the mix and the paths of what was dropped (empty when it
/// parsed as it is).
pub fn mix(v: Value) -> Result<(Mix, Vec<String>), String> {
    let (mix, removed) = parse::<Mix>(v)?;
    Ok((mix, removed.iter().map(|r| path_text(&r.at)).collect()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn show() -> Value {
        use crate::mix::model::{Bus, Strip};
        use crate::mix::Format;
        let mut m = Mix::default();
        m.strips.push(Strip::new("vox", Format::Mono));
        m.strips.push(Strip::new("gtr", Format::Mono));
        m.buses.push(Bus::new("main", Format::Stereo));
        let mut v = serde_json::to_value(&m).unwrap();
        v["strips"][0]["chain"] = json!([
            {"id": "eq-1", "type": "eq",
             "bands": [{"type": "bell", "freq_hz": 1000, "q": 1, "gain_db": 3}]},
            {"id": "x-1", "type": "future-reverb", "size": 3},
            {"id": "comp", "type": "compressor", "threshold_db": -20.0, "ratio": 4.0,
             "knee_db": 0.0, "attack_ms": 5.0, "release_ms": 100.0, "makeup_db": 0.0,
             "detector": "peak"}
        ]);
        v["logic"] = json!([{
            "id": "g", "name": "G",
            "blocks": [
                {"id": "b", "type": "button"},
                {"id": "n", "type": "future-block", "x": 3.0},
                {"id": "m", "type": "set-mute", "signal": "gtr"}
            ],
            "wires": [
                {"from_block": "b", "from_port": "out", "to_block": "n", "to_port": "in"},
                {"from_block": "b", "from_port": "out", "to_block": "m", "to_port": "in"}
            ]
        }]);
        v
    }

    /// The test show with what 1.5 knows only, so it parses strictly.
    fn known() -> Value {
        let mut v = show();
        v["strips"][0]["chain"].as_array_mut().unwrap().remove(1);
        v["logic"][0]["blocks"].as_array_mut().unwrap().remove(1);
        v["logic"][0]["wires"].as_array_mut().unwrap().remove(0);
        v
    }

    #[test]
    fn a_known_mix_parses_unchanged() {
        let (m, dropped) = mix(known()).unwrap();
        assert!(dropped.is_empty());
        assert_eq!(m, serde_json::from_value::<Mix>(known()).unwrap());
    }

    #[test]
    fn a_newer_mix_drops_what_it_does_not_know() {
        let mut v = show();
        v["future_top"] = json!({"a": 1});
        v["strips"][0]["future_field"] = json!(true);
        v["strips"][0]["chain"][0]["future_param"] = json!(2);
        v["buses"][0]["future_field"] = json!("x");
        v["control_room"]["future_field"] = json!([1]);
        v["logic"][0]["future_field"] = json!(1);
        let (m, mut dropped) = mix(v).unwrap();
        dropped.sort();
        assert_eq!(
            dropped,
            [
                "buses[0].future_field",
                "control_room.future_field",
                "future_top",
                "logic[0].blocks[1]",
                "logic[0].future_field",
                "logic[0].wires[0]",
                "strips[0].chain[0].future_param",
                "strips[0].chain[1]",
                "strips[0].future_field",
            ]
        );
        assert_eq!(m, serde_json::from_value::<Mix>(known()).unwrap());
        m.validate(2, 96).unwrap();
    }

    #[test]
    fn unknown_values_outside_an_array_are_repaired_only_when_optional() {
        // A required field: removed, then missing: not repaired.
        let mut v = known();
        v["strips"][1]["format"] = json!("future-format");
        let e = mix(v).unwrap_err();
        assert!(e.starts_with("strips[1].format: unknown variant"), "{e}");
        assert!(e.ends_with("after dropping what a newer version may have added: strips[1]: missing field `format`"), "{e}");
        // An optional one: removed.
        let mut v = known();
        v["outputs"] = json!([{"signal": "main", "format": "future-format"}]);
        let (m, dropped) = mix(v).unwrap();
        assert_eq!(dropped, ["outputs[0].format"]);
        assert_eq!(m.outputs[0].format, None);
        // An untagged enum's error names neither a field nor a variant.
        let mut v = known();
        v["strips"][1]["patch"][0] = json!({"input": 1, "channel": 2, "future": 1});
        assert!(mix(v).is_err());
    }

    #[test]
    fn a_wrong_type_is_not_repaired() {
        let mut v = known();
        v["strips"][0]["fader_db"] = json!("loud");
        v["strips"][0]["future_field"] = json!(true);
        let e = mix(v).unwrap_err();
        assert!(e.starts_with("strips[0]."), "{e}");
        let mut v = known();
        v["strips"][0].as_object_mut().unwrap().remove("name");
        assert!(mix(v).is_err());
    }

    #[test]
    fn the_api_stays_strict() {
        let mut v = known();
        v["strips"][0]["future_field"] = json!(true);
        assert!(serde_json::from_value::<Mix>(v).is_err());
        assert!(serde_json::from_value::<super::super::chain::Proc>(
            json!({"id": "x", "type": "eq", "bands": [], "future": 1})
        )
        .is_err());
    }
}
