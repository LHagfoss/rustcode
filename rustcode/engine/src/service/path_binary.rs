//! The `rustcode` a shell runs, and whether it is the engine RustCode runs
//! (issue #3411).
//!
//! A machine can hold several copies: the app's bundled engine, Homebrew's,
//! the install script's under the home directory, `cargo install`'s. The one
//! first on `PATH` is what a shell runs, so it is also what
//! `rustcode server install` and version checks between machines use. When it
//! is an older copy, those checks compare the wrong binary and the service
//! runs something else than the app.
//!
//! This says which copy a shell finds, which version it is, and the exact
//! command that fixes it. Nothing here changes the machine.
//!
//! "What a shell finds" is not this process's `PATH`: a desktop app launched
//! from the Dock inherits launchd's short path, with neither `/opt/homebrew/bin`
//! nor `~/.rustcode/bin` in it, so the app's own engine would report the copy a
//! `launchctl` job runs rather than the one the user's terminal runs. On macOS
//! and Linux the login shell's `PATH` is asked for first (`shell_env`), as
//! harness detection does for the same reason; Windows keeps its own `PATH`.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long `rustcode --version` may take before the copy counts as unknown.
const VERSION_TIMEOUT: Duration = Duration::from_secs(5);

/// The `rustcode` a shell finds first on `PATH`, with the version it reports.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PathRustcode {
    pub path: PathBuf,
    /// `None` when it did not answer `--version` (it may not be runnable).
    pub version: Option<String>,
}

/// The extensions to try, as the OS names them.
fn extensions() -> Vec<String> {
    let mut exts = vec![String::new()];
    if cfg!(windows) {
        let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
        exts.extend(
            pathext
                .split(';')
                .filter(|ext| !ext.is_empty())
                .map(str::to_ascii_lowercase),
        );
    }
    exts
}

/// The `PATH` a shell resolves against: the login shell's directories first,
/// then this process's, so a Dock-launched app reports what the user's
/// terminal runs, then the well-known install directories (Homebrew's
/// prefixes, `path_helper`'s files, `~/.local/bin` and friends) the shell
/// probe may have failed to report. Windows has no login shell to ask and
/// keeps its own.
fn search_path() -> std::ffi::OsString {
    let login = login_shell_path();
    let process = std::env::var_os("PATH");
    let chosen = chosen_path(login, process, cfg!(windows));
    if cfg!(windows) {
        return chosen;
    }
    let mut directories: Vec<PathBuf> = std::env::split_paths(&chosen).collect();
    crate::shell_env::add_well_known_path_dirs(&mut directories);
    std::env::join_paths(directories).unwrap_or_default()
}

/// [`search_path`] with both halves named, so the order is testable.
fn chosen_path(
    login: Option<std::ffi::OsString>,
    process: Option<std::ffi::OsString>,
    windows: bool,
) -> std::ffi::OsString {
    let process = process.unwrap_or_default();
    if windows {
        return process;
    }
    let mut directories: Vec<PathBuf> = Vec::new();
    for value in [login.as_deref(), Some(process.as_os_str())]
        .into_iter()
        .flatten()
    {
        if value.is_empty() {
            continue;
        }
        for directory in std::env::split_paths(value) {
            if !directories.contains(&directory) {
                directories.push(directory);
            }
        }
    }
    std::env::join_paths(directories).unwrap_or_default()
}

/// The login shell's `PATH`. A test never starts a login shell: it would be
/// slow, and it would read the developer's dotfiles.
fn login_shell_path() -> Option<std::ffi::OsString> {
    #[cfg(test)]
    {
        None
    }
    #[cfg(not(test))]
    {
        crate::shell_env::login_shell_var("PATH").map(std::ffi::OsString::from)
    }
}

/// Whether a shell could run `path`: `PATH` lookup never returns a directory
/// or a file without an execute bit, so neither may shadow the real copy.
fn is_program(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// `rustcode` on `path`, trying each of `exts`. The name is fixed, so no
/// shell syntax or folders can come in through it.
fn find_on(path: &std::ffi::OsStr, exts: &[String]) -> Option<PathBuf> {
    for directory in std::env::split_paths(path) {
        for ext in exts {
            let candidate = directory.join(format!("rustcode{ext}"));
            if is_program(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

/// `rustcode 1.28.0` → `1.28.0`; anything else is no version.
fn version_from(output: &str) -> Option<String> {
    let line = output.lines().next()?.trim();
    let version = line.strip_prefix("rustcode").map(str::trim).unwrap_or(line);
    (!version.is_empty() && version.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .then(|| version.to_owned())
}

/// The `rustcode` a shell runs, or `None` when no `PATH` has one.
pub async fn observe() -> Option<PathRustcode> {
    // Asking a login shell is blocking (it spawns one and caches its
    // environment), and this runs from the status of a live server.
    let path = match tokio::runtime::Handle::try_current() {
        Ok(runtime) => runtime.spawn_blocking(search_path).await.ok()?,
        Err(_) => search_path(),
    };
    let binary = find_on(&path, &extensions())?;
    let version = ask_version(&binary).await;
    Some(PathRustcode {
        path: binary,
        version,
    })
}

async fn ask_version(binary: &Path) -> Option<String> {
    let mut command = crate::platform::tokio_command(binary);
    command
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        // The timeout ends the wait, not the child: a binary that hangs must
        // not outlive it.
        .kill_on_drop(true);
    let output = tokio::time::timeout(VERSION_TIMEOUT, command.output())
        .await
        .ok()?
        .ok()?;
    version_from(&String::from_utf8_lossy(&output.stdout))
}

/// Why the copy on `PATH` is the wrong one, in the words `server status` and
/// the desktop app show: which copy it is, which engine this is, and the
/// command that fixes it. `None` when both are the same version.
///
/// `this` is the engine the user is talking to — the app's, or the running
/// server's binary — and `this_version` its version.
pub fn mismatch(found: &PathRustcode, this: &Path, this_version: &str) -> Option<String> {
    let found_version = found.version.as_deref()?;
    if found_version == this_version {
        return None;
    }
    let home = home_directory();
    let advice = advice(
        found,
        found_version,
        this,
        this_version,
        cfg!(windows),
        home.as_deref(),
    );
    Some(advice)
}

fn home_directory() -> Option<PathBuf> {
    crate::home::home_dir()
}

/// Which copy is newer, when both versions can be read as numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    /// The copy on `PATH` is older: update it, or drop it.
    Older,
    /// The copy on `PATH` is newer: this engine is the one to update.
    Newer,
    /// Versions that do not parse (`dev`, not a `X.Y.Z`): no direction.
    Unknown,
}

/// Where Homebrew keeps what it installed: Apple Silicon under
/// `/opt/homebrew`, Intel Macs under `/usr/local/Cellar` with their
/// executables symlinked into `/usr/local/bin`.
const HOMEBREW_PREFIXES: &[&str] = &["/opt/homebrew/", "/usr/local/Cellar/"];

/// Whether `path` is one of the given prefixes, directly or through the
/// symlink Homebrew installs into `/usr/local/bin` (Intel) or
/// `/opt/homebrew/bin` (Apple Silicon).
fn under_any(path: &Path, prefixes: &[&str]) -> bool {
    let matches = |candidate: &Path| {
        let text = candidate.to_string_lossy();
        prefixes.iter().any(|prefix| text.starts_with(prefix))
    };
    matches(path) || path.canonicalize().is_ok_and(|real| matches(&real))
}

/// Whether `path` is a copy Homebrew installed, so `brew upgrade` is its fix.
fn from_homebrew(path: &Path) -> bool {
    under_any(path, HOMEBREW_PREFIXES)
}

/// One field of a prerelease suffix: numbers sort before text, as semver has
/// it, so `1.27.0-beta.2` is older than `1.27.0-beta.b` and `.10` newer than
/// `.9`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Field {
    Number(u64),
    Text(String),
}

/// A version as its core numbers and its prerelease fields, for semver's
/// order. `None` for anything that is not `X.Y.Z[-pre]` (`dev`, `unknown`).
fn order(value: &str) -> Option<((u64, u64, u64), Vec<Field>)> {
    let value = value.trim().trim_start_matches('v');
    // Build metadata takes no part in ordering: `1.21.0+build.7` is `1.21.0`.
    let value = value.split('+').next().unwrap_or(value);
    let (core, pre) = match value.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (value, None),
    };
    let mut parts = core.split('.');
    let core = (
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    );
    if parts.next().is_some() {
        return None;
    }
    let mut fields = Vec::new();
    for field in pre.into_iter().flat_map(|pre| pre.split('.')) {
        if field.is_empty() {
            return None;
        }
        fields.push(match field.parse::<u64>() {
            Ok(number) => Field::Number(number),
            Err(_) => Field::Text(field.to_owned()),
        });
    }
    Some((core, fields))
}

fn direction(found: &str, this: &str) -> Direction {
    let (Some((found_core, found_pre)), Some((this_core, this_pre))) = (order(found), order(this))
    else {
        return Direction::Unknown;
    };
    // A release is newer than any prerelease of it.
    let ordering = found_core.cmp(&this_core).then_with(|| {
        match (found_pre.is_empty(), this_pre.is_empty()) {
            (true, true) => std::cmp::Ordering::Equal,
            (true, false) => std::cmp::Ordering::Greater,
            (false, true) => std::cmp::Ordering::Less,
            (false, false) => found_pre.cmp(&this_pre),
        }
    });
    match ordering {
        std::cmp::Ordering::Less => Direction::Older,
        std::cmp::Ordering::Greater => Direction::Newer,
        std::cmp::Ordering::Equal => Direction::Unknown,
    }
}

/// The one thing to run about `found`. Homebrew's copy is upgraded with
/// `brew`; the install script's copy is not updated with the app, so it is
/// deleted or replaced; anything else is a copy the user put there. A copy
/// *newer* than this engine is the other way round: the engine is what gets
/// updated.
fn advice(
    found: &PathRustcode,
    found_version: &str,
    this: &Path,
    this_version: &str,
    windows: bool,
    home: Option<&Path>,
) -> String {
    let path = found.path.display();
    let who = format!(
        "`rustcode` on PATH is {found_version} ({path}), but this engine is {this_version} ({}).",
        this.display()
    );
    let direction = direction(found_version, this_version);
    if !windows && from_homebrew(&found.path) {
        return match direction {
            Direction::Newer => format!(
                "{who} Update RustCode here (Settings › About › Check for updates), or remove {path}."
            ),
            _ => format!("{who} Run `brew upgrade lhagfoss/tap/rustcode`."),
        };
    }
    // The install script (install.md) puts a copy in the home directory that
    // the app's own updater never touches.
    if home.is_some_and(|home| found.path.starts_with(home.join(".rustcode").join("bin"))) {
        let delete = if windows {
            format!("Remove-Item \"{path}\"")
        } else {
            format!("rm {path}")
        };
        return format!(
            "{who} That copy comes from the install script and is not updated with the app: delete it (`{delete}`), or replace it with {}.",
            this.display()
        );
    }
    if direction == Direction::Newer {
        return format!(
            "{who} Update RustCode here, or remove {path} so a shell finds {}.",
            this.display()
        );
    }
    format!(
        "{who} Update {path} to {this_version}, or remove it so a shell finds {}.",
        this.display()
    )
}

/// What [`link`] should do about the copy on `PATH`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LinkPlan {
    /// A shell already runs this engine, or the same version of it.
    Current,
    /// Homebrew owns the copy: `brew upgrade` is its update, and replacing its
    /// link would be undone by the next `brew` run.
    Homebrew,
    /// Replace the copy on `PATH` with this engine.
    Replace,
}

fn link_plan(found: &PathRustcode, engine: &Path, engine_version: &str, windows: bool) -> LinkPlan {
    if same_file(&found.path, engine) || found.version.as_deref() == Some(engine_version) {
        LinkPlan::Current
    } else if !windows && from_homebrew(&found.path) {
        LinkPlan::Homebrew
    } else {
        LinkPlan::Replace
    }
}

/// What [`link`] did, in the words the CLI prints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Linked {
    /// No `PATH` has a `rustcode`: there is nothing to point.
    NotOnPath,
    /// A shell already finds this engine's version at the path.
    Current(PathBuf),
    /// Homebrew's copy: upgrade it with `brew`.
    Homebrew(PathBuf),
    /// The copy at `path` now is this engine (it was `was`).
    Replaced { path: PathBuf, was: Option<String> },
}

/// Point the `rustcode` a shell runs at this engine (issue #3411): the one
/// action behind "the app's version is not the one on PATH". On macOS and Linux
/// the copy becomes a symlink to this engine, so the next app update reaches
/// it too; Windows cannot symlink without privilege and gets a copy. Homebrew's
/// copy is left to `brew upgrade`, and a development build is refused.
pub async fn link(engine_version: &str) -> anyhow::Result<Linked> {
    let engine = std::env::current_exe()?;
    super::refuse_dev_binary(&engine, false)?;
    let Some(found) = observe().await else {
        return Ok(Linked::NotOnPath);
    };
    match link_plan(&found, &engine, engine_version, cfg!(windows)) {
        LinkPlan::Current => Ok(Linked::Current(found.path)),
        LinkPlan::Homebrew => Ok(Linked::Homebrew(found.path)),
        LinkPlan::Replace => {
            replace_with(&found.path, &engine).map_err(|error| {
                anyhow::Error::new(error).context(format!(
                    "could not replace {} (does your user own its folder?)",
                    found.path.display()
                ))
            })?;
            Ok(Linked::Replaced {
                path: found.path,
                was: found.version,
            })
        }
    }
}

/// Whether both paths are the same file, a link and its target included.
fn same_file(a: &Path, b: &Path) -> bool {
    a == b
        || matches!(
            (a.canonicalize(), b.canonicalize()),
            (Ok(a), Ok(b)) if a == b
        )
}

/// A symlink whose target is gone: nothing runs it, so the next install has to
/// replace the link itself rather than update what it named.
fn dangling(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink())
        && std::fs::metadata(path).is_err()
}

/// The files an install of RustCode writes, in the order a shell could find
/// them: the app's and the install script's in the home folder, then the
/// system one (`install.sh`: `/usr/local/bin` when it is writable, else
/// `~/.local/bin`). Windows has the one copy the script and the app write,
/// `%USERPROFILE%\.rustcode\bin`.
fn install_copies(home: Option<&Path>, windows: bool) -> Vec<PathBuf> {
    let name = if windows { "rustcode.exe" } else { "rustcode" };
    let mut dirs = Vec::new();
    if let Some(home) = home {
        dirs.push(home.join(".rustcode").join("bin"));
        if !windows {
            dirs.push(home.join(".local").join("bin"));
        }
    }
    if !windows {
        dirs.push(PathBuf::from("/usr/local/bin"));
    }
    dirs.into_iter().map(|dir| dir.join(name)).collect()
}

/// Who owns a `rustcode`, which decides whether this engine may replace it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Owner {
    /// Homebrew's: `brew upgrade` is its update, and replacing it would be
    /// undone by the next `brew` run.
    Homebrew,
    /// `cargo install`'s: cargo owns the folder it installs into.
    Cargo,
    /// A cargo build, inside a target directory: the next build replaces it.
    Development,
    /// The install script's or the app's: this engine may replace it.
    Ours,
    /// Somewhere else on `PATH`: a copy a person put there (their own build in
    /// `~/bin`, a tarball under `/opt`, a `cargo install --root`), which is
    /// named and never touched. Replacing it would delete work this app did
    /// not write.
    Foreign,
}

fn owner(path: &Path, home: Option<&Path>, windows: bool, engine: &Path) -> Owner {
    if !windows && from_homebrew(path) {
        return Owner::Homebrew;
    }
    let home_cargo = home.map(|home| home.join(".cargo"));
    let env_cargo = std::env::var_os("CARGO_HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from);
    if home_cargo
        .into_iter()
        .chain(env_cargo)
        .any(|cargo| path.starts_with(cargo.join("bin")))
    {
        return Owner::Cargo;
    }
    // A link is judged by what it points at: the app's engine is not a build,
    // a debug binary linked into a bin folder still is one.
    let program = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let target_dir = std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from);
    if super::is_dev_binary(&program, target_dir.as_deref()) {
        return Owner::Development;
    }
    // Only the files this app and the install script write are this app's:
    // anywhere else on PATH is somebody's copy, and is named rather than
    // replaced, whatever version it is.
    if same_file(path, engine)
        || install_copies(home, windows)
            .iter()
            .any(|copy| copy == path)
    {
        return Owner::Ours;
    }
    Owner::Foreign
}

/// The `rustcode` a shell runs first, else the first one an install wrote in a
/// known folder: the copy this app is responsible for. `None` when there is
/// none at all.
async fn find_installed_copy(home: Option<&Path>, windows: bool) -> Option<PathBuf> {
    let found = observe().await.map(|found| found.path);
    for path in found.into_iter().chain(install_copies(home, windows)) {
        // `symlink_metadata`: a copy that is a link counts, and a dangling one
        // is still this app's to replace.
        if std::fs::symlink_metadata(&path).is_ok() {
            return Some(path);
        }
    }
    None
}

/// What [`update_installed_copy`] should do about the copy it found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UpdatePlan {
    /// This engine's version, a newer one, or one that cannot be read: leave
    /// it alone.
    Keep,
    /// An older copy: replace it with this engine.
    Replace,
}

fn update_plan(copy_version: Option<&str>, engine_version: &str) -> UpdatePlan {
    // `dev` or a copy that does not answer: no direction, so nothing is
    // replaced on a guess.
    match copy_version {
        Some(copy) if direction(copy, engine_version) == Direction::Older => UpdatePlan::Replace,
        _ => UpdatePlan::Keep,
    }
}

/// What [`update_installed_copy`] did, in the words the CLI prints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstalledCopy {
    /// No `rustcode` is installed where a shell or an install would put one.
    Missing,
    /// The copy is this engine's version, a newer one, this engine itself, or
    /// unreadable.
    Current(PathBuf),
    /// The copy at `path` now is this engine (it was `was`).
    Replaced { path: PathBuf, was: Option<String> },
    /// Homebrew's or `cargo install`'s copy: its own tool updates it.
    Managed { path: PathBuf, by: &'static str },
    /// A cargo build: never replaced, since the next build replaces it.
    Development(PathBuf),
    /// A copy this app did not write (a person's own build in `~/bin`, a
    /// tarball under `/opt`): named, and left exactly as it is.
    Foreign(PathBuf),
}

/// Bring the `rustcode` a shell runs up to this engine's version (issue
/// #3487): the app's own updater replaces the app and its engine, and the copy
/// an earlier install left behind would keep a terminal on the old one.
///
/// The copy a shell runs first is the one updated; when there is none, the
/// first one an install wrote (`install_copies`) is. The copy becomes a real
/// file rather than a link, so it still runs when the app is moved or removed,
/// and the next app update replaces it again. A copy Homebrew or `cargo
/// install` owns is named and left to its tool, a development build is never
/// touched, and only a copy read as *older* is replaced — or a link whose
/// target is gone, which cannot be read at all.
pub async fn update_installed_copy(engine_version: &str) -> anyhow::Result<InstalledCopy> {
    let home = home_directory();
    let windows = cfg!(windows);
    let Some(path) = find_installed_copy(home.as_deref(), windows).await else {
        return Ok(InstalledCopy::Missing);
    };
    update_copy_at(path, &std::env::current_exe()?, engine_version).await
}

/// [`update_installed_copy`] for one copy, with `engine` standing in for this
/// process's own binary: so a test can watch which copies are written and
/// which are left alone.
async fn update_copy_at(
    path: PathBuf,
    engine: &Path,
    engine_version: &str,
) -> anyhow::Result<InstalledCopy> {
    let home = home_directory();
    let windows = cfg!(windows);
    match owner(&path, home.as_deref(), windows, engine) {
        Owner::Homebrew => {
            return Ok(InstalledCopy::Managed {
                path,
                by: "Homebrew",
            });
        }
        Owner::Cargo => {
            return Ok(InstalledCopy::Managed { path, by: "cargo" });
        }
        Owner::Development => return Ok(InstalledCopy::Development(path)),
        Owner::Foreign => return Ok(InstalledCopy::Foreign(path)),
        Owner::Ours => {}
    }
    // A link that already is this engine needs nothing; a dangling one is the
    // one case a copy cannot be asked its version, and the link is replaced.
    if same_file(&path, engine) {
        return Ok(InstalledCopy::Current(path));
    }
    let gone = dangling(&path);
    let version = if gone { None } else { ask_version(&path).await };
    if !gone && update_plan(version.as_deref(), engine_version) != UpdatePlan::Replace {
        return Ok(InstalledCopy::Current(path));
    }
    super::refuse_dev_binary(engine, false)?;
    copy_over(&path, engine).map_err(|error| {
        anyhow::Error::new(error).context(format!(
            "could not replace {} (does your user own its folder?)",
            path.display()
        ))
    })?;
    Ok(InstalledCopy::Replaced { path, was: version })
}

/// Make `target` this `engine` without a moment when `target` is missing: a
/// staged link or copy in the same folder is renamed over it.
fn replace_with(target: &Path, engine: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let directory = directory_of(target)?;
        let staged = directory.join(format!(".rustcode-link-{}", std::process::id()));
        let _ = std::fs::remove_file(&staged);
        std::os::unix::fs::symlink(engine, &staged)?;
        std::fs::rename(&staged, target).inspect_err(|_| {
            let _ = std::fs::remove_file(&staged);
        })
    }
    #[cfg(not(unix))]
    {
        copy_over(target, engine)
    }
}

/// Make `target` a *copy* of this `engine`, staged and renamed like
/// [`replace_with`]. The automatic update leaves a file that still runs when
/// the app it came from is moved or deleted, where a link into the app bundle
/// would dangle; Windows cannot symlink without privilege and copies anyway.
fn copy_over(target: &Path, engine: &Path) -> std::io::Result<()> {
    let directory = directory_of(target)?;
    // A start that died between the copy and the rename leaves its staged file
    // beside the target: nobody else writes these names, so they go now rather
    // than staying in the user's bin folder for good.
    remove_stale_copies(directory);
    let staged = directory.join(format!(".rustcode-copy-{}", std::process::id()));
    let _ = std::fs::remove_file(&staged);
    // A running executable cannot be overwritten, but it can be renamed.
    std::fs::copy(engine, &staged)?;
    let old = directory.join(format!(
        "{}.old",
        target
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    ));
    let _ = std::fs::remove_file(&old);
    std::fs::rename(target, &old).inspect_err(|_| {
        let _ = std::fs::remove_file(&staged);
    })?;
    if let Err(error) = std::fs::rename(&staged, target) {
        let _ = std::fs::rename(&old, target);
        let _ = std::fs::remove_file(&staged);
        return Err(error);
    }
    let _ = std::fs::remove_file(&old);
    Ok(())
}

fn remove_stale_copies(directory: &Path) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with(".rustcode-copy-")
        {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

fn directory_of(target: &Path) -> std::io::Result<&Path> {
    target.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "the path has no folder")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(path: &str, version: &str) -> PathRustcode {
        PathRustcode {
            path: PathBuf::from(path),
            version: Some(version.to_owned()),
        }
    }

    #[test]
    fn versions_are_read_from_the_binarys_own_answer() {
        assert_eq!(version_from("rustcode 1.28.0\n").as_deref(), Some("1.28.0"));
        assert_eq!(version_from("1.2.3").as_deref(), Some("1.2.3"));
        assert_eq!(version_from("rustcode\n"), None);
        assert_eq!(version_from("error: not found"), None);
        assert_eq!(version_from(""), None);
    }

    #[test]
    fn the_first_rustcode_on_the_path_wins_and_other_names_never_match() {
        let dir = std::env::temp_dir().join(format!("rc-path-{}", std::process::id()));
        let first = dir.join("first");
        let second = dir.join("second");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        // Copies `PATH` could actually run: a file without an execute bit is
        // not one (see `a_file_that_cannot_be_run_is_not_the_copy_a_shell_runs`).
        #[cfg(unix)]
        let executable = |path: &Path| {
            use std::os::unix::fs::PermissionsExt;
            std::fs::write(path, "").unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        #[cfg(not(unix))]
        let executable = |path: &Path| std::fs::write(path, "").unwrap();
        executable(&first.join("rustcode-tui"));
        executable(&second.join("rustcode"));
        let path = std::env::join_paths([&first, &second]).unwrap();
        let exts = vec![String::new()];
        assert_eq!(find_on(&path, &exts), Some(second.join("rustcode")));
        // On Windows the resolved name carries its extension.
        executable(&first.join("rustcode.exe"));
        let with_exe = vec![String::new(), ".exe".to_owned()];
        assert_eq!(find_on(&path, &with_exe), Some(first.join("rustcode.exe")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_older_homebrew_copy_names_the_brew_command() {
        let found = found("/opt/homebrew/bin/rustcode", "0.96.0");
        let text = mismatch(
            &found,
            Path::new("/Users/ada/.local/bin/rustcode"),
            "1.28.0",
        )
        .unwrap();
        assert!(
            text.contains("0.96.0 (/opt/homebrew/bin/rustcode)"),
            "{text}"
        );
        assert!(text.contains("this engine is 1.28.0"), "{text}");
        assert!(
            text.contains("brew upgrade lhagfoss/tap/rustcode"),
            "{text}"
        );
    }

    #[test]
    fn the_install_scripts_copy_is_deleted_or_replaced() {
        let found = found("/Users/ada/.rustcode/bin/rustcode", "1.25.0");
        let text = advice(
            &found,
            "1.25.0",
            Path::new("/Applications/RustCode.app/Contents/MacOS/rustcode"),
            "1.28.0",
            false,
            Some(Path::new("/Users/ada")),
        );
        assert!(
            text.contains("rm /Users/ada/.rustcode/bin/rustcode"),
            "{text}"
        );
        assert!(text.contains("replace it with"), "{text}");
        // Windows names the same copy with its own command.
        let windows = advice(
            &found,
            "1.25.0",
            Path::new("C:\\Program Files\\RustCode\\rustcode.exe"),
            "1.28.0",
            true,
            Some(Path::new("/Users/ada")),
        );
        assert!(windows.contains("Remove-Item"), "{windows}");
    }

    #[test]
    fn another_copy_is_updated_or_removed() {
        let found = found("/usr/bin/rustcode", "1.20.0");
        let text = advice(
            &found,
            "1.20.0",
            Path::new("/opt/rustcode/rustcode"),
            "1.28.0",
            false,
            Some(Path::new("/Users/ada")),
        );
        assert!(
            text.contains("Update /usr/bin/rustcode to 1.28.0"),
            "{text}"
        );
        assert!(text.contains("or remove it so a shell finds"), "{text}");
    }

    /// An app launched from the Dock inherits launchd's short `PATH`, which has
    /// neither Homebrew's nor the install script's directory in it: the copy a
    /// terminal runs is found through the login shell's `PATH`.
    #[test]
    fn the_path_a_shell_uses_comes_before_the_process_path() {
        let login = std::ffi::OsString::from("/opt/homebrew/bin:/Users/ada/.rustcode/bin");
        let process = std::ffi::OsString::from("/usr/bin:/bin:/opt/homebrew/bin");
        let chosen: Vec<PathBuf> =
            std::env::split_paths(&chosen_path(Some(login), Some(process), false)).collect();
        assert_eq!(
            chosen,
            vec![
                PathBuf::from("/opt/homebrew/bin"),
                // Where the install script puts its copy: invisible to a Dock
                // launch, so the warning must find it through the login shell.
                PathBuf::from("/Users/ada/.rustcode/bin"),
                // The process's own directories still count, once.
                PathBuf::from("/usr/bin"),
                PathBuf::from("/bin"),
            ]
        );
        // A process that already has the full path is not duplicated.
        let only = std::ffi::OsString::from("/usr/bin:/bin");
        assert_eq!(
            chosen_path(Some(only.clone()), Some(only.clone()), false),
            only
        );
        // No login shell to ask (a test, or a missing one): the process's.
        assert_eq!(chosen_path(None, Some(only.clone()), false), only);
        assert_eq!(chosen_path(Some(only.clone()), None, false), only);
        // Windows keeps its own PATH: there is no login shell to ask.
        let windows = chosen_path(
            Some(std::ffi::OsString::from("C:\\Login\\bin")),
            Some(std::ffi::OsString::from("C:\\Windows\\System32")),
            true,
        );
        assert_eq!(windows, std::ffi::OsString::from("C:\\Windows\\System32"));
    }

    /// Intel Homebrew is `/usr/local/bin` symlinked into `/usr/local/Cellar`,
    /// so the link is resolved before the prefix is matched.
    #[test]
    fn a_homebrew_copy_is_recognised_through_its_bin_symlink() {
        let dir = std::env::temp_dir().join(format!("rc-brew-{}", std::process::id()));
        let cellar = dir.join("Cellar/rustcode/1.28.0/bin");
        let bin = dir.join("bin");
        std::fs::create_dir_all(&cellar).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(cellar.join("rustcode"), "").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(cellar.join("rustcode"), bin.join("rustcode")).unwrap();
        #[cfg(unix)]
        {
            // `canonicalize` returns the real path (`/private/var/...` on
            // macOS), so the prefix the copy is matched against is real too.
            let root = dir.canonicalize().unwrap();
            let prefixes = [format!("{}/Cellar/", root.display())];
            let prefixes: Vec<&str> = prefixes.iter().map(String::as_str).collect();
            assert!(under_any(&bin.join("rustcode"), &prefixes));
            // A copy that is nobody's symlink is not Homebrew's.
            let plain = dir.join("plain/rustcode");
            std::fs::create_dir_all(plain.parent().unwrap()).unwrap();
            std::fs::write(&plain, "").unwrap();
            assert!(!under_any(&plain, &prefixes));
        }
        // The real prefixes keep matching at the string level.
        assert!(from_homebrew(Path::new(
            "/usr/local/Cellar/rustcode/1.28.0/bin/rustcode"
        )));
        assert!(from_homebrew(Path::new("/opt/homebrew/bin/rustcode")));
        assert!(!from_homebrew(Path::new("/usr/bin/rustcode")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A `rustcode` without an execute bit is no copy `PATH` runs, so it must
    /// not shadow the real one and leave it unreported.
    #[cfg(unix)]
    #[test]
    fn a_file_that_cannot_be_run_is_not_the_copy_a_shell_runs() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("rc-exec-{}", std::process::id()));
        let shadow = dir.join("shadow");
        let real = dir.join("real");
        std::fs::create_dir_all(&shadow).unwrap();
        std::fs::create_dir_all(&real).unwrap();
        // A copy that is there but cannot be run, as an interrupted install
        // leaves it.
        std::fs::write(shadow.join("rustcode"), "").unwrap();
        std::fs::write(real.join("rustcode"), "").unwrap();
        std::fs::set_permissions(
            real.join("rustcode"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let path = std::env::join_paths([&shadow, &real]).unwrap();
        let exts = vec![String::new()];
        assert_eq!(find_on(&path, &exts), Some(real.join("rustcode")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_copy_newer_than_this_engine_points_at_this_engine_instead() {
        let found = found("/opt/homebrew/bin/rustcode", "1.29.0");
        let text = advice(
            &found,
            "1.29.0",
            Path::new("/Applications/RustCode.app/Contents/MacOS/rustcode"),
            "1.28.0",
            false,
            Some(Path::new("/Users/ada")),
        );
        assert!(text.contains("Update RustCode here"), "{text}");
        assert!(!text.contains("brew upgrade"), "{text}");

        let elsewhere = advice(
            &found,
            "1.29.0",
            Path::new("/usr/local/bin/rustcode"),
            "1.28.0",
            false,
            Some(Path::new("/Users/ada")),
        );
        assert!(elsewhere.contains("is 1.29.0"), "{elsewhere}");
        assert!(
            !elsewhere.contains("Update /opt/homebrew/bin/rustcode to"),
            "{elsewhere}"
        );
    }

    #[test]
    fn matching_versions_and_unrunnable_copies_report_nothing() {
        let same = found("/opt/homebrew/bin/rustcode", "1.28.0");
        assert_eq!(
            mismatch(&same, Path::new("/opt/homebrew/bin/rustcode"), "1.28.0"),
            None
        );
        let unknown = PathRustcode {
            path: PathBuf::from("/opt/homebrew/bin/rustcode"),
            version: None,
        };
        assert_eq!(
            mismatch(&unknown, Path::new("/usr/local/bin/rustcode"), "1.28.0"),
            None
        );
    }

    #[test]
    fn the_link_plan_leaves_current_and_homebrew_copies_alone() {
        let engine = Path::new("/Applications/RustCode.app/Contents/MacOS/rustcode");
        let same_version = found("/Users/ada/.rustcode/bin/rustcode", "1.28.0");
        assert_eq!(
            link_plan(&same_version, engine, "1.28.0", false),
            LinkPlan::Current
        );
        let brew = found("/opt/homebrew/bin/rustcode", "0.96.0");
        assert_eq!(
            link_plan(&brew, engine, "1.28.0", false),
            LinkPlan::Homebrew
        );
        // Windows has no Homebrew.
        assert_eq!(link_plan(&brew, engine, "1.28.0", true), LinkPlan::Replace);
        let script = found("/Users/ada/.rustcode/bin/rustcode", "1.25.0");
        assert_eq!(
            link_plan(&script, engine, "1.28.0", false),
            LinkPlan::Replace
        );
        // The engine itself is current whatever its version reads as.
        let itself = PathRustcode {
            path: engine.to_path_buf(),
            version: None,
        };
        assert_eq!(
            link_plan(&itself, engine, "1.28.0", false),
            LinkPlan::Current
        );
    }

    #[test]
    fn versions_are_ordered_as_semver_has_them() {
        assert_eq!(direction("1.27.0", "1.28.0"), Direction::Older);
        assert_eq!(direction("1.29.0", "1.28.0"), Direction::Newer);
        assert_eq!(direction("1.28.0", "1.28.0"), Direction::Unknown);
        // A prerelease is older than its release, and than the next one.
        assert_eq!(direction("1.27.0-beta.2", "1.28.0"), Direction::Older);
        assert_eq!(direction("1.28.0-beta.2", "1.28.0"), Direction::Older);
        assert_eq!(direction("1.28.0", "1.28.0-beta.2"), Direction::Newer);
        // Fields count as numbers and compare one by one.
        assert_eq!(
            direction("1.28.0-beta.9", "1.28.0-beta.10"),
            Direction::Older
        );
        assert_eq!(
            direction("1.28.0-beta.10", "1.28.0-beta.9"),
            Direction::Newer
        );
        // Anything that is not a version has no direction at all.
        assert_eq!(direction("dev", "1.28.0"), Direction::Unknown);
        assert_eq!(direction("1.28", "1.28.0"), Direction::Unknown);
    }

    #[test]
    fn an_installed_copy_is_replaced_only_when_it_is_older() {
        // The install script's copy, left at an older version: replaced.
        assert_eq!(update_plan(Some("1.25.0"), "1.28.0"), UpdatePlan::Replace);
        // A beta of an older release is older too.
        assert_eq!(
            update_plan(Some("1.27.0-beta.2"), "1.28.0"),
            UpdatePlan::Replace
        );
        // Current, newer, or no answer at all: left alone.
        assert_eq!(update_plan(Some("1.28.0"), "1.28.0"), UpdatePlan::Keep);
        assert_eq!(update_plan(Some("1.29.0"), "1.28.0"), UpdatePlan::Keep);
        assert_eq!(update_plan(None, "1.28.0"), UpdatePlan::Keep);
        assert_eq!(update_plan(Some("dev"), "1.28.0"), UpdatePlan::Keep);
    }

    #[test]
    fn the_copies_an_install_writes_are_searched_in_shell_order() {
        assert_eq!(
            install_copies(Some(Path::new("/Users/ada")), false),
            vec![
                PathBuf::from("/Users/ada/.rustcode/bin/rustcode"),
                PathBuf::from("/Users/ada/.local/bin/rustcode"),
                PathBuf::from("/usr/local/bin/rustcode"),
            ]
        );
        // Windows has the one the script and the app write, with its extension.
        assert_eq!(
            install_copies(Some(Path::new("C:\\Users\\ada")), true),
            vec![PathBuf::from("C:\\Users\\ada/.rustcode/bin/rustcode.exe")]
        );
        // No home folder: only the system one is left.
        assert_eq!(
            install_copies(None, false),
            vec![PathBuf::from("/usr/local/bin/rustcode")]
        );
    }

    /// Homebrew and `cargo install` own their copies, a build is never
    /// replaced, and anything outside the folders an install writes is
    /// somebody else's: only the install script's, the app's own and the app's
    /// engine are this engine's to replace.
    #[test]
    fn only_the_install_scripts_copy_is_this_apps_to_replace() {
        let home = Path::new("/Users/ada");
        let engine = Path::new("/Applications/RustCode.app/Contents/MacOS/rustcode");
        assert_eq!(
            owner(
                Path::new("/opt/homebrew/bin/rustcode"),
                Some(home),
                false,
                engine
            ),
            Owner::Homebrew
        );
        assert_eq!(
            owner(
                Path::new("/Users/ada/.cargo/bin/rustcode"),
                Some(home),
                false,
                engine
            ),
            Owner::Cargo
        );
        assert_eq!(
            owner(
                Path::new("/Users/ada/code/rustcode/target/debug/rustcode"),
                Some(home),
                false,
                engine
            ),
            Owner::Development
        );
        for path in [
            "/Users/ada/.rustcode/bin/rustcode",
            "/Users/ada/.local/bin/rustcode",
            "/usr/local/bin/rustcode",
        ] {
            assert_eq!(
                owner(Path::new(path), Some(home), false, engine),
                Owner::Ours,
                "{path}"
            );
        }
        // The app's own engine, whatever path it was installed at.
        assert_eq!(owner(engine, Some(home), false, engine), Owner::Ours);
        // Windows has no Homebrew, and its one install folder is the app's.
        assert_eq!(
            owner(
                Path::new("C:\\Users\\ada\\.rustcode\\bin\\rustcode.exe"),
                Some(home),
                true,
                engine
            ),
            Owner::Ours
        );
        // A copy a person put somewhere else is not the app's, whatever its
        // version: their own build in `~/bin`, a `cargo install --root`, a
        // tarball under `/opt`.
        for path in [
            "/Users/ada/bin/rustcode",
            "/Users/ada/.local/rustcode/bin/rustcode",
            "/opt/rustcode/bin/rustcode",
        ] {
            assert_eq!(
                owner(Path::new(path), Some(home), false, engine),
                Owner::Foreign,
                "{path}"
            );
        }
    }

    /// A copy somewhere else is named and left exactly as it was: the app
    /// replaces only files it writes, never a person's own build (issue
    /// #3487).
    #[tokio::test]
    async fn a_foreign_copy_is_left_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let engine = dir.path().join("engine");
        std::fs::write(&engine, "new engine").unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let copy = bin.join("rustcode");
        std::fs::write(&copy, "a person's own build").unwrap();
        let outcome = update_copy_at(copy.clone(), &engine, "1.30.0")
            .await
            .expect("a foreign copy is not an error");
        assert_eq!(outcome, InstalledCopy::Foreign(copy.clone()));
        // Untouched: not replaced, not renamed aside, nothing staged beside it.
        assert_eq!(
            std::fs::read_to_string(&copy).unwrap(),
            "a person's own build"
        );
        let left: Vec<_> = std::fs::read_dir(&bin)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(left, vec![std::ffi::OsString::from("rustcode")]);
    }

    /// A link to a copy that moved leaves a name nothing can run: the next
    /// install replaces the link rather than asking it a version.
    #[cfg(unix)]
    #[test]
    fn a_dangling_link_is_replaceable() {
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("rustcode");
        std::os::unix::fs::symlink(dir.path().join("gone/rustcode"), &link).unwrap();
        assert!(dangling(&link));
        // A file, and a link that answers, are not dangling.
        let file = dir.path().join("real");
        std::fs::write(&file, "").unwrap();
        assert!(!dangling(&file));
        std::os::unix::fs::symlink(&file, dir.path().join("good")).unwrap();
        assert!(!dangling(&dir.path().join("good")));
    }

    /// An attempt that died between the copy and the rename leaves a staged
    /// file; the next one clears it instead of letting it pile up.
    #[test]
    fn a_stale_staged_copy_is_cleared() {
        let dir = tempfile::tempdir().unwrap();
        let engine = dir.path().join("engine");
        std::fs::write(&engine, "new engine").unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let target = bin.join("rustcode");
        std::fs::write(&target, "old engine").unwrap();
        let leftover = bin.join(".rustcode-copy-999999");
        std::fs::write(&leftover, "half a copy").unwrap();
        copy_over(&target, &engine).unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "new engine");
        let left: Vec<_> = std::fs::read_dir(&bin)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(left, vec![std::ffi::OsString::from("rustcode")]);
    }

    #[test]
    fn replacing_a_stale_copy_makes_the_path_run_this_engine() {
        let dir = tempfile::tempdir().unwrap();
        let engine = dir.path().join("engine");
        std::fs::write(&engine, "new engine").unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let target = bin.join("rustcode");
        std::fs::write(&target, "old engine").unwrap();
        replace_with(&target, &engine).unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "new engine");
        // No staged file or old copy is left in the folder.
        let left: Vec<_> = std::fs::read_dir(&bin)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(left, vec![std::ffi::OsString::from("rustcode")]);
        #[cfg(unix)]
        assert!(std::fs::symlink_metadata(&target).unwrap().is_symlink());
    }
}
