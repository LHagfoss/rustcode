//! The in-app updater, the SwiftUI app's `Updater.swift` ported: ask GitHub
//! what the update channel offers, and on macOS install it in place. It
//! downloads `RustCode-<ver>-macos-<arch>.app.zip`, checks it against the
//! release's `SHA256SUMS`, unpacks it with `ditto`, requires a valid code
//! signature and this app's bundle identifier, then a detached shell swaps the
//! bundle once this process has quit and opens the new one.
//!
//! The channel is Stable or Beta (Settings › About). Stable reads the tag that
//! `releases/latest` redirects to, as it always has. Beta also offers
//! prereleases, which `releases/latest` never points at, so it reads the
//! release feed instead (`releases.atom`, which lists prereleases) and offers
//! the newest version of either kind. Betas ship the macOS app only, so a beta
//! is offered only where there is an app to install (docs/desktop.md › Updater).
//!
//! No tauri-plugin-updater: it needs its own signing key pair, and every
//! release would fail without that secret. SHA256SUMS is what the installers
//! already trust. Windows and Linux report the release and leave installing
//! it to the installer (docs/install.md).

use serde::Serialize;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tauri::ipc::Channel;
use tauri::AppHandle;

const REPO: &str = "LHagfoss/rustcode";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    /// This app's version.
    current: String,
    /// The version to update to, when the channel has one that is newer and
    /// ready to install here.
    latest: Option<String>,
    /// The channel offers an update this platform can install.
    available: bool,
    /// The app can replace itself (a macOS release bundle in a folder it may write).
    can_install: bool,
    /// A `tauri dev` build, which never updates itself.
    development: bool,
    /// The release page, for platforms that update with the installer.
    page: Option<String>,
    /// The channels this build offers: `beta` only where a beta has an app.
    channels: Vec<&'static str>,
    /// The channel this answer was for, so an answer to the previous setting is
    /// never shown as the current one.
    channel: &'static str,
}

/// Which releases the app offers: stable ones, or prereleases too.
///
/// Betas are GitHub prereleases, so `releases/latest` skips them; on Beta the
/// check reads the release feed and offers the newest version of either kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReleaseChannel {
    Stable,
    Beta,
}

/// The channel a check runs on. Anything but `beta` is Stable, so a missing or
/// unknown setting never opts anyone into a prerelease, and a Windows or Linux
/// build stays on Stable: a beta ships the macOS app only.
fn channel_of(name: Option<&str>) -> ReleaseChannel {
    match (name.map(str::trim), offers_beta()) {
        (Some("beta"), true) => ReleaseChannel::Beta,
        _ => ReleaseChannel::Stable,
    }
}

impl ReleaseChannel {
    fn name(self) -> &'static str {
        match self {
            ReleaseChannel::Stable => "stable",
            ReleaseChannel::Beta => "beta",
        }
    }
}

/// Whether a beta can be installed on this platform: betas build the macOS app
/// and CLI only (.github/workflows/build.yml).
fn offers_beta() -> bool {
    cfg!(target_os = "macos")
}

fn channel_names() -> Vec<&'static str> {
    if offers_beta() {
        vec!["stable", "beta"]
    } else {
        vec!["stable"]
    }
}

/// One dot-separated field of a prerelease suffix: `beta.10` orders as text
/// then number.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Identifier {
    /// Sorts before any text field, as semver has it.
    Number(u64),
    Text(String),
}

/// A release version as semver orders them: `1.20.0-beta.2` is older than
/// `1.20.0`, and `1.20.0-beta.10` is newer than `1.20.0-beta.9`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Version {
    core: (u64, u64, u64),
    /// The `-pre` fields; empty for a release.
    pre: Vec<Identifier>,
}

impl Version {
    fn is_prerelease(&self) -> bool {
        !self.pre.is_empty()
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        match self.core.cmp(&other.core) {
            Ordering::Equal => {}
            ordering => return ordering,
        }
        // A release is newer than any prerelease of it, and fields compare one
        // by one, the longer suffix being newer when the shared fields tie.
        match (self.pre.is_empty(), other.pre.is_empty()) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            (false, false) => self.pre.cmp(&other.pre),
        }
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.core.0, self.core.1, self.core.2)?;
        for (i, field) in self.pre.iter().enumerate() {
            write!(f, "{}", if i == 0 { '-' } else { '.' })?;
            match field {
                Identifier::Number(n) => write!(f, "{n}")?,
                Identifier::Text(text) => write!(f, "{text}")?,
            }
        }
        Ok(())
    }
}

/// `X.Y.Z[-pre]`, with or without the `v` the tags carry. Build metadata
/// (`+…`) takes no part in ordering, so it is dropped: `1.21.0+build.7` is
/// `1.21.0`.
fn parse_version(version: &str) -> Option<Version> {
    let version = version.trim().trim_start_matches('v');
    let version = version.split('+').next().unwrap_or(version);
    let (core, pre) = match version.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (version, None),
    };
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    let core = (parts.next()??, parts.next()??, parts.next()??);
    if parts.next().is_some() {
        return None;
    }
    let mut fields = Vec::new();
    if let Some(pre) = pre {
        for field in pre.split('.') {
            // The identifiers semver allows: alphanumerics and hyphens.
            if field.is_empty() || !field.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                return None;
            }
            fields.push(match field.parse::<u64>() {
                Ok(n) => Identifier::Number(n),
                Err(_) => Identifier::Text(field.to_string()),
            });
        }
    }
    Some(Version { core, pre: fields })
}

/// Whether `latest` is newer than `current`; a beta is older than its release.
fn newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(latest), Some(current)) => latest > current,
        _ => false,
    }
}

/// The versions `channel` offers, newest first, without the one skipped: a
/// skipped beta hides that version only, so the next one shows as usual.
fn candidates(tags: &[String], channel: ReleaseChannel, skipped: Option<&str>) -> Vec<String> {
    let skipped = skipped.and_then(parse_version);
    let mut versions: Vec<Version> = tags.iter().filter_map(|tag| parse_version(tag)).collect();
    versions.sort_by(|a, b| b.cmp(a));
    versions.dedup();
    versions
        .into_iter()
        .filter(|version| channel == ReleaseChannel::Beta || !version.is_prerelease())
        .filter(|version| skipped.as_ref() != Some(version))
        .map(|version| version.to_string())
        .collect()
}

/// Every release tag a `releases.atom` feed names, in feed order. The feed is
/// the release list without the API's rate limit (a check runs every 30 s),
/// and unlike `releases/latest` it includes prereleases.
///
/// Only the entries' own `<link>` elements count, and only a href that names
/// `…/releases/tag/vX.Y.Z[-pre]`: a release body is quoted HTML that links the
/// releases mentioned in its notes, and those are not this release's tag.
fn tags_in_feed(feed: &str) -> Vec<String> {
    const PATH: &str = "/releases/tag/";
    feed.match_indices("<link")
        .filter_map(|(at, _)| {
            let element = feed.get(at..)?.split_once('>')?.0;
            let href = element.split_once("href=\"")?.1.split_once('"')?.0;
            let tag = href.split_once(PATH)?.1;
            // A tag with build metadata is skipped rather than compared wrongly.
            (parse_version(tag).is_some() && !tag.contains('+')).then(|| tag.to_string())
        })
        .collect()
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

/// The latest stable release's tag, from where `releases/latest` redirects
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
        .filter(|tag| parse_version(tag).is_some())
        .ok_or_else(|| "GitHub did not answer the update check.".to_string())
}

/// The release tags the channel chooses from: the one `releases/latest` names
/// on Stable, every release on Beta.
async fn release_tags(
    client: &reqwest::Client,
    channel: ReleaseChannel,
) -> Result<Vec<String>, String> {
    match channel {
        ReleaseChannel::Stable => Ok(vec![latest_tag(client).await?]),
        ReleaseChannel::Beta => {
            let response = client
                .get(format!("https://github.com/{REPO}/releases.atom"))
                .send()
                .await
                .map_err(|e| format!("GitHub did not answer the update check: {e}"))?;
            if !response.status().is_success() {
                return Err("GitHub did not answer the update check.".to_string());
            }
            let feed = response.text().await.map_err(|e| e.to_string())?;
            let tags = tags_in_feed(&feed);
            if tags.is_empty() {
                return Err("GitHub did not answer the update check.".to_string());
            }
            Ok(tags)
        }
    }
}

async fn exists(client: &reqwest::Client, url: &str) -> bool {
    client
        .head(url)
        .send()
        .await
        .is_ok_and(|r| r.status().is_success())
}

/// Whether the release can be installed now: its app is uploaded and its
/// `SHA256SUMS` lists it.
async fn ready(client: &reqwest::Client, tag: &str, asset: &str) -> bool {
    if !exists(client, &download_url(tag, asset)).await {
        return false;
    }
    match fetch(client, &download_url(tag, "SHA256SUMS")).await {
        Ok(sums) => checksum(&String::from_utf8_lossy(&sums), asset).is_some(),
        Err(_) => false,
    }
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

/// What the channel offers this app, if anything: the newest version that is
/// newer than this one (any, for a dev build) and, on macOS, uploaded with its
/// `SHA256SUMS`. A release whose app is not there yet is passed over for the
/// one before it.
async fn select(
    client: &reqwest::Client,
    current: &str,
    channel: ReleaseChannel,
    skipped: Option<&str>,
    development: bool,
) -> Result<Option<String>, String> {
    let tags = release_tags(client, channel).await?;
    for version in candidates(&tags, channel, skipped) {
        // Newest first: once one is not newer, none after it is.
        if !development && !newer(&version, current) {
            break;
        }
        // Tagged, but not finished: the release goes public with the macOS
        // app minutes before the other platforms and `SHA256SUMS` land, and
        // an install needs the sums to verify the download. Offer it only
        // once both are there.
        if cfg!(target_os = "macos")
            && !ready(client, &format!("v{version}"), &asset_name(&version)).await
        {
            continue;
        }
        return Ok(Some(version));
    }
    Ok(None)
}

async fn check(
    app: &AppHandle,
    channel: ReleaseChannel,
    skipped: Option<&str>,
) -> Result<UpdateInfo, String> {
    let current = app.package_info().version.to_string();
    let development = cfg!(debug_assertions);
    let client = client()?;
    let latest = select(&client, &current, channel, skipped, development).await?;
    let page = match &latest {
        Some(version) => format!("https://github.com/{REPO}/releases/tag/v{version}"),
        None => format!("https://github.com/{REPO}/releases"),
    };
    Ok(UpdateInfo {
        current,
        available: latest.is_some(),
        latest,
        can_install: can_install(),
        development,
        page: Some(page),
        channels: channel_names(),
        channel: channel.name(),
    })
}

/// The update channels this build offers, without a release check: Settings
/// shows the picker before, and without, any answer from GitHub (a check can
/// fail offline, which must not take the picker away).
#[tauri::command]
pub fn update_channels() -> Vec<&'static str> {
    channel_names()
}

/// A release's notes (Markdown), for What's New: the curated section of the
/// repo's CHANGELOG.md at that release's tag when it has one, else the GitHub
/// release body, which is generated from the pull request list. `None` when
/// neither exists or the release has no notes.
#[tauri::command]
pub async fn release_notes(version: String) -> Result<Option<String>, String> {
    let version = version.trim().trim_start_matches('v');
    if parse_version(version).is_none() {
        return Err(format!("not a version: {version}"));
    }
    // What release.sh wrote: one line per change, in the user's words, with the
    // pull request number. GitHub's body is the fallback, not the main course.
    if let Some(notes) = changelog_notes(version).await {
        return Ok(Some(notes));
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

/// The curated notes for `version` from the repo's CHANGELOG.md at that tag.
/// `None` when the tag has no CHANGELOG.md, it has no section for the version,
/// or the request fails: the release body is what is left to say.
async fn changelog_notes(version: &str) -> Option<String> {
    let response = client()
        .ok()?
        .get(format!(
            "https://raw.githubusercontent.com/{REPO}/v{version}/CHANGELOG.md"
        ))
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let text = response.text().await.ok()?;
    changelog_section(&text, version)
}

/// One version's part of CHANGELOG.md: from its `## [v1.20.1](…)` heading to
/// the next version's, without the heading itself (scripts/release.sh writes a
/// `### Category` and its `- note (#N)` lines under it).
fn changelog_section(changelog: &str, version: &str) -> Option<String> {
    let wanted = format!("v{}", version.trim_start_matches('v'));
    let mut lines: Vec<&str> = Vec::new();
    let mut inside = false;
    for line in changelog.lines() {
        // A version heading ends the section it was reading.
        let Some(rest) = line.strip_prefix("## ") else {
            if inside {
                lines.push(line);
            }
            continue;
        };
        if inside {
            break;
        }
        inside = heading_names(rest.trim(), &wanted);
    }
    let notes = lines.join("\n");
    let notes = notes.trim();
    (!notes.is_empty()).then(|| notes.to_string())
}

/// Whether a `## …` heading names this version: `[v1.20.1](url) - 2026-11-08`,
/// the form release.sh writes, or a bare `v1.20.1`.
fn heading_names(rest: &str, wanted: &str) -> bool {
    let name = match rest.strip_prefix('[') {
        Some(linked) => match linked.split_once(']') {
            Some((name, _)) => name,
            None => return false,
        },
        None => rest.split_whitespace().next().unwrap_or_default(),
    };
    name == wanted || name == wanted.trim_start_matches('v')
}

#[tauri::command]
pub async fn update_check(
    app: AppHandle,
    channel: Option<String>,
    skipped: Option<String>,
) -> Result<UpdateInfo, String> {
    check(&app, channel_of(channel.as_deref()), skipped.as_deref()).await
}

/// Where an install is, for the sidebar's progress bar.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase", tag = "phase")]
pub enum InstallProgress {
    /// Bytes of the app zip so far, and its size when the server said.
    Downloading { received: u64, total: Option<u64> },
    /// Checking the checksum and the code signature.
    Verifying,
    /// Swapping the bundle; the app quits next.
    Installing,
}

/// Downloads, verifies and stages the update, then quits; the app reopens
/// updated. An error leaves the running app as it was.
#[tauri::command]
pub async fn update_install(
    app: AppHandle,
    channel: Option<String>,
    skipped: Option<String>,
    on_progress: Channel<InstallProgress>,
) -> Result<(), String> {
    let info = check(&app, channel_of(channel.as_deref()), skipped.as_deref()).await?;
    if !info.available || !info.can_install {
        return Err("No update this app can install itself.".into());
    }
    let version = info.latest.unwrap_or_default();
    let tag = format!("v{version}");
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
        let body = download(&client, &download_url(&tag, &asset), |received, total| {
            let _ = on_progress.send(InstallProgress::Downloading { received, total });
        })
        .await?;
        std::fs::write(&zip, &body).map_err(|e| e.to_string())?;
        let _ = on_progress.send(InstallProgress::Verifying);
        let sums = fetch(&client, &download_url(&tag, "SHA256SUMS")).await?;
        let expected = checksum(&String::from_utf8_lossy(&sums), &asset)
            .ok_or("The release checksums do not list the app.")?;
        let staged = stage(&app, &zip, &work, &expected)?;
        let _ = on_progress.send(InstallProgress::Installing);
        Ok(staged)
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

/// Like `fetch`, reporting progress at most every 64 KiB.
async fn download(
    client: &reqwest::Client,
    url: &str,
    mut progress: impl FnMut(u64, Option<u64>),
) -> Result<Vec<u8>, String> {
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("The download failed: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("The download failed (HTTP {}).", response.status()));
    }
    let total = response.content_length();
    let mut body = Vec::with_capacity(total.unwrap_or(0).min(512 << 20) as usize);
    let mut reported = 0u64;
    progress(0, total);
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| format!("The download failed: {e}"))?
    {
        body.extend_from_slice(&chunk);
        let received = body.len() as u64;
        if received - reported >= 64 * 1024 || Some(received) == total {
            reported = received;
            progress(received, total);
        }
    }
    Ok(body)
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

    /// The top of the real CHANGELOG.md, three versions deep.
    const CHANGELOG: &str = "## [v1.21.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.21.0) - 2026-11-09\n\n### Features\n- A newer thing. (#3200)\n## [v1.20.1](https://github.com/LHagfoss/rustcode/releases/tag/v1.20.1) - 2026-11-08\n\n### Features\n- The desktop app uses the lavender accent by default. (#3192)\n## [v1.20.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.20.0) - 2026-11-07\n\n### Fixes\n- An older thing. (#3190)\n";

    #[test]
    fn reads_one_versions_changelog_section() {
        assert_eq!(
            changelog_section(CHANGELOG, "1.20.1").as_deref(),
            Some("### Features\n- The desktop app uses the lavender accent by default. (#3192)")
        );
        // The tag may arrive with its `v`.
        assert_eq!(
            changelog_section(CHANGELOG, "v1.20.0").as_deref(),
            Some("### Fixes\n- An older thing. (#3190)")
        );
    }

    #[test]
    fn a_version_the_changelog_does_not_have_is_no_section() {
        // `v1.20.1` is not a prefix of `v1.20.10`, and an unknown version has nothing at all.
        assert_eq!(changelog_section(CHANGELOG, "1.20.10"), None);
        assert_eq!(changelog_section(CHANGELOG, "0.1.0"), None);
        assert_eq!(changelog_section("", "1.20.1"), None);
    }

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
    fn compares_prereleases_as_semver_orders_them() {
        assert!(newer("1.20.0-beta.2", "1.20.0-beta.1"));
        assert!(newer("1.20.0-beta.10", "1.20.0-beta.9"));
        assert!(newer("1.20.0-beta.1", "1.19.9"));
        assert!(newer("1.20.0", "1.20.0-rc.1"));
        assert!(!newer("1.20.0-beta.1", "1.20.0-beta.1"));
        assert!(!newer("1.20.0-beta.1", "1.20.0"));
        assert!(newer("1.20.0-rc.1", "1.20.0-beta.2"));
        assert!(!newer("1.20.0-beta.2", "1.20.0-rc.1"));
        assert_eq!(
            parse_version("v1.20.0-beta.2").unwrap().to_string(),
            "1.20.0-beta.2"
        );
    }

    fn tags(list: &[&str]) -> Vec<String> {
        list.iter().map(|tag| tag.to_string()).collect()
    }

    #[test]
    fn the_stable_channel_never_offers_a_prerelease() {
        let feed = tags(&["v1.21.0-beta.2", "v1.20.0", "v1.21.0-beta.1", "v1.19.0"]);
        assert_eq!(
            candidates(&feed, ReleaseChannel::Stable, None),
            tags(&["1.20.0", "1.19.0"])
        );
    }

    #[test]
    fn the_beta_channel_offers_the_newest_of_beta_and_stable() {
        let feed = tags(&[
            "v1.19.0",
            "v1.20.0-beta.2",
            "v1.20.0-beta.10",
            "v1.21.0-beta.1",
        ]);
        assert_eq!(
            candidates(&feed, ReleaseChannel::Beta, None),
            tags(&["1.21.0-beta.1", "1.20.0-beta.10", "1.20.0-beta.2", "1.19.0"])
        );
        // A stable release outranks the betas of itself, so it is offered next.
        let feed = tags(&["v1.20.0-beta.2", "v1.20.0"]);
        assert_eq!(
            candidates(&feed, ReleaseChannel::Beta, None),
            tags(&["1.20.0", "1.20.0-beta.2"])
        );
    }

    #[test]
    fn a_skipped_beta_is_hidden_and_the_next_one_shows() {
        let feed = tags(&["v1.20.0-beta.3", "v1.20.0-beta.2", "v1.19.0"]);
        let offered = candidates(&feed, ReleaseChannel::Beta, Some("1.20.0-beta.3"));
        assert_eq!(offered, tags(&["1.20.0-beta.2", "1.19.0"]));
        assert!(!offered.contains(&"1.20.0-beta.3".to_string()));
        // The version skipped is named with or without its `v`, as the UI and the tag have it.
        assert_eq!(
            candidates(&feed, ReleaseChannel::Beta, Some("v1.20.0-beta.3")),
            tags(&["1.20.0-beta.2", "1.19.0"])
        );
        // Skipping a beta does not hide the stable release that follows it.
        let feed = tags(&["v1.20.0", "v1.20.0-beta.3"]);
        assert_eq!(
            candidates(&feed, ReleaseChannel::Beta, Some("1.20.0-beta.3")),
            tags(&["1.20.0"])
        );
    }

    #[test]
    fn switching_back_to_stable_offers_no_downgrade() {
        // On 1.21.0-beta.2, the newest stable is older: nothing is offered.
        let stable = tags(&["v1.20.0"]);
        let offered = candidates(&stable, ReleaseChannel::Stable, None);
        assert!(offered
            .iter()
            .all(|version| !newer(version, "1.21.0-beta.2")));
        assert!(!newer(&offered[0], "1.21.0-beta.2"));
        // Once 1.21.0 ships, it is newer than the beta that led to it.
        assert!(newer("1.21.0", "1.21.0-beta.2"));
    }

    #[test]
    fn reads_release_tags_from_the_feed() {
        let feed = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <entry>
    <id>tag:github.com,2008:Repository/1290807019/v1.20.0-beta.2</id>
    <link rel="alternate" type="text/html" href="https://github.com/LHagfoss/rustcode/releases/tag/v1.20.0-beta.2"/>
    <title>v1.20.0-beta.2</title>
    <content type="html">&lt;p&gt;Includes &lt;a href=&quot;https://github.com/LHagfoss/rustcode/releases/tag/v1.9.0&quot;&gt;v1.9.0&lt;/a&gt;.&lt;/p&gt;</content>
  </entry>
  <entry>
    <link rel="alternate" type="text/html" href="https://github.com/LHagfoss/rustcode/releases/tag/v1.19.0"/>
  </entry>
  <entry>
    <link rel="alternate" type="text/html" href="https://github.com/LHagfoss/rustcode/releases/tag/nightly"/>
  </entry>
  <link type="text/html" rel="alternate" href="https://github.com/LHagfoss/rustcode/releases"/>
</feed>"#;
        // The tag quoted in the notes is not this release's tag, and a tag that
        // is not a version is not one either.
        assert_eq!(tags_in_feed(feed), tags(&["v1.20.0-beta.2", "v1.19.0"]));
        assert!(tags_in_feed("").is_empty());
    }

    #[test]
    fn build_metadata_takes_no_part_in_ordering() {
        assert_eq!(
            parse_version("v1.21.0+build.7").unwrap().to_string(),
            "1.21.0"
        );
        assert_eq!(
            parse_version("1.21.0-beta.2+sha.abc").unwrap().to_string(),
            "1.21.0-beta.2"
        );
        assert!(!newer("1.21.0+build.7", "1.21.0"));
        assert_eq!(
            candidates(
                &tags(&["v1.21.0+build.7", "v1.20.0"]),
                ReleaseChannel::Stable,
                None
            ),
            tags(&["1.21.0", "1.20.0"])
        );
        // A tag that is not three numbers and an optional prerelease is no version.
        assert!(parse_version("1.21").is_none());
        assert!(parse_version("nightly").is_none());
        assert!(parse_version("1.21.0-beta 2").is_none());
    }

    #[test]
    fn a_beta_is_offered_only_where_it_has_the_app_it_installs() {
        assert_eq!(channel_names().contains(&"beta"), offers_beta());
        // A Windows or Linux build stays on Stable whatever the setting says.
        assert_eq!(
            channel_of(Some("beta")),
            if offers_beta() {
                ReleaseChannel::Beta
            } else {
                ReleaseChannel::Stable
            }
        );
        assert_eq!(channel_of(Some("stable")), ReleaseChannel::Stable);
        assert_eq!(channel_of(None), ReleaseChannel::Stable);
        assert_eq!(channel_of(Some("garbage")), ReleaseChannel::Stable);
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

    #[tokio::test]
    async fn download_reports_progress_to_the_end() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let body = vec![7u8; 300 * 1024];
        let served = body.clone();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 1024];
            let _ = socket.read(&mut request).await;
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                served.len()
            );
            socket.write_all(head.as_bytes()).await.unwrap();
            for part in served.chunks(16 * 1024) {
                socket.write_all(part).await.unwrap();
            }
        });
        let mut seen = Vec::new();
        let got = download(
            &client().unwrap(),
            &format!("http://127.0.0.1:{port}/app.zip"),
            |received, total| seen.push((received, total)),
        )
        .await
        .unwrap();
        assert_eq!(got, body);
        let total = Some(body.len() as u64);
        assert_eq!(seen.first(), Some(&(0, total)));
        assert_eq!(seen.last(), Some(&(body.len() as u64, total)));
        assert!(seen.len() >= 3, "progress in steps: {seen:?}");
        assert!(seen.windows(2).all(|w| w[0].0 <= w[1].0));
    }

    #[tokio::test]
    #[ignore = "needs the network"]
    async fn the_latest_release_and_its_app_are_found() {
        let client = client().unwrap();
        let tag = latest_tag(&client).await.expect("latest tag");
        let latest = tag.trim_start_matches('v');
        let url = download_url(&tag, &asset_name(latest));
        println!(
            "tag {tag} newer than 1.0.0-beta.1: {} asset {url}",
            newer(latest, "1.0.0-beta.1")
        );
        assert!(exists(&client, &url).await, "the app zip is found");
        let feed = release_tags(&client, ReleaseChannel::Beta)
            .await
            .expect("release feed");
        let offered = candidates(&feed, ReleaseChannel::Beta, None);
        println!("beta channel offers {offered:?} from {} tags", feed.len());
        assert!(!offered.is_empty(), "the feed names releases");
    }
}
