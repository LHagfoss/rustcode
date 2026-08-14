/// Typed tool call/result envelope — preserves call IDs end-to-end.
/// ApiNative calls never go through fenced Markdown internally.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCallEnvelope {
    pub call_id: String,
    pub tool_name: String,
    pub arguments: serde_json::Value,
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq)]
pub struct ToolResultEnvelope {
    pub call_id: String,
    pub tool_name: String,
    pub success: bool,
    pub error_kind: Option<ToolErrorKind>,
    pub retryable: bool,
    pub exit_code: Option<i32>,
    pub changed_paths: Vec<String>,
    pub output: String,
    pub truncated: bool,
    pub full_output_artifact: Option<String>,
    pub replayed: bool,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolErrorKind {
    Validation,
    InvalidArguments,
    EditMismatch,
    PermissionDenied,
    CommandFailed,
    CompilerFailed,
    Cancelled,
    McpFailed,
    Internal,
    OutputLimit,
    ProviderFailed,
    UnavailableDependency,
    Unknown,
}

impl ToolErrorKind {
    /// Compatibility helper for legacy display-only callers. Execution paths
    /// should set this kind at the boundary from typed state instead of parsing
    /// human-facing prose.
    #[allow(dead_code)]
    pub fn from_message(msg: &str) -> Self {
        let lower = msg.to_ascii_lowercase();
        if lower.contains("missing") || lower.contains("invalid argument") {
            Self::InvalidArguments
        } else if lower.contains("edit") && lower.contains("mismatch") {
            Self::EditMismatch
        } else if lower.contains("permission") || lower.contains("denied") {
            Self::PermissionDenied
        } else if lower.contains("compiler") || lower.contains("cargo check") {
            Self::CompilerFailed
        } else if lower.contains("cancelled") {
            Self::Cancelled
        } else if lower.contains("not found") || lower.contains("unavailable") {
            Self::UnavailableDependency
        } else if lower.contains("exit code") {
            Self::CommandFailed
        } else {
            Self::Unknown
        }
    }
}

#[allow(dead_code)]
pub fn is_api_native(url: &str, protocol: crate::config::ToolProtocol) -> bool {
    matches!(protocol, crate::config::ToolProtocol::ApiNative)
        || crate::config::provider_supports_function_calling(url)
}
