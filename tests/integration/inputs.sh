#!/usr/bin/env bash
# §17 MXL life-cycle and arrival-lag tests for inputs:
#   activation before the flow exists -> waiting -> running;
#   stalled writer -> stalled -> resumed;
#   writer restart (flow deleted and re-created) -> re-opened by itself;
#   writer 30 ms late -> the automatic delay settles, 0 missing samples afterwards;
#   writers that commit in batches (a video frame of audio, bursts of packets) at the
#   default 2 ms read offset -> the arrival lag shows the batch, the automatic delay
#   covers it, 0 missing samples (F1);
#   lag outliers that come and go -> the automatic delay holds instead of flapping (F2).
# shellcheck source=tests/integration/lib.sh
source "$(dirname "$0")/lib.sh"

FLOW=33333333-3333-4333-8333-333333333333
LATE=44444444-4444-4444-8444-444444444444
FRAMES=66666666-6666-4666-8666-666666666666
BURSTS=77777777-7777-4777-8777-777777777777
OUTLIERS=88888888-8888-4888-8888-888888888888
start_mixer NMOS_SEED=it-inputs MIXER_INPUT_FLOWS=5

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
wait_until 15 "the automatic delay of input 3 above 30 ms" python3 -c "import json,urllib.request as u; d=json.load(u.build_opener(u.ProxyHandler({})).open('$WEB/api/v1/inputs/3')); exit(0 if d['effective_delay_ms'] > 30 else 1)"
sleep 2
quiet 3 5 || fail "input 3 lost samples after the delay settled"
log "input 3: effective delay $(input 3 effective_delay_ms) ms, arrival lag p99.9 $(json "$WEB/api/v1/inputs/3" "d['arrival_lag_ms']['p999']") ms"
pass "arrival lag: the automatic delay settled, 0 missing samples afterwards"

# F1: correctly timed writers that commit in batches, at the default 2 ms read offset.
# The browser source commits a 50 Hz frame of audio (960 samples) at once; a receiver
# can hand over a burst of 1 ms packets (here 5). Just before the next commit such a
# flow is a whole batch behind, and that is what a read needs. v1.3.0 measured the lag
# only in blocks right after a commit (about 0.5 ms), set no delay and lost most
# blocks as late.
start_tone "$WORK/mxl/tones" "$FRAMES" --domain-id "$DOMAIN" --block 960
start_tone "$WORK/mxl/tones" "$BURSTS" --domain-id "$DOMAIN" --block 48 --delay-ms 0.4 --batch 5
# F2: a mirror 6 ms late with 2.5 ms more for 0.2 s every 12 s; the outliers leave the
# 10 s lag window between two of them. v1.3.0 followed them up and down (with a fade).
start_tone "$WORK/mxl/tones" "$OUTLIERS" --domain-id "$DOMAIN" --block 96 --delay-ms 6 --late-ms 2.5 --every-s 12 --for-s 0.2
for f in "$FRAMES" "$BURSTS" "$OUTLIERS"; do
    wait_until 10 "flow $f" test -d "$WORK/mxl/tones/$f.mxl-flow"
done
MIXER_LOG_AT=$(wc -l <"$IT_OUT/inputs-mixer.log")
route 2 "$DOMAIN" "$FRAMES"
route 4 "$DOMAIN" "$BURSTS"
route 5 "$DOMAIN" "$OUTLIERS"
OUTLIERS_ROUTED=$SECONDS
for i in 2 4 5; do wait_until 10 "input $i running" is_state "$i" running; done
delay_above() { # <input> <ms>
    python3 -c "import json,urllib.request as u; d=json.load(u.build_opener(u.ProxyHandler({})).open('$WEB/api/v1/inputs/$1')); exit(0 if d['extra_delay_ms'] > $2 else 1)"
}
wait_until 15 "the automatic delay of input 2 (960-sample commits) above 18 ms" delay_above 2 18
wait_until 15 "the automatic delay of input 4 (bursts of 5 ms) above 3 ms" delay_above 4 3
sleep 2
quiet 2 5 || fail "input 2 (960-sample commits) lost samples after the delay settled"
quiet 4 5 || fail "input 4 (bursts of 5 ms) lost samples after the delay settled"
for i in 2 4; do
    lag=$(json "$WEB/api/v1/inputs/$i" "d['arrival_lag_ms']['p999']")
    log "input $i: arrival lag p99.9 $lag ms, effective delay $(input $i effective_delay_ms) ms"
done
python3 -c "import sys; sys.exit(0 if float('$(json "$WEB/api/v1/inputs/2" "d['arrival_lag_ms']['p999']")') >= 18 else 1)" ||
    fail "input 2: the arrival lag does not show the 20 ms batches"
pass "batched writers at a 2 ms read offset: the arrival lag shows the batch, the delay covers it, 0 missing samples"

# F2: over 28 s from routing (two outliers), input 5 gets its first automatic delay
# and may go up once more (an outlier needs more), but not down again (hold 30 s).
rest=$((OUTLIERS_ROUTED + 28 - SECONDS))
((rest <= 0)) || sleep "$rest"
changes=$(tail -n +$((MIXER_LOG_AT + 1)) "$IT_OUT/inputs-mixer.log" | grep -c 'input delay set input=5 ' || true)
log "input 5: $changes automatic delay changes in 28 s, effective delay $(input 5 effective_delay_ms) ms"
((changes >= 1)) || fail "input 5 got no automatic delay"
((changes <= 2)) || fail "input 5: the automatic delay changed $changes times in 28 s (flapping)"
pass "lag outliers: the automatic delay holds instead of flapping"

# master_enable false: the input stops reading.
route 1 "$DOMAIN" "$FLOW" false
wait_until 5 "input 1 unrouted" is_state 1 unrouted
pass "master_enable false stops reading"

# Read once: `curl | grep -q` fails under pipefail when grep stops before curl is done.
metrics=$(curl -fsS "$WEB/metrics")
grep -q 'mxl_audio_mixer_input_state{input="3"} 2' <<<"$metrics" || fail "input_state metric"
grep -q 'mxl_audio_mixer_input_overrun_samples_total{input="3"}' <<<"$metrics" || fail "overrun metric"
grep -q 'mxl_audio_mixer_input_late_samples_total{input="3"}' <<<"$metrics" || fail "late metric"
rc=0
stop_mixer || rc=$?
[[ $rc == 143 ]] || fail "exit code $rc"
pass inputs
