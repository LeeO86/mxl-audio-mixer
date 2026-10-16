#!/usr/bin/env bash
# Milestone 0 on a target node: MXL audio/float32 write and read on the TAI block
# clock with the DSP thread's real-time settings. See README.md.
#
#   tests/hw/m0-block-clock/run.sh [minutes]      (default 10)
#
# Needs: libmxl 218ddaa (scripts/build-mxl.sh), Rust, CAP_SYS_NICE for SCHED_FIFO
# (run as root or give the binary the capability), an exclusive CPU.
set -euo pipefail

MINUTES="${1:-10}"
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../../.." && pwd)"
OUT="${OUT:-$PWD/m0-block-clock-$(date -u +%Y%m%dT%H%M%SZ)}"
# The mixer's rule: the last CPU whose SMT siblings are all ours (a whole core).
CPU="${SPIKE_CPU:-}"
[[ -n "$CPU" ]] || CPU=$(python3 - <<'EOF'
import os
cpus = sorted(os.sched_getaffinity(0))
def whole(c):
    try:
        text = open(f"/sys/devices/system/cpu/cpu{c}/topology/thread_siblings_list").read().strip()
    except OSError:
        return False
    sib = set()
    for part in text.split(","):
        a, _, b = part.partition("-")
        sib |= set(range(int(a), int(b or a) + 1))
    return sib <= set(cpus)
print(next((c for c in reversed(cpus) if whole(c)), cpus[-1]))
EOF
)
PRIORITY="${MIXER_RT_PRIORITY:-49}"
PORT="${NMOS_PORT:-3399}"
DOMAIN_ROOT="$(mktemp -d /dev/shm/m0-hw.XXXXXX)"
mkdir -p "$OUT"
trap 'rm -rf "$DOMAIN_ROOT"' EXIT

offset=$(python3 -c "import time; print(round(time.clock_gettime(time.CLOCK_TAI) - time.clock_gettime(time.CLOCK_REALTIME)))")
echo "kernel TAI-UTC offset: $offset s"
[[ "$offset" == 37 ]] || { echo "FAIL: CLOCK_TAI is not UTC + 37 s on this host"; exit 1; }

(cd "$ROOT/spikes/m0" && cargo build --release -q)
BIN="$ROOT/spikes/m0/target/release/m0-spike"

NMOS_SEED=m0-hw NMOS_PORT="$PORT" NMOS_HOST_ADDRESS="${NMOS_HOST_ADDRESS:-$(ip -4 route get 1.1.1.1 | sed -n 's/.* src \([0-9.]*\).*/\1/p')}" \
    MXL_DOMAIN_SCAN_PATH="$DOMAIN_ROOT" MXL_LIBRARY="${MXL_LIBRARY:-/opt/mxl/lib/libmxl.so}" \
    MIXER_RT_PRIORITY="$PRIORITY" SPIKE_CPU="$CPU" SPIKE_DURATION_S=$((MINUTES * 60)) \
    SPIKE_REPORT="$OUT/report.json" "$BIN" >"$OUT/spike.log" 2>&1 &
PID=$!
for _ in $(seq 1 100); do curl -fs -o /dev/null "http://127.0.0.1:$PORT/x-nmos/node/v1.3/self" && break; sleep 0.1; done

# Loopback: connect the receiver to the node's own sender through IS-05.
base="http://127.0.0.1:$PORT/x-nmos"
receiver=$(curl -fs "$base/connection/v1.2/single/receivers/" | python3 -c "import json,sys; print(json.load(sys.stdin)[0].rstrip('/'))")
sender=$(curl -fs "$base/node/v1.3/senders" | python3 -c "import json,sys; print(json.load(sys.stdin)[0]['id'])")
leg=$(curl -fs "$base/connection/v1.2/single/senders/$sender/active" | python3 -c "import json,sys; print(json.dumps(json.load(sys.stdin)['transport_params'][0]))")
curl -fs -o /dev/null -X PATCH -H 'content-type: application/json' "$base/connection/v1.2/single/receivers/$receiver/staged" \
    -d "{\"sender_id\": \"$sender\", \"master_enable\": true, \"activation\": {\"mode\": \"activate_immediate\"}, \"transport_params\": [$leg]}"
echo "running $MINUTES min on CPU $CPU at SCHED_FIFO $PRIORITY; results in $OUT"
wait "$PID"

python3 - "$OUT/report.json" <<'EOF'
import json, sys
r = json.load(open(sys.argv[1]))
block_us = r["block_samples"] / 48.0 * 1000
checks = [
    ("SCHED_FIFO in effect", "SCHED_FIFO" in r["scheduling"] and "refused" not in r["scheduling"], r["scheduling"]),
    ("no deadline miss (wake-up later than one block)", r["overruns"] == 0, r["overruns"]),
    ("max wake jitter below one block", r["wake_jitter"]["max_us"] < block_us, f'{r["wake_jitter"]["max_us"]} us < {block_us} us'),
    ("no write errors", r["write_errors"] == 0, r["write_errors"]),
    ("every block read", r["too_early_blocks"] + r["too_late_blocks"] + r["other_read_errors"] == 0,
     f'early {r["too_early_blocks"]}, late {r["too_late_blocks"]}, other {r["other_read_errors"]}'),
    ("every sample correct", r["mismatched_samples"] == 0, r["mismatched_samples"]),
]
print(json.dumps({k: r[k] for k in ("wake_jitter", "block_work", "input_arrival_lag")}, indent=1))
failed = False
for name, ok, value in checks:
    print(f"{'PASS' if ok else 'FAIL'}  {name}: {value}")
    failed |= not ok
sys.exit(1 if failed else 0)
EOF
