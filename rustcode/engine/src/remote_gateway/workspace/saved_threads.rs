//! What a device's thread list shows: each saved thread as the desktop app's
//! sidebar shows it, the projects those threads belong to, and the watcher
//! that pushes their changes to subscribed devices
//! (`docs/remote-protocol/README.md`, *Saved sessions*).
//!
//! Everything is read from the session store under the gateway's config
//! directory and from the live list the gateway holds. Unread marks, the
//! sidebar's manual order and projects the app knows before their first
//! thread live in the app's own storage, so no device sees them.

use super::*;
use rustcode_session::{SessionMeta, SessionMetadata, SessionStore};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::SystemTime;

/// How often the watcher looks for changes while a device is subscribed.
const WATCH_INTERVAL: Duration = Duration::from_secs(5);

/// The part of a frame a page or a change frame may fill, leaving room for
/// the envelope and the wire-name translation.
const FRAME_BUDGET: usize = MAX_REMOTE_FRAME_BYTES / 2;

/// What the gateway knows of a thread from processes, not files.
#[derive(Debug, Clone, Copy)]
pub(super) struct Live {
    pub state: SavedSessionState,
    /// The shared thread's activity, when it is in the live list.
    pub activity: Option<SessionActivity>,
}

/// Whether `session_id` is shared, held by a process or only saved, and what
/// a shared one is doing.
pub(super) fn live_state(hub: &SessionHub, config: &Path, session_id: &str) -> Live {
    if let Some(info) = hub.session_info(session_id) {
        return Live {
            state: SavedSessionState::Shared,
            activity: Some(info.activity),
        };
    }
    let open = hub.is_announced(session_id)
        || owner_alive(&config.join("remote").join("owners"), session_id)
        || crate::sessions::session_open_by(config, session_id).is_some();
    Live {
        state: if open {
            SavedSessionState::Open
        } else {
            SavedSessionState::Saved
        },
        activity: None,
    }
}

fn read_json(path: &Path) -> Option<serde_json::Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

fn rfc3339(time: chrono::DateTime<chrono::Utc>) -> String {
    time.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// The status the desktop app shows: a settled thread stays `done` while its
/// last turn winds down, a live turn comes next (from the share, else from the
/// turn locks of whichever process runs it), then what the store holds.
fn status_of(directory: &Path, activity: Option<SessionActivity>) -> &'static str {
    let stored = read_json(&directory.join("status.json"))
        .and_then(|value| value.get("status")?.as_str().map(str::to_owned));
    if stored.as_deref() == Some("done") {
        return "done";
    }
    match activity {
        Some(SessionActivity::Running) => "running",
        Some(SessionActivity::AwaitingApproval | SessionActivity::AwaitingQuestion) => {
            "needs_input"
        }
        Some(SessionActivity::Idle) => stored_status(stored.as_deref()),
        None => match crate::sessions::activity::activity_in(directory) {
            (_, true) => "needs_input",
            (true, false) => "running",
            (false, false) => stored_status(stored.as_deref()),
        },
    }
}

fn stored_status(stored: Option<&str>) -> &'static str {
    match stored {
        Some("review") => "review",
        Some("stalled") => "stalled",
        _ => "idle",
    }
}

fn snoozed_until(directory: &Path) -> Option<String> {
    let until = read_json(&directory.join("snooze.json"))?.as_u64()?;
    let wakes = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(i64::try_from(until).ok()?)?;
    (wakes > chrono::Utc::now()).then(|| rfc3339(wakes))
}

/// A folder the desktop app never lists as a project: a drive's root, or
/// RustCode's own data folder, where the store and worktrees live.
fn not_a_project(config: &Path, path: &Path) -> bool {
    path.parent().is_none() || path.starts_with(config)
}

/// The project the desktop app files a thread under: a worktree's repository,
/// else the repository recorded for its folder, else the folder itself. None
/// for a thread whose folder was never recorded.
fn thread_project(
    config: &Path,
    directory: &Path,
    workspace: Option<&Path>,
    metadata: Option<&SessionMetadata>,
) -> Option<String> {
    let workspace = workspace?;
    let worktree_repo = read_json(&directory.join("worktree.json"))
        .and_then(|value| value.get("repo")?.as_str().map(PathBuf::from));
    let path = worktree_repo
        .or_else(|| metadata.and_then(|metadata| metadata.repo.clone()))
        .unwrap_or_else(|| workspace.to_path_buf());
    let path = rustcode_session::strip_verbatim(&path);
    (!not_a_project(config, &path)).then(|| path.to_string_lossy().into_owned())
}

/// One saved thread as `list_saved_sessions` reports it.
pub(super) fn entry(
    config: &Path,
    store: &SessionStore,
    meta: &SessionMeta,
    session_id: String,
    live: Live,
) -> RemoteSavedSession {
    let directory = meta
        .path
        .parent()
        .map_or_else(|| store.session_dir(&session_id), Path::to_path_buf);
    let updated_at = std::fs::metadata(&meta.path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .map(|modified| rfc3339(modified.into()));
    let open_in = (live.state == SavedSessionState::Open)
        .then(|| crate::sessions::session_open_by(config, &session_id))
        .flatten()
        .flatten()
        .map(saved_session_holder);
    let workspace = meta
        .workspace_cwd
        .as_ref()
        .map(|cwd| rustcode_session::strip_verbatim(cwd));
    let metadata = store.load_session_metadata(&session_id);
    let harness = read_json(&directory.join("harness.json"));
    let harness_field = |field: &str| {
        harness
            .as_ref()
            .and_then(|record| record.get(field)?.as_str())
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let parent = metadata
        .as_ref()
        .and_then(|metadata| metadata.parent.as_ref())
        .and_then(|parent| {
            Some(RemoteThreadParent {
                session_id: parent.id.clone()?,
                relationship: parent.relationship().as_str().to_owned(),
            })
        });
    RemoteSavedSession {
        state: live.state,
        open_in,
        parent,
        title: meta
            .title
            .chars()
            .take(MAX_SAVED_SESSION_TITLE_CHARS)
            .collect(),
        project: thread_project(config, &directory, workspace.as_deref(), metadata.as_ref()),
        workspace: workspace.map(|cwd| cwd.to_string_lossy().into_owned()),
        updated_at,
        message_count: meta.message_count as u64,
        status: Some(status_of(&directory, live.activity).to_owned()),
        pinned: directory.join("pin.json").is_file(),
        snoozed_until: snoozed_until(&directory),
        harness: Some(
            harness_field("harness").unwrap_or_else(|| crate::harness::BUILTIN_ID.to_owned()),
        ),
        model: harness_field("model").or_else(|| {
            metadata
                .as_ref()
                .and_then(|metadata| metadata.model.as_ref())
                .map(|model| model.profile.clone())
                .filter(|profile| !profile.is_empty())
        }),
        scheduled_job: metadata.and_then(|metadata| metadata.scheduled_job),
        session_id,
    }
}

/// The projects `entries` belong to, most recently updated first.
pub(super) fn projects(entries: &[RemoteSavedSession]) -> Vec<RemoteProject> {
    let mut by_path: HashMap<&str, RemoteProject> = HashMap::new();
    for entry in entries {
        let Some(path) = entry.project.as_deref() else {
            continue;
        };
        let project = by_path.entry(path).or_insert_with(|| RemoteProject {
            path: path.to_owned(),
            name: Path::new(path).file_name().map_or_else(
                || path.to_owned(),
                |name| name.to_string_lossy().into_owned(),
            ),
            thread_count: 0,
            running: false,
            needs_input: false,
            updated_at: None,
        });
        project.thread_count += 1;
        let status = entry.status.as_deref();
        project.needs_input |= status == Some("needs_input");
        project.running |= matches!(status, Some("running" | "needs_input"));
        if entry.updated_at > project.updated_at {
            project.updated_at.clone_from(&entry.updated_at);
        }
    }
    let mut projects: Vec<RemoteProject> = by_path.into_values().collect();
    projects.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| left.path.cmp(&right.path))
    });
    projects
}

/// The answer to `list_projects`, bounded to [`MAX_PROJECTS`] and one frame.
pub(super) fn list_projects(entries: &[RemoteSavedSession]) -> Result<RemoteResult> {
    let mut all = projects(entries);
    let mut truncated = all.len() > MAX_PROJECTS;
    all.truncate(MAX_PROJECTS);
    let mut bytes = 0;
    let mut kept = 0;
    for project in &all {
        bytes += serde_json::to_vec(project)?.len() + 1;
        if bytes > FRAME_BUDGET {
            truncated = true;
            break;
        }
        kept += 1;
    }
    all.truncate(kept);
    Ok(RemoteResult::Projects {
        projects: all,
        truncated,
    })
}

type Stamp = [Option<(SystemTime, u64)>; 3];

fn stamp_of(path: &Path) -> Option<(SystemTime, u64)> {
    let metadata = std::fs::metadata(path).ok()?;
    Some((metadata.modified().ok()?, metadata.len()))
}

/// Every saved thread's entry, with each transcript read again only when it,
/// its title or its workspace record changed since the last scan.
#[derive(Default)]
pub(super) struct SavedIndex {
    metas: HashMap<PathBuf, (Stamp, Option<SessionMeta>)>,
}

impl SavedIndex {
    pub(super) fn scan(
        &mut self,
        config: &Path,
        live: &dyn Fn(&str) -> Live,
    ) -> Vec<RemoteSavedSession> {
        let store = SessionStore::new(config);
        let mut seen = HashSet::new();
        let mut kept = HashSet::new();
        let mut entries = Vec::new();
        for path in store.session_history_paths() {
            let Some(id) = SessionStore::session_id_from_path(&path) else {
                continue;
            };
            if !rustcode_session::valid_session_id(&id)
                || seen.contains(&id)
                || !crate::sessions::session_file_in_store(config, &path)
            {
                continue;
            }
            let Some(directory) = path.parent() else {
                continue;
            };
            let stamp = [
                stamp_of(&path),
                stamp_of(&directory.join("title.txt")),
                stamp_of(&directory.join(rustcode_session::SESSION_WORKSPACE_FILE)),
            ];
            let cached = self.metas.get(&path).filter(|(known, _)| *known == stamp);
            let meta = match cached {
                Some((_, meta)) => meta.clone(),
                None => {
                    let meta = store.load_session_meta(&path);
                    self.metas.insert(path.clone(), (stamp, meta.clone()));
                    meta
                }
            };
            kept.insert(path);
            if let Some(meta) = meta {
                seen.insert(id.clone());
                let state = live(&id);
                entries.push(entry(config, &store, &meta, id, state));
            }
        }
        self.metas.retain(|path, _| kept.contains(path));
        entries
    }
}

/// The state change frames are measured against.
#[derive(Default)]
struct Baseline {
    sessions: HashMap<String, RemoteSavedSession>,
    projects: HashMap<String, RemoteProject>,
}

impl Baseline {
    fn of(entries: Vec<RemoteSavedSession>) -> Self {
        let projects = projects(&entries)
            .into_iter()
            .map(|project| (project.path.clone(), project))
            .collect();
        let sessions = entries
            .into_iter()
            .map(|entry| (entry.session_id.clone(), entry))
            .collect();
        Self { sessions, projects }
    }
}

enum Change {
    Session(RemoteSavedSession),
    Removed(String),
    Project(RemoteProject),
    RemovedProject(String),
}

fn empty_change_frame() -> RemoteSavedSessionsChanged {
    RemoteSavedSessionsChanged {
        protocol_version: REMOTE_PROTOCOL_VERSION,
        sessions: Vec::new(),
        removed: Vec::new(),
        projects: Vec::new(),
        removed_projects: Vec::new(),
    }
}

/// The `saved_sessions_changed` frames that take a client from `before` to
/// `after`, each within [`FRAME_BUDGET`]; none when nothing changed.
fn change_frames(before: &Baseline, after: &Baseline) -> Vec<RemoteSavedSessionsChanged> {
    let mut changes: Vec<Change> = Vec::new();
    let mut sessions: Vec<&RemoteSavedSession> = after
        .sessions
        .values()
        .filter(|entry| before.sessions.get(&entry.session_id) != Some(*entry))
        .collect();
    sessions.sort_by(|left, right| right.session_id.cmp(&left.session_id));
    changes.extend(sessions.into_iter().cloned().map(Change::Session));
    let mut removed: Vec<&String> = before
        .sessions
        .keys()
        .filter(|id| !after.sessions.contains_key(*id))
        .collect();
    removed.sort();
    changes.extend(removed.into_iter().cloned().map(Change::Removed));
    let mut projects: Vec<&RemoteProject> = after
        .projects
        .values()
        .filter(|project| before.projects.get(&project.path) != Some(*project))
        .collect();
    projects.sort_by(|left, right| left.path.cmp(&right.path));
    changes.extend(projects.into_iter().cloned().map(Change::Project));
    let mut gone: Vec<&String> = before
        .projects
        .keys()
        .filter(|path| !after.projects.contains_key(*path))
        .collect();
    gone.sort();
    changes.extend(gone.into_iter().cloned().map(Change::RemovedProject));

    let mut frames = Vec::new();
    let mut frame = empty_change_frame();
    let mut bytes = 0;
    for change in changes {
        let size = match &change {
            Change::Session(entry) => serde_json::to_vec(entry).map_or(0, |bytes| bytes.len()),
            Change::Project(project) => serde_json::to_vec(project).map_or(0, |bytes| bytes.len()),
            Change::Removed(text) | Change::RemovedProject(text) => text.len() + 3,
        } + 1;
        if bytes > 0 && bytes + size > FRAME_BUDGET {
            frames.push(std::mem::replace(&mut frame, empty_change_frame()));
            bytes = 0;
        }
        bytes += size;
        match change {
            Change::Session(entry) => frame.sessions.push(entry),
            Change::Removed(id) => frame.removed.push(id),
            Change::Project(project) => frame.projects.push(project),
            Change::RemovedProject(path) => frame.removed_projects.push(path),
        }
    }
    if bytes > 0 {
        frames.push(frame);
    }
    frames
}

#[derive(Default)]
struct Subscribers {
    active: HashMap<u64, FrameSink>,
    /// Subscribed, waiting for a scan read after they asked.
    pending: Vec<(u64, FrameSink, String)>,
    running: bool,
}

/// Pushes `saved_sessions_changed` to every subscribed connection. One task
/// scans while any device is subscribed and stops when the last one leaves.
#[derive(Default)]
pub(super) struct SavedWatch {
    subscribers: Mutex<Subscribers>,
    wake: tokio::sync::Notify,
}

type Scan = Arc<dyn Fn() -> Vec<RemoteSavedSession> + Send + Sync>;

impl SavedWatch {
    fn subscribers(&self) -> std::sync::MutexGuard<'_, Subscribers> {
        self.subscribers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    /// Subscribe a connection. It is answered with `saved_sessions_subscribed`
    /// after the next scan, which is started now.
    pub(super) fn subscribe(
        self: &Arc<Self>,
        connection: u64,
        sink: &FrameSink,
        request_id: String,
        scan: Scan,
    ) {
        let start = {
            let mut subscribers = self.subscribers();
            subscribers.active.remove(&connection);
            subscribers.pending.retain(|(id, _, _)| *id != connection);
            subscribers
                .pending
                .push((connection, sink.clone(), request_id));
            !std::mem::replace(&mut subscribers.running, true)
        };
        if start {
            tokio::spawn(self.clone().run(scan));
        }
        self.wake.notify_one();
    }

    pub(super) fn unsubscribe(&self, connection: u64) {
        let mut subscribers = self.subscribers();
        subscribers.active.remove(&connection);
        subscribers.pending.retain(|(id, _, _)| *id != connection);
    }

    async fn run(self: Arc<Self>, scan: Scan) {
        let mut baseline: Option<Baseline> = None;
        loop {
            {
                let mut subscribers = self.subscribers();
                if subscribers.active.is_empty() && subscribers.pending.is_empty() {
                    subscribers.running = false;
                    return;
                }
            }
            let scan = scan.clone();
            if let Ok(entries) = tokio::task::spawn_blocking(move || scan()).await {
                let current = Baseline::of(entries);
                let frames: Vec<String> = baseline
                    .as_ref()
                    .map(|before| change_frames(before, &current))
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|frame| {
                        serde_json::to_string(&RemoteFrame::SavedSessionsChanged(frame)).ok()
                    })
                    .collect();
                let mut subscribers = self.subscribers();
                subscribers
                    .active
                    .retain(|_, sink| frames.iter().all(|frame| sink.send(frame.clone()).is_ok()));
                for (connection, sink, request_id) in std::mem::take(&mut subscribers.pending) {
                    let answer =
                        RemoteResponse::new(request_id, RemoteResult::SavedSessionsSubscribed);
                    send(&sink, answer);
                    subscribers.active.insert(connection, sink);
                }
                baseline = Some(current);
            }
            tokio::select! {
                () = tokio::time::sleep(WATCH_INTERVAL) => {}
                () = self.wake.notified() => {}
            }
        }
    }
}

/// The folder a device asked to start a thread in: one inside the allowed
/// roots, as before, or exactly the folder of a project the store's threads
/// belong to, which `list_projects` reports wherever it lies.
pub(super) fn resolve_creation_directory(config: &Path, path: Option<&str>) -> Result<PathBuf> {
    let refusal = match resolve_directory(path, &allowed_roots(config)?) {
        Ok(directory) => return Ok(directory),
        Err(error) => error,
    };
    let Some(named) = path
        .map(str::trim)
        .filter(|path| Path::new(path).is_absolute())
        .and_then(|path| Path::new(path).canonicalize().ok())
        .filter(|path| path.is_dir())
    else {
        return Err(refusal);
    };
    let store = SessionStore::new(config);
    let mut checked = HashSet::new();
    for history in store.session_history_paths() {
        let (Some(id), Some(directory)) = (
            SessionStore::session_id_from_path(&history),
            history.parent(),
        ) else {
            continue;
        };
        if !crate::sessions::session_file_in_store(config, &history) {
            continue;
        }
        let workspace = store.load_session_workspace(&id).map(|record| record.cwd);
        let metadata = store.load_session_metadata(&id);
        let Some(project) =
            thread_project(config, directory, workspace.as_deref(), metadata.as_ref())
        else {
            continue;
        };
        if checked.insert(project.clone())
            && Path::new(&project).canonicalize().ok().as_deref() == Some(named.as_path())
        {
            return Ok(named);
        }
    }
    Err(refusal)
}

#[cfg(test)]
#[path = "saved_threads_tests.rs"]
mod tests;
