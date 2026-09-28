//! UI-neutral contract for frontends that control and observe a RustCode session.

mod apply;
mod attachments;
mod config;
mod context;
mod events;
mod input;
mod native_commands;
mod snapshot;
mod tasks;
mod transcript;
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
pub use config::{
    AgentMode, AppConfig, ModelProfile, SessionMeta, ToolProtocol, config_dir, save_config,
};
pub use context::{
    SkillInfo, discover_skills, estimate_message_tokens, estimate_tokens, tool_system_prompt,
};
pub use events::{
    ApprovalChoice, ControllerError, ControllerEvent, ControllerUpdate, TurnUpdate,
    accepts_generation,
};
pub use input::{
    ActivityKind, ActivitySnapshot, CommandInfo, build_help_text, classify_activity,
    classify_live_tools, command_token, filtered_commands, get_completion_len,
    list_project_file_paths, summarize_tool_call,
};
pub use snapshot::{
    ApprovalAction, ApprovalBatchPrompt, ApprovalPrompt, Command, ControllerHandle,
    ControllerSnapshot, DraftSubmitMode, ModelChoice, PendingPrompt, PendingPromptKind,
    PendingQuestion, PromptSubmitMode, QuestionPrompt, SessionChoice, TranscriptItem,
};
pub use tasks::{
    TaskDisplay, background_command_label, background_task_snapshots, has_background_tasks,
    stop_background_tasks,
};
pub use transcript::{
    AgentUiEvent, SubAgentStatus, TokenUsage, Verbosity, mcp_tool_display_name,
    sanitize_recap_content,
};

pub use worker::InteractiveController;
