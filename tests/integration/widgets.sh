#!/usr/bin/env bash
# Widgets for operator screens (§12.6, Appendix B.2): /widgets JSON and CORS (the
# listed origin only, Vary: Origin), widget pages with the CSP and no
# X-Frame-Options, 400/404/405, and a framing check in headless Chrome from an
# allowed and a refused origin (web/tests/widgets.mjs). Needs `npm ci` in web/ and
# Chrome. Screenshots: $IT_OUT/widgets/.
# shellcheck source=tests/integration/lib.sh
source "$(dirname "$0")/lib.sh"

FLOW=88888888-8888-4888-8888-888888888888
DOMAIN=99999999-9999-4999-8999-999999999999
ALLOWED_PORT=${WIDGET_TEST_PORT:-8198}
REFUSED_PORT=$((ALLOWED_PORT + 1))
ALLOWED="http://127.0.0.1:$ALLOWED_PORT"
REFUSED="http://localhost:$REFUSED_PORT"
start_tone "$WORK/mxl/tones" "$FLOW" --domain-id "$DOMAIN" --channels 2
wait_until 10 "tone flow" test -d "$WORK/mxl/tones/$FLOW.mxl-flow"
start_mixer NMOS_SEED=it-widgets WIDGET_FRAME_ANCESTORS="'self' $ALLOWED"
route 1 "$DOMAIN" "$FLOW"
wait_until 5 "input 1 running" is_state 1 running
for s in "vox:Vox:1" "gtr:Guitar:2" "keys:Keys:1"; do
    IFS=: read -r id name ch <<<"$s"
    api POST /api/v1/strips "{\"id\": \"$id\", \"name\": \"$name\", \"format\": \"mono\", \"patch\": [{\"input\": 1, \"channel\": $ch}]}" >/dev/null
done
api POST /api/v1/buses '{"id": "main", "name": "Main", "format": "stereo", "loudness": true}' >/dev/null
api PUT /api/v1/sends/vox/main >/dev/null
api PUT /api/v1/sends/gtr/main >/dev/null
api PUT /api/v1/layouts '[{"name": "Show", "pages": [{"name": "Talent", "signals": ["vox", "gtr", "keys"]}, {"name": "Main", "signals": ["main"]}]}]' >/dev/null
api POST /api/v1/monitors '{"id": "main", "name": "Main", "format": "stereo", "source": {"signal": "main"}}' >/dev/null
api POST /api/v1/talkback/destinations '{"id": "studio", "name": "Studio", "bus": "main"}' >/dev/null
api POST /api/v1/strips/vox/chain '{"id": "comp", "type": "compressor", "threshold_db": -30, "ratio": 4, "knee_db": 0, "attack_ms": 1, "release_ms": 200, "makeup_db": 0, "detector": "peak"}' >/dev/null

headers() { curl -s -o /dev/null -D - "$@" | tr -d '\r'; }
ids=$(json "$WEB/widgets" "','.join(w['id'] for w in d)")
[[ "$ids" == strip,fader-bank,loudness,monitor,talkback ]] || fail "widgets $ids"
[[ "$(json "$WEB/widgets" "d[0]['params']['properties']['strip']['enum']")" == "['vox', 'gtr', 'keys', 'main']" ]] || fail "strip params"
h=$(headers -H "Origin: $ALLOWED" "$WEB/widgets")
grep -qi "^access-control-allow-origin: $ALLOWED$" <<<"$h" || fail "no CORS for the listed origin: $h"
grep -qi "^vary: origin" <<<"$h" || fail "Vary: Origin"
h=$(headers -H "Origin: $REFUSED" "$WEB/widgets")
! grep -qi "^access-control-allow-origin" <<<"$h" || fail "CORS for an unlisted origin"
code=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$WEB/widgets")
h=$(headers -X POST "$WEB/widgets")
if [[ "$code" != 405 ]] || ! grep -qi "^allow: GET" <<<"$h"; then
    fail "POST /widgets: $code"
fi
pass "/widgets: strip, fader-bank, loudness, monitor, talkback; CORS for the listed origin only, Vary: Origin; 405 with Allow: GET"

h=$(headers "$WEB/widget/strip?strip=vox&theme=transparent")
grep -q "^HTTP/1.1 200" <<<"$h" || fail "widget page: $h"
grep -qi "^content-security-policy: frame-ancestors 'self' $ALLOWED$" <<<"$h" || fail "CSP: $h"
! grep -qi "^x-frame-options" <<<"$h" || fail "X-Frame-Options on a widget page"
status() { curl -s -o /dev/null -w '%{http_code}' "$WEB/widget/$1"; }
for case in "strip?strip=nope:400" "strip:400" "strip?strip=vox&theme=pink:400" "fader-bank?layout=Show&page=3:400" \
    "fader-bank?layout=Nope&page=1:400" "fader-bank?layout=Show:400" "fader-bank?strips=vox,gtr:200" \
    "fader-bank?layout=Show&page=2:200" "fader-bank?layout=Show&page=Talent:200" "loudness?signal=main:200" "monitor?monitor=main:200" "monitor?monitor=nope:400" "vca?vca=1:404" "nope:404"; do
    [[ "$(status "${case%:*}")" == "${case##*:}" ]] || fail "/widget/${case%:*}: $(status "${case%:*}"), not ${case##*:}"
done
pass "widget pages: CSP frame-ancestors, no X-Frame-Options; 400 for bad parameters, 404 for unknown widgets"

PARENTS="$WORK/parents"
mkdir -p "$PARENTS"
cp "$(dirname "$0")/frame.html" "$PARENTS/"
python3 -m http.server "$ALLOWED_PORT" --bind 127.0.0.1 --directory "$PARENTS" >/dev/null 2>&1 &
PIDS+=($!)
python3 -m http.server "$REFUSED_PORT" --bind 127.0.0.1 --directory "$PARENTS" >/dev/null 2>&1 &
PIDS+=($!)
wait_until 5 "parent pages" http_ok "$ALLOWED/frame.html"
wait_until 5 "parent pages" http_ok "$REFUSED/frame.html"
node "$ROOT/web/tests/widgets.mjs" "$WEB" "$ALLOWED" "$REFUSED" "$IT_OUT/widgets" || fail "web/tests/widgets.mjs"

rc=0
stop_mixer || rc=$?
[[ $rc == 143 ]] || fail "exit code $rc"
pass widgets
