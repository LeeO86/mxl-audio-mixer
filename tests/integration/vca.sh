#!/usr/bin/env bash
# VCAs and mute groups (§9) through MXL, sample-exact: a VCA's fader on its
# members, nested VCAs, VCA mute (the members' mutes unchanged), VCA PFL on the
# monitor, a mute group with a mute-safe strip, a snapshot of the VCAs, a nesting
# cycle refused; none of it swaps the plan.
# shellcheck source=tests/integration/lib.sh
source "$(dirname "$0")/lib.sh"

FLOW=88888888-8888-4888-8888-888888888888
DOMAIN=99999999-9999-4999-8999-999999999999
start_tone "$WORK/mxl/tones" "$FLOW" --domain-id "$DOMAIN" --channels 2
wait_until 10 "tone flow" test -d "$WORK/mxl/tones/$FLOW.mxl-flow"
start_mixer NMOS_SEED=it-vca
route 1 "$DOMAIN" "$FLOW"
wait_until 5 "input 1 running" is_state 1 running

# vox (tone 1) hard left, gtr (tone 2) hard right, into the programme bus.
api POST /api/v1/strips '{"id": "vox", "format": "mono", "pan": -1, "patch": [{"input": 1, "channel": 1}]}' >/dev/null
api POST /api/v1/strips '{"id": "gtr", "format": "mono", "pan": 1, "patch": [{"input": 1, "channel": 2}]}' >/dev/null
api POST /api/v1/buses '{"id": "pgm", "name": "Programme", "format": "stereo"}' >/dev/null
api PUT /api/v1/sends/vox/pgm >/dev/null
api PUT /api/v1/sends/gtr/pgm >/dev/null
api PUT /api/v1/outputs/pgm >/dev/null
api POST /api/v1/monitors '{"id": "main", "name": "Main", "format": "stereo", "source": {"signal": "pgm"}, "publish": true}' >/dev/null
OWN=$(json "$WEB/api/v1/status" "d['mxl']['output_domain_dir']")
PGM=$(json "$WEB/api/v1/outputs/pgm" "d['mxl_flow_id']")
MON=$(json "$WEB/api/v1/status" "[o['mxl_flow_id'] for o in d['outputs'] if o['signal'] == 'monitor:main'][0]")
OUT_FLOW=$PGM
settled 1
probe 1 1:1:1 2:2:1
swaps() { json "$WEB/api/v1/status" "d['dsp']['plan_swaps']"; }
SWAPS=$(swaps)

vca() { api PATCH "/api/v1/vcas/$1" "$2" >/dev/null; sleep 0.2; }
api POST /api/v1/vcas '{"id": "band", "name": "Band", "fader_db": -20, "members": ["vox", "gtr"]}' >/dev/null
sleep 0.2
probe 1 1:1:0.1 2:2:0.1
pass "VCA band at −20 dB: both members at 0.1, their faders unchanged"
api POST /api/v1/vcas '{"id": "all", "name": "All", "fader_db": -6.0206, "members": ["band"]}' >/dev/null
sleep 0.2
probe 1 1:1:0.05 2:2:0.05
pass "nested: VCA all (−6 dB) over band (−20 dB): 0.05"
vca all '{"mute": true}'
probe 1 1:1:0 2:2:0
[[ "$(json "$WEB/api/v1/strips/vox" "d['mute']")" == False ]] || fail "VCA mute changed vox's own mute"
vca all '{"mute": false}'
vca all '{"fader_db": 0}'
vca band '{"fader_db": 0}'
probe 1 1:1:1 2:2:1
pass "VCA mute mutes the nested members without touching their own mute"

code=$(curl -s -o /dev/null -w '%{http_code}' -X PATCH -H 'content-type: application/json' "$WEB/api/v1/vcas/band" -d '{"members": ["vox", "gtr", "all"]}')
[[ "$code" == 400 ]] || fail "a nesting cycle answered $code"
pass "a VCA nested in itself is refused (400)"

[[ "$(swaps)" == "$SWAPS" ]] || fail "VCAs swapped the plan ($SWAPS → $(swaps))"
api POST /api/v1/vcas '{"id": "lead", "name": "Lead", "fader_db": -10, "members": ["vox"]}' >/dev/null
api PATCH /api/v1/control-room '{"solo": {"programme_db": "-inf"}}' >/dev/null
api PUT /api/v1/solo/lead '{"kind": "pfl"}' >/dev/null
sleep 0.3
OUT_FLOW=$MON probe 1 1:1:0.70710677 2:1:0.70710677
curl -fsS -o /dev/null -X DELETE "$WEB/api/v1/solo"
sleep 0.3
pass "PFL of VCA lead: its member vox, pre-fader, on the monitor"
api DELETE /api/v1/vcas/lead >/dev/null
# A solo swaps the plan (the solo bus); count again from here.
SWAPS=$(swaps)

api POST /api/v1/mute-groups '{"id": "band", "name": "Band", "active": true, "members": ["vox", "gtr"]}' >/dev/null
sleep 0.2
probe 1 1:1:0 2:2:0
api PATCH /api/v1/strips/gtr '{"mute_safe": true}' >/dev/null
sleep 0.2
probe 1 1:1:0 2:2:1
api PATCH /api/v1/mute-groups/band '{"active": false}' >/dev/null
sleep 0.2
probe 1 1:1:1 2:2:1
pass "mute group band: both muted; gtr mute-safe stays; inactive: both back"

api PATCH /api/v1/vcas/band '{"fader_db": -20}' >/dev/null
SNAP=$(api POST /api/v1/snapshots '{"name": "VCA -20"}' | python3 -c "import json,sys; print(json.load(sys.stdin)['id'])")
vca band '{"fader_db": 0}'
probe 1 1:1:1 2:2:1
api POST "/api/v1/snapshots/$SNAP/recall" '{"sections": ["vcas"]}' >/dev/null
sleep 0.3
probe 1 1:1:0.1 2:2:0.1
[[ "$(json "$WEB/api/v1/vcas/band" "d['fader_db']")" == -20.0 ]] || fail "VCA after the recall: $(curl -fsS "$WEB/api/v1/vcas/band")"
pass "a snapshot recalls the VCA fader (section vcas)"

[[ "$(swaps)" == "$SWAPS" ]] || fail "VCAs and mute groups swapped the plan ($SWAPS → $(swaps))"
pass "no plan swap for any VCA or mute group change"

api DELETE /api/v1/strips/vox >/dev/null
[[ "$(json "$WEB/api/v1/vcas/band" "d['members']")" == "['gtr']" ]] || fail "a deleted strip stays a member"
pass "a deleted strip leaves its VCAs"

rc=0
stop_mixer || rc=$?
[[ $rc == 143 ]] || fail "exit code $rc"
pass vca
