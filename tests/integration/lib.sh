#!/usr/bin/env bash
# Helpers for the integration tests. Needs: the mixer and mxl-mixer-tone binaries
# ($MIXER_BIN, $TONE_BIN; default target/debug), MediaMTX ($MEDIAMTX_BIN, for
# the preview), libmxl in the linker path
# (LD_LIBRARY_PATH=/opt/mxl/lib), python3, curl, docker for the registry.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
MIXER_BIN="${MIXER_BIN:-$ROOT/target/debug/mxl-audio-mixer}"
TONE_BIN="${TONE_BIN:-$ROOT/target/debug/mxl-mixer-tone}"
IT_OUT="${IT_OUT:-$ROOT/ci-artifacts/integration}"
HOST_IP="${HOST_IP:-$(ip -4 route get 1.1.1.1 | sed -n 's/.* src \([0-9.]*\).*/\1/p')}"
REGISTRY_IMAGE="${NMOS_REGISTRY_IMAGE:-mirror.gcr.io/rhastie/nmos-cpp@sha256:c54c8ded3b61740dbd579c03892b0229842250417aa397d87a92e2b91ba645b8}"
WEB="http://127.0.0.1:${WEB_PORT:-8190}"
NMOS="http://127.0.0.1:${NMOS_PORT:-3342}"
mkdir -p "$IT_OUT"
WORK="$(mktemp -d /dev/shm/mixer-it.XXXXXX)"
PIDS=()
CONTAINERS=()

cleanup() {
    for pid in "${PIDS[@]}"; do kill -TERM "$pid" 2>/dev/null || true; done
    for pid in "${PIDS[@]}"; do wait "$pid" 2>/dev/null || true; done
    for c in "${CONTAINERS[@]}"; do docker rm -f "$c" >/dev/null 2>&1 || true; done
    rm -rf "$WORK"
}
trap cleanup EXIT

log() { echo "[$(basename "$0")] $*"; }
fail() { log "FAIL: $*"; exit 1; }
pass() { log "PASS: $*"; }

wait_until() { # <seconds> <description> <command...>
    local secs=$1 what=$2
    shift 2
    for _ in $(seq 1 $((secs * 10))); do
        if "$@" >/dev/null 2>&1; then return 0; fi
        sleep 0.1
    done
    fail "timed out after ${secs}s waiting for $what"
}

http_ok() { curl -fsS -o /dev/null "$1"; }

json() { # <url> <python expression on d>
    curl -fsS "$1" | python3 -c "import json,sys; d=json.load(sys.stdin); print($2)"
}

input() { # <n> <field>
    json "$WEB/api/v1/inputs/$1" "d['$2']"
}

is_state() { [[ "$(input "$1" state)" == "$2" ]]; }

start_registry() { # registration 8010, query 8011
    local name="mixer-it-registry-$$"
    docker run -d --name "$name" --network host -v "$ROOT/ci/registry.json:/home/registry.json:ro" "$REGISTRY_IMAGE" >/dev/null
    CONTAINERS+=("$name")
    wait_until 60 "nmos-cpp registry" http_ok "http://$HOST_IP:8011/x-nmos/query/v1.3/nodes"
}

MIXER_PID=""
# The built-in MediaMTX on ports other than the defaults, so a mixer running on
# this machine does not collide; without MEDIAMTX_BIN the mixer only logs that
# it cannot start it.
start_mixer() { # [VAR=value...] ; logs to $IT_OUT/<test>-mixer.log
    env HOST_ID=it NMOS_HOST_ADDRESS="$HOST_IP" MXL_DOMAIN_SCAN_PATH="$WORK/mxl" \
        MIXER_STATE_DIR="$WORK/state" MIXER_INPUT_FLOWS=4 \
        MEDIAMTX_RTSP_PORT=8964 MEDIAMTX_WHEP_PORT=8609 MEDIAMTX_ICE_UDP_PORT=8209 "$@" \
        "$MIXER_BIN" >>"$IT_OUT/$(basename "$0" .sh)-mixer.log" 2>&1 &
    MIXER_PID=$!
    PIDS+=("$MIXER_PID")
    wait_until 30 "mixer /livez" http_ok "$WEB/livez"
}

stop_mixer() { # returns the exit code
    kill -TERM "$MIXER_PID"
    local rc=0
    wait "$MIXER_PID" || rc=$?
    return $rc
}

TONE_PID=""
start_tone() { # <domain dir> <flow id> [args...]
    local dir=$1 flow=$2
    shift 2
    "$TONE_BIN" --domain "$dir" --flow "$flow" "$@" >>"$IT_OUT/$(basename "$0" .sh)-tone.log" 2>&1 &
    TONE_PID=$!
    PIDS+=("$TONE_PID")
}

domain_id() { python3 -c "import json; print(json.load(open('$1/domain_def.json'))['id'])"; }

route() { # <input n> <domain id> <flow id> [master_enable]
    local receiver
    receiver=$(json "$NMOS/x-nmos/node/v1.3/receivers" "[r['id'] for r in d if r['label'] == 'Input $1'][0]")
    local code
    code=$(curl -s -o /dev/null -w '%{http_code}' -X PATCH -H 'content-type: application/json' \
        "$NMOS/x-nmos/connection/v1.2/single/receivers/$receiver/staged" \
        -d "{\"master_enable\": ${4:-true}, \"activation\": {\"mode\": \"activate_immediate\"}, \"transport_params\": [{\"mxl_domain_id\": \"$2\", \"mxl_flow_id\": \"$3\"}]}")
    [[ "$code" == 200 ]] || fail "IS-05 activation of input $1 answered $code"
}

PROBE_BIN="${PROBE_BIN:-$ROOT/target/debug/mxl-mixer-probe}"

api() { # <method> <path> [body] ; prints the response, fails on HTTP errors
    curl -fsS -X "$1" -H 'content-type: application/json' "$WEB$2" ${3:+-d "$3"}
}

settled() { # <input>: wait until its automatic delay held for 1.5 s (at most 10 s)
    local last now same=0
    last=$(input "$1" effective_delay_ms)
    for _ in $(seq 1 100); do
        sleep 0.1
        now=$(input "$1" effective_delay_ms)
        if [[ "$now" == "$last" ]]; then
            same=$((same + 1))
            ((same >= 15)) && return 0
        else
            same=0
            last=$now
        fi
    done
}

glitches() { # input 1 missing samples + DSP deadline misses + its delay (an automatic change fades the input)
    local missing misses delay
    missing=$(input 1 missing_samples)
    misses=$(curl -fsS "$WEB/metrics" | sed -n 's/^mxl_audio_mixer_dsp_deadline_misses_total \(.*\)/\1/p')
    delay=$(json "$WEB/api/v1/inputs/1" "round(d['effective_delay_ms'] * 48)")
    echo $((missing + misses + delay))
}

# probe <seconds> <CH:TONE:GAIN...>: read $OUT_FLOW in $OWN with mxl-mixer-probe;
# fails unless every check is within 1e-5. The probe's JSON is left in $PROBED.
export PROBED=""
probe() {
    local secs=$1 out args=() before attempt
    shift
    for e in "$@"; do args+=(--expect "$e"); done
    for attempt in 1 2 3; do
        before=$(glitches)
        out=$("$PROBE_BIN" --domain "$OWN" --flow "$OUT_FLOW" --seconds "$secs" "${args[@]}")
        echo "$out" >>"$IT_OUT/$(basename "$0" .sh)-probe.log"
        if python3 - "$out" <<'EOF'; then
import json, sys
d = json.loads(sys.argv[1])
assert d["samples"] > 0 and d["failed_reads"] == 0, d
for c in d["checks"]:
    assert c["max_error"] < 1e-5, c
EOF
            PROBED="$out"
            return 0
        fi
        # A late input block (missing samples, faded) or a missed DSP deadline on
        # this shared machine is not a mixing error: measure once more.
        if ((attempt == 3)) || [[ "$(glitches)" == "$before" ]]; then
            fail "probe: $out"
        fi
        log "the input was late, its delay changed or the DSP missed a deadline during the probe; measuring again"
    done
}

mkdir -p "$WORK/mxl" "$WORK/state"
