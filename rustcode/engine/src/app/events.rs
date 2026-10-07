#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalDecision {
    Approve,
    ApproveAndRemember(String),
    ForbidAndRemember(String),
    Deny,
    ApproveAll,
    Custom(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuestionAnswer {
    Selected(String),
    Custom(String),
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateDecision {
    UpdateNow,
    Skip,
    SkipUntilNextVersion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overlay {
    CommandPalette,
    History,
    Model,
    Theme,
    McpConfig,
    Verbosity,
    Thinking,
    Effort,
    Protocol,
    Yolo,
    ToolConfirmation,
    Question,
    Subagents,
    Context,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionAction {
    Latest,
    Id(String),
}
