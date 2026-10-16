#!/usr/bin/env bash
# Two mixers (§13): the gallery rides the far-side desk, then builds the mix-minus
# return and the far-side in-ears. Audio stays on MXL; this checks the control.
# shellcheck source=tests/integration/lib.sh
source "$(dirname "$0")/lib.sh"

export LD_LIBRARY_PATH="/opt/mxl/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
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
boot gallery \
    WEB_PORT=8190 NMOS_PORT=3342 \
    MXL_DOMAIN_SCAN_PATH="$HOME_DIR/mxl" MIXER_STATE_DIR="$HOME_DIR/state" MXL_OUTPUT_DOMAIN_DIR="$HOME_DIR/mxl/out" \
    MEDIAMTX_RTSP_PORT=8964 MEDIAMTX_WHEP_PORT=8609 MEDIAMTX_ICE_UDP_PORT=8209 \
    MIXER_LINK_ROLE=master MIXER_LINK_TOKEN="$TOKEN" \
    MIXER_LINK_PEERS="ws://127.0.0.1:8191/api/v1/link"

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
