use crate::app::{AppState, AppStatus, StreamTracker, ToolConfirmation};
use std::sync::Arc;
use tokio::sync::Mutex;

use super::compiler::{append_compiler_diagnostics, cached_compiler_check, run_compiler_check};
use super::events::{ToolResult, ToolResultMetadata};
use super::subagents::handle_agent_tool;
use super::{
    REPLAYABLE_READ_LIMIT, is_mutating_tool, mutation_made_progress, path_mtime, tool_signature,
    view_file_unchanged_since_last_read,
};

#[cfg(test)]
#[path = "tool_exec/compiler_tests.rs"]
mod compiler_tests;
#[path = "tool_exec/preview.rs"]
mod preview;
#[path = "tool_exec/replay.rs"]
mod replay;
#[path = "tool_exec/result.rs"]
mod result;

#[cfg(test)]
pub(crate) use preview::extract_diff_block;
pub(crate) use preview::{
    final_tool_diff, get_diff_preview, get_tool_project_root,
    tool_result_precludes_preview_fallback,
};
pub(crate) use result::{
    bounded_tool_result_history_message, compact_replayed_read_result, finalize_tool_result,
    replay_cached_view_file_subrange, stable_arguments_hash, subagent_tool_history_message,
    tool_result_from_execution, tool_result_history_message,
};

fn cached_read_covers_request(
    cached: &crate::app::CachedReadOutput,
    tool_name: &str,
    args: &serde_json::Value,
) -> bool {
    if !cached.success
        || cached.truncated
        || cached.replayable_content.is_none()
        || !matches!(
            cached.completeness,
            rustcode_core::ToolResultCompleteness::Complete
                | rustcode_core::ToolResultCompleteness::UserLimited
        )
    {
        return false;
    }
    let Some(inspection) = cached.inspection.as_ref() else {
        return false;
    };
    let Some((requested_path, requested_start, requested_end)) =
        crate::network::loop_detect::read_target(tool_name, args)
    else {
        return false;
    };
    let Some(requested_end) = requested_end else {
        // An omitted end_line means read through the file's end. A cached
        // finite range cannot prove that request is complete.
        return false;
    };
    let requested_offset = args
        .get("content_offset")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    if cached.content_offset != requested_offset {
        return false;
    }
    let stored_path = inspection
        .returned_path
        .as_ref()
        .or(inspection.requested_path.as_ref());
    if stored_path != Some(&requested_path) {
        return false;
    }
    let Some(stored_range) = inspection
        .returned_range
        .as_ref()
        .or(inspection.requested_range.as_ref())
    else {
        return false;
    };
    let stored_start = stored_range.start.unwrap_or(1);
    let stored_end = stored_range.end.unwrap_or(u64::MAX);
    (requested_start as u64) >= stored_start && requested_end as u64 <= stored_end
}

fn replay_inspection_for_request(
    cached: &crate::app::CachedReadOutput,
    tool_name: &str,
    args: &serde_json::Value,
) -> Option<rustcode_core::InspectionResultMetadata> {
    let mut inspection = cached.inspection.clone()?;
    if let Some((path, start, end)) = crate::network::loop_detect::read_target(tool_name, args) {
        inspection.requested_path = Some(path);
        inspection.requested_range = Some(rustcode_core::InspectionRange {
            start: Some(start as u64),
            end: end.map(|value| value as u64),
        });
        if cached_read_covers_request(cached, tool_name, args) {
            inspection.returned_range = inspection.requested_range.clone();
        }
    }
    Some(inspection)
}

/// A parsed `ask_question` entry: header, question, `(label, description)`
/// options, and whether several options may be ticked.
type ParsedQuestion = (String, String, Vec<(String, String)>, bool);

fn parse_question_option(raw: &serde_json::Value) -> Option<(String, String)> {
    if let Some(label) = raw.as_str() {
        return Some((label.to_owned(), String::new()));
    }
    let object = raw.as_object()?;
    let label = object
        .get("label")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_owned();
    let description = object
        .get("description")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_owned();
    if label.trim().is_empty() && description.trim().is_empty() {
        return None;
    }
    Some((label, description))
}

fn parse_question_object(raw: &serde_json::Value) -> Option<ParsedQuestion> {
    let object = raw.as_object()?;
    let header = object
        .get("header")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_owned();
    let question = object
        .get("question")
        .or_else(|| object.get("prompt"))
        .or_else(|| object.get("message"))
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_owned();
    let options = object
        .get("options")
        .and_then(|value| value.as_array())
        .map(|items| items.iter().filter_map(parse_question_option).collect())
        .unwrap_or_default();
    let multi = object
        .get("multiple")
        .or_else(|| object.get("is_multi_select"))
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    Some((header, question, options, multi))
}

/// Parse an `ask_question` call into a question chain. Accepts the chained
/// opencode-style `questions: [{header, question, options: [{label,
/// description}], multiple}]` shape first, then the legacy flat
/// `question`/`options`/`is_multi_select` shape (a one-question chain).
fn parse_question_chain(args: &serde_json::Value) -> Vec<ParsedQuestion> {
    if let Some(items) = args.get("questions").and_then(|value| value.as_array()) {
        let chained = items
            .iter()
            .filter_map(parse_question_object)
            .collect::<Vec<_>>();
        if !chained.is_empty() {
            return chained;
        }
    }
    let single =
        parse_question_object(args).unwrap_or((String::new(), String::new(), Vec::new(), false));
    // The legacy shape nests nothing: a failed object parse still yields one
    // question slot so the defaults below apply.
    let multi = args
        .get("is_multi_select")
        .and_then(|value| value.as_bool())
        .unwrap_or(single.3);
    vec![(single.0, single.1, single.2, multi)]
}

/// Map the answered-question channel payload to a tool result. The channel
/// already carries the fully formatted submission (`User selected: …` /
/// `User answers: …`); the legacy cancel text maps back to a typed
/// cancellation instead of a bogus successful selection.
pub(crate) fn map_question_channel_result(
    answer: Option<String>,
) -> crate::tools::ToolExecutionOutput {
    match answer {
        Some(text) if text.trim() == "User cancelled prompt." => {
            crate::tools::ToolExecutionOutput::failure_with_kind(
                "User cancelled or provided no selection.".to_string(),
                crate::tools::ToolErrorKind::Cancelled,
                true,
            )
        }
        Some(text) if !text.is_empty() => crate::tools::ToolExecutionOutput::success(text),
        _ => crate::tools::ToolExecutionOutput::failure_with_kind(
            "User cancelled or provided no selection.".to_string(),
            crate::tools::ToolErrorKind::Cancelled,
            true,
        ),
    }
}

pub(crate) async fn ask_user_question(
    state: &Arc<Mutex<AppState>>,
    cancel_token: &tokio_util::sync::CancellationToken,
    args: &serde_json::Value,
) -> (crate::tools::ToolExecutionOutput, std::time::Duration) {
    let questions = parse_question_chain(args)
        .into_iter()
        .map(|(header, question, options, multi)| {
            let question = if question.trim().is_empty() {
                "Please confirm how to proceed:".to_owned()
            } else {
                question
            };
            let (labels, descriptions): (Vec<String>, Vec<String>) = options.into_iter().unzip();
            let (labels, descriptions) = if labels.is_empty() {
                (vec!["Proceed".to_owned(), "Cancel".to_owned()], Vec::new())
            } else {
                (labels, descriptions)
            };
            crate::app::PendingQuestion::new(question, labels, multi)
                .with_header(header)
                .with_descriptions(descriptions)
        })
        .collect::<Vec<_>>();

    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    {
        let mut s = state.lock().await;
        s.begin_question_chain(questions);
        s.question_response = Some(tx);
        s.status = AppStatus::AwaitingQuestion;
        s.request_redraw();
    }
    let _ = crate::notifications::notify_pending_confirmation("ask_question");

    let start_wait = std::time::Instant::now();
    let answer = tokio::select! {
        _ = cancel_token.cancelled() => None,
        res = rx => res.ok(),
    };
    let user_wait = start_wait.elapsed();

    {
        let mut s = state.lock().await;
        let pending_changed = s.pending_question.take().is_some();
        s.question_response = None;
        s.pending_question_queue.clear();
        s.pending_question_done.clear();
        let status_changed = if s.status == AppStatus::AwaitingQuestion {
            s.status = AppStatus::Streaming;
            true
        } else {
            false
        };
        if pending_changed || status_changed {
            s.request_redraw();
        }
    }

    // The channel already carries the fully formatted submission
    // (`User selected: …` / `User answers: …`); the legacy cancel text maps
    // back to a typed cancellation instead of a bogus successful selection.
    let out = map_question_channel_result(answer);
    (out, user_wait)
}

pub(crate) async fn confirm_and_execute(
    client: &reqwest::Client,
    state: &Arc<Mutex<AppState>>,
    cancel_token: &tokio_util::sync::CancellationToken,
    name: &str,
    args: &serde_json::Value,
    display_name: &str,
    bypass_confirm: bool,
    workspace_root: Option<std::path::PathBuf>,
    live_key: Option<&str>,
) -> (
    crate::tools::ToolExecutionOutput,
    Option<String>,
    std::time::Duration,
) {
    let (mut result, diff, user_wait) = confirm_and_execute_for_call(
        client,
        state,
        cancel_token,
        name,
        args,
        display_name,
        bypass_confirm,
        workspace_root,
        live_key,
        None,
    )
    .await;

    // Standalone subagent calls have no batch-level compiler check.
    if matches!(
        name,
        "replace_file_content"
            | "multi_replace_file_content"
            | "write_to_file"
            | "delete_file"
            | "move_file"
            | "copy_file"
    ) && result.success
        && let Some(cwd) = get_tool_project_root(name, args)
        && let Some(errors) = run_compiler_check(&cwd, cancel_token).await
    {
        result.content.push_str("\n\nCompiler errors/warnings:\n");
        result.content.push_str(&errors);
        if !errors.starts_with("__BUILD_UNVERIFIED__") {
            result.error_kind = Some(crate::tools::ToolErrorKind::CompilerFailed);
            result.retryable = true;
        }
    }

    (result, diff, user_wait)
}

pub(crate) async fn confirm_and_execute_for_call(
    client: &reqwest::Client,
    state: &Arc<Mutex<AppState>>,
    cancel_token: &tokio_util::sync::CancellationToken,
    name: &str,
    args: &serde_json::Value,
    display_name: &str,
    bypass_confirm: bool,
    workspace_root: Option<std::path::PathBuf>,
    live_key: Option<&str>,
    call_id: Option<&str>,
) -> (
    crate::tools::ToolExecutionOutput,
    Option<String>,
    std::time::Duration,
) {
    confirm_and_execute_for_call_with_assessment(
        client,
        state,
        cancel_token,
        name,
        args,
        display_name,
        bypass_confirm,
        workspace_root,
        live_key,
        call_id,
        None,
    )
    .await
}

pub(crate) async fn confirm_and_execute_for_call_with_assessment(
    client: &reqwest::Client,
    state: &Arc<Mutex<AppState>>,
    cancel_token: &tokio_util::sync::CancellationToken,
    name: &str,
    args: &serde_json::Value,
    display_name: &str,
    bypass_confirm: bool,
    workspace_root: Option<std::path::PathBuf>,
    live_key: Option<&str>,
    call_id: Option<&str>,
    assessment: Option<crate::tools::ShellAssessment>,
) -> (
    crate::tools::ToolExecutionOutput,
    Option<String>,
    std::time::Duration,
) {
    let normalized_args;
    let args = if let Some(requested) = args.get("filesystem_write_path") {
        let Some(path) = requested.as_str() else {
            return (
                crate::tools::ToolExecutionOutput::failure_with_kind(
                    "error: filesystem_write_path must be an absolute directory string".to_string(),
                    crate::tools::ToolErrorKind::PermissionDenied,
                    false,
                ),
                None,
                std::time::Duration::ZERO,
            );
        };
        let Some(workspace) = workspace_root.as_deref() else {
            return (
                crate::tools::ToolExecutionOutput::failure_with_kind(
                    "error: one-shot filesystem permission requires an active workspace"
                        .to_string(),
                    crate::tools::ToolErrorKind::PermissionDenied,
                    false,
                ),
                None,
                std::time::Duration::ZERO,
            );
        };
        let resolved =
            match crate::tools::exec::sandbox::resolve_scoped_writable_root(path, workspace) {
                Ok(path) => path,
                Err(error) => {
                    return (
                        crate::tools::ToolExecutionOutput::failure_with_kind(
                            format!("error: {error}"),
                            crate::tools::ToolErrorKind::PermissionDenied,
                            false,
                        ),
                        None,
                        std::time::Duration::ZERO,
                    );
                }
            };
        normalized_args = {
            let mut args = args.clone();
            args.as_object_mut()
                .expect("tool arguments are JSON objects")
                .insert(
                    "filesystem_write_path".to_string(),
                    serde_json::Value::String(resolved.display().to_string()),
                );
            args
        };
        &normalized_args
    } else {
        args
    };
    let (agent_mode, auto_confirm, task_working_directory) = {
        let s = state.lock().await;
        (
            s.agent_mode,
            s.auto_confirm,
            s.task_working_directory
                .clone()
                .or_else(|| s.workspace_root.clone()),
        )
    };
    let mut authorization = crate::tools::execution_authorization(
        name,
        args,
        call_id,
        agent_mode,
        auto_confirm,
        bypass_confirm,
        assessment.as_ref(),
    );
    let denied_prefix_covers_call = if name == "run_command" {
        let prefixes = state.lock().await.config.denied_command_prefixes.clone();
        crate::tools::denied_command_prefix_covers_call(name, args, &prefixes)
    } else {
        false
    };
    if denied_prefix_covers_call {
        authorization = crate::tools::AuthorizationDecision::Deny(
            "command blocked by a saved user forbid rule".to_string(),
        );
    }
    // Subagent tool calls use this per-call confirmation path instead of the
    // parent turn's batch policy. Honor the same explicit user-approved
    // command rules here, while keeping Plan-mode denial and every other Deny
    // decision intact. The helpers reject environment/background variants and
    // unsafe shell composition before allowing a rule to match.
    let saved_prefix_covers_call = if name == "run_command" {
        let prefixes = state.lock().await.config.approved_command_prefixes.clone();
        crate::tools::approved_command_prefix_covers_call(name, args, &prefixes)
    } else {
        false
    };
    if !denied_prefix_covers_call
        && matches!(
            &authorization,
            crate::tools::AuthorizationDecision::RequireConfirmation
        )
        && saved_prefix_covers_call
    {
        authorization = crate::tools::AuthorizationDecision::Allow;
    }
    if let crate::tools::AuthorizationDecision::Deny(reason) = authorization.clone() {
        return (
            crate::tools::ToolExecutionOutput::failure_with_kind(
                format!("error: {reason}"),
                crate::tools::ToolErrorKind::PermissionDenied,
                false,
            ),
            None,
            std::time::Duration::ZERO,
        );
    }

    struct ToolCleanup {
        state: Arc<Mutex<AppState>>,
        tool_name: String,
    }
    impl Drop for ToolCleanup {
        fn drop(&mut self) {
            let state = self.state.clone();
            let tool_name = self.tool_name.clone();
            tokio::spawn(async move {
                let mut s = state.lock().await;
                if let Some(pos) = s.running_tools.iter().position(|t| t == &tool_name) {
                    s.running_tools.remove(pos);
                }
            });
        }
    }

    let diff_opt = get_diff_preview(name, args);

    let needs_confirm = matches!(
        authorization,
        crate::tools::AuthorizationDecision::RequireConfirmation
    );
    let mut user_wait_dur = std::time::Duration::ZERO;
    let mut confirmation_transition_redrawn = false;
    let result = if !needs_confirm {
        dbg_log!("Executing tool '{}' immediately...", name);
        let tool_name = name.to_string();
        {
            let mut s = state.lock().await;
            s.running_tools.push(tool_name.clone());
        }
        let _cleanup = ToolCleanup {
            state: Arc::clone(state),
            tool_name,
        };

        let name_owned = name.to_string();
        let args_owned = args.clone();
        let call_id_owned = call_id.map(str::to_owned);
        let session_id = { state.lock().await.active_session_id.clone() };
        let sandbox_mode_for_task = { state.lock().await.config.sandbox_mode };
        let workspace_root_for_task = workspace_root.clone();
        let task_working_directory_for_task = task_working_directory.clone();
        let live_key_owned = live_key.map(str::to_owned);
        let cancel_token_for_task = cancel_token.clone();
        let client_for_task = client.clone();
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel();
        let run_fut = async move {
            if name_owned == "search_web" {
                return match crate::tools::search_web_async(&args_owned, &client_for_task).await {
                    Ok(output) => crate::tools::ToolExecutionOutput::success(output),
                    Err(error) => {
                        crate::tools::ToolExecutionOutput::failure(format!("error: {error}"))
                    }
                };
            }

            let mcp_registry = crate::mcp::get_mcp_registry();
            tokio::task::spawn_blocking(move || {
                crate::mcp::DIRECT_MCP_REGISTRY.sync_scope(mcp_registry, || {
                    crate::tools::set_active_session_id(Some(session_id));
                    crate::tools::set_active_workspace_context(
                        workspace_root_for_task,
                        task_working_directory_for_task,
                        false,
                        Some(sandbox_mode_for_task),
                    );
                    let result = if name_owned == "run_command" && live_key_owned.is_some() {
                        let callback: crate::tools::CommandProgressCallback =
                            Arc::new(move |bytes, stderr| {
                                let _ = progress_tx.send((bytes.to_vec(), stderr));
                            });
                        crate::tools::run_command_output_with_progress_cancellable_for_call(
                            &args_owned,
                            callback,
                            Some(cancel_token_for_task),
                            call_id_owned.as_deref(),
                        )
                        .unwrap_or_else(|error| {
                            crate::tools::ToolExecutionOutput::failure_with_kind(
                                format!("error: {error}"),
                                crate::tools::ToolErrorKind::CommandFailed,
                                true,
                            )
                        })
                    } else if name_owned == "render_video" && live_key_owned.is_some() {
                        let callback: crate::tools::CommandProgressCallback =
                            Arc::new(move |bytes, stderr| {
                                let _ = progress_tx.send((bytes.to_vec(), stderr));
                            });
                        crate::tools::execute_video_with_progress(
                            &name_owned,
                            &args_owned,
                            Some(cancel_token_for_task),
                            Some(callback),
                        )
                    } else {
                        crate::tools::execute_with_metadata_cancellable_for_call(
                            &name_owned,
                            &args_owned,
                            Some(cancel_token_for_task),
                            call_id_owned.as_deref(),
                        )
                    };
                    crate::tools::set_active_workspace_context(None, None, false, None);
                    crate::tools::set_active_session_id(None);
                    result
                })
            })
            .await
            .unwrap_or_else(|e| {
                crate::tools::ToolExecutionOutput::failure(format!("tool panicked: {e}"))
            })
        };
        tokio::pin!(run_fut);
        let is_cancellable_process = matches!(
            name,
            "generate_sound_effect"
                | "generate_music"
                | "inspect_media"
                | "validate_video_project"
                | "render_video"
                | "run_command"
        );
        let mut progress_open = true;
        loop {
            tokio::select! {
                res = &mut run_fut => {
                    break res;
                }
                event = progress_rx.recv(), if progress_open => {
                    if let Some((bytes, stderr)) = event {
                        if let Some(key) = live_key {
                            state.lock().await.append_live_tool_output(key, &bytes, stderr);
                        }
                    } else {
                        progress_open = false;
                    }
                }
                _ = cancel_token.cancelled(), if !is_cancellable_process => {
                    dbg_log!("Tool execution cancelled during spawn_blocking await (immediate execution)");
                    break crate::tools::ToolExecutionOutput::failure_with_kind(
                        "error: tool execution cancelled by user".to_string(),
                        crate::tools::ToolErrorKind::Cancelled,
                        true,
                    );
                }
            }
        }
    } else {
        dbg_log!("Tool '{}' requires confirmation", name);
        let path = if let Some(p) = args
            .get("path")
            .or_else(|| args.get("output_path"))
            .or_else(|| args.get("project_path"))
            .and_then(|p| p.as_str())
        {
            p.to_string()
        } else if let Some(cmd) = args.get("command").and_then(|c| c.as_str()) {
            cmd.to_string()
        } else if let (Some(src), Some(dest)) = (
            args.get("src").and_then(|s| s.as_str()),
            args.get("dest").and_then(|d| d.as_str()),
        ) {
            format!("{src} -> {dest}")
        } else {
            "?".to_string()
        };
        let render_preview = (name == "render_video")
            .then(|| crate::tools::render_confirmation_preview(args, workspace_root.as_deref()))
            .flatten();
        let (preview, content_bytes) = if let Some(preview) = render_preview {
            let content_bytes = preview.len();
            (preview, content_bytes)
        } else if let Some(ref d) = diff_opt {
            (d.clone(), d.len())
        } else {
            if name == "run_command" {
                let command = args
                    .get("command")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                let sandbox_mode = state.lock().await.config.sandbox_mode;
                let one_shot_network_access = args
                    .get("network_access")
                    .and_then(serde_json::Value::as_bool)
                    == Some(true);
                let one_shot_filesystem_write_path = args
                    .get("filesystem_write_path")
                    .and_then(serde_json::Value::as_str);
                (
                    crate::tools::command_confirmation_preview(
                        command,
                        sandbox_mode,
                        one_shot_network_access,
                        one_shot_filesystem_write_path,
                    ),
                    command.len(),
                )
            } else {
                let content = args.get("content").and_then(|c| c.as_str()).unwrap_or("");
                let preview = content.lines().take(6).collect::<Vec<_>>().join("\n");
                (preview, content.len())
            }
        };
        let (tx, rx) = tokio::sync::oneshot::channel::<crate::app::ToolConfirmationResponse>();
        {
            let mut s = state.lock().await;
            s.modal_scroll_row = 0;
            s.tool_confirmation_selected = 0;
            s.pending_tool_confirmation = Some(vec![ToolConfirmation {
                request_id: Some(
                    call_id
                        .map(str::to_owned)
                        .unwrap_or_else(|| format!("local:{}", stable_arguments_hash(args))),
                ),
                tool_name: display_name.to_string(),
                path,
                content_preview: preview,
                content_bytes,
                rememberable_prefix: (name == "run_command")
                    .then(|| crate::tools::rememberable_command_prefix_for_call(args))
                    .flatten(),
                forbidden_prefix: (name == "run_command")
                    .then(|| crate::tools::rememberable_command_forbid_prefix_for_call(args))
                    .flatten(),
            }]);
            s.tool_confirmation_response = Some(tx);
            s.status = AppStatus::AwaitingToolConfirmation;
            s.request_redraw();
        }
        let _ = crate::notifications::notify_pending_confirmation(name);
        dbg_log!("Awaiting user confirmation for '{}'", name);
        let start_wait = std::time::Instant::now();
        let rx_res = rx.await;
        user_wait_dur = start_wait.elapsed();

        if let Ok(crate::app::ToolConfirmationResponse::ApproveAndRemember(prefix)) = &rx_res
            && name == "run_command"
            && crate::tools::rememberable_command_prefix_for_call(args).as_deref()
                == Some(prefix.as_str())
        {
            let mut state = state.lock().await;
            let stored_prefix = crate::tools::persisted_approved_command_prefix(prefix);
            if !state
                .config
                .approved_command_prefixes
                .contains(&stored_prefix)
            {
                state.config.approved_command_prefixes.push(stored_prefix);
                crate::config::save_entire_config(&state.config);
            }
        }
        if let Ok(crate::app::ToolConfirmationResponse::ForbidAndRemember(prefix)) = &rx_res
            && name == "run_command"
            && crate::tools::rememberable_command_forbid_prefix_for_call(args).as_deref()
                == Some(prefix.as_str())
        {
            let mut state = state.lock().await;
            if !state.config.denied_command_prefixes.contains(prefix) {
                state.config.denied_command_prefixes.push(prefix.clone());
                crate::config::save_entire_config(&state.config);
            }
        }

        let res = match rx_res {
            Ok(crate::app::ToolConfirmationResponse::ForbidAndRemember(_)) => {
                let mut s = state.lock().await;
                s.pending_tool_confirmation = None;
                s.status = AppStatus::Streaming;
                s.request_redraw();
                confirmation_transition_redrawn = true;
                crate::tools::ToolExecutionOutput::failure_with_kind(
                    "error: command blocked by a saved user forbid rule".to_string(),
                    crate::tools::ToolErrorKind::PermissionDenied,
                    false,
                )
            }
            Ok(
                crate::app::ToolConfirmationResponse::Approve
                | crate::app::ToolConfirmationResponse::ApproveAndRemember(_),
            ) => {
                dbg_log!("User approved tool call '{}', executing...", name);
                let tool_name = name.to_string();
                {
                    let mut s = state.lock().await;
                    s.pending_tool_confirmation = None;
                    s.status = AppStatus::Streaming;
                    s.stream_tracker = Some(StreamTracker::new());
                    s.running_tools.push(tool_name.clone());
                    s.request_redraw();
                }
                confirmation_transition_redrawn = true;
                let _cleanup = ToolCleanup {
                    state: Arc::clone(state),
                    tool_name,
                };

                let name_owned = name.to_string();
                let args_owned = args.clone();
                let call_id_owned = call_id.map(str::to_owned);
                let session_id = { state.lock().await.active_session_id.clone() };
                let sandbox_mode_for_task = { state.lock().await.config.sandbox_mode };
                let workspace_root_for_task = workspace_root.clone();
                let task_working_directory_for_task = task_working_directory.clone();
                let cancel_token_for_task = cancel_token.clone();
                let live_key_for_task = live_key.map(str::to_owned);
                let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel();
                let mcp_registry = crate::mcp::get_mcp_registry();
                let run_fut = tokio::task::spawn_blocking(move || {
                    crate::mcp::DIRECT_MCP_REGISTRY.sync_scope(mcp_registry, || {
                        crate::tools::set_active_session_id(Some(session_id));
                        crate::tools::set_active_workspace_context(
                            workspace_root_for_task,
                            task_working_directory_for_task,
                            false,
                            Some(sandbox_mode_for_task),
                        );
                        let result = if name_owned == "render_video" && live_key_for_task.is_some()
                        {
                            let callback: crate::tools::CommandProgressCallback =
                                Arc::new(move |bytes, stderr| {
                                    let _ = progress_tx.send((bytes.to_vec(), stderr));
                                });
                            crate::tools::execute_video_with_progress(
                                &name_owned,
                                &args_owned,
                                Some(cancel_token_for_task),
                                Some(callback),
                            )
                        } else {
                            crate::tools::execute_with_metadata_cancellable_for_call(
                                &name_owned,
                                &args_owned,
                                Some(cancel_token_for_task),
                                call_id_owned.as_deref(),
                            )
                        };
                        crate::tools::set_active_workspace_context(None, None, false, None);
                        crate::tools::set_active_session_id(None);
                        result
                    })
                });
                let is_cancellable_process = matches!(
                    name,
                    "generate_sound_effect"
                        | "generate_music"
                        | "inspect_media"
                        | "validate_video_project"
                        | "render_video"
                        | "run_command"
                );

                tokio::pin!(run_fut);
                let mut progress_open = true;
                loop {
                    tokio::select! {
                        res = &mut run_fut => {
                            break res.unwrap_or_else(|e| {
                                crate::tools::ToolExecutionOutput::failure(format!("tool panicked: {e}"))
                            });
                        }
                        event = progress_rx.recv(), if progress_open => {
                            if let Some((bytes, stderr)) = event {
                                if let Some(key) = live_key {
                                    state.lock().await.append_live_tool_output(key, &bytes, stderr);
                                }
                            } else {
                                progress_open = false;
                            }
                        }
                        _ = cancel_token.cancelled(), if !is_cancellable_process => {
                            dbg_log!("Tool execution cancelled during spawn_blocking await");
                            break crate::tools::ToolExecutionOutput::failure_with_kind(
                                "error: tool execution cancelled by user".to_string(),
                                crate::tools::ToolErrorKind::Cancelled,
                                true,
                            );
                        }
                    }
                }
            }
            Ok(crate::app::ToolConfirmationResponse::Deny) => {
                dbg_log!("User denied tool call '{}'", name);
                let _ = crate::notifications::notify_finished(
                    crate::notifications::FinishedStatus::Denied,
                );
                crate::tools::ToolExecutionOutput::failure_with_kind(
                    "error: user denied this tool call".to_string(),
                    crate::tools::ToolErrorKind::PermissionDenied,
                    false,
                )
            }
            Err(_) => {
                dbg_log!("Confirmation channel closed for '{}'", name);
                crate::tools::ToolExecutionOutput::failure_with_kind(
                    "error: confirmation channel closed".to_string(),
                    crate::tools::ToolErrorKind::Internal,
                    true,
                )
            }
        };
        {
            let mut s = state.lock().await;
            let pending_changed = s.pending_tool_confirmation.take().is_some();
            let status_changed = s.status != AppStatus::Streaming;
            s.status = AppStatus::Streaming;
            s.stream_tracker = Some(StreamTracker::new());
            if !confirmation_transition_redrawn && (pending_changed || status_changed) {
                s.request_redraw();
            }
        }
        res
    };

    (result, diff_opt, user_wait_dur)
}

#[allow(clippy::too_many_arguments)]
#[cfg(test)]
pub(crate) async fn execute_tool_batch(
    client: &reqwest::Client,
    state: &Arc<Mutex<AppState>>,
    cancel_token: &tokio_util::sync::CancellationToken,
    tool_calls: &[crate::tools::ToolCall],
    approved: bool,
    edit_root: &Option<std::path::PathBuf>,
    compile_dirty: &mut bool,
    compile_cache: &mut Option<(std::path::PathBuf, Option<String>)>,
    user_wait_duration: &mut std::time::Duration,
    deferred_notice: Option<String>,
) -> Vec<ToolResult> {
    execute_tool_batch_with_assessments(
        client,
        state,
        cancel_token,
        tool_calls,
        approved,
        edit_root,
        compile_dirty,
        compile_cache,
        user_wait_duration,
        deferred_notice,
        &Default::default(),
    )
    .await
}

pub(crate) async fn execute_tool_batch_with_assessments(
    client: &reqwest::Client,
    state: &Arc<Mutex<AppState>>,
    cancel_token: &tokio_util::sync::CancellationToken,
    tool_calls: &[crate::tools::ToolCall],
    approved: bool,
    edit_root: &Option<std::path::PathBuf>,
    compile_dirty: &mut bool,
    compile_cache: &mut Option<(std::path::PathBuf, Option<String>)>,
    user_wait_duration: &mut std::time::Duration,
    deferred_notice: Option<String>,
    assessment_cache: &crate::tools::ShellAssessmentCache,
) -> Vec<ToolResult> {
    if cancel_token.is_cancelled() {
        return tool_calls.iter().map(cancelled_tool_result).collect();
    }

    if !approved {
        return tool_calls
            .iter()
            .map(|call| ToolResult {
                tool_name: call.name.clone(),
                content: "error: user denied this tool call".to_string(),
                diff: None,
                file_preview: None,
                metadata: ToolResultMetadata {
                    success: false,
                    error_kind: Some(crate::tools::ToolErrorKind::PermissionDenied),
                    retryable: false,
                    ..Default::default()
                },
            })
            .collect::<Vec<_>>();
    }

    // Keep the executor's per-call compiler-check and cache invalidation
    // semantics for internal callers, but never run calls concurrently. The
    // model-round boundary normally supplies one call; this sequential
    // fallback also keeps direct/test callers deterministic.
    if tool_calls.len() > 1 {
        let mut results = Vec::with_capacity(tool_calls.len());
        for call in tool_calls {
            results.extend(
                Box::pin(execute_tool_batch_with_assessments(
                    client,
                    state,
                    cancel_token,
                    std::slice::from_ref(call),
                    approved,
                    edit_root,
                    compile_dirty,
                    compile_cache,
                    user_wait_duration,
                    deferred_notice.clone(),
                    assessment_cache,
                ))
                .await,
            );
        }
        return results;
    }

    dbg_log!("Executing {} tool calls sequentially", tool_calls.len());
    let mut results = Vec::with_capacity(tool_calls.len());
    for call in tool_calls {
        let name = &call.name;
        let args = &call.arguments;
        let replay = {
            let s = state.lock().await;
            replay::successful_side_effect_replay(&s.history, call)
        };
        if let Some(result) = replay {
            results.push(result);
            continue;
        }
        let live_key = {
            let mut s = state.lock().await;
            s.begin_live_tool_call(call.call_id.as_deref(), name, args)
        };
        let client_clone = client.clone();
        let state_clone = Arc::clone(state);
        let cancel_token_clone = cancel_token.clone();
        let name_clone = name.clone();
        let args_clone = args.clone();
        let call_id_owned = call.call_id.clone();
        let plan_mode_denied = {
            let plan_mode = state.lock().await.agent_mode == crate::config::AgentMode::Plan;
            plan_mode && !crate::tools::allowed_in_plan_mode(name)
        };
        let execution_live_key = live_key.clone();
        let (executed_name, execution, diff_opt, replay_artifact, user_wait) = async move {
            let call_for_policy = crate::tools::ToolCall {
                name: name_clone.clone(),
                arguments: args_clone.clone(),
                call_id: call_id_owned.clone(),
            };
            let is_read_only = crate::tools::is_read_only_call(&call_for_policy);
            let mut replay_artifact = None;

            let mut is_repeat = false;
            let mut view_path: Option<String> = None;
            let mut view_mtime: Option<std::time::SystemTime> = None;

            let already_loaded_skill = if name_clone == "use_skill" {
                let requested = args_clone.get("name").and_then(|value| value.as_str());
                let loaded = {
                    let s = state_clone.lock().await;
                    crate::skills::loaded_skills_since_latest_user(&s.history)
                };
                requested.and_then(|requested| {
                    loaded
                        .iter()
                        .find(|loaded| loaded.eq_ignore_ascii_case(requested))
                        .cloned()
                })
            } else {
                None
            };

            if is_read_only && already_loaded_skill.is_none() {
                if name_clone == "view_file" {
                    if let Some(p) = args_clone.get("path").and_then(|p| p.as_str()) {
                        let current = path_mtime(p);
                        let stored = {
                            let s = state_clone.lock().await;
                            s.read_file_mtimes.get(p).copied()
                        };
                        is_repeat = view_file_unchanged_since_last_read(stored, current);
                        view_path = Some(p.to_string());
                        view_mtime = path_mtime(p);
                    }
                } else {
                    let sig = tool_signature(&name_clone, &args_clone);
                    is_repeat = {
                        let s = state_clone.lock().await;
                        s.recent_read_calls.iter().any(|c| c == &sig)
                    };
                }
            }

            // Compact replay is safe only when the bounded body itself is
            // retained. A notice about an earlier failure, truncated read, or
            // over-threshold body is not evidence when history trimming may
            // have removed that body, so execute those reads again.
            let cached_repeat = if already_loaded_skill.is_some() {
                None
            } else if is_repeat {
                let s = state_clone.lock().await;
                let reusable = |previous: &crate::app::CachedReadOutput| {
                    previous.success
                        && !previous.truncated
                        && previous.replayable_content.is_some()
                        && matches!(
                            previous.completeness,
                            rustcode_core::ToolResultCompleteness::Complete
                                | rustcode_core::ToolResultCompleteness::UserLimited
                        )
                };
                let exact = s
                    .recent_read_outputs
                    .get(&tool_signature(&name_clone, &args_clone))
                    .filter(|previous| reusable(previous))
                    .cloned()
                    .map(|previous| (previous, false));
                exact.or_else(|| {
                    if name_clone == "view_file" {
                        s.recent_read_outputs
                            .values()
                            .filter(|previous| {
                                reusable(previous)
                                    && cached_read_covers_request(
                                        previous,
                                        &name_clone,
                                        &args_clone,
                                    )
                            })
                            .min_by_key(|previous| {
                                previous
                                    .inspection
                                    .as_ref()
                                    .and_then(|inspection| inspection.returned_range.as_ref())
                                    .and_then(|range| {
                                        Some(
                                            range
                                                .end
                                                .unwrap_or(u64::MAX)
                                                .saturating_sub(range.start.unwrap_or(1)),
                                        )
                                    })
                                    .unwrap_or(u64::MAX)
                            })
                            .cloned()
                            .map(|previous| (previous, true))
                    } else {
                        None
                    }
                })
            } else {
                None
            };
            is_repeat = cached_repeat.is_some();

            let session_title_unavailable = name_clone == "set_session_title"
                && !state_clone.lock().await.session_title_tool_available;
            let (execution, diff_opt, user_wait) = if let Some(skill_name) = already_loaded_skill {
                (
                    crate::tools::ToolExecutionOutput::success(format!(
                        "Skill `{skill_name}` is already loaded and active above. Proceed with the actual task using its instructions; do not call `use_skill` again for this request."
                    )),
                    None,
                    std::time::Duration::ZERO,
                )
            } else if session_title_unavailable {
                (
                    crate::tools::ToolExecutionOutput::failure_with_kind(
                        "error: set_session_title is only available during the first turn of a new session".to_string(),
                        crate::tools::ToolErrorKind::UnavailableDependency,
                        false,
                    ),
                    None,
                    std::time::Duration::ZERO,
                )
            } else if is_repeat {
                let tuple = match cached_repeat {
                    Some((previous, covered_subrange)) => {
                        let mut content = if covered_subrange {
                            replay_cached_view_file_subrange(
                                &name_clone,
                                &args_clone,
                                previous.replayable_content.as_deref(),
                            )
                            .unwrap_or_else(|| {
                                compact_replayed_read_result(
                                    &name_clone,
                                    &args_clone,
                                    previous.replayable_content.as_deref(),
                                )
                            })
                        } else {
                            compact_replayed_read_result(
                                &name_clone,
                                &args_clone,
                                previous.replayable_content.as_deref(),
                            )
                        };
                        if let Some(path) = previous.full_output_artifact.as_deref() {
                            content.push_str(&format!(
                                " The bounded output remains available at: {path}."
                            ));
                        }
                        replay_artifact = previous.full_output_artifact;
                        (
                            crate::tools::ToolExecutionOutput {
                                content,
                                success: previous.success,
                                pending: false,
                                command: None,
                                exit_code: previous.exit_code,
                                truncated: previous.truncated,
                                completeness: previous.completeness,
                                replayed: true,
                                error_kind: previous.error_kind,
                                retryable: previous.retryable,
                                command_status: None,
                            },
                            None,
                        )
                    }
                    None => unreachable!("repeat requires a replayable cached body"),
                };
                (tuple.0, tuple.1, std::time::Duration::ZERO)
            } else if name_clone == "ask_question" {
                let (output, wait) =
                    ask_user_question(&state_clone, &cancel_token_clone, &args_clone).await;
                (output, None, wait)
            } else if plan_mode_denied {
                (
                    crate::tools::ToolExecutionOutput::failure_with_kind(
                        "error: Plan mode is active; this tool is not permitted.".to_string(),
                        crate::tools::ToolErrorKind::PermissionDenied,
                        false,
                    ),
                    None,
                    std::time::Duration::ZERO,
                )
            } else if crate::tools::is_agent_tool(&name_clone) {
                (
                    handle_agent_tool(
                        &client_clone,
                        &state_clone,
                        &cancel_token_clone,
                        &name_clone,
                        &args_clone,
                    )
                    .await,
                    None,
                    std::time::Duration::ZERO,
                )
            } else {
                let workspace_root = { state_clone.lock().await.workspace_root.clone() };
                confirm_and_execute_for_call_with_assessment(
                    &client_clone,
                    &state_clone,
                    &cancel_token_clone,
                    &name_clone,
                    &args_clone,
                    &name_clone,
                    true, // bypass confirmation
                    workspace_root,
                    Some(&execution_live_key),
                    call_id_owned.as_deref(),
                    crate::tools::shell_assessment_for_call(assessment_cache, call)
                        .cloned(),
                )
                .await
            };

            {
                let mut s = state_clone.lock().await;
                if let Some(p) = view_path
                    && !is_repeat
                {
                    if let Some(mt) = view_mtime {
                        s.read_file_mtimes.insert(p, mt);
                    } else {
                        s.read_file_mtimes.remove(&p);
                    }
                }
                if is_read_only && !is_repeat {
                    let sig = tool_signature(&name_clone, &args_clone);
                    s.recent_read_outputs.insert(
                        sig.clone(),
                        crate::app::CachedReadOutput {
                            replayable_content: (execution.success
                                && !execution.truncated
                                && execution.content.len() <= REPLAYABLE_READ_LIMIT)
                                .then(|| execution.content.clone()),
                            content_offset: args_clone
                                .get("content_offset")
                                .and_then(serde_json::Value::as_u64)
                                .unwrap_or(0),
                            success: execution.success,
                            exit_code: execution.exit_code,
                            truncated: execution.truncated,
                            completeness: execution.completeness,
                            full_output_artifact: None,
                            error_kind: execution.error_kind,
                            retryable: execution.retryable,
                            inspection: None,
                        },
                    );
                    if !s.recent_read_calls.contains(&sig) {
                        s.recent_read_calls.push_back(sig);
                        while s.recent_read_calls.len() > 8 {
                            s.recent_read_calls.pop_front();
                        }
                        while s.recent_read_outputs.len() > 8
                            && let Some(oldest) = s
                                .recent_read_outputs
                                .keys()
                                .find(|key| !s.recent_read_calls.contains(key))
                                .cloned()
                        {
                            s.recent_read_outputs.remove(&oldest);
                        }
                    }
                }
            }

            (name_clone, execution, diff_opt, replay_artifact, user_wait)
        }
        .await;
        {
            let mut s = state.lock().await;
            s.finish_live_tool_call(&live_key);
        }
        *user_wait_duration += user_wait;
        let preview_fallback = if tool_result_precludes_preview_fallback(&execution.content) {
            None
        } else {
            diff_opt
        };
        let final_diff = final_tool_diff(&execution.content, preview_fallback);
        let title_was_set = executed_name == "set_session_title" && execution.success;
        let mut result = tool_result_from_execution(&executed_name, args, execution, final_diff);
        result.metadata.full_output_artifact = replay_artifact;
        results.push(result);
        if title_was_set {
            let mut s = state.lock().await;
            s.session_title_tool_available = false;
            s.invalidate_session_title_cache();
            s.request_redraw();
        }
        if cancel_token.is_cancelled() {
            break;
        }
    }
    let batch_changed_files = results.iter().any(|result| {
        is_mutating_tool(&result.tool_name)
            && mutation_made_progress(result.metadata.success, &result.content)
    });
    if batch_changed_files {
        {
            let mut s = state.lock().await;
            s.recent_read_calls.clear();
            s.recent_read_outputs.clear();
            s.read_file_mtimes.clear();
        }
        // Recursive batches share this cache; each successful edit invalidates it.
        *compile_dirty = true;
        let root = tool_calls
            .iter()
            .zip(&results)
            .find(|(_, result)| {
                is_mutating_tool(&result.tool_name)
                    && mutation_made_progress(result.metadata.success, &result.content)
            })
            .and_then(|(call, _)| get_tool_project_root(&call.name, &call.arguments))
            .or_else(|| edit_root.clone())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        if let Some(compiler_errors) =
            cached_compiler_check(&root, compile_dirty, compile_cache, cancel_token).await
        {
            dbg_log!("Inline compiler check returned diagnostics after edit");
            if let Some(result) = results
                .iter_mut()
                .find(|result| is_mutating_tool(&result.tool_name))
            {
                append_compiler_diagnostics(result, &compiler_errors);
            }
        }
    }
    for (result, call) in results.iter_mut().zip(tool_calls) {
        let notice = (result.tool_name == "use_skill")
            .then_some(deferred_notice.as_deref())
            .flatten();
        let cached_inspection = if result.metadata.replayed {
            let s = state.lock().await;
            s.recent_read_outputs
                .get(&tool_signature(&call.name, &call.arguments))
                .or_else(|| {
                    if call.name == "view_file" {
                        s.recent_read_outputs.values().find(|cached| {
                            cached_read_covers_request(cached, &call.name, &call.arguments)
                        })
                    } else {
                        None
                    }
                })
                .and_then(|cached| {
                    replay_inspection_for_request(cached, &call.name, &call.arguments)
                })
        } else {
            None
        };
        let mut finalized = finalize_tool_result(result.clone(), notice);
        if let Some(inspection) = cached_inspection {
            finalized.metadata.inspection = Some(inspection);
        }
        *result = finalized;
        if crate::tools::is_read_only_call(call) {
            let sig = tool_signature(&call.name, &call.arguments);
            if let Some(cached) = state.lock().await.recent_read_outputs.get_mut(&sig) {
                cached.success = result.metadata.success;
                cached.exit_code = result.metadata.exit_code;
                cached.truncated = result.metadata.truncated;
                cached.completeness = result.metadata.completeness;
                cached.error_kind = result.metadata.error_kind;
                cached.retryable = result.metadata.retryable;
                cached.inspection = result.metadata.inspection.clone();
                if result.metadata.full_output_artifact.is_some() {
                    cached.full_output_artifact = result.metadata.full_output_artifact.clone();
                }
            }
        }
    }
    results
}

fn cancelled_tool_result(call: &crate::tools::ToolCall) -> ToolResult {
    ToolResult {
        tool_name: call.name.clone(),
        content: "error: tool call cancelled before execution".to_string(),
        diff: None,
        file_preview: None,
        metadata: ToolResultMetadata {
            success: false,
            error_kind: Some(crate::tools::ToolErrorKind::Cancelled),
            retryable: false,
            ..Default::default()
        },
    }
}

#[cfg(test)]
mod cancellation_tests {
    use super::execute_tool_batch_with_assessments;
    use crate::app::AppState;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    #[tokio::test]
    async fn cancelled_batch_is_reported_as_cancelled_when_approval_returns_false() {
        let temp = tempfile::tempdir().expect("temporary output directory");
        let call = crate::tools::ToolCall {
            name: "write_to_file".to_owned(),
            arguments: serde_json::json!({
                "path": temp.path().join("should-not-exist.txt"),
                "content": "must not be written",
            }),
            call_id: Some("cancelled-call".to_owned()),
        };
        let state = Arc::new(Mutex::new(AppState::new()));
        let cancellation = tokio_util::sync::CancellationToken::new();
        cancellation.cancel();
        let mut compile_dirty = false;
        let mut compile_cache = None;
        let mut user_wait_duration = std::time::Duration::ZERO;

        let results = execute_tool_batch_with_assessments(
            &reqwest::Client::new(),
            &state,
            &cancellation,
            &[call],
            false,
            &None,
            &mut compile_dirty,
            &mut compile_cache,
            &mut user_wait_duration,
            None,
            &Default::default(),
        )
        .await;

        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].metadata.error_kind,
            Some(crate::tools::ToolErrorKind::Cancelled)
        );
        assert!(temp.path().read_dir().unwrap().next().is_none());
    }
}

#[cfg(test)]
mod question_tests {
    use super::{map_question_channel_result, parse_question_chain};

    #[test]
    fn chained_shape_parses_headers_labels_descriptions_and_multi() {
        let args = serde_json::json!({
            "questions": [
                {
                    "header": "Source",
                    "question": "Where from?",
                    "options": [
                        {"label": "CHANGELOG", "description": "curated"},
                        "Releases API"
                    ],
                    "multiple": true
                },
                {"question": "How many?", "options": ["3", "5"]}
            ]
        });
        let chain = parse_question_chain(&args);
        assert_eq!(chain.len(), 2);
        assert_eq!(chain[0].0, "Source");
        assert_eq!(chain[0].1, "Where from?");
        assert_eq!(
            chain[0].2,
            vec![
                ("CHANGELOG".to_owned(), "curated".to_owned()),
                ("Releases API".to_owned(), String::new()),
            ]
        );
        assert!(chain[0].3);
        assert_eq!(chain[1].0, "");
        assert!(!chain[1].3);
    }

    #[test]
    fn legacy_flat_shape_parses_as_single_question() {
        let args = serde_json::json!({
            "question": "Proceed?",
            "options": ["Yes", "No"],
            "is_multi_select": true
        });
        let chain = parse_question_chain(&args);
        assert_eq!(chain.len(), 1);
        assert_eq!(chain[0].1, "Proceed?");
        assert_eq!(chain[0].2.len(), 2);
        assert!(chain[0].3);
    }

    #[test]
    fn empty_args_yield_one_default_slot() {
        let chain = parse_question_chain(&serde_json::json!({}));
        assert_eq!(chain.len(), 1);
    }

    #[test]
    fn channel_results_map_answers_and_legacy_cancel_text() {
        let ok = map_question_channel_result(Some("User selected: X".to_owned()));
        assert!(ok.success);
        assert_eq!(ok.content, "User selected: X");

        let chained = map_question_channel_result(Some("User answers:\n[H] Q? → A".to_owned()));
        assert!(chained.success);
        assert!(chained.content.contains("User answers:"));

        for cancelled in [
            Some("User cancelled prompt.".to_owned()),
            Some(String::new()),
            None,
        ] {
            let out = map_question_channel_result(cancelled);
            assert!(!out.success, "cancel must fail: {}", out.content);
            assert!(out.content.contains("cancelled"));
        }
    }
}
