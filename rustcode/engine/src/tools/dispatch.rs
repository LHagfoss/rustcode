use super::schema::{mcp_canonical_name_for_clients, mcp_raw_name_is_unique};
use super::{
    CommandProgressCallback, TOOLS, ToolErrorKind, ToolExecutionOutput, audio, exec, filesystem,
    search, video,
};
use serde_json::Value;
use std::sync::Arc;

/// Present a handler failure as the model-facing `error:` line.
///
/// Handlers are inconsistent about whether their message already opens with
/// `error:`, and prefixing unconditionally produced `error: error: ...`, which
/// reads like the harness lost track of its own output.
pub(super) fn as_error_message(message: &str) -> String {
    let trimmed = message.trim_start();
    if trimmed.to_ascii_lowercase().starts_with("error:") {
        trimmed.to_string()
    } else {
        format!("error: {trimmed}")
    }
}

/// Extract model-facing text from an MCP tool result. MCP servers may expose
/// their payload in `structuredContent` without duplicating it in `content`;
/// preserve the structured value in that case so RustCode does not hand the
/// model an empty tool result.
fn mcp_result_content(value: &Value) -> String {
    let result = value.get("result");
    let text = result
        .and_then(|r| r.get("content"))
        .and_then(Value::as_array)
        .map(|content| {
            content
                .iter()
                .filter_map(|item| item.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();

    if !text.trim().is_empty() {
        return text;
    }

    if let Some(structured) = result.and_then(|r| r.get("structuredContent")) {
        return serde_json::to_string_pretty(structured).unwrap_or_default();
    }

    serde_json::to_string_pretty(value).unwrap_or_default()
}

/// Execute a tool by name and return its output with metadata.
///
/// MUST be called off the async runtime (e.g. inside `spawn_blocking`):
/// MCP calls block on the runtime handle and will panic if invoked on a
/// runtime worker thread.
pub(crate) fn execute_with_metadata(name: &str, args: &Value) -> ToolExecutionOutput {
    execute_with_metadata_cancellable(name, args, None)
}

/// Cancellable variant of [`execute_with_metadata`].
///
/// Same off-runtime contract: call only from blocking threads, never from
/// async tasks running on the Tokio runtime.
pub(crate) fn execute_with_metadata_cancellable(
    name: &str,
    args: &Value,
    cancel_token: Option<tokio_util::sync::CancellationToken>,
) -> ToolExecutionOutput {
    execute_with_metadata_cancellable_for_call(name, args, cancel_token, None)
}

/// Call-scoped variant of [`execute_with_metadata_cancellable`].
///
/// Same off-runtime contract: the MCP path uses `Handle::block_on`, so this
/// must run on a blocking thread (see `network/tool_exec.rs` `spawn_blocking`
/// call sites), never directly on a runtime worker.
pub(crate) fn execute_with_metadata_cancellable_for_call(
    name: &str,
    args: &Value,
    cancel_token: Option<tokio_util::sync::CancellationToken>,
    call_id: Option<&str>,
) -> ToolExecutionOutput {
    if let Some(kind) = match name {
        "generate_sound_effect" => Some(audio::GenerationKind::Sfx),
        "generate_music" => Some(audio::GenerationKind::Music),
        _ => None,
    } {
        return match audio::generate_with_cancel(kind, args, cancel_token) {
            Ok(output) => ToolExecutionOutput::success(output),
            Err(error) => ToolExecutionOutput::failure_with_kind(
                as_error_message(&error.message),
                audio::map_error_kind(error.kind),
                audio::is_retryable_error(error.kind),
            ),
        };
    }
    if matches!(
        name,
        "inspect_media" | "validate_video_project" | "render_video"
    ) {
        return execute_video_with_progress(name, args, cancel_token, None);
    }
    // Snapshot the clients and release the registry before the call: the UI
    // locks the same registry to label tool rows, so holding it across a slow
    // `tools/call` freezes rendering and serializes calls to other servers.
    let mcp_clients = crate::mcp::get_mcp_registry()
        .lock()
        .map(|reg| reg.values().cloned().collect::<Vec<_>>())
        .ok();
    if let Some(mut clients) = mcp_clients {
        clients.sort_by(|a, b| a.name.cmp(&b.name));
        for client in &clients {
            if let Ok(tools) = client.get_tools()
                && tools
                    .iter()
                    .find_map(|t| {
                        let raw = t.get("name").and_then(|n| n.as_str())?;
                        let canonical = mcp_canonical_name_for_clients(&client.name, raw, &clients);
                        (name == canonical
                            || (name == raw && mcp_raw_name_is_unique(name, &clients)))
                        .then_some(raw)
                    })
                    .is_some()
            {
                let handle = tokio::runtime::Handle::current();
                let client_clone = Arc::clone(&client);
                let name_owned = name.to_string();
                let args_clone = args.clone();
                let raw_name = tools
                    .iter()
                    .find_map(|tool| {
                        let raw = tool.get("name").and_then(|n| n.as_str())?;
                        let canonical = mcp_canonical_name_for_clients(&client.name, raw, &clients);
                        (name == canonical
                            || (name == raw && mcp_raw_name_is_unique(name, &clients)))
                        .then_some(raw.to_string())
                    })
                    .unwrap_or(name_owned.clone());

                let res = handle.block_on(async move {
                    client_clone
                        .call(
                            "tools/call",
                            serde_json::json!({
                                "name": raw_name,
                                "arguments": args_clone
                            }),
                        )
                        .await
                });

                return match res {
                    Ok(val) => {
                        let success = !val
                            .get("result")
                            .and_then(|result| result.get("isError"))
                            .and_then(Value::as_bool)
                            .unwrap_or(false);
                        ToolExecutionOutput {
                            content: mcp_result_content(&val),
                            success,
                            pending: false,
                            command: None,
                            exit_code: None,
                            truncated: false,
                            completeness: rustcode_core::ToolResultCompleteness::Complete,
                            replayed: false,
                            error_kind: (!success).then_some(ToolErrorKind::McpFailed),
                            retryable: false,
                            command_status: None,
                        }
                    }
                    Err(e) => ToolExecutionOutput::failure_with_kind(
                        format!("error: MCP tool call failed: {e}"),
                        ToolErrorKind::McpFailed,
                        true,
                    ),
                };
            }
        }
    }

    if name == "run_command" {
        return match exec::run_command_output_with_call_id(args, cancel_token, call_id) {
            Ok(output) => output,
            Err(error) => ToolExecutionOutput::failure_with_kind(
                as_error_message(&error),
                ToolErrorKind::CommandFailed,
                true,
            ),
        };
    }
    if name == "view_file" {
        return match filesystem::view_file_output(args) {
            Ok(output) => ToolExecutionOutput {
                content: output.content,
                success: true,
                pending: false,
                command: None,
                exit_code: None,
                truncated: output.truncated,
                completeness: output.completeness,
                replayed: false,
                error_kind: None,
                retryable: false,
                command_status: None,
            },
            Err(error) => ToolExecutionOutput::failure_with_kind(
                as_error_message(&error),
                ToolErrorKind::InvalidArguments,
                false,
            ),
        };
    }
    if name == "zoom_context" {
        return match super::context_archive::zoom(args) {
            Ok(content) => {
                let page: Value = serde_json::from_str(&content).expect("zoom_context emits JSON");
                let mut output = ToolExecutionOutput::success(content);
                if page["complete"] != true {
                    output.truncated = page["next_offset"].as_u64().is_some();
                    output.completeness = if output.truncated {
                        rustcode_core::ToolResultCompleteness::ByteTruncated
                    } else {
                        rustcode_core::ToolResultCompleteness::UserLimited
                    };
                }
                output
            }
            Err(error) => ToolExecutionOutput::failure(as_error_message(&error)),
        };
    }
    if name == "get_project_map" {
        return search::get_project_map_execution_output(args).unwrap_or_else(|error| {
            ToolExecutionOutput::failure_with_kind(
                as_error_message(&error),
                ToolErrorKind::InvalidArguments,
                false,
            )
        });
    }
    if matches!(name, "grep" | "glob" | "list_directory") {
        let result = match name {
            "grep" => search::grep_execution_output(args),
            "glob" => search::glob_execution_output(args),
            "list_directory" => search::list_directory_output(args),
            _ => unreachable!(),
        };
        return result.unwrap_or_else(|error| {
            ToolExecutionOutput::failure_with_kind(
                as_error_message(&error),
                ToolErrorKind::InvalidArguments,
                false,
            )
        });
    }

    match TOOLS.iter().find(|t| t.name == name) {
        Some(tool) => match (tool.handler)(args) {
            Ok(out) => ToolExecutionOutput::success(out),
            // A refused note is the caller's to reword, not a harness fault.
            Err(e) if name == "remember" && crate::memory::is_refusal(&e) => {
                ToolExecutionOutput::failure_with_kind(
                    as_error_message(&e),
                    ToolErrorKind::Validation,
                    false,
                )
            }
            Err(e) => ToolExecutionOutput::failure(as_error_message(&e)),
        },
        None => {
            let suggestion = super::fuzzy_match_tool_name(name)
                .map(|close| format!(" Did you mean '{close}'?"))
                .unwrap_or_default();
            ToolExecutionOutput::failure_with_kind(
                format!(
                    "error: unknown tool '{name}'.{suggestion} Available: {}",
                    TOOLS.iter().map(|t| t.name).collect::<Vec<_>>().join(", ")
                ),
                ToolErrorKind::UnavailableDependency,
                false,
            )
        }
    }
}

pub(crate) fn execute_video_with_progress(
    name: &str,
    args: &Value,
    cancel_token: Option<tokio_util::sync::CancellationToken>,
    progress: Option<CommandProgressCallback>,
) -> ToolExecutionOutput {
    match video::execute_with_cancel_and_progress(name, args, cancel_token, progress) {
        Ok(output) => ToolExecutionOutput::success(output),
        Err(error) => ToolExecutionOutput::failure_with_kind(
            as_error_message(&error.message),
            video::map_error_kind(error.kind),
            matches!(
                error.kind,
                video::VideoErrorKind::ProcessFailed | video::VideoErrorKind::Cancelled
            ),
        ),
    }
}

#[allow(
    dead_code,
    reason = "preserved display-only interface for direct callers"
)]
pub fn execute(name: &str, args: &Value) -> String {
    execute_with_metadata(name, args).content
}

pub fn needs_confirmation(name: &str) -> bool {
    TOOLS
        .iter()
        .find(|t| t.name == name)
        .map(|t| t.requires_confirmation)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::mcp_result_content;

    #[test]
    fn refused_memory_note_is_a_validation_error() {
        // Refused before anything is read or written, so no store is touched.
        let output = super::execute_with_metadata(
            "remember",
            &serde_json::json!({"key": "api_key", "value": "0a1b2c3d4e5f"}),
        );
        assert!(!output.success);
        assert_eq!(output.error_kind, Some(super::ToolErrorKind::Validation));
        assert!(output.content.contains("(rule: credential assignment)"));
        assert!(!output.content.contains("0a1b2c3d4e5f"));
    }

    #[test]
    fn structured_only_mcp_results_reach_the_model() {
        let result = serde_json::json!({
            "result": {
                "structuredContent": {"mailboxes": [{"name": "INBOX"}]},
                "content": []
            }
        });
        let content = mcp_result_content(&result);
        assert!(content.contains("mailboxes"));
        assert!(content.contains("INBOX"));
    }

    #[test]
    fn ordinary_mcp_text_content_is_not_duplicated() {
        let result = serde_json::json!({
            "result": {
                "structuredContent": {"value": 1},
                "content": [{"type": "text", "text": "already formatted"}]
            }
        });
        assert_eq!(mcp_result_content(&result), "already formatted");
    }
}
