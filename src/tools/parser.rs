//! Compatibility facade for the protocol parser.
//!
//! Parsing is dependency-neutral and lives in `rustcode-tool-protocol`; this
//! module keeps the existing `crate::tools` API and supplies the root tool
//! registry when producing schema-aware diagnostics.

pub use rustcode_tool_protocol::{
    is_code_editing_tool, is_tool_call_start, parse_tool_call, parse_tool_calls,
};

pub(crate) use rustcode_tool_protocol::{find_closing_tool_fence, repair_json};

pub fn diagnose_failed_tool_call(text: &str) -> Option<String> {
    rustcode_tool_protocol::diagnose_failed_tool_call_with_validator(text, |calls| {
        super::validate_tool_calls(
            calls,
            crate::config::DEFAULT_MAX_MUTATING_CALLS_PER_RESPONSE,
        )
    })
}
