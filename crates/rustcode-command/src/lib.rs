//! Dependency-neutral foreground shell execution.
//!
//! This crate owns only process lifecycle and bounded stdout/stderr capture.
//! Tool schemas, command authorization, workspace path resolution, and
//! background task orchestration stay in the application crate.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

/// Maximum bytes retained for each output stream.
pub const MAX_OUTPUT_BYTES: usize = 100_000;
const CAPTURE_HEAD_BYTES: usize = MAX_OUTPUT_BYTES * 3 / 10;
const CAPTURE_TAIL_BYTES: usize = MAX_OUTPUT_BYTES - CAPTURE_HEAD_BYTES;

/// A fully resolved command request. Callers resolve aliases such as
/// `sandbox` and inject the effective environment before crossing this seam.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandRequest {
    pub command: String,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(OsString, OsString)>,
    pub timeout: Duration,
    /// Background callers request a process group so they can terminate the
    /// shell and its descendants from the application-owned task manager.
    pub process_group: bool,
}

/// Callback invoked as bytes arrive from stdout or stderr.
pub type ProgressCallback = Arc<dyn Fn(&[u8], bool) + Send + Sync + 'static>;

/// Callback invoked immediately after the child process is spawned. The root
/// background-task adapter uses this to publish the PID without moving its
/// task registry into this crate.
pub type StartedCallback = Arc<dyn Fn(u32) + Send + Sync + 'static>;

/// Bounded output retained from one process stream.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CapturedOutput {
    bytes: Vec<u8>,
    total_bytes: usize,
}

impl CapturedOutput {
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn captured_len(&self) -> usize {
        self.bytes.len()
    }

    pub fn total_bytes(&self) -> usize {
        self.total_bytes
    }

    pub fn is_truncated(&self) -> bool {
        self.total_bytes > MAX_OUTPUT_BYTES
    }
}

/// Result of a completed process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandOutput {
    pub success: bool,
    pub exit_code: Option<i32>,
    pub stdout: CapturedOutput,
    pub stderr: CapturedOutput,
}

#[derive(Default)]
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

    fn finish(self) -> CapturedOutput {
        let mut bytes = Vec::with_capacity(self.head.len() + self.tail.len());
        bytes.extend_from_slice(&self.head);
        bytes.extend(self.tail);
        CapturedOutput {
            bytes,
            total_bytes: self.total_bytes,
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn shell_command(command: &str) -> Command {
    let bash = std::path::Path::new("/bin/bash");
    let mut cmd = if bash.is_file() {
        Command::new(bash)
    } else {
        Command::new("sh")
    };
    if bash.is_file() {
        cmd.args(["-o", "pipefail", "-c", command]);
    } else {
        cmd.args(["-c", command]);
    }
    cmd
}

#[cfg(target_os = "windows")]
fn shell_command(command: &str) -> Command {
    let mut cmd = Command::new("cmd");
    cmd.args(["/C", command]);
    cmd
}

fn build_command(request: &CommandRequest) -> Command {
    let mut command = shell_command(&request.command);
    command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());
    if let Some(cwd) = &request.cwd {
        command.current_dir(cwd);
    }
    for (key, value) in &request.env {
        command.env(key, value);
    }
    #[cfg(unix)]
    if request.process_group {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command
}

/// Run a resolved command with the existing timeout and output semantics.
pub fn run_with_timeout(
    request: &CommandRequest,
    progress: Option<ProgressCallback>,
) -> Result<CommandOutput, String> {
    run_internal(request, Some(request.timeout), progress, None)
}

/// Run a resolved command until it exits. This is retained for the root
/// background adapter, whose existing behavior has no command timeout.
pub fn run_until_exit(
    request: &CommandRequest,
    progress: Option<ProgressCallback>,
    started: Option<StartedCallback>,
) -> Result<CommandOutput, String> {
    run_internal(request, None, progress, started)
}

fn run_internal(
    request: &CommandRequest,
    timeout: Option<Duration>,
    progress: Option<ProgressCallback>,
    started: Option<StartedCallback>,
) -> Result<CommandOutput, String> {
    let mut child = build_command(request)
        .spawn()
        .map_err(|e| format!("failed to spawn process: {e}"))?;
    if let Some(callback) = started {
        callback(child.id());
    }
    let child_stdout = child.stdout.take().ok_or("no stdout pipe")?;
    let child_stderr = child.stderr.take().ok_or("no stderr pipe")?;

    let stdout_progress = progress.clone();
    let out_handle = spawn_output_reader(child_stdout, stdout_progress, false);
    let err_handle = spawn_output_reader(child_stderr, progress, true);

    let status = if let Some(timeout) = timeout {
        let start = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
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
        }
    } else {
        child
            .wait()
            .map_err(|e| format!("failed to wait on process: {e}"))?
    };

    let stdout = out_handle.join().unwrap_or_default().finish();
    let stderr = err_handle.join().unwrap_or_default().finish();
    Ok(CommandOutput {
        success: status.success(),
        exit_code: status.code(),
        stdout,
        stderr,
    })
}

fn spawn_output_reader<R: Read + Send + 'static>(
    mut reader: R,
    progress: Option<ProgressCallback>,
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

/// Format bounded output with the same head/tail marker used by RustCode's
/// existing command tool.
pub fn format_bounded_output(output: &CapturedOutput) -> String {
    if !output.is_truncated() {
        return String::from_utf8_lossy(&output.bytes).to_string();
    }

    let head_len = CAPTURE_HEAD_BYTES.min(output.bytes.len());
    let tail_len = output.bytes.len().saturating_sub(head_len);
    let head = String::from_utf8_lossy(&output.bytes[..head_len]);
    let tail = String::from_utf8_lossy(&output.bytes[head_len..]);
    format!(
        "{head}\n... (truncated, {} bytes total — showing first {head_len} and last {tail_len} bytes) ...\n{tail}\n",
        output.total_bytes
    )
}

#[cfg(test)]
mod tests {
    use super::{
        CommandRequest, MAX_OUTPUT_BYTES, ProgressCallback, StartedCallback, format_bounded_output,
        run_until_exit, run_with_timeout,
    };
    use std::ffi::OsString;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    fn request(command: &str) -> CommandRequest {
        CommandRequest {
            command: command.to_owned(),
            cwd: None,
            env: Vec::new(),
            timeout: Duration::from_secs(5),
            process_group: false,
        }
    }

    #[test]
    fn shell_preserves_pipefail_and_chaining() {
        #[cfg(not(target_os = "windows"))]
        {
            let output = run_with_timeout(&request("false | tail -n 1"), None).unwrap();
            assert!(!output.success);
            assert_eq!(output.exit_code, Some(1));
        }
    }

    #[test]
    fn explicit_environment_is_visible_to_the_shell() {
        #[cfg(not(target_os = "windows"))]
        {
            let mut command = request("printf '%s' \"$RUSTCODE_COMMAND_TEST\"");
            command.env.push((
                OsString::from("RUSTCODE_COMMAND_TEST"),
                OsString::from("present"),
            ));
            let output = run_with_timeout(&command, None).unwrap();
            assert_eq!(output.stdout.bytes(), b"present");
        }
    }

    #[test]
    fn invalid_working_directory_returns_spawn_error() {
        let mut command = request("true");
        command.cwd = Some(PathBuf::from("/definitely/not/a/rustcode-directory"));
        let error = run_with_timeout(&command, None).unwrap_err();
        assert!(error.starts_with("failed to spawn process:"), "{error}");
    }

    #[test]
    fn timeout_returns_the_existing_error_without_result_output() {
        #[cfg(not(target_os = "windows"))]
        {
            let mut command = request("sleep 1");
            command.timeout = Duration::from_millis(1);
            let error = run_with_timeout(&command, None).unwrap_err();
            assert_eq!(error, "command timed out after 1 ms and was killed");
        }
    }

    #[test]
    fn output_capture_is_bounded_and_keeps_both_ends() {
        #[cfg(not(target_os = "windows"))]
        {
            let output = run_with_timeout(
                &request("printf 'START_MARKER'; head -c 200000 /dev/zero; printf 'END_MARKER'"),
                None,
            )
            .unwrap();
            assert!(output.stdout.captured_len() <= MAX_OUTPUT_BYTES);
            assert!(output.stdout.is_truncated());
            let formatted = format_bounded_output(&output.stdout);
            assert!(formatted.contains("START_MARKER"));
            assert!(formatted.contains("END_MARKER"));
        }
    }

    #[test]
    fn progress_reports_stdout_and_stderr_chunks() {
        #[cfg(not(target_os = "windows"))]
        {
            let events = Arc::new(Mutex::new(Vec::new()));
            let captured = Arc::clone(&events);
            let callback: ProgressCallback = Arc::new(move |bytes, stderr| {
                captured.lock().unwrap().push((bytes.to_vec(), stderr));
            });
            run_with_timeout(&request("printf out; printf err >&2"), Some(callback)).unwrap();
            let events = events.lock().unwrap();
            assert!(
                events
                    .iter()
                    .any(|(bytes, stderr)| !stderr && bytes == b"out")
            );
            assert!(
                events
                    .iter()
                    .any(|(bytes, stderr)| *stderr && bytes == b"err")
            );
        }
    }

    #[test]
    fn started_callback_receives_the_child_pid() {
        let pid = Arc::new(AtomicU32::new(0));
        let captured = Arc::clone(&pid);
        let callback: StartedCallback = Arc::new(move |child_pid| {
            captured.store(child_pid, Ordering::Relaxed);
        });
        run_until_exit(&request("true"), None, Some(callback)).unwrap();
        assert!(pid.load(Ordering::Relaxed) > 0);
    }

    #[test]
    fn captured_output_handles_non_utf8_bytes() {
        #[cfg(not(target_os = "windows"))]
        {
            let output = run_with_timeout(&request("printf '\\377'"), None).unwrap();
            assert_eq!(output.stdout.bytes(), &[255]);
        }
    }
}
