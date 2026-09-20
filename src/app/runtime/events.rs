use crate::app::{AppState, ApprovalDecision, QuestionAnswer};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use super::{AppError, AppRunControl, AppRuntime};
use crate::app::AppEvent;
use crate::ui::TuiEvent;

pub(super) async fn apply_approval_decision(
    state: &Arc<Mutex<AppState>>,
    cancel_token: &mut CancellationToken,
    decision: ApprovalDecision,
) {
    let approved = match decision {
        ApprovalDecision::Approve => true,
        ApprovalDecision::ApproveAll => {
            state.lock().await.auto_confirm = true;
            true
        }
        ApprovalDecision::Deny => false,
        ApprovalDecision::Custom(reason) => !reason.trim().is_empty(),
    };
    if !approved {
        cancel_token.cancel();
        *cancel_token = CancellationToken::new();
    }
    let mut state = state.lock().await;
    if let Some(tx) = state.tool_confirmation_response.take() {
        let _ = tx.send(approved);
    }
    state.pending_tool_confirmation = None;
    state.request_redraw();
}

pub(super) async fn apply_question_answer(
    state: &Arc<Mutex<AppState>>,
    cancel_token: &mut CancellationToken,
    answer: QuestionAnswer,
) {
    if matches!(answer, QuestionAnswer::Cancelled) {
        cancel_token.cancel();
        *cancel_token = CancellationToken::new();
        let mut state = state.lock().await;
        if let Some(tx) = state.question_response.take() {
            let _ = tx.send("User cancelled prompt.".to_owned());
        }
        state.clear_question_chain();
        state.enter_idle();
        state.request_redraw();
        return;
    }
    let current = match answer {
        QuestionAnswer::Selected(answer) | QuestionAnswer::Custom(answer) => answer,
        QuestionAnswer::Cancelled => unreachable!("cancelled handled above"),
    };
    let mut state = state.lock().await;
    if !state.pending_question_queue.is_empty() {
        // More questions remain in the chain: record this answer, advance to
        // the next question, and keep waiting — the tool call resolves only
        // once every question is answered (or the chain is cancelled).
        state.advance_question_chain(current);
        state.request_redraw();
        return;
    }
    let answers = state.take_question_chain_answers(Some(current.clone()));
    // A stale event with no active chain (e.g. a double Enter racing the
    // submit) falls back to the raw answer instead of an empty submission.
    let output = if answers.is_empty() {
        format!("User selected: {current}")
    } else {
        crate::app::state::format_question_chain_answers(&answers)
    };
    if let Some(tx) = state.question_response.take() {
        let _ = tx.send(output);
    }
    state.request_redraw();
}

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
                state.composer().replace_input(prompt);
                state.request_redraw();
                Ok(AppRunControl::Continue)
            }
            AppEvent::CancelActiveTurn => {
                self.current_cancel_token.cancel();
                self.current_cancel_token = CancellationToken::new();
                let mut state = self.app_state.lock().await;
                state.invalidate_orchestrator();
                state.pending_queue.clear();
                state.background_turn_context = None;
                state.clear_active_turn_projection();
                state.enter_idle();
                state.request_redraw();
                Ok(AppRunControl::Continue)
            }
            AppEvent::Exit => {
                let state = self.app_state.lock().await;
                state.subagent_supervisor.shutdown();
                Ok(AppRunControl::Exit(crate::ExitSummary::from_state(&state)))
            }
            AppEvent::CloseOverlay => {
                let mut state = self.app_state.lock().await;
                state.overlays().close_all();
                state.request_redraw();
                Ok(AppRunControl::Continue)
            }
            AppEvent::ApprovalDecision(decision) => {
                apply_approval_decision(&self.app_state, &mut self.current_cancel_token, decision)
                    .await;
                Ok(AppRunControl::Continue)
            }
            AppEvent::AnswerQuestion(answer) => {
                apply_question_answer(&self.app_state, &mut self.current_cancel_token, answer)
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
