use crate::app::ChatMessage;

use super::super::events::{ToolResult, ToolResultMetadata};
use super::super::is_mutating_tool;
use super::super::output::{INCOMPLETE_TOOL_RESULT_MARKER, truncate_tool_output_for_message};
use super::preview::get_file_preview;
use rustcode_core::{InspectionRange, InspectionResultMetadata, ToolResultCompleteness};

fn parse_view_header(content: &str) -> Option<(String, u64, u64, u64)> {
    let rest = content.strip_prefix("[File: ")?;
    let (path, rest) = rest.split_once(", Lines ")?;
    let (range, rest) = rest.split_once(" of ")?;
    let (start, end) = range.split_once(" to ")?;
    let total = rest.split_once(',')?.0;
    Some((
        path.to_string(),
        start.parse().ok()?,
        end.parse().ok()?,
        total.parse().ok()?,
    ))
}

fn inspection_result_metadata(
    tool_name: &str,
    args: &serde_json::Value,
    content: &str,
    completeness: ToolResultCompleteness,
) -> Option<InspectionResultMetadata> {
    let fingerprint = if crate::network::loop_detect::inspection_target(tool_name, args).is_some()
        || crate::network::loop_detect::is_read_only(tool_name)
    {
        // Use the detector's category rather than the exact range identity so
        // native reads and equivalent shell reads share one stable fingerprint.
        crate::network::loop_detect::signatures(tool_name, args).1
    } else {
        return None;
    };
    let read_target = crate::network::loop_detect::read_target(tool_name, args);
    let requested_path = args
        .get("path")
        .and_then(|value| value.as_str())
        .map(str::to_owned)
        .or_else(|| read_target.as_ref().map(|(path, _, _)| path.clone()));
    let requested_range = read_target.map(|(_, start, end)| InspectionRange {
        start: Some(start as u64),
        end: end.map(|value| value as u64),
    });
    let parsed = parse_view_header(content);
    let returned_path = parsed
        .as_ref()
        .map(|(path, _, _, _)| path.clone())
        .or_else(|| requested_path.clone());
    let returned_range = parsed.as_ref().map(|(_, start, end, _)| InspectionRange {
        start: Some(*start),
        end: Some(*end),
    });
    let complete = matches!(
        completeness,
        ToolResultCompleteness::Complete | ToolResultCompleteness::UserLimited
    );
    let next_range = (!complete)
        .then(|| {
            parsed.as_ref().and_then(|(_, _, end, total)| {
                (*end < *total).then_some(InspectionRange {
                    start: Some(end + 1),
                    end: Some(*total),
                })
            })
        })
        .flatten();
    Some(InspectionResultMetadata {
        requested_path,
        requested_range,
        returned_path,
        returned_range,
        complete,
        next_range,
        delivered_ranges: Vec::new(),
        fingerprint,
    })
}

fn numbered_lines(content: &str) -> Vec<(u64, String)> {
    content
        .lines()
        .filter_map(|line| {
            let (number, text) = line.split_once(": ")?;
            Some((number.parse().ok()?, text.to_string()))
        })
        .collect()
}

fn contiguous_ranges(numbers: &[u64]) -> Vec<InspectionRange> {
    let mut sorted = numbers.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let Some(mut start) = sorted.first().copied() else {
        return Vec::new();
    };
    let mut end = start;
    let mut ranges = Vec::new();
    for number in sorted.into_iter().skip(1) {
        if number == end.saturating_add(1) {
            end = number;
        } else {
            ranges.push(InspectionRange {
                start: Some(start),
                end: Some(end),
            });
            start = number;
            end = number;
        }
    }
    ranges.push(InspectionRange {
        start: Some(start),
        end: Some(end),
    });
    ranges
}

/// Reconcile inspection ranges with the output that actually reached the
/// provider. Final byte bounding can retain a head and tail, or cut through a
/// long numbered source line, so the original view-file header is no longer a
/// truthful returned range.
fn reconcile_truncated_inspection(
    inspection: &mut InspectionResultMetadata,
    original_content: &str,
    delivered_content: &str,
) {
    let original_by_number = numbered_lines(original_content)
        .into_iter()
        .collect::<std::collections::HashMap<_, _>>();
    let delivered_lines = numbered_lines(delivered_content);

    // A numbered line is complete only when its text still exactly matches
    // the pre-bounded output. A mismatch means byte bounding cut through that
    // line; it must be requested again rather than advertised as returned.
    let complete_numbers = delivered_lines
        .iter()
        .filter_map(|(number, text)| {
            original_by_number
                .get(number)
                .filter(|original| *original == text)
                .map(|_| *number)
        })
        .collect::<Vec<_>>();
    let ranges = contiguous_ranges(&complete_numbers);
    inspection.delivered_ranges = ranges.clone();
    inspection.returned_range = ranges.first().cloned();

    let Some((_, original_start, original_end, total)) = parse_view_header(original_content) else {
        // Non-view inspection tools do not have line-oriented continuation
        // metadata, but they should never retain a stale range.
        inspection.next_range = None;
        return;
    };
    let source_start = original_start;
    let source_end = original_end.min(total);

    // Find the first omitted source line without iterating across a potentially
    // very large file. This also handles a head/tail bounded result where the
    // middle is absent from the model-facing payload.
    let mut cursor = source_start;
    let mut next_start = None;
    let mut next_end = source_end;
    for range in &ranges {
        let (Some(start), Some(end)) = (range.start, range.end) else {
            continue;
        };
        if end < cursor || start > source_end {
            continue;
        }
        if start > cursor {
            next_start = Some(cursor);
            next_end = start.saturating_sub(1).min(source_end);
            break;
        }
        cursor = end.saturating_add(1);
        if cursor > source_end {
            break;
        }
    }
    if next_start.is_none() && cursor <= source_end {
        next_start = Some(cursor);
    }

    inspection.next_range = next_start.map(|start| InspectionRange {
        start: Some(start),
        end: Some(next_end),
    });
}

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
    // The execution layer owns completeness. Preserve the more specific
    // filesystem classification and only strengthen a legacy/ambiguous
    // `truncated` bit; never inspect human-facing output text here.
    let completeness = if execution.truncated {
        match execution.completeness {
            ToolResultCompleteness::Complete | ToolResultCompleteness::UserLimited => {
                ToolResultCompleteness::ByteTruncated
            }
            completeness => completeness,
        }
    } else {
        execution.completeness
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
    let inspection = inspection_result_metadata(tool_name, args, &execution.content, completeness);
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
            inspection,
        },
    }
}

pub(crate) fn finalize_tool_result_for_prefix(
    mut result: ToolResult,
    deferred_notice: Option<&str>,
    prefix: &str,
) -> ToolResult {
    normalize_incomplete_metadata(&mut result);
    if let Some(notice) = deferred_notice {
        result.content.push_str("\n\n");
        result.content.push_str(notice);
    }
    let original_content = result.content.clone();
    let bounded = truncate_tool_output_for_message(&result.tool_name, result.content, prefix);
    result.content = bounded.content;
    if bounded.truncated {
        result.metadata.truncated = true;
        result.metadata.completeness = ToolResultCompleteness::ByteTruncated;
        if result.metadata.full_output_artifact.is_none() {
            result.metadata.full_output_artifact = bounded.full_output_artifact;
        }
        if result.metadata.error_kind.is_none() {
            result.metadata.error_kind = Some(crate::tools::ToolErrorKind::OutputLimit);
        }
        if let Some(inspection) = result.metadata.inspection.as_mut() {
            inspection.complete = false;
            reconcile_truncated_inspection(inspection, &original_content, &result.content);
        }
    }
    normalize_incomplete_metadata(&mut result);
    result
}

/// Keep the model-facing transcript honest even when a result was bounded by
/// a lower-level tool or reconstructed from an older/replayed history record.
/// The typed field is authoritative, while this compact marker makes the same
/// fact unambiguous in providers that reason primarily from result text.
fn normalize_incomplete_metadata(result: &mut ToolResult) {
    let completeness = if result.metadata.truncated
        && result.metadata.completeness == ToolResultCompleteness::Complete
    {
        ToolResultCompleteness::ByteTruncated
    } else {
        result.metadata.completeness
    };
    result.metadata.completeness = completeness;
    if matches!(
        completeness,
        ToolResultCompleteness::LineTruncated | ToolResultCompleteness::ByteTruncated
    ) {
        result.metadata.truncated = true;
        if !result.content.contains(INCOMPLETE_TOOL_RESULT_MARKER) {
            result.content.push_str(&format!(
                "\n\n{INCOMPLETE_TOOL_RESULT_MARKER} completeness={}; content is partial and must not be treated as complete.]",
                completeness.as_str()
            ));
        }
    }
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
    mut result: ToolResult,
    prefix: &str,
    answered_call: Option<String>,
) -> ChatMessage {
    normalize_incomplete_metadata(&mut result);
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
            inspection: envelope.inspection,
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
