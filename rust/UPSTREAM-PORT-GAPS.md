# Upstream port gaps

Upstream changes this pipeline deliberately did not port. One row per deferral,
appended by rust/tools/upstream-sync.sh. `divergence` means the omission is a
real behavioural difference (Rust still does the unhardened thing); `absent`
means the feature does not exist in Rust, so nothing diverged. Bulk review:
grep for `open` rows, work them in batches, flip the status here in the same PR.

| date | upstream | change | why not ported | kind | status |
|---|---|---|---|---|---|
| 2026-11-26 | `078c177` | Default CORS Allowed Hosts to same-origin instead of any site | Rust enforces no CORS at all: no layer, no origin check, so the server still accepts any origin | divergence | open |
| 2026-11-26 | `0309dec` | Require admin auth for the hardware_log_init load-data request | The load-data handler is not ported, so there is no request to guard | absent | open |
| 2026-11-26 | `b1caaea` | Reject database-backup file names that are not plain file names | Rust has no download/restore/delete database handlers, so no filename reaches a path | absent | open |
| 2026-11-26 | `b73bf292` | Redact secrets from the download-logs zip file | No log-zip download endpoint in Rust | absent | open |
| 2026-11-26 | `a2bfb14` | Tie Flask debug-enabled to Server Debug Mode | Rust has no Flask/debug reloader; its DEBUG flag already governs dev features | absent | open |
