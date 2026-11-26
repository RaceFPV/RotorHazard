#!/bin/bash
# Fail-fast probe: can the Rust server load a real RHAPI plugin via the bridge?
set -uo pipefail
cd "$HOME/git/RotorHazard" || exit 1

PORT=5882
DATA=$(mktemp -d /tmp/rh-plug.XXXXXX)
printf '%s' '{"SECRETS":{"ADMIN_USERNAME":"","ADMIN_PASSWORD":""},"GENERAL":{"HTTP_PORT":5882,"DEBUG":false,"MOCK_NODE_SIGNAL":"2"}}' > "$DATA/config.json"
mkdir -p "$DATA/plugins"
cp -r /tmp/plugin-fpv/custom_plugins/rh_connector_trackside "$DATA/plugins/" 2>/dev/null || { echo "plugin copy failed"; exit 1; }

RH_DATA_DIR="$DATA" RH_NODES=8 ./rust/target/release/rh-server > "$DATA/server.log" 2>&1 &
SPID=$!
for i in $(seq 1 40); do sleep 0.5; curl -sf "http://127.0.0.1:$PORT/api/status" >/dev/null 2>&1 && break; done

"$HOME/.venvs/rh/bin/python" -u rust/plugin-bridge/rhapi_bridge.py \
    --server "http://127.0.0.1:$PORT" --plugins-dir "$DATA/plugins" \
    --plugins rh_connector_trackside > "$DATA/bridge.log" 2>&1 &
BPID=$!
sleep 10
kill "$BPID" 2>/dev/null || true
kill "$SPID" 2>/dev/null || true
wait 2>/dev/null || true

echo "=== bridge.log ==="
head -40 "$DATA/bridge.log"
echo "=== server.log (plugin lines) ==="
grep -i 'plugin\|rhapi' "$DATA/server.log" | head -10
echo "=== server log tail ==="
tail -4 "$DATA/server.log"
