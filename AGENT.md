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

# protocol integration tests (62: core 42 + more 7 + autoadjust 7
# + heartbeat 6, ~50s)
# each test spawns a real server process
cargo test --release \
    --test protocol_core --test protocol_more --test protocol_autoadjust \
    --test protocol_heartbeat \
    --manifest-path rust/server/Cargo.toml
```

Do NOT use `cargo test` without `--release` for protocol tests — the
server startup is slower in debug mode and can cause timeouts.

## CI cost policy

Two workflows, deliberately separate so a test run never looks like a release:

- `rust-ci.yml` — build + unit + protocol tests only, `contents: read`. Runs for
  commits touching `rust/**` or the workflow file, on PRs, and when called by
  the release workflow as its gate. It never packages, uploads or publishes.
- `rust-release.yml` — the binaries. A tag push (`rh-server-rust-v*`) runs the
  test gate via `workflow_call`, builds x86_64 / aarch64 / Windows / macOS, and
  publishes a GitHub release. Manual dispatch builds the same four artifacts
  and stops there (no publish), for exercising packaging without a tag.

The macOS runner bills at 10x minutes, which is why ordinary branch commits
never package. Path filters are not evaluated for tag pushes, so tags are
handled entirely by the release workflow.

A merge that only brings in upstream Python changes therefore runs no Rust CI —
the upstream-sync job has already run `cargo test` and the protocol suite as
its own gate before pushing.

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
| `phase3.rs` | `Language.py` etc. | Language/callout cleanup |
| `pages.rs` | `PageCache.py` | HTML page rendering |
| `util.rs` | `RHUtils.py` | Shared utilities |
| `imdtabler.rs` | IMDTabler page | External Java tool integration |

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

`.github/workflows/upstream-sync.yml` runs daily at 06:00 UTC (and on manual
dispatch):

1. Fetches `upstream` (RotorHazard/RotorHazard)
2. **Sync cursor is ancestry, not a marker file.** If `upstream/main` is
   already an ancestor of `HEAD`, the branch has absorbed that upstream state
   and the run exits. There is no `.upstream-sync-sha` any more: a marker
   could be lost (cache eviction, fresh checkout) and a lost marker silently
   widened the port window to merge-base..HEAD.
3. Diffs `src/server/` + `src/interface/` across the unported range; if no
   ported file changed, merges upstream and pushes without a port
4. Checks the contributor allowlist (line-wise; display names and logins)
5. Sanitizes the diff (strips *comment lines only*; string literals are kept
   deliberately, because event names and message texts live in them) and
   appends the **entire current Rust tree** (`rust/server/src` +
   `rust/server/tests`, ~219k tokens, about three cents) to the prompt. The
   model can only quote anchors from text it has been shown; without this
   every `old` block was a guess.
6. Sends it to the configured LLM provider and validates the reply. The call
   lives in `rust/tools/llm_call.py`; OpenRouter and Anthropic share one body
   shape and differ only in URL and auth headers. Config: `LLM_API_KEY`
   (secret, required for a port), `LLM_BASE_URL` (default OpenRouter chat
   completions), `LLM_MODEL` (default `deepseek/deepseek-v4.1-flash-1017`),
   `LLM_MAX_TOKENS` (default 64000), `LLM_STYLE` (openai|anthropic, inferred
   from the URL). With no `LLM_API_KEY` the run merges and pushes unported.
7. Gates: `cargo test --release` by **exit status**, then the Python protocol
   suite against the built release binary
8. Stages everything (`git add -A`) and, by default, commits to
   `upstream-sync/<sha>` and opens a **pull request** against `rust` — nothing
   lands unreviewed. `SYNC_MODE=push` reverts to a direct push. Any gate
   failure resets the branch to the pre-merge SHA.

A port that produces no usable edits (`no_edits`, `parse_error`) FAILS the
run rather than committing a green merge-only commit: silence is
indistinguishable from a real sync in the history. `ACCEPT_NO_PORT=1`
accepts one deliberately. An LLM API error also fails and reverts.

PRs opened with `GITHUB_TOKEN` do not trigger `rust-ci.yml` (a GitHub
limitation, not a config error). The script's own gates — `cargo test` and
the protocol suite — are what vouch for the port. CI checks on the PR would
need a separate fine-grained PAT; nothing is wired for that yet.

Merge-only outcomes (no ported files changed, untrusted contributor, no key)
carry no ported Rust and push straight to `rust` — they are not review
tasks. Only an actual port opens a PR.

Placement rule: GitHub only loads `schedule` and `workflow_dispatch` from the
**default branch**, which here is `main` (a 1:1 upstream mirror) while all work
happens on `rust`. The workflow file therefore exists on both branches and the
job checks out `ref: rust`. Deleting the copy on `main` silently disables the
schedule.

Security (`rust/tools/validate_port.py`, called by the script):
- Rules run on **added lines only**. Whole-file scanning rejected real code:
  auth modules say `PASSWORD`, `db.rs`/`heatgen.rs` say `include_str!`,
  `imdtabler.rs` says `std::process::Command`.
- Output shapes: `{"path", "new_content"}` for small files, and
  `{"path", "edits": [{"old", "new"}]}` for everything else. Anchors must
  match exactly once, are applied in order, and are the reason a 3,000-line
  `socket.rs` change is a few hundred output tokens instead of a full
  regeneration. Rejections are named: `anchor_missing`, `anchor_ambiguous`,
  `anchor_too_short`, `anchor_malformed`.
- Structural bans: `unsafe {`, process spawn, raw TCP/UDP, `include_*!`,
  `extern crate`, shell paths, new URLs
- Secret bans: key prefixes (`sk-`, `ghp_`, `AKIA`, `xox*`) and
  `name = "<12+ char literal>"` assignments; a bare `password` identifier is
  fine
- Injection bans on instruction-shaped added text
- Truncation guard: a re-emit shorter than 50% of an existing file is refused
  before it lands (full-file responses are capped by max_tokens)
- Size limit: output over 20x the Python diff is rejected

Tool self-tests (no network, no key): `rust/tools/test_llm_call.py`,
`rust/tools/test_validate_port.py`, `rust/tools/test_pipeline_stub.py` (stub
provider, proves transport + parse + identical round trip).

Run manually: `./rust/tools/upstream-sync.sh [--dry-run]`
Validator tests: `python3 rust/tools/test_validate_port.py` (copies real
modules into a temp tree; covers unchanged re-emits of files containing banned
words, added-line rejections, truncation, path escape, size limits).
Escape hatches: `SKIP_LLM_PORT=1`, `SKIP_PY_SUITE=1`, `TRUSTED_CONTRIBUTORS`.
The script is the source of truth: `generate_sync.py` (which regenerated it
from an embedded template) has been deleted.

`github.com/RaceFPV/RustHazard` is a point-in-time snapshot of the port, not an
active remote — do not push there.

## Plugin bridge (third-party plugins)

`rust/plugin-bridge/rhapi_bridge.py` is an optional Python sidecar that
runs community plugins (unmodified RHAPI code) against the Rust server:

```
Rust server ←→ localhost socket.io ←→ rhapi_bridge.py ←→ user plugins
```

- Plugin RHAPI calls go out as `rhapi_call` {object, method, args} with an ack;
  `pluginrpc.rs` answers (db, race control, ui, config, marshal path)
- Server emits `rhapi_event` {event, args} for internal events; the bridge
  dispatches them to plugin hooks
- Inbound: the bridge declares its listener names via `plugin.listen`; the
  socket connect handler registers a forwarder per name, so a client emit of
  `ts_race_stage` reaches the plugin and its return value becomes the socket ack
  (`plugin_invoke` → `plugin_result`, 10s timeout, null if no bridge)

**Coverage**: `rust/plugin-bridge/RHAPI-COVERAGE.md` (generated by
`rust/tools/gen_rhapi_coverage.py`) inventories every public member of the real
RHAPI — 262 across 19 namespaces — against the bridge and `pluginrpc.rs`.
All 262 are callable; the file lists the ones that are approximations rather
than implementations (`db.open_readonly` cannot hand over a live DB handle,
export/import and heat generation run server-side, event results are not cached,
`interface.*` registrations are accepted no-ops). `COMPATIBILITY.md` answers the
narrower question — what one specific plugin needs.

**The bridge ships inside the binary.** `rust/server/build.rs` embeds
`rhapi_bridge.py` and `rh_compat/**` and derives a revision hash from their
contents; `plugin_support::materialise()` writes them to `<data_dir>/plugin-bridge/` and
rewrites only when the binary's copy differs. On start-up (after the port is
bound) `plugin_support::autostart()` spawns `python -u <bridge> --server
http://127.0.0.1:<port> --plugins-dir <data_dir>/plugins`, logs to
`<data_dir>/plugin-bridge.log`, and restarts a crashed bridge up to five times. So
upgrading is still "replace one binary": no download, no bridge version to
manage, no drift between server and bridge. `RH_PLUGIN_BRIDGE=0` disables it.

The data directory belongs to the user — after an upgrade from the Python server
it already holds their `config.json`, `database.db` and `plugins/`. The bridge
therefore lives in its own `plugin-bridge/` subdirectory and nothing else in the
data dir is created, modified or removed (there is a unit test that fails if a
single file outside it changes, and the child runs with
`PYTHONDONTWRITEBYTECODE=1` so importing a plugin does not litter `plugins/`
with `__pycache__`).
The one host requirement is Python 3 with `python-socketio`; without it the
server logs a clear warning and runs normally without plugins. That state is also
surfaced where users look: the plugins page shows a banner naming the problem and
the install command, and `plugin_list` (which used to emit an empty array) now
lists installed plugins from their manifests plus a `bridge` object with the
detected interpreter, its version and whether the bridge is running.

**Fresh installs ship no plugins.** The binary only ever writes `plugin-bridge/`
(there is a test asserting that a fresh data dir contains exactly that); plugins
appear only when the user's own `plugins/` folder has them. If a plugin looks
"already installed", check the data dir named on the first log line — an earlier
RotorHazard install is being reused.

**The settings page plugin manager** (`settings.html`, section "Plugins") is fed
by the same `plugin_list` event as `/plugins`, and it expects Python's shape:
`id` (the directory name), plus `loaded` and `load_issue`. The bridge reports the
real per-plugin result through `plugin.report_status` after loading, so a plugin
that raises shows "Not loaded: RuntimeError: boom" rather than "Not loaded:
undefined". `plugin_delete` removes `<data>/plugins/<domain>` (single path
component only, and only a directory with `__init__.py`); `plugin_install`
answers with a message that file installs are not supported yet.

One trap worth remembering: templates render missing `getConfig` keys as the
literal string `None`. `settings.html` calls `brightness_logsl.position({{ ... }})`,
so a missing `LED.ledBrightness` default threw a ReferenceError that killed the
whole `$(document).ready` block - every handler on the page, including the plugin
list, silently never registered. `read_config_item` now carries Config.py's
UI/LED/PLUGINS defaults, and that one call renders through `|tojson`.

**CORS is enforced, not advertised.** `GENERAL.CORS_ALLOWED_HOSTS` is read
once at startup (`src/cors.rs`, mirroring server.py's SocketIO construction):
empty means same-origin only, `*` means any, anything else is an allow list.
The hardening middleware refuses a foreign `Origin` on `/socket.io` with
`400 Not an accepted origin.` - engineio's behaviour, and the only thing that
guards a WebSocket upgrade, since browsers never apply CORS to one. Plain HTTP
routes simply omit the header instead, exactly as the reference server does.
Because the browser's own page is same-origin, this never affects normal use;
it stops a foreign page from opening a socket and driving a race. Tests:
`tests/cors_policy.rs`.

**Tests that protect this**: `rust/plugin-bridge/test_plugin_fixture.sh` runs an
in-repo fixture plugin (`tests/fixture_plugin`) that calls 42 representative
RHAPI members, registers a panel/option/attributes, stages a race and listens for
laps (`tests/fixture_client.py` asserts the panel, the call results, the '1'/'0'
option round-trip and the lap events). It needs no network and takes ~25s, so it
is a hard gate in `rust-ci.yml` AND in the upstream-sync `run_gates()`; the
sync gate also runs the real Connector-FPVTrackSide e2e whenever its clone
succeeds. Without this, a port could compile, pass the protocol suite and still
break every plugin.

**Panels**: `ui.register_panel` + `fields.register_option` are stored in
`SharedState` and served on `load_data [{"type":"ui","value":"settings"}]` in the
`RHUI.emit_ui` shape; `rh-ui.js` builds them into `#custom-ui`. Note the frontend
sends object load_types, so `handle_load_data` normalises both forms.

Verified against Connector-FPVTrackSide (the most-used community plugin):
stage, lap telemetry, stop/save, lean mode, event naming, frequency and colour
setup, abort, and the marshal round trip all work; `COMPATIBILITY.md` (generated
by `rust/tools/gen_compat_matrix.py`) reports 0 of 50 RHAPI calls missing.

**Running the end-to-end test**

```bash
git clone https://github.com/RotorHazard/Connector-FPVTrackSide /tmp/plugin-fpv
bash rust/plugin-bridge/e2e_plugin.sh      # server + bridge + a client playing FPVTrackSide
```

`rust/plugin-bridge/probe_plugin.sh` is the lighter "does it load" check.

**Server-internal imports**: plugins import `RHRace`, `RHUI`, `eventmanager`,
`RHUtils`, `Database` — server modules a Rust deployment does not have.
`rh_compat/` ships AST-sliced copies of exactly those symbols, regenerated from
`src/server` by `rust/tools/gen_rh_compat.py` (it keeps `@dataclass` decorators
and typing imports; dropping either breaks `UIField()`).

**Gotchas that cost real time**

- Never dispatch plugin work on the socket.io reader thread: handlers make
  blocking RPC calls and the reader is what delivers their acks. Both
  `plugin_invoke` and `rhapi_event` run on worker threads.
- Forwarders must use `TryData<Value>`; a mandatory `Data` extractor silently
  skips the handler when a client emits no payload.
- Event args are dict-like with record values wrapped: `args['node_index']` and
  `args['lap'].lap_time_stamp` both work, as in Python.
- Attribute tables are `(id, name, value)` where `id` is the owning row — not
  `(pilot_id, attribute, value)`. Values are text, but plugins pass integers;
  only a real null clears one.
- `pilot_add` must set `active` (NOT NULL).
- The server may report failure inside the result envelope; the bridge raises on
  that, otherwise plugins receive an error object where a list is expected.

Without the bridge: no plugins, no problem. The server is pure Rust.

## Things that are intentionally NOT ported

- **Climate sensor** (BME280) — dropped per user decision
- **LED matrix handlers** (bitmap/character/graph) — dropped per user decision
- **cv2_leds** (OpenCV panel emulation) — dev tool only
- **INA219 battery sensor** — stubbed in `sensor.rs` (dormant, needs hardware)
- **Python plugin host** — replaced by the out-of-process bridge design

## Known gaps / TODO

- [ ] Plugin settings panels: `fields.register_*` and `ui.register_panel` are
      accepted as no-ops, so a plugin's configuration panel does not render in
      the Rust UI — its behaviour (race control, lean mode, marshal) is fully
      functional without it
- [ ] Plugin host scaling: `Bridge.call` is synchronous with a 10s timeout, so a
      slow plugin handler delays other plugin events; fine for one plugin,
      revisit before hosting several

- [ ] Remaining protocol test files (lifecycle 15, phase2 5, phase3 11,
      auth 3, export 2, results 3, ui 2, new_features 5 = 46 tests) —
      optional while the Python suite gates CI; each is a mechanical
      conversion following `protocol_core.rs`
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
