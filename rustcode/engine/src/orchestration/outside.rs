//! Outside agents: another agent platform (Claude Desktop, Cursor, Codex, a
//! script) using RustCode's orchestration tools as an independent MCP client,
//! with a scoped, named, revocable grant of its own (`docs/mcp-outside-agents.md`).
//!
//! The pieces:
//!
//! - [`Scope`] and [`required_scope`]: what a grant may call. `read_only`
//!   reads threads, usage and projects; `delegate` also starts, messages,
//!   fans out and interrupts threads; `manage` adds the management tools
//!   (`merge_when_green`, `cleanup_worktree`, `update_session`,
//!   `configure_session`), which the engine runs itself
//!   ([`super::outside_manage`]) since no thread is behind the call. A tool nobody
//!   classified is refused to every grant, so a new internal tool is never
//!   exposed by accident.
//! - The grant store (`<config>/outside-agents.json`, mode 0600): one entry per
//!   connected app. The token is shown once when the grant is created; only
//!   its SHA-256 is kept.
//! - [`answer_with`]: the server-side gate every call from `rustcode
//!   mcp-connect` passes through, in the engine, whatever the bridge sent:
//!   authenticate, check the scope, ask the user in the app when the grant
//!   asks, run, and record the call in the data store (tool, outcome and
//!   timing, never arguments).
//!
//! Threads an outside agent starts are standalone threads of the user's, and
//! run at Auto-edit under a `delegate` grant (Full only under `manage`): the
//! grant's scope is also the ceiling for the work it starts.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use super::hub::{CallAnswer, CallRequest};

/// The caller id an outside agent's calls carry inside the engine, so the
/// thread machinery (links, `wait_session`) can tell it from a thread.
pub const CALLER_PREFIX: &str = "outside:";
/// The environment variable `rustcode mcp-connect` reads its token from.
pub const TOKEN_ENV: &str = "RUSTCODE_MCP_TOKEN";
/// The tool every grant has: who is calling, what it may do, and what exists.
pub const WHOAMI: &str = "whoami";
/// Internal requests the bridge makes of the engine to build `initialize` and
/// `tools/list`, so the scope is decided by the engine and never by the bridge.
pub const CONTROL_INSTRUCTIONS: &str = "$instructions";
pub const CONTROL_TOOLS: &str = "$tools";
/// The most grants at once.
pub const MAX_GRANTS: usize = 20;
/// How long a change waits for the user in the app.
pub const APPROVAL_WAIT: Duration = Duration::from_secs(120);
/// Token prefix, so a leaked one is recognisable in a scan.
const TOKEN_PREFIX: &str = "rcx_";
const MAX_NAME_CHARS: usize = 60;
/// A grant's last-used time is written at most this often.
const TOUCH_EVERY: Duration = Duration::from_secs(10);
const GRANT_FILE: &str = "outside-agents.json";
/// The platforms the setup flow knows; anything else is `other`.
pub const PLATFORMS: [&str; 4] = ["claude-desktop", "cursor", "codex", "other"];

/// What a grant may do. Ordered: each level includes the one before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    ReadOnly,
    Delegate,
    Manage,
    /// Manage, and changing RustCode's own settings (#3821).
    Admin,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::Delegate => "delegate",
            Self::Manage => "manage",
            Self::Admin => "admin",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::ReadOnly => "Read-only",
            Self::Delegate => "Delegate",
            Self::Manage => "Manage",
            Self::Admin => "Admin",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_lowercase().replace('-', "_").as_str() {
            "read_only" | "readonly" | "read" => Some(Self::ReadOnly),
            "delegate" => Some(Self::Delegate),
            "manage" => Some(Self::Manage),
            "admin" => Some(Self::Admin),
            _ => None,
        }
    }

    fn meaning(self) -> &'static str {
        match self {
            Self::ReadOnly => {
                "read threads, usage, harnesses and machines; nothing is started, sent or changed"
            }
            Self::Delegate => {
                "read, and also start, fan out, message and interrupt threads (the work you start runs at Auto-edit)"
            }
            Self::Manage => {
                "delegate, and also the management tools (merge, clean up, projects, scheduled work) as RustCode offers them to outside agents"
            }
            Self::Admin => {
                "manage, and also change RustCode's own settings, skills, MCP servers, default harness and theme"
            }
        }
    }
}

/// The scope a tool needs, `None` for a tool no outside agent may call.
/// Everything that is not listed here is refused to every grant.
pub fn required_scope(tool: &str) -> Option<Scope> {
    use super::*;
    Some(match tool {
        WHOAMI
        | LIST_SESSIONS
        | READ_SESSION
        | SEARCH_SESSIONS
        | LIST_WORKTREES
        | LIST_HARNESSES
        | LIST_MACHINES
        | LIST_PROJECTS
        | LIST_PULL_REQUESTS
        | LIST_PENDING_REQUESTS
        | THREAD_USAGE
        | QUERY_DATA
        | READ_ACTIVITY
        | THREAD_STATUS
        | READ_PROJECT
        | LIST_SCHEDULED_TASKS
        | WAIT_SESSION
        | WORKTREE_STATUS
        | crate::manage::MANAGE_SETTINGS
        | crate::manage::MANAGE_CAPABILITIES => Scope::ReadOnly,
        START_SESSION | MESSAGE_SESSION | FAN_OUT | INTERRUPT_SESSION | MANAGE_QUEUE => {
            Scope::Delegate
        }
        MERGE_WHEN_GREEN
        | CLEANUP_WORKTREE
        | UPDATE_SESSION
        | CONFIGURE_SESSION
        | HANDOFF_WORKTREE
        | TRANSFER_SESSION
        | ORGANIZE_THREADS
        | ANSWER_REQUEST
        | CREATE_PROJECT
        | CLONE_PROJECT
        | UPDATE_PROJECT
        | REMOVE_PROJECT
        | SCHEDULE_TASK
        | DELETE_SCHEDULED_TASK
        | RUN_SCHEDULED_TASK_NOW => Scope::Manage,
        _ => return None,
    })
}

/// The scope one call needs: a manage tool's reads need Read-only, and its
/// writes Admin. Every other tool needs what [`required_scope`] says.
pub fn required_scope_for(tool: &str, args: &Value) -> Option<Scope> {
    if crate::manage::is_write(tool, args) {
        return Some(Scope::Admin);
    }
    required_scope(tool)
}

/// Whether the engine can run `tool` for an outside agent without a thread
/// behind it. A tool that is allowed by scope but not here is answered with
/// that, not listed.
pub fn implemented(tool: &str) -> bool {
    use super::*;
    matches!(
        tool,
        WHOAMI
            | LIST_SESSIONS
            | READ_SESSION
            | SEARCH_SESSIONS
            | LIST_PROJECTS
            | LIST_HARNESSES
            | LIST_MACHINES
            | THREAD_STATUS
            | WAIT_SESSION
            | THREAD_USAGE
            | QUERY_DATA
            | READ_ACTIVITY
            | START_SESSION
            | MESSAGE_SESSION
            | FAN_OUT
            | INTERRUPT_SESSION
            | UPDATE_SESSION
            | CONFIGURE_SESSION
            | MERGE_WHEN_GREEN
            | CLEANUP_WORKTREE
            | crate::manage::MANAGE_SETTINGS
            | crate::manage::MANAGE_CAPABILITIES
    )
}

/// Whether a call changes something, so a grant that asks must be answered
/// by the user first.
pub fn changes_things(tool: &str, args: &Value) -> bool {
    required_scope_for(tool, args).is_some_and(|scope| scope > Scope::ReadOnly)
}

// ------------------------------------------------------------------ grants

/// One connected app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    #[serde(default)]
    pub expires_at: Option<i64>,
    pub id: String,
    pub name: String,
    pub platform: String,
    pub scope: Scope,
    /// Ask the user in the app before each call that changes something.
    pub ask: bool,
    /// SHA-256 of the token, hex. The token itself is never stored.
    pub token_hash: String,
    pub created_at: String,
    #[serde(default)]
    pub last_used_at: Option<String>,
    #[serde(default)]
    pub last_tool: Option<String>,
    /// OAuth grants only: SHA-256 of the current refresh token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_hash: Option<String>,
    /// OAuth grants only: when the sign-in ends and the app must be approved again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_until: Option<i64>,
    /// OAuth grants only: SHA-256 of the client id the refresh token is bound to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oauth_client: Option<String>,
}

impl Grant {
    /// An OAuth grant whose access token and sign-in have both run out.
    fn is_dead(&self, now: i64) -> bool {
        self.expires_at.is_some_and(|expiry| expiry <= now)
            && self.refresh_until.is_none_or(|until| until <= now)
    }

    /// What Settings shows: everything but the hash.
    pub fn view(&self) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "platform": self.platform,
            "scope": self.scope.as_str(),
            "ask": self.ask,
            "createdAt": self.created_at,
            "lastUsedAt": self.last_used_at,
            "lastTool": self.last_tool,
            // A hosted agent that signed in on the public page, and until when.
            "hosted": self.oauth_client.is_some(),
            "signedInUntil": self.refresh_until,
        })
    }
}

#[derive(Default, Serialize, Deserialize)]
struct Store {
    #[serde(default)]
    clients: Vec<Grant>,
}

/// Serialises read-modify-write of the grant file inside this process.
static STORE_LOCK: Mutex<()> = Mutex::new(());
static TOUCHED: LazyLock<Mutex<std::collections::HashMap<String, Instant>>> =
    LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));

/// Where the machine's grants are kept.
pub fn grant_path() -> Option<PathBuf> {
    crate::config::get_config_dir().map(|dir| dir.join(GRANT_FILE))
}

fn lock() -> std::sync::MutexGuard<'static, ()> {
    STORE_LOCK.lock().unwrap_or_else(|error| error.into_inner())
}

fn read(path: &Path) -> Vec<Grant> {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Store>(&bytes).ok())
        .map(|store| store.clients)
        .unwrap_or_default()
}

fn write(path: &Path, clients: &[Grant]) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(&json!({ "clients": clients }))
        .map_err(|error| error.to_string())?;
    crate::service::write_private(path, &bytes).map_err(|error| error.to_string())
}

pub fn list_at(path: &Path) -> Vec<Grant> {
    let _guard = lock();
    read(path)
}

pub fn list() -> Vec<Grant> {
    grant_path().map(|path| list_at(&path)).unwrap_or_default()
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex::encode(hasher.finalize())
}

/// Equal in time that does not depend on where they differ.
pub(super) fn same(left: &str, right: &str) -> bool {
    left.len() == right.len()
        && left
            .bytes()
            .zip(right.bytes())
            .fold(0u8, |diff, (a, b)| diff | (a ^ b))
            == 0
}

/// A new grant and its token, the only time the token exists in the clear.
pub fn create_at(
    path: &Path,
    name: &str,
    platform: &str,
    scope: Scope,
    ask: Option<bool>,
) -> Result<(Grant, String), String> {
    let name: String = name.trim().chars().take(MAX_NAME_CHARS).collect();
    if name.is_empty() {
        return Err("name the app that will connect, so you can recognise it later".to_owned());
    }
    let platform = platform.trim().to_lowercase();
    let platform = if PLATFORMS.contains(&platform.as_str()) {
        platform
    } else {
        "other".to_owned()
    };
    let _guard = lock();
    let mut clients = read(path);
    // OAuth sign-ins that ran out would otherwise fill the store.
    let now_secs = chrono::Utc::now().timestamp();
    clients.retain(|grant| !grant.is_dead(now_secs));
    if clients.len() >= MAX_GRANTS {
        return Err(format!(
            "{MAX_GRANTS} apps are already connected; revoke one first"
        ));
    }
    let id = loop {
        let id = format!("{:08x}", rand::random::<u32>());
        if clients.iter().all(|grant| grant.id != id) {
            break id;
        }
    };
    let secret = format!(
        "{:032x}{:032x}",
        rand::random::<u128>(),
        rand::random::<u128>()
    );
    let token = format!("{TOKEN_PREFIX}{id}_{secret}");
    let grant = Grant {
        expires_at: None,
        id,
        name,
        platform,
        scope,
        // Changes ask by default; the owner turns it off per app.
        ask: ask.unwrap_or(scope > Scope::ReadOnly),
        token_hash: hash_token(&token),
        created_at: now(),
        last_used_at: None,
        last_tool: None,
        refresh_hash: None,
        refresh_until: None,
        oauth_client: None,
    };
    clients.push(grant.clone());
    write(path, &clients)?;
    Ok((grant, token))
}

/// Change a grant's access or whether it asks. Takes effect on its next call.
pub fn update_at(
    path: &Path,
    id: &str,
    scope: Option<Scope>,
    ask: Option<bool>,
) -> Result<Option<Grant>, String> {
    let _guard = lock();
    let mut clients = read(path);
    let Some(grant) = clients.iter_mut().find(|grant| grant.id == id) else {
        return Ok(None);
    };
    if let Some(scope) = scope {
        grant.scope = scope;
    }
    if let Some(ask) = ask {
        grant.ask = ask;
    }
    let updated = grant.clone();
    write(path, &clients)?;
    Ok(Some(updated))
}

/// Forget a grant: its token stops working with its next call. Whether it existed.
pub fn revoke_at(path: &Path, id: &str) -> Result<bool, String> {
    let _guard = lock();
    let mut clients = read(path);
    let before = clients.len();
    clients.retain(|grant| grant.id != id);
    if clients.len() == before {
        return Ok(false);
    }
    write(path, &clients)?;
    Ok(true)
}

/// The grant `token` belongs to. Read from the file on every call, so a
/// revocation or a change of scope applies to the very next call.
pub fn authenticate_at(path: &Path, token: &str) -> Option<Grant> {
    authenticate_transport_at(path, token, false)
}

pub(super) fn authenticate_transport_at(path: &Path, token: &str, http: bool) -> Option<Grant> {
    let rest = token.strip_prefix(if http { "rch_" } else { TOKEN_PREFIX })?;
    let (id, _) = rest.split_once('_')?;
    let hash = hash_token(token.trim());
    list_at(path).into_iter().find(|grant| {
        grant.id == id
            && same(&grant.token_hash, &hash)
            && grant
                .expires_at
                .is_none_or(|expiry| expiry > chrono::Utc::now().timestamp())
    })
}

fn touch_at(path: &Path, id: &str, tool: &str) {
    {
        let mut touched = TOUCHED.lock().unwrap_or_else(|error| error.into_inner());
        let key = format!("{}|{id}", path.display());
        if touched
            .get(&key)
            .is_some_and(|last| last.elapsed() < TOUCH_EVERY)
        {
            return;
        }
        touched.insert(key, Instant::now());
    }
    let _guard = lock();
    let mut clients = read(path);
    if let Some(grant) = clients.iter_mut().find(|grant| grant.id == id) {
        grant.last_used_at = Some(now());
        grant.last_tool = Some(tool.to_owned());
        let _ = write(path, &clients);
    }
}

/// The caller id of a grant inside the engine.
pub fn caller_of(grant_id: &str) -> String {
    format!("{CALLER_PREFIX}{grant_id}")
}

/// Whether `caller` is an outside agent rather than a thread.
pub fn is_caller(caller: &str) -> bool {
    caller.starts_with(CALLER_PREFIX)
}

/// The scope of the grant behind `caller`, `None` when it is not an outside
/// caller or its grant is gone.
pub fn scope_of_caller(caller: &str) -> Option<Scope> {
    let id = caller.strip_prefix(CALLER_PREFIX)?;
    list()
        .into_iter()
        .find(|grant| grant.id == id)
        .map(|g| g.scope)
}

/// The name of the app behind `caller`, for the header of a message it sends.
pub fn name_of_caller(caller: &str) -> Option<String> {
    let id = caller.strip_prefix(CALLER_PREFIX)?;
    list()
        .into_iter()
        .find(|grant| grant.id == id)
        .map(|grant| grant.name)
}

/// Make sure this engine's hub is listening, so `rustcode mcp-connect` can
/// find it. Nothing on a platform without the hub's socket.
pub async fn ensure_hub() {
    #[cfg(unix)]
    if let Err(error) = super::hub::shared().await {
        crate::dbg_log!("outside agents: {error}");
    }
}

/// Start listening at engine start when any app is connected, so it does not
/// have to wait for a thread or for Settings to open.
pub async fn bind_hub_when_connected() {
    let _ = super::outside_http::ensure().await;
    if !list().is_empty() {
        ensure_hub().await;
    }
}

// ---------------------------------------------------------------- snippets

/// The config to paste into the other app, for `platform`: its format, a hint
/// of where it goes, and the text. The token is in the text, so this is
/// handed back once, when the grant is made.
pub fn snippet(platform: &str, command: &str, token: &str) -> Value {
    let command_json = serde_json::to_string(command).unwrap_or_else(|_| "\"rustcode\"".into());
    let token_json = serde_json::to_string(token).unwrap_or_default();
    if platform == "codex" {
        let text = format!(
            "[mcp_servers.rustcode]\ncommand = {command_json}\nargs = [\"mcp-connect\"]\n# A call can wait for you to approve it in RustCode.\ntool_timeout_sec = {}\n\n[mcp_servers.rustcode.env]\n{TOKEN_ENV} = {token_json}\n",
            super::hub::TOOL_TIMEOUT_SECS
        );
        return json!({
            "format": "toml",
            "where": "~/.codex/config.toml",
            "text": text,
        });
    }
    let where_ = match platform {
        "claude-desktop" => {
            "Claude Desktop › Settings › Developer › Edit Config (claude_desktop_config.json)"
        }
        "cursor" => "~/.cursor/mcp.json, or Cursor › Settings › MCP",
        _ => "your app's MCP server configuration",
    };
    let value = json!({
        "mcpServers": {
            "rustcode": {
                "command": command,
                "args": ["mcp-connect"],
                "env": { TOKEN_ENV: token },
            }
        }
    });
    json!({
        "format": "json",
        "where": where_,
        "text": serde_json::to_string_pretty(&value).unwrap_or_default(),
    })
}

// --------------------------------------------------------------- approvals

struct Pending {
    id: String,
    client_id: String,
    client_name: String,
    tool: String,
    summary: String,
    asked_at: String,
    answer: Option<oneshot::Sender<bool>>,
}

static PENDING: LazyLock<Mutex<Vec<Pending>>> = LazyLock::new(|| Mutex::new(Vec::new()));

fn pending() -> std::sync::MutexGuard<'static, Vec<Pending>> {
    PENDING.lock().unwrap_or_else(|error| error.into_inner())
}

/// What the app shows to ask: the app, the tool and a short description, with
/// no more of the arguments than the user needs to decide.
pub fn pending_view() -> Vec<Value> {
    pending()
        .iter()
        .map(|item| {
            json!({
                "id": item.id,
                "clientId": item.client_id,
                "clientName": item.client_name,
                "tool": super::mcp_names::public_tool(&item.tool),
                "summary": item.summary,
                "askedAt": item.asked_at,
            })
        })
        .collect()
}

/// Answer a pending call. Whether it was still waiting.
pub fn answer_pending(id: &str, allow: bool) -> bool {
    let sender = {
        let mut waiting = pending();
        waiting
            .iter_mut()
            .find(|item| item.id == id)
            .and_then(|item| item.answer.take())
    };
    sender.is_some_and(|sender| sender.send(allow).is_ok())
}

/// Revoking a grant also ends the questions it has open.
pub fn deny_all_for(client_id: &str) {
    let senders: Vec<_> = pending()
        .iter_mut()
        .filter(|item| item.client_id == client_id)
        .filter_map(|item| item.answer.take())
        .collect();
    for sender in senders {
        let _ = sender.send(false);
    }
}

fn withdraw(id: &str) {
    pending().retain(|item| item.id != id);
    changed();
}

/// Bumped whenever something waiting for the owner appears or goes (a call to
/// allow, a sign-in to approve), so the apps connected to this engine ask
/// again at once instead of at their next poll (`_rustcode/outside/changed`).
static CHANGED: LazyLock<tokio::sync::watch::Sender<u64>> =
    LazyLock::new(|| tokio::sync::watch::channel(0).0);

pub fn changed() {
    CHANGED.send_modify(|count| *count = count.wrapping_add(1));
}

/// Follow [`changed`].
pub fn watch_changes() -> tokio::sync::watch::Receiver<u64> {
    CHANGED.subscribe()
}

/// What the call does, in a line, from the few fields that say it. Text a
/// call carries is cut, and nothing else of the arguments is shown.
pub fn summarize(tool: &str, args: &Value) -> String {
    let text = |key: &str, limit: usize| -> Option<String> {
        args.get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| {
                let cut: String = value.chars().take(limit).collect();
                if value.chars().count() > limit {
                    format!("{cut}…")
                } else {
                    cut
                }
            })
    };
    let mut parts: Vec<String> = Vec::new();
    for (key, label, limit) in [
        ("project", "in", 120),
        ("harness", "on", 40),
        ("session_id", "thread", 60),
        ("title", "titled", 80),
    ] {
        if let Some(value) = text(key, limit) {
            parts.push(format!("{label} {value}"));
        }
    }
    for key in ["prompt", "message"] {
        if let Some(value) = text(key, 160) {
            parts.push(format!("“{value}”"));
        }
    }
    if parts.is_empty() {
        tool.to_owned()
    } else {
        format!("{tool} {}", parts.join(" "))
    }
}

// ------------------------------------------------------------- the runner

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// What runs an allowed call: the engine, or a stand-in in tests.
pub trait Runner: Send + Sync {
    fn run<'a>(
        &'a self,
        caller: &'a str,
        cancel: &'a CancellationToken,
        tool: &'a str,
        args: &'a Value,
    ) -> BoxFuture<'a, CallAnswer>;

    /// What an outside agent should know exists: projects, machines and harnesses.
    fn context<'a>(
        &'a self,
        caller: &'a str,
        cancel: &'a CancellationToken,
    ) -> BoxFuture<'a, Value>;
}

/// The engine answering for itself, without a thread behind the call.
pub struct EngineRunner;

impl Runner for EngineRunner {
    fn run<'a>(
        &'a self,
        caller: &'a str,
        cancel: &'a CancellationToken,
        tool: &'a str,
        args: &'a Value,
    ) -> BoxFuture<'a, CallAnswer> {
        Box::pin(async move {
            let mut args = args.clone();
            match tool {
                // An outside agent has no project of its own: list everything,
                // as far as the cross-project setting allows.
                super::LIST_SESSIONS | super::SEARCH_SESSIONS => {
                    if args.get("all_projects").is_none() {
                        args["all_projects"] = json!(true);
                    }
                }
                // What it starts is a thread of the user's own, never a
                // delegate of a thread that does not exist.
                super::START_SESSION | super::FAN_OUT => args["standalone"] = json!(true),
                _ => {}
            }
            if crate::manage::is_tool(tool) {
                // The gate has checked the scope and asked when the grant
                // asks; a grant that does not ask was set so by the user.
                let answer =
                    crate::manage::answer(tool, &args, crate::manage::Approval::Granted, cancel)
                        .await;
                return CallAnswer {
                    success: answer.success,
                    content: answer.content,
                };
            }
            let output = match tool {
                super::THREAD_USAGE | super::QUERY_DATA | super::READ_ACTIVITY => {
                    data_tool(tool, &args)
                }
                _ => match super::outside_manage::answer(caller, cancel, tool, &args).await {
                    Some(answer) => super::output(tool, answer),
                    None => super::local::answer(caller, cancel, tool, &args).await,
                },
            };
            CallAnswer {
                success: output.success,
                content: output.content,
            }
        })
    }

    fn context<'a>(
        &'a self,
        caller: &'a str,
        cancel: &'a CancellationToken,
    ) -> BoxFuture<'a, Value> {
        Box::pin(async move {
            let mut seen = std::collections::HashSet::new();
            let mut projects: Vec<String> = Vec::new();
            for meta in crate::config::list_sessions() {
                let Some(id) = crate::config::session_id_from_path(&meta.path) else {
                    continue;
                };
                let project = super::local::project_of(&id, &meta);
                if !project.is_empty() && seen.insert(project.clone()) {
                    projects.push(project);
                }
                if projects.len() >= 30 {
                    break;
                }
            }
            let mut context = json!({ "projects": projects });
            let machines = json_of(
                EngineRunner
                    .run(caller, cancel, super::LIST_MACHINES, &json!({}))
                    .await,
            );
            if let Some(machines) = machines {
                context["machines"] = machines;
            }
            if let Some(harnesses) = json_of(
                EngineRunner
                    .run(caller, cancel, super::LIST_HARNESSES, &json!({}))
                    .await,
            ) {
                let rows: Vec<Value> = harnesses
                    .get("harnesses")
                    .and_then(Value::as_array)
                    .map(|rows| {
                        rows.iter()
                            .map(|row| {
                                json!({
                                    "id": row.get("id"),
                                    "name": row.get("name"),
                                    "accounts": row.get("accounts"),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                context["harnesses"] = json!(rows);
            }
            context
        })
    }
}

fn json_of(answer: CallAnswer) -> Option<Value> {
    answer
        .success
        .then(|| serde_json::from_str(&answer.content).ok())
        .flatten()
}

/// `thread_usage`, `query_data` and `read_activity` read the local data store.
/// They cannot be narrowed to a project, so they follow the same switch as
/// reading threads across projects.
fn data_tool(tool: &str, args: &Value) -> crate::tools::ToolExecutionOutput {
    use crate::acp::data;
    let text = |key: &str| args.get(key).and_then(Value::as_str).map(str::to_owned);
    let number = |key: &str| {
        args.get(key)
            .and_then(Value::as_u64)
            .map(|value| value.min(u64::from(u32::MAX)) as u32)
    };
    let answer = if !super::local::read_across_projects() {
        Err(super::local::reach_refused())
    } else {
        match tool {
            super::THREAD_USAGE => data::usage(data::DataUsageRequest {
                since: text("since"),
                group_by: text("group_by"),
                project: text("project"),
            }),
            super::QUERY_DATA => data::query(data::DataQueryRequest {
                sql: text("sql").unwrap_or_default(),
                limit: number("limit"),
            }),
            _ => data::activity(data::DataActivityRequest {
                session_id: text("session_id").unwrap_or_default(),
                after: text("after"),
                order: text("order"),
                limit: number("limit"),
            }),
        }
    };
    match answer {
        Ok(value) => super::output(tool, value),
        Err(message) => super::output(
            tool,
            json!({ "error": { "code": "failed", "message": message } }),
        ),
    }
}

// ----------------------------------------------------------- the gate

/// Where the gate reads and writes. The engine's own locations in production,
/// temporary ones in tests.
pub struct Settings {
    pub grants: PathBuf,
    pub data: Option<PathBuf>,
    pub approval_wait: Duration,
    /// Refuse at once a call that would ask while no app can answer it,
    /// instead of waiting [`APPROVAL_WAIT`] for nobody. Off in tests that
    /// answer the pending call themselves.
    pub needs_approver: bool,
}

impl Settings {
    pub fn machine() -> Option<Self> {
        Some(Self {
            grants: grant_path()?,
            data: crate::data_store::data_path(),
            approval_wait: APPROVAL_WAIT,
            needs_approver: true,
        })
    }
}

fn refusal(text: impl Into<String>) -> CallAnswer {
    CallAnswer::failure(text)
}

fn record(
    settings: &Settings,
    grant: &Grant,
    tool: &str,
    outcome: &str,
    started: Instant,
    detail: Option<String>,
) {
    let Some(path) = settings.data.as_ref() else {
        return;
    };
    let row = crate::data_store::OutsideCallRow {
        client_id: grant.id.clone(),
        client_name: grant.name.clone(),
        platform: Some(grant.platform.clone()),
        scope: grant.scope.as_str().to_owned(),
        tool: tool.to_owned(),
        outcome: outcome.to_owned(),
        duration_ms: Some(started.elapsed().as_millis().min(i64::MAX as u128) as i64),
        detail,
    };
    if let Err(error) = crate::data_store::record_outside_call_at(path, &row) {
        crate::dbg_log!("outside-agent call not recorded: {error}");
    }
}

/// The call, from the engine's own hub.
pub async fn answer(request: CallRequest, cancel: &CancellationToken) -> CallAnswer {
    let Some(settings) = Settings::machine() else {
        return refusal("error: RustCode has no configuration directory. Nothing was sent.");
    };
    answer_with(&settings, &EngineRunner, request, cancel).await
}

/// Every call from an outside agent passes here, in the engine: the bridge in
/// front of it only carries bytes.
pub async fn answer_with(
    settings: &Settings,
    runner: &dyn Runner,
    request: CallRequest,
    cancel: &CancellationToken,
) -> CallAnswer {
    answer_transport_with(settings, runner, request, cancel, false).await
}

pub(super) async fn answer_transport_with(
    settings: &Settings,
    runner: &dyn Runner,
    request: CallRequest,
    cancel: &CancellationToken,
    http: bool,
) -> CallAnswer {
    let started = Instant::now();
    let Some(grant) = authenticate_transport_at(&settings.grants, &request.token, http) else {
        return refusal(
            "error: RustCode did not accept this token. It may have been revoked; create a new one in RustCode › Settings › Outside agents. Nothing was sent.",
        );
    };
    let tool = request.tool.as_str();
    let caller = caller_of(&grant.id);
    match tool {
        CONTROL_INSTRUCTIONS => {
            return CallAnswer {
                success: true,
                content: instructions(&grant),
            };
        }
        CONTROL_TOOLS => {
            return CallAnswer {
                success: true,
                content: json!(tools_for(grant.scope)).to_string(),
            };
        }
        _ => {}
    }
    // The scope is checked before anything else about the call, so a call the
    // grant may not make never reaches validation, the app, or a thread.
    let Some(needed) = required_scope_for(tool, &request.args) else {
        record(
            settings,
            &grant,
            tool,
            "denied",
            started,
            Some("not an outside-agent tool".into()),
        );
        return refusal(format!(
            "error: {tool} is not available to outside agents. Call {WHOAMI} to see what this connection can do. Nothing was sent."
        ));
    };
    if grant.scope < needed {
        record(
            settings,
            &grant,
            tool,
            "denied",
            started,
            Some(format!("needs {}", needed.as_str())),
        );
        return refusal(format!(
            "error: {tool} needs {} access, and “{}” is connected with {}. Ask the user to change its access in RustCode › Settings › Outside agents. Nothing was sent.",
            needed.label(),
            grant.name,
            grant.scope.label()
        ));
    }
    if !implemented(tool) {
        record(settings, &grant, tool, "unavailable", started, None);
        return refusal(format!(
            "error: {tool} is allowed for this connection but RustCode cannot run it for an outside agent yet. Nothing was sent."
        ));
    }
    if tool == WHOAMI {
        let content = whoami(&grant, runner, &caller, cancel).await;
        touch_at(&settings.grants, &grant.id, tool);
        record(settings, &grant, tool, "ok", started, None);
        return CallAnswer {
            success: true,
            content: content.to_string(),
        };
    }
    let checked = if crate::manage::is_tool(tool) {
        crate::manage::validate(tool, &request.args)
    } else {
        super::validate(tool, &request.args)
    };
    if let Err(reason) = checked {
        return refusal(format!(
            "error: invalid {tool} call: {reason}. Nothing was sent."
        ));
    }
    if grant.ask && changes_things(tool, &request.args) {
        if settings.needs_approver && !approver_connected() {
            record(
                settings,
                &grant,
                tool,
                "declined",
                started,
                Some("no app to ask".into()),
            );
            return refusal(no_approver(&grant, tool));
        }
        match ask_user(settings, &grant, tool, &request.args, cancel).await {
            Approval::Allowed => {}
            Approval::Declined => {
                record(settings, &grant, tool, "declined", started, None);
                return refusal(format!(
                    "error: the user declined {tool} in RustCode. Nothing was sent. Do not retry it unless the user asks."
                ));
            }
            Approval::Unanswered => {
                record(
                    settings,
                    &grant,
                    tool,
                    "declined",
                    started,
                    Some("no answer".into()),
                );
                return refusal(format!(
                    "error: the user did not answer {tool} in RustCode in time. Nothing was sent. Ask the user to open RustCode, then try again."
                ));
            }
            Approval::Cancelled => {
                record(
                    settings,
                    &grant,
                    tool,
                    "unavailable",
                    started,
                    Some("cancelled".into()),
                );
                return refusal(format!("error: {tool} was cancelled. Nothing was sent."));
            }
        }
        // The grant may have been changed or revoked while the user decided.
        match authenticate_transport_at(&settings.grants, &request.token, http) {
            Some(current) if current.scope >= needed => {}
            _ => {
                record(
                    settings,
                    &grant,
                    tool,
                    "denied",
                    started,
                    Some("changed while asking".into()),
                );
                return refusal(format!(
                    "error: this connection's access changed while {tool} was waiting. Nothing was sent."
                ));
            }
        }
    }
    touch_at(&settings.grants, &grant.id, tool);
    let answer = runner.run(&caller, cancel, tool, &request.args).await;
    record(
        settings,
        &grant,
        tool,
        if answer.success { "ok" } else { "error" },
        started,
        None,
    );
    answer
}

/// What tells whether an app connected to this engine shows the calls that
/// wait for the owner: one that listed outside agents. Set by the engine's
/// ACP server; unset (no server in this process) counts as connected, so the
/// call waits as it always did.
type ApproverCheck = Box<dyn Fn() -> bool + Send + Sync>;
static APPROVERS: Mutex<Option<ApproverCheck>> = Mutex::new(None);

pub fn set_approvers(check: impl Fn() -> bool + Send + Sync + 'static) {
    *APPROVERS.lock().unwrap_or_else(|error| error.into_inner()) = Some(Box::new(check));
}

fn approver_connected() -> bool {
    APPROVERS
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .as_ref()
        .is_none_or(|check| check())
}

/// The refusal for a call that would ask on a machine where no app can
/// answer (a headless server): say how to stop asking, rather than time out.
fn no_approver(grant: &Grant, tool: &str) -> String {
    format!(
        "error: {tool} needs the user's approval in the RustCode app, and no RustCode app is connected to this machine to ask. Nothing was sent. Tell the user: open the RustCode app (on this machine or one paired with it) and retry, or let “{}” act without asking by running `rustcode outside update {} --ask false` on this machine.",
        grant.name, grant.id
    )
}

enum Approval {
    Allowed,
    Declined,
    Unanswered,
    Cancelled,
}

async fn ask_user(
    settings: &Settings,
    grant: &Grant,
    tool: &str,
    args: &Value,
    cancel: &CancellationToken,
) -> Approval {
    let id = format!("{:016x}", rand::random::<u64>());
    let (sender, receiver) = oneshot::channel();
    pending().push(Pending {
        id: id.clone(),
        client_id: grant.id.clone(),
        client_name: grant.name.clone(),
        tool: tool.to_owned(),
        summary: summarize(tool, args),
        asked_at: now(),
        answer: Some(sender),
    });
    changed();
    let outcome = tokio::select! {
        biased;
        _ = cancel.cancelled() => Approval::Cancelled,
        answer = tokio::time::timeout(settings.approval_wait, receiver) => match answer {
            Ok(Ok(true)) => Approval::Allowed,
            Ok(Ok(false)) | Ok(Err(_)) => Approval::Declined,
            Err(_) => Approval::Unanswered,
        },
    };
    withdraw(&id);
    outcome
}

// -------------------------------------------------- what the agent is told

/// The server instructions an outside agent reads when it connects.
pub fn instructions(grant: &Grant) -> String {
    let ask = if grant.ask && grant.scope > Scope::ReadOnly {
        " Each call that changes something is first put to the user in the RustCode app; if they decline or do not answer, the call fails and you must not retry it unless they ask."
    } else {
        ""
    };
    format!(
        "You are connected to RustCode, the app that owns the user's coding-agent threads, projects, settings and usage on this computer and on the machines paired with it. Each thread runs on a harness (the built-in one, Codex, Claude Code, Gemini CLI, OpenCode or Pi). You are “{}” with {} access: {}.{ask}\n\nStart with `{WHOAMI}`: it lists this connection's access, the tools it has and lacks, the projects, machines and harnesses that exist. `list_threads` finds threads and `read_thread` reads one. `thread_usage` and `query_data` answer usage questions. With Delegate access `start_thread` starts a standalone thread in a project `{WHOAMI}` lists, `fan_out` starts the same task on two to four harnesses, `message_thread` sends an existing thread a message, `interrupt_thread` stops a running turn, and `wait_thread` returns an answer. With Manage access `merge_when_green`, `cleanup_worktree`, `update_thread` and `configure_thread` act on a thread you name with `thread_id`. `manage_settings` and `manage_capabilities` read RustCode's settings, skills, MCP servers, harnesses and themes; with Admin access they also change them. Work you start runs at Auto-edit: it can edit files but a command that needs approval is declined. Tools outside your access are not listed.",
        grant.name,
        grant.scope.label(),
        grant.scope.meaning(),
    )
}

/// The MCP tool entries a grant of `scope` sees: its scope's tools that the
/// engine can run for it, plus `whoami`.
pub fn tools_for(scope: Scope) -> Vec<Value> {
    let mut entries: Vec<Value> = vec![json!({
        "name": WHOAMI,
        "description": "Who is calling and what it may do: this connection's name and access, the tools it has and the ones it lacks, and the projects, machines and harnesses RustCode knows. Call it first.",
        "inputSchema": { "type": "object", "properties": {} },
    })];
    for mut entry in super::mcp_server::tools(super::VERSION, false) {
        let Some(name) = entry.get("name").and_then(Value::as_str).map(str::to_owned) else {
            continue;
        };
        let Some(needed) = required_scope(&name) else {
            continue;
        };
        if needed > scope || !implemented(&name) || name == WHOAMI {
            continue;
        }
        if needed > Scope::ReadOnly {
            let note = format!(
                "\n\n[Outside agent: needs {} access; the user may be asked to approve it in RustCode.{}]",
                needed.label(),
                if name == super::START_SESSION || name == super::FAN_OUT {
                    " The thread starts standalone and runs at Auto-edit."
                } else {
                    ""
                }
            );
            if let Some(description) = entry.get("description").and_then(Value::as_str) {
                entry["description"] = json!(format!("{description}{note}"));
            }
        }
        if crate::manage::is_tool(&name) {
            let note = if scope >= Scope::Admin {
                "\n\n[Outside agent: changes need Admin access, which this connection has; the user may be asked to approve them in RustCode.]"
            } else {
                "\n\n[Outside agent: reads only; changes need Admin access, which this connection does not have.]"
            };
            if let Some(description) = entry.get("description").and_then(Value::as_str) {
                entry["description"] = json!(format!("{description}{note}"));
            }
        }
        entries.push(super::mcp_names::public_entry(entry));
    }
    entries
}

async fn whoami(
    grant: &Grant,
    runner: &dyn Runner,
    caller: &str,
    cancel: &CancellationToken,
) -> Value {
    let mut can: Vec<&str> = Vec::new();
    let mut needs_more: Vec<Value> = Vec::new();
    let mut not_yet: Vec<&str> = Vec::new();
    for name in super::tool_names(super::VERSION) {
        let Some(needed) = required_scope(name) else {
            continue;
        };
        let public = super::mcp_names::public_tool(name);
        if !implemented(name) {
            not_yet.push(public);
        } else if needed <= grant.scope {
            can.push(public);
        } else {
            needs_more.push(json!({ "tool": public, "needs": needed.as_str() }));
        }
    }
    can.insert(0, WHOAMI);
    let mut answer = json!({
        "rustcode": "RustCode owns the user's coding-agent threads, projects, settings and usage and runs each thread on a harness (built-in, Codex, Claude Code, Gemini CLI, OpenCode, Pi).",
        "client": grant.view(),
        "access": {
            "level": grant.scope.as_str(),
            "meaning": grant.scope.meaning(),
            "asksBeforeChanging": grant.ask && grant.scope > Scope::ReadOnly,
            "can": can,
            "needsMoreAccess": needs_more,
            "notAvailableToOutsideAgentsYet": not_yet,
        },
        "notes": [
            "Threads you start are standalone threads of the user's, not delegates of a thread.",
            "Work you start runs at Auto-edit: files can be edited, a command that needs approval is declined.",
        ],
    });
    let context = runner.context(caller, cancel).await;
    if let (Some(answer), Some(context)) = (answer.as_object_mut(), context.as_object()) {
        for (key, value) in context {
            answer.insert(key.clone(), value.clone());
        }
    }
    answer
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Counts what ran, answers like a tool that worked.
    struct Counting(AtomicUsize);

    impl Runner for Counting {
        fn run<'a>(
            &'a self,
            _caller: &'a str,
            _cancel: &'a CancellationToken,
            tool: &'a str,
            _args: &'a Value,
        ) -> BoxFuture<'a, CallAnswer> {
            Box::pin(async move {
                self.0.fetch_add(1, Ordering::SeqCst);
                CallAnswer {
                    success: true,
                    content: json!({ "ran": tool }).to_string(),
                }
            })
        }

        fn context<'a>(
            &'a self,
            _caller: &'a str,
            _cancel: &'a CancellationToken,
        ) -> BoxFuture<'a, Value> {
            Box::pin(async { json!({ "projects": ["/work/app"], "harnesses": [] }) })
        }
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        settings: Settings,
        runner: Counting,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let settings = Settings {
                grants: dir.path().join("outside-agents.json"),
                data: Some(dir.path().join("data.sqlite")),
                approval_wait: Duration::from_millis(300),
                needs_approver: false,
            };
            Self {
                _dir: dir,
                settings,
                runner: Counting(AtomicUsize::new(0)),
            }
        }

        fn grant(&self, scope: Scope, ask: bool) -> (Grant, String) {
            create_at(&self.settings.grants, "Cursor", "cursor", scope, Some(ask)).unwrap()
        }

        async fn call(&self, token: &str, tool: &str, args: Value) -> CallAnswer {
            answer_with(
                &self.settings,
                &self.runner,
                CallRequest {
                    session_id: String::new(),
                    token: token.to_owned(),
                    tool: tool.to_owned(),
                    args,
                    outside: true,
                },
                &CancellationToken::new(),
            )
            .await
        }

        fn ran(&self) -> usize {
            self.runner.0.load(Ordering::SeqCst)
        }

        fn audit(&self) -> Vec<Value> {
            crate::data_store::recent_outside_calls_at(
                self.settings.data.as_ref().unwrap(),
                None,
                50,
            )
            .unwrap()
        }
    }

    #[test]
    fn every_orchestration_tool_is_classified_or_deliberately_left_out() {
        // A tool nobody classified is refused to every grant. Adding one to
        // the orchestration set must be a decision, so list what is left out.
        let left_out: Vec<&str> = super::super::tool_names(super::super::VERSION)
            .into_iter()
            .filter(|name| required_scope(name).is_none())
            .collect();
        for name in &left_out {
            assert!(
                !implemented(name),
                "{name} is implemented but not classified"
            );
        }
        for name in [
            super::super::LIST_SESSIONS,
            super::super::START_SESSION,
            super::super::MERGE_WHEN_GREEN,
        ] {
            assert!(required_scope(name).is_some(), "{name}");
        }
        assert_eq!(required_scope("not_a_tool"), None);
    }

    #[test]
    fn discovery_tools_are_read_only_and_listed_for_every_level() {
        for name in [super::super::SEARCH_SESSIONS, super::super::LIST_PROJECTS] {
            assert_eq!(required_scope(name), Some(Scope::ReadOnly), "{name}");
            assert!(implemented(name), "{name}");
            assert!(!changes_things(name, &json!({})), "{name}");
            let listed = tools_for(Scope::ReadOnly);
            assert!(
                listed
                    .iter()
                    .any(|tool| tool["name"] == super::super::mcp_names::public_tool(name)),
                "{name} is listed to Read-only"
            );
        }
        // `manage_queue` is a thread's own queue: an outside agent has none.
        assert!(!implemented(super::super::MANAGE_QUEUE));
    }

    #[test]
    fn scopes_are_ordered_and_parse_from_what_people_type() {
        assert!(Scope::ReadOnly < Scope::Delegate && Scope::Delegate < Scope::Manage);
        assert_eq!(Scope::parse("read-only"), Some(Scope::ReadOnly));
        assert_eq!(Scope::parse(" Delegate "), Some(Scope::Delegate));
        assert_eq!(Scope::parse("root"), None);
        assert_eq!(
            required_scope(super::super::MERGE_WHEN_GREEN),
            Some(Scope::Manage)
        );
        assert_eq!(
            required_scope(super::super::MESSAGE_SESSION),
            Some(Scope::Delegate)
        );
        assert_eq!(
            required_scope(super::super::THREAD_USAGE),
            Some(Scope::ReadOnly)
        );
    }

    #[test]
    fn a_token_is_stored_as_a_hash_and_found_by_its_grant() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("outside-agents.json");
        let (grant, token) = create_at(
            &path,
            "  Claude Desktop  ",
            "claude-desktop",
            Scope::ReadOnly,
            None,
        )
        .unwrap();
        assert!(token.starts_with("rcx_"));
        assert_eq!(grant.name, "Claude Desktop");
        assert!(!grant.ask, "read-only never asks");
        let file = std::fs::read_to_string(&path).unwrap();
        assert!(!file.contains(&token), "the token is never written down");
        assert!(file.contains(&grant.token_hash));
        assert_eq!(authenticate_at(&path, &token).unwrap().id, grant.id);
        assert!(authenticate_at(&path, "rcx_00000000_wrong").is_none());
        // Same id, wrong secret.
        let forged = format!("rcx_{}_{}", grant.id, "0".repeat(64));
        assert!(authenticate_at(&path, &forged).is_none());
        assert!(grant.view().get("tokenHash").is_none());
        let (delegate, _) = create_at(&path, "Codex", "codex", Scope::Delegate, None).unwrap();
        assert!(delegate.ask, "a grant that changes things asks by default");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o077, 0, "only the owner reads the grants");
        }
    }

    #[test]
    fn grants_need_a_name_and_are_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("outside-agents.json");
        assert!(create_at(&path, "   ", "cursor", Scope::ReadOnly, None).is_err());
        for index in 0..MAX_GRANTS {
            create_at(
                &path,
                &format!("app {index}"),
                "other",
                Scope::ReadOnly,
                None,
            )
            .unwrap();
        }
        let error = create_at(&path, "one more", "other", Scope::ReadOnly, None).unwrap_err();
        assert!(error.contains("revoke"), "{error}");
        assert_eq!(list_at(&path)[0].platform, "other");
    }

    #[tokio::test]
    async fn a_read_only_token_cannot_start_message_or_merge() {
        let fixture = Fixture::new();
        let (_, token) = fixture.grant(Scope::ReadOnly, false);
        for (tool, args, needs) in [
            (
                super::super::START_SESSION,
                json!({ "prompt": "go", "project": "/work/app" }),
                "Delegate",
            ),
            (
                super::super::MESSAGE_SESSION,
                json!({ "session_id": "t", "message": "hi" }),
                "Delegate",
            ),
            (super::super::FAN_OUT, json!({}), "Delegate"),
            (
                super::super::MERGE_WHEN_GREEN,
                json!({ "session_id": "t" }),
                "Manage",
            ),
            (
                super::super::CLEANUP_WORKTREE,
                json!({ "session_id": "t" }),
                "Manage",
            ),
        ] {
            let answer = fixture.call(&token, tool, args).await;
            assert!(!answer.success, "{tool}");
            assert!(answer.content.contains(needs), "{tool}: {}", answer.content);
            assert!(answer.content.contains("Nothing was sent"), "{tool}");
        }
        assert_eq!(fixture.ran(), 0, "nothing reached the engine");
        let audit = fixture.audit();
        assert_eq!(audit.len(), 5);
        assert!(audit.iter().all(|row| row["outcome"] == "denied"));
        assert!(audit.iter().all(|row| row["scope"] == "read_only"));
    }

    #[tokio::test]
    async fn read_only_and_delegate_tokens_are_refused_the_tools_above_their_scope() {
        let fixture = Fixture::new();
        let (_, read) = fixture.grant(Scope::ReadOnly, false);
        let (_, delegate) = fixture.grant(Scope::Delegate, false);
        let named = json!({ "session_id": "t" });
        // Read-only: every tool that changes a thread.
        for (tool, needs) in [
            (super::super::FAN_OUT, "Delegate"),
            (super::super::INTERRUPT_SESSION, "Delegate"),
            (super::super::UPDATE_SESSION, "Manage"),
            (super::super::CONFIGURE_SESSION, "Manage"),
            (super::super::MERGE_WHEN_GREEN, "Manage"),
            (super::super::CLEANUP_WORKTREE, "Manage"),
        ] {
            let answer = fixture.call(&read, tool, named.clone()).await;
            assert!(!answer.success, "{tool}");
            assert!(answer.content.contains(needs), "{tool}: {}", answer.content);
        }
        // Delegate: stops and fans out, but does not manage.
        for tool in [
            super::super::UPDATE_SESSION,
            super::super::CONFIGURE_SESSION,
            super::super::MERGE_WHEN_GREEN,
            super::super::CLEANUP_WORKTREE,
        ] {
            let answer = fixture.call(&delegate, tool, named.clone()).await;
            assert!(
                !answer.success && answer.content.contains("Manage"),
                "{tool}"
            );
        }
        assert_eq!(fixture.ran(), 0, "nothing reached the engine");
        let audit = fixture.audit();
        assert_eq!(audit.len(), 10);
        assert!(audit.iter().all(|row| row["outcome"] == "denied"));
    }

    #[test]
    fn admin_is_above_manage_and_settings_writes_need_it() {
        assert!(Scope::Manage < Scope::Admin);
        assert_eq!(Scope::parse("Admin"), Some(Scope::Admin));
        assert_eq!(Scope::Admin.as_str(), "admin");
        let tool = crate::manage::MANAGE_SETTINGS;
        assert_eq!(required_scope(tool), Some(Scope::ReadOnly));
        assert_eq!(
            required_scope_for(tool, &json!({"action": "get", "key": "orchestration"})),
            Some(Scope::ReadOnly)
        );
        for action in ["set", "reset"] {
            assert_eq!(
                required_scope_for(tool, &json!({"action": action})),
                Some(Scope::Admin),
                "{action}"
            );
            assert!(changes_things(tool, &json!({"action": action})));
        }
        assert!(!changes_things(tool, &json!({"action": "list"})));
        let capabilities = crate::manage::MANAGE_CAPABILITIES;
        assert_eq!(
            required_scope_for(capabilities, &json!({"kind": "mcp", "action": "list"})),
            Some(Scope::ReadOnly)
        );
        assert_eq!(
            required_scope_for(
                capabilities,
                &json!({"kind": "mcp", "action": "disable", "name": "files"})
            ),
            Some(Scope::Admin)
        );
        assert_eq!(
            required_scope_for(capabilities, &json!({"kind": "nonsense"})),
            Some(Scope::Admin),
            "a call that does not parse is never treated as a read"
        );
        let listed = |scope| {
            tools_for(scope)
                .into_iter()
                .find(|entry| entry["name"] == json!(tool))
                .and_then(|entry| entry["description"].as_str().map(str::to_owned))
                .unwrap_or_default()
        };
        assert!(listed(Scope::ReadOnly).contains("reads only"));
        assert!(listed(Scope::Admin).contains("which this connection has"));
    }

    #[tokio::test]
    async fn a_settings_write_below_admin_is_refused_with_the_level_it_needs() {
        let fixture = Fixture::new();
        let (_, manage) = fixture.grant(Scope::Manage, false);
        let (_, admin) = fixture.grant(Scope::Admin, false);
        let tool = crate::manage::MANAGE_SETTINGS;
        let write = json!({"action": "set", "key": "orchestration", "value": false});
        let refused = fixture.call(&manage, tool, write.clone()).await;
        assert!(!refused.success);
        assert!(
            refused.content.contains("needs Admin access"),
            "{}",
            refused.content
        );
        assert_eq!(fixture.ran(), 0, "nothing reached the engine");
        let read = fixture
            .call(
                &manage,
                tool,
                json!({"action": "get", "key": "orchestration"}),
            )
            .await;
        assert!(read.success, "{}", read.content);
        let allowed = fixture.call(&admin, tool, write).await;
        assert!(allowed.success, "{}", allowed.content);
        assert_eq!(fixture.ran(), 2);
        let invalid = fixture
            .call(
                &admin,
                tool,
                json!({"action": "set", "key": "theme", "value": 1}),
            )
            .await;
        assert!(!invalid.success && invalid.content.contains("unknown setting"));
    }

    #[tokio::test]
    async fn a_delegate_token_fans_out_and_interrupts_and_is_audited() {
        let fixture = Fixture::new();
        let (_, token) = fixture.grant(Scope::Delegate, false);
        let fan_out = fixture
            .call(
                &token,
                super::super::FAN_OUT,
                json!({
                    "prompt": "compare",
                    "project": "/work/app",
                    "harnesses": [{ "harness": "codex" }, { "harness": "claude-code" }],
                }),
            )
            .await;
        assert!(fan_out.success, "{}", fan_out.content);
        let stop = fixture
            .call(
                &token,
                super::super::INTERRUPT_SESSION,
                json!({ "session_id": "t" }),
            )
            .await;
        assert!(stop.success, "{}", stop.content);
        assert_eq!(fixture.ran(), 2);
        let audit = fixture.audit();
        assert_eq!(audit.len(), 2);
        assert!(audit.iter().all(|row| row["outcome"] == "ok"));
        assert!(audit.iter().all(|row| row["scope"] == "delegate"));
        let tools: Vec<&str> = audit
            .iter()
            .filter_map(|row| row["tool"].as_str())
            .collect();
        assert!(tools.contains(&"fan_out") && tools.contains(&"interrupt_session"));
    }

    #[tokio::test]
    async fn a_manage_token_runs_the_management_tools_and_an_invalid_call_never_runs() {
        let fixture = Fixture::new();
        let (_, token) = fixture.grant(Scope::Manage, false);
        for (tool, args) in [
            (super::super::MERGE_WHEN_GREEN, json!({ "session_id": "t" })),
            (super::super::CLEANUP_WORKTREE, json!({ "session_id": "t" })),
            (
                super::super::UPDATE_SESSION,
                json!({ "session_id": "t", "title": "Done" }),
            ),
            (
                super::super::CONFIGURE_SESSION,
                json!({ "session_id": "t" }),
            ),
        ] {
            let answer = fixture.call(&token, tool, args).await;
            assert!(answer.success, "{tool}: {}", answer.content);
        }
        assert_eq!(fixture.ran(), 4);
        // The gate validates before it runs: a merge with no thread is refused.
        let invalid = fixture
            .call(&token, super::super::MERGE_WHEN_GREEN, json!({}))
            .await;
        assert!(!invalid.success && invalid.content.contains("invalid"));
        assert_eq!(fixture.ran(), 4);
        let audit = fixture.audit();
        assert_eq!(audit.iter().filter(|row| row["outcome"] == "ok").count(), 4);
    }

    #[tokio::test]
    async fn a_call_that_asks_with_no_app_connected_fails_at_once_and_says_how_to_stop_asking() {
        let mut fixture = Fixture::new();
        fixture.settings.needs_approver = true;
        fixture.settings.approval_wait = Duration::from_secs(60);
        set_approvers(|| false);
        let (grant, token) = create_at(
            &fixture.settings.grants,
            "ChatGPT",
            "other",
            Scope::Manage,
            Some(true),
        )
        .unwrap();
        let started = Instant::now();
        let refused = fixture
            .call(
                &token,
                super::super::CLEANUP_WORKTREE,
                json!({ "session_id": "t" }),
            )
            .await;
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "it did not wait"
        );
        assert!(!refused.success);
        assert!(
            refused
                .content
                .contains(&format!("rustcode outside update {} --ask false", grant.id)),
            "{}",
            refused.content
        );
        assert_eq!(fixture.ran(), 0);
        assert_eq!(fixture.audit()[0]["outcome"], "declined");
    }

    #[tokio::test]
    async fn the_new_tools_ask_the_user_first_and_a_decline_runs_nothing() {
        let fixture = std::sync::Arc::new(Fixture::new());
        let (_, token) = create_at(
            &fixture.settings.grants,
            "Cleanup Bot",
            "other",
            Scope::Manage,
            Some(true),
        )
        .unwrap();
        let waiting = {
            let fixture = std::sync::Arc::clone(&fixture);
            tokio::spawn(async move {
                fixture
                    .call(
                        &token,
                        super::super::CLEANUP_WORKTREE,
                        json!({ "session_id": "t" }),
                    )
                    .await
            })
        };
        let id = wait_for_pending("Cleanup Bot").await;
        let shown = pending_view()
            .into_iter()
            .find(|item| item["id"] == json!(id))
            .unwrap();
        assert_eq!(shown["tool"], "cleanup_worktree");
        assert!(answer_pending(&id, false));
        let declined = waiting.await.unwrap();
        assert!(!declined.success && declined.content.contains("declined"));
        assert_eq!(fixture.ran(), 0);
        assert_eq!(fixture.audit()[0]["outcome"], "declined");
    }

    #[test]
    fn the_tool_list_shows_the_new_tools_at_their_scope() {
        let names = |scope| -> Vec<String> {
            tools_for(scope)
                .iter()
                .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
                .collect()
        };
        let delegate = names(Scope::Delegate);
        for tool in ["fan_out", "interrupt_thread"] {
            assert!(delegate.contains(&tool.to_owned()), "{tool}");
        }
        let manage = names(Scope::Manage);
        for tool in [
            "merge_when_green",
            "cleanup_worktree",
            "update_thread",
            "configure_thread",
        ] {
            assert!(!delegate.contains(&tool.to_owned()), "{tool}");
            assert!(manage.contains(&tool.to_owned()), "{tool}");
        }
        assert!(!manage.contains(&"manage_queue".to_owned()));
    }

    #[tokio::test]
    async fn http_scope_refusal_uses_the_same_gate() {
        let fixture = Fixture::new();
        let token = create_http(
            &fixture.settings.grants,
            "HTTP app",
            "app",
            Scope::ReadOnly,
            None,
        )
        .unwrap()
        .access;
        let answer = answer_transport_with(
            &fixture.settings,
            &fixture.runner,
            CallRequest {
                session_id: String::new(),
                token,
                tool: super::super::START_SESSION.into(),
                args: json!({}),
                outside: true,
            },
            &CancellationToken::new(),
            true,
        )
        .await;
        assert!(!answer.success);
        assert_eq!(fixture.ran(), 0);
    }

    #[tokio::test]
    async fn a_read_only_token_reads() {
        let fixture = Fixture::new();
        let (_, token) = fixture.grant(Scope::ReadOnly, false);
        let answer = fixture
            .call(&token, super::super::LIST_SESSIONS, json!({}))
            .await;
        assert!(answer.success, "{}", answer.content);
        assert_eq!(fixture.ran(), 1);
        assert_eq!(fixture.audit()[0]["outcome"], "ok");
    }

    #[tokio::test]
    async fn a_delegate_token_cannot_manage_and_a_tool_nobody_classified_is_refused() {
        let fixture = Fixture::new();
        let (_, token) = fixture.grant(Scope::Delegate, false);
        let merge = fixture
            .call(
                &token,
                super::super::MERGE_WHEN_GREEN,
                json!({ "session_id": "t" }),
            )
            .await;
        assert!(
            !merge.success && merge.content.contains("Manage"),
            "{}",
            merge.content
        );
        let internal = fixture.call(&token, "run_shell_command", json!({})).await;
        assert!(!internal.success);
        assert!(internal.content.contains("not available to outside agents"));
        // Even the highest scope does not reach a tool nobody classified.
        let (_, manage) = fixture.grant(Scope::Manage, false);
        assert!(
            !fixture
                .call(&manage, "run_shell_command", json!({}))
                .await
                .success
        );
        assert_eq!(fixture.ran(), 0);
    }

    #[tokio::test]
    async fn a_manage_token_is_told_what_is_not_available_yet() {
        let fixture = Fixture::new();
        let (_, token) = fixture.grant(Scope::Manage, false);
        let answer = fixture
            .call(
                &token,
                super::super::HANDOFF_WORKTREE,
                json!({ "session_id": "t" }),
            )
            .await;
        assert!(!answer.success);
        assert!(
            answer
                .content
                .contains("cannot run it for an outside agent yet")
        );
        assert_eq!(fixture.ran(), 0);
        assert_eq!(fixture.audit()[0]["outcome"], "unavailable");
    }

    #[tokio::test]
    async fn a_revoked_or_unknown_token_is_refused_on_its_next_call() {
        let fixture = Fixture::new();
        let (grant, token) = fixture.grant(Scope::ReadOnly, false);
        assert!(
            fixture
                .call(&token, super::super::LIST_SESSIONS, json!({}))
                .await
                .success
        );
        assert!(revoke_at(&fixture.settings.grants, &grant.id).unwrap());
        assert!(!revoke_at(&fixture.settings.grants, &grant.id).unwrap());
        let refused = fixture
            .call(&token, super::super::LIST_SESSIONS, json!({}))
            .await;
        assert!(!refused.success);
        assert!(refused.content.contains("revoked"), "{}", refused.content);
        assert!(
            !fixture
                .call("", super::super::LIST_SESSIONS, json!({}))
                .await
                .success
        );
        assert_eq!(fixture.ran(), 1);
    }

    #[tokio::test]
    async fn lowering_a_scope_applies_to_the_next_call() {
        let fixture = Fixture::new();
        let (grant, token) = fixture.grant(Scope::Delegate, false);
        let args = json!({ "prompt": "go", "project": "/work/app" });
        assert!(
            fixture
                .call(&token, super::super::START_SESSION, args.clone())
                .await
                .success
        );
        update_at(
            &fixture.settings.grants,
            &grant.id,
            Some(Scope::ReadOnly),
            None,
        )
        .unwrap();
        let refused = fixture
            .call(&token, super::super::START_SESSION, args)
            .await;
        assert!(!refused.success && refused.content.contains("Delegate"));
        assert_eq!(fixture.ran(), 1);
    }

    #[tokio::test]
    async fn a_change_waits_for_the_user_and_runs_only_when_allowed() {
        let fixture = std::sync::Arc::new(Fixture::new());
        let (_, token) = fixture.grant(Scope::Delegate, true);
        let args = json!({ "prompt": "fix the build", "project": "/work/app" });

        // Allowed in the app.
        let waiting = {
            let fixture = std::sync::Arc::clone(&fixture);
            let token = token.clone();
            let args = args.clone();
            tokio::spawn(async move {
                fixture
                    .call(&token, super::super::START_SESSION, args)
                    .await
            })
        };
        let id = wait_for_pending("Cursor").await;
        let shown = pending_view()
            .into_iter()
            .find(|item| item["id"] == json!(id))
            .unwrap();
        assert_eq!(shown["tool"], "start_thread");
        assert!(shown["summary"].as_str().unwrap().contains("fix the build"));
        assert_eq!(fixture.ran(), 0, "nothing runs while the user decides");
        assert!(answer_pending(&id, true));
        assert!(waiting.await.unwrap().success);
        assert_eq!(fixture.ran(), 1);

        // Declined in the app.
        let waiting = {
            let fixture = std::sync::Arc::clone(&fixture);
            let token = token.clone();
            let args = args.clone();
            tokio::spawn(async move {
                fixture
                    .call(&token, super::super::START_SESSION, args)
                    .await
            })
        };
        let id = wait_for_pending("Cursor").await;
        assert!(answer_pending(&id, false));
        let declined = waiting.await.unwrap();
        assert!(!declined.success && declined.content.contains("declined"));
        assert_eq!(fixture.ran(), 1);

        // Not answered in time.
        let silent = fixture
            .call(&token, super::super::START_SESSION, args)
            .await;
        assert!(!silent.success && silent.content.contains("did not answer"));
        assert_eq!(fixture.ran(), 1);
        assert!(
            pending_view()
                .iter()
                .all(|item| item["clientName"] != "Cursor")
        );
        let outcomes: Vec<String> = fixture
            .audit()
            .iter()
            .map(|row| row["outcome"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(outcomes, ["declined", "declined", "ok"]);
    }

    async fn wait_for_pending(client: &str) -> String {
        for _ in 0..100 {
            if let Some(item) = pending_view()
                .into_iter()
                .find(|item| item["clientName"] == json!(client))
            {
                return item["id"].as_str().unwrap().to_owned();
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("no pending approval appeared");
    }

    #[tokio::test]
    async fn revoking_while_the_user_is_asked_stops_the_call() {
        let fixture = std::sync::Arc::new(Fixture::new());
        let (grant, token) = create_at(
            &fixture.settings.grants,
            "Revoked mid-ask",
            "other",
            Scope::Delegate,
            Some(true),
        )
        .unwrap();
        let waiting = {
            let fixture = std::sync::Arc::clone(&fixture);
            tokio::spawn(async move {
                fixture
                    .call(
                        &token,
                        super::super::MESSAGE_SESSION,
                        json!({ "session_id": "t", "message": "hi" }),
                    )
                    .await
            })
        };
        wait_for_pending("Revoked mid-ask").await;
        revoke_at(&fixture.settings.grants, &grant.id).unwrap();
        deny_all_for(&grant.id);
        assert!(!waiting.await.unwrap().success);
        assert_eq!(fixture.ran(), 0);
    }

    #[tokio::test]
    async fn concurrent_calls_each_get_their_own_answer_and_audit_row() {
        let fixture = std::sync::Arc::new(Fixture::new());
        let (_, token) = fixture.grant(Scope::ReadOnly, false);
        let mut calls = Vec::new();
        for _ in 0..8 {
            let fixture = std::sync::Arc::clone(&fixture);
            let token = token.clone();
            calls.push(tokio::spawn(async move {
                fixture
                    .call(&token, super::super::LIST_SESSIONS, json!({}))
                    .await
            }));
        }
        for call in calls {
            assert!(call.await.unwrap().success);
        }
        assert_eq!(fixture.ran(), 8);
        assert_eq!(fixture.audit().len(), 8);
    }

    #[tokio::test]
    async fn whoami_says_what_the_connection_is_and_what_it_lacks() {
        let fixture = Fixture::new();
        let (_, token) = fixture.grant(Scope::ReadOnly, false);
        let answer = fixture.call(&token, WHOAMI, json!({})).await;
        assert!(answer.success, "{}", answer.content);
        let value: Value = serde_json::from_str(&answer.content).unwrap();
        assert_eq!(value["client"]["name"], "Cursor");
        assert_eq!(value["access"]["level"], "read_only");
        let can: Vec<&str> = value["access"]["can"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert!(can.contains(&"list_threads") && can.contains(&"whoami"));
        assert!(!can.contains(&"start_thread"));
        let lacks: Vec<&str> = value["access"]["needsMoreAccess"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|row| row["tool"].as_str())
            .collect();
        assert!(lacks.contains(&"start_thread"));
        assert_eq!(value["projects"][0], "/work/app");
        assert!(value.get("tokenHash").is_none() && !answer.content.contains(&token));
        assert_eq!(fixture.ran(), 0, "whoami is answered by the gate");
    }

    #[test]
    fn the_tool_list_and_instructions_follow_the_scope() {
        let names = |scope| -> Vec<String> {
            tools_for(scope)
                .iter()
                .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
                .collect()
        };
        let read = names(Scope::ReadOnly);
        assert!(read.contains(&"whoami".to_owned()) && read.contains(&"list_threads".to_owned()));
        assert!(!read.contains(&"start_thread".to_owned()));
        assert!(!read.contains(&"merge_when_green".to_owned()));
        let delegate = names(Scope::Delegate);
        assert!(delegate.contains(&"start_thread".to_owned()));
        assert!(!delegate.contains(&"merge_when_green".to_owned()));
        let start = tools_for(Scope::Delegate)
            .into_iter()
            .find(|tool| tool["name"] == "start_thread")
            .unwrap();
        assert!(
            start["description"]
                .as_str()
                .unwrap()
                .contains("needs Delegate access")
        );
        let grant = Grant {
            expires_at: None,
            id: "id".into(),
            name: "Codex".into(),
            platform: "codex".into(),
            scope: Scope::Delegate,
            ask: true,
            token_hash: String::new(),
            created_at: String::new(),
            last_used_at: None,
            last_tool: None,
            refresh_hash: None,
            refresh_until: None,
            oauth_client: None,
        };
        let text = instructions(&grant);
        assert!(text.contains("Codex") && text.contains("Delegate") && text.contains("whoami"));
        assert!(text.contains("put to the user"));
    }

    #[test]
    fn the_snippet_carries_the_token_in_the_format_the_platform_reads() {
        let json = snippet("cursor", "/Applications/RustCode/rustcode", "rcx_ab_cd");
        assert_eq!(json["format"], "json");
        let parsed: Value = serde_json::from_str(json["text"].as_str().unwrap()).unwrap();
        let server = &parsed["mcpServers"]["rustcode"];
        assert_eq!(server["command"], "/Applications/RustCode/rustcode");
        assert_eq!(server["args"][0], "mcp-connect");
        assert_eq!(server["env"][TOKEN_ENV], "rcx_ab_cd");
        let toml = snippet("codex", "/bin/rustcode", "rcx_ab_cd");
        assert_eq!(toml["format"], "toml");
        let text = toml["text"].as_str().unwrap();
        assert!(text.contains("[mcp_servers.rustcode]") && text.contains("mcp-connect"));
        assert!(text.contains(&format!("{TOKEN_ENV} = \"rcx_ab_cd\"")));
        assert!(
            snippet("claude-desktop", "rustcode", "t")["where"]
                .as_str()
                .unwrap()
                .contains("claude_desktop_config.json")
        );
    }

    #[test]
    fn an_approval_shows_the_decision_not_the_whole_call() {
        let summary = summarize(
            "start_session",
            &json!({
                "prompt": "x".repeat(400),
                "project": "/work/app",
                "attachments": ["/secret/key"],
                "token": "never shown",
            }),
        );
        assert!(summary.contains("/work/app"));
        assert!(summary.chars().count() < 260, "{summary}");
        assert!(!summary.contains("secret") && !summary.contains("never shown"));
        assert_eq!(summarize("list_sessions", &json!({})), "list_sessions");
    }
}

/// How long an OAuth access token works before the app refreshes it.
pub(super) const HTTP_ACCESS_SECS: i64 = 3600;
/// How long an OAuth sign-in lasts before the app must be approved again.
pub(super) const HTTP_SIGN_IN_SECS: i64 = 30 * 24 * 3600;
const REFRESH_PREFIX: &str = "rcr_";

/// A fresh OAuth access token and refresh token for one grant.
pub(super) struct HttpTokens {
    pub grant: Grant,
    pub access: String,
    pub refresh: String,
}

fn random_secret() -> String {
    format!(
        "{:032x}{:032x}",
        rand::random::<u128>(),
        rand::random::<u128>()
    )
}

/// Give `grant` a new access token and refresh token; the old ones stop working.
fn rotate(grant: &mut Grant, now: i64) -> (String, String) {
    let access = format!("rch_{}_{}", grant.id, random_secret());
    let refresh = format!("{REFRESH_PREFIX}{}_{}", grant.id, random_secret());
    let until = grant.refresh_until.unwrap_or(now + HTTP_SIGN_IN_SECS);
    grant.token_hash = hash_token(&access);
    grant.expires_at = Some((now + HTTP_ACCESS_SECS).min(until));
    grant.refresh_hash = Some(hash_token(&refresh));
    grant.refresh_until = Some(until);
    (access, refresh)
}

/// OAuth credentials share the revocable grant store, but cannot authenticate stdio.
pub(super) fn create_http(
    path: &Path,
    name: &str,
    client_id: &str,
    scope: Scope,
    ask: Option<bool>,
) -> Result<HttpTokens, String> {
    let (mut grant, _) = create_at(path, name, "other", scope, ask)?;
    let now = chrono::Utc::now().timestamp();
    grant.oauth_client = Some(hash_token(client_id));
    let (access, refresh) = rotate(&mut grant, now);
    let _guard = lock();
    let mut clients = read(path);
    let Some(saved) = clients.iter_mut().find(|g| g.id == grant.id) else {
        return Err("grant revoked".into());
    };
    *saved = grant.clone();
    write(path, &clients)?;
    Ok(HttpTokens {
        grant,
        access,
        refresh,
    })
}

/// Trade a refresh token for new tokens on the same grant, keeping its access
/// and the end of its sign-in. A used refresh token never works again.
pub(super) fn refresh_http(path: &Path, refresh: &str, client_id: &str) -> Option<HttpTokens> {
    let rest = refresh.strip_prefix(REFRESH_PREFIX)?;
    let (id, _) = rest.split_once('_')?;
    let hash = hash_token(refresh.trim());
    let client = hash_token(client_id);
    let now = chrono::Utc::now().timestamp();
    let _guard = lock();
    let mut clients = read(path);
    let grant = clients.iter_mut().find(|grant| {
        grant.id == id
            && grant
                .refresh_hash
                .as_deref()
                .is_some_and(|saved| same(saved, &hash))
            && grant
                .oauth_client
                .as_deref()
                .is_some_and(|saved| same(saved, &client))
            && grant.refresh_until.is_some_and(|until| until > now)
    })?;
    let (access, refresh) = rotate(grant, now);
    let grant = grant.clone();
    write(path, &clients).ok()?;
    Some(HttpTokens {
        grant,
        access,
        refresh,
    })
}

// --------------------------------------------------- one-time sign-in codes

const CODE_FILE: &str = "outside-codes.json";
/// How long a sign-in code from `rustcode outside code` can be used.
pub const SIGN_IN_CODE_SECS: i64 = 600;
const MAX_OPEN_CODES: usize = 10;
/// No 0/O or 1/I, so a code read off a screen is typed right.
const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

/// One unused sign-in code. Only its hash is kept.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SignInCode {
    hash: String,
    scope: Scope,
    #[serde(default)]
    ask: Option<bool>,
    expires_at: i64,
}

#[derive(Default, Serialize, Deserialize)]
struct CodeStore {
    #[serde(default)]
    codes: Vec<SignInCode>,
}

/// Where the open sign-in codes are kept.
pub fn sign_in_code_path() -> Option<PathBuf> {
    crate::config::get_config_dir().map(|dir| dir.join(CODE_FILE))
}

fn normalize_code(code: &str) -> String {
    code.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

fn read_codes(path: &Path, now: i64) -> Vec<SignInCode> {
    let mut codes = std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<CodeStore>(&bytes).ok())
        .map(|store| store.codes)
        .unwrap_or_default();
    codes.retain(|code| code.expires_at > now);
    codes
}

fn write_codes(path: &Path, codes: &[SignInCode]) -> Result<(), String> {
    let bytes =
        serde_json::to_vec_pretty(&json!({ "codes": codes })).map_err(|error| error.to_string())?;
    crate::service::write_private(path, &bytes).map_err(|error| error.to_string())
}

/// A one-time code that approves one OAuth sign-in on this machine's sign-in
/// page, at most at `scope`. Shown as `XXXX-XXXX`; it works once, for
/// [`SIGN_IN_CODE_SECS`].
pub fn create_sign_in_code_at(
    path: &Path,
    scope: Scope,
    ask: Option<bool>,
) -> Result<String, String> {
    let now = chrono::Utc::now().timestamp();
    let _guard = lock();
    let mut codes = read_codes(path, now);
    if codes.len() >= MAX_OPEN_CODES {
        // The oldest open code makes room; it was never used.
        codes.sort_by_key(|code| code.expires_at);
        codes.remove(0);
    }
    let raw: String = (0..8)
        .map(|_| CODE_ALPHABET[rand::random::<u8>() as usize % CODE_ALPHABET.len()] as char)
        .collect();
    codes.push(SignInCode {
        hash: hash_token(&raw),
        scope,
        ask,
        expires_at: now + SIGN_IN_CODE_SECS,
    });
    write_codes(path, &codes)?;
    Ok(format!("{}-{}", &raw[..4], &raw[4..]))
}

/// Spend a sign-in code: the access it allows and whether changes ask first,
/// or `None` when it is unknown, used or expired.
pub(super) fn redeem_sign_in_code_at(path: &Path, code: &str) -> Option<(Scope, Option<bool>)> {
    let hash = hash_token(&normalize_code(code));
    let now = chrono::Utc::now().timestamp();
    let _guard = lock();
    let mut codes = read_codes(path, now);
    let index = codes.iter().position(|saved| same(&saved.hash, &hash))?;
    let spent = codes.remove(index);
    write_codes(path, &codes).ok()?;
    Some((spent.scope, spent.ask))
}

pub(super) async fn approve_connection(name: &str, destination: &str, scope: Scope) -> bool {
    let id = format!("{:032x}", rand::random::<u128>());
    let (sender, receiver) = oneshot::channel();
    pending().push(Pending {
        id: id.clone(),
        client_id: id.clone(),
        client_name: name.chars().take(60).collect(),
        tool: "Connect over localhost HTTP".into(),
        summary: format!(
            "Destination: {destination}; requested access: {}",
            scope.label()
        ),
        asked_at: now(),
        answer: Some(sender),
    });
    changed();
    let allowed = matches!(
        tokio::time::timeout(APPROVAL_WAIT, receiver).await,
        Ok(Ok(true))
    );
    withdraw(&id);
    allowed
}
