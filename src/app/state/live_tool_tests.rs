use super::AppState;

#[test]
fn identical_live_calls_have_independent_execution_identity() {
    let mut state = AppState::new();
    let arguments = serde_json::json!({"path": "src/lib.rs"});
    let first = state.begin_live_tool_call(None, "view_file", &arguments);
    let second = state.begin_live_tool_call(None, "view_file", &arguments);

    assert_ne!(first, second);
    assert_eq!(state.live_tool_calls.len(), 2);

    state.finish_live_tool_call(&first);
    assert_eq!(state.live_tool_calls.len(), 1);
    assert_eq!(state.live_tool_calls[0].key, second);

    state.finish_live_tool_call(&second);
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
fn tool_confirmation_selection_moves_between_approve_and_deny() {
    let mut state = AppState::new();
    assert_eq!(state.tool_confirmation_selected, 0);

    state.move_tool_confirmation_selection(1);
    assert_eq!(state.tool_confirmation_selected, 1);
    state.move_tool_confirmation_selection(-1);
    assert_eq!(state.tool_confirmation_selected, 0);
}
