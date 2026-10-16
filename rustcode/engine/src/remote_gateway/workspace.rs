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
const OWNER_START_TIMEOUT: Duration = Duration::from_secs(60);
/// Every session start is answered within this long of its arrival. The app
/// gives up on a request after 20 s; an answer after that is never seen.
const ANSWER_DEADLINE: Duration = Duration::from_secs(15);
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

/// A request the gateway turns down before anything is started, with the
/// code the device is told. Nothing is recorded, so the device may ask again.
#[derive(Debug)]
struct Refused(RemoteErrorCode, String);

impl std::fmt::Display for Refused {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.1)
    }
}

impl std::error::Error for Refused {}

fn refused(code: RemoteErrorCode, message: impl Into<String>) -> anyhow::Error {
    Refused(code, message.into()).into()
}

/// Whether the owner process of `session_id` is running: it holds its lease
/// for as long as it lives. The probe takes a free lease for an instant,
/// which [`owner_state`] rides out.
pub(crate) fn owner_alive(owners: &Path, session_id: &str) -> bool {
    // No lease file, no owner: probing would create one for every session asked about.
    if !owners.join(format!("{session_id}.lock")).exists() {
        return false;
    }
    match lock_private_file(owners, &format!("{session_id}.lock"), OWNER_LEASE) {
        Ok(_free) => false,
        Err(error) => is_lock_busy(&error),
    }
}

/// Sessions with a running owner process, registered with the gateway or not.
pub(crate) fn live_owner_sessions(lifecycle: &super::lifecycle::RemoteLifecycle) -> Vec<String> {
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

fn home_directory() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|home| home.is_absolute())
        .context("home directory unavailable")
}

/// `~` and `~/…` relative to `home`; anything else as written.
fn expand_home(home: &Path, value: &str) -> PathBuf {
    match value {
        "~" | "~/" => home.to_path_buf(),
        _ => match value.strip_prefix("~/") {
            Some(rest) => home.join(rest),
            None => PathBuf::from(value),
        },
    }
}

/// The folders a device may browse and start sessions in: the home
/// directory and the `[remote] roots` of the user configuration, each by its
/// real path. A root that does not exist, or is not absolute, is skipped.
/// Read on every request, so editing `config.toml` needs no gateway restart.
pub fn allowed_roots(config: &Path) -> Result<Vec<PathBuf>> {
    #[derive(Deserialize, Default)]
    struct UserConfig {
        #[serde(default)]
        remote: crate::config::RemoteConfig,
    }
    let home = home_directory()?;
    let mut roots = vec![home.canonicalize().unwrap_or_else(|_| home.clone())];
    let configured = std::fs::read_to_string(config.join(crate::config::CONFIG_TOML_FILE))
        .ok()
        .and_then(|text| toml::from_str::<UserConfig>(&text).ok())
        .map(|file| file.remote.roots)
        .unwrap_or_default();
    for root in configured {
        let root = expand_home(&home, root.trim());
        if root.is_absolute()
            && let Ok(root) = root.canonicalize()
            && root.is_dir()
            && !roots.contains(&root)
        {
            roots.push(root);
        }
    }
    Ok(roots)
}

/// The real path of the folder a device named (home when it named none),
/// which must exist, be a directory, and lie inside one of `roots`.
pub fn resolve_directory(path: Option<&str>, roots: &[PathBuf]) -> Result<PathBuf> {
    if path.is_some_and(|value| value.len() > 4096) {
        return Err(refused(
            RemoteErrorCode::InvalidRequest,
            "the folder path is too long",
        ));
    }
    let home = home_directory()?;
    let named = match path.map(str::trim).filter(|v| !v.is_empty()) {
        None => home,
        Some(value) => expand_home(&home, value),
    };
    if !named.is_absolute() {
        return Err(refused(
            RemoteErrorCode::InvalidRequest,
            "choose an absolute folder path",
        ));
    }
    let path = named.canonicalize().map_err(|_| {
        refused(
            RemoteErrorCode::InvalidRequest,
            format!("{} does not exist or cannot be read", named.display()),
        )
    })?;
    if !path.is_dir() {
        return Err(refused(
            RemoteErrorCode::InvalidRequest,
            format!("{} is a file; choose a folder", path.display()),
        ));
    }
    if !roots.iter().any(|root| path.starts_with(root)) {
        return Err(refused(
            RemoteErrorCode::InvalidRequest,
            format!(
                "{} is outside your home folder; choose a folder inside it, or add one to `roots` under `[remote]` in config.toml on the host",
                path.display()
            ),
        ));
    }
    Ok(path)
}

pub fn list_directories(path: Option<&str>, roots: &[PathBuf]) -> Result<RemoteResult> {
    let path = resolve_directory(path, roots)?;
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
        // Never offer a way up out of the folders a device may use.
        parent: path
            .parent()
            .filter(|parent| roots.iter().any(|root| parent.starts_with(root)))
            .map(|v| v.to_string_lossy().into_owned()),
        path: path.to_string_lossy().into_owned(),
        directories,
        truncated,
    })
}

/// Longest accepted `query` of `list_saved_sessions`, in bytes.
const MAX_SAVED_SESSION_QUERY_BYTES: usize = 256;
/// Longest title reported per saved session, in characters.
const MAX_SAVED_SESSION_TITLE_CHARS: usize = 200;

/// One page of the host's saved sessions, newest first, each marked with
/// whether a process holds it. Only ids the store itself lists, with a file
/// inside the store, are ever reported: the same rule ACP's `session/list`
/// applies.
fn list_saved_sessions(
    config: &Path,
    state_of: impl Fn(&str) -> SavedSessionState,
    cursor: Option<&str>,
    limit: Option<u32>,
    query: Option<&str>,
) -> Result<RemoteResult> {
    let limit = limit.unwrap_or(SAVED_SESSIONS_DEFAULT_LIMIT);
    if !(1..=SAVED_SESSIONS_MAX_LIMIT).contains(&limit) {
        return Err(refused(
            RemoteErrorCode::InvalidRequest,
            format!("limit must be 1 to {SAVED_SESSIONS_MAX_LIMIT}"),
        ));
    }
    if cursor.is_some_and(|cursor| !rustcode_session::valid_session_id(cursor)) {
        return Err(refused(
            RemoteErrorCode::InvalidRequest,
            "the cursor is not one this host issued",
        ));
    }
    let query = query
        .map(str::trim)
        .filter(|query| !query.is_empty())
        .map(|query| {
            if query.len() > MAX_SAVED_SESSION_QUERY_BYTES {
                return Err(refused(
                    RemoteErrorCode::InvalidRequest,
                    format!("query is longer than {MAX_SAVED_SESSION_QUERY_BYTES} bytes"),
                ));
            }
            Ok(query.to_lowercase())
        })
        .transpose()?;
    let store = rustcode_session::SessionStore::new(config);
    let (metas, mut next_cursor) = store
        .list_sessions_page(
            cursor,
            limit as usize,
            &rustcode_session::SessionScope::All,
            |meta| {
                let Some(id) = rustcode_session::SessionStore::session_id_from_path(&meta.path)
                else {
                    return false;
                };
                if !rustcode_session::valid_session_id(&id)
                    || !crate::config::session_file_in_store(config, &meta.path)
                {
                    return false;
                }
                query.as_deref().is_none_or(|query| {
                    meta.title.to_lowercase().contains(query)
                        || id.contains(query)
                        || meta
                            .workspace_cwd
                            .as_ref()
                            .is_some_and(|cwd| cwd.to_string_lossy().to_lowercase().contains(query))
                })
            },
        )
        .map_err(|()| {
            refused(
                RemoteErrorCode::InvalidRequest,
                "unknown or stale cursor; list again from the first page",
            )
        })?;
    let mut sessions = Vec::with_capacity(metas.len());
    let mut bytes = 0usize;
    for meta in metas {
        let Some(session_id) = rustcode_session::SessionStore::session_id_from_path(&meta.path)
        else {
            continue;
        };
        let updated_at = std::fs::metadata(&meta.path)
            .and_then(|metadata| metadata.modified())
            .ok()
            .map(|modified| {
                chrono::DateTime::<chrono::Utc>::from(modified)
                    .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
            });
        let session = RemoteSavedSession {
            state: state_of(&session_id),
            title: meta
                .title
                .chars()
                .take(MAX_SAVED_SESSION_TITLE_CHARS)
                .collect(),
            workspace: meta
                .workspace_cwd
                .as_ref()
                .map(|cwd| cwd.to_string_lossy().into_owned()),
            updated_at,
            message_count: meta.message_count as u64,
            session_id,
        };
        bytes += serde_json::to_vec(&session)?.len() + 1;
        // Long workspace paths must not push the page past one frame: end
        // the page early and continue after the last session that fits.
        if bytes > MAX_REMOTE_FRAME_BYTES / 2 {
            next_cursor = sessions
                .last()
                .map(|last: &RemoteSavedSession| last.session_id.clone());
            break;
        }
        sessions.push(session);
    }
    Ok(RemoteResult::SavedSessions {
        sessions,
        next_cursor,
    })
}

/// The directory a saved session's owner runs in: its recorded workspace,
/// which must still exist. Only a session the store lists, with its file
/// inside the store, is accepted.
fn saved_session_directory(config: &Path, session_id: &str) -> Result<PathBuf> {
    if !rustcode_session::valid_session_id(session_id) {
        return Err(refused(
            RemoteErrorCode::InvalidRequest,
            "not a session identifier",
        ));
    }
    let store = rustcode_session::SessionStore::new(config);
    let not_found = || {
        refused(
            RemoteErrorCode::NotFound,
            "no saved session has this identifier",
        )
    };
    let meta = store.session_meta_by_id(session_id).ok_or_else(not_found)?;
    if !crate::config::session_file_in_store(config, &meta.path) {
        return Err(not_found());
    }
    let cwd = store
        .load_session_workspace(session_id)
        .map(|workspace| workspace.cwd)
        .or(meta.workspace_cwd)
        .ok_or_else(|| {
            refused(
                RemoteErrorCode::InvalidRequest,
                "this session has no recorded workspace; resume it in a terminal",
            )
        })?;
    std::fs::canonicalize(&cwd)
        .ok()
        .filter(|cwd| cwd.is_dir())
        .ok_or_else(|| {
            refused(
                RemoteErrorCode::InvalidRequest,
                format!(
                    "the session's workspace {} no longer exists; resume it in a terminal",
                    cwd.display()
                ),
            )
        })
}

/// The result a launched owner's registration answers `request` with.
fn launched(request: &RemoteRequest, session: RemoteSessionInfo) -> RemoteResult {
    match request.operation {
        RemoteOperation::ResumeSession { .. } => RemoteResult::SessionResumed { session },
        _ => RemoteResult::SessionCreated { session },
    }
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
        hub.advertise_capabilities(&[host_capability::SAVED_SESSIONS]);
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
    /// The stored outcome of `request`, or the receipt of an attempt that
    /// has not finished. The same request ID with other arguments is refused.
    fn prior(receipt: &Path, request: &RemoteRequest) -> Result<Option<Creation>> {
        if !receipt.exists() {
            return Ok(None);
        }
        let stored: Creation = read_private_json(receipt, 64 * 1024)?;
        ensure!(
            stored.request == *request,
            "request identifier already used with different arguments"
        );
        Ok(Some(stored))
    }

    /// Wait for the right to start a session. Starts are decided one at a
    /// time, but a start that cannot even begin by `answer_by` is turned down
    /// instead of answered after the device has stopped listening.
    async fn start_turn(
        gate: &tokio::sync::Mutex<()>,
        answer_by: tokio::time::Instant,
    ) -> Result<tokio::sync::MutexGuard<'_, ()>> {
        tokio::time::timeout_at(answer_by, gate.lock())
            .await
            .map_err(|_| {
                refused(
                    RemoteErrorCode::Busy,
                    "another session is being started on the host; try again in a moment",
                )
            })
    }

    #[allow(clippy::too_many_arguments)]
    async fn create(
        hub: &Arc<SessionHub>,
        config: &Path,
        program: &Path,
        gate: &tokio::sync::Mutex<()>,
        device: &str,
        request: &RemoteRequest,
        path: Option<&str>,
        timing: StartTiming,
    ) -> Result<RemoteResponse> {
        let turn = Self::start_turn(gate, timing.answer_by).await?;
        let receipt = Self::receipt_path(config, device, &request.request_id);
        ensure_private_directory(receipt.parent().unwrap(), "remote session creation")?;
        let previous = Self::prior(&receipt, request)?;
        if let Some(response) = previous.as_ref().and_then(|stored| stored.response.clone()) {
            return Ok(response);
        }
        let directory = resolve_directory(path, &allowed_roots(config)?)?;
        // A home directory works but tracks hundreds of thousands of files:
        // flag it in the session log so the next slow-turn report starts with
        // the workspace root, not the network (#1954).
        if std::env::var_os("HOME").is_some_and(|home| directory == Path::new(&home)) {
            crate::logger::operational_event(
                "remote.create_home_workspace",
                serde_json::json!({
                    "session_hint": path,
                    "directory": directory.display().to_string(),
                }),
            );
        }
        let creation = if let Some(stored) = previous {
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
        Self::launch(
            hub, config, program, turn, &receipt, creation, directory, timing,
        )
        .await
    }

    /// Open a saved session from a device: start a session owner on it in
    /// its recorded workspace, exactly as `create` does for a new one, and
    /// answer once it has registered. A session that is already shared is
    /// answered at once; one that a terminal holds is refused.
    #[allow(clippy::too_many_arguments)]
    async fn resume(
        hub: &Arc<SessionHub>,
        config: &Path,
        program: &Path,
        gate: &tokio::sync::Mutex<()>,
        device: &str,
        request: &RemoteRequest,
        session_id: &str,
        timing: StartTiming,
    ) -> Result<RemoteResponse> {
        let turn = Self::start_turn(gate, timing.answer_by).await?;
        let receipt = Self::receipt_path(config, device, &request.request_id);
        ensure_private_directory(receipt.parent().unwrap(), "remote session creation")?;
        let previous = Self::prior(&receipt, request)?;
        if let Some(response) = previous.as_ref().and_then(|stored| stored.response.clone()) {
            return Ok(response);
        }
        let directory = saved_session_directory(config, session_id)?;
        let creation = if let Some(stored) = previous {
            stored
        } else {
            if let Some(session) = hub.session_info(session_id) {
                let response = RemoteResponse::new(
                    request.request_id.clone(),
                    RemoteResult::SessionResumed { session },
                )
                .with_receipt(ReceiptState::Applied);
                publish_private_json(
                    &receipt,
                    &Creation {
                        request: request.clone(),
                        session_id: session_id.to_owned(),
                        response: Some(response.clone()),
                    },
                )?;
                return Ok(response);
            }
            if hub.is_announced(session_id) {
                return Err(refused(
                    RemoteErrorCode::Busy,
                    "this session is open in a terminal; ask that terminal to share it",
                ));
            }
            let owners = config.join("remote").join("owners");
            if owner_alive(&owners, session_id) {
                return Err(refused(
                    RemoteErrorCode::Busy,
                    "a session owner already holds this session without sharing it; check `rustcode remote status`",
                ));
            }
            ensure!(
                hub.summary().len() < 32,
                "too many shared sessions; close a session first"
            );
            let creation = Creation {
                request: request.clone(),
                session_id: session_id.to_owned(),
                response: None,
            };
            publish_private_json(&receipt, &creation)?;
            creation
        };
        Self::launch(
            hub, config, program, turn, &receipt, creation, directory, timing,
        )
        .await
    }

    /// Start the owner of `creation.session_id` in `directory` unless one is
    /// already running, and wait for it to register with the hub. The device
    /// is answered by `timing.answer_by` whatever happens; an owner that is
    /// still starting then is watched on, up to `timing.start_limit`.
    #[allow(clippy::too_many_arguments)]
    async fn launch(
        hub: &Arc<SessionHub>,
        config: &Path,
        program: &Path,
        turn: tokio::sync::MutexGuard<'_, ()>,
        receipt: &Path,
        creation: Creation,
        directory: PathBuf,
        timing: StartTiming,
    ) -> Result<RemoteResponse> {
        let session_id = creation.session_id.clone();
        let owners = config.join("remote").join("owners");
        ensure_private_directory(&owners, "remote session owners")?;
        let start = OwnerStart {
            verb: match creation.request.operation {
                RemoteOperation::ResumeSession { .. } => "resumed",
                _ => "created",
            },
            log_path: owners.join(format!("{session_id}.log")),
            receipt: receipt.to_path_buf(),
            session_id: session_id.clone(),
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
                .open(&start.log_path)?;
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
                start.failed(format!("the session owner could not be launched ({error})"))
            })?);
        }
        // The decision is made; waiting for registration does not hold up
        // other starts.
        drop(turn);
        let started = Instant::now();
        let mut polls: u32 = 0;
        let session = loop {
            if let Some(info) = hub.session_info(&session_id) {
                break info;
            }
            match child.as_mut() {
                Some(owner) => {
                    if let Ok(Some(status)) = owner.try_wait() {
                        return Err(start.failed(format!(
                            "the session owner exited before registering ({status})"
                        )));
                    }
                }
                // The owner of an earlier attempt: gone without registering
                // means that attempt failed. Probe its lease once a second.
                None => {
                    if polls % 20 == 19 && !owner_alive(&owners, &session_id) {
                        return Err(start.failed(
                            "the owner of an earlier attempt stopped without registering"
                                .to_owned(),
                        ));
                    }
                }
            }
            if started.elapsed() >= timing.start_limit
                && let Some(owner) = child
            {
                return Err(start.stop(owner, timing.start_limit).await);
            }
            if tokio::time::Instant::now() >= timing.answer_by {
                // Still starting. Keep the owner and its receipt; the device
                // learns the session from a retry or `get_request_status`.
                if let Some(owner) = child {
                    let hub = hub.clone();
                    let creation = Creation {
                        request: creation.request.clone(),
                        session_id: session_id.clone(),
                        response: None,
                    };
                    let start = start.clone();
                    tokio::spawn(async move {
                        start
                            .watch(hub, owner, creation, started, timing.start_limit)
                            .await;
                    });
                }
                anyhow::bail!(
                    "session {session_id} is still starting: its owner has not registered with the gateway yet; ask again with the same request to check it (the host waits up to {} s; receipt {}, owner log {})",
                    timing.start_limit.as_secs(),
                    receipt.display(),
                    start.log_path.display()
                );
            }
            polls = polls.wrapping_add(1);
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        if let Some(mut owner) = child {
            // Reap eventually without coupling the owner's lifetime to the gateway.
            tokio::spawn(async move {
                let _ = owner.wait().await;
            });
        }
        record_launched(receipt, creation, session)
    }

    /// The answer to a session start that did not succeed. A refusal or a
    /// failed start is final and leaves nothing behind; anything else with a
    /// receipt on disk may still turn into a session, so it is `unknown`.
    fn start_error(
        config: &Path,
        device: &str,
        request_id: &str,
        error: &anyhow::Error,
    ) -> RemoteResponse {
        let failed = error.downcast_ref::<OwnerStartFailed>().is_some();
        let refusal = error.downcast_ref::<Refused>().map(|refused| refused.0);
        let uncertain =
            !failed && refusal.is_none() && Self::receipt_path(config, device, request_id).exists();
        let mut response = RemoteResponse::error(
            request_id.to_owned(),
            RemoteError::new(
                if failed {
                    RemoteErrorCode::Internal
                } else if let Some(code) = refusal {
                    code
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
    }
}

/// When a session start must be answered, and how long its owner may take.
#[derive(Clone, Copy)]
struct StartTiming {
    answer_by: tokio::time::Instant,
    start_limit: Duration,
}

impl StartTiming {
    fn from_now(answer_within: Duration, start_limit: Duration) -> Self {
        Self {
            answer_by: tokio::time::Instant::now() + answer_within,
            start_limit,
        }
    }
}

/// Record that the owner of `creation` registered, and the answer.
fn record_launched(
    receipt: &Path,
    mut creation: Creation,
    session: RemoteSessionInfo,
) -> Result<RemoteResponse> {
    let response = RemoteResponse::new(
        creation.request.request_id.clone(),
        launched(&creation.request, session),
    )
    .with_receipt(ReceiptState::Applied);
    creation.response = Some(response.clone());
    publish_private_json(receipt, &creation)?;
    Ok(response)
}

/// One owner being started: where its outcome is written down.
#[derive(Clone)]
struct OwnerStart {
    session_id: String,
    verb: &'static str,
    log_path: PathBuf,
    receipt: PathBuf,
}

impl OwnerStart {
    /// Nothing is left of a failed start, so the same request may run again.
    fn failed(&self, reason: String) -> anyhow::Error {
        note_in_log(&self.log_path, &reason);
        let _ = remove_if_exists(&self.receipt);
        let output = match log_tail(&self.log_path) {
            Some(tail) => format!("its last output: {tail}"),
            None => "it wrote nothing".to_owned(),
        };
        OwnerStartFailed(format!(
            "session {} was not {}: {reason}; {output} (owner log {})",
            self.session_id,
            self.verb,
            self.log_path.display()
        ))
        .into()
    }

    /// Stop an owner that did not register within `limit`. One that
    /// registers this late is gone with its connection: the hub closes the
    /// session it opened.
    async fn stop(&self, mut owner: tokio::process::Child, limit: Duration) -> anyhow::Error {
        let _ = owner.start_kill();
        let _ = owner.wait().await;
        self.failed(format!(
            "the session owner did not register with the gateway within {} s and was stopped",
            limit.as_secs_f32()
        ))
    }

    /// Follow an owner the device was already answered about: record its
    /// session in the receipt once it registers, or stop it and withdraw
    /// the receipt when it exits or runs out of time.
    async fn watch(
        self,
        hub: Arc<SessionHub>,
        mut owner: tokio::process::Child,
        creation: Creation,
        started: Instant,
        limit: Duration,
    ) {
        loop {
            if let Some(session) = hub.session_info(&self.session_id) {
                // A retry may have recorded it already; the answer is the same.
                let _ = record_launched(&self.receipt, creation, session);
                note_in_log(
                    &self.log_path,
                    "the owner registered after the device was told it was still starting",
                );
                let _ = owner.wait().await;
                return;
            }
            if let Ok(Some(status)) = owner.try_wait() {
                let _ = self.failed(format!(
                    "the session owner exited before registering ({status})"
                ));
                return;
            }
            if started.elapsed() >= limit {
                let _ = self.stop(owner, limit).await;
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
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
                let config = self.config.clone();
                let sink = sink.clone();
                tokio::spawn(async move {
                    let result = tokio::task::spawn_blocking(move || {
                        list_directories(path.as_deref(), &allowed_roots(&config)?)
                    })
                    .await;
                    let response = match result {
                        Ok(Ok(result)) => RemoteResponse::new(request.request_id, result),
                        Ok(Err(error)) => RemoteResponse::error(
                            request.request_id,
                            RemoteError::new(
                                RemoteErrorCode::InvalidRequest,
                                format!("Cannot browse this folder: {error:#}"),
                            ),
                        ),
                        Err(error) => RemoteResponse::error(
                            request.request_id,
                            RemoteError::new(RemoteErrorCode::Internal, error.to_string()),
                        ),
                    };
                    send(&sink, response);
                });
            }
            RemoteOperation::ListSavedSessions {
                cursor,
                limit,
                query,
            } => {
                let (cursor, limit, query) = (cursor.clone(), *limit, query.clone());
                let hub = self.hub.clone();
                let config = self.config.clone();
                let sink = sink.clone();
                tokio::spawn(async move {
                    let owners = config.join("remote").join("owners");
                    let result = tokio::task::spawn_blocking(move || {
                        list_saved_sessions(
                            &config,
                            |session_id| {
                                if hub.session_info(session_id).is_some() {
                                    SavedSessionState::Shared
                                } else if hub.is_announced(session_id)
                                    || owner_alive(&owners, session_id)
                                {
                                    SavedSessionState::Open
                                } else {
                                    SavedSessionState::Saved
                                }
                            },
                            cursor.as_deref(),
                            limit,
                            query.as_deref(),
                        )
                    })
                    .await;
                    let response = match result {
                        Ok(Ok(result)) => RemoteResponse::new(request.request_id, result),
                        Ok(Err(error)) => {
                            let code = error
                                .downcast_ref::<Refused>()
                                .map_or(RemoteErrorCode::Internal, |refused| refused.0);
                            RemoteResponse::error(
                                request.request_id,
                                RemoteError::new(code, format!("{error:#}")),
                            )
                        }
                        Err(error) => RemoteResponse::error(
                            request.request_id,
                            RemoteError::new(RemoteErrorCode::Internal, error.to_string()),
                        ),
                    };
                    send(&sink, response);
                });
            }
            RemoteOperation::CreateSession { .. } | RemoteOperation::ResumeSession { .. } => {
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
                let hub = self.hub.clone();
                let config = self.config.clone();
                let program = self.program.clone();
                let gate = self.creation.clone();
                let sink = sink.clone();
                let device = device.device_id.clone();
                // The deadline runs from the request's arrival, waiting for
                // other starts included.
                let timing = StartTiming::from_now(ANSWER_DEADLINE, OWNER_START_TIMEOUT);
                tokio::spawn(async move {
                    let request_id = request.request_id.clone();
                    // The start runs in its own task so that even a panic in
                    // it is answered.
                    let work = {
                        let (config, device) = (config.clone(), device.clone());
                        tokio::spawn(async move {
                            match &request.operation {
                                RemoteOperation::ResumeSession { session_id } => {
                                    Self::resume(
                                        &hub, &config, &program, &gate, &device, &request,
                                        session_id, timing,
                                    )
                                    .await
                                }
                                RemoteOperation::CreateSession { path } => {
                                    Self::create(
                                        &hub,
                                        &config,
                                        &program,
                                        &gate,
                                        &device,
                                        &request,
                                        path.as_deref(),
                                        timing,
                                    )
                                    .await
                                }
                                _ => unreachable!("only session starts are routed here"),
                            }
                        })
                    };
                    let response = match work.await {
                        Ok(Ok(response)) => response,
                        // A failed start is a decided outcome, not a lost
                        // one: the device is free to ask again.
                        Ok(Err(error)) => Self::start_error(&config, &device, &request_id, &error),
                        Err(error) => {
                            let unknown =
                                Self::receipt_path(&config, &device, &request_id).exists();
                            let mut response = RemoteResponse::error(
                                request_id,
                                RemoteError::new(
                                    RemoteErrorCode::Internal,
                                    format!("the host failed while starting the session: {error}"),
                                ),
                            );
                            response.receipt = Some(if unknown {
                                ReceiptState::Unknown
                            } else {
                                ReceiptState::Rejected
                            });
                            response
                        }
                    };
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
                            launched(&creation.request, session),
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
        let roots = vec![root.path().canonicalize().unwrap()];
        let RemoteResult::Directories {
            directories,
            parent,
            ..
        } = list_directories(root.path().to_str(), &roots).unwrap()
        else {
            panic!()
        };
        // The root's own parent is not offered: a device cannot climb out.
        assert_eq!(parent, None);
        let RemoteResult::Directories { parent, .. } =
            list_directories(root.path().join("b").to_str(), &roots).unwrap()
        else {
            panic!()
        };
        assert_eq!(parent.as_deref(), roots[0].to_str());
        assert_eq!(
            directories
                .iter()
                .map(|d| d.name.as_str())
                .collect::<Vec<_>>(),
            vec!["A", "b"]
        );
        assert!(resolve_directory(Some("relative"), &roots).is_err());
        assert!(resolve_directory(root.path().join("file").to_str(), &roots).is_err());
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
                &tokio::sync::Mutex::new(()),
                "phone",
                &request,
                Some("/directory/no/longer/present"),
                timing(OWNER_START_TIMEOUT),
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
    /// Generous answer deadline, so only `start_limit` decides a test.
    fn timing(start_limit: Duration) -> StartTiming {
        StartTiming::from_now(Duration::from_secs(30), start_limit)
    }

    /// Let devices use `root`, as `[remote] roots` in the user config does.
    fn allow(config: &Path, root: &Path) {
        std::fs::write(
            config.join(crate::config::CONFIG_TOML_FILE),
            format!("[remote]\nroots = [{:?}]\n", root.to_str().unwrap()),
        )
        .unwrap();
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
        allow(config.path(), directory.path());
        let hub = SessionHub::start(Default::default());
        let error = WorkspaceRouter::create(
            &hub,
            config.path(),
            Path::new("/bin/sh"),
            &tokio::sync::Mutex::new(()),
            "phone",
            &request,
            directory.path().to_str(),
            timing(Duration::from_secs(30)),
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
        allow(config.path(), directory.path());
        let hub = SessionHub::start(Default::default());
        for _ in 0..3 {
            let error = WorkspaceRouter::create(
                &hub,
                config.path(),
                Path::new("/bin/sh"),
                &tokio::sync::Mutex::new(()),
                "phone",
                &request,
                directory.path().to_str(),
                timing(Duration::from_millis(400)),
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
        allow(config.path(), directory.path());
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
        let lease = lock_private_file(&owners, &format!("{session_id}.lock"), OWNER_LEASE).unwrap();
        let lifecycle = crate::remote_gateway::lifecycle::RemoteLifecycle::new(config.path());
        assert_eq!(live_owner_sessions(&lifecycle), vec![session_id.clone()]);
        let hub = SessionHub::start(Default::default());
        // The program does not exist: launching a second owner would say so.
        let error = WorkspaceRouter::create(
            &hub,
            config.path(),
            Path::new("/never/launch"),
            &tokio::sync::Mutex::new(()),
            "phone",
            &request,
            directory.path().to_str(),
            StartTiming::from_now(Duration::from_millis(200), Duration::from_secs(30)),
        )
        .await
        .unwrap_err();
        assert!(error.downcast_ref::<OwnerStartFailed>().is_none());
        let message = error.to_string();
        assert!(message.contains(&session_id), "{message}");
        assert!(message.contains("still starting"), "{message}");
        assert!(message.contains("created/"), "{message}");
        assert!(receipt.exists());

        // When that owner goes away without registering, the retry says so
        // and withdraws the attempt instead of waiting out its deadline.
        let retry = {
            let (hub, config, request) =
                (hub.clone(), config.path().to_path_buf(), request.clone());
            let path = directory.path().to_str().unwrap().to_owned();
            tokio::spawn(async move {
                WorkspaceRouter::create(
                    &hub,
                    &config,
                    Path::new("/never/launch"),
                    &tokio::sync::Mutex::new(()),
                    "phone",
                    &request,
                    Some(&path),
                    timing(Duration::from_secs(30)),
                )
                .await
            })
        };
        tokio::time::sleep(Duration::from_millis(200)).await;
        drop(lease);
        let error = retry.await.unwrap().unwrap_err();
        assert!(
            error.downcast_ref::<OwnerStartFailed>().is_some(),
            "{error}"
        );
        assert!(error.to_string().contains("earlier attempt"), "{error}");
        assert!(!receipt.exists());
    }

    /// A stand-in owner that records its pid and arguments, then waits to
    /// be registered (or stopped) by the test.
    const WAITING_OWNER: &str = "echo $$ > owner.pid\necho \"$@\" > started.args\nexec sleep 600\n";

    /// The session id the stand-in owner in `directory` was started for.
    async fn started_session(directory: &Path) -> String {
        let args = directory.join("started.args");
        for _ in 0..200 {
            if let Ok(args) = std::fs::read_to_string(&args)
                && let Some(id) = args.split_whitespace().last()
            {
                return id.to_owned();
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("the owner was never started");
    }

    fn stop_owner(directory: &Path) {
        if let Ok(pid) = std::fs::read_to_string(directory.join("owner.pid")) {
            // SAFETY: SIGKILL to the stand-in owner this test started.
            unsafe { libc::kill(pid.trim().parse().unwrap(), libc::SIGKILL) };
        }
    }

    /// What the owner socket does when the owner of `session_id` registers.
    fn register(hub: &Arc<SessionHub>, session_id: &str) -> super::super::hub::OwnerRegistration {
        hub.register_owner(
            SessionRegistration {
                session_id: session_id.to_owned(),
                registration_epoch: 1,
            },
            super::super::hub::tests::snapshot(session_id, 1, 0),
        )
        .unwrap()
    }

    fn stored(config: &Path, request_id: &str) -> Option<Creation> {
        read_private_json(
            &WorkspaceRouter::receipt_path(config, "phone", request_id),
            64 * 1024,
        )
        .ok()
    }

    #[tokio::test]
    async fn a_device_starts_a_session_in_a_folder_inside_its_roots() {
        let config = tempfile::tempdir().unwrap();
        let directory = stand_in_owner(WAITING_OWNER);
        allow(config.path(), directory.path());
        let request = creation_request("create-1", directory.path());
        let hub = SessionHub::start(Default::default());
        let gate = Arc::new(tokio::sync::Mutex::new(()));
        let creating = {
            let (hub, gate, config, request) = (
                hub.clone(),
                gate.clone(),
                config.path().to_path_buf(),
                request.clone(),
            );
            let path = directory.path().to_str().unwrap().to_owned();
            tokio::spawn(async move {
                WorkspaceRouter::create(
                    &hub,
                    &config,
                    Path::new("/bin/sh"),
                    &gate,
                    "phone",
                    &request,
                    Some(&path),
                    timing(Duration::from_secs(30)),
                )
                .await
            })
        };
        let session_id = started_session(directory.path()).await;
        let _registration = register(&hub, &session_id);
        let response = creating.await.unwrap().unwrap();
        stop_owner(directory.path());
        assert_eq!(response.receipt, Some(ReceiptState::Applied));
        let RemoteResult::SessionCreated { session } = &response.result else {
            panic!("{response:?}")
        };
        assert_eq!(session.session_id, session_id);
        assert!(rustcode_session::valid_session_id(&session_id));
        // In the list every device sees, and the answer is kept for a retry.
        assert!(
            hub.summary()
                .iter()
                .any(|listed| listed.session_id == session_id)
        );
        assert_eq!(
            stored(config.path(), "create-1").and_then(|stored| stored.response),
            Some(response)
        );
    }

    #[test]
    fn devices_may_only_use_folders_inside_home_or_a_configured_root() {
        let config = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("file"), "").unwrap();
        std::fs::write(
            config.path().join(crate::config::CONFIG_TOML_FILE),
            format!(
                "model = \"anything\"\n[remote]\nroots = [{:?}, \"relative/root\", \"/no/such/root\"]\n",
                root.path().to_str().unwrap()
            ),
        )
        .unwrap();
        let roots = allowed_roots(config.path()).unwrap();
        let home = home_directory().unwrap().canonicalize().unwrap();
        assert_eq!(roots, vec![home, root.path().canonicalize().unwrap()]);
        assert!(resolve_directory(root.path().to_str(), &roots).is_ok());
        for (path, says) in [
            (elsewhere.path().to_path_buf(), "outside your home folder"),
            (root.path().join("missing"), "does not exist"),
            (root.path().join("file"), "is a file"),
            (PathBuf::from("relative"), "absolute"),
        ] {
            let error = resolve_directory(path.to_str(), &roots).unwrap_err();
            assert_eq!(refusal(error), RemoteErrorCode::InvalidRequest);
            let message = resolve_directory(path.to_str(), &roots)
                .unwrap_err()
                .to_string();
            assert!(message.contains(says), "{message}");
        }
        // Without a configuration only home is allowed.
        let bare = tempfile::tempdir().unwrap();
        assert_eq!(allowed_roots(bare.path()).unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_folder_outside_the_roots_is_refused_and_nothing_is_started() {
        let config = tempfile::tempdir().unwrap();
        let directory = stand_in_owner(WAITING_OWNER);
        let request = creation_request("create-1", directory.path());
        let hub = SessionHub::start(Default::default());
        let error = WorkspaceRouter::create(
            &hub,
            config.path(),
            Path::new("/bin/sh"),
            &tokio::sync::Mutex::new(()),
            "phone",
            &request,
            directory.path().to_str(),
            timing(Duration::from_secs(30)),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("outside your home folder"));
        let response = WorkspaceRouter::start_error(config.path(), "phone", "create-1", &error);
        assert_eq!(response.receipt, Some(ReceiptState::Rejected));
        let RemoteResult::Error(error) = &response.result else {
            panic!("{response:?}")
        };
        assert_eq!(error.code, RemoteErrorCode::InvalidRequest);
        assert!(stored(config.path(), "create-1").is_none());
        assert!(!directory.path().join("started.args").exists());
        assert!(!config.path().join("remote/owners").exists());
    }

    #[tokio::test]
    async fn a_failed_owner_is_answered_with_a_final_internal_error() {
        let config = tempfile::tempdir().unwrap();
        let directory = stand_in_owner("echo 'no provider key' >&2\nexit 3\n");
        allow(config.path(), directory.path());
        let request = creation_request("create-1", directory.path());
        let hub = SessionHub::start(Default::default());
        let error = WorkspaceRouter::create(
            &hub,
            config.path(),
            Path::new("/bin/sh"),
            &tokio::sync::Mutex::new(()),
            "phone",
            &request,
            directory.path().to_str(),
            timing(Duration::from_secs(30)),
        )
        .await
        .unwrap_err();
        let response = WorkspaceRouter::start_error(config.path(), "phone", "create-1", &error);
        assert_eq!(response.receipt, Some(ReceiptState::Rejected));
        let RemoteResult::Error(error) = &response.result else {
            panic!("{response:?}")
        };
        assert_eq!(error.code, RemoteErrorCode::Internal);
        assert!(
            error.message.contains("no provider key"),
            "{}",
            error.message
        );
    }

    #[tokio::test]
    async fn an_owner_still_starting_is_answered_in_time_then_stopped_at_the_limit() {
        let config = tempfile::tempdir().unwrap();
        let directory = stand_in_owner(WAITING_OWNER);
        allow(config.path(), directory.path());
        let request = creation_request("create-1", directory.path());
        let hub = SessionHub::start(Default::default());
        let asked = Instant::now();
        let error = WorkspaceRouter::create(
            &hub,
            config.path(),
            Path::new("/bin/sh"),
            &tokio::sync::Mutex::new(()),
            "phone",
            &request,
            directory.path().to_str(),
            StartTiming::from_now(Duration::from_millis(300), Duration::from_millis(1500)),
        )
        .await
        .unwrap_err();
        assert!(asked.elapsed() < Duration::from_secs(1));
        assert!(error.to_string().contains("still starting"), "{error}");
        let response = WorkspaceRouter::start_error(config.path(), "phone", "create-1", &error);
        assert_eq!(response.receipt, Some(ReceiptState::Unknown));
        let RemoteResult::Error(error) = &response.result else {
            panic!("{response:?}")
        };
        assert_eq!(error.code, RemoteErrorCode::OwnerUnavailable);
        // Still running and still on record, for a retry to find.
        let pid = std::fs::read_to_string(directory.path().join("owner.pid")).unwrap();
        assert!(alive(&pid));
        assert!(stored(config.path(), "create-1").is_some());
        // At the start limit it is stopped and the attempt withdrawn.
        for _ in 0..100 {
            if !alive(&pid) && stored(config.path(), "create-1").is_none() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("the late owner was neither stopped nor withdrawn");
    }

    #[tokio::test]
    async fn an_owner_that_registers_after_the_answer_is_found_by_a_retry() {
        let config = tempfile::tempdir().unwrap();
        let directory = stand_in_owner(WAITING_OWNER);
        allow(config.path(), directory.path());
        let request = creation_request("create-1", directory.path());
        let hub = SessionHub::start(Default::default());
        let gate = tokio::sync::Mutex::new(());
        let error = WorkspaceRouter::create(
            &hub,
            config.path(),
            Path::new("/bin/sh"),
            &gate,
            "phone",
            &request,
            directory.path().to_str(),
            StartTiming::from_now(Duration::from_millis(300), Duration::from_secs(30)),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("still starting"), "{error}");
        let session_id = started_session(directory.path()).await;
        let _registration = register(&hub, &session_id);
        // The gateway records it without being asked.
        let mut recorded = None;
        for _ in 0..100 {
            recorded = stored(config.path(), "create-1").and_then(|stored| stored.response);
            if recorded.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        let recorded = recorded.expect("the late registration was recorded");
        let again = WorkspaceRouter::create(
            &hub,
            config.path(),
            Path::new("/never/launch"),
            &gate,
            "phone",
            &request,
            directory.path().to_str(),
            timing(Duration::from_secs(30)),
        )
        .await
        .unwrap();
        stop_owner(directory.path());
        assert_eq!(again, recorded);
        assert!(matches!(
            &again.result,
            RemoteResult::SessionCreated { session } if session.session_id == session_id
        ));
    }

    #[tokio::test]
    async fn a_start_that_cannot_begin_in_time_is_refused_as_busy() {
        let config = tempfile::tempdir().unwrap();
        let directory = tempfile::tempdir().unwrap();
        allow(config.path(), directory.path());
        let request = creation_request("create-1", directory.path());
        let hub = SessionHub::start(Default::default());
        let gate = tokio::sync::Mutex::new(());
        let _another_start = gate.lock().await;
        let error = WorkspaceRouter::create(
            &hub,
            config.path(),
            Path::new("/never/launch"),
            &gate,
            "phone",
            &request,
            directory.path().to_str(),
            StartTiming::from_now(Duration::from_millis(100), Duration::from_secs(30)),
        )
        .await
        .unwrap_err();
        assert_eq!(refusal(error), RemoteErrorCode::Busy);
        assert!(stored(config.path(), "create-1").is_none());
    }

    /// Saved sessions in a fresh store under `config`, oldest first, each
    /// with `workspace` recorded when given.
    fn saved_sessions(config: &Path, sessions: &[(&str, Option<&Path>)]) -> Vec<String> {
        let store = rustcode_session::SessionStore::new(config);
        let ids = sessions
            .iter()
            .map(|(title, workspace)| {
                let id = rustcode_session::next_session_id();
                let history = vec![
                    crate::app::ChatMessage::new("user", *title),
                    crate::app::ChatMessage::new("assistant", "answer"),
                ];
                store.save_session_history(&id, &history);
                store.save_session_title(&id, title);
                if let Some(cwd) = workspace {
                    store
                        .save_session_workspace(
                            &id,
                            &rustcode_session::SessionWorkspace {
                                cwd: cwd.to_path_buf(),
                                additional_directories: Vec::new(),
                                task_workspace_id: None,
                            },
                        )
                        .unwrap();
                }
                id
            })
            .collect();
        rustcode_session::flush_history();
        ids
    }

    fn saved_page(result: RemoteResult) -> (Vec<RemoteSavedSession>, Option<String>) {
        let RemoteResult::SavedSessions {
            sessions,
            next_cursor,
        } = result
        else {
            panic!("not a saved-sessions page: {result:?}");
        };
        (sessions, next_cursor)
    }

    fn refusal(error: anyhow::Error) -> RemoteErrorCode {
        error
            .downcast_ref::<Refused>()
            .unwrap_or_else(|| panic!("not a refusal: {error:#}"))
            .0
    }

    #[test]
    fn saved_sessions_are_paged_newest_first_marked_and_searchable() {
        let config = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let ids = saved_sessions(
            config.path(),
            &[
                ("Fix the parser", Some(project.path())),
                ("Write docs", None),
                ("Tune the Parser cache", Some(project.path())),
            ],
        );
        let shared = ids[1].clone();
        let state_of = |id: &str| {
            if id == shared {
                SavedSessionState::Shared
            } else {
                SavedSessionState::Saved
            }
        };
        let (first, cursor) =
            saved_page(list_saved_sessions(config.path(), state_of, None, Some(2), None).unwrap());
        assert_eq!(
            first
                .iter()
                .map(|s| s.session_id.as_str())
                .collect::<Vec<_>>(),
            [ids[2].as_str(), ids[1].as_str()]
        );
        assert_eq!(first[0].title, "Tune the Parser cache");
        assert_eq!(first[0].state, SavedSessionState::Saved);
        assert_eq!(first[0].message_count, 2);
        assert!(
            first[0]
                .updated_at
                .as_deref()
                .is_some_and(|at| at.ends_with('Z'))
        );
        assert!(first[0].workspace.is_some());
        assert_eq!(first[1].state, SavedSessionState::Shared);
        assert_eq!(first[1].workspace, None);
        let (second, next) = saved_page(
            list_saved_sessions(config.path(), state_of, cursor.as_deref(), Some(2), None).unwrap(),
        );
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].session_id, ids[0]);
        assert_eq!(next, None);

        let (found, _) = saved_page(
            list_saved_sessions(config.path(), state_of, None, None, Some(" parser ")).unwrap(),
        );
        assert_eq!(
            found
                .iter()
                .map(|s| s.session_id.as_str())
                .collect::<Vec<_>>(),
            [ids[2].as_str(), ids[0].as_str()]
        );

        for (cursor, limit, query) in [
            (Some("../escape"), None, None),
            (Some("no-such-session"), None, None),
            (None, Some(0), None),
            (None, Some(SAVED_SESSIONS_MAX_LIMIT + 1), None),
        ] {
            let error = list_saved_sessions(config.path(), state_of, cursor, limit, query)
                .expect_err("refused");
            assert_eq!(refusal(error), RemoteErrorCode::InvalidRequest);
        }
        let long = "x".repeat(MAX_SAVED_SESSION_QUERY_BYTES + 1);
        let error = list_saved_sessions(config.path(), state_of, None, None, Some(&long))
            .expect_err("refused");
        assert_eq!(refusal(error), RemoteErrorCode::InvalidRequest);
    }

    #[test]
    fn only_stored_sessions_with_a_live_workspace_can_be_resumed() {
        let config = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let gone = tempfile::tempdir().unwrap();
        let gone_path = gone.path().to_path_buf();
        let ids = saved_sessions(
            config.path(),
            &[
                ("ok", Some(project.path())),
                ("legacy", None),
                ("removed", Some(&gone_path)),
            ],
        );
        drop(gone);
        assert_eq!(
            saved_session_directory(config.path(), &ids[0]).unwrap(),
            project.path().canonicalize().unwrap()
        );
        assert_eq!(
            refusal(saved_session_directory(config.path(), &ids[1]).unwrap_err()),
            RemoteErrorCode::InvalidRequest
        );
        let error = saved_session_directory(config.path(), &ids[2]).unwrap_err();
        assert!(error.to_string().contains("no longer exists"), "{error:#}");
        assert_eq!(refusal(error), RemoteErrorCode::InvalidRequest);
        for id in ["../../etc", "", "a/b"] {
            assert_eq!(
                refusal(saved_session_directory(config.path(), id).unwrap_err()),
                RemoteErrorCode::InvalidRequest
            );
        }
        assert_eq!(
            refusal(saved_session_directory(config.path(), "unknown-session").unwrap_err()),
            RemoteErrorCode::NotFound
        );
    }

    fn resume_request(id: &str, session_id: &str) -> RemoteRequest {
        RemoteRequest {
            authenticated_device_id: None,
            protocol_version: REMOTE_PROTOCOL_VERSION,
            request_id: id.into(),
            session_id: None,
            registration_epoch: None,
            operation: RemoteOperation::ResumeSession {
                session_id: session_id.into(),
            },
        }
    }

    #[tokio::test]
    async fn resuming_starts_an_owner_on_the_saved_session_in_its_workspace() {
        let config = tempfile::tempdir().unwrap();
        // The stand-in owner records how it was started, then exits.
        let project = stand_in_owner("echo \"$@\" > started.args\npwd > started.cwd\nexit 4\n");
        let ids = saved_sessions(config.path(), &[("saved", Some(project.path()))]);
        let request = resume_request("resume-1", &ids[0]);
        let hub = SessionHub::start(Default::default());
        let error = WorkspaceRouter::resume(
            &hub,
            config.path(),
            Path::new("/bin/sh"),
            &tokio::sync::Mutex::new(()),
            "phone",
            &request,
            &ids[0],
            timing(Duration::from_secs(30)),
        )
        .await
        .unwrap_err();
        assert!(error.downcast_ref::<OwnerStartFailed>().is_some());
        let message = error.to_string();
        assert!(message.contains("was not resumed"), "{message}");
        let args = std::fs::read_to_string(project.path().join("started.args")).unwrap();
        assert_eq!(
            args.trim(),
            format!("session-owner --session-id {}", ids[0])
        );
        let cwd = std::fs::read_to_string(project.path().join("started.cwd")).unwrap();
        assert_eq!(
            Path::new(cwd.trim()).canonicalize().unwrap(),
            project.path().canonicalize().unwrap()
        );
        // A failed start leaves nothing behind: the device may ask again.
        assert!(!WorkspaceRouter::receipt_path(config.path(), "phone", "resume-1").exists());
    }

    #[tokio::test]
    async fn a_session_held_elsewhere_is_not_resumed_a_second_time() {
        let config = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let ids = saved_sessions(
            config.path(),
            &[
                ("terminal", Some(project.path())),
                ("owner", Some(project.path())),
            ],
        );
        let hub = SessionHub::start(Default::default());
        let _announcement = hub
            .announce_owner(ids[0].clone(), "terminal".into(), None)
            .unwrap();
        let owners = config.path().join("remote/owners");
        let _lease = lock_private_file(&owners, &format!("{}.lock", ids[1]), OWNER_LEASE).unwrap();
        for id in &ids {
            let error = WorkspaceRouter::resume(
                &hub,
                config.path(),
                Path::new("/never/launch"),
                &tokio::sync::Mutex::new(()),
                "phone",
                &resume_request(&format!("resume-{id}"), id),
                id,
                timing(Duration::from_millis(200)),
            )
            .await
            .unwrap_err();
            assert_eq!(refusal(error), RemoteErrorCode::Busy);
            assert!(
                !WorkspaceRouter::receipt_path(config.path(), "phone", &format!("resume-{id}"))
                    .exists()
            );
        }
    }

    #[test]
    fn probing_a_session_without_an_owner_leaves_no_lease_file() {
        let config = tempfile::tempdir().unwrap();
        let owners = config.path().join("remote/owners");
        assert!(!owner_alive(&owners, "never-owned"));
        assert!(!owners.join("never-owned.lock").exists());
    }

    #[test]
    fn the_gateway_advertises_saved_sessions() {
        let config = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let hub = SessionHub::start(Default::default());
            let _router = WorkspaceRouter::new(
                hub.clone(),
                config.path().to_path_buf(),
                PathBuf::from("/never/launch"),
            );
            let _again = WorkspaceRouter::new(
                hub.clone(),
                config.path().to_path_buf(),
                PathBuf::from("/never/launch"),
            );
            assert_eq!(hub.capabilities(), [host_capability::SAVED_SESSIONS]);
        });
    }

    #[test]
    fn oversized_folder_listing_is_bounded_before_transport() {
        let root = tempfile::tempdir().unwrap();
        for n in 0..520 {
            std::fs::create_dir(root.path().join(format!("{n:04}{}", "a".repeat(220)))).unwrap();
        }
        let roots = vec![root.path().canonicalize().unwrap()];
        let result = list_directories(root.path().to_str(), &roots).unwrap();
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
    note_owner_pid(&lease);
    let workspace = std::env::current_dir()?;
    // A saved session resumed from a device runs in its recorded workspace:
    // keep what was recorded with it (extra directories, its task worktree)
    // and load it the way ACP and the terminal resume a session.
    let recorded = crate::config::load_session_workspace(session_id).filter(|recorded| {
        rustcode_session::canonicalize_best_effort(&recorded.cwd)
            == rustcode_session::canonicalize_best_effort(&workspace)
    });
    let history = crate::config::load_session_history_direct(session_id);
    let mut state = crate::acp::build_loaded_app_state(session_id, &workspace, history);
    // A detached owner has no `--yolo` flag: start as the session last was,
    // or as the user's `yolo` default for a session that never set it.
    state.restore_auto_confirm();
    crate::config::set_active_session_id(session_id);
    crate::config::record_session_settings(session_id, &state.config);
    let _ = crate::config::save_session_workspace(
        session_id,
        &recorded.unwrap_or(rustcode_session::SessionWorkspace {
            cwd: workspace,
            additional_directories: Vec::new(),
            task_workspace_id: None,
        }),
    );
    Ok((lease, state))
}

/// Record this process in its lease so `/adopt` can find and stop it. The
/// lock itself is the liveness signal; the pid is only a hint and must be
/// validated (executable check) before signalling.
fn note_owner_pid(lease: &std::fs::File) {
    use std::io::{Seek, Write};
    let mut lease = lease;
    let _ = lease.rewind();
    let _ = lease.set_len(0);
    let _ = writeln!(lease, "pid={}", std::process::id());
    let _ = lease.flush();
}

/// The pid recorded in a session lease, if any. Stale when the owner is gone.
pub(crate) fn owner_pid(owners: &Path, session_id: &str) -> Option<u32> {
    let content = std::fs::read_to_string(owners.join(format!("{session_id}.lock"))).ok()?;
    content.strip_prefix("pid=")?.trim().parse().ok()
}
