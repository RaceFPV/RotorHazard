//! Plugin support: materialise the embedded Python bridge and keep it running.
//!
//! RotorHazard plugins are Python. The server is Rust and cannot import them, so
//! plugins run in a child process (`rhapi_bridge.py`) whose bytes are embedded in
//! this binary (see build.rs). On start-up we:
//!
//! 1. write the bridge into `<data_dir>/bridge/` (rewriting when the embedded
//!    copy differs from what is on disk, so upgrading is still "replace the
//!    binary"),
//! 2. find a Python that can `import socketio`,
//! 3. start the bridge against this server's port and restart it if it dies.
//!
//! Nothing here is required for the server to work: no plugins, no Python or
//! `RH_PLUGIN_BRIDGE=0` simply skips it.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

include!(concat!(env!("OUT_DIR"), "/bridge_assets.rs"));

/// Where the bridge lives inside the data directory.
///
/// Deliberately distinctive: the data directory belongs to the user and holds a
/// Python RotorHazard install's config.json, database.db and plugins/ after an
/// upgrade, so this must never collide with a name a user or an older server
/// might already use.
pub fn bridge_dir(data_dir: &str) -> PathBuf {
    Path::new(data_dir).join("plugin-bridge")
}

/// Write the embedded bridge to `<data_dir>/bridge/`, skipping unchanged files.
///
/// Returns the directory. A `.revision` marker means a binary upgrade rewrites
/// the tree exactly once, while an unchanged binary touches nothing (so a bridge
/// that is already running is never disturbed).
pub fn materialise(data_dir: &str) -> std::io::Result<PathBuf> {
    let dir = bridge_dir(data_dir);
    fs::create_dir_all(&dir)?;

    // Compare content, not a marker. The marker alone cannot tell us that a file
    // was edited, truncated by a full disk or replaced by an older binary's
    // leftover, and trusting it would leave a broken bridge in place. Ten small
    // files cost nothing to read.
    let mut updated = 0usize;
    for (rel, contents) in BRIDGE_FILES {
        let path = dir.join(rel);
        let same = fs::read(&path)
            .map(|existing| existing == contents.as_bytes())
            .unwrap_or(false);
        if same {
            continue;
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        // Write-then-rename so a half-written module can never be imported, and
        // so a crashed upgrade leaves either the old file or the new one.
        let tmp = path.with_extension("tmp");
        {
            let mut file = fs::File::create(&tmp)?;
            file.write_all(contents.as_bytes())?;
            file.sync_all()?;
        }
        // Windows refuses a rename over an existing file; a file the interpreter
        // still has mapped is left alone and the error propagates, which skips
        // autostart rather than running a mixed tree.
        let _ = fs::remove_file(&path);
        fs::rename(&tmp, &path)?;
        updated += 1;
    }

    // A module dropped from a newer bridge must not linger: Python would happily
    // import the stale copy and the two versions would disagree. Only ever
    // deletes .py/.pyc inside plugin-bridge/, never anything the user owns.
    let mut removed = 0usize;
    for rel in walk_files(&dir) {
        let shipped = BRIDGE_FILES.iter().any(|(known, _)| *known == rel);
        let stale = rel.ends_with(".py") || rel.ends_with(".pyc");
        if !shipped && stale {
            if fs::remove_file(dir.join(&rel)).is_ok() {
                removed += 1;
            }
        }
    }
    prune_empty_dirs(&dir);

    // Diagnostics only; the content comparison above is what decides.
    let marker = dir.join(".revision");
    if fs::read_to_string(&marker).unwrap_or_default().trim() != BRIDGE_REVISION {
        fs::write(&marker, BRIDGE_REVISION)?;
    }

    if updated > 0 || removed > 0 {
        tracing::info!(
            "plugin bridge: installed {} file(s), removed {} stale file(s) in {} (revision {})",
            updated,
            removed,
            dir.display(),
            BRIDGE_REVISION
        );
    }
    Ok(dir)
}

/// Relative paths of every file under `root`.
fn walk_files(root: &Path) -> Vec<String> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<String>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(base, &path, out);
            } else if let Ok(rel) = path.strip_prefix(base) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out
}

/// Remove directories that became empty after pruning, deepest first.
fn prune_empty_dirs(root: &Path) {
    fn walk(dir: &Path) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        let mut children = Vec::new();
        for entry in entries.flatten() {
            if entry.path().is_dir() {
                children.push(entry.path());
            }
        }
        for child in children {
            walk(&child);
            let _ = fs::remove_dir(&child); // fails unless empty, which is what we want
        }
    }
    walk(root);
}

/// A Python that can load the bridge's only third-party dependency.
fn find_python() -> Option<String> {
    let candidates: &[&str] = if cfg!(windows) {
        &["python", "python3", "py"]
    } else {
        &["python3", "python"]
    };
    for candidate in candidates {
        let ok = Command::new(candidate)
            .args(["-c", "import socketio"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            return Some((*candidate).to_string());
        }
    }
    None
}

/// Plugin directories that would actually load (a subdirectory with __init__.py).
fn plugin_dirs(plugins_dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    if let Ok(entries) = fs::read_dir(plugins_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && path.join("__init__.py").is_file() {
                found.push(path);
            }
        }
    }
    found
}

/// Start the bridge if there is anything for it to load.
pub fn autostart(data_dir: &str, http_port: u16) {
    if std::env::var("RH_PLUGIN_BRIDGE").map(|v| v == "0").unwrap_or(false) {
        tracing::info!("plugin bridge: disabled by RH_PLUGIN_BRIDGE=0");
        return;
    }

    let plugins_dir = Path::new(data_dir).join("plugins");
    let plugins = plugin_dirs(&plugins_dir);
    if plugins.is_empty() {
        tracing::info!(
            "plugin bridge: no plugins in {} -- not starting",
            plugins_dir.display()
        );
        return;
    }

    let dir = match materialise(data_dir) {
        Ok(dir) => dir,
        Err(err) => {
            tracing::error!("plugin bridge: cannot write {}: {}", bridge_dir(data_dir).display(), err);
            return;
        }
    };

    let Some(python) = find_python() else {
        tracing::warn!(
            "plugin bridge: {} plugin(s) present but no Python with the 'socketio' \
             module was found; install python3 and `pip install python-socketio` \
             to enable plugins",
            plugins.len()
        );
        return;
    };

    let script = dir.join("rhapi_bridge.py");
    let url = format!("http://127.0.0.1:{}", http_port);
    let log_path = Path::new(data_dir).join("plugin-bridge.log");

    tracing::info!(
        "plugin bridge: starting {} plugin(s) with {} ({})",
        plugins.len(),
        python,
        url
    );

    std::thread::spawn(move || {
        // Plugins crash. A crashed bridge must not take the server down, and a
        // silent bridge looks like "plugins stopped working", so restart a few
        // times and then give up loudly.
        for attempt in 1..=5u32 {
            let log = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path);
            let mut command = Command::new(&python);
            command
                .arg("-u")   // unbuffered: plugin output must reach plugin-bridge.log live
                // Never write .pyc files into the user's data directory: the
                // import of their plugin would otherwise litter plugins/ with
                // __pycache__ (and fail on a read-only mount).
                .env("PYTHONDONTWRITEBYTECODE", "1")
                .arg(&script)
                .arg("--server")
                .arg(&url)
                .arg("--plugins-dir")
                .arg(&plugins_dir);
            match &log {
                Ok(file) => {
                    command.stdout(Stdio::from(file.try_clone().unwrap()));
                    command.stderr(Stdio::from(file.try_clone().unwrap()));
                }
                Err(_) => {
                    command.stdout(Stdio::null());
                    command.stderr(Stdio::null());
                }
            }
            match command.spawn() {
                Ok(mut child) => match child.wait() {
                    Ok(status) => tracing::warn!(
                        "plugin bridge exited ({}); restart {}/5",
                        status,
                        attempt
                    ),
                    Err(err) => {
                        tracing::warn!("plugin bridge wait failed: {}; restart {}/5", err, attempt)
                    }
                },
                Err(err) => {
                    tracing::error!(
                        "plugin bridge could not start {}: {}; giving up",
                        script.display(),
                        err
                    );
                    return;
                }
            }
            std::thread::sleep(Duration::from_secs(2));
        }
        tracing::error!("plugin bridge failed 5 times; plugins are offline");
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "rh-bridge-{}-{}-{}",
            tag,
            std::process::id(),
            BRIDGE_REVISION
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn embedded_bridge_has_the_entry_point() {
        assert!(BRIDGE_FILES.iter().any(|(rel, _)| *rel == "rhapi_bridge.py"));
        assert!(BRIDGE_FILES
            .iter()
            .any(|(rel, _)| rel.starts_with("rh_compat/")));
    }

    #[test]
    fn materialise_writes_every_file_once() {
        let dir = temp_dir("write");
        let out = materialise(dir.to_str().unwrap()).unwrap();
        for (rel, contents) in BRIDGE_FILES {
            let on_disk = fs::read_to_string(out.join(rel)).unwrap();
            assert_eq!(&on_disk, contents, "{} differs", rel);
        }
        assert_eq!(
            fs::read_to_string(out.join(".revision")).unwrap().trim(),
            BRIDGE_REVISION
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stale_revision_is_repaired() {
        let dir = temp_dir("repair");
        let out = materialise(dir.to_str().unwrap()).unwrap();
        fs::write(out.join(".revision"), "stale").unwrap();
        fs::write(out.join("rhapi_bridge.py"), "# tampered").unwrap();
        materialise(dir.to_str().unwrap()).unwrap();
        assert_eq!(
            fs::read_to_string(out.join("rhapi_bridge.py")).unwrap(),
            BRIDGE_FILES
                .iter()
                .find(|(rel, _)| *rel == "rhapi_bridge.py")
                .map(|(_, c)| *c)
                .unwrap()
        );
        let _ = fs::remove_dir_all(&dir);
    }



    #[test]
    fn upgrading_replaces_changed_files_and_removes_dropped_ones() {
        let dir = temp_dir("upgrade");
        let out = materialise(dir.to_str().unwrap()).unwrap();

        // What an older binary left behind: an outdated entry point and a module
        // that no longer ships.
        fs::write(out.join("rhapi_bridge.py"), "# version 1\n").unwrap();
        fs::create_dir_all(out.join("rh_compat")).unwrap();
        fs::write(out.join("rh_compat/dropped_module.py"), "# gone in v2\n").unwrap();
        fs::create_dir_all(out.join("rh_compat/__pycache__")).unwrap();
        fs::write(out.join("rh_compat/__pycache__/dropped.cpython-312.pyc"), "x").unwrap();
        // A file we do not ship must survive: it is the user's, not ours.
        fs::write(out.join("notes.txt"), "mine").unwrap();

        materialise(dir.to_str().unwrap()).unwrap();

        assert_eq!(
            fs::read_to_string(out.join("rhapi_bridge.py")).unwrap(),
            BRIDGE_FILES
                .iter()
                .find(|(rel, _)| *rel == "rhapi_bridge.py")
                .map(|(_, c)| *c)
                .unwrap(),
            "the outdated entry point was not replaced"
        );
        assert!(
            !out.join("rh_compat/dropped_module.py").exists(),
            "a module the new bridge no longer ships was left behind"
        );
        assert!(
            !out.join("rh_compat/__pycache__").exists(),
            "an empty __pycache__ directory was left behind"
        );
        assert_eq!(fs::read_to_string(out.join("notes.txt")).unwrap(), "mine");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_corrupted_file_is_repaired_without_an_upgrade() {
        let dir = temp_dir("repair-content");
        let out = materialise(dir.to_str().unwrap()).unwrap();
        // Same revision marker, damaged payload: a marker-only check would skip it.
        fs::write(out.join("rhapi_bridge.py"), "def oops(\n").unwrap();
        materialise(dir.to_str().unwrap()).unwrap();
        let expected = BRIDGE_FILES
            .iter()
            .find(|(rel, _)| *rel == "rhapi_bridge.py")
            .map(|(_, c)| *c)
            .unwrap();
        assert_eq!(
            fs::read_to_string(out.join("rhapi_bridge.py")).unwrap(),
            expected
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unchanged_tree_is_not_rewritten() {
        let dir = temp_dir("idempotent");
        let out = materialise(dir.to_str().unwrap()).unwrap();
        let before = fs::metadata(out.join("rhapi_bridge.py")).unwrap().modified().unwrap();
        std::thread::sleep(Duration::from_millis(1100));
        materialise(dir.to_str().unwrap()).unwrap();
        let after = fs::metadata(out.join("rhapi_bridge.py")).unwrap().modified().unwrap();
        assert_eq!(before, after, "an unchanged bridge was rewritten");
        let _ = fs::remove_dir_all(&dir);
    }

    /// The data directory may already hold a Python RotorHazard install. Nothing
    /// outside `plugin-bridge/` may be created, changed or removed - not on first
    /// run, not on upgrade, not when the revision marker is stale.
    #[test]
    fn the_rest_of_the_data_dir_is_never_touched() {
        let dir = temp_dir("non-destructive");
        let root = dir.to_str().unwrap().to_string();

        // A realistic pre-upgrade data directory, as the Python server leaves it.
        let sentinels: &[(&str, &str)] = &[
            ("config.json", "{\"GENERAL\":{\"HTTP_PORT\":5000}}"),
            ("database.db", "SQLite format 3\0"),
            ("plugins/rh_connector_trackside/__init__.py", "# user's plugin\n"),
            ("plugins/rh_connector_trackside/manifest.json", "{\"domain\":\"x\"}"),
            ("custom_plugins/legacy.py", "# an older layout\n"),
            ("static/custom.css", "/* user styling */"),
            ("database.db-wal", "wal"),
        ];
        for (rel, contents) in sentinels {
            let path = Path::new(&root).join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, contents).unwrap();
        }
        let before = snapshot(&root);

        // First run, an unchanged re-run, and an upgrade (stale marker).
        materialise(&root).unwrap();
        materialise(&root).unwrap();
        fs::write(bridge_dir(&root).join(".revision"), "stale").unwrap();
        materialise(&root).unwrap();

        for (rel, contents) in sentinels {
            let on_disk = fs::read_to_string(Path::new(&root).join(rel)).unwrap();
            assert_eq!(&on_disk, contents, "{} was modified", rel);
        }

        let after = snapshot(&root);
        for (path, mtime) in &before {
            assert_eq!(
                after.get(path),
                Some(mtime),
                "{} changed or disappeared",
                path
            );
        }
        for path in after.keys() {
            assert!(
                before.contains_key(path) || path.starts_with("plugin-bridge/"),
                "{} was created outside plugin-bridge/",
                path
            );
        }

        let _ = fs::remove_dir_all(&dir);
    }

    /// Every file under `root`, with the bytes it holds (path -> length + mtime).
    fn snapshot(root: &str) -> std::collections::BTreeMap<String, (u64, std::time::SystemTime)> {
        let mut out = std::collections::BTreeMap::new();
        fn walk(base: &Path, dir: &Path, out: &mut std::collections::BTreeMap<String, (u64, std::time::SystemTime)>) {
            let Ok(entries) = fs::read_dir(dir) else { return };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(base, &path, out);
                    continue;
                }
                let rel = path
                    .strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                let meta = fs::metadata(&path).unwrap();
                out.insert(rel, (meta.len(), meta.modified().unwrap()));
            }
        }
        walk(Path::new(root), Path::new(root), &mut out);
        out
    }

    #[test]
    fn plugin_dirs_need_an_entry_point() {
        let dir = temp_dir("plugins");
        fs::create_dir_all(dir.join("plugins/real")).unwrap();
        fs::write(dir.join("plugins/real/__init__.py"), "").unwrap();
        fs::create_dir_all(dir.join("plugins/empty")).unwrap();
        let found = plugin_dirs(&dir.join("plugins"));
        assert_eq!(found.len(), 1);
        assert!(found[0].ends_with("real"));
        let _ = fs::remove_dir_all(&dir);
    }
}
