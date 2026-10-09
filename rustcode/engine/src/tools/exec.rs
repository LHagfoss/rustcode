use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

// Re-exports needed by exec tools
pub(crate) use super::get_active_session_id;
pub(crate) use super::parse_json_bool;
pub(crate) use super::parse_json_number;

use rustcode_tasks::{
    CancelResult, ProcessTerminator, SessionId, TaskEvent, TaskManager, TaskSpec, TaskStartBarrier,
    TaskState,
};

use super::{Tool, ToolCapability, ToolSafety};

mod policy;
pub(crate) mod sandbox;

#[cfg(test)]
pub(crate) use policy::command_confirmation_scope;
pub(crate) use policy::{
    approved_command_prefix_covers_call, command_confirmation_preview,
    command_requires_confirmation, denied_command_prefix_covers_call,
    persisted_approved_command_prefix, pull_request_base, reject_broad_git_stage,
    rememberable_command_forbid_prefix_for_call, rememberable_command_prefix_for_call,
};
use policy::{has_interactive_sudo, is_short_discovery_command};

static BACKGROUND_TASK_SEQUENCE: AtomicU64 = AtomicU64::new(1);
static BACKGROUND_TASK_MANAGER: OnceLock<TaskManager> = OnceLock::new();
static BACKGROUND_START_BARRIERS: OnceLock<Mutex<HashMap<String, (String, TaskStartBarrier)>>> =
    OnceLock::new();
const BACKGROUND_START_BARRIER_CAPACITY: usize = 1024;

fn background_start_barriers() -> &'static Mutex<HashMap<String, (String, TaskStartBarrier)>> {
    BACKGROUND_START_BARRIERS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn register_background_start(session_id: &str, call_id: &str) -> TaskStartBarrier {
    let barrier = TaskStartBarrier::new();
    let mut barriers = background_start_barriers()
        .lock()
        .expect("background start barrier mutex poisoned");
    if barriers.len() >= BACKGROUND_START_BARRIER_CAPACITY {
        if let Some(key) = barriers.keys().next().cloned() {
            if let Some((_, expired)) = barriers.remove(&key) {
                expired.release();
            }
        }
    }
    if let Some((_, previous)) =
        barriers.insert(call_id.to_owned(), (session_id.to_owned(), barrier.clone()))
    {
        previous.release();
    }
    barrier
}

pub(crate) fn release_background_start(call_id: &str) -> bool {
    let barrier = background_start_barriers()
        .lock()
        .expect("background start barrier mutex poisoned")
        .remove(call_id)
        .map(|(_, barrier)| barrier);
    if let Some(barrier) = barrier {
        barrier.release();
        true
    } else {
        false
    }
}

pub(crate) fn abort_background_starts(session_id: &str) {
    let barriers = {
        let mut all = background_start_barriers()
            .lock()
            .expect("background start barrier mutex poisoned");
        let keys = all
            .iter()
            .filter(|(_, (owner, _))| owner == session_id)
            .map(|(call_id, _)| call_id.clone())
            .collect::<Vec<_>>();
        keys.into_iter()
            .filter_map(|call_id| all.remove(&call_id).map(|(_, barrier)| barrier))
            .collect::<Vec<_>>()
    };
    for barrier in barriers {
        barrier.release();
    }
}

pub fn background_task_manager() -> &'static TaskManager {
    BACKGROUND_TASK_MANAGER.get_or_init(|| TaskManager::new(Arc::new(RootProcessTerminator)))
}

struct RootProcessTerminator;

impl ProcessTerminator for RootProcessTerminator {
    fn terminate(&self, pid: u32) -> bool {
        terminate_background_pid(pid)
    }
}

pub(crate) fn task_event_to_tool_output(
    event: TaskEvent,
) -> Option<(String, String, super::ToolExecutionOutput)> {
    let (id, session_id, output) = match event {
        TaskEvent::Finished {
            id,
            session_id,
            command,
            output,
            ..
        } => {
            let mut output = match output {
                Ok(output) => command_output_to_tool_output(&command, output),
                Err(error) => {
                    let error = error.strip_prefix("failed to spawn process:").map_or_else(
                        || format!("failed to wait: {error}"),
                        |cause| format!("failed to spawn:{cause}"),
                    );
                    super::ToolExecutionOutput::failure(error)
                }
            };
            output.command = Some(command);
            (id, session_id, output)
        }
        TaskEvent::Cancelled {
            id,
            session_id,
            command,
            ..
        } => {
            let mut output = super::ToolExecutionOutput::failure_with_kind(
                "Background task cancelled (termination succeeded; there is no process exit code). Use manage_task action 'logs' to inspect captured output.".to_string(),
                super::ToolErrorKind::Cancelled,
                false,
            );
            output.command = Some(command);
            (id, session_id, output)
        }
        TaskEvent::Started { .. } => return None,
    };
    Some((id.to_string(), session_id.to_string(), output))
}

fn command_output_to_tool_output(
    command: &str,
    output: rustcode_command::CommandOutput,
) -> super::ToolExecutionOutput {
    let command_status = command_result_metadata(&output);
    let out_str = rustcode_command::format_bounded_output(&output.stdout);
    let err_str = rustcode_command::format_bounded_output(&output.stderr);
    let mut full = format!(
        "{}\n{}",
        format_command_status(output.success, &command_status),
        out_str
    );
    if !err_str.is_empty() {
        full.push_str("\nstderr:\n");
        full.push_str(&err_str);
    }
    if !output.success {
        full = format!("exit code {:?}\n{full}", output.exit_code);
    }
    if out_str.is_empty() && err_str.is_empty() {
        full.push_str("\n[no stdout or stderr was produced]");
    }
    super::ToolExecutionOutput {
        content: full,
        success: output.success,
        pending: false,
        command: Some(command.to_owned()),
        exit_code: output.exit_code,
        truncated: output.stdout.is_truncated() || output.stderr.is_truncated(),
        completeness: if output.stdout.is_truncated() || output.stderr.is_truncated() {
            rustcode_core::ToolResultCompleteness::ByteTruncated
        } else {
            rustcode_core::ToolResultCompleteness::Complete
        },
        replayed: false,
        error_kind: (!output.success).then_some(super::ToolErrorKind::CommandFailed),
        retryable: false,
        command_status: Some(command_status),
    }
}

fn command_result_metadata(
    output: &rustcode_command::CommandOutput,
) -> rustcode_core::CommandResultMetadata {
    let output_truncated = output.stdout.is_truncated() || output.stderr.is_truncated();
    rustcode_core::CommandResultMetadata {
        completed: true,
        exit_code: output.exit_code,
        signal: output.signal,
        downstream_consumer_terminated: output.downstream_consumer_terminated,
        bytes_returned: output
            .stdout
            .captured_len()
            .saturating_add(output.stderr.captured_len()) as u64,
        total_output_bytes: Some(
            output
                .stdout
                .total_bytes()
                .saturating_add(output.stderr.total_bytes()) as u64,
        ),
        output_truncated,
    }
}

fn format_command_status(success: bool, status: &rustcode_core::CommandResultMetadata) -> String {
    let signal = status.signal.map_or_else(
        || "none".to_string(),
        |signal| match signal {
            13 => "13 (SIGPIPE)".to_string(),
            signal => signal.to_string(),
        },
    );
    format!(
        "[command status: completed={}; success={success}; exit_code={:?}; signal={signal}; downstream_consumer_terminated={}; bytes_returned={}; total_output_bytes={:?}; output_truncated_by_rustcode={}]",
        status.completed,
        status.exit_code,
        status.downstream_consumer_terminated,
        status.bytes_returned,
        status.total_output_bytes,
        status.output_truncated,
    )
}

fn run_command_schema() -> Value {
    serde_json::json!({
        "type": "object", "properties": {
            "command": { "type": "string" }, "cwd": { "type": "string" },
            "timeout_ms": { "type": "integer", "minimum": 1 },
            "background": { "type": "boolean", "default": false },
            "detached": { "type": "boolean", "default": false },
            "notify_on_complete": { "type": "boolean" },
            "network_access": { "type": "boolean", "default": false },
            "filesystem_write_path": { "type": "string" },
            "env": { "type": "object", "additionalProperties": { "type": "string" } }
        }, "required": ["command"]
    })
}

/// Sent on every request and part of the cached prompt prefix: the POSIX text
/// must stay byte-for-byte stable.
const POSIX_RUN_COMMAND_DESCRIPTION: &str = "Run one shell command; returns stdout, stderr and exit code. Pipelines fail if any stage fails; timeout defaults to 120s; interactive sudo is disabled. Prefer view_file over cat/sed/head for file reads. The OS sandbox (none on Windows) fails closed and a denied command reports which permission to request: network_access=true or filesystem_write_path (an existing absolute directory, outside the workspace in restricted modes) grants it for this command only, with confirmation unless YOLO is on. background=true runs a long job (a build, a test suite) and resumes you on completion (notify_on_complete=false: no turn); a quick command needs no background. detached=true starts a long-lived server or watcher, returns a task ID at once and stays silent unless notify_on_complete=true. Use manage_task for logs, wait and kill; never poll. For external jobs run the provider's blocking watch command once with background=true. A command containing '&' is treated as detached unless the script waits on or kills its own jobs; never add '&' with detached=true. Git: branch and worktree handling follows the repository `AGENTS.md`, which outranks generic workflow skills. Never `git rebase`, `git reset --hard` or force-push in the active checkout, or discard uncommitted work. Branch there with `git switch -c`; `git worktree add` under /tmp only when it is genuinely required (concurrent or unrelated dirty work), then `git worktree remove`, `git worktree prune`, `git branch -d`. End on the original branch after `git pull --ff-only`.";

/// Windows runs commands through PowerShell (see `rustcode_command::ShellKind`),
/// so the model is told to write PowerShell and where it differs from sh.
const WINDOWS_RUN_COMMAND_DESCRIPTION: &str = "Run one PowerShell command; returns stdout, stderr and the exit code of its last statement. The shell is PowerShell 7 (pwsh) when installed, else Windows PowerShell 5.1; the `Shell` line of the runtime context says which (if it says cmd.exe, write cmd syntax instead). Write PowerShell, not POSIX sh: chain with `;` (`&&` and `||` exist only in PowerShell 7); no heredocs, create files with write_to_file; set variables with `$env:NAME = 'value'`; run a quoted path with `& 'C:\\dir with spaces\\tool.exe'`; `/` and `\\` both separate paths; discard output with `> $null`; in 5.1 `curl` and `wget` are aliases, so call `curl.exe`; when a `.ps1` shim is blocked by execution policy call the `.cmd` one (`npm.cmd`). stderr is already captured, so omit `2>&1`. Timeout defaults to 120s; the session is non-interactive. Prefer view_file, grep, glob and list_directory over Get-Content, Select-String and Get-ChildItem. There is no OS sandbox on Windows. background=true runs a long job (a build, a test suite) and resumes you on completion (notify_on_complete=false: no turn); a quick command needs no background. detached=true starts a long-lived server or watcher, returns a task ID at once and stays silent unless notify_on_complete=true. Use manage_task for logs, wait and kill; never poll. For external jobs run the provider's blocking watch command once with background=true. Git: branch and worktree handling follows the repository `AGENTS.md`, which outranks generic workflow skills. Never `git rebase`, `git reset --hard` or force-push in the active checkout, or discard uncommitted work. Branch there with `git switch -c`; `git worktree add` under `$env:TEMP` only when it is genuinely required (concurrent or unrelated dirty work), then `git worktree remove`, `git worktree prune`, `git branch -d`. End on the original branch after `git pull --ff-only`.";

const fn run_command_description(windows: bool) -> &'static str {
    if windows {
        WINDOWS_RUN_COMMAND_DESCRIPTION
    } else {
        POSIX_RUN_COMMAND_DESCRIPTION
    }
}

pub const RUN_COMMAND: Tool = Tool {
    name: "run_command",
    description: run_command_description(cfg!(target_os = "windows")),
    arguments: r#"{"command": "full shell command string", "cwd": "optional working directory", "timeout_ms": "optional timeout in ms", "background": "optional bool for asynchronous execution that pauses until completion (default false)", "detached": "optional bool for a long-lived server/watcher; returns a completed start result with task ID and keeps it killable (default false)", "notify_on_complete": "optional bool; defaults false for detached and true for background", "network_access": "optional bool requesting one-shot network access; requires confirmation unless YOLO is enabled", "filesystem_write_path": "optional existing absolute directory requested for one-command write access; requires confirmation unless YOLO is enabled"}"#,
    handler: run_command,
    requires_confirmation: true,
    schema: run_command_schema,
    capabilities: &[ToolCapability::ExecuteCommands],
    safety: ToolSafety::ProcessControl,
};

fn manage_task_schema() -> Value {
    serde_json::json!({
        "type": "object", "properties": {
            "action": { "type": "string", "enum": ["list", "status", "logs", "kill", "wait"] },
            "task_id": { "type": "string" },
            "task_ids": { "type": "array", "items": { "type": "string" } },
            "timeout_ms": { "type": "integer", "minimum": 1000, "maximum": 1800000 },
            "tail_bytes": { "type": "integer", "minimum": 1, "maximum": 65536 },
            "full": { "type": "boolean", "description": "Whole log instead of its tail" }
        }, "required": ["action"]
    })
}

pub const MANAGE_TASK: Tool = Tool {
    name: "manage_task",
    description: "Manage run_command tasks. 'wait' blocks until a task (or the first of task_ids) finishes; never poll. 'logs' returns captured output. Detached tasks are silent unless waited on or started with notify_on_complete.",
    arguments: r#"{"action": "list, status, logs, kill, or wait", "task_id": "required for status/kill/logs or single-task wait", "task_ids": "optional list of IDs for wait-any", "full": "optional true to retrieve the complete log", "tail_bytes": "optional log tail limit, max 65536", "timeout_ms": "optional wait timeout in ms, default 600000, max 1800000"}"#,
    handler: manage_task_tool,
    requires_confirmation: false,
    schema: manage_task_schema,
    capabilities: &[ToolCapability::ExecuteCommands],
    safety: ToolSafety::ProcessControl,
};

const DEFAULT_COMMAND_TIMEOUT_MS: u64 = 120_000;

/// Return whether a command contains an unquoted shell background operator.
/// This deliberately recognizes only a standalone `&`; `&&`, redirections,
/// and quoted/escaped ampersands are not background jobs.
#[cfg(not(target_os = "windows"))]
pub(crate) fn has_shell_background_operator(command: &str) -> bool {
    let bytes = command.as_bytes();
    let mut single_quote = false;
    let mut double_quote = false;
    let mut escaped = false;

    for (index, &byte) in bytes.iter().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        if byte == b'\\' && !single_quote {
            escaped = true;
            continue;
        }
        if byte == b'\'' && !double_quote {
            single_quote = !single_quote;
            continue;
        }
        if byte == b'"' && !single_quote {
            double_quote = !double_quote;
            continue;
        }
        if byte != b'&' || single_quote || double_quote {
            continue;
        }

        let previous = index.checked_sub(1).and_then(|i| bytes.get(i)).copied();
        let next = bytes.get(index + 1).copied();
        let is_and = previous == Some(b'&') || next == Some(b'&');
        let is_redirection = matches!(previous, Some(b'>') | Some(b'<'))
            || matches!(next, Some(b'>') | Some(b'<') | Some(b'0'..=b'9'));
        if !is_and && !is_redirection {
            return true;
        }
    }
    false
}

#[cfg(target_os = "windows")]
pub(crate) fn has_shell_background_operator(_command: &str) -> bool {
    // PowerShell's `&` is the call operator (`& 'C:\tool.exe'`) and `cmd.exe`
    // uses it as a command separator; neither is a portable background
    // operator. Detached callers should use detached=true without adding
    // shell syntax.
    false
}

/// Return whether a command already synchronizes its own background jobs via
/// `wait` or via `$!` paired with `kill`/`pkill`.
///
/// Such scripts (e.g. `server > log 2>&1 & pid=$!; …; curl …; kill $pid`)
/// start a helper, verify it, then tear it down in one compound command. The
/// verification output is the point of the call, so auto-detaching on the
/// embedded `&` would discard exactly what the author meant to read. These
/// run in the foreground under the normal timeout instead; a bare trailing
/// `&` with no synchronization still auto-detaches.
#[cfg(not(target_os = "windows"))]
fn command_manages_own_background_jobs(command: &str) -> bool {
    // Strip quoted spans (and blank escaped chars) so prose like
    // `echo "wait $!"` cannot opt out of the background safety net.
    let mut code = String::with_capacity(command.len());
    let bytes = command.as_bytes();
    let mut single_quote = false;
    let mut double_quote = false;
    let mut escaped = false;
    for &byte in bytes {
        if escaped {
            escaped = false;
            code.push(' ');
            continue;
        }
        if byte == b'\\' && !single_quote {
            escaped = true;
            code.push(' ');
            continue;
        }
        if byte == b'\'' && !double_quote {
            single_quote = !single_quote;
            continue;
        }
        if byte == b'"' && !single_quote {
            double_quote = !double_quote;
            continue;
        }
        if single_quote || double_quote {
            continue;
        }
        code.push(byte as char);
    }
    let words: Vec<&str> = code
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|token| !token.is_empty())
        .collect();
    let has_wait = words.iter().any(|token| *token == "wait");
    let has_pid_ref = code.contains("$!");
    let has_kill = words
        .iter()
        .any(|token| *token == "kill" || *token == "pkill" || *token == "killall");
    has_wait || (has_pid_ref && has_kill)
}

#[cfg(target_os = "windows")]
fn command_manages_own_background_jobs(_command: &str) -> bool {
    false
}

/// Keep a detached shell alive for its background children while ensuring no
/// child inherits RustCode's output pipes. The shell remains the process-group
/// leader (the process-tree root on Windows), so the task manager can still
/// terminate everything it started.
fn detached_shell_command(command: &str, has_background_operator: bool) -> String {
    rustcode_command::detached_command(
        rustcode_command::host_shell(),
        command,
        has_background_operator,
    )
}

pub fn run_command(args: &Value) -> Result<String, String> {
    run_command_output(args).map(|output| output.content)
}

pub(super) fn run_command_output(args: &Value) -> Result<super::ToolExecutionOutput, String> {
    run_command_output_inner(args, None, None, None, None)
}

#[cfg(test)]
pub(crate) fn run_command_output_with_workspace(
    args: &Value,
    workspace_root: Option<std::path::PathBuf>,
) -> Result<super::ToolExecutionOutput, String> {
    run_command_output_inner(args, None, None, None, Some(workspace_root))
}

pub(crate) fn run_command_output_with_workspace_for_call(
    args: &Value,
    cancel_token: Option<tokio_util::sync::CancellationToken>,
    call_id: Option<&str>,
    workspace_root: Option<std::path::PathBuf>,
) -> super::ToolExecutionOutput {
    run_command_output_with_workspace_and_progress_for_call(
        args,
        None,
        cancel_token,
        call_id,
        workspace_root,
    )
}

#[cfg(test)]
pub(crate) fn run_command_output_cancellable(
    args: &Value,
    cancel_token: Option<tokio_util::sync::CancellationToken>,
) -> Result<super::ToolExecutionOutput, String> {
    match run_command_output_inner(args, None, cancel_token, None, None) {
        Ok(output) => Ok(output),
        Err(error) if error == "command cancelled by user" => {
            Ok(super::ToolExecutionOutput::failure_with_kind(
                "error: tool execution cancelled by user".to_string(),
                super::ToolErrorKind::Cancelled,
                true,
            ))
        }
        Err(error) => Err(error),
    }
}

pub(crate) fn run_command_output_with_call_id(
    args: &Value,
    cancel_token: Option<tokio_util::sync::CancellationToken>,
    call_id: Option<&str>,
) -> Result<super::ToolExecutionOutput, String> {
    match run_command_output_inner(args, None, cancel_token, call_id, None) {
        Ok(output) => Ok(output),
        Err(error) if error == "command cancelled by user" => {
            Ok(super::ToolExecutionOutput::failure_with_kind(
                "error: tool execution cancelled by user".to_string(),
                super::ToolErrorKind::Cancelled,
                true,
            ))
        }
        Err(error) => Err(error),
    }
}

pub(crate) type CommandProgressCallback = rustcode_command::ProgressCallback;

#[cfg(test)]
pub(crate) fn run_command_output_with_progress(
    args: &Value,
    progress: CommandProgressCallback,
) -> Result<super::ToolExecutionOutput, String> {
    run_command_output_inner(args, Some(progress), None, None, None)
}

pub(crate) fn run_command_output_with_progress_cancellable_for_call_and_workspace(
    args: &Value,
    progress: CommandProgressCallback,
    cancel_token: Option<tokio_util::sync::CancellationToken>,
    call_id: Option<&str>,
    workspace_root: Option<std::path::PathBuf>,
) -> super::ToolExecutionOutput {
    run_command_output_with_workspace_and_progress_for_call(
        args,
        Some(progress),
        cancel_token,
        call_id,
        workspace_root,
    )
}

fn run_command_output_with_workspace_and_progress_for_call(
    args: &Value,
    progress: Option<CommandProgressCallback>,
    cancel_token: Option<tokio_util::sync::CancellationToken>,
    call_id: Option<&str>,
    workspace_root: Option<std::path::PathBuf>,
) -> super::ToolExecutionOutput {
    match run_command_output_inner(args, progress, cancel_token, call_id, Some(workspace_root)) {
        Ok(output) => output,
        Err(error) if error == "command cancelled by user" => {
            super::ToolExecutionOutput::failure_with_kind(
                "error: tool execution cancelled by user".to_string(),
                super::ToolErrorKind::Cancelled,
                true,
            )
        }
        Err(error) => super::ToolExecutionOutput::failure_with_kind(
            format!("error: {error}"),
            super::ToolErrorKind::CommandFailed,
            true,
        ),
    }
}

fn run_command_output_inner(
    args: &Value,
    progress: Option<CommandProgressCallback>,
    cancel_token: Option<tokio_util::sync::CancellationToken>,
    call_id: Option<&str>,
    explicit_workspace_root: Option<Option<std::path::PathBuf>>,
) -> Result<super::ToolExecutionOutput, String> {
    let command_str = args
        .get("command")
        .and_then(|c| c.as_str())
        .ok_or("missing 'command' argument")?;

    if let Some(reason) = reject_broad_git_stage(command_str) {
        return Err(reason.to_string());
    }

    if has_interactive_sudo(command_str.trim()) {
        return Err("Interactive 'sudo' commands requiring password input are disabled in subshell execution. Use non-privileged commands or pass 'sudo -n' to fail fast.".to_string());
    }

    let cwd = args.get("cwd").and_then(|c| c.as_str());
    let timeout_ms = args
        .get("timeout_ms")
        .and_then(parse_json_number)
        .unwrap_or(DEFAULT_COMMAND_TIMEOUT_MS);
    let env = args.get("env").and_then(|e| e.as_object());

    let context = super::current_tool_context();
    #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
    let has_explicit_workspace_root = explicit_workspace_root.is_some();
    let workspace_root = explicit_workspace_root.unwrap_or_else(|| super::active_workspace_root());
    let resolved_cwd = match cwd {
        Some("sandbox") | Some("./sandbox") => {
            if let Some(session_id) = get_active_session_id() {
                crate::config::get_active_session_sandbox_dir(&session_id)
            } else {
                None
            }
        }
        Some(other) => Some(
            rustcode_tools::validate_tool_path_with_context(other, &context, false)
                .map_err(|error| format!("invalid command cwd: {error}"))?,
        ),
        None => context
            .task_working_directory
            .clone()
            .or(workspace_root.clone()),
    };

    if let Some(ref cwd_path) = resolved_cwd
        && !cwd_path.is_dir()
    {
        return Err(format!("cwd '{}' is not a directory", cwd_path.display()));
    }

    if let Some(base) = pull_request_base(command_str)
        && remote_base_missing(resolved_cwd.as_deref(), &base)
    {
        return Ok(pr_creation_guard_output(command_str, &base));
    }

    // GUI/Dock launches don't inherit the shell PATH, so agent-run builds/tests
    // (cargo, npm, …) fail to find their toolchain. Seed a toolchain-aware PATH;
    // an explicit PATH in `env` below still overrides it.
    let mut command_env = vec![(
        std::ffi::OsString::from("PATH"),
        std::ffi::OsString::from(crate::platform::augmented_path()),
    )];
    if let Some(env_map) = env {
        for (k, v) in env_map {
            if let Some(val) = v.as_str() {
                command_env.push((k.clone().into(), val.into()));
            }
        }
    }

    let background_requested = args
        .get("background")
        .and_then(parse_json_bool)
        .unwrap_or(false);
    let detached_requested = args
        .get("detached")
        .and_then(parse_json_bool)
        .unwrap_or(false);
    let has_background_operator = has_shell_background_operator(command_str);
    // A shell-level `&` can outlive the shell. Treat it as detached even when
    // the model omitted the JSON background flag, so the child cannot inherit
    // RustCode's pipes and keep a foreground turn stuck indefinitely. Scripts
    // that synchronize their own background jobs (`wait`, or `$!` with
    // `kill`) are exempt: they reap what they spawn, so the verification
    // output they print must be preserved, not discarded.
    let detached = detached_requested
        || (has_background_operator && !command_manages_own_background_jobs(command_str));
    let run_in_bg = (background_requested || detached)
        && (detached || !is_short_discovery_command(command_str));
    let notify_on_complete = args
        .get("notify_on_complete")
        .and_then(parse_json_bool)
        .unwrap_or(!detached);
    let shell_command = if detached {
        detached_shell_command(command_str, has_background_operator)
    } else {
        command_str.to_owned()
    };
    let session_scratch = get_active_session_id()
        .and_then(|session_id| crate::config::get_active_session_sandbox_dir(&session_id));
    #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
    let workspace_root = if !has_explicit_workspace_root {
        workspace_root.or_else(|| {
            resolved_cwd.clone().or_else(|| {
                static TEST_WORKSPACE: std::sync::OnceLock<tempfile::TempDir> =
                    std::sync::OnceLock::new();
                Some(
                    TEST_WORKSPACE
                        .get_or_init(|| tempfile::tempdir().expect("test workspace"))
                        .path()
                        .to_path_buf(),
                )
            })
        })
    } else {
        workspace_root
    };
    let mut writable_roots = workspace_root.iter().cloned().collect::<Vec<_>>();
    let session_scratch_roots = session_scratch.iter().cloned().collect::<Vec<_>>();
    writable_roots.extend(session_scratch_roots.iter().cloned());
    // A user-authorized task worktree outside the original checkout stays
    // writable for the task/session lifetime (#1496). It is part of the
    // active tool context (set by ACP, CLI workspace, or /workspace), not a
    // per-command one-shot grant, and unrelated sessions do not inherit it.
    if let Some(task_dir) = context.task_working_directory.clone()
        && task_dir.is_dir()
        && !writable_roots.iter().any(|root| root == &task_dir)
    {
        writable_roots.push(task_dir);
    }
    let sandbox_mode = super::active_sandbox_mode();
    let one_shot_network_access = args
        .get("network_access")
        .and_then(parse_json_bool)
        .unwrap_or(false);
    let one_shot_writable_roots =
        if !sandbox_mode.is_trusted() && args.get("filesystem_write_path").is_some() {
            let path = args
                .get("filesystem_write_path")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    "filesystem_write_path must be an absolute directory string".to_string()
                })?;
            vec![sandbox::resolve_scoped_writable_root(
                path,
                workspace_root.as_deref().ok_or_else(|| {
                    "one-shot filesystem permission requires an active workspace".to_string()
                })?,
            )?]
        } else {
            Vec::new()
        };
    // Trusted mode bypasses OS sandbox wrapping and runs with the RustCode
    // process's own permissions (#1496). It is the default and the YOLO override;
    // explicit user config can opt into restrictions, project files cannot change it, and
    // shell approval policy still applies. Command failures (GitHub auth,
    // SSH, certificates, network) surface as tool results and must not be
    // misattributed to sandbox denial.
    let sandboxed = if sandbox_mode.is_trusted() {
        sandbox::passthrough_command(&shell_command)
    } else {
        sandbox::command(
            &shell_command,
            sandbox::SandboxPolicy {
                command_cwd: resolved_cwd.as_deref(),
                workspace_root: workspace_root.as_deref(),
                writable_roots: &writable_roots,
                session_scratch_roots: &session_scratch_roots,
                one_shot_writable_roots: &one_shot_writable_roots,
                write_access: sandbox_mode.allows_workspace_write(),
                network_access: sandbox_mode.allows_network() || one_shot_network_access,
            },
        )?
    };
    let command_request = rustcode_command::CommandRequest {
        command: sandboxed.command,
        status_command: Some(command_str.to_owned()),
        sandboxed_shell: true,
        cwd: resolved_cwd.clone(),
        env: command_env,
        timeout: Duration::from_millis(timeout_ms.max(1)),
        process_group: true,
        inherited_fds: sandboxed.inherited_fds,
    };

    let sandbox_context = CommandSandboxContext {
        mode: sandbox_mode,
        network_access: sandbox_mode.allows_network() || one_shot_network_access,
        writable_roots: &writable_roots,
        one_shot_writable_roots: &one_shot_writable_roots,
    };

    if run_in_bg {
        let session_id = get_active_session_id().unwrap_or_default();
        let cmd_str = command_str.to_string();
        let task_id = format!(
            "task_{}_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or(std::time::Duration::from_secs(0))
                .as_millis(),
            BACKGROUND_TASK_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        );

        let task_manager = background_task_manager();
        let mut task_spec = TaskSpec::new(SessionId::new(session_id.clone()), command_request);
        // Keep the user-visible task identity useful even when the detached
        // request uses an internal shell wrapper for pipe/process safety.
        task_spec.command = cmd_str.clone();
        task_spec.notify_on_complete = notify_on_complete;
        task_spec.output_log = task_output_log_path(&session_id, &task_id)?;
        let task_spec = call_id
            .map(|call_id| task_spec.clone().with_call_id(call_id))
            .unwrap_or(task_spec);
        let output_log = task_spec.output_log.clone();
        let start_barrier = call_id
            .filter(|_| crate::acp::is_acp_session(&session_id))
            .map(|call_id| register_background_start(&session_id, call_id));
        let has_start_barrier = start_barrier.is_some();
        // A background job also holds its terminal event behind a barrier, so
        // one that finishes within the grace period can be returned below
        // without a completion notice following it.
        let inline_barrier = if detached {
            None
        } else {
            Some(start_barrier.clone().unwrap_or_default())
        };
        let spawn_result = if let Some(barrier) = start_barrier.or(inline_barrier.clone()) {
            task_manager.spawn_with_id_and_start_barrier(task_id.clone(), task_spec, barrier)
        } else {
            task_manager.spawn_with_id(task_id.clone(), task_spec)
        };
        if let Err(error) = spawn_result {
            if has_start_barrier && let Some(call_id) = call_id {
                release_background_start(call_id);
            }
            return Err(format!("failed to start background task: {error}"));
        }

        // Sequential tool batches execute the next call after this function
        // returns. Wait only for the child PID publication, not completion, so
        // a follow-up request can observe a process that really exists.
        task_manager.wait_until_started(&task_id, Duration::from_secs(5));

        if detached {
            return Ok(super::ToolExecutionOutput {
                content: format!(
                    "Detached task started. Task ID: {task_id}. Status: Running. Command: {cmd_str}. Completion notification: {}. Captured output is available with manage_task action 'logs'.",
                    if notify_on_complete {
                        "enabled"
                    } else {
                        "disabled"
                    }
                ),
                success: true,
                pending: false,
                command: Some(cmd_str),
                // This is a completed *start* result, not the server's exit
                // result; the eventual task event carries that separately.
                exit_code: None,
                truncated: false,
                completeness: rustcode_core::ToolResultCompleteness::Complete,
                replayed: false,
                error_kind: None,
                retryable: false,
                command_status: Some(rustcode_core::CommandResultMetadata {
                    completed: false,
                    ..Default::default()
                }),
            });
        }

        if let Some(barrier) = inline_barrier {
            let finished =
                finished_within_grace(task_manager, &session_id, &task_id, cancel_token.as_ref());
            if let Some(output) = finished
                && barrier.discard()
            {
                // Consumed here: forget the task so nothing reports it again.
                task_manager.take_completion(&session_id, &task_id);
                if has_start_barrier && let Some(call_id) = call_id {
                    release_background_start(call_id);
                }
                if let Some(path) = output_log {
                    let _ = std::fs::remove_file(path);
                }
                return Ok(foreground_command_output(&output, &sandbox_context));
            }
            if !has_start_barrier {
                barrier.release();
            }
        }

        return Ok(super::ToolExecutionOutput {
            content: format!(
                "Task started in background. Task ID: {task_id}. Status: Pending. Command: {cmd_str}. Completion notification: {}. Use manage_task action 'wait' to block until it finishes; captured output is available with manage_task action 'logs'.",
                if notify_on_complete {
                    "enabled"
                } else {
                    "disabled"
                }
            ),
            success: !notify_on_complete,
            pending: notify_on_complete,
            command: Some(cmd_str),
            exit_code: None,
            truncated: false,
            completeness: rustcode_core::ToolResultCompleteness::Complete,
            replayed: false,
            error_kind: None,
            retryable: false,
            command_status: Some(rustcode_core::CommandResultMetadata {
                completed: false,
                ..Default::default()
            }),
        });
    }

    // Admission, cwd and sandbox validation above still apply to cache hits.
    // Explicit environment/grant overrides and background jobs bypass reuse.
    let verification_identity = if !run_in_bg
        && env.is_none()
        && !one_shot_network_access
        && one_shot_writable_roots.is_empty()
        && !cancel_token
            .as_ref()
            .is_some_and(|token| token.is_cancelled())
    {
        resolved_cwd.as_deref().and_then(|root| {
            crate::network::compiler::verification_identity(root, command_str, sandbox_mode)
        })
    } else {
        None
    };
    if let Some(content) = verification_identity
        .as_ref()
        .and_then(crate::network::compiler::cached_verification)
        && !cancel_token
            .as_ref()
            .is_some_and(|token| token.is_cancelled())
    {
        let mut output = super::ToolExecutionOutput::success(content);
        output.exit_code = Some(0);
        output.replayed = true;
        output.command = Some(command_str.to_owned());
        output.command_status = Some(rustcode_core::CommandResultMetadata {
            completed: true,
            exit_code: Some(0),
            ..Default::default()
        });
        return Ok(output);
    }
    let verification_cancel = cancel_token.clone();
    let cancellation = cancel_token.map(|token| {
        std::sync::Arc::new(move || token.is_cancelled()) as rustcode_command::CancellationCallback
    });
    let output =
        rustcode_command::run_with_timeout_cancellable(&command_request, progress, cancellation)?;
    let result = foreground_command_output(&output, &sandbox_context);
    if result.success
        && !result.truncated
        && !verification_cancel
            .as_ref()
            .is_some_and(|token| token.is_cancelled())
    {
        if let Some(identity) = verification_identity {
            crate::network::compiler::record_verification(identity, result.content.clone());
        }
    }
    Ok(result)
}

/// What a failed command's sandbox attribution note needs to know.
struct CommandSandboxContext<'a> {
    mode: crate::config::SandboxMode,
    network_access: bool,
    writable_roots: &'a [std::path::PathBuf],
    one_shot_writable_roots: &'a [std::path::PathBuf],
}

/// Render a finished command as the `run_command` result the model reads.
fn foreground_command_output(
    output: &rustcode_command::CommandOutput,
    sandbox: &CommandSandboxContext<'_>,
) -> super::ToolExecutionOutput {
    let exit_code = output.exit_code.unwrap_or(-1);

    let command_status = command_result_metadata(output);
    let mut result = String::new();
    result.push_str(&format!(
        "{}\nexit code: {exit_code}\n",
        format_command_status(output.success, &command_status)
    ));

    let failed = !output.success;
    let truncated = output.stdout.is_truncated() || output.stderr.is_truncated();
    let stdout = rustcode_command::format_bounded_output(&output.stdout);
    let stderr = rustcode_command::format_bounded_output(&output.stderr);

    if !stdout.is_empty() {
        result.push_str("stdout:\n");
        result.push_str(&stdout);
        if !stdout.ends_with('\n') {
            result.push('\n');
        }
    }
    if !stderr.is_empty() {
        result.push_str("stderr:\n");
        result.push_str(&stderr);
        if !stderr.ends_with('\n') {
            result.push('\n');
        }
    }
    if stdout.is_empty() && stderr.is_empty() {
        result.push_str("(no output)\n");
    }
    // A failed command's own stderr is ambiguous: "error connecting to
    // api.github.com" reads identically to a sandbox network denial and to a
    // dead VPN. Name the restriction the active mode actually enforces so the
    // model sees effective grants rather than only configured defaults.
    // Trusted and unsupported platforms are unwrapped, so they get no attribution.
    if failed && !sandbox.mode.is_trusted() && cfg!(any(target_os = "linux", target_os = "macos")) {
        let observed = format!("{stdout}\n{stderr}");
        let denial = sandbox::classify_denial(&observed);
        // Roots the launcher actually used, so the note can name them.
        let roots = effective_writable_roots(
            sandbox.writable_roots,
            sandbox.one_shot_writable_roots,
            sandbox.mode.allows_workspace_write(),
        );
        result.push('\n');
        result.push_str(&sandbox::failure_attribution(
            sandbox.mode,
            sandbox.network_access,
            true,
            denial,
            &roots,
        ));
        result.push('\n');
    }
    super::ToolExecutionOutput {
        content: result.trim_end().to_string(),
        success: !failed,
        pending: false,
        command: None,
        exit_code: Some(exit_code),
        truncated,
        completeness: if truncated {
            rustcode_core::ToolResultCompleteness::ByteTruncated
        } else {
            rustcode_core::ToolResultCompleteness::Complete
        },
        replayed: false,
        error_kind: failed.then_some(super::ToolErrorKind::CommandFailed),
        retryable: false,
        command_status: Some(command_status),
    }
}

/// The directories a sandboxed command could write to, mirroring what the
/// launcher does with the policy: a read-only mode grants only one-shot
/// grants add to the configured roots. Used only to name them in a denial
/// note; canonicalization and symlink checks stay in the launcher.
fn effective_writable_roots(
    writable_roots: &[std::path::PathBuf],
    one_shot_writable_roots: &[std::path::PathBuf],
    write_access: bool,
) -> Vec<std::path::PathBuf> {
    let mut roots = if write_access {
        writable_roots.to_vec()
    } else {
        Vec::new()
    };
    for root in one_shot_writable_roots {
        if !roots.contains(root) {
            roots.push(root.clone());
        }
    }
    roots
}

/// Check the remote used by the default `gh` repository resolution without
/// running the requested PR mutation. A failed `ls-remote` caused by a
/// transport/authentication problem is deliberately inconclusive and lets
/// `gh` report that real problem; only an absent remote or absent ref blocks.
fn remote_base_missing(cwd: Option<&std::path::Path>, base: &str) -> bool {
    let mut remote = std::process::Command::new("git");
    if let Some(cwd) = cwd {
        remote.current_dir(cwd);
    }
    let Ok(remote) = remote.args(["remote", "get-url", "origin"]).output() else {
        return false;
    };
    if !remote.status.success() {
        return true;
    }

    let ref_name = format!("refs/heads/{base}");
    let mut refs = std::process::Command::new("git");
    if let Some(cwd) = cwd {
        refs.current_dir(cwd);
    }
    let Ok(refs) = refs
        .args(["ls-remote", "--exit-code", "--heads", "origin", &ref_name])
        .output()
    else {
        return false;
    };
    refs.status.code() == Some(2)
}

fn pr_creation_guard_output(command: &str, base: &str) -> super::ToolExecutionOutput {
    super::ToolExecutionOutput {
        content: format!(
            "[harness: PR creation blocked — remote base branch `{base}` does not exist on `origin`. Create or push that base branch, or choose an existing remote base, before retrying `gh pr create`. The PR command was not run.]"
        ),
        success: false,
        pending: false,
        command: Some(command.to_owned()),
        exit_code: Some(2),
        truncated: false,
        completeness: rustcode_core::ToolResultCompleteness::Complete,
        replayed: false,
        error_kind: Some(super::ToolErrorKind::CommandFailed),
        retryable: false,
        command_status: Some(rustcode_core::CommandResultMetadata {
            completed: true,
            exit_code: Some(2),
            ..Default::default()
        }),
    }
}

/// How long a `background=true` start waits for the command to finish before
/// handing back a task ID. Quick commands sent to the background otherwise
/// cost a second model request just to read their result.
const BACKGROUND_INLINE_GRACE: Duration = Duration::from_millis(1500);

/// The output of a background task that exited on its own within the grace
/// period. Cancelled or unspawnable tasks, and a cancelled tool call, report
/// through the ordinary background path instead.
fn finished_within_grace(
    manager: &TaskManager,
    session_id: &str,
    task_id: &str,
    cancel_token: Option<&tokio_util::sync::CancellationToken>,
) -> Option<rustcode_command::CommandOutput> {
    let deadline = std::time::Instant::now() + BACKGROUND_INLINE_GRACE;
    loop {
        if cancel_token.is_some_and(|token| token.is_cancelled()) {
            return None;
        }
        if let Some(completion) = manager.completion(session_id, task_id) {
            return completion.output;
        }
        if std::time::Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

const DEFAULT_WAIT_TIMEOUT_MS: u64 = 600_000;
const MAX_WAIT_TIMEOUT_MS: u64 = 1_800_000;

fn wait_timeout(args: &Value) -> std::time::Duration {
    let ms = args
        .get("timeout_ms")
        .and_then(|timeout| timeout.as_u64())
        .unwrap_or(DEFAULT_WAIT_TIMEOUT_MS);
    std::time::Duration::from_millis(ms.clamp(1_000, MAX_WAIT_TIMEOUT_MS))
}

fn task_output_log_path(
    session_id: &str,
    task_id: &str,
) -> Result<Option<std::path::PathBuf>, String> {
    let Some(session_dir) = crate::config::get_active_session_dir(session_id) else {
        return Ok(None);
    };
    let directory = session_dir.join("task-logs");
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("failed to create private task log directory: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("failed to protect task log directory: {error}"))?;
    }
    Ok(Some(directory.join(format!("{task_id}.log"))))
}

/// Terminal results `manage_task` `wait` already handed to the model, keyed by
/// (session, task). The completion notice for the same task would repeat the
/// output, so the turn loop drops it instead of delivering it twice.
fn wait_delivered() -> &'static Mutex<std::collections::HashSet<(String, String)>> {
    static DELIVERED: OnceLock<Mutex<std::collections::HashSet<(String, String)>>> =
        OnceLock::new();
    DELIVERED.get_or_init(Default::default)
}

pub(crate) fn note_wait_delivered(session_id: &str, task_id: &str) {
    wait_delivered()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert((session_id.to_owned(), task_id.to_owned()));
}

/// Whether `wait` already returned this task's terminal result to the model.
pub(crate) fn background_result_delivered_by_wait(session_id: &str, task_id: &str) -> bool {
    wait_delivered()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .contains(&(session_id.to_owned(), task_id.to_owned()))
}

/// Block until one background task reaches a terminal state, so agents wait
/// with a single tool call instead of hand-rolled sleep/poll shell loops.
/// Runs on a blocking worker thread; Esc still interrupts the wait through
/// the normal tool-cancellation path.
fn wait_for_background_tasks(
    manager: &TaskManager,
    session_id: &str,
    task_ids: &[String],
    timeout: std::time::Duration,
) -> String {
    // Subscribe before checking the roster: a task that finishes after this
    // point still delivers its terminal event to us, while one that finished
    // before is simply absent from the roster below.
    let subscription = manager.subscribe_session(session_id.to_owned());
    let deliver = |completion: &rustcode_tasks::TaskCompletion| {
        note_wait_delivered(session_id, completion.id.as_str());
        format_completion_result(completion)
    };
    if task_ids.is_empty() {
        return "No task IDs were provided to wait for.".to_owned();
    }
    if let Some(completion) = manager
        .completions(session_id)
        .into_iter()
        .find(|completion| task_ids.iter().any(|id| id == completion.id.as_str()))
    {
        return deliver(&completion);
    }
    let running = manager.list(session_id);
    let matched = task_ids
        .iter()
        .filter(|id| running.iter().any(|task| task.id.as_str() == id.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    if matched.is_empty() {
        if let Some(completion) = manager
            .completions(session_id)
            .into_iter()
            .find(|completion| task_ids.iter().any(|id| id == completion.id.as_str()))
        {
            return deliver(&completion);
        }
        return format!(
            "None of the requested tasks are running or retained: {}",
            task_ids.join(", ")
        );
    }
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) else {
            break;
        };
        match subscription.recv_timeout(remaining) {
            Ok(event)
                if task_ids.iter().any(|id| id == event.task_id().as_str())
                    && event.is_terminal() =>
            {
                if let Some(completion) = manager.completion(session_id, event.task_id()) {
                    return deliver(&completion);
                }
                let task_id = event.task_id().to_string();
                note_wait_delivered(session_id, &task_id);
                return format_wait_result(&task_id, event);
            }
            Ok(_) => continue,
            Err(_) => break,
        }
    }
    // The wait expired. A task that finished in a way this fresh
    // subscription could not observe is already gone from the roster; its
    // result still arrives via the automatic completion notice.
    if let Some(completion) = manager
        .completions(session_id)
        .into_iter()
        .find(|completion| task_ids.iter().any(|id| id == completion.id.as_str()))
    {
        deliver(&completion)
    } else {
        let still_running = manager.list(session_id);
        let running_ids = matched
            .iter()
            .filter(|id| {
                still_running
                    .iter()
                    .any(|task| task.id.as_str() == id.as_str())
            })
            .cloned()
            .collect::<Vec<_>>();
        if running_ids.is_empty() {
            format!(
                "Requested tasks are no longer retained: {}",
                task_ids.join(", ")
            )
        } else {
            format!(
                "Tasks {} are still running after {}s. Call 'wait' again to keep waiting, or continue other work meanwhile.",
                running_ids.join(", "),
                timeout.as_secs()
            )
        }
    }
}

fn format_completion_result(completion: &rustcode_tasks::TaskCompletion) -> String {
    use rustcode_tasks::TaskTerminalReason;
    let status = match &completion.reason {
        TaskTerminalReason::Exited {
            success: true,
            code,
        } => {
            format!(
                "exited successfully (code {})",
                code.map_or("unknown".to_owned(), |c| c.to_string())
            )
        }
        TaskTerminalReason::Exited {
            success: false,
            code,
        } => {
            format!(
                "failed (exit code {})",
                code.map_or("unknown".to_owned(), |c| c.to_string())
            )
        }
        TaskTerminalReason::Signalled { signal } => {
            format!("terminated by {} ({signal})", signal_name(*signal))
        }
        TaskTerminalReason::Cancelled => "cancelled".to_owned(),
        TaskTerminalReason::SpawnFailed(error) => format!("spawn failed: {error}"),
        TaskTerminalReason::Failed(error) => format!("failed: {error}"),
    };
    let output = completion
        .output
        .as_ref()
        .map(|output| {
            let stdout = rustcode_command::format_bounded_output(&output.stdout);
            let stderr = rustcode_command::format_bounded_output(&output.stderr);
            let mut text = String::new();
            if !stdout.is_empty() {
                text.push_str(&stdout);
            }
            if !stderr.is_empty() {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str("stderr:\n");
                text.push_str(&stderr);
            }
            if text.is_empty() {
                "[no stdout or stderr was produced]".to_owned()
            } else {
                text
            }
        })
        .or_else(|| completion.error.clone())
        .unwrap_or_else(|| {
            if completion.output_log.is_some() {
                "[no in-memory output retained; inspect the captured log]".to_owned()
            } else {
                "[no output captured]".to_owned()
            }
        });
    format!(
        "Task '{}' {}. Started at {} ms; ended at {} ms. Command: {}. Output:\n{}{}{}",
        completion.id,
        status,
        completion.started_at_unix_ms,
        completion.ended_at_unix_ms,
        completion.command,
        output,
        completion
            .log_error
            .as_ref()
            .map_or_else(String::new, |error| format!(
                "\nLog capture stopped early: {error}"
            )),
        if completion.output_log.is_some() {
            format!(
                "\nCaptured log: manage_task action 'logs' for task '{}'.",
                completion.id
            )
        } else {
            String::new()
        }
    )
}

fn signal_name(signal: i32) -> &'static str {
    match signal {
        1 => "SIGHUP",
        2 => "SIGINT",
        3 => "SIGQUIT",
        6 => "SIGABRT",
        9 => "SIGKILL",
        13 => "SIGPIPE",
        14 => "SIGALRM",
        15 => "SIGTERM",
        _ => "signal",
    }
}

fn task_state_label(state: TaskState) -> &'static str {
    match state {
        TaskState::Starting => "STARTING",
        TaskState::Running { .. } => "RUNNING",
        TaskState::Terminating { .. } => "TERMINATING",
        TaskState::CancelRequested => "CANCEL REQUESTED",
    }
}

fn task_wait_hint(task_id: &str, notify_on_complete: bool, has_log: bool) -> String {
    let policy = if notify_on_complete {
        "A completion turn is enabled."
    } else {
        "Completion is silent; use manage_task action 'wait' when you need the result."
    };
    let log = if has_log {
        format!(" Full output is available with manage_task action 'logs' for '{task_id}'.")
    } else {
        String::new()
    };
    format!("{policy}{log}")
}

pub(crate) fn read_task_log(
    path: &std::path::Path,
    full: bool,
    tail_bytes: usize,
) -> Result<(String, usize, usize), String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file =
        std::fs::File::open(path).map_err(|error| format!("failed to read task log: {error}"))?;
    let total = file
        .metadata()
        .map_err(|error| format!("failed to inspect task log: {error}"))?
        .len() as usize;
    let start = if full {
        0
    } else {
        total.saturating_sub(tail_bytes)
    };
    file.seek(SeekFrom::Start(start as u64))
        .map_err(|error| format!("failed to seek task log: {error}"))?;
    let mut bytes = Vec::with_capacity(total.saturating_sub(start));
    file.take(total.saturating_sub(start) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("failed to read task log: {error}"))?;
    Ok((String::from_utf8_lossy(&bytes).into_owned(), start, total))
}

fn format_wait_result(task_id: &str, event: TaskEvent) -> String {
    match task_event_to_tool_output(event) {
        Some((_, _, output)) => {
            let status = if output.success {
                "finished successfully"
            } else {
                "finished"
            };
            format!("Task '{task_id}' {status}. Output:\n{}", output.content)
        }
        None => format!("Task '{task_id}' finished."),
    }
}

/// Task IDs named by `task_id` and `task_ids` together, in order and without
/// duplicates. An empty or blank field counts as absent, so an empty
/// `task_ids` sent alongside a valid `task_id` cannot hide it.
fn requested_task_ids(args: &Value) -> Vec<String> {
    let mut ids = Vec::<String>::new();
    let listed = args.get("task_ids").and_then(Value::as_array);
    for id in args
        .get("task_id")
        .into_iter()
        .chain(listed.into_iter().flatten())
        .filter_map(Value::as_str)
        .map(str::trim)
    {
        if !id.is_empty() && !ids.iter().any(|known| known == id) {
            ids.push(id.to_owned());
        }
    }
    ids
}

/// The one task a single-task action names, through either field.
fn single_task_id(args: &Value, action: &str) -> Result<String, String> {
    let mut ids = requested_task_ids(args);
    match ids.len() {
        0 => Err(format!("missing 'task_id' argument for {action} action")),
        1 => Ok(ids.remove(0)),
        _ => Err(format!(
            "'{action}' takes one task; pass a single 'task_id' (got {})",
            ids.join(", ")
        )),
    }
}

pub fn manage_task_tool(args: &Value) -> Result<String, String> {
    let action = args
        .get("action")
        .and_then(|a| a.as_str())
        .ok_or("missing 'action' argument (must be 'list', 'status', 'kill', or 'wait')")?;

    let session_id = get_active_session_id().unwrap_or_default();
    let manager = background_task_manager();
    let tasks = manager.list(&session_id);

    match action {
        "list" => {
            if tasks.is_empty() {
                return Ok("No running background tasks.".to_string());
            }
            let mut out = String::from("Running background tasks:\n");
            for info in &tasks {
                let elapsed = info.started_at.elapsed().as_secs();
                let pid_str = task_pid(info.state)
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| "N/A".to_string());
                out.push_str(&format!(
                    "- TaskId: {}, State: {}, PID: {}, Runtime: {}s, Command: {} · notify={} · logs={}\n",
                    info.id,
                    task_state_label(info.state),
                    pid_str,
                    elapsed,
                    info.command,
                    if info.notify_on_complete { "on" } else { "off" },
                    if info.output_log.is_some() { "available" } else { "unavailable" }
                ));
            }
            out.push_str("\nUse manage_task action 'wait' with task_ids to return on the first completion in a set.");
            Ok(out.trim_end().to_string())
        }
        "status" => {
            let task_id = single_task_id(args, "status")?;
            let task_id = task_id.as_str();

            if let Some(info) = tasks.iter().find(|info| info.id.as_str() == task_id) {
                let elapsed = info.started_at.elapsed().as_secs();
                let pid_str = task_pid(info.state)
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| "N/A".to_string());
                Ok(format!(
                    "TaskId: {}, Status: {}, PID: {}, Runtime: {}s, Command: {}\n{}",
                    task_id,
                    task_state_label(info.state),
                    pid_str,
                    elapsed,
                    info.command,
                    task_wait_hint(task_id, info.notify_on_complete, info.output_log.is_some())
                ))
            } else if let Some(completion) = manager.completion(&session_id, task_id) {
                Ok(format_completion_result(&completion))
            } else {
                Ok(format!(
                    "No retained task status for '{task_id}' in this session."
                ))
            }
        }
        "logs" => {
            let task_id = single_task_id(args, "logs")?;
            let task_id = task_id.as_str();
            let path = tasks
                .iter()
                .find(|task| task.id.as_str() == task_id)
                .and_then(|task| task.output_log.clone())
                .or_else(|| {
                    manager
                        .completion(&session_id, task_id)
                        .and_then(|task| task.output_log)
                });
            let Some(path) = path else {
                return Ok(format!(
                    "No captured task log for '{task_id}' in this session."
                ));
            };
            let full = args.get("full").and_then(parse_json_bool).unwrap_or(false);
            let limit = args
                .get("tail_bytes")
                .and_then(|value| value.as_u64())
                .unwrap_or(16_384)
                .clamp(1, 65_536) as usize;
            let (excerpt, start, total) = read_task_log(&path, full, limit)?;
            let prefix = if start > 0 {
                format!(
                    "Task '{}' log tail ({} of {} bytes):\n",
                    task_id,
                    total - start,
                    total
                )
            } else {
                format!("Task '{}' log ({} bytes):\n", task_id, total)
            };
            let capture_note = manager
                .completion(&session_id, task_id)
                .and_then(|completion| completion.log_error)
                .map_or_else(String::new, |error| {
                    format!("\n[log capture was incomplete: {error}]")
                });
            Ok(format!("{prefix}{excerpt}{capture_note}"))
        }
        "kill" => {
            let task_id = single_task_id(args, "kill")?;

            cancel_result_message(&task_id, manager.cancel_in_session(&session_id, &task_id))
        }
        "wait" => {
            let task_ids = requested_task_ids(args);
            if task_ids.is_empty() {
                return Err("missing 'task_id' or 'task_ids' argument for wait action".to_owned());
            }
            Ok(wait_for_background_tasks(
                manager,
                &session_id,
                &task_ids,
                wait_timeout(args),
            ))
        }
        _ => Err(format!(
            "Unknown action '{action}'. Supported actions: list, status, logs, kill, wait."
        )),
    }
}

fn cancel_result_message(task_id: &str, result: CancelResult) -> Result<String, String> {
    match result {
        CancelResult::Cancelled => Ok(format!("Task '{task_id}' terminated successfully.")),
        CancelResult::Requested => Ok(format!(
            "Task '{task_id}' termination requested; it will be cancelled when its process starts."
        )),
        CancelResult::Failed => Err(format!("Failed to terminate task '{task_id}'.")),
        CancelResult::AlreadyFinished | CancelResult::NotFound => {
            Err(format!("Task '{task_id}' not found."))
        }
    }
}

fn task_pid(state: TaskState) -> Option<u32> {
    match state {
        TaskState::Running { pid } => Some(pid),
        TaskState::Starting | TaskState::Terminating { .. } | TaskState::CancelRequested => None,
    }
}

fn terminate_background_pid(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // Background commands get their own process group at spawn time, so a
        // negative PID terminates the shell and every descendant holding its
        // stdout/stderr pipes.
        let Ok(process_group) = i32::try_from(pid) else {
            return false;
        };
        if process_group <= 0 {
            return false;
        }
        return unsafe { libc::kill(-process_group, libc::SIGKILL) == 0 };
    }
    #[cfg(target_os = "windows")]
    {
        return std::process::Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .status()
            .is_ok_and(|status| status.success());
    }
    #[allow(unreachable_code)]
    false
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BackgroundStopResult {
    pub stopped: usize,
    pub requested: usize,
    pub failed: usize,
}

pub fn stop_background_tasks(session_id: &str) -> BackgroundStopResult {
    let summary = background_task_manager().cancel_session(session_id);
    BackgroundStopResult {
        stopped: summary.cancelled,
        requested: summary.requested,
        failed: summary.failed,
    }
}

#[cfg(test)]
mod tests {
    use super::sandbox;
    #[cfg(unix)]
    use super::terminate_background_pid;
    use super::{
        cancel_result_message, command_confirmation_preview, command_confirmation_scope,
        command_manages_own_background_jobs, command_requires_confirmation, has_interactive_sudo,
        has_shell_background_operator, manage_task_tool, pull_request_base, reject_broad_git_stage,
        run_command, run_command_output, run_command_output_cancellable,
        run_command_output_with_progress, task_event_to_tool_output, wait_for_background_tasks,
    };

    #[test]
    fn run_command_description_matches_the_shell_of_each_platform() {
        use super::{RUN_COMMAND, run_command_description};

        let posix = run_command_description(false);
        assert!(posix.starts_with("Run one shell command; returns stdout, stderr and exit code."));
        assert!(posix.contains("`git worktree add` under /tmp only"));
        assert!(!posix.contains("PowerShell"));
        #[cfg(not(target_os = "windows"))]
        assert_eq!(RUN_COMMAND.description, posix);

        let windows = run_command_description(true);
        assert!(windows.starts_with("Run one PowerShell command;"));
        for fact in [
            "PowerShell 7 (pwsh)",
            "Windows PowerShell 5.1",
            "chain with `;`",
            "no heredocs",
            "$env:NAME = 'value'",
            "`curl.exe`",
            "view_file, grep, glob and list_directory",
            "under `$env:TEMP`",
        ] {
            assert!(windows.contains(fact), "missing {fact:?}");
        }
        // Nothing POSIX-only may leak into the Windows guidance.
        for posix_only in ["/tmp", "Pipelines fail", "containing '&'", "cat/sed/head"] {
            assert!(!windows.contains(posix_only), "leaked {posix_only:?}");
        }
        assert_eq!(RUN_COMMAND.name, "run_command");
    }

    #[cfg(unix)]
    #[test]
    fn background_termination_kills_the_command_process_group() {
        use std::os::unix::process::CommandExt;

        let mut command = std::process::Command::new("sh");
        command
            .args(["-c", "sleep 30 & wait"])
            .process_group(0)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        let mut child = command.spawn().expect("spawn process group");
        std::thread::sleep(std::time::Duration::from_millis(50));

        assert!(terminate_background_pid(child.id()));
        let status = child.wait().expect("reap terminated shell");
        assert!(!status.success());
    }

    fn task_request(
        command: &str,
        cwd: Option<std::path::PathBuf>,
    ) -> rustcode_command::CommandRequest {
        rustcode_command::CommandRequest {
            command: command.to_owned(),
            status_command: None,
            sandboxed_shell: false,
            cwd,
            env: Vec::new(),
            timeout: std::time::Duration::from_secs(5),
            process_group: true,
            inherited_fds: Vec::new(),
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn explicit_workspace_runs_without_thread_local_context() {
        let workspace = tempfile::tempdir().expect("temporary workspace");
        let result = super::run_command_output_with_workspace(
            &serde_json::json!({"command": "pwd"}),
            Some(workspace.path().to_path_buf()),
        )
        .expect("explicit workspace permits sandbox construction");

        assert!(result.success, "{}", result.content);
        assert!(
            result
                .content
                .contains(&workspace.path().display().to_string()),
            "command did not run in the explicit workspace: {}",
            result.content
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn explicit_missing_workspace_fails_closed() {
        super::super::set_active_workspace_context(
            None,
            None,
            false,
            Some(crate::config::SandboxMode::WorkspaceWrite),
        );
        let result =
            super::run_command_output_with_workspace(&serde_json::json!({"command": "pwd"}), None);

        super::super::set_active_workspace_context(None, None, false, None);
        assert!(
            result
                .expect_err("explicitly missing workspace must fail closed")
                .contains("needs an active workspace")
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn explicit_deleted_workspace_fails_closed() {
        let workspace = tempfile::tempdir().expect("temporary workspace");
        let path = workspace.path().to_path_buf();
        drop(workspace);

        let error = super::run_command_output_with_workspace(
            &serde_json::json!({"command": "pwd"}),
            Some(path),
        )
        .expect_err("deleted workspace must fail closed");

        assert!(
            error.contains("cwd") && error.contains("not a directory"),
            "{error}"
        );
    }

    /// The originating session's core complaint: under a restricted mode `gh`
    /// could not reach the API and the failure was reported as a bare
    /// `error connecting to api.github.com`. Run both modes against the same
    /// command so the difference is the mode, not the host (#1540).
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn trusted_mode_reaches_the_network_and_writes_outside_the_workspace() {
        if !sandbox::runtime_tests_available() {
            eprintln!("skipping trusted-mode runtime assertion: sandbox probe unavailable");
            return;
        }
        if std::process::Command::new("python3")
            .arg("-c")
            .arg("pass")
            .status()
            .is_err()
        {
            eprintln!("skipping trusted-mode runtime assertion: python3 is unavailable");
            return;
        }
        let workspace = tempfile::tempdir().expect("temporary workspace");
        let outside = tempfile::tempdir().expect("temporary outside directory");
        let outside_probe = outside.path().join("probe.txt");
        let (echo_tx, echoed) = std::sync::mpsc::channel();
        let listener =
            std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind loopback listener");
        let port = listener.local_addr().expect("listener address").port();
        // Nonblocking accept with a deadline: a denied connect must fail the
        // assertion, never hang the suite.
        listener
            .set_nonblocking(true)
            .expect("set the listener nonblocking");
        let accept = std::thread::spawn(move || {
            use std::io::{Read, Write};
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        if std::time::Instant::now() >= deadline {
                            let _ = echo_tx.send(Err("no connection within 30s".to_string()));
                            return;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(20));
                    }
                    Err(error) => {
                        let _ = echo_tx.send(Err(error.to_string()));
                        return;
                    }
                }
            };
            let mut buffer = [0_u8; 8];
            match stream.read(&mut buffer) {
                Ok(read) => {
                    let _ = stream.write_all(&buffer[..read]);
                    let _ = echo_tx.send(Ok(String::from_utf8_lossy(&buffer[..read]).into_owned()));
                }
                Err(error) => {
                    let _ = echo_tx.send(Err(error.to_string()));
                }
            }
        });
        let command = format!(
            "printf trusted > '{}' && printf ping | python3 -c {script}",
            outside_probe.display(),
            script = format!(
                "'{script}'",
                script = format!(
                    "import socket,sys\ns=socket.create_connection((\"127.0.0.1\",{port}),timeout=10)\ns.sendall(sys.stdin.buffer.read())\nsys.stdout.write(s.recv(4).decode())\n"
                )
                .replace('\'', "'\\''")
            )
        );
        let args = serde_json::json!({"command": command, "timeout_ms": 45_000});

        super::super::set_active_workspace_context(
            Some(workspace.path().to_path_buf()),
            Some(workspace.path().to_path_buf()),
            false,
            Some(crate::config::SandboxMode::Trusted),
        );
        let trusted = run_command_output(&args).expect("trusted command should run");
        let echo_result = echoed
            .recv_timeout(std::time::Duration::from_secs(35))
            .expect("loopback echo must complete");
        accept.join().expect("loopback echo thread");
        super::super::set_active_workspace_context(
            Some(workspace.path().to_path_buf()),
            Some(workspace.path().to_path_buf()),
            false,
            Some(crate::config::SandboxMode::WorkspaceWrite),
        );
        let restricted = run_command_output(&args).expect("restricted command should run");
        super::super::set_active_workspace_context(None, None, false, None);

        assert!(
            trusted.success,
            "trusted mode must reach the network and write outside the workspace: {}",
            trusted.content
        );
        assert_eq!(echo_result.expect("loopback echo"), "ping");
        assert_eq!(
            std::fs::read_to_string(&outside_probe).expect("trusted write"),
            "trusted"
        );

        assert!(
            !restricted.success,
            "the default mode must still deny this: {}",
            restricted.content
        );
        assert!(
            restricted.content.contains("[harness:"),
            "a restricted failure must name the sandbox: {}",
            restricted.content
        );
        let _ = std::fs::remove_file(&outside_probe);
    }

    #[test]
    fn restricted_failures_name_the_sandbox_and_trusted_failures_do_not() {
        let workspace = tempfile::tempdir().expect("temporary workspace");
        let outside = tempfile::tempdir().expect("temporary outside directory");
        let outside_probe = outside.path().join("probe.txt");
        let command = format!("printf x > '{}'", outside_probe.display());

        for (mode, expect_attribution) in [
            (crate::config::SandboxMode::ReadOnly, true),
            (crate::config::SandboxMode::WorkspaceWrite, true),
            (crate::config::SandboxMode::WorkspaceWriteNetwork, true),
            (crate::config::SandboxMode::Trusted, false),
        ] {
            super::super::set_active_workspace_context(
                Some(workspace.path().to_path_buf()),
                Some(workspace.path().to_path_buf()),
                false,
                Some(mode),
            );
            let result = super::run_command_output_with_workspace(
                &serde_json::json!({"command": command}),
                Some(workspace.path().to_path_buf()),
            )
            .expect("command should run");
            super::super::set_active_workspace_context(None, None, false, None);

            if expect_attribution && cfg!(any(target_os = "linux", target_os = "macos")) {
                assert!(
                    result
                        .content
                        .contains(&format!("OS sandbox mode {}", mode.as_str())),
                    "{mode} must name itself: {}",
                    result.content
                );
            } else {
                assert!(
                    !result.content.contains("[harness:"),
                    "trusted mode is unwrapped, so it must not blame a sandbox: {}",
                    result.content
                );
            }
        }
    }

    #[test]
    fn effective_writable_roots_mirror_the_launcher_policy() {
        let workspace = std::path::PathBuf::from("/workspace");
        let scratch = std::path::PathBuf::from("/scratch");
        let one_shot = std::path::PathBuf::from("/one-shot");
        let roots = vec![workspace.clone(), scratch.clone()];

        assert_eq!(
            super::effective_writable_roots(&roots, &[one_shot.clone()], true),
            vec![workspace.clone(), scratch.clone(), one_shot.clone()]
        );
        assert_eq!(
            super::effective_writable_roots(&roots, &[one_shot.clone()], false),
            vec![one_shot.clone()],
            "a read-only mode still permits approved one-shot roots"
        );
        assert_eq!(
            super::effective_writable_roots(&roots, &[workspace.clone()], true),
            roots,
            "a one-shot grant inside an existing root is not listed twice"
        );
    }

    /// Trusted removes OS sandbox wrapping only. The approval decision is
    /// computed before the sandbox mode is read — `authorize_tool_with_args`
    /// takes no mode argument at all — so the decision, the scope shown to the
    /// user, and the one-shot escalation rule must be identical in every mode
    /// (#1540).
    #[test]
    fn trusted_mode_does_not_bypass_the_shell_approval_policy() {
        use crate::config::{AgentMode, SandboxMode};
        use crate::tools::AuthorizationDecision;

        for command in [
            "git push --force origin main",
            "cargo publish",
            "gh pr merge 1531 --squash",
            "npm install",
        ] {
            let args = serde_json::json!({"command": command});
            assert!(command_requires_confirmation(&args), "{command}");
            assert_eq!(
                super::super::authorize_tool_with_args(
                    "run_command",
                    &args,
                    AgentMode::Build,
                    false,
                    false,
                ),
                AuthorizationDecision::RequireConfirmation,
                "{command} must still need approval in trusted mode"
            );
            // The card the user approves is the same shape in every mode and
            // still states the trusted permissions explicitly.
            let trusted_preview =
                command_confirmation_preview(command, SandboxMode::Trusted, false, None);
            let write_preview =
                command_confirmation_preview(command, SandboxMode::WorkspaceWrite, false, None);
            assert!(trusted_preview.contains("scope:"), "{trusted_preview}");
            assert!(
                trusted_preview.contains("effective OS permissions: trusted process permissions"),
                "{trusted_preview}"
            );
            assert_eq!(
                trusted_preview
                    .lines()
                    .filter(|line| !line.starts_with("effective OS permissions"))
                    .collect::<Vec<_>>(),
                write_preview
                    .lines()
                    .filter(|line| !line.starts_with("effective OS permissions"))
                    .collect::<Vec<_>>(),
                "{command} must be approved on the same terms in trusted mode"
            );
        }

        // YOLO also approves one-shot permission requests.
        for args in [
            serde_json::json!({"command": "gh pr list", "network_access": true}),
            serde_json::json!({"command": "cargo fmt", "filesystem_write_path": "/tmp/other"}),
        ] {
            assert_eq!(
                super::super::authorize_tool_with_args(
                    "run_command",
                    &args,
                    AgentMode::Build,
                    true,
                    false,
                ),
                AuthorizationDecision::Allow,
                "{args} must be auto-approved in YOLO"
            );
        }
    }

    #[test]
    fn task_manager_root_adapter_keeps_sessions_isolated() {
        let manager = rustcode_tasks::TaskManager::new(std::sync::Arc::new(|_| true));
        let session_a = manager.subscribe_session("root-session-a");
        let session_b = manager.subscribe_session("root-session-b");
        let hold_open = if cfg!(target_os = "windows") {
            "Start-Sleep -Seconds 1"
        } else {
            "sleep 1"
        };
        let first = manager
            .spawn_with_id(
                "root-session-task-a",
                rustcode_tasks::TaskSpec::new("root-session-a", task_request(hold_open, None)),
            )
            .unwrap();
        let second = manager
            .spawn_with_id(
                "root-session-task-b",
                rustcode_tasks::TaskSpec::new("root-session-b", task_request(hold_open, None)),
            )
            .unwrap();

        assert_eq!(manager.list("root-session-a").len(), 1);
        assert_eq!(manager.list("root-session-a")[0].id, *first.id());
        assert_eq!(manager.list("root-session-b").len(), 1);
        assert_eq!(manager.list("root-session-b")[0].id, *second.id());
        let first_event = session_a.recv().unwrap();
        assert_eq!(first_event.session_id().as_str(), "root-session-a");
        assert_eq!(
            session_b.recv().unwrap().session_id().as_str(),
            "root-session-b"
        );
    }

    #[cfg(unix)]
    #[test]
    fn wait_any_returns_first_completion_without_consuming_or_cancelling_others() {
        let manager =
            rustcode_tasks::TaskManager::new(std::sync::Arc::new(super::RootProcessTerminator));
        let session = "wait-any-session";
        let fast = manager
            .spawn_with_id(
                "wait-any-fast",
                rustcode_tasks::TaskSpec::new(session, task_request("sleep 0.2", None)),
            )
            .unwrap();
        let slow = manager
            .spawn_with_id(
                "wait-any-slow",
                rustcode_tasks::TaskSpec::new(session, task_request("sleep 30", None)),
            )
            .unwrap();

        let result = wait_for_background_tasks(
            &manager,
            session,
            &[fast.id().to_string(), slow.id().to_string()],
            std::time::Duration::from_secs(5),
        );

        assert!(result.contains("wait-any-fast"), "{result}");
        assert!(
            manager
                .list(session)
                .iter()
                .any(|task| task.id == *slow.id())
        );
        assert_eq!(
            manager.cancel(slow.id()),
            rustcode_tasks::CancelResult::Cancelled
        );
    }

    #[test]
    fn task_manager_root_adapter_cancels_before_pid_without_duplicate_terminal() {
        let manager = rustcode_tasks::TaskManager::new(std::sync::Arc::new(|_| true));
        let events = manager.subscribe();
        let task = manager
            .spawn_with_id(
                "root-cancel-before-pid",
                rustcode_tasks::TaskSpec::new(
                    "root-cancel-session",
                    task_request(
                        "printf never-starts",
                        Some(std::path::PathBuf::from("/path/that/does/not/exist")),
                    ),
                ),
            )
            .unwrap();
        let result = manager.cancel(task.id());
        assert!(matches!(
            result,
            rustcode_tasks::CancelResult::Requested | rustcode_tasks::CancelResult::Cancelled
        ));

        let mut terminal_events = 0;
        while let Ok(event) = events.recv() {
            if event.is_terminal() {
                terminal_events += 1;
                assert_eq!(event.task_id(), task.id());
                break;
            }
        }
        assert_eq!(terminal_events, 1);
        assert!(manager.list("root-cancel-session").is_empty());
    }

    #[test]
    fn task_manager_root_adapter_converts_one_completion_once() {
        let manager = rustcode_tasks::TaskManager::new(std::sync::Arc::new(|_| true));
        let events = manager.subscribe();
        let task = manager
            .spawn_with_id(
                "root-completion-once",
                rustcode_tasks::TaskSpec::new(
                    "root-completion-session",
                    task_request(
                        if cfg!(target_os = "windows") {
                            "echo done"
                        } else {
                            "printf done"
                        },
                        None,
                    ),
                ),
            )
            .unwrap();
        let mut terminal_events = Vec::new();
        while let Ok(event) = events.recv() {
            if event.is_terminal() {
                terminal_events.push(event);
                break;
            }
        }
        assert_eq!(terminal_events.len(), 1);
        let (task_id, session_id, converted) =
            task_event_to_tool_output(terminal_events.pop().unwrap()).expect("finished output");
        assert_eq!(task_id, task.id().as_str());
        assert_eq!(session_id, "root-completion-session");
        assert!(converted.success);
        assert_eq!(converted.command.as_deref(), Some("printf done"));
        assert!(converted.content.contains("done"));
    }

    #[test]
    fn task_event_root_adapter_preserves_cancelled_metadata() {
        let event = rustcode_tasks::TaskEvent::Cancelled {
            id: rustcode_tasks::TaskId::new("cancelled-task"),
            session_id: rustcode_tasks::SessionId::new("cancelled-session"),
            call_id: None,
            command: "cargo test".to_string(),
            notify_on_complete: true,
        };

        let (task_id, session_id, output) = task_event_to_tool_output(event).expect("cancelled");
        assert_eq!(task_id, "cancelled-task");
        assert_eq!(session_id, "cancelled-session");
        assert!(!output.success);
        assert_eq!(output.command.as_deref(), Some("cargo test"));
        assert_eq!(
            output.error_kind,
            Some(crate::tools::ToolErrorKind::Cancelled)
        );
        assert!(
            output.content.contains("Background task cancelled"),
            "{}",
            output.content
        );
        assert!(output.content.contains("no process exit code"));
    }

    #[test]
    fn manage_task_kill_cannot_cross_session_boundaries() {
        let owner = "manage-owner-session";
        let other = "manage-other-session";
        let task_id = "manage-owned-task";
        crate::tools::set_active_session_id(Some(owner.to_owned()));
        crate::tools::spawn_background_task_for_test(
            task_id,
            owner,
            if cfg!(target_os = "windows") {
                "Start-Sleep -Seconds 30"
            } else {
                "sleep 30"
            },
        )
        .unwrap();
        for _ in 0..100 {
            if crate::tools::background_task_snapshots(owner)
                .first()
                .is_some_and(|task| task.child_pid.is_some())
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        crate::tools::set_active_session_id(Some(other.to_owned()));
        let result = manage_task_tool(&serde_json::json!({
            "action": "kill",
            "task_id": task_id,
        }));
        assert_eq!(result, Err(format!("Task '{task_id}' not found.")));
        assert_eq!(crate::tools::background_task_snapshots(owner).len(), 1);

        crate::tools::set_active_session_id(Some(owner.to_owned()));
        crate::tools::stop_background_tasks(owner);
        crate::tools::set_active_session_id(None);
    }

    #[test]
    fn manage_task_wait_unknown_task_reports_not_running() {
        let session = "manage-wait-unknown-session";
        crate::tools::set_active_session_id(Some(session.to_owned()));
        let result = manage_task_tool(&serde_json::json!({
            "action": "wait",
            "task_id": "no-such-task",
            "timeout_ms": 1000,
        }));
        assert_eq!(
            result,
            Ok("None of the requested tasks are running or retained: no-such-task".to_string())
        );
        crate::tools::set_active_session_id(None);
    }

    #[test]
    fn manage_task_wait_returns_finished_output() {
        let session = "manage-wait-done-session";
        let task_id = "manage-wait-done-task";
        crate::tools::set_active_session_id(Some(session.to_owned()));
        crate::tools::spawn_background_task_for_test(
            task_id,
            session,
            if cfg!(target_os = "windows") {
                "Start-Sleep -Seconds 2"
            } else {
                "sleep 2"
            },
        )
        .unwrap();
        let result = manage_task_tool(&serde_json::json!({
            "action": "wait",
            "task_id": task_id,
            "timeout_ms": 30000,
        }));
        let output = result.expect("wait should return the finished result");
        assert!(
            output.contains("exited successfully (code 0)"),
            "unexpected wait output: {output}"
        );
        crate::tools::stop_background_tasks(session);
        crate::tools::set_active_session_id(None);
    }

    #[test]
    fn manage_task_wait_timeout_reports_still_running() {
        let session = "manage-wait-timeout-session";
        let task_id = "manage-wait-timeout-task";
        crate::tools::set_active_session_id(Some(session.to_owned()));
        crate::tools::spawn_background_task_for_test(
            task_id,
            session,
            if cfg!(target_os = "windows") {
                "Start-Sleep -Seconds 30"
            } else {
                "sleep 30"
            },
        )
        .unwrap();
        let result = manage_task_tool(&serde_json::json!({
            "action": "wait",
            "task_id": task_id,
            "timeout_ms": 1000,
        }));
        let output = result.expect("wait should report still running");
        assert!(
            output.contains("still running"),
            "unexpected wait output: {output}"
        );
        crate::tools::stop_background_tasks(session);
        crate::tools::set_active_session_id(None);
    }

    #[test]
    fn manage_task_requested_kill_explains_asynchronous_termination() {
        assert_eq!(
            cancel_result_message(
                "task-starting",
                rustcode_tasks::CancelResult::Requested
            ),
            Ok("Task 'task-starting' termination requested; it will be cancelled when its process starts.".to_string())
        );
    }

    #[test]
    fn run_command_reports_stdout_and_stderr_while_running() {
        if !sandbox::runtime_tests_available() {
            return;
        }
        let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured = events.clone();
        let callback: super::CommandProgressCallback = std::sync::Arc::new(move |bytes, stderr| {
            captured
                .lock()
                .unwrap()
                .push((String::from_utf8_lossy(bytes).into_owned(), stderr));
        });
        let output = run_command_output_with_progress(
            &serde_json::json!({"command": "printf out; printf err >&2"}),
            callback,
        )
        .expect("command output");

        assert!(output.success);
        let events = events.lock().unwrap();
        assert!(
            events
                .iter()
                .any(|(text, stderr)| !stderr && text.contains("out"))
        );
        assert!(
            events
                .iter()
                .any(|(text, stderr)| *stderr && text.contains("err"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn cancellable_run_command_returns_one_cancelled_result() {
        if !sandbox::runtime_tests_available() {
            return;
        }
        let token = tokio_util::sync::CancellationToken::new();
        let trigger = token.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(30));
            trigger.cancel();
        });
        let output = run_command_output_cancellable(
            &serde_json::json!({"command": "sleep 0.3"}),
            Some(token),
        )
        .expect("cancellation should be represented as a tool result");

        assert!(!output.success);
        assert_eq!(
            output.error_kind,
            Some(super::super::ToolErrorKind::Cancelled)
        );
        assert_eq!(output.content, "error: tool execution cancelled by user");
    }

    #[test]
    fn broad_git_staging_is_rejected() {
        for command in [
            "git add .",
            "git add -A",
            "git add --all",
            "git add -- .",
            "git commit -a -m feature",
        ] {
            assert!(
                reject_broad_git_stage(command).is_some(),
                "expected broad staging to be rejected: {command}"
            );
        }
        assert!(reject_broad_git_stage("git add src/network.rs").is_none());
    }

    #[test]
    fn pull_request_base_is_parsed_only_for_create_commands() {
        assert_eq!(
            pull_request_base("gh pr create --base main --title change"),
            Some("main".to_string())
        );
        assert_eq!(
            pull_request_base("gh pr create --base=release"),
            Some("release".to_string())
        );
        assert_eq!(pull_request_base("gh pr list --base main"), None);
    }

    #[test]
    fn pr_creation_is_guarded_when_origin_has_no_base() {
        let root = tempfile::tempdir().expect("temporary repository");
        let status = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(root.path())
            .status()
            .expect("git init");
        assert!(status.success());

        assert!(super::remote_base_missing(Some(root.path()), "main"));
        let output = super::pr_creation_guard_output("gh pr create --base main", "main");
        assert!(!output.success);
        assert!(!output.retryable);
        assert!(output.content.contains("was not run"));
    }

    #[test]
    fn run_command_executes_chained_shell_commands() {
        if !sandbox::runtime_tests_available() {
            return;
        }
        let result = run_command(&serde_json::json!({
            "command": "printf one; printf two"
        }))
        .expect("shell command should succeed");

        assert!(result.contains("exit code: 0"));
        assert!(result.contains("onetwo"));
    }

    #[test]
    fn run_command_supports_conditional_chaining() {
        if !sandbox::runtime_tests_available() {
            return;
        }
        let result = run_command(&serde_json::json!({
            "command": "printf first && printf second"
        }))
        .expect("shell command should succeed");

        assert!(result.contains("firstsecond"));
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn run_command_propagates_pipeline_failures() {
        if !sandbox::runtime_tests_available() {
            return;
        }
        let result = run_command(&serde_json::json!({
            "command": "false | tail -n 1"
        }))
        .expect("run_command reports command failure in its output");

        assert!(result.contains("exit code: 1"), "{result}");
    }

    #[test]
    fn command_execution_metadata_classifies_nonzero_exit_only() {
        if !sandbox::runtime_tests_available() {
            return;
        }
        let failed = run_command_output(&serde_json::json!({"command": "false"}))
            .expect("false should return a structured command result");
        assert!(!failed.success);
        assert_eq!(
            failed.error_kind,
            Some(super::super::ToolErrorKind::CommandFailed)
        );

        let passed = run_command_output(&serde_json::json!({"command": "true"}))
            .expect("true should return a structured command result");
        assert!(passed.success);
        assert_eq!(passed.error_kind, None);
    }

    #[cfg(unix)]
    #[test]
    fn command_result_envelope_marks_sigpipe_as_downstream_completion() {
        if !sandbox::runtime_tests_available() {
            return;
        }
        let output = run_command_output(&serde_json::json!({
            "command": "yes | head -n 1"
        }))
        .expect("pipeline should return a structured command result");

        assert!(output.success, "{}", output.content);
        let status = output.command_status.expect("command status metadata");
        assert!(status.completed);
        assert_eq!(status.exit_code, Some(141));
        assert_eq!(status.signal, Some(libc::SIGPIPE));
        assert!(status.downstream_consumer_terminated);
        assert!(output.content.contains("SIGPIPE"));
        assert!(
            output
                .content
                .contains("output_truncated_by_rustcode=false")
        );
    }

    #[test]
    fn command_result_envelope_marks_a_successful_completion() {
        if !sandbox::runtime_tests_available() {
            return;
        }
        let output = run_command_output(&serde_json::json!({
            "command": "printf complete"
        }))
        .expect("command should return a structured result");

        let status = output.command_status.expect("command status metadata");
        assert!(output.success);
        assert!(status.completed);
        assert_eq!(status.exit_code, Some(0));
        assert_eq!(status.bytes_returned, 8);
        assert_eq!(status.total_output_bytes, Some(8));
        assert!(!status.output_truncated);
    }

    #[test]
    fn background_command_start_is_pending_and_names_command() {
        if !sandbox::runtime_tests_available() {
            return;
        }
        let command = "sleep 3; printf background-output";
        let output = run_command_output(&serde_json::json!({
            "command": command,
            "background": true,
        }))
        .expect("background command should be accepted");

        assert!(!output.success, "starting is not completed success");
        assert_eq!(output.exit_code, None, "the process has not exited yet");
        assert!(
            output.content.contains("Status: Pending"),
            "{}",
            output.content
        );
        assert!(output.content.contains(command), "{}", output.content);
    }

    #[cfg(unix)]
    #[test]
    fn shell_background_operator_detection_ignores_quotes_redirections_and_and() {
        assert!(has_shell_background_operator(
            "python3 -m http.server 8080 &"
        ));
        assert!(has_shell_background_operator("server & echo ready"));
        for command in [
            "printf '&'",
            "printf \"&\"",
            "printf escaped\\&",
            "printf one && printf two",
            "printf out 2>&1",
            "printf out &>file",
        ] {
            assert!(
                !has_shell_background_operator(command),
                "not a background operator: {command}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn detached_server_start_is_completed_but_remains_tracked_and_killable() {
        if !sandbox::runtime_tests_available() {
            return;
        }
        let session_id = format!(
            "detached-server-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        super::super::set_active_session_id(Some(session_id.clone()));
        let output = run_command_output(&serde_json::json!({
            "command": "sleep 30",
            "background": true,
            "detached": true,
        }))
        .expect("detached server should start");
        let snapshots = super::super::background_task_snapshots(&session_id);
        let stop = super::super::stop_background_tasks(&session_id);
        super::super::set_active_session_id(None);

        assert!(output.success);
        assert!(!output.pending);
        assert_eq!(output.exit_code, None);
        assert!(output.content.contains("Detached task started"));
        assert!(output.content.contains("Task ID:"));
        assert_eq!(snapshots.len(), 1, "detached task was not retained");
        assert!(
            snapshots[0].child_pid.is_some(),
            "detached task never started"
        );
        assert_eq!(stop.stopped, 1, "detached task was not terminated");
    }

    #[cfg(unix)]
    #[test]
    fn self_managed_background_scripts_are_exempt_from_auto_detach() {
        // Shape of the frozen session's lost verification: start a server,
        // probe it, then tear it down in one compound command. The embedded
        // `&` is real, but the script reaps its own job, so its output must
        // be preserved instead of detached into the void.
        let verify = "bun run dev > /tmp/rc-dev.log 2>&1 &\nDEV_PID=$!\nfor i in $(seq 1 30); do\n  if grep -q \"Ready\" /tmp/rc-dev.log 2>/dev/null; then break; fi\n  sleep 1\ndone\ncurl -s \"http://localhost:3000/api/releases\" | head -c 100\necho \"\"\nkill $DEV_PID 2>/dev/null\npkill -f \"next dev\" 2>/dev/null\necho \"done\"";
        assert!(has_shell_background_operator(verify));
        assert!(command_manages_own_background_jobs(verify));
        for command in [
            "sleep 0.1 & pid=$!; kill $pid; echo reaped",
            "sleep 0.1 & wait $!; echo done",
            "server & client; wait; echo done",
        ] {
            assert!(
                command_manages_own_background_jobs(command),
                "self-managed: {command}"
            );
        }
        for command in [
            "sleep 30 &",
            "server & echo ready",
            "printf 'wait $!' &",
            "echo \"wait\"",
            "printf out 2>&1",
        ] {
            assert!(
                !command_manages_own_background_jobs(command),
                "not self-managed: {command}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn self_managed_background_script_runs_in_foreground_with_output() {
        if !sandbox::runtime_tests_available() {
            return;
        }
        let session_id = format!(
            "self-managed-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        super::super::set_active_session_id(Some(session_id.clone()));
        let output = run_command_output(&serde_json::json!({
            "command": "sleep 0.2 & pid=$!; wait $pid; echo verified-$pid",
        }))
        .expect("self-managed script should run in the foreground");
        super::super::set_active_session_id(None);

        assert!(
            !output.content.contains("Detached task started"),
            "self-managed script must not detach: {}",
            output.content
        );
        assert_eq!(output.exit_code, Some(0));
        assert!(
            output.content.contains("verified-"),
            "verification output must be preserved: {}",
            output.content
        );
    }

    #[cfg(unix)]
    #[test]
    fn shell_background_operator_is_auto_detached_without_background_flag() {
        if !sandbox::runtime_tests_available() {
            return;
        }
        let session_id = format!(
            "nested-background-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        super::super::set_active_session_id(Some(session_id.clone()));
        let output = run_command_output(&serde_json::json!({
            "command": "sleep 30 &",
        }))
        .expect("shell background command should start detached");
        // The pid is recorded by the task's own thread. A second was not
        // always enough under a full parallel test run, and a task without a
        // pid cannot be stopped.
        for _ in 0..1000 {
            if super::super::background_task_snapshots(&session_id)
                .first()
                .is_some_and(|task| task.child_pid.is_some())
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let snapshots = super::super::background_task_snapshots(&session_id);
        let stop = super::super::stop_background_tasks(&session_id);
        super::super::set_active_session_id(None);

        assert!(output.success);
        assert!(!output.pending);
        assert!(output.content.contains("Detached task started"));
        assert_eq!(
            snapshots.len(),
            1,
            "implicit detached task was not retained"
        );
        // A task whose process is still starting is cancelled by request and
        // reported as `requested`, not `stopped`. Which of the two the stop
        // lands on is a matter of timing; that it lands is what is tested.
        assert_eq!(
            stop.stopped + stop.requested,
            1,
            "implicit detached task was not terminated: output={:?} snapshots={snapshots:?}",
            output.content
        );
        assert_eq!(stop.failed, 0);
    }

    #[test]
    fn short_discovery_commands_ignore_background_request() {
        if !sandbox::runtime_tests_available() {
            return;
        }
        for command in [
            "printf synchronous-output",
            "pwd",
            "test -e Cargo.toml",
            "ls src",
            "stat Cargo.toml",
            "find src -name exec.rs -type f",
        ] {
            let output = run_command_output(&serde_json::json!({
                "command": command,
                "background": true,
            }))
            .expect("short discovery command should execute");

            assert!(!output.pending, "short command was backgrounded: {command}");
            assert!(output.exit_code.is_some(), "missing exit code: {command}");
            assert_eq!(output.command, None, "sync command metadata: {command}");
        }
    }

    #[test]
    fn background_request_is_preserved_for_long_or_mutating_commands() {
        if !sandbox::runtime_tests_available() {
            return;
        }
        let command = "sleep 3";
        let output = run_command_output(&serde_json::json!({
            "command": command,
            "background": true,
        }))
        .expect("long command should be accepted");

        assert!(output.pending, "command was forced synchronous: {command}");
        assert_eq!(output.command.as_deref(), Some(command));
    }

    fn unique_session(prefix: &str) -> String {
        format!(
            "{prefix}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )
    }

    fn next_terminal_event(
        events: &rustcode_tasks::TaskSubscription,
        timeout: std::time::Duration,
    ) -> Option<rustcode_tasks::TaskEvent> {
        let deadline = std::time::Instant::now() + timeout;
        while let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) {
            match events.recv_timeout(remaining) {
                Ok(event) if event.is_terminal() => return Some(event),
                Ok(_) => continue,
                Err(_) => break,
            }
        }
        None
    }

    #[cfg(unix)]
    #[test]
    fn fast_background_command_returns_its_foreground_result_without_a_notification() {
        if !sandbox::runtime_tests_available() {
            return;
        }
        let session_id = unique_session("inline-background-test");
        super::super::set_active_session_id(Some(session_id.clone()));
        let events = super::background_task_manager().subscribe_session(session_id.clone());
        let command = "sleep 0.2; printf inline-output; printf inline-error >&2; exit 3";
        let foreground = run_command_output(&serde_json::json!({ "command": command }))
            .expect("foreground command should run");
        let output = run_command_output(&serde_json::json!({
            "command": command,
            "background": true,
            "notify_on_complete": true,
        }))
        .expect("background command should run");
        let snapshots = super::super::background_task_snapshots(&session_id);
        let completions = super::super::recent_background_task_completions(&session_id);
        let late_event = next_terminal_event(&events, std::time::Duration::from_millis(400));
        super::super::set_active_session_id(None);

        assert!(
            output.content.contains("inline-output"),
            "{}",
            output.content
        );
        assert!(
            output.content.contains("inline-error"),
            "{}",
            output.content
        );
        assert!(!output.content.contains("Task ID"), "{}", output.content);
        assert_eq!(output.content, foreground.content);
        assert_eq!(output.exit_code, Some(3));
        assert_eq!(output.success, foreground.success);
        assert_eq!(output.error_kind, foreground.error_kind);
        assert_eq!(output.command, foreground.command);
        assert!(!output.pending);
        assert!(snapshots.is_empty(), "task still listed: {snapshots:?}");
        assert!(
            completions.is_empty(),
            "completion retained: {completions:?}"
        );
        assert!(late_event.is_none(), "notified anyway: {late_event:?}");
    }

    #[cfg(unix)]
    #[test]
    fn slow_background_command_still_returns_a_task_id_and_notifies() {
        if !sandbox::runtime_tests_available() {
            return;
        }
        let session_id = unique_session("slow-background-test");
        super::super::set_active_session_id(Some(session_id.clone()));
        let events = super::background_task_manager().subscribe_session(session_id.clone());
        let started = std::time::Instant::now();
        let output = run_command_output(&serde_json::json!({
            "command": "sleep 3; printf slow-output",
            "background": true,
        }))
        .expect("background command should start");
        let waited = started.elapsed();
        let event = next_terminal_event(&events, std::time::Duration::from_secs(20));
        super::super::set_active_session_id(None);

        assert!(output.pending);
        assert!(
            output
                .content
                .contains("Task started in background. Task ID: ")
        );
        assert!(
            waited >= super::BACKGROUND_INLINE_GRACE,
            "returned before the grace period: {waited:?}"
        );
        let event = event.expect("slow task should publish its completion");
        assert!(output.content.contains(event.task_id().as_str()));
        assert_eq!(event.notify_on_complete(), Some(true));
        let (_, _, result) = task_event_to_tool_output(event).expect("finished output");
        assert!(result.content.contains("slow-output"), "{}", result.content);
    }

    #[cfg(unix)]
    #[test]
    fn detached_command_returns_at_once_even_when_it_finishes_quickly() {
        if !sandbox::runtime_tests_available() {
            return;
        }
        let session_id = unique_session("detached-grace-test");
        super::super::set_active_session_id(Some(session_id.clone()));
        let events = super::background_task_manager().subscribe_session(session_id.clone());
        let started = std::time::Instant::now();
        let output = run_command_output(&serde_json::json!({
            "command": "sleep 0.5; printf detached-output",
            "detached": true,
            "notify_on_complete": true,
        }))
        .expect("detached command should start");
        let waited = started.elapsed();
        let event = next_terminal_event(&events, std::time::Duration::from_secs(20));
        super::super::set_active_session_id(None);

        assert!(output.content.contains("Detached task started"));
        assert!(
            waited < std::time::Duration::from_millis(450),
            "detached start waited for the command: {waited:?}"
        );
        let event = event.expect("detached task should still publish its completion");
        assert!(output.content.contains(event.task_id().as_str()));
    }

    #[test]
    fn task_id_arguments_merge_both_fields_and_ignore_empty_ones() {
        use super::{requested_task_ids, single_task_id};
        let ids = |args: serde_json::Value| requested_task_ids(&args);
        assert_eq!(
            ids(serde_json::json!({ "task_id": "a", "task_ids": [] })),
            ["a"]
        );
        assert_eq!(
            ids(serde_json::json!({ "task_ids": ["a", "b"] })),
            ["a", "b"]
        );
        assert_eq!(
            ids(serde_json::json!({ "task_id": "b", "task_ids": ["a", "b", "a", "", 7] })),
            ["b", "a"]
        );
        assert!(ids(serde_json::json!({ "task_id": " ", "task_ids": [""] })).is_empty());
        assert!(ids(serde_json::json!({ "task_id": null, "task_ids": null })).is_empty());

        assert_eq!(
            single_task_id(
                &serde_json::json!({ "task_id": "", "task_ids": ["a"] }),
                "logs"
            ),
            Ok("a".to_owned())
        );
        assert_eq!(
            single_task_id(
                &serde_json::json!({ "task_id": "a", "task_ids": ["a"] }),
                "kill"
            ),
            Ok("a".to_owned())
        );
        assert!(single_task_id(&serde_json::json!({ "task_ids": [] }), "status").is_err());
        assert!(single_task_id(&serde_json::json!({ "task_ids": ["a", "b"] }), "kill").is_err());
    }

    #[test]
    fn wait_uses_task_id_when_task_ids_is_empty() {
        let result = manage_task_tool(&serde_json::json!({
            "action": "wait",
            "task_id": "task-id-shadow-test",
            "task_ids": [],
            "timeout_ms": 1000,
        }))
        .expect("wait should accept the single task_id");
        assert!(
            result.contains("task-id-shadow-test") && !result.contains("No task IDs"),
            "{result}"
        );
        assert!(
            manage_task_tool(&serde_json::json!({ "action": "wait", "task_ids": [] })).is_err()
        );
    }

    #[test]
    fn npm_prefix_discovery_is_classified_as_read_only() {
        assert!(command_confirmation_scope("npm config get prefix").is_none());
    }

    #[test]
    fn short_discovery_classifier_is_conservative() {
        for command in [
            "which markdownlint",
            "command -v markdownlint",
            "type markdownlint",
            "npm config get prefix",
            "find . -maxdepth 2 -type f",
            "rg --files src",
            "ls src",
            "stat Cargo.toml",
        ] {
            assert!(
                super::is_short_discovery_command(command),
                "expected short discovery command: {command}"
            );
        }

        for command in [
            "find / -type f",
            "rg TODO /",
            "ls /",
            "npm install",
            "cargo test",
            "printf output > result.txt",
        ] {
            assert!(
                !super::is_short_discovery_command(command),
                "must remain background-capable: {command}"
            );
        }
    }

    #[test]
    fn destructive_git_recovery_commands_require_confirmation() {
        for command in [
            "git restore -- src/GameScene.ts",
            "git checkout -- src/GameScene.ts",
            "git reset --hard HEAD",
            "git clean -fd",
            "git branch -D old-feature",
            "git push --force origin main",
        ] {
            assert!(
                command_requires_confirmation(&serde_json::json!({"command": command})),
                "must confirm: {command}"
            );
            assert!(
                command_confirmation_scope(command).is_some(),
                "must name scope: {command}"
            );
        }
    }

    #[test]
    fn chained_git_commands_are_checked_per_segment() {
        assert!(!command_requires_confirmation(
            &serde_json::json!({"command": "git status --short; git diff --stat; git log -1"})
        ));
        let command = "git status --short; git restore -- src/GameScene.ts";
        let scope = command_confirmation_scope(command).expect("restore segment is destructive");
        assert!(scope.contains("git restore"), "scope: {scope}");
        let preview = command_confirmation_preview(
            command,
            crate::config::SandboxMode::WorkspaceWrite,
            true,
            None,
        );
        assert!(
            preview.contains("resolved command: git status"),
            "preview: {preview}"
        );
        assert!(
            preview.contains("effective OS permissions:"),
            "preview: {preview}"
        );
        assert!(
            preview.contains("network access (one time)"),
            "preview: {preview}"
        );
        let scoped_preview = command_confirmation_preview(
            command,
            crate::config::SandboxMode::ReadOnly,
            false,
            Some("/private/tmp/generated-assets"),
        );
        assert!(
            scoped_preview.contains("write access to '/private/tmp/generated-assets' (one time)")
        );
        assert!(scoped_preview.contains("effective OS permissions: read-only"));
        assert!(preview.contains("scope: git restore"), "preview: {preview}");
    }

    #[test]
    fn git_options_before_subcommand_do_not_hide_destructive_scope() {
        assert!(command_requires_confirmation(&serde_json::json!({
            "command": "git -C /tmp/project --work-tree=/tmp/project restore -- file.ts"
        })));
        assert!(command_requires_confirmation(&serde_json::json!({
            "command": "git -c core.autocrlf=false checkout -- file.ts"
        })));
    }

    #[test]
    fn read_only_git_inspection_stays_non_blocking() {
        for command in [
            "git status --short",
            "git diff -- src/GameScene.ts",
            "git log -5 --oneline",
            "git show HEAD:src/GameScene.ts",
            "git rev-parse --show-toplevel",
        ] {
            assert!(
                !command_requires_confirmation(&serde_json::json!({"command": command})),
                "must not confirm: {command}"
            );
        }
    }

    #[test]
    fn allowlisted_read_only_shell_commands_stay_non_blocking() {
        for command in [
            "gh issue list --repo lhagfoss/rustcode",
            "gh auth status",
            "gh api repos/LHagfoss/rustcode/releases?per_page=100 --jq .[].tag_name",
            "gh api 'repos/LHagfoss/rustcode/releases?per_page=100' --jq '[.[] | select(.draft == false)]'",
            "gh api --method GET repos/LHagfoss/rustcode/releases --paginate -q .[].tag_name",
            "gh api --method=GET --hostname github.com repos/LHagfoss/rustcode/releases --jq=.[].tag_name",
            "gh api -XGET repos/LHagfoss/rustcode/releases --jq '[.[] | .tag_name]'",
            "gh release list --limit 10",
            "gh release view v0.56.16 --json tagName",
            "printf '%s' \"$HOME\"",
            "rg -n AutoConfirm src/",
            "pwd",
        ] {
            assert!(
                !command_requires_confirmation(&serde_json::json!({"command": command})),
                "must not confirm: {command}"
            );
        }
    }

    #[test]
    fn gh_api_write_forms_and_release_mutations_still_require_confirmation() {
        for command in [
            "gh api repos/LHagfoss/rustcode/releases -f tag_name=v0.57.0",
            "gh api repos/LHagfoss/rustcode/releases -F tag_name=v0.57.0",
            "gh api repos/LHagfoss/rustcode/releases --input release.json",
            "gh api repos/LHagfoss/rustcode/releases --header 'X-HTTP-Method-Override: POST'",
            "gh api --method POST repos/LHagfoss/rustcode/releases",
            "gh api repos/LHagfoss/rustcode/releases -X PATCH",
            "gh api repos/LHagfoss/rustcode/releases --method='GET -F tag_name=x'",
            "gh api \"$HOME\" --jq .",
            "gh api 'repos/LHagfoss/rustcode/releases --method POST'",
            "gh api repos/LHagfoss/rustcode/releases --unknown-flag",
            "gh api graphql -f query='mutation { deleteRelease }'",
            "gh release create v0.57.0",
            "gh release edit v0.56.16 --title latest",
            "gh release delete v0.56.16",
        ] {
            assert!(
                command_requires_confirmation(&serde_json::json!({"command": command})),
                "must confirm: {command}"
            );
        }
    }

    #[test]
    fn path_qualified_inspection_names_require_confirmation() {
        for command in [
            "./gh api repos/LHagfoss/rustcode/releases",
            "'/tmp/cat' README.md",
            "echo data | ./cat",
        ] {
            assert!(
                command_requires_confirmation(&serde_json::json!({"command": command})),
                "{command}"
            );
        }
    }

    #[test]
    fn quoted_gh_api_filters_stay_one_segment_and_shell_writes_do_not() {
        for command in [
            "gh api repos/LHagfoss/rustcode/releases --jq '[.[] | select(.draft == false)]'",
            "gh api repos/LHagfoss/rustcode/releases --jq '[.[] | .tag_name]' && gh release list",
        ] {
            assert!(
                !command_requires_confirmation(&serde_json::json!({"command": command})),
                "must not confirm: {command}"
            );
        }

        for command in [
            "gh api repos/LHagfoss/rustcode/releases --jq '[.[] | .tag_name]' && rm -rf /tmp/example",
            "gh api repos/LHagfoss/rustcode/releases --jq \"$(rm -rf /tmp/example)\"",
            "gh api repos/LHagfoss/rustcode/releases --jq '[.[] | .tag_name]'\\|rm -rf /tmp/example",
            "gh api repos/LHagfoss/rustcode/releases --jq '[.[] | .tag_name]",
        ] {
            assert!(
                command_requires_confirmation(&serde_json::json!({"command": command})),
                "must confirm: {command}"
            );
        }
    }

    #[test]
    fn harmless_file_inspection_shells_are_allowed() {
        for command in [
            "cat src/main.rs",
            "head -40 src/main.rs",
            "tail -40 src/main.rs",
            "sed -n '1,40p' src/main.rs",
            "grep -n TODO src/main.rs",
            "cat src/main.rs | grep TODO",
        ] {
            assert!(
                !command_requires_confirmation(&serde_json::json!({"command": command})),
                "harmless inspection must remain available: {command}"
            );
        }
    }

    #[test]
    fn unsafe_file_inspection_shells_still_require_confirmation() {
        for command in [
            "cat src/main.rs > /tmp/main.rs",
            "sed -i 's/old/new/' file.txt",
            "sed 'w output.txt' input.txt",
            "cat src/main.rs | tee /tmp/main.rs",
            "cat \"$(rm -rf /tmp/example)\"",
            "echo \"$(rm -rf /tmp/example)\"",
            "cat \"`rm -rf /tmp/example`\"",
        ] {
            assert!(
                command_requires_confirmation(&serde_json::json!({"command": command})),
                "must confirm potentially mutating inspection command: {command}"
            );
        }
    }

    #[test]
    fn unknown_or_mutating_shell_commands_require_confirmation() {
        for command in [
            "gh issue close 1 --repo lhagfoss/rustcode",
            "rm -rf /tmp/example",
            "python -c 'print(1)'",
            "cargo test",
            "find . -exec rm -f {} \\;",
            "command rm -rf /tmp/example",
        ] {
            assert!(
                command_requires_confirmation(&serde_json::json!({"command": command})),
                "must confirm: {command}"
            );
        }
    }

    #[test]
    fn unknown_segment_in_shell_chain_requires_confirmation() {
        assert!(command_requires_confirmation(&serde_json::json!({
            "command": "git status --short && gh issue close 1"
        })));
    }

    #[test]
    fn interactive_sudo_is_detected() {
        for cmd in [
            "sudo",
            "sudo apt update",
            "sudo -S apt update",
            "sudo --stdin apt update",
            "sudo -nS apt update",
            "sudo -u root apt update",
            "sudo grep -n foo file",
            "sudo -- grep -n foo file",
            "echo -n hi && sudo rm x",
            "echo -n hi; sudo rm x",
            "echo -n hi | sudo tee /etc/hosts",
            "echo -n hi\nsudo rm x",
            "echo $(sudo cat /etc/shadow)",
            "echo `sudo cat /etc/shadow`",
            "/usr/bin/sudo apt update",
        ] {
            assert!(has_interactive_sudo(cmd), "expected rejection for: {cmd:?}");
        }
    }

    #[test]
    fn non_interactive_and_sudo_free_commands_are_allowed() {
        for cmd in [
            "",
            "grep -n foo file",
            "echo -n hi && echo there",
            "echo 'sudo apt update'",
            "sudo -n apt update",
            "sudo --non-interactive apt update",
            "sudo -n -u root apt update",
            "sudo -u root -n apt update",
            "sudo --user=root -n apt update",
            "sudo -nu root apt update",
            "sudo -n grep -S foo file",
            "echo hi && sudo -n rm x",
        ] {
            assert!(!has_interactive_sudo(cmd), "expected allow for: {cmd:?}");
        }
    }

    #[test]
    fn interactive_sudo_is_rejected_by_run_command() {
        let err = run_command(&serde_json::json!({
            "command": "sudo grep -n foo file"
        }))
        .expect_err("interactive sudo should be rejected");

        assert!(err.contains("Interactive 'sudo' commands"));
    }

    // Compiler errors, test failures, and stack traces overwhelmingly land at
    // the *end* of a failing command's output. A head-only truncation (the
    // prior behavior) would throw that away before the model ever sees it.
    #[test]
    fn a_failing_command_with_oversized_output_keeps_the_tail() {
        if !sandbox::runtime_tests_available() {
            return;
        }
        let result = run_command(&serde_json::json!({
            "command": "printf 'START_MARKER\\n'; \
                i=0; while [ $i -lt 20000 ]; do printf 'filler line %d\\n' $i; i=$((i+1)); done; \
                printf 'END_MARKER\\n'; exit 1"
        }))
        .expect("run_command reports failure via exit code, not Err");

        assert!(
            result.contains("exit code: 1"),
            "got: {}",
            &result[..200.min(result.len())]
        );
        assert!(
            result.contains("END_MARKER"),
            "tail must survive truncation on failure so the model can see what broke"
        );
        assert!(
            result.contains("truncated"),
            "output should be reported as truncated"
        );
    }

    // Successful output should stay concise (still bounded) but the shared
    // truncation must not silently drop either end.
    #[test]
    fn oversized_output_is_bounded_and_keeps_both_head_and_tail() {
        if !sandbox::runtime_tests_available() {
            return;
        }
        let result = run_command(&serde_json::json!({
            "command": "printf 'START_MARKER\\n'; \
                i=0; while [ $i -lt 20000 ]; do printf 'filler line %d\\n' $i; i=$((i+1)); done; \
                printf 'END_MARKER\\n'"
        }))
        .expect("shell command should succeed");

        assert!(result.contains("exit code: 0"));
        assert!(
            result.contains("START_MARKER"),
            "head must survive truncation"
        );
        assert!(
            result.contains("END_MARKER"),
            "tail must survive truncation"
        );
        assert!(
            result.len() < 200_000,
            "result must actually be bounded, got {} bytes",
            result.len()
        );
    }

    #[test]
    fn cat_and_head_are_read_only_and_execute_cleanly() {
        if !sandbox::runtime_tests_available() {
            return;
        }
        assert!(!command_requires_confirmation(&serde_json::json!({
            "command": "cat Cargo.toml"
        })));
        assert!(!command_requires_confirmation(&serde_json::json!({
            "command": "head -n 5 Cargo.toml"
        })));

        let result = run_command(&serde_json::json!({
            "command": "head -n 2 Cargo.toml"
        }))
        .expect("head command should execute cleanly");
        assert!(result.contains("exit code: 0"));
        assert!(result.contains("[package]"));
    }
}

#[cfg(test)]
mod generation_verification_tests {
    use super::*;
    #[test]
    fn completed_shell_verification_reuses_success_until_relevant_edit() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir(directory.path().join("src")).unwrap();
        std::fs::write(
            directory.path().join("Cargo.toml"),
            "[package]\nname='verification_fixture'\nversion='0.1.0'\nedition='2021'\n",
        )
        .unwrap();
        std::fs::write(
            directory.path().join("src/lib.rs"),
            "pub fn verified() {}\n",
        )
        .unwrap();
        let args = serde_json::json!({"command":"cargo check --offline", "cwd":directory.path(), "timeout_ms":30000});
        crate::tools::set_active_workspace_context(
            Some(directory.path().to_path_buf()),
            None,
            false,
            Some(crate::config::SandboxMode::Trusted),
        );
        let first = run_command_output(&args).unwrap();
        assert!(first.success, "{}", first.content);
        // The first check may create Cargo.lock, advancing its input generation.
        let stabilized = run_command_output(&args).unwrap();
        assert!(stabilized.success, "{}", stabilized.content);
        let repeated = run_command_output(&args).unwrap();
        assert!(
            repeated.success && repeated.replayed,
            "{}",
            repeated.content
        );
        assert!(repeated.content.contains("CACHED VERIFICATION"));
        std::fs::write(directory.path().join("src/lib.rs"), "pub fn broken( {\n").unwrap();
        let changed = run_command_output(&args).unwrap();
        assert!(!changed.success && !changed.replayed, "{}", changed.content);
        crate::tools::set_active_workspace_context(None, None, false, None);
    }
}
