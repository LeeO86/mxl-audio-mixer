#!/usr/bin/env bash
# The automatable items of the release checklist (docs/release-notes/<version>-checklist.md)
# on a target node, one after the other; writes results.md (to paste into the release
# notes) and every item's log into $OUT.
#
#   tests/hw/release/run.sh [item...]      (default: env block-clock load registry amwa)
#
# Items:
#   env          host, kernel, CPU, the kernel TAI offset, the cgroup CPU limit, the commit
#   block-clock  tests/hw/m0-block-clock (BLOCK_MINUTES, default 10)
#   load         tests/perf/section16.sh for LOAD_MINUTES (default 60); pass: dsp_load_ratio
#                < 0.5 and 0 deadline misses; with nr_throttled of the cgroup before and after
#   registry     the mixer with NMOS_REGISTRY_ADDRESS/PORT (required): registered, listed by
#                the Query API (/readyz 200), a published bus's sender too; restarted: same
#                node id and show; SIGTERM: exit 143, gone from the registry
#   amwa         ci/amwa.sh off (needs docker and NMOS_TESTING_DIR/NMOS_TESTING_PYTHON);
#                skipped without them
#
# Needs: a checkout of the release commit, libmxl 218ddaa (LD_LIBRARY_PATH, default
# /opt/mxl/lib), Rust (it builds the release binaries), python3, curl; root or
# CAP_SYS_NICE for the real-time items (MIXER_RT_PRIORITY, default 49).
set -uo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../../.." && pwd)"
OUT="${OUT:-$PWD/release-tests-$(date -u +%Y%m%dT%H%M%SZ)}"
ITEMS=("$@")
((${#ITEMS[@]})) || ITEMS=(env block-clock load registry amwa)
export LD_LIBRARY_PATH="${LD_LIBRARY_PATH:-/opt/mxl/lib}"
export MIXER_RT_PRIORITY="${MIXER_RT_PRIORITY:-49}"
BIN="$ROOT/target/release"
mkdir -p "$OUT"
RESULTS="$OUT/results.md"
ROWS=()

log() { echo "[release] $*"; }
row() { # <item> <PASS|FAIL|SKIP> <details>
    ROWS+=("| $1 | $2 | $3 |")
    log "$1: $2 — $3"
}

cgroup_stat() { # nr_throttled of this cgroup (v2), or "n/a"
    local cg
    cg=$(sed -n 's/^0:://p' /proc/self/cgroup 2>/dev/null)
    sed -n 's/^nr_throttled //p' "/sys/fs/cgroup${cg}/cpu.stat" 2>/dev/null || echo "n/a"
}

build() {
    [[ -x "$BIN/mxl-audio-mixer" && -x "$BIN/mxl-mixer-tone" ]] && return 0
    log "building the release binaries"
    (cd "$ROOT" && cargo build --release --bins -q) >"$OUT/build.log" 2>&1
}

item_env() {
    local offset cpu limit sha
    offset=$(python3 -c "import time; print(round(time.clock_gettime(time.CLOCK_TAI) - time.clock_gettime(time.CLOCK_REALTIME)))")
    cpu=$(sed -n 's/^model name\s*: //p' /proc/cpuinfo | head -1)
    limit=$(cat "/sys/fs/cgroup$(sed -n 's/^0:://p' /proc/self/cgroup)/cpu.max" 2>/dev/null || echo "n/a")
    sha=$(git -C "$ROOT" rev-parse --short=7 HEAD 2>/dev/null || echo "?")
    {
        echo "- host: $(hostname), kernel $(uname -r), $(nproc) CPUs ($cpu)"
        echo "- cgroup cpu.max: $limit"
        echo "- kernel TAI offset: $offset s"
        echo "- commit: $sha (image git-$sha)"
    } >"$OUT/env.md"
    if [[ "$offset" == 37 ]]; then
        row env PASS "TAI offset 37 s; commit $sha; $(nproc) CPUs"
    else
        row env FAIL "CLOCK_TAI is UTC + $offset s, not + 37 s"
    fi
}

item_block_clock() {
    local minutes="${BLOCK_MINUTES:-10}"
    if OUT="$OUT/block-clock" "$ROOT/tests/hw/m0-block-clock/run.sh" "$minutes" >"$OUT/block-clock.log" 2>&1; then
        row block-clock PASS "$minutes min at SCHED_FIFO $MIXER_RT_PRIORITY: $(grep -m1 -o '"max_us": [0-9.]*' "$OUT/block-clock.log" | head -1 | sed 's/"max_us"/max wake jitter µs/')"
    else
        row block-clock FAIL "see block-clock.log: $(grep -m3 FAIL "$OUT/block-clock.log" | tr '\n' ' ')"
    fi
}

item_load() {
    local minutes="${LOAD_MINUTES:-60}" before after line
    before=$(cgroup_stat)
    SECONDS=$((minutes * 60)) BIN="$BIN" "$ROOT/tests/perf/section16.sh" >"$OUT/load.log" 2>&1
    after=$(cgroup_stat)
    line=$(grep -m1 '^blocks ' "$OUT/load.log")
    if [[ -z "$line" ]]; then
        row load FAIL "no result; see load.log"
        return
    fi
    local throttled="n/a"
    [[ "$before" != n/a && "$after" != n/a ]] && throttled=$((after - before))
    if python3 - "$line" <<'EOF'; then
import re, sys
l = sys.argv[1]
ratio = float(re.search(r"load ratio ([0-9.]+)", l)[1])
misses = int(re.search(r"deadline misses (\d+)", l)[1])
sys.exit(0 if ratio < 0.5 and misses == 0 else 1)
EOF
        row load PASS "$minutes min: $line; nr_throttled +$throttled"
    else
        row load FAIL "$minutes min: $line; nr_throttled +$throttled"
    fi
}

item_registry() {
    if [[ -z "${NMOS_REGISTRY_ADDRESS:-}" || -z "${NMOS_REGISTRY_PORT:-}" ]]; then
        row registry SKIP "NMOS_REGISTRY_ADDRESS and NMOS_REGISTRY_PORT not set"
        return
    fi
    local work web nmos query state pid node rc
    work=$(mktemp -d /dev/shm/release-reg.XXXXXX)
    web="http://127.0.0.1:${REG_WEB_PORT:-8190}"
    nmos="${REG_NMOS_PORT:-3342}"
    query="http://${NMOS_QUERY_ADDRESS:-$NMOS_REGISTRY_ADDRESS}:${NMOS_QUERY_PORT:-$((NMOS_REGISTRY_PORT + 1))}/x-nmos/query/v1.3"
    state="$work/state"
    start() {
        env HOST_ID=release-test NMOS_SEED=release-test WEB_PORT="${web##*:}" NMOS_PORT="$nmos" \
            MXL_DOMAIN_SCAN_PATH="$work/mxl" MIXER_STATE_DIR="$state" MXL_CLEANUP_ON_EXIT=true \
            "$BIN/mxl-audio-mixer" >>"$OUT/registry-mixer.log" 2>&1 &
        pid=$!
        for _ in $(seq 1 300); do
            [[ "$(curl -s -o /dev/null -w '%{http_code}' "$web/readyz")" == 200 ]] && return 0
            sleep 0.1
        done
        return 1
    }
    # The Query API pages its answers (nmos-cpp: 10 by default).
    listed() { curl -fs "$query/$1?paging.limit=1000" | python3 -c "import json,sys; ids=[x['id'] for x in json.load(sys.stdin)]; sys.exit(0 if '$2' in ids else 1)"; }
    fail() {
        row registry FAIL "$1"
        kill -TERM "$pid" 2>/dev/null
        wait "$pid" 2>/dev/null
        rm -rf "$work"
    }
    mkdir -p "$work/mxl" "$state"
    start || { fail "/readyz not 200 within 30 s (see registry-mixer.log)"; return; }
    node=$(curl -fs "$web/api/v1/status" | python3 -c "import json,sys; print(json.load(sys.stdin)['nmos']['node_id'])")
    curl -fs -o /dev/null -X POST -H 'content-type: application/json' "$web/api/v1/buses" -d '{"id": "pgm", "name": "Programme", "format": "stereo"}'
    curl -fs -o /dev/null -X PUT "$web/api/v1/outputs/pgm"
    local sender=""
    for _ in $(seq 1 50); do
        sender=$(curl -fs "$web/api/v1/outputs/pgm" | python3 -c "import json,sys; print(json.load(sys.stdin)['nmos_sender_id'])" 2>/dev/null) && listed senders "$sender" && break
        sleep 0.2
    done
    listed nodes "$node" || { fail "node $node not in the Query API"; return; }
    listed senders "$sender" || { fail "sender $sender of the published bus not in the Query API"; return; }
    local inputs receivers=0
    inputs=$(curl -fs "$web/api/v1/inputs" | python3 -c "import json,sys; print(len(json.load(sys.stdin)))")
    for _ in $(seq 1 50); do
        receivers=$(python3 - "$query" "$node" <<'EOF'
import json, sys, urllib.request
q, node = sys.argv[1], sys.argv[2]
get = lambda p: json.load(urllib.request.urlopen(f"{q}/{p}?paging.limit=1000"))
devices = {d["id"] for d in get("devices") if d["node_id"] == node}
print(sum(1 for r in get("receivers") if r["device_id"] in devices))
EOF
)
        [[ "$receivers" == "$inputs" ]] && break
        sleep 0.2
    done
    [[ "$receivers" == "$inputs" ]] || { fail "$receivers of $inputs receivers in the Query API"; return; }
    kill -TERM "$pid"
    rc=0
    wait "$pid" || rc=$?
    start || { fail "no /readyz 200 after the restart"; return; }
    local again show
    again=$(curl -fs "$web/api/v1/status" | python3 -c "import json,sys; print(json.load(sys.stdin)['nmos']['node_id'])")
    show=$(curl -fs "$web/api/v1/buses/pgm" -o /dev/null -w '%{http_code}')
    [[ "$again" == "$node" && "$show" == 200 ]] || { fail "after the restart: node $again (was $node), bus pgm $show"; return; }
    local t0 t1 rc2=0
    t0=$(date +%s.%N)
    kill -TERM "$pid"
    wait "$pid" || rc2=$?
    t1=$(date +%s.%N)
    local gone=no
    for _ in $(seq 1 50); do
        listed nodes "$node" || { gone=yes; break; }
        sleep 0.2
    done
    rm -rf "$work"
    if [[ "$rc" == 143 && "$rc2" == 143 && "$gone" == yes ]]; then
        row registry PASS "node $node, its $receivers receivers and the published sender listed; restart: same node id and show; SIGTERM: 143 in $(python3 -c "print(f'{$t1 - $t0:.2f}')") s, deregistered"
    else
        row registry FAIL "SIGTERM exit codes $rc, $rc2; deregistered: $gone"
    fi
}

item_amwa() {
    if ! command -v docker >/dev/null || [[ -z "${NMOS_TESTING_DIR:-}" ]]; then
        row amwa SKIP "needs docker and NMOS_TESTING_DIR (an nmos-testing checkout), NMOS_TESTING_PYTHON"
        return
    fi
    if AMWA_OUT="$OUT/amwa" "$ROOT/ci/amwa.sh" off "$BIN/mxl-audio-mixer" >"$OUT/amwa.log" 2>&1; then
        row amwa PASS "$(grep -E '^\| (IS|BCP)' "$OUT/amwa/summary.md" 2>/dev/null | tr '\n' ' ')"
    else
        row amwa FAIL "see amwa.log"
    fi
}

build || { log "the build failed: $OUT/build.log"; exit 1; }
for item in "${ITEMS[@]}"; do
    log "== $item"
    case "$item" in
        env) item_env ;;
        block-clock) item_block_clock ;;
        load) item_load ;;
        registry) item_registry ;;
        amwa) item_amwa ;;
        *) row "$item" FAIL "unknown item" ;;
    esac
done

{
    echo "# Release tests on $(hostname), $(date -u +%Y-%m-%dT%H:%MZ)"
    echo
    [[ -f "$OUT/env.md" ]] && cat "$OUT/env.md" && echo
    echo "| Item | Result | Details |"
    echo "| --- | --- | --- |"
    printf '%s\n' "${ROWS[@]}"
    echo
    echo "The manual items of the checklist are not in this file."
} >"$RESULTS"
log "results: $RESULTS"
! grep -q '| FAIL |' "$RESULTS"
