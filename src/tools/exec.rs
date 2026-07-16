use serde_json::Value;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

// Re-exports needed by exec tools
pub(crate) use super::get_active_session_id;
pub(crate) use super::parse_json_bool;
pub(crate) use super::parse_json_number;
pub(crate) use super::register_wakeup_callback;
pub(crate) use super::{BackgroundTaskInfo, WAKEUP_CALLBACK};

const MAX_COMMAND_OUTPUT_BYTES: usize = 100_000;
const DEFAULT_COMMAND_TIMEOUT_MS: u64 = 120_000;

pub fn run_command(args: &Value) -> Result<String, String> {
    let command_str = args
        .get("command")
        .and_then(|c| c.as_str())
        .ok_or("missing 'command' argument")?;
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

    if let Some(ref cwd_path) = resolved_cwd {
        if !cwd_path.is_dir() {
            return Err(format!("cwd '{}' is not a directory", cwd_path.display()));
        }
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

        if let Some(mut tasks) = get_background_tasks().lock().ok() {
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
                    if let Some(pid) = Some(child.id()) {
                        if let Some(mut tasks) = get_background_tasks().lock().ok() {
                            if let Some(info) = tasks.get_mut(&task_id_clone) {
                                info.child_pid = Some(pid);
                            }
                        }
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

            if let Some(mut tasks) = get_background_tasks().lock().ok() {
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
