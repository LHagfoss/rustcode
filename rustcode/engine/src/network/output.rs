use std::sync::atomic::{AtomicU64, Ordering};
use std::{fs::OpenOptions, io::Write};

use regex::Regex;

const MAX_TOOL_OUTPUT_BYTES: usize = 50 * 1024;
const MAX_TOOL_OUTPUT_LINES: usize = 1000;
pub(crate) const INCOMPLETE_TOOL_RESULT_MARKER: &str = "[tool_result_incomplete:";
pub(crate) const COMPLETED_MUTATION_MARKER: &str = "[mutation_completed_with_clipped_output]";
pub(crate) const COMPLETED_MUTATION_NOTICE: &str = "[mutation_completed_with_clipped_output] Mutation completed successfully. Only the output or diff preview was clipped for context; do not retry the mutation. Use the saved artifact or a focused read if more detail is needed.";
static NEXT_ARTIFACT_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static SENSITIVE_ASSIGNMENT: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
    Regex::new(
        r#"(?i)(\b(?:api[_ -]?key|access[_ -]?token|authorization|password|secret|token|cookie)\b(\s*[:=]\s*))(["']?)([^\s"'`]+)(["']?)"#,
    )
    .expect("sensitive output regex")
});
static AUTH_HEADER: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
    Regex::new(r#"(?i)(\bauthorization\s*:\s*)((?:bearer|basic|token)\s+)?([^\s"'`]+)"#)
        .expect("authorization header regex")
});

/// Remove common credential assignments before output is persisted or shown
/// to the model. This intentionally leaves ordinary token counts and prose
/// untouched while covering shell assignments, YAML fields, and headers.
///
/// Every tool result passes through here, file reads included, so a value is
/// redacted only when it can be a literal credential. Text that names where a
/// credential comes from (`$TOKEN`, `<token>`) or is plainly source code
/// (`token: Option<String>,`) is left alone: rewriting it showed the model a
/// file that did not exist, and its edits against that text could not match.
pub(crate) fn sanitize_tool_output(result: &str) -> String {
    let redacted = AUTH_HEADER.replace_all(result, |found: &regex::Captures<'_>| {
        let value = &found[3];
        // With a scheme in front the value is a credential unless it is a
        // reference; without one it may be a field declaration.
        let keep = if found.get(2).is_some() {
            is_credential_reference(value)
        } else {
            is_credential_reference(value) || is_code_expression(value)
        };
        if keep {
            found[0].to_owned()
        } else {
            format!("{}[REDACTED]", &found[1])
        }
    });
    SENSITIVE_ASSIGNMENT
        .replace_all(&redacted, |found: &regex::Captures<'_>| {
            let (separator, value) = (&found[2], &found[4]);
            let quoted = !found[3].is_empty();
            // `KEY=value` with nothing around it is how env files and shell
            // assignments are written, where a bare word is the secret itself.
            if is_literal_assignment(separator, quoted, value) {
                format!("{}{}[REDACTED]{}", &found[1], &found[3], &found[5])
            } else {
                found[0].to_owned()
            }
        })
        .into_owned()
}

/// Whether the value assigned to a credential name can be the credential
/// itself rather than a reference to it or a piece of code.
fn is_literal_assignment(separator: &str, quoted: bool, value: &str) -> bool {
    let env_style = separator == "=" && !value.ends_with([',', ';', ')']);
    !is_credential_reference(value) && (quoted || env_style || !is_code_expression(value))
}

/// Like `SENSITIVE_ASSIGNMENT`, but the credential word may end a longer name
/// (`SOLIDTIME_API_KEY=`) and prose may assign with "is". Nothing is rewritten
/// from this pattern, so it can afford to be wider.
static NOTE_ASSIGNMENT: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
    Regex::new(
        r#"(?i)\b[\w.-]*?(?:api[_ -]?key|access[_ -]?key|secret[_ -]?key|private[_ -]?key|access[_ -]?token|authorization|password|passwd|secret|token|cookie)\b(\s*[:=]\s*|\s+is\s+)(["']?)([^\s"'`]+)"#,
    )
    .expect("note assignment regex")
});
static BEARER_VALUE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
    Regex::new(r#"(?i)\bbearer\s+([^\s"'`]+)"#).expect("bearer value regex")
});
/// Prefixes that providers put on issued credentials. Anchored at a word
/// start and followed by a key-length body, so `task-`, `risk-` or a word
/// containing `akia` is not one.
static PROVIDER_TOKEN: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
    Regex::new(
        r"\b(?:sk-[A-Za-z0-9_-]{8,}|ghp_[A-Za-z0-9]{8,}|github_pat_[A-Za-z0-9_]{8,}|xox[abprs]-[A-Za-z0-9-]{8,}|AKIA[0-9A-Z]{16})",
    )
    .expect("provider token regex")
});

/// Name the rule under which `text` holds a literal credential, for stores
/// that refuse such text instead of redacting it. Uses the same distinction
/// as `sanitize_tool_output`: text that names where a credential comes from
/// (an environment variable, `$VAR`, a file path) is not a credential.
pub(crate) fn literal_credential_rule(text: &str) -> Option<&'static str> {
    if text.contains("-----BEGIN ") {
        return Some("PEM block");
    }
    if PROVIDER_TOKEN.is_match(text) {
        return Some("provider token prefix");
    }
    let header = AUTH_HEADER.captures_iter(text).any(|found| {
        let value = &found[3];
        !is_credential_reference(value) && (found.get(2).is_some() || !is_code_expression(value))
    });
    if header {
        return Some("authorization header");
    }
    let assignment = NOTE_ASSIGNMENT.captures_iter(text).any(|found| {
        let (separator, value) = (found[1].trim(), &found[3]);
        !is_path_reference(value) && is_literal_assignment(separator, !found[2].is_empty(), value)
    });
    if assignment {
        return Some("credential assignment");
    }
    let bearer = BEARER_VALUE
        .captures_iter(text)
        .any(|found| !is_credential_reference(&found[1]) && !is_code_expression(&found[1]));
    bearer.then_some("bearer token")
}

/// A file a credential is read from. A base64 secret can start with `/`, so
/// an absolute path has to read as one: short plain segments, at least two.
fn is_path_reference(value: &str) -> bool {
    let path = value.trim_end_matches([',', ';', ')', '.', '/']);
    if path.starts_with("~/") || path.starts_with("./") || path.starts_with("../") {
        return true;
    }
    path.strip_prefix('/').is_some_and(|rest| {
        rest.contains('/')
            && rest.split('/').all(|segment| {
                !segment.is_empty()
                    && segment.len() <= 32
                    && segment.chars().all(|character| {
                        character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
                    })
            })
    })
}

/// A variable, placeholder or template that stands in for a credential.
fn is_credential_reference(value: &str) -> bool {
    value.starts_with(['$', '<', '{', '%']) || value.starts_with("[REDACTED")
}

/// An unquoted value that reads as code rather than as a literal: a type, a
/// call, or a name such as `String`, `self.token` or `None`.
fn is_code_expression(value: &str) -> bool {
    if value.contains(['(', ')', '<', '>', '[', ']', '{', '}', '&', '*']) || value.contains("::") {
        return true;
    }
    let name = value.trim_end_matches([',', ';']);
    const SIZED_TYPES: &[&str] = &[
        "u8", "u16", "u32", "u64", "u128", "i8", "i16", "i32", "i64", "i128", "f32", "f64",
    ];
    SIZED_TYPES.contains(&name)
        || (!name.is_empty()
            && name.split('.').all(|segment| {
                !segment.is_empty()
                    && segment
                        .chars()
                        .all(|character| character.is_ascii_alphabetic() || character == '_')
            }))
}

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
    truncate_tool_output_for_message_with_completion(name, result, message_prefix, None)
}

/// Bound a tool payload while preserving an explicit completion fact for a
/// mutation whose display output or diff was clipped after execution.
pub(crate) fn truncate_tool_output_for_message_with_completion(
    name: &str,
    result: String,
    message_prefix: &str,
    completion_notice: Option<&str>,
) -> BoundedToolOutput {
    let result = sanitize_tool_output(&result);
    let max_bytes = MAX_TOOL_OUTPUT_BYTES.saturating_sub(message_prefix.len());
    // Leave room for the bounded-output explanation and its machine-readable
    // incomplete marker in the final model-visible message.
    let suffix_lines = 2 + completion_notice
        .filter(|notice| !notice.is_empty())
        .map_or(0, |notice| notice.lines().count() + 2);
    let max_lines =
        MAX_TOOL_OUTPUT_LINES.saturating_sub(message_prefix.matches('\n').count() + suffix_lines);
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
        let tail: String =
            if tail_count > 0 && line_count > 1 && line_count >= head_count + tail_count {
                lines[line_count - tail_count..].join("\n")
            } else {
                String::new()
            };
        let omitted_lines = line_count.saturating_sub(head_count + tail_count);
        let omitted_bytes = bytes.saturating_sub(head.len() + tail.len());
        let completion_note = completion_notice
            .filter(|notice| !notice.is_empty())
            .map(|notice| format!("{notice}\n\n"))
            .unwrap_or_default();
        let mut output = format!(
            "{head}\n\n... [{omitted_lines} lines / {omitted_bytes} bytes truncated] ...\n\n{tail}\n\n[Output truncated: {bytes} bytes total, {line_count} lines.{path_note}]\n\n{completion_note}{INCOMPLETE_TOOL_RESULT_MARKER} completeness=byte_truncated; content is partial and must not be treated as complete.]"
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
            let marker = format!(
                "{completion_note}{INCOMPLETE_TOOL_RESULT_MARKER} completeness=byte_truncated; content is partial and must not be treated as complete.]"
            );
            let content_budget = max_bytes.saturating_sub(marker.len() + 2);
            while !output.is_char_boundary(content_budget) {
                output.pop();
            }
            output.truncate(content_budget);
            output.push_str("\n\n");
            output.push_str(&marker);
            return BoundedToolOutput {
                content: output,
                truncated: true,
                full_output_artifact: saved_path,
            };
        }
    }
}

pub(crate) fn save_full_tool_output(name: &str, content: &str) -> Option<String> {
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
    fn credential_assignments_are_redacted_before_model_output_and_artifacts() {
        let content = r#"TOKEN="eyJhbGciOi..."
api_key: sk-secret-value
Authorization: Bearer bearer-secret
token usage: 1234
"#;
        let out = truncate_tool_output("run_command", content.to_string());

        assert!(!out.contains("eyJhbGciOi"));
        assert!(!out.contains("sk-secret-value"));
        assert!(!out.contains("bearer-secret"));
        assert!(out.contains("token usage: 1234"));

        let oversized = format!("TOKEN=long-lived-secret\n{}", "x".repeat(60_000));
        let bounded = truncate_tool_output("run_command", oversized);
        let marker = "Full output saved to: ";
        let path = bounded
            .split_once(marker)
            .and_then(|(_, rest)| rest.lines().next())
            .expect("bounded output must name its artifact");
        let artifact = std::fs::read_to_string(path).expect("artifact must be readable");
        assert!(!artifact.contains("long-lived-secret"));
    }

    #[test]
    fn literal_credentials_are_redacted_in_every_common_form() {
        for (input, leaked) in [
            ("TOKEN=abcdefgh", "abcdefgh"),
            ("export API_KEY=sk-live-0a1b2c", "sk-live"),
            ("password: hunter2", "hunter2"),
            ("secret = 'p4ss word'", "p4ss"),
            ("token = \"ghp_0123456789abcdef\"", "ghp_"),
            ("Authorization: Bearer abcdef", "abcdef"),
            ("-H \"Authorization: Basic dXNlcjpwYXNz\"", "dXNlcjpwYXNz"),
            ("authorization: 0123456789abcdef", "0123456789abcdef"),
        ] {
            let out = sanitize_tool_output(input);
            assert!(out.contains("[REDACTED]"), "{input:?} -> {out:?}");
            assert!(!out.contains(leaked), "{input:?} -> {out:?}");
        }
        // The quote that closes the argument survives the redaction.
        assert_eq!(
            sanitize_tool_output("curl -H \"Authorization: Bearer abc123\" https://x"),
            "curl -H \"Authorization: [REDACTED]\" https://x"
        );
    }

    #[test]
    fn references_and_source_code_are_shown_as_written() {
        // Rewriting any of these shows the model a file that does not exist,
        // and an edit written against that text cannot match the real one.
        for line in [
            "curl -sS \"$URL\" -H \"Authorization: Bearer $T\"",
            "  -H \"Authorization: Bearer ${SOLIDTIME_API_KEY}\" \\",
            "Authorization: Bearer <token>",
            "export TOKEN=$SOLIDTIME_API_KEY",
            "password: {{ vault_password }}",
            "    pub token: Option<String>,",
            "    secret: &str,",
            "    authorization: String,",
            "    password: str = None",
            "let token = self.next_token();",
            "const TOKEN = process.env.TOKEN;",
            "client = Client(api_key=api_key, timeout=3)",
            "token: the bearer token used for every request",
            "    cookie: u64,",
        ] {
            assert_eq!(sanitize_tool_output(line), line);
        }
    }

    #[test]
    fn literal_credentials_are_named_by_rule_without_the_value() {
        for (text, rule) in [
            ("SOLIDTIME_API_KEY=st_0a1b2c3d4e", "credential assignment"),
            ("export GITHUB_TOKEN=abcdefgh", "credential assignment"),
            ("password: hunter2", "credential assignment"),
            ("the password is hunter2", "credential assignment"),
            ("token: \"abcdef\"", "credential assignment"),
            (
                "aws secret=/Xk3abcdefghijklmnopqrstuvwxyz0123456789AB",
                "credential assignment",
            ),
            ("Authorization: Bearer abcdef", "authorization header"),
            (
                "send Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0In0 with it",
                "bearer token",
            ),
            ("key sk-live-0a1b2c3d4e5f", "provider token prefix"),
            ("ghp_0123456789abcdefghij", "provider token prefix"),
            ("xoxb-1234-5678-abcdef", "provider token prefix"),
            ("AKIAIOSFODNN7EXAMPLE", "provider token prefix"),
            ("-----BEGIN OPENSSH PRIVATE KEY-----", "PEM block"),
        ] {
            assert_eq!(literal_credential_rule(text), Some(rule), "{text:?}");
        }
    }

    #[test]
    fn text_that_names_a_credential_source_is_not_a_literal_credential() {
        for text in [
            "Token via SOLIDTIME_API_KEY, or WORKLOG_ENV_FILE=.env containing TOKEN.",
            "export TOKEN=$SOLIDTIME_API_KEY",
            "api key: SOLIDTIME_API_KEY",
            "token: ~/.config/solidtime/token",
            "password=/run/secrets/db_password",
            "Authorization: Bearer $T",
            "uses a bearer token from the keychain",
            "the secret is stored in the keychain",
            "set max_tokens=4096; token_count: 12; tokens: 1200",
            "task-runner, risk-assessment-notes and ask-before-edit are skills",
            "Nakiajarvi is the test fixture city",
            "the private key lives in ~/.ssh/id_ed25519",
        ] {
            assert_eq!(literal_credential_rule(text), None, "{text:?}");
        }
    }

    #[test]
    fn oversized_line_count_is_truncated_with_head_and_tail_kept() {
        let content: String = (1..=2000).map(|n| format!("line {n}\n")).collect();
        let out = truncate_tool_output("grep", content);

        assert!(
            out.contains("line 1\n"),
            "head must survive, got head missing"
        );
        assert!(
            out.contains("line 2000"),
            "tail must survive, got tail missing"
        );
        assert!(
            out.contains("[Output truncated:"),
            "must carry an explicit marker"
        );
        assert!(
            out.len() < 2000 * 8,
            "result must actually be smaller than the input"
        );
    }

    #[test]
    fn oversized_byte_count_is_truncated_even_with_few_lines() {
        // A handful of very long lines can exceed the byte cap without
        // exceeding the line cap — must still be bounded.
        let content = format!("{}\n{}\n", "a".repeat(40_000), "b".repeat(40_000));
        let out = truncate_tool_output("run_command", content);
        assert!(out.contains("[Output truncated:"));
        assert!(
            out.len() <= MAX_TOOL_OUTPUT_BYTES,
            "bounded output was {} bytes",
            out.len()
        );
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
        let content: String = (1..=2000)
            .map(|n| format!("line {n}: {}\n", "x".repeat(100)))
            .collect();
        let out = truncate_tool_output("run_command", content);

        assert!(out.contains("[Output truncated:"));
        assert!(
            out.len() <= MAX_TOOL_OUTPUT_BYTES,
            "bounded output was {} bytes",
            out.len()
        );
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
        let mut content: String = (1..=2000)
            .map(|n| format!("build progress {n}\n"))
            .collect();
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
    fn clipped_mutation_output_preserves_completion_semantics() {
        let content: String = (1..=2000).map(|n| format!("diff line {n}\n")).collect();
        let bounded = truncate_tool_output_for_message_with_completion(
            "write_to_file",
            content,
            "write_to_file: ",
            Some(COMPLETED_MUTATION_NOTICE),
        );

        assert!(bounded.truncated);
        assert!(bounded.content.contains(COMPLETED_MUTATION_MARKER));
        assert!(bounded.content.contains("do not retry the mutation"));
        assert!(bounded.content.contains(INCOMPLETE_TOOL_RESULT_MARKER));
    }

    #[test]
    fn exact_follow_up_read_recovers_the_full_content() {
        let content: String = (1..=2000).map(|n| format!("line {n}\n")).collect();
        let out = truncate_tool_output("grep", content.clone());
        let marker = "Full output saved to: ";
        let start = out
            .find(marker)
            .expect("truncation marker names the saved path")
            + marker.len();
        let path = out[start..].lines().next().expect("path on its own line");
        let recovered = std::fs::read_to_string(path).expect("saved file readable");
        assert!(
            !out.contains(&content),
            "bounded output must not contain the full payload"
        );
        assert_eq!(
            recovered, content,
            "saved artifact must be byte-identical to the original"
        );
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

        assert_ne!(
            paths[0], paths[1],
            "same-name outputs must not share an artifact path"
        );
        let recovered = [
            std::fs::read(paths[0]).unwrap_or_else(|error| {
                panic!("failed to read first artifact {}: {error}", paths[0])
            }),
            std::fs::read(paths[1]).unwrap_or_else(|error| {
                panic!("failed to read second artifact {}: {error}", paths[1])
            }),
        ];
        assert!(recovered.contains(&first.into_bytes()));
        assert!(recovered.contains(&second.into_bytes()));
    }
}
