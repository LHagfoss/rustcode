/// Typed tool call/result envelope — preserves call IDs end-to-end.
/// ApiNative calls never go through fenced Markdown internally.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCallEnvelope {
    pub call_id: String,
    pub tool_name: String,
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolResultEnvelope {
    pub call_id: String,
    pub tool_name: String,
    pub success: bool,
    pub error_kind: Option<String>,
    pub output: String,
    pub truncated: bool,
}

pub fn is_api_native(url: &str, protocol: crate::config::ToolProtocol) -> bool {
    matches!(protocol, crate::config::ToolProtocol::ApiNative) || crate::config::provider_supports_function_calling(url)
}
