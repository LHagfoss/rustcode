use super::{
    Command, ControllerSnapshot, ControllerUpdate, InteractiveController, accepts_generation,
};
use crate::app::{AppState, AppStatus, ChatMessage, PendingQuestion, ToolConfirmation};
use std::time::Duration;

#[tokio::test]
async fn lifecycle_lists_saved_sessions_and_resumes_them_in_the_chosen_workspace() {
    use tokio::io::AsyncWriteExt;

    let workspace = tempfile::tempdir().expect("temporary workspace");
    std::fs::create_dir(workspace.path().join(".rustcode")).expect("project config directory");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock provider");
    let provider = format!(
        "http://{}/v1/chat/completions",
        listener.local_addr().unwrap()
    );
    std::fs::write(
        workspace.path().join(".rustcode/config.toml"),
        format!(
            "default = \"controller-mock\"\n[[models]]\nname = \"controller-mock\"\nurl = \"{provider}\"\nmodel = \"controller-mock\"\ntool_protocol = \"native\"\n"
        ),
    )
    .expect("project config");
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("provider request");
        read_provider_request(&mut socket).await;
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: {\"choices\":[{\"delta\":{\"content\":\"saved answer\"}}]}\n\n")
            .await
            .expect("send answer");
        socket
            .write_all(b"data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n")
            .await
            .expect("finish answer");
        socket
            .write_all(b"data: [DONE]\n\n")
            .await
            .expect("finish stream");
    });

    let (handle, mut updates) = InteractiveController::spawn(
        &tokio::runtime::Handle::current(),
        workspace.path().to_path_buf(),
    );
    let _initial = updates.recv().await.expect("initial snapshot");
    handle
        .send(Command::StartNew(workspace.path().to_path_buf()))
        .expect("start saved session");
    let started = updates.recv().await.expect("start snapshot");
    let ControllerUpdate::Snapshot(started) = started.update else {
        panic!("StartNew should return a snapshot");
    };
    let saved_id = started.session_id.expect("session ID");
    handle
        .send(Command::Submit("saved prompt".to_owned()))
        .expect("save a conversation turn");
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(event) = updates.recv().await {
            if matches!(event.update, ControllerUpdate::Snapshot(snapshot) if !snapshot.turn_active)
            {
                break;
            }
        }
    })
    .await
    .expect("saved turn completion");
    server.await.expect("mock provider");
    handle.send(Command::ListSessions).expect("list sessions");
    let listed = updates.recv().await.expect("session list snapshot");
    let ControllerUpdate::Snapshot(listed) = listed.update else {
        panic!("ListSessions should return a snapshot");
    };
    assert!(
        listed.sessions.iter().any(|session| session.id == saved_id),
        "expected {saved_id}, session list: {:?}",
        listed.sessions,
    );

    handle
        .send(Command::Resume {
            session_id: saved_id.clone(),
            workspace: workspace.path().to_path_buf(),
        })
        .expect("resume saved session");
    let resumed = loop {
        let event = updates.recv().await.expect("resumed snapshot");
        if event.generation > listed.generation {
            break event;
        }
    };
    let ControllerUpdate::Snapshot(resumed) = resumed.update else {
        panic!("Resume should return a snapshot");
    };
    assert_eq!(
        resumed.workspace.as_deref(),
        Some(
            workspace
                .path()
                .canonicalize()
                .expect("canonical workspace")
                .as_path()
        )
    );
    assert_eq!(resumed.session_id.as_deref(), Some(saved_id.as_str()));
    assert_eq!(resumed.transcript[0].content, "saved prompt");
    assert!(resumed.generation > listed.generation);

    handle.send(Command::Shutdown).expect("shutdown");
    assert!(
        updates.recv().await.is_none(),
        "shutdown should finish worker"
    );
    let persisted = crate::config::load_session_history_direct(&saved_id);
    assert!(
        persisted
            .iter()
            .any(|message| message.content == "saved prompt")
    );
}

#[tokio::test]
async fn lifecycle_rejects_invalid_workspace_and_model_without_replacing_session() {
    let workspace = tempfile::tempdir().expect("temporary workspace");
    let (handle, mut updates) = InteractiveController::spawn(
        &tokio::runtime::Handle::current(),
        workspace.path().to_path_buf(),
    );
    let _initial = updates.recv().await.expect("initial snapshot");
    handle
        .send(Command::StartNew(workspace.path().to_path_buf()))
        .expect("start session");
    let started = updates.recv().await.expect("start snapshot");
    let ControllerUpdate::Snapshot(started) = started.update else {
        panic!("StartNew should return a snapshot");
    };

    handle
        .send(Command::SelectModel("not-configured".to_owned()))
        .expect("select invalid model");
    assert!(matches!(
        updates.recv().await.expect("model error").update,
        ControllerUpdate::Error(super::ControllerError::Model(_))
    ));
    handle
        .send(Command::StartNew(workspace.path().join("missing")))
        .expect("start invalid workspace");
    assert!(matches!(
        updates.recv().await.expect("workspace error").update,
        ControllerUpdate::Error(super::ControllerError::InvalidWorkspace(_))
    ));

    handle.send(Command::ListSessions).expect("list sessions");
    let unchanged = updates.recv().await.expect("unchanged snapshot");
    let ControllerUpdate::Snapshot(unchanged) = unchanged.update else {
        panic!("ListSessions should return a snapshot");
    };
    assert_eq!(unchanged.session_id, started.session_id);
    assert_eq!(unchanged.generation, started.generation);
    handle.send(Command::Shutdown).expect("shutdown");
}

#[tokio::test]
async fn lifecycle_switch_cancels_old_turn_before_publishing_new_generation() {
    use tokio::io::AsyncWriteExt;

    let old_workspace = tempfile::tempdir().expect("old workspace");
    std::fs::create_dir(old_workspace.path().join(".rustcode"))
        .expect("old project config directory");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock provider");
    let provider = format!(
        "http://{}/v1/chat/completions",
        listener.local_addr().unwrap()
    );
    std::fs::write(
        old_workspace.path().join(".rustcode/config.toml"),
        format!(
            "default = \"controller-mock\"\n[[models]]\nname = \"controller-mock\"\nurl = \"{provider}\"\nmodel = \"controller-mock\"\ntool_protocol = \"native\"\n"
        ),
    )
    .expect("old project config");
    let (first_chunk_tx, first_chunk_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("provider request");
        read_provider_request(&mut socket).await;
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
            )
            .await
            .expect("send headers");
        socket
            .write_all(
                b"data: {\"choices\":[{\"delta\":{\"content\":\"old workspace response\"}}]}\n\n",
            )
            .await
            .expect("send first chunk");
        let _ = first_chunk_tx.send(());
        let _ = release_rx.await;
        let _ = socket.write_all(b"data: [DONE]\n\n").await;
    });

    let new_workspace = tempfile::tempdir().expect("new workspace");
    let (handle, mut updates) = InteractiveController::spawn(
        &tokio::runtime::Handle::current(),
        old_workspace.path().to_path_buf(),
    );
    let _initial = updates.recv().await.expect("initial snapshot");
    handle
        .send(Command::StartNew(old_workspace.path().to_path_buf()))
        .expect("start old session");
    let started = updates.recv().await.expect("old session snapshot");
    let old_generation = started.generation;
    handle
        .send(Command::Submit("old prompt".to_owned()))
        .expect("submit old prompt");
    tokio::time::timeout(Duration::from_secs(10), first_chunk_rx)
        .await
        .expect("provider first chunk timeout")
        .expect("first chunk signal");

    handle
        .send(Command::StartNew(new_workspace.path().to_path_buf()))
        .expect("switch workspace");
    let mut saw_old_cancel = false;
    let switched = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(event) = updates.recv().await {
            if event.generation == old_generation
                && matches!(
                    event.update,
                    ControllerUpdate::Turn(super::TurnUpdate::Cancelled)
                )
            {
                saw_old_cancel = true;
            }
            if event.generation > old_generation
                && let ControllerUpdate::Snapshot(snapshot) = event.update
            {
                return snapshot;
            }
        }
        panic!("controller closed before workspace switch completed");
    })
    .await
    .expect("workspace switch timeout");
    assert!(saw_old_cancel);
    assert!(switched.generation > old_generation);
    assert_eq!(
        switched.workspace.as_deref(),
        Some(
            new_workspace
                .path()
                .canonicalize()
                .expect("canonical workspace")
                .as_path()
        )
    );
    let stale = super::ControllerEvent {
        generation: old_generation,
        update: ControllerUpdate::Turn(super::TurnUpdate::TextDelta("late".to_owned())),
    };
    assert!(!accepts_generation(switched.generation, &stale));

    let _ = release_tx.send(());
    let _ = server.await;
    handle.send(Command::Shutdown).expect("shutdown");
}

#[test]
fn snapshot_projects_session_transcript_runtime_state_without_terminal_fields() {
    let workspace = tempfile::tempdir().expect("temporary workspace");
    let mut state = AppState::new_with_workspace_session(workspace.path(), Some("session-7"));
    state.workspace_root = Some(workspace.path().to_path_buf());
    let nested_workspace = workspace.path().join("nested");
    std::fs::create_dir(&nested_workspace).expect("nested workspace");
    state.task_working_directory = Some(nested_workspace.clone());
    state
        .history_picker_sessions
        .push(rustcode_session::SessionMeta {
            path: workspace.path().join("sessions/session-8.jsonl"),
            title: "Saved session".to_owned(),
            when: "today".to_owned(),
            message_count: 4,
        });
    state.active_session_id = "session-7".to_owned();
    state.model_name = "model-7".to_owned();
    state.history.push(ChatMessage::new("user", "first"));
    state.history.push(ChatMessage::new("assistant", "second"));
    state.current_response = std::sync::Arc::new("live".to_owned());
    state.pending_queue = vec!["queued one".to_owned(), "queued two".to_owned()];
    state.status = AppStatus::AwaitingQuestion;
    state.pending_question = Some(PendingQuestion::new(
        "Pick one".to_owned(),
        vec!["A".to_owned(), "B".to_owned()],
        false,
    ));
    state.pending_tool_confirmation = Some(vec![ToolConfirmation {
        tool_name: "write_file".to_owned(),
        path: "src/main.rs".to_owned(),
        content_preview: "fn main() {}".to_owned(),
        content_bytes: 12,
        rememberable_prefix: None,
        forbidden_prefix: None,
    }]);

    let snapshot = ControllerSnapshot::from_state(7, &state);

    assert_eq!(snapshot.generation, 7);
    assert_eq!(snapshot.session_id.as_deref(), Some("session-7"));
    assert_eq!(
        snapshot.workspace.as_deref(),
        Some(nested_workspace.as_path())
    );
    assert_eq!(snapshot.selected_model.as_deref(), Some("model-7"));
    assert_eq!(
        snapshot.sessions,
        [super::SessionChoice {
            id: "session-8".to_owned(),
            title: "Saved session".to_owned(),
            when: "today".to_owned(),
            message_count: 4,
        }]
    );
    assert_eq!(
        snapshot.models,
        state
            .config
            .models
            .iter()
            .map(|model| super::ModelChoice {
                id: model.model.clone(),
                label: model.name.clone(),
            })
            .collect::<Vec<_>>()
    );
    assert_eq!(
        snapshot
            .transcript
            .iter()
            .map(|item| item.content.as_str())
            .collect::<Vec<_>>(),
        ["first", "second"]
    );
    assert_eq!(snapshot.queued_count, 2);
    assert!(snapshot.turn_active);
    assert_eq!(snapshot.live_response, "live");
    let question = snapshot.pending_question.expect("question projection");
    assert_eq!(question.text, "Pick one");
    assert_eq!(question.options, ["A", "B"]);
    assert!(!question.multiple);
    let approval = snapshot.pending_approval.expect("approval projection");
    assert_eq!(approval.tool_name, "write_file");
    assert_eq!(approval.description, "src/main.rs\nfn main() {}");
}

#[tokio::test]
async fn controller_question_answer_resolves_the_existing_response_channel() {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let mut state = AppState::new();
    state.status = AppStatus::AwaitingQuestion;
    state.pending_question = Some(PendingQuestion::new(
        "Continue?".to_owned(),
        vec!["Proceed".to_owned()],
        false,
    ));
    state.question_response = Some(tx);
    let state = std::sync::Arc::new(tokio::sync::Mutex::new(state));
    let mut cancel_token = tokio_util::sync::CancellationToken::new();

    super::worker::answer_question(&state, &mut cancel_token, "Proceed".to_owned())
        .await
        .expect("pending question should accept its answer");

    assert_eq!(
        rx.await.expect("question response"),
        "User selected: Proceed"
    );
    assert!(state.lock().await.pending_question.is_none());
}

#[tokio::test]
async fn controller_approval_resolves_the_existing_response_channel() {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let mut state = AppState::new();
    state.status = AppStatus::AwaitingToolConfirmation;
    state.pending_tool_confirmation = Some(vec![ToolConfirmation {
        tool_name: "write_file".to_owned(),
        path: "src/main.rs".to_owned(),
        content_preview: "fn main() {}".to_owned(),
        content_bytes: 12,
        rememberable_prefix: None,
        forbidden_prefix: None,
    }]);
    state.tool_confirmation_response = Some(tx);
    let state = std::sync::Arc::new(tokio::sync::Mutex::new(state));
    let mut cancel_token = tokio_util::sync::CancellationToken::new();

    super::worker::apply_approval(&state, &mut cancel_token, super::ApprovalChoice::Approve)
        .await
        .expect("pending approval should accept a choice");

    assert_eq!(
        rx.await.expect("approval response"),
        crate::app::ToolConfirmationResponse::Approve
    );
    assert!(state.lock().await.pending_tool_confirmation.is_none());
}

#[tokio::test]
async fn cancelled_turn_can_be_followed_by_a_new_submit() {
    use tokio::io::AsyncWriteExt;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock provider");
    let provider = format!(
        "http://{}/v1/chat/completions",
        listener.local_addr().unwrap()
    );
    let (first_chunk_tx, first_chunk_rx) = tokio::sync::oneshot::channel();
    let (release_first_tx, release_first_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut first_socket, _) = listener.accept().await.expect("first provider request");
        read_provider_request(&mut first_socket).await;
        first_socket
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
            )
            .await
            .expect("send first response headers");
        first_socket
            .write_all(b"data: {\"choices\":[{\"delta\":{\"content\":\"first chunk\"}}]}\n\n")
            .await
            .expect("send first response chunk");
        let _ = first_chunk_tx.send(());
        let _ = release_first_rx.await;
        let _ = first_socket
            .write_all(b"data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n")
            .await;
        let _ = first_socket.write_all(b"data: [DONE]\n\n").await;
        drop(first_socket);

        let (mut second_socket, _) = listener.accept().await.expect("second provider request");
        read_provider_request(&mut second_socket).await;
        second_socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: {\"choices\":[{\"delta\":{\"content\":\"second chunk\"}}]}\n\n")
            .await
            .expect("send second response text");
        tokio::time::sleep(Duration::from_millis(60)).await;
        second_socket
            .write_all(b"data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n")
            .await
            .expect("send second response finish");
        second_socket
            .write_all(b"data: [DONE]\n\n")
            .await
            .expect("finish second stream");
    });

    let workspace = tempfile::tempdir().expect("temporary workspace");
    std::fs::create_dir(workspace.path().join(".rustcode")).expect("project config directory");
    std::fs::write(
        workspace.path().join(".rustcode/config.toml"),
        format!(
            "default = \"controller-mock\"\n[[models]]\nname = \"controller-mock\"\nurl = \"{provider}\"\nmodel = \"controller-mock\"\ntool_protocol = \"native\"\n"
        ),
    )
    .expect("project config");
    let (handle, mut updates) = InteractiveController::spawn(
        &tokio::runtime::Handle::current(),
        workspace.path().to_path_buf(),
    );
    let _initial = updates.recv().await.expect("initial snapshot");
    handle
        .send(Command::StartNew(workspace.path().to_path_buf()))
        .expect("start session");
    let _started = updates.recv().await.expect("start snapshot");
    handle
        .send(Command::Submit("first prompt".to_owned()))
        .expect("submit first prompt");

    tokio::time::timeout(Duration::from_secs(10), first_chunk_rx)
        .await
        .expect("provider first chunk timeout")
        .expect("first chunk signal");
    let mut saw_first_text = false;
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(event) = updates.recv().await {
            if matches!(event.update, ControllerUpdate::Turn(super::TurnUpdate::TextDelta(ref text)) if text == "first chunk") {
                saw_first_text = true;
                break;
            }
        }
    })
    .await
    .expect("first text delta timeout");
    assert!(saw_first_text);
    handle.send(Command::Cancel).expect("cancel active turn");
    let _ = release_first_tx.send(());
    handle
        .send(Command::Submit("second prompt".to_owned()))
        .expect("submit while cancellation unwinds");

    let mut saw_cancelled = false;
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(event) = updates.recv().await {
            match event.update {
                ControllerUpdate::Turn(super::TurnUpdate::Cancelled) => saw_cancelled = true,
                ControllerUpdate::Snapshot(snapshot) if saw_cancelled && !snapshot.turn_active => {
                    break;
                }
                _ => {}
            }
        }
    })
    .await
    .expect("cancel completion timeout");
    assert!(saw_cancelled);

    let mut saw_second_prompt = false;
    let mut saw_second_text = false;
    tokio::time::timeout(Duration::from_secs(15), async {
        while let Some(event) = updates.recv().await {
            match event.update {
                ControllerUpdate::Turn(super::TurnUpdate::PromptStarted(prompt))
                    if prompt == "second prompt" =>
                {
                    saw_second_prompt = true
                }
                ControllerUpdate::Turn(super::TurnUpdate::TextDelta(text))
                    if text == "second chunk" =>
                {
                    saw_second_text = true
                }
                ControllerUpdate::Turn(super::TurnUpdate::TurnFinished) if saw_second_prompt => {
                    break;
                }
                _ => {}
            }
        }
    })
    .await
    .expect("second turn timeout");
    assert!(saw_second_prompt);
    assert!(saw_second_text);
    server.await.expect("mock provider server");
    handle.send(Command::Shutdown).expect("shutdown");
}

#[tokio::test]
async fn cancel_resolves_pending_approval_and_question_before_followup_submit() {
    let (approval_tx, approval_rx) = tokio::sync::oneshot::channel();
    let (question_tx, question_rx) = tokio::sync::oneshot::channel();
    let mut state = AppState::new();
    state.status = AppStatus::AwaitingToolConfirmation;
    state.pending_tool_confirmation = Some(vec![ToolConfirmation {
        tool_name: "write_file".to_owned(),
        path: "src/main.rs".to_owned(),
        content_preview: "fn main() {}".to_owned(),
        content_bytes: 12,
        rememberable_prefix: None,
        forbidden_prefix: None,
    }]);
    state.tool_confirmation_response = Some(approval_tx);
    state.pending_question = Some(PendingQuestion::new(
        "Continue?".to_owned(),
        vec!["Proceed".to_owned()],
        false,
    ));
    state.question_response = Some(question_tx);
    let lease = state.claim_orchestrator().expect("active queue lease");
    let state = std::sync::Arc::new(tokio::sync::Mutex::new(state));
    let (observed_tx, observed_rx) = tokio::sync::oneshot::channel();
    let waiter_state = std::sync::Arc::clone(&state);
    let turn_task = tokio::spawn(async move {
        let approval = approval_rx.await.expect("approval should be resolved");
        let question = question_rx.await.expect("question should be resolved");
        waiter_state.lock().await.release_orchestrator(&lease);
        let _ = observed_tx.send((approval, question));
    });
    let mut session = super::worker::ActiveSession {
        generation: 1,
        state: std::sync::Arc::clone(&state),
        cancel_token: tokio_util::sync::CancellationToken::new(),
        turn_task: Some(turn_task),
    };
    let (updates, _receiver) = tokio::sync::mpsc::unbounded_channel();

    tokio::time::timeout(
        Duration::from_secs(3),
        super::worker::cancel_active_turn(&mut session, &updates),
    )
    .await
    .expect("pending interactions must not block cancellation");

    let (approval, question) = observed_rx.await.expect("waiter result");
    assert_eq!(approval, crate::app::ToolConfirmationResponse::Deny);
    assert_eq!(question, "User cancelled prompt.");
    assert!(!session.cancel_token.is_cancelled());
    let queued = super::worker::queue_prompt(&session.state, "after cancel".to_owned()).await;
    let super::worker::QueuePrompt::Start(lease, _) = queued else {
        panic!("follow-up submit should claim a fresh orchestrator lease");
    };
    session.state.lock().await.release_orchestrator(&lease);
}

async fn read_provider_request(socket: &mut tokio::net::TcpStream) {
    use tokio::io::AsyncReadExt;

    let mut request = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        let read = socket
            .read(&mut buffer)
            .await
            .expect("read provider request");
        if read == 0 {
            break;
        }
        request.extend_from_slice(&buffer[..read]);
        let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") else {
            continue;
        };
        let headers = String::from_utf8_lossy(&request[..header_end]);
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
            .unwrap_or(0);
        if request.len() >= header_end + 4 + content_length {
            break;
        }
    }
}

#[test]
fn generation_filter_rejects_events_from_an_older_session() {
    let event = super::ControllerEvent {
        generation: 6,
        update: super::ControllerUpdate::Turn(super::TurnUpdate::Cancelled),
    };
    assert!(!accepts_generation(7, &event));
    assert!(accepts_generation(6, &event));
}

#[tokio::test]
async fn worker_starts_without_a_session_and_rejects_submit_until_selection() {
    let runtime = tokio::runtime::Handle::current();
    let workspace = tempfile::tempdir().expect("temporary workspace");
    let (handle, mut updates) = InteractiveController::spawn(&runtime, workspace.path().into());

    let initial = updates.recv().await.expect("initial snapshot");
    assert!(matches!(
        initial.update,
        ControllerUpdate::Snapshot(snapshot) if snapshot.session_id.is_none()
    ));
    assert_eq!(initial.generation, 0);

    handle
        .send(Command::Submit("not yet".to_owned()))
        .expect("command channel is open");
    assert!(matches!(
        updates.recv().await.expect("submit error"),
        super::ControllerEvent {
            generation: 0,
            update: ControllerUpdate::Error(super::ControllerError::NoActiveSession),
        }
    ));
}

#[tokio::test]
async fn starting_workspace_and_submitting_forwards_ordered_turn_and_persists_history() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock provider");
    let provider = format!(
        "http://{}/v1/chat/completions",
        listener.local_addr().unwrap()
    );
    let request = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("provider request");
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4096];
        loop {
            let read = socket
                .read(&mut buffer)
                .await
                .expect("read provider request");
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
            let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n")
            else {
                continue;
            };
            let headers = String::from_utf8_lossy(&request[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or(0);
            if request.len() >= header_end + 4 + content_length {
                break;
            }
        }
        let header =
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";
        socket
            .write_all(header.as_bytes())
            .await
            .expect("send provider headers");
        socket
            .write_all(b"data: {\"choices\":[{\"delta\":{\"content\":\"mock reply\"}}]}\n\n")
            .await
            .expect("send provider text");
        tokio::time::sleep(Duration::from_millis(60)).await;
        socket
            .write_all(b"data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n")
            .await
            .expect("send provider finish");
        socket
            .write_all(b"data: [DONE]\n\n")
            .await
            .expect("close provider stream");
        String::from_utf8_lossy(&request).into_owned()
    });

    let workspace = tempfile::tempdir().expect("temporary workspace");
    std::fs::create_dir(workspace.path().join(".rustcode")).expect("project config directory");
    std::fs::write(
            workspace.path().join(".rustcode/config.toml"),
            format!(
                "default = \"controller-mock\"\n[[models]]\nname = \"controller-mock\"\nurl = \"{provider}\"\nmodel = \"controller-mock\"\ntool_protocol = \"native\"\n"
            ),
        ).expect("project config");
    let (handle, mut updates) = InteractiveController::spawn(
        &tokio::runtime::Handle::current(),
        workspace.path().to_path_buf(),
    );
    let _initial = updates.recv().await.expect("initial snapshot");
    handle
        .send(Command::StartNew(workspace.path().to_path_buf()))
        .expect("start session");
    let started = tokio::time::timeout(Duration::from_secs(5), updates.recv())
        .await
        .expect("start snapshot timeout")
        .expect("start snapshot");
    let session_id = match started.update {
        ControllerUpdate::Snapshot(snapshot) => {
            assert_eq!(snapshot.generation, 1);
            assert_eq!(
                snapshot.workspace.as_deref(),
                Some(workspace.path().canonicalize().unwrap().as_path())
            );
            snapshot.session_id.expect("new session id")
        }
        update => panic!("expected start snapshot, got {update:?}"),
    };
    handle
        .send(Command::Submit("hello controller".to_owned()))
        .expect("submit prompt");

    let mut turn_updates = Vec::new();
    let completion = tokio::time::timeout(Duration::from_secs(15), async {
        let mut turn_finished = false;
        while let Some(event) = updates.recv().await {
            assert_eq!(event.generation, 1);
            match event.update {
                ControllerUpdate::Turn(update) => {
                    turn_finished |= update == super::TurnUpdate::TurnFinished;
                    turn_updates.push(update);
                }
                ControllerUpdate::Snapshot(snapshot) if turn_finished && !snapshot.turn_active => {
                    break;
                }
                _ => {}
            }
        }
    })
    .await;
    if completion.is_err() {
        let provider_request = request.await.expect("provider request should finish");
        panic!("turn completion timeout; updates={turn_updates:?}; request={provider_request}");
    }
    assert_eq!(
        turn_updates,
        [
            super::TurnUpdate::PromptStarted("hello controller".to_owned()),
            super::TurnUpdate::TextDelta("mock reply".to_owned()),
            super::TurnUpdate::TurnFinished,
        ]
    );
    let provider_request = request.await.expect("provider task");
    assert!(provider_request.contains("hello controller"));
    crate::config::flush_history();
    let history = crate::config::load_session_history_direct(&session_id);
    assert!(
        history
            .iter()
            .any(|message| message.role == "user" && message.content == "hello controller")
    );
    assert!(
        history
            .iter()
            .any(|message| message.role == "assistant" && message.content == "mock reply")
    );
    handle.send(Command::Shutdown).expect("shutdown");
}

#[tokio::test]
async fn provider_error_is_reported_and_a_later_submit_can_run() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock provider");
    let provider = format!(
        "http://{}/v1/chat/completions",
        listener.local_addr().unwrap()
    );
    let server = tokio::spawn(async move {
        for attempt in 0..2 {
            let (mut socket, _) = listener.accept().await.expect("provider request");
            let mut request = Vec::new();
            let mut buffer = [0_u8; 4096];
            loop {
                let read = socket
                    .read(&mut buffer)
                    .await
                    .expect("read provider request");
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
                let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n")
                else {
                    continue;
                };
                let headers = String::from_utf8_lossy(&request[..header_end]);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    })
                    .unwrap_or(0);
                if request.len() >= header_end + 4 + content_length {
                    break;
                }
            }
            if attempt == 0 {
                socket.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 4\r\nConnection: close\r\n\r\nnope")
                        .await.expect("send provider failure");
            } else {
                let header = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";
                socket
                    .write_all(header.as_bytes())
                    .await
                    .expect("send provider headers");
                socket
                    .write_all(b"data: {\"choices\":[{\"delta\":{\"content\":\"recovered\"}}]}\n\n")
                    .await
                    .expect("send provider text");
                tokio::time::sleep(Duration::from_millis(60)).await;
                socket
                    .write_all(
                        b"data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
                    )
                    .await
                    .expect("send provider finish");
                socket
                    .write_all(b"data: [DONE]\n\n")
                    .await
                    .expect("close provider stream");
            }
        }
    });

    let workspace = tempfile::tempdir().expect("temporary workspace");
    std::fs::create_dir(workspace.path().join(".rustcode")).expect("project config directory");
    std::fs::write(
            workspace.path().join(".rustcode/config.toml"),
            format!(
                "default = \"controller-mock\"\n[[models]]\nname = \"controller-mock\"\nurl = \"{provider}\"\nmodel = \"controller-mock\"\ntool_protocol = \"native\"\n"
            ),
        ).expect("project config");
    let (handle, mut updates) = InteractiveController::spawn(
        &tokio::runtime::Handle::current(),
        workspace.path().to_path_buf(),
    );
    let _initial = updates.recv().await.expect("initial snapshot");
    handle
        .send(Command::StartNew(workspace.path().to_path_buf()))
        .expect("start session");
    let _started = updates.recv().await.expect("start snapshot");
    handle
        .send(Command::Submit("first attempt".to_owned()))
        .expect("submit first prompt");

    let mut observed = Vec::new();
    let first_error = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if let Some(event) = updates.recv().await {
                if let ControllerUpdate::Error(error) = &event.update {
                    break error.clone();
                }
                observed.push(event);
            }
        }
    })
    .await;
    let first_error = first_error.unwrap_or_else(|_error| {
        panic!(
            "provider error timeout; observed={observed:?}; server_done={}",
            server.is_finished()
        )
    });
    assert!(matches!(first_error, super::ControllerError::Provider(_)));

    handle
        .send(Command::Submit("second attempt".to_owned()))
        .expect("submit after provider failure");
    let mut saw_prompt = false;
    let mut saw_recovery_text = false;
    let mut saw_duplicate_error = false;
    tokio::time::timeout(Duration::from_secs(15), async {
        while let Some(event) = updates.recv().await {
            match event.update {
                ControllerUpdate::Turn(super::TurnUpdate::PromptStarted(prompt))
                    if prompt == "second attempt" =>
                {
                    saw_prompt = true
                }
                ControllerUpdate::Turn(super::TurnUpdate::TextDelta(text))
                    if text == "recovered" =>
                {
                    saw_recovery_text = true
                }
                ControllerUpdate::Error(super::ControllerError::Provider(_)) => {
                    saw_duplicate_error = true
                }
                ControllerUpdate::Turn(super::TurnUpdate::TurnFinished) if saw_prompt => break,
                _ => {}
            }
        }
    })
    .await
    .expect("second turn timeout");
    assert!(saw_prompt);
    assert!(saw_recovery_text);
    assert!(!saw_duplicate_error);
    server.await.expect("mock provider server");
    handle.send(Command::Shutdown).expect("shutdown");
}
