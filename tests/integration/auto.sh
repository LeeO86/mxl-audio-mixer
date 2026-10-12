#!/usr/bin/env bash
# The control engine (§8) through MXL: gain sharing sample-exact on two equal
# tones (NOM, a weight, bypass, a muted member, disabled; no plan swap); the VAD
# on speech (espeak-ng) and not on a tone; a tone ducked under the speech
# (depth, onset latency, release) measured on the output; an auto-mix of two
# talkers taking turns, with last-mic-hold; deleted signals leave it all.
# Needs espeak-ng.
# shellcheck source=tests/integration/lib.sh
source "$(dirname "$0")/lib.sh"

command -v espeak-ng >/dev/null || fail "espeak-ng is needed (apt install espeak-ng)"

# Speech: one sentence at 48 kHz, padded with silence to a 6 s period.
espeak-ng -v en -s 170 -w "$WORK/espeak.wav" "Good evening, here is the news from the city."
read -r ONSET END < <(python3 - "$WORK/espeak.wav" "$WORK/speech.wav" <<'EOF'
import struct, sys, wave
src = wave.open(sys.argv[1])
rate, n = src.getframerate(), src.getnframes()
x = struct.unpack("<%dh" % n, src.readframes(n))
period = 6 * 48000
step = rate / 48000
y = []
for i in range(period):
    t = i * step
    k = int(t)
    y.append(x[k] + (x[k + 1] - x[k]) * (t - k) if k + 1 < n else 0.0)
loud = [i for i, v in enumerate(y) if abs(v) > 0.02 * 32768]
assert loud and loud[-1] < period // 2 - 4800, "the sentence must fit in 2.9 s"
w = wave.open(sys.argv[2], "wb")
w.setnchannels(1)
w.setsampwidth(2)
w.setframerate(48000)
w.writeframes(struct.pack("<%dh" % period, *[int(max(-32768, min(32767, v))) for v in y]))
print(loud[0], loud[-1])
EOF
)
log "speech from sample $ONSET to $END of each 6 s period"

TONES=88888888-8888-4888-8888-888888888888
VOX=77777777-7777-4777-8777-777777777777
GUEST=66666666-6666-4666-8666-666666666666
DOMAIN=99999999-9999-4999-8999-999999999999
start_tone "$WORK/mxl/tones" "$TONES" --domain-id "$DOMAIN" --channels 2
start_tone "$WORK/mxl/tones" "$VOX" --domain-id "$DOMAIN" --channels 1 --wav "$WORK/speech.wav"
start_tone "$WORK/mxl/tones" "$GUEST" --domain-id "$DOMAIN" --channels 1 --wav "$WORK/speech.wav" --offset-s 3
for f in $TONES $VOX $GUEST; do wait_until 10 "flow $f" test -d "$WORK/mxl/tones/$f.mxl-flow"; done
start_mixer NMOS_SEED=it-auto
route 1 "$DOMAIN" "$TONES"
route 2 "$DOMAIN" "$VOX"
route 3 "$DOMAIN" "$GUEST"
for i in 1 2 3; do wait_until 5 "input $i running" is_state "$i" running; done

# t1, t2 (tones 1 and 2) hard left and right into the programme; vox and guest
# (the speech) into it too, muted for now.
api POST /api/v1/strips '{"id": "t1", "format": "mono", "pan": -1, "patch": [{"input": 1, "channel": 1}]}' >/dev/null
api POST /api/v1/strips '{"id": "t2", "format": "mono", "pan": 1, "patch": [{"input": 1, "channel": 2}]}' >/dev/null
api POST /api/v1/strips '{"id": "vox", "format": "mono", "patch": [{"input": 2, "channel": 1}]}' >/dev/null
api POST /api/v1/strips '{"id": "guest", "format": "mono", "patch": [{"input": 3, "channel": 1}]}' >/dev/null
api POST /api/v1/buses '{"id": "pgm", "name": "Programme", "format": "stereo"}' >/dev/null
for s in t1 t2; do api PUT "/api/v1/sends/$s/pgm" >/dev/null; done
api PUT /api/v1/outputs/pgm >/dev/null
OWN=$(json "$WEB/api/v1/status" "d['mxl']['output_domain_dir']")
OUT_FLOW=$(json "$WEB/api/v1/outputs/pgm" "d['mxl_flow_id']")
settled 1
probe 1 1:1:1 2:2:1

swaps() { json "$WEB/api/v1/status" "d['dsp']['plan_swaps']"; }
am() { api PATCH /api/v1/automixes/tones "$1" >/dev/null; sleep 0.3; }
api POST /api/v1/automixes '{"id": "tones", "name": "Tones", "last_mic_hold": false, "members": ["t1", "t2"]}' >/dev/null
sleep 0.5
SWAPS=$(swaps)
probe 1 1:1:0.70710677 2:2:0.70710677
pass "auto-mix of two equal tones: −3.01 dB each, their powers add up to one open input (NOM)"
read -r G1 G2 < <(python3 -c "w = 10 ** 0.6; print((w / (w + 1)) ** 0.5, (1 / (w + 1)) ** 0.5)")
am '{"members": [{"signal": "t1", "weight_db": 6}, "t2"]}'
probe 1 "1:1:$G1" "2:2:$G2"
pass "a weight of +6 dB on t1: $G1 and $G2"
am '{"members": [{"signal": "t1"}, {"signal": "t2", "bypass": true}]}'
probe 1 1:1:1 2:2:1
pass "t2 bypassed: 0 dB, and t1 alone in the sharing: 0 dB"
# Members as objects reset their settings; bare ids would keep them.
am '{"members": [{"signal": "t1"}, {"signal": "t2"}]}'
api PATCH /api/v1/strips/t2 '{"mute": true}' >/dev/null
sleep 0.3
probe 1 1:1:1 2:2:0
api PATCH /api/v1/strips/t2 '{"mute": false}' >/dev/null
sleep 0.3
probe 1 1:1:0.70710677 2:2:0.70710677
pass "a muted member leaves the sharing; unmuted, it shares again"
am '{"enabled": false}'
probe 1 1:1:1 2:2:1
[[ "$(swaps)" == "$SWAPS" ]] || fail "auto-mix settings swapped the plan ($SWAPS → $(swaps))"
pass "disabled: both at 0 dB; no plan swap for weights, bypass, mutes or enabling"
# Before the fader by default: a pre-fader aux carries the auto gain, the fader does not change it.
am '{"enabled": true}'
api POST /api/v1/buses '{"id": "aux", "name": "Aux", "format": "mono"}' >/dev/null
api PUT /api/v1/sends/t1/aux '{"tap": "pre-fader"}' >/dev/null
api PATCH /api/v1/strips/t1 '{"fader_db": -6.0206}' >/dev/null
api PUT /api/v1/outputs/aux >/dev/null
sleep 0.5
OUT_FLOW=$(json "$WEB/api/v1/outputs/aux" "d['mxl_flow_id']") probe 1 1:1:0.70710677
probe 1 1:1:0.35355338 2:2:0.70710677
api PATCH /api/v1/strips/t1 '{"fader_db": 0}' >/dev/null
api DELETE /api/v1/outputs/aux >/dev/null
api DELETE /api/v1/buses/aux >/dev/null
pass "the auto gain acts before the fader: t1 pre-fader into an aux at 0.707; its fader at −6 dB on top in the programme only (0.354)"
api DELETE /api/v1/automixes/tones >/dev/null

# The VAD: t1 (a tone) and vox (speech) analysed; 12 s, two periods.
api PUT /api/v1/vad/t1 >/dev/null
api PUT /api/v1/vad/vox '{"hold_ms": 300}' >/dev/null
sleep 0.5
python3 - "$WEB" <<'EOF' || fail "VAD"
import json, sys, time, urllib.request
web = sys.argv[1]
get = lambda p: json.load(urllib.request.urlopen(web + p))
start = {v["signal"]: v["state"] for v in get("/api/v1/vad")}
time.sleep(12.5)
end = {v["signal"]: v["state"] for v in get("/api/v1/vad")}
onsets = {s: end[s]["speech_onsets"] - start[s]["speech_onsets"] for s in end}
assert onsets["t1"] == 0, ("the tone counted as speech", end["t1"])
assert 2 <= onsets["vox"] <= 3, ("speech onsets in two periods", onsets, end["vox"])
level = end["vox"]["speech_level_dbfs"]
print(f"vox: {onsets['vox']} onsets in 12.5 s, speech level {level} dBFS; t1: none")
EOF
curl -fsS "$WEB/metrics" | grep -q '^mxl_audio_mixer_vad_active{signal="vox"} [01]$' || fail "vad_active metric"
pass "VAD: speech onsets once per period on vox, none on the tone; vad_active in the metrics"
api DELETE /api/v1/vad/t1 >/dev/null

# Ducking: t1 down by 20 dB under vox's speech; t1 alone on the left of pgm.
api POST /api/v1/ducks '{"id": "bed", "name": "Bed", "keys": ["vox"], "targets": ["t1"], "depth_db": -20, "attack_ms": 50, "hold_ms": 300, "release_ms": 500}' >/dev/null
sleep 1
settled 2
DELAY=$(json "$WEB/api/v1/inputs/2" "round(d['effective_delay_ms'] * 48)")
"$PROBE_BIN" --domain "$OWN" --flow "$OUT_FLOW" --seconds 17 --dump "$WORK/duck.txt" >/dev/null
python3 - "$WORK/duck.txt" "$ONSET" "$END" "$DELAY" <<'EOF' || fail "ducking"
import math, sys
rows = [l.split() for l in open(sys.argv[1])]
onset, end, delay = int(sys.argv[2]), int(sys.argv[3]), int(sys.argv[4])
period = 6 * 48000
first = int(rows[0][0])
x = [float(r[1]) for r in rows]
ref = 0.1 / math.sqrt(2)
def gain_db(i):  # the 480 samples from output index i
    k = i - first
    seg = x[k:k + 480]
    if len(seg) < 480 or any(math.isnan(v) for v in seg):
        return None
    rms = math.sqrt(sum(v * v for v in seg) / 480)
    return 20 * math.log10(max(rms, 1e-9) / ref)
lat, depth, back = [], [], []
p0 = (first - delay) // period
for p in range(p0, p0 + 4):
    s, e = p * period + onset + delay, p * period + end + delay
    if s - 4800 < first or e + 2 * 48000 > first + len(x):
        continue
    before = gain_db(s - 4800)
    t = next((i for i in range(s, e, 48) if (gain_db(i) or 0) < -10), None)
    assert before is not None and abs(before) < 0.1, ("not at 0 dB before the speech", before)
    assert t is not None, "never ducked"
    lat.append((t - s) / 48)
    inside = [gain_db(i) for i in range(s + 24000, e, 480)]
    depth.append(sum(abs(g + 20) < 0.5 for g in inside) / len(inside))
    after = [gain_db(i) for i in range(e + 72000, min(e + 96000, (p + 1) * period + onset + delay - 4800), 480)]
    back.append(max(abs(g) for g in after))
assert lat, "no whole speech period in the dump"
assert max(lat) < 200, ("onset to −10 dB", lat)
assert min(depth) > 0.9, ("at −20 dB from 0.5 s into the speech", depth)
assert max(back) < 0.1, ("back to 0 dB 1.5 s after the speech", back)
print(f"periods {len(lat)}: onset to −10 dB {lat} ms; at −20 ±0.5 dB {[round(d, 3) for d in depth]} of the speech; after it within {max(back):.3f} dB of 0")
EOF
pass "ducking: the tone under the speech at −20 dB, down within 200 ms of the onset, back after hold and release"
api DELETE /api/v1/ducks/bed >/dev/null

# Two talkers taking turns (guest 3 s after vox), last-mic-hold on.
api POST /api/v1/automixes '{"id": "panel", "name": "Panel", "members": ["vox", "guest"]}' >/dev/null
sleep 1
python3 - "$WEB" <<'EOF' || fail "auto-mix of talkers"
import json, sys, time, urllib.request
web = sys.argv[1]
get = lambda p: json.load(urllib.request.urlopen(web + p))
rows = []
until = time.time() + 12.5
while time.time() < until:
    vad = {v["signal"]: v["state"]["speech_active"] for v in get("/api/v1/vad")}
    g = get("/api/v1/automixes/panel")["gains_db"]
    rows.append((vad.get("vox"), vad.get("guest"), g.get("vox"), g.get("guest")))
    time.sleep(0.05)
db = lambda v: -200.0 if v is None else v
alone = lambda a, b: [r for r in rows if r[a] and not r[b]]
vox, guest = alone(0, 1), alone(1, 0)
assert len(vox) > 20 and len(guest) > 20, (len(vox), len(guest))
ok_vox = sum(db(r[2]) > -1 and db(r[3]) < -20 for r in vox) / len(vox)
ok_guest = sum(db(r[3]) > -1 and db(r[2]) < -20 for r in guest) / len(guest)
assert ok_vox > 0.9 and ok_guest > 0.9, (ok_vox, ok_guest)
# Nobody speaking: the gains are those of the last talker (no −3 dB / −3 dB).
quiet = [r for r in rows if not r[0] and not r[1]]
held = sum(max(db(r[2]), db(r[3])) > -1 for r in quiet) / max(len(quiet), 1)
assert held > 0.9, ("last-mic-hold", held, quiet[:5])
print(f"vox alone: {ok_vox:.2f}, guest alone: {ok_guest:.2f} of the polls with the talker at 0 dB and the other below −20 dB; nobody: {held:.2f} held")
EOF
pass "auto-mix of two talkers: the one speaking near 0 dB, the other below −20 dB; last-mic-hold keeps the last talker"

# Snapshots: a moderation (the duck, no auto-mix), then the panel (the auto-mix, no duck).
api POST /api/v1/ducks '{"id": "bed", "name": "Bed", "keys": ["vox"], "targets": ["t1"]}' >/dev/null
api DELETE /api/v1/automixes/panel >/dev/null
api POST /api/v1/snapshots '{"id": "moderation", "name": "Moderation"}' >/dev/null
api DELETE /api/v1/ducks/bed >/dev/null
api POST /api/v1/automixes '{"id": "panel", "name": "Panel", "members": ["vox", "guest"]}' >/dev/null
api POST /api/v1/snapshots '{"id": "panel", "name": "Panel"}' >/dev/null
api POST /api/v1/snapshots/moderation/recall '{}' >/dev/null
[[ "$(json "$WEB/api/v1/mix" "([a['id'] for a in d.get('automixes', [])], [x['id'] for x in d.get('ducks', [])])")" == "([], ['bed'])" ]] || fail "moderation recalled: $(curl -fsS "$WEB/api/v1/mix" | head -c 300)"
api POST /api/v1/snapshots/panel/recall '{"sections": ["strips"]}' >/dev/null
[[ "$(json "$WEB/api/v1/ducks" "len(d)")" == 1 ]] || fail "a strips-only recall changed the ducks"
api POST /api/v1/snapshots/panel/recall '{}' >/dev/null
[[ "$(json "$WEB/api/v1/mix" "([a['id'] for a in d.get('automixes', [])], [x['id'] for x in d.get('ducks', [])])")" == "(['panel'], [])" ]] || fail "panel recalled"
sleep 1
[[ "$(json "$WEB/api/v1/automixes/panel" "sorted(d['gains_db'])")" == "['guest', 'vox']" ]] || fail "the recalled auto-mix does not run"
pass "snapshots: a moderation (duck) and a panel (auto-mix) recalled in turn; a strips-only recall leaves them"

code=$(curl -s -o /dev/null -w '%{http_code}' -X PUT -H 'content-type: application/json' "$WEB/api/v1/vad/vox" -d '{"threshold": 0.99}')
[[ "$code" == 400 ]] || fail "threshold 0.99 answered $code"
code=$(curl -s -o /dev/null -w '%{http_code}' -X POST -H 'content-type: application/json' "$WEB/api/v1/ducks" -d '{"keys": ["vox"], "targets": ["vox"]}')
[[ "$code" == 400 ]] || fail "a duck of its own key answered $code"
code=$(curl -s -o /dev/null -w '%{http_code}' -X POST -H 'content-type: application/json' "$WEB/api/v1/automixes" -d '{"members": ["guest"]}')
[[ "$code" == 400 ]] || fail "a strip in two auto-mixes answered $code"
pass "refused: a threshold of 0.99, a duck of its own key, a strip in two auto-mixes (400)"

api POST /api/v1/ducks '{"id": "bed", "name": "Bed", "keys": ["vox"], "targets": ["t1"]}' >/dev/null
api DELETE /api/v1/strips/vox >/dev/null
[[ "$(json "$WEB/api/v1/automixes/panel" "[m['signal'] for m in d['members']]")" == "['guest']" ]] || fail "a deleted strip stays in its auto-mix"
[[ "$(json "$WEB/api/v1/ducks/bed" "d['keys']")" == "[]" ]] || fail "a deleted strip stays a duck key"
[[ "$(json "$WEB/api/v1/vad" "[v['signal'] for v in d]")" == "['guest']" ]] || fail "a deleted strip stays analysed"
pass "a deleted strip leaves its auto-mix, its ducks and the VAD"

rc=0
stop_mixer || rc=$?
[[ $rc == 143 ]] || fail "exit code $rc"
pass auto
