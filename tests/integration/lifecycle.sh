#!/usr/bin/env bash
# The registry is reached by a DNS name (localhost). G14: start -> ready -> SIGTERM -> exit 143, node deregistered, own domain removed
# (MXL_CLEANUP_ON_EXIT=true). Also G9: a connection survives a restart.
# shellcheck source=tests/integration/lib.sh
source "$(dirname "$0")/lib.sh"

start_registry
start_mixer NMOS_SEED=it-lifecycle NMOS_REGISTRY_ADDRESS=localhost NMOS_REGISTRY_PORT=8010 MXL_CLEANUP_ON_EXIT=true
wait_until 30 "readiness (registered and listed by the Query API)" http_ok "$WEB/readyz"
node=$(json "$WEB/api/v1/status" "d['nmos']['node_id']")
domain_dir=$(json "$WEB/api/v1/status" "d['mxl']['output_domain_dir']")
[[ -f "$domain_dir/domain_def.json" ]] || fail "no domain_def.json in $domain_dir"
http_ok "http://$HOST_IP:8011/x-nmos/query/v1.3/nodes/$node" || fail "the Query API does not list $node"
pass "ready, node $node registered, output domain $domain_dir"

# G9: activate input 2 onto a flow, restart, and find the connection active again.
start_tone "$WORK/mxl/tones" 22222222-2222-4222-8222-222222222222
wait_until 10 "tone flow" test -d "$WORK/mxl/tones/22222222-2222-4222-8222-222222222222.mxl-flow"
route 2 "$(domain_id "$WORK/mxl/tones")" 22222222-2222-4222-8222-222222222222
wait_until 10 "input 2 running" is_state 2 running
rc=0
stop_mixer || rc=$?
[[ $rc == 143 ]] || fail "exit code $rc after SIGTERM, not 143"
code=$(curl -s -o /dev/null -w '%{http_code}' "http://$HOST_IP:8011/x-nmos/query/v1.3/nodes/$node")
[[ $code == 404 ]] || fail "node $node still in the registry (HTTP $code)"
[[ ! -e "$domain_dir" ]] || fail "own output domain $domain_dir not removed"
[[ -e "$WORK/mxl/tones" ]] || fail "another domain was removed"
pass "SIGTERM: exit 143, node deregistered, own domain removed"

start_mixer NMOS_SEED=it-lifecycle NMOS_REGISTRY_ADDRESS=localhost NMOS_REGISTRY_PORT=8010 MXL_CLEANUP_ON_EXIT=true
wait_until 15 "input 2 running after the restart" is_state 2 running
[[ "$(json "$WEB/api/v1/status" "d['nmos']['node_id']")" == "$node" ]] || fail "node id changed across the restart"
pass "the connection and the node id survive a restart"

# A failed start leaves no domain behind: a busy WEB_PORT exits 75 before the
# output domain is created.
rc=0
env HOST_ID=it-busy NMOS_HOST_ADDRESS="$HOST_IP" MXL_DOMAIN_SCAN_PATH="$WORK/mxl" \
    MIXER_STATE_DIR="$WORK/state-busy" NMOS_PORT=3352 MXL_OUTPUT_DOMAIN_DIR="$WORK/mxl/busy" \
    MEDIAMTX_RTSP_PORT=8974 MEDIAMTX_WHEP_PORT=8619 MEDIAMTX_ICE_UDP_PORT=8219 \
    "$MIXER_BIN" >>"$IT_OUT/lifecycle-mixer.log" 2>&1 || rc=$?
[[ $rc == 75 ]] || fail "busy WEB_PORT: exit $rc, not 75"
[[ ! -e "$WORK/mxl/busy" ]] || fail "busy WEB_PORT: the output domain was created"
rc=0
stop_mixer || rc=$?
[[ $rc == 143 ]] || fail "exit code $rc"
pass "a busy port exits 75 and creates no output domain"

# G1: a state directory that is not writable exits 78 (not for root, which may write anyway).
if [[ $(id -u) != 0 ]]; then
    mkdir -p "$WORK/state-ro" && chmod 555 "$WORK/state-ro"
    rc=0
    env HOST_ID=it-ro NMOS_HOST_ADDRESS="$HOST_IP" MXL_DOMAIN_SCAN_PATH="$WORK/mxl" \
        MIXER_STATE_DIR="$WORK/state-ro" MXL_OUTPUT_DOMAIN_DIR="$WORK/mxl/ro" \
        "$MIXER_BIN" >>"$IT_OUT/lifecycle-mixer.log" 2>&1 || rc=$?
    [[ $rc == 78 ]] || fail "read-only MIXER_STATE_DIR: exit $rc, not 78"
    [[ ! -e "$WORK/mxl/ro" ]] || fail "read-only MIXER_STATE_DIR: the output domain was created"
    pass "a read-only MIXER_STATE_DIR exits 78"
fi
rc=0
env HOST_ID=it-scan NMOS_HOST_ADDRESS="$HOST_IP" MXL_DOMAIN_SCAN_PATH="$WORK/missing" \
    MIXER_STATE_DIR="$WORK/state-scan" "$MIXER_BIN" >>"$IT_OUT/lifecycle-mixer.log" 2>&1 || rc=$?
[[ $rc == 78 && ! -e "$WORK/missing" ]] || fail "missing MXL_DOMAIN_SCAN_PATH: exit $rc, not 78"
pass "a missing MXL_DOMAIN_SCAN_PATH exits 78"

# G2: an existing domain_def.json with another id is kept, never overwritten.
mkdir -p "$WORK/mxl/kept"
echo '{"id": "77777777-7777-4777-8777-777777777777", "label": "kept", "description": "pre-made", "tags": {}}' \
    >"$WORK/mxl/kept/domain_def.json"
before=$(md5sum <"$WORK/mxl/kept/domain_def.json")
start_mixer NMOS_SEED=it-kept MXL_OUTPUT_DOMAIN_DIR="$WORK/mxl/kept"
[[ "$(json "$WEB/api/v1/status" "d['mxl']['output_domain_id']")" == 77777777-7777-4777-8777-777777777777 ]] ||
    fail "the existing domain id was not kept"
[[ "$(md5sum <"$WORK/mxl/kept/domain_def.json")" == "$before" ]] || fail "domain_def.json was rewritten"
rc=0
stop_mixer || rc=$?
[[ $rc == 143 ]] || fail "exit code $rc"
pass "an existing domain id is kept and its domain_def.json untouched"
pass lifecycle
