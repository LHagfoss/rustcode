#[test]
fn notifying_background_start_is_pending_not_failed() {
    let notifying = super::background_start_output("task-1", "sleep 5".into(), true, false);
    assert!(notifying.pending);
    assert!(!notifying.success);
    let silent = super::background_start_output("task-2", "sleep 5".into(), false, false);
    assert!(!silent.pending);
    assert!(silent.success);
}

#[test]
fn only_a_bare_trailing_search_counts_as_the_search_status() {
    assert!(super::ends_in_bare_search("rg -n foo src"));
    assert!(super::ends_in_bare_search("cd /repo && rg -n 'a|b' src"));
    assert!(super::ends_in_bare_search("cd /repo; grep -rn foo ."));
    assert!(super::ends_in_bare_search("git grep foo"));
    // Pipelines fail on any stage, so exit 1 is not the search's own.
    assert!(!super::ends_in_bare_search("rg foo | head"));
    assert!(!super::ends_in_bare_search("rg foo && cargo test"));
    assert!(!super::ends_in_bare_search("cargo test"));
}

#[test]
fn rg_grep_style_recursive_flags_get_a_replace_hint() {
    assert!(super::rg_replace_flag_hint("cd /repo && rg -rn \"TokenUsage\" src").is_some());
    assert!(super::rg_replace_flag_hint("/opt/homebrew/bin/rg -nr foo").is_some());
    assert!(super::rg_replace_flag_hint("rg -n foo src").is_none());
    // A lone -r is usually a deliberate replacement.
    assert!(super::rg_replace_flag_hint("rg -o 'v(\\d+)' -r '$1' Cargo.toml").is_none());
    // grep's -rn is recursive and correct.
    assert!(super::rg_replace_flag_hint("grep -rn foo .").is_none());
    assert!(super::rg_replace_flag_hint("rg foo | grep -rn bar").is_none());
}

#[cfg(unix)]
fn project_with_policy(policy: &str) -> tempfile::TempDir {
    let root = tempfile::tempdir().expect("project");
    std::fs::create_dir_all(root.path().join(".git")).expect(".git");
    std::fs::create_dir_all(root.path().join(".rustcode")).expect(".rustcode");
    std::fs::write(root.path().join(".rustcode/command-policy.toml"), policy).expect("policy");
    root
}

#[cfg(unix)]
#[test]
fn the_command_guard_refuses_a_command_the_project_policy_denies() {
    let root = project_with_policy(
        "[[rule]]\nname = \"no-echo\"\naction = \"deny\"\nprogram = \"echo\"\nmessage = \"Print with printf.\"\n",
    );
    let error = super::run_command_output_with_workspace(
        &serde_json::json!({"command": "cd . && echo hi"}),
        Some(root.path().to_path_buf()),
    )
    .expect_err("echo is denied");
    assert!(error.contains("`no-echo`"), "{error}");
    assert!(error.contains("Print with printf."), "{error}");
}

#[cfg(unix)]
#[test]
fn the_command_guard_runs_a_rewritten_command_and_says_so() {
    let root = project_with_policy(
        "[[rule]]\nname = \"mark\"\naction = \"rewrite\"\nprogram = \"printenv\"\nprefix = \"env GUARD_MARK=yes\"\n",
    );
    let output = super::run_command_output_with_workspace(
        &serde_json::json!({"command": "printenv GUARD_MARK"}),
        Some(root.path().to_path_buf()),
    )
    .expect("printenv runs");
    assert!(output.success, "{}", output.content);
    assert!(output.content.contains("`mark`"), "{}", output.content);
    assert!(output.content.contains("yes"), "{}", output.content);
}

#[test]
fn a_search_that_finds_nothing_is_an_answer_not_a_failure() {
    let output = super::run_command_output_with_workspace(
        &serde_json::json!({"command": "grep rustcode-no-such-line /dev/null"}),
        None,
    )
    .expect("grep runs");
    assert!(output.success, "{}", output.content);
    assert!(
        output.content.contains("(no matches)"),
        "{}",
        output.content
    );

    // A real error (exit 2) still fails.
    let output = super::run_command_output_with_workspace(
        &serde_json::json!({"command": "grep foo /rustcode/no/such/file"}),
        None,
    )
    .expect("grep runs");
    assert!(!output.success, "{}", output.content);
}
use super::sandbox;
#[cfg(unix)]
use super::terminate_background_pid;
use super::{
    TaskOutputTail, cancel_result_message, command_confirmation_preview,
    command_confirmation_scope, command_manages_own_background_jobs, command_requires_confirmation,
    has_interactive_sudo, has_shell_background_operator, manage_task_tool, output_tail,
    pull_request_base, reject_broad_git_stage, run_command, run_command_output,
    run_command_output_cancellable, run_command_output_with_progress, task_event_to_tool_output,
    task_tail_bytes, wait_for_background_tasks,
};
use std::io::Write;

#[test]
fn run_command_description_matches_the_shell_of_each_platform() {
    use super::{RUN_COMMAND, run_command_description};

    let posix = run_command_description(false);
    assert!(posix.starts_with("Run one shell command; returns stdout, stderr and exit code."));
    assert!(posix.contains("`git worktree add` under /tmp only"));
    assert!(!posix.contains("PowerShell"));
    #[cfg(not(target_os = "windows"))]
    assert_eq!(RUN_COMMAND.description, posix);

    let windows = run_command_description(true);
    assert!(windows.starts_with("Run one PowerShell command;"));
    for fact in [
        "PowerShell 7 (pwsh)",
        "Windows PowerShell 5.1",
        "chain with `;`",
        "no heredocs",
        "$env:NAME = 'value'",
        "`curl.exe`",
        "view_file, grep, glob and list_directory",
        "under `$env:TEMP`",
    ] {
        assert!(windows.contains(fact), "missing {fact:?}");
    }
    // Nothing POSIX-only may leak into the Windows guidance.
    for posix_only in ["/tmp", "Pipelines fail", "containing '&'", "cat/sed/head"] {
        assert!(!windows.contains(posix_only), "leaked {posix_only:?}");
    }
    assert_eq!(RUN_COMMAND.name, "run_command");
}

#[cfg(unix)]
#[test]
fn background_termination_kills_the_command_process_group() {
    use std::os::unix::process::CommandExt;

    let mut command = crate::platform::command("sh");
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

fn task_request(
    command: &str,
    cwd: Option<std::path::PathBuf>,
) -> rustcode_command::CommandRequest {
    rustcode_command::CommandRequest {
        command: command.to_owned(),
        status_command: None,
        sandboxed_shell: false,
        cwd,
        env: Vec::new(),
        timeout: std::time::Duration::from_secs(5),
        process_group: true,
        inherited_fds: Vec::new(),
    }
}

#[cfg(target_os = "macos")]
#[test]
fn explicit_workspace_runs_without_thread_local_context() {
    let workspace = tempfile::tempdir().expect("temporary workspace");
    let result = super::run_command_output_with_workspace(
        &serde_json::json!({"command": "pwd"}),
        Some(workspace.path().to_path_buf()),
    )
    .expect("explicit workspace permits sandbox construction");

    assert!(result.success, "{}", result.content);
    assert!(
        result
            .content
            .contains(&workspace.path().display().to_string()),
        "command did not run in the explicit workspace: {}",
        result.content
    );
}

#[cfg(target_os = "macos")]
#[test]
fn explicit_missing_workspace_fails_closed() {
    super::super::set_active_workspace_context(
        None,
        None,
        false,
        Some(crate::config::SandboxMode::WorkspaceWrite),
    );
    let result =
        super::run_command_output_with_workspace(&serde_json::json!({"command": "pwd"}), None);

    super::super::set_active_workspace_context(None, None, false, None);
    assert!(
        result
            .expect_err("explicitly missing workspace must fail closed")
            .contains("needs an active workspace")
    );
}

#[cfg(target_os = "macos")]
#[test]
fn explicit_deleted_workspace_fails_closed() {
    let workspace = tempfile::tempdir().expect("temporary workspace");
    let path = workspace.path().to_path_buf();
    drop(workspace);

    let error = super::run_command_output_with_workspace(
        &serde_json::json!({"command": "pwd"}),
        Some(path),
    )
    .expect_err("deleted workspace must fail closed");

    assert!(
        error.contains("cwd") && error.contains("not a directory"),
        "{error}"
    );
}

/// The originating session's core complaint: under a restricted mode `gh`
/// could not reach the API and the failure was reported as a bare
/// `error connecting to api.github.com`. Run both modes against the same
/// command so the difference is the mode, not the host (#1540).
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn trusted_mode_reaches_the_network_and_writes_outside_the_workspace() {
    if !sandbox::runtime_tests_available() {
        eprintln!("skipping trusted-mode runtime assertion: sandbox probe unavailable");
        return;
    }
    if crate::platform::command("python3")
        .arg("-c")
        .arg("pass")
        .status()
        .is_err()
    {
        eprintln!("skipping trusted-mode runtime assertion: python3 is unavailable");
        return;
    }
    let workspace = tempfile::tempdir().expect("temporary workspace");
    let outside = tempfile::tempdir().expect("temporary outside directory");
    let outside_probe = outside.path().join("probe.txt");
    let (echo_tx, echoed) = std::sync::mpsc::channel();
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind loopback listener");
    let port = listener.local_addr().expect("listener address").port();
    // Nonblocking accept with a deadline: a denied connect must fail the
    // assertion, never hang the suite.
    listener
        .set_nonblocking(true)
        .expect("set the listener nonblocking");
    let accept = std::thread::spawn(move || {
        use std::io::{Read, Write};
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if std::time::Instant::now() >= deadline {
                        let _ = echo_tx.send(Err("no connection within 30s".to_string()));
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                Err(error) => {
                    let _ = echo_tx.send(Err(error.to_string()));
                    return;
                }
            }
        };
        let mut buffer = [0_u8; 8];
        match stream.read(&mut buffer) {
            Ok(read) => {
                let _ = stream.write_all(&buffer[..read]);
                let _ = echo_tx.send(Ok(String::from_utf8_lossy(&buffer[..read]).into_owned()));
            }
            Err(error) => {
                let _ = echo_tx.send(Err(error.to_string()));
            }
        }
    });
    let command = format!(
            "printf trusted > '{}' && printf ping | python3 -c {script}",
            outside_probe.display(),
            script = format!(
                "'{script}'",
                script = format!(
                    "import socket,sys\ns=socket.create_connection((\"127.0.0.1\",{port}),timeout=10)\ns.sendall(sys.stdin.buffer.read())\nsys.stdout.write(s.recv(4).decode())\n"
                )
                .replace('\'', "'\\''")
            )
        );
    let args = serde_json::json!({"command": command, "timeout_ms": 45_000});

    super::super::set_active_workspace_context(
        Some(workspace.path().to_path_buf()),
        Some(workspace.path().to_path_buf()),
        false,
        Some(crate::config::SandboxMode::Trusted),
    );
    let trusted = run_command_output(&args).expect("trusted command should run");
    let echo_result = echoed
        .recv_timeout(std::time::Duration::from_secs(35))
        .expect("loopback echo must complete");
    accept.join().expect("loopback echo thread");
    super::super::set_active_workspace_context(
        Some(workspace.path().to_path_buf()),
        Some(workspace.path().to_path_buf()),
        false,
        Some(crate::config::SandboxMode::WorkspaceWrite),
    );
    let restricted = run_command_output(&args).expect("restricted command should run");
    super::super::set_active_workspace_context(None, None, false, None);

    assert!(
        trusted.success,
        "trusted mode must reach the network and write outside the workspace: {}",
        trusted.content
    );
    assert_eq!(echo_result.expect("loopback echo"), "ping");
    assert_eq!(
        std::fs::read_to_string(&outside_probe).expect("trusted write"),
        "trusted"
    );

    assert!(
        !restricted.success,
        "the default mode must still deny this: {}",
        restricted.content
    );
    assert!(
        restricted.content.contains("[harness:"),
        "a restricted failure must name the sandbox: {}",
        restricted.content
    );
    let _ = std::fs::remove_file(&outside_probe);
}

#[test]
fn restricted_failures_name_the_sandbox_and_trusted_failures_do_not() {
    let workspace = tempfile::tempdir().expect("temporary workspace");
    let outside = tempfile::tempdir().expect("temporary outside directory");
    let outside_probe = outside.path().join("probe.txt");
    let command = format!("printf x > '{}'", outside_probe.display());

    for (mode, expect_attribution) in [
        (crate::config::SandboxMode::ReadOnly, true),
        (crate::config::SandboxMode::WorkspaceWrite, true),
        (crate::config::SandboxMode::WorkspaceWriteNetwork, true),
        (crate::config::SandboxMode::Trusted, false),
    ] {
        super::super::set_active_workspace_context(
            Some(workspace.path().to_path_buf()),
            Some(workspace.path().to_path_buf()),
            false,
            Some(mode),
        );
        let result = super::run_command_output_with_workspace(
            &serde_json::json!({"command": command}),
            Some(workspace.path().to_path_buf()),
        )
        .expect("command should run");
        super::super::set_active_workspace_context(None, None, false, None);

        if expect_attribution && cfg!(any(target_os = "linux", target_os = "macos")) {
            assert!(
                result
                    .content
                    .contains(&format!("OS sandbox mode {}", mode.as_str())),
                "{mode} must name itself: {}",
                result.content
            );
        } else {
            assert!(
                !result.content.contains("[harness:"),
                "trusted mode is unwrapped, so it must not blame a sandbox: {}",
                result.content
            );
        }
    }
}

#[test]
fn effective_writable_roots_mirror_the_launcher_policy() {
    let workspace = std::path::PathBuf::from("/workspace");
    let scratch = std::path::PathBuf::from("/scratch");
    let one_shot = std::path::PathBuf::from("/one-shot");
    let roots = vec![workspace.clone(), scratch.clone()];

    assert_eq!(
        super::effective_writable_roots(&roots, &[one_shot.clone()], true),
        vec![workspace.clone(), scratch.clone(), one_shot.clone()]
    );
    assert_eq!(
        super::effective_writable_roots(&roots, &[one_shot.clone()], false),
        vec![one_shot.clone()],
        "a read-only mode still permits approved one-shot roots"
    );
    assert_eq!(
        super::effective_writable_roots(&roots, &[workspace.clone()], true),
        roots,
        "a one-shot grant inside an existing root is not listed twice"
    );
}

/// Trusted removes OS sandbox wrapping only. The approval decision is
/// computed before the sandbox mode is read — `authorize_tool_with_args`
/// takes no mode argument at all — so the decision, the scope shown to the
/// user, and the one-shot escalation rule must be identical in every mode
/// (#1540).
#[test]
fn trusted_mode_does_not_bypass_the_shell_approval_policy() {
    use crate::config::{AgentMode, SandboxMode};
    use crate::tools::AuthorizationDecision;

    for command in [
        "git push --force origin main",
        "cargo publish",
        "gh pr merge 1531 --squash",
        "npm install",
    ] {
        let args = serde_json::json!({"command": command});
        assert!(command_requires_confirmation(&args), "{command}");
        assert_eq!(
            super::super::authorize_tool_with_args(
                "run_command",
                &args,
                AgentMode::Build,
                false,
                false,
            ),
            AuthorizationDecision::RequireConfirmation,
            "{command} must still need approval in trusted mode"
        );
        // The card the user approves is the same shape in every mode and
        // still states the trusted permissions explicitly.
        let trusted_preview =
            command_confirmation_preview(command, SandboxMode::Trusted, false, None);
        let write_preview =
            command_confirmation_preview(command, SandboxMode::WorkspaceWrite, false, None);
        assert!(trusted_preview.contains("scope:"), "{trusted_preview}");
        assert!(
            trusted_preview.contains("effective OS permissions: trusted process permissions"),
            "{trusted_preview}"
        );
        assert_eq!(
            trusted_preview
                .lines()
                .filter(|line| !line.starts_with("effective OS permissions"))
                .collect::<Vec<_>>(),
            write_preview
                .lines()
                .filter(|line| !line.starts_with("effective OS permissions"))
                .collect::<Vec<_>>(),
            "{command} must be approved on the same terms in trusted mode"
        );
    }

    // YOLO also approves one-shot permission requests.
    for args in [
        serde_json::json!({"command": "gh pr list", "network_access": true}),
        serde_json::json!({"command": "cargo fmt", "filesystem_write_path": "/tmp/other"}),
    ] {
        assert_eq!(
            super::super::authorize_tool_with_args(
                "run_command",
                &args,
                AgentMode::Build,
                true,
                false,
            ),
            AuthorizationDecision::Allow,
            "{args} must be auto-approved in YOLO"
        );
    }
}

#[test]
fn task_manager_root_adapter_keeps_sessions_isolated() {
    let manager = rustcode_tasks::TaskManager::new(std::sync::Arc::new(|_| true));
    let session_a = manager.subscribe_session("root-session-a");
    let session_b = manager.subscribe_session("root-session-b");
    let hold_open = if cfg!(target_os = "windows") {
        "Start-Sleep -Seconds 1"
    } else {
        "sleep 1"
    };
    let first = manager
        .spawn_with_id(
            "root-session-task-a",
            rustcode_tasks::TaskSpec::new("root-session-a", task_request(hold_open, None)),
        )
        .unwrap();
    let second = manager
        .spawn_with_id(
            "root-session-task-b",
            rustcode_tasks::TaskSpec::new("root-session-b", task_request(hold_open, None)),
        )
        .unwrap();

    assert_eq!(manager.list("root-session-a").len(), 1);
    assert_eq!(manager.list("root-session-a")[0].id, *first.id());
    assert_eq!(manager.list("root-session-b").len(), 1);
    assert_eq!(manager.list("root-session-b")[0].id, *second.id());
    let first_event = session_a.recv().unwrap();
    assert_eq!(first_event.session_id().as_str(), "root-session-a");
    assert_eq!(
        session_b.recv().unwrap().session_id().as_str(),
        "root-session-b"
    );
}

#[cfg(unix)]
#[test]
fn wait_any_returns_first_completion_without_consuming_or_cancelling_others() {
    let manager =
        rustcode_tasks::TaskManager::new(std::sync::Arc::new(super::RootProcessTerminator));
    let session = "wait-any-session";
    let fast = manager
        .spawn_with_id(
            "wait-any-fast",
            rustcode_tasks::TaskSpec::new(session, task_request("sleep 0.2", None)),
        )
        .unwrap();
    let slow = manager
        .spawn_with_id(
            "wait-any-slow",
            rustcode_tasks::TaskSpec::new(session, task_request("sleep 30", None)),
        )
        .unwrap();

    let result = wait_for_background_tasks(
        &manager,
        session,
        &[fast.id().to_string(), slow.id().to_string()],
        std::time::Duration::from_secs(5),
    );

    assert!(result.contains("wait-any-fast"), "{result}");
    assert!(
        manager
            .list(session)
            .iter()
            .any(|task| task.id == *slow.id())
    );
    assert_eq!(
        manager.cancel(slow.id()),
        rustcode_tasks::CancelResult::Cancelled
    );
}

#[cfg(unix)]
#[test]
fn wait_streams_a_running_tasks_output_tail_while_blocked() {
    // A wait used to stay silent until it returned, so its card sat empty
    // for the whole command (#2230). It now forwards appended log bytes.
    let manager =
        rustcode_tasks::TaskManager::new(std::sync::Arc::new(super::RootProcessTerminator));
    let session = "wait-tail-session";
    let log = std::env::temp_dir().join(format!(
        "rustcode-wait-tail-{}-{}.log",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    // The manager creates the log itself (`create_new`), so the test only
    // appends to it the way a running command would.
    let task = manager
        .spawn_with_id(
            "wait-tail-task",
            rustcode_tasks::TaskSpec::new(session, task_request("sleep 30", None))
                .with_output_log(log.clone()),
        )
        .unwrap();

    let seen: std::sync::Arc<std::sync::Mutex<Vec<u8>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = std::sync::Arc::clone(&seen);
    super::super::set_active_tool_progress(Some(std::sync::Arc::new(
        move |bytes: &[u8], _stderr: bool| {
            sink.lock().unwrap().extend_from_slice(bytes);
        },
    )));

    let mut tail = TaskOutputTail::new(&manager, session, &[task.id().to_string()]);
    // The log already held the stream label; the wait forwards from here on.
    tail.poll();
    assert!(seen.lock().unwrap().is_empty(), "nothing new yet");

    std::fs::OpenOptions::new()
        .append(true)
        .open(&log)
        .unwrap()
        .write_all(b"running 3 tests\n")
        .unwrap();
    tail.poll();

    let forwarded = String::from_utf8(seen.lock().unwrap().clone()).unwrap();
    assert!(forwarded.contains("running 3 tests"), "{forwarded:?}");
    // Reading again forwards nothing twice.
    tail.poll();
    assert_eq!(
        String::from_utf8(seen.lock().unwrap().clone()).unwrap(),
        forwarded
    );

    super::super::set_active_tool_progress(None);
    // Cancellation is asynchronous: either outcome is a request the task
    // manager owns, exactly as the sibling wait tests assert.
    assert!(matches!(
        manager.cancel(task.id()),
        rustcode_tasks::CancelResult::Requested | rustcode_tasks::CancelResult::Cancelled
    ));
    let _ = std::fs::remove_file(&log);
}

#[test]
fn a_wait_without_a_live_cell_forwards_nothing() {
    // Outside a turn there is no card to stream into, and the wait must not
    // read logs it has nowhere to put.
    let manager =
        rustcode_tasks::TaskManager::new(std::sync::Arc::new(super::RootProcessTerminator));
    let session = "wait-tail-absent-session";
    super::super::set_active_tool_progress(None);
    assert!(super::super::current_tool_progress().is_none());
    let mut tail = TaskOutputTail::new(&manager, session, &["missing".to_owned()]);
    assert!(tail.logs.is_empty());
    tail.poll();
}

#[tokio::test]
async fn completion_sink_defers_to_a_wait_in_flight() {
    let session = "wait-in-flight-session";
    let in_flight = super::WaitInFlight::new(session, &["waited".to_owned()]);
    let answered = tokio::spawn(super::background_result_answered_by_wait(session, "waited"));
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(
        !answered.is_finished(),
        "the sink must let the wait decide first"
    );
    super::note_wait_delivered(session, "waited");
    drop(in_flight);

    assert!(answered.await.expect("sink check finishes"));
    assert!(!super::background_result_answered_by_wait(session, "unwaited").await);
}

#[test]
fn task_manager_root_adapter_cancels_before_pid_without_duplicate_terminal() {
    let manager = rustcode_tasks::TaskManager::new(std::sync::Arc::new(|_| true));
    let events = manager.subscribe();
    let task = manager
        .spawn_with_id(
            "root-cancel-before-pid",
            rustcode_tasks::TaskSpec::new(
                "root-cancel-session",
                task_request(
                    "printf never-starts",
                    Some(std::path::PathBuf::from("/path/that/does/not/exist")),
                ),
            ),
        )
        .unwrap();
    let result = manager.cancel(task.id());
    assert!(matches!(
        result,
        rustcode_tasks::CancelResult::Requested | rustcode_tasks::CancelResult::Cancelled
    ));

    let mut terminal_events = 0;
    while let Ok(event) = events.recv() {
        if event.is_terminal() {
            terminal_events += 1;
            assert_eq!(event.task_id(), task.id());
            break;
        }
    }
    assert_eq!(terminal_events, 1);
    assert!(manager.list("root-cancel-session").is_empty());
}

#[test]
fn task_manager_root_adapter_converts_one_completion_once() {
    let manager = rustcode_tasks::TaskManager::new(std::sync::Arc::new(|_| true));
    let events = manager.subscribe();
    let task = manager
        .spawn_with_id(
            "root-completion-once",
            rustcode_tasks::TaskSpec::new(
                "root-completion-session",
                task_request(
                    if cfg!(target_os = "windows") {
                        "echo done"
                    } else {
                        "printf done"
                    },
                    None,
                ),
            ),
        )
        .unwrap();
    let mut terminal_events = Vec::new();
    while let Ok(event) = events.recv() {
        if event.is_terminal() {
            terminal_events.push(event);
            break;
        }
    }
    assert_eq!(terminal_events.len(), 1);
    let (task_id, session_id, converted) =
        task_event_to_tool_output(terminal_events.pop().unwrap()).expect("finished output");
    assert_eq!(task_id, task.id().as_str());
    assert_eq!(session_id, "root-completion-session");
    assert!(converted.success);
    assert_eq!(converted.command.as_deref(), Some("printf done"));
    assert!(converted.content.contains("done"));
}

#[test]
fn task_event_root_adapter_preserves_cancelled_metadata() {
    let event = rustcode_tasks::TaskEvent::Cancelled {
        id: rustcode_tasks::TaskId::new("cancelled-task"),
        session_id: rustcode_tasks::SessionId::new("cancelled-session"),
        call_id: None,
        command: "cargo test".to_string(),
        notify_on_complete: true,
    };

    let (task_id, session_id, output) = task_event_to_tool_output(event).expect("cancelled");
    assert_eq!(task_id, "cancelled-task");
    assert_eq!(session_id, "cancelled-session");
    assert!(!output.success);
    assert_eq!(output.command.as_deref(), Some("cargo test"));
    assert_eq!(
        output.error_kind,
        Some(crate::tools::ToolErrorKind::Cancelled)
    );
    assert!(
        output.content.contains("Background task cancelled"),
        "{}",
        output.content
    );
    assert!(output.content.contains("no process exit code"));
}

#[test]
fn manage_task_kill_cannot_cross_session_boundaries() {
    let owner = "manage-owner-session";
    let other = "manage-other-session";
    let task_id = "manage-owned-task";
    crate::tools::set_active_session_id(Some(owner.to_owned()));
    crate::tools::spawn_background_task_for_test(
        task_id,
        owner,
        if cfg!(target_os = "windows") {
            "Start-Sleep -Seconds 30"
        } else {
            "sleep 30"
        },
    )
    .unwrap();
    for _ in 0..100 {
        if crate::tools::background_task_snapshots(owner)
            .first()
            .is_some_and(|task| task.child_pid.is_some())
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    crate::tools::set_active_session_id(Some(other.to_owned()));
    let result = manage_task_tool(&serde_json::json!({
        "action": "kill",
        "task_id": task_id,
    }));
    assert_eq!(result, Err(format!("Task '{task_id}' not found.")));
    assert_eq!(crate::tools::background_task_snapshots(owner).len(), 1);

    crate::tools::set_active_session_id(Some(owner.to_owned()));
    crate::tools::stop_background_tasks(owner);
    crate::tools::set_active_session_id(None);
}

#[test]
fn manage_task_wait_unknown_task_reports_not_running() {
    let session = "manage-wait-unknown-session";
    crate::tools::set_active_session_id(Some(session.to_owned()));
    let result = manage_task_tool(&serde_json::json!({
        "action": "wait",
        "task_id": "no-such-task",
        "timeout_ms": 1000,
    }));
    assert_eq!(
        result,
        Ok("None of the requested tasks are running or retained: no-such-task".to_string())
    );
    crate::tools::set_active_session_id(None);
}

#[test]
fn manage_task_wait_returns_finished_output() {
    let session = "manage-wait-done-session";
    let task_id = "manage-wait-done-task";
    crate::tools::set_active_session_id(Some(session.to_owned()));
    crate::tools::spawn_background_task_for_test(
        task_id,
        session,
        if cfg!(target_os = "windows") {
            "Start-Sleep -Seconds 2"
        } else {
            "sleep 2"
        },
    )
    .unwrap();
    let result = manage_task_tool(&serde_json::json!({
        "action": "wait",
        "task_id": task_id,
        "timeout_ms": 30000,
    }));
    let output = result.expect("wait should return the finished result");
    assert!(
        output.contains("exited successfully (code 0)"),
        "unexpected wait output: {output}"
    );
    crate::tools::stop_background_tasks(session);
    crate::tools::set_active_session_id(None);
}

#[test]
fn manage_task_wait_timeout_reports_still_running() {
    let session = "manage-wait-timeout-session";
    let task_id = "manage-wait-timeout-task";
    crate::tools::set_active_session_id(Some(session.to_owned()));
    crate::tools::spawn_background_task_for_test(
        task_id,
        session,
        if cfg!(target_os = "windows") {
            "Start-Sleep -Seconds 30"
        } else {
            "sleep 30"
        },
    )
    .unwrap();
    let result = manage_task_tool(&serde_json::json!({
        "action": "wait",
        "task_id": task_id,
        "timeout_ms": 1000,
    }));
    let output = result.expect("wait should report still running");
    assert!(
        output.contains("still running"),
        "unexpected wait output: {output}"
    );
    // The report names the task and the way on; a timed-out wait leaves the task running.
    assert!(output.contains(task_id), "{output}");
    assert!(output.contains("manage_task kill"), "{output}");
    assert!(
        crate::tools::background_task_snapshots(session)
            .iter()
            .any(|task| task.id == task_id),
        "a wait that timed out must not stop the task"
    );
    crate::tools::stop_background_tasks(session);
    crate::tools::set_active_session_id(None);
}

#[test]
fn manage_task_wait_stops_when_the_turn_is_cancelled() {
    let session = "manage-wait-cancel-session";
    let task_id = "manage-wait-cancel-task";
    crate::tools::set_active_session_id(Some(session.to_owned()));
    crate::tools::spawn_background_task_for_test(
        task_id,
        session,
        if cfg!(target_os = "windows") {
            "Start-Sleep -Seconds 30"
        } else {
            "sleep 30"
        },
    )
    .unwrap();
    let token = tokio_util::sync::CancellationToken::new();
    token.cancel();
    crate::tools::set_active_tool_cancel(Some(token));
    let started = std::time::Instant::now();
    let output = manage_task_tool(&serde_json::json!({
        "action": "wait",
        "task_id": task_id,
        "timeout_ms": 30_000,
    }))
    .expect("a cancelled wait reports, it does not fail");
    crate::tools::set_active_tool_cancel(None);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "the wait held the thread for {:?} after the turn was cancelled",
        started.elapsed()
    );
    assert!(output.contains("Stopped waiting"), "{output}");
    assert!(
        crate::tools::background_task_snapshots(session)
            .iter()
            .any(|task| task.id == task_id),
        "stopping a wait must not stop the task"
    );
    crate::tools::stop_background_tasks(session);
    crate::tools::set_active_session_id(None);
}

#[test]
fn manage_task_requested_kill_explains_asynchronous_termination() {
    assert_eq!(
            cancel_result_message(
                "task-starting",
                rustcode_tasks::CancelResult::Requested
            ),
            Ok("Task 'task-starting' termination requested; it will be cancelled when its process starts.".to_string())
        );
}

#[test]
fn run_command_reports_stdout_and_stderr_while_running() {
    if !sandbox::runtime_tests_available() {
        return;
    }
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

#[cfg(unix)]
#[test]
fn cancellable_run_command_returns_one_cancelled_result() {
    if !sandbox::runtime_tests_available() {
        return;
    }
    let token = tokio_util::sync::CancellationToken::new();
    let trigger = token.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(30));
        trigger.cancel();
    });
    let output =
        run_command_output_cancellable(&serde_json::json!({"command": "sleep 0.3"}), Some(token))
            .expect("cancellation should be represented as a tool result");

    assert!(!output.success);
    assert_eq!(
        output.error_kind,
        Some(super::super::ToolErrorKind::Cancelled)
    );
    assert_eq!(output.content, "error: tool execution cancelled by user");
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
fn pull_request_base_is_parsed_only_for_create_commands() {
    assert_eq!(
        pull_request_base("gh pr create --base main --title change"),
        Some("main".to_string())
    );
    assert_eq!(
        pull_request_base("gh pr create --base=release"),
        Some("release".to_string())
    );
    assert_eq!(pull_request_base("gh pr list --base main"), None);
}

#[test]
fn pr_creation_is_guarded_when_origin_has_no_base() {
    let root = tempfile::tempdir().expect("temporary repository");
    let status = crate::platform::command("git")
        .args(["init", "-q"])
        .current_dir(root.path())
        .status()
        .expect("git init");
    assert!(status.success());

    assert!(super::remote_base_missing(Some(root.path()), "main"));
    let output = super::pr_creation_guard_output("gh pr create --base main", "main");
    assert!(!output.success);
    assert!(!output.retryable);
    assert!(output.content.contains("was not run"));
}

#[test]
fn run_command_executes_chained_shell_commands() {
    if !sandbox::runtime_tests_available() {
        return;
    }
    let result = run_command(&serde_json::json!({
        "command": "printf one; printf two"
    }))
    .expect("shell command should succeed");

    assert!(result.contains("exit code: 0"));
    assert!(result.contains("onetwo"));
}

#[test]
fn run_command_supports_conditional_chaining() {
    if !sandbox::runtime_tests_available() {
        return;
    }
    let result = run_command(&serde_json::json!({
        "command": "printf first && printf second"
    }))
    .expect("shell command should succeed");

    assert!(result.contains("firstsecond"));
}

#[cfg(not(target_os = "windows"))]
#[test]
fn run_command_propagates_pipeline_failures() {
    if !sandbox::runtime_tests_available() {
        return;
    }
    let result = run_command(&serde_json::json!({
        "command": "false | tail -n 1"
    }))
    .expect("run_command reports command failure in its output");

    assert!(result.contains("exit code: 1"), "{result}");
}

#[test]
fn home_traversal_timeout_hint_only_follows_a_home_wide_timeout() {
    use super::{HOME_TRAVERSAL_TIMEOUT_HINT, with_home_traversal_hint};
    let timeout = "command timed out after 20000 ms and was killed".to_string();
    let hinted = with_home_traversal_hint(timeout.clone(), true);
    assert!(hinted.starts_with(&timeout));
    assert!(hinted.ends_with(HOME_TRAVERSAL_TIMEOUT_HINT));
    assert_eq!(with_home_traversal_hint(timeout.clone(), false), timeout);
    let spawn = "failed to spawn process: no such file".to_string();
    assert_eq!(with_home_traversal_hint(spawn.clone(), true), spawn);
}

#[cfg(unix)]
#[test]
fn a_stalled_home_wide_walk_times_out_with_traversal_guidance() {
    if !sandbox::runtime_tests_available() {
        return;
    }
    // `sleep` stands in for a walk blocked on a protected folder (#2022).
    let error = run_command_output(&serde_json::json!({
        "command": "find ~ -maxdepth 0 >/dev/null 2>&1; sleep 5",
        "timeout_ms": 200,
    }))
    .expect_err("the stalled walk must time out");
    assert!(
        error.starts_with("command timed out after 200 ms and was killed"),
        "{error}"
    );
    assert!(
        error.contains(super::HOME_TRAVERSAL_TIMEOUT_HINT),
        "{error}"
    );

    let scoped = run_command_output(&serde_json::json!({
        "command": "sleep 5",
        "timeout_ms": 200,
    }))
    .expect_err("a plain sleep must time out");
    assert!(!scoped.contains("hint:"), "{scoped}");
}

#[test]
fn command_execution_metadata_classifies_nonzero_exit_only() {
    if !sandbox::runtime_tests_available() {
        return;
    }
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

#[cfg(unix)]
#[test]
fn command_result_envelope_marks_sigpipe_as_downstream_completion() {
    if !sandbox::runtime_tests_available() {
        return;
    }
    let output = run_command_output(&serde_json::json!({
        "command": "yes | head -n 1"
    }))
    .expect("pipeline should return a structured command result");

    assert!(output.success, "{}", output.content);
    let status = output.command_status.expect("command status metadata");
    assert!(status.completed);
    assert_eq!(status.exit_code, Some(141));
    assert_eq!(status.signal, Some(libc::SIGPIPE));
    assert!(status.downstream_consumer_terminated);
    assert!(output.content.contains("SIGPIPE"));
    assert!(
        output
            .content
            .contains("output_truncated_by_rustcode=false")
    );
}

#[test]
fn command_result_envelope_marks_a_successful_completion() {
    if !sandbox::runtime_tests_available() {
        return;
    }
    let output = run_command_output(&serde_json::json!({
        "command": "printf complete"
    }))
    .expect("command should return a structured result");

    let status = output.command_status.expect("command status metadata");
    assert!(output.success);
    assert!(status.completed);
    assert_eq!(status.exit_code, Some(0));
    assert_eq!(status.bytes_returned, 8);
    assert_eq!(status.total_output_bytes, Some(8));
    assert!(!status.output_truncated);
}

#[test]
fn background_command_start_is_pending_and_names_command() {
    if !sandbox::runtime_tests_available() {
        return;
    }
    let command = "sleep 3; printf background-output";
    let output = run_command_output(&serde_json::json!({
        "command": command,
        "background": true,
    }))
    .expect("background command should be accepted");

    assert!(!output.success, "starting is not completed success");
    assert_eq!(output.exit_code, None, "the process has not exited yet");
    assert!(
        output.content.contains("Status: Running"),
        "{}",
        output.content
    );
    assert!(output.content.contains(command), "{}", output.content);
}

#[cfg(unix)]
#[test]
fn shell_background_operator_detection_ignores_quotes_redirections_and_and() {
    assert!(has_shell_background_operator(
        "python3 -m http.server 8080 &"
    ));
    assert!(has_shell_background_operator("server & echo ready"));
    for command in [
        "printf '&'",
        "printf \"&\"",
        "printf escaped\\&",
        "printf one && printf two",
        "printf out 2>&1",
        "printf out &>file",
    ] {
        assert!(
            !has_shell_background_operator(command),
            "not a background operator: {command}"
        );
    }
}

#[cfg(unix)]
#[test]
fn detached_server_start_is_completed_but_remains_tracked_and_killable() {
    if !sandbox::runtime_tests_available() {
        return;
    }
    let session_id = format!(
        "detached-server-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    super::super::set_active_session_id(Some(session_id.clone()));
    let output = run_command_output(&serde_json::json!({
        "command": "sleep 30",
        "background": true,
        "detached": true,
    }))
    .expect("detached server should start");
    let snapshots = super::super::background_task_snapshots(&session_id);
    let stop = super::super::stop_background_tasks(&session_id);
    super::super::set_active_session_id(None);

    assert!(output.success);
    assert!(!output.pending);
    assert_eq!(output.exit_code, None);
    assert!(output.content.contains("Detached task started"));
    assert!(output.content.contains("Task ID:"));
    assert_eq!(snapshots.len(), 1, "detached task was not retained");
    assert!(
        snapshots[0].child_pid.is_some(),
        "detached task never started"
    );
    assert_eq!(stop.stopped, 1, "detached task was not terminated");
}

#[cfg(unix)]
#[test]
fn self_managed_background_scripts_are_exempt_from_auto_detach() {
    // Shape of the frozen session's lost verification: start a server,
    // probe it, then tear it down in one compound command. The embedded
    // `&` is real, but the script reaps its own job, so its output must
    // be preserved instead of detached into the void.
    let verify = "bun run dev > /tmp/rc-dev.log 2>&1 &\nDEV_PID=$!\nfor i in $(seq 1 30); do\n  if grep -q \"Ready\" /tmp/rc-dev.log 2>/dev/null; then break; fi\n  sleep 1\ndone\ncurl -s \"http://localhost:3000/api/releases\" | head -c 100\necho \"\"\nkill $DEV_PID 2>/dev/null\npkill -f \"next dev\" 2>/dev/null\necho \"done\"";
    assert!(has_shell_background_operator(verify));
    assert!(command_manages_own_background_jobs(verify));
    for command in [
        "sleep 0.1 & pid=$!; kill $pid; echo reaped",
        "sleep 0.1 & wait $!; echo done",
        "server & client; wait; echo done",
    ] {
        assert!(
            command_manages_own_background_jobs(command),
            "self-managed: {command}"
        );
    }
    for command in [
        "sleep 30 &",
        "server & echo ready",
        "printf 'wait $!' &",
        "echo \"wait\"",
        "printf out 2>&1",
    ] {
        assert!(
            !command_manages_own_background_jobs(command),
            "not self-managed: {command}"
        );
    }
}

#[cfg(unix)]
#[test]
fn self_managed_background_script_runs_in_foreground_with_output() {
    if !sandbox::runtime_tests_available() {
        return;
    }
    let session_id = format!(
        "self-managed-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    super::super::set_active_session_id(Some(session_id.clone()));
    let output = run_command_output(&serde_json::json!({
        "command": "sleep 0.2 & pid=$!; wait $pid; echo verified-$pid",
    }))
    .expect("self-managed script should run in the foreground");
    super::super::set_active_session_id(None);

    assert!(
        !output.content.contains("Detached task started"),
        "self-managed script must not detach: {}",
        output.content
    );
    assert_eq!(output.exit_code, Some(0));
    assert!(
        output.content.contains("verified-"),
        "verification output must be preserved: {}",
        output.content
    );
}

#[cfg(unix)]
#[test]
fn shell_background_operator_is_auto_detached_without_background_flag() {
    if !sandbox::runtime_tests_available() {
        return;
    }
    let session_id = format!(
        "nested-background-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    super::super::set_active_session_id(Some(session_id.clone()));
    let output = run_command_output(&serde_json::json!({
        "command": "sleep 30 &",
    }))
    .expect("shell background command should start detached");
    // The pid is recorded by the task's own thread. A second was not
    // always enough under a full parallel test run, and a task without a
    // pid cannot be stopped.
    for _ in 0..1000 {
        if super::super::background_task_snapshots(&session_id)
            .first()
            .is_some_and(|task| task.child_pid.is_some())
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let snapshots = super::super::background_task_snapshots(&session_id);
    let stop = super::super::stop_background_tasks(&session_id);
    // How each task ended, so a rare failure under a full parallel run (#2763)
    // says whether the task exited on its own before the stop reached it.
    let ended: Vec<_> = snapshots
        .iter()
        .map(|task| super::super::background_task_completion(&session_id, &task.id))
        .collect();
    super::super::set_active_session_id(None);

    assert!(output.success);
    assert!(!output.pending);
    assert!(output.content.contains("Detached task started"));
    assert_eq!(
        snapshots.len(),
        1,
        "implicit detached task was not retained"
    );
    // A task whose process is still starting is cancelled by request and
    // reported as `requested`, not `stopped`. Which of the two the stop
    // lands on is a matter of timing; that it lands is what is tested.
    assert_eq!(
        stop.stopped + stop.requested,
        1,
        "implicit detached task was not terminated: output={:?} snapshots={snapshots:?} stop={stop:?} ended={ended:?}",
        output.content
    );
    assert_eq!(stop.failed, 0);
}

#[test]
fn short_discovery_commands_ignore_background_request() {
    if !sandbox::runtime_tests_available() {
        return;
    }
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
    if !sandbox::runtime_tests_available() {
        return;
    }
    let command = "sleep 3";
    let output = run_command_output(&serde_json::json!({
        "command": command,
        "background": true,
    }))
    .expect("long command should be accepted");

    assert!(output.pending, "command was forced synchronous: {command}");
    assert_eq!(output.command.as_deref(), Some(command));
}

fn unique_session(prefix: &str) -> String {
    format!(
        "{prefix}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

fn next_terminal_event(
    events: &rustcode_tasks::TaskSubscription,
    timeout: std::time::Duration,
) -> Option<rustcode_tasks::TaskEvent> {
    let deadline = std::time::Instant::now() + timeout;
    while let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) {
        match events.recv_timeout(remaining) {
            Ok(event) if event.is_terminal() => return Some(event),
            Ok(_) => continue,
            Err(_) => break,
        }
    }
    None
}

#[cfg(unix)]
#[test]
fn fast_background_command_returns_its_foreground_result_without_a_notification() {
    if !sandbox::runtime_tests_available() {
        return;
    }
    let session_id = unique_session("inline-background-test");
    super::super::set_active_session_id(Some(session_id.clone()));
    let events = super::background_task_manager().subscribe_session(session_id.clone());
    let command = "sleep 0.2; printf inline-output; printf inline-error >&2; exit 3";
    let foreground = run_command_output(&serde_json::json!({ "command": command }))
        .expect("foreground command should run");
    let output = run_command_output(&serde_json::json!({
        "command": command,
        "background": true,
        "notify_on_complete": true,
    }))
    .expect("background command should run");
    let snapshots = super::super::background_task_snapshots(&session_id);
    let completions = super::super::recent_background_task_completions(&session_id);
    let late_event = next_terminal_event(&events, std::time::Duration::from_millis(400));
    super::super::set_active_session_id(None);

    assert!(
        output.content.contains("inline-output"),
        "{}",
        output.content
    );
    assert!(
        output.content.contains("inline-error"),
        "{}",
        output.content
    );
    assert!(!output.content.contains("Task ID"), "{}", output.content);
    assert_eq!(output.content, foreground.content);
    assert_eq!(output.exit_code, Some(3));
    assert_eq!(output.success, foreground.success);
    assert_eq!(output.error_kind, foreground.error_kind);
    assert_eq!(output.command, foreground.command);
    assert!(!output.pending);
    assert!(snapshots.is_empty(), "task still listed: {snapshots:?}");
    assert!(
        completions.is_empty(),
        "completion retained: {completions:?}"
    );
    assert!(late_event.is_none(), "notified anyway: {late_event:?}");
}

#[cfg(unix)]
#[test]
fn slow_background_command_still_returns_a_task_id_and_notifies() {
    if !sandbox::runtime_tests_available() {
        return;
    }
    let session_id = unique_session("slow-background-test");
    super::super::set_active_session_id(Some(session_id.clone()));
    let events = super::background_task_manager().subscribe_session(session_id.clone());
    let started = std::time::Instant::now();
    let output = run_command_output(&serde_json::json!({
        "command": "sleep 3; printf slow-output",
        "background": true,
    }))
    .expect("background command should start");
    let waited = started.elapsed();
    let event = next_terminal_event(&events, std::time::Duration::from_secs(20));
    super::super::set_active_session_id(None);

    assert!(output.pending);
    assert!(
        output
            .content
            .contains("Task started in background. Task ID: ")
    );
    assert!(
        waited >= super::BACKGROUND_INLINE_GRACE,
        "returned before the grace period: {waited:?}"
    );
    let event = event.expect("slow task should publish its completion");
    assert!(output.content.contains(event.task_id().as_str()));
    assert_eq!(event.notify_on_complete(), Some(true));
    let (_, _, result) = task_event_to_tool_output(event).expect("finished output");
    assert!(result.content.contains("slow-output"), "{}", result.content);
}

#[cfg(unix)]
#[test]
fn running_foreground_command_can_be_moved_to_the_background() {
    if !sandbox::runtime_tests_available() {
        return;
    }
    let session_id = unique_session("foreground-handoff-test");
    super::super::set_active_session_id(Some(session_id.clone()));
    let events = super::background_task_manager().subscribe_session(session_id.clone());
    assert_eq!(
        super::move_foreground_commands_to_background(&session_id),
        0
    );
    let user = {
        let session_id = session_id.clone();
        std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while !super::has_foreground_command(&session_id) {
                assert!(
                    std::time::Instant::now() < deadline,
                    "command never started"
                );
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            std::thread::sleep(std::time::Duration::from_millis(300));
            super::move_foreground_commands_to_background(&session_id)
        })
    };
    let started = std::time::Instant::now();
    let output = run_command_output(&serde_json::json!({
        "command": "printf 'early\\n'; sleep 3; printf 'late\\n'",
    }))
    .expect("command should start");
    let returned_after = started.elapsed();
    assert_eq!(user.join().expect("user thread"), 1);
    let event = next_terminal_event(&events, std::time::Duration::from_secs(20));
    super::super::set_active_session_id(None);

    assert!(
        returned_after < std::time::Duration::from_millis(2500),
        "the tool call should return at the hand-off: {returned_after:?}"
    );
    assert!(output.pending);
    assert!(
        output
            .content
            .contains("The user moved this running command to the background"),
        "{}",
        output.content
    );
    assert!(!super::has_foreground_command(&session_id));
    let event = event.expect("moved task should publish its completion");
    assert!(output.content.contains(event.task_id().as_str()));
    assert_eq!(event.notify_on_complete(), Some(true));
    let (_, _, result) = task_event_to_tool_output(event).expect("finished output");
    assert!(result.content.contains("early"), "{}", result.content);
    assert!(result.content.contains("late"), "{}", result.content);
}

#[cfg(unix)]
#[test]
fn detached_command_returns_at_once_even_when_it_finishes_quickly() {
    if !sandbox::runtime_tests_available() {
        return;
    }
    let session_id = unique_session("detached-grace-test");
    super::super::set_active_session_id(Some(session_id.clone()));
    let events = super::background_task_manager().subscribe_session(session_id.clone());
    let started = std::time::Instant::now();
    let output = run_command_output(&serde_json::json!({
        "command": "sleep 0.5; printf detached-output",
        "detached": true,
        "notify_on_complete": true,
    }))
    .expect("detached command should start");
    let waited = started.elapsed();
    let event = next_terminal_event(&events, std::time::Duration::from_secs(20));
    super::super::set_active_session_id(None);

    assert!(output.content.contains("Detached task started"));
    assert!(
        waited < std::time::Duration::from_millis(450),
        "detached start waited for the command: {waited:?}"
    );
    let event = event.expect("detached task should still publish its completion");
    assert!(output.content.contains(event.task_id().as_str()));
}

#[test]
fn task_id_arguments_merge_both_fields_and_ignore_empty_ones() {
    use super::{requested_task_ids, single_task_id};
    let ids = |args: serde_json::Value| requested_task_ids(&args);
    assert_eq!(
        ids(serde_json::json!({ "task_id": "a", "task_ids": [] })),
        ["a"]
    );
    assert_eq!(
        ids(serde_json::json!({ "task_ids": ["a", "b"] })),
        ["a", "b"]
    );
    assert_eq!(
        ids(serde_json::json!({ "task_id": "b", "task_ids": ["a", "b", "a", "", 7] })),
        ["b", "a"]
    );
    assert!(ids(serde_json::json!({ "task_id": " ", "task_ids": [""] })).is_empty());
    assert!(ids(serde_json::json!({ "task_id": null, "task_ids": null })).is_empty());

    assert_eq!(
        single_task_id(
            &serde_json::json!({ "task_id": "", "task_ids": ["a"] }),
            "logs"
        ),
        Ok("a".to_owned())
    );
    assert_eq!(
        single_task_id(
            &serde_json::json!({ "task_id": "a", "task_ids": ["a"] }),
            "kill"
        ),
        Ok("a".to_owned())
    );
    assert!(single_task_id(&serde_json::json!({ "task_ids": [] }), "status").is_err());
    assert!(single_task_id(&serde_json::json!({ "task_ids": ["a", "b"] }), "kill").is_err());
}

// A status check of a finished task reads the end of its output, where
// a failing test run names its failures, not the whole output again.
#[test]
fn a_status_tail_keeps_the_end_of_the_output() {
    let output = format!(
        "{}failures:\n    tests::flaky\n",
        "test a ... ok\n".repeat(1_000)
    );
    let tail = output_tail(output.clone(), 64);
    assert!(tail.ends_with("failures:\n    tests::flaky\n"), "{tail}");
    assert!(tail.starts_with('['), "{tail}");
    assert!(tail.len() < 200, "{}", tail.len());
    assert_eq!(output_tail("short".to_owned(), 64), "short");
    assert!(output_tail("é".repeat(100), 3).ends_with('é'));
    assert_eq!(task_tail_bytes(&serde_json::json!({})), 16_384);
    assert_eq!(
        task_tail_bytes(&serde_json::json!({ "tail_bytes": 8_000 })),
        8_000
    );
}

#[test]
fn wait_uses_task_id_when_task_ids_is_empty() {
    let result = manage_task_tool(&serde_json::json!({
        "action": "wait",
        "task_id": "task-id-shadow-test",
        "task_ids": [],
        "timeout_ms": 1000,
    }))
    .expect("wait should accept the single task_id");
    assert!(
        result.contains("task-id-shadow-test") && !result.contains("No task IDs"),
        "{result}"
    );
    assert!(manage_task_tool(&serde_json::json!({ "action": "wait", "task_ids": [] })).is_err());
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
    let preview = command_confirmation_preview(
        command,
        crate::config::SandboxMode::WorkspaceWrite,
        true,
        None,
    );
    assert!(
        preview.contains("resolved command: git status"),
        "preview: {preview}"
    );
    assert!(
        preview.contains("effective OS permissions:"),
        "preview: {preview}"
    );
    assert!(
        preview.contains("network access (one time)"),
        "preview: {preview}"
    );
    let scoped_preview = command_confirmation_preview(
        command,
        crate::config::SandboxMode::ReadOnly,
        false,
        Some("/private/tmp/generated-assets"),
    );
    assert!(scoped_preview.contains("write access to '/private/tmp/generated-assets' (one time)"));
    assert!(scoped_preview.contains("effective OS permissions: read-only"));
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
        "gh api repos/LHagfoss/rustcode/releases?per_page=100 --jq .[].tag_name",
        "gh api 'repos/LHagfoss/rustcode/releases?per_page=100' --jq '[.[] | select(.draft == false)]'",
        "gh api --method GET repos/LHagfoss/rustcode/releases --paginate -q .[].tag_name",
        "gh api --method=GET --hostname github.com repos/LHagfoss/rustcode/releases --jq=.[].tag_name",
        "gh api -XGET repos/LHagfoss/rustcode/releases --jq '[.[] | .tag_name]'",
        "gh release list --limit 10",
        "gh release view v0.56.16 --json tagName",
        "printf '%s' \"$HOME\"",
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
fn gh_api_write_forms_and_release_mutations_still_require_confirmation() {
    for command in [
        "gh api repos/LHagfoss/rustcode/releases -f tag_name=v0.57.0",
        "gh api repos/LHagfoss/rustcode/releases -F tag_name=v0.57.0",
        "gh api repos/LHagfoss/rustcode/releases --input release.json",
        "gh api repos/LHagfoss/rustcode/releases --header 'X-HTTP-Method-Override: POST'",
        "gh api --method POST repos/LHagfoss/rustcode/releases",
        "gh api repos/LHagfoss/rustcode/releases -X PATCH",
        "gh api repos/LHagfoss/rustcode/releases --method='GET -F tag_name=x'",
        "gh api \"$HOME\" --jq .",
        "gh api 'repos/LHagfoss/rustcode/releases --method POST'",
        "gh api repos/LHagfoss/rustcode/releases --unknown-flag",
        "gh api graphql -f query='mutation { deleteRelease }'",
        "gh release create v0.57.0",
        "gh release edit v0.56.16 --title latest",
        "gh release delete v0.56.16",
    ] {
        assert!(
            command_requires_confirmation(&serde_json::json!({"command": command})),
            "must confirm: {command}"
        );
    }
}

#[test]
fn path_qualified_inspection_names_require_confirmation() {
    for command in [
        "./gh api repos/LHagfoss/rustcode/releases",
        "'/tmp/cat' README.md",
        "echo data | ./cat",
    ] {
        assert!(
            command_requires_confirmation(&serde_json::json!({"command": command})),
            "{command}"
        );
    }
}

#[test]
fn quoted_gh_api_filters_stay_one_segment_and_shell_writes_do_not() {
    for command in [
        "gh api repos/LHagfoss/rustcode/releases --jq '[.[] | select(.draft == false)]'",
        "gh api repos/LHagfoss/rustcode/releases --jq '[.[] | .tag_name]' && gh release list",
    ] {
        assert!(
            !command_requires_confirmation(&serde_json::json!({"command": command})),
            "must not confirm: {command}"
        );
    }

    for command in [
        "gh api repos/LHagfoss/rustcode/releases --jq '[.[] | .tag_name]' && rm -rf /tmp/example",
        "gh api repos/LHagfoss/rustcode/releases --jq \"$(rm -rf /tmp/example)\"",
        "gh api repos/LHagfoss/rustcode/releases --jq '[.[] | .tag_name]'\\|rm -rf /tmp/example",
        "gh api repos/LHagfoss/rustcode/releases --jq '[.[] | .tag_name]",
    ] {
        assert!(
            command_requires_confirmation(&serde_json::json!({"command": command})),
            "must confirm: {command}"
        );
    }
}

#[test]
fn harmless_file_inspection_shells_are_allowed() {
    for command in [
        "cat src/main.rs",
        "head -40 src/main.rs",
        "tail -40 src/main.rs",
        "sed -n '1,40p' src/main.rs",
        "grep -n TODO src/main.rs",
        "cat src/main.rs | grep TODO",
    ] {
        assert!(
            !command_requires_confirmation(&serde_json::json!({"command": command})),
            "harmless inspection must remain available: {command}"
        );
    }
}

#[test]
fn unsafe_file_inspection_shells_still_require_confirmation() {
    for command in [
        "cat src/main.rs > /tmp/main.rs",
        "sed -i 's/old/new/' file.txt",
        "sed 'w output.txt' input.txt",
        "cat src/main.rs | tee /tmp/main.rs",
        "cat \"$(rm -rf /tmp/example)\"",
        "echo \"$(rm -rf /tmp/example)\"",
        "cat \"`rm -rf /tmp/example`\"",
    ] {
        assert!(
            command_requires_confirmation(&serde_json::json!({"command": command})),
            "must confirm potentially mutating inspection command: {command}"
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
    if !sandbox::runtime_tests_available() {
        return;
    }
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
    if !sandbox::runtime_tests_available() {
        return;
    }
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
    if !sandbox::runtime_tests_available() {
        return;
    }
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
