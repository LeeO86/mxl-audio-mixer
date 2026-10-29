#!/usr/bin/env bash
# 1.5.0 C7, C2, C3: virtual outputs (MIXER_OUTPUTS) are MXL flows and NMOS senders
# with stable ids from the start, silent until a source is picked; a bus, a strip's
# direct out, a monitor and an input straight through feed them (up- and down-mixed
# to the flow's format); a restart keeps the flow ids and the sources. Inputs: the
# audio flows are listed, an input is routed through its own IS-05 receiver, and it
# shows the name of what is connected (the MXL flow's label without a registry).
# shellcheck source=tests/integration/lib.sh
source "$(dirname "$0")/lib.sh"

FLOW=12121212-1212-4121-8121-121212121212
DOMAIN=34343434-3434-4343-8343-343434343434
start_tone "$WORK/mxl/tones" "$FLOW" --domain-id "$DOMAIN" --channels 2
wait_until 10 "tone flow" test -d "$WORK/mxl/tones/$FLOW.mxl-flow"
OUTPUTS='[{"key": "pgm", "label": "PGM", "format": "5.1"}, {"label": "Clean"}, {"format": "mono"}]'
start_mixer NMOS_SEED=it-vouts MIXER_OUTPUTS="$OUTPUTS"

# C3: the flows, and a route through the input's own receiver.
[[ "$(json "$WEB/api/v1/flows" "[f['label'] for f in d if f['flow_id'] == '$FLOW'][0]")" == "Test tone" ]] || fail "the tone flow is not listed"
api PUT /api/v1/inputs/1/route "{\"flow_id\": \"$FLOW\", \"domain_id\": \"$DOMAIN\"}" >/dev/null
wait_until 5 "input 1 running" is_state 1 running
[[ "$(json "$NMOS/x-nmos/connection/v1.2/single/receivers/$(json "$NMOS/x-nmos/node/v1.3/receivers" "[r['id'] for r in d if r['label'] == 'Input 1'][0]")/active" "d['transport_params'][0]['mxl_flow_id']")" == "$FLOW" ]] || fail "the receiver's active flow"
pass "flows listed; input 1 routed through IS-05"
# C2: the connected name (the MXL flow's label here, no registry).
connected() { [[ "$(input 1 connected_label)" == "Test tone" ]]; }
wait_until 5 "connected label" connected
pass "input 1 shows what is connected: Test tone"

[[ "$(json "$WEB/api/v1/virtual-outputs" "len(d)")" == 3 ]] || fail "three virtual outputs"
[[ "$(json "$WEB/api/v1/virtual-outputs/3" "d['label'] + ' ' + d['format']")" == "Output 3 mono" ]] || fail "defaults of slot 3"
[[ "$(json "$WEB/api/v1/virtual-outputs/pgm" "d['slot']")" == 1 ]] || fail "PGM by its key"
OWN=$(json "$WEB/api/v1/status" "d['mxl']['output_domain_dir']")
PGM=$(json "$WEB/api/v1/virtual-outputs/1" "d['mxl_flow_id']")
CLEAN=$(json "$WEB/api/v1/virtual-outputs/2" "d['mxl_flow_id']")
SENDER=$(json "$WEB/api/v1/virtual-outputs/1" "d['nmos_sender_id']")
[[ -d "$OWN/$PGM.mxl-flow" && -d "$OWN/$CLEAN.mxl-flow" ]] || fail "no MXL flows for the virtual outputs"
[[ "$(json "$NMOS/x-nmos/node/v1.3/senders/$SENDER" "d['label']")" == PGM ]] || fail "the NMOS sender of PGM"
# The ids are the formulas the platform derives from NMOS_SEED.
python3 - "$PGM" "$SENDER" <<'EOF' || fail "the ids are not UUIDv5 of NMOS_SEED"
import sys, uuid
ns = uuid.UUID("8c76aff5-a8f9-53ce-97d0-766f1074b699")
node = uuid.uuid5(ns, "it-vouts")
assert sys.argv[1] == str(uuid.uuid5(ns, "it-vouts/output/vout:pgm/mxl-flow")), sys.argv[1]
assert sys.argv[2] == str(uuid.uuid5(node, "sender:out-vout:pgm")), sys.argv[2]
EOF
pass "3 virtual outputs: MXL flows and NMOS senders with stable ids"

# Silent before it has a source.
OUT_FLOW=$CLEAN
probe 1 1:1:0 2:1:0
pass "Clean without a source: silence (the flow runs)"

api POST /api/v1/strips '{"id": "vox", "name": "Vox", "format": "mono", "patch": [{"input": 1, "channel": 1}]}' >/dev/null
api POST /api/v1/buses '{"id": "main", "name": "Main", "format": "stereo"}' >/dev/null
api PUT /api/v1/sends/vox/main >/dev/null
api PUT /api/v1/virtual-outputs/2 '{"source": {"signal": "main"}}' >/dev/null
sleep 0.2
probe 1 1:1:0.70710677 2:1:0.70710677
pass "Clean from the main bus"
api PUT /api/v1/virtual-outputs/2 '{"source": {"signal": "vox"}}' >/dev/null
sleep 0.2
probe 1 1:1:0.70710677 2:1:0.70710677
pass "Clean from the vox direct out (mono into stereo, −3 dB)"
api PUT /api/v1/virtual-outputs/2 '{"source": {"input": 1}}' >/dev/null
sleep 0.2
probe 1 1:1:1 2:2:1
pass "Clean from input 1 straight through"
OUT_FLOW=$PGM
api PUT /api/v1/virtual-outputs/1 '{"source": {"signal": "main"}}' >/dev/null
sleep 0.2
probe 1 1:1:0.70710677 2:1:0.70710677 3:1:0 4:1:0 5:1:0 6:1:0
pass "PGM (5.1) from the stereo main bus: L, R; the rest silent"
api POST /api/v1/monitors '{"id": "cr", "name": "CR", "kind": "speakers", "format": "stereo", "source": {"signal": "main"}}' >/dev/null
api PUT /api/v1/virtual-outputs/1 '{"source": {"monitor": "cr"}}' >/dev/null
[[ "$(json "$WEB/api/v1/virtual-outputs/1" "d['source']['monitor']")" == cr ]] || fail "monitor source"
pass "PGM from the control-room monitor"

# Bad sources are refused; a deleted signal silences its outputs.
code=$(curl -s -o /dev/null -w '%{http_code}' -X PUT -H 'content-type: application/json' "$WEB/api/v1/virtual-outputs/2" -d '{"source": {"signal": "nope"}}')
[[ "$code" == 400 ]] || fail "an unknown signal answered $code"
code=$(curl -s -o /dev/null -w '%{http_code}' -X PUT -H 'content-type: application/json' "$WEB/api/v1/virtual-outputs/2" -d '{"source": {"signal": "main", "monitor": "cr"}}')
[[ "$code" == 400 ]] || fail "a mixed source answered $code"
code=$(curl -s -o /dev/null -w '%{http_code}' -X PUT -H 'content-type: application/json' "$WEB/api/v1/virtual-outputs/9" -d '{"source": null}')
[[ "$code" == 404 ]] || fail "slot 9 answered $code"
api PUT /api/v1/virtual-outputs/2 '{"source": {"signal": "vox"}}' >/dev/null
curl -fsS -X DELETE "$WEB/api/v1/strips/vox"
[[ "$(json "$WEB/api/v1/virtual-outputs/2" "d['source']")" == None ]] || fail "the deleted strip still feeds Clean"
curl -fsS "$WEB/metrics" | grep -q '^mxl_audio_mixer_virtual_output_routed{slot="1",output="pgm",label="PGM"} 1$' || fail "metric virtual_output_routed"
api PUT /api/v1/virtual-outputs/3 '{"source": {"input": 1, "format": "stereo"}}' >/dev/null
pass "bad sources refused, a deleted strip leaves silence, metrics"

# A restart with other outputs: PGM (keyed) moved to slot 2 keeps its flow id
# and its source; Clean is mono now; output 3 is gone, its source kept, silent.
rc=0
stop_mixer || rc=$?
[[ $rc == 143 ]] || fail "exit code $rc"
start_mixer NMOS_SEED=it-vouts MIXER_OUTPUTS='[{"label": "Clean", "format": "mono"}, {"key": "pgm", "label": "PGM", "format": "5.1"}]'
[[ "$(json "$WEB/api/v1/virtual-outputs/pgm" "d['slot']")" == 2 ]] || fail "PGM is not slot 2"
[[ "$(json "$WEB/api/v1/virtual-outputs/pgm" "d['mxl_flow_id']")" == "$PGM" ]] || fail "PGM changed its flow id"
[[ "$(json "$WEB/api/v1/virtual-outputs/pgm" "d['source']['monitor']")" == cr ]] || fail "PGM lost its source"
CLEAN=$(json "$WEB/api/v1/virtual-outputs/1" "d['mxl_flow_id']")
[[ "$(python3 -c "import json; print(json.load(open('$OWN/$CLEAN.mxl-flow/flow_def.json'))['channel_count'])")" == 1 ]] || fail "Clean is not mono"
[[ "$(json "$WEB/api/v1/mix" "[r['output'] for r in d['virtual_outputs']]")" == "['pgm', '3']" ]] || fail "the source of output 3 was not kept"
wait_until 5 "input 1 running again" is_state 1 running
pass "a restart with other outputs: keyed PGM keeps its flow id and source, Clean mono, output 3's source kept"

# Deleting the monitor silences PGM.
curl -fsS -X DELETE "$WEB/api/v1/monitors/cr"
[[ "$(json "$WEB/api/v1/virtual-outputs/pgm" "d['source']")" == None ]] || fail "the deleted monitor still feeds PGM"
pass "a deleted monitor leaves silence"

rc=0
stop_mixer || rc=$?
[[ $rc == 143 ]] || fail "exit code $rc"
pass vouts
