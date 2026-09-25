use std::{collections::HashMap, time::Instant};

use rustcode::controller::{
    ApprovalPrompt, ControllerSnapshot, ControllerUpdate, QuestionPrompt, TranscriptItem,
    TurnUpdate,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectionRow {
    User(String),
    Assistant {
        content: String,
        response_time_ms: Option<u64>,
        thought_time_ms: Option<u64>,
    },
    Tool {
        name: String,
        detail: Option<String>,
        content: String,
        status: ToolStatus,
        elapsed_ms: Option<u64>,
    },
    System(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolStatus {
    Running,
    Pending,
    Completed,
    Failed,
}

pub fn project_rows(transcript: &[TranscriptItem], live_response: &str) -> Vec<ProjectionRow> {
    let mut cancellation_shown = false;
    let mut rows = transcript
        .iter()
        .filter_map(|item| {
            if item.role == "user" {
                cancellation_shown = false;
            }
            if item.role == "system"
                && matches!(
                    item.content.trim(),
                    "Request cancelled by user" | "[harness: turn stopped — cancelled]"
                )
            {
                if cancellation_shown {
                    return None;
                }
                cancellation_shown = true;
                return Some(ProjectionRow::System("Stopped by you".to_owned()));
            }
            Some(if let Some(name) = item.tool_name.as_ref() {
                ProjectionRow::Tool {
                    name: name.clone(),
                    detail: item.tool_detail.clone(),
                    content: item.content.clone(),
                    status: if item.tool_pending {
                        ToolStatus::Pending
                    } else if item.tool_success == Some(false) {
                        ToolStatus::Failed
                    } else {
                        ToolStatus::Completed
                    },
                    elapsed_ms: None,
                }
            } else {
                match item.role.as_str() {
                    "user" => ProjectionRow::User(item.content.clone()),
                    "assistant" => ProjectionRow::Assistant {
                        content: item.content.clone(),
                        response_time_ms: item.response_time_ms,
                        thought_time_ms: item.thought_time_ms,
                    },
                    _ => ProjectionRow::System(item.content.clone()),
                }
            })
        })
        .collect::<Vec<_>>();

    if !live_response.is_empty() {
        match rows.last_mut() {
            Some(ProjectionRow::Assistant { content, .. })
                if live_response.starts_with(content.as_str()) =>
            {
                content.push_str(&live_response[content.len()..]);
            }
            Some(ProjectionRow::Assistant { content, .. }) if content.ends_with(live_response) => {}
            Some(ProjectionRow::Assistant { content, .. }) => content.push_str(live_response),
            _ => rows.push(ProjectionRow::Assistant {
                content: live_response.to_owned(),
                response_time_ms: None,
                thought_time_ms: None,
            }),
        }
    }
    rows
}

pub fn can_submit(input: &str) -> bool {
    !input.trim().is_empty()
}

pub fn stop_available(turn_active: bool) -> bool {
    turn_active
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComposerAction {
    Send,
    Stop,
}

pub fn answer_for_question(
    _question: &QuestionPrompt,
    selected_option: Option<&str>,
    freeform: &str,
) -> Option<String> {
    selected_option
        .filter(|answer| !answer.trim().is_empty())
        .map(str::to_owned)
        .or_else(|| (!freeform.trim().is_empty()).then(|| freeform.trim().to_owned()))
}

pub fn toggle_option(selected: &mut Vec<String>, option: &str) {
    if let Some(index) = selected.iter().position(|current| current == option) {
        selected.remove(index);
    } else {
        selected.push(option.to_owned());
    }
}

#[derive(Default)]
pub struct ChatViewState {
    error: Option<String>,
    approval_denied: bool,
    turn_active: bool,
    queued_count: usize,
    stream_rows: Vec<ProjectionRow>,
    active_tools: HashMap<String, (usize, Instant)>,
    turn_started_at: Option<Instant>,
    last_turn_elapsed_ms: Option<u64>,
    thinking_started_at: Option<Instant>,
    pending_question: Option<QuestionPrompt>,
    pending_approval: Option<ApprovalPrompt>,
}

impl ChatViewState {
    pub fn apply_update(&mut self, update: ControllerUpdate) {
        match update {
            ControllerUpdate::Snapshot(snapshot) => self.apply_snapshot(snapshot),
            ControllerUpdate::Turn(update) => self.apply_turn_update(update),
            ControllerUpdate::Error(error) => {
                self.set_error(format!("Controller error: {error:?}"));
            }
        }
    }

    fn apply_snapshot(&mut self, snapshot: ControllerSnapshot) {
        self.turn_active = snapshot.turn_active;
        self.queued_count = snapshot.queued_count;
        self.pending_question = snapshot.pending_question.clone();
        self.pending_approval = snapshot.pending_approval.clone();
        if snapshot.transcript.iter().any(|item| {
            item.role == "user"
                && self
                    .stream_rows
                    .iter()
                    .any(|row| row == &ProjectionRow::User(item.content.clone()))
        }) {
            // Active tools point into this vector. Keep those indices aligned
            // when an in-flight snapshot absorbs the optimistic user row.
            let mut removed_before = vec![0; self.stream_rows.len()];
            let mut removed = 0;
            for (index, row) in self.stream_rows.iter().enumerate() {
                removed_before[index] = removed;
                if matches!(row, ProjectionRow::User(_)) {
                    removed += 1;
                }
            }
            for (row, _) in self.active_tools.values_mut() {
                *row -= removed_before[*row];
            }
            self.stream_rows
                .retain(|row| !matches!(row, ProjectionRow::User(_)));
        }
        if !self.turn_active {
            self.stream_rows.clear();
            self.active_tools.clear();
            self.thinking_started_at = None;
        }
    }

    fn apply_turn_update(&mut self, update: TurnUpdate) {
        match update {
            TurnUpdate::PromptStarted(prompt) => {
                self.turn_active = true;
                self.turn_started_at = Some(Instant::now());
                self.last_turn_elapsed_ms = None;
                self.thinking_started_at = None;
                self.error = None;
                self.approval_denied = false;
                self.stream_rows.clear();
                self.active_tools.clear();
                self.stream_rows.push(ProjectionRow::User(prompt));
            }
            TurnUpdate::TextDelta(text) => {
                self.turn_active = true;
                match self.stream_rows.last_mut() {
                    Some(ProjectionRow::Assistant { content, .. }) => content.push_str(&text),
                    _ => self.stream_rows.push(ProjectionRow::Assistant {
                        content: text,
                        response_time_ms: None,
                        thought_time_ms: None,
                    }),
                }
                if let Some(ProjectionRow::Assistant { content, .. }) = self.stream_rows.last() {
                    let open = content.rfind("<think>");
                    let close = content.rfind("</think>");
                    let thinking = open.is_some_and(|open| close.is_none_or(|close| open > close));
                    if thinking && self.thinking_started_at.is_none() {
                        self.thinking_started_at = Some(Instant::now());
                    } else if !thinking {
                        self.finish_thinking();
                    }
                }
            }
            TurnUpdate::ToolStarted { id, name, detail } => {
                self.finish_thinking();
                self.turn_active = true;
                let row = self.stream_rows.len();
                self.stream_rows.push(ProjectionRow::Tool {
                    name,
                    detail,
                    content: String::new(),
                    status: ToolStatus::Running,
                    elapsed_ms: None,
                });
                self.active_tools.insert(id, (row, Instant::now()));
            }
            TurnUpdate::ToolFinished {
                id,
                content,
                success,
                pending,
            } => {
                self.turn_active = true;
                if let Some((row, started_at)) = self.active_tools.remove(&id)
                    && let Some(ProjectionRow::Tool {
                        content: current,
                        status,
                        elapsed_ms,
                        ..
                    }) = self.stream_rows.get_mut(row)
                {
                    *current = content;
                    *status = if pending {
                        ToolStatus::Pending
                    } else if success {
                        ToolStatus::Completed
                    } else {
                        ToolStatus::Failed
                    };
                    *elapsed_ms = Some(started_at.elapsed().as_millis() as u64);
                }
            }
            TurnUpdate::ApprovalRequested(approvals) => {
                self.turn_active = true;
                self.pending_approval = approvals.into_iter().next();
            }
            TurnUpdate::QuestionRequested(question) => {
                self.turn_active = true;
                self.pending_question = Some(question);
            }
            TurnUpdate::TurnFinished | TurnUpdate::Cancelled => {
                self.turn_active = false;
                self.finish_thinking();
                self.last_turn_elapsed_ms = self
                    .turn_started_at
                    .take()
                    .map(|started| started.elapsed().as_millis() as u64);
            }
        }
    }

    fn finish_thinking(&mut self) {
        if let Some(started) = self.thinking_started_at.take() {
            let elapsed = started.elapsed().as_millis() as u64;
            if let Some(ProjectionRow::Assistant {
                thought_time_ms, ..
            }) = self.stream_rows.last_mut()
            {
                *thought_time_ms = Some(thought_time_ms.unwrap_or(0).saturating_add(elapsed));
            }
        }
    }

    pub fn turn_active(&self) -> bool {
        self.turn_active
    }

    pub fn composer_action(&self) -> ComposerAction {
        if self.turn_active {
            ComposerAction::Stop
        } else {
            ComposerAction::Send
        }
    }

    pub fn queued_message_label(&self) -> Option<String> {
        (self.queued_count > 0).then(|| format!("{} queued", self.queued_count))
    }

    pub fn turn_elapsed_ms(&self) -> Option<u64> {
        self.turn_started_at
            .map(|started| started.elapsed().as_millis() as u64)
            .or(self.last_turn_elapsed_ms)
    }

    pub fn clear_turn_elapsed(&mut self) {
        self.turn_started_at = None;
        self.last_turn_elapsed_ms = None;
        self.thinking_started_at = None;
    }

    pub fn thought_elapsed_ms(&self) -> Option<u64> {
        let Some(ProjectionRow::Assistant {
            thought_time_ms, ..
        }) = self.stream_rows.last()
        else {
            return None;
        };
        match self.thinking_started_at {
            Some(started) => Some(
                thought_time_ms
                    .unwrap_or(0)
                    .saturating_add(started.elapsed().as_millis() as u64),
            ),
            None => *thought_time_ms,
        }
    }

    pub fn stream_rows(&self) -> &[ProjectionRow] {
        &self.stream_rows
    }

    pub fn stream_rows_with_elapsed(&self) -> Vec<ProjectionRow> {
        let mut rows = self.stream_rows.clone();
        for (row, started) in self.active_tools.values() {
            if let Some(ProjectionRow::Tool { elapsed_ms, .. }) = rows.get_mut(*row) {
                *elapsed_ms = Some(started.elapsed().as_millis() as u64);
            }
        }
        rows
    }

    pub fn pending_question(&self) -> Option<&QuestionPrompt> {
        self.pending_question.as_ref()
    }

    pub fn pending_approval(&self) -> Option<&ApprovalPrompt> {
        self.pending_approval.as_ref()
    }

    pub fn begin_user_action(&mut self) {
        self.error = None;
        self.approval_denied = false;
    }

    pub fn set_error(&mut self, error: String) {
        self.error = Some(error);
    }

    pub fn set_approval_denied(&mut self) {
        self.approval_denied = true;
    }

    pub fn approval_status(&self) -> Option<&'static str> {
        self.approval_denied.then_some("Approval denied")
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn composer_enabled(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use rustcode::controller::{
        ApprovalPrompt, ControllerError, ControllerSnapshot, ControllerUpdate, TurnUpdate,
    };
    use rustcode::controller::{QuestionPrompt, TranscriptItem};

    use super::{
        ChatViewState, ComposerAction, ProjectionRow, ToolStatus, answer_for_question, can_submit,
        project_rows, stop_available, toggle_option,
    };

    fn user(content: &str) -> TranscriptItem {
        TranscriptItem {
            role: "user".to_owned(),
            content: content.to_owned(),
            tool_name: None,
            tool_detail: None,
            tool_success: None,
            tool_pending: false,
            response_time_ms: None,
            thought_time_ms: None,
        }
    }

    fn assistant(content: &str) -> TranscriptItem {
        TranscriptItem {
            role: "assistant".to_owned(),
            content: content.to_owned(),
            tool_name: None,
            tool_detail: None,
            tool_success: None,
            tool_pending: false,
            response_time_ms: None,
            thought_time_ms: None,
        }
    }

    fn tool(content: &str, name: &str) -> TranscriptItem {
        TranscriptItem {
            role: "tool".to_owned(),
            content: content.to_owned(),
            tool_name: Some(name.to_owned()),
            tool_detail: None,
            tool_success: Some(true),
            tool_pending: false,
            response_time_ms: None,
            thought_time_ms: None,
        }
    }

    fn system(content: &str) -> TranscriptItem {
        TranscriptItem {
            role: "system".into(),
            ..assistant(content)
        }
    }

    #[test]
    fn cancellation_notices_collapse_per_turn_without_changing_history() {
        let transcript = vec![
            user("first"),
            system("Request cancelled by user"),
            system("[harness: turn stopped — cancelled]"),
            user("second"),
            system("[harness: turn stopped — cancelled]"),
            system("Request cancelled by user"),
        ];
        let original = transcript.clone();
        assert_eq!(
            project_rows(&transcript, ""),
            vec![
                ProjectionRow::User("first".into()),
                ProjectionRow::System("Stopped by you".into()),
                ProjectionRow::User("second".into()),
                ProjectionRow::System("Stopped by you".into()),
            ]
        );
        assert_eq!(transcript, original);
    }

    #[test]
    fn cancellation_projection_preserves_other_errors_and_quoted_markers() {
        let budget = "[harness: stopped after 10 tool round(s) — budget exhausted]";
        let error = "Error from LLM Provider: timeout";
        let marker = "[harness: turn stopped — cancelled]";
        assert_eq!(
            project_rows(
                &[
                    user(marker),
                    assistant(marker),
                    system("Request cancelled by user"),
                    system(error),
                    system(marker),
                    system(budget),
                    system("[harness: turn stopped — dependency unavailable]"),
                ],
                ""
            ),
            vec![
                ProjectionRow::User(marker.into()),
                ProjectionRow::Assistant {
                    content: marker.into(),
                    response_time_ms: None,
                    thought_time_ms: None
                },
                ProjectionRow::System("Stopped by you".into()),
                ProjectionRow::System(error.into()),
                ProjectionRow::System(budget.into()),
                ProjectionRow::System("[harness: turn stopped — dependency unavailable]".into()),
            ]
        );
    }

    #[test]
    fn tool_context_survives_completion_and_saved_projection() {
        let detail = Some("src/main.rs".to_owned());
        let mut view = ChatViewState::default();
        view.apply_turn_update(TurnUpdate::ToolStarted {
            id: "read".into(),
            name: "view_file".into(),
            detail: detail.clone(),
        });
        view.apply_turn_update(TurnUpdate::ToolFinished {
            id: "read".into(),
            content: "file contents".into(),
            success: true,
            pending: false,
        });
        assert!(
            matches!(&view.stream_rows()[0], ProjectionRow::Tool { detail: actual, status: ToolStatus::Completed, .. } if actual == &detail)
        );
        let saved = TranscriptItem {
            tool_detail: detail.clone(),
            ..tool("file contents", "view_file")
        };
        assert!(
            matches!(&project_rows(&[saved], "")[0], ProjectionRow::Tool { detail: actual, .. } if actual == &detail)
        );
    }

    #[test]
    fn projection_keeps_history_and_live_assistant_text_in_order_once() {
        assert_eq!(
            project_rows(&[user("Hi"), assistant("Hello")], "!"),
            vec![
                ProjectionRow::User("Hi".to_owned()),
                ProjectionRow::Assistant {
                    content: "Hello!".to_owned(),
                    response_time_ms: None,
                    thought_time_ms: None
                },
            ]
        );
    }

    #[test]
    fn live_response_already_present_in_history_is_not_duplicated() {
        assert_eq!(
            project_rows(&[assistant("Hello")], "Hello"),
            vec![ProjectionRow::Assistant {
                content: "Hello".to_owned(),
                response_time_ms: None,
                thought_time_ms: None
            }]
        );
    }

    #[test]
    fn transcript_tool_rows_keep_their_position() {
        assert_eq!(
            project_rows(
                &[user("Run"), tool("done", "run_command"), assistant("Okay")],
                ""
            ),
            vec![
                ProjectionRow::User("Run".to_owned()),
                ProjectionRow::Tool {
                    name: "run_command".to_owned(),
                    detail: None,
                    content: "done".to_owned(),
                    status: ToolStatus::Completed,
                    elapsed_ms: None,
                },
                ProjectionRow::Assistant {
                    content: "Okay".to_owned(),
                    response_time_ms: None,
                    thought_time_ms: None
                },
            ]
        );
    }

    #[test]
    fn composer_availability_tracks_text_and_turn_state() {
        assert!(!can_submit(" \n"));
        assert!(can_submit("Explain this"));
        assert!(stop_available(true));
        assert!(!stop_available(false));
    }

    #[test]
    fn question_options_and_freeform_answers_keep_their_text() {
        let question = QuestionPrompt {
            header: "Question".to_owned(),
            text: "Choose".to_owned(),
            options: vec!["One".to_owned(), "Two".to_owned()],
            descriptions: vec![],
            multiple: false,
        };
        assert_eq!(
            answer_for_question(&question, Some("Two"), ""),
            Some("Two".to_owned())
        );
        assert_eq!(
            answer_for_question(&question, None, "Because"),
            Some("Because".to_owned())
        );
        assert_eq!(answer_for_question(&question, None, " \n"), None);
    }

    #[test]
    fn multiple_question_options_toggle_without_losing_order() {
        let mut selected = vec!["One".to_owned()];
        toggle_option(&mut selected, "Two");
        assert_eq!(selected, vec!["One", "Two"]);
        toggle_option(&mut selected, "One");
        assert_eq!(selected, vec!["Two"]);
    }

    #[test]
    fn denial_is_visible_and_provider_errors_leave_composer_enabled() {
        let mut view = ChatViewState::default();
        view.set_approval_denied();
        view.set_error("provider unavailable".to_owned());

        assert_eq!(view.approval_status(), Some("Approval denied"));
        assert_eq!(view.error(), Some("provider unavailable"));
        assert!(view.composer_enabled());
    }

    fn snapshot(turn_active: bool) -> ControllerSnapshot {
        ControllerSnapshot {
            generation: 1,
            workspace: None,
            session_id: Some("session".to_owned()),
            sessions: Vec::new(),
            models: Vec::new(),
            selected_model: None,
            transcript: Vec::new(),
            live_response: String::new(),
            queued_count: 0,
            turn_active,
            auto_approve: true,
            pending_question: None,
            pending_approval: None::<ApprovalPrompt>,
        }
    }

    #[test]
    fn streamed_turn_updates_expose_stop_and_order_live_tool_rows() {
        let mut view = ChatViewState::default();
        view.apply_update(ControllerUpdate::Snapshot(snapshot(false)));
        view.apply_update(ControllerUpdate::Turn(TurnUpdate::PromptStarted(
            "run".to_owned(),
        )));
        view.apply_update(ControllerUpdate::Turn(TurnUpdate::TextDelta(
            "Before ".to_owned(),
        )));
        view.apply_update(ControllerUpdate::Turn(TurnUpdate::ToolStarted {
            id: "tool-1".to_owned(),
            name: "read_file".to_owned(),
            detail: None,
        }));

        assert!(view.turn_active());
        assert!(stop_available(view.turn_active()));
        assert_eq!(
            view.stream_rows(),
            &[
                ProjectionRow::User("run".to_owned()),
                ProjectionRow::Assistant {
                    content: "Before ".to_owned(),
                    response_time_ms: None,
                    thought_time_ms: None
                },
                ProjectionRow::Tool {
                    name: "read_file".to_owned(),
                    detail: None,
                    content: String::new(),
                    status: ToolStatus::Running,
                    elapsed_ms: None,
                },
            ]
        );

        view.apply_update(ControllerUpdate::Turn(TurnUpdate::ToolFinished {
            id: "tool-1".to_owned(),
            content: "file contents".to_owned(),
            success: true,
            pending: false,
        }));
        view.apply_update(ControllerUpdate::Turn(TurnUpdate::TextDelta(
            "After".to_owned(),
        )));
        assert_eq!(view.stream_rows().len(), 4);
        assert!(matches!(
            &view.stream_rows()[2],
            ProjectionRow::Tool { name, content, status: ToolStatus::Completed, elapsed_ms: Some(_), .. }
                if name == "read_file" && content == "file contents"
        ));
        assert_eq!(
            view.stream_rows()[3],
            ProjectionRow::Assistant {
                content: "After".to_owned(),
                response_time_ms: None,
                thought_time_ms: None
            }
        );

        view.apply_update(ControllerUpdate::Turn(TurnUpdate::TurnFinished));
        assert!(!view.turn_active());
        assert!(!stop_available(view.turn_active()));
    }

    #[test]
    fn composer_action_and_queue_label_follow_the_latest_snapshot() {
        let mut view = ChatViewState::default();
        let mut idle = snapshot(false);
        idle.queued_count = 2;
        view.apply_update(ControllerUpdate::Snapshot(idle));

        assert_eq!(view.composer_action(), ComposerAction::Send);
        assert_eq!(view.queued_message_label().as_deref(), Some("2 queued"));

        let mut active = snapshot(true);
        active.queued_count = 1;
        view.apply_update(ControllerUpdate::Snapshot(active));
        assert_eq!(view.composer_action(), ComposerAction::Stop);
        assert_eq!(view.queued_message_label().as_deref(), Some("1 queued"));

        view.apply_update(ControllerUpdate::Snapshot(snapshot(false)));
        assert_eq!(view.queued_message_label(), None);
    }

    #[test]
    fn prompt_starts_as_a_user_row_and_final_snapshot_replaces_it_once() {
        let mut view = ChatViewState::default();
        view.apply_update(ControllerUpdate::Turn(TurnUpdate::PromptStarted(
            "show this immediately".to_owned(),
        )));

        assert_eq!(
            view.stream_rows(),
            &[ProjectionRow::User("show this immediately".to_owned())]
        );

        let mut final_snapshot = snapshot(false);
        final_snapshot
            .transcript
            .push(user("show this immediately"));
        view.apply_update(ControllerUpdate::Snapshot(final_snapshot.clone()));
        assert!(view.stream_rows().is_empty());
        assert_eq!(
            project_rows(&final_snapshot.transcript, &final_snapshot.live_response),
            vec![ProjectionRow::User("show this immediately".to_owned())]
        );
    }

    #[test]
    fn active_snapshot_preserves_tool_completion_targets() {
        let mut view = ChatViewState::default();
        view.apply_turn_update(TurnUpdate::PromptStarted("run".into()));
        for id in ["first", "second"] {
            view.apply_turn_update(TurnUpdate::ToolStarted {
                id: id.into(),
                name: "view_file".into(),
                detail: None,
            });
        }
        let mut active = snapshot(true);
        active.transcript.push(user("run"));
        view.apply_snapshot(active);
        for id in ["second", "first"] {
            view.apply_turn_update(TurnUpdate::ToolFinished {
                id: id.into(),
                content: id.into(),
                success: true,
                pending: false,
            });
        }
        assert!(
            matches!(&view.stream_rows()[0], ProjectionRow::Tool { content, status: ToolStatus::Completed, .. } if content == "first")
        );
        assert!(
            matches!(&view.stream_rows()[1], ProjectionRow::Tool { content, status: ToolStatus::Completed, .. } if content == "second")
        );
        assert!(view.active_tools.is_empty());
    }

    #[test]
    fn starting_tools_ends_unclosed_reasoning_and_its_timer() {
        let mut view = ChatViewState::default();
        view.apply_turn_update(TurnUpdate::PromptStarted("inspect".into()));
        view.apply_turn_update(TurnUpdate::TextDelta("<think>Read the file".into()));
        assert!(view.thinking_started_at.is_some());
        view.apply_turn_update(TurnUpdate::ToolStarted {
            id: "read".into(),
            name: "view_file".into(),
            detail: None,
        });
        assert!(view.thinking_started_at.is_none());
        assert!(
            matches!(&view.stream_rows()[1], ProjectionRow::Assistant { content, thought_time_ms: Some(_), .. } if content == "<think>Read the file")
        );
    }

    #[test]
    fn thinking_duration_belongs_to_current_assistant_phase() {
        let mut view = ChatViewState::default();
        view.apply_turn_update(TurnUpdate::PromptStarted("inspect".into()));
        view.apply_turn_update(TurnUpdate::TextDelta("<think>First step".into()));
        view.thinking_started_at =
            Some(std::time::Instant::now() - std::time::Duration::from_secs(60));
        view.apply_turn_update(TurnUpdate::ToolStarted {
            id: "read".into(),
            name: "view_file".into(),
            detail: None,
        });
        assert_eq!(view.thought_elapsed_ms(), None);
        view.apply_turn_update(TurnUpdate::TextDelta("<think>Second step".into()));
        assert!(
            view.thought_elapsed_ms().unwrap() < 1_000,
            "the second phase must not inherit the previous minute of reasoning"
        );
        view.apply_turn_update(TurnUpdate::TextDelta("</think>Answer".into()));
        assert!(view.thinking_started_at.is_none());
        assert!(view.thought_elapsed_ms().unwrap() < 1_000);
    }

    #[test]
    fn streamed_question_and_approval_events_open_their_dialog_state() {
        let mut view = ChatViewState::default();
        let question = QuestionPrompt {
            header: "Question".to_owned(),
            text: "Continue?".to_owned(),
            options: vec!["Proceed".to_owned()],
            descriptions: vec![],
            multiple: false,
        };
        let approval = ApprovalPrompt {
            request_id: "write-file-1".to_owned(),
            tool_name: "write_file".to_owned(),
            action_summary: "write_file · src/main.rs".to_owned(),
            risk_context: "This action requires your approval before it can continue.".to_owned(),
            description: "src/main.rs".to_owned(),
        };

        view.apply_update(ControllerUpdate::Turn(TurnUpdate::QuestionRequested(
            question.clone(),
        )));
        assert_eq!(view.pending_question(), Some(&question));

        view.apply_update(ControllerUpdate::Turn(TurnUpdate::ApprovalRequested(vec![
            approval.clone(),
        ])));
        assert_eq!(view.pending_approval(), Some(&approval));
        assert!(view.turn_active());
    }

    #[test]
    fn approval_lifecycle_keeps_the_exact_request_until_resolution_snapshot() {
        let mut view = ChatViewState::default();
        let approval = ApprovalPrompt {
            request_id: "9:call-approval".to_owned(),
            tool_name: "write_file".to_owned(),
            action_summary: "write_file · src/main.rs".to_owned(),
            risk_context: "This action requires your approval before it can continue.".to_owned(),
            description: "src/main.rs".to_owned(),
        };

        view.apply_update(ControllerUpdate::Turn(TurnUpdate::ApprovalRequested(vec![
            approval.clone(),
        ])));
        view.apply_update(ControllerUpdate::Turn(TurnUpdate::TextDelta(
            "progress while waiting".to_owned(),
        )));
        view.apply_update(ControllerUpdate::Turn(TurnUpdate::ToolStarted {
            id: "other-tool".to_owned(),
            name: "read_file".to_owned(),
            detail: Some("README.md".to_owned()),
        }));
        assert_eq!(view.pending_approval(), Some(&approval));

        let mut still_pending = snapshot(true);
        still_pending.pending_approval = Some(approval.clone());
        view.apply_update(ControllerUpdate::Snapshot(still_pending));
        assert_eq!(view.pending_approval(), Some(&approval));

        let mut resolved = snapshot(false);
        resolved.pending_approval = None;
        view.apply_update(ControllerUpdate::Snapshot(resolved));
        assert_eq!(view.pending_approval(), None);
    }

    #[test]
    fn final_snapshot_does_not_clear_provider_error_before_user_action() {
        let mut view = ChatViewState::default();
        view.apply_update(ControllerUpdate::Error(ControllerError::Provider(
            "provider unavailable".to_owned(),
        )));
        view.apply_update(ControllerUpdate::Snapshot(snapshot(false)));

        assert_eq!(
            view.error(),
            Some("Controller error: Provider(\"provider unavailable\")")
        );
        assert!(view.composer_enabled());

        view.begin_user_action();
        assert_eq!(view.error(), None);

        view.apply_update(ControllerUpdate::Error(ControllerError::Provider(
            "provider unavailable".to_owned(),
        )));
        view.apply_update(ControllerUpdate::Turn(TurnUpdate::PromptStarted(
            "retry".to_owned(),
        )));
        assert_eq!(view.error(), None);
    }

    #[test]
    fn denial_status_resets_when_the_next_turn_starts() {
        let mut view = ChatViewState::default();
        view.set_approval_denied();
        view.apply_update(ControllerUpdate::Turn(TurnUpdate::PromptStarted(
            "next".to_owned(),
        )));
        assert_eq!(view.approval_status(), None);
    }
}
