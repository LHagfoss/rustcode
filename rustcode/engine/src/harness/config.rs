//! `[harness_defaults]` and `[[harnesses]]` in the user config
//! (`docs/harnesses.md`). Both are optional: a config without them keeps
//! working and starts every thread on the built-in harness. A harness entry's
//! optional `model` and `effort` are the last choice for a new thread on it.

use super::HarnessKind;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessDefaults {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// How long a turn on an external harness may send nothing with nothing
    /// running before it ends as failed (`watchdog`). 0 turns the deadline
    /// off; left out, it is [`super::watchdog::DEFAULT_TIMEOUT`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub silence_timeout_minutes: Option<u64>,
    /// How long a thread on an external harness keeps its process with no
    /// turn, no request waiting on the user and no client activity before the
    /// engine stops it (`session`, the idle reaper). 0 keeps every process
    /// running; left out, it is [`super::session::DEFAULT_IDLE_TIMEOUT`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_timeout_minutes: Option<u64>,
    /// The most harness processes the engine keeps running; above it the least
    /// recently used are stopped first. 0 turns the cap off; left out, it is
    /// [`super::session::DEFAULT_MAX_LIVE`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_live_harnesses: Option<usize>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub onboarded: bool,
}

/// How long a thread on an external harness may keep its process with nothing
/// to do before the engine stops it ([`super::session`]). `None` keeps every
/// process running; an unreadable config keeps the default.
pub(crate) fn idle_timeout() -> Option<Duration> {
    let minutes = config_dir()
        .ok()
        .and_then(|dir| load_in(&dir).0.idle_timeout_minutes);
    idle_from(minutes)
}

/// What `[harness_defaults].idle_timeout_minutes` means: left out is the
/// default, 0 keeps every process, anything else is that many minutes.
fn idle_from(minutes: Option<u64>) -> Option<Duration> {
    match minutes {
        None => Some(super::session::DEFAULT_IDLE_TIMEOUT),
        Some(0) => None,
        Some(minutes) => Some(Duration::from_secs(minutes.saturating_mul(60))),
    }
}

/// How many harness processes the engine keeps running
/// (`[harness_defaults].max_live_harnesses`). `None` turns the cap off.
pub(crate) fn max_live_harnesses() -> Option<usize> {
    let cap = config_dir()
        .ok()
        .and_then(|dir| load_in(&dir).0.max_live_harnesses);
    match cap {
        None => Some(super::session::DEFAULT_MAX_LIVE),
        Some(0) => None,
        Some(cap) => Some(cap),
    }
}

/// How long a turn on an external harness may send nothing with nothing
/// running before the engine ends it ([`super::watchdog`]). `None` turns the
/// deadline off; an unreadable config keeps the default.
pub(crate) fn silence_timeout() -> Option<Duration> {
    let minutes = config_dir()
        .ok()
        .and_then(|dir| load_in(&dir).0.silence_timeout_minutes);
    silence_from(minutes)
}

/// What `[harness_defaults].silence_timeout_minutes` means: left out is the
/// default, 0 turns the deadline off, anything else is that many minutes.
fn silence_from(minutes: Option<u64>) -> Option<Duration> {
    match minutes {
        None => Some(super::watchdog::DEFAULT_TIMEOUT),
        Some(0) => None,
        Some(minutes) => Some(Duration::from_secs(minutes.saturating_mul(60))),
    }
}

/// One `[[harnesses]]` entry. A known harness needs only `id` and
/// `enabled`; a custom one also names its `command`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessEntry {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<HarnessKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
    /// An entry that leaves it out is on: listing a harness means wanting it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// The model last chosen for a new thread on this harness.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The effort last chosen with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

impl HarnessEntry {
    pub fn is_enabled(&self) -> bool {
        self.enabled.unwrap_or(true)
    }
}

/// One `[[harness_accounts]]` entry: a named config directory for a harness
/// (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`), so a thread can run on another login of
/// the same tool. RustCode only names the directory; the harness keeps its own
/// sign-in there.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessAccount {
    pub id: String,
    /// The harness this account belongs to (`claude-code` or `codex`).
    pub harness: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The account's config directory, as written (`~` allowed).
    pub config_dir: String,
}

impl HarnessAccount {
    /// The name the account goes by in the app.
    pub fn display_name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
    }

    /// The directory with `~` expanded.
    pub fn resolved_dir(&self) -> PathBuf {
        PathBuf::from(expand_tilde(&self.config_dir))
    }
}

/// Expand a leading `~` with the user's home directory.
pub fn expand_tilde(path: &str) -> String {
    let Some(rest) = path.strip_prefix('~') else {
        return path.to_owned();
    };
    let Some(home) = std::env::var_os("HOME") else {
        return path.to_owned();
    };
    let mut resolved = PathBuf::from(home);
    let rest = rest.strip_prefix('/').unwrap_or(rest);
    if !rest.is_empty() {
        resolved.push(rest);
    }
    resolved.to_string_lossy().into_owned()
}

/// What `_rustcode/harnesses/accounts/add` asks for.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountChange {
    pub harness: String,
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub config_dir: Option<String>,
}

/// What `_rustcode/harnesses/set` asks for.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessChange {
    pub id: String,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub command: Option<Vec<String>>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub kind: Option<HarnessKind>,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

/// Whether `change` may apply while `defaults` hold: the built-in harness turns off only once
/// another harness starts new threads, so a new thread always has one that is on.
pub(crate) fn check_default(
    defaults: &HarnessDefaults,
    change: &HarnessChange,
) -> Result<(), String> {
    let builtin_default = defaults
        .harness
        .as_deref()
        .is_none_or(|harness| harness == super::BUILTIN_ID);
    if change.id.trim() == super::BUILTIN_ID && change.enabled == Some(false) && builtin_default {
        return Err("choose another default harness before turning RustCode off".to_owned());
    }
    Ok(())
}

/// Apply `change` to `harnesses`.
pub(crate) fn apply_change(
    harnesses: &mut Vec<HarnessEntry>,
    change: HarnessChange,
) -> Result<(), String> {
    let id = change.id.trim().to_owned();
    if !valid_id(&id) {
        return Err(format!(
            "invalid harness id '{id}': use letters, digits, '-', '_' or '.'"
        ));
    }
    if let Some(command) = &change.command
        && command
            .first()
            .is_none_or(|program| program.trim().is_empty())
    {
        return Err("command must name a program".to_owned());
    }
    let known = super::known(&id);
    if id == super::BUILTIN_ID {
        if change.command.is_some() || change.kind.is_some() {
            return Err("the built-in RustCode harness has no command".to_owned());
        }
        // Turned off, it leaves the app's pickers; the engine itself stays, as it runs every
        // harness. Only `enabled` is recorded, and only when it says off.
        if let Some(enabled) = change.enabled {
            match harnesses.iter_mut().find(|entry| entry.id == id) {
                Some(entry) => entry.enabled = (!enabled).then_some(false),
                None if !enabled => harnesses.push(HarnessEntry {
                    id,
                    enabled: Some(false),
                    ..Default::default()
                }),
                None => {}
            }
        }
        return Ok(());
    }
    if known.is_some()
        && change
            .kind
            .is_some_and(|kind| Some(kind) != known.map(|k| k.kind))
    {
        return Err(format!("the kind of '{id}' is fixed"));
    }
    if let Some(kind) = change.kind
        && kind == HarnessKind::Builtin
    {
        return Err("only RustCode is a built-in harness".to_owned());
    }
    match harnesses.iter_mut().find(|entry| entry.id == id) {
        Some(entry) => {
            if let Some(enabled) = change.enabled {
                entry.enabled = Some(enabled);
            }
            if let Some(command) = change.command {
                entry.command = Some(command);
            }
            if let Some(name) = change.name.filter(|name| !name.trim().is_empty()) {
                entry.name = Some(name);
            }
            if known.is_none()
                && let Some(kind) = change.kind
            {
                entry.kind = Some(kind);
            }
        }
        None => {
            if known.is_none() && change.command.is_none() {
                return Err(format!("a custom harness '{id}' needs a command"));
            }
            harnesses.push(HarnessEntry {
                id: id.clone(),
                name: change.name.filter(|name| !name.trim().is_empty()),
                kind: if known.is_some() {
                    None
                } else {
                    Some(change.kind.unwrap_or(HarnessKind::Acp))
                },
                command: change.command,
                enabled: Some(change.enabled.unwrap_or(true)),
                ..Default::default()
            });
        }
    }
    Ok(())
}

/// Keep `model` and `effort` as the last choice for new threads on `id`
/// (blank values clear them). A known harness without an entry gets one that
/// leaves it off, as it was; the built-in one gets an entry of its own.
pub(crate) fn remember(
    harnesses: &mut Vec<HarnessEntry>,
    id: &str,
    model: Option<String>,
    effort: Option<String>,
) -> Result<(), String> {
    let clean = |value: Option<String>| {
        value
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
    };
    let (model, effort) = (clean(model), clean(effort));
    if let Some(entry) = harnesses.iter_mut().find(|entry| entry.id == id) {
        entry.model = model;
        entry.effort = effort;
        return Ok(());
    }
    let builtin = id == super::BUILTIN_ID;
    if !builtin && super::known(id).is_none() {
        return Err(format!("unknown harness '{id}'"));
    }
    if model.is_some() || effort.is_some() {
        harnesses.push(HarnessEntry {
            id: id.to_owned(),
            enabled: (!builtin).then_some(false),
            model,
            effort,
            ..Default::default()
        });
    }
    Ok(())
}

pub(crate) fn remove(harnesses: &mut Vec<HarnessEntry>, id: &str) -> Result<(), String> {
    if id == super::BUILTIN_ID || super::known(id).is_some() {
        return Err(format!(
            "'{id}' is a built-in entry; turn it off instead of removing it"
        ));
    }
    let before = harnesses.len();
    harnesses.retain(|entry| entry.id != id);
    if harnesses.len() == before {
        return Err(format!("unknown harness '{id}'"));
    }
    Ok(())
}

/// The user config directory, or an error a client can show.
pub(crate) fn config_dir() -> Result<std::path::PathBuf, String> {
    crate::config::get_config_dir().ok_or_else(|| "no config directory".to_owned())
}

pub(crate) fn load_in(dir: &Path) -> (HarnessDefaults, Vec<HarnessEntry>) {
    let (_, _, config) = crate::config::load_config_from(dir);
    (config.harness_defaults, config.harnesses)
}

/// The user's named harness accounts (`[[harness_accounts]]`).
pub(crate) fn load_accounts_in(dir: &Path) -> Vec<HarnessAccount> {
    let (_, _, config) = crate::config::load_config_from(dir);
    config.harness_accounts
}

/// The accounts of `harness`, in config order.
pub(crate) fn accounts_of(accounts: &[HarnessAccount], harness: &str) -> Vec<HarnessAccount> {
    accounts
        .iter()
        .filter(|account| account.harness == harness)
        .cloned()
        .collect()
}

/// The account `id` of `harness`, or a message a client can show. An
/// unknown account is an error, never a quiet fall back to the default login.
pub(crate) fn find_account(
    accounts: &[HarnessAccount],
    harness: &str,
    id: &str,
) -> Result<HarnessAccount, String> {
    accounts
        .iter()
        .find(|account| account.harness == harness && account.id == id)
        .cloned()
        .ok_or_else(|| format!("unknown {harness} account '{id}'; add it in Settings › Agents"))
}

/// The account a loaded thread names, for its `_meta` and snapshot row: the
/// real entry, or a placeholder with the record's id when it was removed.
/// Loading never fails on a removed account; only a prompt then does.
pub(crate) fn account_for_load(
    accounts: &[HarnessAccount],
    record: &crate::harness::store::HarnessRecord,
) -> Option<HarnessAccount> {
    let id = record.account.as_deref()?;
    Some(
        find_account(accounts, &record.harness, id).unwrap_or_else(|_| HarnessAccount {
            id: id.to_owned(),
            harness: record.harness.clone(),
            name: None,
            config_dir: String::new(),
        }),
    )
}

/// How one harness keeps named logins: every fact that differs between Claude
/// Code and Codex, in one place, so the account code stays generic.
#[derive(Debug, Clone, Copy)]
pub(crate) struct AccountLayout {
    /// The harness this describes.
    pub harness: &'static str,
    /// The variable the harness reads its config directory from.
    pub env_var: &'static str,
    /// The directory under the home the harness's own sign-in lives in.
    pub dir_name: &'static str,
    /// The prefix a named account's directory is suggested under.
    pub dir_prefix: &'static str,
    /// The name `~/.<dir_name>` is suggested as when it is discovered.
    pub own_name: &'static str,
    /// The harness's own command that signs a login in, run by the user in a
    /// terminal (never by RustCode).
    pub login_command: &'static [&'static str],
    /// The file a login's directory holds once it is signed in, by name only:
    /// its presence is the cheap sign-in check and its contents are never read.
    pub marker: &'static str,
    /// Variables that would override the account's own sign-in.
    pub drop_env: &'static [&'static str],
    /// What makes a directory look like a login directory of this harness.
    pub looks_like: &'static [LoginEntry],
}

/// One name inside a login directory, and whether it is a file or a directory:
/// a directory called `auth.json` is not a Codex sign-in.
#[derive(Debug, Clone, Copy)]
pub(crate) enum LoginEntry {
    File(&'static str),
    Dir(&'static str),
}

const CLAUDE_ACCOUNT: AccountLayout = AccountLayout {
    harness: "claude-code",
    env_var: "CLAUDE_CONFIG_DIR",
    dir_name: ".claude",
    dir_prefix: ".claude-",
    own_name: "Claude",
    login_command: &["claude", "auth", "login"],
    marker: ".credentials.json",
    drop_env: &[
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "CLAUDE_CODE_OAUTH_TOKEN",
    ],
    looks_like: &[
        LoginEntry::File(".claude.json"),
        LoginEntry::File(".credentials.json"),
        LoginEntry::File("settings.json"),
        LoginEntry::Dir("projects"),
    ],
};

const CODEX_ACCOUNT: AccountLayout = AccountLayout {
    harness: "codex",
    env_var: "CODEX_HOME",
    dir_name: ".codex",
    dir_prefix: ".codex-",
    own_name: "Codex",
    login_command: &["codex", "login"],
    marker: "auth.json",
    drop_env: &["OPENAI_API_KEY", "CODEX_API_KEY"],
    looks_like: &[
        LoginEntry::File("config.toml"),
        LoginEntry::File("auth.json"),
        LoginEntry::Dir("sessions"),
    ],
};

/// The harnesses with named logins. Pi, OpenCode, Gemini and a custom harness
/// have none: they run on their own single sign-in.
const ACCOUNT_LAYOUTS: &[AccountLayout] = &[CLAUDE_ACCOUNT, CODEX_ACCOUNT];

/// Every harness with named logins, in Settings' order.
pub(crate) fn account_layouts() -> impl Iterator<Item = &'static AccountLayout> {
    ACCOUNT_LAYOUTS.iter()
}

/// How `harness` keeps named logins, or `None` when it has none.
pub(crate) fn account_layout(harness: &str) -> Option<&'static AccountLayout> {
    ACCOUNT_LAYOUTS
        .iter()
        .find(|layout| layout.harness == harness)
}

/// Whether `config_dir` is the harness's own default sign-in, `~/.claude` or
/// `~/.codex`.
///
/// Such an account is a normal entry, but it runs as the login with no
/// `CLAUDE_CONFIG_DIR`/`CODEX_HOME` at all: macOS keys Claude Code's Keychain
/// items on that variable, so setting it would look signed out, and Codex's
/// `~/.codex` is the login a thread on no account already runs on
/// (`harness/session.rs`).
pub(crate) fn is_default_dir(harness: &str, dir: &Path) -> bool {
    let Some(layout) = account_layout(harness) else {
        return false;
    };
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return false;
    };
    dir == home.join(layout.dir_name)
}

/// The environment that points a harness at one login's own directory: the
/// variable and its value, or `None` for the harness's own default directory,
/// which runs with no variable set (`is_default_dir`).
pub(crate) fn login_env(harness: &str, dir: &Path) -> Option<(&'static str, String)> {
    let layout = account_layout(harness)?;
    if is_default_dir(harness, dir) {
        return None;
    }
    Some((layout.env_var, dir.to_string_lossy().into_owned()))
}

/// A login directory found on this Mac that no account names yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct DiscoveredAccount {
    pub config_dir: String,
    pub suggested_name: String,
}

/// The login directories of `harness` on this Mac that no named account uses
/// yet: `~/.claude` and `~/.claude-*`, or `~/.codex` and `~/.codex-*`. A cheap
/// read of names only: no file is opened, and only a directory that looks like
/// one of its login directories counts (`looks_like`).
pub(crate) fn discovered_accounts(
    harness: &str,
    registered: &[HarnessAccount],
) -> Vec<DiscoveredAccount> {
    match std::env::var_os("HOME").map(PathBuf::from) {
        Some(home) => discovered_accounts_in(&home, harness, registered),
        None => Vec::new(),
    }
}

/// [`discovered_accounts`] against an explicit home, for tests.
pub(crate) fn discovered_accounts_in(
    home: &Path,
    harness: &str,
    registered: &[HarnessAccount],
) -> Vec<DiscoveredAccount> {
    let Some(layout) = account_layout(harness) else {
        return Vec::new();
    };
    let known: Vec<PathBuf> = registered
        .iter()
        .filter(|account| account.harness == harness)
        .map(HarnessAccount::resolved_dir)
        .collect();
    let mut found = Vec::new();
    let default = home.join(layout.dir_name);
    if looks_like_login_dir(layout, &default) {
        found.push(default);
    }
    let mut profiles: Vec<PathBuf> = std::fs::read_dir(home)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(layout.dir_prefix))
        })
        .collect();
    profiles.sort();
    found.extend(
        profiles
            .into_iter()
            .filter(|path| looks_like_login_dir(layout, path)),
    );
    found
        .into_iter()
        .filter(|path| !known.iter().any(|known| known == path))
        .map(|path| DiscoveredAccount {
            suggested_name: suggested_account_name(layout, &path),
            config_dir: tilde_path(&path, home),
        })
        .collect()
}

/// Whether `dir` looks like one of the harness's login directories, by names
/// only: no file is opened. A name counts only as the kind it is: a directory
/// named `auth.json`, or a file named `sessions`, is not a Codex home.
fn looks_like_login_dir(layout: &AccountLayout, dir: &Path) -> bool {
    dir.is_dir()
        && layout.looks_like.iter().any(|entry| match entry {
            LoginEntry::File(name) => dir.join(name).is_file(),
            LoginEntry::Dir(name) => dir.join(name).is_dir(),
        })
}

/// The name a discovered directory is suggested under: the suffix after the
/// harness's prefix, title-cased (`~/.claude-personal` is "Personal"), and the
/// harness's own name for its default directory.
fn suggested_account_name(layout: &AccountLayout, dir: &Path) -> String {
    let name = dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let suffix = name.strip_prefix(layout.dir_prefix).unwrap_or_default();
    let words: Vec<String> = suffix
        .split(['-', '_', '.'])
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect();
    if words.is_empty() {
        layout.own_name.to_owned()
    } else {
        words.join(" ")
    }
}

/// A discovered directory with `~` for the home it lives under, so the config
/// keeps the same shape as an account added by hand.
fn tilde_path(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) => format!("~/{}", rest.to_string_lossy()),
        Err(_) => path.to_string_lossy().into_owned(),
    }
}

/// Where an account's own directory goes when the client names none.
pub(crate) fn default_config_dir(layout: &AccountLayout, id: &str) -> String {
    format!("~/{}{id}", layout.dir_prefix)
}

/// Add an account to the user config. Validates it, derives
/// `~/.claude-<id>` (or the harness's own prefix) when no directory is given,
/// and creates the directory.
pub(crate) fn add_account_in(dir: &Path, change: AccountChange) -> Result<HarnessAccount, String> {
    let harness = change.harness.trim().to_owned();
    let Some(layout) = account_layout(&harness) else {
        return Err(format!(
            "{harness} has no named logins; use Claude Code or Codex"
        ));
    };
    let id = change.id.trim().to_owned();
    if !valid_id(&id) {
        return Err(format!(
            "invalid account id '{id}': use letters, digits, '-', '_' or '.'"
        ));
    }
    let name = change
        .name
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| "an account needs a name".to_owned())?;
    let config_dir = change
        .config_dir
        .map(|dir| dir.trim().to_owned())
        .filter(|dir| !dir.is_empty())
        .unwrap_or_else(|| default_config_dir(layout, &id));
    let account = HarnessAccount {
        id,
        harness,
        name: Some(name),
        config_dir,
    };
    let resolved = account.resolved_dir();
    if !resolved.is_absolute() {
        return Err("the account's config directory must be an absolute path".to_owned());
    }
    crate::config::edit_user_config_in(dir, |config| {
        if let Some(existing) = config
            .harness_accounts
            .iter()
            .find(|existing| existing.harness == account.harness && existing.id == account.id)
        {
            return Err(format!(
                "there is already a {} account '{}'",
                existing.harness, existing.id
            ));
        }
        if let Some(existing) = config
            .harness_accounts
            .iter()
            .find(|existing| existing.resolved_dir() == resolved)
        {
            return Err(format!(
                "{} already uses {}",
                existing.display_name(),
                existing.config_dir
            ));
        }
        config.harness_accounts.push(account.clone());
        Ok(())
    })?;
    std::fs::create_dir_all(&resolved)
        .map_err(|error| format!("could not create {}: {error}", resolved.display()))?;
    Ok(account)
}

/// Remove an account from the user config. The directory stays on disk: it
/// holds the login and the conversations.
pub(crate) fn remove_account_in(
    dir: &Path,
    harness: &str,
    id: &str,
) -> Result<HarnessAccount, String> {
    crate::config::edit_user_config_in(dir, |config| {
        config
            .harness_accounts
            .iter()
            .position(|account| account.harness == harness && account.id == id)
            .map(|at| config.harness_accounts.remove(at))
            .ok_or_else(|| format!("unknown {harness} account '{id}'"))
    })
}

pub(crate) fn edit_in<T>(
    dir: &Path,
    edit: impl FnOnce(&mut HarnessDefaults, &mut Vec<HarnessEntry>) -> Result<T, String>,
) -> Result<T, String> {
    crate::config::edit_user_config_in(dir, |config| {
        edit(&mut config.harness_defaults, &mut config.harnesses)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_round_trips_and_old_configs_still_load() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), "theme = \"dark\"\n").unwrap();
        let (defaults, harnesses) = load_in(dir.path());
        assert_eq!(defaults, HarnessDefaults::default());
        assert!(harnesses.is_empty());

        edit_in(dir.path(), |defaults, harnesses| {
            defaults.harness = Some("codex".into());
            defaults.model = Some("gpt-6-luna".into());
            defaults.effort = Some("low".into());
            defaults.onboarded = true;
            apply_change(
                harnesses,
                HarnessChange {
                    id: "codex".into(),
                    enabled: Some(true),
                    ..Default::default()
                },
            )?;
            apply_change(
                harnesses,
                HarnessChange {
                    id: "my-agent".into(),
                    name: Some("My agent".into()),
                    command: Some(vec!["my-agent".into(), "--acp".into()]),
                    ..Default::default()
                },
            )
        })
        .unwrap();

        let text = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
        assert!(text.contains("[harness_defaults]"), "{text}");
        assert!(text.contains("[[harnesses]]"), "{text}");
        assert!(text.contains("theme = \"dark\""), "{text}");

        let (defaults, harnesses) = load_in(dir.path());
        assert_eq!(defaults.harness.as_deref(), Some("codex"));
        assert_eq!(defaults.effort.as_deref(), Some("low"));
        assert!(defaults.onboarded);
        assert_eq!(
            harnesses,
            vec![
                HarnessEntry {
                    id: "codex".into(),
                    enabled: Some(true),
                    ..Default::default()
                },
                HarnessEntry {
                    id: "my-agent".into(),
                    name: Some("My agent".into()),
                    kind: Some(HarnessKind::Acp),
                    command: Some(vec!["my-agent".into(), "--acp".into()]),
                    enabled: Some(true),
                    ..Default::default()
                },
            ]
        );

        edit_in(dir.path(), |_, harnesses| remove(harnesses, "my-agent")).unwrap();
        assert_eq!(load_in(dir.path()).1.len(), 1);
    }

    #[test]
    fn the_silence_deadline_parses_and_off_is_zero() {
        assert_eq!(
            silence_from(None),
            Some(super::super::watchdog::DEFAULT_TIMEOUT)
        );
        assert_eq!(silence_from(Some(0)), None, "0 turns the deadline off");
        assert_eq!(silence_from(Some(25)), Some(Duration::from_secs(25 * 60)));
        assert_eq!(
            silence_from(Some(u64::MAX)),
            Some(Duration::from_secs(u64::MAX.saturating_mul(60))),
            "a silly value saturates instead of overflowing"
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            "[harness_defaults]\nsilence_timeout_minutes = 0\n",
        )
        .unwrap();
        assert_eq!(load_in(dir.path()).0.silence_timeout_minutes, Some(0));
        edit_in(dir.path(), |defaults, _| {
            defaults.silence_timeout_minutes = None;
            Ok(())
        })
        .unwrap();
        let text = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
        assert!(
            !text.contains("silence_timeout_minutes"),
            "an unset deadline is left out: {text}"
        );
    }

    #[test]
    fn the_idle_timeout_and_the_live_cap_parse_and_off_is_zero() {
        assert_eq!(
            idle_from(None),
            Some(super::super::session::DEFAULT_IDLE_TIMEOUT)
        );
        assert_eq!(idle_from(Some(0)), None, "0 keeps every process running");
        assert_eq!(idle_from(Some(45)), Some(Duration::from_secs(45 * 60)));
        assert_eq!(
            idle_from(Some(u64::MAX)),
            Some(Duration::from_secs(u64::MAX.saturating_mul(60))),
            "a silly value saturates instead of overflowing"
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            "[harness_defaults]\nidle_timeout_minutes = 5\nmax_live_harnesses = 3\n",
        )
        .unwrap();
        let defaults = load_in(dir.path()).0;
        assert_eq!(defaults.idle_timeout_minutes, Some(5));
        assert_eq!(defaults.max_live_harnesses, Some(3));
        edit_in(dir.path(), |defaults, _| {
            defaults.idle_timeout_minutes = None;
            defaults.max_live_harnesses = None;
            Ok(())
        })
        .unwrap();
        let text = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
        assert!(
            !text.contains("idle_timeout_minutes") && !text.contains("max_live_harnesses"),
            "settings left out of the config are never written back: {text}"
        );
    }

    #[test]
    fn documented_toml_parses() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            r#"
[harness_defaults]
harness = "codex"
model = "gpt-6-luna"
effort = "low"
onboarded = true

[[harnesses]]
id = "codex"
enabled = true
model = "gpt-6-luna"
effort = "high"

[[harnesses]]
id = "rustcode"
model = "local"

[[harnesses]]
id = "my-agent"
name = "My agent"
kind = "acp"
command = ["my-agent", "--acp"]
enabled = true
"#,
        )
        .unwrap();
        let (defaults, harnesses) = load_in(dir.path());
        assert!(defaults.onboarded);
        assert_eq!(harnesses.len(), 3);
        assert_eq!(harnesses[0].effort.as_deref(), Some("high"));
        assert_eq!(harnesses[1].model.as_deref(), Some("local"));
        assert_eq!(harnesses[2].kind, Some(HarnessKind::Acp));
        let ids = super::super::resolve_all(&harnesses)
            .into_iter()
            .map(|harness| harness.id)
            .collect::<Vec<_>>();
        assert_eq!(ids.iter().filter(|id| *id == "rustcode").count(), 1);
    }

    #[test]
    fn last_choices_round_trip_per_harness() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            "[harness_defaults]\nharness = \"codex\"\n\n[[harnesses]]\nid = \"codex\"\nenabled = true\n",
        )
        .unwrap();
        edit_in(dir.path(), |_, harnesses| {
            remember(
                harnesses,
                "codex",
                Some("gpt-6-luna".into()),
                Some("high".into()),
            )?;
            remember(
                harnesses,
                "claude-code",
                Some("opus".into()),
                Some("high".into()),
            )?;
            remember(harnesses, "rustcode", Some("local".into()), None)?;
            remember(harnesses, "gemini", None, Some(" ".into()))
        })
        .unwrap();
        let text = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
        assert!(text.contains("model = \"opus\""), "{text}");
        let (defaults, harnesses) = load_in(dir.path());
        assert_eq!(
            defaults.harness.as_deref(),
            Some("codex"),
            "the default harness stays"
        );
        let entry = |id: &str| harnesses.iter().find(|entry| entry.id == id).cloned();
        let codex = entry("codex").unwrap();
        assert_eq!(codex.enabled, Some(true));
        assert_eq!(
            (codex.model.as_deref(), codex.effort.as_deref()),
            (Some("gpt-6-luna"), Some("high"))
        );
        let claude = entry("claude-code").unwrap();
        assert_eq!(
            claude.enabled,
            Some(false),
            "remembering does not turn a harness on"
        );
        assert_eq!(claude.model.as_deref(), Some("opus"));
        let builtin = entry("rustcode").unwrap();
        assert_eq!(builtin.enabled, None);
        assert_eq!(builtin.model.as_deref(), Some("local"));
        assert!(
            entry("gemini").is_none(),
            "nothing to remember adds no entry"
        );
        assert!(super::super::resolve(&harnesses, "claude-code").is_some_and(|h| !h.enabled));

        edit_in(dir.path(), |_, harnesses| {
            remember(harnesses, "codex", None, None)
        })
        .unwrap();
        let codex = load_in(dir.path())
            .1
            .into_iter()
            .find(|e| e.id == "codex")
            .unwrap();
        assert_eq!((codex.model, codex.effort), (None, None));
        let mut harnesses = Vec::new();
        assert!(remember(&mut harnesses, "nope", Some("x".into()), None).is_err());
    }

    #[test]
    fn changes_are_validated() {
        let mut harnesses = Vec::new();
        assert!(
            apply_change(
                &mut harnesses,
                HarnessChange {
                    id: "rustcode".into(),
                    command: Some(vec!["x".into()]),
                    ..Default::default()
                }
            )
            .is_err(),
            "the built-in harness has no command"
        );
        assert!(
            apply_change(
                &mut harnesses,
                HarnessChange {
                    id: "custom".into(),
                    ..Default::default()
                }
            )
            .is_err(),
            "a custom harness needs a command"
        );
        assert!(
            apply_change(
                &mut harnesses,
                HarnessChange {
                    id: "bad id".into(),
                    command: Some(vec!["x".into()]),
                    ..Default::default()
                }
            )
            .is_err()
        );
        assert!(remove(&mut harnesses, "codex").is_err());
        assert!(remove(&mut harnesses, "nope").is_err());
    }

    #[test]
    fn named_accounts_parse_validate_and_remove() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            r#"
[[harness_accounts]]
id = "paral-2"
harness = "claude-code"
name = "Paral 2"
config_dir = "~/.claude-paral-2"
"#,
        )
        .unwrap();
        let accounts = load_accounts_in(dir.path());
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].display_name(), "Paral 2");
        assert_eq!(accounts[0].config_dir, "~/.claude-paral-2");
        assert!(
            accounts[0].resolved_dir().is_absolute(),
            "the user's home is expanded"
        );
        assert_eq!(
            find_account(&accounts, "claude-code", "paral-2")
                .unwrap()
                .id,
            "paral-2"
        );
        assert!(
            find_account(&accounts, "claude-code", "nope").is_err(),
            "an unknown account is an error, never the default login"
        );
        assert!(
            find_account(&accounts, "codex", "paral-2").is_err(),
            "an account belongs to one harness"
        );

        let own = dir.path().join("claude-work");
        let added = add_account_in(
            dir.path(),
            AccountChange {
                harness: "claude-code".into(),
                id: "work".into(),
                name: Some(" Work ".into()),
                config_dir: Some(own.to_string_lossy().into_owned()),
            },
        )
        .unwrap();
        assert_eq!(added.name.as_deref(), Some("Work"));
        assert!(own.is_dir(), "the account's directory is created");
        let accounts = load_accounts_in(dir.path());
        assert_eq!(accounts.len(), 2);
        assert_eq!(
            accounts[1].config_dir,
            own.to_string_lossy(),
            "an explicit directory is kept as given"
        );

        let duplicate = add_account_in(
            dir.path(),
            AccountChange {
                harness: "claude-code".into(),
                id: "work".into(),
                name: Some("Work again".into()),
                config_dir: Some(own.to_string_lossy().into_owned()),
            },
        );
        assert!(duplicate.is_err(), "two accounts cannot share an id");
        let shared = add_account_in(
            dir.path(),
            AccountChange {
                harness: "claude-code".into(),
                id: "work-2".into(),
                name: Some("Work 2".into()),
                config_dir: Some(own.to_string_lossy().into_owned()),
            },
        );
        assert!(shared.is_err(), "two accounts cannot share a directory");
        for (id, name) in [
            ("bad id", Some("Bad")),
            ("work-3", None),
            ("work-4", Some("   ")),
        ] {
            assert!(
                add_account_in(
                    dir.path(),
                    AccountChange {
                        harness: "claude-code".into(),
                        id: id.into(),
                        name: name.map(str::to_owned),
                        config_dir: Some(own.to_string_lossy().into_owned()),
                    },
                )
                .is_err(),
                "{id} / {name:?} must be rejected"
            );
        }
        assert!(
            add_account_in(
                dir.path(),
                AccountChange {
                    harness: "gemini".into(),
                    id: "work-5".into(),
                    name: Some("Work 5".into()),
                    config_dir: Some(own.to_string_lossy().into_owned()),
                },
            )
            .is_err(),
            "Gemini CLI has no named logins"
        );
        assert_eq!(
            default_config_dir(&CLAUDE_ACCOUNT, "paral-2"),
            "~/.claude-paral-2"
        );
        assert_eq!(
            default_config_dir(&CODEX_ACCOUNT, "paral"),
            "~/.codex-paral"
        );

        let removed = remove_account_in(dir.path(), "claude-code", "work").unwrap();
        assert_eq!(removed.id, "work");
        assert!(own.is_dir(), "removing an account never deletes its login");
        assert_eq!(load_accounts_in(dir.path()).len(), 1);
        assert!(remove_account_in(dir.path(), "claude-code", "work").is_err());
    }

    #[test]
    fn a_codex_account_gets_its_own_directory_and_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let added = add_account_in(
            dir.path(),
            AccountChange {
                harness: "codex".into(),
                id: "paral".into(),
                name: Some("Paral".into()),
                config_dir: None,
            },
        )
        .unwrap();
        assert_eq!(added.config_dir, "~/.codex-paral");
        assert!(
            added.resolved_dir().is_dir(),
            "the account's directory is created"
        );
        let accounts = load_accounts_in(dir.path());
        assert_eq!(
            find_account(&accounts, "codex", "paral").unwrap().id,
            "paral"
        );
        assert!(
            find_account(&accounts, "claude-code", "paral").is_err(),
            "a Codex account is not a Claude Code one"
        );
        assert!(!is_default_dir("codex", &added.resolved_dir()));
    }

    #[test]
    fn the_default_claude_directory_is_a_named_account() {
        let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) else {
            return;
        };
        // The engine still treats this directory as the login with no
        // `CLAUDE_CONFIG_DIR` (harness/session.rs), but it is a normal account
        // now, so a thread can move on and off the Mac's own Claude login.
        assert!(is_default_dir("claude-code", &home.join(".claude")));
        assert!(
            is_default_dir("codex", &home.join(".codex")),
            "each harness has a default directory of its own"
        );
        assert!(
            !is_default_dir("codex", &home.join(".claude")),
            "another harness's default directory is not this one's"
        );
        let dir = tempfile::tempdir().unwrap();
        let own = dir.path().join(".claude");
        std::fs::create_dir_all(&own).unwrap();
        let added = add_account_in(
            dir.path(),
            AccountChange {
                harness: "claude-code".into(),
                id: "default".into(),
                name: Some("Claude".into()),
                config_dir: Some(own.to_string_lossy().into_owned()),
            },
        )
        .unwrap();
        assert_eq!(added.resolved_dir(), own);
        // Two accounts still cannot share one directory.
        assert!(
            add_account_in(
                dir.path(),
                AccountChange {
                    harness: "claude-code".into(),
                    id: "other".into(),
                    name: Some("Other".into()),
                    config_dir: Some(own.to_string_lossy().into_owned()),
                },
            )
            .is_err()
        );
    }

    #[test]
    fn discovery_lists_claude_directories_no_account_uses() {
        let home = tempfile::tempdir().unwrap();
        let dir = |name: &str| home.path().join(name);
        std::fs::create_dir_all(dir(".claude/projects")).unwrap();
        std::fs::create_dir_all(dir(".claude-personal")).unwrap();
        std::fs::write(dir(".claude-personal/.claude.json"), "{}").unwrap();
        std::fs::create_dir_all(dir(".claude-work/notes")).unwrap();
        // A file, and a directory that does not look like a config dir, are skipped.
        std::fs::write(dir(".claude-file"), "").unwrap();
        std::fs::create_dir_all(dir(".claude-empty")).unwrap();
        std::fs::create_dir_all(dir("elsewhere")).unwrap();
        // A registered account's directory is not offered again.
        let registered = [HarnessAccount {
            id: "paral-2".into(),
            harness: "claude-code".into(),
            name: Some("Paral 2".into()),
            config_dir: dir(".claude-personal").to_string_lossy().into_owned(),
        }];
        let found = discovered_accounts_in(home.path(), "claude-code", &registered);
        assert_eq!(
            found,
            vec![DiscoveredAccount {
                config_dir: "~/.claude".to_owned(),
                suggested_name: "Claude".to_owned(),
            }]
        );
        // A second profile is suggested under its title-cased suffix.
        let found = discovered_accounts_in(home.path(), "claude-code", &[]);
        assert_eq!(
            found
                .iter()
                .map(|a| (a.config_dir.as_str(), a.suggested_name.as_str()))
                .collect::<Vec<_>>(),
            vec![("~/.claude", "Claude"), ("~/.claude-personal", "Personal"),]
        );
    }

    #[test]
    fn discovery_is_per_harness() {
        let home = tempfile::tempdir().unwrap();
        let dir = |name: &str| home.path().join(name);
        std::fs::create_dir_all(dir(".claude/projects")).unwrap();
        std::fs::create_dir_all(dir(".codex/sessions")).unwrap();
        std::fs::create_dir_all(dir(".codex-paral")).unwrap();
        std::fs::write(dir(".codex-paral/auth.json"), "{}").unwrap();
        // A Codex home that holds only its config still counts.
        std::fs::create_dir_all(dir(".codex-work")).unwrap();
        std::fs::write(dir(".codex-work/config.toml"), "").unwrap();
        // One harness's directories are never offered for the other.
        assert_eq!(
            discovered_accounts_in(home.path(), "codex", &[]),
            vec![
                DiscoveredAccount {
                    config_dir: "~/.codex".to_owned(),
                    suggested_name: "Codex".to_owned(),
                },
                DiscoveredAccount {
                    config_dir: "~/.codex-paral".to_owned(),
                    suggested_name: "Paral".to_owned(),
                },
                DiscoveredAccount {
                    config_dir: "~/.codex-work".to_owned(),
                    suggested_name: "Work".to_owned(),
                },
            ]
        );
        assert_eq!(
            discovered_accounts_in(home.path(), "claude-code", &[])
                .iter()
                .map(|a| a.config_dir.as_str())
                .collect::<Vec<_>>(),
            vec!["~/.claude"]
        );
        // A registered Codex account's directory is not offered again, and a
        // harness with no named logins discovers nothing at all.
        let registered = [HarnessAccount {
            id: "paral".into(),
            harness: "codex".into(),
            name: Some("Paral".into()),
            config_dir: dir(".codex-paral").to_string_lossy().into_owned(),
        }];
        assert!(
            discovered_accounts_in(home.path(), "codex", &registered)
                .iter()
                .all(|a| a.config_dir != "~/.codex-paral")
        );
        assert!(discovered_accounts_in(home.path(), "gemini", &[]).is_empty());
    }

    #[test]
    fn a_login_directory_counts_only_the_kind_of_name_it_holds() {
        let home = tempfile::tempdir().unwrap();
        let dir = |name: &str| home.path().join(name);
        // A directory called `auth.json` is not a Codex sign-in, and a file
        // called `sessions` is not its sessions folder.
        std::fs::create_dir_all(dir(".codex-file-as-dir/auth.json")).unwrap();
        std::fs::create_dir_all(dir(".codex-dir-as-file")).unwrap();
        std::fs::write(dir(".codex-dir-as-file/sessions"), "").unwrap();
        // The same for Claude Code's `projects/` and `.credentials.json`.
        std::fs::create_dir_all(dir(".claude-file-as-dir/.credentials.json")).unwrap();
        std::fs::create_dir_all(dir(".claude-dir-as-file")).unwrap();
        std::fs::write(dir(".claude-dir-as-file/projects"), "").unwrap();
        assert!(discovered_accounts_in(home.path(), "codex", &[]).is_empty());
        assert!(discovered_accounts_in(home.path(), "claude-code", &[]).is_empty());
    }

    #[test]
    fn a_discovered_name_is_title_cased_from_its_suffix() {
        assert_eq!(
            suggested_account_name(&CLAUDE_ACCOUNT, Path::new("/Users/me/.claude")),
            "Claude"
        );
        assert_eq!(
            suggested_account_name(&CLAUDE_ACCOUNT, Path::new("/Users/me/.claude-paral-2")),
            "Paral 2"
        );
        assert_eq!(
            suggested_account_name(&CLAUDE_ACCOUNT, Path::new("/Users/me/.claude-work_account")),
            "Work Account"
        );
        assert_eq!(
            suggested_account_name(&CODEX_ACCOUNT, Path::new("/Users/me/.codex")),
            "Codex"
        );
        assert_eq!(
            suggested_account_name(&CODEX_ACCOUNT, Path::new("/Users/me/.codex-paral-2")),
            "Paral 2"
        );
    }

    #[test]
    fn project_config_cannot_add_accounts() {
        let workspace = tempfile::tempdir().unwrap();
        let project = workspace.path().join(".rustcode");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(
            project.join("config.toml"),
            "[[harness_accounts]]\nid = \"evil\"\nharness = \"claude-code\"\nname = \"Evil\"\nconfig_dir = \"/tmp/evil\"\n",
        )
        .unwrap();
        let (_, _, config) = crate::config::load_config_for_workspace(workspace.path());
        assert!(
            config.harness_accounts.is_empty(),
            "a checked-out project must not choose where a thread signs in"
        );
    }

    #[test]
    fn project_config_cannot_add_harnesses() {
        let workspace = tempfile::tempdir().unwrap();
        let project = workspace.path().join(".rustcode");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(
            project.join("config.toml"),
            "[[harnesses]]\nid = \"evil\"\ncommand = [\"rm\", \"-rf\", \"/\"]\n",
        )
        .unwrap();
        let (_, _, config) = crate::config::load_config_for_workspace(workspace.path());
        assert!(config.harnesses.iter().all(|entry| entry.id != "evil"));
    }

    #[test]
    fn the_builtin_harness_turns_off_once_another_is_the_default() {
        let off = |id: &str| HarnessChange {
            id: id.into(),
            enabled: Some(false),
            ..Default::default()
        };
        let on_builtin = HarnessDefaults::default();
        assert!(check_default(&on_builtin, &off("rustcode")).is_err());
        let on_claude = HarnessDefaults {
            harness: Some("claude-code".into()),
            ..Default::default()
        };
        assert!(check_default(&on_claude, &off("rustcode")).is_ok());

        let mut harnesses = Vec::new();
        apply_change(&mut harnesses, off("rustcode")).unwrap();
        assert_eq!(harnesses[0].id, "rustcode");
        assert!(!harnesses[0].is_enabled());
        assert!(!super::super::resolve_all(&harnesses)[0].enabled);
        apply_change(
            &mut harnesses,
            HarnessChange {
                id: "rustcode".into(),
                enabled: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(harnesses[0].is_enabled());
        assert!(super::super::resolve_all(&harnesses)[0].enabled);
    }
}
