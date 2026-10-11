#!/usr/bin/env bash
# Milestone 4 UI (§12) in headless Chrome against the mixer on MXL: web/tests/ui.mjs
# (two clients, actions through the UI checked on the API). Needs `npm ci` in web/
# and Chrome (CHROME, default /usr/bin/google-chrome). Screenshots: $IT_OUT/ui/.
# shellcheck source=tests/integration/lib.sh
source "$(dirname "$0")/lib.sh"

FLOW=88888888-8888-4888-8888-888888888888
DOMAIN=99999999-9999-4999-8999-999999999999
start_tone "$WORK/mxl/tones" "$FLOW" --domain-id "$DOMAIN" --channels 2
wait_until 10 "tone flow" test -d "$WORK/mxl/tones/$FLOW.mxl-flow"
start_mixer NMOS_SEED=it-ui
route 1 "$DOMAIN" "$FLOW"
wait_until 5 "input 1 running" is_state 1 running

grep -q 'id="app"' <(curl -fsS "$WEB/") || fail "the binary has no UI build (cd web && npm ci && npm run build, then cargo build)"
api POST /api/v1/strips '{"id": "vox", "name": "Vox", "format": "mono", "colour": "#4da3ff", "patch": [{"input": 1, "channel": 1}]}' >/dev/null
api POST /api/v1/strips '{"id": "gtr", "name": "Guitar", "format": "mono", "pan": 0.3, "patch": [{"input": 1, "channel": 2}]}' >/dev/null
api POST /api/v1/buses '{"id": "main", "name": "Main", "format": "stereo", "loudness": true}' >/dev/null
api POST /api/v1/buses '{"id": "aux", "name": "Aux", "format": "mono"}' >/dev/null
api PUT /api/v1/sends/vox/main >/dev/null
api PUT /api/v1/sends/gtr/main >/dev/null
api PUT /api/v1/outputs/main >/dev/null

node "$ROOT/web/tests/ui.mjs" "$WEB" "$IT_OUT/ui" || fail "web/tests/ui.mjs"

rc=0
stop_mixer || rc=$?
[[ $rc == 143 ]] || fail "exit code $rc"
pass ui
