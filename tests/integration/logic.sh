#!/usr/bin/env bash
# The logic editor (§11) end to end through MXL: an HTTP trigger through a
# toggle mutes a strip; a soft key through a select ramps its fader (sample-exact
# on the output); the same key publishes a bus, sends TSL UMD 5.0 to a UDP
# listener and posts a webhook; a log block reaches the events WebSocket; dry
# run lists without acting and applies when it ends; a macro (batch endpoint)
# mutes a bus; type errors answer 400 with the block and port; a stale revision
# answers 409; logic_actions_total counts.
# shellcheck source=tests/integration/lib.sh
source "$(dirname "$0")/lib.sh"

FLOW=a1a1a1a1-a1a1-4a1a-8a1a-a1a1a1a1a1a1
DOMAIN=b2b2b2b2-b2b2-4b2b-8b2b-b2b2b2b2b2b2
TSL_PORT=${LOGIC_TEST_TSL:-8930}
HOOK_PORT=${LOGIC_TEST_HOOK:-8931}
start_tone "$WORK/mxl/tones" "$FLOW" --domain-id "$DOMAIN" --channels 2
wait_until 10 "tone flow" test -d "$WORK/mxl/tones/$FLOW.mxl-flow"
start_mixer NMOS_SEED=it-logic TSL_OUT_TARGET="127.0.0.1:$TSL_PORT" TSL_OUT_SCREEN=4 MIXER_TEST_LOGIC_SWAP_DELAY_MS=400
route 1 "$DOMAIN" "$FLOW"
wait_until 5 "input 1 running" is_state 1 running

# A webhook receiver and a TSL listener, each writing what it got last.
python3 - "$HOOK_PORT" "$WORK/hook.json" <<'EOF' &
import http.server, sys
port, path = int(sys.argv[1]), sys.argv[2]
class H(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("content-length", 0)))
        with open(path + ".tmp", "wb") as f:
            f.write(body)
        import os
        os.replace(path + ".tmp", path)
        self.send_response(204)
        self.end_headers()
    def log_message(self, *a):
        pass
http.server.HTTPServer(("127.0.0.1", port), H).serve_forever()
EOF
PIDS+=("$!")
python3 - "$TSL_PORT" "$WORK/tsl.json" <<'EOF' &
import json, os, socket, struct, sys
port, path = int(sys.argv[1]), sys.argv[2]
s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
s.bind(("127.0.0.1", port))
count = 0
while True:
    p, _ = s.recvfrom(65536)
    count += 1
    _, _, flags, screen = struct.unpack_from("<HBBH", p)
    i, out = 6, []
    while i + 6 <= len(p):
        index, control, n = struct.unpack_from("<HHH", p, i)
        label = p[i + 6:i + 6 + n].decode("utf-16-le" if flags & 1 else "ascii")
        out.append({"screen": screen, "index": index, "lh": (control >> 4) & 3, "rh": control & 3, "label": label})
        i += 6 + n
    with open(path + ".tmp", "w") as f:
        json.dump({"packets": count, "displays": out}, f)
    os.replace(path + ".tmp", path)
EOF
PIDS+=("$!")

# vox (tone 1, −20 dBFS) hard left into a stereo bus; an aux bus to publish.
api POST /api/v1/strips '{"id": "vox", "format": "mono", "pan": -1, "patch": [{"input": 1, "channel": 1}]}' >/dev/null
api POST /api/v1/buses '{"id": "main", "name": "Main", "format": "stereo"}' >/dev/null
api POST /api/v1/buses '{"id": "aux", "name": "Aux", "format": "stereo"}' >/dev/null
api PUT /api/v1/sends/vox/main >/dev/null
api PUT /api/v1/outputs/main >/dev/null
OUT_FLOW=$(json "$WEB/api/v1/outputs/main" "d['mxl_flow_id']")
OWN=$(json "$WEB/api/v1/status" "d['mxl']['output_domain_dir']")
sleep 0.3
probe 1 1:1:1 2:1:0
pass "vox reaches main"

api PUT /api/v1/logic/show '{
  "name": "Show",
  "blocks": [
    {"id": "go", "type": "trigger"},
    {"id": "t", "type": "toggle"},
    {"id": "mute", "type": "set-mute", "signal": "vox"},
    {"id": "soft", "type": "button"},
    {"id": "zero", "type": "const", "value": 0},
    {"id": "down", "type": "const", "value": -6.0206},
    {"id": "sel", "type": "select", "inputs": 2, "value_type": "level"},
    {"id": "fader", "type": "set-fader", "signal": "vox", "ramp_ms": 0},
    {"id": "pub", "type": "set-publish", "signal": "aux"},
    {"id": "label", "type": "text", "value": "LOGIC"},
    {"id": "tsl", "type": "tsl-out", "index": 7},
    {"id": "hook", "type": "webhook", "url": "http://127.0.0.1:'"$HOOK_PORT"'/hook"},
    {"id": "log", "type": "log", "message": "marker"}
  ],
  "wires": [
    {"from_block": "go", "from_port": "out", "to_block": "t", "to_port": "in"},
    {"from_block": "t", "from_port": "out", "to_block": "mute", "to_port": "in"},
    {"from_block": "soft", "from_port": "out", "to_block": "sel", "to_port": "sel"},
    {"from_block": "zero", "from_port": "out", "to_block": "sel", "to_port": "in0"},
    {"from_block": "down", "from_port": "out", "to_block": "sel", "to_port": "in1"},
    {"from_block": "sel", "from_port": "out", "to_block": "fader", "to_port": "level"},
    {"from_block": "soft", "from_port": "out", "to_block": "pub", "to_port": "in"},
    {"from_block": "soft", "from_port": "out", "to_block": "tsl", "to_port": "left"},
    {"from_block": "label", "from_port": "out", "to_block": "tsl", "to_port": "label"},
    {"from_block": "soft", "from_port": "out", "to_block": "hook", "to_port": "in"},
    {"from_block": "go", "from_port": "out", "to_block": "log", "to_port": "in"},
    {"from_block": "sel", "from_port": "out", "to_block": "log", "to_port": "value"}
  ]
}' >/dev/null
[[ "$(json "$WEB/api/v1/logic/show" "d['ports']['sel']['outputs'][0]['type']")" == level ]] || fail "ports"
pass "graph saved with typed ports"

strip() { json "$WEB/api/v1/strips/vox" "$1"; }
# is <url> <python expression> <value>: evaluated on every try of wait_until.
is() { [[ "$(json "$1" "$2")" == "$3" ]]; }
VOX="$WEB/api/v1/strips/vox"
api POST /api/v1/logic/show/trigger/go >/dev/null
wait_until 3 "vox muted" is "$VOX" "d['mute']" True
sleep 0.1
probe 1 1:1:0 2:1:0
api POST /api/v1/logic/show/trigger/go >/dev/null
wait_until 3 "vox unmuted" is "$VOX" "d['mute']" False
pass "HTTP trigger → toggle → set-mute: muted and unmuted"

api PUT /api/v1/logic/show/buttons/soft '{"pressed": true}' >/dev/null
wait_until 3 "vox fader -6 dB" is "$VOX" "round(d['fader_db'], 3)" -6.021
sleep 0.2
probe 1 1:1:0.5 2:1:0
pass "soft key → select → set-fader: −6.02 dB on the output, sample-exact"

wait_until 5 "aux published" is "$WEB/api/v1/outputs" "any(o['signal'] == 'aux' for o in d)" True
pass "soft key → set-publish: aux published (the plan swapped)"

wait_until 3 "TSL packet" test -s "$WORK/tsl.json"
tsl=$(cat "$WORK/tsl.json")
python3 - "$tsl" <<'EOF' || fail "TSL: $tsl"
import json, sys
d = json.loads(sys.argv[1])["displays"]
assert d == [{"screen": 4, "index": 7, "lh": 1, "rh": 0, "label": "LOGIC"}], d
EOF
sleep 1.2
again=$(python3 -c "import json; print(json.load(open('$WORK/tsl.json'))['packets'])")
((again >= 2)) || fail "TSL is not repeated: $again packets"
pass "soft key → tsl-out: screen 4 index 7 red, label LOGIC; repeated ($again packets)"

wait_until 3 "webhook" test -s "$WORK/hook.json"
python3 -c "import json; d = json.load(open('$WORK/hook.json')); assert d == {'graph': 'show', 'block': 'hook', 'value': 1}, d" || fail "webhook body $(cat "$WORK/hook.json")"
pass "soft key → webhook: posted {graph, block, value}"

# The log block on the events socket.
events=$(node --input-type=module - "$WEB" <<'EOF'
const [base] = process.argv.slice(2);
const ws = new WebSocket(base.replace(/^http/, "ws") + "/api/v1/events");
const timer = setTimeout(() => { console.log("timeout"); process.exit(1); }, 5000);
ws.onopen = async () => {
  await new Promise((r) => setTimeout(r, 300));
  await fetch(`${base}/api/v1/logic/show/trigger/go`, { method: "POST" });
};
ws.onmessage = (ev) => {
  const m = JSON.parse(ev.data);
  if (m.type === "logic" && m.log) {
    clearTimeout(timer);
    console.log(JSON.stringify(m));
    process.exit(0);
  }
};
EOF
) || fail "events: $events"
python3 -c "import json,sys; m = json.loads(sys.argv[1]); assert m['log'] == 'marker -6.02' and m['graph'] == 'show' and m['block'] == 'log' and not m['dry_run'], m" "$events" || fail "log event $events"
pass "trigger → log: \"marker -6.02\" on the events WebSocket"
wait_until 3 "vox muted again" is "$VOX" "d['mute']" True
api POST /api/v1/logic/show/trigger/go >/dev/null
wait_until 3 "vox unmuted again" is "$VOX" "d['mute']" False

# Dry run: listed, not applied; applied when it ends.
api PATCH /api/v1/logic/show '{"dry_run": true}' >/dev/null
api PUT /api/v1/logic/show/buttons/soft '{"pressed": false}' >/dev/null
wait_until 3 "dry-run actions" is "$WEB/api/v1/logic/show" "any(a['dry_run'] and a['block'] == 'fader' for a in d['actions'])" True
sleep 0.3
[[ "$(strip "round(d['fader_db'], 3)")" == -6.021 ]] || fail "dry run moved the fader"
[[ "$(json "$WEB/api/v1/outputs" "any(o['signal'] == 'aux' for o in d)")" == True ]] || fail "dry run unpublished"
pass "dry run: actions listed, nothing applied"
api PATCH /api/v1/logic/show '{"dry_run": false}' >/dev/null
wait_until 3 "fader back" is "$VOX" "round(d['fader_db'], 3)" 0.0
wait_until 5 "aux unpublished" is "$WEB/api/v1/outputs" "any(o['signal'] == 'aux' for o in d)" False
sleep 0.2
probe 1 1:1:1 2:1:0
pass "dry run off: the graph's state applied (fader 0 dB, aux unpublished)"

# A macro and the graph that uses it, in one batch: not(key) mutes aux.
api POST /api/v1/logic-batch '{
  "macros": [{"id": "inv", "name": "Invert", "blocks": [
    {"id": "i", "type": "macro-in", "name": "x", "value_type": "boolean"},
    {"id": "n", "type": "not"},
    {"id": "o", "type": "macro-out", "name": "y", "value_type": "boolean"}],
    "wires": [
    {"from_block": "i", "from_port": "out", "to_block": "n", "to_port": "in"},
    {"from_block": "n", "from_port": "out", "to_block": "o", "to_port": "in"}]}],
  "graphs": [{"id": "aux-mute", "name": "Aux", "blocks": [
    {"id": "key", "type": "button"},
    {"id": "m1", "type": "macro", "macro": "inv"},
    {"id": "set", "type": "set-mute", "signal": "aux"}],
    "wires": [
    {"from_block": "key", "from_port": "out", "to_block": "m1", "to_port": "x"},
    {"from_block": "m1", "from_port": "y", "to_block": "set", "to_port": "in"}]}]
}' >/dev/null
wait_until 3 "aux muted" is "$WEB/api/v1/buses/aux" "d['mute']" True
api PUT /api/v1/logic/aux-mute/buttons/key '{"pressed": true}' >/dev/null
wait_until 3 "aux unmuted" is "$WEB/api/v1/buses/aux" "d['mute']" False
[[ "$(json "$WEB/api/v1/logic/aux-mute" "d['values']['m1.n']")" == False ]] || fail "values inside the macro"
[[ "$(json "$WEB/api/v1/logic-macros/inv" "d['exposed']['inputs'][0]['name']")" == x ]] || fail "exposed ports"
pass "batch: a macro and its graph; not(key) mutes aux, values as m1.n"

# A logic change that rewires the plan, and an API change inside its fade
# (MIXER_TEST_LOGIC_SWAP_DELAY_MS widens the fade to 400 ms): both stay, because
# they share the change lock. Before D167 the API's change replaced the logic's
# staged plan, and the unpublish was lost.
api POST /api/v1/buses '{"id": "aux2", "name": "Aux 2", "format": "stereo"}' >/dev/null
api POST /api/v1/buses '{"id": "aux3", "name": "Aux 3", "format": "stereo"}' >/dev/null
api PUT /api/v1/logic/race '{"name": "Race", "blocks": [{"id": "key", "type": "button"}, {"id": "pub", "type": "set-publish", "signal": "aux2"}],
  "wires": [{"from_block": "key", "from_port": "out", "to_block": "pub", "to_port": "in"}]}' >/dev/null
both="sorted(o['signal'] for o in d if o['signal'] in ('aux2', 'aux3'))"
for round in 1 2 3; do
    api PUT /api/v1/logic/race/buttons/key '{"pressed": true}' >/dev/null
    wait_until 5 "aux2 published (round $round)" is "$WEB/api/v1/outputs" "$both" "['aux2']"
    sleep 0.3
    # The logic unpublishes aux2 (a fade, then the swap 400 ms later) ...
    api PUT /api/v1/logic/race/buttons/key '{"pressed": false}' >/dev/null
    sleep 0.1
    # ... and the API publishes aux3 meanwhile.
    api PUT /api/v1/outputs/aux3 >/dev/null
    wait_until 5 "aux2 unpublished, aux3 published (round $round)" is "$WEB/api/v1/outputs" "$both" "['aux3']"
    api DELETE /api/v1/outputs/aux3 >/dev/null
done
pass "a logic unpublish and an API publish inside its fade, 3 times: neither lost"

# Refusals.
code=$(curl -s -o "$WORK/bad.json" -w '%{http_code}' -X PUT -H 'content-type: application/json' "$WEB/api/v1/logic/bad" \
    -d '{"name": "Bad", "blocks": [{"id": "s", "type": "text", "value": "x"}, {"id": "n", "type": "not"}], "wires": [{"from_block": "s", "from_port": "out", "to_block": "n", "to_port": "in"}]}')
[[ "$code" == 400 ]] || fail "type error answered $code"
python3 -c "import json; i = json.load(open('$WORK/bad.json'))['issues'][0]; assert (i['graph'], i['block'], i['port'], i['wire']) == ('bad', 'n', 'in', 0), i" || fail "issue $(cat "$WORK/bad.json")"
code=$(curl -s -o /dev/null -w '%{http_code}' -X PATCH -H 'content-type: application/json' "$WEB/api/v1/logic/show" -d '{"revision": "0000000000000000", "name": "Late"}')
[[ "$code" == 409 ]] || fail "stale revision answered $code"
rev=$(json "$WEB/api/v1/logic/show" "d['revision']")
api PATCH /api/v1/logic/show "{\"revision\": \"$rev\", \"name\": \"Show 2\"}" >/dev/null
code=$(curl -s -o /dev/null -w '%{http_code}' -X DELETE "$WEB/api/v1/logic-macros/inv")
[[ "$code" == 400 ]] || fail "deleting a used macro answered $code"
pass "a string into a boolean: 400 naming block n, port in; stale revision 409; a used macro stays"

actions=$(curl -fsS "$WEB/metrics" | sed -n 's/^mxl_audio_mixer_logic_actions_total{graph="show"} \(.*\)/\1/p')
((actions >= 8)) || fail "logic_actions_total{graph=\"show\"} $actions"
pass "logic_actions_total{graph=\"show\"} $actions"

# mix.json: a fader change is written within 0.5 s, not at once; SIGTERM
# writes what waits.
saved() { python3 -c "import json; print(json.load(open('$WORK/state/mix.json'))['mix']['strips'][0]['fader_db'])"; }
api PATCH /api/v1/strips/vox '{"fader_db": -3}' >/dev/null
[[ "$(saved)" != -3.0 ]] || fail "mix.json was written at once"
sleep 0.7
[[ "$(saved)" == -3.0 ]] || fail "mix.json not written 0.7 s after the change: $(saved)"
api PATCH /api/v1/strips/vox '{"fader_db": -4}' >/dev/null
rc=0
stop_mixer || rc=$?
[[ $rc == 143 ]] || fail "exit code $rc"
[[ "$(saved)" == -4.0 ]] || fail "SIGTERM did not write the waiting change: $(saved)"
pass "mix.json coalesced: a fader change written within 0.5 s, the last one at SIGTERM"
pass logic
