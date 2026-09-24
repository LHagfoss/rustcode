use std::collections::HashMap;

use rustcode::controller::{
    ControllerSnapshot, ControllerUpdate, QuestionPrompt, TranscriptItem, TurnUpdate,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectionRow {
    User(String),
    Assistant(String),
    Tool { name: String, content: String },
    System(String),
}

pub fn project_rows(transcript: &[TranscriptItem], live_response: &str) -> Vec<ProjectionRow> {
    let mut rows = transcript
        .iter()
        .map(|item| {
            if let Some(name) = item.tool_name.as_ref() {
                ProjectionRow::Tool {
                    name: name.clone(),
                    content: item.content.clone(),
                }
            } else {
                match item.role.as_str() {
                    "user" => ProjectionRow::User(item.content.clone()),
                    "assistant" => ProjectionRow::Assistant(item.content.clone()),
                    _ => ProjectionRow::System(item.content.clone()),
                }
            }
        })
        .collect::<Vec<_>>();

    if !live_response.is_empty() {
        match rows.last_mut() {
            Some(ProjectionRow::Assistant(content))
                if live_response.starts_with(content.as_str()) =>
            {
                content.push_str(&live_response[content.len()..]);
            }
            Some(ProjectionRow::Assistant(content)) if content.ends_with(live_response) => {}
            Some(ProjectionRow::Assistant(content)) => content.push_str(live_response),
            _ => rows.push(ProjectionRow::Assistant(live_response.to_owned())),
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
    stream_rows: Vec<ProjectionRow>,
    active_tools: HashMap<String, usize>,
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
        if !self.turn_active {
            self.stream_rows.clear();
            self.active_tools.clear();
        }
    }

    fn apply_turn_update(&mut self, update: TurnUpdate) {
        match update {
            TurnUpdate::PromptStarted(_) => {
                self.turn_active = true;
                self.error = None;
                self.approval_denied = false;
                self.stream_rows.clear();
                self.active_tools.clear();
            }
            TurnUpdate::TextDelta(text) => {
                self.turn_active = true;
                match self.stream_rows.last_mut() {
                    Some(ProjectionRow::Assistant(content)) => content.push_str(&text),
                    _ => self.stream_rows.push(ProjectionRow::Assistant(text)),
                }
            }
            TurnUpdate::ToolStarted { id, name } => {
                self.turn_active = true;
                let row = self.stream_rows.len();
                self.stream_rows.push(ProjectionRow::Tool {
                    name,
                    content: "Running…".to_owned(),
                });
                self.active_tools.insert(id, row);
            }
            TurnUpdate::ToolFinished { id, content } => {
                self.turn_active = true;
                if let Some(row) = self.active_tools.remove(&id)
                    && let Some(ProjectionRow::Tool {
                        content: current, ..
                    }) = self.stream_rows.get_mut(row)
                {
                    *current = content;
                }
            }
            TurnUpdate::ApprovalRequested(_) => self.turn_active = true,
            TurnUpdate::TurnFinished | TurnUpdate::Cancelled => self.turn_active = false,
        }
    }

    pub fn turn_active(&self) -> bool {
        self.turn_active
    }

    pub fn stream_rows(&self) -> &[ProjectionRow] {
        &self.stream_rows
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
        ChatViewState, ProjectionRow, answer_for_question, can_submit, project_rows,
        stop_available, toggle_option,
    };

    fn user(content: &str) -> TranscriptItem {
        TranscriptItem {
            role: "user".to_owned(),
            content: content.to_owned(),
            tool_name: None,
        }
    }

    fn assistant(content: &str) -> TranscriptItem {
        TranscriptItem {
            role: "assistant".to_owned(),
            content: content.to_owned(),
            tool_name: None,
        }
    }

    fn tool(content: &str, name: &str) -> TranscriptItem {
        TranscriptItem {
            role: "tool".to_owned(),
            content: content.to_owned(),
            tool_name: Some(name.to_owned()),
        }
    }

    #[test]
    fn projection_keeps_history_and_live_assistant_text_in_order_once() {
        assert_eq!(
            project_rows(&[user("Hi"), assistant("Hello")], "!"),
            vec![
                ProjectionRow::User("Hi".to_owned()),
                ProjectionRow::Assistant("Hello!".to_owned()),
            ]
        );
    }

    #[test]
    fn live_response_already_present_in_history_is_not_duplicated() {
        assert_eq!(
            project_rows(&[assistant("Hello")], "Hello"),
            vec![ProjectionRow::Assistant("Hello".to_owned())]
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
                    content: "done".to_owned()
                },
                ProjectionRow::Assistant("Okay".to_owned()),
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
            text: "Choose".to_owned(),
            options: vec!["One".to_owned(), "Two".to_owned()],
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
        }));

        assert!(view.turn_active());
        assert!(stop_available(view.turn_active()));
        assert_eq!(
            view.stream_rows(),
            &[
                ProjectionRow::Assistant("Before ".to_owned()),
                ProjectionRow::Tool {
                    name: "read_file".to_owned(),
                    content: "Running…".to_owned(),
                },
            ]
        );

        view.apply_update(ControllerUpdate::Turn(TurnUpdate::ToolFinished {
            id: "tool-1".to_owned(),
            content: "file contents".to_owned(),
        }));
        view.apply_update(ControllerUpdate::Turn(TurnUpdate::TextDelta(
            "After".to_owned(),
        )));
        assert_eq!(
            view.stream_rows(),
            &[
                ProjectionRow::Assistant("Before ".to_owned()),
                ProjectionRow::Tool {
                    name: "read_file".to_owned(),
                    content: "file contents".to_owned(),
                },
                ProjectionRow::Assistant("After".to_owned()),
            ]
        );

        view.apply_update(ControllerUpdate::Turn(TurnUpdate::TurnFinished));
        assert!(!view.turn_active());
        assert!(!stop_available(view.turn_active()));
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
