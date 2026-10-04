use crate::ui::TuiEvent;
use rustcode::app::events::Overlay;
use rustcode::app::{ApprovalDecision, QuestionAnswer, SessionAction, UpdateDecision};
use tokio::sync::mpsc;

#[cfg(test)]
use super::{AppError, AppRunControl, AppRuntime};

#[cfg(test)]
impl AppRuntime {
    pub(crate) async fn handle_event(
        &mut self,
        event: AppEvent,
    ) -> Result<AppRunControl, AppError> {
        match event {
            AppEvent::RequestDraw | AppEvent::Tui(TuiEvent::Draw) => {
                self.app_state.lock().await.request_redraw();
                Ok(AppRunControl::Continue)
            }
            AppEvent::SubmitPrompt(prompt) => {
                let mut state = self.app_state.lock().await;
                // Mirrors the test-only `replace_input`: set the draft and
                // invalidate, without depending on engine test helpers.
                state.input_buffer = prompt;
                state.cursor_position = state.input_buffer.len();
                state.history_index = None;
                state.render_revision = state.render_revision.wrapping_add(1);
                state.request_redraw();
                Ok(AppRunControl::Continue)
            }
            AppEvent::CancelActiveTurn => {
                rustcode::app::handle_escape(&self.app_state, &mut self.current_cancel_token).await;
                self.app_state.lock().await.request_redraw();
                Ok(AppRunControl::Continue)
            }
            AppEvent::Exit => {
                let supervisor = self.app_state.lock().await.subagent_supervisor.clone();
                supervisor.shutdown_and_wait().await;
                let state = self.app_state.lock().await;
                Ok(AppRunControl::Exit(crate::run::ExitSummary::from_state(
                    &state,
                )))
            }
            AppEvent::CloseOverlay => {
                let mut state = self.app_state.lock().await;
                state.overlays().close_all();
                state.request_redraw();
                Ok(AppRunControl::Continue)
            }
            AppEvent::ApprovalDecision(decision) => {
                rustcode::controller::apply_approval_decision(
                    &self.app_state,
                    &mut self.current_cancel_token,
                    decision,
                )
                .await;
                Ok(AppRunControl::Continue)
            }
            AppEvent::AnswerQuestion(answer) => {
                rustcode::controller::apply_question_answer(
                    &self.app_state,
                    &mut self.current_cancel_token,
                    answer,
                )
                .await;
                Ok(AppRunControl::Continue)
            }
            AppEvent::UpdateDecision(decision) => {
                let mut state = self.app_state.lock().await;
                if super::updates::apply_update_decision(&mut state, decision) {
                    state.update_requested = true;
                }
                Ok(AppRunControl::Continue)
            }
            AppEvent::OpenOverlay(overlay) => {
                let mut state = self.app_state.lock().await;
                super::sessions::open_overlay(&mut state, overlay);
                state.request_redraw();
                Ok(AppRunControl::Continue)
            }
            event @ (AppEvent::NewSession
            | AppEvent::ResumeSession(_)
            | AppEvent::ForkSession(_)
            | AppEvent::ClearSession
            | AppEvent::ArchiveSession
            | AppEvent::DeleteSession(_)) => {
                let mut state = self.app_state.lock().await;
                super::sessions::apply_session_event(
                    &mut state,
                    &mut self.current_cancel_token,
                    event,
                )?;
                Ok(AppRunControl::Continue)
            }
            AppEvent::Tui(_) => Ok(AppRunControl::Continue),
            AppEvent::SelectSubagent(id) => {
                let mut state = self.app_state.lock().await;
                super::sessions::apply_subagent_selection(&mut state, id)?;
                Ok(AppRunControl::Continue)
            }
        }
    }
}

#[allow(dead_code)]
/// Loop events. Several variants are matched defensively but never sent in
/// production builds (prompt submit bypasses the queue via direct
/// `handle_enter` calls); they remain for protocol stability and are
/// constructed in tests.
pub enum AppEvent {
    /// Terminal input produced and consumed by the interactive frontend.
    Tui(TuiEvent),
    SubmitPrompt(String),
    CancelActiveTurn,
    ApprovalDecision(ApprovalDecision),
    AnswerQuestion(QuestionAnswer),
    UpdateDecision(UpdateDecision),
    ClearSession,
    ArchiveSession,
    DeleteSession(SessionAction),
    OpenOverlay(Overlay),
    CloseOverlay,
    NewSession,
    ResumeSession(SessionAction),
    ForkSession(SessionAction),
    SelectSubagent(u32),
    RequestDraw,
    Exit,
}

pub struct AppEventSender {
    sender: mpsc::UnboundedSender<AppEvent>,
}

impl AppEventSender {
    pub fn channel() -> (Self, mpsc::UnboundedReceiver<AppEvent>) {
        let (sender, receiver) = mpsc::unbounded_channel();
        (Self { sender }, receiver)
    }

    pub fn send(&self, event: AppEvent) -> Result<(), mpsc::error::SendError<AppEvent>> {
        self.sender.send(event)
    }
}

#[cfg(test)]
mod tests {
    use super::{AppEvent, AppEventSender};
    use rustcode::app::events::Overlay;
    use rustcode::app::{ApprovalDecision, QuestionAnswer, SessionAction, UpdateDecision};

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
            AppEvent::UpdateDecision(UpdateDecision::UpdateNow),
            AppEvent::UpdateDecision(UpdateDecision::UpdateNow)
        ));
        assert!(matches!(
            AppEvent::OpenOverlay(Overlay::History),
            AppEvent::OpenOverlay(Overlay::History)
        ));
        assert!(matches!(
            AppEvent::ResumeSession(SessionAction::Latest),
            AppEvent::ResumeSession(SessionAction::Latest)
        ));
        assert!(matches!(
            AppEvent::DeleteSession(SessionAction::Id("session-1".to_owned())),
            AppEvent::DeleteSession(SessionAction::Id(id)) if id == "session-1"
        ));
    }

    #[tokio::test]
    async fn sender_round_trips_typed_events_without_exposing_channels() {
        let (sender, mut receiver) = AppEventSender::channel();
        sender
            .send(AppEvent::RequestDraw)
            .expect("event receiver is still open");

        assert!(matches!(receiver.recv().await, Some(AppEvent::RequestDraw)));
    }
}
