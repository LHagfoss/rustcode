use std::sync::atomic::{AtomicU64, Ordering};
use std::{fs::OpenOptions, io::Write};

const MAX_TOOL_OUTPUT_BYTES: usize = 50 * 1024;
const MAX_TOOL_OUTPUT_LINES: usize = 1000;
static NEXT_ARTIFACT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(crate) struct BoundedToolOutput {
    pub(crate) content: String,
    pub(crate) truncated: bool,
    pub(crate) full_output_artifact: Option<String>,
}

/// Truncate tool output at execution time if it exceeds size limits.
/// Full output is saved to a temp file so the agent can still access it.
#[allow(
    dead_code,
    reason = "preserved payload-only interface for callers outside history insertion"
)]
pub(crate) fn truncate_tool_output(name: &str, result: String) -> String {
    truncate_tool_output_for_message(name, result, "").content
}

/// Bound a tool payload so adding its provider-facing message prefix cannot
/// push the complete history entry over the result boundary.
pub(crate) fn truncate_tool_output_for_message(
    name: &str,
    result: String,
    message_prefix: &str,
) -> BoundedToolOutput {
    let max_bytes = MAX_TOOL_OUTPUT_BYTES.saturating_sub(message_prefix.len());
    let max_lines = MAX_TOOL_OUTPUT_LINES.saturating_sub(message_prefix.matches('\n').count());
    let bytes = result.len();
    let lines: Vec<&str> = result.lines().collect();
    let line_count = lines.len();

    if bytes <= max_bytes && line_count <= max_lines {
        return BoundedToolOutput {
            content: result,
            truncated: false,
            full_output_artifact: None,
        };
    }

    let saved_path = save_full_tool_output(name, &result);
    let retained_line_budget = max_lines.min(line_count);
    let mut head_count = ((retained_line_budget * 3) / 10).max(1).min(line_count);
    let mut tail_count = ((retained_line_budget * 3) / 10).max(1).min(line_count);
    let path_note = match saved_path.as_deref() {
        Some(path) => format!(
            " Full output saved to: {path}\nUse grep to search the full content or view_file with line offsets to read specific sections."
        ),
        None => String::new(),
    };

    loop {
        let head: String = lines[..head_count.min(line_count)].join("\n");
        let tail: String = if tail_count > 0
            && line_count > 1
            && line_count >= head_count + tail_count
        {
            lines[line_count - tail_count..].join("\n")
        } else {
            String::new()
        };
        let omitted_lines = line_count.saturating_sub(head_count + tail_count);
        let omitted_bytes = bytes.saturating_sub(head.len() + tail.len());
        let mut output = format!(
            "{head}\n\n... [{omitted_lines} lines / {omitted_bytes} bytes truncated] ...\n\n{tail}\n\n[Output truncated: {bytes} bytes total, {line_count} lines.{path_note}]"
        );

        if output.len() <= max_bytes {
            return BoundedToolOutput {
                content: output,
                truncated: true,
                full_output_artifact: saved_path,
            };
        }
        if head_count > 0 {
            head_count -= 1;
        } else if tail_count > 0 {
            tail_count -= 1;
        } else {
            while !output.is_char_boundary(max_bytes) {
                output.pop();
            }
            output.truncate(max_bytes);
            return BoundedToolOutput {
                content: output,
                truncated: true,
                full_output_artifact: saved_path,
            };
        }
    }
}

fn save_full_tool_output(name: &str, content: &str) -> Option<String> {
    let dir = crate::config::get_config_dir()?.join("tool_output");
    let _ = std::fs::create_dir_all(&dir);
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or(std::time::Duration::from_secs(0))
        .as_millis();
    let mut sequence = NEXT_ARTIFACT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let safe_name: String = name
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    loop {
        let path = dir.join(format!("{ts}_{sequence}_{safe_name}.txt"));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                return file
                    .write_all(content.as_bytes())
                    .is_ok()
                    .then(|| path.to_string_lossy().to_string());
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                sequence = sequence.wrapping_add(1);
            }
            Err(_) => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_output_passes_through_unchanged() {
        let small = "line one\nline two\n".to_string();
        assert_eq!(truncate_tool_output("view_file", small.clone()), small);
    }

    #[test]
    fn oversized_line_count_is_truncated_with_head_and_tail_kept() {
        let content: String = (1..=2000).map(|n| format!("line {n}\n")).collect();
        let out = truncate_tool_output("grep", content);

        assert!(out.contains("line 1\n"), "head must survive, got head missing");
        assert!(out.contains("line 2000"), "tail must survive, got tail missing");
        assert!(out.contains("[Output truncated:"), "must carry an explicit marker");
        assert!(out.len() < 2000 * 8, "result must actually be smaller than the input");
    }

    #[test]
    fn oversized_byte_count_is_truncated_even_with_few_lines() {
        // A handful of very long lines can exceed the byte cap without
        // exceeding the line cap — must still be bounded.
        let content = format!("{}\n{}\n", "a".repeat(40_000), "b".repeat(40_000));
        let out = truncate_tool_output("run_command", content);
        assert!(out.contains("[Output truncated:"));
        assert!(out.len() <= MAX_TOOL_OUTPUT_BYTES, "bounded output was {} bytes", out.len());
    }

    #[test]
    fn oversized_byte_only_output_preserves_a_trailing_notice() {
        let content = format!(
            "{}\n{}\n[harness: deferred additional tool calls]",
            "a".repeat(60_000),
            "b".repeat(60_000)
        );
        let out = truncate_tool_output("use_skill", content);

        assert!(
            out.contains("[harness: deferred additional tool calls]"),
            "bounded output must preserve the trailing notice"
        );
        assert!(out.len() <= MAX_TOOL_OUTPUT_BYTES);
        assert_eq!(out.matches("[Output truncated:").count(), 1);
    }

    #[test]
    fn oversized_line_and_byte_count_stays_within_byte_cap() {
        let content: String = (1..=2000).map(|n| format!("line {n}: {}\n", "x".repeat(100))).collect();
        let out = truncate_tool_output("run_command", content);

        assert!(out.contains("[Output truncated:"));
        assert!(out.len() <= MAX_TOOL_OUTPUT_BYTES, "bounded output was {} bytes", out.len());
    }

    #[test]
    fn multiline_history_prefix_counts_toward_the_line_boundary() {
        let prefix = "background_task: Task task_1 completed. Output:\n";
        let content = (1..=MAX_TOOL_OUTPUT_LINES)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n");

        let bounded = truncate_tool_output_for_message("background_task", content, prefix);
        let message = format!("{prefix}{}", bounded.content);

        assert!(bounded.truncated);
        assert!(message.len() <= MAX_TOOL_OUTPUT_BYTES);
        assert!(message.lines().count() <= MAX_TOOL_OUTPUT_LINES);
        assert!(message.contains("[Output truncated:"));
        assert!(bounded.full_output_artifact.is_some());
    }

    #[test]
    fn compiler_diagnostic_in_tail_survives_truncation() {
        let mut content: String = (1..=2000).map(|n| format!("build progress {n}\n")).collect();
        content.push_str("error[E0425]: cannot find value `missing_symbol` in this scope\n");

        let out = truncate_tool_output("cargo_check", content);

        assert!(
            out.contains("error[E0425]: cannot find value `missing_symbol` in this scope"),
            "tail compiler diagnostic must survive truncation, got: {out}"
        );
    }

    #[test]
    fn truncated_output_carries_a_recovery_instruction() {
        let content: String = (1..=2000).map(|n| format!("line {n}\n")).collect();
        let out = truncate_tool_output("grep", content);
        assert!(
            out.contains("Full output saved to:") || out.contains("Use grep"),
            "must tell the model how to recover the omitted content, got: {out}"
        );
    }

    #[test]
    fn exact_follow_up_read_recovers_the_full_content() {
        let content: String = (1..=2000).map(|n| format!("line {n}\n")).collect();
        let out = truncate_tool_output("grep", content.clone());
        let marker = "Full output saved to: ";
        let start = out.find(marker).expect("truncation marker names the saved path") + marker.len();
        let path = out[start..].lines().next().expect("path on its own line");
        let recovered = std::fs::read_to_string(path).expect("saved file readable");
        assert!(!out.contains(&content), "bounded output must not contain the full payload");
        assert_eq!(recovered, content, "saved artifact must be byte-identical to the original");
    }

    #[test]
    fn concurrent_same_name_outputs_save_to_distinct_artifacts() {
        let first = "first\n".to_string() + &"a".repeat(60_000);
        let second = "second\n".to_string() + &"b".repeat(60_000);
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let handles: Vec<_> = [
            (first.clone(), barrier.clone()),
            (second.clone(), barrier.clone()),
        ]
        .into_iter()
        .map(|(content, barrier)| {
            std::thread::spawn(move || {
                barrier.wait();
                truncate_tool_output("same_tool", content)
            })
        })
        .collect();

        let outputs: Vec<String> = handles
            .into_iter()
            .map(|handle| handle.join().expect("output save thread must not panic"))
            .collect();
        let paths: Vec<&str> = outputs
            .iter()
            .map(|output| {
                output
                    .split_once("Full output saved to: ")
                    .and_then(|(_, rest)| rest.lines().next())
                    .expect("truncated output must name its artifact")
            })
            .collect();

        assert_ne!(paths[0], paths[1], "same-name outputs must not share an artifact path");
        let recovered = [
            std::fs::read(paths[0])
                .unwrap_or_else(|error| panic!("failed to read first artifact {}: {error}", paths[0])),
            std::fs::read(paths[1])
                .unwrap_or_else(|error| panic!("failed to read second artifact {}: {error}", paths[1])),
        ];
        assert!(recovered.contains(&first.into_bytes()));
        assert!(recovered.contains(&second.into_bytes()));
    }
}
