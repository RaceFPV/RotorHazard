#!/bin/bash
# Drive the connector through the bridge control channel: the plugin's own
# socket_listen handlers, called with the payloads FPVTrackSide would send.
set -uo pipefail
cd "$HOME/git/RotorHazard" || exit 1

PORT=5883
CTRL=5099
DATA=$(mktemp -d /tmp/rh-drive.XXXXXX)
printf '%s' '{"SECRETS":{"ADMIN_USERNAME":"","ADMIN_PASSWORD":""},"GENERAL":{"HTTP_PORT":5883,"DEBUG":false,"MOCK_NODE_SIGNAL":"2"}}' > "$DATA/config.json"
mkdir -p "$DATA/plugins"
cp -r /tmp/plugin-fpv/custom_plugins/rh_connector_trackside "$DATA/plugins/"

RH_DATA_DIR="$DATA" RH_NODES=8 ./rust/target/release/rh-server > "$DATA/server.log" 2>&1 &
SPID=$!
for i in $(seq 1 40); do sleep 0.5; curl -sf "http://127.0.0.1:$PORT/api/status" >/dev/null 2>&1 && break; done

"$HOME/.venvs/rh/bin/python" -u rust/plugin-bridge/rhapi_bridge.py \
    --server "http://127.0.0.1:$PORT" --plugins-dir "$DATA/plugins" \
    --plugins rh_connector_trackside --control-port "$CTRL" > "$DATA/bridge.log" 2>&1 &
BPID=$!
for i in $(seq 1 30); do sleep 0.5; curl -sf "http://127.0.0.1:$CTRL/" >/dev/null 2>&1 && break; done

say() { echo "=== $1"; }
say "listeners"
curl -s "http://127.0.0.1:$CTRL/" | head -c 600; echo

say "ts_server_info"
curl -s -X POST "http://127.0.0.1:$CTRL/" -d '{"event":"ts_server_info","args":null}' | head -c 700; echo

say "ts_race_stage (2 pilots)"
curl -s -X POST "http://127.0.0.1:$CTRL/" -d '{"event":"ts_race_stage","args":{"p":["Alice","Bob"],"p_id":["ts1","ts2"],"race_id":77,"race_number":1,"round_number":1,"bracket":null}}' | head -c 900; echo

say "ts_race_stop"
curl -s -X POST "http://127.0.0.1:$CTRL/" -d '{"event":"ts_race_stop","args":null}' | head -c 300; echo

say "ts_get_lean_mode"
curl -s -X POST "http://127.0.0.1:$CTRL/" -d '{"event":"ts_get_lean_mode","args":null}' | head -c 300; echo

say "race status after stage"
curl -s "http://127.0.0.1:$PORT/api/status" | head -c 200; echo

say "database check"
"$HOME/.venvs/rh/bin/python" - "$DATA/database.db" <<'PY'
import sqlite3, sys
db = sqlite3.connect(sys.argv[1])
print("heats:", db.execute("select id, note from heat order by id").fetchall())
print("pilots:", db.execute("select id, callsign from pilot order by id").fetchall())
print("slots:", db.execute(
    "select heat_id, node_index, pilot_id from heat_node where pilot_id is not null order by node_index").fetchall())
db.close()
PY

say "bridge errors"
grep -i 'error\|traceback\|AttributeError\|Unknown RHAPI' "$DATA/bridge.log" | head -8

kill "$BPID" 2>/dev/null || true
kill "$SPID" 2>/dev/null || true
wait 2>/dev/null || true
