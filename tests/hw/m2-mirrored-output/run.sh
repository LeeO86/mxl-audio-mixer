#!/usr/bin/env bash
# §17 mirrored outputs, on two nodes with mxl-fabrics-agent. See README.md.
#
#   node A:  tests/hw/m2-mirrored-output/run.sh source [commit_samples]
#   node B:  tests/hw/m2-mirrored-output/run.sh check <sender id> <domain id> <flow id> [seconds]
#
# Both need libmxl 218ddaa in the linker path (or LD_LIBRARY_PATH=/opt/mxl/lib), the
# platform registry in NMOS_REGISTRY_ADDRESS / NMOS_REGISTRY_PORT, NMOS_HOST_ADDRESS
# (the node or pod IP) and the kernel TAI offset set.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../../.." && pwd)"
SCAN="${MXL_DOMAIN_SCAN_PATH:-/Volumes/mxl}"
BIN="${BIN_DIR:-$ROOT/target/release}"
OUT="${OUT:-$PWD/m2-mirrored-output-$(date -u +%Y%m%dT%H%M%SZ)}"
: "${NMOS_REGISTRY_ADDRESS:?set the platform registry}" "${NMOS_REGISTRY_PORT:?}" "${NMOS_HOST_ADDRESS:?}"
mkdir -p "$OUT"
PIDS=()
TONES=""
cleanup() {
    for pid in "${PIDS[@]}"; do kill -TERM "$pid" 2>/dev/null || true; done
    wait
    if [[ -n "$TONES" ]]; then rm -rf "$TONES"; fi
}
trap cleanup EXIT

offset=$(python3 -c "import time; print(round(time.clock_gettime(time.CLOCK_TAI) - time.clock_gettime(time.CLOCK_REALTIME)))")
[[ "$offset" == 37 ]] || { echo "FAIL: CLOCK_TAI is not UTC + 37 s on this host"; exit 1; }
if [[ ! -x "$BIN/mxl-audio-mixer" ]]; then
    (cd "$ROOT" && cargo build --release -q --bins)
fi

json() { curl -fsS "$1" | python3 -c "import json,sys; d=json.load(sys.stdin); print($2)"; }

start_mixer() { # <seed> <web port> <nmos port> <state dir>
    mkdir -p "$4"
    env HOST_ID="$1" NMOS_SEED="$1" WEB_PORT="$2" NMOS_PORT="$3" MIXER_STATE_DIR="$4" \
        MXL_DOMAIN_SCAN_PATH="$SCAN" MXL_CLEANUP_ON_EXIT=true MIXER_INPUT_FLOWS=2 \
        "$BIN/mxl-audio-mixer" >>"$OUT/$1.log" 2>&1 &
    PIDS+=("$!")
    for _ in $(seq 1 100); do curl -fsS -o /dev/null "http://127.0.0.1:$2/readyz" 2>/dev/null && return 0; sleep 0.2; done
    echo "FAIL: mixer $1 not ready (registry reachable?)"
    exit 1
}

activate() { # <nmos port> <sender id or ""> <domain id> <flow id>
    local receiver sender=null
    [[ -n "$2" ]] && sender="\"$2\""
    receiver=$(json "http://127.0.0.1:$1/x-nmos/node/v1.3/receivers" "[r['id'] for r in d if r['label'] == 'Input 1'][0]")
    curl -fsS -o /dev/null -X PATCH -H 'content-type: application/json' \
        "http://127.0.0.1:$1/x-nmos/connection/v1.2/single/receivers/$receiver/staged" \
        -d "{\"sender_id\": $sender, \"master_enable\": true, \"activation\": {\"mode\": \"activate_immediate\"}, \"transport_params\": [{\"mxl_domain_id\": \"$3\", \"mxl_flow_id\": \"$4\"}]}"
}

case "${1:-}" in
source)
    COMMIT="${2:-}"
    TONES="$SCAN/m2-hw-tones"
    TONE_FLOW=$(python3 -c "import uuid; print(uuid.uuid4())")
    TONE_DOMAIN=$(python3 -c "import uuid; print(uuid.uuid4())")
    "$BIN/mxl-mixer-tone" --domain "$TONES" --flow "$TONE_FLOW" --domain-id "$TONE_DOMAIN" --channels 2 >>"$OUT/tone.log" 2>&1 &
    PIDS+=("$!")
    start_mixer m2-hw-source 8164 3316 "$OUT/state-source"
    activate 3316 "" "$TONE_DOMAIN" "$TONE_FLOW"
    web=http://127.0.0.1:8164
    curl -fsS -o /dev/null -X POST -H 'content-type: application/json' "$web/api/v1/strips" \
        -d '{"id": "tone", "format": "mono", "patch": [{"input": 1, "channel": 1}]}'
    curl -fsS -o /dev/null -X POST -H 'content-type: application/json' "$web/api/v1/buses" \
        -d '{"id": "pgm", "name": "M2 HW PGM", "format": "stereo"}'
    curl -fsS -o /dev/null -X PUT "$web/api/v1/sends/tone/pgm"
    body='{}'
    [[ -n "$COMMIT" ]] && body="{\"commit_samples\": $COMMIT}"
    curl -fsS -o /dev/null -X PUT -H 'content-type: application/json' "$web/api/v1/outputs/pgm" -d "$body"
    out=$(curl -fsS "$web/api/v1/outputs/pgm")
    echo "$out" >"$OUT/output.json"
    flow=$(echo "$out" | python3 -c "import json,sys; print(json.load(sys.stdin)['mxl_flow_id'])")
    sender=$(echo "$out" | python3 -c "import json,sys; print(json.load(sys.stdin)['nmos_sender_id'])")
    commit=$(echo "$out" | python3 -c "import json,sys; print(json.load(sys.stdin)['commit_samples'])")
    domain=$(json "$web/api/v1/status" "d['mxl']['output_domain_id']")
    echo "publishing flow $flow (commit $commit samples); on node B run:"
    echo "  tests/hw/m2-mirrored-output/run.sh check $sender $domain $flow 60"
    echo "Ctrl-C stops."
    wait
    ;;
check)
    SENDER="${2:?sender id}" DOMAIN="${3:?domain id}" FLOW="${4:?flow id}" SECONDS_="${5:-60}"
    start_mixer m2-hw-check 8166 3318 "$OUT/state-check"
    activate 3318 "$SENDER" "$DOMAIN" "$FLOW"
    mirror=""
    for _ in $(seq 1 300); do
        mirror=$(find "$SCAN" -maxdepth 2 -name "$FLOW.mxl-flow" -printf '%h\n' 2>/dev/null | head -1)
        [[ -n "$mirror" ]] && break
        sleep 0.1
    done
    [[ -n "$mirror" ]] || { echo "FAIL: flow $FLOW did not appear below $SCAN within 30 s"; exit 1; }
    echo "mirror: $mirror"
    sleep 3
    web=http://127.0.0.1:8166
    missing_before=$(json "$web/api/v1/inputs/1" "d['missing_samples']")
    "$BIN/mxl-mixer-probe" --domain "$mirror" --flow "$FLOW" --seconds "$SECONDS_" \
        --behind-ms "${BEHIND_MS:-150}" --expect 1:1:0.70710677 --expect 2:1:0.70710677 >"$OUT/probe.json"
    curl -fsS "$web/api/v1/inputs/1" >"$OUT/input.json"
    python3 - "$OUT/probe.json" "$OUT/input.json" "$missing_before" "$SECONDS_" <<'EOF'
import json, sys
probe = json.load(open(sys.argv[1]))
inp = json.load(open(sys.argv[2]))
missing = inp["missing_samples"] - int(sys.argv[3])
expected = float(sys.argv[4]) * 48000
lag = inp.get("arrival_lag_ms") or {}
checks = [
    ("full sample rate on the far side", probe["samples"] >= expected - 960, f'{probe["samples"]} of {expected:.0f}'),
    ("no failed reads (gaps)", probe["failed_reads"] == 0, probe["failed_reads"]),
    ("samples correct", all(c["max_error"] < 1e-5 for c in probe["checks"]), [c["max_error"] for c in probe["checks"]]),
    ("mixer B input: 0 missing samples", missing == 0, missing),
]
print(f'mixer B input: state {inp["state"]}, effective delay {inp["effective_delay_ms"]} ms, arrival lag {lag}')
failed = False
for name, ok, value in checks:
    print(f'{"PASS" if ok else "FAIL"}: {name} ({value})')
    failed |= not ok
sys.exit(1 if failed else 0)
EOF
    ;;
*)
    echo "usage: run.sh source [commit_samples] | run.sh check <sender id> <domain id> <flow id> [seconds]"
    exit 2
    ;;
esac
