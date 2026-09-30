//! Binary-level coverage for `--resume <id>`, `-r <id>` and `--continue <id>`.
//!
//! The unknown-id path calls `std::process::exit(1)`, so it can only be
//! asserted by running the real binary — a `#[test]` cannot observe it
//! in-process. Every case gets a private `RUSTCODE_CONFIG_DIR` and `HOME`
//! under the system temp dir, removed on drop, so the developer's real
//! `~/.config/rustcode/sessions` is never read or written.
//!
//! No case needs a TTY: resume-by-id resolves before the terminal is taken
//! over, and the runs that get that far (`--resume` with no id) only assert
//! the persisted session pointer, not a rendered TUI.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const BINARY: &str = env!("CARGO_BIN_EXE_rustcode");
/// Well-formed id that no fixture in this file ever creates.
const UNKNOWN_ID: &str = "ffffffff-ffff-7fff-8fff-ffffffffffff";
/// Oldest first: the leading hex digits of a session id are the store's
/// ordering key, so the id alone decides which session is "most recent".
const OLDER_ID: &str = "018f0000-0000-7000-8000-000000000001";
const RECENT_ID: &str = "018fffff-0000-7000-8000-000000000002";
/// Upper bound for one run. Every case below either exits before terminal
/// setup or fails terminal setup immediately; this only stops a regression
/// that reaches the event loop from stalling the suite.
const RUN_TIMEOUT: Duration = Duration::from_secs(30);

/// A throwaway config dir, home dir and workspace for one child process.
struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new(label: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "rustcode-resume-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("config")).expect("create sandbox config dir");
        std::fs::create_dir_all(root.join("home")).expect("create sandbox home");
        Self { root }
    }

    fn config_dir(&self) -> PathBuf {
        self.root.join("config")
    }

    /// Write a saved session into the legacy `sessions/<id>/history.json`
    /// layout that session discovery walks. A transcript needs at least one
    /// user and one assistant turn to be resumable.
    fn write_session(&self, id: &str, prompt: &str) {
        let directory = self.config_dir().join("sessions").join(id);
        std::fs::create_dir_all(&directory).expect("create session dir");
        let transcript = format!(
            r#"[{{"role":"user","content":"{prompt}"}},{{"role":"assistant","content":"ack"}}]"#
        );
        std::fs::write(directory.join("history.json"), transcript).expect("write transcript");
    }

    /// Simulate a session that existed and was later deleted or archived: the
    /// id stays known to the user, the transcript does not.
    fn delete_session(&self, id: &str) {
        std::fs::remove_dir_all(self.config_dir().join("sessions").join(id))
            .expect("remove session dir");
    }

    /// The session the last run left active. `load_session_into` is the only
    /// startup path that repoints this at an existing session, so it is the
    /// on-disk evidence that a resume actually resolved.
    fn last_active_session_id(&self) -> Option<String> {
        let contents = std::fs::read_to_string(self.config_dir().join("config.toml")).ok()?;
        let config: toml::Value = toml::from_str(&contents).ok()?;
        config
            .get("last_active_session_id")?
            .as_str()
            .map(str::to_owned)
    }

    fn run(&self, args: &[&str]) -> Output {
        let mut command = Command::new(BINARY);
        command
            .args(args)
            // The sandbox root doubles as the workspace so workspace-scoped
            // lookups and project config discovery stay off the real repo.
            .current_dir(&self.root)
            .env("RUSTCODE_CONFIG_DIR", self.config_dir())
            .env("HOME", self.root.join("home"))
            .env("XDG_CONFIG_HOME", self.root.join("home"))
            // Mark the process as a shell-probe child so startup skips
            // spawning the developer's login shell and parsing their dotfiles.
            .env("RUSTCODE_SHELL_PROBE", "1")
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        unsafe {
            use std::os::unix::process::CommandExt;
            command.pre_exec(|| {
                // SAFETY: `setsid` runs in the forked child between fork and
                // exec, where it only affects that child's session.
                libc::setsid();
                Ok(())
            });
        }
        run_bounded(&mut command, args, RUN_TIMEOUT)
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Drain both pipes on threads and kill the child if it outlives `timeout`.
fn run_bounded(command: &mut Command, args: &[&str], timeout: Duration) -> Output {
    let mut child = command.spawn().expect("spawn rustcode");
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let stdout_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout.read_to_end(&mut bytes);
        bytes
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stderr.read_to_end(&mut bytes);
        bytes
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait().expect("poll rustcode") {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                panic!(
                    "`rustcode {}` did not exit within {timeout:?}",
                    args.join(" ")
                );
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    };
    Output {
        status,
        stdout: stdout_reader.join().expect("stdout reader"),
        stderr: stderr_reader.join().expect("stderr reader"),
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn resume_unknown_id_exits_nonzero_on_stderr_only() {
    let sandbox = Sandbox::new("unknown");
    // A resumable session exists, so the failure can only come from the id.
    sandbox.write_session(RECENT_ID, "hello");

    for args in [
        ["--resume", UNKNOWN_ID],
        ["-r", UNKNOWN_ID],
        ["--continue", UNKNOWN_ID],
    ] {
        let output = sandbox.run(&args);
        let stderr = text(&output.stderr);
        assert!(
            !output.status.success(),
            "`rustcode {}` should fail, stdout: {}",
            args.join(" "),
            text(&output.stdout)
        );
        assert!(
            stderr.contains("cannot resume session") && stderr.contains(UNKNOWN_ID),
            "`rustcode {}` stderr should name the id it could not resume: {stderr:?}",
            args.join(" ")
        );
        assert!(
            stderr.contains("session not found"),
            "`rustcode {}` stderr should say the session is missing: {stderr:?}",
            args.join(" ")
        );
        assert!(
            output.stdout.is_empty(),
            "`rustcode {}` must keep stdout clean: {:?}",
            args.join(" "),
            text(&output.stdout)
        );
    }
}

#[test]
fn resume_deleted_id_exits_nonzero_on_stderr_only() {
    let sandbox = Sandbox::new("deleted");
    sandbox.write_session(RECENT_ID, "hello");
    sandbox.delete_session(RECENT_ID);

    let output = sandbox.run(&["--resume", RECENT_ID]);
    let stderr = text(&output.stderr);
    assert!(
        !output.status.success(),
        "a deleted id should fail, stdout: {}",
        text(&output.stdout)
    );
    assert!(
        stderr.contains("cannot resume session") && stderr.contains(RECENT_ID),
        "stderr should name the deleted id: {stderr:?}"
    );
    assert!(
        output.stdout.is_empty(),
        "stdout must stay clean: {:?}",
        text(&output.stdout)
    );
    assert_ne!(
        sandbox.last_active_session_id().as_deref(),
        Some(RECENT_ID),
        "a failed resume must not adopt the deleted session"
    );
}

#[test]
fn bare_resume_still_adopts_the_most_recent_session() {
    let sandbox = Sandbox::new("bare");
    sandbox.write_session(OLDER_ID, "older");
    sandbox.write_session(RECENT_ID, "newer");

    // The sandbox has no terminal, so the run cannot open the TUI. What this
    // guards is that `--resume` with no id still resolves to the most recent
    // session instead of falling into the explicit-id failure path.
    let output = sandbox.run(&["--resume"]);
    let stderr = text(&output.stderr);
    assert!(
        !stderr.contains("cannot resume session"),
        "a bare --resume must not fail as an explicit id: {stderr:?}"
    );
    assert_eq!(
        sandbox.last_active_session_id().as_deref(),
        Some(RECENT_ID),
        "bare --resume should adopt the most recent session, not {OLDER_ID}"
    );
}
