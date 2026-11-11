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

/// One status probe gets a little longer than a version print: Claude Code's
/// own `claude auth status` bound in its ACP adapter is 5 s, and a cold start
/// of that CLI is slower than `--version`.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
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
/// account's directory and with the variables a thread on it would drop
/// (`codex login status`, `claude auth status --json`), which is authoritative
/// about a sign-in that expired behind a credentials file and prints no
/// secret. Only an explicit answer counts as a sign-out: Codex's own
/// `Not logged in`, or Claude's `loggedIn: false` with no API key source or
/// third-party backend. Any other failure (an older CLI without the command, a
/// broken `config.toml`, a shim) is no answer, and the login's own credentials
/// file is looked for by name instead (`.credentials.json`, Codex's
/// `auth.json`), never opened. On macOS Claude Code may keep its sign-in in a
/// Keychain item named after the directory instead, which is asked for
/// existence only. An account on the harness's default directory is the login
/// with no variable set, so it reads as that login does. Nothing is started as
/// a session, and the engine itself reads no token: the CLI reports its own
/// state and prints none. `ok` is a sign-in found, `needs_login` one looked
/// for and not found (only after `probe`), `unknown` not looked for.
pub(crate) fn account_auth(account: &super::config::HarnessAccount, probe: bool) -> &'static str {
    account_auth_with(account, probe, run_quick_env)
}

/// [`account_auth`], with the probe runner passed in so fixture tests can
/// answer for a CLI without starting one.
fn account_auth_with(
    account: &super::config::HarnessAccount,
    probe: bool,
    run: Probe,
) -> &'static str {
    let Some(layout) = super::config::account_layout(&account.harness) else {
        return "unknown";
    };
    let dir = account.resolved_dir();
    let status = super::known(&account.harness).and_then(|known| known.auth_probe);
    if probe && let Some(status) = status {
        let env = super::config::login_env(&account.harness, &dir);
        let env: Vec<(&str, &std::path::Path)> = env
            .as_ref()
            .map(|(name, value)| (*name, std::path::Path::new(value)))
            .into_iter()
            .collect();
        let (program, args) = status.argv();
        // A named account is its own login: the variables RustCode drops for
        // its threads would otherwise answer for it (an API key in the
        // engine's environment is not this account's sign-in).
        if let Some((success, stdout, stderr)) = run(program, args, &env, layout.drop_env)
            && let Some(signed_in) = probe_answer(status, success, &stdout, &stderr)
        {
            return if signed_in { "ok" } else { "needs_login" };
        }
    }
    if dir.join(layout.marker).is_file() {
        return "ok";
    }
    if !probe {
        return "unknown";
    }
    // Claude Code on macOS keeps its sign-in in a Keychain item, not the file:
    // ask only whether it exists, and ask it even when the status command could
    // not answer (a refused read is not the user signing out).
    if layout.keychain {
        let found = if super::config::is_default_dir(&account.harness, &dir) {
            crate::provider_auth::claude_usage::default_keychain_item_exists()
        } else {
            crate::provider_auth::claude_usage::keychain_item_exists(&dir)
        };
        if found {
            return "ok";
        }
    }
    "needs_login"
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

/// What one probe's output says about the sign-in: `Some(true)` signed in,
/// `Some(false)` signed out, `None` no answer we can trust.
fn probe_answer(
    probe: super::AuthProbe,
    success: bool,
    stdout: &str,
    stderr: &str,
) -> Option<bool> {
    match probe {
        super::AuthProbe::Text(_) => {
            let mut output = String::from(stdout);
            output.push('\n');
            output.push_str(stderr);
            status_says_signed_in(success, &output)
        }
        super::AuthProbe::Json(_) => json_says_signed_in(stdout),
    }
}

/// What `claude auth status --json` says about the sign-in. `loggedIn` alone is
/// the claude.ai login; a third-party backend (Bedrock, Vertex, Foundry) keeps
/// its credentials outside the CLI, and an API key signs in without one, so
/// both count as signed in. `None` when the output is not that JSON: an older
/// CLI without the command is no answer, never a sign-out.
fn json_says_signed_in(stdout: &str) -> Option<bool> {
    let text = stdout.trim();
    // A warning line before the object must not hide it; `claude auth status`
    // also prints nothing else on stdout.
    let text = match (text.find('{'), text.rfind('}')) {
        (Some(start), Some(end)) if start < end => &text[start..=end],
        _ => return None,
    };
    let status: serde_json::Value = serde_json::from_str(text).ok()?;
    let logged_in = status.get("loggedIn")?.as_bool()?;
    let elsewhere = status
        .get("apiProvider")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|provider| provider != "firstParty");
    let api_key = status
        .get("apiKeySource")
        .is_some_and(|source| !source.is_null());
    Some(logged_in || elsewhere || api_key)
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
    detect_uncached_with(harness, run_quick_env)
}

/// [`detect_uncached`], with the probe runner passed in so fixture tests can
/// answer for a CLI without starting one.
fn detect_uncached_with(harness: &ResolvedHarness, run: Probe) -> Detection {
    let Some(program) = harness.command.first() else {
        return Detection {
            error: Some("no command configured".to_owned()),
            ..Detection::default()
        };
    };
    if resolve_program(program).is_none() {
        return Detection::default();
    }
    // A command that resolves is not always a working install: `pi-acp` runs
    // under npm's `npx` but spawns `pi` itself, so a machine with `npx` and no
    // `pi` must read as Not installed here, not fail at the first turn.
    if let Some(required) = harness.known.and_then(|known| known.requires)
        && resolve_program(required).is_none()
    {
        return Detection::default();
    }
    let mut detection = Detection {
        installed: true,
        ..Detection::default()
    };
    if let Some(known) = harness.known {
        if let Some((program, args)) = known.version
            && resolve_program(program).is_some()
            && let Some((true, stdout, stderr)) = run(program, args, &[], &[])
        {
            detection.version = parse_version(&format!("{stdout}\n{stderr}"));
        }
        if let Some(probe) = known.auth_probe {
            let (program, args) = probe.argv();
            if let Some((success, stdout, stderr)) = run(program, args, &[], &[])
                && let Some(signed_in) = probe_answer(probe, success, &stdout, &stderr)
            {
                detection.auth = if signed_in { "ok" } else { "needs_login" };
            }
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

/// Runs one status probe: whether it succeeded, its stdout and its stderr, or
/// `None` when it could not start, hung, or had to be killed.
type Probe = fn(&str, &[&str], &[(&str, &Path)], &[&str]) -> Option<(bool, String, String)>;

/// Run `program args` with no input, for at most a few seconds. Returns
/// whether it succeeded, its stdout and its stderr. `env` adds variables and
/// `remove` drops them, for a harness status command that has to look at one
/// login's own directory.
fn run_quick_env(
    program: &str,
    args: &[&str],
    env: &[(&str, &Path)],
    remove: &[&str],
) -> Option<(bool, String, String)> {
    let path = resolve_program(program)?;
    let mut command = crate::platform::command(path);
    command
        .args(args)
        .env("PATH", search_path())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    // A probe answers for the user, not for whichever project the engine
    // happens to sit in: `claude auth status` reads the cwd's own settings
    // (`apiKeyHelper` included).
    if let Some(cwd) = probe_cwd() {
        command.current_dir(cwd);
    }
    for (name, value) in env {
        command.env(name, value);
    }
    for name in remove {
        command.env_remove(name);
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
    Some((
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    ))
}

/// The directory a probe runs in: the user's home, so a project's own settings
/// in the engine's cwd cannot decide a harness-wide answer. `None` inherits
/// the engine's cwd when neither variable names an existing home.
fn probe_cwd() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|home| home.is_dir())
}

/// The `PATH` a harness is looked up on and started with: the engine's own,
/// then the login shell's, so an app started from the Dock finds the same
/// commands as the terminal, then the well-known install directories the
/// shell adds (Homebrew's prefixes, `path_helper`'s files, `~/.local/bin`,
/// `~/.bun/bin`, `~/.cargo/bin`).
///
/// The last group matters on its own: when the shell probe fails or times out
/// -- likely at app launch, or with several harness checks starting at once --
/// the login shell's `PATH` is missing entirely, and the dotfile fallback does
/// not mention `/opt/homebrew/bin`. A `brew`-installed harness is then not
/// found at all, and one found through `npx` cannot start the binary it
/// spawns. Well-known directories are appended rather than prepended, so the
/// process's own order still wins, and Windows is left as it was.
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
    crate::shell_env::add_well_known_path_dirs(&mut dirs);
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
    use super::super::{AuthProbe, KnownHarness};
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
            env: Vec::new(),
            enabled: true,
            known: None,
        };
        let detection = detect(&harness);
        assert!(!detection.installed);
        assert_eq!(detection.auth, "unknown");
        assert!(resolve_program("sh").is_some());
    }

    /// A harness whose command resolves but whose prerequisite does not, as
    /// `npx -y pi-acp` is without `pi`.
    static WITH_PREREQUISITE: KnownHarness = KnownHarness {
        id: "prerequisite",
        name: "Prerequisite",
        kind: HarnessKind::Acp,
        command: &[],
        login_command: None,
        install_hint: None,
        version: None,
        auth_probe: None,
        requires: Some("rustcode-no-such-prerequisite-on-path"),
    };

    #[test]
    fn a_harness_whose_prerequisite_is_missing_is_not_installed() {
        // `npx` starts `pi-acp`, and `pi-acp` cannot start the `pi` it spawns:
        // the list has to say Not installed instead of every turn failing.
        let dir = tempfile::tempdir().unwrap();
        let shim = dir.path().join("rustcode-harness-shim-for-a-prerequisite");
        write_program(&shim);
        let harness = ResolvedHarness {
            id: "prerequisite".into(),
            name: "Prerequisite".into(),
            kind: HarnessKind::Acp,
            command: vec![shim.to_string_lossy().into_owned()],
            env: Vec::new(),
            enabled: false,
            known: None,
        };
        assert!(
            detect_uncached_with(&harness, no_probe_answer).installed,
            "the command itself resolves"
        );
        let required = ResolvedHarness {
            known: Some(&WITH_PREREQUISITE),
            ..harness
        };
        assert!(!detect_uncached_with(&required, no_probe_answer).installed);
        // The case the report was about.
        assert_eq!(crate::harness::known("pi").unwrap().requires, Some("pi"));
    }

    #[test]
    fn a_failed_shell_probe_still_reaches_the_well_known_install_dirs() {
        // The bug: the launch-time login-shell probe lost, the dotfile fallback
        // has no `/opt/homebrew/bin`, so `codex` read as Not installed and Pi
        // could not start the `pi` next to it. Composed from a bare PATH and no
        // probe answer, the well-known directories are what is left.
        let home = tempfile::tempdir().unwrap();
        let local_bin = home.path().join(".local/bin");
        std::fs::create_dir_all(&local_bin).unwrap();
        let installed = local_bin.join("rustcode-harness-installed-by-hand");
        write_program(&installed);

        let mut dirs = vec![PathBuf::from("/usr/bin"), PathBuf::from("/bin")];
        crate::shell_env::extend_with_well_known_dirs(
            &mut dirs,
            crate::shell_env::PathFlavour::MacOs,
            Some(home.path()),
            Some("/usr/bin\n/bin\n/usr/sbin\n"),
            &[String::from("# path_helper file\n/opt/homebrew/bin\n")],
        );
        assert_eq!(
            dirs[..2],
            [PathBuf::from("/usr/bin"), PathBuf::from("/bin")]
        );
        for expected in ["/opt/homebrew/bin", "/opt/homebrew/sbin", "/usr/local/bin"] {
            assert!(dirs.contains(&PathBuf::from(expected)), "{expected}");
        }
        assert!(dirs.contains(&local_bin));
        assert_eq!(
            dirs.iter()
                .filter(|dir| **dir == PathBuf::from("/usr/bin"))
                .count(),
            1,
            "a directory on PATH already is not added twice"
        );
        assert_eq!(
            resolve_in(
                dirs,
                "rustcode-harness-installed-by-hand",
                &Lookup {
                    windows: false,
                    extensions: Vec::new(),
                }
            ),
            Some(installed),
            "the composed list is what a lookup walks"
        );
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
    fn claude_json_status_decides_without_its_exit_code() {
        assert_eq!(
            json_says_signed_in(r#"{"loggedIn": true, "authMethod": "claude.ai"}"#),
            Some(true)
        );
        assert_eq!(
            json_says_signed_in(r#"{"loggedIn": false, "authMethod": "none"}"#),
            Some(false)
        );
        // A key signs in without the claude.ai login, which is what the
        // tester's `authMethod: none` case lacked.
        assert_eq!(
            json_says_signed_in(
                r#"{"loggedIn": true, "authMethod": "api_key", "apiKeySource": "ANTHROPIC_API_KEY"}"#
            ),
            Some(true)
        );
        // A third-party backend keeps its credentials outside the CLI, so
        // `loggedIn: false` there is not a sign-out.
        for provider in ["bedrock", "vertex", "foundry", "gateway"] {
            let status = format!(r#"{{"loggedIn": false, "apiProvider": "{provider}"}}"#);
            assert_eq!(json_says_signed_in(&status), Some(true), "{provider}");
        }
        // Not the shape we know -- an older CLI, a broken one, a shim -- is no
        // answer, never a sign-out.
        assert_eq!(json_says_signed_in(""), None);
        assert_eq!(json_says_signed_in("error: unknown command 'status'"), None);
        assert_eq!(json_says_signed_in(r#"{"authMethod": "none"}"#), None);
        // A warning line before the object must not hide it.
        assert_eq!(
            json_says_signed_in("warning: something\n{\"loggedIn\": false}\n"),
            Some(false)
        );
    }

    #[test]
    fn a_probe_without_an_answer_is_not_a_sign_out() {
        let claude = AuthProbe::Json(&["claude", "auth", "status", "--json"]);
        assert_eq!(probe_answer(claude, false, "not json", ""), None);
        assert_eq!(
            probe_answer(claude, false, r#"{"loggedIn": false}"#, ""),
            Some(false)
        );
        // Codex keeps its text rule: only its own line is a sign-out.
        let codex = AuthProbe::Text(&["codex", "login", "status"]);
        assert_eq!(
            probe_answer(codex, false, "", "Not logged in\n"),
            Some(false)
        );
        assert_eq!(
            probe_answer(codex, true, "Logged in using ChatGPT", ""),
            Some(true)
        );
        assert_eq!(
            probe_answer(codex, false, "error: unexpected argument 'status'", ""),
            None
        );
    }

    #[test]
    fn the_known_probes_ask_their_own_status_commands() {
        let codex = crate::harness::known("codex").unwrap().auth_probe.unwrap();
        assert_eq!(codex.argv(), ("codex", &["login", "status"][..]));
        let claude = crate::harness::known("claude-code")
            .unwrap()
            .auth_probe
            .unwrap();
        assert_eq!(claude.argv(), ("claude", &["auth", "status", "--json"][..]));
        assert!(matches!(claude, AuthProbe::Json(_)));
    }

    /// A probe fixture: Claude Code's `auth status --json` answering signed
    /// out, as the tester's Windows profile did.
    type ProbeAnswer = Option<(bool, String, String)>;
    fn claude_signed_out(
        _program: &str,
        _args: &[&str],
        _env: &[(&str, &Path)],
        _remove: &[&str],
    ) -> ProbeAnswer {
        Some((
            false,
            r#"{"loggedIn": false, "authMethod": "none", "apiProvider": "firstParty"}"#.to_owned(),
            String::new(),
        ))
    }

    /// The same answer signed in, with a failing exit code on purpose: the JSON
    /// decides, not the status.
    fn claude_signed_in(
        _program: &str,
        _args: &[&str],
        _env: &[(&str, &Path)],
        _remove: &[&str],
    ) -> ProbeAnswer {
        Some((
            false,
            r#"{"loggedIn": true, "authMethod": "claude.ai"}"#.to_owned(),
            String::new(),
        ))
    }

    /// A probe that could not start or answer.
    fn no_probe_answer(
        _program: &str,
        _args: &[&str],
        _env: &[(&str, &Path)],
        _remove: &[&str],
    ) -> ProbeAnswer {
        None
    }

    /// A probe fixture for a named login: it checks how that account's probe is
    /// started, then answers signed in.
    fn claude_account_probe(
        program: &str,
        args: &[&str],
        env: &[(&str, &Path)],
        remove: &[&str],
    ) -> ProbeAnswer {
        assert_eq!(program, "claude");
        assert_eq!(args, ["auth", "status", "--json"]);
        let config_dir = env
            .iter()
            .find(|(name, _)| *name == "CLAUDE_CONFIG_DIR")
            .map(|(_, value)| *value)
            .expect("a named account is probed with its own directory");
        assert!(config_dir.is_dir());
        // The engine's own keys are not this login's sign-in.
        for dropped in [
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "CLAUDE_CODE_OAUTH_TOKEN",
        ] {
            assert!(remove.contains(&dropped), "{dropped} is dropped");
        }
        Some((
            true,
            r#"{"loggedIn": true, "authMethod": "claude.ai"}"#.to_owned(),
            String::new(),
        ))
    }

    #[test]
    fn a_named_claude_account_is_read_by_its_own_status_command() {
        use crate::harness::config::HarnessAccount;
        let dir = tempfile::tempdir().unwrap();
        let account = HarnessAccount {
            id: "paral".into(),
            harness: "claude-code".into(),
            name: Some("Paral".into()),
            config_dir: dir.path().to_string_lossy().into_owned(),
        };
        // A cheap look runs nothing, so it says nothing.
        assert_eq!(
            account_auth_with(&account, false, claude_account_probe),
            "unknown"
        );
        assert_eq!(
            account_auth_with(&account, true, claude_account_probe),
            "ok"
        );
        assert_eq!(
            account_auth_with(&account, true, claude_signed_out),
            "needs_login"
        );
    }

    #[test]
    fn a_status_command_that_cannot_answer_falls_back_to_the_credentials_file() {
        use crate::harness::config::HarnessAccount;
        let account = |harness: &str, dir: &Path| HarnessAccount {
            id: "paral".into(),
            harness: harness.into(),
            name: None,
            config_dir: dir.to_string_lossy().into_owned(),
        };
        // Claude Code: the file by name, never opened.
        let signed_in = tempfile::tempdir().unwrap();
        std::fs::write(signed_in.path().join(".credentials.json"), "{}").unwrap();
        assert_eq!(
            account_auth_with(
                &account("claude-code", signed_in.path()),
                true,
                no_probe_answer
            ),
            "ok",
            "an older CLI without the command still reads the file by name"
        );
        // Codex has no Keychain item, so no file and no answer is a look that
        // found no sign-in.
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(
            account_auth_with(&account("codex", empty.path()), true, no_probe_answer),
            "needs_login"
        );
    }

    #[cfg(unix)]
    #[test]
    fn claude_detection_reports_the_status_its_json_answers() {
        static FIXTURE: KnownHarness = KnownHarness {
            id: "fixture",
            name: "Fixture",
            kind: HarnessKind::Acp,
            command: &["sh"],
            login_command: None,
            install_hint: None,
            version: None,
            auth_probe: Some(AuthProbe::Json(&["claude", "auth", "status", "--json"])),
            requires: None,
        };
        let harness = ResolvedHarness {
            id: "fixture".into(),
            name: "Fixture".into(),
            kind: HarnessKind::Acp,
            command: vec!["sh".into()],
            env: Vec::new(),
            enabled: false,
            known: Some(&FIXTURE),
        };
        let signed_out = detect_uncached_with(&harness, claude_signed_out);
        assert!(signed_out.installed);
        assert_eq!(signed_out.auth, "needs_login");
        assert_eq!(
            detect_uncached_with(&harness, claude_signed_in).auth,
            "ok",
            "the JSON decides, not the exit code"
        );
        assert_eq!(
            detect_uncached_with(&harness, no_probe_answer).auth,
            "unknown"
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
