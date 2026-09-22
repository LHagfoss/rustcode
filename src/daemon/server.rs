//! Serialized control plane: at most one accepted connection/frame is in memory.
//! Each connection carries one request, with a deadline for reads and writes.
use super::{
    DaemonError,
    lifecycle::{DaemonLifecycle, DaemonRegistration, remove_if_exists},
    protocol::{DaemonRequest, DaemonResponse, DaemonStatus, read_async_frame, write_async_frame},
    store::JobStore,
};
use anyhow::Result;
use std::{
    fs::{self, File},
    os::unix::fs::PermissionsExt,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::{io::BufReader, net::UnixListener, sync::Notify};

/// Task 3 implements this boundary. All mutations, including CLI/harness requests,
/// pass through RequestHandler; run-now never invents a second execution path.
pub trait SchedulerControl: Send + Sync {
    fn notify_changed(&self);
    fn run_now(&self, job_id: &str) -> super::Result<DaemonResponse>;
    fn snapshot(&self) -> (usize, Option<String>);
}

#[derive(Clone)]
pub struct RequestHandler {
    store: Arc<Mutex<JobStore>>,
    registration: DaemonRegistration,
    database_path: String,
    started: Instant,
    scheduler: Option<Arc<dyn SchedulerControl>>,
}

impl RequestHandler {
    pub fn new(store: JobStore, registration: DaemonRegistration, database_path: String) -> Self {
        Self {
            store: Arc::new(Mutex::new(store)),
            registration,
            database_path,
            started: Instant::now(),
            scheduler: None,
        }
    }

    pub fn store(&self) -> Arc<Mutex<JobStore>> {
        self.store.clone()
    }

    pub fn set_scheduler(&mut self, scheduler: Arc<dyn SchedulerControl>) {
        self.scheduler = Some(scheduler);
    }

    pub fn handle(&self, request: DaemonRequest) -> DaemonResponse {
        match self.dispatch(request) {
            Ok(response) => response,
            Err(error) => {
                let code = match &error {
                    DaemonError::InvalidInput(_) | DaemonError::InvalidSchedule(_) => {
                        "invalid_input"
                    }
                    DaemonError::NotFound(_) => "not_found",
                    DaemonError::Conflict(_) => "conflict",
                    DaemonError::Storage(_) => "storage",
                };
                DaemonResponse::error(code, error.to_string())
            }
        }
    }

    fn dispatch(&self, request: DaemonRequest) -> super::Result<DaemonResponse> {
        match request {
            DaemonRequest::Status => {
                let (active_runs, next_wake_at) =
                    self.scheduler.as_ref().map_or((0, None), |s| s.snapshot());
                return Ok(DaemonResponse::Status {
                    status: DaemonStatus {
                        pid: self.registration.pid,
                        process_start_time: self.registration.process_start_time,
                        instance_id: self.registration.instance_id.clone(),
                        uptime_seconds: self.started.elapsed().as_secs(),
                        socket_path: self.registration.socket_path.to_string_lossy().into(),
                        database_path: self.database_path.clone(),
                        active_runs,
                        next_wake_at,
                    },
                });
            }
            DaemonRequest::Shutdown { instance_id } => {
                return if instance_id == self.registration.instance_id {
                    Ok(DaemonResponse::Ack)
                } else {
                    Err(DaemonError::Conflict("daemon instance changed".into()))
                };
            }
            DaemonRequest::RunNow { job_id } => {
                // Release the store lock before handing control to the scheduler.
                self.store
                    .try_lock()
                    .map_err(|_| DaemonError::Conflict("store is busy".into()))?
                    .get(&job_id)?;
                return match &self.scheduler {
                    Some(scheduler) => scheduler.run_now(&job_id),
                    None => Ok(DaemonResponse::error(
                        "scheduler_unavailable",
                        "scheduler execution is not installed",
                    )),
                };
            }
            _ => {}
        }
        let mut store = self
            .store
            .try_lock()
            .map_err(|_| DaemonError::Conflict("store is busy".into()))?;
        let mut changed = false;
        let response = match request {
            DaemonRequest::Create { job } => {
                store.create(job.clone())?;
                changed = true;
                DaemonResponse::Job { job }
            }
            DaemonRequest::List => DaemonResponse::Jobs {
                jobs: store.list()?,
            },
            DaemonRequest::Get { job_id } => DaemonResponse::Job {
                job: store.get(&job_id)?,
            },
            DaemonRequest::SetPaused { job_id, paused } => {
                store.set_paused(&job_id, paused, chrono::Utc::now())?;
                changed = true;
                DaemonResponse::Ack
            }
            DaemonRequest::Delete { job_id } => {
                store.delete(&job_id)?;
                changed = true;
                DaemonResponse::Ack
            }
            DaemonRequest::History { job_id, limit } => DaemonResponse::History {
                runs: store.history(&job_id, limit)?,
            },
            _ => unreachable!("lifecycle requests handled above"),
        };
        drop(store);
        if changed && let Some(scheduler) = &self.scheduler {
            scheduler.notify_changed();
        }
        Ok(response)
    }
}

pub struct DaemonServer {
    listener: UnixListener,
    pub handler: RequestHandler,
    pub request_timeout: Duration,
    shutdown: Arc<Notify>,
    _ownership: Arc<Ownership>,
}

struct Ownership {
    lifecycle: DaemonLifecycle,
    registration: DaemonRegistration,
    _lock: File,
}

impl Drop for Ownership {
    fn drop(&mut self) {
        // The ownership lock prevents a successor from publishing until cleanup ends.
        if DaemonRegistration::read(&self.lifecycle.registration_path())
            .ok()
            .as_ref()
            == Some(&self.registration)
        {
            let _ = remove_if_exists(&self.lifecycle.registration_path());
        }
        let _ = remove_if_exists(&self.lifecycle.socket_path());
    }
}

impl DaemonServer {
    pub(crate) fn bind(lifecycle: DaemonLifecycle, lock: File) -> Result<Self> {
        let registration = DaemonRegistration::current(lifecycle.socket_path())?;
        let listener = UnixListener::bind(lifecycle.socket_path())?;
        let ownership = Ownership {
            lifecycle: lifecycle.clone(),
            registration: registration.clone(),
            _lock: lock,
        };
        fs::set_permissions(lifecycle.socket_path(), fs::Permissions::from_mode(0o600))?;
        let store = JobStore::open(lifecycle.database_path())?;
        let handler = RequestHandler::new(
            store,
            registration.clone(),
            lifecycle.database_path().to_string_lossy().into(),
        );
        registration.publish(&lifecycle.registration_path())?;
        Ok(Self {
            listener,
            handler,
            request_timeout: Duration::from_secs(2),
            shutdown: Arc::new(Notify::new()),
            _ownership: Arc::new(ownership),
        })
    }

    pub fn shutdown_handle(&self) -> Arc<Notify> {
        self.shutdown.clone()
    }

    pub async fn run(self) -> Result<()> {
        loop {
            let (stream, _) = tokio::select! {
                biased;
                _ = self.shutdown.notified() => break,
                accepted = self.listener.accept() => accepted?,
            };
            let mut reader = BufReader::new(stream);
            let request = tokio::select! {
                biased;
                _ = self.shutdown.notified() => break,
                result = tokio::time::timeout(self.request_timeout, read_async_frame::<_, DaemonRequest>(&mut reader)) => result,
            };
            let (response, stop) = match request {
                Ok(Ok(request)) => {
                    let shutdown = matches!(request, DaemonRequest::Shutdown { .. });
                    let handler = self.handler.clone();
                    let ownership = self._ownership.clone();
                    // SQLite is synchronous. Keep it off the runtime, and retain
                    // ownership if cancellation occurs while a mutation is in flight.
                    let work = tokio::task::spawn_blocking(move || {
                        let _ownership = ownership;
                        handler.handle(request)
                    });
                    let response = match tokio::time::timeout(self.request_timeout, work).await {
                        Ok(result) => result?,
                        // Do not admit more work behind a timed-out handler. A
                        // mutation may have committed; clients must inspect state.
                        Err(_) => {
                            let response = DaemonResponse::error(
                                "handler_timeout",
                                "request outcome unknown; inspect state before retrying",
                            );
                            let _ = tokio::time::timeout(
                                self.request_timeout,
                                write_async_frame(reader.get_mut(), &response),
                            )
                            .await;
                            break;
                        }
                    };
                    let stop = shutdown && matches!(response, DaemonResponse::Ack);
                    (response, stop)
                }
                Ok(Err(error)) => (
                    DaemonResponse::error("invalid_request", error.to_string()),
                    false,
                ),
                Err(_) => (
                    DaemonResponse::error("request_timeout", "request deadline exceeded"),
                    false,
                ),
            };
            // Oversized responses get a small explicit error rather than a silent EOF.
            let send = async {
                match write_async_frame(reader.get_mut(), &response).await {
                    Err(super::protocol::ProtocolError::FrameTooLarge) => {
                        write_async_frame(
                            reader.get_mut(),
                            &DaemonResponse::error(
                                "response_too_large",
                                "response exceeds frame limit; narrow the request",
                            ),
                        )
                        .await
                    }
                    result => result,
                }
            };
            tokio::select! {
                biased;
                _ = self.shutdown.notified() => break,
                _ = tokio::time::timeout(self.request_timeout, send) => {}
            }
            // Always honor accepted shutdown, even if the peer disconnects before ACK.
            if stop {
                break;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::{
        client::DaemonClient,
        model::{JobAction, JobRecord, RetryPolicy, ScheduleSpec},
        protocol::MAX_FRAME_BYTES,
    };
    use std::os::unix::fs::DirBuilderExt;
    use tokio::{io::AsyncWriteExt, net::UnixStream};

    fn job() -> JobRecord {
        let now = chrono::Utc::now();
        JobRecord {
            id: "example".into(),
            name: "example".into(),
            paused: false,
            schedule: ScheduleSpec::Once { at: now },
            action: JobAction::McpCall {
                server: "teams".into(),
                tool: "read".into(),
                arguments: serde_json::json!({}),
                workspace: "/tmp".into(),
            },
            workspace: "/tmp".into(),
            target_session: None,
            retry_policy: RetryPolicy::default(),
            next_due_at: now,
            schedule_revision: 1,
            created_at: now,
            updated_at: now,
        }
    }

    // Abort on assertion failure; normal teardown signals and joins the server.
    struct Running {
        task: tokio::task::JoinHandle<Result<()>>,
        shutdown: Arc<Notify>,
        lifecycle: DaemonLifecycle,
        _directory: tempfile::TempDir,
    }
    impl Running {
        fn new() -> Self {
            let directory = tempfile::tempdir_in("/tmp").unwrap();
            let lifecycle = DaemonLifecycle::new(directory.path());
            let mut server = lifecycle.bind().unwrap();
            server.request_timeout = Duration::from_millis(150);
            Self {
                shutdown: server.shutdown_handle(),
                task: tokio::spawn(server.run()),
                lifecycle,
                _directory: directory,
            }
        }
        fn client(&self) -> DaemonClient {
            DaemonClient::new(self.lifecycle.socket_path())
        }
        async fn finish(mut self) {
            self.shutdown.notify_one();
            tokio::time::timeout(Duration::from_secs(2), &mut self.task)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert!(!self.lifecycle.socket_path().exists());
            assert!(!self.lifecycle.registration_path().exists());
        }
    }
    impl Drop for Running {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    #[tokio::test]
    async fn shared_handler_round_trips_store_operations_without_execution() {
        let running = Running::new();
        let client = running.client();
        let job = job();
        assert_eq!(
            client
                .request(DaemonRequest::Create { job: job.clone() })
                .await
                .unwrap(),
            DaemonResponse::Job { job: job.clone() }
        );
        assert_eq!(
            client.request(DaemonRequest::List).await.unwrap(),
            DaemonResponse::Jobs {
                jobs: vec![job.clone()]
            }
        );
        assert_eq!(
            client
                .request(DaemonRequest::Get {
                    job_id: job.id.clone()
                })
                .await
                .unwrap(),
            DaemonResponse::Job { job: job.clone() }
        );
        assert!(
            matches!(client.request(DaemonRequest::Create { job: job.clone() }).await.unwrap(), DaemonResponse::Error { code, .. } if code == "conflict")
        );
        assert_eq!(
            client
                .request(DaemonRequest::SetPaused {
                    job_id: job.id.clone(),
                    paused: true
                })
                .await
                .unwrap(),
            DaemonResponse::Ack
        );
        assert!(
            matches!(client.request(DaemonRequest::Get { job_id: job.id.clone() }).await.unwrap(), DaemonResponse::Job { job } if job.paused)
        );
        assert_eq!(
            client
                .request(DaemonRequest::History {
                    job_id: job.id.clone(),
                    limit: usize::MAX
                })
                .await
                .unwrap(),
            DaemonResponse::History { runs: vec![] }
        );
        assert!(
            matches!(client.request(DaemonRequest::RunNow { job_id: job.id.clone() }).await.unwrap(), DaemonResponse::Error { code, .. } if code == "scheduler_unavailable")
        );
        assert_eq!(
            client
                .request(DaemonRequest::Delete {
                    job_id: job.id.clone()
                })
                .await
                .unwrap(),
            DaemonResponse::Ack
        );
        assert!(
            matches!(client.request(DaemonRequest::Get { job_id: job.id }).await.unwrap(), DaemonResponse::Error { code, .. } if code == "not_found")
        );
        running.finish().await;
    }

    #[tokio::test]
    async fn malformed_and_oversized_requests_do_not_kill_server() {
        let running = Running::new();
        for payload in [b"{bad json}\n".to_vec(), vec![b'x'; MAX_FRAME_BYTES + 2]] {
            let mut stream = UnixStream::connect(running.lifecycle.socket_path())
                .await
                .unwrap();
            stream.write_all(&payload).await.unwrap();
            let response: DaemonResponse =
                read_async_frame(&mut BufReader::new(stream)).await.unwrap();
            assert!(
                matches!(response, DaemonResponse::Error { code, .. } if code == "invalid_request")
            );
        }
        assert!(matches!(
            running
                .client()
                .request(DaemonRequest::Status)
                .await
                .unwrap(),
            DaemonResponse::Status { .. }
        ));
        running.finish().await;
    }

    #[tokio::test]
    async fn partial_request_times_out_and_next_client_succeeds() {
        let running = Running::new();
        let mut stream = UnixStream::connect(running.lifecycle.socket_path())
            .await
            .unwrap();
        stream.write_all(b"{\"type\":").await.unwrap();
        let response: DaemonResponse = tokio::time::timeout(
            Duration::from_secs(2),
            read_async_frame(&mut BufReader::new(stream)),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(
            matches!(response, DaemonResponse::Error { code, .. } if code == "request_timeout")
        );
        assert!(matches!(
            running
                .client()
                .request(DaemonRequest::Status)
                .await
                .unwrap(),
            DaemonResponse::Status { .. }
        ));
        running.finish().await;
    }

    #[tokio::test]
    async fn wrong_instance_shutdown_is_rejected() {
        let running = Running::new();
        assert!(
            matches!(running.client().request(DaemonRequest::Shutdown { instance_id: "old-instance".into() }).await.unwrap(), DaemonResponse::Error { code, .. } if code == "conflict")
        );
        assert!(running.lifecycle.status().await.unwrap().is_some());
        running.finish().await;
    }

    #[tokio::test]
    async fn shutdown_interrupts_idle_connection_without_leaking_listener() {
        let running = Running::new();
        let _idle = UnixStream::connect(running.lifecycle.socket_path())
            .await
            .unwrap();
        running.finish().await;
    }

    #[tokio::test]
    async fn failed_store_initialization_cleans_socket_and_allows_retry() {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let lifecycle = DaemonLifecycle::new(dir.path());
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(lifecycle.database_path())
            .unwrap();
        assert!(lifecycle.bind().is_err());
        assert!(!lifecycle.socket_path().exists());
        assert!(!lifecycle.registration_path().exists());
        fs::remove_dir(lifecycle.database_path()).unwrap();
        drop(lifecycle.bind().unwrap());
    }
}
