//! UI-neutral contract for frontends that control and observe a RustCode session.

mod apply;
mod attachments;
mod events;
mod native_commands;
mod snapshot;
mod tasks;
mod worker;

use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_APPROVAL_BATCH_ID: AtomicU64 = AtomicU64::new(1);

/// Creates a unique identity for a controller-owned pending approval batch.
pub(crate) fn next_approval_batch_id() -> String {
    format!(
        "controller:{}:{}",
        std::process::id(),
        NEXT_APPROVAL_BATCH_ID.fetch_add(1, Ordering::Relaxed)
    )
}

#[cfg(test)]
mod tests;

pub use apply::{
    apply_approval_decision, apply_approval_decision_for_batch, apply_background_task_event,
    apply_question_answer, spawn_observed_orchestrator,
};
pub use attachments::save_image_attachment;
pub use events::{
    ApprovalChoice, ControllerError, ControllerEvent, ControllerUpdate, TurnUpdate,
    accepts_generation,
};
pub use snapshot::{
    ApprovalAction, ApprovalBatchPrompt, ApprovalPrompt, Command, ControllerHandle,
    ControllerSnapshot, ModelChoice, PendingPrompt, PendingPromptKind, PromptSubmitMode,
    QuestionPrompt, SessionChoice, TranscriptItem,
};
pub use tasks::{
    TaskDisplay, background_command_label, background_task_snapshots, has_background_tasks,
    stop_background_tasks,
};

pub use worker::InteractiveController;
