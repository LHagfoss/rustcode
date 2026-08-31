use crate::app::{AppState, AppStatus, StreamTracker, ToolConfirmation};
use std::sync::Arc;
use tokio::sync::Mutex;

use super::compiler::{append_compiler_diagnostics, cached_compiler_check, run_compiler_check};
use super::events::{ToolResult, ToolResultMetadata};
use super::subagents::handle_agent_tool;
use super::{
    REPLAYABLE_READ_LIMIT, is_mutating_tool, is_read_only_tool, path_mtime, tool_signature,
    view_file_unchanged_since_last_read,
};

#[path = "tool_exec/preview.rs"]
mod preview;
#[path = "tool_exec/result.rs"]
mod result;

pub(crate) use preview::{
    extract_diff_block, final_tool_diff, get_diff_preview, get_tool_project_root,
    tool_result_precludes_preview_fallback,
};
pub(crate) use result::{
    bounded_tool_result_history_message, finalize_tool_result, subagent_tool_history_message,
    tool_result_from_execution, tool_result_history_message,
};
pub(crate) async fn ask_user_question(
    state: &Arc<Mutex<AppState>>,
    cancel_token: &tokio_util::sync::CancellationToken,
    args: &serde_json::Value,
) -> (crate::tools::ToolExecutionOutput, std::time::Duration) {
    let (mut question, mut options, is_multi_select) =
        if let Some(q_arr) = args.get("questions").and_then(|v| v.as_array()) {
            if let Some(first_q) = q_arr.first().and_then(|v| v.as_object()) {
                let q_str = first_q
                    .get("question")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let opts: Vec<String> = first_q
                    .get("options")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|o| o.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default();
                let multi = first_q
                    .get("is_multi_select")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                (q_str, opts, multi)
            } else {
                (String::new(), Vec::new(), false)
            }
        } else {
            let q_str = args
                .get("question")
                .or_else(|| args.get("prompt"))
                .or_else(|| args.get("message"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let opts: Vec<String> = args
                .get("options")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|o| o.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            let multi = args
                .get("is_multi_select")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            (q_str, opts, multi)
        };

    if question.trim().is_empty() {
        question = "Please confirm how to proceed:".to_string();
    }
    if options.is_empty() {
        options = vec!["Proceed".to_string(), "Cancel".to_string()];
    }

    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    {
        let mut s = state.lock().await;
        s.pending_question = Some(crate::app::PendingQuestion::new(
            question,
            options,
            is_multi_select,
        ));
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

    let out = match answer {
        Some(a) if !a.is_empty() => {
            crate::tools::ToolExecutionOutput::success(format!("User selected: {a}"))
        }
        _ => crate::tools::ToolExecutionOutput::failure_with_kind(
            "User cancelled or provided no selection.".to_string(),
            crate::tools::ToolErrorKind::Cancelled,
            true,
        ),
    };
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
    confirm_and_execute_for_call(
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
    .await
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
    let (agent_mode, auto_confirm) = {
        let s = state.lock().await;
        (s.agent_mode, s.auto_confirm)
    };
    if let crate::tools::AuthorizationDecision::Deny(reason) =
        crate::tools::authorize_tool_with_args(name, args, agent_mode, auto_confirm, bypass_confirm)
    {
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
        crate::tools::authorize_tool_with_args(
            name,
            args,
            agent_mode,
            auto_confirm,
            bypass_confirm,
        ),
        crate::tools::AuthorizationDecision::RequireConfirmation
    );
    let mut user_wait_dur = std::time::Duration::ZERO;
    let mut confirmation_transition_redrawn = false;
    let mut result = if !needs_confirm {
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
        let workspace_root_for_task = workspace_root.clone();
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

            tokio::task::spawn_blocking(move || {
                crate::tools::set_active_session_id(Some(session_id));
                crate::tools::set_active_workspace_root(workspace_root_for_task);
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
                crate::tools::set_active_workspace_root(None);
                crate::tools::set_active_session_id(None);
                result
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
                (
                    crate::tools::command_confirmation_preview(command),
                    command.len(),
                )
            } else {
                let content = args.get("content").and_then(|c| c.as_str()).unwrap_or("");
                let preview = content.lines().take(6).collect::<Vec<_>>().join("\n");
                (preview, content.len())
            }
        };
        let (tx, rx) = tokio::sync::oneshot::channel::<bool>();
        {
            let mut s = state.lock().await;
            s.modal_scroll_row = 0;
            s.tool_confirmation_selected = 0;
            s.pending_tool_confirmation = Some(vec![ToolConfirmation {
                tool_name: display_name.to_string(),
                path,
                content_preview: preview,
                content_bytes,
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

        let res = match rx_res {
            Ok(true) => {
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
                let workspace_root_for_task = workspace_root.clone();
                let cancel_token_for_task = cancel_token.clone();
                let live_key_for_task = live_key.map(str::to_owned);
                let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel();
                let run_fut = tokio::task::spawn_blocking(move || {
                    crate::tools::set_active_session_id(Some(session_id));
                    crate::tools::set_active_workspace_root(workspace_root_for_task);
                    let result = if name_owned == "render_video" && live_key_for_task.is_some() {
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
                    crate::tools::set_active_workspace_root(None);
                    crate::tools::set_active_session_id(None);
                    result
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
            Ok(false) => {
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

    if matches!(
        name,
        "replace_file_content"
            | "multi_replace_file_content"
            | "write_to_file"
            | "delete_file"
            | "move_file"
            | "copy_file"
    ) && result.success
    {
        if let Some(cwd) = get_tool_project_root(name, args) {
            if let Some(errors) = run_compiler_check(&cwd).await {
                result.content.push_str("\n\nCompiler errors/warnings:\n");
                result.content.push_str(&errors);
                result.error_kind = Some(crate::tools::ToolErrorKind::CompilerFailed);
                result.retryable = true;
            }
        }
    }

    (result, diff_opt, user_wait_dur)
}

#[allow(clippy::too_many_arguments)]
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

    if tool_calls.len() > 1 {
        let mut results = Vec::with_capacity(tool_calls.len());
        let mut index = 0;
        while index < tool_calls.len() {
            let parallel_run_end = tool_calls[index..]
                .iter()
                .position(|call| !crate::tools::supports_parallel_execution(&call.name))
                .map(|offset| index + offset)
                .unwrap_or(tool_calls.len());

            if parallel_run_end > index + 1 {
                let futures = tool_calls[index..parallel_run_end]
                    .iter()
                    .map(|call| async {
                        let mut read_dirty = false;
                        let mut read_cache = None;
                        let mut user_wait = std::time::Duration::ZERO;
                        execute_tool_batch(
                            client,
                            state,
                            cancel_token,
                            std::slice::from_ref(call),
                            approved,
                            &None,
                            &mut read_dirty,
                            &mut read_cache,
                            &mut user_wait,
                            deferred_notice.clone(),
                        )
                        .await
                    });
                results.extend(
                    futures_util::future::join_all(futures)
                        .await
                        .into_iter()
                        .flatten(),
                );
                index = parallel_run_end;
                continue;
            }

            results.extend(
                Box::pin(execute_tool_batch(
                    client,
                    state,
                    cancel_token,
                    std::slice::from_ref(&tool_calls[index]),
                    approved,
                    edit_root,
                    compile_dirty,
                    compile_cache,
                    user_wait_duration,
                    deferred_notice.clone(),
                ))
                .await,
            );
            index += 1;
        }
        return results;
    }

    dbg_log!("Executing {} tool calls sequentially", tool_calls.len());
    let mut results = Vec::with_capacity(tool_calls.len());
    for call in tool_calls {
        let name = &call.name;
        let args = &call.arguments;
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
            let is_read_only = is_read_only_tool(&name_clone);
            let mut replay_artifact = None;

            let mut is_repeat = false;
            let mut view_path: Option<String> = None;
            let mut view_mtime: Option<std::time::SystemTime> = None;

            if is_read_only {
                if name_clone == "view_file" {
                    if let Some(p) = args_clone.get("path").and_then(|p| p.as_str()) {
                        let sig = tool_signature(&name_clone, &args_clone);
                        let already_seen = {
                            let s = state_clone.lock().await;
                            s.recent_read_calls.iter().any(|c| c == &sig)
                        };
                        if already_seen {
                            let current = path_mtime(p);
                            let stored = {
                                let s = state_clone.lock().await;
                                s.read_file_mtimes.get(p).copied()
                            };
                            is_repeat = view_file_unchanged_since_last_read(stored, current);
                        }
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

            let (execution, diff_opt, user_wait) = if is_repeat {
                let cached = {
                    let s = state_clone.lock().await;
                    s.recent_read_outputs
                        .get(&tool_signature(&name_clone, &args_clone))
                        .cloned()
                };
                let tuple = match cached {
                    Some(previous) => {
                        let content = if let Some(mut content) = previous.replayable_content {
                            content.insert_str(
                                0,
                                "[Unchanged since the last read of this exact range — repeating that output. \
Re-reading will not produce anything new; if an edit failed to match, expand start_line/end_line range or use grep to verify exact target content.]\n",
                            );
                            content
                        } else {
                            let mut notice = "[Notice: This exact read was already executed, but its output exceeded the repeat cache limit and is not repeated. Use the original result or request a narrower range.".to_string();
                            if let Some(path) = previous.full_output_artifact.as_deref() {
                                notice.push_str(&format!(
                                    " The bounded output remains available at: {path}."
                                ));
                            }
                            notice.push(']');
                            notice
                        };
                        replay_artifact = previous.full_output_artifact;
                        (
                            crate::tools::ToolExecutionOutput {
                                content,
                                success: previous.success,
                                pending: false,
                                command: None,
                                exit_code: previous.exit_code,
                                truncated: previous.truncated,
                                replayed: true,
                                error_kind: previous.error_kind,
                                retryable: previous.retryable,
                            },
                            None,
                        )
                    }
                    None => (
                        crate::tools::ToolExecutionOutput::success("[Notice: This exact read tool call was previously executed with identical arguments, \
and the file has not changed since. Its output is above in the context — use it. To see something \
different, read another range or make an edit first; repeating this call returns this same notice.]"
                            .to_string()),
                        None,
                    ),
                };
                (tuple.0, tuple.1, std::time::Duration::ZERO)
            } else if name_clone == "ask_question" {
                let (output, wait) = ask_user_question(&state_clone, &cancel_token_clone, &args_clone).await;
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
                confirm_and_execute_for_call(
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
                            replayable_content: (execution.content.len()
                                <= REPLAYABLE_READ_LIMIT)
                                .then(|| execution.content.clone()),
                            success: execution.success,
                            exit_code: execution.exit_code,
                            truncated: execution.truncated,
                            full_output_artifact: None,
                            error_kind: execution.error_kind,
                            retryable: execution.retryable,
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
        let mut result = tool_result_from_execution(&executed_name, args, execution, final_diff);
        result.metadata.full_output_artifact = replay_artifact;
        results.push(result);
        if cancel_token.is_cancelled() {
            break;
        }
    }
    let batch_changed_files = results.iter().any(|result| {
        is_mutating_tool(&result.tool_name)
            && result.metadata.success
            && !result
                .content
                .trim_start()
                .to_ascii_lowercase()
                .starts_with("error")
    });
    if batch_changed_files {
        {
            let mut s = state.lock().await;
            s.recent_read_calls.clear();
            s.recent_read_outputs.clear();
            s.read_file_mtimes.clear();
        }
        let root = edit_root
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        if let Some(compiler_errors) = cached_compiler_check(&root, compile_dirty, compile_cache)
            .await
            .filter(|e| !e.starts_with("__BUILD_UNVERIFIED__"))
        {
            dbg_log!("Inline compiler check detected errors after edit");
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
        let finalized = finalize_tool_result(result.clone(), notice);
        *result = finalized;
        if is_read_only_tool(&call.name) {
            let sig = tool_signature(&call.name, &call.arguments);
            if let Some(cached) = state.lock().await.recent_read_outputs.get_mut(&sig) {
                cached.success = result.metadata.success;
                cached.exit_code = result.metadata.exit_code;
                cached.truncated = result.metadata.truncated;
                cached.error_kind = result.metadata.error_kind;
                cached.retryable = result.metadata.retryable;
                if result.metadata.full_output_artifact.is_some() {
                    cached.full_output_artifact = result.metadata.full_output_artifact.clone();
                }
            }
        }
    }
    results
}
