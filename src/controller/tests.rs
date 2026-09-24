use super::{
    Command, ControllerSnapshot, ControllerUpdate, InteractiveController, accepts_generation,
};
use crate::app::{AppState, AppStatus, ChatMessage, PendingQuestion, ToolConfirmation};
use std::time::Duration;

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
