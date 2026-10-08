use super::AppState;

#[test]
fn identical_live_calls_have_independent_execution_identity() {
    let mut state = AppState::new();
    let arguments = serde_json::json!({"path": "src/lib.rs"});
    let first = state.begin_live_tool_call(None, "view_file", &arguments);
    let second = state.begin_live_tool_call(None, "view_file", &arguments);

    assert_ne!(first, second);
    assert_eq!(state.live_tool_calls.len(), 2);

    state.finish_live_tool_call(&first, true);
    assert_eq!(state.live_tool_calls.len(), 1);
    assert_eq!(state.live_tool_calls[0].key, second);

    state.finish_live_tool_call(&second, true);
    assert!(state.live_tool_calls.is_empty());
}

#[test]
fn live_command_output_is_bounded_and_presentation_only() {
    let mut state = AppState::new();
    let key = state.begin_live_tool_call(
        Some("native-command"),
        "run_command",
        &serde_json::json!({"command": "cargo test"}),
    );
    state.append_live_tool_output(&key, &vec![b'x'; 40 * 1024], false);
    state.append_live_tool_output(&key, b"compiler error\n", true);

    let call = &state.live_tool_calls[0];
    assert!(call.omitted_output_bytes > 0);
    assert!(call.output.iter().any(|chunk| chunk.stderr));
    assert!(state.history.is_empty());
}

#[test]
fn provider_id_is_retained_without_becoming_the_only_key_component() {
    let mut state = AppState::new();
    let arguments = serde_json::json!({"command": "cargo check"});
    let first = state.begin_live_tool_call(Some("provider-call-7"), "run_command", &arguments);
    let second = state.begin_live_tool_call(Some("provider-call-7"), "run_command", &arguments);

    assert_ne!(first, second);
    assert!(first.contains("provider-call-7"));
    assert_eq!(
        state.live_tool_calls[0].provider_call_id.as_deref(),
        Some("provider-call-7")
    );
}

#[test]
fn speculative_tool_call_updates_target_as_arguments_stream() {
    let mut state = AppState::new();

    // 1. Initial stream chunk with only tool name
    state.update_speculative_live_tool_call(
        Some("call-99"),
        "replace_file_content",
        &serde_json::json!({}),
    );
    assert_eq!(state.live_tool_calls.len(), 1);
    assert_eq!(state.live_tool_calls[0].action, "Edit");
    assert_eq!(state.live_tool_calls[0].target, "?");

    // 2. Mid-stream chunk with TargetFile parsed
    state.update_speculative_live_tool_call(
        Some("call-99"),
        "replace_file_content",
        &serde_json::json!({"TargetFile": "src/symbols.rs"}),
    );
    assert_eq!(state.live_tool_calls.len(), 1);
    assert_eq!(state.live_tool_calls[0].target, "src/symbols.rs");

    // 3. Clear live tool calls at end of turn cleans speculative projections
    state.clear_live_tool_calls();
    assert!(state.live_tool_calls.is_empty());
}

#[test]
fn speculative_providerless_calls_share_one_projection_per_turn() {
    let mut state = AppState::new();

    state.update_speculative_native_tool_call("view_file", &serde_json::json!({}));
    let key = state.live_tool_calls[0].key.clone();
    state.update_speculative_native_tool_call(
        "write_to_file",
        &serde_json::json!({"TargetFile": "src/main.rs"}),
    );

    assert_eq!(state.live_tool_calls.len(), 1);
    assert_eq!(state.live_tool_calls[0].key, key);
    assert_eq!(state.live_tool_calls[0].tool_name, "write_to_file");
    assert_eq!(state.live_tool_calls[0].target, "src/main.rs");

    let execution_key = state.begin_live_tool_call(None, "write_to_file", &serde_json::json!({}));
    assert_eq!(execution_key, key);
    assert!(state.live_tool_calls[0].execution_started);
}

#[test]
fn speculative_structured_calls_keep_distinct_provider_ids() {
    let mut state = AppState::new();

    state.update_speculative_live_tool_call(Some("call-1"), "view_file", &serde_json::json!({}));
    state.update_speculative_live_tool_call(
        Some("call-2"),
        "write_to_file",
        &serde_json::json!({}),
    );

    assert_eq!(state.live_tool_calls.len(), 2);
    assert_eq!(
        state
            .live_tool_calls
            .iter()
            .filter_map(|call| call.provider_call_id.as_deref())
            .collect::<Vec<_>>(),
        vec!["call-1", "call-2"]
    );
}

#[test]
fn execution_adopts_the_speculative_live_tool_projection() {
    let mut state = AppState::new();
    state.update_speculative_live_tool_call(Some("call-99"), "get_time", &serde_json::json!({}));
    let speculative_key = state.live_tool_calls[0].key.clone();

    let execution_key =
        state.begin_live_tool_call(Some("call-99"), "get_time", &serde_json::json!({}));

    assert_eq!(execution_key, speculative_key);
    assert_eq!(state.live_tool_calls.len(), 1);
    assert!(state.live_tool_calls[0].execution_started);
}

#[test]
fn cleanup_removes_all_live_calls_without_touching_history() {
    let mut state = AppState::new();
    state
        .history
        .push(super::ChatMessage::new("user", "keep me"));
    let history = state.history.clone();
    let arguments = serde_json::json!({});
    state.begin_live_tool_call(None, "grep", &arguments);
    state.begin_live_tool_call(None, "grep", &arguments);

    state.clear_live_tool_calls();

    assert!(state.live_tool_calls.is_empty());
    assert!(state.history == history);
}

#[test]
fn active_turn_projection_cleanup_removes_stale_cancelled_state() {
    let mut state = AppState::new();
    state.replace_current_response("stale response");
    state.begin_live_tool_call(None, "grep", &serde_json::json!({"pattern": "old"}));
    state.running_tools.push("grep".to_owned());
    state.generation_start_time = Some(std::time::Instant::now());
    state.stream_tracker = Some(super::super::StreamTracker::new());
    state
        .history
        .push(super::ChatMessage::new("user", "keep me"));

    state.clear_active_turn_projection();

    assert!(state.current_response.is_empty());
    assert!(state.live_tool_calls.is_empty());
    assert!(state.running_tools.is_empty());
    assert!(state.stream_tracker.is_none());
    assert!(state.generation_start_time.is_none());
    assert_eq!(state.history.len(), 1);
}

#[test]
fn tool_confirmation_selection_moves_between_approve_and_deny() {
    let mut state = AppState::new();
    assert_eq!(state.tool_confirmation_selected, 0);

    state.move_tool_confirmation_selection(1);
    assert_eq!(state.tool_confirmation_selected, 1);
    state.move_tool_confirmation_selection(-1);
    assert_eq!(state.tool_confirmation_selected, 0);
}

#[test]
fn tool_confirmation_selection_reaches_the_prefix_choices_and_clamps() {
    let mut state = AppState::new();
    state.pending_tool_confirmation = Some(vec![crate::app::ToolConfirmation {
        request_id: None,
        tool_name: "run_command".to_owned(),
        path: "cargo test --lib".to_owned(),
        content_preview: String::new(),
        content_bytes: 0,
        rememberable_prefix: Some("cargo test".to_owned()),
        forbidden_prefix: Some("cargo test".to_owned()),
    }]);

    state.move_tool_confirmation_selection(1);
    assert_eq!(state.tool_confirmation_selected, 1);
    state.move_tool_confirmation_selection(1);
    assert_eq!(state.tool_confirmation_selected, 2);
    state.move_tool_confirmation_selection(1);
    assert_eq!(state.tool_confirmation_selected, 3);
    // Clamped at the last prefix choice.
    state.move_tool_confirmation_selection(1);
    assert_eq!(state.tool_confirmation_selected, 3);
    state.move_tool_confirmation_selection(-1);
    assert_eq!(state.tool_confirmation_selected, 2);
}

#[test]
fn deferred_speculative_projections_are_dropped_while_running_calls_remain() {
    let mut state = AppState::new();
    state.update_speculative_live_tool_call(
        Some("call-a"),
        "run_command",
        &serde_json::json!({"command": "cargo test"}),
    );
    state.update_speculative_live_tool_call(
        Some("call-b"),
        "run_command",
        &serde_json::json!({"command": "cargo lint"}),
    );
    // Adopt one call; the other represents a scheduler-deferred projection.
    state.begin_live_tool_call(
        Some("call-a"),
        "run_command",
        &serde_json::json!({"command": "cargo test"}),
    );
    assert_eq!(state.live_tool_calls.len(), 2);
    state.clear_speculative_live_tool_calls();
    assert_eq!(state.live_tool_calls.len(), 1);
    assert!(state.live_tool_calls[0].execution_started);
    assert_eq!(
        state.live_tool_calls[0].provider_call_id.as_deref(),
        Some("call-a")
    );
}

#[test]
fn live_command_keeps_supplied_cwd_when_adopting_speculative_call() {
    let mut state = AppState::new();
    state.update_speculative_live_tool_call(
        Some("build"),
        "run_command",
        &serde_json::json!({"command": "cargo build"}),
    );
    state.begin_live_tool_call(
        Some("build"),
        "run_command",
        &serde_json::json!({"command": "cargo build", "cwd": "/tmp/project"}),
    );
    assert_eq!(state.live_tool_calls.len(), 1);
    assert_eq!(
        state.live_tool_calls[0].cwd.as_deref(),
        Some("/tmp/project")
    );
    assert!(
        state.history.is_empty(),
        "presentation metadata stays outside canonical history"
    );
}

#[test]
fn render_projection_marks_withheld_background_outcomes_until_consumed() {
    let mut state = AppState::new();
    state
        .pending_background_outputs
        .push(crate::PendingBackgroundOutput {
            task_id: "build".into(),
            output: crate::tools::ToolExecutionOutput::success("completed".into()),
        });
    let mut cancelled = crate::tools::ToolExecutionOutput::failure("cancelled".into());
    cancelled.error_kind = Some(rustcode_core::ToolErrorKind::Cancelled);
    state
        .pending_background_outputs
        .push(crate::PendingBackgroundOutput {
            task_id: "child".into(),
            output: cancelled,
        });
    let view = crate::controller::render_state(&state);
    assert_eq!(view.pending_background_results.len(), 2);
    assert!(view.pending_background_results[0].success);
    assert!(view.pending_background_results[1].cancelled);
    assert!(state.history.is_empty());
    state.pending_background_outputs.clear();
    assert!(
        crate::controller::render_state(&state)
            .pending_background_results
            .is_empty()
    );
}

#[test]
fn finished_calls_stay_listed_until_their_batch_is_recorded() {
    let mut state = AppState::new();
    state.retain_finished_live_tool_calls();
    let first = state.begin_live_tool_call(None, "view_file", &serde_json::json!({"path":"a"}));
    let second = state.begin_live_tool_call(None, "view_file", &serde_json::json!({"path":"b"}));

    state.finish_live_tool_call(&first, false);
    assert_eq!(state.live_tool_calls.len(), 2);
    assert_eq!(
        state.live_tool_calls[0]
            .finished
            .map(|finish| finish.success),
        Some(false)
    );
    assert!(state.live_tool_calls[1].finished.is_none());

    state.finish_live_tool_call(&second, true);
    state.clear_finished_live_tool_calls();
    assert!(state.live_tool_calls.is_empty());

    // Outside a batch a finished call is dropped at once.
    let lone = state.begin_live_tool_call(None, "view_file", &serde_json::json!({"path":"c"}));
    state.finish_live_tool_call(&lone, true);
    assert!(state.live_tool_calls.is_empty());
}
