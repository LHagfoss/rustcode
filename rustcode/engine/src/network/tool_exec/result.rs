use crate::app::ChatMessage;
use sha2::Digest;

use super::super::events::{ToolResult, ToolResultMetadata};
use super::super::output::{
    COMPLETED_MUTATION_MARKER, COMPLETED_MUTATION_NOTICE, INCOMPLETE_TOOL_RESULT_MARKER,
    sanitize_tool_output, save_full_tool_output, truncate_tool_output_for_message_with_completion,
};
use super::super::{is_mutating_tool, mutation_made_progress};
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
        || (crate::network::loop_detect::is_read_only(tool_name) && tool_name != "use_skill")
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
    canonical_json(arguments).hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn canonical_json(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Object(object) => {
            let mut entries = object.iter().collect::<Vec<_>>();
            entries.sort_by(|(left, _), (right, _)| left.cmp(right));
            let body = entries
                .into_iter()
                .map(|(key, value)| {
                    format!(
                        "{}:{}",
                        serde_json::to_string(key).unwrap_or_default(),
                        canonical_json(value)
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            format!("{{{body}}}")
        }
        serde_json::Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(canonical_json)
                .collect::<Vec<_>>()
                .join(",")
        ),
        primitive => primitive.to_string(),
    }
}

fn compact_reference_path(path: &str) -> String {
    const MAX_PATH_CHARS: usize = 256;
    if path.len() <= MAX_PATH_CHARS {
        return path.to_string();
    }
    let mut bounded = path[..path.floor_char_boundary(MAX_PATH_CHARS)].to_string();
    bounded.push_str("...");
    bounded
}

/// Render the model-facing payload for an unchanged exact repeat. The original
/// body remains in durable history and the replay carries only a compact,
/// explicit status. Re-emitting the body here makes a model more likely to
/// mistake a replay for a fresh or truncated read and start another inspection
/// loop.
pub(crate) fn compact_replayed_read_result(
    tool_name: &str,
    args: &serde_json::Value,
    previous_content: Option<&str>,
) -> String {
    let range = previous_content
        .and_then(parse_view_header)
        .map(|(path, start, end, total)| {
            format!(
                "{} Lines {start} to {end} of {total}",
                compact_reference_path(&path)
            )
        })
        .or_else(|| {
            crate::network::loop_detect::read_target(tool_name, args).map(|(path, start, end)| {
                format!(
                    "{} Lines {start} to {}",
                    compact_reference_path(&path),
                    end.map_or_else(|| "end".to_string(), |end| end.to_string())
                )
            })
        })
        .unwrap_or_else(|| "the same exact read arguments".to_string());
    let fingerprint = stable_arguments_hash(args);

    format!("[Unchanged read replay: tool={tool_name}; fingerprint={fingerprint}; range={range}. ")
        + "The earlier result is complete and remains the canonical evidence in history; "
        + "do not repeat this read. Request a different start_line/end_line range or use grep "
        + "for new evidence.]"
}

/// Re-render a covered `view_file` subrange from a cached complete read.
///
/// Exact repeats intentionally stay compact, but a different range inside a
/// cached read is a legitimate request for evidence the model may need for an
/// edit. Returning only an unchanged-replay notice in that case can strand the
/// model with no usable source text, especially after the surrounding history
/// has been projected or compacted.
pub(crate) fn replay_cached_view_file_subrange(
    tool_name: &str,
    args: &serde_json::Value,
    previous_content: Option<&str>,
) -> Option<String> {
    if tool_name != "view_file" {
        return None;
    }
    let previous_content = previous_content?;
    let (path, requested_start, requested_end) =
        crate::network::loop_detect::read_target(tool_name, args)?;
    let requested_end = requested_end? as u64;
    let (stored_path, stored_start, stored_end, total) = parse_view_header(previous_content)?;
    if path != stored_path
        || (requested_start as u64) < stored_start
        || requested_end > stored_end
        || requested_end < requested_start as u64
    {
        return None;
    }

    let numbered_lines = previous_content
        .lines()
        .filter_map(|line| {
            let (number, _) = line.split_once(": ")?;
            Some((number.parse::<u64>().ok()?, line))
        })
        .collect::<std::collections::HashMap<_, _>>();
    if (requested_start as u64..=requested_end).any(|number| !numbered_lines.contains_key(&number))
    {
        return None;
    }

    let header = previous_content.lines().next()?;
    let old_range = format!("Lines {stored_start} to {stored_end}");
    let new_range = format!("Lines {} to {}", requested_start, requested_end.min(total));
    let header = header.replacen(&old_range, &new_range, 1);
    let requested_end = requested_end.min(total);
    let completeness = if requested_end == total {
        "[Read complete: all lines in the requested range were delivered; no continuation is needed.]"
    } else {
        "[Read complete for the requested range; the file continues beyond this range.]"
    };

    let mut replay = format!("{header}\n{completeness}\n");
    for number in requested_start as u64..=requested_end {
        replay.push_str(numbered_lines.get(&number)?);
        replay.push('\n');
    }
    Some(replay)
}

/// Best-effort shell redirection targets (`> file`, `>> file`, including
/// heredoc writes like `cat > file <<'EOF'`). Quoted regions are ignored so
/// `echo "a > b"` is not a file write; fd duplications (`>&2`, `2>&1`) and
/// `/dev/*` sinks are skipped. Returns workspace-relative paths as written.
pub(crate) fn shell_redirection_targets(command: &str) -> Vec<String> {
    let bytes = command.as_bytes();
    let mut targets = Vec::new();
    let mut single_quote = false;
    let mut double_quote = false;
    let mut escaped = false;
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if escaped {
            escaped = false;
            index += 1;
            continue;
        }
        if byte == b'\\' && !single_quote {
            escaped = true;
            index += 1;
            continue;
        }
        if byte == b'\'' && !double_quote {
            single_quote = !single_quote;
            index += 1;
            continue;
        }
        if byte == b'"' && !single_quote {
            double_quote = !double_quote;
            index += 1;
            continue;
        }
        if byte != b'>' || single_quote || double_quote {
            index += 1;
            continue;
        }
        // Skip `<<` heredoc delimiters and `<` + `>` combos; we only record
        // `>` output targets (which cover `> file <<'EOF'` heredoc writes).
        let previous = index.checked_sub(1).and_then(|i| bytes.get(i)).copied();
        if previous == Some(b'<') {
            index += 1;
            continue;
        }
        let mut cursor = index + 1;
        // `>>` append.
        if bytes.get(cursor) == Some(&b'>') {
            cursor += 1;
        }
        // `>|` noclobber override.
        if bytes.get(cursor) == Some(&b'|') {
            cursor += 1;
        }
        // `>&2` / `>>>...` fd duplication, not a file.
        if bytes.get(cursor) == Some(&b'&') {
            index = cursor + 1;
            continue;
        }
        while bytes.get(cursor).is_some_and(|b| b.is_ascii_whitespace()) {
            cursor += 1;
        }
        if cursor >= bytes.len() {
            break;
        }
        let target = if bytes[cursor] == b'\'' || bytes[cursor] == b'"' {
            let quote = bytes[cursor];
            cursor += 1;
            let start = cursor;
            while cursor < bytes.len() && bytes[cursor] != quote {
                if bytes[cursor] == b'\\' && quote == b'"' {
                    cursor += 2;
                } else {
                    cursor += 1;
                }
            }
            let end = cursor.min(bytes.len());
            cursor = (end + 1).min(bytes.len().saturating_add(1));
            command.get(start..end).unwrap_or("").to_string()
        } else {
            let start = cursor;
            while cursor < bytes.len() {
                let b = bytes[cursor];
                if b.is_ascii_whitespace()
                    || matches!(b, b';' | b'&' | b'|' | b'<' | b'>' | b'(' | b')')
                {
                    break;
                }
                cursor += 1;
            }
            command.get(start..cursor).unwrap_or("").to_string()
        };
        let target = target.trim();
        if !target.is_empty()
            && !target.starts_with('&')
            && !target.starts_with("/dev/")
            && target.chars().any(|c| c != '>' && !c.is_ascii_digit())
        {
            let cleaned = target.trim_matches(|c| c == '\'' || c == '"').trim();
            if !cleaned.is_empty() && !targets.contains(&cleaned.to_string()) {
                targets.push(cleaned.to_string());
            }
        }
        index = cursor.max(index + 1);
    }
    targets
}

pub(crate) fn tool_result_from_execution(
    tool_name: &str,
    args: &serde_json::Value,
    execution: crate::tools::ToolExecutionOutput,
    diff: Option<String>,
) -> ToolResult {
    // The execution layer owns source/read completeness. Request-level
    // bounding is recorded independently during finalization below.
    let completeness = execution.completeness;
    let changed_paths = if tool_name == "run_command" {
        // Shell-created files (redirection, heredocs) bypass write tools but
        // still change the workspace. Record `>`/`>>` targets so file evidence
        // survives regardless of how the bytes got there.
        if execution.success {
            args.get("command")
                .and_then(|value| value.as_str())
                .map(shell_redirection_targets)
                .unwrap_or_default()
        } else {
            Vec::new()
        }
    } else if is_mutating_tool(tool_name) && execution.success {
        if !crate::network::mutation_made_progress(execution.success, &execution.content) {
            Vec::new()
        } else {
            args.get("path")
                .or_else(|| args.get("output_path"))
                .and_then(|value| value.as_str())
                .map(|path| vec![path.to_string()])
                .unwrap_or_default()
        }
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
            execution_us: 0,
            workspace_generation: None,
            call_id: None,
            arguments_hash: stable_arguments_hash(args),
            success: execution.success,
            pending: execution.pending,
            command: execution.command,
            exit_code: execution.exit_code,
            changed_paths,
            truncated: execution.truncated,
            payload_truncated: false,
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
            command_status: execution.command_status,
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
    let completed_mutation = is_mutating_tool(&result.tool_name)
        && mutation_made_progress(result.metadata.success, &result.content);
    let bounded = truncate_tool_output_for_message_with_completion(
        &result.tool_name,
        result.content,
        prefix,
        completed_mutation.then_some(COMPLETED_MUTATION_NOTICE),
    );
    result.content = bounded.content;
    if bounded.truncated {
        result.metadata.truncated = true;
        result.metadata.payload_truncated = true;
        if result.metadata.full_output_artifact.is_none() {
            result.metadata.full_output_artifact = bounded.full_output_artifact;
        }
        if result.metadata.error_kind.is_none() && !completed_mutation {
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
    // Preserve compatibility for old records, where `truncated=true` was the
    // only signal. New request-level clipping sets `payload_truncated`, so it
    // must not overwrite the source/read classification.
    let completeness = if result.metadata.truncated
        && !result.metadata.payload_truncated
        && result.metadata.completeness == ToolResultCompleteness::Complete
    {
        ToolResultCompleteness::ByteTruncated
    } else {
        result.metadata.completeness
    };
    result.metadata.completeness = completeness;
    if result.metadata.payload_truncated
        || matches!(
            completeness,
            ToolResultCompleteness::LineTruncated | ToolResultCompleteness::ByteTruncated
        )
    {
        result.metadata.truncated = true;
        if !result.content.contains(INCOMPLETE_TOOL_RESULT_MARKER) {
            result.content.push_str(&format!(
                "\n\n{INCOMPLETE_TOOL_RESULT_MARKER} completeness={}; content is partial and must not be treated as complete.]",
                if result.metadata.payload_truncated {
                    "payload_truncated"
                } else {
                    completeness.as_str()
                }
            ));
        }
        if result.metadata.payload_truncated
            && is_mutating_tool(&result.tool_name)
            && mutation_made_progress(result.metadata.success, &result.content)
            && !result.content.contains(COMPLETED_MUTATION_MARKER)
        {
            result.content.push_str("\n\n");
            result.content.push_str(COMPLETED_MUTATION_NOTICE);
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
    summarize_successful_verification(&mut result);
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
            workspace_generation: metadata.workspace_generation,
            workspace_epoch: Some(crate::workspace_intelligence::epoch().to_owned()),
            evidence_hash: Some(format!("{:x}", sha2::Sha256::digest(content.as_bytes()))),
            tool_name,
            arguments_hash: metadata.arguments_hash,
            success: envelope.success,
            pending: envelope.pending,
            command: envelope.command,
            exit_code: envelope.exit_code,
            changed_paths: envelope.changed_paths,
            truncated: envelope.truncated,
            payload_truncated: envelope.payload_truncated,
            completeness: envelope.completeness,
            full_output_artifact: envelope.full_output_artifact,
            error_kind: envelope.error_kind.map(|kind| kind.as_str().to_string()),
            retryable: envelope.retryable,
            replayed: envelope.replayed,
            inspection: envelope.inspection,
            command_status: envelope.command_status,
        })
}

const MIN_VERIFICATION_OUTPUT_BYTES: usize = 8 * 1024;

/// Summarize large successful Cargo check/test results as they first enter
/// history. The complete sanitized output stays in its artifact, while failed,
/// incomplete, unknown, and mixed-shell commands retain their original text.
fn summarize_successful_verification(result: &mut ToolResult) {
    if !matches!(result.tool_name.as_str(), "run_command" | "background_task")
        || !result.metadata.success
        || result.metadata.exit_code != Some(0)
        || result.metadata.pending
        || result.content.len() < MIN_VERIFICATION_OUTPUT_BYTES
    {
        return;
    }
    let incomplete = result.metadata.truncated
        || !matches!(
            result.metadata.completeness,
            ToolResultCompleteness::Complete | ToolResultCompleteness::UserLimited
        );
    if incomplete && result.metadata.full_output_artifact.is_none() {
        return;
    }
    let Some(command) = result.metadata.command.as_deref() else {
        return;
    };
    if !known_cargo_verification_command(command) {
        return;
    }
    let sanitized_content = sanitize_tool_output(&result.content);
    let capture_is_partial = incomplete
        || result
            .metadata
            .command_status
            .as_ref()
            .is_some_and(|status| status.output_truncated);
    let Some(mut summary) = cargo_verification_summary(
        command,
        result
            .metadata
            .exit_code
            .expect("checked successful exit code"),
        result.metadata.command_status.as_ref(),
        &sanitized_content,
        capture_is_partial,
    ) else {
        return;
    };
    if summary.len() >= result.content.len() {
        return;
    }
    let artifact = result
        .metadata
        .full_output_artifact
        .clone()
        .or_else(|| save_full_tool_output(&result.tool_name, &sanitized_content));
    let Some(artifact) = artifact else {
        return;
    };
    summary.push_str(&format!(
        "\nFull output saved to: {artifact}\nUse grep to search the full output or view_file with line offsets to inspect it."
    ));
    result.content = summary;
    result.metadata.full_output_artifact = Some(artifact);
}

fn known_cargo_verification_command(command: &str) -> bool {
    let commands = command.split("&&").map(str::trim).collect::<Vec<_>>();
    !commands.is_empty()
        && commands.iter().all(|command| {
            let cargo_command = command.split_whitespace().take(2).collect::<Vec<_>>();
            matches!(cargo_command.as_slice(), ["cargo", "check" | "test"])
                && super::super::compiler::is_verification_command(command)
        })
}

fn cargo_verification_summary(
    command: &str,
    exit_code: i32,
    command_status: Option<&rustcode_core::CommandResultMetadata>,
    content: &str,
    capture_is_partial: bool,
) -> Option<String> {
    let lines = content.lines().collect::<Vec<_>>();
    if lines.iter().any(|line| {
        let line = line.trim_start();
        line.starts_with("error:")
            || line.starts_with("error[")
            || line.starts_with("test ") && line.ends_with(" ... FAILED")
            || line == "failures:"
    }) {
        return None;
    }

    let mut completion = Vec::new();
    let mut warnings = Vec::new();
    let mut diagnostics = Vec::new();
    let mut collecting_warning = false;
    for line in &lines {
        let trimmed = line.trim_start();
        if trimmed.starts_with("Finished ")
            || trimmed.starts_with("running ")
            || trimmed.starts_with("test result:")
            || trimmed.starts_with("Doc-tests ")
            || trimmed.starts_with("Running ")
        {
            completion.push(*line);
            collecting_warning = false;
        }
        if trimmed.starts_with("warning:") {
            warnings.push(*line);
            collecting_warning = true;
        } else if collecting_warning {
            if is_cargo_output_boundary(trimmed) {
                collecting_warning = false;
            } else {
                warnings.push(*line);
            }
        } else if !is_known_cargo_progress_line(trimmed) {
            diagnostics.push(*line);
        }
    }

    let is_test = command.split("&&").any(|segment| {
        segment
            .split_whitespace()
            .take(2)
            .collect::<Vec<_>>()
            .as_slice()
            == ["cargo", "test"]
    });
    let has_test_totals = completion
        .iter()
        .any(|line| line.trim_start().starts_with("test result:"));
    let has_check_completion = completion
        .iter()
        .any(|line| line.trim_start().starts_with("Finished "));
    if (is_test && !has_test_totals) || (!is_test && !has_check_completion) {
        return None;
    }

    let mut summary = vec!["[Successful Cargo verification summary]".to_owned()];
    summary.push(format!("Command: {command}"));
    summary.push(format!("exit code: {exit_code}"));
    if let Some(status) = command_status {
        let signal = status.signal.map_or_else(
            || "none".to_owned(),
            |signal| match signal {
                13 => "13 (SIGPIPE)".to_owned(),
                signal => signal.to_string(),
            },
        );
        summary.push(format!(
            "[command status: completed={}; success=true; exit_code={:?}; signal={signal}; downstream_consumer_terminated={}; bytes_returned={}; total_output_bytes={:?}; output_truncated_by_rustcode={}]",
            status.completed,
            status.exit_code,
            status.downstream_consumer_terminated,
            status.bytes_returned,
            status.total_output_bytes,
            status.output_truncated,
        ));
    }
    if !warnings.is_empty() {
        summary.push("Warnings:".to_owned());
        summary.extend(warnings.into_iter().map(str::to_owned));
    }
    if !diagnostics.is_empty() {
        summary.push("Additional output:".to_owned());
        summary.extend(diagnostics.into_iter().map(str::to_owned));
    }
    if !completion.is_empty() {
        summary.push("Verification totals:".to_owned());
        summary.extend(completion.into_iter().map(str::to_owned));
    }
    if capture_is_partial {
        summary.push(
            "Captured output is partial; totals cover only suites present in this capture. Use the saved artifact to inspect all text returned by the command runner."
                .to_owned(),
        );
    }
    Some(summary.join("\n"))
}

fn is_known_cargo_progress_line(line: &str) -> bool {
    line.is_empty()
        || line.starts_with("stdout:")
        || line.starts_with("stderr:")
        || line.starts_with("[command status:")
        || line.starts_with("exit code:")
        || line.starts_with("Checking ")
        || line.starts_with("Compiling ")
        || line.starts_with("Downloading ")
        || line.starts_with("Downloaded ")
        || line.starts_with("Updating ")
        || line.starts_with("Locking ")
        || line.starts_with("Adding ")
        || line.starts_with("Removing ")
        || line.starts_with("Fresh ")
        || line.starts_with("Waiting for file lock on ")
        || line.starts_with("Running ")
        || line.starts_with("Finished ")
        || line.starts_with("running ")
        || line.starts_with("test result:")
        || line.starts_with("Doc-tests ")
        || (line.starts_with("test ") && line.ends_with(" ... ok"))
}

fn is_cargo_output_boundary(line: &str) -> bool {
    line.starts_with("Checking ")
        || line.starts_with("Compiling ")
        || line.starts_with("Finished ")
        || line.starts_with("Running ")
        || line.starts_with("running ")
        || line.starts_with("test result:")
        || line.starts_with("Doc-tests ")
        || line.starts_with("test ")
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

#[cfg(test)]
mod tests {
    use super::*;

    fn verification_result(command: &str, content: String, success: bool) -> ToolResult {
        ToolResult {
            tool_name: "run_command".to_owned(),
            content,
            diff: None,
            file_preview: None,
            metadata: ToolResultMetadata {
                success,
                command: Some(command.to_owned()),
                exit_code: Some(if success { 0 } else { 101 }),
                ..Default::default()
            },
        }
    }

    #[test]
    fn successful_cargo_test_is_summarized_with_warning_totals_artifact_and_call_pairing() {
        let mut content = String::from(
            "[command status: completed=true; success=true; exit_code=Some(0)]\nexit code: 0\nstdout:\nfatal: additional successful-command diagnostic\nwarning: unused import: `Thing`\n  --> src/lib.rs:4:5\n   |\n4  | use Thing;\n   |     ^^^^^\nwarning: api_key=verification-secret\nwarning: `rustcode-engine` generated 2 warnings\n    Checking rustcode-engine v0.57.6\n    Finished `dev` profile [unoptimized + debuginfo]\n    Finished `test` profile [unoptimized + debuginfo]\nrunning 1822 tests\n",
        );
        for index in 0..700 {
            content.push_str(&format!("test suite::case_{index} ... ok\n"));
        }
        content.push_str(
            "test result: ok. 700 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n",
        );
        let input_bytes = content.len() as u64;
        let original = sanitize_tool_output(&content);
        let command = "cargo check --tests && cargo test --locked";
        let mut result = verification_result(command, content, true);
        result.metadata.command_status = Some(rustcode_core::CommandResultMetadata {
            completed: true,
            exit_code: Some(0),
            signal: None,
            downstream_consumer_terminated: false,
            bytes_returned: input_bytes,
            total_output_bytes: Some(original.len() as u64),
            output_truncated: false,
        });

        let message = tool_result_history_message(result, Some("call-verification".to_owned()));

        assert!(message.content.len() < original.len() / 4);
        assert!(message.content.contains(command));
        assert!(message.content.contains("exit code: 0"));
        assert!(
            message
                .content
                .contains("test result: ok. 700 passed; 0 failed")
        );
        assert!(message.content.contains("warning: unused import: `Thing`"));
        assert!(message.content.contains("src/lib.rs:4:5"));
        assert!(
            message
                .content
                .contains("fatal: additional successful-command diagnostic")
        );
        assert!(message.content.contains("api_key=[REDACTED]"));
        assert!(message.content.contains("bytes_returned="));
        assert!(message.content.contains("Full output saved to:"));
        assert!(!message.content.contains("verification-secret"));
        assert_eq!(message.tool_call_id.as_deref(), Some("call-verification"));

        let record = message.tool_result.expect("structured result record");
        assert!(record.success);
        assert_eq!(record.command.as_deref(), Some(command));
        assert_eq!(record.exit_code, Some(0));
        assert_eq!(record.command_status.unwrap().bytes_returned, input_bytes);
        let artifact = record.full_output_artifact.expect("full output artifact");
        assert_eq!(
            std::fs::read_to_string(artifact).expect("artifact content"),
            original
        );
    }

    #[test]
    fn failed_cargo_test_keeps_diagnostics_unchanged() {
        let content = format!(
            "exit code: 101\nerror: test failed\n{}",
            "diagnostic context\n".repeat(600)
        );

        let message = tool_result_history_message(
            verification_result("cargo test", content.clone(), false),
            Some("call-failed".to_owned()),
        );

        assert!(message.content.ends_with(&content));
        let record = message.tool_result.expect("structured result record");
        assert!(!record.success);
        assert_eq!(record.command.as_deref(), Some("cargo test"));
        assert_eq!(record.exit_code, Some(101));
        assert_eq!(record.full_output_artifact, None);
    }

    #[test]
    fn mixed_shell_command_keeps_successful_cargo_output_unchanged() {
        let content = format!("exit code: 0\n{}", "test output\n".repeat(600));
        let message = tool_result_history_message(
            verification_result("cargo test && echo unrelated", content.clone(), true),
            None,
        );

        assert!(message.content.ends_with(&content));
        assert_eq!(message.tool_result.unwrap().full_output_artifact, None);
    }

    #[test]
    fn unrecognized_cargo_command_keeps_successful_output_unchanged() {
        let content = format!("exit code: 0\n{}", "clippy output\n".repeat(600));
        let message = tool_result_history_message(
            verification_result("cargo clippy --all-targets", content.clone(), true),
            None,
        );

        assert!(message.content.ends_with(&content));
        assert_eq!(message.tool_result.unwrap().full_output_artifact, None);
    }

    #[test]
    fn truncated_success_requires_and_preserves_its_existing_artifact() {
        let dir = tempfile::tempdir().expect("temporary artifact directory");
        let artifact_path = dir.path().join("cargo-output.txt");
        let mut content = String::from("exit code: 0\nrunning 400 tests\n");
        for index in 0..600 {
            content.push_str(&format!("test suite::case_{index} ... ok\n"));
        }
        content.push_str(
            "test result: ok. 400 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n",
        );
        std::fs::write(&artifact_path, &content).expect("write full output artifact");

        let mut result = verification_result("cargo test", content, true);
        result.metadata.truncated = true;
        result.metadata.completeness = ToolResultCompleteness::ByteTruncated;
        result.metadata.full_output_artifact = Some(artifact_path.to_string_lossy().to_string());

        let message = tool_result_history_message(result, None);

        assert!(
            message
                .content
                .contains("test result: ok. 400 passed; 0 failed")
        );
        assert!(
            message
                .content
                .contains(&artifact_path.to_string_lossy().to_string())
        );
        assert!(message.content.contains("Captured output is partial"));
        let record = message.tool_result.expect("structured result record");
        assert!(record.truncated);
        assert_eq!(
            record.full_output_artifact.as_deref(),
            Some(artifact_path.to_string_lossy().as_ref())
        );
    }

    #[test]
    fn history_preserves_evidence_epoch_generation_and_hash_after_resume() {
        let content = "source contents";
        let result = ToolResult {
            tool_name: "view_file".into(),
            content: content.into(),
            diff: None,
            file_preview: None,
            metadata: ToolResultMetadata {
                success: true,
                workspace_generation: Some(7),
                ..Default::default()
            },
        };
        let message =
            tool_result_history_message_with_prefix(result, "view_file: ", Some("call".into()));
        let restored: ChatMessage =
            serde_json::from_slice(&serde_json::to_vec(&message).unwrap()).unwrap();
        let record = restored.tool_result.unwrap();
        assert_eq!(record.workspace_generation, Some(7));
        assert_eq!(
            record.workspace_epoch.as_deref(),
            Some(crate::workspace_intelligence::epoch())
        );
        assert_eq!(
            record.evidence_hash,
            Some(format!("{:x}", sha2::Sha256::digest(content.as_bytes())))
        );
    }

    #[test]
    fn shell_redirection_targets_cover_heredoc_and_append() {
        assert_eq!(
            shell_redirection_targets("cat > package.json <<'EOF'\n{}\nEOF"),
            vec!["package.json".to_string()]
        );
        assert_eq!(
            shell_redirection_targets("echo hi >> src/GameScene.ts"),
            vec!["src/GameScene.ts".to_string()]
        );
        assert_eq!(
            shell_redirection_targets("cat <<'EOF' > src/app.ts\nx\nEOF"),
            vec!["src/app.ts".to_string()]
        );
        assert!(shell_redirection_targets("sed -n '1,2p' src/lib.rs").is_empty());
        assert!(shell_redirection_targets("echo \"a > b\"").is_empty());
        assert!(shell_redirection_targets("cmd 2>&1").is_empty());
        assert!(shell_redirection_targets("echo x > /dev/null").is_empty());
    }

    #[test]
    fn run_command_redirection_populates_changed_paths() {
        let result = tool_result_from_execution(
            "run_command",
            &serde_json::json!({"command": "cat > package.json <<'EOF'\n{}\nEOF"}),
            crate::tools::ToolExecutionOutput::success("exit code: 0".into()),
            None,
        );
        assert_eq!(
            result.metadata.changed_paths,
            vec!["package.json".to_string()]
        );
    }
}
