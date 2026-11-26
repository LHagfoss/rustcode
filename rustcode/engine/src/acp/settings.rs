//! `_rustcode/settings/get|set`: the server settings a client edits on one
//! machine or on all of them (docs/desktop.md › Settings on one machine or
//! all). One registry names every key, its JSON type and the value a client
//! reads when the machine has never set it; a key outside it is refused, so a
//! machine cannot be turned into a general-purpose store over ACP.
//!
//! The values are the client-hosted settings of a machine: the app applies
//! them when it starts or when it reads them. They live in
//! `<config-dir>/settings.json`, apart from `config.toml` because the engine
//! does not act on them yet and they must survive a client that never edits
//! `config.toml`. The harness default and the skill/MCP toggles already have
//! methods of their own (`_rustcode/harnesses/default`, `_rustcode/capabilities*`),
//! so they are not duplicated here.
//!
//! Offline machines keep their file; a client writes to a machine only while
//! its connection is open. `_rustcode/settings/changed` is sent when the file
//! changes, whoever wrote it: the app, `rc settings set`, an agent's
//! `manage_settings` or an edit by hand ([`start_watching`]).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The file these values live in, beside `config.toml`.
pub(crate) const SETTINGS_FILE: &str = "settings.json";

/// The type a key accepts. A client may send any of the three, but only the
/// one the registry declares is stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettingType {
    Object,
    Boolean,
    Number,
    Text,
    /// A JSON array; its entries are read by the code that applies the key.
    List,
}

/// Every server setting, once: `(key, type, default)`. The order is the
/// order a `get` with no keys answers in; it is also the list a client may
/// use to check which keys a machine supports.
fn registry() -> &'static [(&'static str, SettingType, Value)] {
    static REGISTRY: OnceLock<Vec<(&'static str, SettingType, Value)>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        vec![
            ("computer-use", SettingType::Object, json!({})),
            ("default-permission-mode", SettingType::Text, json!("ask")),
            ("resume-after-restart", SettingType::Boolean, json!(true)),
            ("continue-after-limit", SettingType::Boolean, json!(true)),
            ("auto-compact-mode", SettingType::Text, json!("size")),
            (
                "auto-compact-threshold",
                SettingType::Number,
                json!(150_000),
            ),
            ("orchestration", SettingType::Boolean, json!(true)),
            ("delegates-total", SettingType::Number, json!(8)),
            ("delegates-per-thread", SettingType::Number, json!(4)),
            ("trust-delegate-accounts", SettingType::Boolean, json!(true)),
            ("read-across-projects", SettingType::Boolean, json!(true)),
            ("delegate-presets", SettingType::List, json!([])),
            ("limit-fallback", SettingType::Boolean, json!(false)),
            ("limit-fallback-map", SettingType::List, json!([])),
            ("wake-batch-seconds", SettingType::Number, json!(60)),
            // Workflow runs (#3394): workers at once, agent() calls in all, minutes,
            // and how many settled runs a thread keeps.
            ("workflow-concurrency", SettingType::Number, json!(3)),
            ("workflow-call-budget", SettingType::Number, json!(40)),
            ("workflow-timeout-minutes", SettingType::Number, json!(60)),
            ("workflow-kept-runs", SettingType::Number, json!(20)),
            ("workflow-token-budget", SettingType::Number, json!(0)),
            ("workflow-cost-budget-usd", SettingType::Number, json!(0)),
            // Whether a thread is listed the tools few threads need
            // (`orchestration::EXTRA_TOOL_NAMES`), which cost context in every one.
            (
                crate::orchestration::ALL_TOOLS_SETTING,
                SettingType::Boolean,
                json!(false),
            ),
        ]
    })
}

/// The keys a project may set for itself, resolved default → machine →
/// project when a thread in that project reads them (docs/desktop.md ›
/// Settings a project overrides). Credentials, accounts, delegation limits and
/// the machine's own switches are not here: they belong to the machine.
/// `default-harness` is project-only: the machine's default lives in
/// `config.toml` (`_rustcode/harnesses/default`). So are `text-generator`
/// (`{ id, model? }`, over the machine's selection in `text-generators.json`)
/// and `voice` (`{ generator?, model?, language?, cleanup? }`, over the app's
/// own Voice settings).
pub(crate) const PROJECT_KEYS: &[&str] = &[
    "default-harness",
    "text-generator",
    "voice",
    "default-permission-mode",
    "resume-after-restart",
    "continue-after-limit",
    "auto-compact-mode",
    "auto-compact-threshold",
];

/// Project keys with no machine value in this file: an object each.
const PROJECT_ONLY_KEYS: &[&str] = &["default-harness", "text-generator", "voice"];

/// The type a project override of `key` must have, or `None` when a project
/// may not override it.
fn project_key_type(key: &str) -> Option<(&'static str, SettingType)> {
    let known = PROJECT_KEYS.iter().find(|known| **known == key)?;
    if PROJECT_ONLY_KEYS.contains(known) {
        return Some((known, SettingType::Object));
    }
    registry_entry(known).map(|(known, kind, _)| (known, kind))
}

/// How a project is named in the file: its folder (a worktree's repository),
/// without a trailing separator, the way the app's `projectKey` spells it.
pub(crate) fn project_key(path: &str) -> String {
    let trimmed = path.trim_end_matches(['/', '\\']);
    if trimmed.is_empty() {
        path.to_owned()
    } else {
        trimmed.to_owned()
    }
}

fn registry_entry(key: &str) -> Option<(&'static str, SettingType, &'static Value)> {
    registry()
        .iter()
        .find(|(known, ..)| *known == key)
        .map(|(known, kind, default)| (*known, *kind, default))
}

/// `_rustcode/settings/get`: every server setting, or only `keys`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[request(method = "_rustcode/settings/get", response = serde_json::Value)]
pub(crate) struct GetSettingsRequest {
    /// The keys to read; absent reads all of them.
    #[serde(default)]
    pub keys: Option<Vec<String>>,
    /// A project's folder: the values are resolved for a thread there, and
    /// the answer carries the project's own overrides and where each value
    /// comes from.
    #[serde(default)]
    pub project: Option<String>,
}

/// `_rustcode/settings/set`: merge these values into the machine's settings.
#[derive(Debug, Clone, Default, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[request(method = "_rustcode/settings/set", response = serde_json::Value)]
pub(crate) struct SetSettingsRequest {
    #[serde(default)]
    pub values: Map<String, Value>,
    /// Write the values as this project's overrides instead of the machine's.
    #[serde(default)]
    pub project: Option<String>,
    /// Keys to remove: the project's override (with `project`), else the
    /// machine's own value, so the next read falls back a layer.
    #[serde(default)]
    pub clear: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct SettingsFile {
    #[serde(default = "file_version")]
    version: u32,
    #[serde(default)]
    values: Map<String, Value>,
    /// Each project's overrides, keyed by [`project_key`]. A file written
    /// before projects could override anything has none and reads the same.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    projects: BTreeMap<String, Map<String, Value>>,
}

fn file_version() -> u32 {
    1
}

fn settings_path() -> Result<PathBuf, String> {
    crate::config::get_config_dir()
        .map(|dir| dir.join(SETTINGS_FILE))
        .ok_or_else(|| "could not determine the config directory".to_owned())
}

/// The stored file, with unknown or wrong-typed entries dropped: a file
/// edited by hand must not make a read fail.
fn read_file(path: &Path) -> SettingsFile {
    let Ok(text) = std::fs::read_to_string(path) else {
        return SettingsFile::default();
    };
    let file: SettingsFile = serde_json::from_str(&text).unwrap_or_default();
    let mut values = Map::new();
    for (key, value) in file.values {
        if let Some((known, kind, _)) = registry_entry(&key)
            && value_matches(kind, &value)
        {
            values.insert(known.to_owned(), value);
        }
    }
    let mut projects = BTreeMap::new();
    for (project, overrides) in file.projects {
        let kept: Map<String, Value> = overrides
            .into_iter()
            .filter_map(|(key, value)| {
                let (known, kind) = project_key_type(&key)?;
                value_matches(kind, &value).then(|| (known.to_owned(), value))
            })
            .collect();
        if !kept.is_empty() {
            projects.insert(project_key(&project), kept);
        }
    }
    SettingsFile {
        version: 1,
        values,
        projects,
    }
}

fn value_matches(kind: SettingType, value: &Value) -> bool {
    match kind {
        SettingType::Object => value.is_object(),
        SettingType::Boolean => value.is_boolean(),
        SettingType::Number => value.as_f64().is_some_and(|n| n.is_finite()),
        SettingType::Text => value.is_string(),
        SettingType::List => value.is_array(),
    }
}

fn write_file(path: &Path, file: &SettingsFile) -> Result<(), String> {
    let text = serde_json::to_string_pretty(file)
        .map_err(|error| format!("could not serialize settings: {error}"))?;
    crate::atomic_file::write_described(path, text.as_bytes())
}

/// Answer `_rustcode/settings/get`: the requested keys, with each registry
/// default where the machine has no value of its own, and `stored`, the keys
/// this file actually holds. A client seeds a missing key only, so a value
/// another machine wrote before this one first started is not overwritten.
pub(crate) fn read(request: GetSettingsRequest) -> Result<Value, agent_client_protocol::Error> {
    let file = read_file(&settings_path().unwrap_or_default());
    let stored = &file.values;
    let project = request.project.as_deref().map(project_key);
    let overrides = project
        .as_ref()
        .and_then(|key| file.projects.get(key))
        .cloned()
        .unwrap_or_default();
    let mut values = Map::new();
    let keys: Vec<&str> = match &request.keys {
        Some(keys) => {
            for key in keys {
                if registry_entry(key).is_none() {
                    return Err(
                        agent_client_protocol::Error::invalid_params().data(unknown_key(key))
                    );
                }
            }
            keys.iter().map(String::as_str).collect()
        }
        None => registry().iter().map(|(key, ..)| *key).collect(),
    };
    let mut sources = Map::new();
    for key in keys {
        let (key, _, default) = registry_entry(key).expect("checked above");
        let (value, source) = if key == "computer-use" {
            (crate::computer_use::settings_value(), "machine")
        } else if let Some(value) = overrides.get(key) {
            (value.clone(), "project")
        } else if let Some(value) = stored.get(key) {
            (value.clone(), "machine")
        } else {
            (default.clone(), "default")
        };
        values.insert(key.to_owned(), value);
        sources.insert(key.to_owned(), json!(source));
    }
    let stored_keys: Vec<String> = stored.keys().cloned().collect();
    let mut answer = json!({ "values": values, "stored": stored_keys, "sources": sources });
    if request.keys.is_none() {
        let workspace = request.project.as_deref().map(Path::new);
        answer["configIssues"] = config_issues_json(&crate::config::config_load_issues(workspace));
    }
    if let Some(project) = project {
        answer["project"] = json!({
            "path": project,
            "overrides": overrides,
            "overridable": PROJECT_KEYS,
        });
    }
    Ok(answer)
}

/// The config files the engine could not read, for the Settings notice.
fn config_issues_json(issues: &[crate::config::ConfigLoadIssue]) -> Value {
    Value::Array(
        issues
            .iter()
            .map(|issue| {
                json!({
                    "path": issue.path.display().to_string(),
                    "scope": issue.scope,
                    "message": issue.message,
                })
            })
            .collect(),
    )
}

/// One setting's current value, for engine code that has to apply a
/// client-hosted setting itself (#3422). The registry default applies where
/// the machine has not set it.
pub(crate) fn value(key: &str) -> Option<Value> {
    value_for_project(key, None)
}

/// One setting as a thread in `project` reads it: the project's override,
/// else the machine's value, else the registry default.
pub(crate) fn value_for_project(key: &str, project: Option<&Path>) -> Option<Value> {
    let answer = read(GetSettingsRequest {
        keys: Some(vec![key.to_owned()]),
        project: project.map(|path| path.to_string_lossy().into_owned()),
    })
    .ok()?;
    answer.get("values")?.get(key).cloned()
}

/// One setting as a thread reads it, through its project (its repository's
/// folder, as the session store records it).
pub(crate) fn value_for_session(key: &str, session_id: &str) -> Option<Value> {
    let project = crate::sessions::load_session_repo(session_id);
    value_for_project(key, project.as_deref())
}

/// A project's own override of `key`, without falling back a layer: how a
/// project-only key (`text-generator`, `voice`) is read.
pub(crate) fn project_override(key: &str, project: &Path) -> Option<Value> {
    let file = read_file(&settings_path().ok()?);
    file.projects
        .get(&project_key(&project.to_string_lossy()))?
        .get(key)
        .cloned()
}

/// Answer `_rustcode/settings/set`: merge the values, then read them back.
pub(crate) fn write(request: SetSettingsRequest) -> Result<Value, agent_client_protocol::Error> {
    let path = settings_path()
        .map_err(|error| agent_client_protocol::Error::internal_error().data(error))?;
    let mut file = read_file(&path);
    if let Some(project) = request.project.as_deref() {
        let project = project_key(project);
        if project.trim().is_empty() {
            return Err(agent_client_protocol::Error::invalid_params().data("project is empty"));
        }
        let mut overrides = file.projects.remove(&project).unwrap_or_default();
        for (key, value) in &request.values {
            let Some((known, kind)) = project_key_type(key) else {
                return Err(agent_client_protocol::Error::invalid_params()
                    .data(format!("a project cannot override '{key}'")));
            };
            if !value_matches(kind, value) {
                return Err(agent_client_protocol::Error::invalid_params()
                    .data(format!("setting '{known}' has the wrong type")));
            }
            if is_key(known) {
                check_value(known, value).map_err(|message| {
                    agent_client_protocol::Error::invalid_params().data(message)
                })?;
            }
            overrides.insert(known.to_owned(), value.clone());
        }
        for key in &request.clear {
            overrides.remove(key);
        }
        if !overrides.is_empty() {
            file.projects.insert(project.clone(), overrides);
        }
        write_file(&path, &file)
            .map_err(|error| agent_client_protocol::Error::internal_error().data(error))?;
        return read(GetSettingsRequest {
            keys: None,
            project: Some(project),
        });
    }
    let values = &mut file.values;
    for key in &request.clear {
        values.remove(key);
    }
    for (key, value) in &request.values {
        let Some((known, kind, _)) = registry_entry(key) else {
            return Err(agent_client_protocol::Error::invalid_params()
                .data(format!("unknown setting '{key}'")));
        };
        if !value_matches(kind, value) {
            return Err(agent_client_protocol::Error::invalid_params()
                .data(format!("setting '{known}' has the wrong type")));
        }
        check_value(known, value)
            .map_err(|message| agent_client_protocol::Error::invalid_params().data(message))?;
        if known == "computer-use" {
            crate::computer_use::save_settings(value)
                .map_err(|e| agent_client_protocol::Error::invalid_params().data(e))?;
            continue;
        }
        values.insert(known.to_owned(), value.clone());
    }
    write_file(&path, &file)
        .map_err(|error| agent_client_protocol::Error::internal_error().data(error))?;
    read(GetSettingsRequest::default())
}

/// What a person or an agent reads about a key before changing it: a short
/// label (the Settings row's) and, for a text key, the values it accepts.
fn about(key: &str) -> (&'static str, Option<&'static [&'static str]>) {
    match key {
        "computer-use" => ("Computer use (browser and desktop control)", None),
        "default-permission-mode" => ("Default access", Some(&["ask", "edits", "full"])),
        "resume-after-restart" => ("Continue threads after a restart", None),
        "continue-after-limit" => ("Continue threads on their own after a usage limit", None),
        "auto-compact-mode" => ("Auto-compact", Some(&["off", "size", "cache"])),
        "auto-compact-threshold" => ("Auto-compact threshold, in tokens", None),
        "orchestration" => ("Threads can start and message other threads", None),
        "delegates-total" => ("Delegates across all threads", None),
        "delegates-per-thread" => ("Delegates one thread can run at once", None),
        "trust-delegate-accounts" => ("Let delegates use named accounts without asking", None),
        "read-across-projects" => ("Let agents read threads in other projects", None),
        "delegate-presets" => ("Delegate presets", None),
        "limit-fallback" => ("Switch a delegate to another model at a limit", None),
        "limit-fallback-map" => ("Limit fallback models", None),
        "wake-batch-seconds" => ("Merge delegate wakes within, in seconds", None),
        "workflow-concurrency" => ("Workflow workers at once", None),
        "workflow-call-budget" => ("Workflow agent calls per run", None),
        "workflow-timeout-minutes" => ("Workflow run time limit, in minutes", None),
        "workflow-kept-runs" => ("Finished workflow runs kept per thread", None),
        "workflow-token-budget" => ("Workflow run token budget (0 is none)", None),
        "workflow-cost-budget-usd" => ("Workflow run reported cost budget, USD (0 is none)", None),
        crate::orchestration::ALL_TOOLS_SETTING => (
            "List the project, loop, workflow and worktree handoff tools to threads",
            None,
        ),
        _ => ("", None),
    }
}

fn type_name(kind: SettingType) -> &'static str {
    match kind {
        SettingType::Object => "object",
        SettingType::Boolean => "boolean",
        SettingType::Number => "number",
        SettingType::Text => "text",
        SettingType::List => "list",
    }
}

/// Every key with its type, default, label, accepted values and whether a
/// project may override it: what `rc settings list` and
/// `manage_settings` show beside the values.
pub(crate) fn describe() -> Vec<Value> {
    registry()
        .iter()
        .map(|(key, kind, default)| {
            let (label, choices) = about(key);
            let mut entry = json!({
                "key": key,
                "type": type_name(*kind),
                "default": default,
                "label": label,
                "project": PROJECT_KEYS.contains(key),
            });
            if let Some(choices) = choices {
                entry["choices"] = json!(choices);
            }
            entry
        })
        .collect()
}

/// Whether `key` is a machine setting this registry knows.
pub(crate) fn is_key(key: &str) -> bool {
    registry_entry(key).is_some()
}

/// The keys, for a refusal that names what exists.
pub(crate) fn keys() -> Vec<&'static str> {
    registry().iter().map(|(key, ..)| *key).collect()
}

/// A value typed on a command line, as the JSON `key` stores: `true`/`false`
/// (also `on`/`off`, `yes`/`no`), a number, the text itself, or JSON for an
/// object or a list. A wrong value is refused with what the key accepts.
pub(crate) fn parse_text(key: &str, text: &str) -> Result<Value, String> {
    let Some((known, kind, _)) = registry_entry(key) else {
        return Err(unknown_key(key));
    };
    let trimmed = text.trim();
    let value = match kind {
        SettingType::Boolean => match trimmed.to_ascii_lowercase().as_str() {
            "true" | "on" | "yes" | "1" => json!(true),
            "false" | "off" | "no" | "0" => json!(false),
            _ => return Err(format!("{known} is true or false, not '{trimmed}'")),
        },
        SettingType::Number => {
            if let Ok(whole) = trimmed.replace('_', "").parse::<i64>() {
                json!(whole)
            } else {
                match trimmed.parse::<f64>() {
                    Ok(number) if number.is_finite() => json!(number),
                    _ => return Err(format!("{known} is a number, not '{trimmed}'")),
                }
            }
        }
        SettingType::Text => json!(trimmed),
        SettingType::Object | SettingType::List => {
            serde_json::from_str(trimmed).map_err(|error| format!("{known} takes JSON: {error}"))?
        }
    };
    check_value(known, &value)?;
    Ok(value)
}

/// Refuse a value of the wrong type, or a text value outside the key's choices.
pub(crate) fn check(key: &str, value: &Value) -> Result<(), String> {
    check_value(key, value)
}

fn check_value(key: &str, value: &Value) -> Result<(), String> {
    let Some((known, kind, _)) = registry_entry(key) else {
        return Err(unknown_key(key));
    };
    if !value_matches(kind, value) {
        return Err(format!(
            "setting '{known}' has the wrong type: it takes a {}",
            type_name(kind)
        ));
    }
    if let (Some(choices), Some(text)) = (about(known).1, value.as_str())
        && !choices.contains(&text)
    {
        return Err(format!(
            "{known} is one of {}, not '{text}'",
            choices.join(", ")
        ));
    }
    Ok(())
}

fn unknown_key(key: &str) -> String {
    format!(
        "unknown setting '{key}'; the settings are {}",
        keys().join(", ")
    )
}

/// What the file looks like now, for the watcher: modification time and size.
fn file_signature() -> Option<(Option<std::time::SystemTime>, u64)> {
    let meta = std::fs::metadata(settings_path().ok()?).ok()?;
    Some((meta.modified().ok(), meta.len()))
}

/// `_rustcode/settings/changed`: the machine's values, as `get` with no keys
/// answers them, after `settings.json` changed (the app, the CLI, an agent's
/// `manage_settings`, or an edit by hand).
#[derive(Debug, Clone, Serialize, Deserialize, agent_client_protocol::JsonRpcNotification)]
#[notification(method = "_rustcode/settings/changed")]
pub(crate) struct SettingsChangedNotification {
    #[serde(flatten)]
    pub answer: Value,
}

/// Watch `settings.json`, once per engine, and tell every client that read
/// settings when it changes, so a value set outside the app shows there at
/// once instead of at its next start.
pub(crate) fn start_watching(clients: &super::clients::Clients) {
    static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let notify = clients.clone();
    tokio::spawn(async move {
        use crate::themes::watch::{DEBOUNCE, Debouncer, POLL};
        let mut debouncer = Debouncer::new(
            tokio::task::spawn_blocking(file_signature)
                .await
                .unwrap_or_default(),
            DEBOUNCE,
        );
        loop {
            tokio::time::sleep(POLL).await;
            let current = tokio::task::spawn_blocking(file_signature)
                .await
                .unwrap_or_default();
            if debouncer.observe(current, std::time::Instant::now()) {
                let answer =
                    tokio::task::spawn_blocking(|| read(GetSettingsRequest::default())).await;
                if let Ok(Ok(answer)) = answer {
                    notify.publish(
                        super::clients::Topic::Settings,
                        SettingsChangedNotification { answer },
                    );
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_answers_every_registry_default_before_anything_is_set() {
        let answer = read(GetSettingsRequest::default()).unwrap();
        let values = answer["values"].as_object().unwrap();
        assert_eq!(values.len(), registry().len());
        assert_eq!(answer["stored"], json!([]));
        assert_eq!(values["default-permission-mode"], json!("ask"));
        assert_eq!(values["resume-after-restart"], json!(true));
        assert_eq!(values["auto-compact-threshold"], json!(150_000));
    }

    #[test]
    fn a_full_read_lists_unreadable_config_files_and_a_keyed_read_skips_them() {
        let workspace = tempfile::tempdir().unwrap();
        let project = workspace.path().join(".rustcode");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join("config.toml"), "sandbox_mode = [\n").unwrap();
        let answer = read(GetSettingsRequest {
            project: Some(workspace.path().display().to_string()),
            ..Default::default()
        })
        .unwrap();
        let issues = answer["configIssues"].as_array().unwrap();
        let issue = issues
            .iter()
            .find(|issue| issue["scope"] == "project")
            .expect("the broken project file");
        assert!(issue["path"].as_str().unwrap().ends_with("config.toml"));
        assert!(
            issue["message"]
                .as_str()
                .unwrap()
                .contains("Failed to parse")
        );

        let keyed = read(GetSettingsRequest {
            keys: Some(vec!["orchestration".to_owned()]),
            ..Default::default()
        })
        .unwrap();
        assert!(keyed.get("configIssues").is_none());
    }

    #[test]
    fn set_merges_reads_back_and_keeps_the_rest_of_the_file() {
        let answer = write(SetSettingsRequest {
            values: Map::from_iter([
                ("orchestration".to_owned(), json!(false)),
                ("delegates-total".to_owned(), json!(12)),
            ]),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(answer["values"]["orchestration"], json!(false));
        assert_eq!(answer["values"]["delegates-total"], json!(12));
        assert_eq!(answer["values"]["default-permission-mode"], json!("ask"));
        let mut stored: Vec<&str> = answer["stored"]
            .as_array()
            .unwrap()
            .iter()
            .map(|key| key.as_str().unwrap())
            .collect();
        stored.sort();
        assert_eq!(stored, ["delegates-total", "orchestration"]);

        let reread = read(GetSettingsRequest {
            keys: Some(vec!["orchestration".to_owned()]),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(
            reread["values"].as_object().unwrap().len(),
            1,
            "only the asked keys are answered"
        );
        assert_eq!(reread["values"]["orchestration"], json!(false));
        let mut reread_stored: Vec<&str> = reread["stored"]
            .as_array()
            .unwrap()
            .iter()
            .map(|key| key.as_str().unwrap())
            .collect();
        reread_stored.sort();
        assert_eq!(
            reread_stored,
            ["delegates-total", "orchestration"],
            "stored names every key the file holds, whatever was asked"
        );
    }

    #[test]
    fn unknown_keys_and_wrong_types_are_refused_without_a_write() {
        let unknown = write(SetSettingsRequest {
            values: Map::from_iter([("theme".to_owned(), json!("dark"))]),
            ..Default::default()
        });
        assert!(unknown.is_err());
        let wrong = write(SetSettingsRequest {
            values: Map::from_iter([("resume-after-restart".to_owned(), json!("yes"))]),
            ..Default::default()
        });
        assert!(wrong.is_err());
        assert_eq!(
            read(GetSettingsRequest::default()).unwrap()["values"]["resume-after-restart"],
            json!(true)
        );
    }

    #[test]
    fn a_project_override_resolves_over_the_machine_and_the_default() {
        let project = "/work/strict-repo/";
        write(SetSettingsRequest {
            values: Map::from_iter([("default-permission-mode".to_owned(), json!("full"))]),
            ..Default::default()
        })
        .unwrap();
        // Before the project says anything: the machine's value, then the default.
        let before = read(GetSettingsRequest {
            project: Some(project.to_owned()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(before["values"]["default-permission-mode"], json!("full"));
        assert_eq!(
            before["sources"]["default-permission-mode"],
            json!("machine")
        );
        assert_eq!(before["sources"]["resume-after-restart"], json!("default"));

        let answer = write(SetSettingsRequest {
            values: Map::from_iter([
                ("default-permission-mode".to_owned(), json!("ask")),
                ("resume-after-restart".to_owned(), json!(false)),
                (
                    "default-harness".to_owned(),
                    json!({"harness": "codex", "model": "gpt-5"}),
                ),
            ]),
            project: Some(project.to_owned()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(answer["values"]["default-permission-mode"], json!("ask"));
        assert_eq!(
            answer["sources"]["default-permission-mode"],
            json!("project")
        );
        assert_eq!(answer["project"]["path"], json!("/work/strict-repo"));
        assert_eq!(
            answer["project"]["overrides"]["default-harness"]["harness"],
            json!("codex")
        );
        // Another project and the machine itself are untouched.
        assert_eq!(
            value_for_project("default-permission-mode", Some(Path::new("/work/other"))),
            Some(json!("full"))
        );
        assert_eq!(value("default-permission-mode"), Some(json!("full")));
        assert_eq!(
            value_for_project(
                "default-permission-mode",
                Some(Path::new("/work/strict-repo"))
            ),
            Some(json!("ask"))
        );

        // "Use the machine's value" removes the override only.
        write(SetSettingsRequest {
            clear: vec!["default-permission-mode".to_owned()],
            project: Some(project.to_owned()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(
            value_for_project("default-permission-mode", Some(Path::new(project))),
            Some(json!("full"))
        );
        assert_eq!(
            project_override("resume-after-restart", Path::new(project)),
            Some(json!(false))
        );
        let _ = std::fs::remove_file(settings_path().unwrap());
    }

    #[test]
    fn a_project_keeps_its_own_generator_and_voice() {
        let project = "/work/voice-repo";
        write(SetSettingsRequest {
            values: Map::from_iter([
                (
                    "text-generator".to_owned(),
                    json!({"id": "fast", "model": "small"}),
                ),
                (
                    "voice".to_owned(),
                    json!({"cleanup": true, "language": "nb"}),
                ),
            ]),
            project: Some(format!("{project}/")),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(
            project_override("text-generator", Path::new(project)),
            Some(json!({"id": "fast", "model": "small"}))
        );
        assert_eq!(
            project_override("voice", Path::new(project)).unwrap()["cleanup"],
            json!(true)
        );
        assert_eq!(
            project_override("text-generator", Path::new("/work/other")),
            None
        );
        write(SetSettingsRequest {
            clear: vec!["text-generator".to_owned(), "voice".to_owned()],
            project: Some(project.to_owned()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(project_override("voice", Path::new(project)), None);
    }

    #[test]
    fn a_project_cannot_override_a_machine_setting() {
        let refused = write(SetSettingsRequest {
            values: Map::from_iter([("delegates-total".to_owned(), json!(2))]),
            project: Some("/work/repo".to_owned()),
            ..Default::default()
        });
        assert!(refused.is_err());
        let wrong_generator = write(SetSettingsRequest {
            values: Map::from_iter([("text-generator".to_owned(), json!("fast"))]),
            project: Some("/work/repo".to_owned()),
            ..Default::default()
        });
        assert!(wrong_generator.is_err());
        let wrong = write(SetSettingsRequest {
            values: Map::from_iter([("default-harness".to_owned(), json!("codex"))]),
            project: Some("/work/repo".to_owned()),
            ..Default::default()
        });
        assert!(wrong.is_err());
    }

    #[test]
    fn a_hand_edited_file_with_unknown_keys_still_reads() {
        let path = settings_path().unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"{"version":1,"values":{"orchestration":false,"theme":"dark","delegates-total":"many"}}"#,
        )
        .unwrap();
        let values = read(GetSettingsRequest::default()).unwrap()["values"]
            .as_object()
            .unwrap()
            .clone();
        assert_eq!(values["orchestration"], json!(false));
        assert_eq!(values["delegates-total"], json!(8));
        assert!(!values.contains_key("theme"));
        assert_eq!(
            read(GetSettingsRequest::default()).unwrap()["stored"],
            json!(["orchestration"])
        );
        let _ = std::fs::remove_file(path);
    }
}
