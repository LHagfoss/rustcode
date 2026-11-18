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
            &serde_json::from_value::<agent_client_protocol::schema::v1::ClientCapabilities>(value)
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
            &serde_json::from_value::<agent_client_protocol::schema::v1::ClientCapabilities>(value)
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
    crate::config::save_session_repo(&id, Some(std::path::Path::new("/Users/me/code/rustcode")));
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
        content: "<think>The time is 12:20 PM.</think>The current time is 12:20 PM.".to_string(),
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
            SessionUpdate::AgentThoughtChunk(chunk) => match &chunk.content {
                ContentBlock::Text(text) => Some((
                    text.text.clone(),
                    serde_json::Value::Object(chunk.meta.clone().unwrap_or_default())["rustcode"]
                        .clone(),
                )),
                _ => None,
            },
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
    let mut marker = crate::app::ChatMessage::new("system", "[harness: turn stopped — cancelled]");
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
            SessionUpdate::UserMessageChunk(chunk) | SessionUpdate::AgentMessageChunk(chunk) => {
                Some(chunk.meta.clone().unwrap())
            }
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
    let origin = |index: usize| metas[index].as_ref().expect("meta")["rustcode"]["origin"].clone();
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
                            assert_eq!(opt.description.as_deref(), Some(profile.model.as_str()));
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
            async move |_request: agent_client_protocol::UntypedMessage, responder, _connection| {
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
    InitializeRequest::new(agent_client_protocol::schema::ProtocolVersion::V1).client_capabilities(
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
            async move |_request: agent_client_protocol::UntypedMessage, responder, _connection| {
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
        ) -> Result<crate::remote::owner::OwnerLink, crate::remote::owner::OwnerLinkError> {
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
    let logged =
        assert_the_loader_gets_what_happened_while_it_loaded(&notes, &response, base, &transcript);
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
                let mut record = crate::harness::store::HarnessRecord::new("claude-code", cwd_path);
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
