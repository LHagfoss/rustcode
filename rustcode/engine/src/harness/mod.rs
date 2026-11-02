//! External coding-agent harnesses (`docs/harnesses.md`).
//!
//! A harness runs a thread's turns. RustCode's own turn loop is the built-in
//! harness `rustcode`; this module adds external ones: an ACP agent driven
//! over its stdio (kind `acp`), or `codex app-server` (kind `codex`). For an
//! external thread the engine is a proxy: it spawns the harness, translates
//! its output into ordinary ACP `session/update` notifications and
//! `session/request_permission` requests, and records the transcript with
//! the thread.

pub(crate) mod acp_child;
pub(crate) mod codex;
pub mod config;
pub(crate) mod detect;
pub(crate) mod process;
pub(crate) mod rpc;
pub(crate) mod session;
pub(crate) mod store;
pub(crate) mod transcript;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

pub(crate) const BUILTIN_ID: &str = "rustcode";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessKind {
    Builtin,
    Acp,
    Codex,
}

/// What a harness supports. Clients read these flags and never branch on
/// the harness id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Capabilities {
    pub steer: &'static str,
    pub cancel: bool,
    pub resume: bool,
    pub rewind: bool,
    pub images: bool,
    pub models: &'static str,
    pub effort: bool,
    pub permission_modes: Vec<&'static str>,
    pub subagents: bool,
    pub slash_commands: bool,
    pub smart_ui: bool,
    pub rustcode_extensions: bool,
    /// Whether the thread gets the orchestration tools: built in for
    /// RustCode, through the injected `rustcode mcp-orchestrator` for an
    /// external harness (`docs/orchestrator.md`).
    pub orchestration: bool,
    /// The engine snapshots the thread before every turn when its cwd is
    /// in a git repository (`_rustcode/checkpoints/*`).
    pub checkpoints: bool,
    /// The thread can start in its own git worktree (`session/new`
    /// `_meta.rustcode.worktree`); the engine prepares the cwd for any harness.
    pub worktree: bool,
}

impl Capabilities {
    fn builtin() -> Self {
        Self {
            steer: "live",
            cancel: true,
            resume: true,
            rewind: true,
            images: true,
            models: "static",
            effort: true,
            permission_modes: vec!["default", "auto_edit", "full_access"],
            subagents: true,
            slash_commands: true,
            smart_ui: true,
            rustcode_extensions: true,
            orchestration: true,
            checkpoints: true,
            worktree: true,
        }
    }

    fn external(kind: HarnessKind) -> Self {
        Self {
            // `turn/steer` adds input to Codex's running turn; ACP has no
            // way to, so the client sends the message as the next prompt.
            steer: if kind == HarnessKind::Codex {
                "live"
            } else {
                "none"
            },
            cancel: true,
            resume: true,
            rewind: false,
            images: true,
            models: "dynamic",
            effort: kind == HarnessKind::Codex,
            // RustCode never imitates a mode by auto-answering approvals,
            // and no external harness exposes its own modes through here yet.
            permission_modes: Vec::new(),
            subagents: false,
            slash_commands: kind == HarnessKind::Acp,
            smart_ui: false,
            rustcode_extensions: false,
            // Both kinds take a per-thread MCP server (ACP `mcpServers`,
            // Codex `config.mcp_servers`); it needs a unix socket.
            orchestration: cfg!(unix),
            checkpoints: true,
            worktree: true,
        }
    }
}

/// A harness RustCode knows how to start without configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct KnownHarness {
    pub id: &'static str,
    pub name: &'static str,
    pub kind: HarnessKind,
    pub command: &'static [&'static str],
    pub login_command: Option<&'static [&'static str]>,
    pub install_hint: Option<&'static str>,
    /// Arguments that print the version, run on `version_program`.
    pub version: Option<(&'static str, &'static [&'static str])>,
    /// A cheap command whose exit status says whether the user is signed in.
    pub auth_probe: Option<&'static [&'static str]>,
}

pub(crate) const KNOWN: &[KnownHarness] = &[
    KnownHarness {
        id: "codex",
        name: "Codex",
        kind: HarnessKind::Codex,
        command: &["codex", "app-server"],
        login_command: Some(&["codex", "login"]),
        install_hint: Some("brew install codex"),
        version: Some(("codex", &["--version"])),
        auth_probe: Some(&["codex", "login", "status"]),
    },
    KnownHarness {
        id: "claude-code",
        name: "Claude Code",
        kind: HarnessKind::Acp,
        command: &["npx", "-y", "@agentclientprotocol/claude-agent-acp"],
        login_command: Some(&["claude", "/login"]),
        install_hint: Some("npm install -g @anthropic-ai/claude-code"),
        version: Some(("claude", &["--version"])),
        auth_probe: None,
    },
    KnownHarness {
        id: "gemini",
        name: "Gemini CLI",
        kind: HarnessKind::Acp,
        command: &["gemini", "--experimental-acp"],
        login_command: Some(&["gemini"]),
        install_hint: Some("npm install -g @google/gemini-cli"),
        version: Some(("gemini", &["--version"])),
        auth_probe: None,
    },
    KnownHarness {
        id: "opencode",
        name: "OpenCode",
        kind: HarnessKind::Acp,
        command: &["opencode", "acp"],
        login_command: Some(&["opencode", "auth", "login"]),
        install_hint: Some("curl -fsSL https://opencode.ai/install | bash"),
        version: Some(("opencode", &["--version"])),
        auth_probe: None,
    },
];

pub(crate) fn known(id: &str) -> Option<&'static KnownHarness> {
    KNOWN.iter().find(|harness| harness.id == id)
}

/// A harness with its configuration applied: what a thread starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedHarness {
    pub id: String,
    pub name: String,
    pub kind: HarnessKind,
    pub command: Vec<String>,
    pub enabled: bool,
    pub known: Option<&'static KnownHarness>,
}

fn builtin_resolved() -> ResolvedHarness {
    ResolvedHarness {
        id: BUILTIN_ID.to_owned(),
        name: "RustCode".to_owned(),
        kind: HarnessKind::Builtin,
        command: Vec::new(),
        enabled: true,
        known: None,
    }
}

/// Every harness in snapshot order: the built-in one, the known ones, then
/// custom entries in config order.
pub(crate) fn resolve_all(entries: &[config::HarnessEntry]) -> Vec<ResolvedHarness> {
    let mut all = vec![builtin_resolved()];
    for known in KNOWN {
        let entry = entries.iter().find(|entry| entry.id == known.id);
        all.push(ResolvedHarness {
            id: known.id.to_owned(),
            name: entry
                .and_then(|entry| entry.name.clone())
                .unwrap_or_else(|| known.name.to_owned()),
            kind: known.kind,
            command: entry
                .and_then(|entry| entry.command.clone())
                .unwrap_or_else(|| {
                    known
                        .command
                        .iter()
                        .map(|part| (*part).to_owned())
                        .collect()
                }),
            enabled: entry.is_some_and(config::HarnessEntry::is_enabled),
            known: Some(known),
        });
    }
    for entry in entries {
        if entry.id == BUILTIN_ID || known(&entry.id).is_some() {
            continue;
        }
        let Some(command) = entry.command.clone().filter(|command| !command.is_empty()) else {
            continue;
        };
        all.push(ResolvedHarness {
            id: entry.id.clone(),
            name: entry.name.clone().unwrap_or_else(|| entry.id.clone()),
            kind: match entry.kind {
                Some(HarnessKind::Codex) => HarnessKind::Codex,
                _ => HarnessKind::Acp,
            },
            command,
            enabled: entry.is_enabled(),
            known: None,
        });
    }
    all
}

pub(crate) fn resolve(entries: &[config::HarnessEntry], id: &str) -> Option<ResolvedHarness> {
    resolve_all(entries)
        .into_iter()
        .find(|harness| harness.id == id)
}

/// What the engine learned about a harness while running it.
#[derive(Debug, Clone, Default)]
pub(crate) struct Learned {
    pub models: Option<Vec<HarnessModel>>,
    pub error: Option<String>,
    pub resume: Option<bool>,
    pub images: Option<bool>,
    pub effort: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct HarnessModel {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub efforts: Vec<String>,
    #[serde(default)]
    pub default_effort: Option<String>,
}

fn learned() -> &'static Mutex<HashMap<String, Learned>> {
    static LEARNED: OnceLock<Mutex<HashMap<String, Learned>>> = OnceLock::new();
    LEARNED.get_or_init(Default::default)
}

pub(crate) fn learn(id: &str, update: impl FnOnce(&mut Learned)) {
    let mut learned = learned().lock().unwrap_or_else(|error| error.into_inner());
    update(learned.entry(id.to_owned()).or_default());
}

pub(crate) fn learned_for(id: &str) -> Learned {
    learned()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(id)
        .cloned()
        .unwrap_or_default()
}

pub(crate) fn capabilities(harness: &ResolvedHarness) -> Capabilities {
    if harness.kind == HarnessKind::Builtin {
        return Capabilities::builtin();
    }
    let mut capabilities = Capabilities::external(harness.kind);
    let learned = learned_for(&harness.id);
    if let Some(resume) = learned.resume {
        capabilities.resume = resume;
    }
    if let Some(images) = learned.images {
        capabilities.images = images;
    }
    if let Some(effort) = learned.effort {
        capabilities.effort = effort;
    }
    capabilities
}

/// The thread metadata that names its harness, for `session/new` answers.
/// `orchestration` says whether this thread got the orchestration tools.
pub(crate) fn session_meta(
    harness: &ResolvedHarness,
    orchestration: bool,
) -> serde_json::Map<String, Value> {
    let mut capabilities = capabilities(harness);
    capabilities.orchestration = orchestration;
    serde_json::Map::from_iter([(
        "rustcode".to_owned(),
        json!({ "harness": { "id": harness.id, "capabilities": capabilities } }),
    )])
}

/// One `harnesses` row of the snapshot.
fn snapshot_entry(harness: &ResolvedHarness, detection: &detect::Detection) -> Value {
    let learned = learned_for(&harness.id);
    let builtin = harness.kind == HarnessKind::Builtin;
    let models = if builtin {
        builtin_models()
    } else {
        learned.models.clone().unwrap_or_default()
    };
    json!({
        "id": harness.id,
        "name": harness.name,
        "kind": harness.kind,
        "builtin": builtin,
        "enabled": harness.enabled,
        "command": harness.command,
        "installed": detection.installed,
        "version": detection.version,
        "auth": detection.auth,
        "login_command": harness.known.and_then(|known| known.login_command),
        "install_hint": harness.known.and_then(|known| known.install_hint),
        "capabilities": capabilities(harness),
        "models": models,
        "error": learned.error.or_else(|| detection.error.clone()),
    })
}

/// The built-in harness's model profiles.
fn builtin_models() -> Vec<HarnessModel> {
    let (_, _, config) = crate::config::load_config();
    config
        .models
        .iter()
        .map(|profile| HarnessModel {
            id: profile.name.clone(),
            name: profile.name.clone(),
            efforts: Vec::new(),
            default_effort: None,
        })
        .collect()
}

/// `_rustcode/harnesses`: every harness with its detection, and the
/// defaults.
pub(crate) async fn snapshot() -> Result<Value, String> {
    let dir = config::config_dir()?;
    snapshot_in(&dir).await
}

pub(crate) async fn snapshot_in(dir: &std::path::Path) -> Result<Value, String> {
    let (defaults, entries) = config::load_in(dir);
    let harnesses = resolve_all(&entries);
    let detections = futures_detect(&harnesses).await;
    let rows = harnesses
        .iter()
        .zip(detections.iter())
        .map(|(harness, detection)| snapshot_entry(harness, detection))
        .collect::<Vec<_>>();
    Ok(json!({
        "harnesses": rows,
        "default": {
            "harness": defaults.harness.clone().unwrap_or_else(|| BUILTIN_ID.to_owned()),
            "model": defaults.model,
            "effort": defaults.effort,
        },
        "onboarded": defaults.onboarded,
    }))
}

async fn futures_detect(harnesses: &[ResolvedHarness]) -> Vec<detect::Detection> {
    let tasks = harnesses
        .iter()
        .cloned()
        .map(|harness| tokio::task::spawn_blocking(move || detect::detect(&harness)))
        .collect::<Vec<_>>();
    let mut detections = Vec::with_capacity(tasks.len());
    for task in tasks {
        detections.push(task.await.unwrap_or_default());
    }
    detections
}

/// `_rustcode/harnesses/set`.
pub(crate) async fn set(change: config::HarnessChange) -> Result<Value, String> {
    let dir = config::config_dir()?;
    let id = change.id.clone();
    config::edit_in(&dir, |_, harnesses| config::apply_change(harnesses, change))?;
    detect::forget(&id);
    snapshot_in(&dir).await
}

/// `_rustcode/harnesses/remove`.
pub(crate) async fn remove(id: &str) -> Result<Value, String> {
    let dir = config::config_dir()?;
    config::edit_in(&dir, |_, harnesses| config::remove(harnesses, id))?;
    detect::forget(id);
    snapshot_in(&dir).await
}

/// `_rustcode/harnesses/default`.
pub(crate) async fn set_default(
    harness: String,
    model: Option<String>,
    effort: Option<String>,
) -> Result<Value, String> {
    let dir = config::config_dir()?;
    let (_, entries) = config::load_in(&dir);
    if resolve(&entries, &harness).is_none() {
        return Err(format!("unknown harness '{harness}'"));
    }
    config::edit_in(&dir, |defaults, _| {
        defaults.harness = (harness != BUILTIN_ID).then_some(harness);
        defaults.model = model.filter(|model| !model.trim().is_empty());
        defaults.effort = effort.filter(|effort| !effort.trim().is_empty());
        Ok(())
    })?;
    snapshot_in(&dir).await
}

/// `_rustcode/onboarding/complete`.
pub(crate) async fn complete_onboarding() -> Result<Value, String> {
    let dir = config::config_dir()?;
    config::edit_in(&dir, |defaults, _| {
        defaults.onboarded = true;
        Ok(())
    })?;
    snapshot_in(&dir).await
}

/// `_rustcode/harnesses/models`: start the harness briefly if its models are
/// not known yet.
pub(crate) async fn models(id: &str) -> Result<Vec<HarnessModel>, String> {
    let dir = config::config_dir()?;
    let (_, entries) = config::load_in(&dir);
    let harness = resolve(&entries, id).ok_or_else(|| format!("unknown harness '{id}'"))?;
    if harness.kind == HarnessKind::Builtin {
        return Ok(builtin_models());
    }
    if let Some(models) = learned_for(id).models {
        return Ok(models);
    }
    let listed = session::list_models(&harness).await;
    match listed {
        Ok(models) => {
            learn(id, |learned| {
                learned.models = Some(models.clone());
                learned.error = None;
            });
            Ok(models)
        }
        Err(error) => {
            learn(id, |learned| learned.error = Some(error.clone()));
            Err(error)
        }
    }
}

/// The harness a `session/new` asked for in `_meta.rustcode`, with its model
/// and effort; `None` for a built-in thread.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct HarnessRequest {
    pub harness: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
}

pub(crate) fn requested(meta: Option<&serde_json::Map<String, Value>>) -> HarnessRequest {
    let rustcode = meta.and_then(|meta| meta.get("rustcode"));
    let field = |name: &str| {
        rustcode
            .and_then(|rustcode| rustcode.get(name))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    HarnessRequest {
        harness: field("harness"),
        model: field("model"),
        effort: field("effort"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requested_harness_reads_meta() {
        let meta = serde_json::Map::from_iter([(
            "rustcode".to_owned(),
            json!({"harness": "codex", "model": "gpt-6-luna", "effort": " "}),
        )]);
        assert_eq!(
            requested(Some(&meta)),
            HarnessRequest {
                harness: Some("codex".into()),
                model: Some("gpt-6-luna".into()),
                effort: None,
            }
        );
        assert_eq!(requested(None), HarnessRequest::default());
    }

    #[test]
    fn capability_snapshots() {
        let builtin = serde_json::to_value(Capabilities::builtin()).unwrap();
        assert_eq!(
            builtin,
            json!({
                "steer": "live", "cancel": true, "resume": true, "rewind": true,
                "images": true, "models": "static", "effort": true,
                "permission_modes": ["default", "auto_edit", "full_access"],
                "subagents": true, "slash_commands": true, "smart_ui": true,
                "rustcode_extensions": true, "orchestration": true,
                "checkpoints": true, "worktree": true
            })
        );
        let codex = serde_json::to_value(Capabilities::external(HarnessKind::Codex)).unwrap();
        assert_eq!(
            codex,
            json!({
                "steer": "live", "cancel": true, "resume": true, "rewind": false,
                "images": true, "models": "dynamic", "effort": true,
                "permission_modes": [], "subagents": false, "slash_commands": false,
                "smart_ui": false, "rustcode_extensions": false, "orchestration": cfg!(unix),
                "checkpoints": true, "worktree": true
            })
        );
        let acp = serde_json::to_value(Capabilities::external(HarnessKind::Acp)).unwrap();
        assert_eq!(acp["steer"], "none");
        assert_eq!(acp["effort"], false);
        assert_eq!(acp["slash_commands"], true);
    }

    #[test]
    fn resolution_orders_and_overrides() {
        let entries = vec![
            config::HarnessEntry {
                id: "my-agent".into(),
                command: Some(vec!["my-agent".into(), "--acp".into()]),
                ..Default::default()
            },
            config::HarnessEntry {
                id: "gemini".into(),
                command: Some(vec!["/opt/gemini".into(), "--experimental-acp".into()]),
                enabled: Some(true),
                ..Default::default()
            },
        ];
        let all = resolve_all(&entries);
        let ids = all.iter().map(|h| h.id.as_str()).collect::<Vec<_>>();
        assert_eq!(
            ids,
            [
                "rustcode",
                "codex",
                "claude-code",
                "gemini",
                "opencode",
                "my-agent"
            ]
        );
        assert!(all[0].enabled);
        assert!(!all[1].enabled, "known harnesses start off");
        assert_eq!(all[3].command[0], "/opt/gemini");
        assert!(all[3].enabled);
        assert_eq!(all[5].kind, HarnessKind::Acp);
        assert!(all[5].enabled);
        assert_eq!(
            resolve(&entries, "claude-code").unwrap().command,
            ["npx", "-y", "@agentclientprotocol/claude-agent-acp"]
        );
    }

    #[tokio::test]
    async fn snapshot_shape_and_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let snapshot = snapshot_in(dir.path()).await.unwrap();
        assert_eq!(
            snapshot["default"],
            json!({"harness": "rustcode", "model": null, "effort": null})
        );
        assert_eq!(snapshot["onboarded"], false);
        let rows = snapshot["harnesses"].as_array().unwrap();
        assert_eq!(rows[0]["id"], "rustcode");
        assert_eq!(rows[0]["builtin"], true);
        assert_eq!(rows[0]["installed"], true);
        let codex = rows.iter().find(|row| row["id"] == "codex").unwrap();
        for key in [
            "name",
            "kind",
            "builtin",
            "enabled",
            "command",
            "installed",
            "version",
            "auth",
            "login_command",
            "install_hint",
            "capabilities",
            "models",
            "error",
        ] {
            assert!(codex.get(key).is_some(), "missing {key}");
        }
        assert_eq!(codex["kind"], "codex");
        assert_eq!(codex["command"], json!(["codex", "app-server"]));
    }
}
