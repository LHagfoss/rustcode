//! `manage_capabilities` and `rc skill|mcp|harness …` (#3821): what a
//! thread can use. Skills and MCP servers are turned on and off (and an MCP
//! server restarted), a skill is created, the default harness for new threads is chosen, and the
//! theme is switched. The engine's own methods do the work
//! (`_rustcode/capabilities*`, `_rustcode/harnesses/default`,
//! `_rustcode/themes/set`), so the app's Settings and these surfaces change
//! the same files.

use super::{Answer, Approval, gate};
use serde_json::{Value, json};

pub const MANAGE_CAPABILITIES: &str = "manage_capabilities";

/// What a call manages, and what it may do to it.
pub const KINDS: [&str; 4] = ["skill", "mcp", "harness", "theme"];

fn actions(kind: &str) -> &'static [&'static str] {
    match kind {
        "skill" => &["list", "enable", "disable", "create"],
        "mcp" => &["list", "enable", "disable", "restart"],
        "harness" => &["list", "default"],
        "theme" => &["list", "set"],
        _ => &[],
    }
}

pub fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "kind": {"type": "string", "enum": KINDS, "description": "skill, mcp (an MCP server), harness, or theme"},
            "action": {
                "type": "string",
                "enum": ["list", "enable", "disable", "restart", "default", "set", "create"],
                "description": "list (any kind); enable or disable a skill or MCP server; restart an MCP server; default: the harness new threads start on; set: the theme; create: a new user skill from content. Everything but list asks the user first"
            },
            "name": {"type": "string", "description": "The skill, MCP server, harness id or theme, as list names it. create: the new skill's name (lowercase letters, digits, dashes)"},
            "content": {"type": "string", "description": "create: the whole SKILL.md, starting with front matter (name, description saying what it does and when to use it, optional triggers and keywords), then the instructions"},
            "model": {"type": "string", "description": "default: the model new threads on that harness start with"},
            "effort": {"type": "string", "description": "default: the reasoning effort they start with"}
        },
        "required": ["kind", "action"],
        "additionalProperties": false
    })
}

fn text<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
}

/// One change, or a list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    List(String),
    Toggle {
        kind: String,
        name: String,
        enabled: bool,
    },
    Restart(String),
    Default {
        harness: String,
        model: Option<String>,
        effort: Option<String>,
    },
    Theme(String),
    /// A new skill in the user's RustCode skills folder.
    CreateSkill {
        name: String,
        content: String,
    },
}

impl Op {
    pub fn parse(args: &Value) -> Result<Self, String> {
        let kind = text(args, "kind").unwrap_or_default();
        if !KINDS.contains(&kind) {
            return Err(format!("kind is one of {}", KINDS.join(", ")));
        }
        let action = text(args, "action").unwrap_or_default();
        if !actions(kind).contains(&action) {
            return Err(format!(
                "a {kind} takes action {}",
                actions(kind).join(", ")
            ));
        }
        if action == "list" {
            return Ok(Self::List(kind.to_owned()));
        }
        let Some(name) = text(args, "name").map(str::to_owned) else {
            return Err(format!("{action} needs name"));
        };
        Ok(match action {
            "enable" | "disable" => Self::Toggle {
                kind: kind.to_owned(),
                name,
                enabled: action == "enable",
            },
            "restart" => Self::Restart(name),
            "create" => {
                let Some(content) = args
                    .get("content")
                    .and_then(Value::as_str)
                    .filter(|content| !content.trim().is_empty())
                else {
                    return Err("create needs content, the whole SKILL.md".to_owned());
                };
                Self::CreateSkill {
                    name,
                    content: content.to_owned(),
                }
            }
            "default" => Self::Default {
                harness: name,
                model: text(args, "model").map(str::to_owned),
                effort: text(args, "effort").map(str::to_owned),
            },
            _ => Self::Theme(name),
        })
    }

    pub fn is_write(&self) -> bool {
        !matches!(self, Self::List(_))
    }

    /// The approval card's title.
    fn title(&self, before: &Value) -> String {
        match self {
            Self::List(kind) => format!("List {kind}"),
            Self::Toggle {
                kind,
                name,
                enabled,
            } => format!(
                "Turn {} {} {name}",
                if *enabled { "on" } else { "off" },
                if kind == "mcp" { "MCP server" } else { "skill" }
            ),
            Self::Restart(name) => format!("Restart MCP server {name}"),
            Self::CreateSkill { name, .. } => format!("Create skill {name}"),
            Self::Default {
                harness,
                model,
                effort,
            } => {
                let mut after = harness.clone();
                if let Some(model) = model {
                    after.push_str(&format!(" with {model}"));
                }
                if let Some(effort) = effort {
                    after.push_str(&format!(" at {effort} effort"));
                }
                let was = before["harness"].as_str().unwrap_or("the built-in harness");
                format!("Change the default harness for new threads: {was} → {after}")
            }
            Self::Theme(name) => format!(
                "Change the theme: {} → {name}",
                before.as_str().unwrap_or("the current one")
            ),
        }
    }

    /// The command a person runs for the same change.
    fn hint(&self) -> String {
        match self {
            Self::List(kind) => format!("rc {kind} list"),
            Self::Toggle {
                kind,
                name,
                enabled,
            } => format!(
                "rc {kind} {} {name}",
                if *enabled { "enable" } else { "disable" }
            ),
            Self::Restart(name) => format!("rc mcp restart {name}"),
            Self::CreateSkill { name, .. } => {
                format!("write ~/.config/rustcode/skills/{name}/SKILL.md")
            }
            Self::Default { harness, .. } => format!("rc harness default {harness}"),
            Self::Theme(name) => format!("rc theme set {name}"),
        }
    }

    /// The engine request that makes the change or the list.
    fn request(&self) -> (&'static str, Value) {
        match self {
            Self::List(kind) => match kind.as_str() {
                "harness" => ("_rustcode/harnesses", json!({})),
                "theme" => ("_rustcode/themes", json!({})),
                _ => ("_rustcode/capabilities", json!({})),
            },
            Self::Toggle {
                kind,
                name,
                enabled,
            } => (
                "_rustcode/capabilities/set",
                json!({ "kind": kind, "name": name, "enabled": enabled }),
            ),
            Self::Restart(name) => (
                "_rustcode/capabilities/mcp/restart",
                json!({ "name": name }),
            ),
            Self::CreateSkill { name, content } => (
                "_rustcode/skills/add",
                json!({ "name": name, "content": content }),
            ),
            Self::Default {
                harness,
                model,
                effort,
            } => (
                "_rustcode/harnesses/default",
                json!({ "harness": harness, "model": model, "effort": effort }),
            ),
            Self::Theme(name) => ("_rustcode/themes/set", json!({ "name": name })),
        }
    }
}

/// Where a request runs.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Place {
    /// In this engine process, which runs the MCP servers: an agent's call.
    Engine,
    /// A CLI with no server to ask: config files only, nothing is started.
    Files,
}

async fn run_here(op: &Op, place: Place) -> Result<Value, String> {
    match op {
        Op::List(kind) => match kind.as_str() {
            "harness" => crate::harness::snapshot().await,
            "theme" => serde_json::to_value(crate::themes::snapshot(None))
                .map_err(|error| error.to_string()),
            _ => serde_json::to_value(crate::capabilities::snapshot())
                .map_err(|error| error.to_string()),
        },
        Op::Toggle {
            kind,
            name,
            enabled,
        } => {
            if kind == "skill" {
                crate::capabilities::set_skill_enabled(name, *enabled)?;
            } else if place == Place::Engine {
                crate::capabilities::set_mcp_server_enabled(name, *enabled).await?;
            } else {
                let dir = crate::config::get_config_dir()
                    .ok_or("could not determine the config directory")?;
                crate::config::set_mcp_server_enabled_in(&dir, name, *enabled)?;
            }
            serde_json::to_value(crate::capabilities::snapshot()).map_err(|error| error.to_string())
        }
        Op::Restart(name) => {
            if place == Place::Files {
                return Err(format!(
                    "no RustCode server is running, so nothing runs {name} to restart; it starts with the next thread"
                ));
            }
            crate::capabilities::restart_mcp_server(name).await?;
            serde_json::to_value(crate::capabilities::snapshot()).map_err(|error| error.to_string())
        }
        Op::CreateSkill { name, content } => {
            let path = crate::capabilities::add_skill(name, content)?;
            let mut snapshot = serde_json::to_value(crate::capabilities::snapshot())
                .map_err(|error| error.to_string())?;
            snapshot["created"] = json!(path);
            Ok(snapshot)
        }
        Op::Default {
            harness,
            model,
            effort,
        } => crate::harness::set_default(harness.clone(), model.clone(), effort.clone()).await,
        Op::Theme(name) => crate::themes::set_active(name, None)
            .map_err(|error| error.to_string())
            .and_then(|snapshot| serde_json::to_value(snapshot).map_err(|error| error.to_string())),
    }
}

/// What a list shows, short: no descriptions beyond a line, no paths, no
/// header or environment values (the snapshot never has those).
fn shaped(op: &Op, raw: &Value) -> Value {
    let kind = match op {
        Op::List(kind) => kind.as_str(),
        Op::Toggle { kind, .. } => kind.as_str(),
        Op::Restart(_) => "mcp",
        Op::CreateSkill { .. } => "skill",
        Op::Default { .. } => "harness",
        Op::Theme(_) => "theme",
    };
    let rows = |key: &str| raw[key].as_array().cloned().unwrap_or_default();
    if let Op::CreateSkill { name, .. } = op {
        // The path the engine wrote, or the one `_rustcode/skills/add` answers.
        let path = raw["created"].as_str().or_else(|| raw["path"].as_str());
        return json!({ "created": name, "path": path });
    }
    match kind {
        "skill" => json!({ "skills": rows("skills").iter().map(|skill| json!({
            "name": skill["name"],
            "enabled": skill["enabled"],
            "source": skill["source"],
            "read_only": skill["read_only"],
            "description": one_line(skill["description"].as_str().unwrap_or_default()),
        })).collect::<Vec<_>>() }),
        "mcp" => json!({ "mcp_servers": rows("mcp_servers").iter().map(|server| json!({
            "name": server["name"],
            "enabled": server["enabled"],
            "transport": server["transport"],
            "scope": server["scope"],
            "connection": server["connection"],
            "tools": server["tools"].as_array().map_or(0, Vec::len),
            "error": server["error"],
        })).collect::<Vec<_>>() }),
        "harness" => {
            let default = raw["default"].clone();
            json!({
                "default": default,
                "harnesses": rows("harnesses").iter().map(|row| json!({
                    "id": row["id"],
                    "name": row["name"],
                    "installed": row["installed"],
                    "auth": row["auth"],
                    "default": row["id"] == default["harness"],
                    "accounts": row["accounts"].as_array().map(|accounts| accounts.iter()
                        .map(|account| account.get("id").cloned().unwrap_or(Value::Null))
                        .collect::<Vec<_>>()).unwrap_or_default(),
                })).collect::<Vec<_>>(),
            })
        }
        _ => json!({
            "active": raw["active"],
            "themes": rows("themes").iter().map(|theme| theme["name"].clone()).collect::<Vec<_>>(),
        }),
    }
}

fn one_line(text: &str) -> String {
    let line = text.lines().next().unwrap_or_default().trim();
    if line.chars().count() > 120 {
        format!("{}…", line.chars().take(119).collect::<String>())
    } else {
        line.to_owned()
    }
}

/// What the card compares against: the current default harness, or theme.
async fn before(op: &Op) -> Value {
    match op {
        Op::Default { .. } => crate::harness::snapshot()
            .await
            .map(|snapshot| snapshot["default"].clone())
            .unwrap_or_default(),
        Op::Theme(_) => json!(crate::themes::snapshot(None).active),
        _ => Value::Null,
    }
}

/// Answer one `manage_capabilities` call in the engine.
pub async fn answer(
    args: &Value,
    approval: Approval,
    cancel: &tokio_util::sync::CancellationToken,
) -> Answer {
    let op = match Op::parse(args) {
        Ok(op) => op,
        Err(reason) => {
            return Answer::refused(format!(
                "invalid {MANAGE_CAPABILITIES} call: {reason}. Nothing was changed."
            ));
        }
    };
    if op.is_write() {
        let title = op.title(&before(&op).await);
        if let Err(refusal) = gate(
            approval,
            MANAGE_CAPABILITIES,
            &title,
            &op.hint(),
            args,
            cancel,
        )
        .await
        {
            return refusal;
        }
    }
    match run_here(&op, Place::Engine).await {
        Ok(raw) => Answer::ok(shaped(&op, &raw)),
        Err(error) => Answer::refused(format!("{error}. Nothing was changed.")),
    }
}

/// The CLI's call: through the running server when there is one, which runs
/// the MCP servers and tells its clients, else on the config files here.
pub async fn cli(op: &Op) -> Result<(Value, super::Via), String> {
    let (method, params) = op.request();
    match crate::server::ask_running(method, params).await {
        crate::server::Asked::Answer(raw) => Ok((shaped(op, &raw), super::Via::Server)),
        crate::server::Asked::Refused(message) => Err(message),
        crate::server::Asked::NoServer(_) => run_here(op, Place::Files)
            .await
            .map(|raw| (shaped(op, &raw), super::Via::File)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_call_parses_to_one_operation_or_says_what_it_takes() {
        assert_eq!(
            Op::parse(&json!({"kind": "skill", "action": "list"})),
            Ok(Op::List("skill".into()))
        );
        assert_eq!(
            Op::parse(&json!({"kind": "mcp", "action": "disable", "name": "files"})),
            Ok(Op::Toggle {
                kind: "mcp".into(),
                name: "files".into(),
                enabled: false
            })
        );
        assert_eq!(
            Op::parse(
                &json!({"kind": "harness", "action": "default", "name": "codex", "model": "gpt-5"})
            ),
            Ok(Op::Default {
                harness: "codex".into(),
                model: Some("gpt-5".into()),
                effort: None
            })
        );
        assert!(
            Op::parse(&json!({"kind": "skill", "action": "restart", "name": "x"}))
                .unwrap_err()
                .contains("list, enable, disable")
        );
        assert!(Op::parse(&json!({"kind": "theme", "action": "set"})).is_err());
        assert_eq!(
            Op::parse(
                &json!({"kind": "skill", "action": "create", "name": "notes", "content": "---\nname: notes\n---\n"})
            ),
            Ok(Op::CreateSkill {
                name: "notes".into(),
                content: "---\nname: notes\n---\n".into()
            })
        );
        assert!(
            Op::parse(&json!({"kind": "skill", "action": "create", "name": "notes"}))
                .unwrap_err()
                .contains("content")
        );
        assert!(
            Op::parse(&json!({"kind": "mcp", "action": "create", "name": "x", "content": "y"}))
                .is_err()
        );
        assert!(Op::parse(&json!({"kind": "account", "action": "list"})).is_err());
        assert!(!Op::List("mcp".into()).is_write());
        assert!(Op::Restart("files".into()).is_write());
    }

    #[test]
    fn titles_name_the_change_and_hints_the_command() {
        let default = Op::Default {
            harness: "codex".into(),
            model: Some("gpt-5".into()),
            effort: Some("high".into()),
        };
        assert_eq!(
            default.title(&json!({"harness": "claude-code"})),
            "Change the default harness for new threads: claude-code → codex with gpt-5 at high effort"
        );
        assert_eq!(default.hint(), "rc harness default codex");
        let off = Op::Toggle {
            kind: "skill".into(),
            name: "review".into(),
            enabled: false,
        };
        assert_eq!(off.title(&Value::Null), "Turn off skill review");
        assert_eq!(off.hint(), "rc skill disable review");
        let create = Op::CreateSkill {
            name: "notes".into(),
            content: "x".into(),
        };
        assert!(create.is_write());
        assert_eq!(create.title(&Value::Null), "Create skill notes");
        assert_eq!(
            shaped(&create, &json!({"path": "/u/skills/notes/SKILL.md"})),
            json!({"created": "notes", "path": "/u/skills/notes/SKILL.md"})
        );
    }

    #[test]
    fn a_list_is_short_and_carries_no_values_of_secrets() {
        let raw = json!({"mcp_servers": [{
            "name": "api", "enabled": true, "transport": "http", "scope": "user",
            "connection": "connected", "tools": ["a", "b"], "error": null,
            "target": "https://example.com/mcp", "header_keys": ["Authorization"]
        }]});
        let shaped = shaped(&Op::List("mcp".into()), &raw);
        assert_eq!(shaped["mcp_servers"][0]["tools"], json!(2));
        let text = shaped.to_string();
        assert!(!text.contains("Authorization") && !text.contains("example.com"));
    }

    #[tokio::test]
    async fn a_write_without_anyone_to_ask_names_the_command_and_changes_nothing() {
        let answer = answer(
            &json!({"kind": "theme", "action": "set", "name": "light"}),
            Approval::Unavailable("no app is attached"),
            &tokio_util::sync::CancellationToken::new(),
        )
        .await;
        assert!(!answer.success);
        assert!(
            answer.content.contains("rc theme set light"),
            "{}",
            answer.content
        );
    }
}
