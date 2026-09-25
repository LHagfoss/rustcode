//! UI-neutral contract for frontends that control and observe a RustCode session.

mod attachments;
mod events;
mod native_commands;
mod snapshot;
mod worker;

#[cfg(test)]
mod tests;

pub use attachments::save_image_attachment;
pub use events::{
    ApprovalChoice, ControllerError, ControllerEvent, ControllerUpdate, TurnUpdate,
    accepts_generation,
};
pub use snapshot::{
    ApprovalPrompt, Command, ControllerHandle, ControllerSnapshot, ModelChoice, QuestionPrompt,
    SessionChoice, TranscriptItem,
};

pub use worker::InteractiveController;
