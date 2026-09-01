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

pub(crate) fn execute_with_metadata(name: &str, args: &Value) -> ToolExecutionOutput {
    execute_with_metadata_cancellable(name, args, None)
}

pub(crate) fn execute_with_metadata_cancellable(
    name: &str,
    args: &Value,
    cancel_token: Option<tokio_util::sync::CancellationToken>,
) -> ToolExecutionOutput {
    execute_with_metadata_cancellable_for_call(name, args, cancel_token, None)
}

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
    if let Ok(reg) = crate::mcp::get_mcp_registry().lock() {
        let mut clients = reg.values().cloned().collect::<Vec<_>>();
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
                        if let Some(content_arr) = val
                            .get("result")
                            .and_then(|r| r.get("content"))
                            .and_then(|c| c.as_array())
                        {
                            let mut text_parts = Vec::new();
                            for item in content_arr {
                                if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                                    text_parts.push(text.to_string());
                                }
                            }
                            ToolExecutionOutput {
                                content: text_parts.join("\n"),
                                success,
                                pending: false,
                                command: None,
                                exit_code: None,
                                truncated: false,
                                completeness: rustcode_core::ToolResultCompleteness::Complete,
                                replayed: false,
                                error_kind: (!success).then_some(ToolErrorKind::McpFailed),
                                retryable: false,
                            }
                        } else {
                            ToolExecutionOutput {
                                content: serde_json::to_string_pretty(&val).unwrap_or_default(),
                                success,
                                pending: false,
                                command: None,
                                exit_code: None,
                                truncated: false,
                                completeness: rustcode_core::ToolResultCompleteness::Complete,
                                replayed: false,
                                error_kind: (!success).then_some(ToolErrorKind::McpFailed),
                                retryable: false,
                            }
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
            },
            Err(error) => ToolExecutionOutput::failure_with_kind(
                as_error_message(&error),
                ToolErrorKind::InvalidArguments,
                false,
            ),
        };
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
            Err(e) => ToolExecutionOutput::failure(as_error_message(&e)),
        },
        None => ToolExecutionOutput::failure_with_kind(
            format!(
                "error: unknown tool '{name}'. Available: {}",
                TOOLS.iter().map(|t| t.name).collect::<Vec<_>>().join(", ")
            ),
            ToolErrorKind::UnavailableDependency,
            false,
        ),
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
