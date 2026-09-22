use super::{executor::*, model::*};
use chrono::Utc;
use serde_json::json;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct MockActions {
    calls: Mutex<Vec<(JobAction, String, tokio::time::Instant)>>,
    results: Mutex<VecDeque<RunOutcome>>,
}
impl ActionBackend for MockActions {
    fn run(&self, action: JobAction, session: String, _: CancellationToken) -> ExecutionFuture {
        self.calls
            .lock()
            .unwrap()
            .push((action, session, tokio::time::Instant::now()));
        let result = self.results.lock().unwrap().pop_front();
        Box::pin(async move {
            match result {
                Some(result) => result,
                None => std::future::pending().await,
            }
        })
    }
}
fn success(value: serde_json::Value) -> RunOutcome {
    RunOutcome::Succeeded {
        summary: "done".into(),
        output: Some(value.to_string()),
    }
}
fn mcp() -> JobAction {
    JobAction::McpCall {
        server: "teams".into(),
        tool: "send_chat_message".into(),
        arguments: json!({"chat_id":"123", "content":"Good morning!"}),
        workspace: "/recorded/project".into(),
    }
}
fn context(action: JobAction) -> JobRunContext {
    let now = Utc::now();
    JobRunContext {
        job: JobRecord {
            id: "job-4".into(),
            name: "test".into(),
            paused: false,
            schedule: ScheduleSpec::Once { at: now },
            action,
            workspace: "/recorded/project".into(),
            target_session: None,
            retry_policy: RetryPolicy::default(),
            next_due_at: now,
            schedule_revision: 1,
            created_at: now,
            updated_at: now,
        },
        run: JobRunRecord {
            id: "run-4".into(),
            job_id: "job-4".into(),
            schedule_revision: 1,
            idempotency_key: "key".into(),
            scheduled_at: now,
            state: JobRunState::Running,
            attempt: 1,
            lease_owner: None,
            lease_fence: 1,
            lease_expires_at: None,
            started_at: Some(now),
            finished_at: None,
            result_summary: None,
            error_class: None,
            output: None,
        },
        cancellation: CancellationToken::new(),
    }
}
fn executor(results: Vec<RunOutcome>) -> (ActionExecutor, Arc<MockActions>) {
    let backend = Arc::new(MockActions::default());
    backend.results.lock().unwrap().extend(results);
    (
        ActionExecutor::with_backend(backend.clone(), Duration::from_secs(30)),
        backend,
    )
}

#[tokio::test]
async fn direct_mcp_keeps_teams_payload_and_workspace() {
    let (executor, backend) = executor(vec![success(json!({"message_id":"sent"}))]);
    assert!(matches!(
        executor.execute(context(mcp())).await,
        RunOutcome::Succeeded { .. }
    ));
    assert_eq!(backend.calls.lock().unwrap()[0].0, mcp());
}

#[tokio::test]
async fn prompt_propagates_workspace_model_and_explicit_session() {
    let action = JobAction::Prompt {
        prompt: "inspect".into(),
        workspace: "/recorded/project".into(),
        model_profile: Some("review".into()),
        session_id: Some("session-4".into()),
    };
    let (executor, backend) = executor(vec![success(json!("ok"))]);
    executor.execute(context(action.clone())).await;
    let calls = backend.calls.lock().unwrap();
    assert_eq!(calls[0].0, action);
    assert_eq!(calls[0].1, "session-4");
}

#[tokio::test(start_paused = true)]
async fn poll_obeys_interval_max_runs_and_canonical_change_detection() {
    let (executor, backend) = executor(vec![
        success(json!({"a":1,"b":2})),
        RunOutcome::Succeeded {
            summary: "done".into(),
            output: Some("{\"b\":2,\"a\":1}".into()),
        },
        success(json!({"a":2})),
    ]);
    let action = JobAction::Poll {
        action: Box::new(mcp()),
        interval_seconds: 2,
        max_runs: 8,
        deadline_seconds: 20,
        stop_on_change: true,
    };
    assert!(matches!(
        executor.execute(context(action)).await,
        RunOutcome::Succeeded { .. }
    ));
    let calls = backend.calls.lock().unwrap();
    assert_eq!(calls.len(), 3);
    assert_eq!(calls[2].2 - calls[0].2, Duration::from_secs(4));
}

#[tokio::test(start_paused = true)]
async fn poll_stops_at_max_runs_or_deadline_without_extra_dispatch() {
    for (max_runs, deadline, expected) in [(2, 20, 2), (10, 3, 2)] {
        let (executor, backend) = executor((0..10).map(|_| success(json!(1))).collect());
        let action = JobAction::Poll {
            action: Box::new(mcp()),
            interval_seconds: 2,
            max_runs,
            deadline_seconds: deadline,
            stop_on_change: false,
        };
        assert!(matches!(
            executor.execute(context(action)).await,
            RunOutcome::Succeeded { .. }
        ));
        assert_eq!(backend.calls.lock().unwrap().len(), expected);
    }
}

#[tokio::test(start_paused = true)]
async fn cancellation_before_dispatch_is_safe_but_inflight_timeout_is_ambiguous() {
    let (executor, backend) = executor(vec![]);
    let ctx = context(mcp());
    ctx.cancellation.cancel();
    assert!(matches!(
        executor.execute(ctx).await,
        RunOutcome::Cancelled { .. }
    ));
    assert!(backend.calls.lock().unwrap().is_empty());
    assert!(matches!(
        executor.execute(context(mcp())).await,
        RunOutcome::Ambiguous { .. }
    ));
}

#[tokio::test]
async fn shell_policy_rejects_unapproved_mutation() {
    let action = JobAction::ShellCommand {
        command: "touch forbidden".into(),
        working_directory: "/tmp".into(),
        environment_allowlist: vec![],
        timeout_seconds: 1,
    };
    assert!(
        matches!(ActionExecutor::default().execute(context(action)).await, RunOutcome::Permanent { error, .. } if error.contains("confirmation"))
    );
}

#[tokio::test]
async fn shell_timeout_retains_partial_output() {
    let action = JobAction::ShellCommand {
        command: "printf partial; tail -f /dev/null".into(),
        working_directory: "/tmp".into(),
        environment_allowlist: vec![],
        timeout_seconds: 1,
    };
    assert!(
        matches!(ActionExecutor::default().execute(context(action)).await, RunOutcome::Ambiguous { output: Some(output), .. } if output.contains("partial") && output.len() <= MAX_RUN_OUTPUT_BYTES)
    );
}

#[test]
fn recorded_headless_state_does_not_use_process_workspace() {
    let workspace = tempfile::tempdir().unwrap();
    let state = crate::raw_cli::build_scheduled_state(
        "inspect",
        workspace.path(),
        None,
        "daemon-test-session",
    )
    .unwrap();
    assert_eq!(state.workspace_root.as_deref(), Some(workspace.path()));
    assert_eq!(
        state.task_working_directory.as_deref(),
        Some(workspace.path())
    );
    assert_eq!(state.active_session_id, "daemon-test-session");
    assert_eq!(state.history.last().unwrap().content, "inspect");
}

#[tokio::test]
async fn production_mcp_uses_recorded_config_workspace_and_direct_payload() {
    let workspace = tempfile::tempdir().unwrap();
    let script = r#"
read -r line
printf '%s\n' '{"id":1,"result":{"protocolVersion":"2024-11-05","capabilities":{},"serverInfo":{"name":"mock","version":"1"}}}'
read -r line
read -r line
printf '%s\n' '{"id":2,"result":{"tools":[]}}'
read -r line
printf '{"id":3,"result":{"request":%s,"cwd":"%s","config":"%s"}}\n' "$line" "$PWD" "$TASK4_CONFIG"
read -r line
"#;
    std::fs::create_dir(workspace.path().join(".rustcode")).unwrap();
    let config = json!({"mcp_servers": [{"name":"teams", "command":"/bin/sh", "args":["-c",script], "env":{"TASK4_CONFIG":"recorded"}, "enabled":true}]});
    std::fs::write(
        workspace.path().join(".rustcode/config.toml"),
        toml::to_string(&config).unwrap(),
    )
    .unwrap();
    let mut action = mcp();
    if let JobAction::McpCall {
        workspace: path, ..
    } = &mut action
    {
        *path = workspace.path().to_string_lossy().into_owned();
    }
    let result = ActionExecutor::default().execute(context(action)).await;
    let RunOutcome::Succeeded {
        output: Some(output),
        ..
    } = result
    else {
        panic!("{result:?}")
    };
    let output: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(output["request"]["method"], "tools/call");
    assert_eq!(
        output["request"]["params"],
        json!({"name":"send_chat_message","arguments":{"chat_id":"123","content":"Good morning!"}})
    );
    assert_eq!(
        std::path::Path::new(output["cwd"].as_str().unwrap())
            .canonicalize()
            .unwrap(),
        workspace.path().canonicalize().unwrap()
    );
    assert_eq!(output["config"], "recorded");
}

#[tokio::test]
async fn shell_uses_recorded_directory_and_only_allowlisted_environment() {
    let workspace = tempfile::tempdir().unwrap();
    for (command, allowlist, expected) in [
        (
            "pwd",
            vec![],
            workspace
                .path()
                .canonicalize()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
        ),
        ("printf '%s' \"$HOME\"", vec![], String::new()),
        (
            "printf '%s' \"$HOME\"",
            vec!["HOME".to_owned()],
            std::env::var("HOME").unwrap(),
        ),
    ] {
        let action = JobAction::ShellCommand {
            command: command.into(),
            working_directory: workspace.path().to_string_lossy().into_owned(),
            environment_allowlist: allowlist,
            timeout_seconds: 1,
        };
        let result = ActionExecutor::default().execute(context(action)).await;
        let output = match result {
            RunOutcome::Succeeded { output, .. } => output.unwrap_or_default(),
            other => panic!("{other:?}"),
        };
        assert_eq!(output.trim(), expected);
    }
}

#[tokio::test(start_paused = true)]
async fn poll_does_not_replay_an_ambiguous_action() {
    let (executor, backend) = executor(vec![RunOutcome::Ambiguous {
        error: "connection lost".into(),
        output: Some("partial".into()),
    }]);
    let action = JobAction::Poll {
        action: Box::new(mcp()),
        interval_seconds: 1,
        max_runs: 4,
        deadline_seconds: 10,
        stop_on_change: false,
    };
    assert!(matches!(
        executor.execute(context(action)).await,
        RunOutcome::Ambiguous { .. }
    ));
    assert_eq!(backend.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn invalid_poll_never_dispatches() {
    for action in [
        JobAction::Poll {
            action: Box::new(mcp()),
            interval_seconds: 0,
            max_runs: 4,
            deadline_seconds: 10,
            stop_on_change: false,
        },
        JobAction::Poll {
            action: Box::new(mcp()),
            interval_seconds: 1,
            max_runs: 0,
            deadline_seconds: 10,
            stop_on_change: false,
        },
        JobAction::Poll {
            action: Box::new(mcp()),
            interval_seconds: 1,
            max_runs: 4,
            deadline_seconds: 0,
            stop_on_change: false,
        },
    ] {
        let (executor, backend) = executor(vec![]);
        assert!(matches!(
            executor.execute(context(action)).await,
            RunOutcome::Permanent { .. }
        ));
        assert!(backend.calls.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn every_outcome_bounds_unicode_output_and_diagnostics() {
    let huge = "🦀".repeat(MAX_RUN_OUTPUT_BYTES);
    for outcome in [
        RunOutcome::Succeeded {
            summary: huge.clone(),
            output: Some(huge.clone()),
        },
        RunOutcome::Permanent {
            error: huge.clone(),
            output: Some(huge.clone()),
        },
        RunOutcome::Transient {
            error: huge.clone(),
            output: Some(huge.clone()),
        },
        RunOutcome::Ambiguous {
            error: huge.clone(),
            output: Some(huge.clone()),
        },
        RunOutcome::Cancelled {
            output: Some(huge.clone()),
        },
    ] {
        let (executor, _) = executor(vec![outcome]);
        let result = executor.execute(context(mcp())).await;
        let (message, output) = match result {
            RunOutcome::Succeeded { summary, output } => (summary, output),
            RunOutcome::Permanent { error, output }
            | RunOutcome::Transient { error, output }
            | RunOutcome::Ambiguous { error, output } => (error, output),
            RunOutcome::Cancelled { output } => (String::new(), output),
        };
        assert!(message.len() <= MAX_RUN_OUTPUT_BYTES);
        assert!(output.unwrap().len() <= MAX_RUN_OUTPUT_BYTES);
    }
}

#[tokio::test]
async fn executor_deadline_retains_shell_partial_output() {
    let action = JobAction::ShellCommand {
        command: "printf partial; tail -f /dev/null".into(),
        working_directory: "/tmp".into(),
        environment_allowlist: vec![],
        timeout_seconds: 30,
    };
    // Exercise the executor deadline, independently of the shell's own timeout.
    let executor =
        ActionExecutor::with_backend(Arc::new(ProductionTestBackend), Duration::from_millis(200));
    assert!(
        matches!(executor.execute(context(action)).await, RunOutcome::Ambiguous { output: Some(output), .. } if output.contains("partial"))
    );
}

struct ProductionTestBackend;
impl ActionBackend for ProductionTestBackend {
    fn run(
        &self,
        action: JobAction,
        session: String,
        cancellation: CancellationToken,
    ) -> ExecutionFuture {
        let mut ctx = context(action);
        ctx.job.target_session = Some(session);
        ctx.cancellation = cancellation;
        ActionExecutor::default().execute(ctx)
    }
}

#[tokio::test(start_paused = true)]
async fn cancellation_between_poll_calls_is_safe() {
    let (executor, backend) = executor(vec![success(json!(1)), success(json!(2))]);
    let ctx = context(JobAction::Poll {
        action: Box::new(mcp()),
        interval_seconds: 10,
        max_runs: 4,
        deadline_seconds: 60,
        stop_on_change: false,
    });
    let cancellation = ctx.cancellation.clone();
    let task = tokio::spawn(executor.execute(ctx));
    tokio::task::yield_now().await;
    cancellation.cancel();
    assert!(matches!(task.await.unwrap(), RunOutcome::Cancelled { .. }));
    assert_eq!(backend.calls.lock().unwrap().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn poll_hashes_changes_beyond_the_retained_output_limit() {
    let prefix = "x".repeat(MAX_RUN_OUTPUT_BYTES);
    let (executor, backend) = executor(vec![
        success(json!({"long":prefix,"nested":{"a":1}})),
        success(json!({"long":prefix,"nested":{"a":2}})),
    ]);
    let action = JobAction::Poll {
        action: Box::new(mcp()),
        interval_seconds: 1,
        max_runs: 4,
        deadline_seconds: 10,
        stop_on_change: true,
    };
    assert!(matches!(
        executor.execute(context(action)).await,
        RunOutcome::Succeeded { .. }
    ));
    assert_eq!(backend.calls.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn interactive_sudo_and_missing_prompt_workspace_are_permanent_failures() {
    let action = JobAction::ShellCommand {
        command: "sudo cat /dev/null".into(),
        working_directory: "/tmp".into(),
        environment_allowlist: vec![],
        timeout_seconds: 1,
    };
    assert!(matches!(
        ActionExecutor::default().execute(context(action)).await,
        RunOutcome::Permanent { .. }
    ));
    let action = JobAction::Prompt {
        prompt: "inspect".into(),
        workspace: "/nonexistent-task4-workspace".into(),
        model_profile: None,
        session_id: None,
    };
    assert!(matches!(
        ActionExecutor::default().execute(context(action)).await,
        RunOutcome::Permanent { .. }
    ));
}

#[tokio::test]
async fn mcp_process_loss_after_dispatch_is_ambiguous() {
    let workspace = tempfile::tempdir().unwrap();
    let script = r#"
read -r line
printf '%s\n' '{"id":1,"result":{"capabilities":{}}}'
read -r line
read -r line
printf '%s\n' '{"id":2,"result":{"tools":[]}}'
read -r line
exit 1
"#;
    std::fs::create_dir(workspace.path().join(".rustcode")).unwrap();
    let config = json!({"mcp_servers": [{"name":"teams", "command":"/bin/sh", "args":["-c",script], "env":{}, "enabled":true}]});
    std::fs::write(
        workspace.path().join(".rustcode/config.toml"),
        toml::to_string(&config).unwrap(),
    )
    .unwrap();
    let action = JobAction::McpCall {
        server: "teams".into(),
        tool: "send_chat_message".into(),
        arguments: json!({}),
        workspace: workspace.path().to_string_lossy().into_owned(),
    };
    assert!(matches!(
        ActionExecutor::default().execute(context(action)).await,
        RunOutcome::Ambiguous { .. }
    ));
}

#[tokio::test]
async fn shell_timeout_covers_descendants_holding_output_pipes() {
    let action = JobAction::ShellCommand {
        command: "printf partial; tail -f /dev/null &".into(),
        working_directory: "/tmp".into(),
        environment_allowlist: vec![],
        timeout_seconds: 1,
    };
    let result = tokio::time::timeout(
        Duration::from_secs(3),
        ActionExecutor::default().execute(context(action)),
    )
    .await;
    assert!(
        matches!(result, Ok(RunOutcome::Ambiguous { output: Some(output), .. }) if output.contains("partial"))
    );
}

#[tokio::test]
async fn shell_output_capture_overflow_is_bounded_and_not_polled_again() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("large.txt"), "x".repeat(300_000)).unwrap();
    let action = JobAction::ShellCommand {
        command: "cat large.txt".into(),
        working_directory: workspace.path().to_string_lossy().into_owned(),
        environment_allowlist: vec![],
        timeout_seconds: 2,
    };
    let result = ActionExecutor::default().execute(context(action)).await;
    assert!(
        matches!(result, RunOutcome::Permanent { output: Some(output), .. } if output.len() <= MAX_RUN_OUTPUT_BYTES)
    );
}
