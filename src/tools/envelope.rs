/// Typed tool call/result envelope — preserves call IDs end-to-end.
/// ApiNative calls never go through fenced Markdown internally.
use rustcode_core::ToolErrorKind;
use rustcode_core::ToolResultCompleteness;

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
    pub completeness: ToolResultCompleteness,
    pub full_output_artifact: Option<String>,
    pub replayed: bool,
}

#[allow(dead_code)]
pub fn is_api_native(url: &str, protocol: crate::config::ToolProtocol) -> bool {
    matches!(protocol, crate::config::ToolProtocol::ApiNative)
        || crate::config::provider_supports_function_calling(url)
}
