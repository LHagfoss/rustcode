//! What a tool call returns: its display text and the facts beside it.

/// Authoritative facts returned by a tool invocation alongside its display
/// text. Consumers must not reconstruct these fields from `content`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolExecutionOutput {
    pub content: String,
    pub success: bool,
    pub pending: bool,
    pub command: Option<String>,
    pub exit_code: Option<i32>,
    pub truncated: bool,
    /// Machine-readable completeness of the output delivered by the tool.
    /// This must be set by the execution layer, never inferred from display text.
    pub completeness: rustcode_core::ToolResultCompleteness,
    /// True when the harness served a bounded cached read instead of running
    /// the tool again. This is execution state, not display prose.
    pub replayed: bool,
    pub error_kind: Option<rustcode_core::ToolErrorKind>,
    pub retryable: bool,
    pub command_status: Option<rustcode_core::CommandResultMetadata>,
}

impl ToolExecutionOutput {
    pub fn success(content: String) -> Self {
        Self {
            content,
            success: true,
            pending: false,
            command: None,
            exit_code: None,
            truncated: false,
            completeness: rustcode_core::ToolResultCompleteness::Complete,
            replayed: false,
            error_kind: None,
            retryable: false,
            command_status: None,
        }
    }

    pub fn failure(content: String) -> Self {
        Self {
            content,
            success: false,
            pending: false,
            command: None,
            exit_code: None,
            truncated: false,
            completeness: rustcode_core::ToolResultCompleteness::Complete,
            replayed: false,
            error_kind: Some(rustcode_core::ToolErrorKind::Internal),
            retryable: false,
            command_status: None,
        }
    }

    pub fn failure_with_kind(
        content: String,
        error_kind: rustcode_core::ToolErrorKind,
        retryable: bool,
    ) -> Self {
        Self {
            error_kind: Some(error_kind),
            retryable,
            ..Self::failure(content)
        }
    }
}
