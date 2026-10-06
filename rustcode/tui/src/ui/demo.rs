//! Fixed render data for the `/test` visual demo.
//!
//! The returned view belongs only to the TUI presentation. It is never
//! installed in the engine session or sent to the provider.

use rustcode::controller::{
    AppStatus, ChatMessage, LiveToolCall, PendingQuestion, RenderState, ToolCallRef,
    ToolResultRecord, Verbosity,
};
use std::sync::Arc;

pub(crate) struct DemoState {
    view: RenderState,
    question_preview: bool,
}

impl DemoState {
    pub(crate) fn render_state(&self) -> &RenderState {
        &self.view
    }

    pub(crate) fn freeze(&mut self) {
        self.view.config.reduced_motion = true;
        // Future timestamps make elapsed output clamp at zero.
        let started_at = std::time::Instant::now() + std::time::Duration::from_secs(86_400);
        for call in Arc::make_mut(&mut self.view.live_tool_calls) {
            call.started_at = started_at;
        }
    }

    pub(crate) fn toggle_question_preview(&mut self) {
        self.question_preview = !self.question_preview;
        if self.question_preview {
            self.view.status = AppStatus::AwaitingQuestion;
            self.view.set_question_chain(vec![
                PendingQuestion::new(
                    "Should the report include dependency warnings?".to_owned(),
                    vec!["Yes, include them".to_owned(), "No, errors only".to_owned()],
                    false,
                )
                .with_header("Report options".to_owned()),
                PendingQuestion::new(
                    "Would you like a file-by-file summary?".to_owned(),
                    vec!["Yes".to_owned(), "No".to_owned()],
                    false,
                ),
            ]);
        } else {
            self.view.status = AppStatus::Streaming;
            self.view.pending_question = None;
            self.view.pending_question_chain_len = 0;
            self.view.pending_question_chain_position = 0;
            self.view.pending_question_chain_answered = 0;
        }
    }
}

pub(crate) fn demo_state(mut view: RenderState) -> DemoState {
    // Start from a read-only projection, then discard every field that could
    // disclose or act on the real session. AppState::new() is intentionally
    // not used here because it creates and records a session.
    view.revision = 0;
    view.status = AppStatus::Streaming;
    view.input_buffer.clear();
    view.cursor_position = 0;
    view.composer_selection_anchor = None;
    view.ctrl_c_exit_armed = false;
    view.steering_interruptible = false;
    view.steering_escape_will_interrupt = false;
    view.active_suggestion_index = None;
    view.dismissed_completion = None;
    view.command_suggestion = None;
    view.history = rustcode::controller::History::default();
    view.history_display_start = 0;
    view.current_response = Arc::new(String::new());
    view.recap_loading = false;
    view.current_token_usage = None;
    view.current_turn_token_usage = None;
    view.current_round_token_usage = None;
    view.current_round_estimated_input_tokens = 0;
    view.current_round_estimated_output_tokens = 0;
    view.current_provider_request_prompt_estimate = 0;
    view.current_turn_token_usage_is_estimated = false;
    view.token_usage_in_flight = false;
    view.provider_request_in_flight = false;
    view.response_time = None;
    view.current_thought_time_ms = 0;
    view.current_thought_tokens = 0;
    view.current_thought_started_at = None;
    view.model_quota_remaining = None;
    view.provider_rate_limits = None;
    view.generation_start_time = None;
    view.pending_queue.clear();
    view.pending_steers.clear();
    view.pending_tool_confirmation = None;
    view.pending_question = None;
    view.pending_question_chain_len = 0;
    view.pending_question_chain_position = 0;
    view.pending_question_chain_answered = 0;
    view.running_tools.clear();
    view.live_tool_calls = Arc::new(Vec::new());
    view.stream_tracker = None;
    view.expanded_thoughts.clear();
    view.last_copy_text = None;
    view.transient_notice = None;
    view.delegation_active = false;
    view.auto_confirm = false;
    view.config = rustcode::controller::AppConfig::default();
    view.verbosity = Verbosity::Low;
    view.model_name = "TUI demo".to_owned();
    view.api_base_url.clear();
    view.active_session_id = "static-visual-demo".to_owned();
    view.cwd_and_branch = "demo workspace".to_owned();
    view.home_path = None;
    view.active_context_window = 0;
    view.active_model_profile = None;
    view.active_tool_protocol = rustcode::controller::ToolProtocol::default();
    view.background_tasks.clear();
    view.pending_background_results.clear();
    view.waiting_for_background_terminal = false;
    view.subagents.clear();
    view.selected_subagent = None;
    view.selected_subagent_id = None;
    view.show_model_picker = false;
    view.model_picker_search.clear();
    view.show_theme_picker = false;
    view.theme_picker_initial.clear();
    view.show_command_picker = false;
    view.command_picker_search.clear();
    view.show_history_picker = false;
    view.history_picker_sessions.clear();
    view.history_picker_truncated = false;
    view.pending_delete_session_idx = None;
    view.show_subagent_picker = false;
    view.settings_picker = None;
    view.command_panel = None;
    view.show_context_modal = false;
    view.show_status_modal = false;
    view.show_stats_modal = false;
    view.show_session_modal = false;
    view.stats_usage_history.clear();
    view.show_update_prompt = false;
    view.update_check = rustcode_core::update::UpdateState::Unknown;
    view.show_mcp_config = false;
    view.mcp_edit_state = None;
    view.settings_picker = None;
    view.selected_subagent = None;

    let long_warning = format!(
        "warning: dependency graph contains an advisory that should be reviewed before release; {}",
        "the report keeps this detail available in the expanded tool output. ".repeat(2)
    );
    view.history.push(ChatMessage::new(
        "user",
        "Summarize the workspace checks and point out anything that needs attention.",
    ));
    view.history.push(
        ChatMessage::new("assistant", "").with_tool_calls(vec![ToolCallRef {
            id: "demo-call-completed".to_owned(),
            name: "run_command".to_owned(),
            arguments: r#"{"command":"cargo check --workspace"}"#.to_owned(),
        }]),
    );
    let tool_output = [
        "Checking packages: rustcode-engine 0.42s, rustcode-tui 0.18s, rustcode-cli 0.11s",
        "Finished with 0 errors and 2 warnings",
        &long_warning,
    ]
    .join("\n");
    view.history.push(
        ChatMessage::new(
            "tool",
            format!("run_command: exit code: 0\nstdout:\n{tool_output}"),
        )
        .answering(Some("demo-call-completed".to_owned()))
        .with_tool_result(ToolResultRecord {
            tool_name: "run_command".to_owned(),
            success: true,
            command: Some("cargo check --workspace".to_owned()),
            exit_code: Some(0),
            ..Default::default()
        }),
    );
    view.expanded_thoughts.insert(2);
    view.history.push(ChatMessage::new(
        "assistant",
        "Static `/test` preview · Tab: question preview · Esc: close\n\n| File | Result |\n| --- | --- |\n| Cargo.toml | 3 packages checked |\n| src/main.rs | no changes needed |",
    ));
    view.pending_queue = vec!["review remaining warnings".to_owned()];
    let running = LiveToolCall::new(
        "demo-call-running",
        None,
        "run_command",
        "Bash",
        "cargo test --workspace",
    );
    let mut queued = LiveToolCall::new(
        "demo-call-queued",
        None,
        "mcp__mail__search",
        "mail.Search",
        "release notes",
    );
    queued.execution_started = false;
    view.live_tool_calls = Arc::new(vec![running, queued]);
    DemoState {
        view,
        question_preview: false,
    }
}

#[cfg(test)]
mod tests {
    use super::{DemoState, demo_state};
    use crate::{
        inline_terminal::InlineTerminal as Terminal,
        ui::{TranscriptState, render_snapshot::render_snapshot, render_with_transcript_snapshot},
    };
    use ratatui::backend::TestBackend;
    use rustcode::controller::{AppStatus, RenderState};

    fn demo_view() -> DemoState {
        demo_state(RenderState::new())
    }

    #[test]
    fn demo_fixture_is_static_and_contains_all_preview_states() {
        let demo = demo_view();
        let view = demo.render_state();
        assert_eq!(view.active_session_id, "static-visual-demo");
        assert_eq!(view.status, AppStatus::Streaming);
        assert_eq!(view.pending_queue, ["review remaining warnings"]);
        assert_eq!(view.live_tool_calls.len(), 2);
        assert_eq!(view.history.len(), 4);
        assert!(view.pending_question.is_none());
        assert!(
            view.history
                .iter()
                .any(|message| message.content.contains("| Cargo.toml |"))
        );
        assert!(
            view.history
                .iter()
                .any(|message| message.content.contains("Finished with 0 errors"))
        );
    }

    #[test]
    fn demo_uses_existing_renderer_for_each_preview_state_and_is_stable() {
        let mut demo = demo_view();
        demo.freeze();
        let snapshot = render_snapshot(demo.render_state());
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let render = |frame: &mut crate::inline_terminal::Frame<'_>| {
            let mut transcript = TranscriptState::default();
            let _ = render_with_transcript_snapshot(frame, &snapshot, &mut transcript);
        };
        terminal.draw(render).expect("render demo");
        let text = |terminal: &Terminal<TestBackend>| {
            (0..40)
                .map(|row| {
                    (0..120)
                        .map(|column| terminal.backend().buffer()[(column, row)].symbol())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        let first = text(&terminal);

        for expected in [
            "Static /test preview",
            "Ran $ cargo check --workspace",
            "• Running · esc interrupt",
            "├ • Bash cargo test --workspace",
            "└ ◦ mail.Search release notes · queued",
            "Executing · ",
            "Cargo.toml",
            "Finished with 0 errors",
            "warning: dependency graph",
            "Tab: question preview",
        ] {
            assert!(
                first.contains(expected),
                "missing {expected:?} in demo screen: {first}"
            );
        }

        terminal.draw(render).expect("repeat demo render");
        let second = text(&terminal);
        assert_eq!(first, second, "fixed demo changed across redraws");

        demo.toggle_question_preview();
        let question = render_snapshot(demo.render_state());
        terminal
            .draw(|frame| {
                let mut transcript = TranscriptState::default();
                let _ = render_with_transcript_snapshot(frame, &question, &mut transcript);
            })
            .expect("render question preview");
        let question_text = text(&terminal);
        assert!(question_text.contains("Report options"), "{question_text}");
        assert!(question_text.contains("Question 1/2"), "{question_text}");

        demo.toggle_question_preview();
        assert_eq!(demo.render_state().status, AppStatus::Streaming);
        assert!(demo.render_state().pending_question.is_none());
    }
}
