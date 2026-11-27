#!/usr/bin/env python3
"""Drive the server's set_frequency event and report what the operator saw.

Usage: c5mk_client.py <url> <node> <frequency> [<frequency> ...]
Prints one JSON line per step: {"frequency": N, "priority": [...], "ok": bool}
"""

import json
import sys
import time

import socketio

url = sys.argv[1]
node = int(sys.argv[2])
freqs = [int(f) for f in sys.argv[3:]]

messages = []
sio = socketio.Client()


@sio.on("priority_message")
def on_priority(msg):
    messages.append((msg or {}).get("message") if isinstance(msg, dict) else str(msg))


sio.connect(url, wait_timeout=15)
time.sleep(1.0)

for freq in freqs:
    messages.clear()
    sio.emit("set_frequency", {"node": node, "frequency": freq})
    time.sleep(1.2)
    failed = [m for m in messages if m and "Failed to set frequency" in m]
    print(json.dumps({"frequency": freq, "priority": messages, "failed": bool(failed)}))

sio.disconnect()
