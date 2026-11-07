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
pub(crate) mod background;
pub(crate) mod cache;
pub(crate) mod codex;
pub mod config;
pub(crate) mod detect;
pub(crate) mod handoff;
pub(crate) mod pi_mcp;
pub(crate) mod process;
pub(crate) mod rpc;
pub(crate) mod rpc_capture;
pub(crate) mod session;
pub(crate) mod store;
pub(crate) mod switch;
pub(crate) mod transcript;
pub(crate) mod watchdog;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};

pub(crate) const BUILTIN_ID: &str = "rustcode";

/// The `rustcode-viz` level the connected client draws (`acp.rs`
/// `client_visualization_level`). An engine serves one client, so external
/// harnesses started afterwards are taught the same widget types as a
/// RustCode thread; 0 (no client yet, or one that draws none) teaches none.
static CLIENT_VISUALIZATION_LEVEL: AtomicU8 = AtomicU8::new(0);

pub(crate) fn set_client_visualization_level(level: u8) {
    CLIENT_VISUALIZATION_LEVEL.store(level, Ordering::Relaxed);
}

pub(crate) fn client_visualization_level() -> u8 {
    CLIENT_VISUALIZATION_LEVEL.load(Ordering::Relaxed)
}

/// The client's Smart UI state (`acp.rs` `client_smart_ui`). A RustCode thread
/// learns Smart UI from its root prompt and the `smart-ui` skill; an external
/// harness has neither, so it is taught the fence from this.
static CLIENT_SMART_UI: AtomicU8 = AtomicU8::new(0);

pub(crate) fn set_client_smart_ui(smart_ui: crate::app::SmartUi) {
    CLIENT_SMART_UI.store(smart_ui.as_u8(), Ordering::Relaxed);
}

pub(crate) fn client_smart_ui() -> crate::app::SmartUi {
    crate::app::SmartUi::from_u8(CLIENT_SMART_UI.load(Ordering::Relaxed))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessKind {
    Builtin,
    Acp,
    Codex,
}

/// How a harness takes a message the user sends while a turn runs, so a
/// client decides once for every harness instead of branching on its id
/// (`docs/harnesses.md`). The client's policy (`state/queue.ts` in the
/// desktop app) turns this and the turn's state into steer, queue or
/// interrupt-and-restart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Steering {
    /// A message goes into the running turn: `_rustcode/steer` accepts it.
    pub live: bool,
    /// A live steer carries the message's pictures and files, not only its text.
    pub attachments: bool,
    /// A live steer goes in even while an approval or a question waits on the
    /// user; the built-in turn reads its steers between steps only, so it
    /// waits, where Codex and an ACP agent take one at any point of the turn.
    pub while_awaiting_user: bool,
    /// The turn can be stopped and the message sent as a new turn in its place.
    pub interrupt_restart: bool,
    /// A message that does not steer can wait for the next turn.
    pub queue: bool,
}

/// What a harness supports. Clients read these flags and never branch on
/// the harness id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Capabilities {
    /// The [`Steering`] summary older clients read (`live` or `none`);
    /// `steering` is the same fact with the detail. Both are written by
    /// [`Capabilities::take_steers`], so they cannot disagree.
    pub steer: &'static str,
    pub steering: Steering,
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
    /// Record that the harness takes a message mid-turn or not, and whether
    /// such a steer carries more than its text, keeping `steer` in step.
    fn take_steers(&mut self, live: bool, attachments: bool) {
        self.steer = if live { "live" } else { "none" };
        self.steering.live = live;
        self.steering.attachments = live && attachments;
    }

    fn builtin() -> Self {
        Self {
            steer: "live",
            // The built-in turn reads a steer as text, between steps, and
            // only while nothing waits on the user; a message that carries
            // more waits for the next turn.
            steering: Steering {
                live: true,
                attachments: false,
                while_awaiting_user: false,
                interrupt_restart: true,
                queue: true,
            },
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
            // `turn/steer` adds input to Codex's running turn. Plain ACP has no
            // way to, so the client sends the message as the next prompt, unless
            // the agent advertised `_session/steering` (learned at initialize).
            steer: if kind == HarnessKind::Codex {
                "live"
            } else {
                "none"
            },
            // `turn/steer` takes the message's input items, images included.
            // An ACP agent is asked at `initialize` (`capabilities()`), so
            // one that advertised nothing steers never.
            steering: Steering {
                live: kind == HarnessKind::Codex,
                attachments: kind == HarnessKind::Codex,
                while_awaiting_user: true,
                interrupt_restart: true,
                queue: true,
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
    // Pi speaks its own RPC protocol; `pi-acp` adapts it to ACP, as claude-agent-acp does for
    // Claude Code. Pi keeps its own models, logins, skills and sessions (~/.pi).
    KnownHarness {
        id: "pi",
        name: "Pi",
        kind: HarnessKind::Acp,
        command: &["npx", "-y", "pi-acp"],
        login_command: Some(&["pi"]),
        install_hint: Some("npm install -g @earendil-works/pi-coding-agent"),
        version: Some(("pi", &["--version"])),
        auth_probe: None,
    },
];

pub(crate) fn known(id: &str) -> Option<&'static KnownHarness> {
    KNOWN.iter().find(|harness| harness.id == id)
}

/// The name people know a harness by: a built-in one's, else its id.
pub(crate) fn display_name(id: &str) -> String {
    if id == BUILTIN_ID {
        return "RustCode".to_owned();
    }
    known(id).map_or_else(|| id.to_owned(), |harness| harness.name.to_owned())
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

/// The built-in harness as `entries` leave it: on unless turned off in Settings › Harnesses.
fn builtin_in(entries: &[config::HarnessEntry]) -> ResolvedHarness {
    ResolvedHarness {
        enabled: entries
            .iter()
            .find(|entry| entry.id == BUILTIN_ID)
            .is_none_or(config::HarnessEntry::is_enabled),
        ..builtin_resolved()
    }
}

/// Every harness in snapshot order: the built-in one, the known ones, then
/// custom entries in config order.
pub(crate) fn resolve_all(entries: &[config::HarnessEntry]) -> Vec<ResolvedHarness> {
    let mut all = vec![builtin_in(entries)];
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

/// What the engine learned about a harness while running it. Kept in
/// memory and in the config directory's model cache ([`cache`]), so a
/// restarted engine still knows a harness's models and capabilities.
#[derive(Debug, Clone, Default)]
pub(crate) struct Learned {
    pub models: Option<Vec<HarnessModel>>,
    pub error: Option<String>,
    pub resume: Option<bool>,
    pub images: Option<bool>,
    /// The ACP agent takes `_session/steering` (its initialize `_meta.steering.supported`).
    pub steer: Option<bool>,
    pub effort: Option<bool>,
    /// Unix seconds of the last model listing.
    pub fetched_at: Option<i64>,
    /// The harness version that listing came from, when known.
    pub version: Option<String>,
}

impl Learned {
    /// The harness `id` just listed `models`.
    pub(crate) fn listed(&mut self, id: &str, models: Vec<HarnessModel>) {
        self.models = Some(models);
        self.fetched_at = Some(cache::now());
        self.version = detect::cached_version(id).or(self.version.take());
        self.error = None;
    }
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

/// The entry of `id`, read from the model cache the first time.
fn learned_slot<'a>(
    learned: &'a mut HashMap<String, Learned>,
    id: &str,
    dir: Option<&std::path::Path>,
) -> &'a mut Learned {
    learned.entry(id.to_owned()).or_insert_with(|| {
        dir.and_then(|dir| cache::load_in(dir, id))
            .map(cache::to_learned)
            .unwrap_or_default()
    })
}

pub(crate) fn learn(id: &str, update: impl FnOnce(&mut Learned)) {
    let dir = config::config_dir().ok();
    let keep = {
        let mut learned = learned().lock().unwrap_or_else(|error| error.into_inner());
        let slot = learned_slot(&mut learned, id, dir.as_deref());
        update(slot);
        cache::from_learned(slot)
    };
    if let (Some(dir), Some(entry)) = (dir, keep) {
        cache::save_in(&dir, id, entry);
    }
}

pub(crate) fn learned_for(id: &str) -> Learned {
    let dir = config::config_dir().ok();
    let mut learned = learned().lock().unwrap_or_else(|error| error.into_inner());
    learned_slot(&mut learned, id, dir.as_deref()).clone()
}

/// Drop what memory holds for `id`, as an engine restart does (tests).
#[cfg(test)]
pub(crate) fn forget_learned(id: &str) {
    learned()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .remove(id);
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
    if harness.kind == HarnessKind::Acp {
        // `_session/steering` takes the message as prompt content, pictures
        // included, when the agent takes pictures at all.
        let live = learned.steer == Some(true);
        capabilities.take_steers(live, capabilities.images);
    }
    if let Some(effort) = learned.effort {
        capabilities.effort = effort;
    }
    // A model with efforts listed means the harness takes one.
    if learned
        .models
        .as_ref()
        .is_some_and(|models| models.iter().any(|model| !model.efforts.is_empty()))
    {
        capabilities.effort = true;
    }
    capabilities
}

/// `session_meta` of a built-in thread.
pub(crate) fn builtin_session_meta() -> serde_json::Map<String, Value> {
    session_meta(&builtin_resolved(), true)
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

/// [`session_meta`] that also names the account the thread runs on, if any.
pub(crate) fn session_meta_for(
    harness: &ResolvedHarness,
    account: Option<&config::HarnessAccount>,
    orchestration: bool,
) -> serde_json::Map<String, Value> {
    let mut meta = session_meta(harness, orchestration);
    if let Some(account) = account
        && let Some(Value::Object(rustcode)) = meta.get_mut("rustcode")
    {
        rustcode.insert(
            "account".to_owned(),
            json!({ "id": account.id, "name": account.display_name() }),
        );
    }
    meta
}

/// One `harnesses` row of the snapshot. `accounts` are its named-login rows,
/// already built with each sign-in read (`account_rows`).
fn snapshot_entry(
    harness: &ResolvedHarness,
    detection: &detect::Detection,
    entries: &[config::HarnessEntry],
    accounts: &[Value],
    discovered: Option<&[config::DiscoveredAccount]>,
) -> Value {
    let learned = learned_for(&harness.id);
    let builtin = harness.kind == HarnessKind::Builtin;
    let layout = config::account_layout(&harness.id);
    let models = if builtin {
        builtin_models()
    } else {
        learned.models.clone().unwrap_or_default()
    };
    let last = entries
        .iter()
        .find(|entry| entry.id == harness.id)
        .filter(|entry| entry.model.is_some() || entry.effort.is_some())
        .map(|entry| json!({ "model": entry.model, "effort": entry.effort }));
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
        // The model and effort last chosen for a new thread on it.
        "last": last,
        "accounts": accounts,
        // How a named login is run, for a client that offers one: the variable
        // naming its directory and the harness's own sign-in command. Null for
        // a harness without named logins.
        "account_env": layout.map(|layout| layout.env_var),
        "account_login_command": layout.map(|layout| layout.login_command),
        // Logins on this Mac that no account names yet (`discovered_accounts`).
        "discovered_accounts": discovered,
        "error": learned.error.or_else(|| detection.error.clone()),
    })
}

/// The built-in harness's model profiles, each with the reasoning efforts
/// its `effort` option offers (none when it offers no choice) and the one it
/// starts on.
fn builtin_models() -> Vec<HarnessModel> {
    let (_, _, config) = crate::config::load_config();
    profile_models(&config.models)
}

fn profile_models(profiles: &[crate::config::ModelProfile]) -> Vec<HarnessModel> {
    profiles
        .iter()
        .map(|profile| {
            let efforts = crate::remote::projection::reasoning_efforts(profile);
            let efforts = if efforts.len() < 2 {
                Vec::new()
            } else {
                efforts
            };
            let default_effort = (!efforts.is_empty()).then(|| {
                profile
                    .reasoning_effort
                    .clone()
                    .filter(|effort| efforts.contains(effort))
                    .unwrap_or_else(|| "default".to_owned())
            });
            HarnessModel {
                id: profile.name.clone(),
                name: profile.name.clone(),
                efforts,
                default_effort,
            }
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
    snapshot_in_with(dir, false).await
}

/// The snapshot with each account's sign-in read from its credentials;
/// `probe` also runs the harness's own status command for the account
/// (`codex login status`) or asks for the macOS Keychain item its directory
/// names. Only a client's explicit Refresh asks for that.
pub(crate) async fn snapshot_in_with(dir: &std::path::Path, probe: bool) -> Result<Value, String> {
    let (defaults, entries) = config::load_in(dir);
    let accounts = config::load_accounts_in(dir);
    let harnesses = resolve_all(&entries);
    let detections = futures_detect(&harnesses).await;
    // Reading the home directory's names is blocking work like detection:
    // keep it off the async runtime the same way. Each harness with named
    // logins is discovered by the directories it knows (`~/.claude*`,
    // `~/.codex*`), a cheap read of names only.
    let discovered = {
        let registered = accounts.clone();
        let layouts: Vec<&'static str> = config::account_layouts()
            .map(|layout| layout.harness)
            .collect();
        tokio::task::spawn_blocking(move || {
            layouts
                .into_iter()
                .map(|harness| (harness, config::discovered_accounts(harness, &registered)))
                .collect::<std::collections::HashMap<_, _>>()
        })
        .await
        .unwrap_or_default()
    };
    // Each account's sign-in is read the same way: off the runtime, every one
    // of them at once, since a probe runs a status command that can sleep.
    let accounts = account_rows(&harnesses, &accounts, probe).await;
    let rows = harnesses
        .iter()
        .zip(detections.iter())
        .map(|(harness, detection)| {
            let found = discovered.get(harness.id.as_str()).map(Vec::as_slice);
            let accounts = accounts.get(&harness.id).map(Vec::as_slice).unwrap_or(&[]);
            snapshot_entry(harness, detection, &entries, accounts, found)
        })
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

/// Each harness's named-login rows, with each account's sign-in read.
///
/// A probe runs the harness's own status command, which can take seconds:
/// every account is read on the blocking pool at once, as detection is, so a
/// hanging CLI never stalls a runtime worker and the Accounts list is not made
/// to wait for one account after another.
async fn account_rows(
    harnesses: &[ResolvedHarness],
    accounts: &[config::HarnessAccount],
    probe: bool,
) -> std::collections::HashMap<String, Vec<Value>> {
    let tasks = harnesses
        .iter()
        .flat_map(|harness| {
            config::accounts_of(accounts, &harness.id)
                .into_iter()
                .map(|account| {
                    let harness = harness.id.clone();
                    tokio::task::spawn_blocking(move || {
                        let auth = detect::account_auth(&account, probe);
                        (harness, account, auth)
                    })
                })
        })
        .collect::<Vec<_>>();
    let mut rows: std::collections::HashMap<String, Vec<Value>> = Default::default();
    for task in tasks {
        let Ok((harness, account, auth)) = task.await else {
            continue;
        };
        // Whether this is the Mac's own login (`~/.claude`, `~/.codex`),
        // whatever path it was written as: the app must set no
        // `CLAUDE_CONFIG_DIR`/`CODEX_HOME` for it.
        let own_dir = config::is_default_dir(&account.harness, &account.resolved_dir());
        let name = account.display_name().to_owned();
        rows.entry(harness).or_default().push(json!({
            "id": account.id,
            "name": name,
            "config_dir": account.config_dir,
            "own_dir": own_dir,
            "auth": auth,
        }));
    }
    rows
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
    config::edit_in(&dir, |defaults, harnesses| {
        config::check_default(defaults, &change)?;
        config::apply_change(harnesses, change)
    })?;
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

/// `_rustcode/harnesses/accounts/add`: name a config directory of its own.
pub(crate) fn add_account(change: config::AccountChange) -> Result<Value, String> {
    let dir = config::config_dir()?;
    let account = config::add_account_in(&dir, change)?;
    Ok(json!({
        "id": account.id,
        "harness": account.harness,
        "name": account.display_name(),
        "config_dir": account.config_dir,
    }))
}

/// `_rustcode/harnesses/accounts/remove`: forget an account. Its directory,
/// with the login and conversations in it, stays on disk.
pub(crate) fn remove_account(harness: &str, id: &str) -> Result<(), String> {
    let dir = config::config_dir()?;
    config::remove_account_in(&dir, harness.trim(), id.trim()).map(|_| ())
}

/// The snapshot with each account's sign-in looked up in its own credentials
/// (`_rustcode/harnesses/accounts/check`, the Accounts list's Refresh).
pub(crate) async fn check_accounts() -> Result<Value, String> {
    let dir = config::config_dir()?;
    snapshot_in_with(&dir, true).await
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
    config::edit_in(&dir, |defaults, harnesses| {
        let model = model.filter(|model| !model.trim().is_empty());
        let effort = effort.filter(|effort| !effort.trim().is_empty());
        // The default's model and effort are also that harness's last
        // choice, so a new thread on it starts there.
        if model.is_some() || effort.is_some() {
            config::remember(harnesses, &harness, model.clone(), effort.clone())?;
        }
        defaults.harness = (harness != BUILTIN_ID).then_some(harness);
        defaults.model = model;
        defaults.effort = effort;
        Ok(())
    })?;
    snapshot_in(&dir).await
}

/// `_rustcode/harnesses/remember`: keep `model` and `effort` as the last
/// choice for new threads on `harness` (the `[[harnesses]]` entry's `model`
/// and `effort`). Empty values clear it.
pub(crate) fn remember(
    harness: &str,
    model: Option<String>,
    effort: Option<String>,
) -> Result<Value, String> {
    let dir = config::config_dir()?;
    remember_in(&dir, harness, model, effort)
}

pub(crate) fn remember_in(
    dir: &std::path::Path,
    harness: &str,
    model: Option<String>,
    effort: Option<String>,
) -> Result<Value, String> {
    let (_, entries) = config::load_in(dir);
    if resolve(&entries, harness).is_none() {
        return Err(format!("unknown harness '{harness}'"));
    }
    let last = config::edit_in(dir, |_, harnesses| {
        config::remember(harnesses, harness, model, effort)?;
        Ok(harnesses
            .iter()
            .find(|entry| entry.id == harness)
            .filter(|entry| entry.model.is_some() || entry.effort.is_some())
            .map(|entry| json!({ "model": entry.model, "effort": entry.effort })))
    })?;
    Ok(json!({ "harness": harness, "last": last }))
}

/// Remember a choice made inside a thread, off the caller's task; a failure
/// only logs.
pub(crate) fn remember_later(harness: String, model: Option<String>, effort: Option<String>) {
    let write = move || {
        if let Err(error) = remember(&harness, model, effort) {
            crate::dbg_log!("could not remember the model of {harness}: {error}");
        }
    };
    match tokio::runtime::Handle::try_current() {
        Ok(runtime) => {
            runtime.spawn_blocking(write);
        }
        Err(_) => write(),
    }
}

/// The model and effort a new thread on `id` starts with: what `session/new`
/// asked for, else the harness's remembered choice, else
/// `[harness_defaults]` when it names this harness (the built-in one when it
/// names none), else nothing (the harness's own default). A remembered or
/// default model the harness no longer lists (when its list is known) is
/// skipped, and so is an effort its model does not offer.
pub(crate) fn starting_choice(
    id: &str,
    asked: &HarnessRequest,
    defaults: &config::HarnessDefaults,
    entries: &[config::HarnessEntry],
    models: &[HarnessModel],
) -> (Option<String>, Option<String>) {
    let mut candidates = Vec::new();
    if let Some(entry) = entries.iter().find(|entry| entry.id == id) {
        candidates.push((entry.model.clone(), entry.effort.clone()));
    }
    if defaults.harness.as_deref().unwrap_or(BUILTIN_ID) == id {
        candidates.push((defaults.model.clone(), defaults.effort.clone()));
    }
    let listed = |model: &str| models.is_empty() || models.iter().any(|known| known.id == model);
    let model = asked.model.clone().or_else(|| {
        candidates
            .iter()
            .filter_map(|(model, _)| model.clone())
            .find(|model| listed(model))
    });
    let offers = |effort: &str| match model
        .as_deref()
        .and_then(|model| models.iter().find(|known| known.id == model))
    {
        Some(known) => known.efforts.iter().any(|offered| offered == effort),
        None => true,
    };
    let effort = asked.effort.clone().or_else(|| {
        candidates
            .iter()
            .filter(|(chosen, _)| chosen.is_none() || *chosen == model)
            .filter_map(|(_, effort)| effort.clone())
            .find(|effort| offers(effort))
    });
    (model, effort)
}

/// What a built-in `session/new` starts with, by [`starting_choice`].
pub(crate) fn builtin_start(asked: &HarnessRequest) -> HarnessRequest {
    let Ok(dir) = config::config_dir() else {
        return asked.clone();
    };
    let (defaults, entries) = config::load_in(&dir);
    let (model, effort) =
        starting_choice(BUILTIN_ID, asked, &defaults, &entries, &builtin_models());
    HarnessRequest {
        harness: asked.harness.clone(),
        account: asked.account.clone(),
        model,
        effort,
    }
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

/// One model listing of `id` at a time.
fn fetch_lock(id: &str) -> std::sync::Arc<tokio::sync::Mutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<String, std::sync::Arc<tokio::sync::Mutex<()>>>>> =
        OnceLock::new();
    LOCKS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .entry(id.to_owned())
        .or_default()
        .clone()
}

/// `_rustcode/harnesses/models`: the harness's models, from memory or the
/// model cache, else by starting it briefly. A cached list that is old or
/// from another harness version is answered at once and listed again in the
/// background. A failed listing is not remembered, so the next call tries
/// again.
pub(crate) async fn models(id: &str) -> Result<Vec<HarnessModel>, String> {
    let dir = config::config_dir()?;
    let (_, entries) = config::load_in(&dir);
    let harness = resolve(&entries, id).ok_or_else(|| format!("unknown harness '{id}'"))?;
    if harness.kind == HarnessKind::Builtin {
        return Ok(builtin_models());
    }
    if let Some(models) = learned_for(id).models {
        refresh_if_stale(harness);
        return Ok(models);
    }
    let lock = fetch_lock(id);
    let _listing = lock.lock().await;
    if let Some(models) = learned_for(id).models {
        return Ok(models);
    }
    list_and_learn(&harness, true).await
}

/// List `harness`'s models and learn them. `report` keeps a failure as the
/// harness's error; a background refresh that fails keeps the cached list
/// quietly.
async fn list_and_learn(
    harness: &ResolvedHarness,
    report: bool,
) -> Result<Vec<HarnessModel>, String> {
    let id = harness.id.as_str();
    match session::list_models(harness).await {
        Ok(models) => {
            learn(id, |learned| learned.listed(id, models.clone()));
            Ok(models)
        }
        Err(error) => {
            if report {
                learn(id, |learned| learned.error = Some(error.clone()));
            }
            Err(error)
        }
    }
}

/// List `harness`'s models again in the background when the cached list is
/// stale; nothing when a listing already runs.
fn refresh_if_stale(harness: ResolvedHarness) {
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        return;
    };
    runtime.spawn(async move {
        let lock = fetch_lock(&harness.id);
        let Ok(_listing) = lock.try_lock() else {
            return;
        };
        let probed = harness.clone();
        let version = tokio::task::spawn_blocking(move || detect::detect(&probed).version)
            .await
            .ok()
            .flatten();
        let learned = learned_for(&harness.id);
        if cache::is_stale(
            learned.fetched_at,
            learned.version.as_deref(),
            version.as_deref(),
            cache::now(),
        ) {
            crate::dbg_log!("listing the models of {} again", harness.id);
            let _ = list_and_learn(&harness, false).await;
        }
    });
}

/// The harness a `session/new` asked for in `_meta.rustcode`, with its model
/// and effort; `None` for a built-in thread.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct HarnessRequest {
    pub harness: Option<String>,
    /// The named account of that harness (`[[harness_accounts]]`), when the
    /// thread runs on one.
    pub account: Option<String>,
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
        account: field("account"),
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
                account: None,
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
                "steering": {"live": true, "attachments": false, "while_awaiting_user": false,
                    "interrupt_restart": true, "queue": true},
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
                "steering": {"live": true, "attachments": true, "while_awaiting_user": true,
                    "interrupt_restart": true, "queue": true},
                "images": true, "models": "dynamic", "effort": true,
                "permission_modes": [], "subagents": false, "slash_commands": false,
                "smart_ui": false, "rustcode_extensions": false, "orchestration": cfg!(unix),
                "checkpoints": true, "worktree": true
            })
        );
        let acp = serde_json::to_value(Capabilities::external(HarnessKind::Acp)).unwrap();
        assert_eq!(acp["steer"], "none");
        assert_eq!(
            acp["steering"],
            json!({"live": false, "attachments": false, "while_awaiting_user": true,
                "interrupt_restart": true, "queue": true})
        );
        assert_eq!(acp["effort"], false);
        assert_eq!(acp["slash_commands"], true);
    }

    /// Every harness says how it takes a message sent mid-turn, and an ACP
    /// agent only says `live` once it advertised `_session/steering`.
    /// Ids are unique to the test: learning is cached in the real config
    /// directory (its `harness-models.json`).
    #[test]
    fn every_harness_says_how_it_takes_a_message_mid_turn() {
        let queueing = Steering {
            live: false,
            attachments: false,
            while_awaiting_user: true,
            interrupt_restart: true,
            queue: true,
        };
        let builtin = capabilities(&builtin_resolved());
        assert_eq!(builtin.steer, "live");
        assert_eq!(
            builtin.steering,
            Steering {
                live: true,
                attachments: false,
                while_awaiting_user: false,
                interrupt_restart: true,
                queue: true,
            },
            "the built-in turn takes text, between steps, and waits on the user"
        );

        let codex = session::test_support::harness("test-steer-codex", HarnessKind::Codex);
        assert_eq!(capabilities(&codex).steer, "live");
        assert!(
            capabilities(&codex).steering.attachments,
            "images ride a steer"
        );

        for id in [
            "test-steer-claude",
            "test-steer-gemini",
            "test-steer-opencode",
            "test-steer-pi",
        ] {
            forget_learned(id);
            let harness = session::test_support::harness(id, HarnessKind::Acp);
            let quiet = capabilities(&harness);
            assert_eq!(
                (quiet.steer, quiet.steering),
                ("none", queueing.clone()),
                "{id} before it runs"
            );
            learn(id, |learned| {
                learned.steer = Some(true);
                learned.images = Some(true);
            });
            let told = capabilities(&harness);
            assert_eq!(
                (told.steer, told.steering.clone()),
                (
                    "live",
                    Steering {
                        live: true,
                        attachments: true,
                        ..queueing.clone()
                    }
                ),
                "{id} after it advertised `_session/steering`"
            );
            learn(id, |learned| learned.images = Some(false));
            assert!(
                !capabilities(&harness).steering.attachments,
                "{id} takes text only when it takes no pictures"
            );
            forget_learned(id);
        }
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
                "pi",
                "my-agent"
            ]
        );
        assert!(all[0].enabled);
        assert!(!all[1].enabled, "known harnesses start off");
        assert_eq!(all[3].command[0], "/opt/gemini");
        assert!(all[3].enabled);
        assert_eq!(all[6].kind, HarnessKind::Acp);
        assert!(all[6].enabled);
        assert_eq!(
            resolve(&entries, "claude-code").unwrap().command,
            ["npx", "-y", "@agentclientprotocol/claude-agent-acp"]
        );
    }

    #[tokio::test]
    async fn an_account_on_the_default_directory_says_so() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            "[[harness_accounts]]\nid = \"own\"\nharness = \"claude-code\"\nname = \"Claude\"\nconfig_dir = \"~/.claude\"\n\
             \n[[harness_accounts]]\nid = \"work\"\nharness = \"claude-code\"\nname = \"Work\"\nconfig_dir = \"/tmp/claude-work\"\n\
             \n[[harness_accounts]]\nid = \"home\"\nharness = \"codex\"\nname = \"Codex\"\nconfig_dir = \"~/.codex\"\n\
             \n[[harness_accounts]]\nid = \"paral\"\nharness = \"codex\"\nname = \"Paral\"\nconfig_dir = \"/tmp/codex-paral\"\n",
        )
        .unwrap();
        let snapshot = snapshot_in(dir.path()).await.unwrap();
        let row = |id: &str| {
            snapshot["harnesses"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["id"] == id)
                .unwrap()
        };
        let accounts = row("claude-code")["accounts"].as_array().unwrap();
        assert_eq!(accounts[0]["id"], "own");
        assert_eq!(
            accounts[0]["own_dir"], true,
            "~/.claude is the Mac's own login, whatever path it was written as"
        );
        assert_eq!(accounts[1]["id"], "work");
        assert_eq!(accounts[1]["own_dir"], false);
        // Each harness decides which directory is its own.
        let accounts = row("codex")["accounts"].as_array().unwrap();
        assert_eq!(accounts[0]["id"], "home");
        assert_eq!(accounts[0]["own_dir"], true);
        assert_eq!(accounts[1]["id"], "paral");
        assert_eq!(accounts[1]["own_dir"], false);
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
        // How a client signs a named login in and runs it: the engine says, so
        // the app holds no per-harness copy of it.
        assert_eq!(codex["account_env"], "CODEX_HOME");
        assert_eq!(codex["account_login_command"], json!(["codex", "login"]));
        let claude = rows.iter().find(|row| row["id"] == "claude-code").unwrap();
        assert_eq!(claude["account_env"], "CLAUDE_CONFIG_DIR");
        assert_eq!(
            claude["account_login_command"],
            json!(["claude", "auth", "login"])
        );
        // Logins on this Mac are listed for a harness with named logins, and
        // null for one without.
        assert!(claude["discovered_accounts"].is_array());
        assert!(codex["discovered_accounts"].is_array());
        let rustcode = rows.iter().find(|row| row["id"] == "rustcode").unwrap();
        assert_eq!(rustcode["account_env"], Value::Null);
        assert_eq!(rustcode["account_login_command"], Value::Null);
        assert_eq!(rustcode["discovered_accounts"], Value::Null);
    }

    fn listed(id: &str, efforts: &[&str], default_effort: Option<&str>) -> HarnessModel {
        HarnessModel {
            id: id.into(),
            name: id.into(),
            efforts: efforts.iter().map(|effort| (*effort).to_owned()).collect(),
            default_effort: default_effort.map(str::to_owned),
        }
    }

    #[test]
    fn builtin_models_carry_their_efforts() {
        let reasoning = crate::config::ModelProfile {
            name: "luna".into(),
            reasoning_effort: Some("high".into()),
            reasoning_efforts: Some(vec!["low".into(), "medium".into(), "high".into()]),
            ..Default::default()
        };
        let plain = crate::config::ModelProfile {
            name: "local".into(),
            supports_reasoning_effort: Some(false),
            ..Default::default()
        };
        let wire = crate::config::ModelProfile {
            name: "wire".into(),
            supports_reasoning_effort: Some(true),
            ..Default::default()
        };
        let models = profile_models(&[reasoning, plain, wire]);
        assert_eq!(models[0].efforts, ["default", "low", "medium", "high"]);
        assert_eq!(models[0].default_effort.as_deref(), Some("high"));
        assert!(models[1].efforts.is_empty(), "no choice, no effort list");
        assert_eq!(models[1].default_effort, None);
        assert_eq!(models[2].default_effort.as_deref(), Some("default"));
    }

    #[test]
    fn a_new_thread_starts_on_the_asked_then_remembered_then_default_choice() {
        let models = vec![
            listed("opus", &["low", "high"], Some("low")),
            listed("sonnet", &["low", "medium"], Some("low")),
        ];
        let remembered = vec![config::HarnessEntry {
            id: "claude-code".into(),
            model: Some("opus".into()),
            effort: Some("high".into()),
            ..Default::default()
        }];
        let defaults = config::HarnessDefaults {
            harness: Some("claude-code".into()),
            model: Some("sonnet".into()),
            effort: Some("medium".into()),
            onboarded: true,
            ..Default::default()
        };
        let none = HarnessRequest::default();
        let empty = config::HarnessDefaults::default();
        let pick = |asked: &HarnessRequest,
                    entries: &[config::HarnessEntry],
                    defaults: &config::HarnessDefaults| {
            starting_choice("claude-code", asked, defaults, entries, &models)
        };
        let some = |model: &str, effort: &str| (Some(model.to_owned()), Some(effort.to_owned()));
        assert_eq!(pick(&none, &remembered, &defaults), some("opus", "high"));
        assert_eq!(pick(&none, &[], &defaults), some("sonnet", "medium"));
        assert_eq!(
            pick(&none, &[], &empty),
            (None, None),
            "the harness's own default"
        );
        let asked = HarnessRequest {
            harness: Some("claude-code".into()),
            account: None,
            model: Some("sonnet".into()),
            effort: None,
        };
        assert_eq!(
            pick(&asked, &remembered, &defaults),
            some("sonnet", "medium"),
            "an effort remembered with another model is not carried over"
        );
        // A remembered model the harness no longer lists falls back quietly.
        let stale = vec![config::HarnessEntry {
            id: "claude-code".into(),
            model: Some("gone".into()),
            effort: Some("max".into()),
            ..Default::default()
        }];
        assert_eq!(pick(&none, &stale, &defaults), some("sonnet", "medium"));
        // The default of another harness does not apply.
        let codex = config::HarnessDefaults {
            harness: Some("codex".into()),
            ..defaults.clone()
        };
        assert_eq!(pick(&none, &[], &codex), (None, None));
        // Unknown models (nothing listed yet) are kept as remembered.
        assert_eq!(
            starting_choice("claude-code", &none, &codex, &stale, &[]),
            some("gone", "max")
        );
    }

    #[tokio::test]
    async fn remembered_choices_show_in_the_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let answer = remember_in(
            dir.path(),
            "codex",
            Some("gpt-6-luna".into()),
            Some("high".into()),
        )
        .unwrap();
        assert_eq!(
            answer,
            json!({"harness": "codex", "last": {"model": "gpt-6-luna", "effort": "high"}})
        );
        assert!(remember_in(dir.path(), "nope", Some("x".into()), None).is_err());
        let snapshot = snapshot_in(dir.path()).await.unwrap();
        let rows = snapshot["harnesses"].as_array().unwrap();
        let row = |id: &str| rows.iter().find(|row| row["id"] == id).unwrap().clone();
        assert_eq!(
            row("codex")["last"],
            json!({"model": "gpt-6-luna", "effort": "high"})
        );
        assert_eq!(
            row("codex")["enabled"],
            false,
            "remembering does not turn it on"
        );
        assert_eq!(row("opencode")["last"], Value::Null);
        assert_eq!(snapshot["default"]["harness"], "rustcode");
    }

    /// #2797: after an engine restart a restored thread still offers every
    /// model and the effort, from the model cache, before its harness starts.
    #[tokio::test]
    async fn a_restored_thread_offers_cached_models_after_a_restart() {
        let id = "cached-acp-restart";
        let dir = config::config_dir().unwrap();
        learn(id, |learned| {
            learned.listed(
                id,
                vec![
                    listed(
                        "deepseek/deepseek-v4-pro",
                        &["low", "medium", "high", "default"],
                        Some("default"),
                    ),
                    listed(
                        "glm-5",
                        &["low", "medium", "high", "default"],
                        Some("default"),
                    ),
                ],
            );
            learned.effort = Some(true);
        });
        let cached = cache::load_in(&dir, id).expect("the listing is cached on disk");
        assert_eq!(cached.models.len(), 2);
        assert!(cached.fetched_at > 0);

        forget_learned(id);
        let harness = session::test_support::harness(id, HarnessKind::Acp);
        assert!(
            capabilities(&harness).effort,
            "the effort capability survives too"
        );
        let mut record = store::HarnessRecord::new(id, std::env::temp_dir());
        record.model = Some("deepseek/deepseek-v4-pro".into());
        record.effort = Some("high".into());
        let (updates, _) = tokio::sync::mpsc::unbounded_channel();
        let (permissions, _) = tokio::sync::mpsc::unbounded_channel();
        let sink = std::sync::Arc::new(session::test_support::ChannelSink {
            updates,
            permissions,
            choice: None,
        });
        let thread = session::ExternalSession::new(
            "restored".into(),
            harness,
            record,
            Vec::new(),
            sink,
            false,
        );
        let options = serde_json::to_value(thread.config_options().await).unwrap();
        let option = |id: &str| {
            options
                .as_array()
                .unwrap()
                .iter()
                .find(|option| option["id"] == id)
                .cloned()
                .unwrap_or_else(|| panic!("no {id} option in {options}"))
        };
        assert_eq!(option("model")["options"].as_array().unwrap().len(), 2);
        assert_eq!(option("effort")["currentValue"], "high");
        assert_eq!(option("effort")["options"].as_array().unwrap().len(), 4);
    }
}
