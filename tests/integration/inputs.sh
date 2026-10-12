#!/usr/bin/env bash
# §17 MXL life-cycle and arrival-lag tests for inputs:
#   activation before the flow exists -> waiting -> running;
#   stalled writer -> stalled -> resumed;
#   writer restart (flow deleted and re-created) -> re-opened by itself;
#   writer 30 ms late -> the automatic delay settles, 0 missing samples afterwards.
# shellcheck source=tests/integration/lib.sh
source "$(dirname "$0")/lib.sh"

FLOW=33333333-3333-4333-8333-333333333333
LATE=44444444-4444-4444-8444-444444444444
start_mixer NMOS_SEED=it-inputs

# The domain must be visible for the activation; the flow does not exist yet.
mkdir -p "$WORK/mxl/tones"
echo '{"id": "55555555-5555-4555-8555-555555555555", "label": "tones", "description": "test tones", "tags": {}}' >"$WORK/mxl/tones/domain_def.json"
DOMAIN=55555555-5555-4555-8555-555555555555

# quiet <input> <seconds>: no missing samples over that window. A loss is measured
# once more: on a shared runner the writer or the DSP thread can be a block late
# once (the automatic delay then rises); a second loss fails.
quiet() {
    local before after attempt
    for attempt in 1 2; do
        before=$(input "$1" missing_samples)
        sleep "$2"
        after=$(input "$1" missing_samples)
        [[ "$after" == "$before" ]] && return 0
        log "input $1 lost $((after - before)) samples (attempt $attempt; $(glitches) glitches so far)"
    done
    return 1
}

route 1 "$DOMAIN" "$FLOW"
sleep 1
[[ "$(input 1 state)" == waiting ]] || fail "input 1 is $(input 1 state), not waiting, before the flow exists"
start_tone "$WORK/mxl/tones" "$FLOW" --domain-id "$DOMAIN"
wait_until 5 "input 1 running once the flow exists" is_state 1 running
pass "activation before the flow exists: waiting -> running"

sleep 2
quiet 1 3 || fail "missing samples while running"
pass "running: 0 missing samples over 3 s"

kill -STOP "$TONE_PID"
wait_until 5 "input 1 stalled" is_state 1 stalled
sleep 1
[[ "$(input 1 state)" == stalled ]] || fail "input 1 left the stalled state while the writer is stopped"
kill -CONT "$TONE_PID"
wait_until 5 "input 1 resumed" is_state 1 running
if ! grep -q input_stalled "$IT_OUT/inputs-mixer.log" || ! grep -q input_resumed "$IT_OUT/inputs-mixer.log"; then
    fail "input_stalled / input_resumed not logged"
fi
pass "stalled writer: stalled -> resumed"

reopens=$(input 1 reopens)
kill -TERM "$TONE_PID"
wait "$TONE_PID" || true
wait_until 5 "input 1 waiting after the flow is gone" is_state 1 waiting
start_tone "$WORK/mxl/tones" "$FLOW" --domain-id "$DOMAIN"
wait_until 5 "input 1 running again" is_state 1 running
(($(input 1 reopens) > reopens)) || fail "reopens did not increase"
sleep 1
quiet 1 3 || fail "missing samples after the re-open"
pass "writer restart: the input re-opened by itself, 0 missing samples afterwards"

# Arrival lag: a writer 30 ms late. The automatic delay must settle above the lag.
start_tone "$WORK/mxl/tones" "$LATE" --domain-id "$DOMAIN" --delay-ms 30
wait_until 10 "late flow" test -d "$WORK/mxl/tones/$LATE.mxl-flow"
route 3 "$DOMAIN" "$LATE"
wait_until 10 "input 3 running" is_state 3 running
wait_until 15 "the automatic delay of input 3 above 30 ms" python3 -c "import json,urllib.request; d=json.load(urllib.request.urlopen('$WEB/api/v1/inputs/3')); exit(0 if d['effective_delay_ms'] > 30 else 1)"
sleep 2
quiet 3 5 || fail "input 3 lost samples after the delay settled"
log "input 3: effective delay $(input 3 effective_delay_ms) ms, arrival lag p99.9 $(json "$WEB/api/v1/inputs/3" "d['arrival_lag_ms']['p999']") ms"
pass "arrival lag: the automatic delay settled, 0 missing samples afterwards"

# master_enable false: the input stops reading.
route 1 "$DOMAIN" "$FLOW" false
wait_until 5 "input 1 unrouted" is_state 1 unrouted
pass "master_enable false stops reading"

curl -fsS "$WEB/metrics" | grep -q 'mxl_audio_mixer_input_state{input="3"} 2' || fail "input_state metric"
rc=0
stop_mixer || rc=$?
[[ $rc == 143 ]] || fail "exit code $rc"
pass inputs
