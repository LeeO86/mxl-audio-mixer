#!/usr/bin/env bash
# N-1 and the group operations (§5) through MXL: N-1 per participant from the
# programme (each contains all but its own channel: correlation, §17), one N-1 of
# a selection, a mix-minus that follows its bus without a plan swap, a group that
# takes over the strips' sends, link of two mono strips into a stereo strip and
# unlink, both sample-exact.
# shellcheck source=tests/integration/lib.sh
source "$(dirname "$0")/lib.sh"

FLOW=88888888-8888-4888-8888-888888888888
DOMAIN=99999999-9999-4999-8999-999999999999
start_tone "$WORK/mxl/tones" "$FLOW" --domain-id "$DOMAIN" --channels 4
wait_until 10 "tone flow" test -d "$WORK/mxl/tones/$FLOW.mxl-flow"
start_mixer NMOS_SEED=it-n1
route 1 "$DOMAIN" "$FLOW"
wait_until 5 "input 1 running" is_state 1 running

# s1…s4 (tones 1…4) into the mono programme bus, post-fader.
api POST /api/v1/buses '{"id": "pgm", "name": "Programme", "format": "mono"}' >/dev/null
for i in 1 2 3 4; do
    api POST /api/v1/strips "{\"id\": \"s$i\", \"format\": \"mono\", \"patch\": [{\"input\": 1, \"channel\": $i}]}" >/dev/null
    api PUT "/api/v1/sends/s$i/pgm" '{"tap": "post-fader"}' >/dev/null
done
OWN=$(json "$WEB/api/v1/status" "d['mxl']['output_domain_dir']")
flow() { json "$WEB/api/v1/outputs/$1" "d['mxl_flow_id']"; }
swaps() { json "$WEB/api/v1/status" "d['dsp']['plan_swaps']"; }
# corr <bus> : channel 1's correlation with tones 1-4, as "c1 c2 c3 c4"
corr() {
    "$PROBE_BIN" --domain "$OWN" --flow "$(flow "$1")" --seconds 1 --correlate 4 |
        tee -a "$IT_OUT/n1-probe.log" |
        python3 -c "import json,sys; print(' '.join(f'{c:.3f}' for c in json.load(sys.stdin)['correlation']))"
}
# has <bus> <tones in it> <tones not in it>: correlation above 0.4 or below 0.02
has() {
    local c
    c=$(corr "$1")
    python3 - "$c" "$2" "$3" <<'EOF' || fail "$1: correlation $c, expected in: $2, not in: $3"
import sys
c = [float(x) for x in sys.argv[1].split()]
ins = [int(t) for t in sys.argv[2].split(",") if t]
outs = [int(t) for t in sys.argv[3].split(",") if t]
assert all(c[t - 1] > 0.4 for t in ins) and all(c[t - 1] < 0.02 for t in outs), c
EOF
    log "$1: correlation with tones 1-4: $c"
}

made=$(api POST /api/v1/operations/n-1 '{"strips": ["s1", "s2", "s3"], "of": "pgm"}')
[[ "$(python3 -c "import json,sys; print(json.loads(sys.argv[1])['buses'])" "$made")" == "['n1-s1', 'n1-s2', 'n1-s3']" ]] || fail "n-1: $made"
for b in pgm n1-s1 n1-s2 n1-s3; do api PUT "/api/v1/outputs/$b" >/dev/null; done
settled 1
has pgm 1,2,3,4 ""
has n1-s1 2,3,4 1
has n1-s2 1,3,4 2
has n1-s3 1,2,4 3
pass "N-1 per participant from the programme: each contains all but its own channel"

made=$(api POST /api/v1/operations/n-1 '{"strips": ["s1", "s2"], "of": "pgm", "mode": "one"}')
ONE=$(python3 -c "import json,sys; print(json.loads(sys.argv[1])['buses'][0])" "$made")
api PUT "/api/v1/outputs/$ONE" >/dev/null
has "$ONE" 3,4 1,2
pass "one N-1 of a selection ($ONE): the programme without s1 and s2"

made=$(api POST /api/v1/operations/mix-minus '{"of": "pgm", "without": ["s4"], "id": "remote"}')
api PUT /api/v1/outputs/remote >/dev/null
has remote 1,2,3 4
SWAPS=$(swaps)
api PUT /api/v1/sends/s2/pgm '{"gain_db": "-inf"}' >/dev/null
sleep 0.3
has remote 1,3 2,4
has n1-s1 3,4 1,2
[[ "$(swaps)" == "$SWAPS" ]] || fail "a send level of the programme swapped the plan"
api PUT /api/v1/sends/s2/pgm '{"gain_db": 0}' >/dev/null
pass "a mix-minus follows its bus: s2's send muted in the programme leaves remote and the N-1s, without a plan swap"

# Link s3 and s4 into a stereo strip; a stereo bus shows it sample-exact, and again
# after unlinking into two mono strips (hard left and right).
api POST /api/v1/buses '{"id": "st", "name": "Stereo", "format": "stereo"}' >/dev/null
api PUT /api/v1/sends/s3/st '{"tap": "post-pan"}' >/dev/null
api PATCH /api/v1/strips/s3 '{"pan": -1}' >/dev/null
api PUT /api/v1/sends/s4/st '{"tap": "post-pan"}' >/dev/null
api PATCH /api/v1/strips/s4 '{"pan": 1}' >/dev/null
api PUT /api/v1/outputs/st >/dev/null
OUT_FLOW=$(flow st)
sleep 0.3
probe 1 1:3:1 2:4:1
api POST /api/v1/operations/link '{"strips": ["s3", "s4"], "id": "pair", "name": "Pair"}' >/dev/null
[[ "$(json "$WEB/api/v1/strips/pair" "d['format'], [p['channel'] for p in d['patch']]")" == "stereo [3, 4]" ]] || fail "link: $(curl -fsS "$WEB/api/v1/strips/pair")"
sleep 0.3
probe 1 1:3:1 2:4:1
[[ "$(json "$WEB/api/v1/buses/remote" "d['minus']")" == "{'of': 'pgm', 'without': ['pair']}" ]] || fail "the mix-minus did not follow the link: $(curl -fsS "$WEB/api/v1/buses/remote")"
pass "link s3 + s4 into the stereo strip pair: sample-exact on the stereo bus; the mix-minus now excludes pair"
api POST /api/v1/operations/unlink '{"strip": "pair"}' >/dev/null
sleep 0.3
probe 1 1:3:1 2:4:1
pass "unlink pair into pair-l and pair-r (hard left and right): sample-exact"

# Group s1 and s2: refused while N-1s of the programme leave them out (through
# the group they would be back in); without those, their post-fader sends to the
# programme move to the group.
code=$(curl -s -o /dev/null -w '%{http_code}' -X POST -H 'content-type: application/json' "$WEB/api/v1/operations/group" -d '{"strips": ["s1", "s2"], "id": "talent", "format": "mono"}')
[[ "$code" == 400 ]] || fail "a group that breaks N-1s answered $code"
for b in n1-s1 n1-s2 "$ONE"; do api DELETE "/api/v1/buses/$b" >/dev/null; done
made=$(api POST /api/v1/operations/group '{"strips": ["s1", "s2"], "id": "talent", "format": "mono"}')
sends=$(json "$WEB/api/v1/sends" "sorted((s['from'], s['to']) for s in d if s['to'] in ('pgm', 'talent'))")
[[ "$sends" == "[('pair-l', 'pgm'), ('pair-r', 'pgm'), ('s1', 'talent'), ('s2', 'talent'), ('talent', 'pgm')]" ]] || fail "group: $sends"
has pgm 1,2,3,4 ""
pass "group s1 + s2: refused while N-1s leave them out; then their programme sends go through the group talent; the programme still has all four"

rc=0
stop_mixer || rc=$?
[[ $rc == 143 ]] || fail "exit code $rc"
pass n1
