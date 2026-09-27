//! UI-neutral contract for frontends that control and observe a RustCode session.

mod attachments;
mod events;
mod native_commands;
mod snapshot;
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

pub use worker::InteractiveController;
