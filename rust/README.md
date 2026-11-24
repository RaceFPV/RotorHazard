# RotorHazard server — Rust implementation

A Rust rewrite of the RotorHazard race-timing server. Wire-compatible
with the Python server: same database schema, config.json, Socket.IO
protocol, and serial-node firmware protocol.

## Status

- Single-binary server: embedded templates/static, SQLite database,
  Socket.IO + JSON API, admin auth, calibration, LED effects, cluster
  (secondary-timer) support, Linux sensors.
- Hardware: serial RH nodes (STM32/ESP32), runtime USB hot-plug, node
  firmware updates (AN3155 bootloader), mock nodes with simulated RSSI.
- The Python plugin ecosystem is not yet hosted; bundled LED strip
  effects, Linux sensors, and cluster/VRx frameworks are ported natively.

## Building

```
cargo build --release --manifest-path rust/server/Cargo.toml
```

Release bundles (including an aarch64/Pi bundle when the cross target is
installed — `rustup target add aarch64-unknown-linux-gnu`):

```
./rust/build-release.sh [version-tag]
```

Or build natively on the Pi with the same script.

## Running

```
cd rust/target/release        # or an extracted dist bundle
./rh-server                   # data dir auto-resolved (see below)
```

Environment:

| Variable | Purpose |
|---|---|
| `RH_DATA_DIR` | data directory (config.json + database.db) |
| `RH_PORT` | override HTTP port |
| `RH_NODES` | mock node count when no hardware is present (min 8) |

Data-dir resolution order: `RH_DATA_DIR` → `./data` → `~/rh-data` →
serial-discovery default, matching the Python install layout. An
existing Python `data/` directory works as-is (same database schema and
config.json keys); unknown config keys are ignored.

## Upgrading from the Python server

1. Stop the Python server.
2. Point the Rust server at the existing data directory:
   `RH_DATA_DIR=/home/pi/rh-data ./rh-server` (or copy `data/` next to
   the binary).
3. The database migrates in place (SQLite, same schema); a config.json
   with `SERIAL_PORTS` enables hardware nodes:

```json
{
  "SECRETS": { "ADMIN_USERNAME": "", "ADMIN_PASSWORD": "" },
  "GENERAL": { "SERIAL_PORTS": ["ttyAMA0"] }
}
```

## systemd service (Raspberry Pi)

```ini
[Unit]
Description=RotorHazard Rust Server
After=network.target

[Service]
Type=simple
User=pi
Environment=RH_DATA_DIR=/home/pi/rh-data
ExecStart=/home/pi/rh-rust/rh-server
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
```

The serial device must be accessible to the service user — for the
primary UART add a udev rule:

```
KERNEL=="ttyAMA0", GROUP="dialout", MODE="0660"
```

and remove `console=serial0,115200` from `/boot/firmware/cmdline.txt`
plus disable `serial-getty@ttyAMA0` so the login console does not own
the port.

## Docker

`rust/Dockerfile` builds a container image (data mounted at /data).

## Test suites

- `cargo test` (rust/server): 50+ unit tests, including the calibration
  engine validated differentially against Python-generated fixtures.
- `src/tests/protocol/`: language-neutral protocol characterization
  suite (68 tests) — passes against the Python server and this
  implementation:

```
cargo build --manifest-path rust/server/Cargo.toml
RH_TEST_URL=http://127.0.0.1:<port> python3 -m unittest discover -s src/tests/protocol
```
