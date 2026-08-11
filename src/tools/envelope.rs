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
    pub error_kind: Option<String>,
    pub output: String,
    pub truncated: bool,
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolErrorKind {
    InvalidArguments,
    EditMismatch,
    PermissionDenied,
    CommandFailed,
    CompilerFailed,
    Cancelled,
    UnavailableDependency,
    Unknown,
}

impl ToolErrorKind {
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
