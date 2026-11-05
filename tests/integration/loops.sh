#!/usr/bin/env bash
# 1.6.0 D1: an input that reads one of the mixer's own output flows must not come to carry itself
# (a loop through MXL): refused at the UI route, at an IS-05 activation (500 with the reason) and at a
# later mix change; an input routed to an own flow that does not carry it is fine.
# shellcheck source=tests/integration/lib.sh
source "$(dirname "$0")/lib.sh"

start_mixer NMOS_SEED=it-loops MIXER_INPUT_FLOWS=4
api POST /api/v1/buses '{"id": "aux", "format": "stereo"}' >/dev/null
api PUT /api/v1/outputs/aux '{}' >/dev/null
api POST /api/v1/strips '{"id": "vox", "format": "stereo", "patch": [{"input": 1, "channel": 1}, {"input": 1, "channel": 2}]}' >/dev/null
api PUT /api/v1/sends/vox/aux '{}' >/dev/null
DOMAIN=$(json "$WEB/api/v1/status" "d['mxl']['output_domain_id']")
FLOW=$(json "$WEB/api/v1/outputs" "[o['mxl_flow_id'] for o in d if o['signal'] == 'aux'][0]")
code() { curl -s -o "$WORK/answer" -w '%{http_code}' -X "$1" -H 'content-type: application/json' "$2" -d "$3"; }

# Input 1 feeds aux: reading aux's flow on input 1 is a loop.
c=$(code PUT "$WEB/api/v1/inputs/1/route" "{\"flow_id\": \"$FLOW\", \"domain_id\": \"$DOMAIN\"}")
[[ "$c" == 400 ]] && grep -q "feedback loop" "$WORK/answer" || fail "UI route of input 1 to aux answered $c: $(cat "$WORK/answer")"
pass "UI route: $(python3 -c "import json,sys; print(json.load(open(sys.argv[1]))['error'])" "$WORK/answer")"
receiver=$(json "$NMOS/x-nmos/node/v1.3/receivers" "[r['id'] for r in d if r['label'] == 'Input 1'][0]")
c=$(code PATCH "$NMOS/x-nmos/connection/v1.2/single/receivers/$receiver/staged" \
    "{\"master_enable\": true, \"activation\": {\"mode\": \"activate_immediate\"}, \"transport_params\": [{\"mxl_domain_id\": \"$DOMAIN\", \"mxl_flow_id\": \"$FLOW\"}]}")
[[ "$c" == 500 ]] && grep -q "feedback loop" "$WORK/answer" || fail "IS-05 activation of input 1 to aux answered $c: $(cat "$WORK/answer")"
pass "IS-05 activation refused (500 with the loop)"

# Input 3 reads aux (no loop yet); a strip on input 3 into aux then is refused.
c=$(code PUT "$WEB/api/v1/inputs/3/route" "{\"flow_id\": \"$FLOW\", \"domain_id\": \"$DOMAIN\"}")
[[ "$c" == 200 ]] || fail "route of input 3 to aux answered $c: $(cat "$WORK/answer")"
api POST /api/v1/strips '{"id": "ret", "format": "stereo", "patch": [{"input": 3, "channel": 1}, {"input": 3, "channel": 2}]}' >/dev/null
c=$(code PUT "$WEB/api/v1/sends/ret/aux" '{}')
[[ "$c" == 400 ]] && grep -q "feedback loop" "$WORK/answer" || fail "send ret → aux answered $c: $(cat "$WORK/answer")"
pass "a later mix change that makes the own flow carry its reader is refused"
pass loops
