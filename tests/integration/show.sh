#!/usr/bin/env bash
# Milestone 4 backend (§7, §12) end to end through MXL: the embedded UI page,
# snapshots (recall filters, recall-safe, crossfade), layouts, the show history
# (named version, restore), PPM and correlation, the analyser (spectrum of the
# 1 kHz tone) and the events WebSocket (status, mix on change, filtered meters at
# 25 Hz, analysis).
# shellcheck source=tests/integration/lib.sh
source "$(dirname "$0")/lib.sh"

FLOW=88888888-8888-4888-8888-888888888888
DOMAIN=99999999-9999-4999-8999-999999999999
start_tone "$WORK/mxl/tones" "$FLOW" --domain-id "$DOMAIN" --channels 2
wait_until 10 "tone flow" test -d "$WORK/mxl/tones/$FLOW.mxl-flow"
start_mixer NMOS_SEED=it-show
route 1 "$DOMAIN" "$FLOW"
wait_until 5 "input 1 running" is_state 1 running

# vox (tone 1: 1 kHz, −20 dBFS) panned hard left into a stereo bus.
api POST /api/v1/strips '{"id": "vox", "format": "mono", "pan": -1, "patch": [{"input": 1, "channel": 1}]}' >/dev/null
api POST /api/v1/buses '{"id": "main", "name": "Main", "format": "stereo"}' >/dev/null
api PUT /api/v1/sends/vox/main >/dev/null
api PUT /api/v1/outputs/main >/dev/null
OUT_FLOW=$(json "$WEB/api/v1/outputs/main" "d['mxl_flow_id']")
OWN=$(json "$WEB/api/v1/status" "d['mxl']['output_domain_dir']")
probe 1 1:1:1 2:1:0

ctype=$(curl -fsS -o "$WORK/ui.html" -w '%{content_type}' "$WEB/")
[[ "$ctype" == text/html* ]] || fail "/ answered $ctype"
grep -q "mxl-audio-mixer" "$WORK/ui.html" || fail "/ is not the mixer's page"
pass "the web UI page is served at /"

field() { json "$WEB/api/v1/$1" "d$2"; }

# Snapshots.
id=$(api POST /api/v1/snapshots '{"name": "Opening"}' | python3 -c "import json,sys; print(json.load(sys.stdin)['id'])")
[[ "$id" == snapshot-1 ]] || fail "snapshot id $id"
api PATCH /api/v1/strips/vox '{"fader_db": -10, "colour": "#ff8800"}' >/dev/null
api PATCH /api/v1/buses/main '{"mute": true}' >/dev/null
api POST /api/v1/snapshots/snapshot-1/recall '{"classes": ["fader"], "sections": ["strips"]}' >/dev/null
[[ "$(field strips/vox "['fader_db']")" == 0.0 && "$(field buses/main "['mute']")" == True ]] ||
    fail "a filtered recall: vox $(field strips/vox "['fader_db']"), main mute $(field buses/main "['mute']")"
[[ "$(field strips/vox "['colour']")" == "#ff8800" ]] || fail "colour"
pass "recall of strip faders only: vox back to 0 dB, the bus mute kept"
api PATCH /api/v1/strips/vox '{"fader_db": -10, "recall_safe": true}' >/dev/null
api POST /api/v1/snapshots/snapshot-1/recall '{"crossfade_ms": 400}' >/dev/null
[[ "$(field strips/vox "['fader_db']")" == -10.0 && "$(field buses/main "['mute']")" == False ]] ||
    fail "recall-safe: vox $(field strips/vox "['fader_db']"), main mute $(field buses/main "['mute']")"
api PATCH /api/v1/strips/vox '{"recall_safe": false}' >/dev/null
api POST /api/v1/snapshots/snapshot-1/recall '{"crossfade_ms": 400}' >/dev/null
sleep 0.6
probe 1 1:1:1 2:1:0
pass "recall-safe vox kept at −10 dB; a full recall with a 400 ms crossfade restores the mix (sample-exact)"
api PATCH /api/v1/snapshots/snapshot-1 '{"name": "Top of show"}' >/dev/null
[[ "$(field snapshots "[0]['name']")" == "Top of show" ]] || fail "rename"
curl -fsS -o /dev/null -X DELETE "$WEB/api/v1/snapshots/snapshot-1"
[[ "$(field snapshots "")" == "[]" ]] || fail "delete"
pass "snapshot rename and delete"

# Layouts.
api PUT /api/v1/layouts '[{"name": "Main", "pages": [{"name": "Talent", "signals": ["vox", "main"]}]}]' >/dev/null
[[ "$(field layouts "[0]['pages'][0]['signals']")" == "['vox', 'main']" ]] || fail "layouts"
code=$(curl -s -o /dev/null -w '%{http_code}' -X PUT -H 'content-type: application/json' "$WEB/api/v1/layouts" \
    -d '[{"name": "Main", "pages": [{"name": "P", "signals": ["nope"]}]}]')
[[ "$code" == 400 ]] || fail "a layout with a missing signal answered $code"
pass "layouts: stored, a missing signal refused"

# Show history: a named version, then a restore after a change.
[[ "$(field shows/history "[-1]['name']")" == None ]] || fail "no automatic version"
version=$(api POST /api/v1/shows/history '{"name": "before rehearsal"}' | python3 -c "import json,sys; print(json.load(sys.stdin)['id'])")
curl -fsS -o /dev/null -X DELETE "$WEB/api/v1/sends/vox/main"
probe 1 1:1:0 2:1:0
api POST "/api/v1/shows/history/$version/restore" >/dev/null
sleep 0.3
probe 1 1:1:1 2:1:0
[[ "$(field shows/history "[1]['name']")" == "before rehearsal" ]] || fail "history order"
pass "show history: named version $version restored the deleted send (sample-exact)"

# PPM and correlation (vox centred: L = R).
sleep 0.5
for t in ppm_type1_dbfs ppm_type2_dbfs; do
    v=$(field meters "['main']['$t'][0]")
    python3 -c "exit(0 if abs($v + 20.0) < 0.3 else 1)" || fail "$t $v, not −20 dBFS"
done
[[ "$(field meters "['main'].get('correlation')")" == None ]] || fail "correlation with a silent channel"
api PATCH /api/v1/strips/vox '{"pan": 0}' >/dev/null
# The 300 ms window can still hold the pan ramp when a meter tick ran late.
steady() { r=$(field meters "['main']['correlation']") && python3 -c "exit(0 if abs($r - 1.0) < 1e-3 else 1)"; }
wait_until 3 "correlation 1.0 with L = R" steady
r=$(field meters "['main']['correlation']")
pass "PPM Type I and II read −20 dBFS on the steady tone; correlation $r with L = R"

# Analyser.
before=$(curl -fsS "$WEB/metrics" | sed -n 's/^mxl_audio_mixer_dsp_deadline_misses_total \(.*\)/\1/p')
api PUT /api/v1/analysis '{"signal": "vox", "tap": "input"}' >/dev/null
sleep 0.5
read -r band level corr < <(curl -fsS "$WEB/api/v1/analysis" | python3 -c "
import json, sys
v = json.load(sys.stdin)['values']
k = max(range(len(v['spectrum_db'])), key=lambda i: v['spectrum_db'][i])
print(v['bands_hz'][k], v['spectrum_db'][k], v['correlation'])")
python3 -c "exit(0 if $band == 1000.0 and abs($level + 20.0) < 1.0 and abs($corr - 1) < 1e-6 else 1)" ||
    fail "spectrum peak $level dB at $band Hz, correlation $corr"
pass "analyser on vox input: peak band $band Hz at $level dBFS, mono correlation $corr"

# Events WebSocket.
out=$(node "$(dirname "$0")/ws_events.mjs" "$WEB" vox 2 -3)
echo "$out" >>"$IT_OUT/show-ws.log"
python3 - "$out" <<'EOF' || fail "events: $out"
import json, sys
d = json.loads(sys.argv[1])
assert d["status"] >= 8 and d["version"], d
assert 35 <= d["meters"] <= 60, d["meters"]
assert d["meterSignals"] == ["vox"], d["meterSignals"]
assert d["analysis"] >= 35 and d["analysisSignal"] == "vox", d
assert len(d["mixes"]) >= 2 and d["mixes"][-1]["fader_db"] == -3.0, d["mixes"]
assert d["mixes"][-1]["revision"] > d["mixes"][0]["revision"], d["mixes"]
EOF
pass "events: status 5/s, meters of vox only at 25 Hz, analysis, the mix again after a REST change"
curl -fsS -o /dev/null -X DELETE "$WEB/api/v1/analysis"
[[ "$(field analysis "['target']")" == None ]] || fail "analysis stop"

rc=0
stop_mixer || rc=$?
[[ $rc == 143 ]] || fail "exit code $rc"
pass show
