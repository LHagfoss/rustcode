//! Authenticated host-wide operations. Detached owners do not belong to a phone connection.
use super::{
    hub::SessionHub,
    router::{DeviceContext, FrameRouter, FrameSink},
};
use crate::daemon::lifecycle::{
    ensure_private_directory, is_lock_busy, lock_private_file, publish_private_json,
    read_private_json, remove_if_exists,
};
use crate::remote::*;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::os::unix::{fs::OpenOptionsExt, process::CommandExt};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

/// How long a new owner may take to register before it is stopped.
const OWNER_START_TIMEOUT: Duration = Duration::from_secs(20);
const OWNER_LEASE: &str = "remote session";

/// The owner of a new session could not be started, and nothing is left of
/// the attempt: no process and no receipt. The device may simply ask again.
#[derive(Debug)]
struct OwnerStartFailed(String);

impl std::fmt::Display for OwnerStartFailed {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for OwnerStartFailed {}

/// Whether the owner process of `session_id` is running: it holds its lease
/// for as long as it lives. The probe takes a free lease for an instant,
/// which [`owner_state`] rides out.
fn owner_alive(owners: &Path, session_id: &str) -> bool {
    match lock_private_file(owners, &format!("{session_id}.lock"), OWNER_LEASE) {
        Ok(_free) => false,
        Err(error) => is_lock_busy(&error),
    }
}

/// Sessions with a running owner process, registered with the gateway or not.
pub(super) fn live_owner_sessions(lifecycle: &super::lifecycle::RemoteLifecycle) -> Vec<String> {
    let owners = lifecycle.owners_directory();
    let Ok(entries) = std::fs::read_dir(&owners) else {
        return Vec::new();
    };
    let mut sessions: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            Some(name.to_str()?.strip_suffix(".lock")?.to_owned())
        })
        .filter(|id| rustcode_session::valid_session_id(id) && owner_alive(&owners, id))
        .collect();
    sessions.sort();
    sessions
}

/// The gateway's own line in an owner log, so a failed start never leaves it empty.
fn note_in_log(log: &Path, text: &str) {
    use std::io::Write;
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .append(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(log)
    {
        let _ = writeln!(file, "gateway: {text}");
    }
}

/// The last lines an owner wrote, on one line, for an error message.
fn log_tail(log: &Path) -> Option<String> {
    let bytes = std::fs::read(log).ok()?;
    let text = String::from_utf8_lossy(&bytes[bytes.len().saturating_sub(2048)..]).into_owned();
    let mut lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("gateway: "))
        .collect();
    let tail = lines.split_off(lines.len().saturating_sub(3)).join(" | ");
    (!tail.is_empty()).then(|| tail.chars().take(600).collect())
}

pub fn resolve_directory(path: Option<&str>) -> Result<PathBuf> {
    ensure!(
        path.is_none_or(|value| value.len() <= 4096),
        "directory path is too long"
    );
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("home directory unavailable")?;
    let path = match path.map(str::trim).filter(|v| !v.is_empty()) {
        None | Some("~") | Some("~/") => home,
        Some(value) if value.starts_with("~/") => home.join(&value[2..]),
        Some(value) => {
            let path = PathBuf::from(value);
            ensure!(path.is_absolute(), "choose an absolute directory path");
            path
        }
    };
    let path = path
        .canonicalize()
        .context("directory does not exist or is inaccessible")?;
    ensure!(path.is_dir(), "choose a directory, not a file");
    Ok(path)
}

pub fn list_directories(path: Option<&str>) -> Result<RemoteResult> {
    let path = resolve_directory(path)?;
    let mut directories = Vec::new();
    let mut truncated = false;
    let mut bytes = 0usize;
    for entry in std::fs::read_dir(&path)? {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || !entry.path().is_dir() {
            continue;
        }
        if directories.len() == 512 {
            truncated = true;
            break;
        }
        let directory = RemoteDirectory {
            name,
            path: entry.path().to_string_lossy().into_owned(),
        };
        bytes += serde_json::to_vec(&directory)?.len() + 1;
        if bytes > MAX_REMOTE_FRAME_BYTES / 2 {
            truncated = true;
            break;
        }
        directories.push(directory);
    }
    directories.sort_by_key(|entry| entry.name.to_lowercase());
    Ok(RemoteResult::Directories {
        parent: path.parent().map(|v| v.to_string_lossy().into_owned()),
        path: path.to_string_lossy().into_owned(),
        directories,
        truncated,
    })
}

#[derive(Serialize, Deserialize)]
struct Creation {
    request: RemoteRequest,
    session_id: String,
    response: Option<RemoteResponse>,
}

fn planned_creation(request: &RemoteRequest) -> Creation {
    Creation {
        request: request.clone(),
        session_id: rustcode_session::next_session_id(),
        response: None,
    }
}

pub struct WorkspaceRouter {
    hub: Arc<SessionHub>,
    config: PathBuf,
    program: PathBuf,
    creation: Arc<tokio::sync::Mutex<()>>,
}
impl WorkspaceRouter {
    pub fn new(hub: Arc<SessionHub>, config: PathBuf, program: PathBuf) -> Self {
        Self {
            hub,
            config,
            program,
            creation: Arc::new(tokio::sync::Mutex::new(())),
        }
    }
    fn receipt_path(config: &Path, device: &str, request: &str) -> PathBuf {
        let mut digest = Sha256::new();
        digest.update(device);
        digest.update([0]);
        digest.update(request);
        config
            .join("remote")
            .join("created")
            .join(format!("{}.json", hex::encode(digest.finalize())))
    }
    async fn create(
        hub: &SessionHub,
        config: &Path,
        program: &Path,
        device: &str,
        request: &RemoteRequest,
        path: Option<&str>,
        timeout: Duration,
    ) -> Result<RemoteResponse> {
        let receipt = Self::receipt_path(config, device, &request.request_id);
        ensure_private_directory(receipt.parent().unwrap(), "remote session creation")?;
        let previous = if receipt.exists() {
            let stored: Creation = read_private_json(&receipt, 64 * 1024)?;
            ensure!(
                stored.request == *request,
                "request identifier already used with different arguments"
            );
            if let Some(response) = &stored.response {
                return Ok(response.clone());
            }
            Some(stored)
        } else {
            None
        };
        let directory = resolve_directory(path)?;
        let mut creation = if let Some(stored) = previous {
            stored
        } else {
            ensure!(
                hub.summary().len() < 32,
                "too many shared sessions; close a session first"
            );
            let creation = planned_creation(request);
            publish_private_json(&receipt, &creation)?;
            creation
        };
        let session_id = creation.session_id.clone();
        let owners = config.join("remote").join("owners");
        ensure_private_directory(&owners, "remote session owners")?;
        let log_path = owners.join(format!("{session_id}.log"));
        // Nothing is left of a failed start, so the same request may run again.
        let failed = |reason: String| -> anyhow::Error {
            note_in_log(&log_path, &reason);
            let _ = remove_if_exists(&receipt);
            let output = match log_tail(&log_path) {
                Some(tail) => format!("its last output: {tail}"),
                None => "it wrote nothing".to_owned(),
            };
            OwnerStartFailed(format!(
                "session {session_id} was not created: {reason}; {output} (owner log {})",
                log_path.display()
            ))
            .into()
        };
        // A retry finds the owner of an earlier attempt by its lease and
        // waits for that one: a session never has two owner processes.
        let mut child = None;
        if hub.session_info(&session_id).is_none() && !owner_alive(&owners, &session_id) {
            let log = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&log_path)?;
            let mut command = tokio::process::Command::new(program);
            command
                .args(["remote", "session-owner", "--session-id", &session_id])
                .current_dir(directory)
                .env("RUSTCODE_CONFIG_DIR", config)
                .stdin(std::process::Stdio::null())
                .stdout(log.try_clone()?)
                .stderr(log);
            // SAFETY: setsid is async-signal-safe; no shared state is touched.
            unsafe {
                command.as_std_mut().pre_exec(|| {
                    if libc::setsid() == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            child = Some(command.spawn().map_err(|error| {
                failed(format!("the session owner could not be launched ({error})"))
            })?);
        }
        let started = Instant::now();
        let session = loop {
            if let Some(info) = hub.session_info(&session_id) {
                break info;
            }
            if let Some(owner) = child.as_mut()
                && let Ok(Some(status)) = owner.try_wait()
            {
                return Err(failed(format!(
                    "the session owner exited before registering ({status})"
                )));
            }
            if started.elapsed() >= timeout {
                let Some(mut owner) = child else {
                    anyhow::bail!(
                        "session {session_id} is still starting: its owner process is alive but has not registered with this gateway; retry with the same request identifier to check it (receipt {}, owner log {})",
                        receipt.display(),
                        log_path.display()
                    );
                };
                // An owner that registers this late is gone with its
                // connection: the hub closes the session it opened.
                let _ = owner.start_kill();
                let _ = owner.wait().await;
                return Err(failed(format!(
                    "the session owner did not register with the gateway within {} s and was stopped",
                    timeout.as_secs_f32()
                )));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        if let Some(mut owner) = child {
            // Reap eventually without coupling the owner's lifetime to the gateway.
            tokio::spawn(async move {
                let _ = owner.wait().await;
            });
        }
        let mut response = RemoteResponse::new(
            request.request_id.clone(),
            RemoteResult::SessionCreated { session },
        );
        response.receipt = Some(ReceiptState::Applied);
        creation.response = Some(response.clone());
        publish_private_json(&receipt, &creation)?;
        Ok(response)
    }
}
fn send(sink: &FrameSink, response: RemoteResponse) {
    if let Ok(frame) = serde_json::to_string(&RemoteFrame::Response(response)) {
        let _ = sink.send(frame);
    }
}
impl FrameRouter for WorkspaceRouter {
    fn connected(&self, device: &DeviceContext, sink: &FrameSink) {
        self.hub.connected(device, sink);
    }
    fn disconnected(&self, device: &DeviceContext) {
        self.hub.disconnected(device);
    }
    fn frame(&self, device: &DeviceContext, frame: &str, sink: &FrameSink) {
        let Ok(request) = decode_request(frame) else {
            return self.hub.frame(device, frame, sink);
        };
        match &request.operation {
            RemoteOperation::ListDirectories { path } => {
                let path = path.clone();
                let sink = sink.clone();
                tokio::spawn(async move {
                    let result =
                        tokio::task::spawn_blocking(move || list_directories(path.as_deref()))
                            .await;
                    let response = match result {
                        Ok(Ok(result)) => RemoteResponse::new(request.request_id, result),
                        error => RemoteResponse::error(
                            request.request_id,
                            RemoteError::new(
                                RemoteErrorCode::InvalidRequest,
                                format!("Cannot browse this directory: {error:?}"),
                            ),
                        ),
                    };
                    send(&sink, response);
                });
            }
            RemoteOperation::CreateSession { path } => {
                if let Ok(stored) = read_private_json::<Creation>(
                    &Self::receipt_path(&self.config, &device.device_id, &request.request_id),
                    64 * 1024,
                ) && stored.request != request
                {
                    let mut response = RemoteResponse::error(
                        request.request_id,
                        RemoteError::new(
                            RemoteErrorCode::RequestConflict,
                            "request identifier already used with different arguments",
                        ),
                    );
                    response.receipt = Some(ReceiptState::Rejected);
                    send(sink, response);
                    return;
                }
                let path = path.clone();
                let hub = self.hub.clone();
                let config = self.config.clone();
                let program = self.program.clone();
                let gate = self.creation.clone();
                let sink = sink.clone();
                let device = device.device_id.clone();
                tokio::spawn(async move {
                    let _guard = gate.lock().await;
                    let response = Self::create(
                        &hub,
                        &config,
                        &program,
                        &device,
                        &request,
                        path.as_deref(),
                        OWNER_START_TIMEOUT,
                    )
                    .await
                    .unwrap_or_else(|error| {
                        // A failed start is a decided outcome, not a
                        // lost one: the device is free to ask again.
                        let failed = error.downcast_ref::<OwnerStartFailed>().is_some();
                        let uncertain = !failed
                            && Self::receipt_path(&config, &device, &request.request_id).exists();
                        let mut response = RemoteResponse::error(
                            request.request_id.clone(),
                            RemoteError::new(
                                if failed {
                                    RemoteErrorCode::Internal
                                } else if uncertain {
                                    RemoteErrorCode::OwnerUnavailable
                                } else {
                                    RemoteErrorCode::InvalidRequest
                                },
                                format!("{error:#}"),
                            ),
                        );
                        response.receipt = Some(if uncertain {
                            ReceiptState::Unknown
                        } else {
                            ReceiptState::Rejected
                        });
                        response
                    });
                    send(&sink, response);
                });
            }
            RemoteOperation::GetRequestStatus { target_request_id } => {
                let path = Self::receipt_path(&self.config, &device.device_id, target_request_id);
                if let Ok(mut creation) = read_private_json::<Creation>(&path, 64 * 1024) {
                    if creation.response.is_none()
                        && let Some(session) = self.hub.session_info(&creation.session_id)
                    {
                        let mut response = RemoteResponse::new(
                            creation.request.request_id.clone(),
                            RemoteResult::SessionCreated { session },
                        );
                        response.receipt = Some(ReceiptState::Applied);
                        creation.response = Some(response);
                        let _ = publish_private_json(&path, &creation);
                    }
                    let result = creation.response.map(|response| Box::new(response.result));
                    send(
                        sink,
                        RemoteResponse::new(
                            request.request_id,
                            RemoteResult::RequestStatus {
                                target_request_id: target_request_id.clone(),
                                receipt: if result.is_some() {
                                    ReceiptState::Applied
                                } else {
                                    ReceiptState::Received
                                },
                                result,
                            },
                        ),
                    );
                } else {
                    self.hub.frame(device, frame, sink);
                }
            }
            _ => self.hub.frame(device, frame, sink),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn directories_exclude_files_and_bound_listing() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("b")).unwrap();
        std::fs::create_dir(root.path().join("A")).unwrap();
        std::fs::write(root.path().join("file"), "").unwrap();
        let RemoteResult::Directories { directories, .. } =
            list_directories(root.path().to_str()).unwrap()
        else {
            panic!()
        };
        assert_eq!(
            directories
                .iter()
                .map(|d| d.name.as_str())
                .collect::<Vec<_>>(),
            vec!["A", "b"]
        );
        assert!(resolve_directory(Some("relative")).is_err());
        assert!(resolve_directory(root.path().join("file").to_str()).is_err());
    }
    #[test]
    fn mobile_creation_uses_the_normal_chronological_session_identity() {
        let request = RemoteRequest {
            authenticated_device_id: None,
            protocol_version: REMOTE_PROTOCOL_VERSION,
            request_id: "new".into(),
            session_id: None,
            registration_epoch: None,
            operation: RemoteOperation::CreateSession { path: None },
        };
        let creation = planned_creation(&request);
        assert!(rustcode_session::valid_session_id(&creation.session_id));
        let root = tempfile::tempdir().unwrap();
        let path = rustcode_session::SessionStore::new(root.path())
            .canonical_session_dir(&creation.session_id);
        let today = chrono::Local::now().format("%Y/%m/%d").to_string();
        assert!(
            path.to_string_lossy().contains(&today),
            "{} was not filed under {today}",
            path.display()
        );
        assert_ne!(creation.session_id, planned_creation(&request).session_id);
    }

    #[tokio::test]
    async fn completed_creation_replays_after_directory_removal() {
        let config = tempfile::tempdir().unwrap();
        let request = RemoteRequest {
            authenticated_device_id: None,
            protocol_version: REMOTE_PROTOCOL_VERSION,
            request_id: "create-1".into(),
            session_id: None,
            registration_epoch: None,
            operation: RemoteOperation::CreateSession {
                path: Some("/directory/no/longer/present".into()),
            },
        };
        let response = RemoteResponse::new(request.request_id.clone(), RemoteResult::Detached);
        let receipt = WorkspaceRouter::receipt_path(config.path(), "phone", &request.request_id);
        ensure_private_directory(receipt.parent().unwrap(), "test").unwrap();
        publish_private_json(
            &receipt,
            &Creation {
                request: request.clone(),
                session_id: "0123456789abcdef0123456789abcdef".into(),
                response: Some(response.clone()),
            },
        )
        .unwrap();
        let hub = SessionHub::start(Default::default());
        assert_eq!(
            WorkspaceRouter::create(
                &hub,
                config.path(),
                Path::new("/never/launch"),
                "phone",
                &request,
                Some("/directory/no/longer/present"),
                OWNER_START_TIMEOUT,
            )
            .await
            .unwrap(),
            response
        );
        assert_ne!(
            receipt,
            WorkspaceRouter::receipt_path(config.path(), "other-phone", &request.request_id)
        );
    }
    fn creation_request(id: &str, directory: &Path) -> RemoteRequest {
        RemoteRequest {
            authenticated_device_id: None,
            protocol_version: REMOTE_PROTOCOL_VERSION,
            request_id: id.into(),
            session_id: None,
            registration_epoch: None,
            operation: RemoteOperation::CreateSession {
                path: Some(directory.to_string_lossy().into_owned()),
            },
        }
    }

    /// `/bin/sh remote session-owner …` runs the file `remote` of the session
    /// directory: a stand-in owner that never registers.
    fn stand_in_owner(script: &str) -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("remote"), script).unwrap();
        directory
    }

    fn alive(pid: &str) -> bool {
        // SAFETY: signal 0 only checks that the process exists.
        unsafe { libc::kill(pid.trim().parse().unwrap(), 0) == 0 }
    }

    #[tokio::test]
    async fn an_owner_that_exits_before_registering_fails_the_creation_with_its_output() {
        let config = tempfile::tempdir().unwrap();
        let directory = stand_in_owner("echo 'provider key missing' >&2\nexit 3\n");
        let request = creation_request("create-1", directory.path());
        let hub = SessionHub::start(Default::default());
        let error = WorkspaceRouter::create(
            &hub,
            config.path(),
            Path::new("/bin/sh"),
            "phone",
            &request,
            directory.path().to_str(),
            Duration::from_secs(30),
        )
        .await
        .unwrap_err();
        assert!(error.downcast_ref::<OwnerStartFailed>().is_some());
        let message = error.to_string();
        assert!(message.contains("exited before registering"), "{message}");
        assert!(message.contains("provider key missing"), "{message}");
        assert!(message.contains("remote/owners/"), "{message}");
        // Nothing is left to be uncertain about: the request may run again.
        let receipt = WorkspaceRouter::receipt_path(config.path(), "phone", "create-1");
        assert!(!receipt.exists());
        let logs: Vec<_> = std::fs::read_dir(config.path().join("remote/owners"))
            .unwrap()
            .flatten()
            .filter(|entry| entry.path().extension().is_some_and(|e| e == "log"))
            .collect();
        assert_eq!(logs.len(), 1);
        let log = std::fs::read_to_string(logs[0].path()).unwrap();
        assert!(log.contains("provider key missing"), "{log}");
        assert!(log.contains("gateway: the session owner exited"), "{log}");
    }

    #[tokio::test]
    async fn owners_that_never_register_are_stopped_and_do_not_accumulate() {
        let config = tempfile::tempdir().unwrap();
        let directory = stand_in_owner("echo $$ > owner-$$.pid\nexec sleep 600\n");
        let request = creation_request("create-1", directory.path());
        let hub = SessionHub::start(Default::default());
        for _ in 0..3 {
            let error = WorkspaceRouter::create(
                &hub,
                config.path(),
                Path::new("/bin/sh"),
                "phone",
                &request,
                directory.path().to_str(),
                Duration::from_millis(400),
            )
            .await
            .unwrap_err();
            assert!(error.downcast_ref::<OwnerStartFailed>().is_some());
            let message = error.to_string();
            assert!(message.contains("did not register"), "{message}");
            assert!(message.contains("it wrote nothing"), "{message}");
            assert!(!WorkspaceRouter::receipt_path(config.path(), "phone", "create-1").exists());
        }
        let owners: Vec<String> = std::fs::read_dir(directory.path())
            .unwrap()
            .flatten()
            .filter(|entry| entry.path().extension().is_some_and(|e| e == "pid"))
            .map(|entry| std::fs::read_to_string(entry.path()).unwrap())
            .collect();
        assert_eq!(owners.len(), 3);
        assert!(owners.iter().all(|pid| !alive(pid)), "{owners:?}");
        let lifecycle = crate::remote_gateway::lifecycle::RemoteLifecycle::new(config.path());
        assert!(live_owner_sessions(&lifecycle).is_empty());
    }

    #[tokio::test]
    async fn a_retry_waits_for_the_owner_that_is_already_running() {
        let config = tempfile::tempdir().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let request = creation_request("create-1", directory.path());
        let session_id = rustcode_session::next_session_id();
        let receipt = WorkspaceRouter::receipt_path(config.path(), "phone", "create-1");
        ensure_private_directory(receipt.parent().unwrap(), "test").unwrap();
        publish_private_json(
            &receipt,
            &Creation {
                request: request.clone(),
                session_id: session_id.clone(),
                response: None,
            },
        )
        .unwrap();
        // An owner from an earlier attempt holds the session's lease.
        let owners = config.path().join("remote/owners");
        let _lease =
            lock_private_file(&owners, &format!("{session_id}.lock"), OWNER_LEASE).unwrap();
        let lifecycle = crate::remote_gateway::lifecycle::RemoteLifecycle::new(config.path());
        assert_eq!(live_owner_sessions(&lifecycle), vec![session_id.clone()]);
        let hub = SessionHub::start(Default::default());
        // The program does not exist: launching a second owner would say so.
        let error = WorkspaceRouter::create(
            &hub,
            config.path(),
            Path::new("/never/launch"),
            "phone",
            &request,
            directory.path().to_str(),
            Duration::from_millis(200),
        )
        .await
        .unwrap_err();
        assert!(error.downcast_ref::<OwnerStartFailed>().is_none());
        let message = error.to_string();
        assert!(message.contains(&session_id), "{message}");
        assert!(
            message.contains("alive but has not registered"),
            "{message}"
        );
        assert!(message.contains("created/"), "{message}");
        assert!(receipt.exists());
    }

    #[test]
    fn oversized_folder_listing_is_bounded_before_transport() {
        let root = tempfile::tempdir().unwrap();
        for n in 0..520 {
            std::fs::create_dir(root.path().join(format!("{n:04}{}", "a".repeat(220)))).unwrap();
        }
        let result = list_directories(root.path().to_str()).unwrap();
        assert!(matches!(
            &result,
            RemoteResult::Directories {
                truncated: true,
                ..
            }
        ));
        assert!(serde_json::to_vec(&result).unwrap().len() < MAX_REMOTE_FRAME_BYTES);
    }
}

/// Construct the dedicated owner's state while holding its cross-process lease.
pub fn owner_state(
    config: &Path,
    session_id: &str,
) -> Result<(std::fs::File, crate::app::AppState)> {
    ensure!(
        rustcode_session::valid_session_id(session_id),
        "invalid mobile session identifier"
    );
    // The gateway probes a lease to see whether its owner lives, holding a
    // free one for an instant; only a lease that stays taken is another owner.
    let waited = Instant::now();
    let lease = loop {
        match lock_private_file(
            &config.join("remote/owners"),
            &format!("{session_id}.lock"),
            OWNER_LEASE,
        ) {
            Err(error) if is_lock_busy(&error) && waited.elapsed() < Duration::from_secs(2) => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(error) => {
                return Err(error.context("another process already owns this session"));
            }
            Ok(lease) => break lease,
        }
    };
    let workspace = std::env::current_dir()?;
    let mut state = crate::app::AppState::new_with_workspace_session(&workspace, Some(session_id));
    state.workspace_root = Some(workspace.clone());
    state.task_working_directory = Some(workspace.clone());
    state.history = crate::config::load_session_history_direct(session_id).into();
    crate::config::set_active_session_id(session_id);
    crate::config::record_session_settings(session_id, &state.config);
    let _ = crate::config::save_session_workspace(
        session_id,
        &rustcode_session::SessionWorkspace {
            cwd: workspace,
            additional_directories: Vec::new(),
            task_workspace_id: None,
        },
    );
    Ok((lease, state))
}
