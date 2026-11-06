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

/// One `[[harness_accounts]]` entry: a named `CLAUDE_CONFIG_DIR` for a
/// harness, so a thread can run on another login of the same tool. RustCode
/// only names the directory; the harness keeps its own sign-in there.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessAccount {
    pub id: String,
    /// The harness this account belongs to (`claude-code` for now).
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

/// Whether `config_dir` is the harness's own default sign-in.
fn is_default_dir(dir: &Path) -> bool {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return false;
    };
    dir == home.join(".claude")
}

/// Where an account's own directory goes when the client names none.
pub(crate) fn default_config_dir(id: &str) -> String {
    format!("~/.claude-{id}")
}

/// Add an account to the user config. Validates it, derives
/// `~/.claude-<id>` when no directory is given, and creates the directory.
pub(crate) fn add_account_in(dir: &Path, change: AccountChange) -> Result<HarnessAccount, String> {
    let harness = change.harness.trim().to_owned();
    if harness != "claude-code" {
        return Err(format!(
            "{harness} has no named logins yet; only Claude Code does"
        ));
    }
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
        .unwrap_or_else(|| default_config_dir(&id));
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
    if is_default_dir(&resolved) {
        return Err(
            "~/.claude is your default Claude Code login; give the account its own directory"
                .to_owned(),
        );
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
                    harness: "codex".into(),
                    id: "work-5".into(),
                    name: Some("Work 5".into()),
                    config_dir: Some(own.to_string_lossy().into_owned()),
                },
            )
            .is_err(),
            "only Claude Code has named logins for now"
        );
        assert_eq!(default_config_dir("paral-2"), "~/.claude-paral-2");

        let removed = remove_account_in(dir.path(), "claude-code", "work").unwrap();
        assert_eq!(removed.id, "work");
        assert!(own.is_dir(), "removing an account never deletes its login");
        assert_eq!(load_accounts_in(dir.path()).len(), 1);
        assert!(remove_account_in(dir.path(), "claude-code", "work").is_err());
    }

    #[test]
    fn the_default_claude_directory_is_not_an_account() {
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
        let Some(home) = home else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let rejected = add_account_in(
            dir.path(),
            AccountChange {
                harness: "claude-code".into(),
                id: "default".into(),
                name: Some("Default".into()),
                config_dir: Some(home.join(".claude").to_string_lossy().into_owned()),
            },
        );
        assert!(
            rejected.is_err(),
            "the default login is not an account of its own"
        );
        assert!(load_accounts_in(dir.path()).is_empty());
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
