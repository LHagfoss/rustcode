//! Cross-platform self-update engine for RustCode.
//!
//! Checks for the latest release via the GitHub Releases API (falling back
//! to the Homebrew tap if needed). On upgrade, if installed via Homebrew it
//! runs `brew upgrade rustcode`; otherwise it downloads the matching binary archive
//! (.tar.gz on macOS/Linux, .zip on Windows) from GitHub Releases and performs an
//! atomic in-place binary replacement.

use regex::Regex;
use sha2::{Digest, Sha256};
use std::io::Write;
use std::process::Command;
use std::sync::LazyLock;

pub const BREW_UPDATE_COMMAND: &str = "brew update";
pub const BREW_UPGRADE_COMMAND: &str = "brew upgrade rustcode";

const GITHUB_REPO: &str = "LHagfoss/rustcode";
const GITHUB_API_LATEST_RELEASE: &str =
    "https://api.github.com/repos/LHagfoss/rustcode/releases/latest";
const SHA256_MANIFEST_NAME: &str = "SHA256SUMS";

/// Fallback formula URLs in the tap repo.
const FORMULA_URLS: [&str; 2] = [
    "https://raw.githubusercontent.com/lhagfoss/homebrew-tap/main/Formula/rustcode.rb",
    "https://raw.githubusercontent.com/lhagfoss/homebrew-tap/master/Formula/rustcode.rb",
];

/// A semantic version as `(major, minor, patch)`. Ordered field-by-field, so
/// tuple comparison is the version comparison.
pub type Version = (u32, u32, u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateCheck {
    UpToDate { current: Version, latest: Version },
    Available { current: Version, latest: Version },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateState {
    Unknown,
    Checking,
    UpToDate(Version),
    Available(Version),
    Failed,
}

#[derive(serde::Deserialize, Debug, Clone)]
struct GithubRelease {
    tag_name: String,
    #[serde(default)]
    assets: Vec<GithubAsset>,
}

#[derive(serde::Deserialize, Debug, Clone)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

/// Check for updates against GitHub Releases (with fallback to the Homebrew tap).
pub async fn check_for_update(client: &reqwest::Client) -> Result<UpdateCheck, String> {
    let current = current_version();
    let latest = latest_available_version(client)
        .await
        .ok_or_else(|| "could not fetch the latest release information".to_string())?;
    Ok(if latest > current {
        UpdateCheck::Available { current, latest }
    } else {
        UpdateCheck::UpToDate { current, latest }
    })
}

/// The version this binary was built as.
pub fn current_version() -> Version {
    parse_semver(env!("CARGO_PKG_VERSION")).unwrap_or((0, 0, 0))
}

pub fn format_version(v: Version) -> String {
    format!("{}.{}.{}", v.0, v.1, v.2)
}

pub fn parse_semver(s: &str) -> Option<Version> {
    let s = s.trim().trim_start_matches('v');
    let mut it = s.split('.');
    let major = it.next()?.parse().ok()?;
    let minor = it.next()?.parse().ok()?;
    // The patch segment may carry a pre-release/build suffix (e.g. "3-beta");
    // keep only the leading digits.
    let patch_digits: String = it
        .next()?
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    let patch = patch_digits.parse().ok()?;
    Some((major, minor, patch))
}

/// Detect if the current binary is installed via Homebrew.
pub fn is_brew_install() -> bool {
    if cfg!(target_os = "windows") {
        return false;
    }
    if let Ok(exe) = std::env::current_exe() {
        let path = exe.to_string_lossy();
        if path.contains("/Cellar/rustcode")
            || path.contains("/opt/homebrew/")
            || path.contains("/usr/local/Cellar/")
            || path.contains("/home/linuxbrew/")
        {
            return true;
        }
    }
    false
}

/// Expected asset name for the current platform/architecture.
pub fn target_asset_name() -> Option<&'static str> {
    target_asset_name_for(std::env::consts::OS, std::env::consts::ARCH)
}

fn target_asset_name_for(os: &str, arch: &str) -> Option<&'static str> {
    match (os, arch) {
        ("linux", "x86_64") => Some("rustcode-linux-x86_64.tar.gz"),
        ("macos", "aarch64") => Some("rustcode-macos-aarch64.tar.gz"),
        ("windows", "x86_64") => Some("rustcode-windows-x86_64.zip"),
        _ => None,
    }
}

/// Fetch the latest version from GitHub Releases, falling back to Homebrew tap.
pub async fn latest_available_version(client: &reqwest::Client) -> Option<Version> {
    if let Some((v, _)) = fetch_github_latest(client).await {
        return Some(v);
    }
    latest_tap_version(client).await
}

async fn fetch_github_latest(client: &reqwest::Client) -> Option<(Version, String)> {
    let resp = client
        .get(GITHUB_API_LATEST_RELEASE)
        .header("User-Agent", "rustcode")
        .header("Accept", "application/vnd.github.v3+json")
        .send()
        .await
        .ok()?;

    if resp.status().is_success() {
        let release = resp.json::<GithubRelease>().await.ok()?;
        let version = parse_semver(&release.tag_name)?;
        let target_name = target_asset_name()?;
        let download_url = release
            .assets
            .into_iter()
            .find(|a| a.name == target_name)
            .map(|a| a.browser_download_url)?;
        return Some((version, download_url));
    }
    None
}

/// Extract formula version from raw Ruby formula content.
fn parse_formula_version(rb: &str) -> Option<Version> {
    static EXPLICIT: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"(?:version\s+"|tag:\s*")v?(\d+)\.(\d+)\.(\d+)"#).unwrap());
    static ANY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"v?(\d+)\.(\d+)\.(\d+)").unwrap());

    let cap3 = |c: &regex::Captures| -> Option<Version> {
        Some((c[1].parse().ok()?, c[2].parse().ok()?, c[3].parse().ok()?))
    };

    if let Some(c) = EXPLICIT.captures(rb).and_then(|c| cap3(&c)) {
        return Some(c);
    }
    rb.lines()
        .filter(|l| l.contains("url") || l.contains("archive") || l.contains(".tar"))
        .filter_map(|l| ANY.captures(l).and_then(|c| cap3(&c)))
        .max()
}

/// Fetch the latest version published in the Homebrew tap.
pub async fn latest_tap_version(client: &reqwest::Client) -> Option<Version> {
    for url in FORMULA_URLS {
        let resp = client
            .get(url)
            .header("User-Agent", "rustcode")
            .send()
            .await;
        if let Ok(resp) = resp
            && resp.status().is_success()
            && let Ok(body) = resp.text().await
            && let Some(v) = parse_formula_version(&body)
        {
            return Some(v);
        }
    }
    None
}

/// Perform update — automatically chooses Homebrew if installed via brew,
/// otherwise downloads and replaces binary in-place from GitHub Releases.
pub async fn run_update(client: &reqwest::Client, expected: Version) -> Result<(), String> {
    if is_brew_install() {
        println!("Detected Homebrew installation.");
        let brew_result = tokio::task::spawn_blocking(move || run_brew_upgrade(expected)).await;
        match brew_result {
            Ok(Ok(())) => return Ok(()),
            Ok(Err(brew_err)) => {
                eprintln!(
                    "Homebrew upgrade failed ({brew_err}), falling back to direct binary update..."
                );
            }
            Err(join_err) => {
                eprintln!(
                    "Homebrew task failed ({join_err}), falling back to direct binary update..."
                );
            }
        }
    }

    run_direct_upgrade(client, expected).await
}

/// Download matching archive from GitHub Releases and replace the current binary.
pub async fn run_direct_upgrade(client: &reqwest::Client, expected: Version) -> Result<(), String> {
    let asset_name = target_asset_name().ok_or_else(|| {
        format!(
            "Unsupported platform: {}/{}",
            std::env::consts::OS,
            std::env::consts::ARCH
        )
    })?;

    println!(
        "Fetching release download URL for v{}...",
        format_version(expected)
    );
    let _ = std::io::stdout().flush();

    let download_url = format!(
        "https://github.com/{GITHUB_REPO}/releases/download/v{}/{asset_name}",
        format_version(expected)
    );

    println!("Downloading {asset_name} from GitHub Releases...");
    let _ = std::io::stdout().flush();

    let resp = client
        .get(&download_url)
        .header("User-Agent", "rustcode")
        .send()
        .await
        .map_err(|e| format!("failed to download release from {download_url}: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!(
            "download failed with HTTP {} from {download_url}",
            resp.status()
        ));
    }

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("failed to read response bytes: {e}"))?;

    println!("Verifying {asset_name} against {SHA256_MANIFEST_NAME}...");
    let _ = std::io::stdout().flush();
    let manifest_url = format!(
        "https://github.com/{GITHUB_REPO}/releases/download/v{}/{SHA256_MANIFEST_NAME}",
        format_version(expected)
    );
    let manifest_resp = client
        .get(&manifest_url)
        .header("User-Agent", "rustcode")
        .send()
        .await
        .map_err(|e| format!("failed to download checksum manifest from {manifest_url}: {e}"))?;
    if !manifest_resp.status().is_success() {
        return Err(format!(
            "checksum manifest download failed with HTTP {} from {manifest_url}",
            manifest_resp.status()
        ));
    }
    let manifest = manifest_resp
        .text()
        .await
        .map_err(|e| format!("failed to read checksum manifest: {e}"))?;
    verify_checksum_manifest(&manifest, asset_name, &bytes)?;

    println!("Extracting binary...");
    let _ = std::io::stdout().flush();

    let current_exe = std::env::current_exe()
        .map_err(|e| format!("could not determine current executable path: {e}"))?;
    let parent_dir = current_exe
        .parent()
        .ok_or_else(|| "could not determine binary directory".to_string())?;

    // Create temporary file in same parent directory to ensure atomic same-filesystem rename
    let temp_dest = tempfile::Builder::new()
        .prefix(".rustcode_update_")
        .tempfile_in(parent_dir)
        .or_else(|_| tempfile::NamedTempFile::new())
        .map_err(|e| format!("failed to create temp file for extraction: {e}"))?;

    let temp_path = temp_dest.into_temp_path();

    if asset_name.ends_with(".tar.gz") {
        extract_from_tar_gz(&bytes, &temp_path)?;
    } else if asset_name.ends_with(".zip") {
        extract_from_zip(&bytes, &temp_path)?;
    } else {
        return Err(format!("unsupported archive format for {asset_name}"));
    }

    println!("Replacing binary at {}...", current_exe.display());
    let _ = std::io::stdout().flush();

    replace_binary(&temp_path, &current_exe)?;

    Ok(())
}

fn verify_checksum_manifest(manifest: &str, asset_name: &str, bytes: &[u8]) -> Result<(), String> {
    let expected = checksum_for_asset(manifest, asset_name)?;
    let actual = format!("{:x}", Sha256::digest(bytes));
    if expected != actual {
        return Err(format!(
            "SHA-256 mismatch for {asset_name}: expected {expected}, got {actual}"
        ));
    }
    Ok(())
}

fn checksum_for_asset(manifest: &str, asset_name: &str) -> Result<String, String> {
    let mut found = None;
    for line in manifest.lines().filter(|line| !line.trim().is_empty()) {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() < 2 {
            continue;
        }
        let filename = fields[1].strip_prefix('*').unwrap_or(fields[1]);
        if filename != asset_name {
            continue;
        }
        if fields.len() != 2 {
            return Err(format!(
                "SHA256SUMS contains a malformed entry for {asset_name}"
            ));
        }
        let checksum = fields[0].to_ascii_lowercase();
        if checksum.len() != 64 || !checksum.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!(
                "SHA256SUMS contains a malformed checksum for {asset_name}"
            ));
        }
        if found.is_some() {
            return Err(format!(
                "SHA256SUMS contains duplicate entries for {asset_name}"
            ));
        }
        found = Some(checksum);
    }
    found.ok_or_else(|| format!("SHA256SUMS has no entry for {asset_name}"))
}

fn extract_from_tar_gz(bytes: &[u8], target: &std::path::Path) -> Result<(), String> {
    use flate2::read::GzDecoder;
    use tar::Archive;

    let gz = GzDecoder::new(bytes);
    let mut archive = Archive::new(gz);

    for entry in archive
        .entries()
        .map_err(|e| format!("invalid tar archive: {e}"))?
    {
        let mut entry = entry.map_err(|e| format!("invalid tar entry: {e}"))?;
        let path = entry
            .path()
            .map_err(|e| format!("invalid entry path: {e}"))?;
        let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

        if filename.starts_with("rustcode") && !filename.ends_with(".tar.gz") {
            let mut outfile = std::fs::File::create(target)
                .map_err(|e| format!("failed to write extracted file {}: {e}", target.display()))?;
            std::io::copy(&mut entry, &mut outfile)
                .map_err(|e| format!("failed to extract binary: {e}"))?;
            return Ok(());
        }
    }
    Err("could not find rustcode executable inside archive".to_string())
}

fn extract_from_zip(bytes: &[u8], target: &std::path::Path) -> Result<(), String> {
    use std::io::Cursor;
    use zip::ZipArchive;

    let cursor = Cursor::new(bytes);
    let mut archive = ZipArchive::new(cursor).map_err(|e| format!("invalid zip archive: {e}"))?;

    for i in 0..archive.len() {
        let mut file = archive
            .by_index(i)
            .map_err(|e| format!("invalid zip entry: {e}"))?;
        let name = file.name();
        let filename = std::path::Path::new(name)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");

        if filename.starts_with("rustcode") && filename.ends_with(".exe") {
            let mut outfile = std::fs::File::create(target)
                .map_err(|e| format!("failed to write extracted file {}: {e}", target.display()))?;
            std::io::copy(&mut file, &mut outfile)
                .map_err(|e| format!("failed to extract binary: {e}"))?;
            return Ok(());
        }
    }
    Err("could not find rustcode.exe inside zip archive".to_string())
}

fn replace_binary(
    temp_path: &std::path::Path,
    current_exe: &std::path::Path,
) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(temp_path, std::fs::Permissions::from_mode(0o755));
    }

    #[cfg(target_os = "windows")]
    {
        let old_exe = current_exe.with_extension("exe.old");
        let _ = std::fs::remove_file(&old_exe);
        if let Err(e) = std::fs::rename(current_exe, &old_exe) {
            return Err(format!(
                "failed to move existing binary to {}: {e}",
                old_exe.display()
            ));
        }
        if let Err(e) = std::fs::rename(temp_path, current_exe) {
            let _ = std::fs::rename(&old_exe, current_exe);
            return Err(format!(
                "failed to install new executable at {}: {e}",
                current_exe.display()
            ));
        }
        let _ = std::fs::remove_file(&old_exe);
    }

    #[cfg(not(target_os = "windows"))]
    {
        if let Err(_err) = std::fs::rename(temp_path, current_exe) {
            // Cross-device or filesystem boundary fallback: copy then remove
            std::fs::copy(temp_path, current_exe)
                .map_err(|e| format!("failed to overwrite {}: {e}", current_exe.display()))?;
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(current_exe, std::fs::Permissions::from_mode(0o755));
            let _ = std::fs::remove_file(temp_path);
        }
    }

    Ok(())
}

/// Refresh Homebrew and run the upgrade with inherited stdout/stderr.
pub fn run_brew_upgrade(expected: Version) -> Result<(), String> {
    println!("Refreshing Homebrew via `{BREW_UPDATE_COMMAND}`...");
    let _ = std::io::stdout().flush();

    let update_status = Command::new("brew")
        .arg("update")
        .status()
        .map_err(|e| format!("failed to run brew (is Homebrew installed?): {e}"))?;
    if !update_status.success() {
        return Err(format!(
            "`{BREW_UPDATE_COMMAND}` failed with status {update_status}"
        ));
    }

    println!("Updating RustCode via `{BREW_UPGRADE_COMMAND}`...");
    let _ = std::io::stdout().flush();

    let status = Command::new("brew")
        .args(["upgrade", "rustcode"])
        .status()
        .map_err(|e| format!("failed to run brew (is Homebrew installed?): {e}"))?;
    if !status.success() {
        return Err(format!(
            "`{BREW_UPGRADE_COMMAND}` failed with status {status}"
        ));
    }

    let installed = installed_brew_version()?;
    if installed < expected {
        return Err(format!(
            "Homebrew reported success, but rustcode v{} is still installed; expected v{} after refreshing the tap.",
            format_version(installed),
            format_version(expected),
        ));
    }

    Ok(())
}

fn installed_brew_version() -> Result<Version, String> {
    let output = Command::new("brew")
        .args(["list", "--versions", "rustcode"])
        .output()
        .map_err(|e| format!("failed to inspect the installed brew formula: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "could not inspect the installed rustcode formula (brew list exited with {})",
            output.status
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    installed_brew_version_from_output(&stdout).ok_or_else(|| {
        "Homebrew did not report an installed rustcode version after the upgrade".to_string()
    })
}

fn installed_brew_version_from_output(output: &str) -> Option<Version> {
    output.split_whitespace().filter_map(parse_semver).max()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_explicit_version_line() {
        let rb = r#"
            class Rustcode < Formula
              desc "Terminal coding assistant"
              version "0.6.1"
              url "https://example.com/x.tar.gz"
            end
        "#;
        assert_eq!(parse_formula_version(rb), Some((0, 6, 1)));
    }

    #[test]
    fn parses_tag_from_url_when_no_version_line() {
        let rb = r#"
            class Rustcode < Formula
              url "https://github.com/lhagfoss/rustcode/archive/refs/tags/v0.7.3.tar.gz"
              sha256 "abc123"
            end
        "#;
        assert_eq!(parse_formula_version(rb), Some((0, 7, 3)));
    }

    #[test]
    fn picks_highest_semver_across_url_lines() {
        let rb = r#"
              url "https://host/rustcode/archive/v0.6.0.tar.gz"
              head "https://host/rustcode/archive/v0.9.0.tar.gz"
              sha256 "deadbeef1234"
        "#;
        assert_eq!(parse_formula_version(rb), Some((0, 9, 0)));
    }

    #[test]
    fn version_ordering_is_field_wise() {
        assert!((0, 5, 0) < (0, 5, 1));
        assert!((0, 5, 9) < (0, 6, 0));
        assert!((1, 0, 0) > (0, 99, 99));
    }

    #[test]
    fn current_version_matches_cargo() {
        assert_eq!(
            current_version(),
            parse_semver(env!("CARGO_PKG_VERSION")).unwrap()
        );
    }

    #[test]
    fn update_command_is_the_formula_upgrade() {
        assert_eq!(BREW_UPDATE_COMMAND, "brew update");
        assert_eq!(BREW_UPGRADE_COMMAND, "brew upgrade rustcode");
    }

    #[test]
    fn parses_installed_brew_version_output() {
        assert_eq!(
            installed_brew_version_from_output("rustcode 0.29.6\n"),
            Some((0, 29, 6))
        );
        assert_eq!(
            installed_brew_version_from_output("rustcode 0.29.5 0.29.6\n"),
            Some((0, 29, 6))
        );
        assert_eq!(installed_brew_version_from_output(""), None);
    }

    #[test]
    fn target_asset_detection() {
        assert!(target_asset_name().is_some());
    }

    #[test]
    fn target_asset_mapping_covers_supported_platforms() {
        assert_eq!(
            target_asset_name_for("linux", "x86_64"),
            Some("rustcode-linux-x86_64.tar.gz")
        );
        assert_eq!(
            target_asset_name_for("macos", "aarch64"),
            Some("rustcode-macos-aarch64.tar.gz")
        );
        assert_eq!(target_asset_name_for("macos", "x86_64"), None);
        assert_eq!(
            target_asset_name_for("windows", "x86_64"),
            Some("rustcode-windows-x86_64.zip")
        );
        assert_eq!(target_asset_name_for("windows", "aarch64"), None);
    }

    #[test]
    fn checksum_manifest_accepts_exact_asset_and_binary_marker() {
        let bytes = b"rustcode";
        let checksum = format!("{:x}", Sha256::digest(bytes));
        let manifest = format!("{checksum}  rustcode-linux-x86_64.tar.gz\n");
        assert!(verify_checksum_manifest(&manifest, "rustcode-linux-x86_64.tar.gz", bytes).is_ok());

        let marked = format!("{checksum} *rustcode-linux-x86_64.tar.gz\n");
        assert!(verify_checksum_manifest(&marked, "rustcode-linux-x86_64.tar.gz", bytes).is_ok());
    }

    #[test]
    fn checksum_manifest_rejects_missing_malformed_duplicate_and_mismatch() {
        let bytes = b"rustcode";
        let checksum = format!("{:x}", Sha256::digest(bytes));
        assert!(
            checksum_for_asset("", "rustcode-linux-x86_64.tar.gz")
                .unwrap_err()
                .contains("no entry")
        );
        assert!(
            checksum_for_asset(
                "not-a-checksum  rustcode-linux-x86_64.tar.gz",
                "rustcode-linux-x86_64.tar.gz"
            )
            .unwrap_err()
            .contains("malformed")
        );
        assert!(
            checksum_for_asset(
                &format!("{checksum}  rustcode-linux-x86_64.tar.gz trailing"),
                "rustcode-linux-x86_64.tar.gz"
            )
            .unwrap_err()
            .contains("malformed")
        );
        let duplicate = format!(
            "{checksum}  rustcode-linux-x86_64.tar.gz\n{checksum}  rustcode-linux-x86_64.tar.gz"
        );
        assert!(
            checksum_for_asset(&duplicate, "rustcode-linux-x86_64.tar.gz")
                .unwrap_err()
                .contains("duplicate")
        );
        let mismatch = format!("{checksum}  rustcode-linux-x86_64.tar.gz\n");
        assert!(
            verify_checksum_manifest(&mismatch, "rustcode-linux-x86_64.tar.gz", b"different")
                .unwrap_err()
                .contains("mismatch")
        );
    }
}
