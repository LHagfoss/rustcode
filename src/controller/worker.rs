use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::app::AppState;

use super::*;

/// Starts the UI-neutral session worker on the supplied Tokio runtime.
pub struct InteractiveController;

impl InteractiveController {
    pub fn spawn(
        tokio_handle: &tokio::runtime::Handle,
        launch_dir: PathBuf,
    ) -> (ControllerHandle, mpsc::UnboundedReceiver<ControllerEvent>) {
        let (command_sender, command_receiver) = mpsc::unbounded_channel();
        let (update_sender, update_receiver) = mpsc::unbounded_channel();
        tokio_handle.spawn(controller_worker(
            command_receiver,
            update_sender,
            launch_dir,
        ));
        (ControllerHandle::new(command_sender), update_receiver)
    }
}

pub(super) struct ActiveSession {
    pub(super) generation: u64,
    pub(super) state: Arc<Mutex<AppState>>,
    pub(super) cancel_token: CancellationToken,
    pub(super) turn_task: Option<JoinHandle<()>>,
}

async fn controller_worker(
    mut commands: mpsc::UnboundedReceiver<Command>,
    updates: mpsc::UnboundedSender<ControllerEvent>,
    launch_dir: PathBuf,
) {
    let mut active: Option<ActiveSession> = None;
    let mut generation = 0;
    let client = reqwest::Client::new();
    let _ = updates.send(ControllerEvent {
        generation,
        update: ControllerUpdate::Snapshot(empty_snapshot(generation)),
    });

    while let Some(command) = commands.recv().await {
        match command {
            Command::Shutdown => {
                if let Some(session) = active.take() {
                    retire_session(session, &updates).await;
                }
                break;
            }
            Command::StartNew(workspace) => {
                let workspace = workspace.canonicalize().ok().filter(|path| path.is_dir());
                let Some(workspace) = workspace else {
                    send_error(
                        &updates,
                        generation,
                        ControllerError::InvalidWorkspace(
                            "workspace must be an existing directory".to_owned(),
                        ),
                    );
                    continue;
                };
                if let Some(previous) = active.take() {
                    retire_session(previous, &updates).await;
                }
                generation += 1;
                let mut state = AppState::new_with_workspace_session(&workspace, None);
                state.workspace_root = Some(workspace.clone());
                state.task_working_directory = Some(workspace);
                // Native frontends do not have the TUI confirmation overlay.
                state.auto_confirm = true;
                let session = ActiveSession {
                    generation,
                    state: Arc::new(Mutex::new(state)),
                    cancel_token: CancellationToken::new(),
                    turn_task: None,
                };
                send_snapshot(&updates, generation, &session.state).await;
                active = Some(session);
            }
            Command::Resume {
                session_id,
                workspace,
            } => {
                let Some(workspace) = workspace.canonicalize().ok().filter(|path| path.is_dir())
                else {
                    send_error(
                        &updates,
                        generation,
                        ControllerError::InvalidWorkspace(
                            "workspace must be an existing directory".to_owned(),
                        ),
                    );
                    continue;
                };
                let Some(meta) = crate::config::session_meta_by_id(&session_id) else {
                    send_error(
                        &updates,
                        generation,
                        ControllerError::Session(format!("session not found: {session_id}")),
                    );
                    continue;
                };
                if crate::config::load_session_file(&meta.path).is_empty() {
                    send_error(
                        &updates,
                        generation,
                        ControllerError::Session(format!("session has no history: {session_id}")),
                    );
                    continue;
                }
                if let Some(previous) = active.take() {
                    retire_session(previous, &updates).await;
                }
                let mut state = AppState::new_with_workspace_session(&workspace, Some(&session_id));
                state.workspace_root = Some(workspace.clone());
                state.task_working_directory = Some(workspace);
                if let Err(error) = crate::app::session_controller::SessionController::default()
                    .resume(
                        &mut state,
                        crate::app::SessionAction::Id(session_id.clone()),
                    )
                {
                    send_error(
                        &updates,
                        generation,
                        ControllerError::Session(error.to_string()),
                    );
                    continue;
                }
                if let Some(profile_name) = crate::config::load_session_settings(&session_id)
                    .map(|settings| settings.active_profile)
                    && let Some((model, url)) = state
                        .config
                        .models
                        .iter()
                        .find(|profile| {
                            profile.name == profile_name || profile.model == profile_name
                        })
                        .map(|profile| (profile.model.clone(), profile.url.clone()))
                {
                    state.model_name = model;
                    state.api_base_url = url;
                }
                generation += 1;
                state.auto_confirm = true;
                let session = ActiveSession {
                    generation,
                    state: Arc::new(Mutex::new(state)),
                    cancel_token: CancellationToken::new(),
                    turn_task: None,
                };
                send_snapshot(&updates, generation, &session.state).await;
                active = Some(session);
            }
            Command::Submit(prompt) => {
                let Some(session) = active.as_mut() else {
                    send_error(&updates, generation, ControllerError::NoActiveSession);
                    continue;
                };
                match queue_prompt(&session.state, prompt).await {
                    QueuePrompt::Empty => {
                        send_snapshot(&updates, session.generation, &session.state).await;
                    }
                    QueuePrompt::Queued => {}
                    QueuePrompt::Start(lease, starting_history_len) => {
                        session.turn_task = Some(spawn_turn(
                            session.generation,
                            Arc::clone(&session.state),
                            session.cancel_token.clone(),
                            client.clone(),
                            lease,
                            starting_history_len,
                            updates.clone(),
                        ));
                    }
                }
            }
            Command::Cancel => {
                if let Some(session) = active.as_mut() {
                    cancel_active_turn(session, &updates).await;
                } else {
                    send_error(&updates, generation, ControllerError::NoActiveSession);
                }
            }
            Command::SelectModel(model) => {
                let Some(session) = active.as_ref() else {
                    send_error(&updates, generation, ControllerError::NoActiveSession);
                    continue;
                };
                let mut state = session.state.lock().await;
                if let Some((model_name, api_base_url, profile_name)) = state
                    .config
                    .models
                    .iter()
                    .find(|profile| profile.model == model || profile.name == model)
                    .map(|profile| {
                        (
                            profile.model.clone(),
                            profile.url.clone(),
                            profile.name.clone(),
                        )
                    })
                {
                    state.model_name = model_name;
                    state.api_base_url = api_base_url;
                    crate::config::record_session_settings_for_profile(
                        &state.active_session_id,
                        &state.config,
                        &profile_name,
                    );
                    send_snapshot_locked(&updates, session.generation, &state);
                } else {
                    send_error(&updates, session.generation, ControllerError::Model(model));
                }
            }
            Command::ListSessions => {
                if let Some(session) = active.as_ref() {
                    let state = session.state.lock().await;
                    let sessions = crate::app::actions::build_session_list(&state);
                    let mut snapshot = ControllerSnapshot::from_state(session.generation, &state);
                    snapshot.sessions = sessions
                        .into_iter()
                        .map(|session| SessionChoice {
                            id: crate::config::session_id_from_path(&session.path)
                                .unwrap_or_default(),
                            title: session.title,
                            when: session.when,
                            message_count: session.message_count,
                        })
                        .collect();
                    if crate::config::session_has_content(&state.history)
                        && !snapshot
                            .sessions
                            .iter()
                            .any(|choice| choice.id == state.active_session_id)
                    {
                        snapshot.sessions.insert(
                            0,
                            SessionChoice {
                                id: state.active_session_id.clone(),
                                title: crate::config::session_title(&state.history),
                                when: state
                                    .history
                                    .first()
                                    .map(|message| message.timestamp.clone())
                                    .unwrap_or_default(),
                                message_count: state.history.len(),
                            },
                        );
                    }
                    let _ = updates.send(ControllerEvent {
                        generation: session.generation,
                        update: ControllerUpdate::Snapshot(snapshot),
                    });
                } else {
                    let mut state = AppState::new_with_workspace_session(&launch_dir, Some(""));
                    state.workspace_root = Some(launch_dir.clone());
                    state.task_working_directory = Some(launch_dir.clone());
                    state.history_picker_sessions = crate::app::actions::build_session_list(&state);
                    let mut snapshot = ControllerSnapshot::from_state(generation, &state);
                    snapshot.session_id = None;
                    let _ = updates.send(ControllerEvent {
                        generation,
                        update: ControllerUpdate::Snapshot(snapshot),
                    });
                }
            }
            Command::AnswerQuestion(answer) => {
                let Some(session) = active.as_mut() else {
                    send_error(&updates, generation, ControllerError::NoActiveSession);
                    continue;
                };
                match answer_question(&session.state, &mut session.cancel_token, answer).await {
                    Ok(()) => send_snapshot(&updates, session.generation, &session.state).await,
                    Err(error) => send_error(&updates, session.generation, error),
                }
            }
            Command::Approval(choice) => {
                let Some(session) = active.as_mut() else {
                    send_error(&updates, generation, ControllerError::NoActiveSession);
                    continue;
                };
                match apply_approval(&session.state, &mut session.cancel_token, choice).await {
                    Ok(()) => send_snapshot(&updates, session.generation, &session.state).await,
                    Err(error) => send_error(&updates, session.generation, error),
                }
            }
        }
    }
    if let Some(session) = active {
        retire_session(session, &updates).await;
    }
}

async fn retire_session(
    mut session: ActiveSession,
    updates: &mpsc::UnboundedSender<ControllerEvent>,
) {
    if session.turn_task.is_some() {
        cancel_active_turn_inner(&mut session, updates, false).await;
    } else {
        session.cancel_token.cancel();
    }
    let state = session.state.lock().await;
    crate::config::save_session_history(&state.active_session_id, &state.history);
    crate::config::flush_history();
}

fn empty_snapshot(generation: u64) -> ControllerSnapshot {
    ControllerSnapshot {
        generation,
        workspace: None,
        session_id: None,
        sessions: Vec::new(),
        models: Vec::new(),
        selected_model: None,
        transcript: Vec::new(),
        live_response: String::new(),
        queued_count: 0,
        turn_active: false,
        pending_question: None,
        pending_approval: None,
    }
}

async fn send_snapshot(
    updates: &mpsc::UnboundedSender<ControllerEvent>,
    generation: u64,
    state: &Arc<Mutex<AppState>>,
) {
    let state = state.lock().await;
    send_snapshot_locked(updates, generation, &state);
}

fn send_snapshot_locked(
    updates: &mpsc::UnboundedSender<ControllerEvent>,
    generation: u64,
    state: &AppState,
) {
    let _ = updates.send(ControllerEvent {
        generation,
        update: ControllerUpdate::Snapshot(ControllerSnapshot::from_state(generation, state)),
    });
}

fn send_error(
    updates: &mpsc::UnboundedSender<ControllerEvent>,
    generation: u64,
    error: ControllerError,
) {
    let _ = updates.send(ControllerEvent {
        generation,
        update: ControllerUpdate::Error(error),
    });
}

pub(super) enum QueuePrompt {
    Empty,
    Queued,
    Start(crate::app::OrchestratorLease, usize),
}

pub(super) async fn queue_prompt(state: &Arc<Mutex<AppState>>, prompt: String) -> QueuePrompt {
    let mut state = state.lock().await;
    if crate::app::submit_plain_prompt(&mut state, prompt) == crate::app::SubmitOutcome::Empty {
        return QueuePrompt::Empty;
    }
    let starting_history_len = state.history.len();
    let lease = if !state.orchestrator_running {
        let lease = state.claim_orchestrator();
        if lease.is_some() {
            state.status = crate::app::AppStatus::Queued;
        }
        lease
    } else {
        None
    };
    lease.map_or(QueuePrompt::Queued, |lease| {
        QueuePrompt::Start(lease, starting_history_len)
    })
}

pub(super) async fn cancel_active_turn(
    session: &mut ActiveSession,
    updates: &mpsc::UnboundedSender<ControllerEvent>,
) {
    cancel_active_turn_inner(session, updates, true).await;
}

async fn cancel_active_turn_inner(
    session: &mut ActiveSession,
    updates: &mpsc::UnboundedSender<ControllerEvent>,
    publish_snapshot: bool,
) {
    {
        let mut state = session.state.lock().await;
        if let Some(response) = state.tool_confirmation_response.take() {
            let _ = response.send(crate::app::ToolConfirmationResponse::Deny);
        }
        if let Some(response) = state.question_response.take() {
            let _ = response.send("User cancelled prompt.".to_owned());
        }
        state.pending_tool_confirmation = None;
        state.pending_question = None;
        state.clear_question_chain();
    }
    session.cancel_token.cancel();
    if let Some(turn_task) = session.turn_task.take()
        && turn_task.await.is_err()
    {
        send_error(
            updates,
            session.generation,
            ControllerError::Provider("turn worker stopped unexpectedly".to_owned()),
        );
    }
    {
        let mut state = session.state.lock().await;
        state.enter_idle();
        state.clear_active_turn_projection();
    }
    // The old queue has unwound before this fresh token is used for another
    // turn in the same session.
    session.cancel_token = CancellationToken::new();
    if publish_snapshot {
        send_snapshot(updates, session.generation, &session.state).await;
    }
}

pub(super) async fn answer_question(
    state: &Arc<Mutex<AppState>>,
    cancel_token: &mut CancellationToken,
    answer: String,
) -> Result<(), ControllerError> {
    let answer = {
        let state = state.lock().await;
        let Some(question) = state.pending_question.as_ref() else {
            return Err(ControllerError::Session(
                "there is no pending question".to_owned(),
            ));
        };
        if state.question_response.is_none() {
            return Err(ControllerError::Session(
                "the pending question has no response channel".to_owned(),
            ));
        }
        if question.options.iter().any(|option| option == &answer) {
            crate::app::QuestionAnswer::Selected(answer)
        } else {
            crate::app::QuestionAnswer::Custom(answer)
        }
    };
    crate::app::runtime::apply_question_answer(state, cancel_token, answer).await;
    Ok(())
}

pub(super) async fn apply_approval(
    state: &Arc<Mutex<AppState>>,
    cancel_token: &mut CancellationToken,
    choice: ApprovalChoice,
) -> Result<(), ControllerError> {
    {
        let state = state.lock().await;
        if state.pending_tool_confirmation.is_none() || state.tool_confirmation_response.is_none() {
            return Err(ControllerError::Session(
                "there is no pending tool approval".to_owned(),
            ));
        }
    }
    let decision = match choice {
        ApprovalChoice::Approve => crate::app::ApprovalDecision::Approve,
        ApprovalChoice::Deny => crate::app::ApprovalDecision::Deny,
    };
    crate::app::runtime::apply_approval_decision(state, cancel_token, decision).await;
    Ok(())
}

fn spawn_turn(
    generation: u64,
    state: Arc<Mutex<AppState>>,
    cancel_token: CancellationToken,
    client: reqwest::Client,
    lease: crate::app::OrchestratorLease,
    starting_history_len: usize,
    updates: mpsc::UnboundedSender<ControllerEvent>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let (event_sender, mut event_receiver) =
            crate::network::ui_adapter::AgentUiEventSender::channel();
        let queue_state = Arc::clone(&state);
        let recovery_lease = lease.clone();
        let queue_task = tokio::spawn(crate::network::process_queue_orchestrator_with_ui_events(
            client,
            queue_state,
            cancel_token,
            Arc::new(crate::network::policy::InteractivePolicy),
            event_sender,
            lease,
        ));
        let mut queue_task = queue_task;
        let mut event_stream_open = true;
        let mut queue_result = None;
        while event_stream_open || queue_result.is_none() {
            tokio::select! {
                event = event_receiver.recv(), if event_stream_open => {
                    match event {
                        Some(event) => {
                            if let Some(public) = events::from_agent_ui_event(generation, event) {
                                let _ = updates.send(public);
                            }
                        }
                        None => event_stream_open = false,
                    }
                }
                result = &mut queue_task, if queue_result.is_none() => {
                    queue_result = Some(result);
                }
            }
        }
        if matches!(&queue_result, Some(Err(_))) {
            let mut state = state.lock().await;
            state.release_orchestrator(&recovery_lease);
            state.enter_idle();
            state.clear_active_turn_projection();
            drop(state);
            send_error(
                &updates,
                generation,
                ControllerError::Provider("turn worker stopped unexpectedly".to_owned()),
            );
        }
        let snapshot = {
            let state = state.lock().await;
            let provider_error =
                state
                    .history
                    .iter()
                    .skip(starting_history_len)
                    .find_map(|message| {
                        (message.role == "system"
                            && (message.content.starts_with("Error from LLM Provider:")
                                || message
                                    .content
                                    .starts_with("[Recoverable provider interruption:")))
                        .then(|| message.content.clone())
                    });
            (
                ControllerSnapshot::from_state(generation, &state),
                provider_error,
            )
        };
        if let Some(message) = snapshot.1 {
            send_error(&updates, generation, ControllerError::Provider(message));
        }
        let _ = updates.send(ControllerEvent {
            generation,
            update: ControllerUpdate::Snapshot(snapshot.0),
        });
    })
}
