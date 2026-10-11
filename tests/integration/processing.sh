#!/usr/bin/env bash
# Milestone 3 (§6, §7, §17) end to end through MXL: a processing chain on a strip
# (EQ sample-exact, a ramped parameter change without a plan swap, bypass, a
# 48 dB/oct high-pass, a compressor's gain reduction), a look-ahead limiter on a
# published bus (ceiling, latency), loudness (BS.1770) with start/stop/reset and
# its metrics, true peak.
# shellcheck source=tests/integration/lib.sh
source "$(dirname "$0")/lib.sh"

FLOW=88888888-8888-4888-8888-888888888888
DOMAIN=99999999-9999-4999-8999-999999999999
start_tone "$WORK/mxl/tones" "$FLOW" --domain-id "$DOMAIN" --channels 2
wait_until 10 "tone flow" test -d "$WORK/mxl/tones/$FLOW.mxl-flow"
start_mixer NMOS_SEED=it-processing
route 1 "$DOMAIN" "$FLOW"
wait_until 5 "input 1 running" is_state 1 running

# vox (tone 1, −20 dBFS) panned hard left into a stereo bus: L = vox, R = silence.
api POST /api/v1/strips '{"id": "vox", "format": "mono", "pan": -1, "patch": [{"input": 1, "channel": 1}]}' >/dev/null
api POST /api/v1/buses '{"id": "main", "name": "Main", "format": "stereo"}' >/dev/null
api PUT /api/v1/sends/vox/main >/dev/null
api PUT /api/v1/outputs/main >/dev/null
OUT_FLOW=$(json "$WEB/api/v1/outputs/main" "d['mxl_flow_id']")
OWN=$(json "$WEB/api/v1/status" "d['mxl']['output_domain_dir']")
probe 1 1:1:1 2:1:0

swaps() { curl -fsS "$WEB/metrics" | sed -n 's/^mxl_audio_mixer_plan_swaps_total \(.*\)/\1/p'; }
peak() { # <channel> : peak dBFS of channel <channel> in the last probe
    python3 -c "import json,sys; print(json.loads(sys.argv[1])['levels'][$1 - 1]['peak_dbfs'])" "$PROBED"
}

# EQ: a +6.02 dB bell at 1 kHz doubles the 1 kHz tone (zero phase at the centre).
id=$(api POST /api/v1/strips/vox/chain '{"type": "eq", "bands": [{"type": "bell", "freq_hz": 1000, "q": 1, "gain_db": 6.0206}]}' |
    python3 -c "import json,sys; print(json.load(sys.stdin)['id'])")
[[ "$id" == eq-1 ]] || fail "node id $id"
sleep 0.2
probe 1 1:1:2 2:1:0
pass "EQ bell +6.02 dB at 1 kHz: L = 2 × tone (sample-exact)"

before=$(swaps)
api PATCH /api/v1/strips/vox/chain/eq-1 '{"bands": [{"type": "bell", "freq_hz": 1000, "q": 1, "gain_db": 0}], "ramp_ms": 50}' >/dev/null
sleep 0.3
probe 1 1:1:1 2:1:0
[[ "$(swaps)" == "$before" ]] || fail "a parameter change swapped the plan"
pass "EQ gain ramped to 0 dB without a plan swap"

api PATCH /api/v1/strips/vox/chain/eq-1 '{"bands": [{"type": "bell", "freq_hz": 1000, "q": 1, "gain_db": 12}]}' >/dev/null
api PATCH /api/v1/strips/vox/chain/eq-1 '{"bypass": true}' >/dev/null
sleep 0.3
probe 1 1:1:1 2:1:0
pass "bypass: the input passes unchanged"

api POST /api/v1/strips/vox/chain '{"id": "hp", "type": "filter", "mode": "high-pass", "freq_hz": 4000, "slope_db_per_oct": 48}' >/dev/null
sleep 0.3
probe 1
python3 -c "exit(0 if $(peak 1) < -100 else 1)" || fail "high-pass: tone at $(peak 1) dBFS"
pass "48 dB/oct high-pass at 4 kHz: the 1 kHz tone at $(peak 1) dBFS"
curl -fsS -X DELETE "$WEB/api/v1/strips/vox/chain/hp"

api POST /api/v1/strips/vox/chain '{"id": "comp", "type": "compressor", "threshold_db": -30, "ratio": 4, "knee_db": 0, "attack_ms": 1, "release_ms": 500, "makeup_db": 0, "detector": "peak"}' >/dev/null
sleep 0.5
gr=$(json "$WEB/api/v1/meters" "d['vox']['nodes']['comp']['gain_reduction_db']")
python3 -c "exit(0 if abs($gr - 7.5) < 0.5 else 1)" || fail "compressor gain reduction $gr dB, not 7.5"
probe 1
python3 -c "exit(0 if abs($(peak 1) + 27.5) < 0.3 else 1)" || fail "compressed peak $(peak 1)"
pass "compressor −30 dB 4:1: gain reduction $gr dB, peak $(peak 1) dBFS"
curl -fsS -X DELETE "$WEB/api/v1/strips/vox/chain/comp"
code=$(curl -s -o /dev/null -w '%{http_code}' -X PUT -H 'content-type: application/json' "$WEB/api/v1/sends/vox/main" -d '{"tap": {"after": "nope"}}')
[[ "$code" == 400 ]] || fail "a tap after a missing node answered $code"

# Limiter on the bus: ceiling −26 dBFS, 1.5 ms look-ahead.
api POST /api/v1/buses/main/chain '{"id": "lim", "type": "limiter", "ceiling_db": -26, "release_ms": 50, "lookahead_ms": 1.5, "true_peak": false}' >/dev/null
sleep 0.5
probe 1
python3 -c "exit(0 if $(peak 1) <= -25.99 and $(peak 1) > -26.5 else 1)" || fail "limited peak $(peak 1)"
latency=$(json "$WEB/api/v1/outputs/main" "d['latency_ms']")
python3 -c "exit(0 if abs($latency - 5.5) < 1e-6 else 1)" || fail "output latency $latency ms, not 5.5"
pass "limiter: peak $(peak 1) dBFS under the −26 dBFS ceiling; output latency $latency ms (2 + 2 + 1.5)"
curl -fsS -X DELETE "$WEB/api/v1/buses/main/chain/lim"

# Loudness on the bus: L = 1 kHz at −20 dBFS, R silent: −23.0 LUFS.
api PATCH /api/v1/buses/main '{"loudness": true}' >/dev/null
sleep 4
loud() { json "$WEB/api/v1/loudness" "d['main']['$1']"; }
for w in momentary_lufs short_term_lufs integrated_lufs; do
    v=$(loud $w)
    python3 -c "exit(0 if abs($v + 23.0) < 0.2 else 1)" || fail "$w $v, not −23.0"
done
tp=$(loud max_true_peak_dbtp)
python3 -c "exit(0 if abs($tp + 20.0) < 0.3 else 1)" || fail "max true peak $tp"
curl -fsS "$WEB/metrics" | grep -q 'mxl_audio_mixer_loudness_lufs{signal="main",window="integrated"}' || fail "loudness metric"
curl -fsS "$WEB/metrics" | grep -q 'mxl_audio_mixer_true_peak_dbtp{signal="main"}' || fail "true peak metric"
pass "loudness: momentary, short-term, integrated −23.0 LUFS, max true peak $tp dBTP"
curl -fsS -o /dev/null -X POST "$WEB/api/v1/loudness/main/stop"
curl -fsS -o /dev/null -X POST "$WEB/api/v1/loudness/main/reset"
[[ "$(loud integrated_lufs)" == -inf && "$(loud running)" == False ]] || fail "stop and reset"
curl -fsS -o /dev/null -X POST "$WEB/api/v1/loudness/main/start"
sleep 1
python3 -c "exit(0 if abs($(loud integrated_lufs) + 23.0) < 0.2 else 1)" || fail "integration after start"
code=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$WEB/api/v1/loudness/main/explode")
[[ "$code" == 400 ]] || fail "unknown action answered $code"
pass "loudness stop, reset and start"

rc=0
stop_mixer || rc=$?
[[ $rc == 143 ]] || fail "exit code $rc"
pass processing
