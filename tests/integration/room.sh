#!/usr/bin/env bash
# The control room (§10.1) through MXL: a published monitor (MXL flow, NMOS
# sender) on the programme bus, sample-exact: level, dim, cut, swap, left only,
# mute and solo of single channels; personal monitors following the PFL or not;
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

mon '{"mute_channels": [2]}'
probe 1 1:1:1 2:2:0
mon '{"mute_channels": [], "solo_channels": [2]}'
probe 1 1:1:0 2:2:1
mon '{"mute_channels": [2]}'
probe 1 1:1:0 2:2:0
mon '{"mute_channels": [], "solo_channels": []}'
probe 1 1:1:1 2:2:1
code=$(curl -s -o /dev/null -w '%{http_code}' -X PATCH -H 'content-type: application/json' "$WEB/api/v1/monitors/main" -d '{"mute_channels": [3]}')
[[ "$code" == 400 ]] || fail "mute of channel 3 on a stereo monitor answered $code"
pass "channel mute and solo: R muted, R soloed, R soloed and muted, back; a channel the format lacks refused"

solo() { api PUT "/api/v1/solo/$1" "{\"kind\": \"$2\"}" >/dev/null; sleep 0.3; }
api PATCH /api/v1/control-room '{"solo": {"programme_db": "-inf"}}' >/dev/null
api PATCH /api/v1/strips/vox '{"fader_db": "-inf"}' >/dev/null
solo vox pfl
probe 1 1:1:0.70710677 2:1:0.70710677
pass "PFL of vox (fader at −∞): pre-fader, centred at −3 dB on the monitor, the programme replaced"

# Personal monitors without a source: one follows the control room's PFL, one does not
# (their meters: tone 1 at −20 dBFS, centred at −3 dB).
P1=$(api POST /api/v1/personal '{"follows_solo": true}' | python3 -c "import json,sys; print(json.load(sys.stdin)['id'])")
P2=$(api POST /api/v1/personal '{}' | python3 -c "import json,sys; print(json.load(sys.stdin)['id'])")
sleep 0.5
peaks() { json "$WEB/api/v1/meters" "[d['monitor:$1']['peak_dbfs'][c] for c in (0, 1)]"; }
[[ "$(json "$WEB/api/v1/meters" "all(abs(float(p) + 23.0103) < 0.01 for p in d['monitor:$P1']['peak_dbfs'])")" == True ]] || fail "$P1 (follows solo): $(peaks "$P1")"
[[ "$(json "$WEB/api/v1/meters" "all(p is None or float(p) < -100 for p in d['monitor:$P2']['peak_dbfs'])")" == True ]] || fail "$P2 (own solo only): $(peaks "$P2")"
api PATCH "/api/v1/personal/$P1" '{"follows_solo": false}' >/dev/null
sleep 0.5
[[ "$(json "$WEB/api/v1/meters" "all(p is None or float(p) < -100 for p in d['monitor:$P1']['peak_dbfs'])")" == True ]] || fail "$P1 after follows_solo off: $(peaks "$P1")"
curl -fsS -X DELETE "$WEB/api/v1/personal/$P1"
curl -fsS -X DELETE "$WEB/api/v1/personal/$P2"
pass "personal monitors: the one that follows solo hears the PFL of vox at −23.01 dBFS, the other and the first after follows_solo off hear nothing"
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
OUT_FLOW=$MON
api PATCH /api/v1/buses/osc/chain/tone '{"waveform": "pink"}' >/dev/null
sleep 1
"$PROBE_BIN" --domain "$OWN" --flow "$MON" --seconds 3 >"$WORK/pink.json"
corr=$(json "$WEB/api/v1/meters" "d['monitor:main']['correlation']")
python3 - "$(cat "$WORK/pink.json")" "$corr" <<'EOF' || fail "pink noise: $(cat "$WORK/pink.json"), correlation $corr"
import json, sys
d = json.loads(sys.argv[1])
assert all(abs(l["rms_dbfs"] + 18) < 0.5 for l in d["levels"]), d["levels"]
assert abs(float(sys.argv[2])) < 0.2, sys.argv[2]
EOF
api PATCH /api/v1/buses/osc/chain/tone '{"waveform": "sweep", "freq_hz": 20, "sweep_to_hz": 20000, "sweep_s": 1}' >/dev/null
sleep 0.5
"$PROBE_BIN" --domain "$OWN" --flow "$MON" --seconds 2 >"$WORK/sweep.json"
python3 -c "import json,sys; d=json.load(open('$WORK/sweep.json')); sys.exit(0 if all(abs(l['peak_dbfs'] + 18) < 0.1 for l in d['levels']) else 1)" || fail "sweep: $(cat "$WORK/sweep.json")"
api PATCH /api/v1/buses/osc/chain/tone '{"waveform": "sine", "freq_hz": 1000}' >/dev/null
pass "oscillator pink noise at −18 dBFS RMS, independent channels (correlation $corr); a sweep at −18 dBFS peak"

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
