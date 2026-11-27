# Upstream port gaps

Upstream changes this pipeline deliberately did not port. One row per deferral,
appended by rust/tools/upstream-sync.sh. `divergence` means the omission is a
real behavioural difference (Rust still does the unhardened thing); `absent`
means the feature does not exist in Rust, so nothing diverged. Bulk review:
grep for `open` rows, work them in batches, flip the status here in the same PR.

| date | upstream | change | why not ported | kind | status |
|---|---|---|---|---|---|
| 2026-11-27 | `12ea5f4a` | Run register test after frequency setting | Ported: `serial.rs::set_frequency_verified` writes, reads back with retries, and on API 36+ sleeps 30ms and reads `TEST_RX_REGISTER`, exactly as RHInterface.set_frequency; a failing seat logs a warning and emits "Failed to set frequency on node N". Disabled seats power the receiver down with 1111 (5800 below API 24). Covered by unit tests and an eight-seat API 36 board on a pty (`tests/fixtures/test_c5mk_node.sh`). | divergence | fixed |
| 2026-11-26 | `078c177` | Default CORS Allowed Hosts to same-origin instead of any site | Ported: `src/cors.rs` reads the setting once at startup (empty = same-origin, `*` = any, otherwise an allow list) and the hardening middleware refuses a foreign Origin on /socket.io with 400 "Not an accepted origin.", as engineio does; other paths answer without the header. Covered by `tests/cors_policy.rs`. | divergence | fixed |
| 2026-11-26 | `0309dec` | Require admin auth for the hardware_log_init load-data request | The load-data handler is not ported, so there is no request to guard | absent | open |
| 2026-11-26 | `b1caaea` | Reject database-backup file names that are not plain file names | Rust has no download/restore/delete database handlers, so no filename reaches a path | absent | open |
| 2026-11-26 | `b73bf292` | Redact secrets from the download-logs zip file | No log-zip download endpoint in Rust | absent | open |
| 2026-11-26 | `a2bfb14` | Tie Flask debug-enabled to Server Debug Mode | Rust has no Flask/debug reloader; its DEBUG flag already governs dev features | absent | open |
| 2026-11-26 | `diff` | log.py: redact SECRET_KEY/PASSWORD values when adding config files to the log .zip | The Rust server has no log-zip export feature; nothing to redact. | absent | open |
| 2026-11-26 | `diff` | scanner.py: cors_allowed_origins None and debug=False | scanner.py is a standalone dev tool with no Rust counterpart. | absent | open |
| 2026-11-26 | `diff` | server.py: hardware_log_init load type now requires admin auth | The Rust load_data handler has no 'hardware_log_init' load type (no hardware-log streaming). | absent | open |
| 2026-11-26 | `diff` | server.py: get_db_bkp_file_path validates plain file names for download/restore/delete database handlers | Rust has only backup_database (timestamped name) and list_backups; it has no download_database / restore_database / delete_database_file handlers that accept user file names, so there is no path-traversal surface. | absent | open |
| 2026-11-26 | `diff` | server.py: SOCKET_IO.run debug flag driven by GENERAL.DEBUG | The Rust server uses axum, which has no debug/reloader run mode; the setting has no consumer. | absent | open |
| 2026-11-26 | `diff` | server.py/scanner.py: SocketIO cors_allowed_origins from CORS_ALLOWED_HOSTS (None = same-origin only) | The socketioxide layer in the Rust server is not configured with a CORS policy, so socket.io connections may still accept cross-origin clients while Python now restricts them by default. | divergence | open |
| 2026-11-26 | `diff` | advanced-settings.html: added description text for CORS and Debug settings | The template is shared with the Python tree and embedded by the Rust build (assets.rs embeds ../../src/server/templates), so the change is automatically reflected. | absent | open |
| 2026-11-26 | `diff` | server.py: RELEASE_VERSION bump to 4.6.0-dev.2 | The Rust server reports its own independent version string (state::RELEASE_VERSION). | absent | open |
