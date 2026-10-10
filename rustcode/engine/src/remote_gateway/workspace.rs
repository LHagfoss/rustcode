//! Authenticated host-wide operations. Detached owners do not belong to a phone connection.
use super::{
    hub::SessionHub,
    router::{DeviceContext, FrameRouter, FrameSink},
};
use crate::daemon::lifecycle::{
    ensure_private_directory, publish_private_json, random_instance_id, read_private_json,
};
use crate::remote::*;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::os::unix::{fs::OpenOptionsExt, process::CommandExt};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

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
            let creation = Creation {
                request: request.clone(),
                session_id: random_instance_id()?,
                response: None,
            };
            publish_private_json(&receipt, &creation)?;
            creation
        };
        if hub.session_info(&creation.session_id).is_none() {
            let owners = config.join("remote").join("owners");
            ensure_private_directory(&owners, "remote session owners")?;
            let log = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(owners.join(format!("{}.log", creation.session_id)))?;
            let mut command = tokio::process::Command::new(program);
            command
                .args([
                    "remote",
                    "session-owner",
                    "--session-id",
                    &creation.session_id,
                ])
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
            let mut child = command.spawn().context("could not launch session owner")?;
            // Reap eventually without coupling the owner's lifetime to the gateway.
            tokio::spawn(async move {
                let _ = child.wait().await;
            });
        }
        let session = tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                if let Some(info) = hub.session_info(&creation.session_id) {
                    break info;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .context("session is starting; retry with the same request identifier to check it")?;
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
                    let response =
                        Self::create(&hub, &config, &program, &device, &request, path.as_deref())
                            .await
                            .unwrap_or_else(|error| {
                                let uncertain =
                                    Self::receipt_path(&config, &device, &request.request_id)
                                        .exists();
                                let mut response = RemoteResponse::error(
                                    request.request_id.clone(),
                                    RemoteError::new(
                                        if uncertain {
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
    #[tokio::test]
    async fn completed_creation_replays_after_directory_removal() {
        let config = tempfile::tempdir().unwrap();
        let request = RemoteRequest {
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
                Some("/directory/no/longer/present")
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
        session_id.len() == 32 && session_id.bytes().all(|c| c.is_ascii_hexdigit()),
        "invalid mobile session identifier"
    );
    let lease = crate::daemon::lifecycle::lock_private_file(
        &config.join("remote/owners"),
        &format!("{session_id}.lock"),
        "remote session",
    )?;
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
