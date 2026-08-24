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
    pub pending: bool,
    pub command: Option<String>,
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
    /// Stable spelling used in persisted `ToolResultRecord` values. Keep this
    /// explicit instead of relying on Debug formatting so persistence does not
    /// accidentally change when enum formatting changes.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Validation => "Validation",
            Self::InvalidArguments => "InvalidArguments",
            Self::EditMismatch => "EditMismatch",
            Self::PermissionDenied => "PermissionDenied",
            Self::CommandFailed => "CommandFailed",
            Self::CompilerFailed => "CompilerFailed",
            Self::Cancelled => "Cancelled",
            Self::McpFailed => "McpFailed",
            Self::Internal => "Internal",
            Self::OutputLimit => "OutputLimit",
            Self::ProviderFailed => "ProviderFailed",
            Self::UnavailableDependency => "UnavailableDependency",
            Self::Unknown => "Unknown",
        }
    }

    /// Parse the stable persisted spelling. Unknown future values remain in
    /// the JSON string and simply do not become a falsely known enum.
    pub fn from_persisted(value: &str) -> Option<Self> {
        Some(match value {
            "Validation" => Self::Validation,
            "InvalidArguments" => Self::InvalidArguments,
            "EditMismatch" => Self::EditMismatch,
            "PermissionDenied" => Self::PermissionDenied,
            "CommandFailed" => Self::CommandFailed,
            "CompilerFailed" => Self::CompilerFailed,
            "Cancelled" => Self::Cancelled,
            "McpFailed" => Self::McpFailed,
            "Internal" => Self::Internal,
            "OutputLimit" => Self::OutputLimit,
            "ProviderFailed" => Self::ProviderFailed,
            "UnavailableDependency" => Self::UnavailableDependency,
            "Unknown" => Self::Unknown,
            _ => return None,
        })
    }

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
