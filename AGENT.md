# AGENT.md — RotorHazard Rust Port

## What this is

A Rust rewrite of the RotorHazard drone racing server (originally Python).
The Rust server in `rust/server/` is wire-compatible with the Python
original: same database schema, same config.json, same Socket.IO events,
same serial-node protocol. It is deployed and running on a Raspberry Pi 5
with a NuclearHazard STM32 timer board at `racefpv@pi4.k3s.live`.

The Python code in `src/server/` and `src/interface/` is kept as the
**reference implementation** — it is the specification the Rust port
must match. It still runs and can be used as a test oracle via the
Python protocol suite in `src/tests/protocol/`.

## Repository layout

```
rust/server/src/       Rust server (17k lines, 28 modules)
rust/server/tests/     Integration tests (protocol_core, protocol_more)
rust/server/tests/common/mod.rs   Test harness (spawn server, socket.io client)
rust/tools/            Sync scripts (upstream sync, bracket extraction)
rust/plugin-bridge/    Python sidecar for third-party RHAPI plugins
rust/README.md         Install/deploy guide
.github/workflows/     CI (upstream sync)
src/server/            Python server (REFERENCE ONLY — superseded)
src/interface/         Python hardware interfaces (REFERENCE ONLY)
src/node/              Arduino C++ firmware (never was Python)
src/tests/protocol/    Python protocol test suite (68 tests, cross-validates)
```

## Build and test

```bash
# build
cargo build --release --manifest-path rust/server/Cargo.toml

# unit tests (62, <1s)
cargo test --release --manifest-path rust/server/Cargo.toml

# protocol integration tests (26, ~20s — spawns real server processes)
cargo test --release --test protocol_core --test protocol_more \
    --manifest-path rust/server/Cargo.toml
```

Do NOT use `cargo test` without `--release` for protocol tests — the
server startup is slower in debug mode and can cause timeouts.

## Architecture: which Rust module ports which Python module

| Rust module | Python source | What it does |
|---|---|---|
| `main.rs` | `server.py` (startup) | Routes, config load, state init, task spawning |
| `socket.rs` | `server.py` (socket handlers) | 68+ Socket.IO event handlers; ALL interactive UI actions live here |
| `api.rs` | `server.py` (json_endpoints) | HTTP JSON API (`/api/*`) |
| `race.rs` | `RHRace.py` | Race lifecycle: stage → start → laps → stop → save/discard |
| `results.rs` | `Results.py` | Leaderboard calculation (by_race_time, fastest_lap, consecutives) |
| `payloads.rs` | various emit fns | Socket.IO payload builders matching Python's emit shapes |
| `db.rs` | `RHData.py` / `Database.py` | SQLite access, profiles, options, freq arrays |
| `state.rs` | `RHRace` state | SharedState: race status, laps, node pilots, all managers |
| `calibration.rs` | `calibration.py` | EnterAt/ExitAt suggestion engine — **differentially tested** against Python (8 fixtures in tests/fixtures/) |
| `serial.rs` | `serial_node.py` + `RHInterface.py` | Serial node protocol: discovery, read/write blocks, checksums, multi-node |
| `hardware.rs` | `RHInterface.py` | Hardware update loop (50Hz lap-stats polling), crossing detection, hot-plug |
| `actions.rs` | `EventActions.py` + `rh_actions_builtin` | Event actions: speak/message/alert/schedule, %-token substitution |
| `class_rank.rs` | `rh_class_rank_*` + `rh_points_by_position` | Class ranking methods and race points assignment |
| `heatgen.rs` | `rh_heatgenerator_*` | Heat generators: FAI brackets (extracted JSON), ladder, balanced fill |
| `led.rs` | `led_event_manager.py` + `rh_led_handler_strip` | LED strip: effect engine, event wiring, ANSI/LedCtrlr/SPI backends |
| `cluster.rs` | `ClusterNodeSet.py` | Multi-timer clustering: primary/secondary sync, splits |
| `vrx.rs` | `VRxControl.py` | VRx framework (controllers come from plugins) |
| `pluginrpc.rs` | `RHAPI.py` | RPC endpoint for the Python plugin bridge |
| `sensor.rs` | `linux_sensor.py` | Core temp + battery telemetry |
| `mock.rs` | `MockSignal.py` | Mock signal providers for testing |
| `export_import.rs` | `rh_data_export/import_*` | JSON + CSV data export/import |
| `fwupdate.rs` | `stm32loader.py` | STM32 AN3155 bootloader firmware flashing |
| `heatgen.rs` | `rh_heatgenerator_*` | Heat generation (brackets, ladder, balanced) |
| `phase3.rs` | `Language.py` etc. | Language/callout cleanup |
| `pages.rs` | `PageCache.py` | HTML page rendering |
| `util.rs` | `RHUtils.py` | Shared utilities |
| `imdtabler.rs` | IMDTabler page | External Java tool integration |
| `cluster.rs` | `ClusterNodeSet.py` | Multi-timer sync |
| `sensor.rs` | `linux_sensor.py` | CPU temp + battery |
| `pluginrpc.rs` | `RHAPI.py` | Plugin bridge RPC endpoint |
| `fwupdate.rs` | `stm32loader.py` | Node firmware update (AN3155) |

## Critical protocol details (the contract)

These are wire-protocol behaviors that MUST be preserved. The protocol
test suite pins them; if you break one, tests fail:

1. **Socket payload keys differ from HTTP keys**: socket payloads use
   `pilot_id`, HTTP API uses `id`. This is deliberate.
2. **`fdata` entries** use keys `frequency`, `band`, `channel` (not `f`, `b`, `c`).
3. **Zero-lap rows** in the leaderboard have `fastest_lap_raw = 0` (not null)
   and `last_lap_raw = null`.
4. **Race status codes**: READY=0, RACING=1, DONE=2, STAGING=3.
5. **`race_status` broadcasts** fire on every status transition.
6. **`load_data`** responds to the *requesting socket only* (not broadcast)
   for most load types; some (race_status, current_laps) also broadcast.
7. **Calibration suggestion** must produce identical results to Python —
   verified by differential tests in `calibration.rs::tests`.
8. **Serial node protocol**: revision code must have 0x25 in the high byte
   (verification); API level is the low byte. Commands use XOR checksums.
9. **Min-lap behavior**: laps under `MinLapSec` are marked `deleted+noise`
   when `min_lap_behavior` is enabled.
10. **Race history** (`node.history_values/times`) is cleared at race start,
    populated by peak/nadir events during the race, pruned to 60s when READY,
    and persisted to `saved_pilot_race` at save.

## Data flow

```
Browser/UI ←→ Socket.IO ←→ socket.rs handlers ←→ db.rs (SQLite)
                              │
                              ▼
                    race.rs (lifecycle) → hardware.rs → serial.rs → timer nodes
                              │                                    │
                              ▼                                    ▼
                    actions.rs (callouts)                    lap stats (50Hz)
                              │
                              ▼
                    led.rs (LED effects)
```

## Lock ordering (deadlock prevention)

The shared state has several Mutexes. The lock ordering is:
1. `state.db` (SQLite connection) — acquire FIRST when doing DB work
2. `state.race` — acquire SECOND (or alone, briefly)
3. `state.mock` / `state.hardware` / `state.mock_provider` — acquire after DB is released

`register_lap_at` drops the race lock before DB work. `mock_update_tick`
takes DB first (snapshot), then provider+mock. If you add a new code path
that takes multiple locks, follow this order or you will deadlock.

## Adding a new feature (the pattern)

1. Identify the Python source module and its test coverage
2. Create or extend the corresponding Rust module
3. If it's a pure algorithm → write differential tests (Python generates
   fixtures, Rust must match — see `calibration.rs` tests)
4. If it touches the wire protocol → add tests to `tests/protocol_core.rs`
   or a new `tests/protocol_<topic>.rs`
5. Wire it into `socket.rs` handlers / `race.rs` lifecycle / `main.rs` startup
6. If it's an event hook → add `state.actions_engine.do_actions(...)`,
   `state.led_manager.trigger(...)`, `state.vrx_manager.do_*()`, and
   `crate::socket::emit_plugin_event(state, event, &json!({...}))` at the
   same lifecycle point
7. Build, run ALL tests, commit

## Upstream sync pipeline

`.github/workflows/upstream-sync.yml` runs daily:
1. Fetches `upstream` remote (RotorHazard/RotorHazard)
2. Diffs `src/server/` + `src/interface/` since last sync
3. If changes: merges, sanitizes the diff (strips comments/strings),
   sends to Claude API for porting, validates output, runs tests
4. Auto-pushes on success, rolls back + creates issue on failure

Security:
- Contributor allowlist (44 RotorHazard contributors) — unknown
  contributors get flagged instead of auto-ported
- Diff sanitization strips comments/strings before the LLM sees them
- Output validation rejects `unsafe`, network I/O, new deps, secrets
- Size limit: output >20× the Python diff is rejected

Run manually: `./rust/tools/upstream-sync.sh [--dry-run]`
Regenerate the script: `python3 rust/tools/generate_sync.py`

## Plugin bridge (third-party plugins)

`rust/plugin-bridge/rhapi_bridge.py` is an optional Python sidecar that
runs community plugins (unmodified RHAPI code) against the Rust server:

```
Rust server ←→ localhost socket.io ←→ rhapi_bridge.py ←→ user plugins
```

- Server emits `rhapi_event` {event, args} for internal events
- Bridge subscribes, dispatches to plugin hooks
- Plugin RHAPI calls go back as `rhapi_call` {object, method, args} with ack
- Server answers in `pluginrpc.rs` (db reads, race status, ui messaging)

Without the bridge: no plugins, no problem. The server is pure Rust.

## Things that are intentionally NOT ported

- **Climate sensor** (BME280) — dropped per user decision
- **LED matrix handlers** (bitmap/character/graph) — dropped per user decision
- **cv2_leds** (OpenCV panel emulation) — dev tool only
- **INA219 battery sensor** — stubbed in `sensor.rs` (dormant, needs hardware)
- **Python plugin host** — replaced by the out-of-process bridge design

## Known gaps / TODO

- [ ] Remaining protocol test files (lifecycle 15, phase2 5, phase3 11,
       auth 3, export 2, results 3, ui 2, new_features 5 = 46 tests) —
       the pattern is established in `protocol_core.rs`, each file is
       a mechanical conversion
- [ ] ws281x GPIO LED backend (classic Pi 4 PWM/DMA; SPI backend works
       on Pi 5 and Pi 4 with GPIO10 wiring)
- [ ] Node firmware update UI flow (`/updatenodes` page renders but
       upload/browse is stubbed; the flasher itself is complete)
- [ ] Cluster settings UI handlers (status/retry buttons)
- [ ] Mirror-mode results caching
- [ ] `emit_plugin_event` should be called at more lifecycle points
       (heat_set, frequency_set, option_set, pilot_done, etc.)

## Deployed environment (THIS deployment — not universal)

> These are the specifics of one production deployment. Other
> deployments will differ. Do not hardcode these paths in source code.

- **Pi 5** (hostname/address per your SSH config)
- NuclearHazard board, STM32 timer node on `/dev/ttyAMA0` at 921600 baud
- systemd service: `rh-server.service`
- Data dir: `<user-home>/rh-rust/data`
- Binary: `<user-home>/rh-rust/rust/target/release/rh-server`
- udev rule: `ttyAMA0` → group `dialout`, mode 0660
- Serial console REMOVED from cmdline.txt, getty disabled
- Deploy: `rsync` `rust/server/src/` to the Pi, rebuild natively, restart service

## For an LLM picking this up

- Read this file first
- Run `cargo test --release --manifest-path rust/server/Cargo.toml` to
  verify the baseline (62 unit + 26 protocol = 88 tests, all green)
- The Python code in `src/server/` is the SPECIFICATION. When porting
  an upstream change: read the Python diff, understand the behavioral
  change, find the corresponding Rust module, apply the change, test.
- The protocol test suite is the ground truth for wire compatibility.
  If a test fails, the Rust code is wrong (not the test).
- Use `--release` for all test runs. Debug builds are too slow for
  the protocol integration tests.
- When emitting new Socket.IO payloads, match the exact key names the
  Python server uses. The frontend depends on them.
- Lock ordering matters. See "Lock ordering" above.
- The `state.node_count()` method returns the current seat count (it
  can grow at runtime via hot-plug; `num_nodes` is an AtomicUsize).
- When adding a Socket.IO handler, check whether Python's version uses
  `@requires_socketio_auth` — if so, add the `socket_authed` check.
