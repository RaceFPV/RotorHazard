//! IMDTabler integration — port of the server's IMDTabler page logic:
//! shells out to the external IMDTabler.jar (Java) to compute IMD
//! profile tables for the current frequency set.  Java remains an
//! external dependency, as in the Python server.

use serde_json::{json, Value};
use std::process::Command;

/// Path of the IMDTabler jar inside the program directory
/// (IMDTABLER_JAR_NAME in server.py).
pub fn jar_path(data_dir: &str) -> String {
    // the data dir sits next to the program dir in the standard layout;
    // also check the working directory for bundle-style installs
    for base in [
        std::path::Path::new(data_dir).parent().map(|p| p.to_path_buf()),
        Some(std::path::PathBuf::from(".")),
    ]
    .into_iter()
    .flatten()
    {
        let candidate = base.join("static").join("IMDTabler.jar");
        if candidate.exists() {
            return candidate.to_string_lossy().to_string();
        }
    }
    String::new()
}

/// True when the jar exists and java responds (Use_imdtabler_jar_flag).
#[allow(dead_code)] // wired to the imdtabler page render
pub fn available(data_dir: &str) -> bool {
    let jar = jar_path(data_dir);
    !jar.is_empty()
        && Command::new("java")
            .arg("-version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
}

fn version_string(jar: &str) -> String {
    Command::new("java")
        .args(["-jar", jar, "-v"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// Runs IMDTabler for the given frequency list and returns the page
/// payload (emit_imdtabler_data).
pub fn run(data_dir: &str, freq_list: &[u64]) -> Value {
    let jar = jar_path(data_dir);
    if jar.is_empty() {
        return json!({"imdtabler_data": {"error": "IMDTabler.jar not found"}});
    }
    let freq_strs: Vec<String> = freq_list.iter().map(|f| f.to_string()).collect();
    let output = Command::new("java")
        .args(["-jar", &jar])
        .args(&freq_strs)
        .output();
    match output {
        Ok(o) => {
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            );
            json!({"imdtabler_data": {"output": text}})
        }
        Err(e) => json!({"imdtabler_data": {"error": format!("IMDTabler failed: {}", e)}}),
    }
}

/// The page payload with the version string (emit_imdtabler_page).
#[allow(dead_code)] // wired to the imdtabler page render
pub fn page_data(data_dir: &str, profile_freqs: &[i64]) -> Value {
    if !available(data_dir) {
        return json!({"use_imdtabler_jar_flag": false});
    }
    let freq_strs: Vec<String> = profile_freqs.iter().map(|f| f.to_string()).collect();
    let output = Command::new("java")
        .args(["-jar", &jar_path(data_dir)])
        .args(&freq_strs)
        .output()
        .map(|o| {
            format!(
                "{}{}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            )
        })
        .unwrap_or_default();
    json!({
        "use_imdtabler_jar_flag": true,
        "imdtabler_ver": version_string(&jar_path(data_dir)),
        "imdtabler_data": {"output": output},
    })
}
