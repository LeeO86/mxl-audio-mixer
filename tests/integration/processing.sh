#!/usr/bin/env bash
# Milestone 3 (§6, §7, §17) end to end through MXL: a processing chain on a strip
# (EQ sample-exact, a ramped parameter change without a plan swap, bypass, a
# 48 dB/oct high-pass, a compressor's gain reduction), a look-ahead limiter on a
# published bus (ceiling, latency), loudness (BS.1770) with start/stop/reset and
# its metrics, true peak; 1.4.0: M/S / width, EQ pass bands, a format change,
# the down-mix and the surround panner.
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
# probe_peak <python condition on p (channel 1's peak)>: probe until it holds; a
# late input block resets a dynamics node's envelope (a transient gets through),
# so measure again when one came during the probe, at most three times.
probe_peak() {
    local before p
    for _ in 1 2 3; do
        before=$(glitches)
        probe 1
        p=$(peak 1)
        python3 -c "p = $p; exit(0 if ($1) else 1)" && return 0
        [[ "$(glitches)" == "$before" ]] && return 1
        log "the input was late during the peak measurement; measuring again"
    done
    return 1
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
probe_peak "abs(p + 27.5) < 0.3" || fail "compressed peak $(peak 1)"
pass "compressor −30 dB 4:1: gain reduction $gr dB, peak $(peak 1) dBFS"
curl -fsS -X DELETE "$WEB/api/v1/strips/vox/chain/comp"
code=$(curl -s -o /dev/null -w '%{http_code}' -X PUT -H 'content-type: application/json' "$WEB/api/v1/sends/vox/main" -d '{"tap": {"after": "nope"}}')
[[ "$code" == 400 ]] || fail "a tap after a missing node answered $code"

# Limiter on the bus: ceiling −26 dBFS, 1.5 ms look-ahead.
api POST /api/v1/buses/main/chain '{"id": "lim", "type": "limiter", "ceiling_db": -26, "release_ms": 50, "lookahead_ms": 1.5, "true_peak": false}' >/dev/null
sleep 0.5
probe_peak "-26.5 < p <= -25.99" || fail "limited peak $(peak 1)"
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
# The first gating block (400 ms) needs a moment after the start: poll, up to 5 s.
integrated_ok() { python3 -c "exit(0 if abs(float('$(loud integrated_lufs)') + 23.0) < 0.2 else 1)"; }
for _ in $(seq 1 50); do integrated_ok && break; sleep 0.1; done
integrated_ok || fail "integration after start: $(loud integrated_lufs) LUFS, not −23.0"
code=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$WEB/api/v1/loudness/main/explode")
[[ "$code" == 400 ]] || fail "unknown action answered $code"
pass "loudness stop, reset and start"

# 1.4.0 (D1–D5, C10): the M/S / width node, high- and low-pass EQ bands, a format
# change, the down-mix (global and per bus) and the surround panner.
publish() { # <bus>: publish it and probe it from now on
    api PUT "/api/v1/outputs/$1" >/dev/null
    OUT_FLOW=$(json "$WEB/api/v1/outputs/$1" "d['mxl_flow_id']")
}
api POST /api/v1/strips '{"id": "st", "format": "stereo", "patch": [{"input": 1, "channel": 1}, null]}' >/dev/null
api POST /api/v1/buses '{"id": "w", "format": "stereo"}' >/dev/null
api PUT /api/v1/sends/st/w '{"tap": "post-fader"}' >/dev/null
publish w
api POST /api/v1/strips/st/chain '{"id": "width", "type": "width", "width_pct": 0}' >/dev/null
sleep 0.3
probe 1 1:1:0.5 2:1:0.5
before=$(swaps)
api PATCH /api/v1/strips/st/chain/width '{"width_pct": 200, "ramp_ms": 20}' >/dev/null
sleep 0.3
probe 1 1:1:1.5 2:1:-0.5
[[ "$(swaps)" == "$before" ]] || fail "a width change swapped the plan"
api PATCH /api/v1/strips/st/chain/width '{"width_pct": 100, "input": "ms"}' >/dev/null
sleep 0.3
probe 1 1:1:1 2:1:1
pass "M/S / width: 0 % mono, 200 % double side (ramped, no swap), M/S decode"

api PATCH /api/v1/strips/st/chain/width '{"width_pct": 100, "input": "lr"}' >/dev/null
api POST /api/v1/strips/st/chain '{"id": "eq", "type": "eq", "bands": [{"type": "high-pass", "freq_hz": 4000, "q": 0.7, "slope_db_per_oct": 48}, {"type": "bell", "freq_hz": 1000, "q": 1, "gain_db": 0}]}' >/dev/null
sleep 0.3
probe 1
python3 -c "exit(0 if $(peak 1) < -100 else 1)" || fail "EQ high-pass band: tone at $(peak 1) dBFS"
code=$(curl -s -o /dev/null -w '%{http_code}' -X PATCH -H 'content-type: application/json' "$WEB/api/v1/strips/st/chain/eq" \
    -d '{"bands": [{"type": "low-pass", "freq_hz": 4000, "q": 0.7}]}')
[[ "$code" == 400 ]] || fail "a low-pass band without a slope answered $code"
api PATCH /api/v1/strips/st/chain/eq '{"bands": [{"type": "low-pass", "freq_hz": 8000, "q": 0.7, "slope_db_per_oct": 12}]}' >/dev/null
sleep 0.3
probe 1
python3 -c "exit(0 if abs($(peak 1) + 20.0) < 0.2 else 1)" || fail "EQ low-pass band at 8 kHz: tone at $(peak 1) dBFS"
curl -fsS -X DELETE "$WEB/api/v1/strips/st/chain/eq"
pass "EQ high-pass band 48 dB/oct at 4 kHz: tone at -100 dBFS or less; low-pass band at 8 kHz passes 1 kHz"

answer=$(api PATCH /api/v1/strips/st '{"format": "5.1"}')
python3 - "$answer" <<'EOF2' || fail "format change: $answer"
import json, sys
s = json.loads(sys.argv[1])
assert s["format"] == "5.1" and len(s["patch"]) == 6 and len(s["polarity"]) == 6, s
assert s["patch"][0] == {"input": 1, "channel": 1} and s["patch"][2] is None, s["patch"]
width = [n for n in s["chain"] if n["id"] == "width"][0]
assert width["bypass"], width
assert any("width" in w for w in s["warnings"]), s.get("warnings")
assert [w["node"] for w in s["chain_warnings"]] == ["width"], s["chain_warnings"]
EOF2
[[ "$(json "$WEB/api/v1/sends/st/w" "d['tap']")" == post-fader ]] || fail "the send went away"
sleep 0.3
probe 1 1:1:1 2:1:0
pass "format change stereo → 5.1: chain kept (width bypassed and flagged), send kept and down-mixed, patch extended"

# A 5.1 strip with the tone on C into a stereo bus: the global down-mix (−3 dB),
# the bus's own (−6 dB), a global change the bus does not follow, then the global.
api POST /api/v1/strips '{"id": "film", "format": "5.1", "patch": [null, null, {"input": 1, "channel": 1}, null, null, null]}' >/dev/null
api POST /api/v1/buses '{"id": "dm", "format": "stereo"}' >/dev/null
api PUT /api/v1/sends/film/dm '{"tap": "post-fader"}' >/dev/null
publish dm
sleep 0.3
probe 1 1:1:0.7071068 2:1:0.7071068
api PATCH /api/v1/buses/dm '{"downmix": {"centre_db": -6}}' >/dev/null
sleep 0.3
probe 1 1:1:0.5011872 2:1:0.5011872
api PATCH /api/v1/mix/settings '{"downmix": {"centre_db": 0}}' >/dev/null
sleep 0.3
probe 1 1:1:0.5011872
api PATCH /api/v1/buses/dm '{"downmix": null}' >/dev/null
sleep 0.3
probe 1 1:1:1 2:1:1
code=$(curl -s -o /dev/null -w '%{http_code}' -X PATCH -H 'content-type: application/json' "$WEB/api/v1/mix/settings" -d '{"downmix": {"centre_db": 3}}')
[[ "$code" == 400 ]] || fail "a +3 dB centre answered $code"
pass "down-mix: BS.775 default, per-bus override, global change, back to the global"

# A mono strip with a surround panner into a 5.1 bus: Rs at 110°, L at −30° (a
# value change, no plan swap), the LFE send.
api POST /api/v1/strips '{"id": "fx", "format": "mono", "patch": [{"input": 1, "channel": 1}]}' >/dev/null
api POST /api/v1/strips/fx/chain '{"id": "pan", "type": "surround-pan", "layout": "5.1", "azimuth_deg": 110}' >/dev/null
api POST /api/v1/buses '{"id": "sur", "format": "5.1"}' >/dev/null
api PUT /api/v1/sends/fx/sur >/dev/null
publish sur
sleep 0.3
probe 1 1:1:0 2:1:0 3:1:0 4:1:0 5:1:0 6:1:1
before=$(swaps)
api PATCH /api/v1/strips/fx/chain/pan '{"azimuth_deg": -30, "lfe_db": 0, "ramp_ms": 20}' >/dev/null
sleep 0.3
probe 1 1:1:1 2:1:0 3:1:0 4:1:1 6:1:0
[[ "$(swaps)" == "$before" ]] || fail "moving the panner swapped the plan"
pass "surround panner: mono → 5.1 Rs, then L with the LFE send (no plan swap)"

rc=0
stop_mixer || rc=$?
[[ $rc == 143 ]] || fail "exit code $rc"
pass processing
