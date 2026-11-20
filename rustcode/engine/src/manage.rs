//! Managing RustCode itself from an agent or a terminal (#3821): the server
//! settings in `settings.json`, and what threads can use (`capabilities`). One set of operations serves both
//! surfaces, so they cannot drift apart:
//!
//! - `manage_settings`, a tool of the `rustcode` MCP server that external
//!   harnesses (Claude Code, Codex and the rest) and outside agents call. Reads answer at once; a write asks the user through the
//!   thread's own approval card, which names the value before and after.
//! - `rc settings list|get|set|reset`, which sends the same
//!   `_rustcode/settings/*` request to the running server, or reads and writes
//!   the file itself when none runs.
//!
//! The engine watches the file and tells every client that reads settings
//! when it changes (`acp/settings.rs` › `start_watching`), so the app shows a
//! value set here at once, whoever wrote it.

use crate::acp::settings::{self, GetSettingsRequest, SetSettingsRequest};
use crate::orchestration::ask_user::Asker;
use agent_client_protocol::schema::v1::{
    PermissionOption, PermissionOptionKind, RequestPermissionOutcome, RequestPermissionRequest,
    ToolCallUpdate, ToolCallUpdateFields,
};
use serde_json::{Map, Value, json};

pub mod accounts;
pub mod capabilities;
pub mod threads;

pub use capabilities::MANAGE_CAPABILITIES;

pub const MANAGE_SETTINGS: &str = "manage_settings";

/// Every manage tool, for the places that route a call by name.
pub const TOOL_NAMES: [&str; 2] = [MANAGE_SETTINGS, MANAGE_CAPABILITIES];

/// What `manage_settings` may do.
pub const SETTINGS_ACTIONS: [&str; 4] = ["list", "get", "set", "reset"];

pub fn is_tool(name: &str) -> bool {
    TOOL_NAMES.contains(&name)
}

pub fn settings_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "action": {
                "type": "string",
                "enum": SETTINGS_ACTIONS,
                "description": "list: every setting with its value, default, type and accepted values; get: one key; set: change one key (the user approves); reset: back to the default, or with project to the machine's value (the user approves)"
            },
            "key": {"type": "string", "description": "The setting, as list names it, e.g. auto-compact-threshold"},
            "value": {"description": "set: the new value, of the type list gives (a number, true/false, one of the choices, or JSON for a list)"},
            "project": {"type": "string", "description": "Absolute folder of a project: read the values a thread there gets, or set and reset that project's own override"}
        },
        "required": ["action"],
        "additionalProperties": false
    })
}

fn action(args: &Value) -> &str {
    args.get("action").and_then(Value::as_str).unwrap_or("")
}

fn text<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
}

/// Check a call before anything is read or asked.
pub fn validate(tool: &str, args: &Value) -> Result<(), String> {
    if tool == MANAGE_CAPABILITIES {
        return capabilities::Op::parse(args).map(|_| ());
    }
    if tool != MANAGE_SETTINGS {
        return Err(format!("{tool} is not a manage tool"));
    }
    let action = action(args);
    if !SETTINGS_ACTIONS.contains(&action) {
        return Err(format!("action is one of {}", SETTINGS_ACTIONS.join(", ")));
    }
    match args.get("project") {
        None | Some(Value::Null) => {}
        Some(Value::String(path)) if std::path::Path::new(path).is_absolute() => {}
        Some(_) => return Err("project must be an absolute folder".to_owned()),
    }
    if action == "list" {
        return Ok(());
    }
    let Some(key) = text(args, "key") else {
        return Err(format!("{action} needs key"));
    };
    if !settings::is_key(key) {
        return Err(unknown(key));
    }
    if action == "set" && matches!(args.get("value"), None | Some(Value::Null)) {
        return Err("set needs value".to_owned());
    }
    Ok(())
}

/// Whether a call changes something, so the user approves it first.
pub fn is_write(tool: &str, args: &Value) -> bool {
    match tool {
        MANAGE_SETTINGS => matches!(action(args), "set" | "reset"),
        // A call that does not parse is refused before it runs; it is a
        // write for the scope check, so it never passes as a read.
        MANAGE_CAPABILITIES => capabilities::Op::parse(args).map_or(true, |op| op.is_write()),
        _ => false,
    }
}

/// How a write is approved.
pub enum Approval {
    /// Ask through the thread's own approval card.
    Ask { asker: Asker, session_id: String },
    /// Already approved: an outside agent's gate asked, or its grant does
    /// not ask.
    Granted,
    /// Nobody can be asked; the reason says why.
    Unavailable(&'static str),
}

/// A call's answer: whether it worked, and the JSON (or the refusal) the
/// agent reads.
pub struct Answer {
    pub success: bool,
    pub content: String,
}

impl Answer {
    fn ok(value: Value) -> Self {
        Self {
            success: true,
            content: value.to_string(),
        }
    }

    fn refused(message: impl Into<String>) -> Self {
        Self {
            success: false,
            content: format!("error: {}", message.into()),
        }
    }
}

/// Answer one manage call.
pub async fn answer(
    tool: &str,
    args: &Value,
    approval: Approval,
    cancel: &tokio_util::sync::CancellationToken,
) -> Answer {
    if tool == MANAGE_CAPABILITIES {
        return capabilities::answer(args, approval, cancel).await;
    }
    if let Err(reason) = validate(tool, args) {
        return Answer::refused(format!(
            "invalid {tool} call: {reason}. Nothing was changed."
        ));
    }
    let project = text(args, "project").map(str::to_owned);
    match action(args) {
        "list" => match list(project.as_deref()) {
            Ok(value) => Answer::ok(value),
            Err(error) => Answer::refused(error),
        },
        "get" => match get(text(args, "key").unwrap_or_default(), project.as_deref()) {
            Ok(value) => Answer::ok(value),
            Err(error) => Answer::refused(error),
        },
        write => {
            let key = text(args, "key").unwrap_or_default().to_owned();
            let change = match write {
                "set" => {
                    let raw = args.get("value").cloned().unwrap_or_default();
                    match typed_value(&key, raw) {
                        Ok(value) => Change::Set(value),
                        Err(error) => {
                            return Answer::refused(format!("{error}. Nothing was changed."));
                        }
                    }
                }
                _ => Change::Reset,
            };
            let before = match get(&key, project.as_deref()) {
                Ok(before) => before,
                Err(error) => return Answer::refused(error),
            };
            let after_value = match &change {
                Change::Set(value) => value.clone(),
                Change::Reset => reset_value(&key, project.as_deref()),
            };
            if before["value"] == after_value && matches!(change, Change::Set(_)) {
                return Answer::ok(json!({
                    "key": key,
                    "value": after_value,
                    "changed": false,
                    "source": before["source"],
                }));
            }
            let title = format!(
                "Change setting {key}{}: {} → {}",
                project
                    .as_deref()
                    .map(|path| format!(" in {path}"))
                    .unwrap_or_default(),
                shown(&before["value"]),
                shown(&after_value)
            );
            let hint = format!("rc settings {write} {key}");
            if let Err(refusal) = gate(approval, tool, &title, &hint, args, cancel).await {
                return refusal;
            }
            let applied = match change {
                Change::Set(value) => set(&key, value, project.as_deref()),
                Change::Reset => reset(&key, project.as_deref()),
            };
            match applied {
                Ok(_) => match get(&key, project.as_deref()) {
                    Ok(now) => Answer::ok(json!({
                        "key": key,
                        "before": before["value"],
                        "value": now["value"],
                        "source": now["source"],
                        "changed": true,
                    })),
                    Err(error) => Answer::refused(error),
                },
                Err(error) => Answer::refused(format!("{error}. Nothing was changed.")),
            }
        }
    }
}

/// Ask before a write, or say why nobody can be asked. `hint` is the
/// command the user can run instead.
async fn gate(
    approval: Approval,
    tool: &str,
    title: &str,
    hint: &str,
    args: &Value,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<(), Answer> {
    match approval {
        Approval::Granted => Ok(()),
        Approval::Unavailable(why) => Err(Answer::refused(format!(
            "{tool} needs the user's approval to {}, and {why}. Nothing was changed. Ask the user to run `{hint}` instead.",
            lower_first(title)
        ))),
        Approval::Ask { asker, session_id } => {
            if approved(&asker, &session_id, title, args, cancel).await {
                Ok(())
            } else {
                Err(Answer::refused(format!(
                    "the user declined: {title}. Nothing was changed. Do not retry it unless the user asks."
                )))
            }
        }
    }
}

fn lower_first(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}

enum Change {
    Set(Value),
    Reset,
}

/// A value as the agent sent it, in the key's type: `"false"` for a boolean
/// or `"200000"` for a number is read as the command line reads it.
fn typed_value(key: &str, raw: Value) -> Result<Value, String> {
    match raw {
        Value::String(text) => settings::parse_text(key, &text),
        other => {
            settings::check(key, &other)?;
            Ok(other)
        }
    }
}

/// What a reset leaves: the machine's value for a project override, else the
/// default.
fn reset_value(key: &str, project: Option<&str>) -> Value {
    if project.is_some()
        && let Ok(machine) = get(key, None)
    {
        return machine["value"].clone();
    }
    settings::describe()
        .into_iter()
        .find(|entry| entry["key"] == key)
        .map(|entry| entry["default"].clone())
        .unwrap_or_default()
}

/// A value in a card: short, and on one line.
fn shown(value: &Value) -> String {
    let text = match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    if text.chars().count() > 80 {
        format!("{}…", text.chars().take(79).collect::<String>())
    } else {
        text
    }
}

pub(crate) async fn approved(
    asker: &Asker,
    session_id: &str,
    title: &str,
    args: &Value,
    cancel: &tokio_util::sync::CancellationToken,
) -> bool {
    let call = ToolCallUpdate::new(
        format!("manage:{:016x}", rand::random::<u64>()),
        ToolCallUpdateFields::new()
            .title(title.to_owned())
            .raw_input(args.clone()),
    );
    let request = RequestPermissionRequest::new(
        session_id.to_owned(),
        call,
        vec![
            PermissionOption::new("manage_allow", "Allow", PermissionOptionKind::AllowOnce),
            PermissionOption::new("manage_deny", "Deny", PermissionOptionKind::RejectOnce),
        ],
    );
    let response = tokio::select! {
        _ = cancel.cancelled() => None,
        response = asker(request) => response,
    };
    matches!(
        response.map(|response| response.outcome),
        Some(RequestPermissionOutcome::Selected(selected))
            if selected.option_id.0.as_ref() == "manage_allow"
    )
}

// ---------------------------------------------------- the operations

fn unknown(key: &str) -> String {
    format!(
        "unknown setting '{key}'; the settings are {}",
        settings::keys().join(", ")
    )
}

/// `_rustcode/settings/get`'s answer as rows: each key with its value, where
/// it comes from, and what it accepts.
fn rows(answer: &Value) -> Value {
    let rows: Vec<Value> = settings::describe()
        .into_iter()
        .map(|mut entry| {
            let key = entry["key"].as_str().unwrap_or_default().to_owned();
            entry["value"] = answer["values"][&key].clone();
            entry["source"] = answer["sources"][&key].clone();
            entry
        })
        .collect();
    let mut out = json!({ "settings": rows });
    if let Some(project) = answer.get("project") {
        out["project"] = project.clone();
    }
    out
}

/// One row of [`rows`].
fn row(key: &str, answer: &Value) -> Value {
    let mut entry = settings::describe()
        .into_iter()
        .find(|entry| entry["key"] == key)
        .unwrap_or_default();
    entry["value"] = answer["values"][key].clone();
    entry["source"] = answer["sources"][key].clone();
    entry
}

fn get_params(key: Option<&str>, project: Option<&str>) -> Value {
    match key {
        Some(key) => json!({ "keys": [key], "project": project }),
        None => json!({ "project": project }),
    }
}

fn set_params(key: &str, value: Option<Value>, project: Option<&str>) -> Value {
    match value {
        Some(value) => {
            let mut values = Map::new();
            values.insert(key.to_owned(), value);
            json!({ "values": values, "project": project })
        }
        None => json!({ "clear": [key], "project": project }),
    }
}

const GET: &str = "_rustcode/settings/get";
const SET: &str = "_rustcode/settings/set";

/// Run a settings request on this machine's file, in this process. An agent's
/// call runs in the engine already, and the watcher tells the app.
fn local(method: &str, params: Value) -> Result<Value, String> {
    let result = if method == SET {
        let request: SetSettingsRequest =
            serde_json::from_value(params).map_err(|error| error.to_string())?;
        settings::write(request)
    } else {
        let request: GetSettingsRequest =
            serde_json::from_value(params).map_err(|error| error.to_string())?;
        settings::read(request)
    };
    result.map_err(|error| {
        error
            .data
            .as_ref()
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| error.message.clone())
    })
}

/// Every setting: its value as a thread in `project` (or on this machine)
/// reads it, where that value comes from, and what the key accepts.
pub fn list(project: Option<&str>) -> Result<Value, String> {
    Ok(rows(&local(GET, get_params(None, project))?))
}

/// One setting, as [`list`] shows it.
pub fn get(key: &str, project: Option<&str>) -> Result<Value, String> {
    if !settings::is_key(key) {
        return Err(unknown(key));
    }
    Ok(row(key, &local(GET, get_params(Some(key), project))?))
}

/// Set one key, typed already.
fn set(key: &str, value: Value, project: Option<&str>) -> Result<Value, String> {
    local(SET, set_params(key, Some(value), project))
}

/// Remove the machine's value (or the project's override), so the next read
/// falls back a layer.
fn reset(key: &str, project: Option<&str>) -> Result<Value, String> {
    local(SET, set_params(key, None, project))
}

/// A value typed on a command line, as `key` stores it.
pub fn parse_value(key: &str, text: &str) -> Result<Value, String> {
    settings::parse_text(key, text)
}

// ---------------------------------------------------- the command line

/// Where a CLI request was answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Via {
    /// The running server: its clients heard of the change.
    Server,
    /// No server runs: this process used the file.
    File,
}

impl Via {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Server => "server",
            Self::File => "file",
        }
    }
}

/// The request through the running server when there is one, so it is the
/// only writer of the file, else this process reads and writes the file.
async fn via_server(method: &str, params: Value) -> Result<(Value, Via), String> {
    match crate::server::ask_running(method, params.clone()).await {
        crate::server::Asked::Answer(answer) => Ok((answer, Via::Server)),
        crate::server::Asked::Refused(message) => Err(message),
        crate::server::Asked::NoServer(_) => {
            local(method, params).map(|answer| (answer, Via::File))
        }
    }
}

/// `rc settings list`.
pub async fn cli_list(project: Option<&str>) -> Result<Value, String> {
    Ok(rows(&via_server(GET, get_params(None, project)).await?.0))
}

/// `rc settings get`.
pub async fn cli_get(key: &str, project: Option<&str>) -> Result<Value, String> {
    if !settings::is_key(key) {
        return Err(unknown(key));
    }
    Ok(row(
        key,
        &via_server(GET, get_params(Some(key), project)).await?.0,
    ))
}

/// `rc settings set|reset` (`value` is `None` for a reset): write, then
/// read the key back, with the value before it and where it was written.
pub async fn cli_write(
    key: &str,
    value: Option<Value>,
    project: Option<&str>,
) -> Result<Value, String> {
    let before = cli_get(key, project).await?;
    let (_, via) = via_server(SET, set_params(key, value, project)).await?;
    let mut now = cli_get(key, project).await?;
    now["before"] = before["value"].clone();
    now["via"] = json!(via.as_str());
    Ok(now)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calls_are_checked_before_anything_is_read() {
        let ok = |args: Value| validate(MANAGE_SETTINGS, &args).is_ok();
        assert!(ok(json!({"action": "list"})));
        assert!(ok(json!({"action": "get", "key": "orchestration"})));
        assert!(ok(
            json!({"action": "set", "key": "orchestration", "value": false})
        ));
        assert!(ok(
            json!({"action": "reset", "key": "orchestration", "project": "/work/a"})
        ));
        assert!(!ok(json!({"action": "delete", "key": "orchestration"})));
        assert!(!ok(json!({"action": "get"})));
        assert!(!ok(json!({"action": "get", "key": "theme"})));
        assert!(!ok(json!({"action": "set", "key": "orchestration"})));
        assert!(!ok(json!({"action": "list", "project": "relative"})));
    }

    #[test]
    fn only_set_and_reset_change_things() {
        assert!(!is_write(MANAGE_SETTINGS, &json!({"action": "list"})));
        assert!(!is_write(MANAGE_SETTINGS, &json!({"action": "get"})));
        assert!(is_write(MANAGE_SETTINGS, &json!({"action": "set"})));
        assert!(is_write(MANAGE_SETTINGS, &json!({"action": "reset"})));
    }

    #[test]
    fn a_value_as_text_is_read_in_the_key_type() {
        assert_eq!(
            typed_value("orchestration", json!("off")).unwrap(),
            json!(false)
        );
        assert_eq!(
            typed_value("auto-compact-threshold", json!("200_000")).unwrap(),
            json!(200_000)
        );
        assert_eq!(
            typed_value("auto-compact-mode", json!("cache")).unwrap(),
            json!("cache")
        );
        assert!(typed_value("auto-compact-mode", json!("sometimes")).is_err());
        assert!(typed_value("delegates-total", json!(true)).is_err());
        assert!(typed_value("default-permission-mode", json!("yolo")).is_err());
    }

    #[tokio::test]
    async fn a_write_without_anyone_to_ask_is_refused_and_changes_nothing() {
        let cancel = tokio_util::sync::CancellationToken::new();
        let answer = answer(
            MANAGE_SETTINGS,
            &json!({"action": "set", "key": "wake-batch-seconds", "value": 5}),
            Approval::Unavailable("no app is attached"),
            &cancel,
        )
        .await;
        assert!(!answer.success, "{}", answer.content);
        assert!(answer.content.contains("rc settings set"));
        assert_eq!(get("wake-batch-seconds", None).unwrap()["value"], json!(60));
    }

    #[tokio::test]
    async fn a_granted_write_answers_before_and_after_and_a_reset_restores_the_default() {
        let cancel = tokio_util::sync::CancellationToken::new();
        let set = answer(
            MANAGE_SETTINGS,
            &json!({"action": "set", "key": "workflow-concurrency", "value": "5"}),
            Approval::Granted,
            &cancel,
        )
        .await;
        assert!(set.success, "{}", set.content);
        let set: Value = serde_json::from_str(&set.content).unwrap();
        assert_eq!(set["before"], json!(3));
        assert_eq!(set["value"], json!(5));
        assert_eq!(set["source"], json!("machine"));
        let reset = answer(
            MANAGE_SETTINGS,
            &json!({"action": "reset", "key": "workflow-concurrency"}),
            Approval::Granted,
            &cancel,
        )
        .await;
        let reset: Value = serde_json::from_str(&reset.content).unwrap();
        assert_eq!(reset["value"], json!(3));
        assert_eq!(reset["source"], json!("default"));
    }

    #[test]
    fn list_names_every_key_with_its_type_default_and_choices() {
        let listed = list(None).unwrap();
        let rows = listed["settings"].as_array().unwrap();
        assert_eq!(rows.len(), settings::keys().len());
        let mode = rows
            .iter()
            .find(|row| row["key"] == "auto-compact-mode")
            .unwrap();
        assert_eq!(mode["type"], json!("text"));
        assert_eq!(mode["choices"], json!(["off", "size", "cache"]));
        assert_eq!(mode["project"], json!(true));
        assert!(
            rows.iter()
                .all(|row| row["label"].as_str().is_some_and(|l| !l.is_empty()))
        );
    }
}
