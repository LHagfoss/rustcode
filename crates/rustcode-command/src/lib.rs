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
#[derive(Clone, Debug)]
pub struct CommandRequest {
    pub command: String,
    /// Original shell text used to interpret exit metadata when `command` is
    /// wrapped by an operating-system sandbox launcher.
    pub status_command: Option<String>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(OsString, OsString)>,
    pub timeout: Duration,
    /// Background callers request a process group so they can terminate the
    /// shell and its descendants from the application-owned task manager.
    pub process_group: bool,
    /// File descriptors explicitly inherited by the command process.
    /// Linux bubblewrap uses this only to read a seccomp policy at startup.
    pub inherited_fds: Vec<Arc<std::fs::File>>,
}

impl PartialEq for CommandRequest {
    fn eq(&self, other: &Self) -> bool {
        self.command == other.command
            && self.status_command == other.status_command
            && self.cwd == other.cwd
            && self.env == other.env
            && self.timeout == other.timeout
            && self.process_group == other.process_group
            && self.inherited_fds.len() == other.inherited_fds.len()
            && self
                .inherited_fds
                .iter()
                .zip(&other.inherited_fds)
                .all(|(left, right)| Arc::ptr_eq(left, right))
    }
}

impl Eq for CommandRequest {}

/// Callback invoked as bytes arrive from stdout or stderr.
pub type ProgressCallback = Arc<dyn Fn(&[u8], bool) + Send + Sync + 'static>;

/// Callback invoked immediately after the child process is spawned. The root
/// background-task adapter uses this to publish the PID without moving its
/// task registry into this crate.
pub type StartedCallback = Arc<dyn Fn(u32) + Send + Sync + 'static>;

/// Callback polled while a foreground process is running. Returning `true`
/// requests termination of the process and its descendants.
pub type CancellationCallback = Arc<dyn Fn() -> bool + Send + Sync + 'static>;

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
    /// The native terminating signal, or the signal encoded by a pipefail
    /// status such as 141 (SIGPIPE).
    pub signal: Option<i32>,
    /// True when SIGPIPE came from a downstream pipeline consumer stopping
    /// after receiving the requested amount of output.
    pub downstream_consumer_terminated: bool,
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
    build_command_with_environment(request, None)
}

fn build_command_with_environment(
    request: &CommandRequest,
    allowlist: Option<&[String]>,
) -> Command {
    let mut command = shell_command(&request.command);
    if let Some(allowlist) = allowlist {
        command.env_clear();
        for name in allowlist {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
    }
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
    if request.process_group || !request.inherited_fds.is_empty() {
        use std::os::unix::process::CommandExt;
        if request.process_group {
            command.process_group(0);
        }
        let inherited_fds = request
            .inherited_fds
            .iter()
            .map(|file| {
                use std::os::fd::AsRawFd;
                file.as_raw_fd()
            })
            .collect::<Vec<_>>();
        // SAFETY: this child hook only changes close-on-exec flags on caller
        // owned descriptors before exec. The Arc<File>s stay alive because
        // run_* borrows CommandRequest until the child completes.
        unsafe {
            command.pre_exec(move || {
                for fd in &inherited_fds {
                    let flags = libc::fcntl(*fd, libc::F_GETFD);
                    if flags == -1
                        || libc::fcntl(*fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) == -1
                    {
                        return Err(std::io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }
    }
    command
}

/// Run a resolved command with the existing timeout and output semantics.
pub fn run_with_timeout(
    request: &CommandRequest,
    progress: Option<ProgressCallback>,
) -> Result<CommandOutput, String> {
    run_internal(request, Some(request.timeout), progress, None, None)
}

/// Run a resolved command with timeout semantics and cooperative cancellation.
pub fn run_with_timeout_cancellable(
    request: &CommandRequest,
    progress: Option<ProgressCallback>,
    cancellation: Option<CancellationCallback>,
) -> Result<CommandOutput, String> {
    run_internal(request, Some(request.timeout), progress, None, cancellation)
}

/// Scheduled commands inherit only explicitly recorded environment names.
/// Ordinary foreground and background callers retain their existing behavior.
pub fn run_with_timeout_cancellable_env(
    request: &CommandRequest,
    progress: Option<ProgressCallback>,
    cancellation: Option<CancellationCallback>,
    allowlist: &[String],
) -> Result<CommandOutput, String> {
    run_command_internal(
        request,
        Some(request.timeout),
        progress,
        None,
        cancellation,
        build_command_with_environment(request, Some(allowlist)),
    )
}

/// Run a resolved command until it exits. This is retained for the root
/// background adapter, whose existing behavior has no command timeout.
pub fn run_until_exit(
    request: &CommandRequest,
    progress: Option<ProgressCallback>,
    started: Option<StartedCallback>,
) -> Result<CommandOutput, String> {
    run_internal(request, None, progress, started, None)
}

fn run_internal(
    request: &CommandRequest,
    timeout: Option<Duration>,
    progress: Option<ProgressCallback>,
    started: Option<StartedCallback>,
    cancellation: Option<CancellationCallback>,
) -> Result<CommandOutput, String> {
    run_command_internal(
        request,
        timeout,
        progress,
        started,
        cancellation,
        build_command(request),
    )
}

fn run_command_internal(
    request: &CommandRequest,
    timeout: Option<Duration>,
    progress: Option<ProgressCallback>,
    started: Option<StartedCallback>,
    cancellation: Option<CancellationCallback>,
    mut command: Command,
) -> Result<CommandOutput, String> {
    let mut child = command
        .spawn()
        .map_err(|e| format!("failed to spawn process: {e}"))?;
    let start = Instant::now();
    if let Some(callback) = started {
        callback(child.id());
    }
    let child_stdout = child.stdout.take().ok_or("no stdout pipe")?;
    let child_stderr = child.stderr.take().ok_or("no stderr pipe")?;

    let stdout_progress = progress.clone();
    let out_handle = spawn_output_reader(child_stdout, stdout_progress, false);
    let err_handle = spawn_output_reader(child_stderr, progress, true);

    let status = if let Some(timeout) = timeout {
        loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {
                    if cancellation.as_ref().is_some_and(|callback| callback()) {
                        terminate_process_tree(&mut child, request.process_group);
                        return Err("command cancelled by user".to_string());
                    }
                    if start.elapsed() >= timeout {
                        terminate_process_tree(&mut child, request.process_group);
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

    // Descendants can retain stdout/stderr after the shell exits. Include pipe
    // draining in the hard timeout instead of blocking forever in join().
    while !out_handle.is_finished() || !err_handle.is_finished() {
        if cancellation.as_ref().is_some_and(|callback| callback()) {
            terminate_process_tree(&mut child, request.process_group);
            return Err("command cancelled by user".to_string());
        }
        if let Some(timeout) = timeout {
            if start.elapsed() >= timeout {
                terminate_process_tree(&mut child, request.process_group);
                return Err(format!(
                    "command timed out after {} ms and was killed",
                    timeout.as_millis()
                ));
            }
        }
        thread::sleep(Duration::from_millis(20));
    }
    let stdout = out_handle.join().unwrap_or_default().finish();
    let stderr = err_handle.join().unwrap_or_default().finish();
    let status_command = request
        .status_command
        .as_deref()
        .unwrap_or(&request.command);
    let signal = terminating_signal(&status, status_command);
    let downstream_consumer_terminated = is_downstream_consumer_termination(signal, status_command);
    Ok(CommandOutput {
        success: status.success() || downstream_consumer_terminated,
        exit_code: status.code(),
        signal,
        downstream_consumer_terminated,
        stdout,
        stderr,
    })
}

#[cfg(unix)]
fn terminating_signal(status: &std::process::ExitStatus, command: &str) -> Option<i32> {
    use std::os::unix::process::ExitStatusExt;

    status.signal().or_else(|| {
        // With bash's `pipefail`, an upstream process killed by SIGPIPE is
        // reported by the shell as 128 + SIGPIPE rather than as a native
        // signal on the shell's ExitStatus.
        (status.code() == Some(128 + libc::SIGPIPE) && has_shell_pipeline(command))
            .then_some(libc::SIGPIPE)
    })
}

#[cfg(target_os = "windows")]
fn terminating_signal(_status: &std::process::ExitStatus, _command: &str) -> Option<i32> {
    None
}

fn is_downstream_consumer_termination(signal: Option<i32>, command: &str) -> bool {
    #[cfg(unix)]
    {
        signal == Some(libc::SIGPIPE) && has_shell_pipeline(command)
    }
    #[cfg(target_os = "windows")]
    {
        let _ = (signal, command);
        false
    }
}

fn has_shell_pipeline(command: &str) -> bool {
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
        if byte != b'|' || single_quote || double_quote {
            continue;
        }

        let previous = index.checked_sub(1).and_then(|i| bytes.get(i)).copied();
        let next = bytes.get(index + 1).copied();
        if previous != Some(b'|') && next != Some(b'|') {
            return true;
        }
    }
    false
}

fn terminate_process_tree(child: &mut std::process::Child, process_group: bool) {
    #[cfg(unix)]
    if process_group {
        // The shell is created as its own process-group leader. A negative PID
        // targets that group, including descendants that outlive the shell.
        if let Ok(group_id) = libc::pid_t::try_from(child.id())
            && group_id > 0
        {
            // Best effort: wait below still reaps the direct child if it has
            // already exited or the group was already gone.
            unsafe {
                libc::kill(-group_id, libc::SIGKILL);
            }
        }
    }

    #[cfg(windows)]
    {
        // taskkill's tree flag covers descendants even when the shell did not
        // create a Windows process group of its own.
        let _ = Command::new("taskkill")
            .args(["/F", "/T", "/PID", &child.id().to_string()])
            .status();
    }

    let _ = child.kill();
    let _ = child.wait();
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
        CancellationCallback, CommandRequest, MAX_OUTPUT_BYTES, ProgressCallback, StartedCallback,
        format_bounded_output, run_until_exit, run_with_timeout, run_with_timeout_cancellable,
    };
    use std::ffi::OsString;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;

    static MARKER_SEQUENCE: AtomicU64 = AtomicU64::new(1);

    struct MarkerCleanup(PathBuf);

    impl Drop for MarkerCleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn request(command: &str) -> CommandRequest {
        CommandRequest {
            command: command.to_owned(),
            status_command: None,
            cwd: None,
            env: Vec::new(),
            timeout: Duration::from_secs(5),
            process_group: false,
            inherited_fds: Vec::new(),
        }
    }

    #[cfg(unix)]
    #[test]
    fn explicitly_inherited_file_descriptor_reaches_shell_child() {
        use std::os::fd::AsRawFd;

        let file = Arc::new(std::fs::File::open("/dev/null").unwrap());
        let fd = file.as_raw_fd();
        let request = CommandRequest {
            command: format!("test -r /dev/fd/{fd}"),
            inherited_fds: vec![file],
            ..request("")
        };
        let output = run_with_timeout(&request, None).unwrap();
        assert!(output.success);
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

    #[cfg(unix)]
    #[test]
    fn downstream_sigpipe_is_explicit_and_not_a_command_failure() {
        let output = run_with_timeout(&request("yes | head -n 1"), None).unwrap();

        assert!(output.success);
        assert_eq!(output.exit_code, Some(141));
        assert_eq!(output.signal, Some(libc::SIGPIPE));
        assert!(output.downstream_consumer_terminated);
    }

    #[cfg(unix)]
    #[test]
    fn wrapped_command_uses_original_shell_text_for_sigpipe_classification() {
        let original = "yes | head -n 1";
        let wrapped = format!("/bin/bash -o pipefail -c '{}'", original);
        let request = CommandRequest {
            command: wrapped,
            status_command: Some(original.to_owned()),
            ..request("")
        };

        let output = run_with_timeout(&request, None).unwrap();

        assert!(output.success);
        assert_eq!(output.exit_code, Some(141));
        assert_eq!(output.signal, Some(libc::SIGPIPE));
        assert!(output.downstream_consumer_terminated);
    }

    #[cfg(unix)]
    #[test]
    fn encoded_signal_without_a_pipeline_remains_an_exit_code() {
        let output = run_with_timeout(&request("exit 141"), None).unwrap();

        assert!(!output.success);
        assert_eq!(output.exit_code, Some(141));
        assert_eq!(output.signal, None);
        assert!(!output.downstream_consumer_terminated);
    }

    #[cfg(unix)]
    #[test]
    fn native_signal_is_separate_from_exit_code() {
        let output = run_with_timeout(&request("kill -TERM $$"), None).unwrap();

        assert!(!output.success);
        assert_eq!(output.exit_code, None);
        assert_eq!(output.signal, Some(libc::SIGTERM));
        assert!(!output.downstream_consumer_terminated);
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
            assert!(output.stdout.total_bytes() > output.stdout.captured_len());
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

    #[test]
    fn cancellation_returns_once_and_kills_the_process_group() {
        #[cfg(not(target_os = "windows"))]
        {
            let cancelled = Arc::new(AtomicBool::new(false));
            let trigger = Arc::clone(&cancelled);
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(30));
                trigger.store(true, Ordering::Release);
            });
            let callback: CancellationCallback =
                Arc::new(move || cancelled.load(Ordering::Acquire));
            let marker = std::env::temp_dir().join(format!(
                "rustcode-command-cancel-marker-{}-{}",
                std::process::id(),
                MARKER_SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            let _cleanup = MarkerCleanup(marker.clone());
            let mut command = request(&format!(
                "(sleep 0.3; printf descendant > {}) & wait",
                marker.display()
            ));
            command.process_group = true;
            let error = run_with_timeout_cancellable(&command, None, Some(callback)).unwrap_err();
            assert_eq!(error, "command cancelled by user");
            thread::sleep(Duration::from_millis(400));
            assert!(!marker.exists());
        }
    }

    #[test]
    fn timeout_kills_descendants_with_the_process_group() {
        #[cfg(not(target_os = "windows"))]
        {
            let marker = format!(
                "/tmp/rustcode-command-timeout-marker-{}",
                std::process::id()
            );
            let _ = std::fs::remove_file(&marker);
            let mut command = request(&format!("(sleep 0.3; printf descendant > {marker}) & wait"));
            command.process_group = true;
            command.timeout = Duration::from_millis(30);
            let error = run_with_timeout(&command, None).unwrap_err();
            assert_eq!(error, "command timed out after 30 ms and was killed");
            thread::sleep(Duration::from_millis(400));
            assert!(!std::path::Path::new(&marker).exists());
            let _ = std::fs::remove_file(marker);
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_process_tree_path_compiles_and_runs() {
        let mut command = request("exit 0");
        command.process_group = true;
        assert!(run_with_timeout(&command, None).unwrap().success);
    }
}
