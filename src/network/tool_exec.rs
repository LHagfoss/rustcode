use crate::app::{AppState, AppStatus, ChatMessage, StreamTracker, ToolConfirmation};
use std::sync::Arc;
use tokio::sync::Mutex;

use super::compiler::{append_compiler_diagnostics, cached_compiler_check, run_compiler_check};
use super::events::{ToolResult, ToolResultMetadata};
use super::output::truncate_tool_output_for_message;
use super::subagents::handle_agent_tool;
use super::text::cap_diff_lines;
use super::{
    REPLAYABLE_READ_LIMIT, is_mutating_tool, is_read_only_tool, path_mtime, tool_signature,
    view_file_unchanged_since_last_read,
};

pub(crate) fn get_diff_preview(name: &str, args: &serde_json::Value) -> Option<String> {
    if name == "replace_file_content" {
        let (target, replacement) = crate::tools::edit_target_and_replacement(args);
        let search_block = target.as_deref().unwrap_or("");
        let replace_block = replacement.as_deref().unwrap_or("");

        let diff = similar::TextDiff::from_lines(search_block, replace_block);
        let old_slices: Vec<&str> = diff.iter_old_slices().collect();
        let new_slices: Vec<&str> = diff.iter_new_slices().collect();

        let mut prev = String::new();
        for op in diff.ops() {
            let old_slice = &old_slices[op.old_range()];
            let new_slice = &new_slices[op.new_range()];
            match op.tag() {
                similar::DiffTag::Equal => {
                    for (o, n) in old_slice.iter().zip(new_slice.iter()) {
                        prev.push_str(&format!(
                            " {}\x00 {}\n",
                            o.trim_end_matches('\n').trim_end_matches('\r'),
                            n.trim_end_matches('\n').trim_end_matches('\r')
                        ));
                    }
                }
                similar::DiffTag::Delete => {
                    for o in old_slice {
                        prev.push_str(&format!(
                            "-{}\x00~\n",
                            o.trim_end_matches('\n').trim_end_matches('\r')
                        ));
                    }
                }
                similar::DiffTag::Insert => {
                    for n in new_slice {
                        prev.push_str(&format!(
                            "~\x00+{}\n",
                            n.trim_end_matches('\n').trim_end_matches('\r')
                        ));
                    }
                }
                similar::DiffTag::Replace => {
                    let max_len = old_slice.len().max(new_slice.len());
                    for i in 0..max_len {
                        let o_val = old_slice.get(i);
                        let n_val = new_slice.get(i);
                        match (o_val, n_val) {
                            (Some(o), Some(n)) => {
                                prev.push_str(&format!(
                                    "-{}\x00+{}\n",
                                    o.trim_end_matches('\n').trim_end_matches('\r'),
                                    n.trim_end_matches('\n').trim_end_matches('\r')
                                ));
                            }
                            (Some(o), None) => {
                                prev.push_str(&format!(
                                    "-{}\x00~\n",
                                    o.trim_end_matches('\n').trim_end_matches('\r')
                                ));
                            }
                            (None, Some(n)) => {
                                prev.push_str(&format!(
                                    "~\x00+{}\n",
                                    n.trim_end_matches('\n').trim_end_matches('\r')
                                ));
                            }
                            (None, None) => {}
                        }
                    }
                }
            }
        }
        Some(cap_diff_lines(prev))
    } else if name == "write_to_file" && args.get("__rustcode_legacy_write_diff").is_some() {
        let path = args.get("path").and_then(|p| p.as_str()).unwrap_or("");
        let old_content = std::fs::read_to_string(path).unwrap_or_default();
        let new_content = args.get("content").and_then(|c| c.as_str()).unwrap_or("");

        let diff = similar::TextDiff::from_lines(&old_content, new_content);
        let old_slices: Vec<&str> = diff.iter_old_slices().collect();
        let new_slices: Vec<&str> = diff.iter_new_slices().collect();

        let mut prev = String::new();
        for group in diff.grouped_ops(3) {
            for op in group {
                let old_slice = &old_slices[op.old_range()];
                let new_slice = &new_slices[op.new_range()];
                match op.tag() {
                    similar::DiffTag::Equal => {
                        for (o, n) in old_slice.iter().zip(new_slice.iter()) {
                            prev.push_str(&format!(
                                " {}\x00 {}\n",
                                o.trim_end_matches('\n').trim_end_matches('\r'),
                                n.trim_end_matches('\n').trim_end_matches('\r')
                            ));
                        }
                    }
                    similar::DiffTag::Delete => {
                        for o in old_slice {
                            prev.push_str(&format!(
                                "-{}\x00~\n",
                                o.trim_end_matches('\n').trim_end_matches('\r')
                            ));
                        }
                    }
                    similar::DiffTag::Insert => {
                        for n in new_slice {
                            prev.push_str(&format!(
                                "~\x00+{}\n",
                                n.trim_end_matches('\n').trim_end_matches('\r')
                            ));
                        }
                    }
                    similar::DiffTag::Replace => {
                        let max_len = old_slice.len().max(new_slice.len());
                        for i in 0..max_len {
                            let o_val = old_slice.get(i);
                            let n_val = new_slice.get(i);
                            match (o_val, n_val) {
                                (Some(o), Some(n)) => {
                                    prev.push_str(&format!(
                                        "-{}\x00+{}\n",
                                        o.trim_end_matches('\n').trim_end_matches('\r'),
                                        n.trim_end_matches('\n').trim_end_matches('\r')
                                    ));
                                }
                                (Some(o), None) => {
                                    prev.push_str(&format!(
                                        "-{}\x00~\n",
                                        o.trim_end_matches('\n').trim_end_matches('\r')
                                    ));
                                }
                                (None, Some(n)) => {
                                    prev.push_str(&format!(
                                        "~\x00+{}\n",
                                        n.trim_end_matches('\n').trim_end_matches('\r')
                                    ));
                                }
                                (None, None) => {}
                            }
                        }
                    }
                }
            }
        }
        Some(cap_diff_lines(prev))
    } else {
        None
    }
}

pub(crate) fn extract_diff_block(content: &str) -> Option<String> {
    let after_fence = content.split_once("```diff\n")?.1;
    let (body, _) = after_fence.split_once("\n```")?;
    if body.trim().is_empty() {
        None
    } else {
        Some(body.to_string())
    }
}

pub(crate) fn final_tool_diff(result: &str, preview_fallback: Option<String>) -> Option<String> {
    extract_diff_block(result).or_else(|| preview_fallback.filter(|d| !d.trim().is_empty()))
}

pub(crate) fn tool_result_precludes_preview_fallback(content: &str) -> bool {
    let lower = content.trim_start().to_ascii_lowercase();
    lower.starts_with("error") || lower.contains("already applied")
}

pub(crate) fn get_file_preview(name: &str, args: &serde_json::Value) -> Option<(String, String)> {
    if name != "write_to_file" {
        return None;
    }
    Some((
        args.get("path")?.as_str()?.to_string(),
        args.get("content")?.as_str()?.to_string(),
    ))
}

pub(crate) fn get_tool_project_root(_name: &str, args: &serde_json::Value) -> Option<std::path::PathBuf> {
    let raw_path = if let Some(p) = args.get("path").and_then(|p| p.as_str()) {
        Some(p)
    } else if let Some(s) = args.get("src").and_then(|s| s.as_str()) {
        Some(s)
    } else {
        args.get("dest").and_then(|d| d.as_str())
    };

    let resolved = if let Some(rp) = raw_path {
        let p = crate::tools::resolve_tool_path(rp);
        if p.is_relative() {
            std::env::current_dir().unwrap_or_default().join(p)
        } else {
            p
        }
    } else {
        return None;
    };

    let mut current = if resolved.is_dir() {
        resolved.clone()
    } else {
        resolved
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or(resolved)
    };

    loop {
        if current.join("Cargo.toml").exists() || current.join("tsconfig.json").exists() {
            return Some(
                current
                    .canonicalize()
                    .unwrap_or(current),
            );
        }
        if let Some(parent) = current.parent() {
            current = parent.to_path_buf();
        } else {
            break;
        }
    }

    None
}

pub(crate) fn resolve_bin(name: &str) -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    let candidates = [
        format!("{home}/.cargo/bin/{name}"),
        format!("/opt/homebrew/bin/{name}"),
        format!("/usr/local/bin/{name}"),
        format!("/usr/bin/{name}"),
    ];
    for c in candidates {
        if std::path::Path::new(&c).exists() {
            return std::path::PathBuf::from(c);
        }
    }
    std::path::PathBuf::from(name)
}

pub(crate) fn augmented_path() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut dirs = vec![
        format!("{home}/.cargo/bin"),
        "/opt/homebrew/bin".to_string(),
        "/usr/local/bin".to_string(),
        "/usr/bin".to_string(),
        "/bin".to_string(),
    ];
    if let Ok(existing) = std::env::var("PATH") {
        dirs.extend(existing.split(':').map(|s| s.to_string()));
    }
    dirs.join(":")
}

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
        s.pending_question = None;
        s.question_response = None;
        if s.status == AppStatus::AwaitingQuestion {
            s.status = AppStatus::Streaming;
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
        let session_id = { state.lock().await.active_session_id.clone() };
        let workspace_root_for_task = workspace_root.clone();
        let live_key_owned = live_key.map(str::to_owned);
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel();
        let run_fut = tokio::task::spawn_blocking(move || {
            crate::tools::set_active_session_id(Some(session_id));
            crate::tools::set_active_workspace_root(workspace_root_for_task);
            let result = if name_owned == "run_command" && live_key_owned.is_some() {
                let callback: crate::tools::CommandProgressCallback = Arc::new(move |bytes, stderr| {
                    let _ = progress_tx.send((bytes.to_vec(), stderr));
                });
                crate::tools::run_command_output_with_progress(&args_owned, callback)
                    .unwrap_or_else(|error| {
                        crate::tools::ToolExecutionOutput::failure_with_kind(
                            format!("error: {error}"),
                            crate::tools::ToolErrorKind::CommandFailed,
                            true,
                        )
                    })
            } else {
                crate::tools::execute_with_metadata(&name_owned, &args_owned)
            };
            crate::tools::set_active_workspace_root(None);
            crate::tools::set_active_session_id(None);
            result
        });
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
                _ = cancel_token.cancelled() => {
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
        let path = if let Some(p) = args.get("path").and_then(|p| p.as_str()) {
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
        let (preview, content_bytes) = if let Some(ref d) = diff_opt {
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
                }
                let _cleanup = ToolCleanup {
                    state: Arc::clone(state),
                    tool_name,
                };

                let name_owned = name.to_string();
                let args_owned = args.clone();
                let session_id = { state.lock().await.active_session_id.clone() };
                let workspace_root_for_task = workspace_root.clone();
                let run_fut = tokio::task::spawn_blocking(move || {
                    crate::tools::set_active_session_id(Some(session_id));
                    crate::tools::set_active_workspace_root(workspace_root_for_task);
                    let result = crate::tools::execute_with_metadata(&name_owned, &args_owned);
                    crate::tools::set_active_workspace_root(None);
                    crate::tools::set_active_session_id(None);
                    result
                });

                tokio::select! {
                    res = run_fut => {
                        res.unwrap_or_else(|e| {
                            crate::tools::ToolExecutionOutput::failure(format!("tool panicked: {e}"))
                        })
                    }
                    _ = cancel_token.cancelled() => {
                        dbg_log!("Tool execution cancelled during spawn_blocking await");
                        crate::tools::ToolExecutionOutput::failure_with_kind(
                            "error: tool execution cancelled by user".to_string(),
                            crate::tools::ToolErrorKind::Cancelled,
                            true,
                        )
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
            s.pending_tool_confirmation = None;
            s.status = AppStatus::Streaming;
            s.stream_tracker = Some(StreamTracker::new());
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
    let changed_paths = if is_mutating_tool(tool_name) {
        args.get("path")
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
            exit_code: execution.exit_code,
            changed_paths,
            truncated: execution.truncated,
            full_output_artifact: None,
            replayed: execution.replayed,
            error_kind: if execution.success {
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
            exit_code: envelope.exit_code,
            changed_paths: envelope.changed_paths,
            truncated: envelope.truncated,
            full_output_artifact: envelope.full_output_artifact,
            error_kind: envelope
                .error_kind
                .map(|kind| kind.as_str().to_string()),
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
                confirm_and_execute(
                    &state_clone,
                    &cancel_token_clone,
                    &name_clone,
                    &args_clone,
                    &name_clone,
                    true, // bypass confirmation
                    workspace_root,
                    Some(&execution_live_key),
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
