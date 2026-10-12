#!/usr/bin/env bash
# The preview contract (Appendix B.1): monitors as Opus over RTSP to MediaMTX,
# played over WHEP in headless Chrome (web/tests/preview.mjs).
# - Own mode: the mixer starts MediaMTX as a child; a stream starts on a listen
#   request, plays in the browser, its latency is measured with the second-pulse
#   oscillator; it stops after the grace time; the child, killed, is restarted
#   and the stream published again; a personal monitor plays (page and WHEP on
#   the host address); no publishing from the network, only the mixer's paths.
# - Shared mode (PREVIEW_PUBLISH_URL): no child; the streams go to a separate
#   MediaMTX, kept alive by its readers (MEDIAMTX_API_URL).
# Needs MEDIAMTX_BIN (default: mediamtx in PATH), Chrome ($CHROME) and the web
# test dependencies (npm ci in web/).
# shellcheck source=tests/integration/lib.sh
source "$(dirname "$0")/lib.sh"

MEDIAMTX_BIN="${MEDIAMTX_BIN:-$(command -v mediamtx || true)}"
[[ -x "$MEDIAMTX_BIN" ]] || fail "no MediaMTX binary (set MEDIAMTX_BIN)"
LATENCY_MAX_MS="${PREVIEW_LATENCY_MAX_MS:-150}"
export MEDIAMTX_BIN

preview() { json "$WEB/api/v1/preview" "$1"; }
stream() { # <id> <field>
    preview "[s for s in d['streams'] if s['id'] == '$1'][0]['$2']"
}
is_stream() { [[ "$(stream "$1" state)" == "$2" ]]; }
child_running() { [[ "$(preview "d['child']['running']")" == True ]]; }
restarted() { [[ "$(preview "d['child']['restarts']")" -ge 1 ]]; }
play() { # <monitor id> <seconds> [mixer base] ; the JSON of preview.mjs in $PLAYED
    PLAYED=$(node "$ROOT/web/tests/preview.mjs" "${3:-$WEB}" "$1" "$2")
    echo "$PLAYED" >>"$IT_OUT/preview-played.log"
    log "played $1: $PLAYED"
}
played() { python3 -c "import json,sys; d=json.loads(sys.argv[1]); print($1)" "$PLAYED"; }

setup_show() {
    api POST /api/v1/buses '{"id": "osc", "name": "Oscillator", "format": "stereo"}' >/dev/null
    api POST /api/v1/buses/osc/chain '{"id": "pulse", "type": "oscillator", "waveform": "second-pulse", "freq_hz": 1000, "level_dbfs": -18}' >/dev/null
    api POST /api/v1/monitors '{"id": "main", "name": "Main", "format": "stereo", "source": {"signal": "osc"}}' >/dev/null
}

# --- own mode ---
start_mixer NMOS_SEED=it-preview
[[ "$(preview "d['mode']")" == own ]] || fail "mode: $(curl -fsS "$WEB/api/v1/preview")"
wait_until 10 "the MediaMTX child" child_running
CHILD=$(preview "d['child']['pid']")
pass "own mode: MediaMTX child $CHILD started"
setup_show
[[ "$(stream main state)" == idle ]] || fail "a stream nobody listens to: $(stream main state)"
whip() { curl -s -o /dev/null -w '%{http_code}' -X POST -H 'content-type: application/sdp' --data-binary $'v=0\r\n' "$1"; }
[[ "$(whip "http://$HOST_IP:8609/mxl-audio-mixer/x/whip")" == 401 ]] || fail "WHIP from the network not refused"
[[ "$(whip "http://127.0.0.1:8609/other/x/whep")" == 400 ]] || fail "a path outside the prefix is served"
pass "built-in MediaMTX: no publishing from the network, only the mixer's paths"

play main 12
[[ "$(played "d['received_s'] >= 8 and d['peak'] > 0.1")" == True ]] || fail "no audio in the browser: $PLAYED"
[[ "$(played "d['onsets'] >= 9")" == True ]] || fail "pulses: $PLAYED"
LATENCY=$(played "d['latency_ms']['median']")
pass "own mode: monitor main over WHEP in Chrome, a pulse every second, latency median ${LATENCY} ms (max $(played "d['latency_ms']['max']") ms)"
[[ "$(played "d['latency_ms']['median'] <= $LATENCY_MAX_MS")" == True ]] || fail "latency ${LATENCY} ms > ${LATENCY_MAX_MS} ms"

wait_until 15 "the stream to stop after the grace time" is_stream main idle
pass "the stream stopped 10 s after the last listen request"

api POST /api/v1/preview/main/listen >/dev/null
wait_until 10 "stream publishing" is_stream main publishing
kill -KILL "$CHILD"
wait_until 15 "MediaMTX restarted" restarted
wait_until 10 "MediaMTX running" child_running
NEW=$(preview "d['child']['pid']")
[[ "$NEW" != "$CHILD" ]] || fail "same child pid after a restart"
play main 6
[[ "$(played "d['received_s'] >= 3 and d['onsets'] >= 4")" == True ]] || fail "after the restart: $PLAYED"
[[ "$(stream main connects)" -ge 2 ]] || fail "connects: $(stream main connects)"
pass "MediaMTX killed: restarted (pid $NEW), the stream published again and playing"

ID=$(api POST /api/v1/personal '{"source": {"signal": "osc"}}' | python3 -c "import json,sys; print(json.load(sys.stdin)['id'])")
play "$ID" 6 "${WEB/127.0.0.1/$HOST_IP}"
[[ "$(played "d['received_s'] >= 3 and d['onsets'] >= 4")" == True ]] || fail "personal monitor: $PLAYED"
pass "personal monitor $ID: plays over WHEP, the page and WHEP on the host's address"
code=$(curl -s -o /dev/null -w '%{http_code}' -X DELETE "$WEB/api/v1/personal/$ID")
[[ "$code" == 204 ]] || fail "delete personal: $code"

rc=0
stop_mixer || rc=$?
[[ $rc == 143 ]] || fail "exit code $rc after SIGTERM, not 143"
wait_until 5 "the MediaMTX child to end" bash -c "! kill -0 $NEW"
pass "own mode: the child ends with the mixer"

# --- shared mode ---
cat >"$WORK/shared.yml" <<EOF
logLevel: warn
api: true
apiAddress: 127.0.0.1:8977
rtsp: true
rtspAddress: 127.0.0.1:8974
rtspTransports: [tcp]
rtmp: false
hls: false
srt: false
moq: false
webrtc: true
webrtcAddress: :8619
webrtcLocalUDPAddress: :8219
webrtcLocalTCPAddress: ""
webrtcAllowOrigins: ["*"]
paths:
  all_others:
EOF
(cd "$WORK" && exec "$MEDIAMTX_BIN" "$WORK/shared.yml") >>"$IT_OUT/preview-mediamtx.log" 2>&1 &
PIDS+=("$!")
wait_until 10 "the shared MediaMTX" http_ok "http://127.0.0.1:8977/v3/paths/list"
start_mixer NMOS_SEED=it-preview MEDIAMTX_BIN=/nonexistent \
    PREVIEW_PUBLISH_URL=rtsp://127.0.0.1:8974 PREVIEW_WHEP_URL=http://127.0.0.1:8619 \
    MEDIAMTX_API_URL=http://127.0.0.1:8977
[[ "$(preview "d['mode']")" == shared && "$(preview "d['child']")" == None ]] || fail "shared mode: $(curl -fsS "$WEB/api/v1/preview")"
# The show (state dir) is still there.
play main 6
[[ "$(played "d['received_s'] >= 3 and d['onsets'] >= 4")" == True ]] || fail "shared mode: $PLAYED"
pass "shared mode: no child, monitor main published to the shared MediaMTX and playing (latency median $(played "d['latency_ms']['median']") ms)"

# Readers on MediaMTX keep the stream without listen requests: one request, then
# 20 s of playing (the grace time is 10 s).
wait_until 15 "the stream to stop" is_stream main idle
PLAYED=$(node "$ROOT/web/tests/preview.mjs" "$WEB" main 20 once)
log "played main once: $PLAYED"
[[ "$(played "d['received_s'] >= 18")" == True ]] || fail "a reader on MediaMTX did not keep the stream: $PLAYED"
wait_until 20 "the stream to stop without readers" is_stream main idle
pass "shared mode: a WHEP reader on MediaMTX kept the stream for 20 s after a single listen request; without readers it stopped"
