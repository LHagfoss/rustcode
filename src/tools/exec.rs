use serde_json::Value;
use std::io::Read;
use std::process::{Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

// Re-exports needed by exec tools
pub(crate) use super::get_active_session_id;
pub(crate) use super::get_background_tasks;
pub(crate) use super::parse_json_bool;
pub(crate) use super::parse_json_number;
pub(crate) use super::{BackgroundTaskInfo, WAKEUP_CALLBACK};

use super::{Tool, ToolCapability, ToolSafety};

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
    description: "Run one command through the platform shell and return stdout/stderr and the exit code. The command may use normal shell syntax, including ';' or '&&' to chain commands, pipes, redirects, and environment assignments. Supports an optional working directory, environment overrides, timeout (default 120s), and background execution ('background': true). Note: Interactive 'sudo' requiring passwords is disabled; use non-privileged commands or 'sudo -n'.",
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
    // NOTE: `Unknown` looks unintended (this is a process-control tool), but it
    // preserves the pre-refactor behavior: `authorize_tool` requires
    // confirmation for `Unknown` tools despite `requires_confirmation: false`.
    safety: ToolSafety::Unknown,
};

const MAX_COMMAND_OUTPUT_BYTES: usize = 100_000;
const DEFAULT_COMMAND_TIMEOUT_MS: u64 = 120_000;

fn is_shell_read_command(cmd: &str) -> bool {
    let trimmed = cmd.trim();
    let parts: Vec<&str> = trimmed.split_whitespace().collect();
    if parts.is_empty() {
        return false;
    }
    let binary = parts[0];
    if matches!(binary, "cat" | "sed" | "head" | "tail" | "less" | "more") {
        if trimmed.contains('>') || trimmed.contains("<<") || trimmed.contains('|') {
            return false;
        }
        return true;
    }
    false
}

/// Short sudo options that consume a value (either glued to the flag or as the
/// following token), e.g. `-u root`, `-p "prompt"`.
const SUDO_SHORT_OPTS_WITH_VALUE: &str = "CghpRTtUu";
/// Long sudo options that consume a value when not written as `--opt=value`.
const SUDO_LONG_OPTS_WITH_VALUE: &[&str] = &[
    "close-from",
    "group",
    "host",
    "prompt",
    "chroot",
    "command-timeout",
    "type",
    "other-user",
    "user",
    "role",
];

/// Split a command line into the individual simple commands it may run.
///
/// This deliberately is not a shell parser: it only breaks on the separators
/// that can introduce a new command (`;`, `&&`, `||`, `|`, `&`, newline) and on
/// command-substitution / grouping boundaries (`$(`, `)`, backtick, `{`, `}`),
/// so that each resulting segment can be inspected for its own first token.
/// Splitting too eagerly (e.g. on a separator inside quotes) only makes the
/// sudo guard more conservative, never less.
fn split_command_segments(cmd: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    for ch in cmd.chars() {
        match ch {
            ';' | '\n' | '|' | '&' | '`' | '(' | ')' | '{' | '}' => {
                segments.push(std::mem::take(&mut current));
            }
            _ => current.push(ch),
        }
    }
    segments.push(current);
    segments
}

fn git_subcommand<'a>(tokens: &'a [&'a str]) -> Option<(&'a str, usize)> {
    let first = tokens.first()?.rsplit(['/', '\\']).next()?;
    if first != "git" {
        return None;
    }

    let mut index = 1;
    while index < tokens.len() {
        let token = tokens[index];
        if token == "--" {
            return tokens
                .get(index + 1)
                .copied()
                .map(|subcommand| (subcommand, index + 1));
        }
        if !token.starts_with('-') {
            return Some((token, index));
        }
        if matches!(
            token,
            "-C" | "-c"
                | "--git-dir"
                | "--work-tree"
                | "--namespace"
                | "--exec-path"
                | "--config"
                | "--super-prefix"
        ) && !token.contains('=')
        {
            index += 2;
        } else {
            index += 1;
        }
    }
    None
}

fn has_force_flag(tokens: &[&str]) -> bool {
    tokens.iter().any(|token| {
        *token == "-f" || *token == "-ff" || *token == "-D" || token.starts_with("--force")
    })
}

fn destructive_git_scope(segment: &str) -> Option<String> {
    let tokens = segment.split_whitespace().collect::<Vec<_>>();
    let (subcommand, subcommand_index) = git_subcommand(&tokens)?;
    let arguments = &tokens[subcommand_index + 1..];
    if has_force_flag(arguments) {
        return Some(format!("git {subcommand} force operation"));
    }

    let scope = match subcommand {
        "restore" => "working-tree or index paths",
        "checkout" => "checked-out paths or branch state",
        "reset" => "HEAD, index, and possibly working-tree paths",
        "clean" => "untracked files and directories",
        "branch"
            if arguments
                .iter()
                .any(|arg| *arg == "-d" || *arg == "--delete") =>
        {
            "deleted local branch"
        }
        _ => return None,
    };
    Some(format!("git {subcommand}: {scope}"))
}

fn is_read_only_git(tokens: &[&str]) -> bool {
    let Some((subcommand, subcommand_index)) = git_subcommand(tokens) else {
        return false;
    };
    if destructive_git_scope(&tokens.join(" ")).is_some() {
        return false;
    }

    let arguments = &tokens[subcommand_index + 1..];
    if arguments.iter().any(|argument| {
        *argument == "-o"
            || *argument == "--output"
            || argument.starts_with("--output=")
            || *argument == "--ext-diff"
    }) {
        return false;
    }

    matches!(
        subcommand,
        "status" | "diff" | "log" | "show" | "rev-parse" | "describe"
    ) || (subcommand == "branch"
        && arguments.iter().all(|argument| {
            matches!(
                *argument,
                "-a" | "--all" | "-r" | "--remotes" | "-v" | "--verbose" | "--show-current"
            )
        }))
}

fn is_read_only_gh(tokens: &[&str]) -> bool {
    let Some(binary) = tokens.first() else {
        return true;
    };
    if binary.rsplit(['/', '\\']).next() != Some("gh") {
        return false;
    }

    match tokens.get(1..).unwrap_or_default() {
        ["help", ..] | ["--help", ..] | ["-h", ..] => true,
        ["auth", "status", ..] | ["auth", "help", ..] => true,
        ["issue", "list", ..] | ["issue", "view", ..] => true,
        ["pr", "list", ..] | ["pr", "view", ..] => true,
        _ => false,
    }
}

fn is_read_only_segment(segment: &str) -> bool {
    let tokens = segment.split_whitespace().collect::<Vec<_>>();
    let Some(binary) = tokens.first().map(|token| token.rsplit(['/', '\\']).next()) else {
        return true;
    };

    match binary {
        Some("git") => is_read_only_git(&tokens),
        Some("gh") => is_read_only_gh(&tokens),
        Some("command") => tokens.get(1) == Some(&"-v"),
        Some("find") => !tokens[1..].iter().any(|argument| {
            matches!(
                *argument,
                "-delete"
                    | "-exec"
                    | "-execdir"
                    | "-ok"
                    | "-okdir"
                    | "-fprint"
                    | "-fprintf"
                    | "-fls"
            )
        }),
        Some(
            "date" | "echo" | "false" | "grep" | "ls" | "printf" | "pwd" | "rg" | "test" | "true"
            | "type" | "uname" | "which",
        ) => true,
        _ => false,
    }
}

pub(crate) fn command_confirmation_scope(command: &str) -> Option<String> {
    let segments = split_command_segments(command);
    let git_scopes = segments
        .iter()
        .filter_map(|segment| destructive_git_scope(segment))
        .collect::<Vec<_>>();
    if !git_scopes.is_empty() {
        return Some(git_scopes.join("; "));
    }
    if command
        .chars()
        .any(|character| matches!(character, '<' | '>'))
    {
        return Some("shell redirection".to_string());
    }
    if segments.iter().all(|segment| is_read_only_segment(segment)) {
        None
    } else {
        Some("unclassified or potentially mutating shell command".to_string())
    }
}

pub(crate) fn command_requires_confirmation(args: &Value) -> bool {
    args.get("command")
        .and_then(Value::as_str)
        .map(|command| command_confirmation_scope(command).is_some())
        .unwrap_or(true)
}

pub(crate) fn command_confirmation_preview(command: &str) -> String {
    let scope = command_confirmation_scope(command).unwrap_or("command execution".to_string());
    format!("resolved command: {command}\nscope: {scope}")
}

fn is_sudo_binary(token: &str) -> bool {
    token
        .rsplit(['/', '\\'])
        .next()
        .is_some_and(|name| name == "sudo")
}

/// Decide whether a single simple command is an interactive `sudo` invocation.
///
/// Returns `true` only when the segment's *first* token is `sudo` and its own
/// option list either fails to pass `-n`/`--non-interactive` or asks sudo to
/// read the password from stdin via `-S`/`--stdin`. Options belonging to the
/// sub-command (`sudo grep -n foo`) are never consulted: scanning stops at the
/// first non-option token.
fn segment_is_interactive_sudo(segment: &str) -> bool {
    let mut tokens = segment.split_whitespace();
    let Some(first) = tokens.next() else {
        return false;
    };
    if !is_sudo_binary(first) {
        return false;
    }

    let mut non_interactive = false;
    let mut reads_stdin = false;

    while let Some(token) = tokens.next() {
        if token == "--" {
            break;
        }
        if let Some(long) = token.strip_prefix("--") {
            let name = long.split('=').next().unwrap_or(long);
            match name {
                "non-interactive" => non_interactive = true,
                "stdin" => reads_stdin = true,
                _ => {
                    if SUDO_LONG_OPTS_WITH_VALUE.contains(&name) && !long.contains('=') {
                        tokens.next();
                    }
                }
            }
            continue;
        }
        if let Some(short) = token.strip_prefix('-')
            && !short.is_empty()
        {
            let mut chars = short.chars();
            while let Some(ch) = chars.next() {
                match ch {
                    'n' => non_interactive = true,
                    'S' => reads_stdin = true,
                    c if SUDO_SHORT_OPTS_WITH_VALUE.contains(c) => {
                        // The value is either glued to the flag (`-uroot`) or is
                        // the next token (`-u root`); either way this bundle ends.
                        if chars.next().is_none() {
                            tokens.next();
                        }
                        break;
                    }
                    _ => {}
                }
            }
            continue;
        }
        // First non-option token: everything after this belongs to the
        // sub-command sudo is about to run.
        break;
    }

    reads_stdin || !non_interactive
}

/// True when any command in `cmd` is a `sudo` invocation that could prompt for
/// (or be fed) a password.
fn has_interactive_sudo(cmd: &str) -> bool {
    split_command_segments(cmd)
        .iter()
        .any(|segment| segment_is_interactive_sudo(segment))
}

pub(crate) fn reject_broad_git_stage(cmd: &str) -> Option<&'static str> {
    for segment in split_command_segments(cmd) {
        let tokens = segment.split_whitespace().collect::<Vec<_>>();
        if tokens.len() >= 3
            && tokens[0] == "git"
            && tokens[1] == "commit"
            && tokens[2..]
                .iter()
                .any(|token| *token == "-a" || *token == "--all")
        {
            return Some(
                "Refusing `git commit -a/--all`. Stage explicit feature paths first so unrelated user changes cannot enter the commit.",
            );
        }
        if tokens.len() >= 3
            && tokens[0] == "git"
            && tokens[1] == "add"
            && (tokens[2] == "."
                || tokens[2] == "-A"
                || tokens[2] == "--all"
                || (tokens[2] == "--" && tokens.get(3) == Some(&".")))
        {
            return Some(
                "Refusing broad git staging. Stage explicit feature paths (for example, `git add src/network.rs`) so unrelated user changes cannot enter the commit.",
            );
        }
    }
    None
}

pub fn run_command(args: &Value) -> Result<String, String> {
    run_command_output(args).map(|output| output.content)
}

pub(super) fn run_command_output(args: &Value) -> Result<super::ToolExecutionOutput, String> {
    let command_str = args
        .get("command")
        .and_then(|c| c.as_str())
        .ok_or("missing 'command' argument")?;

    if is_shell_read_command(command_str) {
        return Err("Do not use run_command with cat, sed, head, tail, or less/more to read files. Use the native 'view_file' tool instead. This keeps token usage low and allows the harness to manage file context correctly.".to_string());
    }

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

    let mut cmd = if cfg!(target_os = "windows") {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", command_str]);
        c
    } else {
        let mut c = std::process::Command::new("sh");
        c.args(["-c", command_str]);
        c
    };

    cmd.stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());
    if let Some(ref cwd_path) = resolved_cwd {
        cmd.current_dir(cwd_path);
    }
    // GUI/Dock launches don't inherit the shell PATH, so agent-run builds/tests
    // (cargo, npm, …) fail to find their toolchain. Seed a toolchain-aware PATH;
    // an explicit PATH in `env` below still overrides it.
    cmd.env("PATH", crate::network::augmented_path());
    if let Some(env_map) = env {
        for (k, v) in env_map {
            if let Some(val) = v.as_str() {
                cmd.env(k, val);
            }
        }
    }

    let run_in_bg = args
        .get("background")
        .and_then(parse_json_bool)
        .unwrap_or(false);
    if run_in_bg {
        let session_id = get_active_session_id().unwrap_or_default();
        let cmd_str = command_str.to_string();
        let task_id = format!(
            "task_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or(std::time::Duration::from_secs(0))
                .as_millis()
        );

        let resolved_cwd_clone = resolved_cwd.clone();
        let env_clone = env.cloned();
        let task_id_clone = task_id.clone();

        if let Ok(mut tasks) = get_background_tasks().lock() {
            tasks.insert(
                task_id.clone(),
                BackgroundTaskInfo {
                    id: task_id.clone(),
                    command: cmd_str.clone(),
                    start_time: std::time::Instant::now(),
                    child_pid: None,
                    cancel_sender: None,
                },
            );
        }

        std::thread::spawn(move || {
            let mut cmd = if cfg!(target_os = "windows") {
                let mut c = std::process::Command::new("cmd");
                c.args(["/C", &cmd_str]);
                c
            } else {
                let mut c = std::process::Command::new("sh");
                c.args(["-c", &cmd_str]);
                c
            };

            cmd.stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .stdin(Stdio::null());

            if let Some(ref cwd_path) = resolved_cwd_clone {
                cmd.current_dir(cwd_path);
            }
            cmd.env("PATH", crate::network::augmented_path());
            if let Some(env_map) = env_clone {
                for (k, v) in env_map {
                    if let Some(val) = v.as_str() {
                        cmd.env(k, val);
                    }
                }
            }

            let output = match cmd.spawn() {
                Ok(child) => {
                    if let Some(pid) = Some(child.id())
                        && let Ok(mut tasks) = get_background_tasks().lock()
                        && let Some(info) = tasks.get_mut(&task_id_clone)
                    {
                        info.child_pid = Some(pid);
                    }

                    match child.wait_with_output() {
                        Ok(output) => {
                            let out_str = String::from_utf8_lossy(&output.stdout).to_string();
                            let err_str = String::from_utf8_lossy(&output.stderr).to_string();
                            let mut full = out_str;
                            if !err_str.is_empty() {
                                if !full.is_empty() {
                                    full.push('\n');
                                }
                                full.push_str("stderr:\n");
                                full.push_str(&err_str);
                            }
                            let success = output.status.success();
                            let exit_code = output.status.code();
                            if !success {
                                full = format!("exit code {exit_code:?}\n{full}");
                            }
                            super::ToolExecutionOutput {
                                content: full,
                                success,
                                exit_code,
                                truncated: false,
                                replayed: false,
                                error_kind: (!success).then_some(super::ToolErrorKind::CommandFailed),
                                retryable: false,
                            }
                        }
                        Err(e) => {
                            super::ToolExecutionOutput::failure(format!("failed to wait: {e}"))
                        }
                    }
                }
                Err(e) => super::ToolExecutionOutput::failure(format!("failed to spawn: {e}")),
            };

            if let Ok(mut tasks) = get_background_tasks().lock() {
                tasks.remove(&task_id_clone);
            }

            if let Some(cb) = WAKEUP_CALLBACK.get() {
                cb(session_id, task_id_clone, output);
            }
        });

        return Ok(super::ToolExecutionOutput {
            content: format!(
                "Task started in background. Task ID: {task_id}. Status: Running. You will be notified automatically with the full output when it completes — do NOT poll manage_task for status in a loop; stop calling tools now so execution pauses until completion."
            ),
            success: true,
            exit_code: None,
            truncated: false,
            replayed: false,
            error_kind: None,
            retryable: false,
        });
    }

    let output = run_with_timeout(cmd, Duration::from_millis(timeout_ms.max(1)))?;
    let exit_code = output.status.code().unwrap_or(-1);

    let mut result = String::new();
    result.push_str(&format!("exit code: {exit_code}\n"));

    let failed = !output.status.success();
    let truncated = output.stdout.len() > MAX_COMMAND_OUTPUT_BYTES
        || output.stderr.len() > MAX_COMMAND_OUTPUT_BYTES;
    let stdout = truncate_bytes(&output.stdout, MAX_COMMAND_OUTPUT_BYTES, failed);
    let stderr = truncate_bytes(&output.stderr, MAX_COMMAND_OUTPUT_BYTES, failed);

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

    let tasks_lock = get_background_tasks();
    let mut tasks = tasks_lock
        .lock()
        .map_err(|e| format!("failed to lock background tasks: {e}"))?;

    match action {
        "list" => {
            if tasks.is_empty() {
                return Ok("No running background tasks.".to_string());
            }
            let mut out = String::from("Running background tasks:\n");
            for (id, info) in tasks.iter() {
                let elapsed = info.start_time.elapsed().as_secs();
                let pid_str = info
                    .child_pid
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| "N/A".to_string());
                out.push_str(&format!(
                    "- TaskId: {}, PID: {}, Runtime: {}s, Command: {}\n",
                    id, pid_str, elapsed, info.command
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

            if let Some(info) = tasks.get(task_id) {
                let elapsed = info.start_time.elapsed().as_secs();
                let pid_str = info
                    .child_pid
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

            if let Some(info) = tasks.remove(task_id) {
                if let Some(pid) = info.child_pid {
                    #[cfg(target_os = "windows")]
                    let _ = std::process::Command::new("taskkill")
                        .args(["/F", "/PID", &pid.to_string()])
                        .output();
                    #[cfg(not(target_os = "windows"))]
                    let _ = std::process::Command::new("kill")
                        .args(["-9", &pid.to_string()])
                        .output();
                }
                Ok(format!("Task '{task_id}' terminated successfully."))
            } else {
                Err(format!("Task '{task_id}' not found."))
            }
        }
        _ => Err(format!(
            "Unknown action '{action}'. Supported actions: list, status, kill."
        )),
    }
}

fn run_with_timeout(mut cmd: std::process::Command, timeout: Duration) -> Result<Output, String> {
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to spawn process: {e}"))?;
    let mut child_stdout = child.stdout.take().ok_or("no stdout pipe")?;
    let mut child_stderr = child.stderr.take().ok_or("no stderr pipe")?;

    let out_handle = thread::spawn(move || {
        let mut b = Vec::new();
        let _ = child_stdout.read_to_end(&mut b);
        b
    });
    let err_handle = thread::spawn(move || {
        let mut b = Vec::new();
        let _ = child_stderr.read_to_end(&mut b);
        b
    });

    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "command timed out after {} ms and was killed",
                        timeout.as_millis()
                    ));
                }
                thread::sleep(Duration::from_millis(20));
            }
            Err(e) => return Err(format!("failed to wait on process: {e}")),
        }
    };

    let stdout_bytes = out_handle.join().unwrap_or_default();
    let stderr_bytes = err_handle.join().unwrap_or_default();

    Ok(Output {
        status,
        stdout: stdout_bytes,
        stderr: stderr_bytes,
    })
}

/// Truncates to at most `max` bytes, keeping both head and tail rather than
/// dropping the tail wholesale — compiler errors, test failures, and stack
/// traces overwhelmingly appear at the *end* of command output, so a
/// head-only cut can throw away the only useful part of a failing command's
/// output before it ever reaches the model. `keep_tail_priority` (set for a
/// nonzero exit code) biases the split further toward the tail.
fn truncate_bytes(bytes: &[u8], max: usize, keep_tail_priority: bool) -> String {
    if bytes.len() <= max {
        return String::from_utf8_lossy(bytes).to_string();
    }
    let tail_len = if keep_tail_priority {
        max * 7 / 10
    } else {
        max * 3 / 10
    };
    let head_len = max - tail_len;
    let head = String::from_utf8_lossy(&bytes[..head_len]);
    let tail = String::from_utf8_lossy(&bytes[bytes.len() - tail_len..]);
    format!(
        "{head}\n... (truncated, {} bytes total — showing first {head_len} and last {tail_len} bytes) ...\n{tail}\n",
        bytes.len()
    )
}

#[cfg(test)]
mod tests {
    use super::{
        command_confirmation_preview, command_confirmation_scope, command_requires_confirmation,
        has_interactive_sudo, reject_broad_git_stage, run_command, run_command_output,
    };

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
}
