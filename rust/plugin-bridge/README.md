# RustHazard plugin bridge (optional sidecar)

Runs **third-party RHAPI plugins** against the Rust server without any
Python inside the server itself.

## How it works

```
┌──────────────┐   socket.io (localhost)   ┌──────────────────┐
│  Rust server │ ◄───────────────────────► │  rhapi_bridge.py │──► user plugins
│  (pure Rust) │    events out, RPC in     │  (RHAPI shim)    │    (unmodified)
└──────────────┘                           └──────────────────┘
```

- The server exposes a small RPC surface (`rhapi_call` with ack) that
  mirrors the RHAPI object graph (`db.*`, `race.*`, `ui.*`).
- The bridge loads plugins from a directory (unmodified Python), hands
  them a `rhapi` object whose attribute calls proxy to the server, and
  delivers server broadcasts to their `rhapi.events.on()` hooks.
- The bridge is optional: without it the server runs as if no plugins
  are installed. Plugins never affect race timing — event hooks run
  out-of-process.

## Usage

```
mkdir plugins/mydemo            # manifest.json + __init__.py
python3 rust/plugin-bridge/rhapi_bridge.py \
    --server http://127.0.0.1:5000 --plugins-dir ./plugins
```

Plugin layout is the standard RotorHazard plugin layout:

```
plugins/mydemo/
    manifest.json        # {"name": "My Demo", ...}
    __init__.py          # def initialize(rhapi): ...
```

Example plugin:

```python
# plugins/mydemo/__init__.py
def initialize(rhapi):
    rhapi.events.on('raceStart', lambda args:
        rhapi.ui.message_notify('Race started!'))
```

## Supported RPC surface (grows on demand)

- `db.pilots / pilot / pilot_add / pilot_alter`
- `db.heats / heat_slots / raceclasses / raceformats`
- `db.saved_races / saved_race_laps`
- `db.get_option / set_option`
- `race.status / pilots / results`
- `ui.message_speak / message_notify / message_alert / clear_messages`

New methods are two-part edits: a match arm in
`rust/server/src/pluginrpc.rs` and (if needed) a shim node in the
bridge — third-party plugins discover the surface through the same
object graph, so no plugin changes are needed when methods are added.
