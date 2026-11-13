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
    /// The harness's process id: the root of the tree whose children a turn is
    /// waiting on ([`super::child_activity`]).
    pub(crate) fn pid(&self) -> Option<u32> {
        self.group.and_then(|group| u32::try_from(group).ok())
    }

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

/// How a harness child behaves with no terminal. It has no TTY, so a git
/// editor (`rebase --continue`, a merge, a commit) or a pager waits for a
/// person who will never answer: the child hangs until someone stops it.
/// Explicit `env` still wins, so a harness that needs a value can set one.
const NON_INTERACTIVE_ENV: [(&str, &str); 6] = [
    ("GIT_EDITOR", "true"),
    ("GIT_SEQUENCE_EDITOR", "true"),
    ("GIT_PAGER", "cat"),
    ("PAGER", "cat"),
    ("GIT_TERMINAL_PROMPT", "0"),
    ("GIT_MERGE_AUTOEDIT", "no"),
];

/// Start `command` in `cwd`. `log_session` names the thread whose debug log
/// receives the child's stderr. `env` adds variables; `remove` drops ones the
/// login shell brought in, so a chosen harness account is not overridden.
pub(crate) fn spawn(
    harness_id: &str,
    command: &[String],
    cwd: &Path,
    log_session: Option<String>,
    env: &[(String, String)],
    remove: &[&str],
) -> Result<(HarnessProcess, ChildStdout, ChildStdin), String> {
    spawn_with(
        harness_id,
        command,
        cwd,
        log_session,
        env,
        remove,
        super::build_env::Priority::Normal,
    )
}

/// Run `builder`'s child at `priority`: the process and everything it starts.
fn apply_priority(builder: &mut tokio::process::Command, priority: super::build_env::Priority) {
    use super::build_env::Priority;
    match priority {
        Priority::Normal => {}
        #[cfg(target_os = "macos")]
        Priority::MacBackground => {
            // SAFETY: setpriority(2) is async-signal-safe and touches no
            // shared state; this is what `taskpolicy -b` does to a process.
            unsafe {
                builder.pre_exec(|| {
                    libc::setpriority(libc::PRIO_DARWIN_PROCESS, 0, libc::PRIO_DARWIN_BG);
                    Ok(())
                });
            }
        }
        #[cfg(unix)]
        Priority::Nice(level) => {
            // SAFETY: setpriority(2) is async-signal-safe and touches no shared state.
            unsafe {
                builder.pre_exec(move || {
                    libc::setpriority(libc::PRIO_PROCESS, 0, level);
                    Ok(())
                });
            }
        }
        #[cfg(windows)]
        Priority::BelowNormal => {
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;
            // Setting the flags replaces `hide_console_window`'s, so keep them.
            builder.creation_flags(CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS);
        }
        // Another operating system's policy: nothing to set here.
        #[allow(unreachable_patterns)]
        _ => {}
    }
}

/// [`spawn`] at `priority`.
pub(crate) fn spawn_with(
    harness_id: &str,
    command: &[String],
    cwd: &Path,
    log_session: Option<String>,
    env: &[(String, String)],
    remove: &[&str],
    priority: super::build_env::Priority,
) -> Result<(HarnessProcess, ChildStdout, ChildStdin), String> {
    let program = command
        .first()
        .ok_or_else(|| format!("{harness_id} has no command configured"))?;
    let path = super::detect::resolve_program(program).ok_or_else(|| not_found_message(program))?;
    let mut builder = crate::platform::tokio_command(path);
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
    for name in remove {
        builder.env_remove(name);
    }
    // No TTY: an editor or pager would wait forever. These first, so an
    // explicit `env` below still wins.
    builder.envs(NON_INTERACTIVE_ENV);
    builder.envs(env.iter().map(|(name, value)| (name, value)));
    #[cfg(unix)]
    builder.process_group(0);
    apply_priority(&mut builder, priority);
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

    #[cfg(unix)]
    #[tokio::test]
    async fn a_child_gets_no_editor_or_pager_on_stdin_and_env_wins() {
        use tokio::io::AsyncReadExt;

        async fn read(dir: &Path, env: &[(String, String)]) -> String {
            let (process, mut stdout, _stdin) = spawn(
                "printer",
                &[
                    "sh".into(),
                    "-c".into(),
                    "printf '%s|%s|%s|%s|%s|%s' \"$GIT_EDITOR\" \"$GIT_SEQUENCE_EDITOR\" \"$GIT_PAGER\" \"$PAGER\" \"$GIT_TERMINAL_PROMPT\" \"$GIT_MERGE_AUTOEDIT\"".into(),
                ],
                dir,
                None,
                env,
                &[],
            )
            .unwrap();
            let mut text = String::new();
            stdout.read_to_string(&mut text).await.unwrap();
            process.kill();
            text
        }

        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            read(dir.path(), &[]).await,
            "true|true|cat|cat|0|no",
            "a harness child must not open an editor or pager"
        );
        let explicit = [(String::from("GIT_EDITOR"), String::from("vi"))];
        assert_eq!(read(dir.path(), &explicit).await, "vi|true|cat|cat|0|no");
    }

    #[tokio::test]
    async fn missing_programs_fail_clearly_and_kill_takes_the_group() {
        let dir = tempfile::tempdir().unwrap();
        let error = spawn(
            "ghost",
            &["rustcode-no-such-harness-binary".into()],
            dir.path(),
            None,
            &[],
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
