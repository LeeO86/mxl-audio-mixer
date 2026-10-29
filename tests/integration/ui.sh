#!/usr/bin/env bash
# Milestone 4 UI (§12) in headless Chrome against the mixer on MXL: web/tests/ui.mjs
# (two clients, actions through the UI checked on the API), then the logic editor
# (§11.3): web/tests/logic.mjs, then the console's strip handling (1.4.0):
# web/tests/console.mjs, and the format, down-mix and spatial nodes of 1.4.0:
# web/tests/spatial.mjs, then the console of 1.5.0 (web/tests/console15.mjs) and
# its MIDI control with a fake Web MIDI (web/tests/midi.mjs). Needs `npm ci` in web/
# and Chrome (CHROME, default /usr/bin/google-chrome). Screenshots: $IT_OUT/ui/.
# shellcheck source=tests/integration/lib.sh
source "$(dirname "$0")/lib.sh"

FLOW=88888888-8888-4888-8888-888888888888
DOMAIN=99999999-9999-4999-8999-999999999999
start_tone "$WORK/mxl/tones" "$FLOW" --domain-id "$DOMAIN" --channels 2
wait_until 10 "tone flow" test -d "$WORK/mxl/tones/$FLOW.mxl-flow"
TSL_UDP=${TSL_TEST_UDP:-8930}
start_mixer NMOS_SEED=it-ui TSL_ENABLE=true TSL_UDP_PORT="$TSL_UDP" TSL_TCP_PORT=$((TSL_UDP + 1))
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

TSL_UDP="$TSL_UDP" node "$ROOT/web/tests/ui.mjs" "$WEB" "$IT_OUT/ui" || fail "web/tests/ui.mjs"
node "$ROOT/web/tests/logic.mjs" "$WEB" "$IT_OUT/ui" || fail "web/tests/logic.mjs"
node "$ROOT/web/tests/console.mjs" "$WEB" "$IT_OUT/ui" || fail "web/tests/console.mjs"
node "$ROOT/web/tests/spatial.mjs" "$WEB" "$IT_OUT/ui" || fail "web/tests/spatial.mjs"
node "$ROOT/web/tests/console15.mjs" "$WEB" "$IT_OUT/ui" || fail "web/tests/console15.mjs"
node "$ROOT/web/tests/midi.mjs" "$WEB" "$IT_OUT/ui" || fail "web/tests/midi.mjs"

rc=0
stop_mixer || rc=$?
[[ $rc == 143 ]] || fail "exit code $rc"
pass ui
