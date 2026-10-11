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
rc=0
stop_mixer || rc=$?
[[ $rc == 143 ]] || fail "exit code $rc"
pass lifecycle
