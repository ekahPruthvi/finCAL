//! Self-update from the GitHub releases built by
//! `.github/workflows/updater.yml`.
//!
//! The workflow uploads `finCAL-x86_64.tar.gz` for every `v*` tag. `--update`
//! asks the GitHub API for the latest release, downloads that tarball, and
//! atomically replaces the running binary.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

const LATEST_API: &str = "https://api.github.com/repos/ekahPruthvi/finCAL/releases/latest";
const RELEASES_URL: &str = "https://github.com/ekahPruthvi/finCAL/releases/download";

fn asset_name() -> Result<&'static str, String> {
    match std::env::consts::ARCH {
        "x86_64" => Ok("finCAL-x86_64.tar.gz"),
        other => Err(format!("finCAL has no prebuilt binary for {other}")),
    }
}

pub fn run() -> Result<(), String> {
    let asset = asset_name()?;
    let exe = std::env::current_exe()
        .map_err(|error| format!("cannot locate the finCAL binary: {error}"))?
        .canonicalize()
        .map_err(|error| format!("cannot resolve the finCAL binary: {error}"))?;

    println!("checking GitHub for the latest finCAL release...");
    let body = curl_text(&latest_api())?;
    let release: Value = serde_json::from_str(&body)
        .map_err(|error| format!("unexpected reply from GitHub: {error}"))?;
    let tag = release["tag_name"]
        .as_str()
        .ok_or_else(|| "the latest GitHub release has no tag".to_string())?;

    let current = env!("CARGO_PKG_VERSION");
    if !is_newer(tag, current) {
        println!("finCAL is up to date (v{current})");
        return Ok(());
    }

    let url = release["assets"]
        .as_array()
        .and_then(|assets| {
            assets
                .iter()
                .find(|candidate| candidate["name"].as_str() == Some(asset))
        })
        .and_then(|candidate| candidate["browser_download_url"].as_str())
        .map(str::to_string)
        .unwrap_or_else(|| format!("{RELEASES_URL}/{tag}/{asset}"));

    let dir = std::env::temp_dir().join(format!("fincal-update-{}", std::process::id()));
    fs::create_dir_all(&dir)
        .map_err(|error| format!("cannot create {}: {error}", dir.display()))?;
    println!("downloading {tag}...");
    let result = download(&url, &dir, asset).and_then(|new_binary| {
        replace_binary(&new_binary, &exe)?;
        Ok(())
    });
    let _ = fs::remove_dir_all(&dir);
    result?;

    println!(
        "updated finCAL v{current} -> {} ({})",
        tag.trim_start_matches('v'),
        exe.display()
    );
    println!("restart any running `finCAL -s` service to use the new binary.");
    Ok(())
}

/// Debug builds may point the updater at a local release file for testing.
fn latest_api() -> String {
    #[cfg(debug_assertions)]
    if let Ok(url) = std::env::var("FINCAL_UPDATE_API") {
        if !url.is_empty() {
            return url;
        }
    }
    LATEST_API.to_string()
}

fn curl_text(url: &str) -> Result<String, String> {
    let output = Command::new("curl")
        .args(["-fsSL", "--retry", "2", "-H", "Accept: application/vnd.github+json", url])
        .output()
        .map_err(curl_error)?;
    if !output.status.success() {
        if output.status.code() == Some(22) {
            return Err(format!(
                "no readable finCAL release at {url} — the repository may be private or have no release tag yet"
            ));
        }
        return Err(format!("GitHub request failed ({}): {url}", output.status));
    }
    String::from_utf8(output.stdout).map_err(|error| format!("invalid UTF-8 from GitHub: {error}"))
}

fn download(url: &str, dir: &Path, asset: &str) -> Result<PathBuf, String> {
    let tarball = dir.join(asset);
    let status = Command::new("curl")
        .arg("-fsSL")
        .arg("--retry")
        .arg("2")
        .arg("-o")
        .arg(&tarball)
        .arg(url)
        .status()
        .map_err(curl_error)?;
    if !status.success() {
        return Err(format!("download failed ({}): {url}", status));
    }

    let status = Command::new("tar")
        .arg("-xzf")
        .arg(&tarball)
        .arg("-C")
        .arg(dir)
        .arg("finCAL")
        .status()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                "finCAL --update needs `tar` to unpack the release".to_string()
            } else {
                format!("cannot run tar: {error}")
            }
        })?;
    if !status.success() {
        return Err("the downloaded archive is not a valid finCAL release".into());
    }

    let binary = dir.join("finCAL");
    if !binary.is_file() {
        return Err("the release archive does not contain finCAL".into());
    }
    Ok(binary)
}

fn curl_error(error: std::io::Error) -> String {
    if error.kind() == std::io::ErrorKind::NotFound {
        "finCAL --update needs `curl` to download releases".to_string()
    } else {
        format!("cannot run curl: {error}")
    }
}

/// Copies the new binary beside the current one, then renames it over the
/// old file so the replacement is atomic (the running process keeps its inode).
fn replace_binary(new_binary: &Path, exe: &Path) -> Result<(), String> {
    let filename = exe
        .file_name()
        .ok_or_else(|| format!("invalid binary path {}", exe.display()))?;
    let staged = exe.with_file_name(format!(".{}.new", filename.to_string_lossy()));

    fs::copy(new_binary, &staged)
        .map_err(|error| format!("cannot write {}: {error}", staged.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&staged, fs::Permissions::from_mode(0o755))
            .map_err(|error| format!("cannot make {} executable: {error}", staged.display()))?;
    }
    if let Err(error) = fs::rename(&staged, exe) {
        let _ = fs::remove_file(&staged);
        return Err(format!(
            "cannot replace {}: {error} (try running with the permissions that own it)",
            exe.display()
        ));
    }
    Ok(())
}

/// True when `latest` is a newer `major.minor.patch` version than `current`.
fn is_newer(latest: &str, current: &str) -> bool {
    let mut latest = version_parts(latest);
    let mut current = version_parts(current);
    let len = latest.len().max(current.len());
    latest.resize(len, 0);
    current.resize(len, 0);
    latest > current
}

fn version_parts(text: &str) -> Vec<u64> {
    text.trim()
        .trim_start_matches('v')
        .split('.')
        .map(|part| {
            part.chars()
                .take_while(|character| character.is_ascii_digit())
                .collect::<String>()
                .parse()
                .unwrap_or(0)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn compares_release_tags() {
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(is_newer("v0.10.0", "0.9.9"));
        assert!(is_newer("v1.0", "0.9.9"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("v0.0.9", "0.1.0"));
        assert!(!is_newer("v0.1.0", "0.1.0-beta.1"));
    }

    #[test]
    fn replaces_the_binary_in_place() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("fincal-replace-{stamp}"));
        fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("finCAL");
        let new_binary = dir.join("downloaded-finCAL");
        fs::write(&exe, b"old").unwrap();
        fs::write(&new_binary, b"new").unwrap();

        replace_binary(&new_binary, &exe).unwrap();
        assert_eq!(fs::read_to_string(&exe).unwrap(), "new");
        assert!(!dir.join(".finCAL.new").exists());

        fs::remove_dir_all(&dir).unwrap();
    }
}
