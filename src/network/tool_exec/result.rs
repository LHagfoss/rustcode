use crate::app::ChatMessage;

use super::super::events::{ToolResult, ToolResultMetadata};
use super::super::is_mutating_tool;
use super::super::output::truncate_tool_output_for_message;
use super::preview::get_file_preview;
use rustcode_core::ToolResultCompleteness;

pub(crate) fn stable_arguments_hash(arguments: &serde_json::Value) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    arguments.to_string().hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

pub(crate) fn tool_result_from_execution(
    tool_name: &str,
    args: &serde_json::Value,
    execution: crate::tools::ToolExecutionOutput,
    diff: Option<String>,
) -> ToolResult {
    let completeness = if execution.truncated {
        if tool_name == "view_file" {
            ToolResultCompleteness::LineTruncated
        } else {
            ToolResultCompleteness::ByteTruncated
        }
    } else if tool_name == "view_file" && execution.content.contains("end of requested range") {
        ToolResultCompleteness::UserLimited
    } else {
        ToolResultCompleteness::Complete
    };
    let changed_paths = if is_mutating_tool(tool_name) && execution.success {
        args.get("path")
            .or_else(|| args.get("output_path"))
            .and_then(|value| value.as_str())
            .map(|path| vec![path.to_string()])
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    ToolResult {
        tool_name: tool_name.to_string(),
        content: execution.content,
        diff,
        file_preview: get_file_preview(tool_name, args),
        metadata: ToolResultMetadata {
            call_id: None,
            arguments_hash: stable_arguments_hash(args),
            success: execution.success,
            pending: execution.pending,
            command: execution.command,
            exit_code: execution.exit_code,
            changed_paths,
            truncated: execution.truncated,
            completeness,
            full_output_artifact: None,
            replayed: execution.replayed,
            error_kind: if execution.pending || execution.success {
                None
            } else {
                execution.error_kind.or_else(|| {
                    Some(if tool_name == "run_command" {
                        crate::tools::ToolErrorKind::CommandFailed
                    } else {
                        crate::tools::ToolErrorKind::Internal
                    })
                })
            },
            retryable: execution.retryable,
        },
    }
}

pub(crate) fn finalize_tool_result_for_prefix(
    mut result: ToolResult,
    deferred_notice: Option<&str>,
    prefix: &str,
) -> ToolResult {
    if let Some(notice) = deferred_notice {
        result.content.push_str("\n\n");
        result.content.push_str(notice);
    }
    let bounded = truncate_tool_output_for_message(&result.tool_name, result.content, prefix);
    result.content = bounded.content;
    if bounded.truncated {
        result.metadata.truncated = true;
        result.metadata.completeness = ToolResultCompleteness::ByteTruncated;
        result.metadata.full_output_artifact = bounded.full_output_artifact;
        if result.metadata.error_kind.is_none() {
            result.metadata.error_kind = Some(crate::tools::ToolErrorKind::OutputLimit);
        }
    }
    result
}

pub(crate) fn finalize_tool_result(
    result: ToolResult,
    deferred_notice: Option<&str>,
) -> ToolResult {
    let prefix = format!("{}: ", result.tool_name);
    finalize_tool_result_for_prefix(result, deferred_notice, &prefix)
}

pub(crate) fn tool_result_history_message(
    result: ToolResult,
    answered_call: Option<String>,
) -> ChatMessage {
    let prefix = format!("{}: ", result.tool_name);
    tool_result_history_message_with_prefix(result, &prefix, answered_call)
}

pub(crate) fn tool_result_history_message_with_prefix(
    result: ToolResult,
    prefix: &str,
    answered_call: Option<String>,
) -> ChatMessage {
    let envelope = result.execution_envelope();
    let ToolResult {
        tool_name,
        content,
        diff,
        file_preview,
        metadata,
    } = result;
    ChatMessage::new("tool", format!("{prefix}{content}"))
        .answering(answered_call)
        .with_diff(diff)
        .with_file_preview(file_preview)
        .with_tool_result(crate::app::ToolResultRecord {
            tool_name,
            arguments_hash: metadata.arguments_hash,
            success: envelope.success,
            pending: envelope.pending,
            command: envelope.command,
            exit_code: envelope.exit_code,
            changed_paths: envelope.changed_paths,
            truncated: envelope.truncated,
            completeness: envelope.completeness,
            full_output_artifact: envelope.full_output_artifact,
            error_kind: envelope.error_kind.map(|kind| kind.as_str().to_string()),
            retryable: envelope.retryable,
            replayed: envelope.replayed,
        })
}

pub(crate) fn bounded_tool_result_history_message(
    result: ToolResult,
    prefix: &str,
    answered_call: Option<String>,
) -> ChatMessage {
    let result = finalize_tool_result_for_prefix(result, None, prefix);
    tool_result_history_message_with_prefix(result, prefix, answered_call)
}

pub(crate) fn subagent_tool_history_message(
    tool_name: &str,
    args: &serde_json::Value,
    execution: crate::tools::ToolExecutionOutput,
    diff: Option<String>,
    answered_call: Option<String>,
) -> ChatMessage {
    let prefix = format!("{tool_name}: ");
    bounded_tool_result_history_message(
        tool_result_from_execution(tool_name, args, execution, diff),
        &prefix,
        answered_call,
    )
}
