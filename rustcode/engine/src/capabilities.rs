//! Skills and MCP servers as the settings surfaces list and toggle them
//! (#2165). The TUI `/skills` and `/mcp` panels and the macOS app's Settings
//! read the same snapshot and write through the same two toggles, so the
//! frontends cannot drift apart.
//!
//! **Scope.** Every toggle is stored in the user config (`config.toml` in the
//! RustCode config directory) and applies to every project: a skill toggle is
//! the `disabled_skills` list, an MCP toggle is the server's `enabled` flag.
//! A project config file can neither set `disabled_skills` nor be edited by a
//! toggle; a server only a project file declares is listed with scope
//! `project` and has to be changed in that file.
//!
//! **Open sessions.** A toggle takes effect on the next model request of every
//! session in the process that made it: skill routing re-reads the catalog
//! when its generation moves, and MCP servers are started or stopped in the
//! process-wide registry. Another RustCode process (a second terminal, the app
//! while a terminal is open) reads `disabled_skills` the next time it scans
//! skills and its MCP servers keep running until it restarts or toggles them
//! itself. An in-flight turn is never changed mid-request.
//!
//! **Installed vs enabled, configured vs connected.** A skill is listed when it
//! is discovered on disk and `enabled` says whether the model is offered it.
//! An MCP server is listed when it is configured (or still connected) and
//! `connection` says whether it is actually up; a server is never reported as
//! connected unless its client is in the registry.

use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// One discovered skill.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillEntry {
    pub name: String,
    pub description: String,
    pub path: String,
    /// The kind of root it was found in: `workspace`, `universal`,
    /// `rustcode`, `configured`, or `claude-project` / `claude-user` for a
    /// harness's own roots.
    pub source: &'static str,
    /// A harness owns this folder: Settings lists it, and a toggle only stops
    /// RustCode from offering the skill. Nothing is ever written there.
    pub read_only: bool,
    pub enabled: bool,
    /// The skill's `SKILL.md`, so Settings can copy the definition to another
    /// machine. Absent when it is unreadable or larger than
    /// [`SKILL_CONTENT_LIMIT`].
    pub content: Option<String>,
}

/// How large a `SKILL.md` may be before the snapshot leaves its content out.
pub const SKILL_CONTENT_LIMIT: u64 = 64 * 1024;

/// Whether a configured MCP server is up in this process.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum McpConnection {
    Connected,
    Connecting,
    NotConnected,
    Disabled,
    Failed,
}

impl McpConnection {
    pub fn label(self) -> &'static str {
        match self {
            Self::Connected => "connected",
            Self::Connecting => "connecting",
            Self::NotConnected => "not connected",
            Self::Disabled => "disabled",
            Self::Failed => "failed",
        }
    }
}

/// One configured or connected MCP server, without header or environment
/// values: those are usually secrets. The key names are kept so Settings can
/// copy the definition to another machine and say which values to set there.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpEntry {
    pub name: String,
    /// `stdio` or `http`.
    pub transport: &'static str,
    /// The command line or URL, with credentials and secret-looking
    /// arguments masked.
    pub target: String,
    /// The unmasked target as its parts, when there was nothing to mask, so
    /// Settings can copy the definition faithfully. `None` when the target
    /// carries a password or a secret-looking argument: a masked value is
    /// never copied.
    pub copy_target: Option<Vec<String>>,
    pub enabled: bool,
    /// `user` when the user config declares it and a toggle reaches it,
    /// `project` when only a project config file does, `session` when it is
    /// connected but no longer configured.
    pub scope: &'static str,
    pub connection: McpConnection,
    pub tools: Vec<String>,
    pub error: Option<String>,
    /// The names of its environment variables, never their values.
    pub env_keys: Vec<String>,
    /// The names of its HTTP headers, never their values.
    pub header_keys: Vec<String>,
}

impl McpEntry {
    pub fn can_toggle(&self) -> bool {
        self.scope == "user"
    }

    /// `connected (3 tools)`, `failed: …` and so on, for the TUI.
    pub fn status_label(&self) -> String {
        match (self.connection, &self.error) {
            (McpConnection::Connected, _) => format!("connected ({} tools)", self.tools.len()),
            (McpConnection::Failed, Some(error)) => format!("failed: {error}"),
            (connection, _) => connection.label().to_owned(),
        }
    }
}

/// Everything the settings surfaces show.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilitySnapshot {
    pub skills: Vec<SkillEntry>,
    pub mcp_servers: Vec<McpEntry>,
    /// The user config every toggle is written to.
    pub config_path: Option<String>,
}

fn root_kind_name(kind: crate::skills::SkillRootKind) -> &'static str {
    use crate::skills::SkillRootKind;
    match kind {
        SkillRootKind::Workspace => "workspace",
        SkillRootKind::Universal => "universal",
        SkillRootKind::Rustcode => "rustcode",
        SkillRootKind::Configured => "configured",
        // A harness's own roots (#2898): listed, read only, and toggling one
        // only stops RustCode from offering it.
        SkillRootKind::HarnessProject => "claude-project",
        SkillRootKind::HarnessUser => "claude-user",
    }
}

/// Mark each skill enabled or not and name the root it came from.
pub fn skill_entries_from(
    skills: &[crate::skills::SkillMetadata],
    roots: &[crate::skills::SkillRoot],
    disabled: &HashSet<String>,
) -> Vec<SkillEntry> {
    skills
        .iter()
        .map(|skill| {
            let root = roots.iter().find(|root| skill.path.starts_with(&root.path));
            SkillEntry {
                name: skill.name.clone(),
                description: skill.description.clone(),
                path: skill.path.display().to_string(),
                source: root
                    .map(|root| root_kind_name(root.kind))
                    .unwrap_or("configured"),
                read_only: root.is_some_and(|root| root.kind.is_harness_owned()),
                enabled: !disabled.contains(&skill.name),
                content: skill_content(&skill.path),
            }
        })
        .collect()
}

/// Every discovered skill, enabled or not.
pub fn skill_entries() -> Vec<SkillEntry> {
    skill_entries_from(
        &crate::skills::discover_all_skills(),
        &crate::skills::skill_roots(),
        &crate::skills::disabled_skill_names(),
    )
}

/// Combine the servers active in the workspace, the user config's own list
/// and the live statuses into one row per server.
pub(crate) fn mcp_entries_from(
    configured: &[crate::config::McpServerConfig],
    user: &[crate::config::McpServerConfig],
    statuses: &[crate::tools::McpServerStatus],
) -> Vec<McpEntry> {
    use crate::tools::McpServerState;
    let user_names: HashSet<&str> = user.iter().map(|server| server.name.as_str()).collect();
    let configs: HashMap<&str, &crate::config::McpServerConfig> = configured
        .iter()
        .map(|server| (server.name.as_str(), server))
        .collect();
    let failures = crate::mcp::mcp_startup_failures();
    statuses
        .iter()
        .map(|status| {
            let config = configs.get(status.name.as_str()).copied();
            let (connection, tools, error) = match &status.state {
                McpServerState::Running { tools } => {
                    (McpConnection::Connected, tools.clone(), None)
                }
                McpServerState::Connecting => (McpConnection::Connecting, Vec::new(), None),
                McpServerState::NotRunning => (McpConnection::NotConnected, Vec::new(), None),
                McpServerState::Disabled => (McpConnection::Disabled, Vec::new(), None),
                McpServerState::Failed(reason) => (
                    McpConnection::Failed,
                    Vec::new(),
                    Some(
                        failures
                            .get(&status.name)
                            .cloned()
                            .unwrap_or_else(|| reason.clone()),
                    ),
                ),
            };
            let copy_target = config.and_then(|server| {
                let parts = raw_target_parts(server);
                (parts.join(" ") == redacted_target(server)).then_some(parts)
            });
            McpEntry {
                name: status.name.clone(),
                transport: match config {
                    Some(server) if server.is_remote() => "http",
                    _ => "stdio",
                },
                target: config.map(redacted_target).unwrap_or_default(),
                copy_target,
                enabled: config.is_none_or(|server| server.enabled),
                scope: if config.is_none() {
                    "session"
                } else if user_names.contains(status.name.as_str()) {
                    "user"
                } else {
                    "project"
                },
                connection,
                tools,
                error,
                env_keys: config
                    .map(|server| sorted_keys(&server.env))
                    .unwrap_or_default(),
                header_keys: config
                    .map(|server| sorted_keys(&server.headers))
                    .unwrap_or_default(),
            }
        })
        .collect()
}

fn sorted_keys(map: &HashMap<String, String>) -> Vec<String> {
    let mut keys: Vec<String> = map.keys().cloned().collect();
    keys.sort();
    keys
}

/// The skill's `SKILL.md` text, when it is readable and small enough to copy.
fn skill_content(path: &Path) -> Option<String> {
    let file = path.join("SKILL.md");
    let metadata = std::fs::metadata(&file).ok()?;
    if !metadata.is_file() || metadata.len() > SKILL_CONTENT_LIMIT {
        return None;
    }
    std::fs::read_to_string(file).ok()
}

fn mcp_workspace() -> std::path::PathBuf {
    crate::tools::active_workspace_root()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| std::path::PathBuf::from("."))
}

/// Every configured or connected MCP server in this process.
pub fn mcp_entries() -> Vec<McpEntry> {
    let Some(dir) = crate::config::get_config_dir() else {
        return Vec::new();
    };
    let configured = crate::config::workspace_mcp_servers_in(&dir, &mcp_workspace());
    let user = crate::config::load_config_from(&dir).2.mcp_servers;
    mcp_entries_from(&configured, &user, &crate::tools::current_server_statuses())
}

pub fn snapshot() -> CapabilitySnapshot {
    CapabilitySnapshot {
        skills: skill_entries(),
        mcp_servers: mcp_entries(),
        config_path: crate::config::get_config_dir()
            .map(|dir| dir.join("config.toml").display().to_string()),
    }
}

fn user_config_dir() -> Result<std::path::PathBuf, String> {
    crate::config::get_config_dir()
        .ok_or_else(|| "could not determine the config directory".to_owned())
}

/// Turn a discovered skill on or off for every project, effective on the next
/// request. Returns the new `disabled_skills` list.
pub fn set_skill_enabled(name: &str, enabled: bool) -> Result<Vec<String>, String> {
    set_skill_enabled_in(&user_config_dir()?, name, enabled)
}

/// Write a skill's `SKILL.md` under the user's RustCode skills directory, so
/// another machine's settings page can copy its definition here (#3202). The
/// content is the definition; any other files the source skill had do not
/// travel, because the snapshot carries the `SKILL.md` only.
pub fn add_skill(name: &str, content: &str) -> Result<String, String> {
    let name = name.trim();
    if !is_valid_skill_name(name) {
        return Err(format!(
            "invalid skill name '{name}'; use lowercase letters, digits and dashes"
        ));
    }
    if content.trim().is_empty() {
        return Err("the skill is empty".to_owned());
    }
    if content.len() as u64 > SKILL_CONTENT_LIMIT {
        return Err("the skill is too large to copy".to_owned());
    }
    let dir = user_config_dir()?.join("skills").join(name);
    let file = dir.join("SKILL.md");
    if file.exists() {
        return Err(format!(
            "a skill named '{name}' already exists on this machine"
        ));
    }
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("could not create {}: {error}", dir.display()))?;
    std::fs::write(&file, content)
        .map_err(|error| format!("could not write {}: {error}", file.display()))?;
    crate::skills::bump_skill_catalog_generation();
    Ok(file.display().to_string())
}

fn is_valid_skill_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

pub(crate) fn set_skill_enabled_in(
    dir: &Path,
    name: &str,
    enabled: bool,
) -> Result<Vec<String>, String> {
    let known = crate::skills::discover_all_skills()
        .iter()
        .any(|skill| skill.name == name);
    // Re-enabling a skill that has since vanished only cleans up the list.
    if !known && !enabled {
        return Err(format!("no skill named '{name}' was discovered"));
    }
    let disabled = crate::config::set_skill_enabled_in(dir, name, enabled)?;
    crate::skills::bump_skill_catalog_generation();
    Ok(disabled)
}

/// Turn a user-config MCP server on or off, starting or stopping it in this
/// process. A server that fails to start stays enabled and is reported as
/// failed rather than connected.
pub async fn set_mcp_server_enabled(name: &str, enabled: bool) -> Result<McpEntry, String> {
    crate::config::set_mcp_server_enabled_in(&user_config_dir()?, name, enabled)?;
    crate::mcp::start_deferred_servers().await;
    apply_mcp_toggle(name, enabled).await;
    mcp_entries()
        .into_iter()
        .find(|entry| entry.name == name)
        .ok_or_else(|| format!("MCP server '{name}' is no longer configured"))
}

/// Save a new server to the user config, validated like `rustcode mcp add`,
/// and start it in this process. A start failure is recorded for the
/// settings surfaces rather than returned: the server stays configured.
pub async fn add_mcp_server(spec: &crate::config::McpServerSpec) -> Result<McpEntry, String> {
    let server = crate::config::mcp_server_from_spec(spec)?;
    let name = server.name.clone();
    crate::config::add_mcp_server_in(&user_config_dir()?, server, false).map_err(|error| {
        error.replace(
            "pass --force to replace it",
            "remove it first or choose another name",
        )
    })?;
    apply_mcp_toggle(&name, true).await;
    mcp_entries()
        .into_iter()
        .find(|entry| entry.name == name)
        .ok_or_else(|| format!("MCP server '{name}' is not active in this workspace"))
}

/// Remove a server from the user config and stop it in this process.
pub async fn remove_mcp_server(name: &str) -> Result<(), String> {
    crate::config::remove_mcp_server_in(&user_config_dir()?, name)?;
    crate::mcp::start_deferred_servers().await;
    crate::mcp::shutdown_server(name).await;
    Ok(())
}

/// Restart a configured server in place: stop it and start it again from the
/// current config, leaving the config file untouched. A start failure is
/// recorded like a toggle's, so the settings surfaces report it in the
/// snapshot rather than as a failed request.
pub async fn restart_mcp_server(name: &str) -> Result<(), String> {
    let workspace = mcp_workspace();
    let servers = crate::config::workspace_mcp_servers_in(&user_config_dir()?, &workspace);
    let Some(server) = servers.iter().find(|server| server.name == name) else {
        return Err(format!("MCP server '{name}' is no longer configured"));
    };
    if !server.enabled {
        return Err(format!("MCP server '{name}' is disabled"));
    }
    crate::mcp::start_deferred_servers().await;
    crate::mcp::shutdown_server(name).await;
    if let Err(error) = crate::mcp::start_server_by_name_in_workspace(name, &workspace).await {
        crate::mcp::record_mcp_startup_failure(name, &error);
    }
    Ok(())
}

/// Start or stop a server after its toggle was saved, recording a start
/// failure so the settings surfaces can show it.
pub async fn apply_mcp_toggle(name: &str, enabled: bool) {
    if enabled {
        if let Err(error) =
            crate::mcp::start_server_by_name_in_workspace(name, &mcp_workspace()).await
        {
            crate::mcp::record_mcp_startup_failure(name, &error);
        }
    } else {
        crate::mcp::shutdown_server(name).await;
    }
}

/// The live state of each of `servers`, in order, for the TUI `/mcp` panel:
/// `connected (3 tools)`, `not connected`, `disabled` or `failed: …`. Reads
/// only the registry and recorded failures, so it is cheap enough per frame.
pub fn mcp_live_labels(servers: &[crate::config::McpServerConfig]) -> Vec<String> {
    let running: HashMap<String, usize> = crate::mcp::get_mcp_registry()
        .lock()
        .map(|registry| {
            registry
                .values()
                .map(|client| {
                    let tools = client.get_tools().map(|tools| tools.len()).unwrap_or(0);
                    (client.name.clone(), tools)
                })
                .collect()
        })
        .unwrap_or_default();
    let failures = crate::mcp::mcp_startup_failures();
    servers
        .iter()
        .map(|server| {
            if let Some(tools) = running.get(&server.name) {
                format!("connected ({tools} tools)")
            } else if !server.enabled {
                "disabled".to_owned()
            } else if let Some(reason) = failures.get(&server.name) {
                format!("failed: {}", reason.lines().next().unwrap_or("").trim())
            } else {
                "not connected".to_owned()
            }
        })
        .collect()
}

/// The TUI `/skills` panel: every discovered skill with a toggle.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SkillsPanel {
    pub entries: Vec<SkillEntry>,
    pub selected: usize,
    /// The last toggle's outcome or error, shown in the footer.
    pub notice: Option<String>,
}

impl SkillsPanel {
    pub fn load() -> Self {
        Self {
            entries: skill_entries(),
            ..Self::default()
        }
    }

    pub fn select(&mut self, index: usize) {
        self.selected = index.min(self.entries.len().saturating_sub(1));
    }

    /// Flip the selected skill and save it to the user config. Returns the
    /// new `disabled_skills` list so the caller can keep its in-memory config
    /// from writing the old one back.
    pub fn toggle_selected(&mut self) -> Option<Vec<String>> {
        self.toggle_selected_in(&user_config_dir().ok()?)
    }

    pub(crate) fn toggle_selected_in(&mut self, dir: &Path) -> Option<Vec<String>> {
        let entry = self.entries.get(self.selected)?;
        let (name, enabled) = (entry.name.clone(), !entry.enabled);
        // The entry was discovered when the panel loaded, so no rescan.
        match crate::config::set_skill_enabled_in(dir, &name, enabled) {
            Ok(disabled) => {
                crate::skills::bump_skill_catalog_generation();
                for entry in &mut self.entries {
                    entry.enabled = !disabled.contains(&entry.name);
                }
                self.notice = Some(format!(
                    "{name} {} · saved to the user config, applies from the next message",
                    if enabled { "enabled" } else { "disabled" }
                ));
                Some(disabled)
            }
            Err(error) => {
                self.notice = Some(error);
                None
            }
        }
    }
}

/// Arguments that name a secret: `--token`, `API_KEY=…` and the like.
fn is_secret_name(name: &str) -> bool {
    let name = name.trim_start_matches('-').to_ascii_lowercase();
    [
        "token", "secret", "password", "passwd", "apikey", "api-key", "api_key", "auth",
    ]
    .iter()
    .any(|word| name.contains(word))
}

/// The command line or URL of a server with credentials masked, so a settings
/// page can show where a server points without leaking what unlocks it.
/// The target as configured, before secrets are masked.
pub fn raw_target_parts(server: &crate::config::McpServerConfig) -> Vec<String> {
    if server.is_remote() {
        return vec![server.url.clone().unwrap_or_default()];
    }
    let mut parts = vec![server.command.clone()];
    parts.extend(server.args.iter().cloned());
    parts
}

pub fn redacted_target(server: &crate::config::McpServerConfig) -> String {
    if server.is_remote() {
        let raw = server.url.as_deref().unwrap_or_default();
        return match reqwest::Url::parse(raw) {
            Ok(mut url) => {
                let _ = url.set_username("");
                let _ = url.set_password(None);
                url.set_query(None);
                url.set_fragment(None);
                url.to_string()
            }
            Err(_) => "(invalid URL)".to_owned(),
        };
    }
    let mut parts = vec![server.command.clone()];
    let mut mask_next = false;
    for arg in &server.args {
        if mask_next {
            parts.push("•••".to_owned());
            mask_next = false;
        } else if let Some((key, _)) = arg.split_once('=')
            && is_secret_name(key)
        {
            parts.push(format!("{key}=•••"));
        } else {
            mask_next = arg.starts_with('-') && is_secret_name(arg);
            parts.push(arg.clone());
        }
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::McpServerConfig;
    use crate::skills::{SkillMetadata, SkillRoot, SkillRootKind};
    use crate::tools::{McpServerState, McpServerStatus};
    use std::path::PathBuf;

    fn skill(name: &str, path: &str) -> SkillMetadata {
        SkillMetadata {
            name: name.to_owned(),
            description: format!("{name} skill"),
            path: PathBuf::from(path),
            triggers: Vec::new(),
            keywords: Vec::new(),
            priority: 0,
        }
    }

    fn stdio(name: &str, args: &[&str]) -> McpServerConfig {
        McpServerConfig {
            name: name.to_owned(),
            command: "npx".to_owned(),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            env: HashMap::from([("API_KEY".to_owned(), "hunter2".to_owned())]),
            url: None,
            headers: HashMap::new(),
            client_id: None,
            enabled: true,
            always_include: false,
            timeout_secs: None,
        }
    }

    #[test]
    fn skills_are_listed_with_their_root_and_enabled_state() {
        let roots = [
            SkillRoot {
                path: PathBuf::from("/work/.rustcode/skills"),
                kind: SkillRootKind::Workspace,
            },
            SkillRoot {
                path: PathBuf::from("/home/.agents/skills"),
                kind: SkillRootKind::Universal,
            },
        ];
        let skills = [
            skill("deploy", "/work/.rustcode/skills/deploy"),
            skill("review", "/home/.agents/skills/review"),
        ];
        let disabled = HashSet::from(["review".to_owned()]);
        let entries = skill_entries_from(&skills, &roots, &disabled);
        assert_eq!(entries[0].source, "workspace");
        assert!(entries[0].enabled);
        assert!(!entries[0].read_only);
        assert_eq!(entries[1].source, "universal");
        assert!(!entries[1].enabled, "a disabled skill stays listed");
    }

    // Settings must not offer to edit a file in another tool's folder (#2898).
    #[test]
    fn a_harness_owned_skill_is_listed_read_only() {
        let roots = [
            SkillRoot {
                path: PathBuf::from("/home/.claude/skills"),
                kind: SkillRootKind::HarnessUser,
            },
            SkillRoot {
                path: PathBuf::from("/work/.claude/skills"),
                kind: SkillRootKind::HarnessProject,
            },
        ];
        let skills = [
            skill("user-only", "/home/.claude/skills/user-only"),
            skill("project-only", "/work/.claude/skills/project-only"),
        ];

        let entries = skill_entries_from(&skills, &roots, &HashSet::new());

        assert_eq!(entries[0].source, "claude-user");
        assert!(entries[0].read_only);
        assert_eq!(entries[1].source, "claude-project");
        assert!(entries[1].read_only);
        assert!(entries[0].enabled);
    }

    #[test]
    fn mcp_rows_separate_configured_from_connected_and_user_from_project() {
        let mut off = stdio("off", &[]);
        off.enabled = false;
        let configured = [
            stdio("up", &[]),
            stdio("broken", &[]),
            off,
            stdio("project", &[]),
        ];
        let user = [stdio("up", &[]), stdio("broken", &[]), stdio("off", &[])];
        let statuses = [
            McpServerStatus {
                name: "broken".into(),
                state: McpServerState::Failed("spawn failed".into()),
            },
            McpServerStatus {
                name: "ghost".into(),
                state: McpServerState::Running {
                    tools: vec!["a".into()],
                },
            },
            McpServerStatus {
                name: "off".into(),
                state: McpServerState::Disabled,
            },
            McpServerStatus {
                name: "project".into(),
                state: McpServerState::NotRunning,
            },
            McpServerStatus {
                name: "up".into(),
                state: McpServerState::Running {
                    tools: vec!["read".into(), "write".into()],
                },
            },
        ];
        let rows = mcp_entries_from(&configured, &user, &statuses);
        let row = |name: &str| rows.iter().find(|row| row.name == name).unwrap();

        assert_eq!(row("up").connection, McpConnection::Connected);
        assert_eq!(row("up").status_label(), "connected (2 tools)");
        assert_eq!(row("broken").connection, McpConnection::Failed);
        assert_eq!(row("broken").status_label(), "failed: spawn failed");
        assert!(
            row("broken").enabled,
            "a failed server is not reported as off"
        );
        assert_eq!(row("off").connection, McpConnection::Disabled);
        assert!(!row("off").enabled);
        assert_eq!(row("project").scope, "project");
        assert!(!row("project").can_toggle());
        assert_eq!(row("project").connection, McpConnection::NotConnected);
        assert_eq!(row("ghost").scope, "session");
        assert!(row("up").can_toggle());
    }

    #[test]
    fn the_panel_toggle_saves_and_updates_every_row() {
        let dir = tempfile::TempDir::new().unwrap();
        let entry = |name: &str| SkillEntry {
            name: name.to_owned(),
            description: String::new(),
            path: format!("/skills/{name}"),
            source: "universal",
            read_only: false,
            enabled: true,
            content: None,
        };
        let mut panel = SkillsPanel {
            entries: vec![entry("deploy"), entry("review")],
            ..SkillsPanel::default()
        };
        panel.select(7);
        assert_eq!(panel.selected, 1, "selection is clamped to the list");

        assert_eq!(
            panel.toggle_selected_in(dir.path()),
            Some(vec!["review".to_owned()])
        );
        assert!(panel.entries[0].enabled);
        assert!(!panel.entries[1].enabled);
        assert!(panel.notice.as_deref().unwrap().contains("review disabled"));
        assert_eq!(
            crate::config::load_config_from(dir.path())
                .2
                .disabled_skills,
            vec!["review".to_owned()]
        );

        assert_eq!(panel.toggle_selected_in(dir.path()), Some(Vec::new()));
        assert!(panel.entries[1].enabled);

        assert_eq!(SkillsPanel::default().toggle_selected_in(dir.path()), None);
    }

    /// The keys the macOS app decodes (`CapabilitiesTests.swift`).
    #[test]
    fn the_snapshot_wire_shape_matches_the_mac_app() {
        let snapshot = CapabilitySnapshot {
            skills: skill_entries_from(&[skill("deploy", "/p/deploy")], &[], &HashSet::new()),
            mcp_servers: mcp_entries_from(
                &[stdio("idle", &[])],
                &[stdio("idle", &[])],
                &[McpServerStatus {
                    name: "idle".into(),
                    state: McpServerState::NotRunning,
                }],
            ),
            config_path: Some("/c/config.toml".into()),
        };
        assert_eq!(
            serde_json::to_value(snapshot).unwrap(),
            serde_json::json!({
                "skills": [{"name": "deploy", "description": "deploy skill", "path": "/p/deploy",
                            "source": "configured", "readOnly": false, "enabled": true,
                            "content": null}],
                "mcpServers": [{"name": "idle", "transport": "stdio", "target": "npx",
                                "copyTarget": ["npx"], "enabled": true, "scope": "user",
                                "connection": "not_connected", "tools": [], "error": null,
                                "envKeys": ["API_KEY"], "headerKeys": []}],
                "configPath": "/c/config.toml",
            })
        );
    }

    #[test]
    fn targets_never_show_credentials() {
        let server = stdio(
            "s",
            &[
                "-y",
                "server",
                "--token",
                "abc123",
                "API_KEY=xyz",
                "--port=1",
            ],
        );
        let target = redacted_target(&server);
        assert_eq!(target, "npx -y server --token ••• API_KEY=••• --port=1");
        assert!(!target.contains("hunter2"));

        let mut remote = stdio("r", &[]);
        remote.command.clear();
        remote.url = Some("https://user:pw@mcp.example.com/mcp?key=secret#x".into());
        assert_eq!(redacted_target(&remote), "https://mcp.example.com/mcp");

        let row = &mcp_entries_from(
            &[remote.clone()],
            &[remote],
            &[McpServerStatus {
                name: "r".into(),
                state: McpServerState::NotRunning,
            }],
        )[0];
        let json = serde_json::to_string(row).unwrap();
        assert!(!json.contains("secret") && !json.contains("pw@") && !json.contains("hunter2"));
        assert_eq!(row.transport, "http");
    }
}
