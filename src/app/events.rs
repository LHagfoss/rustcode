use crate::ui::TuiEvent;
use tokio::sync::mpsc;

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ApprovalDecision {
    Approve,
    Deny,
    ApproveAll,
    Custom(String),
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum QuestionAnswer {
    Selected(String),
    Custom(String),
    Cancelled,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Overlay {
    CommandPalette,
    History,
    Model,
    Theme,
    McpConfig,
    Verbosity,
    Thinking,
    Protocol,
    ToolConfirmation,
    Question,
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SessionAction {
    Latest,
    Id(String),
}

#[allow(dead_code)]
pub(crate) enum AppEvent {
    Tui(TuiEvent),
    SubmitPrompt(String),
    CancelActiveTurn,
    ApprovalDecision(ApprovalDecision),
    AnswerQuestion(QuestionAnswer),
    OpenOverlay(Overlay),
    CloseOverlay,
    NewSession,
    ResumeSession(SessionAction),
    ForkSession(SessionAction),
    SelectSubagent(u32),
    RequestDraw,
    Exit,
}

#[allow(dead_code)]
pub(crate) enum AppCommand {
    SubmitPrompt(String),
    CancelActiveTurn,
    ApprovalDecision(ApprovalDecision),
    AnswerQuestion(QuestionAnswer),
    NewSession,
    ResumeSession(SessionAction),
    ForkSession(SessionAction),
    SelectSubagent(u32),
    Exit,
}

pub(crate) struct AppEventSender {
    sender: mpsc::UnboundedSender<AppEvent>,
}

impl AppEventSender {
    pub(crate) fn channel() -> (Self, mpsc::UnboundedReceiver<AppEvent>) {
        let (sender, receiver) = mpsc::unbounded_channel();
        (Self { sender }, receiver)
    }

    pub(crate) fn send(&self, event: AppEvent) -> Result<(), mpsc::error::SendError<AppEvent>> {
        self.sender.send(event)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AppEvent, AppEventSender, ApprovalDecision, Overlay, QuestionAnswer, SessionAction,
    };
    use crate::ui::TuiEvent;

    #[test]
    fn approval_and_submit_events_preserve_payloads() {
        let submit = AppEvent::SubmitPrompt("fix the parser".to_string());
        let approval = AppEvent::ApprovalDecision(ApprovalDecision::Custom("once".to_string()));

        assert!(matches!(submit, AppEvent::SubmitPrompt(prompt) if prompt == "fix the parser"));
        assert!(matches!(
            approval,
            AppEvent::ApprovalDecision(ApprovalDecision::Custom(reason)) if reason == "once"
        ));
        let answer = AppEvent::AnswerQuestion(QuestionAnswer::Custom("later".to_string()));
        assert!(matches!(
            answer,
            AppEvent::AnswerQuestion(QuestionAnswer::Custom(value)) if value == "later"
        ));
    }

    #[test]
    fn control_events_keep_their_distinct_meanings() {
        assert!(matches!(
            AppEvent::CancelActiveTurn,
            AppEvent::CancelActiveTurn
        ));
        assert!(matches!(AppEvent::Exit, AppEvent::Exit));
        assert!(matches!(
            AppEvent::OpenOverlay(Overlay::History),
            AppEvent::OpenOverlay(Overlay::History)
        ));
        assert!(matches!(
            AppEvent::ResumeSession(SessionAction::Latest),
            AppEvent::ResumeSession(SessionAction::Latest)
        ));
    }

    #[tokio::test]
    async fn sender_round_trips_typed_events_without_exposing_channels() {
        let (sender, mut receiver) = AppEventSender::channel();
        sender
            .send(AppEvent::Tui(TuiEvent::Draw))
            .expect("event receiver is still open");

        assert!(matches!(
            receiver.recv().await,
            Some(AppEvent::Tui(TuiEvent::Draw))
        ));
    }
}
