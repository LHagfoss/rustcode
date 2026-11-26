//! Opening a built-in thread: the agent builds the state of a new or loaded
//! session and hands the server the trait object that runs it.

use super::*;

/// A built-in thread the agent opened, with what its first client gets.
pub(crate) struct OpenedThread {
    pub(crate) session_id: String,
    pub(crate) agent: Arc<dyn builtin::BuiltinAgent>,
    pub(crate) clients: clients::SessionClients,
    pub(crate) snapshot: builtin::AttachSnapshot,
}

/// Open a new thread in `cwd` with the model and effort `start` names.
/// `attach` gives the clients of the session id the agent picked.
pub(super) fn create_thread(
    cwd: &std::path::Path,
    start: &crate::harness::HarnessRequest,
    info: clients::ClientInfo,
    attach: impl FnOnce(&str) -> clients::SessionClients,
) -> OpenedThread {
    let mut state = new_session_state(cwd);
    let session_id = state.active_session_id.clone();
    let clients = attach(&session_id);
    apply_client_info(&mut state, info, &clients);
    state.raw_cli_mode = false;
    state.task_working_directory = Some(cwd.to_path_buf());
    // ACP's requested cwd is both the task's initial location
    // and its hard filesystem boundary, independent of the
    // directory from which RustCode itself was launched.
    state.workspace_root = Some(cwd.to_path_buf());
    apply_requested_model_and_effort(&mut state, start);
    opened(state, clients)
}

/// Open the saved thread `session_id` in `cwd` with `history`.
pub(super) fn load_thread(
    session_id: &str,
    cwd: &std::path::Path,
    history: Vec<crate::app::ChatMessage>,
    info: clients::ClientInfo,
    clients: clients::SessionClients,
) -> OpenedThread {
    let mut state = build_loaded_app_state(session_id, cwd, history);
    apply_client_info(&mut state, info, &clients);
    opened(state, clients)
}

fn opened(state: crate::app::AppState, clients: clients::SessionClients) -> OpenedThread {
    let snapshot = builtin::AttachSnapshot {
        compaction: state.last_compaction.clone(),
        config_options: build_session_config_options(&state),
        usage: usage::usage_update(&state),
    };
    let session_id = state.active_session_id.clone();
    crate::workflow::checkpoint::register(&session_id, question_asker(clients.clone()));
    OpenedThread {
        session_id,
        agent: super::prompt::builtin_agent(Arc::new(Mutex::new(state))),
        clients,
        snapshot,
    }
}

/// How a workflow's `checkpoint()` asks this built-in thread's user: a
/// question card on its clients, the thread waiting on the user meanwhile.
fn question_asker(clients: clients::SessionClients) -> crate::orchestration::ask_user::Asker {
    Arc::new(move |mut request| {
        let clients = clients.clone();
        Box::pin(async move {
            request.session_id = clients.session_id().to_owned().into();
            let _waiting = crate::thread_status::waiting(clients.session_id());
            clients.request_approval(request).await.ok()
        })
    })
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

/// A built-in `session/new` honours `_meta.rustcode.model` and `effort` the
/// same way external harnesses do (docs/harnesses.md). Model first, since
/// the effort choices depend on it; an unknown value is ignored and the
/// session keeps its default.
pub(super) fn apply_requested_model_and_effort(
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
