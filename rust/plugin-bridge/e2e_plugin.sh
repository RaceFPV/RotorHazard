#!/bin/bash
# End-to-end plugin transport: server + bridge + a client playing FPVTrackSide.
set -uo pipefail
cd "$(cd "$(dirname "$0")/../.." && pwd)" || exit 1
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PY="${PYTHON_BIN:-$HOME/.venvs/rh/bin/python}"

PORT=5884
DATA=$(mktemp -d /tmp/rh-e2e.XXXXXX)
printf '%s' '{"SECRETS":{"ADMIN_USERNAME":"","ADMIN_PASSWORD":""},"GENERAL":{"HTTP_PORT":5884,"DEBUG":false,"MOCK_NODE_SIGNAL":"2"}}' > "$DATA/config.json"
mkdir -p "$DATA/plugins"
cp -r "${PLUGIN_DIR:-/tmp/plugin-fpv/custom_plugins/rh_connector_trackside}" "$DATA/plugins/"

# The server carries the bridge inside the binary and starts it when
# <data_dir>/plugins has plugins, which is the path users get. PYTHON_BIN puts
# the venv's python first so the spawned bridge can import socketio.
PATH="$(dirname "$PY"):$PATH" RH_DATA_DIR="$DATA" RH_NODES=8 \
    ./rust/target/release/rh-server > "$DATA/server.log" 2>&1 &
SPID=$!
for i in $(seq 1 40); do sleep 0.5; curl -sf "http://127.0.0.1:$PORT/api/status" >/dev/null 2>&1 && break; done

for i in $(seq 1 40); do
    sleep 0.5
    grep -q "loaded plugin: rh_connector_trackside" "$DATA/bridge.log" 2>/dev/null && break
done

echo "=== bridge startup (spawned by the server)"
head -6 "$DATA/bridge.log"

echo "=== client (plays FPVTrackSide)"
"$PY" "$HERE/fpvtrackside_client.py" "http://127.0.0.1:$PORT"

echo "=== database"
"$PY" - "$DATA/database.db" <<'PY'
import sqlite3, sys
db = sqlite3.connect(sys.argv[1])
print("heats :", db.execute("select id, note from heat order by id").fetchall())
print("pilots:", db.execute("select id, callsign from pilot order by id").fetchall())
print("slots :", db.execute("select heat_id, node_index, pilot_id from heat_node where pilot_id is not null order by node_index").fetchall())
print("races :", db.execute("select id, heat_id, round_id, class_id from saved_race_meta order by id").fetchall())
print("laps  :", db.execute("select race_id, pilot_id, lap_time_stamp from saved_race_lap order by id").fetchall())
print("freqs :", db.execute("select id, frequencies from profiles").fetchall())
db.close()
import json, os
cfg = json.load(open(os.path.join(os.path.dirname(sys.argv[1]), "config.json")))
print("UI.timerName      :", cfg.get("UI", {}).get("timerName"))
print("LED.seatColors    :", cfg.get("LED", {}).get("seatColors"))
opt = sqlite3.connect(sys.argv[1]).execute(
    "select option_value from global_settings where option_name = '_ts_lean_mode'").fetchall()
print("_ts_lean_mode     :", opt)
db.close()
PY

echo "=== bridge errors"
grep -i "error\|traceback" "$DATA/bridge.log" | head -6

kill "$SPID" 2>/dev/null || true
wait 2>/dev/null || true
