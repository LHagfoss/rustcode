use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

// Re-exports needed by exec tools
pub(crate) use super::WAKEUP_CALLBACK;
pub(crate) use super::get_active_session_id;
pub(crate) use super::parse_json_bool;
pub(crate) use super::parse_json_number;

use rustcode_tasks::{
    CancelResult, ProcessTerminator, SessionId, TaskEvent, TaskManager, TaskSpec, TaskState,
};

use super::{Tool, ToolCapability, ToolSafety};

mod policy;

pub(crate) use policy::{
    command_confirmation_preview, command_confirmation_scope, command_requires_confirmation,
    reject_broad_git_stage,
};
use policy::{has_interactive_sudo, is_short_discovery_command};

static BACKGROUND_TASK_SEQUENCE: AtomicU64 = AtomicU64::new(1);
static BACKGROUND_TASK_MANAGER: OnceLock<TaskManager> = OnceLock::new();

pub(crate) fn background_task_manager() -> &'static TaskManager {
    BACKGROUND_TASK_MANAGER.get_or_init(|| {
        let manager = TaskManager::new(Arc::new(RootProcessTerminator));
        let subscription = manager.subscribe();
        std::thread::Builder::new()
            .name("rustcode-task-events".to_string())
            .spawn(move || {
                while let Ok(event) = subscription.recv() {
                    dispatch_background_event(event);
                }
            })
            .expect("failed to start background task event dispatcher");
        manager
    })
}

struct RootProcessTerminator;

impl ProcessTerminator for RootProcessTerminator {
    fn terminate(&self, pid: u32) -> bool {
        terminate_background_pid(pid)
    }
}

fn dispatch_background_event(event: TaskEvent) {
    let TaskEvent::Finished {
        id,
        session_id,
        command,
        output,
    } = event
    else {
        return;
    };

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
    if let Some(callback) = WAKEUP_CALLBACK.get() {
        callback(session_id.to_string(), id.to_string(), output);
    }
}

fn command_output_to_tool_output(
    command: &str,
    output: rustcode_command::CommandOutput,
) -> super::ToolExecutionOutput {
    let out_str = rustcode_command::format_bounded_output(&output.stdout);
    let err_str = rustcode_command::format_bounded_output(&output.stderr);
    let mut full = out_str;
    if !err_str.is_empty() {
        if !full.is_empty() {
            full.push('\n');
        }
        full.push_str("stderr:\n");
        full.push_str(&err_str);
    }
    if !output.success {
        full = format!("exit code {:?}\n{full}", output.exit_code);
    }
    super::ToolExecutionOutput {
        content: full,
        success: output.success,
        pending: false,
        command: Some(command.to_owned()),
        exit_code: output.exit_code,
        truncated: output.stdout.is_truncated() || output.stderr.is_truncated(),
        replayed: false,
        error_kind: (!output.success).then_some(super::ToolErrorKind::CommandFailed),
        retryable: false,
    }
}

fn run_command_schema() -> Value {
    serde_json::json!({
        "type": "object", "properties": {
            "command": { "type": "string" }, "cwd": { "type": "string" },
            "timeout_ms": { "type": "integer", "minimum": 1 },
            "background": { "type": "boolean", "default": false },
            "env": { "type": "object", "additionalProperties": { "type": "string" } }
        }, "required": ["command"]
    })
}

pub const RUN_COMMAND: Tool = Tool {
    name: "run_command",
    description: "Run one command through the platform shell and return stdout/stderr and the exit code. Pipelines propagate failure from every stage. Supports normal shell syntax, an optional working directory, environment overrides, timeout (default 120s), and background execution. For external jobs, start the provider's blocking watch command once in the background; completion notifications arrive automatically, so never poll. Interactive sudo requiring a password is disabled.",
    arguments: r#"{"command": "full shell command string", "cwd": "optional working directory", "timeout_ms": "optional timeout in ms", "background": "optional bool to run asynchronously in background (default false)"}"#,
    handler: run_command,
    requires_confirmation: true,
    schema: run_command_schema,
    capabilities: &[ToolCapability::ExecuteCommands],
    safety: ToolSafety::ProcessControl,
};

fn manage_task_schema() -> Value {
    serde_json::json!({
        "type": "object", "properties": {
            "action": { "type": "string", "enum": ["list", "status", "kill"] },
            "task_id": { "type": "string" }
        }, "required": ["action"]
    })
}

pub const MANAGE_TASK: Tool = Tool {
    name: "manage_task",
    description: "Manage background tasks spawned with run_command (action: 'list', 'status', or 'kill'). Do NOT poll 'status' or 'list' in a loop — completion notifications arrive automatically. Stop calling tools to wait for completion.",
    arguments: r#"{"action": "list, status, or kill", "task_id": "required for status/kill"}"#,
    handler: manage_task_tool,
    requires_confirmation: false,
    schema: manage_task_schema,
    capabilities: &[ToolCapability::ExecuteCommands],
    safety: ToolSafety::ProcessControl,
};

const DEFAULT_COMMAND_TIMEOUT_MS: u64 = 120_000;

pub fn run_command(args: &Value) -> Result<String, String> {
    run_command_output(args).map(|output| output.content)
}

pub(super) fn run_command_output(args: &Value) -> Result<super::ToolExecutionOutput, String> {
    run_command_output_inner(args, None, None)
}

pub(crate) fn run_command_output_cancellable(
    args: &Value,
    cancel_token: Option<tokio_util::sync::CancellationToken>,
) -> Result<super::ToolExecutionOutput, String> {
    match run_command_output_inner(args, None, cancel_token) {
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

pub(crate) fn run_command_output_with_progress(
    args: &Value,
    progress: CommandProgressCallback,
) -> Result<super::ToolExecutionOutput, String> {
    run_command_output_inner(args, Some(progress), None)
}

pub(crate) fn run_command_output_with_progress_cancellable(
    args: &Value,
    progress: CommandProgressCallback,
    cancel_token: Option<tokio_util::sync::CancellationToken>,
) -> Result<super::ToolExecutionOutput, String> {
    match run_command_output_inner(args, Some(progress), cancel_token) {
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

fn run_command_output_inner(
    args: &Value,
    progress: Option<CommandProgressCallback>,
    cancel_token: Option<tokio_util::sync::CancellationToken>,
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

    let resolved_cwd = match cwd {
        Some("sandbox") | Some("./sandbox") => {
            if let Some(session_id) = get_active_session_id() {
                crate::config::get_active_session_sandbox_dir(&session_id)
            } else {
                None
            }
        }
        Some(other) => Some(crate::tools::resolve_tool_path(other)),
        None => super::ACTIVE_WORKSPACE_ROOT.with(|root| root.borrow().clone()),
    };

    if let Some(ref cwd_path) = resolved_cwd
        && !cwd_path.is_dir()
    {
        return Err(format!("cwd '{}' is not a directory", cwd_path.display()));
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

    let command_request = rustcode_command::CommandRequest {
        command: command_str.to_owned(),
        cwd: resolved_cwd.clone(),
        env: command_env,
        timeout: Duration::from_millis(timeout_ms.max(1)),
        process_group: true,
    };

    let run_in_bg = args
        .get("background")
        .and_then(parse_json_bool)
        .unwrap_or(false)
        && !is_short_discovery_command(command_str);
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
        task_manager
            .spawn_with_id(
                task_id.clone(),
                TaskSpec::new(SessionId::new(session_id.clone()), command_request),
            )
            .map_err(|error| format!("failed to start background task: {error}"))?;

        return Ok(super::ToolExecutionOutput {
            content: format!(
                "Task started in background. Task ID: {task_id}. Status: Pending. Command: {cmd_str}. You will be notified automatically with the full output when it completes — do NOT poll manage_task for status in a loop; stop calling tools now so execution pauses until completion."
            ),
            success: false,
            pending: true,
            command: Some(cmd_str),
            exit_code: None,
            truncated: false,
            replayed: false,
            error_kind: None,
            retryable: false,
        });
    }

    let cancellation = cancel_token.map(|token| {
        std::sync::Arc::new(move || token.is_cancelled()) as rustcode_command::CancellationCallback
    });
    let output =
        rustcode_command::run_with_timeout_cancellable(&command_request, progress, cancellation)?;
    let exit_code = output.exit_code.unwrap_or(-1);

    let mut result = String::new();
    result.push_str(&format!("exit code: {exit_code}\n"));

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
    Ok(super::ToolExecutionOutput {
        content: result.trim_end().to_string(),
        success: !failed,
        pending: false,
        command: None,
        exit_code: Some(exit_code),
        truncated,
        replayed: false,
        error_kind: failed.then_some(super::ToolErrorKind::CommandFailed),
        retryable: false,
    })
}

pub fn manage_task_tool(args: &Value) -> Result<String, String> {
    let action = args
        .get("action")
        .and_then(|a| a.as_str())
        .ok_or("missing 'action' argument (must be 'list', 'status', or 'kill')")?;

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
                    "- TaskId: {}, PID: {}, Runtime: {}s, Command: {}\n",
                    info.id, pid_str, elapsed, info.command
                ));
            }
            out.push_str("\n(Note: You will be notified automatically with the full output when tasks complete — do NOT poll manage_task for status in a loop; stop calling tools now so execution pauses until completion.)");
            Ok(out.trim_end().to_string())
        }
        "status" => {
            let task_id = args
                .get("task_id")
                .and_then(|t| t.as_str())
                .ok_or("missing 'task_id' argument for status action")?;

            if let Some(info) = tasks.iter().find(|info| info.id.as_str() == task_id) {
                let elapsed = info.started_at.elapsed().as_secs();
                let pid_str = task_pid(info.state)
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| "N/A".to_string());
                Ok(format!(
                    "TaskId: {}, Status: RUNNING, PID: {}, Runtime: {}s, Command: {}\n(Note: You will be notified automatically with the full output when this task completes — do NOT poll manage_task for status in a loop; stop calling tools now so execution pauses until completion.)",
                    task_id, pid_str, elapsed, info.command
                ))
            } else {
                Ok(format!(
                    "TaskId '{task_id}' is not running (finished or cancelled)."
                ))
            }
        }
        "kill" => {
            let task_id = args
                .get("task_id")
                .and_then(|t| t.as_str())
                .ok_or("missing 'task_id' argument for kill action")?;

            match manager.cancel(task_id) {
                CancelResult::Cancelled | CancelResult::Requested => {
                    Ok(format!("Task '{task_id}' terminated successfully."))
                }
                CancelResult::Failed => Err(format!("Failed to terminate task '{task_id}'.")),
                CancelResult::AlreadyFinished | CancelResult::NotFound => {
                    Err(format!("Task '{task_id}' not found."))
                }
            }
        }
        _ => Err(format!(
            "Unknown action '{action}'. Supported actions: list, status, kill."
        )),
    }
}

fn task_pid(state: TaskState) -> Option<u32> {
    match state {
        TaskState::Running { pid } => Some(pid),
        TaskState::Starting | TaskState::CancelRequested => None,
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
pub(crate) struct BackgroundStopResult {
    pub stopped: usize,
    pub failed: usize,
}

pub(crate) fn stop_background_tasks(session_id: &str) -> BackgroundStopResult {
    let summary = background_task_manager().cancel_session(session_id);
    BackgroundStopResult {
        stopped: summary.cancelled + summary.requested,
        failed: summary.failed,
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::terminate_background_pid;
    use super::{
        command_confirmation_preview, command_confirmation_scope, command_output_to_tool_output,
        command_requires_confirmation, has_interactive_sudo, reject_broad_git_stage, run_command,
        run_command_output, run_command_output_cancellable, run_command_output_with_progress,
    };

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
            cwd,
            env: Vec::new(),
            timeout: std::time::Duration::from_secs(5),
            process_group: true,
        }
    }

    #[test]
    fn task_manager_root_adapter_keeps_sessions_isolated() {
        let manager = rustcode_tasks::TaskManager::new(std::sync::Arc::new(|_| true));
        let session_a = manager.subscribe_session("root-session-a");
        let session_b = manager.subscribe_session("root-session-b");
        let first = manager
            .spawn_with_id(
                "root-session-task-a",
                rustcode_tasks::TaskSpec::new(
                    "root-session-a",
                    task_request(
                        if cfg!(target_os = "windows") {
                            "echo a"
                        } else {
                            "printf a"
                        },
                        None,
                    ),
                ),
            )
            .unwrap();
        let second = manager
            .spawn_with_id(
                "root-session-task-b",
                rustcode_tasks::TaskSpec::new(
                    "root-session-b",
                    task_request(
                        if cfg!(target_os = "windows") {
                            "echo b"
                        } else {
                            "printf b"
                        },
                        None,
                    ),
                ),
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
        match &terminal_events[0] {
            rustcode_tasks::TaskEvent::Finished {
                output: Ok(output), ..
            } => {
                let converted = command_output_to_tool_output(task.id().as_str(), output.clone());
                assert!(converted.success);
                assert_eq!(converted.command.as_deref(), Some(task.id().as_str()));
                assert!(converted.content.contains("done"));
            }
            other => panic!("expected one successful completion, got {other:?}"),
        }
    }

    #[test]
    fn run_command_reports_stdout_and_stderr_while_running() {
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
    fn run_command_executes_chained_shell_commands() {
        let result = run_command(&serde_json::json!({
            "command": "printf one; printf two"
        }))
        .expect("shell command should succeed");

        assert!(result.contains("exit code: 0"));
        assert!(result.contains("onetwo"));
    }

    #[test]
    fn run_command_supports_conditional_chaining() {
        let result = run_command(&serde_json::json!({
            "command": "printf first && printf second"
        }))
        .expect("shell command should succeed");

        assert!(result.contains("firstsecond"));
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn run_command_propagates_pipeline_failures() {
        let result = run_command(&serde_json::json!({
            "command": "false | tail -n 1"
        }))
        .expect("run_command reports command failure in its output");

        assert!(result.contains("exit code: 1"), "{result}");
    }

    #[test]
    fn command_execution_metadata_classifies_nonzero_exit_only() {
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

    #[test]
    fn background_command_start_is_pending_and_names_command() {
        let command = "sleep 1; printf background-output";
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

    #[test]
    fn short_discovery_commands_ignore_background_request() {
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
        let command = "sleep 1";
        let output = run_command_output(&serde_json::json!({
            "command": command,
            "background": true,
        }))
        .expect("long command should be accepted");

        assert!(output.pending, "command was forced synchronous: {command}");
        assert_eq!(output.command.as_deref(), Some(command));
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
        let preview = command_confirmation_preview(command);
        assert!(
            preview.contains("resolved command: git status"),
            "preview: {preview}"
        );
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
    fn sed_commands_require_confirmation() {
        for command in [
            "sed -i 's/old/new/' file.txt",
            "sed 'w output.txt' input.txt",
        ] {
            assert!(
                command_requires_confirmation(&serde_json::json!({"command": command})),
                "must confirm potentially mutating sed command: {command}"
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
