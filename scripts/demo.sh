#!/usr/bin/env bash
# A demo for trying the console or developing the UI: an 8-channel tone flow, the
# mixer on it (web UI on http://127.0.0.1:8160/) and a small show (8 strips, a
# stereo main, two auxes, processing). With espeak-ng, host and guest speak in
# turns, in an auto-mix, with the music ducked. Stops everything on Ctrl-C.
#
#   cargo build --release --bins && scripts/demo.sh
#
# Needs libmxl (LD_LIBRARY_PATH, docs/development.md); for listening in the
# browser, MediaMTX (MEDIAMTX_BIN, the same file). State and the MXL domain go to
# a temporary directory under /dev/shm.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="${BIN:-$ROOT/target/release}"
WORK="$(mktemp -d /dev/shm/mixer-demo.XXXXXX)"
WEB="http://127.0.0.1:${WEB_PORT:-8160}"
NMOS="http://127.0.0.1:${NMOS_PORT:-3312}"
FLOW=11111111-2222-4333-8444-555555555555
DOMAIN=66666666-7777-4888-8999-aaaaaaaaaaaa
PIDS=()
cleanup() {
    for p in "${PIDS[@]}"; do kill -TERM "$p" 2>/dev/null || true; done
    wait 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT INT TERM

"$BIN/mxl-mixer-tone" --domain "$WORK/mxl/tones" --flow "$FLOW" --domain-id "$DOMAIN" --channels 8 &
PIDS+=($!)
env HOST_ID=demo MXL_DOMAIN_SCAN_PATH="$WORK/mxl" MIXER_STATE_DIR="$WORK/state" MIXER_INPUT_FLOWS=4 \
    NMOS_LABEL="${NMOS_LABEL:-Demo mixer}" "$BIN/mxl-audio-mixer" &
PIDS+=($!)
for _ in $(seq 1 100); do curl -fsS -o /dev/null "$WEB/livez" 2>/dev/null && break; sleep 0.1; done

api() { curl -fsS -o /dev/null -X "$1" -H 'content-type: application/json' "$WEB$2" ${3:+-d "$3"}; }
receiver=$(curl -fsS "$NMOS/x-nmos/node/v1.3/receivers" |
    python3 -c "import json,sys; print([r['id'] for r in json.load(sys.stdin) if r['label'] == 'Input 1'][0])")
api PATCH "/x-nmos/connection/v1.2/single/receivers/$receiver/staged" \
    "{\"master_enable\": true, \"activation\": {\"mode\": \"activate_immediate\"}, \"transport_params\": [{\"mxl_domain_id\": \"$DOMAIN\", \"mxl_flow_id\": \"$FLOW\"}]}" ||
    curl -fsS -o /dev/null -X PATCH -H 'content-type: application/json' \
        "$NMOS/x-nmos/connection/v1.2/single/receivers/$receiver/staged" \
        -d "{\"master_enable\": true, \"activation\": {\"mode\": \"activate_immediate\"}, \"transport_params\": [{\"mxl_domain_id\": \"$DOMAIN\", \"mxl_flow_id\": \"$FLOW\"}]}"

api POST /api/v1/buses '{"id": "main", "name": "Main", "format": "stereo", "loudness": true}'
api POST /api/v1/buses '{"id": "aux-1", "name": "Aux 1", "format": "mono", "colour": "#3fb968"}'
api POST /api/v1/buses '{"id": "aux-2", "name": "Aux 2", "format": "mono", "colour": "#e0a83a"}'
i=0
for s in "host:Host:#4da3ff" "guest:Guest:#4da3ff" "phone:Phone:#b07cff" "music-l:Music L:#e05a4e" \
    "music-r:Music R:#e05a4e" "sfx:SFX:#e0a83a" "amb:Ambience:#3fb968" "spare:Spare:"; do
    IFS=: read -r id name colour <<<"$s"
    i=$((i + 1))
    pan=$(python3 -c "print({4: -1, 5: 1}.get($i, round(($i - 4.5) / 6, 2)))")
    api POST /api/v1/strips "{\"id\": \"$id\", \"name\": \"$name\", \"format\": \"mono\", \"pan\": $pan, \"fader_db\": -6, ${colour:+\"colour\": \"$colour\",} \"patch\": [{\"input\": 1, \"channel\": $i}]}"
    api PUT "/api/v1/sends/$id/main"
done
api PUT /api/v1/sends/host/aux-1 '{"gain_db": -3}'
api PUT /api/v1/sends/guest/aux-1 '{"gain_db": -3}'
api PUT /api/v1/sends/phone/aux-2 '{"gain_db": 0, "tap": "pre-fader"}'
api PATCH /api/v1/strips/spare '{"mute": true}'
for s in host guest; do
    api POST "/api/v1/strips/$s/chain" '{"id": "hpf", "type": "filter", "mode": "high-pass", "freq_hz": 80, "slope_db_per_oct": 24}'
    api POST "/api/v1/strips/$s/chain" '{"id": "eq", "type": "eq", "bands": [{"type": "low-shelf", "freq_hz": 200, "q": 0.7, "gain_db": -2}, {"type": "bell", "freq_hz": 3000, "q": 1.4, "gain_db": 3}, {"type": "high-shelf", "freq_hz": 10000, "q": 0.7, "gain_db": 2}]}'
    api POST "/api/v1/strips/$s/chain" '{"id": "comp", "type": "compressor", "threshold_db": -26, "ratio": 3, "knee_db": 6, "attack_ms": 5, "release_ms": 150, "makeup_db": 2, "detector": "rms"}'
    api POST "/api/v1/strips/$s/chain" '{"id": "deess", "type": "de-esser", "mode": "split-band", "freq_hz": 6000, "threshold_db": -30, "range_db": 6, "attack_ms": 1, "release_ms": 80, "listen": false}'
done
api POST /api/v1/buses/main/chain '{"id": "lim", "type": "limiter", "ceiling_db": -1, "release_ms": 50, "lookahead_ms": 1.5, "true_peak": true}'
api PUT /api/v1/outputs/main
api POST /api/v1/buses '{"id": "osc", "name": "Oscillator", "format": "stereo"}'
api POST /api/v1/buses/osc/chain '{"id": "tone", "type": "oscillator", "waveform": "ebu-ident", "freq_hz": 1000, "level_dbfs": -18, "bypass": true}'
api POST /api/v1/monitors '{"id": "main", "name": "Main", "kind": "speakers", "format": "stereo", "source": {"signal": "main"}, "calibration_spl": 85, "publish": true}'
api POST /api/v1/monitors '{"id": "phones", "name": "Phones", "kind": "headphones", "format": "stereo", "source": {"signal": "main"}}'
# Talkback: the spare channel as the director's mic, into both auxes (IFB interrupting).
api POST /api/v1/talkback/mics '{"id": "director", "name": "Director", "input": 1, "channel": 8, "priority": 5}'
api POST /api/v1/talkback/destinations '{"id": "ifb", "name": "IFB 1", "bus": "aux-1", "mode": "interrupt", "return": {"input": 1}}'
api POST /api/v1/talkback/destinations '{"id": "studio", "name": "Studio", "bus": "aux-2"}'
api PATCH /api/v1/control-room '{"sources": [{"signal": "main"}, {"signal": "aux-1"}, {"signal": "aux-2"}, {"signal": "osc"}, {"input": 1}]}'
# The phone guest's N-1: the main mix without the phone (it follows the main mix).
api POST /api/v1/operations/n-1 '{"strips": ["phone"], "of": "main", "format": "mono"}'
# VCAs for the layout's pages, and a mute group for the playout.
api POST /api/v1/vcas '{"id": "talent", "name": "Talent", "members": ["host", "guest", "phone"], "colour": "#4da3ff"}'
api POST /api/v1/vcas '{"id": "playout", "name": "Playout", "members": ["music-l", "music-r", "sfx", "amb"], "colour": "#3fb968"}'
api POST /api/v1/mute-groups '{"id": "playout", "name": "Playout", "members": ["music-l", "music-r", "sfx", "amb"]}'
api PUT /api/v1/layouts '[{"name": "Show", "pages": [{"name": "Talent", "signals": ["host", "guest", "phone"]}, {"name": "Playout", "signals": ["music-l", "music-r", "sfx", "amb"]}]}]'
# With espeak-ng: host and guest speak in turns (inputs 2 and 3, a 10 s period),
# in an auto-mix and auto-levelled, and the music ducks under their speech to
# a ratio of 5 LU (§8).
if command -v espeak-ng >/dev/null; then
    espeak-ng -v en -s 165 -w "$WORK/host.wav" "Good evening, and welcome to the programme. Tonight we talk about the city."
    espeak-ng -v en+f3 -s 165 -w "$WORK/guest.wav" "Thank you. It is a pleasure to be here, and there is a lot to say."
    for who in host:2:0 guest:3:5; do
        IFS=: read -r id input offset <<<"$who"
        flow=$(python3 -c "import uuid; print(uuid.uuid5(uuid.UUID('$FLOW'), '$id'))")
        "$BIN/mxl-mixer-tone" --domain "$WORK/mxl/tones" --flow "$flow" --domain-id "$DOMAIN" --channels 1 \
            --wav "$WORK/$id.wav" --gap-s "$(python3 -c "import wave; w = wave.open('$WORK/$id.wav'); print(10 - w.getnframes() / w.getframerate())")" --offset-s "$offset" &
        PIDS+=($!)
        for _ in $(seq 1 50); do [[ -d "$WORK/mxl/tones/$flow.mxl-flow" ]] && break; sleep 0.1; done
        receiver=$(curl -fsS "$NMOS/x-nmos/node/v1.3/receivers" |
            python3 -c "import json,sys; print([r['id'] for r in json.load(sys.stdin) if r['label'] == 'Input $input'][0])")
        curl -fsS -o /dev/null -X PATCH -H 'content-type: application/json' \
            "$NMOS/x-nmos/connection/v1.2/single/receivers/$receiver/staged" \
            -d "{\"master_enable\": true, \"activation\": {\"mode\": \"activate_immediate\"}, \"transport_params\": [{\"mxl_domain_id\": \"$DOMAIN\", \"mxl_flow_id\": \"$flow\"}]}"
        api PATCH "/api/v1/strips/$id" "{\"patch\": [{\"input\": $input, \"channel\": 1}]}"
    done
    api POST /api/v1/automixes '{"id": "panel", "name": "Panel", "members": ["host", "guest"]}'
    api POST /api/v1/ducks '{"id": "bed", "name": "Music under speech", "keys": ["host", "guest"], "targets": ["music-l", "music-r"], "mode": "ratio"}'
    for id in host guest; do api PUT "/api/v1/autolevels/$id"; done
fi
api POST /api/v1/snapshots '{"name": "Top of show"}'

echo "demo running: $WEB/ (Ctrl-C stops)"
wait
