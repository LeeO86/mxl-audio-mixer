#!/usr/bin/env bash
# Two mixers (§13): the gallery rides the far-side desk, then builds the mix-minus
# return and the far-side in-ears. Audio stays on MXL; this checks the control.
# shellcheck source=tests/integration/lib.sh
source "$(dirname "$0")/lib.sh"

export LD_LIBRARY_PATH="/opt/mxl/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
# A stopped mixer would not take the SIGTERM of the cleanup.
trap 'kill -CONT "${PIDS[@]}" 2>/dev/null || true; cleanup' EXIT
TOKEN="link-token-test"
FAR="$WORK/far"
HOME_DIR="$WORK/home"
mkdir -p "$FAR/mxl" "$FAR/state" "$HOME_DIR/mxl" "$HOME_DIR/state"
FAR_WEB="http://127.0.0.1:8191"
HOME_WEB="http://127.0.0.1:8190"

boot() { # <host id> <env...>
    local name=$1
    shift
    env HOST_ID="$name" NMOS_HOST_ADDRESS="$HOST_IP" MIXER_INPUT_FLOWS=2 "$@" \
        "$MIXER_BIN" >>"$IT_OUT/link-$name.log" 2>&1 &
    PIDS+=("$!")
}

boot far \
    WEB_PORT=8191 NMOS_PORT=3352 \
    MXL_DOMAIN_SCAN_PATH="$FAR/mxl" MIXER_STATE_DIR="$FAR/state" MXL_OUTPUT_DOMAIN_DIR="$FAR/mxl/out" \
    MEDIAMTX_RTSP_PORT=8974 MEDIAMTX_WHEP_PORT=8619 MEDIAMTX_ICE_UDP_PORT=8219 \
    MIXER_LINK_ROLE=slave MIXER_LINK_TOKEN="$TOKEN" MIXER_LINK_TAKEOVER=deny
FAR_PID=${PIDS[-1]}
boot gallery \
    WEB_PORT=8190 NMOS_PORT=3342 \
    MXL_DOMAIN_SCAN_PATH="$HOME_DIR/mxl" MIXER_STATE_DIR="$HOME_DIR/state" MXL_OUTPUT_DOMAIN_DIR="$HOME_DIR/mxl/out" \
    MEDIAMTX_RTSP_PORT=8964 MEDIAMTX_WHEP_PORT=8609 MEDIAMTX_ICE_UDP_PORT=8209 \
    MIXER_LINK_ROLE=master MIXER_LINK_TOKEN="$TOKEN" \
    MIXER_LINK_PEERS="ws://127.0.0.1:8191/api/v1/link"
GALLERY_PID=${PIDS[-1]}

wait_until 40 "far side" http_ok "$FAR_WEB/livez"
wait_until 40 "gallery" http_ok "$HOME_WEB/livez"

post() { # <base> <path> <body>
    curl -fsS -X POST -H 'content-type: application/json' -d "$3" "$1$2" >/dev/null \
        || fail "POST $2 failed"
}
for id in presenter guest-a guest-b gallery; do
    post "$FAR_WEB" /api/v1/strips "{\"id\":\"$id\",\"name\":\"$id\",\"format\":\"mono\"}"
done
post "$HOME_WEB" /api/v1/buses '{"id":"main","name":"Main","format":"stereo"}'
post "$HOME_WEB" /api/v1/strips '{"id":"far-mix","name":"Far mix","format":"stereo"}'

wait_until 15 "far-side desk on the gallery" \
    bash -c "curl -fsS '$HOME_WEB/api/v1/remotes' | grep -q presenter"
post "$HOME_WEB" /api/v1/remotes/s1/command '{"op":"fader","signal":"presenter","db":-12}'
wait_until 8 "presenter fader on the far side" \
    bash -c "curl -fsS '$FAR_WEB/api/v1/strips/presenter' | grep -q '\"fader_db\":-12'"

post "$HOME_WEB" /api/v1/remotes/s1/return \
    '{"of":"main","without":["far-mix"],"return_strip":"gallery","voices":["presenter","guest-a","guest-b"]}'
wait_until 8 "in-ear for the presenter" http_ok "$FAR_WEB/api/v1/buses/iem-presenter"
[[ "$(json "$FAR_WEB/api/v1/buses/iem-presenter" "d['compensate']")" == "False" ]] || fail "the in-ear is delay-compensated"
python3 - "$FAR_WEB" <<'PY' || fail "the in-ear is not the other voices plus the return"
import json, sys, urllib.request
base = sys.argv[1]
sends = json.load(urllib.request.urlopen(base + "/api/v1/sends"))
ears = [s["from"] for s in sends if s["to"] == "iem-presenter"]
assert sorted(ears) == ["gallery", "guest-a", "guest-b"], ears
assert all(s["from"] != "presenter" for s in sends if s["to"] == "iem-presenter")
PY
[[ "$(json "$HOME_WEB/api/v1/buses/ret-s1" "d['minus']['of'], d['minus']['without']")" == "main ['far-mix']" ]] \
    || fail "the gallery return is not main minus the far side: $(curl -fsS "$HOME_WEB/api/v1/buses/ret-s1")"
http_ok "$HOME_WEB/api/v1/outputs/ret-s1" || fail "the return is not published"

post "$HOME_WEB" /api/v1/remotes/s1/command \
    '{"op":"send","from":"guest-a","to":"iem-presenter","db":-6,"mute":false}'
wait_until 8 "aux send" \
    bash -c "curl -fsS '$FAR_WEB/api/v1/sends/guest-a/iem-presenter' | grep -q '\"gain_db\":-6'"
# Building again keeps the level the gallery set.
post "$HOME_WEB" /api/v1/remotes/s1/return \
    '{"of":"main","without":["far-mix"],"return_strip":"gallery","voices":["presenter","guest-a","guest-b"]}'
sleep 0.4
[[ "$(json "$FAR_WEB/api/v1/sends/guest-a/iem-presenter" "d['gain_db']")" == "-6.0" || "$(json "$FAR_WEB/api/v1/sends/guest-a/iem-presenter" "d['gain_db']")" == "-6" ]] \
    || fail "the in-ear send was reset: $(curl -fsS "$FAR_WEB/api/v1/sends/guest-a/iem-presenter")"
[[ "$(json "$HOME_WEB/api/v1/outputs" "len([o for o in d if o['signal']=='ret-s1'])")" == "1" ]] \
    || fail "the return was published twice"

pass "gallery rides the far side; the return is main minus the far side; each in-ear is the other voices plus that return"

FAR_WS="ws://127.0.0.1:8191/api/v1/link"
peer() { node "$(dirname "$0")/link_peer.mjs" "$@"; }
far_up() { [[ "$(json "$FAR_WEB/api/v1/status" "d['link']['up']")" == "$1" ]]; }
gallery_up() { [[ "$(json "$HOME_WEB/api/v1/remotes" "d['peers'][0]['up']")" == "$1" ]]; }
metric() { curl -fsS "$1/metrics" | grep -qF "mxl_audio_mixer_remote_link_up{peer=\"$2\"} $3"; }
local_fader() { # <dB>; prints the HTTP status
    curl -sS -o /dev/null -w '%{http_code}' -X PATCH -H 'content-type: application/json' \
        -d "{\"fader_db\": $1}" "$FAR_WEB/api/v1/strips/presenter"
}
closed() { # <peer output> <python condition on d>
    python3 -c "import json,sys; d=json.loads(sys.argv[1]); sys.exit(0 if ($2) else 1)" "$1"
}

rtt=$(json "$HOME_WEB/api/v1/remotes" "d['peers'][0]['rtt_ms']")
python3 -c "import sys; sys.exit(0 if 0 <= float(sys.argv[1]) < 1000 else 1)" "$rtt" \
    || fail "the gallery's round trip is not measured on its own clock: $rtt"
metric "$FAR_WEB" master 1 || fail "the far side does not report its master up"
metric "$HOME_WEB" s1 1 || fail "the gallery does not report s1 up"

out=$(peer master "$FAR_WS" "$TOKEN")
closed "$out" "d['why'] == 'closed' and d['messages'][:1] == [{'type': 'ack', 'seq': 0, 'ok': False, 'error': 'another master is connected'}]" \
    || fail "a second master was not refused: $out"
far_up True || fail "the refused master took the link down"
out=$(peer silent "$FAR_WS")
closed "$out" "d['why'] == 'closed' and 4 <= d['closed_after_s'] < 10" \
    || fail "a connection without hello was not closed after 5 s: $out"
out=$(peer big "$FAR_WS")
closed "$out" "d['why'] == 'closed' and d['closed_after_s'] < 4" || fail "a 5 MiB message was taken: $out"
pass "one master at a time; no hello or an oversized message closes the socket"

# A gallery that hangs (no close, no pings): the far side drops it within 5 s,
# holds the mix and, with takeover deny, refuses local changes.
kill -STOP "$GALLERY_PID"
wait_until 9 "the far side to see the master gone" far_up False
metric "$FAR_WEB" master 0 || fail "remote_link_up{peer=\"master\"} is not 0 on the far side"
[[ "$(local_fader -3)" == 400 ]] || fail "takeover deny let a local change through"
[[ "$(json "$FAR_WEB/api/v1/strips/presenter" "d['fader_db']")" == "-12.0" ]] || fail "the far side did not hold its mix"
kill -CONT "$GALLERY_PID"
wait_until 20 "the gallery to reconnect" far_up True
wait_until 5 "s1 up on the gallery" gallery_up True
post "$HOME_WEB" /api/v1/remotes/s1/command '{"op":"fader","signal":"presenter","db":-20}'
wait_until 8 "the master's fader after the reconnect" \
    bash -c "curl -fsS '$FAR_WEB/api/v1/strips/presenter' | grep -q '\"fader_db\":-20'"
[[ "$(local_fader -3)" == 200 ]] || fail "a local change was refused with the master connected"

# A far side that hangs: the gallery drops it within 5 s and reconnects later.
kill -STOP "$FAR_PID"
wait_until 9 "the gallery to see s1 gone" gallery_up False
metric "$HOME_WEB" s1 0 || fail "remote_link_up{peer=\"s1\"} is not 0 on the gallery"
kill -CONT "$FAR_PID"
wait_until 25 "the gallery back on s1" gallery_up True
wait_until 5 "the far side back up" far_up True
pass "a hung peer is dropped within 5 s and the link comes back; takeover deny holds the far side meanwhile"
