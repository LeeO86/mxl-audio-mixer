#!/usr/bin/env bash
# The control engine (§8) through MXL: gain sharing sample-exact on two equal
# tones (NOM, a weight, bypass, a muted member, disabled; no plan swap); the VAD
# on speech (espeak-ng) and not on a tone; a tone ducked under the speech
# (depth, onset latency, release) measured on the output; an auto-mix of two
# talkers taking turns, with last-mic-hold; auto-level (a tone to its target on
# the output, the speech offset, the freeze); a ratio duck (the ratio held,
# interview passthrough); deleted signals leave it all.
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
[[ "$(json "$WEB/api/v1/vad/vox" "d['state']['detector']")" == silero ]] || fail "the default detector is not Silero"
api PUT /api/v1/vad/t1 '{"detector": "features"}' >/dev/null
sleep 0.3
[[ "$(json "$WEB/api/v1/vad/t1" "d['state']['detector']")" == features ]] || fail "the feature detector was not switched on"
pass "VAD: speech onsets once per period on vox (Silero), none on the tone; the feature detector on request; vad_active in the metrics"
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
api DELETE /api/v1/automixes/panel >/dev/null

# Auto-level (§8.2): t1 (1 kHz, −23 LUFS) to −30 LUFS, instantly (attack and
# release 0); measured on the programme's left channel.
api PUT /api/v1/autolevels/t1 '{"target_lufs": -30, "attack_ms": 0, "release_ms": 0}' >/dev/null
sleep 4
"$PROBE_BIN" --domain "$OWN" --flow "$OUT_FLOW" --seconds 1 --dump "$WORK/level.txt" >/dev/null
read -r LEVEL GAIN < <(json "$WEB/api/v1/autolevels/t1" "d['state']['level_lufs'], d['state']['gain_db']")
python3 - "$WORK/level.txt" "$LEVEL" "$GAIN" <<'EOF' || fail "auto-level"
import math, sys
x = [float(l.split()[1]) for l in open(sys.argv[1])]
level, gain = float(sys.argv[2]), float(sys.argv[3])
rms = 20 * math.log10(math.sqrt(sum(v * v for v in x) / len(x)))
# A 1 kHz tone's loudness is its RMS level (the K filter's +0.69 dB there is the −0.691).
assert abs(level + 23.0) < 0.1, ("t1's level", level)
assert abs(gain + 7.0) < 0.1, ("gain", gain)
assert abs(rms + 30.0) < 0.1, ("the output", rms)
print(f"t1 at {level} LUFS, gain {gain} dB, the output at {rms:.2f} dBFS RMS")
EOF
api DELETE /api/v1/autolevels/t1 >/dev/null
# vox (speech in the first half of each 6 s period): +2 LU while it speaks,
# frozen in the silence.
api PUT /api/v1/autolevels/vox >/dev/null
python3 - "$WEB" <<'EOF' || fail "auto-level on speech"
import json, sys, time, urllib.request
web = sys.argv[1]
get = lambda p: json.load(urllib.request.urlopen(web + p))
rows = []
until = time.time() + 12.5
while time.time() < until:
    rows.append(get("/api/v1/autolevels/vox")["state"])
    time.sleep(0.05)
speech = [r for r in rows if r["speech"]]
frozen = [r for r in rows if r["frozen"]]
assert speech and all(r["target_lufs"] == -21.0 for r in speech), speech[:3]
assert all(r["target_lufs"] == -23.0 for r in rows if not r["speech"])
assert len(frozen) > 20, len(frozen)
# Frozen stretches keep the gain.
moved = [b["gain_db"] - a["gain_db"] for a, b in zip(rows, rows[1:]) if a["frozen"] and b["frozen"]]
assert max(map(abs, moved)) == 0, max(map(abs, moved))
print(f"{len(speech)} of {len(rows)} polls with speech (target −21), {len(frozen)} frozen with the gain held; last {rows[-1]}")
EOF
api DELETE /api/v1/autolevels/vox >/dev/null
pass "auto-level: a tone to −30 LUFS on the output; speech gets +2 LU; the gain holds in the silence"

# A ratio duck: the commentator vox over a bed of t1 (the tone) and the guest
# (an interview in the bed, speaking in vox's pauses).
api POST /api/v1/ducks '{"id": "com", "name": "Commentary", "keys": ["vox"], "targets": ["t1", "guest"], "mode": "ratio"}' >/dev/null
# t1 and the guest are tapped from this plan on: 3 s until their short-term
# loudness is there.
sleep 3.5
python3 - "$WEB" "$IT_OUT/auto-ratio.json" <<'EOF' || fail "ratio duck (the polls: $IT_OUT/auto-ratio.json)"
import json, sys, time, urllib.request
web = sys.argv[1]
get = lambda p: json.load(urllib.request.urlopen(web + p))
rows = []
until = time.time() + 18.5
while time.time() < until:
    d = get("/api/v1/ducks/com")
    rows.append((d["control"], d["gain_db"]))
    time.sleep(0.05)
json.dump(rows, open(sys.argv[2], "w"))
c = [r[0] for r in rows]
states = {x["state"] for x in c}
assert {"duck", "interview"} <= states, states
for x, g in rows:
    assert abs(g + x["reduction_db"]) < 0.02, (g, x)
# (Right after vox's 3.7 s pauses its short-term loudness is silence: no ratio.)
duck = [x for x in c if x["state"] == "duck" and x["reduction_db"] > 1 and x["ratio_lu"] is not None]
held = [x for x in duck if abs(x["ratio_lu"] - 5) < 2]
median = sorted(x["ratio_lu"] for x in duck)[len(duck) // 2] if duck else None
# The speech's short-term loudness builds up after each 3.7 s pause and the
# bed's still holds the guest's last turn: the stages follow, about 1 LU over.
assert len(duck) > 20 and abs(median - 5) < 1.5, (len(duck), median, duck[:5])
assert max(x["reduction_db"] for x in duck) > 8, duck[:5]
# The guest alone: back fast (interview passthrough). A turn is the guest's
# speech: the interview polls, a new turn after a second or more without one (a
# flicker of the commentator's VAD does not split it). In every turn of 1.5 s
# or more the bed must have come back below 1 dB.
turns, cur, gap = [], [], 0
for x in c:
    if x["state"] == "interview":
        cur.append(x)
        gap = 0
    else:
        gap += 1
        if gap >= 20 and cur:
            turns.append(cur)
            cur = []
if cur:
    turns.append(cur)
long = [t for t in turns if len(t) >= 30]
ends = [round(min(x["reduction_db"] for x in t), 2) for t in long]
assert ends and max(ends) < 1.0, (ends, "".join(x["state"][0] for x in c))
print(f"states {sorted(states)}; ducking: median ratio {median} LU, {len(held)} of {len(duck)} polls within 2 LU of the 5 LU target "
      f"(reductions {min(x['reduction_db'] for x in duck)}–{max(x['reduction_db'] for x in duck)} dB); "
      f"after the guest's turns {ends} dB")
EOF
api PATCH /api/v1/ducks/com '{"ratio": {"bed_voice": false}}' >/dev/null
[[ "$(json "$WEB/api/v1/ducks/com" "d['ratio']['bed_voice'], d['ratio']['target_lu']")" == "False 5.0" ]] || fail "a partial ratio patch"
[[ "$(json "$WEB/api/v1/vad" "'guest' in [v['signal'] for v in d]")" == False ]] || fail "the bed is still analysed"
code=$(curl -s -o /dev/null -w '%{http_code}' -X PATCH -H 'content-type: application/json' "$WEB/api/v1/ducks/com" -d '{"ratio": {"target_lu": 13}}')
[[ "$code" == 400 ]] || fail "a ratio target of 13 LU answered $code"
api DELETE /api/v1/ducks/com >/dev/null
api POST /api/v1/automixes '{"id": "panel", "name": "Panel", "members": ["vox", "guest"]}' >/dev/null
pass "ratio duck: the bed held about 5 LU under the commentator; the guest speaking alone brings it back (interview passthrough); bed_voice off stops analysing the bed"

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
