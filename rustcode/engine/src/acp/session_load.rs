//! `session/list` and `session/load`: listing threads, replaying their history, and attaching to an open one.

use super::*;

pub(super) fn parse_session_cursor(
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

pub(super) fn session_info(
    meta: &crate::sessions::SessionMeta,
    workspace: Option<rustcode_session::SessionWorkspace>,
) -> SessionInfo {
    let session_id = crate::sessions::session_id_from_path(&meta.path).unwrap_or_default();
    let (cwd, workspace_unknown) =
        listed_folder(workspace_cwd(workspace), home_folder().as_deref());
    let mut info = SessionInfo::new(session_id.clone(), cwd).title(meta.title.clone());
    let mut extra = serde_json::Map::new();
    extra.insert("messageCount".into(), serde_json::json!(meta.message_count));
    extra.insert(
        "workspaceUnknown".into(),
        serde_json::json!(workspace_unknown),
    );
    let metadata = crate::sessions::load_session_metadata(&session_id);
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
    if let Some(model) = crate::sessions::load_session_model(&session_id)
        .map(|model| model.profile)
        .or_else(|| {
            crate::sessions::load_session_settings(&session_id)
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
    // A scheduled job's run thread: the app leaves it out of the thread list
    // and shows it in the job's view instead.
    if let Some(job) = metadata
        .as_ref()
        .and_then(|metadata| metadata.scheduled_job.clone())
    {
        extra = crate::worktrees::merge_rustcode_meta(Some(extra), "scheduledJob", job.into());
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
pub(super) fn session_updated_at(
    meta: &crate::sessions::SessionMeta,
) -> Option<chrono::DateTime<chrono::Utc>> {
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

pub(super) fn session_file_is_contained(path: &std::path::Path) -> bool {
    crate::config::get_config_dir()
        .is_some_and(|config| crate::sessions::session_file_in_store(&config, path))
}

pub(super) fn acp_list_sessions(
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
        Some(filter) => crate::sessions::SessionScope::WorkspaceExact(filter.clone()),
        None => crate::sessions::SessionScope::All,
    };
    let (metas, next) =
        crate::sessions::list_sessions_page(cursor, ACP_SESSION_PAGE_SIZE, &scope, |meta| {
            // Path containment and id shape are not workspace concerns, so they
            // stay a separate narrowing step on the store's own listing.
            if !session_file_is_contained(&meta.path) {
                return false;
            }
            let id = crate::sessions::session_id_from_path(&meta.path);
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
pub(super) fn replay_history_updates_with_compaction(
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
            role if role == crate::harness::handoff::ROLE => {
                updates.extend(crate::harness::handoff::divider(message));
            }
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
    if let Some(workspace) = crate::sessions::load_session_workspace(session_id)
        && let Some(descriptor_id) = workspace.task_workspace_id
        && let Some(manager) = crate::sessions::workspace_manager()
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
pub(super) fn new_session_state(cwd: &std::path::Path) -> crate::app::AppState {
    let session_id = crate::sessions::create_detached_session();
    let state = crate::app::AppState::new_with_workspace_session(cwd, Some(&session_id));
    if !session_id.is_empty() {
        crate::sessions::record_session_settings(&session_id, &state.config);
        state.record_session_model();
    }
    state
}

pub(super) fn canonical_acp_cwd(
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
pub(super) fn session_new_cwd(
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

pub(super) fn session_workspace(
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
pub(super) fn apply_client_info(
    state: &mut crate::app::AppState,
    info: clients::ClientInfo,
    clients: &clients::SessionClients,
) {
    let secret_clients = clients.clone();
    state.secret_asker = Some(std::sync::Arc::new(move |request| {
        let clients = secret_clients.clone();
        Box::pin(async move { clients.request(None, request).await.ok() })
    }));
    state.visualization_level = info.visualization_level;
    state.smart_ui = info.smart_ui;
    state.preview_client = preview::preview_client_for(info.preview, clients);
    state.desktop_client = preview::desktop_client_for(info.desktop, clients);
    (state.orchestration_client, state.orchestration_version) =
        preview::orchestration_for(info.orchestration, clients);
    // No app hosting the tools: this engine serves the reads and names the app
    // for the rest (#3422). An app that turned them off keeps them off.
    state.local_orchestration = info.engine_hosts_orchestration(clients);
    state.smart_ui_client = preview::smart_ui_client_for(
        info.smart_ui == crate::app::SmartUi::On && info.smart_ui_preview,
        clients,
    );
}

/// The session `session/new` creates on the built-in harness: the engine's own
/// chat state, its task route and the client it publishes to. Also what a
/// thread a paired device asked for starts as (`super::hosted`), which is why
/// it is a function of its own.
pub(super) async fn create_builtin_session(
    server: &AcpServer,
    peer: &clients::Peer,
    request: NewSessionRequest,
) -> Result<NewSessionResponse, agent_client_protocol::Error> {
    let cwd = session_new_cwd(&request.cwd)?;
    let mut state = new_session_state(&cwd);
    let session_id = state.active_session_id.clone();
    // The name the thread was made with, from the app that made it (issue 3524).
    crate::sessions::save_requested_title(&session_id, request.meta.as_ref());
    // Who started this thread, from the `_meta.rustcode.parent` its
    // caller gave: kept in the thread's own metadata, not only in
    // the link book the app prunes (`thread_parent.rs`).
    if let Some((parent, relationship)) = crate::thread_parent::requested(request.meta.as_ref()) {
        crate::thread_parent::record(&session_id, &parent, relationship);
    }
    // A scheduled job's run: its thread lives in the job's view, not the list.
    if let Some(job) = crate::sessions::requested_scheduled_job(request.meta.as_ref()) {
        crate::sessions::save_session_scheduled_job(&session_id, &job);
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
    let _ = crate::sessions::save_session_workspace(
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

pub(super) async fn acp_load_session(
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
    let (meta, history) = crate::sessions::load_session_by_id(&session_id).ok_or_else(|| {
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
    if let Some(workspace) = crate::sessions::load_session_workspace(&session_id)
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
        let _ = crate::sessions::save_session_workspace(
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
pub(super) async fn attach_open_builtin(
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
        let title = crate::sessions::session_meta_by_id(&session_id)
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
pub(super) const ATTACH_ATTEMPTS: usize = 4;

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
