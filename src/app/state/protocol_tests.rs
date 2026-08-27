use super::{AppState, PendingQuestion};
use crate::config::ToolProtocol;

#[test]
fn notices_are_appended_to_history() {
    let mut state = AppState::new();
    state.set_warning_notice("Custom warning");
    state.set_notice("Execution error occurred");

    let notices = &state.history[state.history.len() - 2..];
    assert_eq!(notices[0].role, "system");
    assert_eq!(notices[0].content, "Custom warning");
    assert_eq!(notices[1].role, "system");
    assert_eq!(notices[1].content, "Execution error occurred");
    assert!(state.redraw_requested);
}

#[test]
fn expired_ctrl_c_arming_is_cleared_and_requests_a_redraw() {
    let mut state = AppState::new();
    state.ctrl_c_exit_deadline =
        Some(std::time::Instant::now() - std::time::Duration::from_secs(1));
    state.redraw_requested = false;
    let revision = state.render_revision;

    assert!(state.expire_ctrl_c_exit_arming(std::time::Instant::now()));
    assert!(state.ctrl_c_exit_deadline.is_none());
    assert!(state.redraw_requested);
    assert_ne!(state.render_revision, revision);
    assert!(!state.expire_ctrl_c_exit_arming(std::time::Instant::now()));
}

#[test]
fn clearing_ctrl_c_arming_requests_a_redraw_only_when_armed() {
    let mut state = AppState::new();
    state.redraw_requested = false;
    state.clear_ctrl_c_exit_arming();
    assert!(!state.redraw_requested);

    state.ctrl_c_exit_deadline =
        Some(std::time::Instant::now() + std::time::Duration::from_secs(1));
    state.clear_ctrl_c_exit_arming();
    assert!(state.ctrl_c_exit_deadline.is_none());
    assert!(state.redraw_requested);
}

#[test]
fn pending_question_starts_with_empty_custom_input() {
    let question = PendingQuestion::new(
        "How should we proceed?".to_string(),
        vec!["Continue".to_string(), "Stop".to_string()],
        false,
    );

    assert_eq!(question.selected, 0);
    assert_eq!(question.chosen, vec![false, false]);
    assert_eq!(question.custom_input, None);
    assert_eq!(question.custom_cursor, 0);
}

#[test]
fn pending_question_editing_preserves_utf8_boundaries() {
    let mut question = PendingQuestion::new("Q".to_string(), vec![], false);
    question.activate_custom_input();
    question.insert_str("héllo");
    question.move_cursor_home();
    question.move_cursor_right();
    question.delete_char_after();

    assert_eq!(question.custom_input.as_deref(), Some("hllo"));
    assert_eq!(question.custom_cursor, "h".len());
}

#[test]
fn pending_question_word_navigation_and_deletion_use_cursor_position() {
    let mut question = PendingQuestion::new("Q".to_string(), vec![], false);
    question.activate_custom_input();
    question.insert_str("one two three");
    question.move_cursor_word_left();
    assert_eq!(question.custom_cursor, "one two ".len());

    question.delete_word_before();
    assert_eq!(question.custom_input.as_deref(), Some("one three"));
    assert_eq!(question.custom_cursor, "one ".len());

    question.move_cursor_word_right();
    assert_eq!(question.custom_cursor, "one three".len());
}

#[test]
fn pending_question_paste_ignores_line_breaks() {
    let mut question = PendingQuestion::new("Q".to_string(), vec![], false);
    question.activate_custom_input();
    question.insert_str("first\r\nsecond\nthird");

    assert_eq!(question.custom_input.as_deref(), Some("firstsecondthird"));
    assert_eq!(question.custom_cursor, "firstsecondthird".len());
}

#[test]
fn pending_question_word_navigation_preserves_multibyte_boundaries() {
    let mut question = PendingQuestion::new("Q".to_string(), vec![], false);
    question.activate_custom_input();
    question.insert_str("one  😀");
    question.move_cursor_home();
    question.move_cursor_word_right();

    assert_eq!(question.custom_cursor, "one  ".len());
    assert!(
        question
            .custom_input
            .as_ref()
            .is_some_and(|text| text.is_char_boundary(question.custom_cursor))
    );

    question.move_cursor_word_right();
    assert_eq!(question.custom_cursor, "one  😀".len());
}

#[test]
fn known_providers_get_structured_calls_and_local_servers_keep_text() {
    let mut s = AppState::new();
    s.config.tool_protocol = ToolProtocol::Json;

    assert_eq!(
        s.tool_protocol_for(
            "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions"
        ),
        ToolProtocol::ApiNative
    );
    assert_eq!(
        s.tool_protocol_for("https://api.openai.com/v1/chat/completions"),
        ToolProtocol::ApiNative
    );
    // A local server may not implement function calling, so it keeps the
    // configured text protocol.
    assert_eq!(
        s.tool_protocol_for("http://localhost:11434/v1/chat/completions"),
        ToolProtocol::Json
    );
}

#[test]
fn probe_result_decides_for_gateways_and_is_remembered() {
    let mut s = AppState::new();
    s.config.tool_protocol = ToolProtocol::Json;
    let gateway = "http://localhost:3000/v1/chat/completions";

    // A gateway says nothing by its hostname, so it must be probed.
    assert!(s.function_calling_unknown(gateway));
    assert_eq!(s.tool_protocol_for(gateway), ToolProtocol::Json);

    s.record_function_calling_support(gateway, true);
    assert!(!s.function_calling_unknown(gateway));
    assert_eq!(s.tool_protocol_for(gateway), ToolProtocol::ApiNative);
}

#[test]
fn rejected_probe_falls_back_from_api_native_to_json() {
    let mut s = AppState::new();
    s.config.tool_protocol = ToolProtocol::ApiNative;
    let gateway = "http://localhost:3000/v1/chat/completions";

    s.record_function_calling_support(gateway, false);

    assert_eq!(s.tool_protocol_for(gateway), ToolProtocol::Json);
}

#[test]
fn known_hosts_skip_the_probe() {
    let s = AppState::new();
    assert!(!s.function_calling_unknown("https://api.openai.com/v1/chat/completions"));
}

#[test]
fn a_profile_override_beats_detection() {
    let mut s = AppState::new();
    s.config.models.push(crate::config::ModelProfile {
        name: "local-caller".to_string(),
        url: "http://localhost:1234/v1/chat/completions".to_string(),
        model: "qwen".to_string(),
        context_window: None,
        engine: None,
        api_key: None,
        env_key: None,
        tool_protocol: Some(ToolProtocol::ApiNative),
        enable_thinking: None,
        reasoning_effort: None,
        max_tokens: None,
        supports_vision: None,
        ..Default::default()
    });

    assert_eq!(
        s.tool_protocol_for("http://localhost:1234/v1/chat/completions"),
        ToolProtocol::ApiNative
    );
}
