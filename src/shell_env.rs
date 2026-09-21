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
//! Values are never logged — only presence/source is reported for diagnostics.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
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

/// Fast path + fallback lookup. Process env always wins so explicit
/// `FOO=bar rustcode` overrides and test harnesses keep working.
pub fn env_var(name: &str) -> Option<String> {
    if let Ok(val) = std::env::var(name)
        && !val.trim().is_empty()
    {
        return Some(val);
    }
    shell_import()
        .get(name)
        .cloned()
        .filter(|v| !v.trim().is_empty())
}

/// Like [`env_var`] but reports where the value came from (never the value).
pub fn env_var_source(name: &str) -> EnvSource {
    if let Ok(val) = std::env::var(name)
        && !val.trim().is_empty()
    {
        return EnvSource::Process;
    }
    let cache = shell_import();
    if cache.contains_key(name) {
        if probe_succeeded() {
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
        if let Some(val) = cache.get(*name)
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

fn probe_ok() -> &'static OnceLock<bool> {
    static PROBE_OK: OnceLock<bool> = OnceLock::new();
    &PROBE_OK
}

fn probe_succeeded() -> bool {
    probe_ok().get().copied().unwrap_or(false)
}

fn shell_import() -> &'static HashMap<String, String> {
    static CACHE: OnceLock<HashMap<String, String>> = OnceLock::new();
    CACHE.get_or_init(|| {
        // Skip probing inside our own probe child (guard against recursion
        // if a dotfile re-execs rustcode).
        if std::env::var_os(PROBE_MARKER).is_some() {
            probe_ok().set(false).ok();
            return parse_dotfiles();
        }
        match probe_login_shell() {
            Some(map) if !map.is_empty() => {
                probe_ok().set(true).ok();
                // Merge: live probe wins, dotfile parse fills gaps (e.g. a
                // file the running shell did not source).
                let mut merged = parse_dotfiles();
                for (k, v) in map {
                    merged.insert(k, v);
                }
                merged
            }
            _ => {
                probe_ok().set(false).ok();
                parse_dotfiles()
            }
        }
    })
}

fn user_shell() -> String {
    std::env::var("SHELL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "/bin/sh".to_string())
}

/// Spawn `$SHELL -l -i -c printenv` (with degraded fallbacks) and parse
/// `KEY=VALUE` lines. Runs with a hard timeout so a slow Arch zsh setup
/// (oh-my-zsh, powerlevel10k) can never hang startup.
fn probe_login_shell() -> Option<HashMap<String, String>> {
    let shell = user_shell();
    // Most to least complete sourcing. fish understands -l/-i too; plain sh
    // ignores unknown flags gracefully enough to still try.
    let attempts: &[&[&str]] = &[&["-l", "-i"], &["-i"], &["-l"], &[]];
    for flags in attempts {
        if let Some(map) = probe_once(&shell, flags) {
            return Some(map);
        }
    }
    // Last resort: if $SHELL itself is missing/broken, try common shells.
    if shell != "/bin/sh"
        && let Some(map) = probe_once("/bin/sh", &["-l"])
    {
        return Some(map);
    }
    None
}

fn probe_once(shell: &str, flags: &[&str]) -> Option<HashMap<String, String>> {
    use std::process::{Command, Stdio};

    let mut cmd = Command::new(shell);
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

    let mut child = cmd.spawn().ok()?;
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
    let output = match rx.recv_timeout(PROBE_TIMEOUT) {
        Ok(out) => out,
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
    };
    let status = child.wait().ok()?;
    if !status.success() {
        return None;
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
    if map.is_empty() { None } else { Some(map) }
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
