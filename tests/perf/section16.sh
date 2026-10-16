#!/usr/bin/env bash
# The §16-shaped load: 64 mono strips (HPF, 4-band EQ, compressor; a de-esser on 16),
# 32 stereo buses, 16 aux buses, 8 N-1 buses (mix-minus of main without one strip
# each: 95 mirrored sends each), 672 sends, 4 VCAs over the strips, 8 published
# outputs, fed by 8 tone flows of 8 channels. Measures the DSP block time over SECONDS (default 60)
# and prints mean, dsp_load_ratio, the slowest histogram bucket and deadline misses.
#
#   cargo build --release --bins && tests/perf/section16.sh
#
# Needs libmxl (LD_LIBRARY_PATH). Ports WEB_PORT (8180) and NMOS_PORT (3332).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BIN="${BIN:-$ROOT/target/release}"
SECONDS_RUN="${SECONDS:-60}"
WORK="$(mktemp -d /dev/shm/mixer-perf.XXXXXX)"
mkdir -p "$WORK/mxl"
export WEB_PORT="${WEB_PORT:-8180}" NMOS_PORT="${NMOS_PORT:-3332}"
WEB="http://127.0.0.1:$WEB_PORT"
NMOS="http://127.0.0.1:$NMOS_PORT"
DOMAIN=12121212-3434-4565-8787-909090909090
PIDS=()
cleanup() {
    for p in "${PIDS[@]}"; do kill -TERM "$p" 2>/dev/null || true; done
    wait 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT INT TERM

# The tone writers off the DSP thread's core (the mixer's rule: the last CPU whose
# SMT siblings are all ours) and, with MIXER_RT_PRIORITY and the right to use it,
# at SCHED_FIFO 4 below the DSP thread, so a late source measures the writers less
# and the mixer more.
TONE_CPUS=$(python3 - <<'EOF'
import os
cpus = sorted(os.sched_getaffinity(0))
def siblings(c):
    try:
        text = open(f"/sys/devices/system/cpu/cpu{c}/topology/thread_siblings_list").read().strip()
    except OSError:
        return None
    out = []
    for part in text.split(","):
        a, _, b = part.partition("-")
        out += range(int(a), int(b or a) + 1)
    return out
if len(cpus) > 1:
    dsp = next((c for c in reversed(cpus) if (s := siblings(c)) is not None and set(s) <= set(cpus)), cpus[-1])
    core = set(siblings(dsp) or []) | {dsp}
    print(",".join(str(c) for c in cpus if c not in core))
EOF
)
TONE=()
[[ -n "$TONE_CPUS" ]] && TONE+=(taskset -c "$TONE_CPUS")
TONE_PRIORITY=""
# Only a priority the mixer takes (an integer 1-49); "off", "false" or 0 leave
# the writers SCHED_OTHER.
if [[ "${MIXER_RT_PRIORITY:-}" =~ ^[0-9]+$ ]] && ((10#$MIXER_RT_PRIORITY >= 1 && 10#$MIXER_RT_PRIORITY <= 49)); then
    TONE_PRIORITY=$((10#$MIXER_RT_PRIORITY > 4 ? 10#$MIXER_RT_PRIORITY - 4 : 1))
    if chrt -f "$TONE_PRIORITY" true 2>/dev/null; then
        TONE+=(chrt -f "$TONE_PRIORITY")
    else
        TONE_PRIORITY=""
    fi
fi
echo "tone writers: CPUs ${TONE_CPUS:-any}, $([[ -n "$TONE_PRIORITY" ]] && echo "SCHED_FIFO $TONE_PRIORITY" || echo SCHED_OTHER)"
for f in 1 2 3 4 5 6 7 8; do
    "${TONE[@]}" "$BIN/mxl-mixer-tone" --domain "$WORK/mxl/tones" --flow "00000000-0000-4000-8000-00000000000$f" \
        --domain-id "$DOMAIN" --channels 8 >/dev/null 2>&1 &
    PIDS+=($!)
done
env HOST_ID=perf MXL_DOMAIN_SCAN_PATH="$WORK/mxl" MIXER_STATE_DIR="$WORK/state" MIXER_INPUT_FLOWS=8 \
    "$BIN/mxl-audio-mixer" >"$WORK/mixer.log" 2>&1 &
PIDS+=($!)
for _ in $(seq 1 100); do curl -fsS -o /dev/null "$WEB/livez" 2>/dev/null && break; sleep 0.1; done
sleep 1
for f in 1 2 3 4 5 6 7 8; do
    receiver=$(curl -fsS "$NMOS/x-nmos/node/v1.3/receivers" |
        python3 -c "import json,sys; print([r['id'] for r in json.load(sys.stdin) if r['label'] == 'Input $f'][0])")
    curl -fsS -o /dev/null -X PATCH -H 'content-type: application/json' \
        "$NMOS/x-nmos/connection/v1.2/single/receivers/$receiver/staged" \
        -d "{\"master_enable\": true, \"activation\": {\"mode\": \"activate_immediate\"}, \"transport_params\": [{\"mxl_domain_id\": \"$DOMAIN\", \"mxl_flow_id\": \"00000000-0000-4000-8000-00000000000$f\"}]}"
done

python3 - >"$WORK/show.json" <<'EOF'
import json
strips, buses, sends, outputs = [], [], [], []
def send(a, b):
    sends.append({"from": a, "to": b, "tap": "post-pan" if a.startswith("s") else "post-fader",
                  "gain_db": 0, "mute": False, "phase": False})
for i in range(64):
    chain = [
        {"id": "hpf", "type": "filter", "mode": "high-pass", "freq_hz": 80, "slope_db_per_oct": 24},
        {"id": "eq", "type": "eq", "bands": [
            {"type": "low-shelf", "freq_hz": 120, "q": 0.7, "gain_db": -2},
            {"type": "bell", "freq_hz": 500, "q": 1, "gain_db": -1},
            {"type": "bell", "freq_hz": 3000, "q": 1.4, "gain_db": 2},
            {"type": "high-shelf", "freq_hz": 10000, "q": 0.7, "gain_db": 1}]},
        {"id": "comp", "type": "compressor", "threshold_db": -26, "ratio": 3, "knee_db": 6,
         "attack_ms": 5, "release_ms": 150, "makeup_db": 2, "detector": "rms"},
    ]
    if i < 16:
        chain.append({"id": "deess", "type": "de-esser", "mode": "split-band", "freq_hz": 6000,
                      "threshold_db": -30, "range_db": 6, "attack_ms": 1, "release_ms": 80, "listen": False})
    strips.append({"id": f"s{i}", "name": f"S{i}", "format": "mono",
                   "patch": [{"input": i // 8 + 1, "channel": i % 8 + 1}], "trim_db": 0,
                   "polarity": [False], "fader_db": -6, "mute": False, "pan": 0, "width": 1, "chain": chain})
bus = lambda id, fmt: {"id": id, "name": id, "format": fmt, "fader_db": 0, "mute": False}
buses.append(bus("main", "stereo"))
groups = [f"g{k}" for k in range(31)]
auxes = [f"a{k}" for k in range(16)]
n1 = [f"n{k}" for k in range(8)]
buses += [bus(g, "stereo") for g in groups] + [bus(a, "mono") for a in auxes]
buses += [{**bus(n, "mono"), "minus": {"of": "main", "without": [f"s{k}"]}} for k, n in enumerate(n1)]
for i in range(64):
    send(f"s{i}", "main")
    send(f"s{i}", groups[i % 31])
    for k in range(8):
        send(f"s{i}", auxes[(i + k) % 16])
for g in groups:
    send(g, "main")
send("a0", "main")
assert len(sends) == 672, len(sends)
vcas = [{"id": f"v{k}", "name": f"V{k}", "fader_db": 0, "members": [f"s{i}" for i in range(k * 16, k * 16 + 16)]}
        for k in range(4)]
outputs = [{"signal": s} for s in ["main", "g0", "g1", "a0", "a1", "n0", "n1", "n2"]]
print(json.dumps({"version": 1, "mix": {"strips": strips, "buses": buses, "sends": sends, "outputs": outputs, "vcas": vcas}}))
EOF
curl -fsS -o /dev/null -X POST -H 'content-type: application/json' "$WEB/api/v1/config/import" --data-binary @"$WORK/show.json"
sleep 3

metrics() { curl -fsS "$WEB/metrics"; }
value() { sed -n "s/^mxl_audio_mixer_$1 \(.*\)/\1/p"; }
before=$(metrics)
sleep "$SECONDS_RUN"
after=$(metrics)
echo "dsp thread: $(curl -fsS "$WEB/api/v1/status" | python3 -c "import json, sys; print(json.load(sys.stdin)['dsp']['scheduling'])")"
python3 - "$before" "$after" "$SECONDS_RUN" <<'EOF'
import re, sys
def parse(t):
    out = {}
    for line in t.splitlines():
        m = re.match(r'^mxl_audio_mixer_(\w+)(\{[^}]*\})? (\S+)$', line)
        if m:
            out[m[1] + (m[2] or "")] = float(m[3])
    return out
b, a, secs = parse(sys.argv[1]), parse(sys.argv[2]), int(sys.argv[3])
d = lambda k: a.get(k, 0) - b.get(k, 0)
n = d("dsp_block_seconds_count")
mean = d("dsp_block_seconds_sum") / n
buckets = sorted((float(re.search(r'le="([^"]+)"', k)[1]), d(k)) for k in a
                 if k.startswith("dsp_block_seconds_bucket") and "+Inf" not in k)
slowest = next((le for le, c in buckets if c >= n), float("inf"))
steps = {k: v for k, v in a.items() if k.startswith("nodes{")}
def samples(name):
    return int(sum(d(k) for k in a if k == name or k.startswith(name + "{")))
missing, late, overrun = samples("input_missing_samples_total"), samples("input_late_samples_total"), samples("input_overrun_samples_total")
print(f"blocks {int(n)} in {secs} s; mean block {mean * 1e6:.0f} µs; load ratio {mean / 0.002:.3f}; "
      f"all blocks ≤ {slowest * 1e3:g} ms; deadline misses {int(d('dsp_deadline_misses_total'))}; "
      f"missing input samples {missing} (late source {late}, ring overrun {overrun}, other {missing - late - overrun}); "
      f"meter drops {int(d('meter_drops_total'))}")
print("plan steps:", ", ".join(f"{re.search(r'type=\"([^\"]+)\"', k)[1]} {int(v)}" for k, v in sorted(steps.items())))
EOF
