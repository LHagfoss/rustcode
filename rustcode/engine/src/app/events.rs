#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalDecision {
    Approve,
    ApproveAndRemember(String),
    ForbidAndRemember(String),
    Deny,
    ApproveAll,
    Custom(String),
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuestionAnswer {
    Selected(String),
    Custom(String),
    Cancelled,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateDecision {
    UpdateNow,
    Skip,
    SkipUntilNextVersion,
}

#[allow(dead_code)]
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

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionAction {
    Latest,
    Id(String),
}

#[allow(dead_code)]
pub(crate) enum AppCommand {
    SubmitPrompt(String),
    CancelActiveTurn,
    ApprovalDecision(ApprovalDecision),
    AnswerQuestion(QuestionAnswer),
    ClearSession,
    ArchiveSession,
    DeleteSession(SessionAction),
    NewSession,
    ResumeSession(SessionAction),
    ForkSession(SessionAction),
    SelectSubagent(u32),
    Exit,
}
