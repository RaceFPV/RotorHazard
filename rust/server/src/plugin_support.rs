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
pub fn bridge_dir(data_dir: &str) -> PathBuf {
    Path::new(data_dir).join("bridge")
}

/// Write the embedded bridge to `<data_dir>/bridge/`, skipping unchanged files.
///
/// Returns the directory. A `.revision` marker means a binary upgrade rewrites
/// the tree exactly once, while an unchanged binary touches nothing (so a bridge
/// that is already running is never disturbed).
pub fn materialise(data_dir: &str) -> std::io::Result<PathBuf> {
    let dir = bridge_dir(data_dir);
    fs::create_dir_all(&dir)?;
    let marker = dir.join(".revision");
    let current = fs::read_to_string(&marker).unwrap_or_default();
    let mut wrote = 0usize;

    if current.trim() != BRIDGE_REVISION {
        for (rel, contents) in BRIDGE_FILES {
            let path = dir.join(rel);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            let same = fs::read_to_string(&path)
                .map(|existing| existing == *contents)
                .unwrap_or(false);
            if !same {
                // Write-then-rename so a half-written module can never be imported.
                let tmp = path.with_extension("tmp");
                {
                    let mut file = fs::File::create(&tmp)?;
                    file.write_all(contents.as_bytes())?;
                    file.sync_all()?;
                }
                let _ = fs::remove_file(&path);
                fs::rename(&tmp, &path)?;
                wrote += 1;
            }
        }
        fs::write(&marker, BRIDGE_REVISION)?;
        tracing::info!(
            "plugin bridge: wrote {} file(s) to {} (revision {})",
            wrote,
            dir.display(),
            BRIDGE_REVISION
        );
    }
    Ok(dir)
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
    let log_path = Path::new(data_dir).join("bridge.log");

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
                .arg("-u")   // unbuffered: plugin output must reach bridge.log live
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
