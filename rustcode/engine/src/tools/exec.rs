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
    command_requires_confirmation, denied_command_prefix_covers_call, destructive_command_reason,
    display_approved_command_prefix, is_help_or_version_command, persisted_approved_command_prefix,
    pull_request_base, reject_broad_git_stage, rememberable_command_forbid_prefix_for_call,
    rememberable_command_prefix_for_call, timed_approval_covers_call, unix_now_secs,
};
use policy::{has_interactive_sudo, is_home_wide_traversal, is_short_discovery_command};

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

/// Foreground commands currently running per session, and how many times the
/// user has asked to move them to the background.
#[derive(Default)]
struct ForegroundCommands {
    running: usize,
    handoff_requests: u64,
}

fn foreground_commands() -> &'static Mutex<HashMap<String, ForegroundCommands>> {
    static COMMANDS: OnceLock<Mutex<HashMap<String, ForegroundCommands>>> = OnceLock::new();
    COMMANDS.get_or_init(Default::default)
}

/// Ask every foreground command the session is running to continue as a
/// background task. Returns how many commands were running.
pub fn move_foreground_commands_to_background(session_id: &str) -> usize {
    let mut commands = foreground_commands()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    match commands.get_mut(session_id) {
        Some(entry) if entry.running > 0 => {
            entry.handoff_requests += 1;
            entry.running
        }
        _ => 0,
    }
}

/// Whether the session has a foreground command that could be moved.
pub fn has_foreground_command(session_id: &str) -> bool {
    foreground_commands()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(session_id)
        .is_some_and(|entry| entry.running > 0)
}

/// One running foreground command; unregisters itself when dropped.
struct ForegroundCommand {
    session_id: String,
    requests_at_start: u64,
}

impl ForegroundCommand {
    fn start(session_id: &str) -> Self {
        let mut commands = foreground_commands()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let entry = commands.entry(session_id.to_owned()).or_default();
        entry.running += 1;
        Self {
            session_id: session_id.to_owned(),
            requests_at_start: entry.handoff_requests,
        }
    }

    fn handoff_requested(&self) -> bool {
        foreground_commands()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&self.session_id)
            .is_some_and(|entry| entry.handoff_requests != self.requests_at_start)
    }
}

impl Drop for ForegroundCommand {
    fn drop(&mut self) {
        let mut commands = foreground_commands()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(entry) = commands.get_mut(&self.session_id) {
            entry.running = entry.running.saturating_sub(1);
            if entry.running == 0 {
                commands.remove(&self.session_id);
            }
        }
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
            "command": { "type": "string" }, "cwd": { "type": "string", "description": "\"sandbox\" is the session scratch directory, also $RUSTCODE_SANDBOX in the command" },
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
const POSIX_RUN_COMMAND_DESCRIPTION: &str = "Run one shell command; returns stdout, stderr and exit code. Pipelines fail if any stage fails; timeout defaults to 120s; interactive sudo is disabled. Prefer view_file over cat/sed/head for file reads. The OS sandbox (none on Windows) fails closed and a denied command reports which permission to request: network_access=true or filesystem_write_path (an existing absolute directory, outside the workspace in restricted modes) grants it for this command only, with confirmation unless YOLO is on. background=true runs a long job (a build, a test suite) without blocking you: it returns a task ID at once, you keep working, and the result is delivered when it finishes (notify_on_complete=false: silent); a quick command needs no background. detached=true starts a long-lived server or watcher, returns a task ID at once and stays silent unless notify_on_complete=true. Use manage_task for logs, wait and kill; never poll. For external jobs run the provider's blocking watch command once with background=true. A command containing '&' is treated as detached unless the script waits on or kills its own jobs; never add '&' with detached=true. Git: follow the repository `AGENTS.md`, which outranks generic workflow skills; never `git rebase`, `git reset --hard` or force-push, or discard uncommitted work. Branch with `git switch -c`; `git worktree add` under /tmp only when genuinely required, then `git worktree remove`, `git worktree prune`, `git branch -d`; finish on the original branch after `git pull --ff-only`.";

/// Windows runs commands through PowerShell (see `rustcode_command::ShellKind`),
/// so the model is told to write PowerShell and where it differs from sh.
const WINDOWS_RUN_COMMAND_DESCRIPTION: &str = "Run one PowerShell command; returns stdout, stderr and the exit code of its last statement. The shell is PowerShell 7 (pwsh) when installed, else Windows PowerShell 5.1; the `Shell` line of the runtime context says which (if it says cmd.exe, write cmd syntax instead). Write PowerShell, not POSIX sh: chain with `;` (`&&` and `||` exist only in PowerShell 7); no heredocs, create files with write_to_file; set variables with `$env:NAME = 'value'`; run a quoted path with `& 'C:\\dir with spaces\\tool.exe'`; `/` and `\\` both separate paths; discard output with `> $null`; in 5.1 `curl` and `wget` are aliases, so call `curl.exe`; when a `.ps1` shim is blocked by execution policy call the `.cmd` one (`npm.cmd`). stderr is already captured, so omit `2>&1`. Timeout defaults to 120s; the session is non-interactive. Prefer view_file, grep, glob and list_directory over Get-Content, Select-String and Get-ChildItem. There is no OS sandbox on Windows. background=true runs a long job (a build, a test suite) without blocking you: it returns a task ID at once, you keep working, and the result is delivered when it finishes (notify_on_complete=false: silent); a quick command needs no background. detached=true starts a long-lived server or watcher, returns a task ID at once and stays silent unless notify_on_complete=true. Use manage_task for logs, wait and kill; never poll. For external jobs run the provider's blocking watch command once with background=true. Git: follow the repository `AGENTS.md`, which outranks generic workflow skills; never `git rebase`, `git reset --hard` or force-push, or discard uncommitted work. Branch with `git switch -c`; `git worktree add` under `$env:TEMP` only when genuinely required, then `git worktree remove`, `git worktree prune`, `git branch -d`; finish on the original branch after `git pull --ff-only`.";

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
    arguments: r#"{"command": "full shell command string", "cwd": "optional working directory", "timeout_ms": "optional timeout in ms", "background": "optional bool; run without blocking, the result is delivered on completion (default false)", "detached": "optional bool for a long-lived server/watcher; returns a completed start result with task ID and keeps it killable (default false)", "notify_on_complete": "optional bool; defaults false for detached and true for background", "network_access": "optional bool requesting one-shot network access; requires confirmation unless YOLO is enabled", "filesystem_write_path": "optional existing absolute directory requested for one-command write access; requires confirmation unless YOLO is enabled"}"#,
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
    description: "Manage run_command tasks. 'wait' blocks until a task (or the first of task_ids) finishes; use it only when nothing else is left to do, and never poll. 'logs' returns captured output. Detached tasks are silent unless waited on.",
    arguments: r#"{"action": "list, status, logs, kill, or wait", "task_id": "required for status/kill/logs or single-task wait", "task_ids": "optional list of IDs for wait-any", "full": "optional true to retrieve the complete log", "tail_bytes": "optional log tail limit, max 65536", "timeout_ms": "optional wait timeout in ms, default 300000, max 1800000"}"#,
    handler: manage_task_tool,
    requires_confirmation: false,
    schema: manage_task_schema,
    capabilities: &[ToolCapability::ExecuteCommands],
    safety: ToolSafety::ProcessControl,
};

const DEFAULT_COMMAND_TIMEOUT_MS: u64 = 120_000;

/// Default timeout for a command that walks the whole home directory or `/`
/// when the call names no `timeout_ms`. Such a walk can stall on a
/// privacy-protected or cloud-backed folder rather than fail (#2022); a short
/// bound turns that stall into a quick, explained failure.
const HOME_TRAVERSAL_TIMEOUT_MS: u64 = 20_000;

const HOME_TRAVERSAL_TIMEOUT_HINT: &str = "hint: this command walks the whole home directory (or /). Privacy-protected folders (macOS Desktop, Documents, Downloads, Library, iCloud Drive, ~/Library/CloudStorage) and network or cloud mounts can block a background process indefinitely instead of failing, and the output is lost when it is killed. Do not retry a home-wide search: list ~ shallowly (ls ~) and search inside a specific directory such as ~/code, or pass timeout_ms explicitly if a full scan is really needed.";

/// Attach traversal guidance to a timeout of a home-wide walk; any other
/// error passes through unchanged.
fn with_home_traversal_hint(error: String, home_traversal: bool) -> String {
    if home_traversal && crate::network::loop_detect::is_command_timeout(&error) {
        format!("{error}\n{HOME_TRAVERSAL_TIMEOUT_HINT}")
    } else {
        error
    }
}

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
fn detached_shell_command(
    command: &str,
    has_background_operator: bool,
    log: Option<&std::path::Path>,
) -> String {
    rustcode_command::detached_command_with_log(
        rustcode_command::host_shell(),
        command,
        has_background_operator,
        log,
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

/// Apply the project's command policy (`crate::command_guard`) to a command
/// before it runs: a denied command fails with the rule's message, a command a
/// rule rewrites runs as rewritten and its output says so, and output a hint
/// rule recognises (a failure another thread caused) ends with its hint.
fn run_command_output_inner(
    args: &Value,
    progress: Option<CommandProgressCallback>,
    cancel_token: Option<tokio_util::sync::CancellationToken>,
    call_id: Option<&str>,
    explicit_workspace_root: Option<Option<std::path::PathBuf>>,
) -> Result<super::ToolExecutionOutput, String> {
    use crate::command_guard::{Verdict, check, hint, record_violation, refusal};
    let Some(command_str) = args.get("command").and_then(Value::as_str) else {
        return run_command_output_unguarded(
            args,
            progress,
            cancel_token,
            call_id,
            explicit_workspace_root,
        );
    };
    let workspace = match &explicit_workspace_root {
        Some(root) => root.clone(),
        None => super::active_workspace_root(),
    };
    let guard_dir = match args.get("cwd").and_then(Value::as_str) {
        Some("sandbox" | "./sandbox") => None,
        Some(cwd) if std::path::Path::new(cwd).is_absolute() => Some(std::path::PathBuf::from(cwd)),
        Some(cwd) => workspace.as_ref().map(|root| root.join(cwd)),
        None => super::current_tool_context()
            .task_working_directory
            .clone()
            .or(workspace),
    };
    let guard_env: Vec<(String, String)> = args
        .get("env")
        .and_then(Value::as_object)
        .map(|env| {
            env.iter()
                .filter_map(|(name, value)| Some((name.clone(), value.as_str()?.to_owned())))
                .collect()
        })
        .unwrap_or_default();
    let verdict = guard_dir
        .as_deref()
        .map_or(Verdict::Allow, |dir| check(dir, command_str, &guard_env));
    let thread_id = get_active_session_id().unwrap_or_default();
    let with_hint = |result: Result<super::ToolExecutionOutput, String>| {
        let Some(dir) = guard_dir.as_deref() else {
            return result;
        };
        match result {
            Ok(mut output) => {
                if let Some(text) = hint(dir, command_str, &output.content) {
                    output.content = format!("{}\n{text}", output.content.trim_end());
                }
                Ok(output)
            }
            Err(error) => Err(match hint(dir, command_str, &error) {
                Some(text) => format!("{}\n{text}", error.trim_end()),
                None => error,
            }),
        }
    };
    match verdict {
        Verdict::Allow => with_hint(run_command_output_unguarded(
            args,
            progress,
            cancel_token,
            call_id,
            explicit_workspace_root,
        )),
        Verdict::Deny { .. } => {
            record_violation(&thread_id, "rustcode", &verdict);
            Err(refusal(&verdict).unwrap_or_default())
        }
        Verdict::Rewrite {
            ref command,
            ref rules,
        } => {
            record_violation(&thread_id, "rustcode", &verdict);
            let mut rewritten = args.clone();
            rewritten["command"] = Value::String(command.clone());
            let note = format!(
                "[command guard: rule `{}` ran this as: {command}]\n",
                rules.join("`, `")
            );
            let mut output = with_hint(run_command_output_unguarded(
                &rewritten,
                progress,
                cancel_token,
                call_id,
                explicit_workspace_root,
            ))?;
            output.content = format!("{note}{}", output.content);
            Ok(output)
        }
    }
}

fn run_command_output_unguarded(
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
    let home_traversal = is_home_wide_traversal(command_str);
    let timeout_ms = args
        .get("timeout_ms")
        .and_then(parse_json_number)
        .unwrap_or(if home_traversal {
            HOME_TRAVERSAL_TIMEOUT_MS
        } else {
            DEFAULT_COMMAND_TIMEOUT_MS
        });
    let env = args.get("env").and_then(|e| e.as_object());

    let context = super::current_tool_context();
    #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
    let has_explicit_workspace_root = explicit_workspace_root.is_some();
    let workspace_root = explicit_workspace_root.unwrap_or_else(|| super::active_workspace_root());
    let resolved_cwd = match cwd {
        Some("sandbox") | Some("./sandbox") => {
            if let Some(session_id) = get_active_session_id() {
                crate::sessions::get_active_session_sandbox_dir(&session_id)
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
    // Secrets the user gave this thread through `request_secret` with `env`:
    // the value reaches the process, never the arguments or the transcript.
    if let Some(session_id) = get_active_session_id() {
        for (name, value) in crate::orchestration::secret::thread_env(&session_id) {
            command_env.push((name.into(), value.into()));
        }
    }
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
    // A detached command writes its output straight to its task log (its
    // pipes are cut), so the task identity and log are fixed before the
    // wrapper is built.
    let detached_task = if detached {
        let task_id = next_task_id();
        let session_id = get_active_session_id().unwrap_or_default();
        let log = task_output_log_path(&session_id, &task_id)?;
        Some((task_id, log))
    } else {
        None
    };
    let detached_log = detached_task.as_ref().and_then(|(_, log)| log.as_deref());
    let shell_command = if detached {
        detached_shell_command(command_str, has_background_operator, detached_log)
    } else {
        command_str.to_owned()
    };
    let session_scratch = get_active_session_id()
        .and_then(|session_id| crate::sessions::get_active_session_sandbox_dir(&session_id));
    // The file tools' `sandbox/` is the session scratch directory; name it
    // for the shell too, so a command can reach what those tools wrote. It
    // goes right after PATH so an explicit `env` entry still wins.
    if let Some(scratch) = session_scratch.as_ref() {
        command_env.insert(
            1,
            (
                std::ffi::OsString::from("RUSTCODE_SANDBOX"),
                scratch.as_os_str().to_owned(),
            ),
        );
    }
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
    // The detached wrapper appends to its task log from inside the sandbox.
    if let Some(log_dir) = detached_log.and_then(std::path::Path::parent)
        && !writable_roots.iter().any(|root| root == log_dir)
    {
        writable_roots.push(log_dir.to_path_buf());
    }
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
        let (task_id, detached_output_log) = match detached_task {
            Some((task_id, log)) => (task_id, Some(log)),
            None => (next_task_id(), None),
        };

        let task_manager = background_task_manager();
        let mut task_spec = TaskSpec::new(SessionId::new(session_id.clone()), command_request);
        // Keep the user-visible task identity useful even when the detached
        // request uses an internal shell wrapper for pipe/process safety.
        task_spec.command = cmd_str.clone();
        task_spec.notify_on_complete = notify_on_complete;
        task_spec.output_log = match detached_output_log {
            Some(log) => log,
            None => task_output_log_path(&session_id, &task_id)?,
        };
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
                return Ok(foreground_command_output(
                    command_str,
                    &output,
                    &sandbox_context,
                ));
            }
            if !has_start_barrier {
                barrier.release();
            }
        }

        return Ok(background_start_output(
            &task_id,
            cmd_str,
            notify_on_complete,
            false,
        ));
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
    let output = match get_active_session_id() {
        // Inside a session the user can move a slow command to the background.
        Some(session_id) => {
            match run_foreground_or_hand_off(
                &session_id,
                command_str,
                &command_request,
                progress,
                cancellation,
                call_id,
            )
            .map_err(|error| with_home_traversal_hint(error, home_traversal))?
            {
                ForegroundOutcome::Finished(output) => output,
                ForegroundOutcome::MovedToBackground(result) => return Ok(result),
            }
        }
        None => {
            rustcode_command::run_with_timeout_cancellable(&command_request, progress, cancellation)
                .map_err(|error| with_home_traversal_hint(error, home_traversal))?
        }
    };
    let result = foreground_command_output(command_str, &output, &sandbox_context);
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

enum ForegroundOutcome {
    Finished(rustcode_command::CommandOutput),
    MovedToBackground(super::ToolExecutionOutput),
}

/// Run a foreground command, handing the still-running process to the task
/// manager if the user asks for it. Output is teed into a task log from the
/// start so `manage_task` `logs` covers the whole run after a hand-off.
fn run_foreground_or_hand_off(
    session_id: &str,
    command_str: &str,
    request: &rustcode_command::CommandRequest,
    progress: Option<CommandProgressCallback>,
    cancellation: Option<rustcode_command::CancellationCallback>,
    call_id: Option<&str>,
) -> Result<ForegroundOutcome, String> {
    let task_id = next_task_id();
    let log_path = task_output_log_path(session_id, &task_id).ok().flatten();
    let log_writer = log_path
        .as_deref()
        .and_then(|path| rustcode_tasks::create_task_log(path).ok());
    let log_path = log_path.filter(|_| log_writer.is_some());
    let moved = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let tee: CommandProgressCallback = {
        let moved = Arc::clone(&moved);
        Arc::new(move |bytes: &[u8], stderr: bool| {
            if let Some(log) = log_writer.as_ref() {
                log(bytes, stderr);
            }
            // The tool call has returned once moved: stop feeding its cell.
            if !moved.load(Ordering::Relaxed)
                && let Some(live) = progress.as_ref()
            {
                live(bytes, stderr);
            }
        })
    };
    let foreground = Arc::new(ForegroundCommand::start(session_id));
    let handoff = {
        let foreground = Arc::clone(&foreground);
        Arc::new(move || foreground.handoff_requested()) as rustcode_command::HandoffCallback
    };
    let outcome =
        rustcode_command::run_with_timeout_or_handoff(request, Some(tee), cancellation, handoff);
    let remove_log = || {
        if let Some(path) = log_path.as_deref() {
            let _ = std::fs::remove_file(path);
        }
    };
    match outcome {
        Ok(rustcode_command::RunOutcome::Finished(output)) => {
            remove_log();
            Ok(ForegroundOutcome::Finished(output))
        }
        Err(error) => {
            remove_log();
            Err(error)
        }
        Ok(rustcode_command::RunOutcome::HandedOff(running)) => {
            moved.store(true, Ordering::Relaxed);
            let mut spec = TaskSpec::new(SessionId::new(session_id), request.clone());
            spec.command = command_str.to_owned();
            spec.notify_on_complete = true;
            spec.output_log = log_path;
            if let Some(call_id) = call_id {
                spec = spec.with_call_id(call_id);
            }
            let pid = running.pid();
            background_task_manager()
                .adopt_with_id(task_id.clone(), spec, pid, move || running.wait())?;
            Ok(ForegroundOutcome::MovedToBackground(
                background_start_output(&task_id, command_str.to_owned(), true, true),
            ))
        }
    }
}

/// What a failed command's sandbox attribution note needs to know.
struct CommandSandboxContext<'a> {
    mode: crate::config::SandboxMode,
    network_access: bool,
    writable_roots: &'a [std::path::PathBuf],
    one_shot_writable_roots: &'a [std::path::PathBuf],
}

/// Whether the command ends in a bare `rg`/`grep` search, outside any
/// pipeline. Only then is the shell's exit status the search's own.
fn ends_in_bare_search(command: &str) -> bool {
    // Mask quoted text so a `|` or `;` inside a pattern is not taken for
    // shell syntax.
    let mut quote = None;
    let command: String = command
        .chars()
        .map(|c| match (quote, c) {
            (None, '\'' | '"') => {
                quote = Some(c);
                'q'
            }
            (Some(open), _) if c == open => {
                quote = None;
                'q'
            }
            (Some(_), _) => 'q',
            (None, c) => c,
        })
        .collect();
    let command = command.as_str();
    let last = command
        .rsplit(['\n', ';'])
        .next()
        .unwrap_or(command)
        .rsplit("&&")
        .next()
        .unwrap_or(command)
        .rsplit("||")
        .next()
        .unwrap_or(command)
        .trim();
    if last.contains('|') {
        return false;
    }
    let mut words = last.split_whitespace();
    match words.next() {
        Some("rg" | "grep" | "egrep" | "fgrep") => true,
        Some("git") => words.next() == Some("grep"),
        _ => false,
    }
}

/// `rg` and `grep` exit 1 when nothing matched. With no output at all that is
/// an answer, not a failure, and counting it as one fed the failed-call
/// streaks in session 01a15ef714ce.
fn is_empty_search_miss(command: &str, output: &rustcode_command::CommandOutput) -> bool {
    output.exit_code == Some(1)
        && output.signal.is_none()
        && output.stdout.total_bytes() == 0
        && output.stderr.total_bytes() == 0
        && ends_in_bare_search(command)
}

/// `rg -rn` is a grep habit, but in ripgrep `-r` is `--replace`: the pattern
/// is searched and each match is rewritten to `n`. Point that out once in the
/// result. A lone `-r` is left alone, as it is usually a deliberate replace.
fn rg_replace_flag_hint(command: &str) -> Option<&'static str> {
    let mut in_rg = false;
    for word in command.split_whitespace() {
        match word {
            "rg" => in_rg = true,
            path if path.ends_with("/rg") => in_rg = true,
            "|" | "&&" | "||" | ";" => in_rg = false,
            flag if in_rg
                && flag.len() > 2
                && flag.starts_with('-')
                && !flag.starts_with("--")
                && flag[1..].chars().all(|c| c.is_ascii_alphabetic())
                && flag[1..].contains('r') =>
            {
                return Some(
                    "[note: in rg, -r is --replace, not recursive; rg already searches recursively, so drop the r]",
                );
            }
            _ => {}
        }
    }
    None
}

/// Render a finished command as the `run_command` result the model reads.
fn foreground_command_output(
    command: &str,
    output: &rustcode_command::CommandOutput,
    sandbox: &CommandSandboxContext<'_>,
) -> super::ToolExecutionOutput {
    let exit_code = output.exit_code.unwrap_or(-1);
    let search_miss = is_empty_search_miss(command, output);
    let succeeded = output.success || search_miss;

    let command_status = command_result_metadata(output);
    let mut result = String::new();
    result.push_str(&format!(
        "{}\nexit code: {exit_code}\n",
        format_command_status(succeeded, &command_status)
    ));

    let failed = !succeeded;
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
    if search_miss {
        result.push_str("(no matches)\n");
    } else if stdout.is_empty() && stderr.is_empty() {
        result.push_str("(no output)\n");
    }
    if let Some(hint) = rg_replace_flag_hint(command) {
        result.push_str(hint);
        result.push('\n');
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
    let mut remote = crate::platform::command("git");
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
    let mut refs = crate::platform::command("git");
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

fn next_task_id() -> String {
    format!(
        "task_{}_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or(std::time::Duration::from_secs(0))
            .as_millis(),
        BACKGROUND_TASK_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}

/// The start result for a command that now runs as a background task.
fn background_start_output(
    task_id: &str,
    cmd_str: String,
    notify_on_complete: bool,
    moved_by_user: bool,
) -> super::ToolExecutionOutput {
    super::ToolExecutionOutput {
        content: format!(
            "Task started in background. Task ID: {task_id}. Status: Running. Command: {cmd_str}. Completion notification: {}{} Captured output is available with manage_task action 'logs'; never poll.",
            background_follow_up(notify_on_complete),
            if moved_by_user {
                " The user moved this running command to the background; it was not interrupted."
            } else {
                ""
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
    }
}

/// What the model is told to do after a command went to the background.
fn background_follow_up(notify_on_complete: bool) -> &'static str {
    if notify_on_complete {
        "enabled. It is detached from this call: keep working on whatever does not depend on it, and its result is delivered to you when it finishes. Call manage_task action 'wait' only when nothing else is left to do."
    } else {
        "disabled. Call manage_task action 'wait' when you need the result."
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

/// Five minutes: long enough for most builds, short enough that the agent
/// checks in, reads the output and decides instead of blocking (#2785).
const DEFAULT_WAIT_TIMEOUT_MS: u64 = 300_000;
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
    let Some(session_dir) = crate::sessions::get_active_session_dir(session_id) else {
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

/// Tasks a `manage_task` `wait` is blocked on right now, counted per waiter.
/// A completion sink that sees one of these defers to the wait: the wait is
/// subscribed first, so it decides whether the result reaches the model.
fn wait_in_flight() -> &'static Mutex<HashMap<(String, String), usize>> {
    static IN_FLIGHT: OnceLock<Mutex<HashMap<(String, String), usize>>> = OnceLock::new();
    IN_FLIGHT.get_or_init(Default::default)
}

struct WaitInFlight {
    keys: Vec<(String, String)>,
}

impl WaitInFlight {
    fn new(session_id: &str, task_ids: &[String]) -> Self {
        let keys = task_ids
            .iter()
            .map(|id| (session_id.to_owned(), id.clone()))
            .collect::<Vec<_>>();
        let mut in_flight = wait_in_flight()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for key in &keys {
            *in_flight.entry(key.clone()).or_default() += 1;
        }
        Self { keys }
    }
}

impl Drop for WaitInFlight {
    fn drop(&mut self) {
        let mut in_flight = wait_in_flight()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for key in &self.keys {
            if let Some(count) = in_flight.get_mut(key) {
                *count -= 1;
                if *count == 0 {
                    in_flight.remove(key);
                }
            }
        }
    }
}

/// Whether a `manage_task` `wait` answers this task's completion, so the
/// completion sink must not record it a second time (#2232). A wait still
/// blocked on the task is let finish first: it holds the earlier
/// subscription and returns the same terminal event.
pub(crate) async fn background_result_answered_by_wait(session_id: &str, task_id: &str) -> bool {
    let key = (session_id.to_owned(), task_id.to_owned());
    while wait_in_flight()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .contains_key(&key)
    {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    background_result_delivered_by_wait(session_id, task_id)
}

/// Streams bytes appended to the waited tasks' output logs into the live tool
/// cell while the wait is still blocked (#2230). The wait sleeps on a task
/// event channel, not on the process, so it polls the logs on each short
/// slice and never delays the terminal result.
struct TaskOutputTail {
    logs: Vec<(std::path::PathBuf, u64)>,
}

impl TaskOutputTail {
    fn new(manager: &TaskManager, session_id: &str, task_ids: &[String]) -> Self {
        let logs = manager
            .list(session_id)
            .into_iter()
            .filter(|task| task_ids.iter().any(|id| id == task.id.as_str()))
            .filter_map(|task| task.output_log)
            .map(|path| (path, 0))
            .collect();
        Self { logs }
    }

    /// Forward whatever the tasks wrote since the last poll. A wait with no
    /// live cell (not inside a turn) has nowhere to put the bytes, so it reads
    /// nothing.
    fn poll(&mut self) {
        use std::io::{Read, Seek, SeekFrom};
        let Some(progress) = super::current_tool_progress() else {
            return;
        };
        for (path, offset) in &mut self.logs {
            let Ok(mut file) = std::fs::File::open(&*path) else {
                continue;
            };
            if file.seek(SeekFrom::Start(*offset)).is_err() {
                continue;
            }
            let mut buffer = Vec::new();
            match file.take(64 * 1024).read_to_end(&mut buffer) {
                Ok(0) | Err(_) => {}
                Ok(read) => {
                    *offset += read as u64;
                    progress(&buffer, false);
                }
            }
        }
    }
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
    let _in_flight = WaitInFlight::new(session_id, task_ids);
    let subscription = manager.subscribe_session(session_id.to_owned());
    let deliver = |completion: &rustcode_tasks::TaskCompletion| {
        note_wait_delivered(session_id, completion.id.as_str());
        format_completion_result(completion, None)
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
    let mut tail = TaskOutputTail::new(manager, session_id, task_ids);
    loop {
        let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) else {
            break;
        };
        // Wake often enough to forward new output, but never past the deadline.
        let slice = remaining.min(std::time::Duration::from_millis(250));
        // The turn stopped: give the thread back at once. The tasks keep running;
        // stopping a wait is not stopping the build.
        if crate::tools::active_tool_cancelled() {
            return still_running_report(
                manager,
                session_id,
                &matched,
                "Stopped waiting: the turn was cancelled.",
            );
        }
        match subscription.recv_timeout(slice) {
            Ok(event)
                if task_ids.iter().any(|id| id == event.task_id().as_str())
                    && event.is_terminal() =>
            {
                tail.poll();
                if let Some(completion) = manager.completion(session_id, event.task_id()) {
                    return deliver(&completion);
                }
                let task_id = event.task_id().to_string();
                note_wait_delivered(session_id, &task_id);
                return format_wait_result(&task_id, event);
            }
            Ok(_) => tail.poll(),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => tail.poll(),
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
            still_running_report(
                manager,
                session_id,
                &running_ids,
                &format!("Still running after waiting {}s.", timeout.as_secs()),
            )
        }
    }
}

/// `completion` as a tool result; with `tail`, only that many bytes from the
/// end of its output, where a failure says what went wrong.
fn format_completion_result(
    completion: &rustcode_tasks::TaskCompletion,
    tail: Option<usize>,
) -> String {
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
    let output = match tail {
        Some(limit) => output_tail(output, limit),
        None => output,
    };
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

/// The last `limit` bytes of `output`, cut at a line where one is near, with
/// a note of what was left out.
fn output_tail(output: String, limit: usize) -> String {
    if output.len() <= limit {
        return output;
    }
    let mut start = output.len() - limit;
    while !output.is_char_boundary(start) {
        start += 1;
    }
    if let Some(line) = output[start..].find('\n').filter(|line| *line < limit / 2) {
        start += line + 1;
    }
    format!(
        "[{start} earlier bytes not shown; manage_task action 'logs' with full=true reads them]\n{}",
        &output[start..]
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

/// The `tail_bytes` a status or logs read asks for, 16 KB by default.
fn task_tail_bytes(args: &serde_json::Value) -> usize {
    args.get("tail_bytes")
        .and_then(|value| value.as_u64())
        .unwrap_or(16_384)
        .clamp(1, 65_536) as usize
}

/// What a wait that ends before its tasks do hands back: each task with how
/// long it has run and the last lines of its output, then what the agent can
/// do next. The tasks are left running.
fn still_running_report(
    manager: &TaskManager,
    session_id: &str,
    ids: &[String],
    headline: &str,
) -> String {
    let running = manager.list(session_id);
    let mut out = String::from(headline);
    for id in ids {
        let Some(task) = running.iter().find(|task| task.id.as_str() == id.as_str()) else {
            continue;
        };
        out.push_str(&format!(
            "\n\nTask '{id}' (`{}`) has run for {}s.",
            task.command,
            task.started_at.elapsed().as_secs()
        ));
        if let Some(path) = &task.output_log
            && let Ok((excerpt, _, _)) = read_task_log(path, false, 2048)
        {
            let lines: Vec<&str> = excerpt
                .lines()
                .filter(|line| !line.trim().is_empty())
                .collect();
            let last = &lines[lines.len().saturating_sub(12)..];
            if !last.is_empty() {
                out.push_str("\nLast output:\n");
                out.push_str(&last.join("\n"));
            }
        }
    }
    out.push_str(
        "\n\nNext: continue other work (the result arrives when the task ends), read more with \
         manage_task logs, wait again with a timeout_ms that fits, or stop a task you no longer need \
         with manage_task kill. The tasks are still running.",
    );
    out
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
                // Its completion was delivered already; a status check reads
                // its end unless asked for all of it.
                let full = args.get("full").and_then(parse_json_bool).unwrap_or(false);
                let tail = (!full).then(|| task_tail_bytes(args));
                Ok(format_completion_result(&completion, tail))
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
            let (excerpt, start, total) = read_task_log(&path, full, task_tail_bytes(args))?;
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
        return crate::platform::command("taskkill")
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
#[path = "exec_tests.rs"]
mod tests;

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
