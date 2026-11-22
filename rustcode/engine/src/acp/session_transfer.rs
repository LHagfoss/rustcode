//! Moving a thread between machines:
//! - `_rustcode/session/export`: reads a thread's canonical history, metadata,
//!   harness state and machine-origin trail as a chunked base64 JSON bundle so
//!   even large threads stay well below the remote gateway's 1 MiB frame cap.
//! - `_rustcode/session/import`: reassembles the bundle on the destination
//!   machine, resolves the target folder and harness smartly (restoring the
//!   thread's remembered folder, login, and untouched native session when
//!   moving back), appends a move divider, and writes the thread durably.
//! - `_rustcode/session/remove`: closes and deletes the source copy only after
//!   the destination has confirmed persistence, announcing its removal to all
//!   clients connected to the source machine.

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::harness::{self, store as harness_store};
use crate::sessions::MachineOrigin;
use rustcode_core::ChatMessage;

/// Raw bytes per export/import chunk (384 KiB raw -> 512 KiB base64, well below
/// `remote_gateway::gateway::MAX_FRAME_BYTES` = 1 MiB).
const CHUNK_BYTES: usize = 384 * 1024;
/// Maximum reassembled bundle size accepted by import (64 MiB).
const MAX_BUNDLE_BYTES: usize = 64 * 1024 * 1024;
/// Maximum concurrent in-flight exports or imports cached in memory.
const MAX_IN_FLIGHT: usize = 8;
/// Idle timeout for in-flight chunk caches.
const CACHE_TTL: Duration = Duration::from_secs(120);
const BUNDLE_VERSION: u32 = 1;

// ----------------------------------------------------------------- requests

#[derive(Debug, Clone, Default, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[request(method = "_rustcode/session/export", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExportSessionRequest {
    pub session_id: String,
    /// Snapshot id returned on the first chunk (`offset == 0`) so later chunks
    /// read the exact same serialized bytes.
    #[serde(default)]
    pub export_id: Option<String>,
    #[serde(default)]
    pub offset: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[request(method = "_rustcode/session/import", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ImportSessionRequest {
    /// Absent on the first chunk, which creates the upload buffer.
    #[serde(default)]
    pub upload_id: Option<String>,
    #[serde(default)]
    pub offset: u64,
    /// Base64 of up to `CHUNK_BYTES` of the exported UTF-8 JSON bundle.
    #[serde(default)]
    pub data: String,
    /// True on the final chunk: parses, writes the thread and returns its summary.
    #[serde(default)]
    pub done: bool,
    /// Explicit target folder on this machine (highest precedence when set).
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    /// Suggested target folder from project matching (used when this machine
    /// has no remembered origin folder for this thread).
    #[serde(default)]
    pub suggested_cwd: Option<PathBuf>,
    /// Preferred harness id on this machine; falls back smartly if unavailable.
    #[serde(default)]
    pub harness: Option<String>,
    /// Preferred named account id on this machine.
    #[serde(default)]
    pub account: Option<String>,
    /// Preferred model id on this machine.
    #[serde(default)]
    pub model: Option<String>,
    /// Preferred reasoning effort on this machine.
    #[serde(default)]
    pub effort: Option<String>,
    /// Stable id of the machine this thread moved from.
    #[serde(default)]
    pub from_machine_id: Option<String>,
    /// Human-readable name of the machine this thread moved from.
    #[serde(default)]
    pub from_machine_name: Option<String>,
    /// Stable id of this (destination) machine, if known to the caller.
    #[serde(default)]
    pub to_machine_id: Option<String>,
    /// Human-readable name of this (destination) machine.
    #[serde(default)]
    pub to_machine_name: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[request(method = "_rustcode/session/remove", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RemoveSessionRequest {
    pub session_id: String,
}

// ------------------------------------------------------------------- bundle

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExportedHarness {
    harness: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    account: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    native_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    handoff_from: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionTransferBundle {
    version: u32,
    session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_machine_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(default)]
    user_titled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cwd: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    status: Option<String>,
    #[serde(default)]
    pinned: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    snoozed_until: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    yolo: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    access: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model: Option<rustcode_session::SessionModel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    compaction: Option<rustcode_session::SessionCompaction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    harness: Option<ExportedHarness>,
    #[serde(default)]
    history: Vec<ChatMessage>,
    #[serde(default)]
    origins: BTreeMap<String, MachineOrigin>,
}

// ------------------------------------------------------------- chunk caches

struct CachedExport {
    session_id: String,
    bytes: Vec<u8>,
    touched: Instant,
}

struct CachedImport {
    bytes: Vec<u8>,
    touched: Instant,
}

fn export_cache() -> &'static Mutex<HashMap<String, CachedExport>> {
    static CACHE: OnceLock<Mutex<HashMap<String, CachedExport>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn import_cache() -> &'static Mutex<HashMap<String, CachedImport>> {
    static CACHE: OnceLock<Mutex<HashMap<String, CachedImport>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn next_transfer_id(prefix: &str) -> String {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{prefix}-{}-{n}", std::process::id())
}

fn prune_exports(map: &mut HashMap<String, CachedExport>, now: Instant) {
    map.retain(|_, entry| now.duration_since(entry.touched) <= CACHE_TTL);
    while map.len() >= MAX_IN_FLIGHT {
        if let Some(oldest) = map
            .iter()
            .min_by_key(|(_, entry)| entry.touched)
            .map(|(k, _)| k.clone())
        {
            map.remove(&oldest);
        } else {
            break;
        }
    }
}

fn prune_imports(map: &mut HashMap<String, CachedImport>, now: Instant) {
    map.retain(|_, entry| now.duration_since(entry.touched) <= CACHE_TTL);
    while map.len() >= MAX_IN_FLIGHT {
        if let Some(oldest) = map
            .iter()
            .min_by_key(|(_, entry)| entry.touched)
            .map(|(k, _)| k.clone())
        {
            map.remove(&oldest);
        } else {
            break;
        }
    }
}

fn invalid(message: impl Into<String>) -> agent_client_protocol::Error {
    agent_client_protocol::Error::invalid_params().data(message.into())
}

fn own_machine_id() -> Option<String> {
    let dir = crate::config::get_config_dir()?;
    let id = crate::fleet::own_id(&dir);
    (!id.is_empty()).then_some(id)
}

fn own_machine_name() -> String {
    crate::fleet::own_name_override()
        .or_else(crate::remote_gateway::gateway::os_host_name)
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "This computer".to_owned())
}

fn count_conversation_turns(history: &[ChatMessage]) -> usize {
    history
        .iter()
        .filter(|msg| matches!(msg.role.as_str(), "user" | "assistant"))
        .count()
}

// ------------------------------------------------------------------- export

/// `_rustcode/session/export`: serialize a thread once and return one base64
/// chunk at `request.offset`.
pub(crate) async fn export_session(
    external: &super::harnesses::ExternalSessions,
    request: ExportSessionRequest,
) -> Result<Value, agent_client_protocol::Error> {
    let session_id = request.session_id.trim().to_owned();
    if !super::valid_acp_session_id(&session_id) {
        return Err(invalid("invalid session id"));
    }
    if crate::thread_status::is_running(&session_id) {
        return Err(invalid(
            "stop the running turn before moving this thread to another machine",
        ));
    }

    // Continuation chunk from an existing snapshot.
    if let Some(export_id) = request
        .export_id
        .as_deref()
        .filter(|id| !id.is_empty())
        .filter(|_| request.offset > 0)
    {
        let mut cache = export_cache()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(entry) = cache.get_mut(export_id) else {
            return Err(invalid("export snapshot expired; retry the move"));
        };
        if entry.session_id != session_id {
            return Err(invalid("export snapshot does not match session"));
        }
        entry.touched = Instant::now();
        let total = entry.bytes.len();
        let start = (request.offset as usize).min(total);
        let end = (start + CHUNK_BYTES).min(total);
        let data = base64::engine::general_purpose::STANDARD.encode(&entry.bytes[start..end]);
        let done = end >= total;
        let id = export_id.to_owned();
        if done {
            cache.remove(&id);
        }
        return Ok(json!({
            "exportId": id,
            "offset": start,
            "nextOffset": end,
            "totalBytes": total,
            "data": data,
            "done": done,
        }));
    }

    // First chunk: flush pending writes and build a consistent snapshot.
    crate::sessions::flush_history();
    let live_external = super::harnesses::get(external, &session_id).await;
    let record = match &live_external {
        Some(session) => Some(session.record()),
        None => harness_store::load_for_restore(&session_id).map_err(invalid)?,
    };
    let disk_session = crate::sessions::load_session_by_id(&session_id);
    if record.is_none() && disk_session.is_none() {
        return Err(invalid(format!("unknown session: {session_id}")));
    }
    let history = match &live_external {
        Some(session) => session.recorded_history(),
        None => match disk_session.as_ref() {
            Some((_, messages)) => messages.clone(),
            None => crate::sessions::load_session_history_direct(&session_id),
        },
    };
    let workspace = crate::sessions::load_session_workspace(&session_id);
    let cwd = workspace
        .as_ref()
        .map(|w| w.cwd.clone())
        .or_else(|| record.as_ref().map(|r| r.cwd.clone()));
    let metadata = crate::sessions::load_session_metadata(&session_id);
    let title = crate::sessions::load_session_title(&session_id)
        .or_else(|| record.as_ref().and_then(|r| r.title.clone()))
        .or_else(|| {
            disk_session
                .as_ref()
                .map(|(meta, _)| meta.title.clone())
                .filter(|t| !t.trim().is_empty())
        });
    let status = crate::thread_status::current(&session_id);
    let status_str = matches!(
        status,
        crate::thread_status::Status::Review | crate::thread_status::Status::Done
    )
    .then(|| status.as_str().to_owned());
    let bundle = SessionTransferBundle {
        version: BUNDLE_VERSION,
        session_id: session_id.clone(),
        source_machine_id: own_machine_id(),
        title,
        user_titled: crate::sessions::session_user_titled(&session_id),
        cwd,
        status: status_str,
        pinned: crate::thread_status::pinned(&session_id),
        snoozed_until: crate::thread_status::snoozed_until(&session_id),
        yolo: metadata.as_ref().and_then(|m| m.yolo),
        access: crate::sessions::load_session_access(&session_id),
        model: metadata.as_ref().and_then(|m| m.model.clone()),
        compaction: metadata.as_ref().and_then(|m| m.compaction.clone()),
        harness: record.map(|r| ExportedHarness {
            harness: r.harness,
            account: r.account,
            native_session_id: r.native_session_id,
            model: r.model,
            effort: r.effort,
            handoff_from: r.handoff_from,
        }),
        history,
        origins: crate::sessions::load_session_machine_origins(&session_id),
    };
    let bytes = serde_json::to_vec(&bundle)
        .map_err(|error| invalid(format!("could not serialize thread: {error}")))?;
    if bytes.len() > MAX_BUNDLE_BYTES {
        return Err(invalid(format!(
            "this thread is too large to move ({} MiB exceeds the {} MiB limit)",
            bytes.len() / (1024 * 1024),
            MAX_BUNDLE_BYTES / (1024 * 1024)
        )));
    }
    let total = bytes.len();
    let end = CHUNK_BYTES.min(total);
    let data = base64::engine::general_purpose::STANDARD.encode(&bytes[..end]);
    let done = end >= total;
    let export_id = next_transfer_id("exp");
    if !done {
        let mut cache = export_cache()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let now = Instant::now();
        prune_exports(&mut cache, now);
        cache.insert(
            export_id.clone(),
            CachedExport {
                session_id,
                bytes,
                touched: now,
            },
        );
    }
    Ok(json!({
        "exportId": export_id,
        "offset": 0,
        "nextOffset": end,
        "totalBytes": total,
        "data": data,
        "done": done,
    }))
}

// ------------------------------------------------------------------- import

/// Outcome of `_rustcode/session/import`, used by `acp.rs` to announce the
/// imported thread and answer the client.
pub(crate) struct ImportedSession {
    pub session_id: String,
    pub cwd: PathBuf,
    pub title: Option<String>,
    pub harness: String,
    pub account: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub harness_changed: bool,
    pub resumed_native: bool,
    pub used_fallback_cwd: bool,
}

impl ImportedSession {
    pub(crate) fn to_response(&self) -> Value {
        json!({
            "sessionId": self.session_id,
            "cwd": self.cwd,
            "title": self.title,
            "harness": self.harness,
            "account": self.account,
            "model": self.model,
            "effort": self.effort,
            "harnessChanged": self.harness_changed,
            "resumedNative": self.resumed_native,
            "usedFallbackCwd": self.used_fallback_cwd,
        })
    }
}

/// `_rustcode/session/import`: accept one chunk of a thread bundle; on
/// `done: true`, validate, resolve folder/harness/origin smartly, and persist.
pub(crate) async fn import_session(
    server: &super::AcpServer,
    request: ImportSessionRequest,
) -> Result<(Value, Option<ImportedSession>), agent_client_protocol::Error> {
    let chunk = base64::engine::general_purpose::STANDARD
        .decode(request.data.as_bytes())
        .map_err(|error| invalid(format!("invalid base64 chunk: {error}")))?;
    if chunk.len() > CHUNK_BYTES + 1024 {
        return Err(invalid("chunk exceeds size limit"));
    }

    let assembled = if request.upload_id.is_none() && request.offset == 0 && request.done {
        if chunk.len() > MAX_BUNDLE_BYTES {
            return Err(invalid("thread bundle exceeds size limit"));
        }
        chunk
    } else {
        let mut cache = import_cache()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let now = Instant::now();
        let upload_id = match request.upload_id.as_deref().filter(|id| !id.is_empty()) {
            Some(id) => id.to_owned(),
            None => {
                if request.offset != 0 {
                    return Err(invalid("missing uploadId for continuation chunk"));
                }
                prune_imports(&mut cache, now);
                let id = next_transfer_id("imp");
                cache.insert(
                    id.clone(),
                    CachedImport {
                        bytes: Vec::new(),
                        touched: now,
                    },
                );
                id
            }
        };
        let Some(entry) = cache.get_mut(&upload_id) else {
            return Err(invalid("upload session expired; retry the move"));
        };
        if entry.bytes.len() as u64 != request.offset {
            return Err(invalid(format!(
                "unexpected chunk offset {} (expected {})",
                request.offset,
                entry.bytes.len()
            )));
        }
        if entry.bytes.len() + chunk.len() > MAX_BUNDLE_BYTES {
            cache.remove(&upload_id);
            return Err(invalid("thread bundle exceeds size limit"));
        }
        entry.bytes.extend_from_slice(&chunk);
        entry.touched = now;
        let received = entry.bytes.len();
        if !request.done {
            return Ok((
                json!({
                    "uploadId": upload_id,
                    "receivedBytes": received,
                    "done": false,
                }),
                None,
            ));
        }
        cache
            .remove(&upload_id)
            .map(|e| e.bytes)
            .unwrap_or_default()
    };

    let bundle: SessionTransferBundle = serde_json::from_slice(&assembled)
        .map_err(|error| invalid(format!("invalid thread bundle: {error}")))?;
    if bundle.version != BUNDLE_VERSION {
        return Err(invalid(format!(
            "unsupported thread bundle version {}",
            bundle.version
        )));
    }
    let session_id = bundle.session_id.trim().to_owned();
    if !super::valid_acp_session_id(&session_id) {
        return Err(invalid("invalid session id in bundle"));
    }
    if crate::thread_status::is_running(&session_id) {
        return Err(invalid(
            "a thread with this id is currently running on this machine",
        ));
    }

    // Close any idle in-memory copy of this session id before overwriting disk.
    if super::harnesses::close(&server.external, &session_id, &server.remote).await {
        server.clients.forget_session(&session_id);
        server.presence.session_closed(&session_id);
    }
    super::close_builtin(server, &session_id).await;
    server.presence.session_closed(&session_id);

    let imported = persist_imported_bundle(bundle, &request)?;
    let response = imported.to_response();
    Ok((response, Some(imported)))
}

fn non_empty_path(path: Option<&PathBuf>) -> Option<&Path> {
    path.map(PathBuf::as_path)
        .filter(|p| !p.as_os_str().is_empty() && p.as_os_str() != "~")
}

fn resolve_import_cwd(
    explicit: Option<&Path>,
    prior_cwd: Option<&Path>,
    suggested: Option<&Path>,
) -> Result<(PathBuf, bool), agent_client_protocol::Error> {
    if let Some(path) = explicit {
        return Ok((super::canonical_acp_cwd(path, "session/import")?, false));
    }
    if let Some(path) = prior_cwd
        && let Ok(canonical) = super::canonical_acp_cwd(path, "session/import")
    {
        return Ok((canonical, false));
    }
    if let Some(path) = suggested
        && let Ok(canonical) = super::canonical_acp_cwd(path, "session/import")
    {
        return Ok((canonical, false));
    }
    let fallback = super::session_new_cwd(Path::new(""))?;
    Ok((fallback, true))
}

fn persist_imported_bundle(
    bundle: SessionTransferBundle,
    request: &ImportSessionRequest,
) -> Result<ImportedSession, agent_client_protocol::Error> {
    let session_id = bundle.session_id;
    let from_harness = bundle
        .harness
        .as_ref()
        .map(|h| h.harness.clone())
        .unwrap_or_else(|| harness::BUILTIN_ID.to_owned());
    let from_account = bundle.harness.as_ref().and_then(|h| h.account.clone());
    let from_model = bundle
        .harness
        .as_ref()
        .and_then(|h| h.model.clone())
        .or_else(|| bundle.model.as_ref().map(|m| m.profile.clone()));
    let from_effort = bundle
        .harness
        .as_ref()
        .and_then(|h| h.effort.clone())
        .or_else(|| {
            bundle
                .model
                .as_ref()
                .and_then(|m| m.reasoning_effort.clone())
        });
    let turn_count = count_conversation_turns(&bundle.history);

    // Record where the thread just came from so moving back restores its exact
    // folder, login, and (if untouched) native harness session.
    let mut origins = bundle.origins;
    let from_id = request
        .from_machine_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .or_else(|| {
            bundle
                .source_machine_id
                .as_deref()
                .filter(|id| !id.is_empty())
        })
        .map(str::to_owned);
    if let Some(key) = from_id {
        origins.insert(
            key,
            MachineOrigin {
                cwd: bundle.cwd.clone(),
                harness: Some(from_harness.clone()),
                account: from_account.clone(),
                model: from_model.clone(),
                effort: from_effort.clone(),
                native_session_id: bundle
                    .harness
                    .as_ref()
                    .and_then(|h| h.native_session_id.clone()),
                turn_count,
            },
        );
    }

    // Look up whether this thread previously ran on THIS machine.
    let to_id = request
        .to_machine_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .or_else(own_machine_id);
    let prior = to_id.as_deref().and_then(|id| origins.get(id)).cloned();

    let (cwd, used_fallback_cwd) = resolve_import_cwd(
        non_empty_path(request.cwd.as_ref()),
        prior.as_ref().and_then(|origin| origin.cwd.as_deref()),
        non_empty_path(request.suggested_cwd.as_ref()),
    )?;

    // Resolve harness, account, model & effort on this machine.
    let dir = harness::config::config_dir().map_err(invalid)?;
    let (defaults, entries) = harness::config::load_in(&dir);
    let accounts = harness::config::load_accounts_in(&dir);

    let desired_harness = request
        .harness
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .unwrap_or(&from_harness);

    let is_usable_external = |id: &str| {
        id != harness::BUILTIN_ID
            && harness::resolve(&entries, id).is_some_and(|resolved| resolved.enabled)
    };

    let chosen_harness = if desired_harness == harness::BUILTIN_ID {
        harness::BUILTIN_ID.to_owned()
    } else if is_usable_external(desired_harness) {
        desired_harness.to_owned()
    } else if let Some(prev) = prior
        .as_ref()
        .and_then(|o| o.harness.as_deref())
        .filter(|id| *id == harness::BUILTIN_ID || is_usable_external(id))
    {
        prev.to_owned()
    } else if let Some(def) = defaults
        .harness
        .as_deref()
        .filter(|id| *id == harness::BUILTIN_ID || is_usable_external(id))
    {
        def.to_owned()
    } else {
        harness::BUILTIN_ID.to_owned()
    };

    let from_machine_name = request
        .from_machine_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or("another machine")
        .to_owned();
    let to_machine_name = request
        .to_machine_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(own_machine_name);

    let harness_changed = chosen_harness != from_harness;
    let mut history = bundle.history;

    let (chosen_account_id, chosen_account_name, chosen_model, chosen_effort, resumed_native) =
        if chosen_harness == harness::BUILTIN_ID {
            harness_store::remove(&session_id);
            let builtin_model = request
                .model
                .clone()
                .or_else(|| from_model.clone())
                .or_else(|| prior.as_ref().and_then(|o| o.model.clone()));
            let builtin_effort = request
                .effort
                .clone()
                .or_else(|| from_effort.clone())
                .or_else(|| prior.as_ref().and_then(|o| o.effort.clone()));
            if let Some(profile) = builtin_model.clone() {
                crate::sessions::save_session_model(
                    &session_id,
                    rustcode_session::SessionModel {
                        profile,
                        reasoning_effort: builtin_effort.clone(),
                        fast: bundle.model.as_ref().is_some_and(|m| m.fast),
                    },
                );
            }
            (None, None, builtin_model, builtin_effort, false)
        } else {
            // Find a valid named account on this machine: explicit request ->
            // same account id if harness unchanged -> remembered account on
            // this machine if same harness -> default login (`None`).
            let candidate_accounts: [Option<&str>; 3] = [
                request
                    .account
                    .as_deref()
                    .map(str::trim)
                    .filter(|a| !a.is_empty()),
                (!harness_changed)
                    .then_some(from_account.as_deref())
                    .flatten(),
                prior
                    .as_ref()
                    .filter(|o| o.harness.as_deref() == Some(chosen_harness.as_str()))
                    .and_then(|o| o.account.as_deref()),
            ];
            let account = candidate_accounts
                .into_iter()
                .flatten()
                .find_map(|id| harness::config::find_account(&accounts, &chosen_harness, id).ok());
            let account_id = account.as_ref().map(|a| a.id.clone());
            let account_name = account.as_ref().map(|a| a.display_name().to_owned());

            let asked = harness::HarnessRequest {
                harness: Some(chosen_harness.clone()),
                account: account_id.clone(),
                model: request
                    .model
                    .clone()
                    .or_else(|| (!harness_changed).then(|| from_model.clone()).flatten())
                    .or_else(|| {
                        prior
                            .as_ref()
                            .filter(|o| o.harness.as_deref() == Some(chosen_harness.as_str()))
                            .and_then(|o| o.model.clone())
                    }),
                effort: request
                    .effort
                    .clone()
                    .or_else(|| (!harness_changed).then(|| from_effort.clone()).flatten())
                    .or_else(|| {
                        prior
                            .as_ref()
                            .filter(|o| o.harness.as_deref() == Some(chosen_harness.as_str()))
                            .and_then(|o| o.effort.clone())
                    }),
            };
            let (model, effort) = harness::starting_choice(
                &chosen_harness,
                &asked,
                &defaults,
                &entries,
                &harness::learned_for(&chosen_harness)
                    .models
                    .unwrap_or_default(),
            );

            // If returning to a machine where this thread's native session is
            // still at the exact same turn count (no turns ran elsewhere), we
            // can resume that native session directly without re-injecting a
            // `<context>` preamble.
            let prior_native = prior.as_ref().and_then(|origin| {
                let same_harness = origin.harness.as_deref() == Some(chosen_harness.as_str());
                let same_account = origin.account == account_id;
                let same_cwd = origin
                    .cwd
                    .as_deref()
                    .is_some_and(|p| super::opens_in(p, &cwd));
                let same_turns = origin.turn_count == turn_count;
                let no_pending_handoff = bundle
                    .harness
                    .as_ref()
                    .and_then(|h| h.handoff_from.as_ref())
                    .is_none();
                (same_harness && same_account && same_cwd && same_turns && no_pending_handoff)
                    .then(|| origin.native_session_id.clone())
                    .flatten()
            });
            let resumed_native = prior_native.is_some();

            let mut record = harness_store::HarnessRecord::new(&chosen_harness, cwd.clone());
            record.account = account_id.clone();
            record.native_session_id = prior_native;
            record.title = bundle.title.clone();
            record.model = model.clone();
            record.effort = effort.clone();
            record.usage = None;
            if !resumed_native && turn_count > 0 {
                let origin_label = format!(
                    "{from_machine_name} · {}",
                    harness::handoff::label(&from_harness, from_account.as_deref())
                );
                // Keep any earlier un-flushed handoff origin if the thread was
                // moved twice before its next prompt.
                record.handoff_from = bundle
                    .harness
                    .as_ref()
                    .and_then(|h| h.handoff_from.clone())
                    .or(Some(origin_label));
            }
            harness_store::save(&session_id, &record);
            (account_id, account_name, model, effort, resumed_native)
        };

    // Append a quiet divider so the transcript records the move (and any
    // harness fallback) across reloads.
    history.push(harness::handoff::marker_machine(
        &from_harness,
        from_account.as_deref(),
        &chosen_harness,
        chosen_account_name.as_deref(),
        &from_machine_name,
        &to_machine_name,
        chosen_model.as_deref(),
        resumed_native,
    ));

    // Persist history, title, workspace, repo, metadata, status & origins durably.
    crate::sessions::save_session_history(&session_id, &history);
    crate::sessions::flush_history();
    if let Some(title) = bundle.title.as_deref().filter(|t| !t.trim().is_empty()) {
        if bundle.user_titled {
            crate::sessions::save_user_session_title(&session_id, title);
        } else {
            crate::sessions::save_session_title(&session_id, title);
        }
    }
    crate::sessions::save_session_workspace(
        &session_id,
        &super::session_workspace(cwd.clone(), vec![]),
    )
    .map_err(|error| invalid(format!("could not save thread workspace: {error}")))?;
    let repo = crate::worktrees::repo_of(&cwd);
    crate::sessions::save_session_repo(&session_id, repo.as_deref());
    if let Some(yolo) = bundle.yolo {
        crate::sessions::save_session_yolo(&session_id, yolo);
    }
    if let Some(access) = bundle.access.as_deref() {
        crate::sessions::save_session_access(&session_id, access);
    }
    if let Some(compaction) = bundle.compaction {
        crate::sessions::save_session_compaction(&session_id, compaction);
    }
    crate::sessions::save_session_machine_origins(&session_id, origins);
    crate::thread_status::restore_state(
        &session_id,
        bundle.status.as_deref(),
        bundle.pinned,
        bundle.snoozed_until,
    );

    Ok(ImportedSession {
        session_id,
        cwd,
        title: bundle.title,
        harness: chosen_harness,
        account: chosen_account_id,
        model: chosen_model,
        effort: chosen_effort,
        harness_changed,
        resumed_native,
        used_fallback_cwd,
    })
}

// ------------------------------------------------------------------- remove

/// `_rustcode/session/remove`: close and delete a moved thread from its source
/// machine after the destination has confirmed import.
pub(crate) async fn remove_session(
    server: &super::AcpServer,
    request: RemoveSessionRequest,
) -> Result<Value, agent_client_protocol::Error> {
    let session_id = request.session_id.trim().to_owned();
    if !super::valid_acp_session_id(&session_id) {
        return Err(invalid("invalid session id"));
    }
    if crate::thread_status::is_running(&session_id) {
        return Err(invalid("stop the running turn before removing this thread"));
    }
    if super::harnesses::close(&server.external, &session_id, &server.remote).await {
        server.clients.forget_session(&session_id);
        server.presence.session_closed(&session_id);
    }
    super::close_builtin(server, &session_id).await;
    server.presence.session_closed(&session_id);
    let removed = crate::sessions::delete_session_by_id(&session_id);
    server.announce_thread_deleted(super::ThreadDeletedNotification {
        session_id: session_id.clone(),
    });
    Ok(json!({ "sessionId": session_id, "removed": removed }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::JsonRpcMessage;

    #[test]
    fn request_methods_match_wire_names() {
        assert!(ExportSessionRequest::matches_method(
            "_rustcode/session/export"
        ));
        assert!(ImportSessionRequest::matches_method(
            "_rustcode/session/import"
        ));
        assert!(RemoveSessionRequest::matches_method(
            "_rustcode/session/remove"
        ));
    }

    #[tokio::test]
    async fn round_trip_export_and_import_preserves_history_and_bounce_back_origin() {
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        let session_id = crate::sessions::create_detached_session();
        let history = vec![
            ChatMessage::new("user", "Check the latency spikes"),
            ChatMessage::new("assistant", "Looking at the metrics now."),
        ];
        crate::sessions::save_session_history(&session_id, &history);
        crate::sessions::flush_history();
        crate::sessions::save_user_session_title(&session_id, "Latency audit");
        crate::sessions::save_session_workspace(
            &session_id,
            &rustcode_session::SessionWorkspace {
                cwd: dir_a.path().to_path_buf(),
                additional_directories: vec![],
                task_workspace_id: None,
            },
        )
        .unwrap();
        let mut record =
            harness_store::HarnessRecord::new("claude-code", dir_a.path().to_path_buf());
        record.native_session_id = Some("native-mac-1".to_owned());
        record.model = Some("claude-sonnet-5".to_owned());
        harness_store::save(&session_id, &record);

        let external: super::super::harnesses::ExternalSessions = Default::default();
        // Export from machine A.
        let exported = export_session(
            &external,
            ExportSessionRequest {
                session_id: session_id.clone(),
                export_id: None,
                offset: 0,
            },
        )
        .await
        .unwrap();
        assert_eq!(exported["done"], json!(true));
        let b64 = exported["data"].as_str().unwrap().to_owned();

        // Simulate moving to machine B (where claude-code might be configured or
        // fall back; pass suggested_cwd = dir_b).
        let raw = base64::engine::general_purpose::STANDARD
            .decode(b64.as_bytes())
            .unwrap();
        let bundle: SessionTransferBundle = serde_json::from_slice(&raw).unwrap();
        assert_eq!(bundle.session_id, session_id);
        assert_eq!(bundle.title.as_deref(), Some("Latency audit"));
        assert!(bundle.user_titled);
        assert_eq!(bundle.history.len(), 2);

        let imported_b = persist_imported_bundle(
            bundle,
            &ImportSessionRequest {
                suggested_cwd: Some(dir_b.path().to_path_buf()),
                from_machine_id: Some("gw-mac".to_owned()),
                from_machine_name: Some("MacBook".to_owned()),
                to_machine_id: Some("gw-vps".to_owned()),
                to_machine_name: Some("VPS".to_owned()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            imported_b.cwd,
            rustcode_session::strip_verbatim(&std::fs::canonicalize(dir_b.path()).unwrap())
        );
        assert!(!imported_b.resumed_native);

        // Moving back to machine A without adding turns restores dir_a!
        let exported_back = export_session(
            &external,
            ExportSessionRequest {
                session_id: session_id.clone(),
                export_id: None,
                offset: 0,
            },
        )
        .await
        .unwrap();
        let raw_back = base64::engine::general_purpose::STANDARD
            .decode(exported_back["data"].as_str().unwrap().as_bytes())
            .unwrap();
        let bundle_back: SessionTransferBundle = serde_json::from_slice(&raw_back).unwrap();
        assert!(bundle_back.origins.contains_key("gw-mac"));
        assert!(
            bundle_back
                .history
                .iter()
                .any(|m| m.role == harness::handoff::ROLE)
        );

        let imported_a = persist_imported_bundle(
            bundle_back,
            &ImportSessionRequest {
                // Even without passing suggested_cwd, the remembered origin for
                // gw-mac restores dir_a!
                from_machine_id: Some("gw-vps".to_owned()),
                from_machine_name: Some("VPS".to_owned()),
                to_machine_id: Some("gw-mac".to_owned()),
                to_machine_name: Some("MacBook".to_owned()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            imported_a.cwd,
            rustcode_session::strip_verbatim(&std::fs::canonicalize(dir_a.path()).unwrap())
        );

        assert!(crate::sessions::delete_session_by_id(&session_id));
        assert!(crate::sessions::load_session_by_id(&session_id).is_none());
    }

    #[tokio::test]
    async fn multi_chunk_export_reassembles_identically() {
        let dir = tempfile::tempdir().unwrap();
        let session_id = crate::sessions::create_detached_session();
        // Exceeds CHUNK_BYTES (384 KiB) so export spans multiple chunks.
        let big = "a".repeat(CHUNK_BYTES + 40_000);
        let history = vec![
            ChatMessage::new("user", big.clone()),
            ChatMessage::new("assistant", "Got it."),
        ];
        crate::sessions::save_session_history(&session_id, &history);
        crate::sessions::flush_history();
        crate::sessions::save_session_workspace(
            &session_id,
            &rustcode_session::SessionWorkspace {
                cwd: dir.path().to_path_buf(),
                additional_directories: vec![],
                task_workspace_id: None,
            },
        )
        .unwrap();

        let external: super::super::harnesses::ExternalSessions = Default::default();
        let mut offset = 0u64;
        let mut export_id: Option<String> = None;
        let mut chunks = 0usize;
        let mut assembled = Vec::new();
        loop {
            let page = export_session(
                &external,
                ExportSessionRequest {
                    session_id: session_id.clone(),
                    export_id: export_id.clone(),
                    offset,
                },
            )
            .await
            .unwrap();
            export_id = page["exportId"].as_str().map(str::to_owned);
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(page["data"].as_str().unwrap().as_bytes())
                .unwrap();
            assembled.extend_from_slice(&bytes);
            chunks += 1;
            if page["done"].as_bool() == Some(true) {
                break;
            }
            offset = page["nextOffset"].as_u64().unwrap();
        }
        assert!(chunks >= 2, "expected multiple chunks, got {chunks}");
        let bundle: SessionTransferBundle = serde_json::from_slice(&assembled).unwrap();
        assert_eq!(bundle.history[0].content, big);
        assert!(crate::sessions::delete_session_by_id(&session_id));
    }
}
