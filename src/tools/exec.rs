use serde_json::Value;
use std::collections::VecDeque;
use std::io::Read;
use std::process::{Child, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

// Re-exports needed by exec tools
pub(crate) use super::get_active_session_id;
pub(crate) use super::get_background_tasks;
pub(crate) use super::parse_json_bool;
pub(crate) use super::parse_json_number;
pub(crate) use super::{BackgroundTaskInfo, WAKEUP_CALLBACK};

use super::{Tool, ToolCapability, ToolSafety};

static BACKGROUND_TASK_SEQUENCE: AtomicU64 = AtomicU64::new(1);

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
    safety: ToolSafety::ProcessControl,
};

const MAX_COMMAND_OUTPUT_BYTES: usize = 100_000;
const DEFAULT_COMMAND_TIMEOUT_MS: u64 = 120_000;
const CAPTURE_HEAD_BYTES: usize = MAX_COMMAND_OUTPUT_BYTES * 3 / 10;
const CAPTURE_TAIL_BYTES: usize = MAX_COMMAND_OUTPUT_BYTES - CAPTURE_HEAD_BYTES;

struct BoundedOutput {
    head: Vec<u8>,
    tail: VecDeque<u8>,
    total_bytes: usize,
}

impl BoundedOutput {
    fn push(&mut self, bytes: &[u8]) {
        self.total_bytes = self.total_bytes.saturating_add(bytes.len());

        let head_remaining = CAPTURE_HEAD_BYTES.saturating_sub(self.head.len());
        let head_len = head_remaining.min(bytes.len());
        self.head.extend_from_slice(&bytes[..head_len]);

        for byte in &bytes[head_len..] {
            if self.tail.len() == CAPTURE_TAIL_BYTES {
                self.tail.pop_front();
            }
            self.tail.push_back(*byte);
        }
    }

    fn captured_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.head.len() + self.tail.len());
        bytes.extend_from_slice(&self.head);
        bytes.extend(self.tail.iter().copied());
        bytes
    }

    fn captured_len(&self) -> usize {
        self.head.len() + self.tail.len()
    }

    fn is_truncated(&self) -> bool {
        self.total_bytes > MAX_COMMAND_OUTPUT_BYTES
    }
}

impl Default for BoundedOutput {
    fn default() -> Self {
        Self {
            head: Vec::with_capacity(CAPTURE_HEAD_BYTES),
            tail: VecDeque::with_capacity(CAPTURE_TAIL_BYTES),
            total_bytes: 0,
        }
    }
}

struct CommandOutput {
    status: ExitStatus,
    stdout: BoundedOutput,
    stderr: BoundedOutput,
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
            "cat" | "date" | "echo" | "false" | "grep" | "head" | "less" | "ls" | "more" | "printf"
            | "pwd" | "rg" | "stat" | "tail" | "test" | "true" | "type" | "uname" | "which",
        ) => true,
        Some("npm") => matches!(
            tokens.get(1..).unwrap_or_default(),
            ["config", "get", key] if !key.starts_with('-')
        ),
        _ => false,
    }
}

/// Return whether a command is small enough to run inline even when the model
/// requests background execution. This is deliberately narrower than the
/// read-only authorization classifier: a read-only command can still walk a
/// large tree or produce unbounded output.
fn is_short_discovery_command(command: &str) -> bool {
    let command = command.trim();
    if command.is_empty()
        || command
            .chars()
            .any(|character| matches!(character, ';' | '\n' | '|' | '&' | '<' | '>' | '`' | '$'))
    {
        return false;
    }

    let tokens = command.split_whitespace().collect::<Vec<_>>();
    if tokens.is_empty() || tokens.len() > 8 || !is_read_only_segment(command) {
        return false;
    }

    let binary = tokens[0].rsplit(['/', '\\']).next().unwrap_or(tokens[0]);
    let arguments = &tokens[1..];
    match binary {
        "find" => {
            let path = arguments.first().copied().unwrap_or("");
            let bounded_depth = arguments.windows(2).any(|window| {
                window[0] == "-maxdepth" && window[1].parse::<u8>().is_ok_and(|depth| depth <= 3)
            });
            !path.starts_with('/') && (path != "." || bounded_depth)
        }
        "ls" | "rg" | "stat" => !arguments
            .iter()
            .any(|argument| *argument == "/" || argument.starts_with('/')),
        _ => true,
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
    run_command_output_inner(args, None)
}

pub(crate) type CommandProgressCallback = Arc<dyn Fn(&[u8], bool) + Send + Sync + 'static>;

pub(crate) fn run_command_output_with_progress(
    args: &Value,
    progress: CommandProgressCallback,
) -> Result<super::ToolExecutionOutput, String> {
    run_command_output_inner(args, Some(progress))
}

fn run_command_output_inner(
    args: &Value,
    progress: Option<CommandProgressCallback>,
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

        let resolved_cwd_clone = resolved_cwd.clone();
        let env_clone = env.cloned();
        let task_id_clone = task_id.clone();
        let command_for_thread = cmd_str.clone();
        let command_for_output = cmd_str.clone();

        if let Ok(mut tasks) = get_background_tasks().lock() {
            tasks.insert(
                task_id.clone(),
                BackgroundTaskInfo {
                    id: task_id.clone(),
                    session_id: session_id.clone(),
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
                c.args(["/C", &command_for_thread]);
                c
            } else {
                let mut c = std::process::Command::new("sh");
                c.args(["-c", &command_for_thread]);
                #[cfg(unix)]
                {
                    use std::os::unix::process::CommandExt;
                    c.process_group(0);
                }
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

            let mut output = match cmd.spawn() {
                Ok(child) => {
                    if let Some(pid) = Some(child.id())
                        && let Ok(mut tasks) = get_background_tasks().lock()
                        && let Some(info) = tasks.get_mut(&task_id_clone)
                    {
                        info.child_pid = Some(pid);
                    }

                    if super::background_task_cancelled(&task_id_clone) {
                        terminate_background_pid(child.id());
                    }

                    match wait_with_bounded_output(child) {
                        Ok(output) => {
                            let out_str = format_bounded_output(&output.stdout);
                            let err_str = format_bounded_output(&output.stderr);
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
                                pending: false,
                                command: Some(command_for_output.clone()),
                                exit_code,
                                truncated: output.stdout.is_truncated()
                                    || output.stderr.is_truncated(),
                                replayed: false,
                                error_kind: (!success)
                                    .then_some(super::ToolErrorKind::CommandFailed),
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
            output.command = Some(command_for_thread);

            if let Ok(mut tasks) = get_background_tasks().lock() {
                tasks.remove(&task_id_clone);
            }

            let cancelled = super::take_background_task_cancelled(&task_id_clone);
            if !cancelled && let Some(cb) = WAKEUP_CALLBACK.get() {
                cb(session_id, task_id_clone, output);
            }
        });

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

    let output = run_with_timeout(cmd, Duration::from_millis(timeout_ms.max(1)), progress)?;
    let exit_code = output.status.code().unwrap_or(-1);

    let mut result = String::new();
    result.push_str(&format!("exit code: {exit_code}\n"));

    let failed = !output.status.success();
    let truncated = output.stdout.is_truncated() || output.stderr.is_truncated();
    let stdout = format_bounded_output(&output.stdout);
    let stderr = format_bounded_output(&output.stderr);

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
                super::mark_background_task_cancelled(task_id);
                if let Some(pid) = info.child_pid
                    && !terminate_background_pid(pid)
                {
                    super::clear_background_task_cancelled(task_id);
                    tasks.insert(task_id.to_owned(), info);
                    return Err(format!("Failed to terminate task '{task_id}'."));
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
    let candidates = {
        let Ok(mut tasks) = get_background_tasks().lock() else {
            return BackgroundStopResult {
                stopped: 0,
                failed: 1,
            };
        };
        let task_ids = tasks
            .iter()
            .filter(|(_, task)| task.session_id == session_id)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for task_id in &task_ids {
            super::mark_background_task_cancelled(task_id);
        }
        task_ids
            .into_iter()
            .filter_map(|id| tasks.remove(&id).map(|task| (id, task)))
            .collect::<Vec<_>>()
    };

    let mut result = BackgroundStopResult::default();
    for (task_id, task) in candidates {
        let terminated = task.child_pid.is_none_or(terminate_background_pid);
        if terminated {
            result.stopped += 1;
        } else {
            result.failed += 1;
            super::clear_background_task_cancelled(&task_id);
            if let Ok(mut tasks) = get_background_tasks().lock() {
                tasks.insert(task_id, task);
            }
        }
    }
    result
}

fn spawn_output_reader<R: Read + Send + 'static>(
    mut reader: R,
    progress: Option<CommandProgressCallback>,
    is_stderr: bool,
) -> thread::JoinHandle<BoundedOutput> {
    thread::spawn(move || {
        let mut output = BoundedOutput::default();
        let mut chunk = [0u8; 4096];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    if let Some(callback) = progress.as_ref() {
                        callback(&chunk[..read], is_stderr);
                    }
                    output.push(&chunk[..read]);
                }
            }
        }
        output
    })
}

fn wait_with_bounded_output(mut child: Child) -> Result<CommandOutput, String> {
    let child_stdout = child.stdout.take().ok_or("no stdout pipe")?;
    let child_stderr = child.stderr.take().ok_or("no stderr pipe")?;
    let out_handle = spawn_output_reader(child_stdout, None, false);
    let err_handle = spawn_output_reader(child_stderr, None, true);
    let status = child
        .wait()
        .map_err(|e| format!("failed to wait on process: {e}"))?;
    let stdout = out_handle.join().unwrap_or_default();
    let stderr = err_handle.join().unwrap_or_default();
    Ok(CommandOutput {
        status,
        stdout,
        stderr,
    })
}

fn run_with_timeout(
    mut cmd: std::process::Command,
    timeout: Duration,
    progress: Option<CommandProgressCallback>,
) -> Result<CommandOutput, String> {
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to spawn process: {e}"))?;
    let child_stdout = child.stdout.take().ok_or("no stdout pipe")?;
    let child_stderr = child.stderr.take().ok_or("no stderr pipe")?;

    let stdout_progress = progress.clone();
    let out_handle = spawn_output_reader(child_stdout, stdout_progress, false);
    let stderr_progress = progress;
    let err_handle = spawn_output_reader(child_stderr, stderr_progress, true);

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

    let stdout = out_handle.join().unwrap_or_default();
    let stderr = err_handle.join().unwrap_or_default();

    Ok(CommandOutput {
        status,
        stdout,
        stderr,
    })
}

/// Truncates to at most `max` bytes, keeping both head and tail rather than
/// dropping the tail wholesale — compiler errors, test failures, and stack
/// traces overwhelmingly appear at the *end* of command output, so a
/// head-only cut can throw away the only useful part of a failing command's
/// output before it ever reaches the model. `keep_tail_priority` (set for a
/// nonzero exit code) biases the split further toward the tail.
fn format_bounded_output(output: &BoundedOutput) -> String {
    let bytes = output.captured_bytes();
    if !output.is_truncated() {
        return String::from_utf8_lossy(&bytes).to_string();
    }
    let head_len = output.head.len();
    let tail_len = output.tail.len();
    let head = String::from_utf8_lossy(&bytes[..head_len]);
    let tail = String::from_utf8_lossy(&bytes[head_len..]);
    format!(
        "{head}\n... (truncated, {} bytes total — showing first {head_len} and last {tail_len} bytes) ...\n{tail}\n",
        output.total_bytes
    )
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::terminate_background_pid;
    use super::{
        command_confirmation_preview, command_confirmation_scope, command_requires_confirmation,
        has_interactive_sudo, reject_broad_git_stage, run_command, run_command_output,
        run_command_output_with_progress,
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
    fn command_output_capture_is_bounded_before_process_exit() {
        let output = super::run_with_timeout(
            {
                let mut command = std::process::Command::new("sh");
                command.args(["-c", "head -c 200000 /dev/zero"]);
                command.stdout(std::process::Stdio::piped());
                command.stderr(std::process::Stdio::piped());
                command
            },
            std::time::Duration::from_secs(5),
            None,
        )
        .expect("command output");

        assert!(output.stdout.captured_len() <= super::MAX_COMMAND_OUTPUT_BYTES);
        assert!(output.stderr.captured_len() <= super::MAX_COMMAND_OUTPUT_BYTES);
        assert!(output.stdout.is_truncated());
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
