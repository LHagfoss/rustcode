//! Starting and stopping a harness process. Each runs in its own process
//! group in the thread's working directory; its stderr goes to the thread's
//! debug log. Killing a harness kills its whole group, so tools it started
//! go with it.

use std::collections::HashSet;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use tokio::io::AsyncBufReadExt;
use tokio::process::{Child, ChildStdin, ChildStdout};

/// Process groups of running harnesses, killed when the engine stops.
fn live_groups() -> &'static Mutex<HashSet<i32>> {
    static GROUPS: OnceLock<Mutex<HashSet<i32>>> = OnceLock::new();
    GROUPS.get_or_init(Default::default)
}

pub(crate) struct HarnessProcess {
    child: Mutex<Option<Child>>,
    group: Option<i32>,
}

impl HarnessProcess {
    /// Kill the harness and its process group now.
    pub(crate) fn kill(&self) {
        if let Some(group) = self.group {
            kill_group(group);
            live_groups()
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .remove(&group);
        }
        let child = self
            .child
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some(mut child) = child {
            let _ = child.start_kill();
            // Reap it so it does not linger as a zombie.
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                handle.spawn(async move {
                    let _ = child.wait().await;
                });
            }
        }
    }
}

impl Drop for HarnessProcess {
    fn drop(&mut self) {
        self.kill();
    }
}

fn kill_group(group: i32) {
    #[cfg(unix)]
    // SAFETY: kill(2) with a negative pid signals a process group we created.
    unsafe {
        libc::kill(-group, libc::SIGKILL);
    }
    #[cfg(not(unix))]
    let _ = group;
}

/// Kill every harness process group this engine started. Runs when the ACP
/// connection closes and when the engine exits.
pub(crate) fn kill_all() {
    let groups = std::mem::take(
        &mut *live_groups()
            .lock()
            .unwrap_or_else(|error| error.into_inner()),
    );
    for group in groups {
        kill_group(group);
    }
}

/// The message a thread shows when its harness cannot start.
pub(crate) fn not_found_message(program: &str) -> String {
    format!("{program} not found on PATH")
}

/// Start `command` in `cwd`. `log_session` names the thread whose debug log
/// receives the child's stderr.
pub(crate) fn spawn(
    harness_id: &str,
    command: &[String],
    cwd: &Path,
    log_session: Option<String>,
    env: &[(String, String)],
) -> Result<(HarnessProcess, ChildStdout, ChildStdin), String> {
    let program = command
        .first()
        .ok_or_else(|| format!("{harness_id} has no command configured"))?;
    let path = super::detect::resolve_program(program).ok_or_else(|| not_found_message(program))?;
    let mut builder = tokio::process::Command::new(path);
    builder
        .args(&command[1..])
        .current_dir(cwd)
        .env("PATH", super::detect::search_path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    // The engine's own settings are not the harness's business; it keeps
    // the rest of the user's environment and its own sign-in.
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("RUSTCODE_") {
            builder.env_remove(name);
        }
    }
    builder.envs(env.iter().map(|(name, value)| (name, value)));
    #[cfg(unix)]
    builder.process_group(0);
    let mut child = builder
        .spawn()
        .map_err(|error| format!("could not start {program}: {error}"))?;
    let stdout = child.stdout.take().ok_or("harness stdout unavailable")?;
    let stdin = child.stdin.take().ok_or("harness stdin unavailable")?;
    if let Some(stderr) = child.stderr.take() {
        let harness_id = harness_id.to_owned();
        tokio::spawn(async move {
            let mut lines = tokio::io::BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                match &log_session {
                    Some(session) => crate::logger::append_line_for_session(
                        session,
                        &format!("[harness {harness_id}] {line}"),
                    ),
                    None => crate::dbg_log!("[harness {harness_id}] {line}"),
                }
            }
        });
    }
    let group = child.id().map(|pid| pid as i32);
    if let Some(group) = group {
        live_groups()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(group);
    }
    Ok((
        HarnessProcess {
            child: Mutex::new(Some(child)),
            group,
        },
        stdout,
        stdin,
    ))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn missing_programs_fail_clearly_and_kill_takes_the_group() {
        let dir = tempfile::tempdir().unwrap();
        let error = spawn(
            "ghost",
            &["rustcode-no-such-harness-binary".into()],
            dir.path(),
            None,
            &[],
        )
        .err()
        .unwrap();
        assert_eq!(error, "rustcode-no-such-harness-binary not found on PATH");

        let (process, _stdout, _stdin) = spawn(
            "sleeper",
            &["sh".into(), "-c".into(), "sleep 30 & sleep 30".into()],
            dir.path(),
            None,
            &[],
        )
        .unwrap();
        let group = process.group.unwrap();
        process.kill();
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        // SAFETY: probing a process group with signal 0.
        let alive = unsafe { libc::kill(-group, 0) } == 0;
        assert!(!alive, "the harness group outlived kill");
    }
}
