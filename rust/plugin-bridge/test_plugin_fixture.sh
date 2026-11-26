#!/bin/bash
# Fast, self-contained plugin test: the in-repo fixture plugin against a real
# server. No network, no external clone — safe to run in every CI job and in
# the upstream-sync gate.
#
# The bridge is NOT started here: the server carries it inside the binary and
# spawns it when <data_dir>/plugins has plugins. That is the shipping path, so
# this script is also the test for it.
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$(cd "$HERE/../.." && pwd)" || exit 1
PY="${PYTHON_BIN:-$HOME/.venvs/rh/bin/python}"

PORT="${FIXTURE_PORT:-5888}"
DATA=$(mktemp -d /tmp/rh-fixture.XXXXXX)
mkdir -p "$DATA/plugins"
cp -r "$HERE/tests/fixture_plugin" "$DATA/plugins/"

printf '%s' "{\"SECRETS\":{\"ADMIN_USERNAME\":\"\",\"ADMIN_PASSWORD\":\"\"},\"GENERAL\":{\"HTTP_PORT\":$PORT,\"DEBUG\":false,\"MOCK_NODE_SIGNAL\":\"2\"}}" > "$DATA/config.json"

# The client needs python-socketio; the server finds its own python for the bridge.
PATH="$HOME/.venvs/rh/bin:$PATH" PYTHON_BIN="$PY" RH_DATA_DIR="$DATA" RH_NODES=8 \
    ./rust/target/release/rh-server > "$DATA/server.log" 2>&1 &
SPID=$!
for i in $(seq 1 40); do sleep 0.5; curl -sf "http://127.0.0.1:$PORT/api/status" >/dev/null 2>&1 && break; done

# Wait for the server-spawned bridge to load the plugin.
LOADED=0
for i in $(seq 1 40); do
    sleep 0.5
    if grep -q "loaded plugin: fixture_plugin" "$DATA/plugin-bridge.log" 2>/dev/null; then
        LOADED=1
        break
    fi
done
if [ "$LOADED" != "1" ]; then
    echo "FAIL: the server did not start the bridge (no 'loaded plugin' in plugin-bridge.log)"
    echo "--- server log"
    tail -20 "$DATA/server.log"
    echo "--- bridge log"
    tail -20 "$DATA/plugin-bridge.log" 2>/dev/null
    kill "$SPID" 2>/dev/null || true
    rm -rf "$DATA"
    exit 1
fi

"$PY" "$HERE/tests/fixture_client.py" "http://127.0.0.1:$PORT"
RC=$?

if [ "$RC" != "0" ]; then
    echo "--- bridge log"
    tail -40 "$DATA/plugin-bridge.log" 2>/dev/null
    echo "--- server log"
    tail -20 "$DATA/server.log"
fi

kill "$SPID" 2>/dev/null || true
wait 2>/dev/null || true
rm -rf "$DATA"
exit "$RC"
