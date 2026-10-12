#!/usr/bin/env bash
# Tally and audio-follows-video (§10.3) through MXL: TSL UMD 5.0 over UDP and TCP
# (tsl_send.py, UTF-16 labels), TSL_SCREEN, an AFV strip closed without tally,
# opened when its index is on air (sample-exact), closed on preview only, its fade,
# the tally API and metric.
# shellcheck source=tests/integration/lib.sh
source "$(dirname "$0")/lib.sh"

FLOW=88888888-8888-4888-8888-888888888888
DOMAIN=99999999-9999-4999-8999-999999999999
UDP=${TSL_TEST_UDP:-8920}
TCP=${TSL_TEST_TCP:-8921}
SEND="$(dirname "$0")/tsl_send.py"
start_tone "$WORK/mxl/tones" "$FLOW" --domain-id "$DOMAIN" --channels 2
wait_until 10 "tone flow" test -d "$WORK/mxl/tones/$FLOW.mxl-flow"
start_mixer NMOS_SEED=it-tally TSL_ENABLE=true TSL_UDP_PORT="$UDP" TSL_TCP_PORT="$TCP" TSL_SCREEN=2
route 1 "$DOMAIN" "$FLOW"
wait_until 5 "input 1 running" is_state 1 running

# vox (tone 1, −20 dBFS) hard left into a stereo bus, following tally index 3.
api POST /api/v1/strips '{"id": "vox", "format": "mono", "pan": -1, "patch": [{"input": 1, "channel": 1}], "tally_index": 3, "afv": {"enabled": true, "depth_db": "-inf", "fade_in_ms": 0, "fade_out_ms": 0}}' >/dev/null
api POST /api/v1/buses '{"id": "main", "name": "Main", "format": "stereo"}' >/dev/null
api PUT /api/v1/sends/vox/main >/dev/null
api PUT /api/v1/outputs/main >/dev/null
OUT_FLOW=$(json "$WEB/api/v1/outputs/main" "d['mxl_flow_id']")
OWN=$(json "$WEB/api/v1/status" "d['mxl']['output_domain_dir']")
sleep 0.3
probe 1 1:1:0 2:1:0
pass "AFV strip without tally: closed"

tally() { json "$WEB/api/v1/tally" "$1"; }
python3 "$SEND" udp 127.0.0.1 "$UDP" 1 3 1 0 "other screen"
sleep 0.3
[[ "$(tally "d['strips']['vox']['on_air']")" == False ]] || fail "screen 1 was not ignored"
python3 "$SEND" udp 127.0.0.1 "$UDP" 2 3 1 0 "Kamera 3 Zürich"
wait_until 3 "vox on air" test "$(tally "d['strips']['vox']['afv_open']")" == True
[[ "$(tally "d['lamps']['3']['label']")" == "Kamera 3 Zürich" ]] || fail "label $(tally "d['lamps']['3']['label']")"
sleep 0.2
probe 1 1:1:1 2:1:0
pass "UDP: index 3 on air on screen 2 (UTF-16 label): vox open, sample-exact"

python3 "$SEND" tcp 127.0.0.1 "$TCP" 2 3 0 2 "Kamera 3"
wait_until 3 "vox off air" test "$(tally "d['strips']['vox']['afv_open']")" == False
[[ "$(tally "d['strips']['vox']['preview']")" == True ]] || fail "preview"
sleep 0.2
probe 1 1:1:0 2:1:0
pass "TCP (DLE/STX): index 3 on preview only: vox closed"

# A 1 s fade in: part-way after 0.4 s, full after 1.2 s.
api PATCH /api/v1/strips/vox '{"afv": {"enabled": true, "depth_db": "-inf", "fade_in_ms": 1000, "fade_out_ms": 0}}' >/dev/null
python3 "$SEND" udp 127.0.0.1 "$UDP" 2 3 1 0 "Kamera 3"
sleep 0.4
mid=$(json "$WEB/api/v1/meters" "d['vox']['peak_dbfs'][0]")
sleep 0.8
full=$(json "$WEB/api/v1/meters" "d['vox']['peak_dbfs'][0]")
python3 -c "exit(0 if -40 < $mid < -21 and abs($full + 20) < 0.1 else 1)" || fail "fade in: $mid dBFS after 0.4 s, $full after 1.2 s"
pass "AFV fade in over 1 s: $mid dBFS after 0.4 s, $full dBFS after 1.2 s"

# Off: AFV no longer touches the strip.
api PATCH /api/v1/strips/vox '{"afv": null}' >/dev/null
python3 "$SEND" udp 127.0.0.1 "$UDP" 2 3 0 0 ""
sleep 0.3
probe 1 1:1:1 2:1:0
packets=$(curl -fsS "$WEB/metrics" | sed -n 's/^mxl_audio_mixer_tally_packets_total \(.*\)/\1/p')
((packets >= 5)) || fail "tally_packets_total $packets"
pass "AFV removed: vox follows its fader only; $packets packets counted"

rc=0
stop_mixer || rc=$?
[[ $rc == 143 ]] || fail "exit code $rc"
pass tally
