#!/usr/bin/env bash
# The rollback target (1.5.1, unchanged) loads what 1.6 saves: builds the
# released v1.5.1 in a git worktree, runs its tolerant load and validation
# (as at its start, src/main.rs) on tests/fixtures/rollback-1.6.json and writes
# what it made of it to tests/fixtures/rollback-1.6.loaded-by-1.5.1.json, which
# `cargo test` (mix::rollback) then checks: valid, the same published outputs.
#
#   tests/rollback/check-1.5.1.sh [worktree dir]   # needs git, cargo; no libmxl
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
TREE="${1:-$ROOT/target/rollback-1.5.1}"
[[ -d "$TREE" ]] || git -C "$ROOT" worktree add --detach "$TREE" v1.5.1 >/dev/null
cat >"$TREE/tests/rollback_from_1_6.rs" <<'RS'
#[test]
fn load() {
    let fixture = std::env::var("FIXTURE").unwrap();
    let out = std::env::var("OUT").unwrap();
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(fixture).unwrap()).unwrap();
    let summary = match mxl_audio_mixer::mix::tolerant::mix(v) {
        Ok((mix, dropped)) => {
            let check = mix.validate(8, 96);
            serde_json::json!({
                "valid": check.is_ok(),
                "error": check.err(),
                "dropped": dropped,
                "outputs": mix.outputs,
                "sends": mix.sends,
                "chains": mix.chains().map(|(id, c)| (id.to_string(), c.iter().map(|p| p.id.clone()).collect::<Vec<_>>())).collect::<std::collections::BTreeMap<_, _>>(),
                "virtual_outputs": mix.virtual_outputs,
            })
        }
        Err(e) => serde_json::json!({"valid": false, "error": e}),
    };
    std::fs::write(out, serde_json::to_string_pretty(&summary).unwrap() + "\n").unwrap();
}
RS
(cd "$TREE" && CARGO_TARGET_DIR="$TREE/target" FIXTURE="$ROOT/tests/fixtures/rollback-1.6.json" \
    OUT="$ROOT/tests/fixtures/rollback-1.6.loaded-by-1.5.1.json" \
    cargo test --locked --test rollback_from_1_6 -q)
python3 -c "import json,sys; d=json.load(open(sys.argv[1])); print('1.5.1:', 'valid' if d['valid'] else 'INVALID ' + str(d['error']), '; outputs', [o['signal'] for o in d.get('outputs', [])])" "$ROOT/tests/fixtures/rollback-1.6.loaded-by-1.5.1.json"
