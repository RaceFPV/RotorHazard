//! Install a community plugin from GitHub, mirroring the Python installer.
//!
//! The Python server (`util/plugin_installation.py`) downloads a release, checks
//! it, and writes only the files under `<domain>/` into the plugin directory,
//! rolling back on any failure. This does the same, with the same download
//! preference order: a `<domain>.zip` asset, else the manifest's `zip_filename`
//! asset, else the repository archive at the tag.

use std::fs;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::pluginrepo;

/// Where a plugin's bits come from and what they must hash to.
#[derive(Debug, PartialEq)]
pub struct Plan {
    pub domain: String,
    pub url: String,
    pub sha256: Option<String>,
}

/// The release to install: what was asked for, else the newest stable, else the
/// newest pre-release - the same fallbacks as `download_plugin`.
pub fn resolve_tag(entry: &Value, requested: Option<&str>) -> Result<String, String> {
    if let Some(tag) = requested.filter(|tag| !tag.is_empty()) {
        return Ok(tag.to_string());
    }
    for key in ["last_version", "last_prerelease"] {
        if let Some(tag) = entry.get(key).and_then(|value| value.as_str()) {
            if !tag.is_empty() {
                return Ok(tag.to_string());
            }
        }
    }
    Err("Unable to determine the plugin version to download".to_string())
}

pub fn choose_download(entry: &Value, tag: &str) -> Result<Plan, String> {
    let domain = entry
        .get("manifest")
        .and_then(|manifest| manifest.get("domain"))
        .and_then(|value| value.as_str())
        .ok_or("Plugin data was not found")?
        .to_string();
    let repository = entry
        .get("repository")
        .and_then(|value| value.as_str())
        .ok_or("repository missing from registry entry")?;

    let releases = entry
        .get("releases")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();
    let release = releases
        .iter()
        .find(|release| release.get("tag_name").and_then(|v| v.as_str()) == Some(tag))
        .ok_or_else(|| format!("Release {} was not found for {}", tag, domain))?;
    let assets = release
        .get("assets")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();

    let asset_url = |name: &str| {
        format!(
            "https://github.com/{}/releases/download/{}/{}",
            repository, tag, name
        )
    };
    let hash_of = |name: &str| -> Option<String> {
        assets
            .iter()
            .find(|asset| asset.get("name").and_then(|v| v.as_str()) == Some(name))
            .and_then(|asset| asset.get("sha256"))
            .and_then(|value| value.as_str())
            .map(str::to_string)
    };

    let preferred = format!("{}.zip", domain);
    if hash_of(&preferred).is_some()
        || assets
            .iter()
            .any(|asset| asset.get("name").and_then(|v| v.as_str()) == Some(preferred.as_str()))
    {
        return Ok(Plan {
            domain,
            url: asset_url(&preferred),
            sha256: hash_of(&preferred),
        });
    }

    if let Some(named) = entry
        .get("manifest")
        .and_then(|manifest| manifest.get("zip_filename"))
        .and_then(|value| value.as_str())
    {
        if assets
            .iter()
            .any(|asset| asset.get("name").and_then(|v| v.as_str()) == Some(named))
        {
            return Ok(Plan {
                domain,
                url: asset_url(named),
                sha256: hash_of(named),
            });
        }
    }

    Ok(Plan {
        domain,
        url: format!(
            "https://github.com/{}/archive/refs/tags/{}.zip",
            repository, tag
        ),
        sha256: None,
    })
}

fn safe_relative(name: &str, domain: &str) -> Option<String> {
    let identifier = format!("{}/", domain);
    let position = name.find(&identifier)?;
    let relative = name[position + identifier.len()..].trim_start_matches('/');
    if relative.is_empty() {
        return None;
    }
    if relative.split('/').any(|part| part == ".." || part == ".") {
        return None;
    }
    Some(relative.to_string())
}

/// Writes every file under `<domain>/` in the archive to `target`.
pub fn extract_zip(bytes: &[u8], domain: &str, target: &Path) -> Result<usize, String> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|err| format!("not a zip archive: {}", err))?;
    let mut written = 0;
    for index in 0..archive.len() {
        let mut file = archive
            .by_index(index)
            .map_err(|err| format!("bad zip entry: {}", err))?;
        if file.is_dir() {
            continue;
        }
        let name = file.name().to_string();
        let Some(relative) = safe_relative(&name, domain) else {
            continue;
        };
        let out_path: PathBuf = target.join(&relative);
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|err| format!("could not create {}: {}", parent.display(), err))?;
        }
        let mut out = fs::File::create(&out_path)
            .map_err(|err| format!("could not write {}: {}", out_path.display(), err))?;
        std::io::copy(&mut file, &mut out)
            .map_err(|err| format!("could not write {}: {}", out_path.display(), err))?;
        written += 1;
    }
    if written == 0 {
        return Err(format!("the archive contained no {}/ directory", domain));
    }
    Ok(written)
}

fn manifest_requires(domain: &str, target: &Path, rhapi: (u64, u64)) -> Result<(), String> {
    let path = target.join("manifest.json");
    let text = fs::read_to_string(&path)
        .map_err(|_| format!("{} has no manifest.json", domain))?;
    let manifest: Value =
        serde_json::from_str(&text).map_err(|err| format!("{} manifest is invalid: {}", domain, err))?;
    if manifest
        .get("domain")
        .and_then(|value| value.as_str())
        .map(|found| found != domain)
        .unwrap_or(true)
    {
        return Err(format!(
            "{} manifest does not declare domain {}",
            domain, domain
        ));
    }
    let required = manifest
        .get("required_rhapi_version")
        .and_then(|value| value.as_str())
        .unwrap_or("0.0");
    if !pluginrepo::version_at_least(required, rhapi) {
        return Err(format!(
            "{} requires RHAPI {}, this server is {}.{}",
            domain, required, rhapi.0, rhapi.1
        ));
    }
    Ok(())
}

/// Downloads, verifies and installs one plugin. Returns a short status line;
/// on any error the plugin directory is left as it was found.
pub fn install(
    entry: &Value,
    requested_tag: Option<&str>,
    plugins_dir: &Path,
    rhapi: (u64, u64),
    fetch: &dyn Fn(&str) -> Result<Vec<u8>, String>,
) -> Result<String, String> {
    let tag = resolve_tag(entry, requested_tag)?;
    let plan = choose_download(entry, &tag)?;
    let bytes = fetch(&plan.url)?;

    if let Some(expected) = &plan.sha256 {
        let digest = Sha256::digest(&bytes);
        let actual = digest
            .iter()
            .map(|byte| format!("{:02x}", byte))
            .collect::<String>();
        if !actual.eq_ignore_ascii_case(expected) {
            return Err(format!(
                "Downloaded asset for {} does not match the expected hash",
                plan.domain
            ));
        }
    }

    let target = plugins_dir.join(&plan.domain);
    if target.exists() {
        fs::remove_dir_all(&target)
            .map_err(|err| format!("could not replace {}: {}", target.display(), err))?;
    }
    fs::create_dir_all(&target)
        .map_err(|err| format!("could not create {}: {}", target.display(), err))?;

    let outcome = extract_zip(&bytes, &plan.domain, &target)
        .and_then(|_| manifest_requires(&plan.domain, &target, rhapi));
    if let Err(error) = outcome {
        let _ = fs::remove_dir_all(&target);
        return Err(error);
    }
    Ok(format!("Installed {} {}", plan.domain, tag))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn zip_with(files: &[(&str, &str)]) -> Vec<u8> {
        let mut buffer = Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buffer);
            let options = zip::write::SimpleFileOptions::default();
            for (name, body) in files {
                writer.start_file(*name, options).unwrap();
                writer.write_all(body.as_bytes()).unwrap();
            }
            writer.finish().unwrap();
        }
        buffer.into_inner()
    }

    fn entry(value: Value) -> Value {
        value
    }

    fn registry_entry() -> Value {
        serde_json::json!({
            "manifest": {"domain": "fixture", "version": "1.0.0"},
            "repository": "RaceFPV/fixture",
            "last_version": "1.0.0",
            "last_prerelease": "2.0.0-rc",
            "releases": [
                {"tag_name": "1.0.0", "assets": [
                    {"name": "fixture.zip", "sha256": "abc"},
                    {"name": "notes.txt"}
                ]},
                {"tag_name": "2.0.0-rc", "assets": []}
            ]
        })
    }

    #[test]
    fn tag_falls_back_to_stable_then_prerelease() {
        let entry = registry_entry();
        assert_eq!(resolve_tag(&entry, None).unwrap(), "1.0.0");
        assert_eq!(resolve_tag(&entry, Some("2.0.0-rc")).unwrap(), "2.0.0-rc");
        let mut prerelease_only = entry.clone();
        prerelease_only["last_version"] = Value::Null;
        assert_eq!(resolve_tag(&prerelease_only, None).unwrap(), "2.0.0-rc");
        let mut none = entry.clone();
        none["last_version"] = Value::Null;
        none["last_prerelease"] = Value::Null;
        assert!(resolve_tag(&none, None).is_err());
    }

    #[test]
    fn the_domain_zip_asset_is_preferred() {
        let plan = choose_download(&registry_entry(), "1.0.0").unwrap();
        assert_eq!(
            plan.url,
            "https://github.com/RaceFPV/fixture/releases/download/1.0.0/fixture.zip"
        );
        assert_eq!(plan.sha256.as_deref(), Some("abc"));
    }

    #[test]
    fn no_asset_falls_back_to_the_repository_archive() {
        let plan = choose_download(&registry_entry(), "2.0.0-rc").unwrap();
        assert_eq!(
            plan.url,
            "https://github.com/RaceFPV/fixture/archive/refs/tags/2.0.0-rc.zip"
        );
        assert!(plan.sha256.is_none());
    }

    #[test]
    fn a_zip_filename_in_the_manifest_is_used_when_named() {
        // no <domain>.zip asset, so the manifest's zip_filename is consulted
        let mut entry = registry_entry();
        entry["manifest"]["zip_filename"] = serde_json::json!("release-bundle.zip");
        entry["releases"][0]["assets"] = serde_json::json!([
            {"name": "notes.txt"},
            {"name": "release-bundle.zip", "sha256": "def"}
        ]);
        let plan = choose_download(&entry, "1.0.0").unwrap();
        assert!(plan.url.ends_with("release-bundle.zip"), "{}", plan.url);
        assert_eq!(plan.sha256.as_deref(), Some("def"));
    }

    #[test]
    fn a_zip_filename_that_is_not_an_asset_falls_back_to_the_archive() {
        let mut entry = registry_entry();
        entry["manifest"]["zip_filename"] = serde_json::json!("missing.zip");
        entry["releases"][0]["assets"] = serde_json::json!([{"name": "notes.txt"}]);
        let plan = choose_download(&entry, "1.0.0").unwrap();
        assert!(plan.url.contains("/archive/refs/tags/1.0.0.zip"), "{}", plan.url);
    }

    #[test]
    fn only_the_plugin_directory_is_extracted() {
        let bytes = zip_with(&[
            ("repo-1.0.0/fixture/__init__.py", "print('hi')"),
            ("repo-1.0.0/fixture/manifest.json", "{\"domain\":\"fixture\",\"required_rhapi_version\":\"1.1\"}"),
            ("repo-1.0.0/README.md", "not mine"),
            ("repo-1.0.0/other/secret.py", "not mine either"),
        ]);
        let dir = std::env::temp_dir().join(format!("rhpi-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let written = extract_zip(&bytes, "fixture", &dir).unwrap();
        assert_eq!(written, 2);
        assert!(dir.join("__init__.py").is_file());
        assert!(dir.join("manifest.json").is_file());
        assert!(!dir.join("README.md").exists(), "outside the domain folder");
        assert!(!dir.join("other").exists(), "outside the domain folder");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn traversal_entries_are_skipped() {
        let bytes = zip_with(&[
            ("repo/fixture/ok.py", "ok"),
            ("repo/fixture/../../evil.py", "evil"),
        ]);
        let dir = std::env::temp_dir().join(format!("rhpi-trav-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        extract_zip(&bytes, "fixture", &dir).unwrap();
        assert!(dir.join("ok.py").is_file());
        assert!(!dir.parent().unwrap().join("evil.py").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_newer_rhapi_requirement_fails_and_rolls_back() {
        let bytes = zip_with(&[
            ("repo/fixture/__init__.py", "print('hi')"),
            ("repo/fixture/manifest.json", "{\"domain\":\"fixture\",\"required_rhapi_version\":\"9.0\"}"),
        ]);
        let dir = std::env::temp_dir().join(format!("rhpi-new-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let entry = entry(registry_entry());
        let fetch = move |_url: &str| Ok(bytes.clone());
        let result = install(&entry, None, &dir, (1, 7), &fetch);
        assert!(result.is_err(), "must refuse a plugin it cannot run");
        assert!(!dir.join("fixture").exists(), "nothing is left behind");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_matching_hash_installs_and_a_wrong_one_does_not() {
        let bytes = zip_with(&[
            ("repo/fixture/__init__.py", "print('hi')"),
            ("repo/fixture/manifest.json", "{\"domain\":\"fixture\",\"required_rhapi_version\":\"1.1\"}"),
        ]);
        let digest = Sha256::digest(&bytes);
        let hex = digest.iter().map(|b| format!("{:02x}", b)).collect::<String>();

        let dir = std::env::temp_dir().join(format!("rhpi-ok-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut entry = registry_entry();
        entry["releases"][0]["assets"][0]["sha256"] = serde_json::json!(hex);
        let payload = bytes.clone();
        let fetch = move |_url: &str| Ok(payload.clone());
        install(&entry, None, &dir, (1, 7), &fetch).expect("hash matches, install succeeds");
        assert!(dir.join("fixture").join("__init__.py").is_file());
        fs::remove_dir_all(&dir).unwrap();

        let dir = std::env::temp_dir().join(format!("rhpi-bad-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut entry = registry_entry();
        entry["releases"][0]["assets"][0]["sha256"] = serde_json::json!("deadbeef");
        let payload = bytes.clone();
        let fetch = move |_url: &str| Ok(payload.clone());
        let result = install(&entry, None, &dir, (1, 7), &fetch);
        assert!(result.is_err(), "bad hash is refused");
        assert!(!dir.join("fixture").exists());
        fs::remove_dir_all(&dir).unwrap();
    }
}
