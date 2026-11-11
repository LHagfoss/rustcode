//! Native Windows coverage, without compiling unrelated Unix-only library tests.
#![cfg(windows)]
use rustcode::daemon::{
    client::DaemonClient,
    lifecycle::DaemonLifecycle,
    model::{JobAction, JobRecord, JobRunState, RetryPolicy, ScheduleSpec},
    protocol::{DaemonRequest, DaemonResponse},
};
use std::{sync::Arc, time::Duration};

#[test]
fn paths_are_scoped_to_the_config_directory() {
    let lifecycle = DaemonLifecycle::new(r"C:\profiles\one");
    let root = std::path::Path::new(r"C:\profiles\one\daemon");
    assert_eq!(lifecycle.socket_path(), root.join("control.endpoint"));
    assert_eq!(lifecycle.database_path(), root.join("jobs.sqlite"));
    assert_eq!(
        lifecycle.registration_path(),
        root.join("registration.json")
    );
    assert_eq!(lifecycle.log_path(), root.join("daemon.log"));
}

#[test]
fn foreground_child() {
    let Some(root) = std::env::var_os("RUSTCODE_DAEMON_TEST_ROOT") else {
        return;
    };
    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(DaemonLifecycle::new(root).run())
        .unwrap();
}

#[tokio::test]
async fn detached_start_status_logs_and_stop() {
    let directory = tempfile::tempdir().unwrap();
    let lifecycle = DaemonLifecycle::new(directory.path());
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "foreground_child", "--nocapture"])
        .env("RUSTCODE_DAEMON_TEST_ROOT", directory.path());
    let status = lifecycle.start(command).await.unwrap();
    let health = lifecycle.status().await;
    let logs = lifecycle.read_log_tail(10);
    let stopped = lifecycle.stop().await.unwrap().unwrap();
    assert_ne!(status.pid, std::process::id());
    assert_eq!(status.instance_id, stopped.instance_id);
    assert_eq!(health.unwrap().unwrap().instance_id, status.instance_id);
    assert!(logs.is_ok());
    assert!(!lifecycle.socket_path().exists());
    assert!(!lifecycle.registration_path().exists());
}

#[tokio::test]
async fn scheduled_job_executes_and_history_survives_restart() {
    use rustcode::daemon::executor::{ExecutionFuture, JobExecutor, JobRunContext, RunOutcome};
    struct Executor;
    impl JobExecutor for Executor {
        fn execute(&self, _: JobRunContext) -> ExecutionFuture {
            Box::pin(async {
                RunOutcome::Succeeded {
                    summary: "done".into(),
                    output: Some("executed".into()),
                }
            })
        }
    }
    let directory = tempfile::tempdir().unwrap();
    let lifecycle = DaemonLifecycle::new(directory.path());
    let mut server = lifecycle.bind().unwrap();
    assert!(lifecycle.bind().is_err());
    server.install_executor(Arc::new(Executor), 1);
    let task = tokio::spawn(server.run());
    let client = DaemonClient::new(lifecycle.socket_path());
    let now = chrono::Utc::now();
    let workspace = directory.path().to_string_lossy().into_owned();
    let job = JobRecord {
        id: "windows-job".into(),
        name: "Windows job".into(),
        paused: false,
        schedule: ScheduleSpec::Once { at: now },
        action: JobAction::Prompt {
            prompt: "test".into(),
            workspace: workspace.clone(),
            model_profile: None,
            session_id: None,
            settings: None,
        },
        workspace,
        target_session: None,
        retry_policy: RetryPolicy::default(),
        next_due_at: now,
        schedule_revision: 1,
        created_at: now,
        updated_at: now,
    };
    assert!(matches!(
        client.request(DaemonRequest::Create { job }).await.unwrap(),
        DaemonResponse::Job { .. }
    ));
    let result = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let DaemonResponse::History { runs } = client
                .request(DaemonRequest::History {
                    job_id: "windows-job".into(),
                    limit: 1,
                })
                .await
                .unwrap()
                && runs
                    .first()
                    .is_some_and(|run| run.state == JobRunState::Succeeded)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    lifecycle.stop().await.unwrap();
    task.await.unwrap().unwrap();
    result.unwrap();
    let server = lifecycle.bind().unwrap();
    let task = tokio::spawn(server.run());
    let history = client
        .request(DaemonRequest::History {
            job_id: "windows-job".into(),
            limit: 1,
        })
        .await
        .unwrap();
    lifecycle.stop().await.unwrap();
    task.await.unwrap().unwrap();
    assert!(
        matches!(history, DaemonResponse::History { runs } if runs[0].output.as_deref() == Some("executed"))
    );
}
