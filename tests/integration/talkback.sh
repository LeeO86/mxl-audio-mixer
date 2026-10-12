#!/usr/bin/env bash
# Talkback (§10.2) through MXL, sample-exact with mxl-mixer-probe:
# - an IFB bus (interrupt): the director's mic cuts the programme; the director
#   outranks the producer; the producer alone; mix mode adds talk to the
#   programme; a momentary talk ends when it is not repeated;
# - the control-room monitor dimmed while a control-room mic talks;
# - listen-back of the IFB's return on the monitor;
# - a strip on a talkback mic;
# - browser talkback: headless Chrome publishes a microphone by WHIP
#   (web/tests/talkback.mjs), the mixer reads it from the built-in MediaMTX and a
#   strip on it reaches an MXL output; its latency from the bursts on each second.
# Needs MEDIAMTX_BIN and Chrome for the browser part.
# shellcheck source=tests/integration/lib.sh
source "$(dirname "$0")/lib.sh"

MEDIAMTX_BIN="${MEDIAMTX_BIN:-$(command -v mediamtx || true)}"
[[ -x "$MEDIAMTX_BIN" ]] || fail "no MediaMTX binary (set MEDIAMTX_BIN)"
export MEDIAMTX_BIN
LATENCY_MAX_MS="${TALKBACK_LATENCY_MAX_MS:-150}"

FLOW=88888888-8888-4888-8888-888888888888
DOMAIN=99999999-9999-4999-8999-999999999999
start_tone "$WORK/mxl/tones" "$FLOW" --domain-id "$DOMAIN" --channels 4
wait_until 10 "tone flow" test -d "$WORK/mxl/tones/$FLOW.mxl-flow"
start_mixer NMOS_SEED=it-talkback
route 1 "$DOMAIN" "$FLOW"
wait_until 5 "input 1 running" is_state 1 running

# Programme: vox (tone 1) into the mono IFB bus and the stereo programme bus.
api POST /api/v1/strips '{"id": "vox", "format": "mono", "patch": [{"input": 1, "channel": 1}]}' >/dev/null
api POST /api/v1/buses '{"id": "ifb", "name": "IFB Presenter", "format": "mono"}' >/dev/null
api POST /api/v1/buses '{"id": "pgm", "name": "Programme", "format": "stereo"}' >/dev/null
api PUT /api/v1/sends/vox/ifb '{"tap": "post-fader"}' >/dev/null
api PUT /api/v1/sends/vox/pgm >/dev/null
api PUT /api/v1/outputs/ifb >/dev/null
api POST /api/v1/monitors '{"id": "main", "name": "Main", "format": "stereo", "source": {"signal": "pgm"}, "publish": true}' >/dev/null
# The director (tone 2, priority 5) and the producer (tone 3, priority 1, not in the control room).
api POST /api/v1/talkback/mics '{"id": "dir", "name": "Director", "input": 1, "channel": 2, "priority": 5}' >/dev/null
api POST /api/v1/talkback/mics '{"id": "prod", "name": "Producer", "input": 1, "channel": 3, "priority": 1, "dims_control_room": false}' >/dev/null
api POST /api/v1/talkback/destinations '{"id": "ifb", "name": "Presenter", "bus": "ifb", "mode": "interrupt", "return": {"input": 1}}' >/dev/null
OWN=$(json "$WEB/api/v1/status" "d['mxl']['output_domain_dir']")
IFB=$(json "$WEB/api/v1/outputs/ifb" "d['mxl_flow_id']")
MON=$(json "$WEB/api/v1/status" "[o['mxl_flow_id'] for o in d['outputs'] if o['signal'] == 'monitor:main'][0]")
H=0.70710677
DIMMED=0.063018    # 0.7071 at −21 dB
on() { api PUT "/api/v1/talkback/talk/$1/$2" "${3:-}" >/dev/null; sleep 0.2; }
off() { curl -fsS -o /dev/null -X DELETE "$WEB/api/v1/talkback/talk/$1/$2"; sleep 0.2; }

OUT_FLOW=$IFB
settled 1
probe 1 1:1:1
pass "IFB without talk: the programme"
on dir ifb
probe 1 1:2:1
OUT_FLOW=$MON probe 1 1:1:$DIMMED 2:1:$DIMMED
pass "the director talks: the IFB's programme cut, the director heard; the monitor dimmed by 21 dB"
on prod ifb
probe 1 1:2:1
off dir ifb
probe 1 1:3:1
OUT_FLOW=$MON probe 1 1:1:$H 2:1:$H
pass "priority: the director outranks the producer; the producer alone is heard and does not dim the monitor"
api PATCH /api/v1/talkback/destinations/ifb '{"mode": "mix"}' >/dev/null
api PATCH /api/v1/strips/vox '{"mute": true}' >/dev/null
sleep 0.3
probe 1 1:3:1
off prod ifb
probe 1 1:3:0
api PATCH /api/v1/strips/vox '{"mute": false}' >/dev/null
sleep 0.3
on prod ifb
level=$(probe 1 >/dev/null; python3 -c "import json,sys; print(json.loads(sys.argv[1])['levels'][0]['rms_dbfs'])" "$PROBED")
off prod ifb
# Two tones of 0.1 peak: −20 dBFS RMS together (each −23).
python3 -c "import sys; sys.exit(0 if abs($level + 20.0) < 0.2 else 1)" || fail "mix mode: programme and talk at $level dBFS RMS"
pass "mix mode: the producer over the programme (−20 dBFS RMS together), the producer alone with the programme muted"

api PATCH /api/v1/talkback/destinations/ifb '{"mode": "interrupt"}' >/dev/null
on dir ifb '{"momentary": true}'
probe 1 1:2:1
sleep 4.5
[[ "$(json "$WEB/api/v1/talkback" "d['talking']")" == "{}" ]] || fail "momentary talk still on: $(curl -fsS "$WEB/api/v1/talkback")"
probe 1 1:1:1
pass "a momentary talk not repeated ended after 3 s"

OUT_FLOW=$MON
api PUT /api/v1/listen-back '{"destination": "ifb"}' >/dev/null
sleep 0.3
probe 1 1:1:1 2:2:1
curl -fsS -o /dev/null -X DELETE "$WEB/api/v1/listen-back"
sleep 0.3
probe 1 1:1:$H 2:1:$H
pass "listen-back: the monitor plays the IFB's return (input 1), then the programme again"

api POST /api/v1/buses '{"id": "rec", "name": "Recorder", "format": "mono"}' >/dev/null
api POST /api/v1/strips '{"id": "tb", "format": "mono", "patch": [{"talkback": "dir"}]}' >/dev/null
api PUT /api/v1/sends/tb/rec '{"tap": "post-fader"}' >/dev/null
api PUT /api/v1/outputs/rec >/dev/null
OUT_FLOW=$(json "$WEB/api/v1/outputs/rec" "d['mxl_flow_id']")
sleep 0.3
probe 1 1:2:1
pass "a strip on the director's talkback mic: sample-exact"

# --- browser talkback ---
api POST /api/v1/strips '{"id": "remote", "format": "mono", "patch": [{"talkback": "browser-1"}]}' >/dev/null
api POST /api/v1/buses '{"id": "web", "name": "Web talkback", "format": "mono"}' >/dev/null
api PUT /api/v1/sends/remote/web >/dev/null
api PUT /api/v1/outputs/web >/dev/null
OUT_FLOW=$(json "$WEB/api/v1/outputs/web" "d['mxl_flow_id']")
node "$ROOT/web/tests/talkback.mjs" "$WEB" it-browser 14 >"$WORK/talkback.json" 2>>"$IT_OUT/talkback-browser.log" &
BROWSER=$!
PIDS+=("$BROWSER")
browser_receiving() { [[ "$(json "$WEB/api/v1/talkback" "[m['state'] for m in d['browser_mics']]")" == "['receiving']" ]]; }
wait_until 20 "the browser mic received" browser_receiving
sleep 2
out=$("$PROBE_BIN" --domain "$OWN" --flow "$OUT_FLOW" --seconds 8 --onsets 0.1)
echo "$out" >>"$IT_OUT/talkback-probe.log"
wait "$BROWSER" || fail "web/tests/talkback.mjs failed"
log "browser: $(cat "$WORK/talkback.json")"
log "mic: $(json "$WEB/api/v1/talkback" "d['browser_mics']")"
python3 - "$out" "$LATENCY_MAX_MS" <<'EOF' || fail "browser talkback: $out"
import json, sys
d = json.loads(sys.argv[1])
ms = sorted(o["ms_in_second"] for o in d["onsets"])
assert len(ms) >= 6, f"{len(ms)} bursts in 8 s"
assert d["levels"][0]["peak_dbfs"] > -10, d["levels"]
median = ms[len(ms) // 2]
print(f"[talkback.sh] browser talkback latency: median {median:.1f} ms, {ms[0]:.1f}–{ms[-1]:.1f} ms over {len(ms)} bursts")
assert median <= float(sys.argv[2]), f"median {median} ms"
EOF
pass "browser talkback: WHIP from Chrome, read and decoded by the mixer, a strip on it in an MXL output, a burst every second"
wait_until 40 "the browser mic removed when it stops registering" bash -c "[[ \$(curl -fsS $WEB/api/v1/talkback | python3 -c 'import json,sys; print(len(json.load(sys.stdin)[\"browser_mics\"]))') == 0 ]]"
pass "the browser mic removed 30 s after its last registration"

rc=0
stop_mixer || rc=$?
[[ $rc == 143 ]] || fail "exit code $rc"
pass talkback
