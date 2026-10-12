#!/usr/bin/env bash
# The control room (§10.1) through MXL: a published monitor (MXL flow, NMOS
# sender) on the programme bus, sample-exact: level, dim, cut, swap, left only;
# PFL (replacing the programme), AFL, interlock, solo-in-place with a solo-safe
# strip; an input as the source; an oscillator bus (§10.2) on the monitor.
# shellcheck source=tests/integration/lib.sh
source "$(dirname "$0")/lib.sh"

FLOW=88888888-8888-4888-8888-888888888888
DOMAIN=99999999-9999-4999-8999-999999999999
start_tone "$WORK/mxl/tones" "$FLOW" --domain-id "$DOMAIN" --channels 2
wait_until 10 "tone flow" test -d "$WORK/mxl/tones/$FLOW.mxl-flow"
start_mixer NMOS_SEED=it-room
route 1 "$DOMAIN" "$FLOW"
wait_until 5 "input 1 running" is_state 1 running

# vox (tone 1) hard left, gtr (tone 2) hard right, into the programme bus.
api POST /api/v1/strips '{"id": "vox", "format": "mono", "pan": -1, "patch": [{"input": 1, "channel": 1}]}' >/dev/null
api POST /api/v1/strips '{"id": "gtr", "format": "mono", "pan": 1, "patch": [{"input": 1, "channel": 2}]}' >/dev/null
api POST /api/v1/buses '{"id": "pgm", "name": "Programme", "format": "stereo"}' >/dev/null
api PUT /api/v1/sends/vox/pgm >/dev/null
api PUT /api/v1/sends/gtr/pgm >/dev/null
api PUT /api/v1/outputs/pgm >/dev/null
api POST /api/v1/monitors '{"id": "main", "name": "Main", "format": "stereo", "source": {"signal": "pgm"}, "publish": true}' >/dev/null
OWN=$(json "$WEB/api/v1/status" "d['mxl']['output_domain_dir']")
MON=$(json "$WEB/api/v1/status" "[o['mxl_flow_id'] for o in d['outputs'] if o['signal'] == 'monitor:main'][0]")
PGM=$(json "$WEB/api/v1/outputs/pgm" "d['mxl_flow_id']")
OUT_FLOW=$MON
sleep 0.3
probe 1 1:1:1 2:2:1
label=$(json "$NMOS/x-nmos/node/v1.3/senders" "[s['label'] for s in d if s['flow_id']][0:5]")
grep -q "Monitor Main" <<<"$label" || fail "no sender Monitor Main: $label"
pass "monitor Main on the programme: sample-exact, published with the NMOS sender Monitor Main"

mon() { api PATCH /api/v1/monitors/main "$1" >/dev/null; sleep 0.2; }
mon '{"level_db": -20}'
probe 1 1:1:0.1 2:2:0.1
mon '{"level_db": 0, "dim": true}'
probe 1 1:1:0.089125094 2:2:0.089125094
mon '{"dim": false, "cut": true}'
probe 1 1:1:0 2:2:0
mon '{"cut": false, "check": "swap"}'
probe 1 1:2:1 2:1:1
mon '{"check": "left"}'
probe 1 1:1:1 2:1:1
mon '{"check": "stereo"}'
pass "level −20 dB, dim (−21 dB), cut, swap, left only: sample-exact"

solo() { api PUT "/api/v1/solo/$1" "{\"kind\": \"$2\"}" >/dev/null; sleep 0.3; }
api PATCH /api/v1/control-room '{"solo": {"programme_db": "-inf"}}' >/dev/null
api PATCH /api/v1/strips/vox '{"fader_db": "-inf"}' >/dev/null
solo vox pfl
probe 1 1:1:0.70710677 2:1:0.70710677
pass "PFL of vox (fader at −∞): pre-fader, centred at −3 dB on the monitor, the programme replaced"
api PATCH /api/v1/strips/vox '{"fader_db": 0}' >/dev/null
solo vox afl
probe 1 1:1:1 2:1:0
pass "AFL of vox: post-pan (hard left)"
api PATCH /api/v1/control-room '{"solo": {"mode": "interlock"}}' >/dev/null
solo vox pfl
solo gtr pfl
[[ "$(json "$WEB/api/v1/solo" "sorted(d)")" == "['gtr']" ]] || fail "interlock: $(curl -fsS "$WEB/api/v1/solo")"
probe 1 1:2:0.70710677 2:2:0.70710677
pass "interlock: soloing gtr ended vox's solo"

curl -fsS -o /dev/null -X DELETE "$WEB/api/v1/solo"
api PATCH /api/v1/control-room '{"solo": {"mode": "additive", "sip": true}}' >/dev/null
OUT_FLOW=$PGM
solo vox afl
probe 1 1:1:1 2:2:0
api PATCH /api/v1/strips/gtr '{"solo_safe": true}' >/dev/null
sleep 0.2
probe 1 1:1:1 2:2:1
curl -fsS -o /dev/null -X DELETE "$WEB/api/v1/solo"
sleep 0.3
api PATCH /api/v1/strips/gtr '{"solo_safe": false}' >/dev/null
sleep 0.2
probe 1 1:1:1 2:2:1
OUT_FLOW=$MON
probe 1 1:1:1 2:2:1
pass "solo-in-place: gtr muted in the programme while vox is soloed, kept when solo-safe; everything back after the solo"

api PATCH /api/v1/buses/pgm '{"fader_db": "-inf"}' >/dev/null
mon '{"source": {"input": 1}}'
probe 1 1:1:1 2:2:1
pass "input 1 as the monitor source"

api POST /api/v1/buses '{"id": "osc", "name": "Oscillator", "format": "stereo"}' >/dev/null
api POST /api/v1/buses/osc/chain '{"id": "tone", "type": "oscillator", "waveform": "sine", "freq_hz": 1000, "level_dbfs": -18}' >/dev/null
mon '{"source": {"signal": "osc"}}'
sleep 0.5
for k in osc monitor:main; do
    p=$(json "$WEB/api/v1/meters" "d['$k']['peak_dbfs']")
    python3 -c "exit(0 if all(abs(v + 18) < 0.05 for v in $p) else 1)" || fail "$k peak $p"
done
pass "oscillator bus at 1 kHz −18 dBFS on the monitor"

# Bass management on a 5.1 monitor fed by the stereo oscillator (L → L, R → R).
api POST /api/v1/monitors '{"id": "surround", "name": "Surround", "format": "5.1", "source": {"signal": "osc"}, "bass_management": {"enabled": true, "crossover_hz": 80, "lfe_gain_db": 10}}' >/dev/null
code=$(curl -s -o /dev/null -w '%{http_code}' -X PATCH -H 'content-type: application/json' "$WEB/api/v1/monitors/main" \
    -d '{"bass_management": {"enabled": true, "crossover_hz": 80, "lfe_gain_db": 10}}')
[[ "$code" == 400 ]] || fail "bass management on a stereo monitor answered $code"
peaks() { json "$WEB/api/v1/meters" "[round(v, 1) for v in d['monitor:surround']['peak_dbfs']]"; }
sleep 0.5
read -r l lfe < <(json "$WEB/api/v1/meters" "' '.join(str(d['monitor:surround']['peak_dbfs'][i]) for i in (0, 3))")
python3 -c "exit(0 if abs($l + 18) < 0.1 and $lfe < -60 else 1)" || fail "1 kHz: L $l, LFE $lfe ($(peaks))"
api PATCH /api/v1/buses/osc/chain/tone '{"freq_hz": 30}' >/dev/null
sleep 1
read -r l lfe < <(json "$WEB/api/v1/meters" "' '.join(str(d['monitor:surround']['peak_dbfs'][i]) for i in (0, 3))")
python3 -c "exit(0 if $l < -45 and abs($lfe + 12.1) < 0.3 else 1)" || fail "30 Hz: L $l, LFE $lfe ($(peaks))"
pass "bass management (80 Hz): 1 kHz stays on L, 30 Hz goes to the LFE at $lfe dBFS (L + R); a stereo monitor refuses it"
api PATCH /api/v1/monitors/surround '{"bass_management": {"enabled": false, "crossover_hz": 80, "lfe_gain_db": 10}}' >/dev/null
sleep 1
read -r l lfe < <(json "$WEB/api/v1/meters" "' '.join(str(d['monitor:surround']['peak_dbfs'][i]) for i in (0, 3))")
python3 -c "exit(0 if abs($l + 18) < 0.2 else 1)" || fail "bass management off: L $l"
pass "bass management off: 30 Hz on L again"

rc=0
stop_mixer || rc=$?
[[ $rc == 143 ]] || fail "exit code $rc"
pass room
