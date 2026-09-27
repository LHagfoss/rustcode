//! Compatibility facade for the protocol parser.
//!
//! Parsing is dependency-neutral and lives in `rustcode-tool-protocol`; this
//! module keeps the existing `crate::tools` API and supplies the root tool
//! registry when producing schema-aware diagnostics.

#[cfg(test)]
pub(crate) use rustcode_tool_protocol::repair_json;
pub use rustcode_tool_protocol::{
    has_incomplete_actionable_tool_call, parse_tool_call, parse_tool_calls,
};

pub fn diagnose_failed_tool_call(text: &str) -> Option<String> {
    if let Some(diagnostic) = rustcode_tool_protocol::diagnose_reasoning_leakage(text) {
        return Some(diagnostic);
    }
    rustcode_tool_protocol::diagnose_failed_tool_call_with_validator(text, |calls| {
        super::validate_tool_calls(
            calls,
            crate::config::DEFAULT_MAX_MUTATING_CALLS_PER_RESPONSE,
        )
    })
}
