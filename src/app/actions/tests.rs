use super::{handle_ctrl_c, label_idle_summary, parse_token_count};

#[test]
fn idle_summary_has_a_deterministic_recap_heading() {
    assert_eq!(
        label_idle_summary("# Summary\n\nThe session is complete."),
        "## Conversation recap\n\n# Summary\n\nThe session is complete."
    );
}

#[test]
fn idle_summary_does_not_duplicate_an_existing_recap_heading() {
    let summary = "## Conversation recap\n\nThe session is complete.";
    assert_eq!(label_idle_summary(summary), summary);
}

async fn pending_response_server() -> (String, tokio::sync::oneshot::Receiver<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test server");
    let address = listener.local_addr().expect("test server address");
    let (accepted_tx, accepted_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let (socket, _) = listener.accept().await.expect("accept request");
        accepted_tx.send(()).ok();
        std::future::pending::<()>().await;
        drop(socket);
    });
    (format!("http://{address}"), accepted_rx)
}

#[test]
fn parse_token_count_plain_and_k_suffix() {
    assert_eq!(parse_token_count("262144"), Some(262144));
    assert_eq!(parse_token_count("256k"), Some(256 * 1024));
    assert_eq!(parse_token_count("256K"), Some(256 * 1024));
    assert_eq!(parse_token_count("abc"), None);
    assert_eq!(parse_token_count(""), None);
}

#[test]
fn background_terminal_commands_list_and_stop_the_active_session_only() {
    let session_id = "actions-background-session";
    let other_session_id = "actions-background-other";
    let task_id = "actions-background-task".to_string();
    let other_task_id = "actions-background-other-task".to_string();
    let long_command = if cfg!(target_os = "windows") {
        "ping -n 30 127.0.0.1 > NUL"
    } else {
        "sleep 30"
    };
    crate::tools::spawn_background_task_for_test(&task_id, session_id, long_command).unwrap();
    crate::tools::spawn_background_task_for_test(&other_task_id, other_session_id, long_command)
        .unwrap();

    for _ in 0..100 {
        if crate::tools::background_task_snapshots(session_id)
            .first()
            .is_some_and(|task| task.child_pid.is_some())
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    let listed = super::background_terminal_list(session_id);
    assert!(listed.contains("1 background terminal running"));
    assert!(listed.contains(&task_id));
    assert!(!listed.contains(&other_task_id));

    assert_eq!(
        super::stop_background_terminals(session_id),
        "Stopped 1 background terminal."
    );
    for _ in 0..100 {
        if crate::tools::background_task_snapshots(session_id).is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(crate::tools::background_task_snapshots(session_id).is_empty());
    assert_eq!(
        crate::tools::background_task_snapshots(other_session_id).len(),
        1
    );
    crate::tools::stop_background_tasks(other_session_id);
}

#[tokio::test]
async fn ctrl_c_requires_a_second_press_to_exit_without_cancelling() {
    use std::sync::Arc;
    use tokio::sync::Mutex;

    let mut app = crate::app::AppState::new();
    app.status = crate::app::AppStatus::Streaming;
    let state = Arc::new(Mutex::new(app));

    assert!(!handle_ctrl_c(&state).await);
    assert!(state.lock().await.ctrl_c_exit_deadline.is_some());
    assert_eq!(state.lock().await.status, crate::app::AppStatus::Streaming);

    assert!(handle_ctrl_c(&state).await);
}

#[tokio::test]
async fn expired_ctrl_c_requires_a_fresh_second_press() {
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use tokio::sync::Mutex;

    let mut app = crate::app::AppState::new();
    app.ctrl_c_exit_deadline = Some(Instant::now() - Duration::from_secs(1));
    let state = Arc::new(Mutex::new(app));

    assert!(!handle_ctrl_c(&state).await);
    assert!(state.lock().await.ctrl_c_exit_deadline.is_some());
}

#[test]
fn command_autocomplete_replaces_only_the_command_token() {
    let mut state = crate::app::AppState::new();
    state.input_buffer = "/mo --fast".to_owned();
    state.cursor_position = state.input_buffer.len();
    state.active_suggestion_index = Some(0);

    super::apply_autocomplete(&mut state);

    assert_eq!(state.input_buffer, "/model --fast");
    assert_eq!(state.active_suggestion_index, None);
}

#[tokio::test]
async fn enter_accepts_file_completion_without_submitting_the_prompt() {
    use std::sync::Arc;
    use tokio::sync::Mutex;
    use tokio_util::sync::CancellationToken;

    let state = Arc::new(Mutex::new(crate::app::AppState::new()));
    {
        let mut state = state.lock().await;
        state.input_buffer = "inspect @Cargo".to_owned();
        state.cursor_position = state.input_buffer.len();
        state.active_suggestion_index = Some(0);
    }
    let client = reqwest::Client::new();
    let mut cancel = CancellationToken::new();

    assert!(!super::handle_enter(&state, &client, &mut cancel).await);
    let state = state.lock().await;
    assert!(state.input_buffer.contains("Cargo"));
    assert!(state.input_buffer.ends_with(' '));
    assert!(state.history.is_empty());
}

#[test]
fn start_new_session_clears_history_and_starts_fresh() {
    let mut state = crate::app::AppState::new();
    let initial_session_id = state.active_session_id.clone();
    state
        .history
        .push(crate::app::ChatMessage::new("user", "hello old chat"));
    state.history.push(crate::app::ChatMessage::new(
        "assistant",
        "response old chat",
    ));

    super::start_new_session(&mut state);

    assert_ne!(state.active_session_id, initial_session_id);
    assert_eq!(state.history.len(), 1);
    assert_eq!(state.history[0].role, "system");
    assert_eq!(state.history[0].content, "✨ New chat started");
}

#[tokio::test]
async fn start_new_session_cancels_active_subagent_tasks() {
    let mut state = crate::app::AppState::new();
    let supervisor = state.subagent_supervisor.clone();
    let id = crate::app::SubagentId::from_raw(1);
    supervisor
        .spawn(
            id,
            tokio_util::sync::CancellationToken::new(),
            std::future::pending::<Result<String, String>>(),
        )
        .unwrap();

    super::start_new_session(&mut state);

    let completion = supervisor.wait(id).await.unwrap();
    assert_eq!(completion.status, crate::app::SubAgentStatus::Cancelled);
    assert!(state.subagents.is_empty());
}

#[test]
fn render_codex_rate_limit_windows() {
    let mut text = String::from("Session usage:");
    let limits = serde_json::json!({
        "primary": {"used_percent": 20.0, "window_minutes": 300, "resets_at": 1_700_000_000_i64},
        "secondary": {"used_percent": 50.0, "limit_window_seconds": 86400}
    });

    super::append_codex_rate_limits(&mut text, &limits);

    assert!(text.contains("ChatGPT primary (5h): 80.0% remaining"));
    assert!(text.contains("ChatGPT secondary (1d): 50.0% remaining"));
    assert!(text.contains("resets "));
}

#[test]
fn manual_compaction_discards_result_after_session_only_change() {
    let original = vec![
        crate::app::ChatMessage::new("user", "original task"),
        crate::app::ChatMessage::new("assistant", "original response"),
    ];
    let captured_history = original.clone();
    let mut live_history = original;
    let expected = live_history.clone();
    let compacted = vec![crate::app::ChatMessage::new(
        "system",
        "compacted old session",
    )];

    let applied = super::try_merge_compacted_history(
        "new-session",
        &mut live_history,
        "old-session",
        &captured_history,
        compacted,
    );

    assert!(!applied);
    assert!(live_history == expected);
}

#[test]
fn manual_compaction_discards_result_after_token_usage_change() {
    let original = vec![
        crate::app::ChatMessage::new("user", "original task"),
        crate::app::ChatMessage::new("assistant", "original response"),
    ];
    let captured_history = original.clone();
    let mut live_history = original;
    live_history[1].token_usage = Some(crate::app::TokenUsage {
        prompt_tokens: 12,
        completion_tokens: 8,
        total_tokens: 20,
        cached_tokens: Some(4),
    });
    let expected = live_history.clone();
    let compacted = vec![crate::app::ChatMessage::new("system", "compacted history")];

    let applied = super::try_merge_compacted_history(
        "active-session",
        &mut live_history,
        "active-session",
        &captured_history,
        compacted,
    );

    assert!(!applied);
    assert!(live_history == expected);
}

#[test]
fn manual_compaction_stale_report_skips_new_session_history() {
    let mut live_history = vec![crate::app::ChatMessage::new("user", "new session task")];
    let expected = live_history.clone();

    super::report_stale_compaction("new-session", "old-session", &mut live_history);

    assert!(live_history == expected);
}

#[test]
fn manual_compaction_stale_report_preserves_same_session_history() {
    let mut live_history = vec![crate::app::ChatMessage::new(
        "assistant",
        "response completed while compaction ran",
    )];
    live_history[0].response_time_ms = Some(250);
    let expected = live_history.clone();

    super::report_stale_compaction("active-session", "active-session", &mut live_history);

    assert!(live_history[..expected.len()] == expected);
    assert_eq!(live_history.len(), expected.len() + 1);
    assert!(
        live_history
            .last()
            .unwrap()
            .content
            .contains("discarded as stale")
    );
}

#[test]
fn manual_compaction_preserves_messages_appended_to_original_prefix() {
    let original = vec![
        crate::app::ChatMessage::new("user", "original task"),
        crate::app::ChatMessage::new("assistant", "original response"),
    ];
    let captured_history = original.clone();
    let mut live_history = original;
    let appended = crate::app::ChatMessage::new("user", "message appended during compaction");
    live_history.push(appended.clone());
    let compacted = vec![crate::app::ChatMessage::new("system", "compacted history")];
    let expected = vec![compacted[0].clone(), appended];

    let applied = super::try_merge_compacted_history(
        "active-session",
        &mut live_history,
        "active-session",
        &captured_history,
        compacted,
    );

    assert!(applied);
    assert!(live_history == expected);
}

#[tokio::test]
async fn manual_compaction_cancellation_interrupts_detached_request() {
    use crate::app::state::AppState;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::Mutex;
    use tokio_util::sync::CancellationToken;

    let (url, request_accepted) = pending_response_server().await;
    let mut app = AppState::new();
    app.api_base_url = url;
    app.model_name = "model".to_string();
    app.history = (0..(crate::network::compaction::KEEP_RECENT_TURNS + 2))
        .map(|index| crate::app::ChatMessage::new("user", format!("message {index}")))
        .collect();
    app.input_buffer = "/compact".to_string();
    let state = Arc::new(Mutex::new(app));
    let client = reqwest::Client::new();
    let mut cancel_token = CancellationToken::new();
    let compact_token = cancel_token.clone();

    assert!(!super::handle_enter(&state, &client, &mut cancel_token).await);
    tokio::time::timeout(Duration::from_secs(10), request_accepted)
        .await
        .expect("manual compaction request must start")
        .expect("manual compaction server must signal acceptance");

    {
        let mut app = state.lock().await;
        app.input_buffer = "/cancel".to_string();
    }
    assert!(!super::handle_enter(&state, &client, &mut cancel_token).await);
    assert!(compact_token.is_cancelled());
    assert!(!cancel_token.is_cancelled());

    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if state
                .lock()
                .await
                .history
                .iter()
                .any(|message| message.content.starts_with("History compaction failed:"))
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("cancelling the active token must stop manual compaction");
}

#[tokio::test]
async fn test_goal_command_flow() {
    use crate::app::state::AppState;
    use std::sync::Arc;
    use tokio::sync::Mutex;
    use tokio_util::sync::CancellationToken;

    let state = Arc::new(Mutex::new(AppState::new()));
    let client = reqwest::Client::new();
    let mut cancel_token = CancellationToken::new();

    // Empty goal
    {
        let mut s = state.lock().await;
        s.input_buffer = "/goal ".to_string();
    }
    let trigger = super::handle_enter(&state, &client, &mut cancel_token).await;
    assert!(!trigger);
    {
        let s = state.lock().await;
        assert!(!s.continuous_mode);
        assert!(s.history.last().unwrap().content.contains("Usage:"));
    }

    // Valid goal
    {
        let mut s = state.lock().await;
        s.input_buffer = "/goal fix build issues".to_string();
        s.history.clear();
    }
    let trigger2 = super::handle_enter(&state, &client, &mut cancel_token).await;
    assert!(trigger2);
    {
        let s = state.lock().await;
        assert!(s.continuous_mode);
        assert!(
            s.history
                .last()
                .unwrap()
                .content
                .contains("Goal: fix build issues")
        );
        assert!(
            s.history
                .last()
                .unwrap()
                .content
                .contains("Continuous autoloop mode is active")
        );
        assert!(s.input_buffer.is_empty());
    }
}

#[tokio::test]
async fn clear_and_new_preserve_history() {
    use crate::app::ChatMessage;
    use std::sync::Arc;
    use tokio::sync::Mutex;
    use tokio_util::sync::CancellationToken;

    let state = Arc::new(Mutex::new(crate::app::state::AppState::new()));
    let client = reqwest::Client::new();
    let mut cancel_token = CancellationToken::new();
    let original_history = vec![
        ChatMessage::new("user", "original prompt"),
        ChatMessage::new("assistant", "original answer"),
    ];

    {
        let mut s = state.lock().await;
        s.history.replace(original_history.clone());
        s.input_buffer = "/clear".to_string();
    }
    assert!(!super::handle_enter(&state, &client, &mut cancel_token).await);
    {
        let s = state.lock().await;
        assert!(s.history.as_slice() == original_history);
        assert_eq!(s.history_display_start, s.history.len());
    }

    {
        let mut s = state.lock().await;
        s.input_buffer = "/new".to_string();
    }
    assert!(!super::handle_enter(&state, &client, &mut cancel_token).await);
    {
        let s = state.lock().await;
        assert_eq!(s.history.len(), 1);
        assert_eq!(s.history[0].role, "system");
        assert!(s.history[0].content.contains("New chat"));
        assert_eq!(s.history_display_start, 0);
    }
}

#[tokio::test]
async fn verbosity_command_opens_picker_on_the_active_value() {
    use crate::app::{AppStatus, Verbosity};
    use std::sync::Arc;
    use tokio::sync::Mutex;
    use tokio_util::sync::CancellationToken;

    let state = Arc::new(Mutex::new(crate::app::AppState::new()));
    let client = reqwest::Client::new();
    let mut cancel_token = CancellationToken::new();

    {
        let mut s = state.lock().await;
        s.verbosity = Verbosity::High;
        s.input_buffer = "/verbosity".to_owned();
    }

    assert!(!super::handle_enter(&state, &client, &mut cancel_token).await);

    let s = state.lock().await;
    assert_eq!(s.status, AppStatus::VerbosityPicker);
    assert_eq!(s.modal_picker_index, 1);
}

#[tokio::test]
async fn yolo_command_opens_picker_and_accepts_arguments() {
    use crate::app::AppStatus;
    use std::sync::Arc;
    use tokio::sync::Mutex;
    use tokio_util::sync::CancellationToken;

    let state = Arc::new(Mutex::new(crate::app::AppState::new()));
    let client = reqwest::Client::new();
    let mut cancel_token = CancellationToken::new();

    state.lock().await.input_buffer = "/yolo".to_owned();
    assert!(!super::handle_enter(&state, &client, &mut cancel_token).await);
    {
        let s = state.lock().await;
        assert_eq!(s.status, AppStatus::YoloPicker);
        assert_eq!(s.modal_picker_index, 1);
    }

    state.lock().await.input_buffer = "/yolo on".to_owned();
    assert!(!super::handle_enter(&state, &client, &mut cancel_token).await);
    {
        let s = state.lock().await;
        assert!(s.auto_confirm);
        assert_eq!(s.history.last().unwrap().content, "YOLO mode enabled");
    }

    state.lock().await.input_buffer = "/yolo off".to_owned();
    assert!(!super::handle_enter(&state, &client, &mut cancel_token).await);
    {
        let s = state.lock().await;
        assert!(!s.auto_confirm);
        assert_eq!(s.history.last().unwrap().content, "YOLO mode disabled");
    }

    state.lock().await.input_buffer = "/yolo toggle".to_owned();
    assert!(!super::handle_enter(&state, &client, &mut cancel_token).await);
    {
        let s = state.lock().await;
        assert!(s.auto_confirm);
        assert_eq!(s.history.last().unwrap().content, "YOLO mode enabled");
    }
}

#[tokio::test]
async fn test_theme_command_flow() {
    use crate::app::state::AppState;
    use std::sync::Arc;
    use tokio::sync::Mutex;
    use tokio_util::sync::CancellationToken;

    let state = Arc::new(Mutex::new(AppState::new()));
    let client = reqwest::Client::new();
    let mut cancel_token = CancellationToken::new();

    let initial_theme = {
        let s = state.lock().await;
        s.config.theme.clone()
    };

    // Open theme picker modal via /theme
    {
        let mut s = state.lock().await;
        s.input_buffer = "/theme".to_string();
    }
    let trigger = super::handle_enter(&state, &client, &mut cancel_token).await;
    assert!(!trigger);
    {
        let s = state.lock().await;
        assert!(s.show_theme_picker);
    }

    // Switch to theme 'nord'
    {
        let mut s = state.lock().await;
        s.input_buffer = "/theme nord".to_string();
    }
    let trigger2 = super::handle_enter(&state, &client, &mut cancel_token).await;
    assert!(!trigger2);
    {
        let s = state.lock().await;
        assert_eq!(s.config.theme, "nord");
        assert_eq!(s.history.last().unwrap().role, "system");
        assert!(s.history.last().unwrap().content.contains("nord"));
    }

    // Switch to unknown theme
    {
        let mut s = state.lock().await;
        s.input_buffer = "/theme unknown_theme".to_string();
    }
    let trigger3 = super::handle_enter(&state, &client, &mut cancel_token).await;
    assert!(!trigger3);
    {
        let s = state.lock().await;
        assert_eq!(s.config.theme, "nord");
        assert!(
            s.history
                .last()
                .unwrap()
                .content
                .contains("Unknown theme 'unknown_theme'")
        );
    }

    // Restore initial theme so test execution does not leak theme changes into user config file
    {
        let mut s = state.lock().await;
        s.input_buffer = format!("/theme {}", initial_theme);
    }
    let _ = super::handle_enter(&state, &client, &mut cancel_token).await;
}

#[tokio::test]
async fn context_command_opens_modal_and_sets_window() {
    use crate::app::state::AppState;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    let state = Arc::new(Mutex::new(AppState::new()));
    let client = reqwest::Client::new();
    let mut cancel_token = tokio_util::sync::CancellationToken::new();

    // 1. /context with no args opens show_context_modal
    {
        let mut s = state.lock().await;
        s.input_buffer = "/context".to_string();
    }
    let trigger = super::handle_enter(&state, &client, &mut cancel_token).await;
    assert!(!trigger);
    {
        let s = state.lock().await;
        assert!(s.show_context_modal);
        assert!(s.modal_open());
    }

    // 2. /context with token count sets context window
    {
        let mut s = state.lock().await;
        s.show_context_modal = false;
        s.input_buffer = "/context 256k".to_string();
    }
    let trigger2 = super::handle_enter(&state, &client, &mut cancel_token).await;
    assert!(!trigger2);
    {
        let s = state.lock().await;
        assert!(!s.show_context_modal);
        let default_name = s.config.default.big().to_string();
        let profile = s.config.models.iter().find(|m| m.name == default_name);
        assert_eq!(profile.and_then(|p| p.context_window), Some(262144));
    }
}

#[tokio::test]
async fn update_command_initiates_check_and_sets_notice() {
    use crate::app::state::AppState;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    let state = Arc::new(Mutex::new(AppState::new()));
    let client = reqwest::Client::new();
    let mut cancel_token = tokio_util::sync::CancellationToken::new();

    {
        let mut s = state.lock().await;
        s.input_buffer = "/update".to_string();
    }
    let trigger = super::handle_enter(&state, &client, &mut cancel_token).await;
    assert!(!trigger);
    {
        let s = state.lock().await;
        assert!(s.input_buffer.is_empty());
        assert_eq!(s.cursor_position, 0);
        assert_eq!(s.update_check, crate::update::UpdateState::Checking);
        assert!(
            s.history
                .last()
                .unwrap()
                .content
                .contains("Checking for a RustCode update")
        );
    }
}
