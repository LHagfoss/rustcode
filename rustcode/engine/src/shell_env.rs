//! Login/interactive shell environment fallback for provider API keys.
//!
//! Problem: `std::env::var` only sees the environment RustCode inherited at
//! startup. Keys exported in `~/.zshrc` (interactive-only) are invisible when
//! RustCode is launched from a desktop entry, systemd unit, tmux server, IDE,
//! or any non-interactive context — and Arch zsh setups commonly keep exports
//! in `~/.zshrc` while macOS setups often have them in `~/.zprofile` (login)
//! or the inherited GUI session, which is why "works on my Mac, not on Arch"
//! happens.
//!
//! Fix: [`env_var`] checks the process environment first (fast path, always
//! wins), then falls back to a cached login+interactive shell probe
//! (`$SHELL -l -i -c printenv`), then to a static parse of common dotfiles.
//! Values are never logged — only presence/source is reported for diagnostics,
//! and one `shell_env.probe` op event records whether the probe answered.
//!
//! A probe that failed is not the last word: the dotfile fallback misses what
//! the shell itself adds (macOS's `path_helper`, a package manager's init in
//! `.zshrc`), and a launch-time probe can lose to a cold start, so
//! [`retry_failed_probe`] looks again when a Check asks for it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

/// A launch-time probe must never stall startup.
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
/// A redetect is a deliberate "look again" and can afford to wait longer.
const RETRY_PROBE_TIMEOUT: Duration = Duration::from_secs(10);
/// Env marker set on the probe child so heavy dotfiles can early-return.
const PROBE_MARKER: &str = "RUSTCODE_SHELL_PROBE";

/// Where a variable value came from. Used by `doctor` and debugging.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvSource {
    Process,
    LoginShell,
    Dotfile,
    Missing,
}

impl std::fmt::Display for EnvSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EnvSource::Process => write!(f, "process env"),
            EnvSource::LoginShell => write!(f, "login shell"),
            EnvSource::Dotfile => write!(f, "shell dotfile"),
            EnvSource::Missing => write!(f, "missing"),
        }
    }
}

/// The install directories a shell on this machine adds that a `PATH`
/// inherited from a Dock launch can lack. Homebrew's prefixes and the
/// per-user bin directories are not in launchd's `PATH`, and no dotfile need
/// mention them: on macOS `/usr/libexec/path_helper` (run from
/// `/etc/profile`, which only a login shell reads) adds `/etc/paths` and
/// `/etc/paths.d/*`.
///
/// Named as a value rather than read from `cfg!` at use sites so a test
/// answers the same on every host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PathFlavour {
    MacOs,
    Unix,
    Windows,
}

impl PathFlavour {
    pub(crate) fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Unix
        }
    }
}

/// The directories to try after whatever `PATH` already holds, hardest hit
/// first: Homebrew's Apple-silicon prefix, then its Intel and classic
/// `/usr/local` one, then what `path_helper` lists, then the per-user bins
/// `cargo`, `bun` and `pip --user` install into. `etc_paths` and `etc_paths_d`
/// are the contents of `/etc/paths` and of each `/etc/paths.d/*` file, read by
/// the caller so this stays pure. Windows has no such convention and adds
/// nothing.
pub(crate) fn well_known_dirs(
    flavour: PathFlavour,
    home: Option<&Path>,
    etc_paths: Option<&str>,
    etc_paths_d: &[String],
) -> Vec<PathBuf> {
    if flavour == PathFlavour::Windows {
        return Vec::new();
    }
    let mut dirs = Vec::new();
    if flavour == PathFlavour::MacOs {
        dirs.extend(
            ["/opt/homebrew/bin", "/opt/homebrew/sbin", "/usr/local/bin"].map(PathBuf::from),
        );
        for text in std::iter::once(etc_paths)
            .flatten()
            .chain(etc_paths_d.iter().map(String::as_str))
        {
            dirs.extend(path_file_dirs(text));
        }
    }
    if let Some(home) = home {
        dirs.extend([".local/bin", ".bun/bin", ".cargo/bin"].map(|suffix| home.join(suffix)));
    }
    dirs
}

/// [`well_known_dirs`] appended to `dirs`, each only when `dirs` does not
/// already name it, so the entries the process inherited keep their
/// precedence and no directory is tried twice.
pub(crate) fn extend_with_well_known_dirs(
    dirs: &mut Vec<PathBuf>,
    flavour: PathFlavour,
    home: Option<&Path>,
    etc_paths: Option<&str>,
    etc_paths_d: &[String],
) {
    for dir in well_known_dirs(flavour, home, etc_paths, etc_paths_d) {
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
}

/// [`extend_with_well_known_dirs`] with this host's own inputs: `$HOME` and,
/// on macOS, the `path_helper` files read once per process (they do not change
/// while it runs).
pub(crate) fn add_well_known_path_dirs(dirs: &mut Vec<PathBuf>) {
    let (etc_paths, etc_paths_d) = path_helper_files();
    let home = std::env::var_os("HOME").map(PathBuf::from);
    extend_with_well_known_dirs(
        dirs,
        PathFlavour::current(),
        home.as_deref(),
        etc_paths.as_deref(),
        etc_paths_d,
    );
}

/// The directory lines of a `path_helper` file: one per line, blank lines and
/// comments skipped, and only absolute paths (a line is not shell code).
fn path_file_dirs(text: &str) -> impl Iterator<Item = PathBuf> + '_ {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
}

/// `/etc/paths` and the contents of `/etc/paths.d/*`, read in the order
/// `path_helper` reads them (that directory's files by name).
fn path_helper_files() -> &'static (Option<String>, Vec<String>) {
    static FILES: OnceLock<(Option<String>, Vec<String>)> = OnceLock::new();
    FILES.get_or_init(|| {
        if PathFlavour::current() != PathFlavour::MacOs {
            return (None, Vec::new());
        }
        let mut files: Vec<PathBuf> = std::fs::read_dir("/etc/paths.d")
            .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
            .unwrap_or_default();
        files.sort();
        let contents = files
            .iter()
            .filter_map(|file| std::fs::read_to_string(file).ok())
            .collect();
        (std::fs::read_to_string("/etc/paths").ok(), contents)
    })
}

/// Fast path + fallback lookup. Process env always wins so explicit
/// `FOO=bar rustcode` overrides and test harnesses keep working.
pub fn env_var(name: &str) -> Option<String> {
    if let Ok(val) = std::env::var(name)
        && !val.trim().is_empty()
    {
        return Some(val);
    }
    shell_import()
        .vars
        .get(name)
        .cloned()
        .filter(|v| !v.trim().is_empty())
}

/// The login shell's value of `name`, ignoring the process environment.
/// External harness commands are looked up on this `PATH` too: an app
/// launched from the Dock inherits a much shorter one than the terminal.
#[cfg_attr(test, allow(dead_code))]
pub(crate) fn login_shell_var(name: &str) -> Option<String> {
    shell_import()
        .vars
        .get(name)
        .cloned()
        .filter(|value| !value.trim().is_empty())
}

/// Like [`env_var`] but reports where the value came from (never the value).
pub fn env_var_source(name: &str) -> EnvSource {
    if let Ok(val) = std::env::var(name)
        && !val.trim().is_empty()
    {
        return EnvSource::Process;
    }
    let cache = shell_import();
    if cache.vars.contains_key(name) {
        if cache.probed {
            EnvSource::LoginShell
        } else {
            EnvSource::Dotfile
        }
    } else {
        EnvSource::Missing
    }
}

/// Copy shell-provided values for `names` into this process when the process
/// itself does not define them. This makes child processes (MCP servers,
/// tool shells) inherit keys even when RustCode was started outside the
/// user's interactive shell. Call once at startup.
pub fn hydrate_missing(names: &[&str]) {
    let cache = shell_import();
    for name in names {
        if std::env::var_os(name).is_some() {
            continue;
        }
        if let Some(val) = cache.vars.get(*name)
            && !val.trim().is_empty()
        {
            // SAFETY: single-threaded startup hydration before worker threads
            // that read env are spawned; no concurrent `getenv` mutation.
            unsafe { std::env::set_var(name, val) };
        }
    }
}

/// Collect candidate env names from profiles plus well-known provider keys.
pub fn hydrate_provider_keys(configured: &[String]) {
    const KNOWN: &[&str] = &[
        "OPENAI_API_KEY",
        "ANTHROPIC_API_KEY",
        "GEMINI_API_KEY",
        "GOOGLE_API_KEY",
        "OPENCODE_API_KEY",
        "TINKER_API_KEY",
        "EXA_API_KEY",
        "TAVILY_API_KEY",
        "OPENROUTER_API_KEY",
        "GROQ_API_KEY",
        "MISTRAL_API_KEY",
        "DEEPSEEK_API_KEY",
        "XAI_API_KEY",
        "TOGETHER_API_KEY",
        "FIREWORKS_API_KEY",
        "CEREBRAS_API_KEY",
        "AZURE_OPENAI_API_KEY",
    ];
    let mut names: Vec<&str> = KNOWN.to_vec();
    // Leak is acceptable here: startup-only, tiny, avoids lifetime plumbing.
    for extra in configured {
        if !extra.trim().is_empty() && !names.contains(&extra.as_str()) {
            names.push(Box::leak(extra.clone().into_boxed_str()));
        }
    }

    // An interactive launch already inherited the environment from the
    // user's shell in the common case. Avoid starting another interactive
    // shell before the TUI owns the terminal when all configured providers
    // are already available. Apart from unnecessary startup work, an
    // interactive probe can briefly manipulate the controlling terminal's
    // process group on some macOS terminals.
    if !configured.is_empty()
        && configured
            .iter()
            .all(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()))
    {
        return;
    }

    hydrate_missing(&names);
}

/// Import every login-shell variable this process lacks. For launches that
/// inherit no shell environment at all, such as the macOS app starting the
/// engine with `--acp`: MCP servers and tool shells read variables (e.g.
/// `DAILY_DEV_API_KEY`) that no profile or `[mcp_servers.env]` names, so the
/// provider-key list in [`hydrate_provider_keys`] cannot cover them.
pub fn hydrate_all_missing() {
    let cache = shell_import();
    for (name, val) in missing_shell_vars(&cache.vars, |name| std::env::var_os(name).is_some()) {
        // SAFETY: startup hydration before the session spawns tasks that
        // read the environment; see `hydrate_missing`.
        unsafe { std::env::set_var(name, val) };
    }
}

fn missing_shell_vars<'a>(
    shell: &'a HashMap<String, String>,
    present: impl Fn(&str) -> bool,
) -> Vec<(&'a str, &'a str)> {
    let mut vars: Vec<_> = shell
        .iter()
        .filter(|(name, val)| inherit_from_shell(name) && !val.trim().is_empty() && !present(name))
        .map(|(name, val)| (name.as_str(), val.as_str()))
        .collect();
    vars.sort_unstable();
    vars
}

/// Variables describing the probe shell's own session, not the user's setup.
fn inherit_from_shell(name: &str) -> bool {
    const SESSION: &[&str] = &[
        "_",
        "PWD",
        "OLDPWD",
        "SHLVL",
        "TERM",
        "TERM_PROGRAM",
        "TERM_PROGRAM_VERSION",
        "TERM_SESSION_ID",
        "COLORTERM",
        "TTY",
        "PS1",
        "PS2",
        "PS3",
        "PS4",
        "PROMPT",
        "RPROMPT",
        "ZSH_AUTOSUGGEST_MANUAL_REBIND",
        PROBE_MARKER,
    ];
    !SESSION.contains(&name) && !name.starts_with("POWERLEVEL9K_") && !name.starts_with("ITERM_")
}

/// One import of the login shell's environment.
struct ShellEnv {
    vars: HashMap<String, String>,
    /// Whether a live probe produced `vars`. Without it the map is only the
    /// static dotfile fallback, which lacks what the shell itself adds and is
    /// worth retrying.
    probed: bool,
}

fn shell_cache() -> &'static Mutex<Option<Arc<ShellEnv>>> {
    static CACHE: Mutex<Option<Arc<ShellEnv>>> = Mutex::new(None);
    &CACHE
}

fn cache_lock() -> MutexGuard<'static, Option<Arc<ShellEnv>>> {
    shell_cache()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

/// The login shell's environment, imported once per process. A failed probe is
/// cached too -- a probe is slow, and every caller would otherwise pay for it
/// -- but [`retry_failed_probe`] looks again when a Check asks.
fn shell_import() -> Arc<ShellEnv> {
    if let Some(cached) = cache_lock().as_ref() {
        return Arc::clone(cached);
    }
    // Our own probe child (a dotfile re-execing rustcode): dotfiles only, and
    // no `shell_env.probe` event, which would ask this import again.
    if std::env::var_os(PROBE_MARKER).is_some() {
        let fallback = Arc::new(ShellEnv {
            vars: parse_dotfiles(),
            probed: false,
        });
        *cache_lock() = Some(Arc::clone(&fallback));
        return fallback;
    }
    // Probing under no lock: it takes seconds, and another thread asking at
    // the same time gets an answer just as good as this one.
    let (imported, outcome, elapsed) = probe_env(PROBE_TIMEOUT);
    let loaded = Arc::new(imported);
    {
        let mut slot = cache_lock();
        if let Some(cached) = slot.as_ref() {
            return Arc::clone(cached);
        }
        *slot = Some(Arc::clone(&loaded));
    }
    // Logged once the cache answers, so anything the logger asks the
    // environment for does not start another probe.
    emit_probe_event(outcome, elapsed, "launch");
    loaded
}

/// Probe the login shell again when the cached import is only the dotfile
/// fallback, with the longer timeout a deliberate Check can afford. The
/// launch-time probe can lose to a cold start, and caching that loss would
/// hide every command the shell adds for the life of the process. Returns
/// whether the cache now holds a live probe's answer.
pub(crate) fn retry_failed_probe() -> bool {
    if shell_import().probed {
        return true;
    }
    let (imported, outcome, elapsed) = probe_env(RETRY_PROBE_TIMEOUT);
    let succeeded = imported.probed;
    if succeeded {
        *cache_lock() = Some(Arc::new(imported));
    }
    emit_probe_event(outcome, elapsed, "retry");
    succeeded
}

/// Run the probe once and merge its answer over the static dotfile fallback:
/// the live shell wins, and the dotfiles fill the gaps a running shell did not
/// source (a file the shell skips, or one from another shell).
fn probe_env(timeout: Duration) -> (ShellEnv, ProbeOutcome, Duration) {
    let started = Instant::now();
    let (answer, outcome) = probe_login_shell(timeout);
    let mut vars = parse_dotfiles();
    let answer = answer.filter(|answer| !answer.is_empty());
    if let Some(answer) = answer {
        vars.extend(answer);
    }
    let imported = ShellEnv {
        vars,
        probed: outcome == ProbeOutcome::Ok,
    };
    (imported, outcome, started.elapsed())
}

/// Report what the probe answered and how long it took, so a report about a
/// missing command has evidence instead of a guess.
fn emit_probe_event(outcome: ProbeOutcome, elapsed: Duration, phase: &str) {
    crate::logger::operational_event(
        "shell_env.probe",
        serde_json::json!({
            "outcome": outcome.label(),
            "duration_ms": elapsed.as_millis() as u64,
            "phase": phase,
        }),
    );
}

fn user_shell() -> String {
    std::env::var("SHELL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "/bin/sh".to_string())
}

/// See [`probe_login_shell`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProbeOutcome {
    /// The shell answered with an environment.
    Ok,
    /// It did not answer within the timeout and was killed.
    Timeout,
    /// It exited non-zero, could not be started, or printed no variables.
    Failed,
}

impl ProbeOutcome {
    fn label(self) -> &'static str {
        match self {
            ProbeOutcome::Ok => "ok",
            ProbeOutcome::Timeout => "timeout",
            ProbeOutcome::Failed => "failed",
        }
    }
}

/// What one probe attempt produced.
enum Attempt {
    Env(HashMap<String, String>),
    /// The attempt had to be killed at the timeout.
    TimedOut,
    /// The shell started and answered with nothing usable, or did not start.
    Failed,
}

/// Spawn `$SHELL -l -i -c printenv` (with degraded fallbacks) and parse
/// `KEY=VALUE` lines. The whole probe, fallback attempts included, fits in
/// `timeout`, so a slow Arch zsh setup (oh-my-zsh, powerlevel10k) can never
/// hang startup however many attempts are left to try. The outcome says
/// whether the shell answered at all, or only ran out of time.
fn probe_login_shell(timeout: Duration) -> (Option<HashMap<String, String>>, ProbeOutcome) {
    let shell = user_shell();
    let deadline = Instant::now() + timeout;
    // Most to least complete sourcing. fish understands -l/-i too; plain sh
    // ignores unknown flags gracefully enough to still try.
    let attempts: &[&[&str]] = &[&["-l", "-i"], &["-i"], &["-l"], &[]];
    let mut timed_out = false;
    for flags in attempts {
        let Some(left) = probe_budget(deadline) else {
            break;
        };
        match probe_once(&shell, flags, left) {
            Attempt::Env(map) => return (Some(map), ProbeOutcome::Ok),
            Attempt::TimedOut => timed_out = true,
            Attempt::Failed => {}
        }
    }
    // Last resort: if $SHELL itself is missing/broken, try common shells.
    if shell != "/bin/sh"
        && let Some(left) = probe_budget(deadline)
        && let Attempt::Env(map) = probe_once("/bin/sh", &["-l"], left)
    {
        return (Some(map), ProbeOutcome::Ok);
    }
    let outcome = if timed_out {
        ProbeOutcome::Timeout
    } else {
        ProbeOutcome::Failed
    };
    (None, outcome)
}

/// What is left of the probe's budget, or `None` once it is spent.
fn probe_budget(deadline: Instant) -> Option<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|left| !left.is_zero())
}

fn probe_once(shell: &str, flags: &[&str], timeout: Duration) -> Attempt {
    use std::process::Stdio;

    let mut cmd = crate::platform::command(shell);
    for flag in flags {
        cmd.arg(flag);
    }
    cmd.arg("-c")
        .arg("printenv")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        // Let heavy dotfiles skip instant prompts / wizards during probes.
        .env(PROBE_MARKER, "1")
        .env("POWERLEVEL9K_DISABLE_CONFIGURATION_WIZARD", "true")
        .env("POWERLEVEL9K_INSTANT_PROMPT", "off");
    // Drop ZSH startup slowness knobs that are safe to disable for a probe.
    cmd.env("ZSH_AUTOSUGGEST_MANUAL_REBIND", "1");

    // The probe may use `-i` so it can source interactive shell config, but
    // it must never participate in the parent's terminal job-control group.
    // In particular, an interactive zsh can otherwise leave the parent TUI
    // in a background process group on macOS, causing `tcsetattr` during raw
    // mode setup to raise SIGTTOU (`suspended (tty output)`).
    #[cfg(unix)]
    unsafe {
        use std::os::unix::process::CommandExt;

        cmd.pre_exec(|| {
            // The probe child is not a process-group leader, so setsid should
            // normally succeed. If a platform rejects it, retaining the
            // existing probe fallback is safer than aborting startup.
            let _ = libc::setsid();
            Ok(())
        });
    }

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(_) => return Attempt::Failed,
    };
    let (tx, rx) = std::sync::mpsc::channel();
    let stdout_handle = child.stdout.take();
    std::thread::spawn(move || {
        use std::io::Read;
        let mut out = Vec::new();
        if let Some(mut pipe) = stdout_handle {
            let _ = pipe.read_to_end(&mut out);
        }
        let _ = tx.send(out);
    });
    let output = match rx.recv_timeout(timeout) {
        Ok(out) => out,
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return Attempt::TimedOut;
        }
    };
    let Ok(status) = child.wait() else {
        return Attempt::Failed;
    };
    if !status.success() {
        return Attempt::Failed;
    }
    let text = String::from_utf8_lossy(&output);
    let map: HashMap<String, String> = text
        .lines()
        .filter_map(|line| {
            let (k, v) = line.split_once('=')?;
            let k = k.trim();
            if valid_env_name(k) && !v.is_empty() {
                Some((k.to_string(), v.to_string()))
            } else {
                None
            }
        })
        .collect();
    if map.is_empty() {
        Attempt::Failed
    } else {
        Attempt::Env(map)
    }
}

fn valid_env_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !name.chars().next().is_some_and(|c| c.is_ascii_digit())
}

/// Static fallback when the shell cannot be spawned (minimal containers,
/// broken rc that exits non-zero under `-i`, probe timeout). Parses simple
/// assignments without executing anything.
fn parse_dotfiles() -> HashMap<String, String> {
    let mut merged = HashMap::new();
    for path in dotfile_paths() {
        if let Ok(text) = std::fs::read_to_string(&path)
            && let Ok(home) = std::env::var("HOME")
        {
            for (k, v) in parse_dotfile_text(&text, &home) {
                merged.insert(k, v);
            }
        } else if let Ok(text) = std::fs::read_to_string(&path) {
            for (k, v) in parse_dotfile_text(&text, "") {
                merged.insert(k, v);
            }
        }
    }
    merged
}

fn dotfile_paths() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let Some(home) = home else { return Vec::new() };
    // zsh honours $ZDOTDIR; Arch users with a custom layout keep their rc
    // there instead of $HOME.
    let zdotdir = std::env::var_os("ZDOTDIR")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from);
    let zdir = zdotdir.as_deref().unwrap_or(&home);

    let mut paths = vec![
        home.join(".profile"),
        home.join(".zshenv"),
        zdir.join(".zshenv"),
        home.join(".bash_profile"),
        home.join(".bash_login"),
        home.join(".zprofile"),
        zdir.join(".zprofile"),
        home.join(".bashrc"),
        home.join(".zshrc"),
        zdir.join(".zshrc"),
        home.join(".zlogin"),
        home.join(".config/fish/config.fish"),
    ];
    // Deduplicate (ZDOTDIR == HOME is the common case).
    paths.dedup();
    paths.into_iter().filter(|p| p.is_file()).collect()
}

/// Parse one dotfile's text into `(KEY, value)` pairs. Handles:
/// `export FOO=bar`, `FOO=bar`, `typeset -x FOO=bar`, `declare -x FOO=bar`,
/// `set -gx FOO bar` (fish). Skips command substitutions / backticks since
/// those require execution (covered by the live shell probe instead).
fn parse_dotfile_text(text: &str, home: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // fish: `set -gx FOO bar baz...` / `set -x FOO "bar"`
        if line.starts_with("set ") && (line.contains(" -g") || line.contains(" -x")) {
            if let Some((k, v)) = parse_fish_set(line, home) {
                out.push((k, v));
            }
            continue;
        }
        let mut rest = line;
        for prefix in ["export ", "typeset -x ", "typeset -gx ", "declare -x "] {
            if let Some(stripped) = rest.strip_prefix(prefix) {
                rest = stripped.trim();
                break;
            }
        }
        // `export FOO` without `=` carries no value here.
        let Some((key, raw_value)) = rest.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if !valid_env_name(key) {
            continue;
        }
        let mut value = raw_value.trim();
        // Strip trailing comments outside quotes (best effort).
        value = strip_trailing_comment(value);
        if value.contains("$(") || value.contains('`') {
            continue; // needs execution; live probe covers it
        }
        let unquoted = unquote_with_home(value, home);
        out.push((key.to_string(), expand_home(&unquoted, home)));
    }
    out
}

fn parse_fish_set(line: &str, home: &str) -> Option<(String, String)> {
    // Tokenize respecting simple quotes.
    let tokens = shell_split(line);
    if tokens.len() < 3 || tokens[0] != "set" {
        return None;
    }
    let mut idx = 1;
    while idx < tokens.len() && tokens[idx].starts_with('-') {
        idx += 1;
    }
    if idx >= tokens.len() {
        return None;
    }
    let key = tokens[idx].clone();
    if !valid_env_name(&key) {
        return None;
    }
    let value = tokens[idx + 1..].join(" ");
    if value.contains("$(") || value.contains('`') {
        return None;
    }
    Some((key, expand_home(&value, home)))
}

/// Minimal quote-aware splitter for fish `set` lines.
fn shell_split(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut in_token = false;
    for ch in line.chars() {
        match quote {
            Some(q) => {
                if ch == q {
                    quote = None;
                } else {
                    cur.push(ch);
                }
                in_token = true;
            }
            None => match ch {
                '\'' | '"' => {
                    quote = Some(ch);
                    in_token = true;
                }
                c if c.is_whitespace() => {
                    if in_token {
                        tokens.push(std::mem::take(&mut cur));
                        in_token = false;
                    }
                }
                _ => {
                    cur.push(ch);
                    in_token = true;
                }
            },
        }
    }
    if in_token || !cur.is_empty() {
        tokens.push(cur);
    }
    tokens
}

fn strip_trailing_comment(value: &str) -> &str {
    let mut in_single = false;
    let mut in_double = false;
    let bytes = value.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'\'' if !in_double => in_single = !in_single,
            b'"' if !in_single => in_double = !in_double,
            b'#' if !in_single && !in_double && i > 0 && bytes[i - 1].is_ascii_whitespace() => {
                // Only a comment when preceded by whitespace.
                return value[..i].trim_end();
            }
            _ => {}
        }
    }
    value
}

fn unquote_with_home(value: &str, home: &str) -> String {
    let value = value.trim();
    if value.len() >= 2 {
        let bytes = value.as_bytes();
        if (bytes[0] == b'"' && bytes[value.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[value.len() - 1] == b'\'')
        {
            return expand_inline_vars(&value[1..value.len() - 1], home);
        }
    }
    // `export PATH=$HOME/bin:$PATH` — expand the common prefixes inline.
    expand_inline_vars(value, home)
}

fn expand_home(value: &str, home: &str) -> String {
    if value == "~" {
        return home.to_string();
    }
    if let Some(rest) = value.strip_prefix("~/") {
        return format!("{home}/{rest}");
    }
    expand_inline_vars(value, home)
}

fn expand_inline_vars(value: &str, home: &str) -> String {
    let mut out = value.to_string();
    // Prefer the explicitly passed home (dotfile parse context); fall back
    // to the process HOME for direct callers.
    let home_owned;
    let home = if home.is_empty() {
        match std::env::var("HOME") {
            Ok(h) => {
                home_owned = h;
                home_owned.as_str()
            }
            Err(_) => return out,
        }
    } else {
        home
    };
    out = out.replace("$HOME", home).replace("${HOME}", home);
    if let Some(tilde) = out.strip_prefix("~/") {
        out = format!("{home}/{tilde}");
    } else if out == "~" {
        out = home.to_string();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_known_dirs_follow_the_platform() {
        let home = Path::new("/Users/me");
        // Windows keeps the `PATH` it has.
        assert!(
            well_known_dirs(PathFlavour::Windows, Some(home), Some("/usr/bin\n"), &[]).is_empty()
        );
        // Every Unix adds the per-user install points, never the macOS ones.
        assert_eq!(
            well_known_dirs(PathFlavour::Unix, Some(home), Some("/usr/bin\n"), &[]),
            vec![
                home.join(".local/bin"),
                home.join(".bun/bin"),
                home.join(".cargo/bin"),
            ]
        );
        // macOS adds Homebrew's prefixes and what `path_helper` lists: the
        // files' own order, one directory per line, comments skipped.
        assert_eq!(
            well_known_dirs(
                PathFlavour::MacOs,
                Some(home),
                Some("\n/usr/bin\n# not a directory\n/bin\n"),
                &[String::from("/usr/sbin\nrelative/dir\n")],
            ),
            vec![
                PathBuf::from("/opt/homebrew/bin"),
                PathBuf::from("/opt/homebrew/sbin"),
                PathBuf::from("/usr/local/bin"),
                PathBuf::from("/usr/bin"),
                PathBuf::from("/bin"),
                PathBuf::from("/usr/sbin"),
                home.join(".local/bin"),
                home.join(".bun/bin"),
                home.join(".cargo/bin"),
            ]
        );
        // No home to add per-user directories from is not an error.
        assert_eq!(
            well_known_dirs(PathFlavour::Unix, None, None, &[]),
            Vec::<PathBuf>::new()
        );
    }

    #[test]
    fn adding_well_known_dirs_keeps_the_existing_order_and_dedupes() {
        let home = Path::new("/Users/me");
        let mut dirs = vec![PathBuf::from("/usr/bin"), home.join(".local/bin")];
        extend_with_well_known_dirs(&mut dirs, PathFlavour::Unix, Some(home), None, &[]);
        assert_eq!(
            dirs,
            vec![
                PathBuf::from("/usr/bin"),
                home.join(".local/bin"),
                home.join(".bun/bin"),
                home.join(".cargo/bin"),
            ],
            "what the process inherited keeps its precedence, and no directory appears twice"
        );
    }

    #[test]
    fn process_env_wins_over_shell_cache() {
        // SAFETY: test-only mutation of a unique key; tests in this module
        // do not run concurrently against this name.
        unsafe { std::env::set_var("RUSTCODE_SHELL_ENV_TEST_KEY", "from-process") };
        assert_eq!(
            env_var("RUSTCODE_SHELL_ENV_TEST_KEY").as_deref(),
            Some("from-process")
        );
        unsafe { std::env::remove_var("RUSTCODE_SHELL_ENV_TEST_KEY") };
    }

    #[test]
    fn imports_missing_user_vars_but_not_shell_session_state() {
        let shell: HashMap<String, String> = [
            ("DAILY_DEV_API_KEY", "dd-123"),
            ("HOME", "/Users/me"),
            ("PWD", "/Users/me"),
            ("SHLVL", "2"),
            ("_", "/usr/bin/printenv"),
            ("TERM", "xterm-256color"),
            ("POWERLEVEL9K_INSTANT_PROMPT", "off"),
            (PROBE_MARKER, "1"),
            ("BLANK", "  "),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();

        let imported = missing_shell_vars(&shell, |name| name == "HOME");

        assert_eq!(imported, vec![("DAILY_DEV_API_KEY", "dd-123")]);
    }

    #[test]
    fn parses_common_export_forms() {
        let text = r#"
# comment
export OPENAI_API_KEY=sk-live-123
ANTHROPIC_API_KEY="sk-ant-456"  # trailing comment
typeset -x GEMINI_API_KEY='gem-789'
FOO=$(some-command)  # must be skipped (needs execution)
BAR=`other`  # skipped too
export EMPTY_OK=""
TILDE=~/keys/file
"#;
        let parsed: HashMap<String, String> =
            parse_dotfile_text(text, "/home/test").into_iter().collect();
        assert_eq!(parsed["OPENAI_API_KEY"], "sk-live-123");
        assert_eq!(parsed["ANTHROPIC_API_KEY"], "sk-ant-456");
        assert_eq!(parsed["GEMINI_API_KEY"], "gem-789");
        assert!(!parsed.contains_key("FOO"));
        assert!(!parsed.contains_key("BAR"));
        assert_eq!(parsed["TILDE"], "/home/test/keys/file");
    }

    #[test]
    fn parses_fish_set_lines() {
        let text = "set -gx OPENCODE_API_KEY oc-123\nset -x TINKER_API_KEY tink-456\n";
        let parsed: HashMap<String, String> =
            parse_dotfile_text(text, "/home/test").into_iter().collect();
        assert_eq!(parsed["OPENCODE_API_KEY"], "oc-123");
        assert_eq!(parsed["TINKER_API_KEY"], "tink-456");
    }

    #[test]
    fn rejects_invalid_names() {
        assert!(!valid_env_name("1FOO"));
        assert!(!valid_env_name("FOO-BAR"));
        assert!(valid_env_name("_FOO123"));
    }

    #[test]
    fn missing_key_reports_missing() {
        assert_eq!(
            env_var_source("RUSTCODE_DEFINITELY_MISSING_KEY_XYZ"),
            EnvSource::Missing
        );
        assert!(env_var("RUSTCODE_DEFINITELY_MISSING_KEY_XYZ").is_none());
    }
}
