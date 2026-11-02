//! The in-app updater, the SwiftUI app's `Updater.swift` ported: ask GitHub
//! for the latest release, and on macOS install it in place. It downloads
//! `RustCode-<ver>-macos-<arch>.app.zip`, checks it against the release's
//! `SHA256SUMS`, unpacks it with `ditto`, requires a valid code signature and
//! this app's bundle identifier, then a detached shell swaps the bundle once
//! this process has quit and opens the new one.
//!
//! No tauri-plugin-updater: it needs its own signing key pair, and every
//! release would fail without that secret. SHA256SUMS is what the installers
//! already trust. Windows and Linux report the release and leave installing
//! it to the installer (docs/install.md).

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tauri::AppHandle;

const REPO: &str = "LHagfoss/rustcode";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    /// This app's version.
    current: String,
    /// The latest release's version, when GitHub answered.
    latest: Option<String>,
    /// The latest release is newer and has an app for this platform.
    available: bool,
    /// The app can replace itself (a macOS release bundle in a folder it may write).
    can_install: bool,
    /// A `tauri dev` build, which never updates itself.
    development: bool,
    /// The release page, for platforms that update with the installer.
    page: Option<String>,
}

/// `X.Y.Z[-pre]` as ((X, Y, Z), has a pre-release suffix).
fn parse(version: &str) -> Option<((u64, u64, u64), bool)> {
    let version = version.trim().trim_start_matches('v');
    let (core, pre) = match version.split_once('-') {
        Some((core, _)) => (core, true),
        None => (version, false),
    };
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    let triple = (parts.next()??, parts.next()??, parts.next()??);
    if parts.next().is_some() {
        return None;
    }
    Some((triple, pre))
}

/// Whether `latest` is newer than `current`; a beta is older than its release.
fn newer(latest: &str, current: &str) -> bool {
    match (parse(latest), parse(current)) {
        (Some((l, lpre)), Some((c, cpre))) => l > c || (l == c && cpre && !lpre),
        _ => false,
    }
}

fn arch() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "x86_64"
    }
}

fn asset_name(version: &str) -> String {
    format!("RustCode-{version}-macos-{}.app.zip", arch())
}

fn download_url(tag: &str, asset: &str) -> String {
    format!("https://github.com/{REPO}/releases/download/{tag}/{asset}")
}

/// The one well-formed `SHA256SUMS` entry for `asset`.
fn checksum(sums: &str, asset: &str) -> Option<String> {
    let mut found = sums.lines().filter_map(|line| {
        let fields: Vec<&str> = line.split_whitespace().collect();
        (fields.get(1).map(|f| f.trim_start_matches('*')) == Some(asset)).then_some(fields)
    });
    let fields = found.next()?;
    if found.next().is_some() || fields.len() != 2 {
        return None;
    }
    let hash = fields[0].to_ascii_lowercase();
    (hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit())).then_some(hash)
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent("rustcode-desktop-updater")
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(600))
        .build()
        .map_err(|e| e.to_string())
}

/// The latest release's tag, from where `releases/latest` redirects
/// (`…/releases/tag/vX.Y.Z`); betas are prereleases and never latest.
async fn latest_tag(client: &reqwest::Client) -> Result<String, String> {
    let response = client
        .head(format!("https://github.com/{REPO}/releases/latest"))
        .send()
        .await
        .map_err(|e| format!("GitHub did not answer the update check: {e}"))?;
    let url = response.url().clone();
    url.path()
        .rsplit_once("/releases/tag/")
        .map(|(_, tag)| tag.to_string())
        .filter(|tag| parse(tag).is_some())
        .ok_or_else(|| "GitHub did not answer the update check.".to_string())
}

async fn exists(client: &reqwest::Client, url: &str) -> bool {
    client
        .head(url)
        .send()
        .await
        .is_ok_and(|r| r.status().is_success())
}

/// The `.app` this process runs from, when it runs from one.
fn bundle_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.ancestors()
        .find(|p| p.extension().is_some_and(|e| e == "app"))
        .map(Path::to_path_buf)
}

fn writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".rustcode-update-probe-{}", std::process::id()));
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

fn can_install() -> bool {
    cfg!(target_os = "macos")
        && !cfg!(debug_assertions)
        && bundle_path()
            .and_then(|b| b.parent().map(writable))
            .unwrap_or(false)
}

async fn check(app: &AppHandle) -> Result<(UpdateInfo, String), String> {
    let current = app.package_info().version.to_string();
    let development = cfg!(debug_assertions);
    let client = client()?;
    let tag = latest_tag(&client).await?;
    let latest = tag.trim_start_matches('v').to_string();
    let page = format!("https://github.com/{REPO}/releases/tag/{tag}");
    // A development build is always behind, as in the Swift app, but it only
    // links to the release.
    let mut available = development || newer(&latest, &current);
    if available && cfg!(target_os = "macos") {
        // Tagged, but its macOS app is not uploaded yet.
        available = exists(&client, &download_url(&tag, &asset_name(&latest))).await;
    }
    let info = UpdateInfo {
        current,
        latest: Some(latest),
        available,
        can_install: can_install(),
        development,
        page: Some(page),
    };
    Ok((info, tag))
}

/// A release's notes (its GitHub body, Markdown), for What's New. `None` when
/// the release does not exist or has no notes.
#[tauri::command]
pub async fn release_notes(version: String) -> Result<Option<String>, String> {
    let version = version.trim().trim_start_matches('v');
    if parse(version).is_none() {
        return Err(format!("not a version: {version}"));
    }
    let response = client()?
        .get(format!(
            "https://api.github.com/repos/{REPO}/releases/tags/v{version}"
        ))
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("GitHub did not answer: {e}"))?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !response.status().is_success() {
        return Err(format!("GitHub answered {}", response.status()));
    }
    let text = response.text().await.map_err(|e| e.to_string())?;
    let release: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    Ok(release
        .get("body")
        .and_then(|body| body.as_str())
        .map(str::trim)
        .filter(|body| !body.is_empty())
        .map(str::to_owned))
}

#[tauri::command]
pub async fn update_check(app: AppHandle) -> Result<UpdateInfo, String> {
    check(&app).await.map(|(info, _)| info)
}

/// Downloads, verifies and stages the update, then quits; the app reopens
/// updated. An error leaves the running app as it was.
#[tauri::command]
pub async fn update_install(app: AppHandle) -> Result<(), String> {
    let (info, tag) = check(&app).await?;
    if !info.available || !info.can_install {
        return Err("No update this app can install itself.".into());
    }
    let version = info.latest.unwrap_or_default();
    let asset = asset_name(&version);
    let client = client()?;
    let work = std::env::temp_dir().join(format!(
        "rustcode-update-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let zip = work.join(&asset);
    let result = async {
        let body = fetch(&client, &download_url(&tag, &asset)).await?;
        std::fs::write(&zip, &body).map_err(|e| e.to_string())?;
        let sums = fetch(&client, &download_url(&tag, "SHA256SUMS")).await?;
        let expected = checksum(&String::from_utf8_lossy(&sums), &asset)
            .ok_or("The release checksums do not list the app.")?;
        stage(&app, &zip, &work, &expected)
    }
    .await;
    match result {
        Ok(new_app) => relaunch(&app, &new_app),
        Err(e) => {
            let _ = std::fs::remove_dir_all(&work);
            Err(e)
        }
    }
}

async fn fetch(client: &reqwest::Client, url: &str) -> Result<Vec<u8>, String> {
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("The download failed: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("The download failed (HTTP {}).", response.status()));
    }
    Ok(response.bytes().await.map_err(|e| e.to_string())?.to_vec())
}

#[cfg(target_os = "macos")]
fn run(tool: &str, args: &[&std::ffi::OsStr]) -> Result<String, String> {
    let output = std::process::Command::new(tool)
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        let name = Path::new(tool)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy();
        return Err(format!("{name} failed while installing the update."));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Verifies the zip and unpacks it; returns the new RustCode.app.
#[cfg(target_os = "macos")]
fn stage(app: &AppHandle, zip: &Path, work: &Path, expected: &str) -> Result<PathBuf, String> {
    let digest = run(
        "/usr/bin/shasum",
        &["-a".as_ref(), "256".as_ref(), zip.as_os_str()],
    )?;
    if digest
        .split_whitespace()
        .next()
        .map(str::to_ascii_lowercase)
        .as_deref()
        != Some(expected)
    {
        return Err("The download does not match the release checksum.".into());
    }
    let unpacked = work.join("unpacked");
    run(
        "/usr/bin/ditto",
        &[
            "-x".as_ref(),
            "-k".as_ref(),
            zip.as_os_str(),
            unpacked.as_os_str(),
        ],
    )?;
    let new_app = unpacked.join("RustCode.app");
    if !new_app.join("Contents/Info.plist").is_file() {
        return Err("The download has no RustCode.app.".into());
    }
    run(
        "/usr/bin/codesign",
        &[
            "--verify".as_ref(),
            "--deep".as_ref(),
            "--strict".as_ref(),
            new_app.as_os_str(),
        ],
    )?;
    let plist = new_app.join("Contents/Info.plist");
    let id = run(
        "/usr/libexec/PlistBuddy",
        &[
            "-c".as_ref(),
            "Print :CFBundleIdentifier".as_ref(),
            plist.as_os_str(),
        ],
    )?;
    if id.trim() != app.config().identifier {
        return Err("The download is not this app.".into());
    }
    Ok(new_app)
}

#[cfg(not(target_os = "macos"))]
fn stage(_: &AppHandle, _: &Path, _: &Path, _: &str) -> Result<PathBuf, String> {
    Err("Run the installer again to update RustCode on this platform.".into())
}

/// A detached shell waits for this process to exit, swaps the bundle and
/// opens the new one; the app then quits.
fn relaunch(app: &AppHandle, new_app: &Path) -> Result<(), String> {
    let current = bundle_path().ok_or("RustCode is not running from an app bundle.")?;
    let script = r#"while kill -0 "$1" 2>/dev/null; do sleep 0.2; done
rm -rf "$2.old" && mv "$2" "$2.old" && mv "$3" "$2" && rm -rf "$2.old"
xattr -dr com.apple.quarantine "$2" 2>/dev/null
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$2" 2>/dev/null
open "$2"
rm -rf "$4""#;
    let work = new_app.parent().and_then(Path::parent).unwrap_or(new_app);
    std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(script)
        .arg("sh")
        .arg(std::process::id().to_string())
        .arg(&current)
        .arg(new_app)
        .arg(work)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    app.exit(0);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert!(newer("0.116.0", "0.115.0"));
        assert!(newer("v1.0.0", "0.999.9"));
        assert!(newer("0.116.0", "0.116.0-beta.2"));
        assert!(!newer("0.115.0", "0.115.0"));
        assert!(!newer("0.114.9", "0.115.0"));
        assert!(!newer("garbage", "0.115.0"));
    }

    #[test]
    fn reads_one_checksum() {
        let hash = "a".repeat(64);
        let asset = "RustCode-0.116.0-macos-aarch64.app.zip";
        let sums = format!("{hash}  {asset}\n{}  other.zip\n", "b".repeat(64));
        assert_eq!(checksum(&sums, asset), Some(hash.clone()));
        assert_eq!(
            checksum(&format!("{hash} *{asset}\n"), asset),
            Some(hash.clone())
        );
        assert_eq!(checksum(&format!("{sums}{hash}  {asset}\n"), asset), None);
        assert_eq!(checksum(&format!("{hash}  {asset} extra\n"), asset), None);
        assert_eq!(checksum(&format!("xyz  {asset}\n"), asset), None);
        assert_eq!(checksum("", asset), None);
    }
}
