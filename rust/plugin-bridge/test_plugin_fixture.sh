#!/bin/bash
# Fast, self-contained plugin test: the in-repo fixture plugin against a real
# server. No network, no external clone — safe to run in every CI job and in
# the upstream-sync gate.
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$(cd "$HERE/../.." && pwd)" || exit 1
PY="${PYTHON_BIN:-$HOME/.venvs/rh/bin/python}"

PORT="${FIXTURE_PORT:-5888}"
DATA=$(mktemp -d /tmp/rh-fixture.XXXXXX)
mkdir -p "$DATA/plugins"
cp -r "$HERE/tests/fixture_plugin" "$DATA/plugins/"

printf '%s' "{\"SECRETS\":{\"ADMIN_USERNAME\":\"\",\"ADMIN_PASSWORD\":\"\"},\"GENERAL\":{\"HTTP_PORT\":$PORT,\"DEBUG\":false,\"MOCK_NODE_SIGNAL\":\"2\"}}" > "$DATA/config.json"

RH_DATA_DIR="$DATA" RH_NODES=8 ./rust/target/release/rh-server > "$DATA/server.log" 2>&1 &
SPID=$!
for i in $(seq 1 40); do sleep 0.5; curl -sf "http://127.0.0.1:$PORT/api/status" >/dev/null 2>&1 && break; done

"$PY" -u "$HERE/rhapi_bridge.py" \
    --server "http://127.0.0.1:$PORT" --plugins-dir "$DATA/plugins" \
    --plugins fixture_plugin > "$DATA/bridge.log" 2>&1 &
BPID=$!
sleep 4

"$PY" "$HERE/tests/fixture_client.py" "http://127.0.0.1:$PORT"
RC=$?

if [ "$RC" != "0" ]; then
    echo "--- bridge log"
    tail -40 "$DATA/bridge.log"
    echo "--- server log"
    tail -20 "$DATA/server.log"
fi

kill "$BPID" 2>/dev/null || true
kill "$SPID" 2>/dev/null || true
wait 2>/dev/null || true
rm -rf "$DATA"
exit "$RC"
