mod agents;
mod capabilities;
mod commands;
mod config;
mod connections;
mod edit_diff;
mod permissions;
mod presence;
mod preview;
mod prompt;
mod questions;
mod remote;
mod rewind;
mod session;
mod session_move;
mod steer;
mod streaming;
pub(crate) mod usage;

pub(crate) use agent_client_protocol::schema::v1::{
    AgentCapabilities, CancelNotification, CloseSessionRequest, CloseSessionResponse, ContentBlock,
    InitializeRequest, InitializeResponse, ListSessionsRequest, ListSessionsResponse,
    LoadSessionRequest, LoadSessionResponse, NewSessionRequest, NewSessionResponse,
    PromptCapabilities, PromptRequest, PromptResponse, SessionCapabilities,
    SessionCloseCapabilities, SessionInfo, SessionInfoUpdate, SessionListCapabilities,
    SessionNotification, SessionUpdate, SetSessionConfigOptionRequest,
    SetSessionConfigOptionResponse,
};
#[cfg(test)]
use agent_client_protocol::schema::v1::{
    SessionConfigId, SessionConfigKind, SessionConfigOptionCategory, SessionConfigOptionValue,
    SessionConfigSelectOptions, StopReason,
};
use agent_client_protocol::{Agent, Client, Stdio};
use rustcode_tasks::TaskEvent;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use tokio::sync::Mutex;

pub(crate) use config::{build_session_config_options, handle_set_config_option};
#[cfg(test)]
pub(crate) use permissions::{ApprovalRequirement, approval_requirement};
pub(crate) use prompt::prompt_input;
pub(crate) use session::{AcpSession, KnownTaskIds, SessionTurnState, Sessions, new_registry};
#[cfg(test)]
pub(crate) use streaming::{AcpEventStream, acp_stop_reason};

const ACP_SESSION_PAGE_SIZE: usize = 50;

fn valid_acp_session_id(session_id: &str) -> bool {
    rustcode_session::valid_session_id(session_id)
}

/// Highest level of the `_meta.rustcode.visualizations` capability the
/// engine knows (#2088, #2217). A client's initialize
/// `clientCapabilities._meta.rustcode.visualizations` says which
/// `rustcode-viz` widgets it renders: 1 is bar chart, table and tree; 2 adds
/// line chart, stat list and checklist. Its sessions are taught those types.
const VISUALIZATIONS_CAPABILITY_VERSION: u64 = 2;

/// The `rustcode-viz` level an ACP client advertised, capped at the highest
/// the engine knows. Absent, malformed, or zero means 0: the client would
/// only see JSON.
fn client_visualization_level(
    capabilities: &agent_client_protocol::schema::v1::ClientCapabilities,
) -> u8 {
    capabilities
        .meta
        .as_ref()
        .and_then(|meta| meta.get("rustcode"))
        .and_then(|rustcode| rustcode.get("visualizations"))
        .and_then(serde_json::Value::as_u64)
        .map_or(0, |level| {
            level.min(VISUALIZATIONS_CAPABILITY_VERSION) as u8
        })
}

/// Version of the `_meta.rustcode.smartUi` capability (#2484). A client
/// advertises it while the user has Smart UI turned on, and its sessions are
/// then taught `rustcode-ui v1` blocks. `0` means the client can run them but
/// the setting is off: the agent then points the user at the setting instead
/// of inventing a format.
const SMART_UI_CAPABILITY_VERSION: u64 = 1;

fn client_smart_ui(
    capabilities: &agent_client_protocol::schema::v1::ClientCapabilities,
) -> crate::app::SmartUi {
    let version = capabilities
        .meta
        .as_ref()
        .and_then(|meta| meta.get("rustcode"))
        .and_then(|rustcode| rustcode.get("smartUi"))
        .and_then(serde_json::Value::as_u64);
    match version {
        Some(version) if version >= SMART_UI_CAPABILITY_VERSION => crate::app::SmartUi::On,
        Some(0) => crate::app::SmartUi::Off,
        _ => crate::app::SmartUi::Unsupported,
    }
}

fn server_capabilities() -> AgentCapabilities {
    AgentCapabilities::new()
        .load_session(true)
        .prompt_capabilities(PromptCapabilities::new().image(true))
        .session_capabilities(
            SessionCapabilities::new()
                .list(SessionListCapabilities::new())
                .close(SessionCloseCapabilities::new()),
        )
        .meta(serde_json::Map::from_iter([(
            "rustcode".to_owned(),
            serde_json::json!({
                "visualizations": VISUALIZATIONS_CAPABILITY_VERSION,
                "smartUi": SMART_UI_CAPABILITY_VERSION,
                "preview": crate::preview::PREVIEW_CAPABILITY_VERSION,
                "remote": remote::REMOTE_CAPABILITY_VERSION,
                "steer": steer::STEER_CAPABILITY_VERSION,
                "agents": agents::AGENTS_CAPABILITY_VERSION,
                "rewind": 1,
                "sessionMove": 1,
                "pullRequests": 1,
            }),
        )]))
}

fn session_id_is_active(sessions: &HashMap<String, AcpSession>, session_id: &str) -> bool {
    sessions.contains_key(session_id)
}

fn workspace_cwd(
    workspace: Option<rustcode_session::SessionWorkspace>,
) -> Option<std::path::PathBuf> {
    let cwd = workspace?.cwd;
    let canonical = std::fs::canonicalize(cwd).ok()?;
    canonical.is_dir().then_some(canonical)
}

fn parse_session_cursor(
    cursor: Option<&str>,
) -> Result<Option<&str>, agent_client_protocol::Error> {
    let Some(cursor) = cursor else {
        return Ok(None);
    };
    if cursor.len() > 131 {
        return Err(agent_client_protocol::Error::invalid_params().data("invalid session cursor"));
    }
    let Some(session_id) = cursor.strip_prefix("v1:") else {
        return Err(agent_client_protocol::Error::invalid_params().data("invalid session cursor"));
    };
    if !valid_acp_session_id(session_id) {
        return Err(agent_client_protocol::Error::invalid_params().data("invalid session cursor"));
    }
    Ok(Some(session_id))
}

fn session_info(
    meta: &crate::config::SessionMeta,
    workspace: Option<rustcode_session::SessionWorkspace>,
) -> SessionInfo {
    let session_id = crate::config::session_id_from_path(&meta.path).unwrap_or_default();
    let (cwd, workspace_unknown) = workspace_cwd(workspace)
        .map(|cwd| (cwd, false))
        .unwrap_or_else(|| (std::env::current_dir().unwrap_or_default(), true));
    let mut info = SessionInfo::new(session_id.clone(), cwd).title(meta.title.clone());
    let mut extra = serde_json::Map::new();
    extra.insert("messageCount".into(), serde_json::json!(meta.message_count));
    extra.insert(
        "workspaceUnknown".into(),
        serde_json::json!(workspace_unknown),
    );
    if let Some(created_at_ms) =
        crate::config::load_session_metadata(&session_id).map(|metadata| metadata.created_at_ms)
        && let Some(timestamp) = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(
            created_at_ms.min(i64::MAX as u64) as i64,
        )
    {
        extra.insert(
            "createdAt".into(),
            serde_json::json!(timestamp.to_rfc3339()),
        );
    }
    // The profile recorded for the chat, else the one its settings log last
    // saw: chats that never changed model only have the latter.
    if let Some(model) = crate::config::load_session_model(&session_id)
        .map(|model| model.profile)
        .or_else(|| {
            crate::config::load_session_settings(&session_id)
                .map(|settings| settings.active_profile)
                .filter(|profile| !profile.is_empty())
        })
    {
        extra.insert("model".into(), serde_json::json!(model));
    }
    // The last message's time, not the file's: opening or re-saving a chat
    // rewrites its history, which made every opened chat read "now".
    if let Some(timestamp) = session_updated_at(meta) {
        info = info.updated_at(timestamp.to_rfc3339());
    }
    info.meta = Some(extra);
    info
}

/// When the chat last changed: its last message, else the history file's
/// modification time for transcripts written without timestamps.
fn session_updated_at(meta: &crate::config::SessionMeta) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(&meta.when)
        .map(|timestamp| timestamp.with_timezone(&chrono::Utc))
        .ok()
        .or_else(|| {
            std::fs::metadata(&meta.path)
                .and_then(|metadata| metadata.modified())
                .ok()
                .map(Into::into)
        })
}

fn session_file_is_contained(path: &std::path::Path) -> bool {
    crate::config::get_config_dir()
        .is_some_and(|config| crate::config::session_file_in_store(&config, path))
}

fn acp_list_sessions(
    request: ListSessionsRequest,
) -> Result<ListSessionsResponse, agent_client_protocol::Error> {
    let filter_cwd = request
        .cwd
        .map(|cwd| {
            if !cwd.is_absolute() {
                return Err(agent_client_protocol::Error::invalid_params()
                    .data("session/list cwd must be absolute"));
            }
            let cwd = std::fs::canonicalize(&cwd).map_err(|error| {
                agent_client_protocol::Error::invalid_params()
                    .data(format!("invalid session/list cwd: {error}"))
            })?;
            if !cwd.is_dir() {
                return Err(agent_client_protocol::Error::invalid_params()
                    .data("session/list cwd must be a directory"));
            }
            Ok(cwd)
        })
        .transpose()?;
    let cursor = parse_session_cursor(request.cursor.as_deref())?;
    // `session/list` is a per-project API: the client's `cwd` selects one
    // project, so it takes the store's exact-match scope rather than the
    // parent/child scope the picker uses (#1533). Legacy sessions with no
    // recorded workspace and sessions whose workspace has since been deleted
    // stay unlisted, as they were before.
    let scope = match &filter_cwd {
        Some(filter) => crate::config::SessionScope::WorkspaceExact(filter.clone()),
        None => crate::config::SessionScope::All,
    };
    let (metas, next) =
        crate::config::list_sessions_page(cursor, ACP_SESSION_PAGE_SIZE, &scope, |meta| {
            // Path containment and id shape are not workspace concerns, so they
            // stay a separate narrowing step on the store's own listing.
            if !session_file_is_contained(&meta.path) {
                return false;
            }
            let id = crate::config::session_id_from_path(&meta.path);
            id.as_deref().is_some_and(valid_acp_session_id)
        })
        .map_err(|_| {
            agent_client_protocol::Error::invalid_params().data("unknown or stale session cursor")
        })?;
    let sessions = metas
        .iter()
        .map(|meta| {
            // Already carried by the row the store returned; re-reading the
            // record here would cost one file per legacy session (#1533).
            let workspace =
                meta.workspace_cwd
                    .clone()
                    .map(|cwd| rustcode_session::SessionWorkspace {
                        cwd,
                        additional_directories: Vec::new(),
                        task_workspace_id: None,
                    });
            session_info(meta, workspace)
        })
        .collect();
    Ok(ListSessionsResponse::new(sessions).next_cursor(next.map(|id| format!("v1:{id}"))))
}

fn replay_history_updates(
    history: &[crate::app::ChatMessage],
    cwd: Option<&std::path::Path>,
) -> Vec<SessionUpdate> {
    replay_history_updates_with_compaction(history, cwd, None)
}

/// The replay of `history`, with the compaction card where a compaction
/// left its summary. `compaction` is the session's recorded last
/// compaction; it describes the latest summary only.
fn replay_history_updates_with_compaction(
    history: &[crate::app::ChatMessage],
    cwd: Option<&std::path::Path>,
    compaction: Option<&crate::app::CompactionStats>,
) -> Vec<SessionUpdate> {
    use agent_client_protocol::schema::v1::{
        ContentBlock, ContentChunk, TextContent, ToolCall, ToolCallContent, ToolCallStatus,
        ToolCallUpdate, ToolCallUpdateFields,
    };
    let latest_boundary = history
        .iter()
        .rposition(|message| message.compaction_boundary.is_some());
    let mut updates = Vec::new();
    for (index, message) in history.iter().enumerate() {
        match message.role.as_str() {
            "system" if message.compaction_boundary.is_some() => {
                let summary = message
                    .compaction_boundary
                    .as_ref()
                    .map(|boundary| boundary.summary.as_str())
                    .filter(|summary| !summary.trim().is_empty());
                let stats = compaction.filter(|_| Some(index) == latest_boundary);
                updates.push(usage::compaction_card(stats, summary, None));
            }
            // A stop the user made is stored as a system marker; it
            // replays as the empty reply that carries the turn's "Stopped
            // after" timing, as the assistant-role marker does.
            "system" if crate::network::lifecycle::is_turn_cancelled_marker(&message.content) => {
                let mut replayed = vec![SessionUpdate::AgentMessageChunk(ContentChunk::new(
                    ContentBlock::Text(TextContent::new(String::new())),
                ))];
                if let Some(turn) = &message.turn {
                    streaming::attach_turn_timing(&mut replayed, turn);
                }
                updates.extend(replayed);
            }
            "user" if !message.content.is_empty() => {
                let mut chunk = ContentChunk::new(ContentBlock::Text(TextContent::new(
                    message.content.clone(),
                )));
                if let Some(meta) = streaming::message_time_meta(&message.timestamp) {
                    chunk = chunk.meta(meta);
                }
                updates.push(SessionUpdate::UserMessageChunk(chunk))
            }
            "assistant" => {
                // Reasoning goes out as thought chunks, as it did live, so
                // the client labels a replayed thought as it did then.
                // A stop marker is turn bookkeeping, not a reply: a cancel
                // replays as an empty reply that carries the turn's "Stopped
                // after" timing, any other stop as its short notice.
                let mut replayed =
                    match crate::network::lifecycle::stop_marker_notice(&message.content) {
                        Some(notice) => vec![SessionUpdate::AgentMessageChunk(ContentChunk::new(
                            ContentBlock::Text(TextContent::new(
                                if crate::network::lifecycle::is_turn_cancelled_marker(
                                    &message.content,
                                ) {
                                    String::new()
                                } else {
                                    notice
                                },
                            )),
                        ))],
                        None => streaming::AcpEventStream::replay_assistant_text(
                            &message.content,
                            message.reasoning_summary,
                            message.thought_time_ms.filter(|ms| *ms > 0),
                        ),
                    };
                if let Some(turn) = &message.turn {
                    streaming::attach_turn_timing(&mut replayed, turn);
                }
                streaming::attach_message_time(&mut replayed, &message.timestamp);
                updates.extend(replayed);
                for call in &message.tool_calls {
                    let raw_input = serde_json::from_str(&call.arguments)
                        .unwrap_or_else(|_| serde_json::json!({"raw": call.arguments}));
                    updates.push(SessionUpdate::ToolCall(
                        ToolCall::new(call.id.clone(), call.name.clone())
                            .status(ToolCallStatus::Pending)
                            .raw_input(raw_input),
                    ));
                }
            }
            "tool" | "function" => {
                if let Some(call_id) = message.tool_call_id.as_deref() {
                    let success = message
                        .tool_result
                        .as_ref()
                        .is_some_and(|result| result.success);
                    let mut content = vec![ToolCallContent::from(ContentBlock::Text(
                        TextContent::new(message.content.clone()),
                    ))];
                    if let Some(result) = message.tool_result.as_ref() {
                        content.extend(edit_diff::edit_diff_content(
                            &result.tool_name,
                            success && !result.pending,
                            &message.content,
                            None,
                            &result.changed_paths,
                            cwd,
                        ));
                    }
                    updates.push(SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                        call_id.to_owned(),
                        ToolCallUpdateFields::new()
                            .status(if success {
                                ToolCallStatus::Completed
                            } else {
                                ToolCallStatus::Failed
                            })
                            .content(content)
                            .raw_output(
                                serde_json::json!({"content": message.content, "success": success}),
                            ),
                    )));
                } else if !message.content.is_empty() {
                    updates.push(SessionUpdate::AgentMessageChunk(ContentChunk::new(
                        ContentBlock::Text(TextContent::new(message.content.clone())),
                    )));
                }
            }
            _ => {}
        }
    }
    updates
}

pub(crate) fn build_loaded_app_state(
    session_id: &str,
    cwd: &std::path::Path,
    history: Vec<crate::app::ChatMessage>,
) -> crate::app::AppState {
    let mut state = crate::app::AppState::new_with_workspace_session(cwd, Some(session_id));
    state.restore_session_model();
    state.restore_session_compaction();
    state.task_working_directory = Some(cwd.to_path_buf());
    state.workspace_root = Some(cwd.to_path_buf());
    // Reattach the session's recorded task worktree so resume reuses its
    // writable roots and cwd instead of starting over in the source checkout
    // (#1496). A missing or foreign worktree falls back to `cwd`.
    if let Some(workspace) = crate::config::load_session_workspace(session_id)
        && let Some(descriptor_id) = workspace.task_workspace_id
        && let Some(manager) = crate::config::workspace_manager()
        && let Ok(descriptor) = manager.resume(&descriptor_id)
    {
        state.workspace_root = Some(descriptor.workspace_path.clone());
        state.task_working_directory = Some(descriptor.workspace_path.clone());
    }
    state.history.replace(history);
    state
}

/// State for `session/new`. The id is always fresh: reusing the shared
/// last-active session, as the TUI does when it is still empty, would hand two
/// ACP sessions (or two processes) the same history file.
fn new_session_state(cwd: &std::path::Path) -> crate::app::AppState {
    let session_id = crate::config::create_detached_session();
    let state = crate::app::AppState::new_with_workspace_session(cwd, Some(&session_id));
    if !session_id.is_empty() {
        crate::config::record_session_settings(&session_id, &state.config);
        state.record_session_model();
    }
    state
}

fn canonical_acp_cwd(
    cwd: &std::path::Path,
    operation: &str,
) -> Result<std::path::PathBuf, agent_client_protocol::Error> {
    if !cwd.is_absolute() {
        return Err(agent_client_protocol::Error::invalid_params()
            .data(format!("{operation} cwd must be absolute")));
    }
    let canonical = std::fs::canonicalize(cwd).map_err(|error| {
        agent_client_protocol::Error::invalid_params()
            .data(format!("invalid {operation} cwd: {error}"))
    })?;
    if !canonical.is_dir() {
        return Err(agent_client_protocol::Error::invalid_params()
            .data(format!("{operation} cwd must be a directory")));
    }
    Ok(canonical)
}

fn session_workspace(
    cwd: std::path::PathBuf,
    additional_directories: Vec<std::path::PathBuf>,
) -> rustcode_session::SessionWorkspace {
    rustcode_session::SessionWorkspace {
        cwd,
        additional_directories,
        task_workspace_id: None,
    }
}

async fn acp_load_session(
    server: &AcpServer,
    request: LoadSessionRequest,
    connection: &agent_client_protocol::ConnectionTo<Client>,
) -> Result<LoadSessionResponse, agent_client_protocol::Error> {
    let session_id = request.session_id.to_string();
    if !valid_acp_session_id(&session_id) {
        return Err(agent_client_protocol::Error::invalid_params().data("invalid session id"));
    }
    let cwd = canonical_acp_cwd(&request.cwd, "session/load")?;
    let (meta, history) = crate::config::load_session_by_id(&session_id).ok_or_else(|| {
        agent_client_protocol::Error::invalid_params()
            .data(format!("unknown or unresumable ACP session: {session_id}"))
    })?;
    let canonical_history = std::fs::canonicalize(&meta.path).map_err(|error| {
        agent_client_protocol::Error::invalid_params()
            .data(format!("session history is unavailable: {error}"))
    })?;
    let sessions_root = crate::config::get_config_dir()
        .map(|root| rustcode_session::threads_dir(&root))
        .and_then(|path| std::fs::canonicalize(path).ok())
        .ok_or_else(|| {
            agent_client_protocol::Error::invalid_params().data("session storage is unavailable")
        })?;
    if !canonical_history.starts_with(&sessions_root) {
        return Err(agent_client_protocol::Error::invalid_params()
            .data("session history escaped the session store"));
    }
    if let Some(workspace) = crate::config::load_session_workspace(&session_id)
        && std::fs::canonicalize(workspace.cwd).ok().as_ref() != Some(&cwd)
    {
        return Err(agent_client_protocol::Error::invalid_params()
            .data("session/load cwd does not match the recorded workspace"));
    }
    {
        let mut sessions = server.sessions.lock().await;
        if session_id_is_active(&sessions, &session_id) {
            return Err(
                agent_client_protocol::Error::invalid_params().data("session is already active")
            );
        }
        let mut state = build_loaded_app_state(&session_id, &cwd, history.clone());
        state.visualization_level = server.visualization_level.load(Ordering::Relaxed);
        state.smart_ui =
            crate::app::SmartUi::from_u8(server.client_smart_ui.load(Ordering::Relaxed));
        state.preview_client = preview::preview_client_for(
            server.client_has_preview.load(Ordering::Relaxed),
            connection,
            &session_id,
        );
        let config_options = build_session_config_options(&state);
        let loaded_usage = usage::usage_update(&state);
        let loaded_compaction = state.last_compaction.clone();
        let state = Arc::new(Mutex::new(state));
        let (task_sender, task_receiver) = std::sync::mpsc::sync_channel(64);
        let known_task_ids = Arc::new(std::sync::Mutex::new(KnownTaskIds::default()));
        let terminal_backlog = Arc::new(std::sync::Mutex::new(VecDeque::new()));
        let terminal_overflow = Arc::new(AtomicBool::new(false));
        let task_sink = Arc::new(SessionTaskSink {
            session_id: session_id.clone(),
            state: Arc::clone(&state),
            connection: connection.clone(),
            runtime: tokio::runtime::Handle::current(),
            terminal_ledger: std::sync::Mutex::new(TerminalLedger::default()),
            terminal_backlog: Arc::clone(&terminal_backlog),
            terminal_overflow: Arc::clone(&terminal_overflow),
        });
        server
            .task_routes
            .lock()
            .expect("ACP task routes mutex poisoned")
            .insert(
                session_id.clone(),
                TaskRoute {
                    sender: task_sender,
                    known_task_ids: Arc::clone(&known_task_ids),
                    sink: Some(task_sink),
                },
            );
        mark_acp_session(&session_id);
        sessions.insert(
            session_id.clone(),
            AcpSession {
                state,
                cwd: cwd.clone(),
                turns: Arc::new(SessionTurnState::new()),
                task_events: Arc::new(std::sync::Mutex::new(task_receiver)),
                known_task_ids,
                terminal_backlog,
                terminal_overflow,
            },
        );
        drop(sessions);
        let replay = replay_history_updates_with_compaction(
            &history,
            Some(&cwd),
            loaded_compaction.as_ref(),
        );
        let _ = crate::config::save_session_workspace(
            &session_id,
            &session_workspace(cwd, request.additional_directories.clone()),
        );
        for update in replay {
            if let Err(error) =
                connection.send_notification(SessionNotification::new(session_id.clone(), update))
            {
                unregister_acp_session(server, &session_id).await;
                return Err(agent_client_protocol::Error::internal_error().data(error.to_string()));
            }
        }
        if !meta.title.trim().is_empty() {
            if let Err(error) = connection.send_notification(SessionNotification::new(
                session_id.clone(),
                SessionUpdate::SessionInfoUpdate(
                    SessionInfoUpdate::new().title(meta.title.clone()),
                ),
            )) {
                unregister_acp_session(server, &session_id).await;
                return Err(agent_client_protocol::Error::internal_error().data(error.to_string()));
            }
        }
        for update in [commands::available_commands_update(), loaded_usage] {
            if let Err(error) =
                connection.send_notification(SessionNotification::new(session_id.clone(), update))
            {
                unregister_acp_session(server, &session_id).await;
                return Err(agent_client_protocol::Error::internal_error().data(error.to_string()));
            }
        }
        return Ok(LoadSessionResponse::new().config_options(config_options));
    }
}

const ACP_TERMINAL_LEDGER_CAPACITY: usize = 1024;
const ACP_TERMINAL_BACKLOG_CAPACITY: usize = 256;
static ACTIVE_ACP_SESSIONS: std::sync::OnceLock<std::sync::Mutex<HashSet<String>>> =
    std::sync::OnceLock::new();

pub(crate) fn is_acp_session(session_id: &str) -> bool {
    ACTIVE_ACP_SESSIONS
        .get_or_init(|| std::sync::Mutex::new(HashSet::new()))
        .lock()
        .expect("ACP session registry mutex poisoned")
        .contains(session_id)
}

fn mark_acp_session(session_id: &str) {
    ACTIVE_ACP_SESSIONS
        .get_or_init(|| std::sync::Mutex::new(HashSet::new()))
        .lock()
        .expect("ACP session registry mutex poisoned")
        .insert(session_id.to_owned());
    // Other processes, the remote gateway first, see the session as open in
    // the app for as long as it is loaded here.
    if let Some(config_dir) = crate::config::get_config_dir() {
        crate::config::hold_session_open(
            &config_dir,
            session_id,
            crate::config::SessionHolder::App,
        );
    }
}

fn unmark_acp_session(session_id: &str) {
    crate::config::release_session_open(session_id, crate::config::SessionHolder::App);
    if let Some(sessions) = ACTIVE_ACP_SESSIONS.get() {
        sessions
            .lock()
            .expect("ACP session registry mutex poisoned")
            .remove(session_id);
    }
}

#[derive(Default)]
struct TerminalLedger {
    ids: HashSet<String>,
    order: VecDeque<String>,
}

impl TerminalLedger {
    fn claim(&mut self, id: &str) -> bool {
        if !self.ids.insert(id.to_owned()) {
            return false;
        }
        self.order.push_back(id.to_owned());
        while self.order.len() > ACP_TERMINAL_LEDGER_CAPACITY {
            if let Some(expired) = self.order.pop_front() {
                self.ids.remove(&expired);
            }
        }
        true
    }
}

struct SessionTaskSink {
    session_id: String,
    state: Arc<Mutex<crate::app::AppState>>,
    connection: agent_client_protocol::ConnectionTo<Client>,
    runtime: tokio::runtime::Handle,
    terminal_ledger: std::sync::Mutex<TerminalLedger>,
    terminal_backlog: Arc<std::sync::Mutex<VecDeque<TaskEvent>>>,
    terminal_overflow: Arc<AtomicBool>,
}

impl SessionTaskSink {
    fn handle_terminal(
        &self,
        event: TaskEvent,
        prompt_sender: std::sync::mpsc::SyncSender<TaskEvent>,
    ) {
        let id = event.task_id().to_string();
        let should_resume_model = event.notify_on_complete().unwrap_or(true);
        if !self
            .terminal_ledger
            .lock()
            .expect("ACP terminal ledger mutex poisoned")
            .claim(&id)
        {
            return;
        }
        let Some((task_id, event_session_id, output)) =
            crate::tools::task_event_to_tool_output(event.clone())
        else {
            return;
        };
        let call_id = event
            .call_id()
            .map(str::to_owned)
            .unwrap_or_else(|| task_id.clone());
        let state = Arc::clone(&self.state);
        let connection = self.connection.clone();
        let session_id = self.session_id.clone();
        let terminal_backlog = Arc::clone(&self.terminal_backlog);
        let terminal_overflow = Arc::clone(&self.terminal_overflow);
        self.runtime.spawn(async move {
            {
                // A `manage_task` `wait` result already records this
                // completion; a second copy would reach the provider twice.
                if !crate::tools::background_result_answered_by_wait(&event_session_id, &task_id)
                    .await
                {
                    // The record answers the card the client already shows.
                    // That call was answered when the task went to the
                    // background, so the provider request carries it as text.
                    let mut state = state.lock().await;
                    state.history.push(crate::background_task_history_message(
                        &task_id,
                        Some(&call_id),
                        output.clone(),
                    ));
                    crate::config::save_session_history(&event_session_id, &state.history);
                }
            }
            let update = SessionUpdate::ToolCallUpdate(
                agent_client_protocol::schema::v1::ToolCallUpdate::new(
                    call_id,
                    agent_client_protocol::schema::v1::ToolCallUpdateFields::new()
                        .status(if output.success {
                            agent_client_protocol::schema::v1::ToolCallStatus::Completed
                        } else {
                            agent_client_protocol::schema::v1::ToolCallStatus::Failed
                        })
                        .raw_output(serde_json::json!({
                            "content": output.content,
                            "exitCode": output.exit_code,
                            "changedPaths": [],
                            "truncated": output.truncated,
                            "commandStatus": output.command_status,
                        })),
                ),
            );
            let _ = connection.send_notification(SessionNotification::new(session_id, update));
            // Do not expose the fallback event until its history and ACP
            // notification have been handled. This keeps a prompt that wins
            // the receive race from resuming before durable completion.
            if should_resume_model {
                {
                    let mut backlog = terminal_backlog
                        .lock()
                        .expect("ACP terminal backlog mutex poisoned");
                    backlog.push_back(event.clone());
                    while backlog.len() > ACP_TERMINAL_BACKLOG_CAPACITY {
                        backlog.pop_front();
                        terminal_overflow.store(true, Ordering::Release);
                    }
                }
                let _ = prompt_sender.try_send(event);
            }
        });
    }
}

#[derive(Clone)]
struct TaskRoute {
    sender: std::sync::mpsc::SyncSender<TaskEvent>,
    known_task_ids: Arc<std::sync::Mutex<KnownTaskIds>>,
    sink: Option<Arc<SessionTaskSink>>,
}

type TaskRoutes = Arc<std::sync::Mutex<HashMap<String, TaskRoute>>>;

fn route_task_event(routes: &TaskRoutes, event: TaskEvent) {
    let session_id = event.session_id().to_string();
    let mut routes = routes.lock().expect("ACP task routes mutex poisoned");
    let Some(route) = routes.get(&session_id) else {
        return;
    };
    route
        .known_task_ids
        .lock()
        .expect("ACP known task IDs mutex poisoned")
        .insert(event.task_id().to_string());
    if event.is_terminal()
        && let Some(sink) = &route.sink
    {
        sink.handle_terminal(event, route.sender.clone());
        return;
    }
    match route.sender.try_send(event) {
        Ok(()) => {}
        Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
            routes.remove(&session_id);
        }
        Err(std::sync::mpsc::TrySendError::Full(_)) => {
            // Terminal events are already persisted and retained in the
            // bounded sink backlog. Keep the route alive so the prompt can
            // consume the coalesced completion once its queue drains.
        }
    }
}

#[derive(Clone)]
struct AcpServer {
    sessions: Sessions,
    client: Arc<reqwest::Client>,
    auto_approve: bool,
    task_routes: TaskRoutes,
    /// Set from the client's initialize capabilities; read when each session
    /// state is created so its prompt never changes mid-session.
    visualization_level: Arc<AtomicU8>,
    /// What the client advertised in `_meta.rustcode.smartUi` (#2484), as
    /// [`crate::app::SmartUi::as_u8`].
    client_smart_ui: Arc<AtomicU8>,
    /// The client advertised `_meta.rustcode.preview` (#2138), so its
    /// sessions get `localhost_preview`.
    client_has_preview: Arc<AtomicBool>,
    /// Discord Rich Presence, for a client that asked for it (#2179).
    presence: presence::Presence,
    /// `/remote` shares and their pairing offers.
    remote: remote::RemoteManager,
}

fn unknown_session(session_id: &str) -> agent_client_protocol::Error {
    agent_client_protocol::Error::invalid_params()
        .data(format!("unknown ACP session: {session_id}"))
}

impl AcpServer {
    /// An open session's state and working directory.
    async fn session_handle(
        &self,
        session_id: &str,
    ) -> Option<(Arc<Mutex<crate::app::AppState>>, std::path::PathBuf)> {
        self.sessions
            .lock()
            .await
            .get(session_id)
            .map(|session| (Arc::clone(&session.state), session.cwd.clone()))
    }

    fn new(auto_approve: bool) -> Result<Self, agent_client_protocol::Error> {
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
        let task_routes = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let task_subscription = crate::tools::background_task_manager().subscribe();
        let router_routes = Arc::clone(&task_routes);
        std::thread::Builder::new()
            .name("rustcode-acp-task-router".to_owned())
            .spawn(move || {
                while let Ok(event) = task_subscription.recv() {
                    route_task_event(&router_routes, event);
                }
            })
            .map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
        Ok(Self {
            sessions: new_registry(),
            client: Arc::new(client),
            auto_approve,
            task_routes,
            visualization_level: Arc::new(AtomicU8::new(0)),
            client_smart_ui: Arc::new(AtomicU8::new(0)),
            client_has_preview: Arc::new(AtomicBool::new(false)),
            presence: presence::Presence::default(),
            remote: remote::RemoteManager::default(),
        })
    }
}

async fn unregister_acp_session(server: &AcpServer, session_id: &str) {
    server
        .remote
        .stop_session(
            session_id,
            crate::remote::SessionCloseReason::SessionChanged,
        )
        .await;
    server
        .task_routes
        .lock()
        .expect("ACP task routes mutex poisoned")
        .remove(session_id);
    let session = server.sessions.lock().await.remove(session_id);
    unmark_acp_session(session_id);
    if let Some(session) = session {
        session.turns.cancel_active().await;
    }
}

/// The user started writing a message: start the deferred MCP servers now,
/// so the turn the message starts does not wait for them. Answers at once;
/// the servers start in the background, once.
#[derive(
    Debug,
    Clone,
    Default,
    serde::Serialize,
    serde::Deserialize,
    agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/warm", response = serde_json::Value)]
pub(crate) struct WarmRequest {}

/// The project's pull requests (#2645): `state` is `open` (the default),
/// `closed`, `merged` or `all`. Answers `{list}` or `{error: {code, message}}`.
#[derive(
    Debug,
    Clone,
    Default,
    serde::Serialize,
    serde::Deserialize,
    agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/pulls", response = serde_json::Value)]
pub(crate) struct PullsRequest {
    pub cwd: std::path::PathBuf,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub search: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

/// One pull request in full. Answers `{pull}` or `{error: {code, message}}`.
#[derive(
    Debug,
    Clone,
    Default,
    serde::Serialize,
    serde::Deserialize,
    agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/pulls/view", response = serde_json::Value)]
pub(crate) struct PullViewRequest {
    pub cwd: std::path::PathBuf,
    pub number: u64,
}

pub async fn run_acp(auto_approve: bool) -> Result<(), Box<dyn std::error::Error>> {
    let startup_state = crate::app::AppState::new();
    // Servers start on the session's first turn, not here: the app attaches
    // an engine for chats that may never run one (#2503).
    crate::mcp::defer_enabled_servers(&startup_state.config.mcp_servers);

    let server = AcpServer::new(auto_approve)?;
    Agent
        .builder()
        .name("rustcode")
        .on_receive_request(
            async |request: InitializeRequest, responder, _connection| {
                server.visualization_level.store(
                    client_visualization_level(&request.client_capabilities),
                    Ordering::Relaxed,
                );
                server.client_smart_ui.store(
                    client_smart_ui(&request.client_capabilities).as_u8(),
                    Ordering::Relaxed,
                );
                server.client_has_preview.store(
                    crate::preview::client_meta_supports_preview(
                        request.client_capabilities.meta.as_ref(),
                    ),
                    Ordering::Relaxed,
                );
                server
                    .presence
                    .set_client(presence::client_meta_wants_presence(
                        request.client_capabilities.meta.as_ref(),
                    ));
                responder.respond(
                    InitializeResponse::new(request.protocol_version)
                        .agent_capabilities(server_capabilities()),
                )
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: NewSessionRequest, responder, _connection| {
                    let cwd = canonical_acp_cwd(&request.cwd, "session/new")?;
                    let mut state = new_session_state(&cwd);
                    state.visualization_level = server.visualization_level.load(Ordering::Relaxed);
                    state.smart_ui = crate::app::SmartUi::from_u8(
                        server.client_smart_ui.load(Ordering::Relaxed),
                    );
                    state.smart_ui = crate::app::SmartUi::from_u8(
                        server.client_smart_ui.load(Ordering::Relaxed),
                    );
                    let session_id = state.active_session_id.clone();
                    state.preview_client = preview::preview_client_for(
                        server.client_has_preview.load(Ordering::Relaxed),
                        &_connection,
                        &session_id,
                    );
                    state.raw_cli_mode = false;
                    state.task_working_directory = Some(cwd.clone());
                    // ACP's requested cwd is both the task's initial location
                    // and its hard filesystem boundary, independent of the
                    // directory from which RustCode itself was launched.
                    state.workspace_root = Some(cwd.clone());
                    let config_options = build_session_config_options(&state);
                    let _ = crate::config::save_session_workspace(
                        &session_id,
                        &session_workspace(cwd.clone(), request.additional_directories.clone()),
                    );
                    let (task_sender, task_receiver) = std::sync::mpsc::sync_channel(64);
                    let known_task_ids = Arc::new(std::sync::Mutex::new(KnownTaskIds::default()));
                    let terminal_backlog = Arc::new(std::sync::Mutex::new(VecDeque::new()));
                    let terminal_overflow = Arc::new(AtomicBool::new(false));
                    let state = Arc::new(Mutex::new(state));
                    let task_sink = Arc::new(SessionTaskSink {
                        session_id: session_id.clone(),
                        state: Arc::clone(&state),
                        connection: _connection.clone(),
                        runtime: tokio::runtime::Handle::current(),
                        terminal_ledger: std::sync::Mutex::new(TerminalLedger::default()),
                        terminal_backlog: Arc::clone(&terminal_backlog),
                        terminal_overflow: Arc::clone(&terminal_overflow),
                    });
                    mark_acp_session(&session_id);
                    server
                        .task_routes
                        .lock()
                        .expect("ACP task routes mutex poisoned")
                        .insert(
                            session_id.clone(),
                            TaskRoute {
                                sender: task_sender,
                                known_task_ids: Arc::clone(&known_task_ids),
                                sink: Some(task_sink),
                            },
                        );
                    server.sessions.lock().await.insert(
                        session_id.clone(),
                        AcpSession {
                            state: Arc::clone(&state),
                            cwd,
                            turns: Arc::new(SessionTurnState::new()),
                            task_events: Arc::new(std::sync::Mutex::new(task_receiver)),
                            known_task_ids,
                            terminal_backlog,
                            terminal_overflow,
                        },
                    );
                    responder.respond(
                        NewSessionResponse::new(session_id.clone()).config_options(config_options),
                    )?;
                    // Commands, then the session's opening usage, so the client
                    // shows its context meter from the start rather than only
                    // after the first turn ends (#2414 follow-up).
                    let opening_usage = {
                        let state = state.lock().await;
                        usage::usage_update(&state)
                    };
                    for update in [commands::available_commands_update(), opening_usage] {
                        if let Err(error) = _connection
                            .send_notification(SessionNotification::new(session_id.clone(), update))
                        {
                            unregister_acp_session(&server, &session_id).await;
                            return Err(agent_client_protocol::Error::internal_error()
                                .data(error.to_string()));
                        }
                    }
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: ListSessionsRequest, responder, _connection| {
                responder.respond(acp_list_sessions(request)?)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: presence::PresenceRequest, responder, _connection| {
                    responder.respond(server.presence.status(request.cwd))
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: presence::SetPresenceRequest, responder, _connection| {
                    responder.respond(server.presence.set_enabled(request)?)
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: rewind::RewindRequest, responder, _connection| {
                    let Some((state, _)) = server.session_handle(&request.session_id).await else {
                        return responder.respond_with_error(unknown_session(&request.session_id));
                    };
                    let answer =
                        rewind::rewind(&mut *state.lock().await, request.user_index, &request.text);
                    match answer {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder.respond_with_error(error),
                    }
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: session_move::MoveSessionRequest, responder, _connection| {
                    if !valid_acp_session_id(&request.session_id) {
                        return responder.respond_with_error(
                            agent_client_protocol::Error::invalid_params()
                                .data("invalid session id"),
                        );
                    }
                    let cwd = match canonical_acp_cwd(&request.cwd, "session/move") {
                        Ok(cwd) => cwd,
                        Err(error) => return responder.respond_with_error(error),
                    };
                    // An open chat has a turn, tools and tasks bound to its folder.
                    if session_id_is_active(&*server.sessions.lock().await, &request.session_id) {
                        return responder.respond_with_error(
                            agent_client_protocol::Error::invalid_params()
                                .data("close the session before moving it"),
                        );
                    }
                    match session_move::move_session(&request.session_id, cwd) {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder.respond_with_error(error),
                    }
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: usage::UsageLimitsRequest, responder, connection| {
                    let Some((state, _)) = server.session_handle(&request.session_id).await else {
                        return responder.respond_with_error(unknown_session(&request.session_id));
                    };
                    connection.spawn(async move {
                        responder.respond(usage::limits(&state).await)?;
                        Ok(())
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: agents::AgentsRequest, responder, _connection| {
                    let Some((state, cwd)) = server.session_handle(&request.session_id).await
                    else {
                        return responder.respond_with_error(unknown_session(&request.session_id));
                    };
                    match agents::answer(&state, &cwd, &server.client, request.action).await {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder.respond_with_error(error),
                    }
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: steer::SteerRequest, responder, _connection| {
                    let state = server
                        .sessions
                        .lock()
                        .await
                        .get(&request.session_id)
                        .map(|session| Arc::clone(&session.state));
                    let Some(state) = state else {
                        return responder.respond_with_error(
                            agent_client_protocol::Error::invalid_params()
                                .data(format!("unknown ACP session: {}", request.session_id)),
                        );
                    };
                    let answer = steer::accept(&mut *state.lock().await, &request.text);
                    responder.respond(answer)
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: remote::RemoteStatusRequest, responder, _connection| {
                    responder.respond(server.remote.status(&request.session_id).await)
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: remote::RemoteStartRequest, responder, connection| {
                    let session = {
                        let sessions = server.sessions.lock().await;
                        sessions.get(&request.session_id).map(|session| {
                            remote::RemoteSession::from_acp(
                                request.session_id.clone(),
                                session,
                                connection.clone(),
                                Arc::clone(&server.client),
                                server.auto_approve,
                                server.presence.clone(),
                                server.remote.clone(),
                            )
                        })
                    };
                    let Some(session) = session else {
                        return Err(agent_client_protocol::Error::invalid_params()
                            .data(format!("unknown ACP session: {}", request.session_id)));
                    };
                    responder.respond(server.remote.start(session, request.pair).await?)
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: remote::RemotePairRequest, responder, _connection| {
                    responder.respond(server.remote.pair(&request.session_id).await)
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: remote::RemoteStopRequest, responder, _connection| {
                    responder.respond(server.remote.stop(&request.session_id).await?)
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |_request: capabilities::CapabilitiesRequest, responder, _connection| {
                responder.respond(capabilities::snapshot_value()?)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: capabilities::SetCapabilityRequest, responder, connection| {
                // Starting an MCP server can take seconds; keep the dispatch
                // loop free while it does.
                connection.spawn(async move {
                    match capabilities::set_capability(request).await {
                        Ok(snapshot) => responder.respond(snapshot)?,
                        Err(error) => responder.respond_with_error(error)?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: capabilities::AddMcpServerRequest, responder, connection| {
                connection.spawn(async move {
                    match capabilities::add_mcp_server(request).await {
                        Ok(snapshot) => responder.respond(snapshot)?,
                        Err(error) => responder.respond_with_error(error)?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: capabilities::RemoveMcpServerRequest, responder, connection| {
                connection.spawn(async move {
                    match capabilities::remove_mcp_server(request).await {
                        Ok(snapshot) => responder.respond(snapshot)?,
                        Err(error) => responder.respond_with_error(error)?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |_request: WarmRequest, responder, connection| {
                connection.spawn(async move {
                    crate::mcp::start_deferred_servers().await;
                    Ok(())
                })?;
                responder.respond(serde_json::json!({}))
            },
            agent_client_protocol::on_receive_request!(),
        )
        // `gh` can take seconds, so neither holds the dispatch loop.
        .on_receive_request(
            async move |request: PullsRequest, responder, connection| {
                connection.spawn(async move {
                    let listed = crate::pull_requests::list(
                        &request.cwd,
                        request.state.as_deref().unwrap_or("open"),
                        request.search.as_deref(),
                        request.limit.unwrap_or(30),
                    )
                    .await;
                    responder.respond(match listed {
                        Ok(list) => serde_json::json!({ "list": list }),
                        Err(error) => serde_json::json!({ "error": error }),
                    })?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: PullViewRequest, responder, connection| {
                connection.spawn(async move {
                    let viewed = crate::pull_requests::view(&request.cwd, request.number).await;
                    responder.respond(match viewed {
                        Ok(pull) => serde_json::json!({ "pull": pull }),
                        Err(error) => serde_json::json!({ "error": error }),
                    })?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        // Each runs gh or the account store, so none holds the dispatch loop.
        .on_receive_request(
            async move |_request: connections::ConnectionsRequest, responder, connection| {
                connection.spawn(async move {
                    match connections::snapshot_value().await {
                        Ok(value) => responder.respond(value)?,
                        Err(error) => responder.respond_with_error(error)?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |_request: connections::GitHubLoginRequest, responder, connection| {
                connection.spawn(async move {
                    match connections::github_login().await {
                        Ok(value) => responder.respond(value)?,
                        Err(error) => responder.respond_with_error(error)?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |_request: connections::GitHubWaitRequest, responder, connection| {
                connection.spawn(async move {
                    match connections::github_wait().await {
                        Ok(value) => responder.respond(value)?,
                        Err(error) => responder.respond_with_error(error)?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: connections::GitHubLogoutRequest, responder, connection| {
                connection.spawn(async move {
                    match connections::github_logout(request).await {
                        Ok(value) => responder.respond(value)?,
                        Err(error) => responder.respond_with_error(error)?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: connections::AccountRefreshRequest, responder, connection| {
                connection.spawn(async move {
                    match connections::account_refresh(request).await {
                        Ok(value) => responder.respond(value)?,
                        Err(error) => responder.respond_with_error(error)?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: connections::AccountLogoutRequest, responder, connection| {
                connection.spawn(async move {
                    match connections::account_logout(request).await {
                        Ok(value) => responder.respond(value)?,
                        Err(error) => responder.respond_with_error(error)?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |_request: connections::GitHubCancelRequest, responder, _connection| {
                crate::connections::cancel_github_login();
                responder.respond(serde_json::Value::Null)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: LoadSessionRequest, responder, connection| {
                    let response = acp_load_session(&server, request, &connection).await?;
                    responder.respond(response)
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: CloseSessionRequest, responder, _connection| {
                    let session_id = request.session_id.to_string();
                    server
                        .remote
                        .stop_session(
                            &session_id,
                            crate::remote::SessionCloseReason::SessionChanged,
                        )
                        .await;
                    server
                        .task_routes
                        .lock()
                        .expect("ACP task routes mutex poisoned")
                        .remove(&session_id);
                    let session = server.sessions.lock().await.remove(&session_id);
                    unmark_acp_session(&session_id);
                    if let Some(session) = session {
                        session.turns.cancel_active().await;
                    }
                    server.presence.session_closed(&session_id);
                    crate::tools::abort_background_starts(&session_id);
                    crate::tools::stop_background_tasks(&session_id);
                    responder.respond(CloseSessionResponse::new())
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: SetSessionConfigOptionRequest, responder, _connection| {
                    let session_id = request.session_id.to_string();
                    let state = server
                        .sessions
                        .lock()
                        .await
                        .get(&session_id)
                        .map(|session| Arc::clone(&session.state));
                    let Some(state) = state else {
                        return Err(agent_client_protocol::Error::invalid_params()
                            .data(format!("unknown ACP session: {session_id}")));
                    };

                    let mut state = state.lock().await;
                    let config_options =
                        handle_set_config_option(&mut state, &request.config_id, &request.value)?;
                    responder.respond(SetSessionConfigOptionResponse::new(config_options))
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            {
                let server = server.clone();
                async move |notification: CancelNotification, _connection| {
                    let turns = server
                        .sessions
                        .lock()
                        .await
                        .get(notification.session_id.0.as_ref())
                        .map(|session| Arc::clone(&session.turns));
                    if let Some(turns) = turns {
                        turns.cancel_active().await;
                    }
                    crate::tools::abort_background_starts(notification.session_id.0.as_ref());
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: PromptRequest, responder, connection| {
                    let session_id = request.session_id.to_string();
                    let session = server
                        .sessions
                        .lock()
                        .await
                        .get(&session_id)
                        .map(|session| {
                            (
                                Arc::clone(&session.state),
                                session.cwd.clone(),
                                Arc::clone(&session.turns),
                                Arc::clone(&session.task_events),
                                Arc::clone(&session.known_task_ids),
                                Arc::clone(&session.terminal_backlog),
                                Arc::clone(&session.terminal_overflow),
                            )
                        });
                    let Some((
                        state,
                        cwd,
                        turns,
                        task_events,
                        known_task_ids,
                        terminal_backlog,
                        terminal_overflow,
                    )) = session
                    else {
                        return Err(agent_client_protocol::Error::invalid_params()
                            .data(format!("unknown ACP session: {session_id}")));
                    };
                    let has_images = request
                        .prompt
                        .iter()
                        .any(|block| matches!(block, ContentBlock::Image(_)));
                    if has_images {
                        if let Some(message) = state.lock().await.image_support_error() {
                            return Err(agent_client_protocol::Error::invalid_params()
                                .data(message.to_string()));
                        }
                    }
                    let text = prompt_input(&request.prompt)
                        .map_err(|message| {
                            agent_client_protocol::Error::invalid_params().data(message)
                        })?
                        .text;

                    // A report only reads the session, so it answers now, beside
                    // a running turn, instead of queueing at the turn gate.
                    if !has_images && commands::is_report(&text) {
                        let task_connection = connection.clone();
                        connection.spawn(async move {
                            match prompt::run_report(&state, &session_id, &text, &task_connection)
                                .await
                            {
                                Ok(stop_reason) => {
                                    responder.respond(PromptResponse::new(stop_reason))?
                                }
                                Err(error) => responder.respond_with_error(error)?,
                            }
                            Ok(())
                        })?;
                        return Ok(());
                    }

                    // Request handlers run on ACP's dispatch loop. A model turn can
                    // perform many network and tool operations, so awaiting it here
                    // prevents the connection from processing any other messages.
                    let scheduled_turn = turns.schedule();
                    let wakeup_turns = Arc::downgrade(&turns);
                    let prompt_server = server.clone();
                    let task_connection = connection.clone();
                    let presence_cwd = cwd.clone();
                    server.presence.turn_started(&session_id, &presence_cwd);
                    connection.spawn(async move {
                        let result = prompt::run_prompt(
                            Arc::clone(&state),
                            cwd,
                            scheduled_turn,
                            session_id.clone(),
                            task_events,
                            known_task_ids,
                            terminal_backlog,
                            terminal_overflow,
                            text,
                            task_connection,
                            prompt_server.client,
                            prompt_server.auto_approve,
                            Some(prompt_server.remote.clone()),
                            wakeup_turns,
                        )
                        .await;
                        prompt_server
                            .presence
                            .turn_finished(&session_id, &presence_cwd);

                        // Steers the turn ended before reading go back to the
                        // client, which sends them next.
                        let unused = steer::end(&mut *state.lock().await, &session_id);
                        match result {
                            Ok(stop_reason) => {
                                responder.respond(
                                    PromptResponse::new(stop_reason)
                                        .meta(steer::unused_meta(unused)),
                                )?;
                            }
                            Err(error) => {
                                responder.respond_with_error(error)?;
                            }
                        }
                        Ok(())
                    })?;
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_close({
            let server = server.clone();
            async move |_connection| {
                server.remote.shutdown().await;
                let route_ids = server
                    .task_routes
                    .lock()
                    .expect("ACP task routes mutex poisoned")
                    .drain()
                    .map(|(session_id, _)| session_id)
                    .collect::<Vec<_>>();
                let sessions = server
                    .sessions
                    .lock()
                    .await
                    .drain()
                    .map(|(session_id, session)| (session_id, session.turns))
                    .collect::<Vec<_>>();
                for (session_id, turns) in sessions {
                    unmark_acp_session(&session_id);
                    turns.cancel_active().await;
                }
                for session_id in route_ids {
                    unmark_acp_session(&session_id);
                    crate::tools::abort_background_starts(&session_id);
                    crate::tools::stop_background_tasks(&session_id);
                }
                Ok(())
            }
        })
        .connect_to(Stdio::new())
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::{ContentBlock, TextContent};
    use std::path::PathBuf;

    fn task_event(session_id: &str, task_id: &str) -> TaskEvent {
        TaskEvent::Finished {
            id: task_id.into(),
            session_id: session_id.into(),
            call_id: None,
            command: "cargo test".to_owned(),
            notify_on_complete: true,
            output: Ok(rustcode_command::CommandOutput {
                success: true,
                exit_code: Some(0),
                signal: None,
                downstream_consumer_terminated: false,
                stdout: Default::default(),
                stderr: Default::default(),
            }),
        }
    }

    #[test]
    fn acp_task_router_isolates_session_events() {
        let routes = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let (a_sender, a_receiver) = std::sync::mpsc::sync_channel(2);
        let (b_sender, b_receiver) = std::sync::mpsc::sync_channel(2);
        routes.lock().unwrap().insert(
            "a".to_owned(),
            TaskRoute {
                sender: a_sender,
                known_task_ids: Arc::new(std::sync::Mutex::new(KnownTaskIds::default())),
                sink: None,
            },
        );
        routes.lock().unwrap().insert(
            "b".to_owned(),
            TaskRoute {
                sender: b_sender,
                known_task_ids: Arc::new(std::sync::Mutex::new(KnownTaskIds::default())),
                sink: None,
            },
        );

        route_task_event(&routes, task_event("a", "task-a"));
        assert_eq!(a_receiver.try_recv().unwrap().task_id().as_str(), "task-a");
        assert!(b_receiver.try_recv().is_err());
        route_task_event(&routes, task_event("b", "task-b"));
        assert_eq!(b_receiver.try_recv().unwrap().task_id().as_str(), "task-b");
    }

    #[test]
    fn acp_task_router_removes_disconnected_bounded_route() {
        let routes = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        routes.lock().unwrap().insert(
            "closed".to_owned(),
            TaskRoute {
                sender,
                known_task_ids: Arc::new(std::sync::Mutex::new(KnownTaskIds::default())),
                sink: None,
            },
        );
        drop(receiver);

        route_task_event(&routes, task_event("closed", "task-closed"));

        assert!(!routes.lock().unwrap().contains_key("closed"));
    }

    #[test]
    fn acp_terminal_ledger_is_bounded() {
        let mut ledger = TerminalLedger::default();
        for index in 0..(ACP_TERMINAL_LEDGER_CAPACITY + 1) {
            assert!(ledger.claim(&format!("task-{index}")));
        }
        assert_eq!(ledger.ids.len(), ACP_TERMINAL_LEDGER_CAPACITY);
        assert!(!ledger.ids.contains("task-0"));
    }

    #[test]
    fn extracts_text_blocks_from_prompt() {
        let prompt = vec![
            ContentBlock::Text(TextContent::new("inspect the project")),
            ContentBlock::Text(TextContent::new(" and explain it")),
        ];

        assert_eq!(
            prompt_input(&prompt).unwrap().text,
            "inspect the project and explain it"
        );
    }

    #[test]
    fn session_cursor_is_versioned_bounded_and_rejects_path_ids() {
        assert_eq!(parse_session_cursor(None).unwrap(), None);
        assert_eq!(
            parse_session_cursor(Some("v1:session-123")).unwrap(),
            Some("session-123")
        );
        assert!(parse_session_cursor(Some("v2:session-123")).is_err());
        assert!(parse_session_cursor(Some("v1:../escape")).is_err());
        assert!(parse_session_cursor(Some(&format!("v1:{}", "a".repeat(129)))).is_err());
        assert!(!valid_acp_session_id("../escape"));
        assert!(!valid_acp_session_id(""));
    }

    #[test]
    fn initialization_advertises_v1_list_and_load_support() {
        let capabilities = serde_json::to_value(server_capabilities()).unwrap();
        assert_eq!(capabilities["loadSession"], true);
        assert!(capabilities["sessionCapabilities"]["list"].is_object());
        assert!(capabilities["sessionCapabilities"]["close"].is_object());
        assert_eq!(capabilities["promptCapabilities"]["image"], true);
        assert_eq!(
            capabilities["_meta"]["rustcode"]["visualizations"],
            VISUALIZATIONS_CAPABILITY_VERSION
        );
    }

    #[test]
    fn visualization_support_comes_only_from_the_client_capability() {
        let parse = |value: serde_json::Value| {
            client_visualization_level(
                &serde_json::from_value::<agent_client_protocol::schema::v1::ClientCapabilities>(
                    value,
                )
                .unwrap(),
            )
        };
        let level = |value: serde_json::Value| {
            parse(serde_json::json!({"_meta": {"rustcode": {"visualizations": value}}}))
        };
        assert_eq!(level(serde_json::json!(1)), 1);
        assert_eq!(level(serde_json::json!(2)), 2);
        // A newer client is taught only the types this engine knows.
        assert_eq!(level(serde_json::json!(3)), 2);
        assert_eq!(level(serde_json::json!(u64::MAX)), 2);
        assert_eq!(parse(serde_json::json!({})), 0);
        assert_eq!(parse(serde_json::json!({"_meta": {"rustcode": {}}})), 0);
        assert_eq!(level(serde_json::json!(0)), 0);
        assert_eq!(level(serde_json::json!("yes")), 0);
        assert_eq!(level(serde_json::json!(-1)), 0);
        assert_eq!(
            parse(serde_json::json!({"_meta": {"other": {"visualizations": 1}}})),
            0
        );
        // A fresh or loaded ACP session never renders unless the server says so.
        assert_eq!(crate::app::AppState::new().visualization_level, 0);
    }

    #[test]
    fn smart_ui_comes_only_from_the_client_capability() {
        use crate::app::SmartUi;
        let parse = |value: serde_json::Value| {
            client_smart_ui(
                &serde_json::from_value::<agent_client_protocol::schema::v1::ClientCapabilities>(
                    value,
                )
                .unwrap(),
            )
        };
        let smart_ui = |value| serde_json::json!({"_meta": {"rustcode": {"smartUi": value}}});
        assert_eq!(parse(smart_ui(serde_json::json!(1))), SmartUi::On);
        assert_eq!(parse(smart_ui(serde_json::json!(0))), SmartUi::Off);
        assert_eq!(
            parse(smart_ui(serde_json::json!(true))),
            SmartUi::Unsupported
        );
        assert_eq!(
            parse(serde_json::json!({"_meta": {"rustcode": {"visualizations": 2}}})),
            SmartUi::Unsupported
        );
        assert_eq!(parse(serde_json::json!({})), SmartUi::Unsupported);
        assert_eq!(crate::app::AppState::new().smart_ui, SmartUi::Unsupported);
        for value in [SmartUi::Unsupported, SmartUi::Off, SmartUi::On] {
            assert_eq!(SmartUi::from_u8(value.as_u8()), value);
        }
    }

    #[test]
    fn a_saved_session_model_restores_profile_and_effort() {
        let mut state = crate::app::AppState::new();
        state.config.models = vec![crate::config::ModelProfile {
            name: "deep".into(),
            model: "deep-1".into(),
            url: "http://localhost:1/v1".into(),
            ..Default::default()
        }];
        let missing = rustcode_session::SessionModel {
            profile: "gone".into(),
            reasoning_effort: Some("high".into()),
            fast: false,
        };
        assert!(!state.apply_session_model(&missing));
        let saved = rustcode_session::SessionModel {
            profile: "deep".into(),
            reasoning_effort: Some("high".into()),
            fast: true,
        };
        assert!(state.apply_session_model(&saved));
        assert!(state.fast_mode, "fast mode is restored with the model");
        assert_eq!(state.model_name, "deep-1");
        assert_eq!(state.config.default.big(), "deep");
        assert_eq!(
            state.config.models[0].reasoning_effort.as_deref(),
            Some("high")
        );
    }

    #[test]
    fn loading_an_already_active_session_is_rejected() {
        let mut state = crate::app::AppState::new();
        state.active_session_id = "active-session".into();
        let state = Arc::new(Mutex::new(state));
        let (_sender, receiver) = std::sync::mpsc::sync_channel(1);
        let mut sessions = HashMap::new();
        sessions.insert(
            "active-session".into(),
            AcpSession {
                state,
                cwd: PathBuf::from("/tmp"),
                turns: Arc::new(SessionTurnState::new()),
                task_events: Arc::new(std::sync::Mutex::new(receiver)),
                known_task_ids: Arc::new(std::sync::Mutex::new(KnownTaskIds::default())),
                terminal_backlog: Arc::new(std::sync::Mutex::new(VecDeque::new())),
                terminal_overflow: Arc::new(AtomicBool::new(false)),
            },
        );
        assert!(session_id_is_active(&sessions, "active-session"));
        assert!(!session_id_is_active(&sessions, "another-session"));
    }

    #[tokio::test]
    async fn unregistering_failed_session_load_removes_registry_and_task_route() {
        let session_id = "failed-replay-session";
        let server = AcpServer {
            sessions: new_registry(),
            client: Arc::new(reqwest::Client::new()),
            auto_approve: false,
            task_routes: Arc::new(std::sync::Mutex::new(HashMap::new())),
            visualization_level: Arc::new(AtomicU8::new(0)),
            client_smart_ui: Arc::new(AtomicU8::new(0)),
            client_has_preview: Arc::new(AtomicBool::new(false)),
            presence: presence::Presence::default(),
            remote: remote::RemoteManager::default(),
        };
        let (task_sender, task_receiver) = std::sync::mpsc::sync_channel(1);
        let state = Arc::new(Mutex::new(crate::app::AppState::new()));
        let known_task_ids = Arc::new(std::sync::Mutex::new(KnownTaskIds::default()));
        server.task_routes.lock().unwrap().insert(
            session_id.into(),
            TaskRoute {
                sender: task_sender,
                known_task_ids: Arc::clone(&known_task_ids),
                sink: None,
            },
        );
        server.sessions.lock().await.insert(
            session_id.into(),
            AcpSession {
                state,
                cwd: PathBuf::from("/tmp"),
                turns: Arc::new(SessionTurnState::new()),
                task_events: Arc::new(std::sync::Mutex::new(task_receiver)),
                known_task_ids,
                terminal_backlog: Arc::new(std::sync::Mutex::new(VecDeque::new())),
                terminal_overflow: Arc::new(AtomicBool::new(false)),
            },
        );
        mark_acp_session(session_id);

        unregister_acp_session(&server, session_id).await;

        assert!(!server.sessions.lock().await.contains_key(session_id));
        assert!(!server.task_routes.lock().unwrap().contains_key(session_id));
        assert!(!is_acp_session(session_id));
    }

    #[test]
    fn session_list_filters_and_paginates_with_standard_titles() {
        let config_dir = crate::config::get_config_dir().unwrap();
        let store = rustcode_session::SessionStore::new(config_dir);
        let cwd = tempfile::tempdir().unwrap();
        let history = vec![
            crate::app::ChatMessage::new("user", "Listable session title"),
            crate::app::ChatMessage::new("assistant", "answer"),
        ];
        for index in 0..51 {
            let id = format!("acp-list-{index:03}");
            store.save_session_history(&id, &history);
            store
                .save_session_workspace(
                    &id,
                    &rustcode_session::SessionWorkspace {
                        cwd: cwd.path().to_path_buf(),
                        additional_directories: Vec::new(),
                        task_workspace_id: None,
                    },
                )
                .unwrap();
        }
        rustcode_session::flush_history();

        let first =
            acp_list_sessions(ListSessionsRequest::new().cwd(cwd.path().to_path_buf())).unwrap();
        assert_eq!(first.sessions.len(), ACP_SESSION_PAGE_SIZE);
        let first_session = &first.sessions[0];
        assert_eq!(
            first_session.title.as_deref(),
            Some("Listable session title")
        );
        assert_eq!(
            first_session.meta.as_ref().unwrap().get("messageCount"),
            Some(&serde_json::json!(2))
        );
        let cursor = first.next_cursor.expect("second page cursor");
        let second = acp_list_sessions(
            ListSessionsRequest::new()
                .cwd(cwd.path().to_path_buf())
                .cursor(cursor),
        )
        .unwrap();
        assert_eq!(second.sessions.len(), 1);
        assert!(second.next_cursor.is_none());
        assert!(acp_list_sessions(ListSessionsRequest::new().cwd("relative")).is_err());
        for index in 0..51 {
            let id = format!("acp-list-{index:03}");
            rustcode_session::SessionStore::delete_session_file(
                &store.session_dir(&id).join(rustcode_session::HISTORY_FILE),
            );
        }
    }

    #[test]
    fn session_list_skips_legacy_ids_that_cannot_be_loaded_or_used_as_cursors() {
        let config_dir = crate::config::get_config_dir().unwrap();
        let store = rustcode_session::SessionStore::new(config_dir);
        let history = vec![
            crate::app::ChatMessage::new("user", "legacy pagination prompt"),
            crate::app::ChatMessage::new("assistant", "answer"),
        ];
        let mut ids = Vec::new();
        for index in 0..49 {
            let id = format!("session-y-{index:03}");
            store.save_session_history(&id, &history);
            ids.push(id);
        }
        for index in 0..2 {
            let id = format!("session-a-{index:03}");
            store.save_session_history(&id, &history);
            ids.push(id);
        }
        // This legacy path-derived ID sorts at the page boundary, but cannot
        // be represented by the validated ACP session/load and cursor APIs.
        let invalid_id = "session-m.bad";
        store.save_session_history(invalid_id, &history);
        rustcode_session::flush_history();

        let first = acp_list_sessions(ListSessionsRequest::new()).unwrap();
        assert_eq!(first.sessions.len(), ACP_SESSION_PAGE_SIZE);
        assert!(
            first
                .sessions
                .iter()
                .all(|session| valid_acp_session_id(session.session_id.0.as_ref()))
        );
        let cursor = first.next_cursor.expect("one safe legacy session remains");
        assert!(parse_session_cursor(Some(&cursor)).is_ok());
        let second = acp_list_sessions(ListSessionsRequest::new().cursor(cursor)).unwrap();
        assert_eq!(second.sessions.len(), 1);
        assert!(second.next_cursor.is_none());
        assert_ne!(second.sessions[0].session_id.0.as_ref(), invalid_id);

        for id in ids
            .into_iter()
            .chain(std::iter::once(invalid_id.to_owned()))
        {
            rustcode_session::SessionStore::delete_session_file(
                &store.session_dir(&id).join(rustcode_session::HISTORY_FILE),
            );
        }
    }

    #[test]
    fn session_workspace_preserves_client_additional_directories() {
        let cwd = PathBuf::from("/workspace/project");
        let additional_directories = vec![
            PathBuf::from("/workspace/shared"),
            PathBuf::from("/workspace/generated"),
        ];

        let workspace = session_workspace(cwd.clone(), additional_directories.clone());

        assert_eq!(workspace.cwd, cwd);
        assert_eq!(workspace.additional_directories, additional_directories);
    }

    #[test]
    fn loaded_session_history_replays_user_assistant_and_tool_events_in_order() {
        let mut assistant = crate::app::ChatMessage::new("assistant", "Checking the file.");
        assistant.tool_calls.push(crate::app::ToolCallRef {
            id: "call-1".into(),
            name: "read_file".into(),
            arguments: r#"{"path":"src/main.rs"}"#.into(),
        });
        let mut result = crate::app::ChatMessage::new("tool", "file contents");
        result.tool_call_id = Some("call-1".into());
        result.tool_result = Some(crate::app::ToolResultRecord {
            workspace_generation: None,
            workspace_epoch: None,
            evidence_hash: None,
            tool_name: "read_file".into(),
            success: true,
            ..Default::default()
        });
        let updates = replay_history_updates(
            &[
                crate::app::ChatMessage::new("user", "inspect the file"),
                assistant,
                result,
                crate::app::ChatMessage::new("assistant", "It defines main."),
            ],
            None,
        );

        assert!(matches!(
            updates.first(),
            Some(SessionUpdate::UserMessageChunk(_))
        ));
        assert!(matches!(
            updates.get(1),
            Some(SessionUpdate::AgentMessageChunk(_))
        ));
        assert!(matches!(updates.get(2), Some(SessionUpdate::ToolCall(_))));
        assert!(matches!(
            updates.get(3),
            Some(SessionUpdate::ToolCallUpdate(_))
        ));
        assert!(matches!(
            updates.get(4),
            Some(SessionUpdate::AgentMessageChunk(_))
        ));
        assert_eq!(updates.len(), 5);
    }

    #[test]
    fn edit_tool_updates_carry_diff_content_live_and_on_replay() {
        let receipt = "successfully replaced target_content in 'src/a.rs' (resolved path: '/repo/src/a.rs')\n\n```diff\n@@ -1 +1 @@\n-old\n+new\n\n```";
        let mut stream = AcpEventStream::new().with_cwd(PathBuf::from("/repo"));
        let live = stream.updates(crate::network::AgentUiEvent::ToolFinished {
            id: "call-edit".into(),
            result: crate::network::events::ToolResult {
                tool_name: "replace_file_content".into(),
                content: receipt.into(),
                diff: None,
                file_preview: None,
                metadata: crate::network::events::ToolResultMetadata {
                    success: true,
                    changed_paths: vec!["src/a.rs".into()],
                    ..Default::default()
                },
            },
        });
        let [SessionUpdate::ToolCallUpdate(update)] = live.as_slice() else {
            panic!("expected one tool update, got {live:?}");
        };
        let content = serde_json::to_value(update.fields.content.as_ref().unwrap()).unwrap();
        assert_eq!(content[0]["type"], "diff");
        assert_eq!(content[0]["path"], "/repo/src/a.rs");
        assert_eq!(content[0]["oldText"], "old\n");
        assert_eq!(content[0]["newText"], "new\n");

        let mut result =
            crate::app::ChatMessage::new("tool", format!("replace_file_content: {receipt}"));
        result.tool_call_id = Some("call-edit".into());
        result.tool_result = Some(crate::app::ToolResultRecord {
            tool_name: "replace_file_content".into(),
            success: true,
            changed_paths: vec!["src/a.rs".into()],
            ..Default::default()
        });
        let replayed = replay_history_updates(&[result], Some(std::path::Path::new("/repo")));
        let [SessionUpdate::ToolCallUpdate(update)] = replayed.as_slice() else {
            panic!("expected one replayed tool update, got {replayed:?}");
        };
        let content = serde_json::to_value(update.fields.content.as_ref().unwrap()).unwrap();
        assert_eq!(content[0]["type"], "content");
        assert_eq!(content[1]["type"], "diff");
        assert_eq!(content[1]["newText"], "new\n");

        // A failed edit carries its receipt only.
        let mut stream = AcpEventStream::new();
        let failed = stream.updates(crate::network::AgentUiEvent::ToolFinished {
            id: "call-fail".into(),
            result: crate::network::events::ToolResult {
                tool_name: "replace_file_content".into(),
                content: receipt.into(),
                diff: None,
                file_preview: None,
                metadata: crate::network::events::ToolResultMetadata::default(),
            },
        });
        let [SessionUpdate::ToolCallUpdate(update)] = failed.as_slice() else {
            panic!("expected one tool update");
        };
        assert!(update.fields.content.is_none());
    }

    #[test]
    fn replayed_background_completion_updates_its_card_without_prose() {
        let completion = crate::background_task_history_message(
            "task-1",
            Some("call-bg"),
            crate::tools::ToolExecutionOutput::success("tests passed".to_owned()),
        );

        let replayed = replay_history_updates(&[completion], None);

        let [SessionUpdate::ToolCallUpdate(update)] = replayed.as_slice() else {
            panic!("expected one replayed tool update, got {replayed:?}");
        };
        assert_eq!(update.tool_call_id.0.as_ref(), "call-bg");
    }

    #[test]
    fn loaded_session_state_keeps_transcript_and_uses_its_own_identity() {
        let cwd = tempfile::tempdir().unwrap();
        let history = vec![
            crate::app::ChatMessage::new("user", "earlier task"),
            crate::app::ChatMessage::new("assistant", "earlier answer"),
        ];
        let state = build_loaded_app_state("session-loaded", cwd.path(), history.clone());
        assert_eq!(state.active_session_id, "session-loaded");
        assert_eq!(state.task_working_directory.as_deref(), Some(cwd.path()));
        assert_eq!(state.workspace_root.as_deref(), Some(cwd.path()));
        assert_eq!(state.history.to_vec(), history);

        let mut state = state;
        state.workspace_root = None;
        assert_eq!(
            state.effective_workspace_root().as_deref(),
            Some(cwd.path())
        );
    }

    #[test]
    fn new_sessions_never_reuse_the_shared_last_active_session() {
        let cwd = tempfile::tempdir().unwrap();
        // The TUI reuses an empty last-active session; another process may own it.
        let (_, _, mut config) = crate::config::load_config();
        let shared = crate::config::create_new_session(&mut config);

        let first = new_session_state(cwd.path());
        let second = new_session_state(cwd.path());

        assert!(!first.active_session_id.is_empty());
        assert_ne!(first.active_session_id, shared);
        assert_ne!(second.active_session_id, shared);
        assert_ne!(first.active_session_id, second.active_session_id);
        let (_, _, config) = crate::config::load_config();
        assert_eq!(
            config.last_active_session_id.as_deref(),
            Some(shared.as_str())
        );
    }

    #[tokio::test]
    async fn a_loaded_session_persists_only_to_its_own_history() {
        let store = rustcode_session::SessionStore::new(crate::config::get_config_dir().unwrap());
        let cwd = tempfile::tempdir().unwrap();
        // session/new, then session/load of an older session, as the macOS app does.
        let created = new_session_state(cwd.path()).active_session_id;
        crate::config::set_active_session_id(&created);
        let loaded = rustcode_session::next_session_id();
        let earlier = vec![
            crate::app::ChatMessage::new("user", "check my daily dev feed"),
            crate::app::ChatMessage::new("assistant", "Here is your feed."),
        ];
        store.save_session_history(&loaded, &earlier);
        crate::config::flush_history();

        let mut state = build_loaded_app_state(&loaded, cwd.path(), earlier);
        state.history.push(crate::app::ChatMessage::new(
            "user",
            "follow-up in the loaded session",
        ));
        let state = Arc::new(Mutex::new(state));
        crate::network::prepare_turn_request(
            &reqwest::Client::new(),
            &state,
            0,
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
        .expect("request preparation");
        crate::config::flush_history();

        let loaded_history = store.load_session_file(
            &store
                .session_dir(&loaded)
                .join(rustcode_session::HISTORY_FILE),
        );
        assert_eq!(
            loaded_history
                .last()
                .map(|message| message.content.as_str()),
            Some("follow-up in the loaded session")
        );
        let created_history = store.load_session_file(
            &store
                .session_dir(&created)
                .join(rustcode_session::HISTORY_FILE),
        );
        assert!(
            created_history.is_empty(),
            "the session from session/new must not receive the loaded transcript"
        );
    }

    #[test]
    fn acp_cwd_must_be_an_absolute_existing_directory() {
        let directory = tempfile::tempdir().unwrap();
        assert_eq!(
            canonical_acp_cwd(directory.path(), "session/new").unwrap(),
            std::fs::canonicalize(directory.path()).unwrap()
        );
        assert!(canonical_acp_cwd(std::path::Path::new("relative"), "session/new").is_err());
        assert!(
            canonical_acp_cwd(
                std::path::Path::new("/definitely/not/a/directory"),
                "session/new"
            )
            .is_err()
        );
    }

    #[test]
    fn session_info_uses_standard_title_and_marks_legacy_workspace_unknown() {
        let meta = crate::config::SessionMeta {
            path: PathBuf::from("/tmp/sessions/session-123/history.json"),
            title: "Inspect project".into(),
            when: "2026-09-24T00:00:00Z".into(),
            message_count: 7,
            workspace_cwd: None,
        };
        let info = session_info(&meta, None);
        assert_eq!(info.session_id.0.as_ref(), "session-123");
        assert_eq!(info.title.as_deref(), Some("Inspect project"));
        assert_eq!(
            info.meta.unwrap().get("messageCount"),
            Some(&serde_json::json!(7))
        );
    }

    #[test]
    fn session_info_dates_a_chat_by_its_last_message_not_its_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("history.json");
        // Written now, as opening a chat rewrites its history.
        std::fs::write(&path, "[]").unwrap();
        let meta = crate::config::SessionMeta {
            path,
            title: "Old chat".into(),
            when: "2026-09-24T10:33:25.122403+02:00".into(),
            message_count: 2,
            workspace_cwd: None,
        };
        assert_eq!(
            session_info(&meta, None).updated_at.as_deref(),
            Some("2026-09-24T08:33:25.122403+00:00")
        );

        let untimed = crate::config::SessionMeta {
            when: String::new(),
            ..meta
        };
        assert!(session_info(&untimed, None).updated_at.is_some());
    }

    #[test]
    fn session_state_can_keep_workspace_root() {
        let mut state = crate::app::AppState::new();
        state.workspace_root = Some(PathBuf::from("/tmp/project"));
        assert_eq!(state.workspace_root, Some(PathBuf::from("/tmp/project")));
    }

    fn call(name: &str) -> crate::tools::ToolCall {
        crate::tools::ToolCall {
            name: name.to_string(),
            arguments: serde_json::json!({}),
            call_id: Some(format!("call-{name}")),
        }
    }

    #[test]
    fn acp_permissions_allow_reads_but_request_mutations() {
        assert_eq!(
            approval_requirement(&[call("grep")], crate::config::AgentMode::Build, false),
            ApprovalRequirement::Allow
        );
        assert_eq!(
            approval_requirement(
                &[call("write_to_file")],
                crate::config::AgentMode::Build,
                false
            ),
            ApprovalRequirement::Request
        );
    }

    #[test]
    fn acp_yolo_is_an_explicit_permission_bypass() {
        assert_eq!(
            approval_requirement(
                &[call("write_to_file")],
                crate::config::AgentMode::Build,
                true
            ),
            ApprovalRequirement::Allow
        );
    }

    #[test]
    fn acp_yolo_auto_approves_one_shot_network_permission() {
        let call = crate::tools::ToolCall {
            name: "run_command".to_owned(),
            arguments: serde_json::json!({
                "command": "cargo test",
                "network_access": true
            }),
            call_id: Some("call-network".to_owned()),
        };
        assert_eq!(
            approval_requirement(&[call], crate::config::AgentMode::Build, true),
            ApprovalRequirement::Allow
        );
    }

    #[test]
    fn acp_yolo_auto_approves_one_shot_filesystem_permission() {
        let call = crate::tools::ToolCall {
            name: "run_command".to_owned(),
            arguments: serde_json::json!({
                "command": "touch /tmp/release.txt",
                "filesystem_write_path": "/tmp"
            }),
            call_id: Some("call-filesystem".to_owned()),
        };
        assert_eq!(
            approval_requirement(&[call], crate::config::AgentMode::Build, true),
            ApprovalRequirement::Allow
        );
    }

    #[test]
    fn acp_plan_mode_denies_mutations_even_with_yolo() {
        assert!(matches!(
            approval_requirement(
                &[call("write_to_file")],
                crate::config::AgentMode::Plan,
                true
            ),
            ApprovalRequirement::Deny(_)
        ));
    }

    #[tokio::test]
    async fn session_turns_are_serialized() {
        let turn_state = Arc::new(SessionTurnState::new());
        let first = turn_state.begin().await;
        let second_state = Arc::clone(&turn_state);
        let second = tokio::spawn(async move { second_state.begin().await });

        tokio::task::yield_now().await;
        assert!(
            !second.is_finished(),
            "a second prompt must wait for the active turn"
        );

        drop(first);
        let second = second.await.expect("queued prompt task");
        assert!(!second.cancel_token().is_cancelled());
    }

    #[tokio::test]
    async fn a_report_answers_while_a_turn_holds_the_session() {
        let turn_state = SessionTurnState::new();
        let _running = turn_state.begin().await;
        let state = Mutex::new(crate::app::AppState::new());
        let updates = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            prompt::report_updates(&state, "/status"),
        )
        .await
        .expect("a report must not wait for the running turn");
        assert!(matches!(
            updates.as_slice(),
            [SessionUpdate::AgentMessageChunk(chunk), SessionUpdate::UsageUpdate(_)]
                if matches!(&chunk.content, ContentBlock::Text(text) if text.text.starts_with("**Status**"))
                    && chunk.meta.as_ref().unwrap()["rustcode"]["report"] == true
        ));
        assert!(state.lock().await.history.is_empty());
        let context = prompt::report_updates(&state, "/context").await;
        let SessionUpdate::AgentMessageChunk(chunk) = &context[0] else {
            panic!("expected the context reply");
        };
        let meta = &chunk.meta.as_ref().unwrap()["rustcode"];
        assert_eq!(meta["kind"], "context");
        assert_eq!(meta["report"], true);
    }

    #[tokio::test]
    async fn cancelling_a_session_cancels_only_the_active_turn() {
        let turn_state = SessionTurnState::new();
        let first = turn_state.begin().await;

        assert!(turn_state.cancel_active().await);
        assert!(first.cancel_token().is_cancelled());
        drop(first);

        assert!(!turn_state.cancel_active().await);
        let next = turn_state.begin().await;
        assert!(!next.cancel_token().is_cancelled());
    }

    #[tokio::test]
    async fn cancellation_reaches_an_accepted_prompt_waiting_for_the_session_gate() {
        let turn_state = Arc::new(SessionTurnState::new());
        let first = turn_state.begin().await;
        let queued_state = Arc::clone(&turn_state);
        let queued = tokio::spawn(async move { queued_state.begin().await });
        tokio::task::yield_now().await;

        assert!(turn_state.cancel_active().await);
        assert!(first.cancel_token().is_cancelled());
        drop(first);

        let queued = queued.await.expect("queued prompt task");
        assert!(queued.cancel_token().is_cancelled());
        drop(queued);

        let later = turn_state.begin().await;
        assert!(!later.cancel_token().is_cancelled());
    }

    #[tokio::test]
    async fn cancellation_reaches_a_prompt_before_its_task_starts() {
        let turn_state = SessionTurnState::new();
        let scheduled = turn_state.schedule();

        assert!(turn_state.cancel_active().await);
        let turn = scheduled.begin().await;
        assert!(turn.cancel_token().is_cancelled());
    }

    #[test]
    fn acp_event_stream_emits_incremental_text_without_final_duplication() {
        let mut stream = AcpEventStream::new();
        let updates = stream.updates(crate::network::AgentUiEvent::TextDelta {
            text: "hello".to_string(),
        });
        assert!(matches!(
            updates.as_slice(),
            [SessionUpdate::AgentMessageChunk(chunk)]
                if matches!(&chunk.content, ContentBlock::Text(text) if text.text == "hello")
        ));

        let updates = stream.updates(crate::network::AgentUiEvent::TurnFinished {
            content: "hello".to_string(),
            completed: true,
            timing: None,
        });
        assert!(
            updates.is_empty(),
            "final content must not repeat streamed text"
        );
    }

    #[test]
    fn acp_event_stream_sends_only_the_unstreamed_rest_of_a_partial_answer() {
        let mut stream = AcpEventStream::new();
        stream.updates(crate::network::AgentUiEvent::TextDelta {
            text: "Here's the weather. It's comfortable with".to_string(),
        });

        let updates = stream.updates(crate::network::AgentUiEvent::TurnFinished {
            content: "Here's the weather. It's comfortable with light wind.".to_string(),
            completed: true,
            timing: None,
        });
        assert!(matches!(
            updates.as_slice(),
            [SessionUpdate::AgentMessageChunk(chunk)]
                if matches!(&chunk.content, ContentBlock::Text(text) if text.text == " light wind.")
        ));
    }

    #[test]
    fn acp_event_stream_prose_before_a_tool_call_is_not_the_answer_start() {
        let mut stream = AcpEventStream::new();
        stream.updates(crate::network::AgentUiEvent::TextDelta {
            text: "Let me".to_string(),
        });
        stream.updates(crate::network::AgentUiEvent::ToolStarted {
            name: "search_web".to_string(),
            id: "call-1".to_string(),
            detail: None,
            summary: None,
        });

        let updates = stream.updates(crate::network::AgentUiEvent::TurnFinished {
            content: "Let me know if you need more.".to_string(),
            completed: true,
            timing: None,
        });
        assert!(matches!(
            updates.as_slice(),
            [SessionUpdate::AgentMessageChunk(chunk)]
                if matches!(&chunk.content, ContentBlock::Text(text) if text.text == "Let me know if you need more.")
        ));
    }

    #[test]
    fn acp_event_stream_uses_final_text_when_no_delta_was_observed() {
        let mut stream = AcpEventStream::new();
        let updates = stream.updates(crate::network::AgentUiEvent::TurnFinished {
            content: "fallback".to_string(),
            completed: true,
            timing: None,
        });
        assert!(matches!(
            updates.as_slice(),
            [SessionUpdate::AgentMessageChunk(chunk)]
                if matches!(&chunk.content, ContentBlock::Text(text) if text.text == "fallback")
        ));
    }

    #[test]
    fn acp_event_stream_sends_stop_markers_as_notices_not_answers() {
        let mut stream = AcpEventStream::new();
        let cancelled = stream.updates(crate::network::AgentUiEvent::TurnFinished {
            content: "[harness: turn stopped — cancelled]".to_string(),
            completed: false,
            timing: None,
        });
        assert!(cancelled.is_empty(), "the stop reason reports a cancel");

        let stopped = stream.updates(crate::network::AgentUiEvent::TurnFinished {
            content: "[harness: turn stopped — loop_escalation]".to_string(),
            completed: false,
            timing: None,
        });
        let [SessionUpdate::AgentMessageChunk(chunk)] = stopped.as_slice() else {
            panic!("expected one notice chunk, got {stopped:?}");
        };
        assert!(
            matches!(&chunk.content, ContentBlock::Text(text) if text.text == "Stopped: the agent was repeating itself")
        );
        assert_eq!(
            chunk
                .meta
                .as_ref()
                .and_then(|meta| meta["rustcode"]["kind"].as_str()),
            Some("notice")
        );
        let resumable = |chunk: &agent_client_protocol::schema::v1::ContentChunk| {
            chunk
                .meta
                .as_ref()
                .and_then(|meta| meta["rustcode"]["resumable"].as_bool())
        };
        assert_eq!(
            resumable(chunk),
            Some(false),
            "a guard's stop is not resumed"
        );

        let dropped = stream.updates(crate::network::AgentUiEvent::TurnFinished {
            content: "[harness: turn stopped — provider_error]".to_string(),
            completed: false,
            timing: None,
        });
        let [SessionUpdate::AgentMessageChunk(chunk)] = dropped.as_slice() else {
            panic!("expected one notice chunk, got {dropped:?}");
        };
        assert_eq!(
            resumable(chunk),
            Some(true),
            "a provider's stop can continue"
        );
    }

    // #2608: a notice is sent as one non-resumable notice chunk, which the
    // app shows as a toast and keeps out of the transcript.
    #[test]
    fn acp_event_stream_sends_a_notice_as_a_plain_notice_chunk() {
        use agent_client_protocol::schema::v1::{ContentBlock, SessionUpdate};
        let mut stream = AcpEventStream::new();
        let text =
            "Fast mode was requested, but the provider served this response at the default tier.";
        let updates = stream.updates(crate::network::AgentUiEvent::Notice {
            message: text.to_string(),
        });
        let [SessionUpdate::AgentMessageChunk(chunk)] = updates.as_slice() else {
            panic!("expected one notice chunk, got {updates:?}");
        };
        assert!(matches!(&chunk.content, ContentBlock::Text(t) if t.text == text));
        let meta = &chunk.meta.as_ref().expect("notice meta")["rustcode"];
        assert_eq!(meta["kind"], "notice");
        assert_eq!(meta["resumable"], false);
    }

    // #2584: the provider's own message is the stop notice, and the
    // `provider_error` marker after it adds no second, vaguer one.
    #[test]
    fn acp_event_stream_sends_a_provider_refusal_as_the_only_stop_notice() {
        use agent_client_protocol::schema::v1::{ContentBlock, SessionUpdate};
        let mut stream = AcpEventStream::new();
        let refusal = stream.updates(crate::network::AgentUiEvent::ProviderError {
            message: "lfm2.5 · 507: Model does not fit in memory.".to_string(),
        });
        let [SessionUpdate::AgentMessageChunk(chunk)] = refusal.as_slice() else {
            panic!("expected one notice chunk, got {refusal:?}");
        };
        assert!(
            matches!(&chunk.content, ContentBlock::Text(text) if text.text == "lfm2.5 · 507: Model does not fit in memory.")
        );
        let meta = &chunk.meta.as_ref().expect("notice meta")["rustcode"];
        assert_eq!(meta["kind"], "notice");
        assert_eq!(meta["resumable"], true);

        let marker = stream.updates(crate::network::AgentUiEvent::TurnFinished {
            content: "[harness: turn stopped — provider_error]".to_string(),
            completed: false,
            timing: None,
        });
        assert!(marker.is_empty(), "{marker:?}");
    }

    #[test]
    fn acp_event_stream_keeps_final_text_after_an_earlier_tool_round_delta() {
        let mut stream = AcpEventStream::new();
        stream.updates(crate::network::AgentUiEvent::TextDelta {
            text: "checking".to_string(),
        });

        let updates = stream.updates(crate::network::AgentUiEvent::TurnFinished {
            content: "done".to_string(),
            completed: true,
            timing: None,
        });
        assert!(matches!(
            updates.as_slice(),
            [SessionUpdate::AgentMessageChunk(chunk)]
                if matches!(&chunk.content, ContentBlock::Text(text) if text.text == "done")
        ));
    }

    #[test]
    fn acp_event_stream_plain_text_remains_unchanged() {
        let mut stream = AcpEventStream::new();
        let updates = stream.updates(crate::network::AgentUiEvent::TextDelta {
            text: "Hello, how can I help you today?".to_string(),
        });
        assert_eq!(updates.len(), 1);
        assert!(matches!(
            &updates[0],
            SessionUpdate::AgentMessageChunk(chunk)
                if matches!(&chunk.content, ContentBlock::Text(text) if text.text == "Hello, how can I help you today?")
        ));
    }

    #[test]
    fn acp_event_stream_complete_think_blocks_emitted_as_thought_chunks() {
        let mut stream = AcpEventStream::new();
        let updates = stream.updates(crate::network::AgentUiEvent::TextDelta {
            text: "<think>Let me analyze the problem first.</think>".to_string(),
        });
        assert_eq!(updates.len(), 1);
        assert!(matches!(
            &updates[0],
            SessionUpdate::AgentThoughtChunk(chunk)
                if matches!(&chunk.content, ContentBlock::Text(text) if text.text == "Let me analyze the problem first.")
        ));
    }

    #[test]
    fn acp_event_stream_thought_tags_split_across_multiple_text_delta_events() {
        let mut stream = AcpEventStream::new();

        // Opening tag split: "<th" + "ink>Internal reasoning</th" + "ink>Visible answer"
        let mut all_updates = Vec::new();
        all_updates.extend(stream.updates(crate::network::AgentUiEvent::TextDelta {
            text: "<th".to_string(),
        }));
        all_updates.extend(stream.updates(crate::network::AgentUiEvent::TextDelta {
            text: "ink>Internal reasoning</th".to_string(),
        }));
        all_updates.extend(stream.updates(crate::network::AgentUiEvent::TextDelta {
            text: "ink>Visible answer".to_string(),
        }));

        let message_chunks: Vec<String> = all_updates
            .iter()
            .filter_map(|u| match u {
                SessionUpdate::AgentMessageChunk(chunk) => match &chunk.content {
                    ContentBlock::Text(t) => Some(t.text.clone()),
                    _ => None,
                },
                _ => None,
            })
            .collect();

        let thought_chunks: Vec<String> = all_updates
            .iter()
            .filter_map(|u| match u {
                SessionUpdate::AgentThoughtChunk(chunk) => match &chunk.content {
                    ContentBlock::Text(t) => Some(t.text.clone()),
                    _ => None,
                },
                _ => None,
            })
            .collect();

        assert_eq!(thought_chunks.join(""), "Internal reasoning");
        assert_eq!(message_chunks.join(""), "Visible answer");
    }

    #[test]
    fn acp_event_stream_text_before_and_after_thought_block() {
        let mut stream = AcpEventStream::new();
        let updates = stream.updates(crate::network::AgentUiEvent::TextDelta {
            text: "Prefix prose. <think>Secret reasoning.</think> Suffix prose.".to_string(),
        });

        assert_eq!(updates.len(), 3);
        assert!(matches!(
            &updates[0],
            SessionUpdate::AgentMessageChunk(chunk)
                if matches!(&chunk.content, ContentBlock::Text(text) if text.text == "Prefix prose. ")
        ));
        assert!(matches!(
            &updates[1],
            SessionUpdate::AgentThoughtChunk(chunk)
                if matches!(&chunk.content, ContentBlock::Text(text) if text.text == "Secret reasoning.")
        ));
        assert!(matches!(
            &updates[2],
            SessionUpdate::AgentMessageChunk(chunk)
                if matches!(&chunk.content, ContentBlock::Text(text) if text.text == " Suffix prose.")
        ));
    }

    #[test]
    fn acp_event_stream_tool_call_round_followed_by_final_answer() {
        let mut stream = AcpEventStream::new();

        // Round 1: Model thinks about calling get_time, then calls tool
        let r1_deltas = stream.updates(crate::network::AgentUiEvent::TextDelta {
            text: "<think>I need to check the current time.</think>".to_string(),
        });
        assert!(matches!(
            r1_deltas.as_slice(),
            [SessionUpdate::AgentThoughtChunk(chunk)]
                if matches!(&chunk.content, ContentBlock::Text(text) if text.text == "I need to check the current time.")
        ));

        let tool_start = stream.updates(crate::network::AgentUiEvent::ToolStarted {
            detail: None,
            summary: None,
            name: "get_time".to_string(),
            id: "call-1".to_string(),
        });
        assert!(matches!(
            tool_start.as_slice(),
            [SessionUpdate::ToolCall(call)]
                if call.tool_call_id.to_string() == "call-1"
        ));

        let tool_finish = stream.updates(crate::network::AgentUiEvent::ToolFinished {
            id: "call-1".to_string(),
            result: crate::network::events::ToolResult {
                tool_name: "get_time".to_string(),
                content: "12:20 PM".to_string(),
                diff: None,
                file_preview: None,
                metadata: crate::network::events::ToolResultMetadata {
                    execution_us: 0,
                    workspace_generation: None,
                    pending: false,
                    command: None,
                    call_id: Some("call-1".to_string()),
                    arguments_hash: "hash".to_string(),
                    success: true,
                    exit_code: None,
                    changed_paths: Vec::new(),
                    truncated: false,
                    payload_truncated: false,
                    completeness: rustcode_core::ToolResultCompleteness::Complete,
                    full_output_artifact: None,
                    replayed: false,
                    error_kind: None,
                    retryable: false,
                    inspection: None,
                    command_status: None,
                },
            },
        });
        assert!(matches!(
            tool_finish.as_slice(),
            [SessionUpdate::ToolCallUpdate(update)]
                if update.tool_call_id.to_string() == "call-1"
        ));

        // Round 2: Model thinks about the result and emits the final answer
        let r2_deltas = stream.updates(crate::network::AgentUiEvent::TextDelta {
            text: "<think>The time is 12:20 PM.</think>The current time is 12:20 PM.".to_string(),
        });
        assert_eq!(r2_deltas.len(), 2);
        assert!(matches!(
            &r2_deltas[0],
            SessionUpdate::AgentThoughtChunk(chunk)
                if matches!(&chunk.content, ContentBlock::Text(text) if text.text == "The time is 12:20 PM.")
        ));
        assert!(matches!(
            &r2_deltas[1],
            SessionUpdate::AgentMessageChunk(chunk)
                if matches!(&chunk.content, ContentBlock::Text(text) if text.text == "The current time is 12:20 PM.")
        ));

        // TurnFinished does not duplicate
        let turn_finished = stream.updates(crate::network::AgentUiEvent::TurnFinished {
            content: "<think>The time is 12:20 PM.</think>The current time is 12:20 PM."
                .to_string(),
            completed: true,
            timing: None,
        });
        assert!(
            turn_finished.is_empty(),
            "TurnFinished must not duplicate final answer"
        );
    }

    #[test]
    fn acp_event_stream_turn_finished_fallback_strips_think_blocks_when_not_streamed() {
        let mut stream = AcpEventStream::new();
        let finished = stream.updates(crate::network::AgentUiEvent::TurnFinished {
            content: "<think>Reasoning scratchpad</think>Unstreamed answer".to_string(),
            completed: true,
            timing: None,
        });
        assert_eq!(finished.len(), 1);
        assert!(matches!(
            &finished[0],
            SessionUpdate::AgentMessageChunk(chunk)
                if matches!(&chunk.content, ContentBlock::Text(text) if text.text == "Unstreamed answer")
        ));
    }

    fn thought_chunks(updates: &[SessionUpdate]) -> Vec<(String, serde_json::Value)> {
        updates
            .iter()
            .filter_map(|update| match update {
                SessionUpdate::AgentThoughtChunk(chunk) => {
                    match &chunk.content {
                        ContentBlock::Text(text) => {
                            Some(
                                (
                                    text.text.clone(),
                                    serde_json::Value::Object(
                                        chunk.meta.clone().unwrap_or_default(),
                                    )["rustcode"]
                                        .clone(),
                                ),
                            )
                        }
                        _ => None,
                    }
                }
                _ => None,
            })
            .collect()
    }

    // #2299: a thought says whether it is the provider's summary or the
    // model's reasoning text, and whitespace alone never opens one.
    #[test]
    fn acp_event_stream_labels_thoughts_by_source_and_skips_empty_ones() {
        let mut stream = AcpEventStream::new();
        let raw = stream.updates(crate::network::AgentUiEvent::TextDelta {
            text: "<think>\nLet me check.</think>Answer".to_string(),
        });
        assert_eq!(
            thought_chunks(&raw),
            vec![(
                "Let me check.".to_string(),
                serde_json::json!({"thoughtSource": "raw"})
            )]
        );

        assert!(
            stream
                .updates(crate::network::AgentUiEvent::ReasoningSource { summary: true })
                .is_empty()
        );
        let empty = stream.updates(crate::network::AgentUiEvent::TextDelta {
            text: "<think>\n\n".to_string(),
        });
        assert!(thought_chunks(&empty).is_empty(), "{empty:?}");
        let summary = stream.updates(crate::network::AgentUiEvent::TextDelta {
            text: "**Searching online**</think>".to_string(),
        });
        assert_eq!(
            thought_chunks(&summary),
            vec![(
                "**Searching online**".to_string(),
                serde_json::json!({"thoughtSource": "summary"})
            )]
        );
    }

    // #2307: a replayed turn's last reply carries how long the turn took
    // and when it ended, so the Mac app shows them as it did live. A turn
    // with no end, as one still open, carries nothing.
    #[test]
    fn replayed_system_cancel_marker_is_a_timed_empty_reply() {
        let turn = crate::app::TurnTiming {
            turn_id: "t1".into(),
            started_at: Some("2026-10-30T12:42:27Z".into()),
            ended_at: Some("2026-10-30T12:42:37Z".into()),
            elapsed_work_ms: Some(9_770),
            outcome: Some(crate::app::TurnOutcome::Cancelled),
        };
        let mut note = crate::app::ChatMessage::new("system", "Request cancelled by user");
        note.turn = Some(turn.clone());
        let mut marker =
            crate::app::ChatMessage::new("system", "[harness: turn stopped — cancelled]");
        marker.turn = Some(turn);
        let updates = replay_history_updates(&[note, marker], None);
        let [SessionUpdate::AgentMessageChunk(chunk)] = updates.as_slice() else {
            panic!("expected one empty reply, got {updates:?}");
        };
        assert!(matches!(&chunk.content, ContentBlock::Text(text) if text.text.is_empty()));
        assert_eq!(
            chunk.meta.as_ref().unwrap()["rustcode"]["turn"]["outcome"],
            "cancelled"
        );
    }

    #[test]
    fn replayed_stop_marker_is_a_timed_empty_reply_or_its_notice() {
        let mut cancelled =
            crate::app::ChatMessage::new("assistant", "[harness: turn stopped — cancelled]");
        cancelled.turn = Some(crate::app::TurnTiming {
            turn_id: "t1".into(),
            started_at: Some("2026-10-30T13:42:27Z".into()),
            ended_at: Some("2026-10-30T13:42:37Z".into()),
            elapsed_work_ms: Some(9_700),
            outcome: Some(crate::app::TurnOutcome::Cancelled),
        });
        let stopped =
            crate::app::ChatMessage::new("assistant", "[harness: turn stopped — loop_escalation]");
        let updates = replay_history_updates(&[cancelled, stopped], None);
        let texts: Vec<(String, Option<String>)> = updates
            .iter()
            .filter_map(|update| match update {
                SessionUpdate::AgentMessageChunk(chunk) => match &chunk.content {
                    ContentBlock::Text(text) => Some((
                        text.text.clone(),
                        chunk.meta.as_ref().and_then(|meta| {
                            meta["rustcode"]["turn"]["outcome"]
                                .as_str()
                                .map(str::to_owned)
                        }),
                    )),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        assert_eq!(
            texts,
            vec![
                (String::new(), Some("cancelled".to_owned())),
                ("Stopped: the agent was repeating itself".to_owned(), None),
            ]
        );
    }

    #[test]
    fn replayed_reply_carries_its_finished_turn_timing() {
        let turn = crate::app::TurnTiming {
            turn_id: "turn:1".into(),
            started_at: Some("2026-10-22T20:58:02+00:00".into()),
            ended_at: Some("2026-10-22T20:59:52+00:00".into()),
            elapsed_work_ms: Some(109_582),
            outcome: Some(crate::app::TurnOutcome::Completed),
        };
        let mut reply = crate::app::ChatMessage::new("assistant", "<think>Hm.</think>Done.");
        reply.turn = Some(turn.clone());
        let mut open = crate::app::ChatMessage::new("assistant", "Still going.");
        open.timestamp = String::new();
        open.turn = Some(crate::app::TurnTiming {
            ended_at: None,
            outcome: None,
            ..turn
        });

        let updates = replay_history_updates(&[reply, open], None);

        let metas = updates
            .iter()
            .filter_map(|update| match update {
                SessionUpdate::AgentMessageChunk(chunk) => Some(chunk.meta.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(metas.len(), 2);
        assert_eq!(
            metas[0].as_ref().unwrap()["rustcode"]["turn"],
            serde_json::json!({
                "id": "turn:1",
                "startedAt": "2026-10-22T20:58:02+00:00",
                "endedAt": "2026-10-22T20:59:52+00:00",
                "elapsedMs": 109_582,
                "outcome": "completed",
            })
        );
        assert!(metas[1].is_none());
    }

    // #2461, #2463: replayed messages say when they were sent and written.
    #[test]
    fn replayed_messages_carry_their_time() {
        let mut question = crate::app::ChatMessage::new("user", "Why?");
        question.timestamp = "2026-10-27T09:16:01.473007+01:00".into();
        let mut answer = crate::app::ChatMessage::new("assistant", "<think>Hm.</think>Because.");
        answer.timestamp = "2026-10-27T09:16:16.043688+01:00".into();

        let updates = replay_history_updates(&[question, answer], None);

        let metas = updates
            .iter()
            .filter_map(|update| match update {
                SessionUpdate::UserMessageChunk(chunk)
                | SessionUpdate::AgentMessageChunk(chunk) => Some(chunk.meta.clone().unwrap()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            metas
                .iter()
                .map(|meta| meta["rustcode"]["at"].clone())
                .collect::<Vec<_>>(),
            vec![
                serde_json::json!("2026-10-27T09:16:01.473007+01:00"),
                serde_json::json!("2026-10-27T09:16:16.043688+01:00"),
            ]
        );
    }

    // #2299: a replayed turn sends its reasoning as thought chunks with the
    // label and duration it had live, not as `<think>` inside message text.
    #[test]
    fn replayed_reasoning_is_sent_as_labelled_thoughts() {
        let mut summarized = crate::app::ChatMessage::new(
            "assistant",
            "<think>\n**Inspecting site**\n</think>\n\nDone.",
        );
        summarized.reasoning_summary = true;
        summarized.thought_time_ms = Some(4000);
        let mut reasoned =
            crate::app::ChatMessage::new("assistant", "<think>Let me check.</think>Fine.");
        reasoned.thought_time_ms = Some(0);

        let updates = replay_history_updates(&[summarized, reasoned], None);

        assert_eq!(
            thought_chunks(&updates),
            vec![
                (
                    "**Inspecting site**\n".to_string(),
                    serde_json::json!({"thoughtSource": "summary", "thoughtMs": 4000})
                ),
                (
                    "Let me check.".to_string(),
                    serde_json::json!({"thoughtSource": "raw"})
                ),
            ]
        );
        let prose = updates
            .iter()
            .filter_map(|update| match update {
                SessionUpdate::AgentMessageChunk(chunk) => match &chunk.content {
                    ContentBlock::Text(text) => Some(text.text.clone()),
                    _ => None,
                },
                _ => None,
            })
            .collect::<String>();
        assert_eq!(prose, "\n\nDone.Fine.");
        assert!(!prose.contains("<think>"));
    }

    #[test]
    fn replay_shows_the_compaction_card_where_the_summary_is() {
        let summary = crate::network::compaction::durable_compaction_record_message(
            "- Goal: ship the fix",
            &[],
            "/tmp/archive.jsonl",
        );
        let notice = crate::app::ChatMessage::new(
            "system",
            "🧹 History compacted: reduced context from 161495 to 4200 tokens.",
        );
        let stats = crate::app::CompactionStats::new(
            crate::app::CompactionTrigger::Manual,
            crate::app::CompactionMode::AiSummary,
            161_495,
            4_200,
            116,
            2,
        );
        let history = [
            summary,
            crate::app::ChatMessage::new("user", "next"),
            notice,
        ];

        let updates = replay_history_updates_with_compaction(&history, None, Some(&stats));

        let [
            SessionUpdate::AgentMessageChunk(card),
            SessionUpdate::UserMessageChunk(_),
        ] = updates.as_slice()
        else {
            panic!("expected the card, then the user message: {updates:?}");
        };
        let meta = &card.meta.as_ref().expect("card meta")["rustcode"];
        assert_eq!(meta["kind"], "compaction");
        assert_eq!(meta["summary"], "- Goal: ship the fix");
        assert_eq!(meta["count"], 3);
        assert_eq!(meta["beforeTokens"], 161_495);
    }

    #[test]
    fn acp_event_stream_carries_tool_detail_and_live_output() {
        let mut stream = AcpEventStream::new();
        let started = stream.updates(crate::network::AgentUiEvent::ToolStarted {
            detail: Some("cargo test".to_string()),
            summary: Some("Running the tests".to_string()),
            name: "run_command".to_string(),
            id: "call-1".to_string(),
        });
        let [SessionUpdate::ToolCall(call)] = started.as_slice() else {
            panic!("expected one tool call, got {started:?}");
        };
        assert_eq!(
            call.meta.as_ref().unwrap()["rustcode"]["detail"],
            "cargo test"
        );
        assert_eq!(
            call.meta.as_ref().unwrap()["rustcode"]["summary"],
            "Running the tests"
        );

        let output = stream.updates(crate::network::AgentUiEvent::ToolOutput {
            id: "call-1".to_string(),
            text: "running 3 tests\n".to_string(),
        });
        let [SessionUpdate::ToolCallUpdate(update)] = output.as_slice() else {
            panic!("expected one tool update, got {output:?}");
        };
        assert_eq!(update.tool_call_id.to_string(), "call-1");
        assert_eq!(update.fields.status, None);
        assert_eq!(
            update.meta.as_ref().unwrap()["rustcode"]["outputDelta"],
            "running 3 tests\n"
        );
    }

    #[test]
    fn acp_event_stream_reports_tool_start_and_completion() {
        let mut stream = AcpEventStream::new();
        let started = stream.updates(crate::network::AgentUiEvent::ToolStarted {
            detail: None,
            summary: None,
            name: "grep".to_string(),
            id: "call-1".to_string(),
        });
        assert!(matches!(
            started.as_slice(),
            [SessionUpdate::ToolCall(call)]
                if call.tool_call_id.to_string() == "call-1"
                    && call.status == agent_client_protocol::schema::v1::ToolCallStatus::InProgress
        ));

        let pending = stream.updates(crate::network::AgentUiEvent::ToolFinished {
            id: "call-1".to_string(),
            result: crate::network::events::ToolResult {
                tool_name: "run_command".to_string(),
                content: "Task started in background".to_string(),
                diff: None,
                file_preview: None,
                metadata: crate::network::events::ToolResultMetadata {
                    execution_us: 0,
                    workspace_generation: None,
                    pending: true,
                    call_id: Some("call-1".to_string()),
                    arguments_hash: "hash".to_string(),
                    ..Default::default()
                },
            },
        });
        assert!(matches!(
            pending.as_slice(),
            [SessionUpdate::ToolCallUpdate(update)]
                if update.tool_call_id.to_string() == "call-1"
                    && update.fields.status
                        == Some(agent_client_protocol::schema::v1::ToolCallStatus::InProgress)
        ));

        let finished = stream.updates(crate::network::AgentUiEvent::ToolFinished {
            id: "call-1".to_string(),
            result: crate::network::events::ToolResult {
                tool_name: "grep".to_string(),
                content: "match".to_string(),
                diff: None,
                file_preview: None,
                metadata: crate::network::events::ToolResultMetadata {
                    execution_us: 0,
                    workspace_generation: None,
                    pending: false,
                    command: None,
                    call_id: Some("call-1".to_string()),
                    arguments_hash: "hash".to_string(),
                    success: true,
                    exit_code: None,
                    changed_paths: Vec::new(),
                    truncated: false,
                    payload_truncated: false,
                    completeness: rustcode_core::ToolResultCompleteness::Complete,
                    full_output_artifact: None,
                    replayed: false,
                    error_kind: None,
                    retryable: false,
                    inspection: None,
                    command_status: None,
                },
            },
        });
        assert!(matches!(
            finished.as_slice(),
            [SessionUpdate::ToolCallUpdate(update)]
                if update.tool_call_id.to_string() == "call-1"
                    && update.fields.status
                        == Some(agent_client_protocol::schema::v1::ToolCallStatus::Completed)
        ));
    }

    #[test]
    fn acp_stop_reason_preserves_cancellation_and_turn_limits() {
        assert_eq!(
            acp_stop_reason(true, Some("provider_error")),
            StopReason::Cancelled
        );
        assert_eq!(
            acp_stop_reason(false, Some("budget:max_tool_rounds")),
            StopReason::MaxTurnRequests
        );
        assert_eq!(
            acp_stop_reason(false, Some("loop_escalation")),
            StopReason::MaxTurnRequests
        );
        assert_eq!(
            acp_stop_reason(false, Some("output_limit:default:8192:8192")),
            StopReason::MaxTokens
        );
        assert_eq!(
            acp_stop_reason(false, Some("completed")),
            StopReason::EndTurn
        );
    }

    #[test]
    fn acp_session_new_returns_model_option_with_all_profiles_and_default_big_selected() {
        let state = crate::app::AppState::new();
        let options = build_session_config_options(&state);

        let ids: Vec<_> = options.iter().map(|option| option.id.0.as_ref()).collect();
        assert_eq!(ids, ["model", "tool_protocol"]);
        let model_opt = &options[0];
        assert_eq!(model_opt.id.0.as_ref(), "model");
        assert_eq!(model_opt.name, "Model");
        assert_eq!(model_opt.category, Some(SessionConfigOptionCategory::Model));

        match &model_opt.kind {
            SessionConfigKind::Select(select) => {
                assert_eq!(
                    select.current_value.0.as_ref(),
                    state.config.default.big(),
                    "default.big profile must be initially selected"
                );
                match &select.options {
                    SessionConfigSelectOptions::Ungrouped(opts) => {
                        assert_eq!(opts.len(), state.config.models.len());
                        for (opt, profile) in opts.iter().zip(state.config.models.iter()) {
                            assert_eq!(opt.value.0.as_ref(), profile.name);
                            assert_eq!(opt.name, profile.name);
                            if profile.model.is_empty() {
                                assert_eq!(opt.description, None);
                            } else {
                                assert_eq!(
                                    opt.description.as_deref(),
                                    Some(profile.model.as_str())
                                );
                            }
                        }
                    }
                    _ => panic!("expected ungrouped select options"),
                }
            }
            _ => panic!("expected select config option kind"),
        }
    }

    #[test]
    fn acp_new_session_response_contains_config_options() {
        let state = crate::app::AppState::new();
        let options = build_session_config_options(&state);
        let resp = NewSessionResponse::new("test-session").config_options(options);
        assert!(resp.config_options.is_some());
        let config_opts = resp.config_options.unwrap();
        assert_eq!(config_opts.len(), 2);
        assert_eq!(config_opts[0].id.0.as_ref(), "model");
    }

    /// The macOS app (`macos/Sources/RustCodeKit/ACPTypes.swift`,
    /// `ModelOptions`) reads exactly these wire fields.
    #[test]
    fn acp_model_config_option_wire_shape_matches_the_macos_client() {
        let state = crate::app::AppState::new();
        let value = serde_json::to_value(build_session_config_options(&state)).unwrap();
        let model = &value[0];
        assert_eq!(model["id"], "model");
        assert_eq!(model["type"], "select");
        assert_eq!(model["currentValue"], state.config.default.big());
        let options = model["options"].as_array().unwrap();
        assert_eq!(options.len(), state.config.models.len());
        for (option, profile) in options.iter().zip(state.config.models.iter()) {
            assert_eq!(option["value"], profile.name.as_str());
            assert_eq!(option["name"], profile.name.as_str());
        }
    }

    #[test]
    fn acp_selecting_valid_model_updates_session_endpoint_and_model() {
        let mut state = crate::app::AppState::new();
        let target_profile = state
            .config
            .models
            .iter()
            .find(|m| m.name != state.config.default.big())
            .expect("should have at least one non-default model profile")
            .clone();

        let updated_options = handle_set_config_option(
            &mut state,
            &SessionConfigId::new("model"),
            &SessionConfigOptionValue::value_id(target_profile.name.clone()),
        )
        .expect("selecting a valid model should succeed");

        assert_eq!(state.api_base_url, target_profile.url);
        assert_eq!(state.model_name, target_profile.model);

        // Model first; an effort option follows when the profile has choices.
        assert!(!updated_options.is_empty());
        match &updated_options[0].kind {
            SessionConfigKind::Select(select) => {
                assert_eq!(select.current_value.0.as_ref(), target_profile.name);
            }
            _ => panic!("expected select config option kind"),
        }
    }

    #[test]
    fn acp_offers_and_sets_reasoning_effort_for_profiles_with_choices() {
        let mut state = crate::app::AppState::new();
        let active = state
            .active_model_profile()
            .expect("an active profile")
            .name;
        let profile = state
            .config
            .models
            .iter_mut()
            .find(|profile| profile.name == active)
            .unwrap();
        profile.supports_reasoning_effort = Some(true);
        profile.reasoning_efforts = Some(vec!["low".into(), "high".into()]);
        profile.reasoning_effort = None;

        let effort = |options: &[agent_client_protocol::schema::v1::SessionConfigOption]| {
            options
                .iter()
                .find(|option| option.id.0.as_ref() == "effort")
                .map(|option| match &option.kind {
                    SessionConfigKind::Select(select) => select.current_value.0.to_string(),
                    _ => panic!("expected a select"),
                })
        };
        assert_eq!(
            effort(&build_session_config_options(&state)).as_deref(),
            Some("default")
        );

        let updated = handle_set_config_option(
            &mut state,
            &SessionConfigId::new("effort"),
            &SessionConfigOptionValue::value_id("high"),
        )
        .expect("a listed effort is accepted");
        assert_eq!(effort(&updated).as_deref(), Some("high"));

        assert!(
            handle_set_config_option(
                &mut state,
                &SessionConfigId::new("effort"),
                &SessionConfigOptionValue::value_id("ludicrous"),
            )
            .is_err()
        );
    }

    #[test]
    fn acp_selecting_unknown_model_returns_invalid_params() {
        let mut state = crate::app::AppState::new();
        let initial_url = state.api_base_url.clone();
        let initial_model = state.model_name.clone();

        let err = handle_set_config_option(
            &mut state,
            &SessionConfigId::new("model"),
            &SessionConfigOptionValue::value_id("non_existent_profile_12345"),
        )
        .unwrap_err();

        assert_eq!(err.code, agent_client_protocol::ErrorCode::InvalidParams);
        assert_eq!(state.api_base_url, initial_url);
        assert_eq!(state.model_name, initial_model);
    }

    #[test]
    fn acp_selecting_unknown_config_id_returns_invalid_params() {
        let mut state = crate::app::AppState::new();
        let err = handle_set_config_option(
            &mut state,
            &SessionConfigId::new("unsupported_config"),
            &SessionConfigOptionValue::value_id("gemini-3.6-flash"),
        )
        .unwrap_err();

        assert_eq!(err.code, agent_client_protocol::ErrorCode::InvalidParams);
    }

    #[test]
    fn acp_selecting_boolean_value_for_model_returns_invalid_params() {
        let mut state = crate::app::AppState::new();
        let err = handle_set_config_option(
            &mut state,
            &SessionConfigId::new("model"),
            &SessionConfigOptionValue::boolean(true),
        )
        .unwrap_err();

        assert_eq!(err.code, agent_client_protocol::ErrorCode::InvalidParams);
    }

    #[test]
    fn acp_two_sessions_can_select_different_models_independently() {
        let mut state1 = crate::app::AppState::new();
        let mut state2 = crate::app::AppState::new();

        assert!(
            state1.config.models.len() >= 2,
            "need at least 2 profiles to test independence"
        );
        let profile1 = state1.config.models[0].clone();
        let profile2 = state1.config.models[1].clone();

        handle_set_config_option(
            &mut state1,
            &SessionConfigId::new("model"),
            &SessionConfigOptionValue::value_id(profile1.name.clone()),
        )
        .expect("setting model for session 1");

        handle_set_config_option(
            &mut state2,
            &SessionConfigId::new("model"),
            &SessionConfigOptionValue::value_id(profile2.name.clone()),
        )
        .expect("setting model for session 2");

        assert_eq!(state1.api_base_url, profile1.url);
        assert_eq!(state1.model_name, profile1.model);

        assert_eq!(state2.api_base_url, profile2.url);
        assert_eq!(state2.model_name, profile2.model);
    }
}
