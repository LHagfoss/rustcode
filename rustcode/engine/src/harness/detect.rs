//! Whether a harness is installed, its version and sign-in, found cheaply:
//! a `PATH` lookup and, where the harness has one, a quick status command
//! under a timeout. The lookup follows the platform's shell, so on Windows a
//! bare name tries each `PATHEXT` extension (`codex` is `codex.exe`, npm's
//! `npx` is `npx.cmd`). Results are cached for a minute.

use super::{HarnessKind, ResolvedHarness};
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const PROBE_TIMEOUT: Duration = Duration::from_secs(4);
const CACHE_TTL: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Detection {
    pub installed: bool,
    pub version: Option<String>,
    /// `ok`, `needs_login` or `unknown`.
    pub auth: &'static str,
    pub error: Option<String>,
}

impl Default for Detection {
    fn default() -> Self {
        Self {
            installed: false,
            version: None,
            auth: "unknown",
            error: None,
        }
    }
}

fn cache() -> &'static Mutex<HashMap<String, (Instant, Vec<String>, Detection)>> {
    static CACHE: OnceLock<Mutex<HashMap<String, (Instant, Vec<String>, Detection)>>> =
        OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// Drop the cached detection of `id`, after its entry changed.
pub(crate) fn forget(id: &str) {
    cache()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .remove(id);
}

/// Whether a named account's config directory holds a sign-in.
///
/// A `probe` look runs the harness's own cheap status command with the
/// account's directory (`codex login status`), which is authoritative about a
/// sign-in that expired behind `auth.json` and prints no secret. Only the
/// CLI's own `Not logged in` counts as a sign-out; any other failure (an older
/// CLI without the command, a broken `config.toml`, a shim) is no answer, and
/// the login's own credentials file is looked for by name instead
/// (`.credentials.json`, Codex's `auth.json`), never opened. Claude Code has no
/// status command: its macOS Keychain item, named after the directory, is asked
/// for existence too. An account on the harness's default directory is the
/// login with no variable set, so it reads as that login does. Nothing is
/// started as a session and no token is read. `ok` is a sign-in found,
/// `needs_login` one looked for and not found (only after `probe`), `unknown`
/// not looked for.
pub(crate) fn account_auth(account: &super::config::HarnessAccount, probe: bool) -> &'static str {
    let Some(layout) = super::config::account_layout(&account.harness) else {
        return "unknown";
    };
    let dir = account.resolved_dir();
    let status = super::known(&account.harness).and_then(|known| known.auth_probe);
    if probe && let Some(args) = status {
        let env = super::config::login_env(&account.harness, &dir);
        let env: Vec<(&str, &std::path::Path)> = env
            .as_ref()
            .map(|(name, value)| (*name, std::path::Path::new(value)))
            .into_iter()
            .collect();
        if let Some((success, output)) = run_quick_env(args[0], &args[1..], &env) {
            match status_says_signed_in(success, &output) {
                Some(true) => return "ok",
                Some(false) => return "needs_login",
                // No answer we can trust: fall through to the file by name.
                None => {}
            }
        }
    }
    if dir.join(layout.marker).is_file() {
        return "ok";
    }
    if !probe {
        return "unknown";
    }
    if status.is_some() {
        // A harness with a status command has no Keychain to ask as well.
        return "needs_login";
    }
    // Claude Code: on macOS its sign-in is a Keychain item named after the
    // directory, and the default login's after none.
    let found = if super::config::is_default_dir(&account.harness, &dir) {
        crate::provider_auth::claude_usage::default_keychain_item_exists()
    } else {
        crate::provider_auth::claude_usage::keychain_item_exists(&dir)
    };
    if found { "ok" } else { "needs_login" }
}

/// What a harness's own sign-in status command said. Only the Codex CLI's own
/// `Not logged in` (printed with exit 1) is a sign-out; a non-zero exit for any
/// other reason is no answer at all.
fn status_says_signed_in(success: bool, output: &str) -> Option<bool> {
    if output.to_ascii_lowercase().contains("not logged in") {
        return Some(false);
    }
    success.then_some(true)
}

/// The version the last detection of `id` found, without running anything.
pub(crate) fn cached_version(id: &str) -> Option<String> {
    cache()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(id)
        .and_then(|(_, _, detection)| detection.version.clone())
}

pub(crate) fn detect(harness: &ResolvedHarness) -> Detection {
    if harness.kind == HarnessKind::Builtin {
        return Detection {
            installed: true,
            version: Some(env!("CARGO_PKG_VERSION").to_owned()),
            auth: "ok",
            error: None,
        };
    }
    if let Some((at, command, detection)) = cache()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(&harness.id)
        && at.elapsed() < CACHE_TTL
        && *command == harness.command
    {
        return detection.clone();
    }
    let detection = detect_uncached(harness);
    cache()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(
            harness.id.clone(),
            (Instant::now(), harness.command.clone(), detection.clone()),
        );
    detection
}

fn detect_uncached(harness: &ResolvedHarness) -> Detection {
    let Some(program) = harness.command.first() else {
        return Detection {
            error: Some("no command configured".to_owned()),
            ..Detection::default()
        };
    };
    if resolve_program(program).is_none() {
        return Detection::default();
    }
    let mut detection = Detection {
        installed: true,
        ..Detection::default()
    };
    if let Some(known) = harness.known {
        if let Some((program, args)) = known.version
            && resolve_program(program).is_some()
            && let Some((true, output)) = run_quick(program, args)
        {
            detection.version = parse_version(&output);
        }
        if let Some(probe) = known.auth_probe
            && let Some((success, output)) = run_quick(probe[0], &probe[1..])
        {
            detection.auth = if success && !output.to_ascii_lowercase().contains("not logged in") {
                "ok"
            } else {
                "needs_login"
            };
        }
    }
    detection
}

/// The first version-looking word of `output`: `codex-cli 0.162.1` gives
/// `0.162.1`, `2.1.2 (Claude Code)` gives `2.1.2`.
pub(crate) fn parse_version(output: &str) -> Option<String> {
    output
        .split_whitespace()
        .map(|word| word.trim_start_matches('v'))
        .find(|word| {
            word.chars().next().is_some_and(|c| c.is_ascii_digit())
                && word.contains('.')
                && word
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '+')
        })
        .map(str::to_owned)
}

/// Run `program args` with no input, for at most a few seconds. Returns
/// whether it succeeded and its stdout and stderr.
fn run_quick(program: &str, args: &[&str]) -> Option<(bool, String)> {
    run_quick_env(program, args, &[])
}

/// [`run_quick`] with environment variables added, for a harness status
/// command that has to look at one login's own directory.
fn run_quick_env(program: &str, args: &[&str], env: &[(&str, &Path)]) -> Option<(bool, String)> {
    let path = resolve_program(program)?;
    let mut command = std::process::Command::new(path);
    command
        .args(args)
        .env("PATH", search_path())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    for (name, value) in env {
        command.env(name, value);
    }
    let mut child = command.spawn().ok()?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < PROBE_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(25));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let output = child.wait_with_output().ok()?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push('\n');
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    Some((output.status.success(), text))
}

/// The `PATH` a harness is looked up on and started with: the engine's own,
/// then the login shell's, so an app started from the Dock finds the same
/// commands as the terminal.
pub(crate) fn search_path() -> std::ffi::OsString {
    #[cfg_attr(test, allow(unused_mut))]
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    #[cfg(not(test))]
    if let Some(login) = crate::shell_env::login_shell_var("PATH") {
        for dir in std::env::split_paths(&login) {
            if !dirs.contains(&dir) {
                dirs.push(dir);
            }
        }
    }
    std::env::join_paths(dirs).unwrap_or_default()
}

/// The names one configured program becomes, platform by platform: as written
/// on Unix; on Windows, a name with no extension of its own gains each
/// `PATHEXT` one (`codex` is `codex.exe`, npm's `npx` is `npx.cmd`), because
/// the extensionless file a Unix shell would run is not a Windows program.
struct Lookup {
    windows: bool,
    extensions: Vec<std::ffi::OsString>,
}

impl Lookup {
    fn current() -> Self {
        Self {
            windows: cfg!(windows),
            extensions: if cfg!(windows) {
                parse_pathext(std::env::var_os("PATHEXT").as_deref())
            } else {
                Vec::new()
            },
        }
    }

    /// File names to try for `program`, in the order a Windows shell tries
    /// them. A name that already has an extension is taken literally.
    fn names(&self, program: &str) -> Vec<std::ffi::OsString> {
        if !self.windows || Path::new(program).extension().is_some() {
            return vec![program.into()];
        }
        let names: Vec<std::ffi::OsString> = self
            .extensions
            .iter()
            .map(|extension| {
                let mut name = std::ffi::OsString::from(program);
                name.push(extension);
                name
            })
            .collect();
        if names.is_empty() {
            vec![program.into()]
        } else {
            names
        }
    }
}

/// The extensions `PATHEXT` lists, in order; `None` or an empty value is the
/// Windows default.
fn parse_pathext(value: Option<&std::ffi::OsStr>) -> Vec<std::ffi::OsString> {
    const DEFAULT: &str = ".COM;.EXE;.BAT;.CMD";
    let value = value
        .filter(|value| !value.is_empty())
        .unwrap_or(std::ffi::OsStr::new(DEFAULT));
    let extensions: Vec<std::ffi::OsString> = value
        .to_string_lossy()
        .split(';')
        .map(str::trim)
        .filter(|extension| !extension.is_empty())
        .map(Into::into)
        .collect();
    if extensions.is_empty() {
        DEFAULT.split(';').map(Into::into).collect()
    } else {
        extensions
    }
}

/// Where `program` resolves: itself when it is a path, else the first
/// executable of that name on [`search_path`]. A bare name is looked up the
/// way this platform's shell would find it (see [`Lookup`]).
pub(crate) fn resolve_program(program: &str) -> Option<PathBuf> {
    let lookup = Lookup::current();
    if is_program_path(program) {
        return resolve_path(program, &lookup);
    }
    resolve_in(
        std::env::split_paths(&search_path()).filter(|dir| !dir.as_os_str().is_empty()),
        program,
        &lookup,
    )
}

/// An explicit path, with its `PATHEXT` extension filled in on Windows.
fn resolve_path(program: &str, lookup: &Lookup) -> Option<PathBuf> {
    lookup
        .names(program)
        .into_iter()
        .map(PathBuf::from)
        .find(|path| is_executable(path))
}

/// Whether `program` names a file itself rather than one to look up on
/// `PATH`. Windows accepts either separator, and `C:name` is drive-relative.
fn is_program_path(program: &str) -> bool {
    program.contains('/')
        || (cfg!(windows) && (program.contains('\\') || program.as_bytes().get(1) == Some(&b':')))
}

/// The first executable of `program`'s names across `dirs`, in order.
fn resolve_in(
    dirs: impl IntoIterator<Item = PathBuf>,
    program: &str,
    lookup: &Lookup,
) -> Option<PathBuf> {
    let names = lookup.names(program);
    for dir in dirs {
        for name in &names {
            let candidate = dir.join(name);
            if is_executable(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_parse() {
        assert_eq!(
            parse_version("codex-cli 0.162.1\n").as_deref(),
            Some("0.162.1")
        );
        assert_eq!(parse_version("opencode v2.1.2").as_deref(), Some("2.1.2"));
        assert_eq!(
            parse_version("2.1.2 (Claude Code)").as_deref(),
            Some("2.1.2")
        );
        assert_eq!(parse_version("no version here"), None);
    }

    #[test]
    fn missing_programs_are_not_installed() {
        let harness = ResolvedHarness {
            id: "ghost".into(),
            name: "Ghost".into(),
            kind: HarnessKind::Acp,
            command: vec!["rustcode-no-such-harness-binary".into()],
            enabled: true,
            known: None,
        };
        let detection = detect(&harness);
        assert!(!detection.installed);
        assert_eq!(detection.auth, "unknown");
        assert!(resolve_program("sh").is_some());
    }

    fn windows_lookup(extensions: &[&str]) -> Lookup {
        Lookup {
            windows: true,
            extensions: extensions.iter().map(Into::into).collect(),
        }
    }

    /// A file that `is_executable` accepts on this host too, so the Windows
    /// candidate order can be tested from Unix.
    fn write_program(path: &Path) {
        std::fs::write(path, b"").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[test]
    fn a_windows_bare_name_tries_pathext_extensions() {
        let lookup = windows_lookup(&[".COM", ".EXE", ".BAT", ".CMD"]);
        let names = lookup.names("codex");
        assert_eq!(
            names
                .first()
                .map(|name| name.to_string_lossy().into_owned()),
            Some("codex.COM".into())
        );
        assert!(names.iter().any(|name| name == "codex.EXE"));
        // npm installs `npx.cmd`, never an executable `npx`.
        let npx = lookup.names("npx");
        assert!(npx.iter().any(|name| name == "npx.CMD"));
        // A name that already has an extension is taken literally.
        assert_eq!(
            lookup.names("codex.exe"),
            vec![std::ffi::OsString::from("codex.exe")]
        );
    }

    #[test]
    fn a_unix_bare_name_is_literal() {
        let lookup = Lookup {
            windows: false,
            extensions: vec![".EXE".into()],
        };
        assert_eq!(
            lookup.names("codex"),
            vec![std::ffi::OsString::from("codex")]
        );
    }

    #[test]
    fn pathext_is_read_in_order_with_a_windows_default() {
        let default: Vec<std::ffi::OsString> = [".COM", ".EXE", ".BAT", ".CMD"]
            .iter()
            .map(Into::into)
            .collect();
        assert_eq!(parse_pathext(None), default);
        assert_eq!(parse_pathext(Some(std::ffi::OsStr::new(""))), default);
        // An entry may be empty and is skipped; case is the user's own.
        assert_eq!(
            parse_pathext(Some(std::ffi::OsStr::new(".exe;;.cmd"))),
            vec![
                std::ffi::OsString::from(".exe"),
                std::ffi::OsString::from(".cmd")
            ]
        );
    }

    #[test]
    fn a_windows_executable_is_found_through_a_path_directory() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("codex.exe");
        write_program(&exe);
        // The Unix literal name is not enough on Windows, which is the reported
        // bug: the file exists and its directory is on PATH, yet detection
        // said installed=false.
        assert_eq!(
            resolve_in(
                [dir.path().to_owned()],
                "codex",
                &Lookup {
                    windows: false,
                    extensions: Vec::new(),
                }
            ),
            None
        );
        assert_eq!(
            resolve_in([dir.path().to_owned()], "codex", &windows_lookup(&[".exe"])),
            Some(exe)
        );
    }

    #[test]
    fn a_windows_command_wrapper_resolves_as_cmd() {
        let dir = tempfile::tempdir().unwrap();
        let shim = dir.path().join("npx.cmd");
        write_program(&shim);
        assert_eq!(
            resolve_in(
                [dir.path().to_owned()],
                "npx",
                &windows_lookup(&[".exe", ".cmd"])
            ),
            Some(shim)
        );
    }

    #[test]
    fn an_explicit_windows_path_gains_its_extension() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("codex.exe");
        write_program(&exe);
        let without_extension = dir.path().join("codex");
        let program = without_extension.to_string_lossy().into_owned();
        assert!(is_program_path(&program));
        assert_eq!(
            resolve_path(&program, &windows_lookup(&[".exe"])),
            Some(exe)
        );
        assert_eq!(
            resolve_path(
                &program,
                &Lookup {
                    windows: false,
                    extensions: Vec::new(),
                }
            ),
            None
        );
    }

    #[test]
    fn only_the_harnesss_own_not_logged_in_is_a_sign_out() {
        // The Codex CLI exits 1 with this line when the login is gone.
        assert_eq!(status_says_signed_in(false, "Not logged in\n"), Some(false));
        assert_eq!(
            status_says_signed_in(true, "Logged in using ChatGPT"),
            Some(true)
        );
        // Another failure (an older CLI, a broken config.toml, a shim) says
        // nothing, so the credentials file decides instead.
        assert_eq!(
            status_says_signed_in(false, "error: unexpected argument 'status'"),
            None
        );
        assert_eq!(status_says_signed_in(false, ""), None);
    }

    #[test]
    fn an_accounts_sign_in_is_read_by_name_and_never_by_starting_the_harness() {
        use crate::harness::config::HarnessAccount;
        let dir = tempfile::tempdir().unwrap();
        let account = |harness: &str, config_dir: &str| HarnessAccount {
            id: "paral".into(),
            harness: harness.into(),
            name: Some("Paral".into()),
            config_dir: config_dir.into(),
        };
        let codex = account("codex", &dir.path().to_string_lossy());
        assert_eq!(
            account_auth(&codex, false),
            "unknown",
            "a cheap look reads no file, so an empty Codex home is not looked at"
        );
        std::fs::write(dir.path().join("auth.json"), "{}").unwrap();
        assert_eq!(
            account_auth(&codex, false),
            "ok",
            "auth.json's presence is a sign-in, by name only"
        );
        // Claude Code reads its own credentials file the same way.
        let claude_dir = tempfile::tempdir().unwrap();
        let claude = account("claude-code", &claude_dir.path().to_string_lossy());
        assert_eq!(account_auth(&claude, false), "unknown");
        std::fs::write(claude_dir.path().join(".credentials.json"), "").unwrap();
        assert_eq!(account_auth(&claude, false), "ok");
        // A harness with no named logins answers nothing about one.
        assert_eq!(account_auth(&account("gemini", "/tmp/x"), true), "unknown");
    }
}
