//! Whether a harness is installed, its version and sign-in, found cheaply:
//! a `PATH` lookup and, where the harness has one, a quick status command
//! under a timeout. Results are cached for a minute.

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

/// Where `program` resolves: itself when it is a path, else the first
/// executable of that name on [`search_path`].
pub(crate) fn resolve_program(program: &str) -> Option<PathBuf> {
    if program.contains(std::path::MAIN_SEPARATOR) {
        let path = PathBuf::from(program);
        return is_executable(&path).then_some(path);
    }
    std::env::split_paths(&search_path())
        .filter(|dir| !dir.as_os_str().is_empty())
        .map(|dir| dir.join(program))
        .find(|candidate| is_executable(candidate))
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
