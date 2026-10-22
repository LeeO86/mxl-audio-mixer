#!/usr/bin/env bash
# Milestone 2 (§4.2, §4.3, §5, §17): input patch, strips, a bus and sends; a
# published bus is an MXL flow with the expected samples (pan law, gain, polarity,
# width mapping) and an NMOS sender; a strip published as its direct out; the
# taps of a strip and a bus; parameter changes and rewiring; meters;
# a restart keeps the output flow id and a downstream reader (a second mixer)
# receives again; unpublishing removes the flow and the sender.
# shellcheck source=tests/integration/lib.sh
source "$(dirname "$0")/lib.sh"

FLOW=66666666-6666-4666-8666-666666666666
DOMAIN=77777777-7777-4777-8777-777777777777
start_tone "$WORK/mxl/tones" "$FLOW" --domain-id "$DOMAIN" --channels 2
wait_until 10 "tone flow" test -d "$WORK/mxl/tones/$FLOW.mxl-flow"
start_mixer NMOS_SEED=it-mix
route 1 "$DOMAIN" "$FLOW"
wait_until 5 "input 1 running" is_state 1 running

api POST /api/v1/strips '{"id": "vox", "name": "Vox", "format": "mono", "patch": [{"input": 1, "channel": 1}]}' >/dev/null
api POST /api/v1/strips '{"id": "music", "format": "stereo", "patch": [{"input": 1, "channel": 1}, {"input": 1, "channel": 2}]}' >/dev/null
api POST /api/v1/buses '{"id": "main", "name": "Main", "format": "stereo"}' >/dev/null
api PUT /api/v1/sends/vox/main >/dev/null
api PUT /api/v1/outputs/main >/dev/null
OUT_FLOW=$(json "$WEB/api/v1/outputs/main" "d['mxl_flow_id']")
OWN=$(json "$WEB/api/v1/status" "d['mxl']['output_domain_dir']")
[[ -d "$OWN/$OUT_FLOW.mxl-flow" ]] || fail "no MXL flow for the published bus"
[[ "$(json "$WEB/api/v1/strips/vox" "d['patch_state'][0]")" == ok ]] || fail "vox patch state"
pass "strips, bus, send and a published output created"

# A mono strip centred into a stereo bus: −3 dB on each side.
probe 1 1:1:0.70710677 2:1:0.70710677
pass "mono strip at the centre: L = R = tone 1 × −3 dB (sample-exact)"

# Plan swaps do not disturb unrelated paths: the output stays sample-exact while
# other buses and sends come and go.
probe 3 1:1:0.70710677 2:1:0.70710677 &
probing=$!
sleep 0.5
swaps=$(curl -fsS "$WEB/metrics" | sed -n 's/^mxl_audio_mixer_plan_swaps_total \(.*\)/\1/p')
for i in 1 2 3 4 5; do
    api POST /api/v1/buses "{\"id\": \"aux-$i\", \"format\": \"mono\"}" >/dev/null
    api PUT "/api/v1/sends/vox/aux-$i" '{"tap": "pre-fader"}' >/dev/null
    curl -fsS -X DELETE "$WEB/api/v1/buses/aux-$i"
done
wait "$probing" || fail "the output changed during plan swaps"
after=$(curl -fsS "$WEB/metrics" | sed -n 's/^mxl_audio_mixer_plan_swaps_total \(.*\)/\1/p')
((after - swaps >= 15)) || fail "expected 15 plan swaps, got $((after - swaps))"
pass "$((after - swaps)) plan swaps while reading: the output stayed sample-exact"

# Parameter change (mute vox) and rewiring (a new send, polarity inverted, −6 dB).
api PATCH /api/v1/strips/vox '{"mute": true}' >/dev/null
api PUT /api/v1/sends/music/main '{"gain_db": -6.0206, "phase": true}' >/dev/null
sleep 0.2
probe 1 1:1:-0.5 2:2:-0.5
pass "mute and a new send with phase: L = −0.5 × tone 1, R = −0.5 × tone 2"

api PATCH /api/v1/strips/vox '{"mute": false, "pan": -1}' >/dev/null
curl -fsS -X DELETE "$WEB/api/v1/sends/music/main"
sleep 0.2
probe 1 1:1:1 2:1:0
pass "pan hard left, send deleted: L = tone 1, R = silence"

# A subgroup (C12): vox reaches main through sub; its send there says pre-fader
# −12 dB, but a subgroup takes it after the fader and pan at 0 dB.
curl -fsS -X DELETE "$WEB/api/v1/sends/vox/main"
api POST /api/v1/buses '{"id": "sub", "name": "Sub", "format": "stereo"}' >/dev/null
api PUT /api/v1/sends/vox/sub '{"tap": "pre-fader", "gain_db": -12}' >/dev/null
api PUT /api/v1/sends/sub/main >/dev/null
api PATCH /api/v1/strips/vox '{"fader_db": -6.0206}' >/dev/null
api POST /api/v1/snapshots '{"id": "before-sub"}' >/dev/null
api PATCH /api/v1/buses/sub '{"subgroup": true}' >/dev/null
sleep 0.2
probe 1 1:1:0.5 2:1:0
locked() { [[ "$(json "$WEB/api/v1/sends/vox/sub" "(d['tap'], d['gain_db'])")" == "('post-pan', 0.0)" ]]; }
locked || fail "the send into the subgroup was not locked"
# A recall of the send at −12 dB stores and plays the locked send.
api POST /api/v1/snapshots/before-sub/recall '{}' >/dev/null
sleep 0.2
locked || fail "a recall stored another level into the subgroup"
probe 1 1:1:0.5 2:1:0
curl -fsS -X DELETE "$WEB/api/v1/snapshots/before-sub"
# Clearing the flag keeps the send at 0 dB post-pan.
api PATCH /api/v1/buses/sub '{"subgroup": false}' >/dev/null
locked || fail "clearing the flag changed the send"
api PATCH /api/v1/buses/sub '{"subgroup": true}' >/dev/null
code=$(curl -s -o /dev/null -w '%{http_code}' -X PUT -H 'content-type: application/json' "$WEB/api/v1/sends/vox/sub" -d '{"gain_db": -6}')
[[ "$code" == 400 ]] || fail "a send level into a subgroup answered $code"
api PUT /api/v1/sends/vox/sub '{"mute": true}' >/dev/null
sleep 0.2
probe 1 1:1:0 2:1:0
api PUT /api/v1/sends/vox/sub '{"mute": false}' >/dev/null
# The group operation makes a subgroup: a send level into it is refused too.
group=$(api POST /api/v1/operations/group '{"strips": ["music"]}' | python3 -c "import json,sys; print(json.load(sys.stdin)['buses'][0])")
[[ "$(json "$WEB/api/v1/buses/$group" "d.get('subgroup')")" == True ]] || fail "the group operation made no subgroup"
code=$(curl -s -o /dev/null -w '%{http_code}' -X PUT -H 'content-type: application/json' "$WEB/api/v1/sends/music/$group" -d '{"gain_db": -6}')
[[ "$code" == 400 ]] || fail "a send level into a group answered $code"
curl -fsS -X DELETE "$WEB/api/v1/buses/$group"
pass "subgroup: its send post-pan at 0 dB (L = tone 1 × −6 dB fader), stored so after a recall and after clearing the flag, level locked (400, also in a group), mute kept"

# Deleting a bus (C9) takes its sends, VCA membership and layout place with it;
# a logic block that names it stays with no target.
api POST /api/v1/vcas '{"id": "grp", "name": "Grp", "members": ["sub", "vox"]}' >/dev/null
api PUT /api/v1/logic/del '{"name": "Del", "enabled": false, "blocks": [{"id": "m", "type": "set-mute", "signal": "sub"}], "wires": []}' >/dev/null
curl -fsS -X DELETE "$WEB/api/v1/buses/sub"
[[ "$(json "$WEB/api/v1/sends" "len([s for s in d if 'sub' in (s['from'], s['to'])])")" == 0 ]] || fail "sends of the deleted bus"
[[ "$(json "$WEB/api/v1/vcas/grp" "d['members']")" == "['vox']" ]] || fail "the deleted bus is still in the VCA"
[[ "$(json "$WEB/api/v1/logic/del" "d['blocks'][0]['signal']")" == "" ]] || fail "the logic block still names the deleted bus"
curl -fsS -X DELETE "$WEB/api/v1/logic/del"
curl -fsS -X DELETE "$WEB/api/v1/vcas/grp"
api PATCH /api/v1/strips/vox '{"fader_db": 0}' >/dev/null
api PUT /api/v1/sends/vox/main >/dev/null
sleep 0.2
probe 1 1:1:1 2:1:0
pass "bus deleted with its sends, VCA membership and logic target; vox straight to main again"
# A strip published as its own flow (its direct out): post-fader, before the pan.
api PATCH /api/v1/strips/vox '{"fader_db": -6.0206}' >/dev/null
api PUT /api/v1/outputs/vox >/dev/null
DIRECT=$(json "$WEB/api/v1/outputs/vox" "d['mxl_flow_id']")
[[ "$(json "$WEB/api/v1/outputs/vox" "d['format']")" == mono ]] || fail "vox direct out is not mono"
label=$(json "$NMOS/x-nmos/node/v1.3/senders" "[s['label'] for s in d if s['flow_id']]")
grep -q "'Vox'" <<<"$label" || fail "no sender Vox: $label"
MAIN_FLOW=$OUT_FLOW
OUT_FLOW=$DIRECT
sleep 0.2
probe 1 1:1:0.5
OUT_FLOW=$MAIN_FLOW
curl -fsS -X DELETE "$WEB/api/v1/outputs/vox"
api PATCH /api/v1/strips/vox '{"fader_db": 0}' >/dev/null
pass "strip vox published as its direct out: mono, post-fader (−6 dB), before the pan; NMOS sender Vox"

# The taps a source offers (GET /api/v1/taps): a bus has no input or post-pan.
[[ "$(json "$WEB/api/v1/taps" "'input' in d['vox'] and 'post-pan' in d['vox'] and 'input' not in d['main'] and 'post-pan' not in d['main'] and 'post-fader' in d['main']")" == True ]] ||
    fail "taps: $(curl -fsS "$WEB/api/v1/taps")"
pass "taps: a strip has input and post-pan, a bus has neither"

api PUT /api/v1/outputs/main '{"format": "mono"}' >/dev/null
wait_until 5 "mono output flow" python3 -c "import json; exit(0 if json.load(open('$OWN/$OUT_FLOW.mxl-flow/flow_def.json'))['channel_count'] == 1 else 1)"
sleep 0.2
probe 1 1:1:0.7079458
api PUT /api/v1/outputs/main '{}' >/dev/null
pass "published as mono: (L + R) × −3 dB sum"

sleep 0.2
peak=$(json "$WEB/api/v1/meters" "d['vox']['peak_dbfs'][0]")
python3 -c "exit(0 if -20.5 < $peak < -19.5 else 1)" || fail "vox meter peak $peak dBFS, not about −20"
pass "meters: vox peak $peak dBFS"

hold=$(json "$WEB/api/v1/meters" "d['vox']['peak_hold_dbfs'][0]")
python3 -c "exit(0 if -20.5 < $hold < -19.5 else 1)" || fail "vox peak hold $hold dBFS"
api PATCH /api/v1/strips/vox '{"mute": true}' >/dev/null
sleep 0.3
curl -fsS -X POST "$WEB/api/v1/meters/vox/clear"
sleep 0.2
[[ "$(json "$WEB/api/v1/meters" "d['vox']['peak_hold_dbfs'][0]")" == -inf ]] || fail "peak hold not cleared"
api PATCH /api/v1/strips/vox '{"mute": false}' >/dev/null
pass "peak hold $hold dBFS, cleared on request"

sender=$(json "$NMOS/x-nmos/node/v1.3/senders" "[s['id'] for s in d if s['label'] == 'Main'][0]")
active_flow=$(json "$NMOS/x-nmos/connection/v1.2/single/senders/$sender/active" "d['transport_params'][0]['mxl_flow_id']")
[[ "$active_flow" == "$OUT_FLOW" ]] || fail "sender active mxl_flow_id $active_flow, not $OUT_FLOW"
pass "NMOS sender Main reports mxl_flow_id $OUT_FLOW"
curl -fsS "$WEB/metrics" | grep -q 'mxl_audio_mixer_output_commit_lag_ms{output="main"}' || fail "output_commit_lag_ms metric"
curl -fsS "$WEB/metrics" | grep -q 'mxl_audio_mixer_plan_swaps_total' || fail "plan_swaps_total metric"

# A second mixer reads the output; the first restarts.
OWN_ID=$(json "$WEB/api/v1/status" "d['mxl']['output_domain_id']")
B_WEB=http://127.0.0.1:8162
env HOST_ID=it-b NMOS_HOST_ADDRESS="$HOST_IP" MXL_DOMAIN_SCAN_PATH="$WORK/mxl" MIXER_STATE_DIR="$WORK/state-b" \
    MIXER_INPUT_FLOWS=2 WEB_PORT=8162 NMOS_PORT=3314 NMOS_SEED=it-mix-b \
    "$MIXER_BIN" >>"$IT_OUT/mix-mixer-b.log" 2>&1 &
PIDS+=("$!")
wait_until 30 "mixer B" http_ok "$B_WEB/livez"
receiver=$(json "http://127.0.0.1:3314/x-nmos/node/v1.3/receivers" "[r['id'] for r in d if r['label'] == 'Input 1'][0]")
curl -fsS -o /dev/null -X PATCH -H 'content-type: application/json' \
    "http://127.0.0.1:3314/x-nmos/connection/v1.2/single/receivers/$receiver/staged" \
    -d "{\"master_enable\": true, \"activation\": {\"mode\": \"activate_immediate\"}, \"transport_params\": [{\"mxl_domain_id\": \"$OWN_ID\", \"mxl_flow_id\": \"$OUT_FLOW\"}]}"
b_running() { [[ "$(json "$B_WEB/api/v1/inputs/1" "d['state']")" == running ]]; }
wait_until 5 "mixer B reading the output" b_running
rc=0
stop_mixer || rc=$?
[[ $rc == 143 ]] || fail "exit code $rc"
[[ ! -d "$OWN/$OUT_FLOW.mxl-flow" ]] || fail "the output flow is still there after SIGTERM"
start_mixer NMOS_SEED=it-mix
[[ "$(json "$WEB/api/v1/outputs/main" "d['mxl_flow_id']")" == "$OUT_FLOW" ]] || fail "output flow id changed after the restart"
wait_until 5 "input 1 running after the restart" is_state 1 running
wait_until 10 "mixer B reading again" b_running
sleep 1
before=$(json "$B_WEB/api/v1/inputs/1" "d['missing_samples']")
sleep 2
after=$(json "$B_WEB/api/v1/inputs/1" "d['missing_samples']")
[[ "$before" == "$after" ]] || fail "mixer B lost samples after the restart"
probe 1 1:1:1 2:1:0
pass "restart: same output flow id, the mix restored, a downstream reader receives again"

curl -fsS -X DELETE "$WEB/api/v1/outputs/main"
wait_until 5 "flow removed" test ! -d "$OWN/$OUT_FLOW.mxl-flow"
code=$(curl -s -o /dev/null -w '%{http_code}' "$NMOS/x-nmos/node/v1.3/senders/$sender")
[[ "$code" == 404 ]] || fail "sender still there after unpublish ($code)"
pass "unpublish: flow and sender removed"

code=$(curl -s -o /dev/null -w '%{http_code}' -X PUT -H 'content-type: application/json' "$WEB/api/v1/sends/main/main" -d '{}')
[[ "$code" == 400 ]] || fail "a bus sending to itself answered $code"
api POST /api/v1/buses '{"id": "a", "format": "stereo"}' >/dev/null
api PUT /api/v1/sends/main/a >/dev/null
code=$(curl -s -o "$IT_OUT/mix-loop.json" -w '%{http_code}' -X PUT -H 'content-type: application/json' "$WEB/api/v1/sends/a/main" -d '{}')
if [[ "$code" != 400 ]] || ! grep -q "feedback loop" "$IT_OUT/mix-loop.json"; then
    fail "a feedback loop answered $code"
fi
pass "feedback without a block delay is rejected"

rc=0
stop_mixer || rc=$?
[[ $rc == 143 ]] || fail "exit code $rc"
pass mix
