//! Binary-level coverage for `rustcode threads settle|unsettle|review` (#3151).
//!
//! The commands write a thread's `status.json` and exit non-zero on an unknown
//! id, so they are asserted by running the real binary. Every case gets a
//! private `RUSTCODE_CONFIG_DIR` and `HOME` under the system temp dir, removed
//! on drop, so the developer's real `~/.config/rustcode/threads` is never read
//! or written.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const BINARY: &str = env!("CARGO_BIN_EXE_rustcode");
/// Well-formed ids that no fixture here shares a long prefix with, so one
/// case's threads never make another case's prefix ambiguous.
const TITLE_ONE: &str = "018f0000-0000-7000-8000-000000000001";
const TITLE_TWO: &str = "018f0000-0000-7000-8000-000000000002";
const OTHER: &str = "018fffff-0000-7000-8000-000000000003";
/// No fixture creates this one.
const UNKNOWN_ID: &str = "ffffffff-ffff-7fff-8fff-ffffffffffff";
/// Upper bound for one run: each exits long before the terminal is set up.
const RUN_TIMEOUT: Duration = Duration::from_secs(30);

/// A throwaway config dir, home dir and workspace for one child process.
struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new(label: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "rustcode-threads-status-{label}-{}-{}",
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

    /// Write a saved thread in the legacy `threads/<id>/history.json` layout,
    /// which discovery walks and the startup migration moves into place.
    fn write_thread(&self, id: &str, title: &str) {
        let directory = self.config_dir().join("threads").join(id);
        std::fs::create_dir_all(&directory).expect("create thread dir");
        let transcript =
            r#"[{"role":"user","content":"hello"},{"role":"assistant","content":"ack"}]"#;
        std::fs::write(directory.join("history.json"), transcript).expect("write transcript");
        std::fs::write(directory.join("title.txt"), title).expect("write title");
    }

    fn run(&self, args: &[&str]) -> Output {
        let mut command = Command::new(BINARY);
        command
            .args(args)
            // The sandbox root doubles as the workspace so project config
            // discovery and workspace-scoped listings stay off the real repo.
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

    /// The thread's directory as the CLI reports it, so the test never assumes
    /// where the store files it (legacy layout or `threads/YYYY/MM/DD/<id>`).
    fn thread_dir(&self, id: &str) -> PathBuf {
        let output = self.run(&["threads", "path", id]);
        assert!(
            output.status.success(),
            "`threads path {id}` failed: {}",
            text(&output.stderr)
        );
        PathBuf::from(text(&output.stdout).trim())
    }

    /// The status the engine stored for a thread, or `None` for active.
    fn stored_status(&self, id: &str) -> Option<String> {
        let path = self.thread_dir(id).join("status.json");
        let text = std::fs::read_to_string(path).ok()?;
        let value: serde_json::Value = serde_json::from_str(&text).ok()?;
        value.get("status")?.as_str().map(str::to_owned)
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

fn lines(output: &Output) -> Vec<String> {
    text(&output.stdout)
        .lines()
        .map(|line| line.trim_end().to_owned())
        .collect()
}

#[test]
fn settle_marks_every_given_thread_done_and_says_which_changed() {
    let sandbox = Sandbox::new("settle");
    sandbox.write_thread(TITLE_ONE, "Fix the parser");
    sandbox.write_thread(OTHER, "Ship the release");

    let output = sandbox.run(&["threads", "settle", TITLE_ONE, OTHER]);
    assert!(
        output.status.success(),
        "settle should succeed: {}",
        text(&output.stderr)
    );
    assert_eq!(
        lines(&output),
        [
            format!("settled {TITLE_ONE} Fix the parser"),
            format!("settled {OTHER} Ship the release"),
        ]
    );
    for id in [TITLE_ONE, OTHER] {
        assert_eq!(sandbox.stored_status(id).as_deref(), Some("done"), "{id}");
    }
}

#[test]
fn a_unique_prefix_settles_the_thread_and_prints_its_full_id() {
    let sandbox = Sandbox::new("prefix");
    sandbox.write_thread(TITLE_ONE, "Fix the parser");
    sandbox.write_thread(OTHER, "Ship the release");

    let output = sandbox.run(&["threads", "settle", "018f0"]);
    assert!(
        output.status.success(),
        "a unique prefix should resolve: {}",
        text(&output.stderr)
    );
    assert_eq!(
        lines(&output),
        [format!("settled {TITLE_ONE} Fix the parser")]
    );
    assert_eq!(sandbox.stored_status(TITLE_ONE).as_deref(), Some("done"));
    assert_eq!(sandbox.stored_status(OTHER), None, "the other thread stays");
}

#[test]
fn an_ambiguous_prefix_fails_and_lists_the_candidates() {
    let sandbox = Sandbox::new("ambiguous");
    sandbox.write_thread(TITLE_ONE, "Fix the parser");
    sandbox.write_thread(TITLE_TWO, "Fix the linter");

    let output = sandbox.run(&["threads", "settle", "018f0000"]);
    assert!(!output.status.success(), "an ambiguous prefix must fail");
    let stderr = text(&output.stderr);
    assert!(stderr.contains("matches 2 threads"), "{stderr:?}");
    assert!(
        stderr.contains(TITLE_ONE) && stderr.contains(TITLE_TWO),
        "{stderr:?}"
    );
    assert!(output.stdout.is_empty(), "{:?}", text(&output.stdout));
    for id in [TITLE_ONE, TITLE_TWO] {
        assert_eq!(sandbox.stored_status(id), None, "{id} is untouched");
    }
}

#[test]
fn an_unknown_id_exits_nonzero_and_the_others_still_change() {
    let sandbox = Sandbox::new("unknown");
    sandbox.write_thread(TITLE_ONE, "Fix the parser");

    let output = sandbox.run(&["threads", "settle", UNKNOWN_ID, TITLE_ONE]);
    assert!(!output.status.success(), "an unknown id must fail the run");
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains(UNKNOWN_ID) && stderr.contains("No saved thread"),
        "{stderr:?}"
    );
    assert_eq!(
        lines(&output),
        [format!("settled {TITLE_ONE} Fix the parser")]
    );
    assert_eq!(sandbox.stored_status(TITLE_ONE).as_deref(), Some("done"));
}

#[test]
fn unsettle_restores_active_and_review_is_its_own_status() {
    let sandbox = Sandbox::new("unsettle");
    sandbox.write_thread(TITLE_ONE, "Fix the parser");

    assert!(
        sandbox
            .run(&["threads", "settle", TITLE_ONE])
            .status
            .success()
    );
    assert_eq!(sandbox.stored_status(TITLE_ONE).as_deref(), Some("done"));

    let output = sandbox.run(&["threads", "unsettle", TITLE_ONE]);
    assert!(
        output.status.success(),
        "unsettle should succeed: {}",
        text(&output.stderr)
    );
    assert_eq!(
        lines(&output),
        [format!("active {TITLE_ONE} Fix the parser")]
    );
    assert_eq!(
        sandbox.stored_status(TITLE_ONE),
        None,
        "active is stored as no status, the way `idle` is"
    );

    let output = sandbox.run(&["threads", "review", TITLE_ONE]);
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        lines(&output),
        [format!("review {TITLE_ONE} Fix the parser")]
    );
    assert_eq!(sandbox.stored_status(TITLE_ONE).as_deref(), Some("review"));

    // Settling a review thread keeps the one stored status the model has.
    assert!(
        sandbox
            .run(&["threads", "settle", TITLE_ONE])
            .status
            .success()
    );
    assert_eq!(sandbox.stored_status(TITLE_ONE).as_deref(), Some("done"));
}

#[test]
fn unsettle_leaves_a_review_thread_in_review() {
    let sandbox = Sandbox::new("unsettle-review");
    sandbox.write_thread(TITLE_ONE, "Fix the parser");

    assert!(
        sandbox
            .run(&["threads", "review", TITLE_ONE])
            .status
            .success()
    );
    let output = sandbox.run(&["threads", "unsettle", TITLE_ONE]);
    assert!(
        output.status.success(),
        "leaving a review thread alone is not a failure: {}",
        text(&output.stderr)
    );
    assert_eq!(lines(&output), [format!("kept {TITLE_ONE} review")]);
    assert_eq!(sandbox.stored_status(TITLE_ONE).as_deref(), Some("review"));

    // Settled is the state it does move.
    assert!(
        sandbox
            .run(&["threads", "settle", TITLE_ONE])
            .status
            .success()
    );
    let output = sandbox.run(&["threads", "unsettle", TITLE_ONE]);
    assert_eq!(
        lines(&output),
        [format!("active {TITLE_ONE} Fix the parser")]
    );
    assert_eq!(sandbox.stored_status(TITLE_ONE), None);
}

#[test]
fn an_exact_id_settles_a_thread_whose_folder_has_no_transcript_yet() {
    let sandbox = Sandbox::new("no-history");
    let directory = sandbox.config_dir().join("threads").join(OTHER);
    std::fs::create_dir_all(&directory).expect("create thread dir");
    std::fs::write(directory.join("title.txt"), "Started a moment ago").expect("write title");

    let output = sandbox.run(&["threads", "settle", OTHER]);
    assert!(
        output.status.success(),
        "a thread folder with no history is still that thread: {}",
        text(&output.stderr)
    );
    assert_eq!(
        lines(&output),
        [format!("settled {OTHER} Started a moment ago")]
    );
    assert_eq!(sandbox.stored_status(OTHER).as_deref(), Some("done"));

    // Only a full id reaches it: a prefix would be guessing at a folder no list vouches for.
    let output = sandbox.run(&["threads", "settle", "018fff"]);
    assert!(!output.status.success(), "a prefix is not a folder lookup");
}

#[test]
fn settle_needs_at_least_one_id() {
    let sandbox = Sandbox::new("no-id");
    let output = sandbox.run(&["threads", "settle"]);
    assert!(!output.status.success());
    let stderr = text(&output.stderr);
    assert!(stderr.contains("ID"), "{stderr:?}");
}
