#!/usr/bin/env bash
# §17 MXL life-cycle and arrival-lag tests for inputs:
#   activation before the flow exists -> waiting -> running;
#   stalled writer -> stalled -> resumed;
#   writer restart (flow deleted and re-created) -> re-opened by itself;
#   writer 30 ms late -> the automatic delay settles, 0 missing samples afterwards;
#   writers that commit in batches (a video frame of audio, bursts of packets) at the
#   default 2 ms read offset -> the arrival lag shows the batch, the automatic delay
#   covers it, 0 missing samples (F1);
#   lag outliers that come and go -> the automatic delay holds instead of flapping (F2);
#   a steady writer with a rare long outlier -> the first late block raises the
#   delay, later outliers are covered (1.4.1).
# shellcheck source=tests/integration/lib.sh
source "$(dirname "$0")/lib.sh"

FLOW=33333333-3333-4333-8333-333333333333
LATE=44444444-4444-4444-8444-444444444444
FRAMES=66666666-6666-4666-8666-666666666666
BURSTS=77777777-7777-4777-8777-777777777777
OUTLIERS=88888888-8888-4888-8888-888888888888
RARE=99999999-9999-4999-8999-999999999999
start_mixer NMOS_SEED=it-inputs MIXER_INPUT_FLOWS=6

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

# Input 2 reads the same flow: both re-open by themselves, each with a reader of
# its own (an MXL instance per open: a shared one could hand back the old reader,
# mapped on the deleted files).
route 2 "$DOMAIN" "$FLOW"
wait_until 5 "input 2 running" is_state 2 running
reopens=$(input 1 reopens)
kill -TERM "$TONE_PID"
wait "$TONE_PID" || true
wait_until 5 "input 1 waiting after the flow is gone" is_state 1 waiting
start_tone "$WORK/mxl/tones" "$FLOW" --domain-id "$DOMAIN"
wait_until 5 "input 1 running again" is_state 1 running
wait_until 5 "input 2 running again" is_state 2 running
(($(input 1 reopens) > reopens)) || fail "reopens did not increase"
# The writer restarted at once, three times: the flow is deleted and re-created
# within about one control tick, so the inputs re-open while the DSP thread still
# holds the old readers. 1.4.0 got the cached old reader back: stalled, on the
# deleted files.
for _ in 1 2 3; do
    kill -TERM "$TONE_PID"
    wait "$TONE_PID" || true
    start_tone "$WORK/mxl/tones" "$FLOW" --domain-id "$DOMAIN"
    sleep 1.5
done
wait_until 5 "input 1 running after the quick restarts" is_state 1 running
wait_until 5 "input 2 running after the quick restarts" is_state 2 running
sleep 1
quiet 2 3 || fail "input 2 (the same flow) lost samples after the re-open"
if grep -q "$FLOW.*(deleted)" "/proc/$MIXER_PID/maps"; then
    fail "the mixer still maps the deleted flow: $(grep -c "$FLOW.*(deleted)" "/proc/$MIXER_PID/maps") mappings"
fi
sleep 1
quiet 1 3 || fail "missing samples after the re-open"
pass "writer restart: two inputs on the flow re-opened by themselves, 0 missing samples afterwards, nothing of the deleted flow mapped"

# Arrival lag: a writer 30 ms late. The automatic delay must settle above the lag.
start_tone "$WORK/mxl/tones" "$LATE" --domain-id "$DOMAIN" --delay-ms 30
WRITERS=("$TONE_PID")
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
WRITERS+=("$TONE_PID")
start_tone "$WORK/mxl/tones" "$BURSTS" --domain-id "$DOMAIN" --block 48 --delay-ms 0.4 --batch 5
WRITERS+=("$TONE_PID")
# F2: a mirror 6 ms late with 2.5 ms more for 0.2 s every 12 s; the outliers leave the
# 10 s lag window between two of them. v1.3.0 followed them up and down (with a fade).
start_tone "$WORK/mxl/tones" "$OUTLIERS" --domain-id "$DOMAIN" --block 96 --delay-ms 6 --late-ms 2.5 --every-s 12 --for-s 0.2
WRITERS+=("$TONE_PID")
# 1.4.1: the gateways of test-all, per packet and on time but once in a while one
# packet 6 ms late (here every 8 s). Far below the p99.9; 1.4.0 lost a block at each.
start_tone "$WORK/mxl/tones" "$RARE" --domain-id "$DOMAIN" --block 48 --delay-ms 0.4 --late-ms 6 --every-s 8 --for-s 0.001
WRITERS+=("$TONE_PID")
for f in "$FRAMES" "$BURSTS" "$OUTLIERS" "$RARE"; do
    wait_until 10 "flow $f" test -d "$WORK/mxl/tones/$f.mxl-flow"
done
MIXER_LOG_AT=$(wc -l <"$IT_OUT/inputs-mixer.log")
route 2 "$DOMAIN" "$FRAMES"
route 4 "$DOMAIN" "$BURSTS"
route 5 "$DOMAIN" "$OUTLIERS"
route 6 "$DOMAIN" "$RARE"
OUTLIERS_ROUTED=$SECONDS
for i in 2 4 5 6; do wait_until 10 "input $i running" is_state "$i" running; done
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
# At least 9 s after routing input 6 its first outlier has come and raised the delay.
rest=$((OUTLIERS_ROUTED + 9 - SECONDS))
((rest <= 0)) || sleep "$rest"
RARE_MISSING=$(input 6 missing_samples)
RARE_AT=$SECONDS

# F2: over 28 s from routing (two outliers), input 5 gets its automatic delay and may
# go up when an outlier needs more (a slow shared runner adds rises), but it must not
# go down again (hold 30 s). v1.3.0 went down 10 s after each outlier.
rest=$((OUTLIERS_ROUTED + 28 - SECONDS))
((rest <= 0)) || sleep "$rest"
changes=$(tail -n +$((MIXER_LOG_AT + 1)) "$IT_OUT/inputs-mixer.log" | grep 'input delay set input=5 ' || true)
read -r ups downs < <(python3 -c "
import re, sys
c = [(float(a), float(b)) for a, b in re.findall(r'from_ms=([0-9.]+) to_ms=([0-9.]+)', sys.stdin.read())]
print(sum(b > a for a, b in c), sum(b < a for a, b in c))" <<<"$changes")
log "input 5: $ups rises and $downs falls of the automatic delay in 28 s, effective delay $(input 5 effective_delay_ms) ms"
((ups >= 1)) || fail "input 5 got no automatic delay"
((downs == 0)) || fail "input 5: the automatic delay went down $downs times in 28 s (flapping)"
pass "lag outliers: the automatic delay holds instead of flapping"

# 1.4.1: since the first outlier (and its rise) input 6 lost nothing over two more.
((SECONDS - RARE_AT >= 16)) || sleep $((16 - (SECONDS - RARE_AT)))
missing=$(($(input 6 missing_samples) - RARE_MISSING))
log "input 6: $missing samples lost after its first outlier, delay $(input 6 extra_delay_ms) ms"
((missing == 0)) || fail "input 6: the rare 6 ms outliers lost $missing samples after the first one"
# The outlier reads 6-7 ms late at block granularity: a delay of 5 ms or more covers it.
delay_above 6 4.5 || fail "input 6: the automatic delay does not cover the 6 ms outlier"
pass "a rare long outlier: the first late block raises the delay, the later ones are covered"

# Five writers stopped at once: their reads wait for samples that do not come.
# The wait is a quarter block shared by all inputs of a block; 1.4.0 gave each
# input its own, so five late inputs took longer than the block, every block a
# deadline miss (also at the start, before the delays were set).
misses() { json "$WEB/api/v1/status" "d['dsp']['deadline_misses']"; }
# The misses of 3 s with the writers running first: a loaded machine has some.
base=$(misses)
sleep 3
before=$(misses)
kill -STOP "${WRITERS[@]}"
sleep 3
after=$(misses)
kill -CONT "${WRITERS[@]}"
for i in 2 3 4 5 6; do wait_until 10 "input $i running again" is_state "$i" running; done
# The writers catch up after the stop (seconds behind, then faster than real time):
# that is no lag, the delays stay where they were (1.4.1 review: they went to the
# maximum, 150 ms).
sleep 2
for i in 2 3 4 5 6; do
    d=$(input "$i" extra_delay_ms)
    python3 -c "import sys; sys.exit(0 if $d < 60 else 1)" || fail "input $i: delay $d ms after its writer caught up"
done
log "five stopped writers: $((after - before)) deadline misses in 3 s ($((before - base)) before, running)"
((after - before - (before - base) < 50)) || fail "five stopped writers: $((after - before)) deadline misses in 3 s, $((before - base)) running"
pass "late inputs share one short wait: no deadline misses with five stopped writers"

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
