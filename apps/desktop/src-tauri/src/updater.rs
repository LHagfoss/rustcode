//! The in-app updater, the SwiftUI app's `Updater.swift` ported: ask GitHub
//! what the update channel offers, and install it in place. Every download is
//! checked against the release's `SHA256SUMS` before a byte of it is written
//! or run; what happens next depends on how this copy was installed
//! (`Install`):
//!
//! - macOS app bundle: `RustCode-<ver>-macos-<arch>.app.zip`, unpacked with
//!   `ditto`, a valid code signature and this app's bundle identifier
//!   required, then a detached shell swaps the bundle once this process has
//!   quit and opens the new one.
//! - Linux AppImage (`$APPIMAGE`): the new AppImage is written next to the
//!   running one, made executable and renamed over it, then launched once this
//!   process has quit. No root needed.
//! - Linux `.deb` (a dpkg-owned binary under `/usr`): the package is saved in
//!   a private temp directory and installed with `pkexec apt-get install -y`
//!   (`pkexec dpkg -i` without apt-get), then the app reopens.
//! - Windows (an NSIS install, `uninstall.exe` beside the app): the setup is
//!   run silently with the flags Tauri's NSIS template reads (`/S /UPDATE
//!   /R`: no UI, keep the install, start the app again), and this process
//!   quits so the installer can replace its files.
//!
//! Any other copy (a tarball, Flatpak, a folder it cannot write) reports the
//! release and opens its page instead.
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
//! already trust.

use serde::Serialize;
use sha2::{Digest, Sha256};
use std::ffi::OsString;
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
    /// The app can replace itself (see `Install`).
    can_install: bool,
    /// What to tell the user about how this copy updates, when the default
    /// ("installs and reopens" / "opens the release page") is not the whole
    /// story: the .deb asks for a password, a read-only folder cannot update.
    install_note: Option<String>,
    /// A `tauri dev` build, which never updates itself.
    development: bool,
    /// The release page: the fallback for a copy that cannot install itself.
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

/// The release asset an install kind updates from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Package {
    /// `RustCode-<ver>-macos-<arch>.app.zip`
    MacApp,
    /// `RustCode-<ver>-linux-x86_64.AppImage`
    AppImage,
    /// `RustCode-<ver>-linux-x86_64.deb`
    Deb,
    /// `RustCode-<ver>-windows-x64-setup.exe` (NSIS)
    WindowsSetup,
}

impl Package {
    /// The asset's name in the release (.github/workflows/build.yml), or `None`
    /// where the release ships none for `arch`: the Linux and Windows apps are
    /// built for x86_64 only.
    fn asset(self, version: &str, arch: &str) -> Option<String> {
        let x86_64 = arch == "x86_64";
        match self {
            Package::MacApp => Some(format!("RustCode-{version}-macos-{arch}.app.zip")),
            Package::AppImage => {
                x86_64.then(|| format!("RustCode-{version}-linux-x86_64.AppImage"))
            }
            Package::Deb => x86_64.then(|| format!("RustCode-{version}-linux-x86_64.deb")),
            Package::WindowsSetup => {
                x86_64.then(|| format!("RustCode-{version}-windows-x64-setup.exe"))
            }
        }
    }
}

fn download_url(tag: &str, asset: &str) -> String {
    format!("{}/{asset}", download_base(tag))
}

/// Where a release's assets are served from.
fn download_base(tag: &str) -> String {
    format!("https://github.com/{REPO}/releases/download/{tag}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Os {
    Mac,
    Linux,
    Windows,
    Other,
}

impl Os {
    fn current() -> Os {
        if cfg!(target_os = "macos") {
            Os::Mac
        } else if cfg!(target_os = "linux") {
            Os::Linux
        } else if cfg!(windows) {
            Os::Windows
        } else {
            Os::Other
        }
    }
}

/// What this process can tell about how it was installed, gathered once per
/// check so `classify` is a pure function of it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Host {
    os: Os,
    arch: &'static str,
    /// The running binary.
    exe: Option<PathBuf>,
    /// The `.app` it runs from (macOS).
    bundle: Option<PathBuf>,
    /// `$APPIMAGE`: the AppImage file it was started from (Linux).
    appimage: Option<PathBuf>,
    /// Inside Flatpak or Snap, where the package manager owns the files.
    sandboxed: bool,
    /// The binary belongs to an installed dpkg package (Linux).
    dpkg_owned: bool,
    /// `uninstall.exe` sits beside the binary: an NSIS install (Windows).
    has_uninstaller: bool,
    /// The folder an update would replace files in is writable (macOS bundle
    /// or AppImage).
    folder_writable: bool,
}

/// How this copy of the app was installed, and so how it updates.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Install {
    /// A macOS `.app` in a folder it may write.
    MacApp(PathBuf),
    /// The AppImage file at this path, in a folder it may write.
    AppImage(PathBuf),
    /// A dpkg-owned install; the path is the binary to start again.
    Deb(PathBuf),
    /// An NSIS install.
    Windows,
    /// A bundle or AppImage whose folder cannot be written: no in-place update.
    ReadOnly(PathBuf),
    /// Anything else (a tarball, Flatpak, a dev build): the release page.
    Manual,
}

impl Install {
    fn package(&self) -> Option<Package> {
        match self {
            Install::MacApp(_) => Some(Package::MacApp),
            Install::AppImage(_) => Some(Package::AppImage),
            Install::Deb(_) => Some(Package::Deb),
            Install::Windows => Some(Package::WindowsSetup),
            Install::ReadOnly(_) | Install::Manual => None,
        }
    }

    fn installable(&self) -> bool {
        self.package().is_some()
    }

    /// A line for the update row and Settings when the default does not fit.
    fn note(&self) -> Option<String> {
        match self {
            Install::Deb(_) => Some("Asks for your password to install the package".into()),
            Install::ReadOnly(dir) => Some(format!(
                "RustCode cannot write to {}, so the update opens the release page",
                dir.display()
            )),
            _ => None,
        }
    }
}

/// How `host` was installed. Anything this cannot recognise is `Manual`: the
/// updater only replaces files it knows how to replace.
fn classify(host: &Host) -> Install {
    let in_place = |path: &Path| {
        let folder = path.parent().unwrap_or(path).to_path_buf();
        if host.folder_writable {
            None
        } else {
            Some(Install::ReadOnly(folder))
        }
    };
    match host.os {
        Os::Mac => match &host.bundle {
            Some(bundle) => in_place(bundle).unwrap_or_else(|| Install::MacApp(bundle.clone())),
            None => Install::Manual,
        },
        Os::Linux => {
            // The release has Linux apps for x86_64 only.
            if host.arch != "x86_64" {
                return Install::Manual;
            }
            if let Some(image) = &host.appimage {
                return in_place(image).unwrap_or_else(|| Install::AppImage(image.clone()));
            }
            match &host.exe {
                Some(exe) if !host.sandboxed && host.dpkg_owned && exe.starts_with("/usr/") => {
                    Install::Deb(exe.clone())
                }
                _ => Install::Manual,
            }
        }
        Os::Windows if host.arch == "x86_64" && host.has_uninstaller => Install::Windows,
        Os::Windows => Install::Manual,
        Os::Other => Install::Manual,
    }
}

impl Host {
    fn probe() -> Host {
        let os = Os::current();
        let exe = std::env::current_exe()
            .ok()
            .map(|exe| exe.canonicalize().unwrap_or(exe));
        let bundle = bundle_path();
        let appimage = std::env::var_os("APPIMAGE")
            .map(PathBuf::from)
            .filter(|path| os == Os::Linux && path.is_absolute() && path.is_file());
        let sandboxed = std::env::var_os("FLATPAK_ID").is_some()
            || std::env::var_os("SNAP").is_some()
            || Path::new("/.flatpak-info").exists();
        let dpkg_owned = os == Os::Linux
            && appimage.is_none()
            && exe
                .as_deref()
                .is_some_and(|exe| exe.starts_with("/usr/") && dpkg_owns(exe));
        let has_uninstaller = os == Os::Windows
            && exe
                .as_deref()
                .and_then(Path::parent)
                .is_some_and(|dir| dir.join("uninstall.exe").is_file());
        let folder_writable = if os == Os::Mac {
            bundle.as_deref()
        } else {
            appimage.as_deref()
        }
        .and_then(Path::parent)
        .is_some_and(writable);
        Host {
            os,
            arch: arch(),
            exe,
            bundle,
            appimage,
            sandboxed,
            dpkg_owned,
            has_uninstaller,
            folder_writable,
        }
    }
}

/// Whether dpkg lists `exe` as a file of an installed package. The answer
/// does not change while the app runs, so it is asked once.
fn dpkg_owns(exe: &Path) -> bool {
    static OWNED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OWNED.get_or_init(|| {
        crate::process::command("dpkg-query")
            .arg("-S")
            .arg("--")
            .arg(exe)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    })
}

/// How this copy installs updates.
fn current_install() -> Install {
    // A `tauri dev` build never updates itself.
    if cfg!(debug_assertions) {
        return Install::Manual;
    }
    classify(&Host::probe())
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

/// What the channel offers this app, if anything: the newest version that is
/// newer than this one (any, for a dev build) and, where the app can install
/// itself, uploaded with its `SHA256SUMS`. A release whose package is not there
/// yet is passed over for the one before it.
async fn select(
    client: &reqwest::Client,
    current: &str,
    channel: ReleaseChannel,
    skipped: Option<&str>,
    development: bool,
    package: Option<Package>,
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
        if let Some(asset) = package.and_then(|p| p.asset(&version, arch())) {
            if !ready(client, &format!("v{version}"), &asset).await {
                continue;
            }
        }
        return Ok(Some(version));
    }
    Ok(None)
}

async fn check(
    app: &AppHandle,
    channel: ReleaseChannel,
    skipped: Option<&str>,
    install: &Install,
) -> Result<UpdateInfo, String> {
    let current = app.package_info().version.to_string();
    let development = cfg!(debug_assertions);
    let client = client()?;
    let latest = select(
        &client,
        &current,
        channel,
        skipped,
        development,
        install.package(),
    )
    .await?;
    let page = match &latest {
        Some(version) => format!("https://github.com/{REPO}/releases/tag/v{version}"),
        None => format!("https://github.com/{REPO}/releases"),
    };
    Ok(UpdateInfo {
        current,
        available: latest.is_some(),
        latest,
        can_install: install.installable(),
        install_note: install.note(),
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
    check(
        &app,
        channel_of(channel.as_deref()),
        skipped.as_deref(),
        &current_install(),
    )
    .await
}

/// Where an install is, for the sidebar's progress bar.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase", tag = "phase")]
pub enum InstallProgress {
    /// Bytes of the download so far, and its size when the server said.
    Downloading { received: u64, total: Option<u64> },
    /// Checking the download against the release's checksums.
    Verifying,
    /// Swapping the app in; the app quits next.
    Installing,
}

/// Downloads, verifies and installs the update, then quits; the app reopens
/// updated. An error leaves the running app as it was.
#[tauri::command]
pub async fn update_install(
    app: AppHandle,
    channel: Option<String>,
    skipped: Option<String>,
    on_progress: Channel<InstallProgress>,
) -> Result<(), String> {
    let install = current_install();
    let info = check(
        &app,
        channel_of(channel.as_deref()),
        skipped.as_deref(),
        &install,
    )
    .await?;
    let Some(package) = install.package().filter(|_| info.available) else {
        return Err(install
            .note()
            .unwrap_or_else(|| "No update this app can install itself.".into()));
    };
    let version = info.latest.unwrap_or_default();
    let asset = package
        .asset(&version, arch())
        .ok_or("This release has no app for this platform.")?;
    let client = client()?;
    let bytes = fetch_verified(
        &client,
        &download_base(&format!("v{version}")),
        &asset,
        |progress| {
            let _ = on_progress.send(progress);
        },
    )
    .await?;
    // Verified: nothing from here on installs a file that did not match.
    let _ = on_progress.send(InstallProgress::Installing);
    let quit = app.clone();
    tauri::async_runtime::spawn_blocking(move || install_package(&quit, &install, &asset, &bytes))
        .await
        .map_err(|e| e.to_string())?
}

/// The release asset, checked against the release's `SHA256SUMS`, as bytes
/// that are not written anywhere until they match. `base` is the folder the
/// assets are served from.
async fn fetch_verified(
    client: &reqwest::Client,
    base: &str,
    asset: &str,
    mut progress: impl FnMut(InstallProgress),
) -> Result<Vec<u8>, String> {
    let body = download(client, &format!("{base}/{asset}"), |received, total| {
        progress(InstallProgress::Downloading { received, total })
    })
    .await?;
    progress(InstallProgress::Verifying);
    let sums = fetch(client, &format!("{base}/SHA256SUMS")).await?;
    let expected = checksum(&String::from_utf8_lossy(&sums), asset)
        .ok_or("The release checksums do not list the app.")?;
    verify_digest(&body, &expected)?;
    Ok(body)
}

/// Whether `bytes` hash to `expected`, a lowercase hex SHA-256.
fn verify_digest(bytes: &[u8], expected: &str) -> Result<(), String> {
    let digest: String = Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if digest == expected {
        Ok(())
    } else {
        Err("The download does not match the release checksum.".into())
    }
}

/// Installs the verified `bytes` of `asset` the way `install` updates, then
/// quits the app (every route reopens it).
fn install_package(
    app: &AppHandle,
    install: &Install,
    asset: &str,
    bytes: &[u8],
) -> Result<(), String> {
    let work = private_dir()?;
    let result = match install {
        Install::MacApp(current) => {
            let zip = work.join(asset);
            let staged = std::fs::write(&zip, bytes)
                .map_err(|e| e.to_string())
                .and_then(|()| stage(app, &zip, &work));
            // The script removes the work folder once it has swapped the bundle.
            return match staged {
                Ok(staged) => relaunch(app, current, &staged),
                Err(e) => {
                    let _ = std::fs::remove_dir_all(&work);
                    Err(e)
                }
            };
        }
        Install::AppImage(target) => replace_appimage(target, bytes)
            .and_then(|()| relaunch_after_exit(target, &APPIMAGE_ENV)),
        Install::Deb(exe) => std::fs::write(work.join(asset), bytes)
            .map_err(|e| e.to_string())
            .and_then(|()| install_deb(&work.join(asset)))
            .and_then(|()| relaunch_after_exit(exe, &[])),
        Install::Windows => {
            let setup = work.join(asset);
            // The installer runs from the work folder, so it stays there (the
            // OS clears its temp directory) unless it could not start.
            let started = std::fs::write(&setup, bytes)
                .map_err(|e| e.to_string())
                .and_then(|()| run_installer(&setup));
            if started.is_err() {
                let _ = std::fs::remove_dir_all(&work);
            }
            return started.map(|()| app.exit(0));
        }
        Install::ReadOnly(_) | Install::Manual => Err("This copy cannot update itself.".into()),
    };
    let _ = std::fs::remove_dir_all(&work);
    result.map(|()| app.exit(0))
}

/// A new folder only this user can enter, for a download that is about to be
/// installed (with root, for a .deb): nobody can put a file in it between the
/// checksum and the install.
fn private_dir() -> Result<PathBuf, String> {
    let base = std::env::temp_dir();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    for attempt in 0..8 {
        let dir = base.join(format!(
            "rustcode-update-{}-{stamp}-{attempt}",
            std::process::id()
        ));
        #[cfg_attr(not(unix), allow(unused_mut))]
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        match builder.create(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("Could not make a folder for the update: {e}")),
        }
    }
    Err("Could not make a folder for the update.".into())
}

/// Writes the new AppImage next to the running one and renames it over that
/// file: the swap is atomic, and the running copy keeps its old file open
/// until it quits.
fn replace_appimage(target: &Path, bytes: &[u8]) -> Result<(), String> {
    let dir = target.parent().ok_or("The AppImage has no folder.")?;
    let name = target.file_name().unwrap_or_default().to_string_lossy();
    let staged = dir.join(format!(".{name}.update-{}", std::process::id()));
    let swap = || -> std::io::Result<()> {
        std::fs::write(&staged, bytes)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))?;
        }
        std::fs::rename(&staged, target)
    };
    swap().map_err(|e| {
        let _ = std::fs::remove_file(&staged);
        format!(
            "RustCode could not replace {} ({e}). Download the new AppImage from the release page.",
            target.display()
        )
    })
}

/// Variables the AppImage runtime and its launcher set for the running copy;
/// the new one sets its own.
const APPIMAGE_ENV: [&str; 4] = ["APPDIR", "APPIMAGE", "ARGV0", "OWD"];

/// The arguments of the shell that waits for `pid` to exit, then starts `exe`.
const WAIT_THEN_START: &str = r#"while kill -0 "$1" 2>/dev/null; do sleep 0.2; done
exec "$2""#;

fn relaunch_args(pid: u32, exe: &Path) -> Vec<OsString> {
    vec![
        "-c".into(),
        WAIT_THEN_START.into(),
        "sh".into(),
        pid.to_string().into(),
        exe.as_os_str().to_owned(),
    ]
}

/// Starts `exe` once this process has exited, detached from it. `unset` names
/// environment variables the new process must not inherit.
fn relaunch_after_exit(exe: &Path, unset: &[&str]) -> Result<(), String> {
    let mut command = crate::process::command("/bin/sh");
    command
        .args(relaunch_args(std::process::id(), exe))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    for name in unset {
        command.env_remove(name);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command
        .spawn()
        .map(drop)
        .map_err(|e| format!("The update installed, but RustCode could not reopen: {e}"))
}

/// The first of `names` that is an executable file in `path`'s folders, then in
/// the system's own (an app started from a launcher can have a short PATH).
fn find_tool(name: &str, path: Option<OsString>) -> Option<PathBuf> {
    let system = ["/usr/bin", "/usr/sbin", "/bin", "/sbin"].map(PathBuf::from);
    path.iter()
        .flat_map(std::env::split_paths)
        .chain(system)
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable(candidate))
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

/// What `pkexec` runs to install `deb`: `apt-get install -y <deb>` (which
/// resolves dependencies), else `dpkg -i <deb>`. `None` with neither.
fn deb_install_args(
    apt_get: Option<&Path>,
    dpkg: Option<&Path>,
    deb: &Path,
) -> Option<Vec<OsString>> {
    if let Some(apt_get) = apt_get {
        return Some(vec![
            apt_get.as_os_str().to_owned(),
            "install".into(),
            "-y".into(),
            deb.as_os_str().to_owned(),
        ]);
    }
    dpkg.map(|dpkg| {
        vec![
            dpkg.as_os_str().to_owned(),
            "-i".into(),
            deb.as_os_str().to_owned(),
        ]
    })
}

/// What a failed `pkexec` run means: 126 is the dialog dismissed (or no
/// permission), 127 no authentication was possible; anything else is the
/// package tool's own failure.
fn pkexec_failure(code: Option<i32>, stderr: &str) -> String {
    match code {
        Some(126) => "The update was cancelled: RustCode was not allowed to install the package. Try again, or download the .deb from the release page.".into(),
        Some(127) => "The update could not ask for permission: no polkit authentication agent answered. Download the .deb from the release page and install it yourself.".into(),
        _ => {
            let detail = stderr
                .lines()
                .rev()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .map(|line| format!(": {}", line.chars().take(200).collect::<String>()))
                .unwrap_or_default();
            match code {
                Some(code) => format!("Installing the package failed (status {code}){detail}"),
                None => format!("Installing the package was interrupted{detail}"),
            }
        }
    }
}

/// Installs the .deb as root through polkit (`pkexec`), which asks the user.
fn install_deb(deb: &Path) -> Result<(), String> {
    let path = std::env::var_os("PATH");
    let pkexec = find_tool("pkexec", path.clone()).ok_or(
        "The update needs pkexec (polkit) to install the package, and this system has none. Download the .deb from the release page.",
    )?;
    let args = deb_install_args(
        find_tool("apt-get", path.clone()).as_deref(),
        find_tool("dpkg", path).as_deref(),
        deb,
    )
    .ok_or("Neither apt-get nor dpkg was found. Download the .deb from the release page.")?;
    let output = crate::process::command(pkexec)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("Could not run pkexec: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(pkexec_failure(
            output.status.code(),
            &String::from_utf8_lossy(&output.stderr),
        ))
    }
}

/// The flags Tauri's NSIS template reads (crates/tauri-bundler/src/bundle/
/// windows/nsis/installer.nsi): `/S` silent, `/UPDATE` keep the install and
/// skip the pages, `/R` start the app again once it is done (silent and
/// passive installs only).
const NSIS_ARGS: [&str; 3] = ["/S", "/UPDATE", "/R"];

/// Runs the NSIS setup silently and leaves it running: the caller quits so it
/// can replace the app's files, and it reopens the app when it is done. The
/// path is passed as one argument, never through a shell.
fn run_installer(setup: &Path) -> Result<(), String> {
    crate::process::command(setup)
        .args(NSIS_ARGS)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(drop)
        .map_err(|e| format!("Could not start the installer: {e}"))
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
    let output = crate::process::command(tool)
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

/// Unpacks the verified zip; returns the new RustCode.app.
#[cfg(target_os = "macos")]
fn stage(app: &AppHandle, zip: &Path, work: &Path) -> Result<PathBuf, String> {
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
fn stage(_: &AppHandle, _: &Path, _: &Path) -> Result<PathBuf, String> {
    Err("Only a macOS app bundle updates this way.".into())
}

/// A detached shell waits for this process to exit, swaps the bundle and
/// opens the new one; the app then quits.
fn relaunch(app: &AppHandle, current: &Path, new_app: &Path) -> Result<(), String> {
    let script = r#"while kill -0 "$1" 2>/dev/null; do sleep 0.2; done
rm -rf "$2.old" && mv "$2" "$2.old" && mv "$3" "$2" && rm -rf "$2.old"
xattr -dr com.apple.quarantine "$2" 2>/dev/null
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$2" 2>/dev/null
open "$2"
rm -rf "$4""#;
    let work = new_app.parent().and_then(Path::parent).unwrap_or(new_app);
    crate::process::command("/bin/sh")
        .arg("-c")
        .arg(script)
        .arg("sh")
        .arg(std::process::id().to_string())
        .arg(current)
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

    #[test]
    fn picks_the_release_asset_per_platform_and_arch() {
        let asset = |package: Package, arch| package.asset("1.22.0", arch);
        assert_eq!(
            asset(Package::MacApp, "aarch64").as_deref(),
            Some("RustCode-1.22.0-macos-aarch64.app.zip")
        );
        assert_eq!(
            asset(Package::MacApp, "x86_64").as_deref(),
            Some("RustCode-1.22.0-macos-x86_64.app.zip")
        );
        assert_eq!(
            asset(Package::AppImage, "x86_64").as_deref(),
            Some("RustCode-1.22.0-linux-x86_64.AppImage")
        );
        assert_eq!(
            asset(Package::Deb, "x86_64").as_deref(),
            Some("RustCode-1.22.0-linux-x86_64.deb")
        );
        assert_eq!(
            asset(Package::WindowsSetup, "x86_64").as_deref(),
            Some("RustCode-1.22.0-windows-x64-setup.exe")
        );
        // The release ships no Linux or Windows app for arm.
        for package in [Package::AppImage, Package::Deb, Package::WindowsSetup] {
            assert_eq!(asset(package, "aarch64"), None);
        }
        assert_eq!(
            download_url("v1.22.0", "SHA256SUMS"),
            "https://github.com/LHagfoss/rustcode/releases/download/v1.22.0/SHA256SUMS"
        );
    }

    fn host(os: Os) -> Host {
        Host {
            os,
            arch: "x86_64",
            exe: None,
            bundle: None,
            appimage: None,
            sandboxed: false,
            dpkg_owned: false,
            has_uninstaller: false,
            folder_writable: true,
        }
    }

    #[test]
    fn a_linux_appimage_installs_in_place_where_its_folder_is_writable() {
        let image = PathBuf::from("/home/me/Applications/RustCode.AppImage");
        let mut linux = Host {
            appimage: Some(image.clone()),
            exe: Some("/tmp/.mount_RustCoXYZ/usr/bin/rustcode-desktop".into()),
            ..host(Os::Linux)
        };
        assert_eq!(classify(&linux), Install::AppImage(image.clone()));
        assert_eq!(classify(&linux).package(), Some(Package::AppImage));
        // `$APPIMAGE` wins over a dpkg-owned binary of the same name.
        linux.dpkg_owned = true;
        assert_eq!(classify(&linux), Install::AppImage(image.clone()));
        // A read-only folder keeps the release page, and says why.
        linux.folder_writable = false;
        let read_only = classify(&linux);
        assert_eq!(read_only, Install::ReadOnly("/home/me/Applications".into()));
        assert!(!read_only.installable());
        assert!(read_only.note().unwrap().contains("/home/me/Applications"));
    }

    #[test]
    fn a_linux_deb_is_a_dpkg_owned_binary_under_usr() {
        let exe = PathBuf::from("/usr/bin/rustcode-desktop");
        let deb = Host {
            exe: Some(exe.clone()),
            dpkg_owned: true,
            ..host(Os::Linux)
        };
        assert_eq!(classify(&deb), Install::Deb(exe.clone()));
        assert_eq!(classify(&deb).package(), Some(Package::Deb));
        assert!(classify(&deb).note().unwrap().contains("password"));
        // Not owned by a package (AUR, a manual copy), not under /usr, or in a
        // sandbox: the release page.
        let owned = |change: fn(&mut Host)| {
            let mut other = deb.clone();
            change(&mut other);
            classify(&other)
        };
        assert_eq!(owned(|h| h.dpkg_owned = false), Install::Manual);
        assert_eq!(
            owned(|h| h.exe = Some("/opt/rustcode/rustcode-desktop".into())),
            Install::Manual
        );
        assert_eq!(
            owned(|h| h.exe = Some("/home/me/rustcode/rustcode-desktop".into())),
            Install::Manual
        );
        assert_eq!(owned(|h| h.sandboxed = true), Install::Manual);
        assert_eq!(owned(|h| h.exe = None), Install::Manual);
        // No Linux app ships for arm.
        assert_eq!(owned(|h| h.arch = "aarch64"), Install::Manual);
    }

    #[test]
    fn windows_installs_from_the_nsis_setup_only() {
        let nsis = Host {
            exe: Some(r"C:\Users\me\AppData\Local\RustCode\rustcode-desktop.exe".into()),
            has_uninstaller: true,
            ..host(Os::Windows)
        };
        assert_eq!(classify(&nsis), Install::Windows);
        assert_eq!(classify(&nsis).package(), Some(Package::WindowsSetup));
        // A copy with no uninstaller beside it is not an NSIS install.
        let portable = Host {
            has_uninstaller: false,
            ..nsis.clone()
        };
        assert_eq!(classify(&portable), Install::Manual);
        let arm = Host {
            arch: "aarch64",
            ..nsis
        };
        assert_eq!(classify(&arm), Install::Manual);
    }

    #[test]
    fn a_macos_bundle_installs_in_place_where_its_folder_is_writable() {
        let bundle = PathBuf::from("/Applications/RustCode.app");
        let mac = Host {
            bundle: Some(bundle.clone()),
            ..host(Os::Mac)
        };
        assert_eq!(classify(&mac), Install::MacApp(bundle));
        let locked = Host {
            folder_writable: false,
            ..mac.clone()
        };
        assert_eq!(classify(&locked), Install::ReadOnly("/Applications".into()));
        let bare = Host {
            bundle: None,
            ..mac
        };
        assert_eq!(classify(&bare), Install::Manual);
        assert_eq!(classify(&host(Os::Other)), Install::Manual);
    }

    #[test]
    fn a_wrong_checksum_is_refused() {
        let body = b"the new app";
        let good: String = Sha256::digest(body)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        assert_eq!(verify_digest(body, &good), Ok(()));
        assert_eq!(
            verify_digest(b"another file", &good),
            Err("The download does not match the release checksum.".to_string())
        );
        assert!(verify_digest(body, &"0".repeat(64)).is_err());
        assert!(verify_digest(body, "").is_err());
    }

    /// Serves `routes` (path, body) over HTTP on a free local port until the
    /// test ends; any other path is a 404.
    async fn serve(routes: Vec<(&'static str, Vec<u8>)>) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let routes = routes.clone();
                tokio::spawn(async move {
                    let mut request = [0u8; 2048];
                    let read = socket.read(&mut request).await.unwrap_or(0);
                    let request = String::from_utf8_lossy(&request[..read]).into_owned();
                    let path = request.split_whitespace().nth(1).unwrap_or("/");
                    let (status, body) = match routes.iter().find(|(route, _)| *route == path) {
                        Some((_, body)) => ("200 OK", body.clone()),
                        None => ("404 Not Found", Vec::new()),
                    };
                    let head = format!(
                        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = socket.write_all(head.as_bytes()).await;
                    let _ = socket.write_all(&body).await;
                });
            }
        });
        base
    }

    fn sums_for(body: &[u8], asset: &str) -> Vec<u8> {
        let hash: String = Sha256::digest(body)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        format!("{hash}  {asset}\n").into_bytes()
    }

    #[tokio::test]
    async fn a_download_that_matches_the_release_sums_is_returned() {
        let asset = "RustCode-1.22.0-linux-x86_64.AppImage";
        let body = vec![9u8; 100 * 1024];
        let base = serve(vec![
            ("/RustCode-1.22.0-linux-x86_64.AppImage", body.clone()),
            ("/SHA256SUMS", sums_for(&body, asset)),
        ])
        .await;
        let mut phases = Vec::new();
        let got = fetch_verified(&client().unwrap(), &base, asset, |p| phases.push(p))
            .await
            .unwrap();
        assert_eq!(got, body);
        assert!(matches!(
            phases.first(),
            Some(InstallProgress::Downloading { received: 0, .. })
        ));
        assert!(matches!(phases.last(), Some(InstallProgress::Verifying)));
    }

    #[tokio::test]
    async fn a_download_that_does_not_match_the_release_sums_is_refused() {
        let asset = "RustCode-1.22.0-windows-x64-setup.exe";
        let served = vec![1u8; 4096];
        // The sums name a different file's hash: a tampered or swapped download.
        let base = serve(vec![
            ("/RustCode-1.22.0-windows-x64-setup.exe", served),
            ("/SHA256SUMS", sums_for(b"the real setup", asset)),
        ])
        .await;
        let err = fetch_verified(&client().unwrap(), &base, asset, |_| {})
            .await
            .unwrap_err();
        assert_eq!(err, "The download does not match the release checksum.");
        // Sums that do not list the asset refuse as well.
        let base = serve(vec![
            ("/RustCode-1.22.0-windows-x64-setup.exe", vec![1u8; 16]),
            ("/SHA256SUMS", sums_for(b"x", "other.exe")),
        ])
        .await;
        let err = fetch_verified(&client().unwrap(), &base, asset, |_| {})
            .await
            .unwrap_err();
        assert_eq!(err, "The release checksums do not list the app.");
        // And so do missing sums.
        let base = serve(vec![(
            "/RustCode-1.22.0-windows-x64-setup.exe",
            vec![1u8; 16],
        )])
        .await;
        assert!(fetch_verified(&client().unwrap(), &base, asset, |_| {})
            .await
            .is_err());
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "rustcode-updater-test-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_appimage_is_replaced_atomically_and_stays_executable() {
        let dir = scratch("appimage");
        let target = dir.join("RustCode.AppImage");
        std::fs::write(&target, b"old").unwrap();
        replace_appimage(&target, b"new build").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new build");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&target).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o755);
        }
        // Nothing but the app is left beside it.
        let names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("RustCode.AppImage")]);
        // A folder that cannot take the file leaves the old app alone and says so.
        let missing = dir.join("gone").join("RustCode.AppImage");
        let err = replace_appimage(&missing, b"x").unwrap_err();
        assert!(err.contains("release page"), "{err}");
        assert_eq!(std::fs::read(&target).unwrap(), b"new build");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn the_download_folder_is_private_and_new() {
        use std::os::unix::fs::PermissionsExt;
        let one = private_dir().unwrap();
        let two = private_dir().unwrap();
        assert_ne!(one, two);
        assert_eq!(
            std::fs::metadata(&one).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let _ = std::fs::remove_dir_all(one);
        let _ = std::fs::remove_dir_all(two);
    }

    #[test]
    fn builds_the_linux_relaunch_and_package_commands() {
        let exe = Path::new("/home/me/Applications/Rust Code $(x).AppImage");
        let args = relaunch_args(4242, exe);
        // The path is one argument, never part of the script text.
        assert_eq!(args[0], "-c");
        assert_eq!(args[1], WAIT_THEN_START);
        assert!(!args[1].to_string_lossy().contains("Rust Code"));
        assert_eq!(args[3], "4242");
        assert_eq!(args[4], exe.as_os_str());

        let deb = Path::new("/tmp/rustcode-update-1/RustCode-1.22.0-linux-x86_64.deb");
        let apt = Path::new("/usr/bin/apt-get");
        let dpkg = Path::new("/usr/bin/dpkg");
        assert_eq!(
            deb_install_args(Some(apt), Some(dpkg), deb),
            Some(vec![
                "/usr/bin/apt-get".into(),
                "install".into(),
                "-y".into(),
                deb.as_os_str().to_owned(),
            ])
        );
        assert_eq!(
            deb_install_args(None, Some(dpkg), deb),
            Some(vec![
                "/usr/bin/dpkg".into(),
                "-i".into(),
                deb.as_os_str().to_owned()
            ])
        );
        assert_eq!(deb_install_args(None, None, deb), None);
    }

    #[test]
    fn says_what_a_failed_pkexec_means() {
        assert!(pkexec_failure(Some(126), "").contains("cancelled"));
        assert!(pkexec_failure(Some(127), "").contains("polkit"));
        let failed = pkexec_failure(
            Some(100),
            "Reading package lists\nE: Unable to locate package x\n\n",
        );
        assert_eq!(
            failed,
            "Installing the package failed (status 100): E: Unable to locate package x"
        );
        assert_eq!(
            pkexec_failure(None, ""),
            "Installing the package was interrupted"
        );
    }

    #[test]
    fn the_windows_setup_runs_silently_and_reopens_the_app() {
        // The flags Tauri's NSIS template reads: no UI, keep the install, relaunch.
        assert_eq!(NSIS_ARGS, ["/S", "/UPDATE", "/R"]);
    }

    #[cfg(unix)]
    #[test]
    fn finds_a_tool_on_the_path_before_the_system_folders() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("tools");
        let tool = dir.join("pkexec-test-tool");
        std::fs::write(&tool, b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        let plain = dir.join("not-executable");
        std::fs::write(&plain, b"").unwrap();
        let path = std::env::join_paths([&dir]).unwrap();
        assert_eq!(
            find_tool("pkexec-test-tool", Some(path.clone())),
            Some(tool)
        );
        assert_eq!(find_tool("not-executable", Some(path.clone())), None);
        assert_eq!(find_tool("no-such-tool-anywhere", Some(path)), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    #[ignore = "needs the network"]
    async fn the_latest_release_and_its_app_are_found() {
        let client = client().unwrap();
        let tag = latest_tag(&client).await.expect("latest tag");
        let latest = tag.trim_start_matches('v');
        let asset = Package::MacApp.asset(latest, arch()).unwrap();
        let url = download_url(&tag, &asset);
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
