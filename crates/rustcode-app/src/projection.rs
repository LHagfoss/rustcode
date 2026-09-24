use rustcode::controller::{QuestionPrompt, TranscriptItem};

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
}

impl ChatViewState {
    pub fn set_error(&mut self, error: String) {
        self.error = Some(error);
    }

    pub fn clear_error(&mut self) {
        self.error = None;
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
}
