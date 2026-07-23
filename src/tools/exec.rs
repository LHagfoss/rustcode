use serde_json::Value;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

// Re-exports needed by exec tools
pub(crate) use super::get_active_session_id;
pub(crate) use super::parse_json_bool;
pub(crate) use super::parse_json_number;
pub(crate) use super::get_background_tasks;
pub(crate) use super::{BackgroundTaskInfo, WAKEUP_CALLBACK};

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

pub fn run_command(args: &Value) -> Result<String, String> {
    let command_str = args
        .get("command")
        .and_then(|c| c.as_str())
        .ok_or("missing 'command' argument")?;

    if is_shell_read_command(command_str) {
        return Err("Do not use run_command with cat, sed, head, tail, or less/more to read files. Use the native 'view_file' tool instead. This keeps token usage low and allows the harness to manage file context correctly.".to_string());
    }

    let trimmed_cmd = command_str.trim();
    if (trimmed_cmd == "sudo" || trimmed_cmd.starts_with("sudo ")) && !trimmed_cmd.contains(" -n ") && !trimmed_cmd.starts_with("sudo -n ") {
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
        Some(other) => Some(PathBuf::from(other)),
        None => None,
    };

    if let Some(ref cwd_path) = resolved_cwd
        && !cwd_path.is_dir() {
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
            if let Some(env_map) = env_clone {
                for (k, v) in env_map {
                    if let Some(val) = v.as_str() {
                        cmd.env(k, val);
                    }
                }
            }

            let result = match cmd.spawn() {
                Ok(child) => {
                    if let Some(pid) = Some(child.id())
                        && let Ok(mut tasks) = get_background_tasks().lock()
                            && let Some(info) = tasks.get_mut(&task_id_clone) {
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
                            if output.status.success() {
                                Ok(full)
                            } else {
                                Err(format!("exit code {:?}\n{}", output.status.code(), full))
                            }
                        }
                        Err(e) => Err(format!("failed to wait: {e}")),
                    }
                }
                Err(e) => Err(format!("failed to spawn: {e}")),
            };

            if let Ok(mut tasks) = get_background_tasks().lock() {
                tasks.remove(&task_id_clone);
            }

            let output_str = match result {
                Ok(out) => out,
                Err(err) => err,
            };

            if let Some(cb) = WAKEUP_CALLBACK.get() {
                cb(session_id, task_id_clone, output_str);
            }
        });

        return Ok(format!(
            "Task started in background. Task ID: {task_id}. Status: Running."
        ));
    }

    let output = run_with_timeout(cmd, Duration::from_millis(timeout_ms.max(1)))?;

    let mut result = String::new();
    result.push_str(&format!(
        "exit code: {}\n",
        output.status.code().unwrap_or(-1)
    ));

    let stdout = truncate_bytes(&output.stdout, MAX_COMMAND_OUTPUT_BYTES);
    let stderr = truncate_bytes(&output.stderr, MAX_COMMAND_OUTPUT_BYTES);

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
    Ok(result.trim_end().to_string())
}

pub fn manage_task_tool(args: &Value) -> Result<String, String> {
    let action = args
        .get("action")
        .and_then(|a| a.as_str())
        .ok_or("missing 'action' argument (must be 'list', 'status', or 'kill')")?;

    let tasks_lock = get_background_tasks();
    let mut tasks = tasks_lock.lock().map_err(|e| format!("failed to lock background tasks: {e}"))?;

    match action {
        "list" => {
            if tasks.is_empty() {
                return Ok("No running background tasks.".to_string());
            }
            let mut out = String::from("Running background tasks:\n");
            for (id, info) in tasks.iter() {
                let elapsed = info.start_time.elapsed().as_secs();
                let pid_str = info.child_pid.map(|p| p.to_string()).unwrap_or_else(|| "N/A".to_string());
                out.push_str(&format!(
                    "- TaskId: {}, PID: {}, Runtime: {}s, Command: {}\n",
                    id, pid_str, elapsed, info.command
                ));
            }
            Ok(out.trim_end().to_string())
        }
        "status" => {
            let task_id = args
                .get("task_id")
                .and_then(|t| t.as_str())
                .ok_or("missing 'task_id' argument for status action")?;

            if let Some(info) = tasks.get(task_id) {
                let elapsed = info.start_time.elapsed().as_secs();
                let pid_str = info.child_pid.map(|p| p.to_string()).unwrap_or_else(|| "N/A".to_string());
                Ok(format!(
                    "TaskId: {}, Status: RUNNING, PID: {}, Runtime: {}s, Command: {}",
                    task_id, pid_str, elapsed, info.command
                ))
            } else {
                Ok(format!("TaskId '{task_id}' is not running (finished or cancelled)."))
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
        _ => Err(format!("Unknown action '{action}'. Supported actions: list, status, kill.")),
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

fn truncate_bytes(bytes: &[u8], max: usize) -> String {
    if bytes.len() <= max {
        return String::from_utf8_lossy(bytes).to_string();
    }
    let head_end = (max * 3 / 4).min(bytes.len());
    let head = &bytes[..head_end];
    let mut out = String::from_utf8_lossy(head).to_string();
    out.push_str(&format!(
        "\n... (truncated, {} bytes total — showing first {} bytes)\n",
        bytes.len(),
        head_end
    ));
    out
}
