//! Compatibility facade for lifecycle values now owned by the neutral crate.
//!
//! Keeping this module preserves the existing `crate::network::lifecycle::*`
//! paths while allowing lifecycle state and formatting to be reused by other
//! workspace crates without networking dependencies.

pub(crate) use rustcode_lifecycle::{
    StopReason, StreamFailure, StreamFailureKind, TurnLifecycle, final_transcript_content,
    is_unavailable_tool_error, stop_reason_for_stream_failure, stream_failure_kind_from_message,
};
