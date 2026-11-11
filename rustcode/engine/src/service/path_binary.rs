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
//! This says which copy `PATH` finds, which version it is, and the exact
//! command that fixes it. Nothing here changes the machine.

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

/// `PATH` env value and the extensions to try, as the OS names them.
fn path_and_extensions() -> (std::ffi::OsString, Vec<String>) {
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
    (std::env::var_os("PATH").unwrap_or_default(), exts)
}

/// `rustcode` on `path`, trying each of `exts`. The name is fixed, so no
/// shell syntax or folders can come in through it.
fn find_on(path: &std::ffi::OsStr, exts: &[String]) -> Option<PathBuf> {
    for directory in std::env::split_paths(path) {
        for ext in exts {
            let candidate = directory.join(format!("rustcode{ext}"));
            if candidate.is_file() {
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

/// The `rustcode` a shell runs, or `None` when `PATH` has none.
pub async fn observe() -> Option<PathRustcode> {
    let (path, exts) = path_and_extensions();
    let binary = find_on(&path, &exts)?;
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
        .stderr(std::process::Stdio::null());
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
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

/// Which copy is newer, when both versions can be read as numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    /// The copy on `PATH` is older: update it, or drop it.
    Older,
    /// The copy on `PATH` is newer: this engine is the one to update.
    Newer,
    /// Versions that do not parse (`dev`, a build suffix): no direction.
    Unknown,
}

fn direction(found: &str, this: &str) -> Direction {
    fn numbers(value: &str) -> Option<Vec<u64>> {
        value
            .split('.')
            .map(str::parse)
            .collect::<Result<_, _>>()
            .ok()
    }
    match (numbers(found), numbers(this)) {
        (Some(found), Some(this)) if found < this => Direction::Older,
        (Some(found), Some(this)) if found > this => Direction::Newer,
        _ => Direction::Unknown,
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
    if !windows
        && (found.path.starts_with("/opt/homebrew/")
            || found.path.starts_with("/usr/local/Cellar/"))
    {
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
        std::fs::write(first.join("rustcode-tui"), "").unwrap();
        std::fs::write(second.join("rustcode"), "").unwrap();
        let path = std::env::join_paths([&first, &second]).unwrap();
        let exts = vec![String::new()];
        assert_eq!(find_on(&path, &exts), Some(second.join("rustcode")));
        // On Windows the resolved name carries its extension.
        std::fs::write(first.join("rustcode.exe"), "").unwrap();
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
        let found = found("/usr/local/bin/rustcode", "1.20.0");
        let text = advice(
            &found,
            "1.20.0",
            Path::new("/opt/rustcode/rustcode"),
            "1.28.0",
            false,
            Some(Path::new("/Users/ada")),
        );
        assert!(
            text.contains("Update /usr/local/bin/rustcode to 1.28.0"),
            "{text}"
        );
        assert!(text.contains("or remove it so a shell finds"), "{text}");
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
}
