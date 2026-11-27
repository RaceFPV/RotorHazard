#!/bin/bash
# End-to-end: does the server drive a C5MK-style eight-seat board correctly?
#
# A pty stands in for the board (tests/fixtures/fake_c5mk.py, node API 36).
# The server discovers eight seats, applies its profile to each, confirms
# frequencies, and reports an untunable frequency instead of pretending.
set -uo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(git -C "$HERE" rev-parse --show-toplevel)"
BIN="$ROOT/rust/target/release/rh-server"
if [ -n "${PYTHON_BIN:-}" ]; then
    PY="$PYTHON_BIN"
else
    PY="$HOME/.venvs/rh/bin/python"
    [ -x "$PY" ] || PY=python3
fi
[ -x "$BIN" ] || { echo "build first: cargo build --release --manifest-path rust/server/Cargo.toml"; exit 1; }

WORK=$(mktemp -d /tmp/rh-c5mk.XXXXXX)
PORT_FILE="$WORK/port"
BOARD_LOG="$WORK/board.json"
DATA="$WORK/data"
mkdir -p "$DATA"
HTTP_PORT=$("$PY" - <<'PY'
import socket
s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()
PY
)

cleanup() {
    [ -n "${SERVER_PID:-}" ] && kill "$SERVER_PID" 2>/dev/null
    [ -n "${BOARD_PID:-}" ] && kill "$BOARD_PID" 2>/dev/null
    [ -n "${KEEP:-}" ] || rm -rf "$WORK"
}
trap cleanup EXIT

# 5900 is deliberately untunable on this fake, to exercise the register test.
"$PY" "$HERE/fake_c5mk.py" --port-file "$PORT_FILE" --log "$BOARD_LOG" \
    --seats 8 --api 36 --unsupported 5900 > "$WORK/board.out" 2>&1 &
BOARD_PID=$!
for _ in $(seq 1 80); do [ -s "$PORT_FILE" ] && break; sleep 0.1; done
PORT=$(cat "$PORT_FILE" 2>/dev/null)
[ -n "$PORT" ] || { echo "FAIL: no pty from the fake board"; cat "$WORK/board.out"; exit 1; }

cat > "$DATA/config.json" <<JSON
{"SECRETS": {"ADMIN_USERNAME": "", "ADMIN_PASSWORD": ""},
 "GENERAL": {"HTTP_PORT": $HTTP_PORT, "MOCK_NODE_SIGNAL": "0", "SERIAL_PORTS": ["$PORT"], "DEBUG": false}}
JSON

PATH="$HOME/.venvs/rh/bin:$PATH" RH_DATA_DIR="$DATA" setsid nohup "$BIN" \
    > "$WORK/server.log" 2>&1 < /dev/null &
SERVER_PID=$!
for _ in $(seq 1 120); do
    sleep 0.5
    curl -sf "http://127.0.0.1:$HTTP_PORT/api/status" > /dev/null 2>&1 && break
done

fail=0
require() { if ! grep -q "$1" "$WORK/server.log"; then echo "FAIL: $2"; fail=1; fi; }
require "Serial multi-node found.*count=8" "the board was not detected as an 8-seat multi-node"
require "Found 8 serial node(s)" "the server did not create eight nodes"

# The startup profile must reach the board exactly: every seat the profile
# defines gets its own frequency, and empty seats stay untouched. Compare
# against the database the server is using rather than a hardcoded band table.
"$PY" - "$BOARD_LOG" "$DATA/database.db" <<'PY' || fail=1
import json, sqlite3, sys, time

board_path, db_path = sys.argv[1], sys.argv[2]
deadline = time.time() + 25
while time.time() < deadline:
    try:
        board = json.load(open(board_path))
        profile = json.loads(sqlite3.connect(db_path)
                             .execute("select frequencies from profiles").fetchone()[0])
    except Exception:
        time.sleep(0.2); continue
    expected = [int(f or 0) for f in profile.get("f", [])]
    got = board.get("freq_values") or []
    programmed = [f for f in expected if f]
    if len(programmed) < 6:
        time.sleep(0.2); continue
    if got == expected:
        print("seats programmed:", got, "slot switches:", board["slot_switches"])
        sys.exit(0)
    time.sleep(0.2)
print("FAIL: board does not hold the profile:", board.get("freq_values"), "vs", expected)
sys.exit(1)
PY

# 5800 must be accepted silently; 5900 must raise the operator warning.
# Seat 1 starts on 5658 (tunable); 5740 stays tunable, 5900 does not.
OUT=$("$PY" "$HERE/c5mk_client.py" "http://127.0.0.1:$HTTP_PORT" 0 5740 5900)
echo "$OUT"
echo "$OUT" | "$PY" -c '
import json, sys
lines = [json.loads(l) for l in sys.stdin if l.strip()]
good = [l for l in lines if l["frequency"] == 5740]
bad = [l for l in lines if l["frequency"] == 5900]
ok = bool(good) and not good[0]["failed"] and bool(bad) and bad[0]["failed"]
print("PASS: untunable frequency reported" if ok else "FAIL: frequency failure not reported as expected")
sys.exit(0 if ok else 1)
' || fail=1

BOARD=$("$PY" -c 'import json,sys; b=json.load(open(sys.argv[1])); print(json.dumps({k: b[k] for k in ("register_tests","register_failures","power_downs","freq_values")}))' "$BOARD_LOG")
echo "board counters: $BOARD"
if [ "$fail" -eq 0 ]; then
    echo "PASS: C5MK-style multi-node board"
else
    echo "server log:"; tail -25 "$WORK/server.log"
    exit 1
fi
