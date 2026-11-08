//! Thread status (`docs/orchestrator.md`, *Thread status*):
//! `idle | running | needs_input | review | done`.
//!
//! `running` and `needs_input` are derived from what the engine is doing now
//! and never stored: a [`TurnGuard`] marks a turn, a [`WaitGuard`] an
//! approval or question the turn waits on. `review` and `done` are set with
//! `_rustcode/session/status` (or `done` by a merged pull request) and stored
//! in the thread's directory as `status.json`. Each change is told to every
//! connected client as `session/update` `session_info_update` carrying
//! `_meta.rustcode.status`, whether or not it has the thread open: thread
//! lists show it.
//!
//! Pins are stored the same way (`pin.json`), set with
//! `_rustcode/session/pin` and carried as `_meta.rustcode.pinned`, so the
//! terminal UI and the apps agree on them. Archiving a thread is `done`.

use agent_client_protocol::schema::v1::{SessionInfoUpdate, SessionUpdate};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

const STATUS_FILE: &str = "status.json";
const PIN_FILE: &str = "pin.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Status {
    Idle,
    Running,
    NeedsInput,
    Review,
    Done,
}

impl Status {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Running => "running",
            Self::NeedsInput => "needs_input",
            Self::Review => "review",
            Self::Done => "done",
        }
    }

    /// A status the user (or the engine) may store.
    fn storable(text: &str) -> Option<Self> {
        match text {
            "idle" => Some(Self::Idle),
            "review" => Some(Self::Review),
            "done" => Some(Self::Done),
            _ => None,
        }
    }
}

#[derive(Default, Clone, Copy)]
struct Live {
    turns: u32,
    waits: u32,
}

type Notifier = Box<dyn Fn(String, SessionUpdate) + Send + Sync>;

fn live() -> &'static Mutex<HashMap<String, Live>> {
    static LIVE: OnceLock<Mutex<HashMap<String, Live>>> = OnceLock::new();
    LIVE.get_or_init(Default::default)
}

fn notifier() -> &'static Mutex<Option<Notifier>> {
    static NOTIFIER: OnceLock<Mutex<Option<Notifier>>> = OnceLock::new();
    NOTIFIER.get_or_init(Default::default)
}

/// Where status changes go: every connected ACP client (`acp/clients.rs`).
pub(crate) fn set_notifier(notify: impl Fn(String, SessionUpdate) + Send + Sync + 'static) {
    *notifier().lock().unwrap_or_else(|error| error.into_inner()) = Some(Box::new(notify));
}

fn stored(session_id: &str) -> Option<Status> {
    let dir = crate::config::get_active_session_dir(session_id)?;
    let text = std::fs::read_to_string(dir.join(STATUS_FILE)).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    Status::storable(value.get("status")?.as_str()?)
}

fn store(session_id: &str, status: Status) -> Result<(), String> {
    let dir = crate::config::get_active_session_dir(session_id)
        .ok_or_else(|| "session storage is unavailable".to_owned())?;
    let path = dir.join(STATUS_FILE);
    if status == Status::Idle {
        let _ = std::fs::remove_file(path);
        return Ok(());
    }
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    let text = json!({ "status": status.as_str(), "at": chrono::Utc::now().to_rfc3339() });
    crate::worktrees::write_atomic(&path, text.to_string().as_bytes()).map_err(|e| e.to_string())
}

/// The thread's status now.
pub(crate) fn current(session_id: &str) -> Status {
    let live = live()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(session_id)
        .copied()
        .unwrap_or_default();
    if live.waits > 0 {
        Status::NeedsInput
    } else if live.turns > 0 {
        Status::Running
    } else {
        stored(session_id).unwrap_or(Status::Idle)
    }
}

pub(crate) fn is_running(session_id: &str) -> bool {
    matches!(current(session_id), Status::Running | Status::NeedsInput)
}

/// Whether a turn runs or waits on the user right now, read from memory only.
///
/// [`is_running`] falls back to the stored status, which is a file read; a
/// caller that looks every 100 ms (a shared thread's loop) wants the live
/// answer alone, which is the same one: a stored `done` or `review` is never
/// `is_running`.
pub(crate) fn is_active(session_id: &str) -> bool {
    let live = live()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(session_id)
        .copied()
        .unwrap_or_default();
    live.turns > 0 || live.waits > 0
}

/// The `session_info_update` that tells the client `status`.
pub(crate) fn update(status: Status) -> SessionUpdate {
    SessionUpdate::SessionInfoUpdate(SessionInfoUpdate::new().meta(serde_json::Map::from_iter([(
        "rustcode".to_owned(),
        json!({ "status": status.as_str() }),
    )])))
}

fn notify(session_id: &str) {
    let status = current(session_id);
    if let Some(notify) = notifier()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .as_ref()
    {
        notify(session_id.to_owned(), update(status));
    }
}

fn change(session_id: &str, edit: impl FnOnce(&mut Live)) {
    let before = current(session_id);
    {
        let mut live = live().lock().unwrap_or_else(|error| error.into_inner());
        let entry = live.entry(session_id.to_owned()).or_default();
        edit(entry);
        if entry.turns == 0 && entry.waits == 0 {
            live.remove(session_id);
        }
    }
    if current(session_id) != before {
        notify(session_id);
    }
}

/// A turn of a thread runs while this lives.
pub(crate) struct TurnGuard(String);

/// Mark a turn of `session_id`. A new turn takes a `review` or `done` thread
/// back to plain (`idle` once it ends): the thread is being worked on again.
pub(crate) fn turn(session_id: &str) -> TurnGuard {
    if stored(session_id).is_some() {
        let _ = store(session_id, Status::Idle);
    }
    change(session_id, |live| live.turns += 1);
    TurnGuard(session_id.to_owned())
}

impl Drop for TurnGuard {
    fn drop(&mut self) {
        change(&self.0, |live| live.turns = live.turns.saturating_sub(1));
    }
}

/// The thread waits on the user (approval or question) while this lives.
pub(crate) struct WaitGuard(String);

pub(crate) fn waiting(session_id: &str) -> WaitGuard {
    change(session_id, |live| live.waits += 1);
    WaitGuard(session_id.to_owned())
}

impl Drop for WaitGuard {
    fn drop(&mut self) {
        change(&self.0, |live| live.waits = live.waits.saturating_sub(1));
    }
}

/// Set a stored status and tell the client.
pub(crate) fn set(session_id: &str, status: &str) -> Result<(), String> {
    if !rustcode_session::valid_session_id(session_id) {
        return Err("invalid session id".to_owned());
    }
    let status = Status::storable(status)
        .ok_or_else(|| format!("status must be review, done or idle, not '{status}'"))?;
    store(session_id, status)?;
    notify(session_id);
    Ok(())
}

/// `_rustcode/session/status`.
#[derive(
    Debug,
    Clone,
    Default,
    serde::Serialize,
    serde::Deserialize,
    agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/session/status", response = serde_json::Value)]
pub(crate) struct SetStatusRequest {
    #[serde(alias = "sessionId")]
    pub session_id: String,
    pub status: String,
}

/// Whether the thread is pinned.
pub(crate) fn pinned(session_id: &str) -> bool {
    crate::config::get_active_session_dir(session_id)
        .is_some_and(|dir| dir.join(PIN_FILE).is_file())
}

/// Pin or unpin a thread and tell the client.
pub(crate) fn set_pinned(session_id: &str, pinned: bool) -> Result<(), String> {
    if !rustcode_session::valid_session_id(session_id) {
        return Err("invalid session id".to_owned());
    }
    let dir = crate::config::get_active_session_dir(session_id)
        .ok_or_else(|| "session storage is unavailable".to_owned())?;
    let path = dir.join(PIN_FILE);
    if pinned {
        std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
        let text = json!({ "pinned": true, "at": chrono::Utc::now().to_rfc3339() });
        crate::worktrees::write_atomic(&path, text.to_string().as_bytes())
            .map_err(|error| error.to_string())?;
    } else {
        let _ = std::fs::remove_file(path);
    }
    if let Some(notify) = notifier()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .as_ref()
    {
        notify(session_id.to_owned(), pin_update(pinned));
    }
    Ok(())
}

/// The `session_info_update` that tells the client a pin changed.
pub(crate) fn pin_update(pinned: bool) -> SessionUpdate {
    SessionUpdate::SessionInfoUpdate(SessionInfoUpdate::new().meta(serde_json::Map::from_iter([(
        "rustcode".to_owned(),
        json!({ "pinned": pinned }),
    )])))
}

/// `_rustcode/session/pin`.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/session/pin", response = serde_json::Value)]
pub(crate) struct PinRequest {
    #[serde(alias = "sessionId")]
    pub session_id: String,
    pub pinned: bool,
}

/// Whether a pull request watch of a thread says the thread is done: its
/// pull request merged, and the thread runs in a worktree (whose branch the
/// pull request carries).
pub(crate) fn merged_means_done(
    watch: &crate::pull_watch::Watch,
    worktree: Option<&crate::worktrees::Worktree>,
) -> bool {
    worktree.is_some()
        && watch
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.state == "merged")
}

/// A pull request watch was checked: a worktree thread whose pull request
/// merged becomes `done`.
pub(crate) fn on_pull_watch(watch: &crate::pull_watch::Watch) {
    let worktree = crate::worktrees::of_session(&watch.session_id);
    if merged_means_done(watch, worktree.as_ref()) && current(&watch.session_id) != Status::Done {
        let _ = set(&watch.session_id, "done");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_state_wins_over_the_stored_one() {
        let id = crate::config::create_detached_session();
        assert_eq!(current(&id), Status::Idle);
        set(&id, "review").unwrap();
        assert_eq!(current(&id), Status::Review);
        assert!(set(&id, "running").is_err());
        {
            let _turn = turn(&id);
            assert_eq!(current(&id), Status::Running);
            {
                let _wait = waiting(&id);
                assert_eq!(current(&id), Status::NeedsInput);
            }
            assert_eq!(current(&id), Status::Running);
        }
        // The turn cleared `review`.
        assert_eq!(current(&id), Status::Idle);
        set(&id, "done").unwrap();
        assert_eq!(current(&id), Status::Done);
        set(&id, "idle").unwrap();
        assert_eq!(current(&id), Status::Idle);
    }

    #[test]
    fn a_pin_round_trips_with_the_thread() {
        use agent_client_protocol::JsonRpcMessage;
        let id = crate::config::create_detached_session();
        assert!(!pinned(&id));
        set_pinned(&id, true).unwrap();
        assert!(pinned(&id));
        set_pinned(&id, true).unwrap();
        assert!(pinned(&id), "pinning twice keeps it pinned");
        set_pinned(&id, false).unwrap();
        assert!(!pinned(&id));
        assert!(set_pinned("../escape", true).is_err());
        let value = serde_json::to_value(pin_update(true)).unwrap();
        assert_eq!(value["_meta"]["rustcode"]["pinned"], true);
        assert!(PinRequest::matches_method("_rustcode/session/pin"));
        let request: PinRequest =
            serde_json::from_value(json!({ "sessionId": id, "pinned": true })).unwrap();
        assert!(request.pinned);
    }

    #[test]
    fn the_cli_writer_stores_the_status_the_app_reads() {
        let id = crate::config::create_detached_session();
        let dir = crate::config::get_active_session_dir(&id).expect("session dir");
        std::fs::write(
            dir.join("history.json"),
            r#"[{"role":"user","content":"hi"},{"role":"assistant","content":"ok"}]"#,
        )
        .expect("write transcript");
        assert!(crate::config::session_ids().contains(&id));

        crate::config::set_session_status(&id, "done").expect("settle");
        assert_eq!(current(&id), Status::Done);
        crate::config::set_session_status(&id, "review").expect("review");
        assert_eq!(current(&id), Status::Review);
        crate::config::set_session_status(&id, "idle").expect("unsettle");
        assert_eq!(current(&id), Status::Idle);

        // The MCP spelling of settled is not what the store holds.
        assert!(crate::config::set_session_status(&id, "settled").is_err());
        // An id with no thread folder is refused instead of creating one.
        assert!(
            crate::config::set_session_status("ffffffff-ffff-7fff-8fff-ffffffffffff", "done")
                .is_err()
        );
    }

    #[test]
    fn the_update_carries_the_status() {
        let value = serde_json::to_value(update(Status::NeedsInput)).unwrap();
        assert_eq!(value["sessionUpdate"], "session_info_update");
        assert_eq!(value["_meta"]["rustcode"]["status"], "needs_input");
    }

    #[test]
    fn a_merged_watch_marks_a_worktree_thread_done() {
        let mut watch: crate::pull_watch::Watch = serde_json::from_value(json!({
            "sessionId": "s", "cwd": "/r", "number": 1, "interests": {},
            "status": "watching", "createdAt": 0, "nextCheckAt": 0
        }))
        .unwrap_or_else(|_| panic!("watch shape"));
        let worktree = crate::worktrees::Worktree {
            path: "/w".into(),
            branch: "feat/x".into(),
            repo: "/r".into(),
            base: "abc".into(),
            base_ref: None,
            created_at: String::new(),
        };
        assert!(!merged_means_done(&watch, Some(&worktree)));
        watch.snapshot = Some(crate::pull_watch::Snapshot {
            state: "merged".into(),
            ..Default::default()
        });
        assert!(merged_means_done(&watch, Some(&worktree)));
        assert!(!merged_means_done(&watch, None));
    }
}
