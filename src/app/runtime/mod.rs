use crate::app::{
    AppEvent, AppEventSender, AppState, AppStatus, ChatMessage, UpdateDecision, Verbosity,
};
use crate::network::{AgentUiEvent, AgentUiEventReceiver, AgentUiEventSender};
use crate::ui;
use crate::ui::{
    FrameRequester, FrameStream, TerminalRuntime, TranscriptState, TuiEvent, TuiEventStream,
};
use crossterm::{
    event::{self, KeyCode, KeyModifiers},
    execute,
};
use ratatui::layout::Size;
use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, MutexGuard, mpsc};
use tokio_util::sync::CancellationToken;

mod events;
mod input;
mod orchestration;
mod render;
mod sessions;
mod terminal;
mod transcript;
mod updates;

use events::{apply_approval_decision, apply_question_answer};
use input::{InputContext, InputFlow, handle_app_event};
#[cfg(test)]
use render::session_title_for_render;
use render::{RenderFrameContext, render_frame};
use sessions::{apply_session_event, apply_subagent_selection, open_overlay};
use terminal::{handle_terminal_resize, notify_response_finished, restore_terminal};
#[cfg(test)]
use transcript::render_finalized_assistant_scrollback;
use updates::{apply_update_decision, run_update_command};

const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(16);
// Streaming text and spinner updates do not need terminal refreshes at the
// input/event poll rate. Event-driven redraws still happen immediately; this
// interval only bounds periodic redraws while a turn is active.
const STREAM_FRAME_INTERVAL: Duration = Duration::from_millis(100);

pub(crate) struct AppRuntime {
    terminal_runtime: Option<TerminalRuntime>,
    app_state: Arc<Mutex<AppState>>,
    client: reqwest::Client,
    current_cancel_token: CancellationToken,
    needs_redraw: bool,
    was_responding: bool,
    terminal_focused: bool,
    transcript_cursor: crate::ui::scrollback::TranscriptCursor,
    transcript_state: TranscriptState,
    stream_commits: crate::ui::scrollback::StreamCommitQueue,
    replaying_transcript: bool,
    terminal_size: Size,
    tui_events: TuiEventStream,
    frame_requester: FrameRequester,
    frame_stream: FrameStream,
    app_event_sender: AppEventSender,
    app_event_receiver: mpsc::UnboundedReceiver<AppEvent>,
    agent_ui_event_sender: AgentUiEventSender,
    agent_ui_event_receiver: AgentUiEventReceiver,
    task_subscriptions: HashMap<String, rustcode_tasks::TaskSubscription>,
}

#[derive(Debug)]
pub(crate) struct AppError(String);

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for AppError {}

#[allow(dead_code)]
pub(crate) enum AppRunControl {
    Continue,
    Exit(crate::ExitSummary),
}

impl AppRuntime {
    pub(crate) fn new(
        mut terminal_runtime: TerminalRuntime,
        app_state: Arc<Mutex<AppState>>,
        client: reqwest::Client,
    ) -> Result<Self, Box<dyn Error>> {
        let terminal_size = terminal_runtime.terminal().size()?;
        let (app_event_sender, app_event_receiver) = AppEventSender::channel();
        let (agent_ui_event_sender, agent_ui_event_receiver) = AgentUiEventSender::channel();
        let (frame_requester, frame_stream) = FrameRequester::new(STREAM_FRAME_INTERVAL);

        Ok(Self {
            terminal_runtime: Some(terminal_runtime),
            app_state,
            client,
            current_cancel_token: CancellationToken::new(),
            needs_redraw: true,
            was_responding: false,
            terminal_focused: true,
            transcript_cursor: crate::ui::scrollback::TranscriptCursor::default(),
            transcript_state: TranscriptState::default(),
            stream_commits: crate::ui::scrollback::StreamCommitQueue::default(),
            replaying_transcript: false,
            terminal_size,
            tui_events: TuiEventStream::new(),
            frame_requester,
            frame_stream,
            app_event_sender,
            app_event_receiver,
            agent_ui_event_sender,
            agent_ui_event_receiver,
            task_subscriptions: HashMap::new(),
        })
    }

    #[cfg(test)]
    pub(crate) fn for_test(app_state: AppState) -> Self {
        let (app_event_sender, app_event_receiver) = AppEventSender::channel();
        let (agent_ui_event_sender, agent_ui_event_receiver) = AgentUiEventSender::channel();
        let (frame_requester, frame_stream) = FrameRequester::new(STREAM_FRAME_INTERVAL);
        Self {
            terminal_runtime: None,
            app_state: Arc::new(Mutex::new(app_state)),
            client: reqwest::Client::new(),
            current_cancel_token: CancellationToken::new(),
            needs_redraw: false,
            was_responding: false,
            terminal_focused: true,
            transcript_cursor: crate::ui::scrollback::TranscriptCursor::default(),
            transcript_state: TranscriptState::default(),
            stream_commits: crate::ui::scrollback::StreamCommitQueue::default(),
            replaying_transcript: false,
            terminal_size: Size::new(80, 24),
            tui_events: TuiEventStream::paused(),
            frame_requester,
            frame_stream,
            app_event_sender,
            app_event_receiver,
            agent_ui_event_sender,
            agent_ui_event_receiver,
            task_subscriptions: HashMap::new(),
        }
    }

    pub(crate) async fn app_state(&self) -> MutexGuard<'_, AppState> {
        self.app_state.lock().await
    }
}

#[cfg(test)]
mod tests {
    use super::{AppRunControl, AppRuntime, apply_update_decision};
    use crate::app::{
        AppEvent, AppState, AppStatus, ApprovalDecision, PendingQuestion, QuestionAnswer,
        SessionAction, UpdateDecision,
    };

    #[tokio::test]
    async fn request_draw_and_submit_are_handled_by_the_runtime() {
        let mut runtime = AppRuntime::for_test(AppState::new());

        assert!(matches!(
            runtime.handle_event(AppEvent::RequestDraw).await,
            Ok(AppRunControl::Continue)
        ));
        assert!(matches!(
            runtime
                .handle_event(AppEvent::SubmitPrompt("hello".to_string()))
                .await,
            Ok(AppRunControl::Continue)
        ));

        let state = runtime.app_state().await;
        assert_eq!(state.input_buffer, "hello");
        assert!(state.redraw_requested);
    }

    #[tokio::test]
    async fn stale_render_metrics_cannot_overwrite_new_state() {
        let runtime = AppRuntime::for_test(AppState::new());
        let (revision, input_area) = {
            let state = runtime.app_state().await;
            (
                state.render_snapshot().revision(),
                ratatui::layout::Rect::new(1, 2, 30, 4),
            )
        };

        let mut state = runtime.app_state().await;
        state.conversation_content_height = 7;
        state.input_text_area = Some(ratatui::layout::Rect::new(3, 4, 20, 2));
        state.request_redraw();

        assert!(!state.publish_render_metrics(revision, 99, input_area));
        assert_eq!(state.conversation_content_height, 7);
        assert_eq!(
            state.input_text_area,
            Some(ratatui::layout::Rect::new(3, 4, 20, 2))
        );
    }

    #[tokio::test]
    async fn render_title_cache_is_selected_against_the_current_session() {
        let mut state = AppState::new();
        state.active_session_id = "session-a".to_owned();
        state.session_title_cache = Some(("session-a".to_owned(), Some("Session A".to_owned())));
        let state = std::sync::Arc::new(tokio::sync::Mutex::new(state));

        let (session_id, title) = super::session_title_for_render(&state).await;

        assert_eq!(session_id, "session-a");
        assert_eq!(title.as_deref(), Some("Session A"));
    }

    #[test]
    fn render_title_cache_does_not_install_a_title_for_another_session() {
        let mut state = AppState::new();
        state.active_session_id = "session-a".to_owned();
        let stale_generation = state.session_title_cache_generation;
        state.invalidate_session_title_cache();

        assert!(!state.install_session_title_cache(
            "session-a",
            stale_generation,
            Some("Stale Session A".to_owned())
        ));

        state.active_session_id = "session-b".to_owned();

        assert!(!state.install_session_title_cache(
            "session-a",
            state.session_title_cache_generation,
            Some("Session A".to_owned())
        ));
        assert!(state.session_title_cache.is_none());
    }

    #[test]
    fn non_streamed_assistant_scrollback_keeps_history_block() {
        let mut state = AppState::new();
        state
            .history
            .push(crate::app::ChatMessage::new("assistant", "final answer"));
        let snapshot = state.render_snapshot();
        let mut cursor = crate::ui::scrollback::TranscriptCursor::default();

        let lines = super::render_finalized_assistant_scrollback(
            &snapshot,
            &mut cursor,
            0,
            "final answer",
            80,
        );
        assert!(
            lines
                .iter()
                .any(|line| line.to_string().contains("final answer"))
        );

        cursor.commit_stable_stream("final answer");
        let separator = super::render_finalized_assistant_scrollback(
            &snapshot,
            &mut cursor,
            0,
            "final answer",
            80,
        );
        assert_eq!(separator, vec![ratatui::text::Line::from("")]);
    }

    #[test]
    fn update_prompt_decisions_close_prompt_and_remember_version() {
        let mut state = AppState::new();
        state.show_update_prompt = true;
        state.update_check = crate::update::UpdateState::Available((0, 30, 0));

        assert!(!apply_update_decision(&mut state, UpdateDecision::Skip));
        assert!(!state.show_update_prompt);

        state.show_update_prompt = true;
        assert!(!apply_update_decision(
            &mut state,
            UpdateDecision::SkipUntilNextVersion
        ));
        assert_eq!(state.dismissed_update_version, Some((0, 30, 0)));

        state.show_update_prompt = true;
        assert!(apply_update_decision(&mut state, UpdateDecision::UpdateNow));
        assert!(!state.show_update_prompt);
    }

    #[tokio::test]
    async fn session_events_are_applied_by_the_runtime_controller() {
        let mut state = AppState::new();
        let old_session = state.active_session_id.clone();
        state
            .history
            .push(crate::app::ChatMessage::new("user", "old"));
        let mut runtime = AppRuntime::for_test(state);

        runtime
            .handle_event(AppEvent::NewSession)
            .await
            .expect("new session event should be handled");

        let state = runtime.app_state().await;
        assert_ne!(state.active_session_id, old_session);
        assert!(!state.show_history_picker);
        assert!(
            state
                .history
                .iter()
                .any(|message| message.content == "Started a new session")
        );
    }

    #[tokio::test]
    async fn delete_session_event_rejects_invalid_ids_without_mutation() {
        let mut runtime = AppRuntime::for_test(AppState::new());

        assert!(
            runtime
                .handle_event(AppEvent::DeleteSession(SessionAction::Id(
                    "../escape".to_owned(),
                )))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn select_subagent_event_switches_context_without_mutating_parent_history() {
        let mut state = AppState::new();
        state
            .history
            .push(crate::app::ChatMessage::new("user", "parent"));
        let id = crate::app::SubagentController.spawn(
            &mut state,
            "child",
            None,
            None,
            false,
            Vec::new(),
            None,
            None,
        );
        let mut runtime = AppRuntime::for_test(state);

        runtime
            .handle_event(AppEvent::SelectSubagent(id.raw()))
            .await
            .expect("selection event should be handled");

        let state = runtime.app_state().await;
        assert_eq!(state.selected_subagent_id, Some(id.raw()));
        assert_eq!(state.history[0].content, "parent");
        assert!(!state.show_subagent_picker);
    }

    #[tokio::test]
    async fn exit_event_returns_a_summary_without_touching_the_terminal() {
        let mut runtime = AppRuntime::for_test(AppState::new());

        assert!(matches!(
            runtime.handle_event(AppEvent::Exit).await,
            Ok(AppRunControl::Exit(_))
        ));
    }

    #[tokio::test]
    async fn approval_events_resolve_the_existing_policy_channel() {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let mut state = AppState::new();
        state.status = AppStatus::AwaitingToolConfirmation;
        state.pending_tool_confirmation = Some(Vec::new());
        state.tool_confirmation_response = Some(tx);
        let mut runtime = AppRuntime::for_test(state);

        runtime
            .handle_event(AppEvent::ApprovalDecision(ApprovalDecision::ApproveAll))
            .await
            .expect("approval event should be handled");

        assert!(rx.await.expect("approval response"));
        let state = runtime.app_state().await;
        assert!(state.auto_confirm);
        assert!(state.pending_tool_confirmation.is_none());
    }

    #[tokio::test]
    async fn denying_approval_cancels_the_turn_before_resolving_it() {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let mut state = AppState::new();
        state.status = AppStatus::AwaitingToolConfirmation;
        state.pending_tool_confirmation = Some(Vec::new());
        state.tool_confirmation_response = Some(tx);
        let mut runtime = AppRuntime::for_test(state);
        let previous_token = runtime.current_cancel_token.clone();

        runtime
            .handle_event(AppEvent::ApprovalDecision(ApprovalDecision::Deny))
            .await
            .expect("approval event should be handled");

        assert!(!rx.await.expect("approval response"));
        assert!(previous_token.is_cancelled());
    }

    #[tokio::test]
    async fn question_events_return_typed_answers_through_the_existing_channel() {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let mut state = AppState::new();
        state.status = AppStatus::AwaitingQuestion;
        state.pending_question = Some(PendingQuestion::new(
            "Where?".to_owned(),
            vec!["Here".to_owned()],
            false,
        ));
        state.question_response = Some(tx);
        let mut runtime = AppRuntime::for_test(state);

        runtime
            .handle_event(AppEvent::AnswerQuestion(QuestionAnswer::Custom(
                "somewhere else".to_owned(),
            )))
            .await
            .expect("question event should be handled");

        assert_eq!(rx.await.expect("question response"), "somewhere else");
        let state = runtime.app_state().await;
        assert!(state.pending_question.is_none());
    }

    #[tokio::test]
    async fn cancelling_a_question_returns_the_legacy_cancel_text() {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let mut state = AppState::new();
        state.status = AppStatus::AwaitingQuestion;
        state.pending_question = Some(PendingQuestion::new(
            "Where?".to_owned(),
            vec!["Here".to_owned()],
            false,
        ));
        state.question_response = Some(tx);
        let mut runtime = AppRuntime::for_test(state);
        let previous_token = runtime.current_cancel_token.clone();

        runtime
            .handle_event(AppEvent::AnswerQuestion(QuestionAnswer::Cancelled))
            .await
            .expect("question event should be handled");

        assert_eq!(
            rx.await.expect("question response"),
            "User cancelled prompt."
        );
        let state = runtime.app_state().await;
        assert_eq!(state.status, AppStatus::Idle);
        assert!(previous_token.is_cancelled());
    }
}
