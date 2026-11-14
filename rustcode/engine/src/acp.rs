mod agents;
mod capabilities;
mod clients;
mod commands;
mod config;
mod connections;
mod cursor;
pub(crate) mod data;
mod edit_diff;
mod environment;
mod harnesses;
mod hosted;
mod mcp_app;
mod mirror;
mod outside;
mod permissions;
mod presence;
mod preview;
mod projects;
mod prompt;
pub(crate) mod questions;
mod remote;
mod rewind;
mod scheduled;
mod server_status;
mod session;
pub(crate) mod session_move;
mod session_repair;
pub(crate) mod settings;
pub(crate) mod steer;
mod streaming;
mod telemetry;
mod themes;
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
use agent_client_protocol::{Agent, Stdio};
use rustcode_tasks::TaskEvent;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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
                "sessionRepair": 1,
                "sessionActivity": 1,
                "childActivity": 1,
                "pullRequests": 3,
                "worktrees": 1,
                "checkpoints": 2,
                "threadStatus": 1,
                // `_meta.rustcode.relationship` on `session/new`, and the same in
                // `session/list`: a client that opens forks and side chats checks it.
                "lineage": 1,
            }),
        )]))
}

/// `_rustcode/rewind` for a checkpoint restore that also rewinds the chat.
pub(crate) fn rewind_chat(
    state: &mut crate::app::AppState,
    user_index: usize,
    text: &str,
) -> Result<serde_json::Value, agent_client_protocol::Error> {
    rewind::rewind(state, user_index, text)
}

fn session_id_is_active(sessions: &HashMap<String, AcpSession>, session_id: &str) -> bool {
    sessions.contains_key(session_id)
}

fn workspace_cwd(
    workspace: Option<rustcode_session::SessionWorkspace>,
) -> Option<std::path::PathBuf> {
    // `canonicalize` returns the verbatim form on Windows, and a thread whose
    // folder is stored that way is a second project with the same name (#3425).
    let cwd = rustcode_session::strip_verbatim(&workspace?.cwd);
    let canonical = std::fs::canonicalize(cwd).ok()?;
    canonical
        .is_dir()
        .then(|| rustcode_session::strip_verbatim(&canonical))
}

/// The user's home folder, without the verbatim prefix a canonical Windows path
/// carries; `None` when this computer has none.
fn home_folder() -> Option<std::path::PathBuf> {
    crate::service::home_dir()
        .ok()
        .map(|home| rustcode_session::strip_verbatim(&home))
}

/// The folder a thread starts in when a request names none: the user's home
/// folder. A start with no folder comes from an orchestrator that has no
/// project on this computer, and the engine's own working directory is `/`
/// under a service manager or the app's install folder on Windows. Neither is a
/// project, and a thread that ran there said "Working directory: /" (#3425).
fn start_folder(
    requested: &std::path::Path,
    home: Option<&std::path::Path>,
) -> Option<std::path::PathBuf> {
    if requested.as_os_str().is_empty() {
        return home.map(std::path::Path::to_path_buf);
    }
    Some(requested.to_path_buf())
}

/// The folder a listing shows for a thread: what its workspace recorded, else
/// the user's home folder, never the engine's own working directory. The bool
/// says the folder is a stand-in, not the thread's project.
fn listed_folder(
    recorded: Option<std::path::PathBuf>,
    home: Option<&std::path::Path>,
) -> (std::path::PathBuf, bool) {
    match recorded {
        Some(cwd) => (cwd, false),
        None => (
            home.map(std::path::Path::to_path_buf).unwrap_or_default(),
            true,
        ),
    }
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
    let (cwd, workspace_unknown) =
        listed_folder(workspace_cwd(workspace), home_folder().as_deref());
    let mut info = SessionInfo::new(session_id.clone(), cwd).title(meta.title.clone());
    let mut extra = serde_json::Map::new();
    extra.insert("messageCount".into(), serde_json::json!(meta.message_count));
    extra.insert(
        "workspaceUnknown".into(),
        serde_json::json!(workspace_unknown),
    );
    let metadata = crate::config::load_session_metadata(&session_id);
    if let Some(created_at_ms) = metadata.as_ref().map(|metadata| metadata.created_at_ms)
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
    // A thread on an external harness names it (`docs/harnesses.md`).
    if let Some((rustcode, model)) = harnesses::list_meta(&session_id) {
        extra.insert("rustcode".into(), rustcode);
        if let Some(model) = model {
            extra.insert("model".into(), serde_json::json!(model));
        }
    }
    if let Some(timestamp) = session_updated_at(meta) {
        info = info.updated_at(timestamp.to_rfc3339());
    }
    // Orchestrator v2: the thread's status and worktree (`docs/orchestrator.md`).
    let status = crate::thread_status::current(&session_id).as_str();
    extra = crate::worktrees::merge_rustcode_meta(Some(extra), "status", status.into());
    extra = crate::worktrees::merge_rustcode_meta(
        Some(extra),
        "snoozedUntil",
        serde_json::json!(crate::thread_status::snoozed_until(&session_id)),
    );
    let pinned = crate::thread_status::pinned(&session_id);
    extra = crate::worktrees::merge_rustcode_meta(Some(extra), "pinned", pinned.into());
    if let Some(worktree) = crate::worktrees::list_meta(&session_id) {
        extra = crate::worktrees::merge_rustcode_meta(Some(extra), "worktree", worktree);
    }
    // The repository the thread's folder belongs to: a main checkout and every
    // worktree of it are one project for the orchestration tools (#3150). It
    // was recorded when the thread was created or attached, so listing threads
    // never runs git (#3162). A thread whose workspace is unknown inherits the
    // engine's cwd, which is not its project: it publishes no repository.
    if !workspace_unknown
        && let Some(repo) = crate::worktrees::recorded_repo(&session_id, metadata.as_ref())
    {
        extra = crate::worktrees::merge_rustcode_meta(Some(extra), "repo", serde_json::json!(repo));
    }
    // The thread an agent started this one from, kept in its own metadata so a
    // pruned link cannot turn it into a top-level thread (`thread_parent.rs`).
    if let Some(parent) = metadata.and_then(|metadata| metadata.parent) {
        let mut value = serde_json::Map::new();
        if let Some(id) = parent.id {
            value.insert("sessionId".into(), serde_json::json!(id));
        }
        if let Some(title) = parent.title {
            value.insert("title".into(), serde_json::json!(title));
        }
        if !value.is_empty() {
            // A record without one is a delegate; the app reads the same default.
            value.insert(
                "relationship".into(),
                serde_json::json!(
                    parent
                        .relationship
                        .unwrap_or(rustcode_session::SessionRelationship::Delegate)
                        .as_str()
                ),
            );
            extra = crate::worktrees::merge_rustcode_meta(Some(extra), "parent", value.into());
        }
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

pub(crate) fn replay_history_updates(
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
            // A user request says who sent it (`_meta.rustcode.origin`); a
            // tool's result or attachment in the user role has no sender.
            "user" if !message.content.is_empty() && message.is_user_request() => {
                updates.push(SessionUpdate::UserMessageChunk(
                    crate::message_origin::user_chunk(message.content.clone(), {
                        let mut meta =
                            streaming::message_time_meta(&message.timestamp).unwrap_or_default();
                        if let Some(origin) = &message.origin {
                            meta.insert(
                                "rustcode".to_owned(),
                                serde_json::json!({"origin": origin}),
                            );
                        }
                        Some(meta)
                    }),
                ))
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
    // Stored and reported as people write it: the verbatim prefix `canonicalize`
    // adds on Windows is a Win32 detail, and a thread that keeps it counts as a
    // second project with the same name (#3425).
    Ok(rustcode_session::strip_verbatim(&canonical))
}

/// Whether a thread recorded in `recorded` opens in `cwd`, a folder
/// `canonical_acp_cwd` resolved. The record may carry Windows' verbatim
/// prefix (`\\?\C:\...`), which `canonicalize` keeps and `cwd` does not, so
/// both are compared without it.
pub(crate) fn opens_in(recorded: &std::path::Path, cwd: &std::path::Path) -> bool {
    std::fs::canonicalize(recorded)
        .ok()
        .is_some_and(|path| rustcode_session::strip_verbatim(&path) == cwd)
}

/// `session/new`'s folder: the one the request names, else the user's home
/// folder, resolved and checked to be a directory.
fn session_new_cwd(
    requested: &std::path::Path,
) -> Result<std::path::PathBuf, agent_client_protocol::Error> {
    let home = home_folder();
    match start_folder(requested, home.as_deref()) {
        Some(folder) => canonical_acp_cwd(&folder, "session/new"),
        None => Err(agent_client_protocol::Error::invalid_params().data(
            "session/new has no folder: this computer has no project named and no home folder",
        )),
    }
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

/// Give a session the client half of what its client advertised: how replies
/// are drawn, and the preview, desktop, orchestration and Smart UI requests.
/// Read once, from the client that created or loaded it, so its prompt never
/// changes mid-session; each request goes to whichever client has the
/// session then.
fn apply_client_info(
    state: &mut crate::app::AppState,
    info: clients::ClientInfo,
    clients: &clients::SessionClients,
) {
    state.visualization_level = info.visualization_level;
    state.smart_ui = info.smart_ui;
    state.preview_client = preview::preview_client_for(info.preview, clients);
    state.desktop_client = preview::desktop_client_for(info.desktop, clients);
    (state.orchestration_client, state.orchestration_version) =
        preview::orchestration_for(info.orchestration, clients);
    // No app hosting the tools: this engine serves the reads and names the app
    // for the rest (#3422). An app that turned them off keeps them off.
    state.local_orchestration = info.orchestration == 0 && !clients.app_attached();
    state.smart_ui_client = preview::smart_ui_client_for(
        info.smart_ui == crate::app::SmartUi::On && info.smart_ui_preview,
        clients,
    );
}

/// The session `session/new` creates on the built-in harness: the engine's own
/// chat state, its task route and the client it publishes to. Also what a
/// thread a paired device asked for starts as (`super::hosted`), which is why
/// it is a function of its own.
async fn create_builtin_session(
    server: &AcpServer,
    peer: &clients::Peer,
    request: NewSessionRequest,
) -> Result<NewSessionResponse, agent_client_protocol::Error> {
    let cwd = session_new_cwd(&request.cwd)?;
    let mut state = new_session_state(&cwd);
    let session_id = state.active_session_id.clone();
    // The name the thread was made with, from the app that made it (issue 3524).
    crate::config::save_requested_title(&session_id, request.meta.as_ref());
    // Who started this thread, from the `_meta.rustcode.parent` its
    // caller gave: kept in the thread's own metadata, not only in
    // the link book the app prunes (`thread_parent.rs`).
    if let Some((parent, relationship)) = crate::thread_parent::requested(request.meta.as_ref()) {
        crate::thread_parent::record(&session_id, &parent, relationship);
    }
    let session_clients = peer.attach(&session_id);
    apply_client_info(&mut state, peer.info(), &session_clients);
    state.raw_cli_mode = false;
    state.task_working_directory = Some(cwd.clone());
    // ACP's requested cwd is both the task's initial location
    // and its hard filesystem boundary, independent of the
    // directory from which RustCode itself was launched.
    state.workspace_root = Some(cwd.clone());
    // What the request asked for, else the built-in harness's
    // remembered choice (docs/harnesses.md).
    apply_requested_model_and_effort(
        &mut state,
        &crate::harness::builtin_start(&crate::harness::requested(request.meta.as_ref())),
    );
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
        state: Arc::clone(&state),
        clients: session_clients,
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
    // Commands, then the session's opening usage, so the client
    // shows its context meter from the start rather than only
    // after the first turn ends (#2414 follow-up).
    let opening_usage = {
        let state = state.lock().await;
        usage::usage_update(&state)
    };
    for update in [commands::available_commands_update(), opening_usage] {
        if let Err(error) = peer.send_snapshot(&session_id, update) {
            unregister_acp_session(server, &session_id).await;
            return Err(agent_client_protocol::Error::internal_error().data(error.to_string()));
        }
    }
    Ok(NewSessionResponse::new(session_id)
        .config_options(config_options)
        .meta(crate::harness::builtin_session_meta()))
}

async fn acp_load_session(
    server: &AcpServer,
    mut request: LoadSessionRequest,
    peer: &clients::Peer,
) -> Result<LoadSessionResponse, agent_client_protocol::Error> {
    let session_id = request.session_id.to_string();
    if !valid_acp_session_id(&session_id) {
        return Err(agent_client_protocol::Error::invalid_params().data("invalid session id"));
    }
    // A thread whose worktree is gone opens in the repository it came from,
    // instead of failing on a folder that no longer exists.
    if let Some(cwd) = crate::worktrees::recover_missing_worktree(&session_id, &request.cwd) {
        request.cwd = cwd;
    }
    // Open here already, from another client or one that has gone: this
    // client joins it, mid-turn or not.
    if let Some(external) = harnesses::get(&server.external, &session_id).await {
        return harnesses::attach_open(&external, &request, peer).await;
    }
    if let Some(response) = attach_open_builtin(server, &request, peer).await? {
        return Ok(response);
    }
    if let Some(record) = crate::harness::store::load_for_restore(&session_id)
        .map_err(|error| agent_client_protocol::Error::invalid_params().data(error))?
    {
        if session_id_is_active(&*server.sessions.lock().await, &session_id) {
            return Err(
                agent_client_protocol::Error::invalid_params().data("session is already active")
            );
        }
        return harnesses::load_session(&server.external, &request, record, peer, &server.remote)
            .await;
    }
    let cwd = canonical_acp_cwd(&request.cwd, "session/load")?;
    // Where the session's log is before its transcript is read: whatever it
    // logs after this is not in the transcript.
    let base = server.clients.load_mark(&session_id);
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
        && !opens_in(&workspace.cwd, &cwd)
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
        // Not attached yet: the replay below attaches the loading client in
        // the same step that queues the transcript to it, so no live update
        // can land between its chunks.
        let session_clients = peer.clients().session(&session_id);
        apply_client_info(&mut state, peer.info(), &session_clients);
        let config_options = build_session_config_options(&state);
        let loaded_usage = usage::usage_update(&state);
        let loaded_compaction = state.last_compaction.clone();
        let state = Arc::new(Mutex::new(state));
        let (task_sender, task_receiver) = std::sync::mpsc::sync_channel(64);
        let known_task_ids = Arc::new(std::sync::Mutex::new(KnownTaskIds::default()));
        let terminal_backlog = Arc::new(std::sync::Mutex::new(VecDeque::new()));
        let terminal_overflow = Arc::new(AtomicBool::new(false));
        let task_sink = Arc::new(SessionTaskSink {
            state: Arc::clone(&state),
            clients: session_clients,
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
        // It asked to resume a session that is not open here (the engine
        // restarted, or it was closed): the transcript replaces what it has.
        let reset = cursor::requested_cursor(&request).is_some();
        let mut snapshot = replay;
        if !meta.title.trim().is_empty() {
            snapshot.push(SessionUpdate::SessionInfoUpdate(
                SessionInfoUpdate::new().title(meta.title.clone()),
            ));
        }
        snapshot.push(commands::available_commands_update());
        snapshot.push(loaded_usage);
        load_pause::here(&session_id).await;
        // Queued and attached under one lock (see `Peer::attach_closed`):
        // the transcript, then what the session logged since `base`.
        match peer.attach_closed(&session_id, base, snapshot, reset) {
            Ok(Some(_)) => {}
            Ok(None) => {
                unregister_acp_session(server, &session_id).await;
                return Err(agent_client_protocol::Error::internal_error()
                    .data("session changed while loading; retry session/load"));
            }
            Err(error) => {
                unregister_acp_session(server, &session_id).await;
                return Err(agent_client_protocol::Error::internal_error().data(error.to_string()));
            }
        }
        return Ok(peer.position(&session_id, reset).finish(
            LoadSessionResponse::new()
                .config_options(config_options)
                .meta(crate::harness::builtin_session_meta()),
        ));
    }
}

/// A test stops a closed-session load after the session is open and before
/// the loading client is attached, to do what another client could do there.
#[cfg(test)]
pub(crate) mod load_pause {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use tokio::sync::Notify;

    type Gate = (Arc<Notify>, Arc<Notify>);

    static GATES: Mutex<Option<HashMap<String, Gate>>> = Mutex::new(None);

    /// The load of `session_id` stops at its next [`here`]: `reached` says it
    /// is there, `go` lets it on.
    pub(crate) fn arm(session_id: &str) -> Gate {
        let gate = (Arc::new(Notify::new()), Arc::new(Notify::new()));
        GATES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_or_insert_with(HashMap::new)
            .insert(session_id.to_owned(), gate.clone());
        gate
    }

    pub(crate) async fn here(session_id: &str) {
        let gate = GATES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_mut()
            .and_then(|gates| gates.remove(session_id));
        if let Some((reached, go)) = gate {
            reached.notify_one();
            go.notified().await;
        }
    }
}

#[cfg(not(test))]
pub(crate) mod load_pause {
    #[inline]
    pub(crate) async fn here(_session_id: &str) {}
}

/// `session/load` of a built-in session already open in this engine: `peer`
/// joins it. `None` when it is not open.
async fn attach_open_builtin(
    server: &AcpServer,
    request: &LoadSessionRequest,
    peer: &clients::Peer,
) -> Result<Option<LoadSessionResponse>, agent_client_protocol::Error> {
    let session_id = request.session_id.to_string();
    let Some((state, cwd)) = server.session_handle(&session_id).await else {
        return Ok(None);
    };
    if canonical_acp_cwd(&request.cwd, "session/load")? != cwd {
        return Err(agent_client_protocol::Error::invalid_params()
            .data("session/load cwd does not match the open session"));
    }
    let compaction = state.lock().await.last_compaction.clone();
    let position = attach_with_replay(
        peer,
        &session_id,
        cursor::requested_cursor(request),
        || {
            let state = Arc::clone(&state);
            async move { state.lock().await.history.clone() }
        },
        |history| replay_history_updates_with_compaction(history, Some(&cwd), compaction.as_ref()),
    )
    .await?;
    let (config_options, usage) = {
        let state = state.lock().await;
        (
            build_session_config_options(&state),
            usage::usage_update(&state),
        )
    };
    // A resumed client has these already: changes to them were updates.
    if !position.resumed {
        let title = crate::config::session_meta_by_id(&session_id)
            .map(|meta| meta.title)
            .filter(|title| !title.trim().is_empty())
            .map(|title| SessionUpdate::SessionInfoUpdate(SessionInfoUpdate::new().title(title)));
        for update in title
            .into_iter()
            .chain([commands::available_commands_update(), usage])
        {
            let _ = peer.send_snapshot(&session_id, update);
        }
    }
    let position = peer.position(&session_id, position.reset);
    Ok(Some(crate::worktrees::finish_load(
        position.finish(
            LoadSessionResponse::new()
                .config_options(config_options)
                .meta(crate::harness::builtin_session_meta()),
        ),
        &session_id,
    )))
}

/// How often a replay is rebuilt when a turn starts or ends while it is
/// built. If it keeps changing, fail without attaching a stale transcript.
const ATTACH_ATTEMPTS: usize = 4;

/// Attach `peer` to the open session `session_id` and replay it: the
/// transcript up to the running turn, what the turn sent so far, then every
/// update live, with nothing lost or sent twice between them.
///
/// `history` reads the session's transcript; `replay` turns a prefix of it
/// into updates. While a turn runs only the messages from before it are
/// replayed: the rest is the turn's, and the turn's own record has it.
///
/// With `resume` (the cursor a reconnecting client last saw, see [`cursor`])
/// it is first tried to send only what came after; when the session's log no
/// longer reaches back that far the transcript is sent after a reset instead.
/// Answers the position to put in the `session/load` response.
pub(crate) async fn attach_with_replay<F>(
    peer: &clients::Peer,
    session_id: &str,
    resume: Option<u64>,
    history: impl Fn() -> F,
    replay: impl Fn(&[crate::app::ChatMessage]) -> Vec<SessionUpdate>,
) -> Result<cursor::Position, agent_client_protocol::Error>
where
    F: std::future::Future<Output = rustcode_core::History>,
{
    if let Some(position) = resume
        .map(|cursor| peer.resume_open(session_id, cursor))
        .transpose()?
        .flatten()
    {
        return Ok(position);
    }
    for _ in 0..ATTACH_ATTEMPTS {
        let (generation, base) = peer.clients().turn_marker(session_id);
        let transcript = history().await;
        let transcript = transcript.as_slice();
        let before_turn = &transcript[..base.unwrap_or(transcript.len()).min(transcript.len())];
        if let Some(position) = peer.attach_open(
            session_id,
            Some(generation),
            replay(before_turn),
            resume.is_some(),
        )? {
            return Ok(position);
        }
    }
    Err(agent_client_protocol::Error::internal_error()
        .data("session changed repeatedly while loading; retry session/load"))
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
    state: Arc<Mutex<crate::app::AppState>>,
    /// Where the session's updates go: the clients that have it.
    clients: clients::SessionClients,
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
        let clients = self.clients.clone();
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
            let _ = clients.update(update);
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
    /// The connected clients: each one's capabilities and the sessions it
    /// created or loaded, which is where those sessions' updates go.
    clients: clients::Clients,
    /// Serializes capability registry changes with the process-wide features
    /// derived from them (presence and remote thread hosting).
    client_capability_updates: Arc<std::sync::Mutex<()>>,
    /// Discord Rich Presence, while a connected client asks for it (#2179).
    presence: presence::Presence,
    /// `/remote` shares and their pairing offers.
    remote: remote::RemoteManager,
    /// Not before this: the next attempt to share a thread by itself, after a
    /// gateway that could not be reached. Without it a gateway that is down
    /// would be retried, and logged, on every pass.
    share_retry_at: Arc<std::sync::Mutex<Option<std::time::Instant>>>,
    /// Threads running on an external harness (`docs/harnesses.md`).
    external: harnesses::ExternalSessions,
    /// The auto-share loop runs, once for the engine.
    auto_sharing: Arc<AtomicBool>,
}

/// Close a built-in thread: its turn, remote share, tasks and background
/// commands stop. Its working directory, when it was open.
async fn close_builtin(server: &AcpServer, session_id: &str) -> Option<std::path::PathBuf> {
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
    server.clients.forget_session(session_id);
    let cwd = match session {
        Some(session) => {
            session.turns.cancel_active().await;
            Some(session.cwd.clone())
        }
        None => None,
    };
    crate::tools::abort_background_starts(session_id);
    crate::tools::stop_background_tasks(session_id);
    cwd
}

/// A built-in `session/new` honours `_meta.rustcode.model` and `effort` the
/// same way external harnesses do (docs/harnesses.md). Model first, since
/// the effort choices depend on it; an unknown value is ignored and the
/// session keeps its default.
fn apply_requested_model_and_effort(
    state: &mut crate::app::AppState,
    asked: &crate::harness::HarnessRequest,
) {
    for (id, value) in [("model", &asked.model), ("effort", &asked.effort)] {
        if let Some(value) = value {
            let _ = handle_set_config_option(
                state,
                &agent_client_protocol::schema::v1::SessionConfigId::new(id),
                &agent_client_protocol::schema::v1::SessionConfigOptionValue::value_id(
                    value.clone(),
                ),
            );
        }
    }
}

fn unknown_session(session_id: &str) -> agent_client_protocol::Error {
    agent_client_protocol::Error::invalid_params()
        .data(format!("unknown ACP session: {session_id}"))
}

/// One open thread auto-share may share, with how recently it was used.
struct Candidate {
    last_active: String,
    target: CandidateTarget,
}

enum CandidateTarget {
    Builtin(Arc<remote::RemoteSession>),
    Harness(String, Arc<crate::harness::session::ExternalSession>),
}

impl Candidate {
    fn session_id(&self) -> String {
        match &self.target {
            CandidateTarget::Builtin(session) => session.session_id.clone(),
            CandidateTarget::Harness(session_id, _) => session_id.clone(),
        }
    }
}

impl AcpServer {
    /// Share every open thread while remote access is switched on for this
    /// machine, and unshare them when it is switched off. A thread on the
    /// built-in harness shares from its `AppState`; one on an external
    /// harness shares the mirror of its updates. Both are the same threads
    /// the app has open, so a device sees exactly those.
    async fn auto_share(&self) {
        if !remote::enabled() {
            if self.remote.shared_count() > 0 {
                self.remote
                    .unshare_all(crate::remote::SessionCloseReason::SharingDisabled)
                    .await;
            }
            return;
        }
        if self
            .share_retry_at
            .lock()
            .ok()
            .and_then(|at| *at)
            .is_some_and(|at| std::time::Instant::now() < at)
        {
            return;
        }
        // The gateway shares at most this many threads per host, so a Mac
        // with more threads open shares the ones used most recently.
        let room = remote::MAX_SHARED_SESSIONS.saturating_sub(self.remote.shared_count());
        let mut candidates: Vec<Candidate> = Vec::new();
        {
            let sessions = self.sessions.lock().await;
            for (session_id, session) in sessions.iter() {
                if self.remote.is_shared(session_id) || self.remote.refusal(session_id).is_some() {
                    continue;
                }
                // Read without waiting: a busy thread's clock is its last
                // message, and a lock held elsewhere only costs it its place
                // in the order.
                let last_active = session
                    .state
                    .try_lock()
                    .ok()
                    .and_then(|state| {
                        state
                            .history
                            .iter()
                            .rev()
                            .map(|message| message.timestamp.clone())
                            .find(|timestamp| !timestamp.is_empty())
                    })
                    .unwrap_or_default();
                candidates.push(Candidate {
                    last_active,
                    target: CandidateTarget::Builtin(remote::RemoteSession::from_acp(
                        session_id.clone(),
                        session,
                        self.clients.session(session_id),
                        Arc::clone(&self.client),
                        self.auto_approve,
                        self.presence.clone(),
                        self.remote.clone(),
                    )),
                });
            }
        }
        {
            let sessions = self.external.lock().await;
            for (session_id, session) in sessions.iter() {
                if self.remote.is_shared(session_id)
                    || self.remote.refusal(session_id).is_some()
                    || session.is_closed()
                {
                    continue;
                }
                candidates.push(Candidate {
                    last_active: session.record().updated_at,
                    target: CandidateTarget::Harness(session_id.clone(), Arc::clone(session)),
                });
            }
        }
        // Most recently active first; a thread with no recorded activity goes
        // last. ISO 8601 strings sort by time as they are.
        candidates.sort_by(|left, right| right.last_active.cmp(&left.last_active));
        // Past the host's bound the least recently active threads stay
        // private, and their status says why.
        for extra in candidates.iter().skip(room) {
            self.remote.refuse(
                &extra.session_id(),
                "this host already shares 32 threads; close one to share this one".to_owned(),
            );
        }
        let mut failed = false;
        for candidate in candidates.into_iter().take(room) {
            let session_id = candidate.session_id();
            let started = match candidate.target {
                CandidateTarget::Builtin(session) => self.remote.start(session, false).await,
                CandidateTarget::Harness(id, session) => {
                    let clients = self.clients.session(&id);
                    self.remote
                        .start_harness(id, session, Some(clients), false)
                        .await
                }
            };
            match started {
                Ok(_) => {
                    // A thread closed while this pass connected has nothing
                    // to share: the close removed its session, so the share
                    // that just registered would outlive it.
                    if !self.thread_is_open(&session_id).await {
                        self.remote
                            .stop_session(
                                &session_id,
                                crate::remote::SessionCloseReason::SessionChanged,
                            )
                            .await;
                    }
                }
                // The gateway itself could not be reached: back off for every
                // thread, not just this one.
                Err(remote::StartError::Link(message)) => {
                    crate::dbg_log!("could not share this thread with a device: {message}");
                    failed = true;
                }
                // This thread alone cannot be shared (it closed, or another
                // process holds it): leave it, and try the others.
                Err(remote::StartError::Refused(message)) => {
                    crate::dbg_log!("thread {session_id} is not shared with a device: {message}");
                    self.remote.refuse(&session_id, message);
                }
            }
        }
        if let Ok(mut retry_at) = self.share_retry_at.lock() {
            *retry_at =
                failed.then(|| std::time::Instant::now() + std::time::Duration::from_secs(30));
        }
    }

    /// Whether `session_id` is still open in this app, under the registry's
    /// own lock: what a share started a moment ago must re-check.
    async fn thread_is_open(&self, session_id: &str) -> bool {
        if self.sessions.lock().await.contains_key(session_id) {
            return true;
        }
        self.external
            .lock()
            .await
            .get(session_id)
            .is_some_and(|session| !session.is_closed())
    }

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
            clients: clients::Clients::default(),
            client_capability_updates: Arc::new(std::sync::Mutex::new(())),
            presence: presence::Presence::default(),
            remote: remote::RemoteManager::default(),
            share_retry_at: Arc::new(std::sync::Mutex::new(None)),
            external: harnesses::ExternalSessions::default(),
            auto_sharing: Arc::new(AtomicBool::new(false)),
        })
    }

    /// `initialize` from the connection `peer`: keep what it advertised, and
    /// start what the engine runs once for all its clients.
    fn client_initialized(&self, peer: &clients::Peer, info: clients::ClientInfo) {
        {
            let _updates = self
                .client_capability_updates
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            self.clients.connect(peer.id(), peer.connection(), info);
            // A thread's status is shown in every client's thread list.
            let clients = self.clients.clone();
            crate::thread_status::set_notifier(move |session_id, update| {
                clients.broadcast(SessionNotification::new(session_id, update));
            });
            crate::thread_status::start_watching();
            self.refresh_presence();
            self.refresh_thread_host();
        }
        // While remote access is on for this machine, every thread the
        // engine has open shares itself, the way a terminal does
        // (`docs/remote.md`). The flag is read again on every pass:
        // `/remote` in a terminal and Settings › Remote access here are one
        // switch.
        if !self.auto_sharing.swap(true, Ordering::AcqRel) {
            let autosharer = self.clone();
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(2));
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                loop {
                    interval.tick().await;
                    autosharer.auto_share().await;
                }
            });
        }
    }

    /// Tell every client that lists threads about a thread just made: the
    /// machine's server owns its threads, so the app on it and every other
    /// connected app list the new row at once, whichever client asked for
    /// `session/new` (docs/desktop.md › Several machines).
    fn announce_thread_created(&self, notification: ThreadCreatedNotification) {
        self.clients
            .broadcast_to(clients::Capability::Threads, notification);
    }

    /// Tell every client that lists threads about a thread just trashed.
    fn announce_thread_deleted(&self, notification: ThreadDeletedNotification) {
        self.clients
            .broadcast_to(clients::Capability::Threads, notification);
    }

    /// Advertise thread creation only while a connected client can list the
    /// thread the engine starts for a paired device.
    fn refresh_thread_host(&self) {
        if !self.clients.any(|info| info.shows_threads) {
            self.remote.clear_thread_host();
            return;
        }
        let hosted_server = self.clone();
        self.remote.set_thread_host(move |ask| {
            let server = hosted_server.clone();
            Box::pin(async move { hosted::start(&server, ask).await })
        });
    }

    /// Presence follows whether any connected client asks for it.
    fn refresh_presence(&self) {
        self.presence
            .set_client(self.clients.any(|info| info.presence));
    }

    /// The connection `id` ended. What it started keeps running: sessions,
    /// turns and harness processes belong to the engine.
    fn client_closed(&self, id: clients::ClientId) {
        let _updates = self
            .client_capability_updates
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.clients.disconnect(id);
        self.refresh_presence();
        self.refresh_thread_host();
    }

    /// The engine stops: every share, harness, turn and background task ends.
    async fn shutdown(&self) {
        {
            let _updates = self
                .client_capability_updates
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            self.remote.clear_thread_host();
        }
        self.remote.shutdown().await;
        harnesses::close_all(&self.external).await;
        let route_ids = self
            .task_routes
            .lock()
            .expect("ACP task routes mutex poisoned")
            .drain()
            .map(|(session_id, _)| session_id)
            .collect::<Vec<_>>();
        let sessions = self
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
    server.clients.forget_session(session_id);
    if let Some(session) = session {
        session.turns.cancel_active().await;
    }
}

/// Watch a pull request from a thread (#2648). Answers `{watch}` or
/// `{error: {code, message}}`.
#[derive(
    Debug,
    Clone,
    Default,
    serde::Serialize,
    serde::Deserialize,
    agent_client_protocol::JsonRpcRequest,
)]
#[serde(rename_all = "camelCase")]
#[request(method = "_rustcode/pulls/watch/start", response = serde_json::Value)]
pub(crate) struct PullWatchStartRequest {
    pub session_id: String,
    pub cwd: std::path::PathBuf,
    pub number: u64,
    #[serde(default)]
    pub interests: Option<crate::pull_watch::Interests>,
    #[serde(default)]
    pub link: bool,
    #[serde(default)]
    pub repo: String,
}

/// Stop watching; `remove` forgets the watch too. Answers `{watch}`, with
/// a null watch when there was none.
#[derive(
    Debug,
    Clone,
    Default,
    serde::Serialize,
    serde::Deserialize,
    agent_client_protocol::JsonRpcRequest,
)]
#[serde(rename_all = "camelCase")]
#[request(method = "_rustcode/pulls/watch/stop", response = serde_json::Value)]
pub(crate) struct PullWatchStopRequest {
    pub session_id: String,
    pub number: u64,
    #[serde(default)]
    pub remove: bool,
}

/// The watches, a thread's when `sessionId` is given. Answers `{watches}`.
#[derive(
    Debug,
    Clone,
    Default,
    serde::Serialize,
    serde::Deserialize,
    agent_client_protocol::JsonRpcRequest,
)]
#[serde(rename_all = "camelCase")]
#[request(method = "_rustcode/pulls/watches", response = serde_json::Value)]
pub(crate) struct PullWatchesRequest {
    /// Claim a linked PR’s pending events and advance its delivered cursor.
    #[serde(default)]
    pub take: Option<u64>,
    #[serde(default)]
    pub session_id: Option<String>,
}

/// A watch changed: checked, stopped, or with new events, which are only
/// in this message once. The app tells the reader; nothing acts on them.
/// A thread this engine started because a paired device asked for one
/// (`docs/remote.md`, *Threads started from a device*). The client lists it as
/// a thread of its own: a thread has no row in the store before its first turn,
/// so listing alone would not show it.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcNotification,
)]
#[notification(method = "_rustcode/threads/created")]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadCreatedNotification {
    pub session_id: String,
    pub cwd: std::path::PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The harness the thread runs on (`rustcode` for the built-in one).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// A thread a client trashed (`_rustcode/threads/delete`): every client that lists threads drops
/// the row, so a machine's app and the app driving it from another computer agree on what is there.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcNotification,
)]
#[notification(method = "_rustcode/threads/deleted")]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadDeletedNotification {
    pub session_id: String,
}

/// The app trashed a thread's folder (its shell moves it to the trash): this is what tells every
/// other client, since no one else can see that folder go.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/threads/delete", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadDeleteRequest {
    pub session_id: String,
}

#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcNotification,
)]
#[notification(method = "_rustcode/pulls/watch")]
pub(crate) struct PullWatchNotification {
    pub watch: crate::pull_watch::Watch,
    pub events: Vec<crate::pull_watch::WatchEvent>,
}

/// Start checking watches, once per engine, for every client that asked about
/// them. Each round tries for the checking lease, so a second engine takes
/// over when the first one exits.
fn start_pull_watching(clients: &clients::Clients) {
    static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let Some(store) = crate::pull_watch::WatchStore::default_store() else {
        return;
    };
    let notify = clients.clone();
    tokio::spawn(async move {
        let mut lease = None;
        let mut announced = std::collections::HashMap::new();
        loop {
            if lease.is_none() {
                lease = store.lease();
            }
            if lease.is_none() {
                // Another process checks GitHub and tells only its own
                // clients: pass on what it found to this engine's.
                for (watch, events) in crate::pull_watch::unannounced(&store.load(), &mut announced)
                {
                    notify.publish(
                        clients::Topic::PullWatches,
                        PullWatchNotification { watch, events },
                    );
                }
            }
            if lease.is_some() {
                crate::pull_watch::check_due(&store, |watch, events| {
                    // A worktree thread whose pull request merged is done.
                    crate::thread_status::on_pull_watch(watch);
                    // A change its thread has not been told about wants the
                    // user's look: the thread's status says so in every
                    // client, and its next `status` call hands the events
                    // over (#3369).
                    crate::thread_status::on_pull_events(&watch.session_id, events);
                    notify.publish(
                        clients::Topic::PullWatches,
                        PullWatchNotification {
                            watch: watch.clone(),
                            events: events.to_vec(),
                        },
                    );
                })
                .await;
            }
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        }
    });
}

/// The user started writing a message: wait for background MCP startup,
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

/// One delegate's pull request and its checks, by branch (the worktree's) or
/// the branch the folder is on. Answers `{branch, pull}` or `{error}`.
#[derive(
    Debug,
    Clone,
    Default,
    serde::Serialize,
    serde::Deserialize,
    agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/pulls/state", response = serde_json::Value)]
pub(crate) struct PullStateRequest {
    pub cwd: std::path::PathBuf,
    #[serde(default)]
    pub branch: Option<String>,
}

/// Merge a pull request once every check passes, as `scripts/pr-merge.sh` does
/// (#3153). Answers `{merge}` or `{error}`.
#[derive(
    Debug,
    Clone,
    Default,
    serde::Serialize,
    serde::Deserialize,
    agent_client_protocol::JsonRpcRequest,
)]
#[serde(rename_all = "camelCase")]
#[request(method = "_rustcode/pulls/merge", response = serde_json::Value)]
pub(crate) struct PullMergeRequest {
    pub cwd: std::path::PathBuf,
    #[serde(default)]
    pub number: Option<u64>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// The orchestration call this merge answers: `_rustcode/pulls/merge/cancel`
    /// with the same id ends it before it merges.
    #[serde(default)]
    pub merge_id: Option<String>,
}

/// Stop the merge started under `merge_id` (the call that asked for it was
/// dropped). Answers `{ cancelled }`: whether one was still running.
#[derive(
    Debug,
    Clone,
    Default,
    serde::Serialize,
    serde::Deserialize,
    agent_client_protocol::JsonRpcRequest,
)]
#[serde(rename_all = "camelCase")]
#[request(method = "_rustcode/pulls/merge/cancel", response = serde_json::Value)]
pub(crate) struct PullMergeCancelRequest {
    pub merge_id: String,
}

/// Remove a merged delegate's worktree and branch (#3153). `merged`, `head_sha`
/// and `branch` are what the app read for its pull request; the engine still
/// reads the branch the worktree is on now and checks it is provably that pull
/// request's head before accepting a rename, and that the worktree is clean and
/// its commits are not lost. Answers `{cleanup}` or `{error}`.
#[derive(
    Debug,
    Clone,
    Default,
    serde::Serialize,
    serde::Deserialize,
    agent_client_protocol::JsonRpcRequest,
)]
#[serde(rename_all = "camelCase")]
#[request(method = "_rustcode/worktrees/cleanup", response = serde_json::Value)]
pub(crate) struct WorktreeCleanupRequest {
    pub path: std::path::PathBuf,
    /// The branch recorded for the worktree, checked against HEAD: when the
    /// worktree is on another, cleanup reads that branch's pull request itself.
    #[serde(default)]
    pub branch: Option<String>,
    /// The commit the pull request was merged at.
    #[serde(default)]
    pub head_sha: Option<String>,
    #[serde(default)]
    pub merged: bool,
}

/// A pull request's diff. Answers `{diff, truncated}` or `{error}`.
#[derive(
    Debug,
    Clone,
    Default,
    serde::Serialize,
    serde::Deserialize,
    agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/pulls/diff", response = serde_json::Value)]
pub(crate) struct PullDiffRequest {
    pub cwd: std::path::PathBuf,
    pub number: u64,
}

/// Post one review the user wrote and submitted (#2646). Answers
/// `{review: {url}}` or `{error: {code, message}}`; `uncertain` means it may
/// have been posted.
#[derive(
    Debug,
    Clone,
    Default,
    serde::Serialize,
    serde::Deserialize,
    agent_client_protocol::JsonRpcRequest,
)]
#[serde(rename_all = "camelCase")]
#[request(method = "_rustcode/pulls/review", response = serde_json::Value)]
pub(crate) struct PullReviewRequest {
    pub cwd: std::path::PathBuf,
    pub repository: String,
    pub number: u64,
    pub commit_id: String,
    pub event: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub comments: Vec<crate::pull_requests::ReviewComment>,
}

/// Whether the client lists a thread the engine starts for a paired device:
/// `_meta.rustcode.threads` in `initialize`. The desktop app does; a terminal's
/// client does not, so its shares never advertise `host_threads`.
pub(crate) fn client_shows_threads(
    meta: Option<&serde_json::Map<String, serde_json::Value>>,
) -> bool {
    meta.and_then(|meta| meta.get("rustcode"))
        .and_then(|rustcode| rustcode.get("threads"))
        .and_then(serde_json::Value::as_u64)
        .is_some_and(|version| version >= 1)
}

pub async fn run_acp(auto_approve: bool) -> Result<(), Box<dyn std::error::Error>> {
    let engine = AcpEngine::start(auto_approve)?;
    // No harness process outlives the engine, however it ends.
    let _kill_harnesses = KillHarnesses;
    // Another local process — the remote gateway relaying a paired desktop —
    // reaches this engine only through a description it publishes here: there
    // is no `rustcode server run` behind `--acp` (`server/engine_endpoint.rs`,
    // #3341). stdio keeps its one client either way.
    let published = publish_endpoint(&engine);
    // Over stdio the engine has this one client and ends with it.
    let served = serve_connection(&engine.server, Stdio::new()).await;
    drop(published);
    engine.shutdown().await;
    served?;
    Ok(())
}

/// Where other local processes can reach this engine while it runs. A failure
/// here leaves the engine serving stdio: the app works, a paired device is
/// told the engine has no endpoint instead of being left without an answer.
fn publish_endpoint(engine: &AcpEngine) -> Option<crate::server::engine_endpoint::Published> {
    let run_dir = match crate::server::run_dir() {
        Ok(dir) => dir,
        Err(error) => {
            crate::dbg_log!("ACP engine: no run directory to publish an endpoint in: {error}");
            return None;
        }
    };
    match crate::server::engine_endpoint::publish(&run_dir, engine) {
        Ok(published) => {
            crate::dbg_log!("ACP engine: published {}", published.endpoint());
            Some(published)
        }
        Err(error) => {
            crate::dbg_log!("ACP engine: could not publish an endpoint: {error}");
            None
        }
    }
}

/// Ends every harness process when dropped: they live as long as the engine
/// process, whichever way it ends, and no longer.
pub struct KillHarnesses;

impl Drop for KillHarnesses {
    fn drop(&mut self) {
        crate::harness::process::kill_all();
    }
}

/// What the engine runs once per process: sessions, their turns and
/// harnesses, shares and the background work. Every ACP connection
/// ([`AcpEngine::serve`]) is a client of it; one closing ends nothing that
/// runs.
#[derive(Clone)]
pub struct AcpEngine {
    server: AcpServer,
}

impl AcpEngine {
    /// Warm what every connection reads and start the engine's own tasks.
    pub fn start(auto_approve: bool) -> Result<Self, agent_client_protocol::Error> {
        let startup_state = crate::app::AppState::new();
        // Connect in the background so a slow server cannot hold up startup.
        crate::mcp::defer_enabled_servers(&startup_state.config.mcp_servers);
        tokio::spawn(crate::mcp::start_deferred_servers());
        // The thread above each delegated one, recorded once for threads older than
        // the record, before the first session list is read (`thread_parent.rs`).
        // After that first run it reads one `store_meta` row and returns.
        crate::thread_parent::ensure_ready();
        crate::thread_index::warm();
        // Metadata for `rustcode usage` and `rustcode data query`; a background
        // backfill of the existing threads runs once, on first open.
        crate::data_store::warm();
        // Links an earlier engine was running when it died are not running now.
        tokio::task::spawn_blocking(crate::data_store::interrupt_orphaned_links);
        let server = AcpServer::new(auto_approve)?;
        // Apps connected through `rustcode mcp-connect` find this engine by its
        // hub, so it listens from the start when any are connected.
        tokio::spawn(crate::orchestration::outside::bind_hub_when_connected());
        // Linked pull requests from before a restart carry on from here, and
        // one made by a tool during this run is picked up on the next round
        // (#3369). Without this a link would be stored and never checked.
        start_pull_watching(&server.clients);
        Ok(Self { server })
    }

    /// An engine that starts and defers nothing (tests): the default
    /// configuration's MCP servers would otherwise start on a test's turn.
    #[cfg(test)]
    pub(crate) fn bare(auto_approve: bool) -> Result<Self, agent_client_protocol::Error> {
        Ok(Self {
            server: AcpServer::new(auto_approve)?,
        })
    }

    /// Serve one client over newline-delimited JSON-RPC on `read` and
    /// `write` until it disconnects.
    pub async fn serve<R, W>(&self, read: R, write: W) -> Result<(), agent_client_protocol::Error>
    where
        R: tokio::io::AsyncRead + Send + Unpin + 'static,
        W: tokio::io::AsyncWrite + Send + Unpin + 'static,
    {
        serve_connection(&self.server, lines_transport(read, write)).await
    }

    /// How many clients are connected.
    pub fn clients(&self) -> usize {
        self.server.clients.count()
    }

    /// The engine stops: every share, harness, turn and background task ends.
    pub async fn shutdown(&self) {
        self.server.shutdown().await;
    }
}

/// Run a turn on the runtime rather than on the connection that asked for
/// it, whose tasks end with it. Answering a client that has gone fails, and
/// the turn does not care.
///
/// A turn's future is deep to poll in a debug build, deeper than a runtime
/// worker's stack; on the connection it ran on the thread serving it (the
/// main thread over stdio). It gets a thread of its own, sized like the
/// main one's and more, and what it spawns runs on the runtime.
fn spawn_turn(
    turn: impl std::future::Future<Output = Result<(), agent_client_protocol::Error>> + Send + 'static,
) {
    spawn_on_large_stack("rustcode-turn", async move {
        if let Err(error) = turn.await {
            crate::dbg_log!("a turn's answer was not delivered: {error}");
        }
    });
}

/// Poll `future` to completion on a new thread with a large stack, as part of
/// the current runtime: what it spawns runs on the runtime's workers.
pub(crate) fn spawn_on_large_stack(
    name: &str,
    future: impl std::future::Future<Output = ()> + Send + 'static,
) {
    const STACK: usize = 64 << 20;
    let runtime = tokio::runtime::Handle::current();
    // A caller scoped to its own MCP servers (a test, `--prompt`) keeps them.
    let mcp = crate::mcp::DIRECT_MCP_REGISTRY.try_with(Clone::clone).ok();
    let spawned = std::thread::Builder::new()
        .name(name.to_owned())
        .stack_size(STACK)
        .spawn(move || match mcp {
            Some(registry) => {
                runtime.block_on(crate::mcp::DIRECT_MCP_REGISTRY.scope(registry, future))
            }
            None => runtime.block_on(future),
        });
    if let Err(error) = spawned {
        crate::dbg_log!("could not start {name}: {error}");
    }
}

/// Show a client's prompt to the session's other clients.
fn echo_prompt(
    clients: &clients::SessionClients,
    sender: clients::ClientId,
    prompt: &[ContentBlock],
    origin: Option<serde_json::Value>,
) {
    let text = prompt
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    if text.is_empty() {
        return;
    }
    // The other clients did not write it: they are told who did.
    clients.update_others(
        sender,
        SessionUpdate::UserMessageChunk(crate::message_origin::user_chunk(
            text,
            origin.map(|origin| {
                serde_json::Map::from_iter([(
                    "rustcode".to_owned(),
                    serde_json::json!({"origin": origin}),
                )])
            }),
        )),
    );
}

/// Newline-delimited JSON-RPC over a byte stream, as over stdio.
pub(crate) fn lines_transport<R, W>(
    read: R,
    write: W,
) -> agent_client_protocol::Lines<
    impl futures_util::Sink<String, Error = std::io::Error> + Send + 'static,
    impl futures_util::Stream<Item = std::io::Result<String>> + Send + 'static,
>
where
    R: tokio::io::AsyncRead + Send + Unpin + 'static,
    W: tokio::io::AsyncWrite + Send + Unpin + 'static,
{
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    let incoming = futures_util::stream::unfold(
        tokio::io::BufReader::new(read).lines(),
        async |mut lines| match lines.next_line().await {
            Ok(Some(line)) => Some((Ok(line), lines)),
            Ok(None) => None,
            Err(error) => Some((Err(error), lines)),
        },
    );
    let outgoing = futures_util::sink::unfold(write, async |mut write, line: String| {
        write.write_all(line.as_bytes()).await?;
        write.write_all(b"\n").await?;
        write.flush().await?;
        Ok::<_, std::io::Error>(write)
    });
    agent_client_protocol::Lines::new(Box::pin(outgoing), Box::pin(incoming))
}

/// Serve one ACP client on `transport` until it disconnects. The sessions it
/// creates or loads, and their turns, belong to `server`: they outlive the
/// connection, and other connections to the same server go on.
async fn serve_connection(
    server: &AcpServer,
    transport: impl agent_client_protocol::ConnectTo<Agent> + 'static,
) -> Result<(), agent_client_protocol::Error> {
    let client_id = server.clients.new_id();
    let served = Agent
        .builder()
        .name("rustcode")
        .on_receive_request(
            async |request: InitializeRequest, responder, connection| {
                let info = clients::ClientInfo::from_capabilities(&request.client_capabilities);
                server.client_initialized(&server.clients.peer(client_id, &connection), info);
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
                    // `_meta.rustcode.worktree`: the thread starts in a new worktree.
                    let (request, worktree) = match crate::worktrees::prepare_new(request) {
                        Ok(prepared) => prepared,
                        Err(error) => return responder.respond_with_error(error),
                    };
                    // The row every other client is told about: the folder the thread runs in,
                    // which is the worktree's when it starts in one, not the checkout the caller
                    // named (it would otherwise be listed under the wrong project).
                    let cwd = request.cwd.clone();
                    // An external harness starts a process, which can take a
                    // while; it does not hold the dispatch loop.
                    let peer = server.clients.peer(client_id, &_connection);
                    if let Some(asked) = harnesses::external_request(&request) {
                        let external = server.external.clone();
                        let remote = server.remote.clone();
                        let harness = asked.harness.clone();
                        let announced = server.clone();
                        return _connection.spawn(async move {
                            match harnesses::new_session(&external, request, asked, &peer, &remote)
                                .await
                            {
                                Ok(response) => {
                                    announced.announce_thread_created(ThreadCreatedNotification {
                                        session_id: response.session_id.to_string(),
                                        cwd: cwd.clone(),
                                        title: None,
                                        harness,
                                        model: None,
                                    });
                                    responder.respond(crate::worktrees::finish_new(
                                        response,
                                        worktree.as_ref(),
                                    ))?;
                                }
                                Err(error) => responder.respond_with_error(error)?,
                            }
                            Ok(())
                        });
                    }
                    let response = create_builtin_session(&server, &peer, request).await?;
                    server.announce_thread_created(ThreadCreatedNotification {
                        session_id: response.session_id.to_string(),
                        cwd,
                        title: None,
                        harness: Some(crate::harness::BUILTIN_ID.to_owned()),
                        model: None,
                    });
                    responder.respond(crate::worktrees::finish_new(response, worktree.as_ref()))?;
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
                async move |request: ThreadDeleteRequest, responder, _connection| {
                    server.announce_thread_deleted(ThreadDeletedNotification {
                        session_id: request.session_id,
                    });
                    responder.respond(serde_json::json!({}))?;
                    Ok(())
                }
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
            async |request: settings::GetSettingsRequest, responder, _connection| {
                responder.respond(settings::read(request)?)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async |request: settings::SetSettingsRequest, responder, _connection| {
                responder.respond(settings::write(request)?)
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
                        Ok(answer) => {
                            // No update says the chat was cut: nobody
                            // resumes from before it.
                            server.clients.invalidate_cursors(&request.session_id);
                            responder.respond(answer)
                        }
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
                async move |request: session_repair::RepairSessionRequest, responder, _connection| {
                    if !valid_acp_session_id(&request.session_id) {
                        return responder.respond_with_error(
                            agent_client_protocol::Error::invalid_params()
                                .data("invalid session id"),
                        );
                    }
                    if session_id_is_active(&*server.sessions.lock().await, &request.session_id) {
                        return responder.respond_with_error(
                            agent_client_protocol::Error::invalid_params()
                                .data("close the session before repairing it"),
                        );
                    }
                    match session_repair::repair_session(&request.session_id) {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder.respond_with_error(error),
                    }
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: usage::UsagePlansRequest, responder, connection| {
                connection.spawn(async move {
                    responder.respond(usage::plans(request).await)?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: usage::UsageLimitsRequest, responder, connection| {
                    let Some((state, _)) = server.session_handle(&request.session_id).await else {
                        // A thread on another harness has no engine session: its plan is that
                        // harness's own (the Codex CLI's windows for Codex).
                        let Some(harness) = usage::external_harness(&request.session_id) else {
                            return responder
                                .respond_with_error(unknown_session(&request.session_id));
                        };
                        let session_id = request.session_id.clone();
                        return connection.spawn(async move {
                            // A named account's own login, so the meter never
                            // shows another home's numbers.
                            let home = usage::account_home(&session_id, &harness);
                            responder
                                .respond(usage::harness_limits(&harness, home.as_deref()).await)?;
                            Ok(())
                        });
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
            async move |_request: commands::CommandsRequest, responder, connection| {
                connection.spawn(async move {
                    match commands::catalog_update() {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder
                            .respond_with_error(agent_client_protocol::Error::internal_error().data(error)),
                    }?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: outside::OutsideListRequest, responder, connection| {
                connection.spawn(async move {
                    match outside::list(request).await {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder
                            .respond_with_error(agent_client_protocol::Error::internal_error().data(error)),
                    }?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: outside::OutsideCreateRequest, responder, connection| {
                connection.spawn(async move {
                    match outside::create(request).await {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder
                            .respond_with_error(agent_client_protocol::Error::invalid_params().data(error)),
                    }?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: outside::OutsideUpdateRequest, responder, connection| {
                connection.spawn(async move {
                    match outside::update(request) {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder
                            .respond_with_error(agent_client_protocol::Error::invalid_params().data(error)),
                    }?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: outside::OutsideRevokeRequest, responder, connection| {
                connection.spawn(async move {
                    match outside::revoke(request) {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder
                            .respond_with_error(agent_client_protocol::Error::internal_error().data(error)),
                    }?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: outside::OutsideAnswerRequest, responder, connection| {
                connection.spawn(async move {
                    match outside::answer(request) {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder
                            .respond_with_error(agent_client_protocol::Error::internal_error().data(error)),
                    }?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: data::DataUsageRequest, responder, connection| {
                connection.spawn(async move {
                    match data::usage(request) {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder
                            .respond_with_error(agent_client_protocol::Error::internal_error().data(error)),
                    }?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: data::DataQueryRequest, responder, connection| {
                connection.spawn(async move {
                    match data::query(request) {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder
                            .respond_with_error(agent_client_protocol::Error::internal_error().data(error)),
                    }?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: data::DataActivityRequest, responder, connection| {
                connection.spawn(async move {
                    match data::activity(request) {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder
                            .respond_with_error(agent_client_protocol::Error::internal_error().data(error)),
                    }?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |_request: data::DataLinksLoadRequest, responder, connection| {
                connection.spawn(async move {
                    match data::links_load() {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder
                            .respond_with_error(agent_client_protocol::Error::internal_error().data(error)),
                    }?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: data::DataLinksSaveRequest, responder, connection| {
                connection.spawn(async move {
                    match data::links_save(request) {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder
                            .respond_with_error(agent_client_protocol::Error::internal_error().data(error)),
                    }?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: data::DataReceiptsGetRequest, responder, connection| {
                connection.spawn(async move {
                    match data::receipts_get(request) {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder
                            .respond_with_error(agent_client_protocol::Error::internal_error().data(error)),
                    }?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: projects::ProjectReadRequest, responder, connection| {
                connection.spawn(async move {
                    match projects::read(request) {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder
                            .respond_with_error(agent_client_protocol::Error::invalid_params().data(error)),
                    }?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: projects::ProjectCreateRequest, responder, connection| {
                connection.spawn(async move {
                    match projects::create(request) {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder
                            .respond_with_error(agent_client_protocol::Error::invalid_params().data(error)),
                    }?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: projects::ProjectCloneRequest, responder, connection| {
                connection.spawn(async move {
                    match projects::clone(request) {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder
                            .respond_with_error(agent_client_protocol::Error::invalid_params().data(error)),
                    }?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: environment::EnvironmentSyncRequest, responder, connection| {
                connection.spawn(async move {
                    match environment::sync(request) {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder
                            .respond_with_error(agent_client_protocol::Error::internal_error().data(error)),
                    }?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |_request: scheduled::ScheduledListRequest, responder, connection| {
                connection.spawn(async move {
                    match scheduled::list().await {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder
                            .respond_with_error(agent_client_protocol::Error::internal_error().data(error)),
                    }?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: scheduled::ScheduledCreateRequest, responder, connection| {
                connection.spawn(async move {
                    match scheduled::create(request).await {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder
                            .respond_with_error(agent_client_protocol::Error::invalid_params().data(error)),
                    }?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: scheduled::ScheduledDeleteRequest, responder, connection| {
                connection.spawn(async move {
                    match scheduled::delete(request.id).await {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder
                            .respond_with_error(agent_client_protocol::Error::internal_error().data(error)),
                    }?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: scheduled::ScheduledRunRequest, responder, connection| {
                connection.spawn(async move {
                    match scheduled::run(request.id).await {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder
                            .respond_with_error(agent_client_protocol::Error::internal_error().data(error)),
                    }?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: mirror::MirrorRequest, responder, connection| {
                connection.spawn(async move {
                    // The mirror reads and writes files; keep that off the connection's task.
                    let answer = tokio::task::spawn_blocking(move || mirror::answer(request))
                        .await
                        .unwrap_or_else(|error| Err(error.to_string()));
                    match answer {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder
                            .respond_with_error(agent_client_protocol::Error::invalid_params().data(error)),
                    }?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |_request: telemetry::TelemetryRequest, responder, connection| {
                connection.spawn(async move {
                    match telemetry::answer() {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder
                            .respond_with_error(agent_client_protocol::Error::internal_error().data(error)),
                    }?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: data::DataReceiptsPutRequest, responder, connection| {
                connection.spawn(async move {
                    match data::receipts_put(request) {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder
                            .respond_with_error(agent_client_protocol::Error::internal_error().data(error)),
                    }?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: telemetry::SetTelemetryRequest, responder, connection| {
                connection.spawn(async move {
                    match telemetry::answer_set(request) {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder
                            .respond_with_error(agent_client_protocol::Error::invalid_params().data(error)),
                    }?;
                    Ok(())
                })
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
        // A tool call to an MCP server waits for the user's approval, so it
        // runs off the dispatch loop.
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: mcp_app::McpAppRequest, responder, connection| {
                    let Some((state, _)) = server.session_handle(&request.session_id).await else {
                        return responder.respond_with_error(unknown_session(&request.session_id));
                    };
                    let builtin = harnesses::get(&server.external, &request.session_id)
                        .await
                        .is_none();
                    let clients = server.clients.session(&request.session_id);
                    let auto_approve = server.auto_approve;
                    connection.spawn(async move {
                        match mcp_app::handle(request, builtin, state, clients, auto_approve).await
                        {
                            Ok(answer) => responder.respond(answer)?,
                            Err(error) => responder.respond_with_error(error)?,
                        }
                        Ok(())
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: steer::SteerRequest, responder, connection| {
                    if let Some(external) =
                        harnesses::get(&server.external, &request.session_id).await
                    {
                        let remote = server.remote.clone();
                        return connection.spawn(async move {
                            let blocks = request.prompt.clone().unwrap_or_else(|| {
                                vec![agent_client_protocol::schema::v1::ContentBlock::Text(
                                    agent_client_protocol::schema::v1::TextContent::new(
                                        request.text.clone(),
                                    ),
                                )]
                            });
                            let accepted = external.steer(&blocks).await;
                            if accepted {
                                remote.note_prompt(&request.session_id, &request.text, request.meta.as_ref().and_then(|v| v.get("rustcode")).and_then(|v| v.get("origin")).cloned().filter(|v| crate::message_origin::from_meta(v).is_some()));
                            }
                            responder.respond(serde_json::json!({ "accepted": accepted }))?;
                            Ok(())
                        });
                    }
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
                    // The built-in harness reads a steer as text; one with images waits for the next turn.
                    let images = request.prompt.iter().flatten().any(|block| {
                        matches!(block, agent_client_protocol::schema::v1::ContentBlock::Image(_))
                    });
                    if images {
                        return responder.respond(serde_json::json!({ "accepted": false }));
                    }
                    let mut state = state.lock().await;
                    let answer = steer::accept(&mut state, &request.text);
                    if answer.get("accepted").and_then(serde_json::Value::as_bool) == Some(true) {
                        if let Some(steer) = state.pending_steers.last_mut() {
                            steer.origin = request.meta.as_ref().and_then(|v| v.get("rustcode")).and_then(|v| v.get("origin")).cloned().filter(|v| crate::message_origin::from_meta(v).is_some());
                        }
                    }
                    responder.respond(answer)
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        // Remote access starts a gateway, waits for a pairing answer and runs
        // launchctl: keep the dispatch loop free while each does.
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: remote::RemoteStatusRequest, responder, connection| {
                    let server = server.clone();
                    connection.spawn(async move {
                        responder.respond(server.remote.status(&request.session_id).await)?;
                        Ok(())
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: remote::RemoteStartRequest, responder, connection| {
                    let server = server.clone();
                    connection.spawn(async move {
                        if request.session_id.is_empty() {
                            // Settings › Remote access: switch the machine on
                            // with no thread named. Every open thread shares
                            // itself from the next auto-share pass.
                            return responder.respond(server.remote.enable().await);
                        }
                        let session = {
                            let sessions = server.sessions.lock().await;
                            sessions.get(&request.session_id).map(|session| {
                                remote::RemoteSession::from_acp(
                                    request.session_id.clone(),
                                    session,
                                    server.clients.session(&request.session_id),
                                    Arc::clone(&server.client),
                                    server.auto_approve,
                                    server.presence.clone(),
                                    server.remote.clone(),
                                )
                            })
                        };
                        let Some(session) = session else {
                            return responder.respond_with_error(
                                agent_client_protocol::Error::invalid_params().data(format!(
                                    "unknown ACP session: {}",
                                    request.session_id
                                )),
                            );
                        };
                        match server.remote.start(session, request.pair).await {
                            Ok(value) => responder.respond(value)?,
                            Err(error) => responder.respond_with_error(error.into_acp())?,
                        }
                        Ok(())
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: remote::RemotePairRequest, responder, connection| {
                    let server = server.clone();
                    connection.spawn(async move {
                        responder.respond(server.remote.pair(&request.session_id).await)?;
                        Ok(())
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: remote::RemoteStopRequest, responder, connection| {
                    let server = server.clone();
                    connection.spawn(async move {
                        match server.remote.stop(&request.session_id).await {
                            Ok(value) => responder.respond(value)?,
                            Err(error) => responder.respond_with_error(error)?,
                        }
                        Ok(())
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: remote::RemoteAnswerRequest, responder, connection| {
                    let server = server.clone();
                    connection.spawn(async move {
                        responder.respond(
                            server
                                .remote
                                .answer_connect(&request.id, request.accept)
                                .await,
                        )?;
                        Ok(())
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                async move |request: remote::RemoteConnectCodeRequest, responder, connection| {
                    connection.spawn(async move {
                        responder.respond(remote::connect_code(request.cancel).await)?;
                        Ok(())
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: remote::RemoteRevokeRequest, responder, connection| {
                    let server = server.clone();
                    connection.spawn(async move {
                        responder.respond(server.remote.revoke(&request.device_id).await)?;
                        Ok(())
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        // Harness detection runs commands and a model listing may start
        // one, so none of these holds the dispatch loop.
        .on_receive_request(
            async move |_request: harnesses::HarnessesRequest, responder, connection| {
                connection.spawn(async move {
                    match harnesses::snapshot().await {
                        Ok(value) => responder.respond(value)?,
                        Err(error) => responder.respond_with_error(error)?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: harnesses::HarnessModelsRequest, responder, connection| {
                connection.spawn(async move {
                    match harnesses::models(request).await {
                        Ok(value) => responder.respond(value)?,
                        Err(error) => responder.respond_with_error(error)?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: harnesses::SetHarnessRequest, responder, connection| {
                connection.spawn(async move {
                    match harnesses::set(request).await {
                        Ok(value) => responder.respond(value)?,
                        Err(error) => responder.respond_with_error(error)?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: harnesses::RemoveHarnessRequest, responder, connection| {
                connection.spawn(async move {
                    match harnesses::remove(request).await {
                        Ok(value) => responder.respond(value)?,
                        Err(error) => responder.respond_with_error(error)?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: harnesses::DefaultHarnessRequest, responder, connection| {
                connection.spawn(async move {
                    match harnesses::set_default(request).await {
                        Ok(value) => responder.respond(value)?,
                        Err(error) => responder.respond_with_error(error)?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: harnesses::RememberHarnessRequest, responder, connection| {
                connection.spawn(async move {
                    match harnesses::remember(request).await {
                        Ok(value) => responder.respond(value)?,
                        Err(error) => responder.respond_with_error(error)?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: harnesses::AddHarnessAccountRequest, responder, connection| {
                connection.spawn(async move {
                    match harnesses::add_account(request).await {
                        Ok(value) => responder.respond(value)?,
                        Err(error) => responder.respond_with_error(error)?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: harnesses::RemoveHarnessAccountRequest, responder, connection| {
                    let external = server.external.clone();
                    connection.spawn(async move {
                        match harnesses::remove_account(&external, request).await {
                            Ok(value) => responder.respond(value)?,
                            Err(error) => responder.respond_with_error(error)?,
                        }
                        Ok(())
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: harnesses::CheckHarnessAccountsRequest, responder, connection| {
                connection.spawn(async move {
                    match harnesses::check_accounts(request).await {
                        Ok(value) => responder.respond(value)?,
                        Err(error) => responder.respond_with_error(error)?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: harnesses::SwitchHarnessRequest, responder, connection| {
                    let server = server.clone();
                    let peer = server.clients.peer(client_id, &connection);
                    connection.spawn(async move {
                        let session_id = request.session_id.clone();
                        let builtin = if harnesses::get(&server.external, &session_id).await.is_some() {
                            None
                        } else if crate::thread_status::is_running(&session_id) {
                            return responder.respond_with_error(
                                agent_client_protocol::Error::invalid_params()
                                    .data("stop the running turn before switching harness"),
                            );
                        } else {
                            close_builtin(&server, &session_id).await
                        };
                        match harnesses::switch(
                            &server.external,
                            request,
                            builtin,
                            &peer,
                            &server.remote,
                        )
                        .await
                        {
                            Ok(value) => responder.respond(value)?,
                            Err(error) => responder.respond_with_error(error)?,
                        }
                        Ok(())
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |_request: harnesses::OnboardingCompleteRequest, responder, connection| {
                connection.spawn(async move {
                    match harnesses::complete_onboarding().await {
                        Ok(value) => responder.respond(value)?,
                        Err(error) => responder.respond_with_error(error)?,
                    }
                    Ok(())
                })
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
            async move |request: capabilities::RestartMcpServerRequest, responder, connection| {
                // Connecting a server again can take the same seconds a start
                // does; keep the dispatch loop free while it does.
                connection.spawn(async move {
                    match capabilities::restart_mcp_server(request).await {
                        Ok(snapshot) => responder.respond(snapshot)?,
                        Err(error) => responder.respond_with_error(error)?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: capabilities::AddSkillRequest, responder, _connection| {
                responder.respond(capabilities::add_skill(request)?)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: capabilities::GetSkillRequest, responder, _connection| {
                responder.respond(capabilities::get_skill(request)?)
            },
            agent_client_protocol::on_receive_request!(),
        )
        // Orchestrator v2 (`docs/orchestrator.md`): worktrees, thread status
        // and checkpoints. Each runs git, so none holds the dispatch loop.
        .on_receive_request(
            async move |request: crate::worktrees::CreateRequest, responder, connection| {
                connection.spawn(async move {
                    let created = tokio::task::spawn_blocking(move || crate::worktrees::create(&request)).await;
                    match created {
                        Ok(Ok(worktree)) => responder.respond(serde_json::json!(worktree))?,
                        Ok(Err(error)) => responder.respond_with_error(error.into_acp())?,
                        Err(error) => responder.respond_with_error(agent_client_protocol::Error::internal_error().data(error.to_string()))?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: crate::worktrees::ListRequest, responder, connection| {
                connection.spawn(async move {
                    let listed = tokio::task::spawn_blocking(move || crate::worktrees::list(&request.repo)).await;
                    match listed {
                        Ok(Ok(worktrees)) => responder.respond(serde_json::json!({ "worktrees": worktrees }))?,
                        Ok(Err(error)) => responder.respond_with_error(error.into_acp())?,
                        Err(error) => responder.respond_with_error(agent_client_protocol::Error::internal_error().data(error.to_string()))?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: crate::worktrees::RemoveRequest, responder, connection| {
                connection.spawn(async move {
                    let removed = tokio::task::spawn_blocking(move || crate::worktrees::remove(&request.path, request.force)).await;
                    match removed {
                        Ok(Ok(answer)) => responder.respond(answer)?,
                        Ok(Err(error)) => responder.respond_with_error(error.into_acp())?,
                        Err(error) => responder.respond_with_error(agent_client_protocol::Error::internal_error().data(error.to_string()))?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: crate::worktrees::StateRequest, responder, connection| {
                connection.spawn(async move {
                    let states = tokio::task::spawn_blocking(move || crate::worktrees::states(&request.paths)).await;
                    match states {
                        Ok(rows) => responder.respond(serde_json::json!({ "worktrees": rows }))?,
                        Err(error) => responder.respond_with_error(agent_client_protocol::Error::internal_error().data(error.to_string()))?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: crate::worktrees::HandoffRequest, responder, connection| {
                connection.spawn(async move {
                    let to = crate::worktrees::Handoff::parse(request.to.trim());
                    let Some(to) = to else {
                        return responder.respond_with_error(
                            agent_client_protocol::Error::invalid_params()
                                .data(serde_json::json!({
                                    "reason": "invalid",
                                    "message": format!("to must be checkout or worktree, not '{}'", request.to),
                                })),
                        );
                    };
                    let moved = tokio::task::spawn_blocking(move || {
                        crate::worktrees::handoff(
                            &request.session_id,
                            to,
                            request.title.as_deref(),
                            request.branch.as_deref(),
                        )
                    })
                    .await;
                    match moved {
                        Ok(Ok(answer)) => responder.respond(answer)?,
                        Ok(Err(error)) => responder.respond_with_error(error.into_acp())?,
                        Err(error) => responder.respond_with_error(agent_client_protocol::Error::internal_error().data(error.to_string()))?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: crate::thread_status::PinRequest, responder, _connection| {
                match crate::thread_status::set_pinned(&request.session_id, request.pinned) {
                    Ok(()) => responder.respond(serde_json::json!({})),
                    Err(message) => responder.respond_with_error(agent_client_protocol::Error::invalid_params().data(message)),
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |_request: permissions::PermissionsRequest, responder, _connection| {
                // The user config is what every request's policy reads, so a
                // client lists and revokes there rather than in one session.
                let (_, _, config) = crate::config::load_config();
                responder.respond(permissions::grants_snapshot(&config))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: permissions::RevokePermissionRequest, responder, _connection| {
                    let (_, _, mut config) = crate::config::load_config();
                    let answer = permissions::revoke_grant(&mut config, &request.id);
                    // Open sessions hold their own copy of the user config: the
                    // one that answers a call must stop covering it now.
                    for session in server.sessions.lock().await.values() {
                        session.state.lock().await.config.timed_command_approvals =
                            config.timed_command_approvals.clone();
                    }
                    responder.respond(answer)
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async |request: themes::ThemesRequest, responder, connection| {
                // A client that lists themes restyles when they change.
                server.clients.subscribe(client_id, clients::Topic::Themes);
                themes::start_watching(&server.clients);
                connection.spawn(async move {
                    match tokio::task::spawn_blocking(move || themes::answer(request)).await {
                        Ok(answer) => responder.respond(answer)?,
                        Err(error) => responder.respond_with_error(agent_client_protocol::Error::internal_error().data(error.to_string()))?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async |request: themes::SetThemeRequest, responder, connection| {
                server.clients.subscribe(client_id, clients::Topic::Themes);
                themes::start_watching(&server.clients);
                connection.spawn(async move {
                    match tokio::task::spawn_blocking(move || themes::answer_set(request)).await {
                        Ok(Ok(answer)) => responder.respond(answer)?,
                        Ok(Err(error)) => responder.respond_with_error(error)?,
                        Err(error) => responder.respond_with_error(agent_client_protocol::Error::internal_error().data(error.to_string()))?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: crate::thread_status::SnoozeRequest, responder, _connection| {
                match crate::thread_status::set_snoozed(&request.session_id, request.until) {
                    Ok(()) => responder.respond(serde_json::json!({})),
                    Err(message) => responder.respond_with_error(agent_client_protocol::Error::invalid_params().data(message)),
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: crate::thread_status::SetStatusRequest, responder, _connection| {
                match crate::thread_status::set(&request.session_id, &request.status) {
                    Ok(()) => responder.respond(serde_json::json!({})),
                    Err(message) => responder.respond_with_error(agent_client_protocol::Error::invalid_params().data(message)),
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: crate::thread_index::SearchRequest, responder, connection| {
                connection.spawn(async move {
                    let answer =
                        tokio::task::spawn_blocking(move || crate::thread_index::answer_search(&request)).await;
                    match answer {
                        Ok(Ok(answer)) => responder.respond(answer)?,
                        Ok(Err(message)) => responder.respond_with_error(
                            agent_client_protocol::Error::internal_error().data(message),
                        )?,
                        Err(error) => responder.respond_with_error(
                            agent_client_protocol::Error::internal_error().data(error.to_string()),
                        )?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: crate::checkpoints::ListRequest, responder, connection| {
                connection.spawn(async move {
                    let answer = tokio::task::spawn_blocking(move || crate::checkpoints::answer_list(&request)).await;
                    match answer {
                        Ok(Ok(answer)) => responder.respond(answer)?,
                        Ok(Err(error)) => responder.respond_with_error(error)?,
                        Err(error) => responder.respond_with_error(agent_client_protocol::Error::internal_error().data(error.to_string()))?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: crate::checkpoints::DiffRequest, responder, connection| {
                connection.spawn(async move {
                    let answer = tokio::task::spawn_blocking(move || crate::checkpoints::answer_diff(&request)).await;
                    match answer {
                        Ok(Ok(answer)) => responder.respond(answer)?,
                        Ok(Err(error)) => responder.respond_with_error(error)?,
                        Err(error) => responder.respond_with_error(agent_client_protocol::Error::internal_error().data(error.to_string()))?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: crate::checkpoints::RestoreRequest, responder, connection| {
                    // Only an open built-in thread rewinds its conversation.
                    let state = server.session_handle(&request.session_id).await.map(|(state, _)| state);
                    let clients = server.clients.clone();
                    connection.spawn(async move {
                        match crate::checkpoints::answer_restore(&request, state).await {
                            Ok(answer) => {
                                // A restore may cut the chat without an update.
                                clients.invalidate_cursors(&request.session_id);
                                responder.respond(answer)?
                            }
                            Err(error) => responder.respond_with_error(error)?,
                        }
                        Ok(())
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |_request: server_status::ServerStatusRequest, responder, _connection| {
                    responder.respond(server_status::status(&server))
                }
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
        .on_receive_request(
            async move |request: PullStateRequest, responder, connection| {
                connection.spawn(async move {
                    let asked = request
                        .branch
                        .filter(|branch| !branch.trim().is_empty());
                    let branch = match asked {
                        Some(branch) => Some(branch),
                        // Resolving HEAD runs git, so it does not hold an async worker.
                        None => {
                            let cwd = request.cwd.clone();
                            match tokio::task::spawn_blocking(move || {
                                crate::pull_requests::current_branch(&cwd)
                            })
                            .await
                            {
                                Ok(branch) => branch,
                                Err(error) => {
                                    return responder.respond_with_error(
                                        agent_client_protocol::Error::internal_error()
                                            .data(error.to_string()),
                                    );
                                }
                            }
                        }
                    };
                    let Some(branch) = branch else {
                        return responder.respond(serde_json::json!({
                            "branch": null,
                            "pull": null,
                            "error": { "code": "not_a_repository", "message": "the folder is not on a git branch" }
                        }));
                    };
                    let answer = match crate::pull_requests::for_branch(&request.cwd, &branch).await {
                        Ok(pull) => serde_json::json!({ "branch": branch, "pull": pull }),
                        Err(error) => serde_json::json!({ "branch": branch, "error": error }),
                    };
                    responder.respond(answer)?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: PullMergeCancelRequest, responder, _connection| {
                let cancelled = crate::pull_requests::cancel_merge(&request.merge_id);
                responder.respond(serde_json::json!({ "cancelled": cancelled }))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: PullMergeRequest, responder, connection| {
                // Registered here, in the order requests arrive, so a cancel that
                // follows this request always finds the merge it is for.
                let cancel = request
                    .merge_id
                    .as_deref()
                    .map(crate::pull_requests::register_merge)
                    .unwrap_or_default();
                connection.spawn(async move {
                    let _tracked = crate::pull_requests::MergeGuard(request.merge_id.clone());
                    let wait = std::time::Duration::from_millis(
                        request
                            .timeout_ms
                            .unwrap_or(crate::pull_requests::MAX_MERGE_WAIT_SECS * 1000)
                            .max(1000),
                    );
                    let answer = match crate::pull_requests::merge_branch_when_green(
                        &request.cwd,
                        request.number,
                        request.branch.clone(),
                        wait,
                        &cancel,
                    )
                    .await
                    {
                        Ok(outcome) => serde_json::json!({ "merge": outcome }),
                        Err(error) => serde_json::json!({ "error": error }),
                    };
                    responder.respond(answer)?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: WorktreeCleanupRequest, responder, connection| {
                connection.spawn(async move {
                    let cleaned = crate::worktrees::cleanup_resolving_rename(
                        request.path,
                        request.branch,
                        request.head_sha,
                        request.merged,
                    )
                    .await;
                    match cleaned {
                        Ok(Ok(cleanup)) => responder.respond(serde_json::json!({ "cleanup": cleanup }))?,
                        Ok(Err(error)) => responder.respond_with_error(error.into_acp())?,
                        Err(error) => responder.respond_with_error(
                            agent_client_protocol::Error::internal_error().data(error.to_string()),
                        )?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: PullDiffRequest, responder, connection| {
                connection.spawn(async move {
                    let diff = crate::pull_requests::diff(&request.cwd, request.number).await;
                    responder.respond(match diff {
                        Ok(diff) => serde_json::to_value(diff).unwrap_or_default(),
                        Err(error) => serde_json::json!({ "error": error }),
                    })?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: PullReviewRequest, responder, connection| {
                connection.spawn(async move {
                    let posted = match crate::pull_requests::review_payload(
                        &request.commit_id,
                        &request.event,
                        &request.body,
                        &request.comments,
                    ) {
                        Ok(payload) => {
                            crate::pull_requests::post_review(
                                &request.cwd,
                                &request.repository,
                                request.number,
                                &payload,
                            )
                            .await
                        }
                        Err(error) => Err(error),
                    };
                    responder.respond(match posted {
                        Ok(review) => serde_json::json!({ "review": review }),
                        Err(error) => serde_json::json!({ "error": error }),
                    })?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: PullWatchStartRequest, responder, _connection| {
                server.clients.subscribe(client_id, clients::Topic::PullWatches);
                start_pull_watching(&server.clients);
                let Some(store) = crate::pull_watch::WatchStore::default_store() else {
                    return responder.respond(serde_json::json!({ "error": { "code": "failed", "message": "No config directory" } }));
                };
                let started = if request.link {
                    match crate::pull_requests::watch_view_on(&request.cwd, &request.repo, request.number).await {
                        Ok(value) => store.link(&request.session_id, &request.cwd, &request.repo, request.number, &value, chrono::Utc::now().timestamp()),
                        Err(error) => Err(error.message),
                    }
                } else { store.start(
                    &request.session_id,
                    &request.cwd,
                    request.number,
                    request.interests.unwrap_or_default(),
                    chrono::Utc::now().timestamp(),
                ) };
                responder.respond(match started {
                    Ok(watch) => serde_json::json!({ "watch": watch }),
                    Err(message) => serde_json::json!({ "error": { "code": "failed", "message": message } }),
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: PullWatchStopRequest, responder, _connection| {
                let stopped = crate::pull_watch::WatchStore::default_store()
                    .and_then(|store| store.stop(&request.session_id, request.number, request.remove).ok())
                    .flatten();
                responder.respond(serde_json::json!({ "watch": stopped }))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: PullWatchesRequest, responder, _connection| {
                // The app asks on launch: watches from before a restart
                // carry on from here.
                server.clients.subscribe(client_id, clients::Topic::PullWatches);
                start_pull_watching(&server.clients);
                if let (Some(number), Some(session_id)) = (request.take, request.session_id.as_deref()) {
                    let taken = crate::pull_watch::WatchStore::default_store()
                        .map(|store| store.take(session_id, number))
                        .transpose();
                    return responder.respond(match taken {
                        Ok(Some(Some((watch, events)))) => serde_json::json!({ "watch": watch, "events": events }),
                        Ok(_) => serde_json::json!({ "events": [] }),
                        Err(error) => serde_json::json!({ "error": error.to_string() }),
                    });
                }
                let watches = crate::pull_watch::WatchStore::default_store()
                    .map(|store| store.list(request.session_id.as_deref()))
                    .unwrap_or_default();
                responder.respond(serde_json::json!({ "watches": watches }))
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
                    let session_id = request.session_id.to_string();
                    let peer = server.clients.peer(client_id, &connection);
                    let response = acp_load_session(&server, request, &peer).await?;
                    let response = crate::worktrees::finish_load(response, &session_id);
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
                    if harnesses::close(&server.external, &session_id, &server.remote).await {
                        server.clients.forget_session(&session_id);
                        server.presence.session_closed(&session_id);
                        return responder.respond(CloseSessionResponse::new());
                    }
                    close_builtin(&server, &session_id).await;
                    server.presence.session_closed(&session_id);
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
                    if let Some(external) = harnesses::get(&server.external, &session_id).await {
                        // Setting an option may start the harness.
                        return _connection.spawn(async move {
                            let value = harnesses::config_value(&request.value);
                            let config_id = request.config_id.0.to_string();
                            match external.set_config(&config_id, value).await {
                                Ok(options) => {
                                    // A model or effort picked in a thread is also the
                                    // harness's choice for the next new thread.
                                    if matches!(config_id.as_str(), "model" | "effort") {
                                        let record = external.record();
                                        crate::harness::remember_later(
                                            external.harness.id.clone(),
                                            record.model,
                                            record.effort,
                                        );
                                    }
                                    responder.respond(SetSessionConfigOptionResponse::new(options))?
                                }
                                Err(message) => responder.respond_with_error(
                                    agent_client_protocol::Error::invalid_params().data(message),
                                )?,
                            }
                            Ok(())
                        });
                    }
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
                    if matches!(request.config_id.0.as_ref(), "model" | "effort") {
                        let settings = crate::remote::projection::project_settings(&state);
                        crate::harness::remember_later(
                            crate::harness::BUILTIN_ID.to_owned(),
                            Some(settings.selected_model).filter(|model| !model.is_empty()),
                            Some(settings.reasoning_effort),
                        );
                    }
                    responder.respond(SetSessionConfigOptionResponse::new(config_options))
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            {
                let server = server.clone();
                async move |notification: CancelNotification, _connection| {
                    if let Some(external) =
                        harnesses::get(&server.external, notification.session_id.0.as_ref()).await
                    {
                        external.cancel();
                        return Ok(());
                    }
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
                    let origin = request.meta.as_ref().and_then(|meta| meta.get("rustcode")).and_then(|v| v.get("origin")).cloned().filter(|v| crate::message_origin::from_meta(v).is_some());
                    // A wake turn (the app telling an orchestrator what its delegates did) is
                    // recorded with how many delegates it carried; its tokens and cost are on
                    // the turn's own row (issue 3649).
                    crate::message_origin::record_wake(
                        &session_id,
                        &request
                            .prompt
                            .iter()
                            .filter_map(|block| match block {
                                ContentBlock::Text(text) => Some(text.text.as_str()),
                                _ => None,
                            })
                            .collect::<Vec<_>>()
                            .join("\n"),
                    );
                    // The access the client runs this thread at, kept where the
                    // engine reads it with no app attached (orchestration/local_threads.rs).
                    if let Some(access) = request
                        .meta
                        .as_ref()
                        .and_then(|meta| meta.get("rustcode"))
                        .and_then(|rustcode| rustcode.get("access"))
                        .and_then(serde_json::Value::as_str)
                    {
                        crate::config::save_session_access(&session_id, access);
                    }
                    if let Some(external) = harnesses::get(&server.external, &session_id).await {
                        // The access level the client runs the thread at: a harness that
                        // sandboxes its own shell leaves the sandbox for Full access.
                        if let Some(full) =
                            crate::harness::requested_full_access(request.meta.as_ref())
                        {
                            external.set_full_access(full);
                        }
                        // A device reading the shared thread sees this prompt
                        // too: a harness does not echo it back.
                        let prompt_text = request
                            .prompt
                            .iter()
                            .filter_map(|block| match block {
                                ContentBlock::Text(text) => Some(text.text.clone()),
                                _ => None,
                            })
                            .collect::<Vec<_>>()
                            .join("\n");
                        server.remote.note_prompt(&session_id, &prompt_text, origin.clone());
                        server.remote.note_prompt_applied(&session_id);
                        let remote = server.remote.clone();
                        let turn_session = session_id.clone();
                        // The other clients that have the thread show the
                        // prompt too; the one that sent it already does.
                        echo_prompt(&server.clients.session(&session_id), client_id, &request.prompt, origin.clone());
                        // The turn belongs to the engine, not this connection:
                        // it runs on if the client goes, and only its answer
                        // is lost.
                        spawn_turn(async move {
                            let result = external.prompt(request.prompt).await;
                            remote.note_turn_returned(&turn_session);
                            match result {
                                Ok(stop_reason) => {
                                    responder.respond(PromptResponse::new(stop_reason))?
                                }
                                Err(message) => responder.respond_with_error(
                                    agent_client_protocol::Error::internal_error().data(message),
                                )?,
                            }
                            Ok(())
                        });
                        return Ok(());
                    }
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
                    echo_prompt(&server.clients.session(&session_id), client_id, &request.prompt, origin.clone());

                    // A report only reads the session, so it answers now, beside
                    // a running turn, instead of queueing at the turn gate.
                    if !has_images && commands::is_report(&text) {
                        let session_clients = server.clients.session(&session_id);
                        connection.spawn(async move {
                            match prompt::run_report(&state, &session_id, &text, &session_clients)
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
                    let session_clients = server.clients.session(&session_id);
                    let presence_cwd = cwd.clone();
                    server.presence.turn_started(&session_id, &presence_cwd);
                    // The turn belongs to the engine, not this connection: it
                    // runs on if the client goes, and only its answer is lost.
                    spawn_turn(async move {
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
                            origin,
                            session_clients,
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
                    });
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_to(transport)
        .await;
    server.client_closed(client_id);
    served
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
            clients: clients::Clients::default(),
            client_capability_updates: Arc::new(std::sync::Mutex::new(())),
            presence: presence::Presence::default(),
            remote: remote::RemoteManager::default(),
            share_retry_at: Arc::new(std::sync::Mutex::new(None)),
            external: harnesses::ExternalSessions::default(),
            auto_sharing: Arc::new(AtomicBool::new(false)),
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
    fn session_info_carries_the_parent_recorded_in_the_thread() {
        let id = crate::config::create_detached_session();
        let directory = crate::config::get_active_session_dir(&id).expect("session dir");
        crate::config::save_session_parent(
            &id,
            &rustcode_session::SessionParent {
                id: Some("parent-session".into()),
                title: Some("Update the sidebar".into()),
                relationship: Some(rustcode_session::SessionRelationship::Fork),
            },
        );
        let meta = crate::config::SessionMeta {
            path: directory.join("history.json"),
            title: "Research: the sidebar".into(),
            when: "2026-09-24T00:00:00Z".into(),
            message_count: 2,
            workspace_cwd: None,
        };
        let info = session_info(&meta, None);
        let parent = info
            .meta
            .and_then(|meta| meta.get("rustcode").cloned())
            .and_then(|rustcode| rustcode.get("parent").cloned())
            .expect("parent meta");
        assert_eq!(parent["sessionId"], "parent-session");
        assert_eq!(parent["title"], "Update the sidebar");
        assert_eq!(parent["relationship"], "fork");
    }

    #[test]
    fn session_info_lists_a_recorded_repository_without_asking_git() {
        let id = crate::config::create_detached_session();
        let directory = crate::config::get_active_session_dir(&id).expect("session dir");
        let repo = PathBuf::from("/Users/me/code/rustcode");
        crate::config::save_session_repo(&id, Some(&repo));
        // A folder that is not a git repository: only the record can name one,
        // and listing reads the record instead of running git (#3162).
        let cwd = tempfile::tempdir().unwrap();
        let meta = crate::config::SessionMeta {
            path: directory.join("history.json"),
            title: "Worktree thread".into(),
            when: "2026-09-24T00:00:00Z".into(),
            message_count: 1,
            workspace_cwd: Some(cwd.path().to_path_buf()),
        };
        let workspace = rustcode_session::SessionWorkspace {
            cwd: cwd.path().to_path_buf(),
            additional_directories: Vec::new(),
            task_workspace_id: None,
        };
        let info = session_info(&meta, Some(workspace));
        let meta = info.meta.expect("thread meta");
        assert_eq!(meta["rustcode"]["repo"], serde_json::json!(repo));
    }

    #[test]
    fn session_info_does_not_publish_a_repository_for_a_thread_with_no_workspace() {
        // A legacy thread inherits the engine's cwd; publishing that folder's
        // repository would give it another project's scope (#3162).
        let id = crate::config::create_detached_session();
        let directory = crate::config::get_active_session_dir(&id).expect("session dir");
        crate::config::save_session_repo(
            &id,
            Some(std::path::Path::new("/Users/me/code/rustcode")),
        );
        let meta = crate::config::SessionMeta {
            path: directory.join("history.json"),
            title: "Legacy thread".into(),
            when: "2026-09-24T00:00:00Z".into(),
            message_count: 1,
            workspace_cwd: None,
        };
        let info = session_info(&meta, None);
        let meta = info.meta.expect("thread meta");
        assert_eq!(meta["workspaceUnknown"], serde_json::json!(true));
        assert!(meta["rustcode"].get("repo").is_none());
    }

    /// A legacy thread has no folder of its own. Listing the engine's working
    /// directory gave it `/` under a service manager, or the app's install
    /// folder on Windows, and that folder then read as a project of the user's
    /// (#3425): a listing falls back to the home folder instead.
    #[test]
    fn a_thread_with_no_workspace_lists_the_home_folder_not_the_engine_folder() {
        let id = crate::config::create_detached_session();
        let directory = crate::config::get_active_session_dir(&id).expect("session dir");
        let meta = crate::config::SessionMeta {
            path: directory.join("history.json"),
            title: "Legacy thread".into(),
            when: "2026-09-24T00:00:00Z".into(),
            message_count: 1,
            workspace_cwd: None,
        };
        let info = session_info(&meta, None);
        assert_eq!(home_folder().as_deref(), Some(info.cwd.as_path()));
        assert_eq!(
            info.meta.expect("thread meta")["workspaceUnknown"],
            serde_json::json!(true)
        );
    }

    /// The choice a start and a listing make when no folder is named, as a pure
    /// function: the engine's own folder is never an answer, the home folder is.
    #[test]
    fn a_start_with_no_folder_runs_in_the_home_folder() {
        let home = std::path::Path::new("/Users/me");
        assert_eq!(
            start_folder(std::path::Path::new(""), Some(home)),
            Some(PathBuf::from("/Users/me"))
        );
        assert_eq!(
            start_folder(std::path::Path::new("/Users/me/code/app"), Some(home)),
            Some(PathBuf::from("/Users/me/code/app")),
            "a folder the request names is the folder it starts in"
        );
        assert_eq!(
            start_folder(std::path::Path::new(""), None),
            None,
            "no folder and no home folder has no answer, never the engine's own"
        );
        assert_eq!(
            listed_folder(None, Some(home)),
            (PathBuf::from("/Users/me"), true)
        );
        assert_eq!(
            listed_folder(Some(PathBuf::from("/Users/me/code/app")), Some(home)),
            (PathBuf::from("/Users/me/code/app"), false)
        );
        assert_eq!(listed_folder(None, None), (PathBuf::new(), true));
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
    fn acp_event_stream_never_streams_a_stop_marker_as_text() {
        // The live reply buffer carries the marker before the turn ends; a
        // provider failure that never reached the model showed it raw.
        let mut stream = AcpEventStream::new();
        stream.updates(crate::network::AgentUiEvent::ProviderError {
            message: "credential resolution failed".to_string(),
        });
        let streamed = stream.updates(crate::network::AgentUiEvent::TextDelta {
            text: "[harness: turn stopped — provider_error]".to_string(),
        });
        assert!(streamed.is_empty(), "{streamed:?}");
        let finished = stream.updates(crate::network::AgentUiEvent::TurnFinished {
            content: "[harness: turn stopped — provider_error]".to_string(),
            completed: false,
            timing: None,
        });
        assert!(
            finished.is_empty(),
            "the provider's message already said it: {finished:?}"
        );
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

    // #3516: a replayed user message says who sent it, beside its time, and
    // a tool's result in the user role says nothing.
    #[test]
    fn replayed_user_messages_carry_their_origin() {
        let mut task = crate::app::ChatMessage::new(
            "user",
            "From thread “Ship the tree” (agent):\n\nReview the rules.",
        );
        task.timestamp = "2026-11-11T09:16:01+01:00".into();
        let wake = crate::app::ChatMessage::new(
            "user",
            "Delegate finished: Review the rows\n\nFinished:\n\n- link_id parent-1/2 · done · “Review the rows” · session child-7\n  answer: Done.\n\n<!-- rustcode:wake -->",
        );
        let typed = crate::app::ChatMessage::new("user", "thanks");
        let result = crate::app::ChatMessage::new("user", "<tool_result>ok</tool_result>");

        let updates = replay_history_updates(&[task, wake, typed, result], None);

        let metas = updates
            .iter()
            .map(|update| match update {
                SessionUpdate::UserMessageChunk(chunk) => chunk.meta.clone(),
                other => panic!("expected user messages: {other:?}"),
            })
            .collect::<Vec<_>>();
        let origin =
            |index: usize| metas[index].as_ref().expect("meta")["rustcode"]["origin"].clone();
        assert_eq!(
            origin(0),
            serde_json::json!({ "kind": "agent", "threads": [{ "title": "Ship the tree" }] })
        );
        assert_eq!(
            metas[0].as_ref().unwrap()["rustcode"]["at"],
            "2026-11-11T09:16:01+01:00"
        );
        assert_eq!(
            origin(1),
            serde_json::json!({
                "kind": "delegate",
                "threads": [{
                    "sessionId": "child-7",
                    "title": "Review the rows",
                    "status": "done",
                    "linkId": "parent-1/2",
                }]
            })
        );
        assert_eq!(origin(2), serde_json::json!({ "kind": "user" }));
        assert!(
            metas[3]
                .as_ref()
                .is_none_or(|meta| meta["rustcode"].get("origin").is_none()),
            "a tool result has no sender"
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
    fn new_session_meta_sets_builtin_effort_and_ignores_unknown_values() {
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
        let current_effort = |state: &crate::app::AppState| {
            build_session_config_options(state)
                .iter()
                .find(|option| option.id.0.as_ref() == "effort")
                .and_then(|option| match &option.kind {
                    SessionConfigKind::Select(select) => Some(select.current_value.0.to_string()),
                    _ => None,
                })
        };

        apply_requested_model_and_effort(
            &mut state,
            &crate::harness::HarnessRequest {
                harness: None,
                account: None,
                model: Some("no-such-model".into()),
                effort: Some("low".into()),
            },
        );
        assert_eq!(current_effort(&state).as_deref(), Some("low"));

        apply_requested_model_and_effort(
            &mut state,
            &crate::harness::HarnessRequest {
                harness: None,
                account: None,
                model: None,
                effort: Some("ludicrous".into()),
            },
        );
        assert_eq!(current_effort(&state).as_deref(), Some("low"));
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

    /// What a test client got for one session.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    enum Seen {
        /// A status broadcast, which every client gets.
        Status,
        /// Text of a reply: a report streams it through the session's clients.
        Message,
        Other,
    }

    /// One test client: it records every `session/update` it gets, answers
    /// any request the engine sends it with `{"client": name}`, and stays
    /// connected until `close` fires.
    async fn test_client(
        transport: agent_client_protocol::Channel,
        name: &'static str,
        seen: tokio::sync::mpsc::UnboundedSender<(String, Seen)>,
        connected: tokio::sync::oneshot::Sender<agent_client_protocol::ConnectionTo<Agent>>,
        close: tokio::sync::oneshot::Receiver<()>,
    ) -> Result<(), agent_client_protocol::Error> {
        agent_client_protocol::Client
            .builder()
            .on_receive_notification(
                async move |notification: SessionNotification, _connection| {
                    let kind = match &notification.update {
                        SessionUpdate::SessionInfoUpdate(info)
                            if info
                                .meta
                                .as_ref()
                                .and_then(|meta| meta.get("rustcode"))
                                .and_then(|rustcode| rustcode.get("status"))
                                .is_some() =>
                        {
                            Seen::Status
                        }
                        SessionUpdate::AgentMessageChunk(_) => Seen::Message,
                        _ => Seen::Other,
                    };
                    let _ = seen.send((notification.session_id.to_string(), kind));
                    Ok(())
                },
                agent_client_protocol::on_receive_notification!(),
            )
            .on_receive_request(
                async move |_request: agent_client_protocol::UntypedMessage,
                            responder,
                            _connection| {
                    responder.respond(serde_json::json!({ "client": name }))
                },
                agent_client_protocol::on_receive_request!(),
            )
            .connect_with(transport, async move |connection| {
                let _ = connected.send(connection);
                let _ = close.await;
                Ok(())
            })
            .await
    }

    fn initialize_with(meta: serde_json::Value) -> InitializeRequest {
        InitializeRequest::new(agent_client_protocol::schema::ProtocolVersion::V1)
            .client_capabilities(
                serde_json::from_value::<agent_client_protocol::schema::v1::ClientCapabilities>(
                    serde_json::json!({ "_meta": meta }),
                )
                .expect("client capabilities"),
            )
    }

    fn report_prompt(session_id: &str) -> PromptRequest {
        PromptRequest::new(
            session_id.to_owned(),
            vec![ContentBlock::Text(TextContent::new("/context"))],
        )
    }

    /// A connection ended because its client hung up. A status broadcast goes
    /// to every client, so one can be on its way while the client leaves; the
    /// write then fails on the closed channel, and the connection ends with
    /// that error instead of `Ok`. Any other error fails the test.
    fn ends_with_its_client(served: Result<(), agent_client_protocol::Error>, name: &str) {
        let Err(error) = served else {
            return;
        };
        let hung_up = error
            .data
            .as_ref()
            .and_then(serde_json::Value::as_str)
            .is_some_and(|data| data.contains("receiver is gone"));
        assert!(hung_up, "{name}'s connection ends cleanly: {error:?}");
    }

    /// What a client got, status broadcasts left out: they go to everyone.
    fn seen_by(
        seen: &mut tokio::sync::mpsc::UnboundedReceiver<(String, Seen)>,
    ) -> HashMap<String, Vec<Seen>> {
        let mut by_session: HashMap<String, Vec<Seen>> = HashMap::new();
        while let Ok((session_id, kind)) = seen.try_recv() {
            if kind != Seen::Status {
                by_session.entry(session_id).or_default().push(kind);
            }
        }
        by_session
    }

    /// Polling a connection's handler chain is deep in a debug build: the
    /// engine runs it on the main thread's 8 MB stack, a test thread has 2 MB.
    fn on_a_large_stack(test: impl std::future::Future<Output = ()> + Send + 'static) {
        std::thread::Builder::new()
            .stack_size(64 << 20)
            .spawn(move || {
                tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                    .expect("runtime")
                    .block_on(test)
            })
            .expect("test thread")
            .join()
            .expect("the test passes");
    }

    #[test]
    fn two_connections_keep_their_own_sessions_and_capabilities() {
        on_a_large_stack(two_connections());
    }

    async fn two_connections() {
        let server = AcpServer::new(false).expect("server");
        let cwd = tempfile::tempdir().expect("cwd");
        let (a_engine, a_transport) = agent_client_protocol::Channel::duplex();
        let (b_engine, b_transport) = agent_client_protocol::Channel::duplex();
        let (a_seen_tx, mut a_seen) = tokio::sync::mpsc::unbounded_channel();
        let (b_seen_tx, mut b_seen) = tokio::sync::mpsc::unbounded_channel();
        let (a_ready_tx, a_ready) =
            tokio::sync::oneshot::channel::<agent_client_protocol::ConnectionTo<Agent>>();
        let (b_ready_tx, b_ready) =
            tokio::sync::oneshot::channel::<agent_client_protocol::ConnectionTo<Agent>>();
        let (close_a, a_closed) = tokio::sync::oneshot::channel();
        let (close_b, b_closed) = tokio::sync::oneshot::channel();

        let script = async {
            let a = a_ready.await.expect("client a connects");
            let b = b_ready.await.expect("client b connects");
            // A is an app with the preview and orchestration; B advertises nothing.
            a.send_request(initialize_with(
                serde_json::json!({ "rustcode": { "preview": 1, "orchestration": 3, "threads": 1, "visualizations": 2, "smartUi": 1 } }),
            ))
            .block_task()
            .await
            .expect("a initializes");
            b.send_request(initialize_with(serde_json::json!({})))
                .block_task()
                .await
                .expect("b initializes");
            #[cfg(unix)]
            assert!(server.remote.can_host_threads());
            let a_session = a
                .send_request(NewSessionRequest::new(cwd.path().to_path_buf()))
                .block_task()
                .await
                .expect("a creates a session")
                .session_id
                .to_string();
            let b_session = b
                .send_request(NewSessionRequest::new(cwd.path().to_path_buf()))
                .block_task()
                .await
                .expect("b creates a session")
                .session_id
                .to_string();

            // Each session took its creator's capabilities.
            let (a_state, b_state) = {
                let sessions = server.sessions.lock().await;
                (
                    Arc::clone(&sessions[&a_session].state),
                    Arc::clone(&sessions[&b_session].state),
                )
            };
            let a_preview = {
                let state = a_state.lock().await;
                assert_eq!(state.orchestration_version, 3);
                assert_eq!(state.visualization_level, 2);
                assert_eq!(state.smart_ui, crate::app::SmartUi::On);
                assert!(state.orchestration_client.is_some());
                assert!(!state.local_orchestration);
                state
                    .preview_client
                    .clone()
                    .expect("a's session has a preview")
            };
            {
                let state = b_state.lock().await;
                assert_eq!(state.orchestration_version, 0);
                assert_eq!(state.visualization_level, 0);
                assert_eq!(state.smart_ui, crate::app::SmartUi::Unsupported);
                assert!(state.orchestration_client.is_none());
                // No app has this session, so the engine serves its reads
                // itself (#3422), even though the other client is an app.
                assert!(state.local_orchestration);
                assert!(state.preview_client.is_none());
            }
            // And its client answers for it.
            assert_eq!(
                a_preview
                    .request(serde_json::json!({ "operation": { "action": "inspect" } }))
                    .await
                    .expect("a answers its session's preview"),
                serde_json::json!({ "client": "a" })
            );

            // A report streams through the session's clients: only its own.
            a.send_request(report_prompt(&a_session))
                .block_task()
                .await
                .expect("a's report");
            b.send_request(report_prompt(&b_session))
                .block_task()
                .await
                .expect("b's report");

            // A leaves. B, its session and A's session all go on.
            let _ = close_a.send(());
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while server.clients.count() != 1 {
                assert!(std::time::Instant::now() < deadline, "a never disconnected");
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            #[cfg(unix)]
            assert!(!server.remote.can_host_threads());
            {
                let sessions = server.sessions.lock().await;
                assert!(sessions.contains_key(&a_session));
                assert!(sessions.contains_key(&b_session));
            }
            assert!(
                a_preview
                    .request(serde_json::json!({ "operation": { "action": "inspect" } }))
                    .await
                    .is_err(),
                "a request has no client once a's is gone"
            );
            b.send_request(report_prompt(&b_session))
                .block_task()
                .await
                .expect("b still runs its session");
            let _ = close_b.send(());
            (a_session, b_session)
        };

        let (served_a, served_b, client_a, client_b, (a_session, b_session)) = tokio::join!(
            serve_connection(&server, a_engine),
            serve_connection(&server, b_engine),
            test_client(a_transport, "a", a_seen_tx, a_ready_tx, a_closed),
            test_client(b_transport, "b", b_seen_tx, b_ready_tx, b_closed),
            script,
        );
        ends_with_its_client(served_a, "a");
        ends_with_its_client(served_b, "b");
        client_a.expect("client a");
        client_b.expect("client b");

        let messages = |seen: &HashMap<String, Vec<Seen>>, session: &str| {
            seen.get(session).map_or(0, |kinds| {
                kinds.iter().filter(|kind| **kind == Seen::Message).count()
            })
        };
        let a_got = seen_by(&mut a_seen);
        let b_got = seen_by(&mut b_seen);
        assert_eq!(a_got.keys().collect::<Vec<_>>(), vec![&a_session]);
        assert_eq!(b_got.keys().collect::<Vec<_>>(), vec![&b_session]);
        assert_eq!(messages(&a_got, &a_session), 1, "a's report reached a");
        assert_eq!(
            messages(&b_got, &b_session),
            2,
            "both of b's reports reached b"
        );
        assert_eq!(server.clients.count(), 0);
        // Closing connections ended nothing: only the engine's shutdown does.
        assert!(server.sessions.lock().await.contains_key(&a_session));
        server.shutdown().await;
        assert!(server.sessions.lock().await.is_empty());
    }

    /// A chat-completions provider whose first streamed reply sends `first`,
    /// then waits for `release` to send `rest` and finish. Any later request
    /// is answered at once.
    async fn held_provider(
        first: &'static str,
        rest: &'static str,
    ) -> (String, tokio::sync::oneshot::Sender<()>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        async fn read_request(socket: &mut tokio::net::TcpStream) {
            let mut request = Vec::new();
            let mut buffer = [0_u8; 4_096];
            loop {
                let Ok(read) = socket.read(&mut buffer).await else {
                    return;
                };
                if read == 0 {
                    return;
                }
                request.extend_from_slice(&buffer[..read]);
                let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") else {
                    continue;
                };
                let headers = String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
                let length = headers
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .and_then(|value| value.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                if request.len() >= end + 4 + length {
                    return;
                }
            }
        }
        fn delta(text: &str) -> String {
            format!(
                "data: {}\n\n",
                serde_json::json!({ "choices": [{ "delta": { "content": text } }] })
            )
        }
        const HEAD: &str =
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";
        const END: &str =
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind provider");
        let address = listener.local_addr().expect("provider address");
        let (release, released) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(async move {
            let mut released = Some(released);
            while let Ok((mut socket, _)) = listener.accept().await {
                let held = released.take();
                tokio::spawn(async move {
                    read_request(&mut socket).await;
                    let _ = socket.write_all(HEAD.as_bytes()).await;
                    match held {
                        Some(released) => {
                            let _ = socket.write_all(delta(first).as_bytes()).await;
                            let _ = socket.flush().await;
                            let _ = released.await;
                            let _ = socket.write_all(delta(rest).as_bytes()).await;
                        }
                        None => {
                            let _ = socket.write_all(delta("ok").as_bytes()).await;
                        }
                    }
                    let _ = socket.write_all(END.as_bytes()).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        (format!("http://{address}/v1/chat/completions"), release)
    }

    /// Run `test` with MCP servers of its own, none: a real turn otherwise
    /// starts the configured ones (the default config names one run by `npx`).
    async fn without_mcp(test: impl std::future::Future<Output = ()>) {
        crate::mcp::DIRECT_MCP_REGISTRY
            .scope(Default::default(), test)
            .await;
    }

    /// Serve a connection on a thread of its own, as the server does.
    fn serve_on_thread(
        server: &AcpServer,
        transport: agent_client_protocol::Channel,
    ) -> tokio::sync::oneshot::Receiver<Result<(), agent_client_protocol::Error>> {
        let (done, served) = tokio::sync::oneshot::channel();
        let server = server.clone();
        spawn_on_large_stack("test-connection", async move {
            let _ = done.send(serve_connection(&server, transport).await);
        });
        served
    }

    /// Point a session at `endpoint`, with an earlier exchange in its history.
    async fn use_provider(server: &AcpServer, session_id: &str, endpoint: &str) {
        use crate::config::{ApiProtocol, ModelProfile};
        let state = Arc::clone(&server.sessions.lock().await[session_id].state);
        let mut state = state.lock().await;
        state.api_base_url = endpoint.to_owned();
        state.model_name = "held-test".to_owned();
        state.config.models = vec![ModelProfile {
            name: state.model_name.clone(),
            url: endpoint.to_owned(),
            model: state.model_name.clone(),
            api_protocol: Some(ApiProtocol::ChatCompletions),
            context_window: Some(8_192),
            ..ModelProfile::default()
        }];
        state.record_function_calling_support(endpoint, false);
        state
            .history
            .push(crate::app::ChatMessage::new("user", "an earlier question"));
        state.history.push(crate::app::ChatMessage::new(
            "assistant",
            "an earlier answer",
        ));
    }

    /// A client that sends every `session/update` it gets to `updates` and
    /// stays connected until `close` fires.
    async fn recording_client(
        transport: agent_client_protocol::Channel,
        updates: tokio::sync::mpsc::UnboundedSender<SessionUpdate>,
        connected: tokio::sync::oneshot::Sender<agent_client_protocol::ConnectionTo<Agent>>,
        close: tokio::sync::oneshot::Receiver<()>,
    ) -> Result<(), agent_client_protocol::Error> {
        agent_client_protocol::Client
            .builder()
            .on_receive_notification(
                async move |notification: SessionNotification, _connection| {
                    let _ = updates.send(notification.update);
                    Ok(())
                },
                agent_client_protocol::on_receive_notification!(),
            )
            .on_receive_request(
                async move |_request: agent_client_protocol::UntypedMessage,
                            responder,
                            _connection| {
                    responder.respond(serde_json::json!({}))
                },
                agent_client_protocol::on_receive_request!(),
            )
            .connect_with(transport, async move |connection| {
                let _ = connected.send(connection);
                let _ = close.await;
                Ok(())
            })
            .await
    }

    /// The text of a message chunk: `("user" | "agent", text)`.
    fn said(update: &SessionUpdate) -> Option<(&'static str, String)> {
        let (who, chunk) = match update {
            SessionUpdate::UserMessageChunk(chunk) => ("user", chunk),
            SessionUpdate::AgentMessageChunk(chunk) => ("agent", chunk),
            _ => return None,
        };
        match &chunk.content {
            ContentBlock::Text(text) => Some((who, text.text.clone())),
            _ => None,
        }
    }

    /// Wait for the message chunk that contains `text`, collecting every
    /// chunk said on the way.
    async fn wait_for_text(
        updates: &mut tokio::sync::mpsc::UnboundedReceiver<SessionUpdate>,
        text: &str,
        said_so_far: &mut Vec<(&'static str, String)>,
    ) {
        let deadline = std::time::Duration::from_secs(20);
        tokio::time::timeout(deadline, async {
            while let Some(update) = updates.recv().await {
                if let Some(chunk) = said(&update) {
                    let found = chunk.1.contains(text);
                    said_so_far.push(chunk);
                    if found {
                        return;
                    }
                }
            }
            panic!("the client closed before {text:?}");
        })
        .await
        .unwrap_or_else(|_| panic!("no {text:?} within {deadline:?}: {said_so_far:?}"));
    }

    async fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !done() {
            assert!(
                std::time::Instant::now() < deadline,
                "{what} never happened"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    #[test]
    fn a_load_does_not_attach_a_stale_replay_after_repeated_turn_changes() {
        on_a_large_stack(without_mcp(replay_retries_until_stable()));
    }

    async fn replay_retries_until_stable() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let server = AcpServer::new(false).expect("server");
        let (engine, transport) = agent_client_protocol::Channel::duplex();
        let served = serve_on_thread(&server, engine);
        let (updates_tx, _updates) = tokio::sync::mpsc::unbounded_channel();
        let (ready_tx, ready) = tokio::sync::oneshot::channel();
        let (close_tx, closed) = tokio::sync::oneshot::channel();
        let client = tokio::spawn(recording_client(transport, updates_tx, ready_tx, closed));
        let connection = ready.await.expect("client connects");
        connection
            .send_request(initialize_with(
                serde_json::json!({ "rustcode": { "preview": 1 } }),
            ))
            .block_task()
            .await
            .expect("client initializes");
        let peer = server
            .clients
            .newest_with(clients::Capability::Preview)
            .expect("initialized peer");

        let session_id = "replay-retry-test";
        let session_clients = peer.clients().session(session_id);
        let reads = Arc::new(AtomicUsize::new(0));
        let result = attach_with_replay(
            &peer,
            session_id,
            None,
            {
                let reads = Arc::clone(&reads);
                move || {
                    let reads = Arc::clone(&reads);
                    let session_clients = session_clients.clone();
                    async move {
                        if reads.fetch_add(1, Ordering::SeqCst) < ATTACH_ATTEMPTS {
                            drop(session_clients.begin_turn(0, None));
                        }
                        rustcode_core::History::default()
                    }
                }
            },
            |_| Vec::new(),
        )
        .await;

        assert!(
            result.is_err(),
            "unstable replay must be retried or refused"
        );
        assert_eq!(reads.load(Ordering::SeqCst), ATTACH_ATTEMPTS);
        assert!(
            peer.clients().session(session_id).requester(None).is_none(),
            "a stale replay never attaches the client"
        );

        let _ = close_tx.send(());
        client
            .await
            .expect("client task")
            .expect("client disconnects cleanly");
        served
            .await
            .expect("server connection")
            .expect("connection closes cleanly");
        server.shutdown().await;
    }

    #[test]
    fn a_turn_outlives_its_client_and_a_client_that_loads_it_mid_turn_gets_all_of_it() {
        on_a_large_stack(without_mcp(turn_outlives_client()));
    }

    async fn turn_outlives_client() {
        let server = AcpServer::new(false).expect("server");
        let cwd = tempfile::tempdir().expect("cwd");
        let cwd_path = std::fs::canonicalize(cwd.path()).expect("cwd");
        let (endpoint, release) = held_provider("the first half, ", "then the rest").await;

        // A starts a thread, prompts, and leaves once the reply has begun.
        let (a_engine, a_transport) = agent_client_protocol::Channel::duplex();
        let (a_updates_tx, mut a_updates) = tokio::sync::mpsc::unbounded_channel();
        let (a_ready_tx, a_ready) = tokio::sync::oneshot::channel();
        let (close_a, a_closed) = tokio::sync::oneshot::channel();
        let a_serving = serve_on_thread(&server, a_engine);
        let a_client = tokio::spawn(recording_client(
            a_transport,
            a_updates_tx,
            a_ready_tx,
            a_closed,
        ));
        let a = a_ready.await.expect("a connects");
        a.send_request(initialize_with(serde_json::json!({})))
            .block_task()
            .await
            .expect("a initializes");
        let session_id = a
            .send_request(NewSessionRequest::new(cwd_path.clone()))
            .block_task()
            .await
            .expect("a creates a session")
            .session_id
            .to_string();
        use_provider(&server, &session_id, &endpoint).await;
        // Not awaited: A leaves before the turn answers it.
        let _unanswered = a.send_request(PromptRequest::new(
            session_id.clone(),
            vec![ContentBlock::Text(TextContent::new("do the thing"))],
        ));
        let mut a_said = Vec::new();
        wait_for_text(&mut a_updates, "the first half", &mut a_said).await;
        let _ = close_a.send(());
        a_client
            .await
            .expect("a's client")
            .expect("a's client ends cleanly");
        ends_with_its_client(a_serving.await.expect("a's connection"), "a");
        assert_eq!(server.clients.count(), 0);
        assert!(
            server.clients.turn_running(&session_id),
            "closing the client did not stop the turn"
        );

        // B loads the thread mid-turn: the history, the turn so far, then live.
        let (b_engine, b_transport) = agent_client_protocol::Channel::duplex();
        let (b_updates_tx, mut b_updates) = tokio::sync::mpsc::unbounded_channel();
        let (b_ready_tx, b_ready) = tokio::sync::oneshot::channel();
        let (close_b, b_closed) = tokio::sync::oneshot::channel();
        let b_serving = serve_on_thread(&server, b_engine);
        let b_client = tokio::spawn(recording_client(
            b_transport,
            b_updates_tx,
            b_ready_tx,
            b_closed,
        ));
        let b = b_ready.await.expect("b connects");
        b.send_request(initialize_with(serde_json::json!({})))
            .block_task()
            .await
            .expect("b initializes");
        b.send_request(LoadSessionRequest::new(
            session_id.clone(),
            cwd_path.clone(),
        ))
        .block_task()
        .await
        .expect("b loads the running thread");
        let mut b_said = Vec::new();
        wait_for_text(&mut b_updates, "the first half", &mut b_said).await;
        assert_eq!(
            b_said,
            vec![
                ("user", "an earlier question".to_owned()),
                ("agent", "an earlier answer".to_owned()),
                ("user", "do the thing".to_owned()),
                ("agent", "the first half, ".to_owned()),
            ],
            "the transcript, then the turn so far, each once"
        );

        let _ = release.send(());
        wait_for_text(&mut b_updates, "then the rest", &mut b_said).await;
        wait_until("the turn ends", || {
            !server.clients.turn_running(&session_id)
        })
        .await;
        let answer = {
            let state = Arc::clone(&server.sessions.lock().await[&session_id].state);
            let state = state.lock().await;
            state.history.last().map(|message| message.content.clone())
        };
        assert_eq!(
            answer.as_deref(),
            Some("the first half, then the rest"),
            "the turn finished with nobody connected for part of it"
        );

        let _ = close_b.send(());
        b_client
            .await
            .expect("b's client")
            .expect("b's client ends cleanly");
        ends_with_its_client(b_serving.await.expect("b's connection"), "b");
        server.shutdown().await;
    }

    #[test]
    fn two_clients_of_one_thread_see_it_live() {
        on_a_large_stack(without_mcp(two_clients_one_thread()));
    }

    async fn two_clients_one_thread() {
        let server = AcpServer::new(false).expect("server");
        let cwd = tempfile::tempdir().expect("cwd");
        let cwd_path = std::fs::canonicalize(cwd.path()).expect("cwd");
        let (endpoint, release) = held_provider("streaming to both, ", "still both").await;
        let mut clients = Vec::new();
        for _ in 0..2 {
            let (engine, transport) = agent_client_protocol::Channel::duplex();
            let (updates_tx, updates) = tokio::sync::mpsc::unbounded_channel();
            let (ready_tx, ready) = tokio::sync::oneshot::channel();
            let (close, closed) = tokio::sync::oneshot::channel();
            let serving = serve_on_thread(&server, engine);
            let client = tokio::spawn(recording_client(transport, updates_tx, ready_tx, closed));
            let connection = ready.await.expect("connects");
            connection
                .send_request(initialize_with(serde_json::json!({})))
                .block_task()
                .await
                .expect("initializes");
            clients.push((connection, updates, close, serving, client));
        }
        let session_id = clients[0]
            .0
            .send_request(NewSessionRequest::new(cwd_path.clone()))
            .block_task()
            .await
            .expect("a creates a session")
            .session_id
            .to_string();
        use_provider(&server, &session_id, &endpoint).await;
        clients[1]
            .0
            .send_request(LoadSessionRequest::new(
                session_id.clone(),
                cwd_path.clone(),
            ))
            .block_task()
            .await
            .expect("b loads the open thread");

        let prompt = clients[0].0.send_request(PromptRequest::new(
            session_id.clone(),
            vec![ContentBlock::Text(TextContent::new("tell us both"))],
        ));
        let mut said = [Vec::new(), Vec::new()];
        for ((_, updates, ..), said) in clients.iter_mut().zip(&mut said) {
            wait_for_text(updates, "streaming to both", said).await;
        }
        let _ = release.send(());
        for ((_, updates, ..), said) in clients.iter_mut().zip(&mut said) {
            wait_for_text(updates, "still both", said).await;
        }
        assert_eq!(
            prompt.block_task().await.expect("the prompt").stop_reason,
            StopReason::EndTurn
        );
        // B is shown A's prompt; A shows its own already.
        let prompt_shown = |said: &[(&str, String)]| {
            said.iter()
                .any(|(who, text)| *who == "user" && text == "tell us both")
        };
        assert!(!prompt_shown(&said[0]), "a: {:?}", said[0]);
        assert!(prompt_shown(&said[1]), "b: {:?}", said[1]);

        for (_, _, close, serving, client) in clients {
            let _ = close.send(());
            client.await.expect("client").expect("client ends cleanly");
            serving
                .await
                .expect("connection")
                .expect("connection ends cleanly");
        }
        server.shutdown().await;
    }

    /// A thread one client makes is the machine's, not its own: every client
    /// that lists threads is told about it, so no sidebar waits for a refresh.
    #[test]
    fn every_client_that_lists_threads_hears_about_a_new_one() {
        on_a_large_stack(thread_created_reaches_every_listing_client());
    }

    async fn thread_created_reaches_every_listing_client() {
        let server = AcpServer::new(false).expect("server");
        let cwd = tempfile::tempdir().expect("cwd");
        let cwd_path = std::fs::canonicalize(cwd.path()).expect("cwd");

        // B lists threads and keeps every creation it is told about.
        let (b_engine, b_transport) = agent_client_protocol::Channel::duplex();
        let (created_tx, mut created) = tokio::sync::mpsc::unbounded_channel();
        let (title_tx, mut titles) = tokio::sync::mpsc::unbounded_channel();
        let (deleted_tx, mut deleted) = tokio::sync::mpsc::unbounded_channel();
        let (b_ready_tx, b_ready) = tokio::sync::oneshot::channel();
        let (close_b, b_closed) = tokio::sync::oneshot::channel();
        let b_serving = serve_on_thread(&server, b_engine);
        let b_client = tokio::spawn(async move {
            agent_client_protocol::Client
                .builder()
                .on_receive_notification(
                    async move |notification: ThreadCreatedNotification, _connection| {
                        let _ = created_tx.send(notification);
                        Ok(())
                    },
                    agent_client_protocol::on_receive_notification!(),
                )
                .on_receive_notification(
                    async move |notification: SessionNotification, _connection| {
                        let _ = title_tx.send(notification);
                        Ok(())
                    },
                    agent_client_protocol::on_receive_notification!(),
                )
                .on_receive_notification(
                    async move |notification: ThreadDeletedNotification, _connection| {
                        let _ = deleted_tx.send(notification);
                        Ok(())
                    },
                    agent_client_protocol::on_receive_notification!(),
                )
                .connect_with(b_transport, async move |connection| {
                    let _ = b_ready_tx.send(connection);
                    let _ = b_closed.await;
                    Ok(())
                })
                .await
        });
        let b = b_ready.await.expect("b connects");
        b.send_request(initialize_with(
            serde_json::json!({ "rustcode": { "threads": 1 } }),
        ))
        .block_task()
        .await
        .expect("b initializes");

        // A makes a thread, without listing threads itself.
        let (a_engine, a_transport) = agent_client_protocol::Channel::duplex();
        let (a_ready_tx, a_ready) = tokio::sync::oneshot::channel();
        let (close_a, a_closed) = tokio::sync::oneshot::channel();
        let a_serving = serve_on_thread(&server, a_engine);
        let a_client = tokio::spawn(async move {
            agent_client_protocol::Client
                .builder()
                .on_receive_notification(
                    async move |_notification: SessionNotification, _connection| Ok(()),
                    agent_client_protocol::on_receive_notification!(),
                )
                .connect_with(a_transport, async move |connection| {
                    let _ = a_ready_tx.send(connection);
                    let _ = a_closed.await;
                    Ok(())
                })
                .await
        });
        let a = a_ready.await.expect("a connects");
        a.send_request(initialize_with(serde_json::json!({})))
            .block_task()
            .await
            .expect("a initializes");
        let session_id = a
            .send_request(NewSessionRequest::new(cwd_path.clone()))
            .block_task()
            .await
            .expect("a creates a session")
            .session_id
            .to_string();

        let announced = tokio::time::timeout(std::time::Duration::from_secs(10), created.recv())
            .await
            .expect("b is told about the new thread")
            .expect("a row");
        assert_eq!(announced.session_id, session_id);
        assert_eq!(announced.cwd, cwd_path);
        assert_eq!(
            announced.harness.as_deref(),
            Some(crate::harness::BUILTIN_ID)
        );

        // Status broadcasts for other threads can arrive before this thread's title.
        server.clients.broadcast(SessionNotification::new(
            "unrelated-thread",
            SessionUpdate::SessionInfoUpdate(SessionInfoUpdate::new().title("Other thread")),
        ));

        // A name the thread takes is in the list too: B does not have the session, and still hears.
        server
            .clients
            .session(&session_id)
            .update(SessionUpdate::SessionInfoUpdate(
                SessionInfoUpdate::new().title("Named elsewhere"),
            ))
            .expect("a title");
        let titled = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while let Some(notification) = titles.recv().await {
                if notification.session_id.to_string() == session_id
                    && matches!(
                        &notification.update,
                        SessionUpdate::SessionInfoUpdate(info) if info.title.is_value()
                    )
                {
                    return notification;
                }
            }
            panic!("title notification channel closed");
        })
        .await
        .expect("b is told the title");
        assert_eq!(titled.session_id.to_string(), session_id);
        assert!(matches!(
            &titled.update,
            SessionUpdate::SessionInfoUpdate(info)
                if info.title.value().map(String::as_str) == Some("Named elsewhere")
        ));

        // Trashing the thread takes the row out of B's list too.
        a.send_request(ThreadDeleteRequest {
            session_id: session_id.clone(),
        })
        .block_task()
        .await
        .expect("the delete request");
        let gone = tokio::time::timeout(std::time::Duration::from_secs(10), deleted.recv())
            .await
            .expect("b is told the thread is gone")
            .expect("a deletion");
        assert_eq!(gone.session_id, session_id);

        for (close, client, serving) in [
            (close_a, a_client, a_serving),
            (close_b, b_client, b_serving),
        ] {
            let _ = close.send(());
            client.await.expect("client").expect("client ends cleanly");
            serving
                .await
                .expect("connection")
                .expect("connection ends cleanly");
        }
        server.shutdown().await;
    }

    /// A thread a paired computer asks this one to start (`create_session`, a remote
    /// `start_session`) is made by the machine's server, not by any client: the app on this
    /// machine, which never asked, still hears about it and lists it at once (issue 3530).
    #[test]
    fn a_thread_started_for_a_paired_computer_reaches_this_machines_app() {
        on_a_large_stack(remote_thread_reaches_the_local_app());
    }

    async fn remote_thread_reaches_the_local_app() {
        let server = AcpServer::new(false).expect("server");
        // The share the new thread gets connects to a gateway this test holds.
        let gateways = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let kept = std::sync::Arc::clone(&gateways);
        struct Held(std::sync::Arc<std::sync::Mutex<Vec<crate::remote::owner::GatewayLink>>>);
        impl crate::remote::owner::OwnerConnector for Held {
            fn connect(
                &mut self,
                _registration: &crate::remote::SessionRegistration,
            ) -> Result<crate::remote::owner::OwnerLink, crate::remote::owner::OwnerLinkError>
            {
                let (owner, gateway) = crate::remote::owner::owner_link_pair(16, 16);
                if let Ok(mut held) = self.0.lock() {
                    held.push(gateway);
                }
                Ok(owner)
            }
        }
        server
            .remote
            .use_connector(move || Box::new(Held(std::sync::Arc::clone(&kept))));

        // This machine's own app lists threads and keeps every creation it is told about.
        let (engine, transport) = agent_client_protocol::Channel::duplex();
        let (created_tx, mut created) = tokio::sync::mpsc::unbounded_channel();
        let (ready_tx, ready) = tokio::sync::oneshot::channel();
        let (close, closed) = tokio::sync::oneshot::channel();
        let serving = serve_on_thread(&server, engine);
        let app = tokio::spawn(async move {
            agent_client_protocol::Client
                .builder()
                .on_receive_notification(
                    async move |notification: ThreadCreatedNotification, _connection| {
                        let _ = created_tx.send(notification);
                        Ok(())
                    },
                    agent_client_protocol::on_receive_notification!(),
                )
                .connect_with(transport, async move |connection| {
                    let _ = ready_tx.send(connection);
                    let _ = closed.await;
                    Ok(())
                })
                .await
        });
        let connection = ready.await.expect("the app connects");
        connection
            .send_request(initialize_with(
                serde_json::json!({ "rustcode": { "threads": 1 } }),
            ))
            .block_task()
            .await
            .expect("the app initializes");

        // The gateway routes the paired computer's request to the engine's thread host.
        let folder = tempfile::tempdir().expect("folder");
        let folder_path = std::fs::canonicalize(folder.path()).expect("folder");
        let info = hosted::start(
            &server,
            remote::ThreadStart {
                path: Some(folder_path.display().to_string()),
                harness: Some(crate::harness::BUILTIN_ID.to_owned()),
                model: None,
            },
        )
        .await
        .expect("the thread starts");

        let announced = tokio::time::timeout(std::time::Duration::from_secs(10), created.recv())
            .await
            .expect("the app is told about the thread")
            .expect("a row");
        assert_eq!(announced.session_id, info.session_id);
        assert_eq!(announced.cwd, folder_path);

        let _ = close.send(());
        app.await.expect("client").expect("client ends cleanly");
        serving
            .await
            .expect("connection")
            .expect("connection ends cleanly");
        server.shutdown().await;
    }

    /// A thread that starts in a new worktree is announced under the worktree, the folder it
    /// runs in, not under the checkout the caller named.
    #[test]
    fn a_worktree_thread_is_announced_under_its_worktree() {
        on_a_large_stack(worktree_thread_is_announced_under_its_worktree());
    }

    async fn worktree_thread_is_announced_under_its_worktree() {
        let server = AcpServer::new(false).expect("server");
        let (_repo_dir, repo) = crate::worktrees::tests::repo();

        let (engine, transport) = agent_client_protocol::Channel::duplex();
        let (created_tx, mut created) = tokio::sync::mpsc::unbounded_channel();
        let (ready_tx, ready) = tokio::sync::oneshot::channel();
        let (close, closed) = tokio::sync::oneshot::channel();
        let serving = serve_on_thread(&server, engine);
        let app = tokio::spawn(async move {
            agent_client_protocol::Client
                .builder()
                .on_receive_notification(
                    async move |notification: ThreadCreatedNotification, _connection| {
                        let _ = created_tx.send(notification);
                        Ok(())
                    },
                    agent_client_protocol::on_receive_notification!(),
                )
                .connect_with(transport, async move |connection| {
                    let _ = ready_tx.send(connection);
                    let _ = closed.await;
                    Ok(())
                })
                .await
        });
        let connection = ready.await.expect("the app connects");
        connection
            .send_request(initialize_with(
                serde_json::json!({ "rustcode": { "threads": 1 } }),
            ))
            .block_task()
            .await
            .expect("the app initializes");

        let mut request = NewSessionRequest::new(repo.clone());
        let mut meta = serde_json::Map::new();
        meta.insert(
            "rustcode".to_owned(),
            serde_json::json!({ "worktree": { "repo": repo.display().to_string() } }),
        );
        request.meta = Some(meta);
        let response = connection
            .send_request(request)
            .block_task()
            .await
            .expect("the thread starts in a worktree");
        let worktree = response
            .meta
            .as_ref()
            .and_then(|meta| meta["rustcode"]["worktree"]["path"].as_str())
            .expect("the answer names the worktree")
            .to_owned();
        assert_ne!(std::path::Path::new(&worktree), repo.as_path());

        let announced = tokio::time::timeout(std::time::Duration::from_secs(10), created.recv())
            .await
            .expect("the app is told about the thread")
            .expect("a row");
        assert_eq!(announced.session_id, response.session_id.to_string());
        assert_eq!(announced.cwd, std::path::PathBuf::from(&worktree));

        let _ = close.send(());
        app.await.expect("client").expect("client ends cleanly");
        serving
            .await
            .expect("connection")
            .expect("connection ends cleanly");
        server.shutdown().await;
    }

    // ---- the replay cursor (`acp/cursor.rs`) ----

    /// One connected test client that keeps every whole `session/update`
    /// notification, `_meta` included.
    struct Noting {
        connection: agent_client_protocol::ConnectionTo<Agent>,
        notes: tokio::sync::mpsc::UnboundedReceiver<SessionNotification>,
        relay: tokio::task::JoinHandle<Result<(), agent_client_protocol::Error>>,
        close: tokio::sync::oneshot::Sender<()>,
        serving: tokio::sync::oneshot::Receiver<Result<(), agent_client_protocol::Error>>,
        client: tokio::task::JoinHandle<Result<(), agent_client_protocol::Error>>,
    }

    impl Noting {
        async fn connect(server: &AcpServer) -> Self {
            let (engine, wire) = agent_client_protocol::Channel::duplex();
            let (relay_transport, transport) = agent_client_protocol::Channel::duplex();
            let (notes_tx, notes) = tokio::sync::mpsc::unbounded_channel();
            // Record in wire order before forwarding the load response. Notification
            // handlers run independently of responses, so sleeping after a response
            // cannot establish that replay has reached the recorder.
            let relay = tokio::spawn(agent_client_protocol::Channel::bridge_with_inspection(
                wire,
                relay_transport,
                move |message| {
                    if let agent_client_protocol::RawJsonRpcMessage::Notification(note) = message
                        && note.method.as_ref() == "session/update"
                    {
                        let notification: SessionNotification = serde_json::from_value(
                            note.params
                                .clone()
                                .expect("session/update params")
                                .into_value(),
                        )
                        .expect("session/update");
                        let status = notification.meta.is_none()
                            && matches!(
                                &notification.update,
                                SessionUpdate::SessionInfoUpdate(info)
                                    if info.meta.as_ref().is_some_and(|meta| {
                                        meta["rustcode"].get("status").is_some()
                                    })
                            );
                        if !status {
                            let _ = notes_tx.send(notification);
                        }
                    }
                    Ok(())
                },
                |_| Ok(()),
            ));
            let (ready_tx, ready) = tokio::sync::oneshot::channel();
            let (close, closed) = tokio::sync::oneshot::channel();
            let serving = serve_on_thread(server, engine);
            let client = tokio::spawn(async move {
                agent_client_protocol::Client
                    .builder()
                    .on_receive_notification(
                        async move |_notification: SessionNotification, _connection| Ok(()),
                        agent_client_protocol::on_receive_notification!(),
                    )
                    .on_receive_request(
                        async move |_request: agent_client_protocol::UntypedMessage,
                                    responder,
                                    _connection| {
                            responder.respond(serde_json::json!({}))
                        },
                        agent_client_protocol::on_receive_request!(),
                    )
                    .connect_with(transport, async move |connection| {
                        let _ = ready_tx.send(connection);
                        let _ = closed.await;
                        Ok(())
                    })
                    .await
            });
            let connection = ready.await.expect("connects");
            connection
                .send_request(initialize_with(serde_json::json!({})))
                .block_task()
                .await
                .expect("initializes");
            Self {
                connection,
                notes,
                relay,
                close,
                serving,
                client,
            }
        }

        /// `session/load`, with the cursor to resume from when given. What
        /// the engine sent before answering, then the answer.
        async fn load(
            &mut self,
            session_id: &str,
            cwd: &std::path::Path,
            resume: Option<u64>,
        ) -> (Vec<SessionNotification>, LoadSessionResponse) {
            let response = self
                .connection
                .send_request(load_request(session_id, cwd, resume))
                .block_task()
                .await
                .expect("session/load");
            // The relay recorded every preceding notification before this answer.
            (self.drain(), response)
        }

        fn drain(&mut self) -> Vec<SessionNotification> {
            let mut got = Vec::new();
            while let Ok(note) = self.notes.try_recv() {
                got.push(note);
            }
            got
        }

        /// Everything that arrives until a chunk says `text`.
        async fn until_text(&mut self, text: &str) -> Vec<SessionNotification> {
            let mut got = Vec::new();
            let deadline = std::time::Duration::from_secs(20);
            tokio::time::timeout(deadline, async {
                while let Some(note) = self.notes.recv().await {
                    let found = said(&note.update).is_some_and(|(_, said)| said.contains(text));
                    got.push(note);
                    if found {
                        return;
                    }
                }
                panic!("the client closed before {text:?}");
            })
            .await
            .unwrap_or_else(|_| panic!("no {text:?} within {deadline:?}"));
            got
        }

        async fn leave(self) {
            let _ = self.close.send(());
            self.client
                .await
                .expect("client task")
                .expect("client ends cleanly");
            if let Err(error) = self.serving.await.expect("connection") {
                // These clients deliberately drop their transport, including
                // mid-turn. An in-flight notification can lose that race.
                assert_eq!(error.code, agent_client_protocol::ErrorCode::InternalError);
                assert_eq!(
                    error.data,
                    Some(serde_json::json!("send failed because receiver is gone")),
                    "unexpected disconnect error: {error:?}"
                );
            }
            self.relay.abort();
        }
    }

    /// Run `work` on a thread named like the ones that serve connections:
    /// the test config directory is per thread name, so what it writes is
    /// what a `session/load` served on such a thread reads.
    fn on_the_connection_thread<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
        std::thread::Builder::new()
            .name("test-connection".to_owned())
            .spawn(work)
            .expect("thread")
            .join()
            .expect("work")
    }

    fn load_request(
        session_id: &str,
        cwd: &std::path::Path,
        resume: Option<u64>,
    ) -> LoadSessionRequest {
        let mut request = LoadSessionRequest::new(session_id.to_owned(), cwd.to_path_buf());
        if let Some(cursor) = resume {
            request.meta = Some(
                serde_json::json!({ "rustcode": { "resume": { "cursor": cursor } } })
                    .as_object()
                    .cloned()
                    .expect("object"),
            );
        }
        request
    }

    fn rustcode_of(meta: Option<&serde_json::Map<String, serde_json::Value>>) -> serde_json::Value {
        meta.and_then(|meta| meta.get("rustcode"))
            .cloned()
            .unwrap_or_default()
    }

    /// The cursor of a logged update; none on what a load sends to rebuild
    /// a view.
    fn cursor_opt(note: &SessionNotification) -> Option<u64> {
        rustcode_of(note.meta.as_ref())["cursor"].as_u64()
    }

    fn cursor_of(note: &SessionNotification) -> u64 {
        cursor_opt(note).unwrap_or_else(|| panic!("an update without a cursor: {note:?}"))
    }

    fn is_reset(note: &SessionNotification) -> bool {
        rustcode_of(note.meta.as_ref())["reset"] == true
    }

    fn answered_cursor(response: &LoadSessionResponse) -> Option<u64> {
        rustcode_of(response.meta.as_ref())["cursor"].as_u64()
    }

    fn answered_reset(response: &LoadSessionResponse) -> bool {
        rustcode_of(response.meta.as_ref())["reset"] == true
    }

    /// The chunks of text among `notes`, each with its cursor.
    fn chunks(notes: &[SessionNotification]) -> Vec<(Option<u64>, &'static str, String)> {
        notes
            .iter()
            .filter_map(|note| said(&note.update).map(|(who, text)| (cursor_opt(note), who, text)))
            .collect()
    }

    /// A built-in thread in `server` that ran one turn, "do the thing" and
    /// its reply, with nobody connected afterwards.
    async fn finished_thread(
        server: &AcpServer,
        endpoint_and_release: (String, tokio::sync::oneshot::Sender<()>),
        cwd: &std::path::Path,
    ) -> String {
        let (endpoint, release) = endpoint_and_release;
        let mut author = Noting::connect(server).await;
        let session_id = author
            .connection
            .send_request(NewSessionRequest::new(cwd.to_path_buf()))
            .block_task()
            .await
            .expect("creates a session")
            .session_id
            .to_string();
        use_provider(server, &session_id, &endpoint).await;
        let prompt = author.connection.send_request(PromptRequest::new(
            session_id.clone(),
            vec![ContentBlock::Text(TextContent::new("do the thing"))],
        ));
        author.until_text("the first half").await;
        let _ = release.send(());
        author.until_text("then the rest").await;
        prompt.block_task().await.expect("the prompt");
        wait_until("the turn ends", || {
            !server.clients.turn_running(&session_id)
        })
        .await;
        author.leave().await;
        session_id
    }

    #[test]
    fn a_load_without_resume_is_unchanged_and_a_load_with_an_old_cursor_resets() {
        on_a_large_stack(without_mcp(old_cursor_resets()));
    }

    async fn old_cursor_resets() {
        let server = AcpServer::new(false).expect("server");
        let cwd = tempfile::tempdir().expect("cwd");
        let cwd_path = std::fs::canonicalize(cwd.path()).expect("cwd");
        let provider = held_provider("the first half, ", "then the rest").await;
        let session_id = finished_thread(&server, provider, &cwd_path).await;
        let transcript = vec![
            ("user", "an earlier question".to_owned()),
            ("agent", "an earlier answer".to_owned()),
            ("user", "do the thing".to_owned()),
            ("agent", "the first half, then the rest".to_owned()),
        ];

        // No resume: what it always was, plus the cursor, and no reset.
        let mut plain = Noting::connect(&server).await;
        let (notes, response) = plain.load(&session_id, &cwd_path, None).await;
        assert!(notes.iter().all(|note| !is_reset(note)));
        assert!(!answered_reset(&response));
        let head = answered_cursor(&response).expect("the answer carries the cursor");
        // What rebuilds the view is not in the log: no cursor to resume from
        // until the answer.
        assert!(notes.iter().all(|note| cursor_opt(note).is_none()));
        assert!(head > 0);
        let said_plain: Vec<_> = chunks(&notes)
            .into_iter()
            .map(|(_, who, text)| (who, text))
            .collect();
        assert_eq!(said_plain, transcript);

        // A cursor from before this engine's log began (another run, or the
        // log aged out): the transcript again, after a reset marker.
        for stale in [1, head + 1_000_000] {
            let mut resumed = Noting::connect(&server).await;
            let (notes, response) = resumed.load(&session_id, &cwd_path, Some(stale)).await;
            assert!(is_reset(&notes[0]), "the reset comes first: {:?}", notes[0]);
            assert!(notes.iter().all(|note| cursor_opt(note).is_none()));
            assert!(matches!(
                notes[0].update,
                SessionUpdate::SessionInfoUpdate(_)
            ));
            assert_eq!(notes.iter().filter(|note| is_reset(note)).count(), 1);
            assert!(answered_reset(&response));
            assert!(answered_cursor(&response).is_some_and(|cursor| cursor >= head));
            let said_reset: Vec<_> = chunks(&notes)
                .into_iter()
                .map(|(_, who, text)| (who, text))
                .collect();
            assert_eq!(said_reset, transcript, "stale cursor {stale}");
            resumed.leave().await;
        }
        plain.leave().await;
        server.shutdown().await;
    }

    /// A second client does something to a closed session while a first one
    /// loads it, after the session is open and before the loader is attached.
    /// The loader is sent, after the transcript, everything the session
    /// logged since before the transcript was read, in order and once, and
    /// the cursor of its answer is the last of them: it never names an
    /// update it was not sent.
    fn assert_the_loader_gets_what_happened_while_it_loaded(
        notes: &[SessionNotification],
        response: &LoadSessionResponse,
        base: u64,
        transcript: &[(&str, String)],
    ) -> Vec<SessionNotification> {
        assert!(is_reset(&notes[0]), "the reset comes first: {:?}", notes[0]);
        let first_logged = notes
            .iter()
            .position(|note| cursor_opt(note).is_some())
            .expect("the updates of the window reach the loader");
        assert!(
            notes[..first_logged]
                .iter()
                .all(|note| cursor_opt(note).is_none()),
            "nothing with a cursor lands inside the transcript"
        );
        let said_before: Vec<_> = chunks(&notes[..first_logged])
            .into_iter()
            .map(|(_, who, text)| (who, text))
            .collect();
        assert_eq!(said_before, transcript);
        let logged = &notes[first_logged..];
        assert!(
            logged.iter().all(|note| cursor_opt(note).is_some()),
            "after the transcript only logged updates follow"
        );
        let cursors: Vec<u64> = logged.iter().map(cursor_of).collect();
        assert_eq!(
            cursors,
            (base + 1..=base + cursors.len() as u64).collect::<Vec<_>>(),
            "every update since before the transcript was read, once, in order"
        );
        assert_eq!(
            answered_cursor(response),
            cursors.last().copied(),
            "the answer names the last update the loader was sent"
        );
        logged.to_vec()
    }

    #[test]
    fn a_closed_thread_load_gets_the_turn_and_title_of_a_second_client_meanwhile() {
        on_a_large_stack(without_mcp(closed_load_with_a_second_client()));
    }

    async fn closed_load_with_a_second_client() {
        let server = AcpServer::new(false).expect("server");
        let cwd = tempfile::tempdir().expect("cwd");
        let cwd_path = std::fs::canonicalize(cwd.path()).expect("cwd");
        // A thread on disk that is not open in this engine.
        let session_id = on_the_connection_thread(|| {
            let session_id = crate::config::create_detached_session();
            crate::config::save_session_history(
                &session_id,
                &[
                    crate::app::ChatMessage::new("user", "an earlier question"),
                    crate::app::ChatMessage::new("assistant", "an earlier answer"),
                ][..],
            );
            // The history is written in the background.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while crate::config::load_session_by_id(&session_id).is_none() {
                assert!(
                    std::time::Instant::now() < deadline,
                    "the history is never written"
                );
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            session_id
        });
        let transcript = vec![
            ("user", "an earlier question".to_owned()),
            ("agent", "an earlier answer".to_owned()),
        ];

        let (reached, go) = load_pause::arm(&session_id);
        let mut loader = Noting::connect(&server).await;
        let load = loader
            .connection
            .send_request(load_request(&session_id, &cwd_path, Some(1)))
            .block_task();
        let mut load = tokio::spawn(load);
        tokio::select! {
            () = reached.notified() => {}
            early = &mut load => panic!("the load ended before its pause: {early:?}"),
            () = tokio::time::sleep(std::time::Duration::from_secs(20)) => {
                panic!("the load never reached its pause")
            }
        }
        let base = server.clients.head(&session_id);

        // Another client loads the open thread, runs a prompt that starts and
        // ends, and the thread is titled: all before the loader is attached.
        let mut second = Noting::connect(&server).await;
        let (_, _) = second.load(&session_id, &cwd_path, None).await;
        let (endpoint, release) = held_provider("reply one, ", "reply two").await;
        use_provider(&server, &session_id, &endpoint).await;
        let prompt = second.connection.send_request(PromptRequest::new(
            session_id.clone(),
            vec![ContentBlock::Text(TextContent::new("second question"))],
        ));
        second.until_text("reply one").await;
        let _ = release.send(());
        second.until_text("reply two").await;
        prompt.block_task().await.expect("the prompt");
        wait_until("the turn ends", || {
            !server.clients.turn_running(&session_id)
        })
        .await;
        server
            .clients
            .session(&session_id)
            .update(SessionUpdate::SessionInfoUpdate(
                SessionInfoUpdate::new().title("A new title"),
            ))
            .expect("title");

        go.notify_one();
        let response = load.await.expect("task").expect("session/load");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let notes = loader.drain();
        let logged = assert_the_loader_gets_what_happened_while_it_loaded(
            &notes,
            &response,
            base,
            &transcript,
        );
        let said: Vec<_> = chunks(&logged)
            .into_iter()
            .map(|(_, who, text)| (who, text))
            .collect();
        assert!(
            said.contains(&("user", "second question".to_owned())),
            "the prompt: {said:?}"
        );
        assert_eq!(
            said.iter()
                .filter(|(who, _)| *who == "agent")
                .map(|(_, text)| text.as_str())
                .collect::<String>(),
            "reply one, reply two",
            "the whole reply, once"
        );
        assert!(
            logged.iter().any(|note| matches!(
                &note.update,
                SessionUpdate::SessionInfoUpdate(info)
                    if serde_json::to_value(info).is_ok_and(|info| info["title"] == "A new title")
            )),
            "the title"
        );

        // It carries on from there: the next update reaches both, in order.
        let last = answered_cursor(&response).expect("cursor");
        server
            .clients
            .session(&session_id)
            .update(SessionUpdate::SessionInfoUpdate(
                SessionInfoUpdate::new().title("Later"),
            ))
            .expect("title");
        let mut after = Vec::new();
        wait_until("the loader is sent the next update", || {
            after.extend(loader.drain());
            !after.is_empty()
        })
        .await;
        assert_eq!(after.iter().map(cursor_of).collect::<Vec<_>>(), [last + 1]);
        second.leave().await;
        loader.leave().await;
        server.shutdown().await;
    }

    #[test]
    fn restart_restores_external_harness_account_native_session_and_parent() {
        on_a_large_stack(without_mcp(async {
            let server = AcpServer::new(false).unwrap();
            let cwd = tempfile::tempdir().unwrap();
            let cwd_path = std::fs::canonicalize(cwd.path()).unwrap();
            let id = on_the_connection_thread({
                let cwd_path = cwd_path.clone();
                move || {
                    let id = crate::config::create_detached_session();
                    let mut record =
                        crate::harness::store::HarnessRecord::new("claude-code", cwd_path);
                    record.account = Some("personal-1".into());
                    record.native_session_id = Some("saved-native-session".into());
                    crate::harness::store::save(&id, &record);
                    crate::config::save_session_parent(
                        &id,
                        &rustcode_session::SessionParent {
                            id: Some("parent-thread".into()),
                            title: None,
                            relationship: Some(rustcode_session::SessionRelationship::Delegate),
                        },
                    );
                    id
                }
            });
            let mut client = Noting::connect(&server).await;
            client.load(&id, &cwd_path, None).await;
            let session = harnesses::get(&server.external, &id).await.unwrap();
            assert_eq!(session.harness.id, "claude-code");
            assert_eq!(session.record().account.as_deref(), Some("personal-1"));
            assert_eq!(
                session.record().native_session_id.as_deref(),
                Some("saved-native-session")
            );
            let inspected = id.clone();
            on_the_connection_thread(move || {
                let parent = crate::config::load_session_parent(&inspected).unwrap();
                assert_eq!(parent.id.as_deref(), Some("parent-thread"));
                assert_eq!(
                    parent.relationship,
                    Some(rustcode_session::SessionRelationship::Delegate)
                );
                session_repair::repair_session(&inspected).unwrap();
                assert_eq!(crate::config::load_session_parent(&inspected), Some(parent));
            });
            client.leave().await;
            server.shutdown().await;
        }));
    }

    #[test]
    fn a_closed_external_thread_load_gets_what_a_second_client_did_meanwhile() {
        on_a_large_stack(without_mcp(closed_external_load_with_a_second_client()));
    }

    async fn closed_external_load_with_a_second_client() {
        let server = AcpServer::new(false).expect("server");
        let cwd = tempfile::tempdir().expect("cwd");
        let cwd_path = std::fs::canonicalize(cwd.path()).expect("cwd");
        let session_id = on_the_connection_thread({
            let cwd_path = cwd_path.clone();
            move || {
                let session_id = crate::config::create_detached_session();
                let mut record = crate::harness::store::HarnessRecord::new("claude-code", cwd_path);
                record.title = Some("Stored title".to_owned());
                crate::harness::store::save(&session_id, &record);
                session_id
            }
        });

        let (reached, go) = load_pause::arm(&session_id);
        let mut loader = Noting::connect(&server).await;
        let mut load = tokio::spawn(
            loader
                .connection
                .send_request(load_request(&session_id, &cwd_path, Some(1)))
                .block_task(),
        );
        tokio::select! {
            () = reached.notified() => {}
            early = &mut load => panic!("the load ended before its pause: {early:?}"),
            () = tokio::time::sleep(std::time::Duration::from_secs(20)) => {
                panic!("the load never reached its pause")
            }
        }
        let base = server.clients.head(&session_id);

        // Another client loads the open thread and a turn starts and ends on
        // it, and the thread is titled, before the loader is attached.
        let mut second = Noting::connect(&server).await;
        second.load(&session_id, &cwd_path, None).await;
        let clients = server.clients.session(&session_id);
        let turn = clients.begin_turn(0, None);
        for update in [
            SessionUpdate::UserMessageChunk(agent_client_protocol::schema::v1::ContentChunk::new(
                ContentBlock::Text(TextContent::new("second question")),
            )),
            SessionUpdate::AgentMessageChunk(agent_client_protocol::schema::v1::ContentChunk::new(
                ContentBlock::Text(TextContent::new("an answer")),
            )),
        ] {
            clients.update(update).expect("update");
        }
        drop(turn);
        clients
            .update(SessionUpdate::SessionInfoUpdate(
                SessionInfoUpdate::new().title("A new title"),
            ))
            .expect("title");

        go.notify_one();
        let response = load.await.expect("task").expect("session/load");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let notes = loader.drain();
        // The stored title is part of the transcript the loader is sent.
        assert!(is_reset(&notes[0]));
        let first_logged = notes
            .iter()
            .position(|note| cursor_opt(note).is_some())
            .expect("the updates of the window reach the loader");
        assert!(
            notes[..first_logged]
                .iter()
                .all(|note| cursor_opt(note).is_none())
        );
        let cursors: Vec<u64> = notes[first_logged..].iter().map(cursor_of).collect();
        assert_eq!(cursors, [base + 1, base + 2, base + 3]);
        assert_eq!(answered_cursor(&response), Some(base + 3));
        assert_eq!(
            chunks(&notes[first_logged..])
                .into_iter()
                .map(|(_, who, text)| (who, text))
                .collect::<Vec<_>>(),
            [
                ("user", "second question".to_owned()),
                ("agent", "an answer".to_owned())
            ]
        );
        second.leave().await;
        loader.leave().await;
        server.shutdown().await;
    }

    #[test]
    fn a_resume_during_a_turn_gets_the_rest_once_and_each_client_keeps_its_own_place() {
        on_a_large_stack(without_mcp(resume_during_a_turn()));
    }

    async fn resume_during_a_turn() {
        let server = AcpServer::new(false).expect("server");
        let cwd = tempfile::tempdir().expect("cwd");
        let cwd_path = std::fs::canonicalize(cwd.path()).expect("cwd");
        let (endpoint, release) = held_provider("the first half, ", "then the rest").await;

        // A starts the turn, sees the first half, and drops off.
        let mut author = Noting::connect(&server).await;
        let session_id = author
            .connection
            .send_request(NewSessionRequest::new(cwd_path.clone()))
            .block_task()
            .await
            .expect("creates a session")
            .session_id
            .to_string();
        use_provider(&server, &session_id, &endpoint).await;
        let _unanswered = author.connection.send_request(PromptRequest::new(
            session_id.clone(),
            vec![ContentBlock::Text(TextContent::new("do the thing"))],
        ));
        let seen = author.until_text("the first half").await;
        let author_cursor = seen
            .iter()
            .filter_map(cursor_opt)
            .max()
            .expect("A saw updates");
        author.leave().await;

        // O looks on for the whole turn: its live updates are the stream.
        let mut observer = Noting::connect(&server).await;
        let (_, observed_load) = observer.load(&session_id, &cwd_path, None).await;
        let observer_from = answered_cursor(&observed_load).expect("cursor");

        // R1 and R2 come back mid-turn with A's cursor; R3 with the one O
        // loaded at. They must not be shown the first half again.
        let mut first = Noting::connect(&server).await;
        let (notes, response) = first
            .load(&session_id, &cwd_path, Some(author_cursor))
            .await;
        assert!(
            notes.iter().all(|note| !is_reset(note)),
            "a recent cursor resumes"
        );
        assert!(!answered_reset(&response));
        assert!(
            chunks(&notes).is_empty(),
            "nothing came after A's cursor yet: {notes:?}"
        );
        let mut second = Noting::connect(&server).await;
        let (notes, _) = second
            .load(&session_id, &cwd_path, Some(author_cursor))
            .await;
        assert!(chunks(&notes).is_empty());
        let mut third = Noting::connect(&server).await;
        let (notes, _) = third
            .load(&session_id, &cwd_path, Some(observer_from))
            .await;
        assert!(notes.iter().all(|note| !is_reset(note)));

        let _ = release.send(());
        let first_got = first.until_text("then the rest").await;
        let second_got = second.until_text("then the rest").await;
        let observer_got = observer.until_text("then the rest").await;
        wait_until("the turn ends", || {
            !server.clients.turn_running(&session_id)
        })
        .await;

        let rest = |notes: &[SessionNotification]| {
            chunks(notes)
                .into_iter()
                .filter(|(_, who, _)| *who == "agent")
                .collect::<Vec<_>>()
        };
        assert_eq!(
            rest(&first_got)
                .iter()
                .map(|(_, _, text)| text.as_str())
                .collect::<String>(),
            "then the rest",
            "the rest once, not the first half again"
        );
        // Resumed and watching all along see the same updates, same cursors.
        assert_eq!(rest(&first_got), rest(&second_got));
        assert_eq!(rest(&first_got), rest(&observer_got));
        assert!(first_got.iter().all(|note| cursor_of(note) > author_cursor));
        let cursors: Vec<_> = first_got.iter().map(cursor_of).collect();
        assert!(
            cursors.windows(2).all(|pair| pair[0] < pair[1]),
            "{cursors:?}"
        );

        // Later, with the turn over, each client resumes from its own place:
        // A's gets the whole reply (the observer's updates after A's cursor),
        // one at the end gets nothing.
        let mut late_a = Noting::connect(&server).await;
        let (from_a, late_a_response) = late_a
            .load(&session_id, &cwd_path, Some(author_cursor))
            .await;
        assert!(from_a.iter().all(|note| !is_reset(note)));
        let end = answered_cursor(&late_a_response).expect("completed replay cursor");
        let mut all_after_author = observer_got;
        tokio::time::timeout(std::time::Duration::from_secs(20), async {
            while !all_after_author
                .iter()
                .any(|note| cursor_opt(note) == Some(end))
            {
                all_after_author.push(observer.notes.recv().await.expect("observer update"));
            }
        })
        .await
        .expect("observer reaches the replay cursor");
        let wanted: Vec<_> = chunks(&all_after_author);
        assert!(!wanted.is_empty());
        assert_eq!(chunks(&from_a), wanted, "A's place");
        let end = from_a.iter().map(cursor_of).max().expect("updates");
        let mut late_b = Noting::connect(&server).await;
        let (from_end, response) = late_b.load(&session_id, &cwd_path, Some(end)).await;
        assert!(
            from_end.is_empty(),
            "nothing after the last cursor: {from_end:?}"
        );
        assert!(answered_cursor(&response).is_some_and(|cursor| cursor >= end));

        for client in [first, second, third, observer, late_a, late_b] {
            client.leave().await;
        }
        server.shutdown().await;
    }
}
