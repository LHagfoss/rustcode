//! Thread status (`docs/orchestrator.md`, *Thread status*):
//! `idle | running | needs_input | stalled | review | done`.
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
    Stalled,
    Review,
    Done,
}

impl Status {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Running => "running",
            Self::NeedsInput => "needs_input",
            Self::Stalled => "stalled",
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
    match value.get("status")?.as_str()? {
        "stalled" => Some(Status::Stalled),
        status => Status::storable(status),
    }
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

fn watched() -> &'static Mutex<std::collections::HashSet<String>> {
    static IDS: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    IDS.get_or_init(Default::default)
}

/// Another engine (the desktop sidecar or the gateway's server), or the CLI,
/// can write the same thread files. Re-broadcast those changes to this engine's
/// clients too. Only threads already read by a client are polled.
pub(crate) fn start_watching() {
    static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    tokio::spawn(async {
        let mut previous = HashMap::new();
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            let ids: Vec<_> = watched()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .cloned()
                .collect();
            let changes = tokio::task::spawn_blocking(move || {
                ids.into_iter()
                    .map(|id| {
                        let state = (current(&id), snoozed_until(&id));
                        (id, state)
                    })
                    .collect::<Vec<_>>()
            })
            .await;
            let Ok(changes) = changes else { continue };
            for (id, state) in changes {
                if previous.insert(id.clone(), state) == Some(state) {
                    continue;
                }
                notify(&id, false);
                notify_snooze(&id, state.1);
            }
        }
    });
}

/// The thread's status now.
pub(crate) fn current(session_id: &str) -> Status {
    snapshot(session_id).0
}

pub(crate) fn snapshot(session_id: &str) -> (Status, bool) {
    watched()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(session_id.to_owned());
    let live = live()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(session_id)
        .copied()
        .unwrap_or_default();
    // Settling is an explicit decision even when the agent's final turn is still running.
    // Keep execution liveness separate (`is_active`) so this does not cancel that turn.
    let stored = stored(session_id);
    let working = live.turns > 0 || live.waits > 0;
    let status = if stored == Some(Status::Done) {
        Status::Done
    } else if live.waits > 0 {
        Status::NeedsInput
    } else if live.turns > 0 {
        Status::Running
    } else {
        stored.unwrap_or_else(|| {
            if unanswered_first_prompt(session_id) {
                Status::Stalled
            } else {
                Status::Idle
            }
        })
    };
    (status, working)
}

pub(crate) const FIRST_RESPONSE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// Older engines left only the user message if an external harness never answered.
fn unanswered_first_prompt(session_id: &str) -> bool {
    let Some(dir) = crate::config::get_active_session_dir(session_id) else {
        return false;
    };
    let old_enough = dir
        .join("history.json")
        .metadata()
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|age| age >= FIRST_RESPONSE_TIMEOUT);
    old_enough
        && crate::harness::store::load(session_id).is_some_and(|record| {
            record.harness != crate::harness::BUILTIN_ID && record.usage.is_none()
        })
        && has_only_first_prompt(&crate::config::load_session_history_direct(session_id))
}

pub(crate) fn has_only_first_prompt(history: &[crate::app::ChatMessage]) -> bool {
    matches!(history, [message] if message.role == "user" && !message.content.trim().is_empty())
}

pub(crate) fn first_response_timed_out(session_id: &str) {
    // A user can settle the thread while its turn is winding down.
    if stored(session_id).is_none() {
        let _ = store(session_id, Status::Stalled);
    }
}

pub(crate) fn is_running(session_id: &str) -> bool {
    is_active(session_id)
}

/// Whether a turn runs or waits on the user right now, read from memory only.
///
/// This remains true when the running turn has already stored `done`.
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
fn status_update(status: Status, self_settled: bool) -> SessionUpdate {
    let mut meta = json!({ "status": status.as_str() });
    if self_settled && status == Status::Done {
        meta["selfSettled"] = json!(true);
    }
    SessionUpdate::SessionInfoUpdate(
        SessionInfoUpdate::new().meta(serde_json::Map::from_iter([("rustcode".to_owned(), meta)])),
    )
}

fn notify(session_id: &str, own: bool) {
    let update = status_update(current(session_id), own);
    if let Some(notify) = notifier()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .as_ref()
    {
        notify(session_id.to_owned(), update);
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
        notify(session_id, false);
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
    set_status(session_id, status, false)
}

/// Restore persisted status, pin and snooze on a thread imported from another
/// machine without firing project settle hooks or live notifications.
pub(crate) fn restore_state(
    session_id: &str,
    status: Option<&str>,
    pinned_flag: bool,
    snoozed: Option<u64>,
) {
    if !rustcode_session::valid_session_id(session_id) {
        return;
    }
    if let Some(raw) = status
        && let Some(parsed) = Status::storable(raw)
        && parsed != Status::Idle
    {
        let _ = store(session_id, parsed);
    }
    if pinned_flag {
        let _ = set_pinned(session_id, true);
    }
    if let Some(until) = snoozed {
        let _ = set_snoozed(session_id, Some(until));
    }
}

pub(crate) fn set_own_status(session_id: &str, status: &str) -> Result<(), String> {
    set_status(session_id, status, true)
}

fn set_status(session_id: &str, status: &str, own: bool) -> Result<(), String> {
    if !rustcode_session::valid_session_id(session_id) {
        return Err("invalid session id".to_owned());
    }
    let status = Status::storable(status)
        .ok_or_else(|| format!("status must be review, done or idle, not '{status}'"))?;
    store(session_id, status)?;
    // A timestamp on the thread's row; settling never removes anything.
    crate::data_store::record_thread_settled(session_id, status == Status::Done);
    if status == Status::Done {
        if let Some(watches) = crate::pull_watch::WatchStore::default_store() {
            watches
                .settle(session_id)
                .map_err(|error| error.to_string())?;
        }
        crate::orchestration::local_threads::clear_waiting_links(session_id);
    }
    notify(session_id, own);
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

/// The owner's persisted snooze deadline, in Unix milliseconds.
pub(crate) fn snoozed_until(id: &str) -> Option<u64> {
    let dir = crate::config::get_active_session_dir(id)?;
    serde_json::from_slice::<Option<u64>>(&std::fs::read(dir.join("snooze.json")).ok()?)
        .ok()
        .flatten()
}

pub(crate) fn set_snoozed(id: &str, until: Option<u64>) -> Result<(), String> {
    if !rustcode_session::valid_session_id(id) {
        return Err("invalid session id".into());
    }
    let dir = crate::config::get_active_session_dir(id)
        .filter(|dir| dir.is_dir())
        .ok_or_else(|| "thread storage is unavailable".to_owned())?;
    crate::worktrees::write_atomic(
        &dir.join("snooze.json"),
        json!(until).to_string().as_bytes(),
    )
    .map_err(|e| e.to_string())?;
    notify_snooze(id, until);
    Ok(())
}

fn notify_snooze(id: &str, until: Option<u64>) {
    if let Some(notify) = notifier()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
    {
        notify(
            id.to_owned(),
            SessionUpdate::SessionInfoUpdate(SessionInfoUpdate::new().meta(
                serde_json::Map::from_iter([(
                    "rustcode".to_owned(),
                    json!({ "snoozedUntil": until }),
                )]),
            )),
        );
    }
}

#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/session/snooze", response = serde_json::Value)]
pub(crate) struct SnoozeRequest {
    pub session_id: String,
    pub until: Option<u64>,
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

/// Tell every connected client about one thread's change, as a status or pin
/// change is told: an engine-side rename (issue 4094).
pub(crate) fn announce(session_id: &str, update: SessionUpdate) {
    if let Some(notify) = notifier()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .as_ref()
    {
        notify(session_id.to_owned(), update);
    }
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

/// A watched or linked pull request (#2648, #3369) was checked and changed in
/// a way its thread has not been told about: mark the thread for review, the
/// engine's existing way of telling every client that a thread wants a look
/// (`review` is stored in `status.json` and surfaced by the apps). The events
/// themselves stay on the watch, so the thread's next
/// `manage_pull_links status` hands over exactly the ones it has not seen.
///
/// A thread with a turn running is not marked: its status is `running` now and
/// the turn ends `idle`, so a stored mark would only be cleared, and the events
/// wait for the thread's next turn either way.
pub(crate) fn on_pull_events(session_id: &str, events: &[crate::pull_watch::WatchEvent]) {
    if events.is_empty() || current(session_id) != Status::Idle {
        return;
    }
    let _ = set(session_id, "review");
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
    fn first_response_timeout_stops_working_and_can_be_retried() {
        let id = crate::config::create_detached_session();
        let first_turn = turn(&id);
        assert_eq!(snapshot(&id), (Status::Running, true));
        first_response_timed_out(&id);
        drop(first_turn);
        assert_eq!(snapshot(&id), (Status::Stalled, false));

        let retry = turn(&id);
        assert_eq!(snapshot(&id), (Status::Running, true));
        drop(retry);
        assert_eq!(snapshot(&id), (Status::Idle, false));
    }

    #[test]
    fn self_settle_survives_the_running_turn_and_pull_news() {
        let id = crate::config::create_detached_session();
        {
            let _turn = turn(&id);
            set(&id, "done").unwrap();
            assert_eq!(current(&id), Status::Done);
            assert!(is_running(&id), "settling does not stop the turn");
            let _wait = waiting(&id);
            assert_eq!(current(&id), Status::Done);
            on_pull_events(
                &id,
                &[crate::pull_watch::WatchEvent {
                    kind: "checks_failed".into(),
                    message: "Checks failed".into(),
                    at: String::new(),
                }],
            );
        }
        assert_eq!(current(&id), Status::Done);
        assert!(!is_running(&id));
        {
            let _user_turn = turn(&id);
            assert_eq!(current(&id), Status::Running);
        }
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
    fn settling_a_thread_keeps_everything_it_recorded() {
        let id = crate::config::create_detached_session();
        let dir = crate::config::get_active_session_dir(&id).expect("session dir");
        let files = [
            ("history.json", r#"[{"role":"user","content":"hi"}]"#),
            ("performance.json", r#"{"turns":[]}"#),
            ("logs/debug.log", "line\n"),
            ("logs/debug.log.1", "older line\n"),
        ];
        for (name, body) in files {
            let path = dir.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        }
        crate::config::set_session_status(&id, "done").expect("settle");
        assert_eq!(current(&id), Status::Done);
        for (name, body) in files {
            assert_eq!(
                std::fs::read_to_string(dir.join(name)).unwrap(),
                body,
                "{name} survives settling"
            );
        }
        // Settled is a status, not a removal: the thread is still listed.
        assert!(crate::config::session_ids().contains(&id));
    }

    #[test]
    fn snooze_round_trips_on_the_owner() {
        let id = crate::config::create_detached_session();
        assert_eq!(snoozed_until(&id), None);
        set_snoozed(&id, Some(123456)).unwrap();
        assert_eq!(snoozed_until(&id), Some(123456));
        set_snoozed(&id, None).unwrap();
        assert_eq!(snoozed_until(&id), None);
        assert!(set_snoozed("../escape", Some(1)).is_err());
    }

    #[test]
    fn the_update_carries_the_status() {
        let value = serde_json::to_value(status_update(Status::NeedsInput, false)).unwrap();
        assert_eq!(value["sessionUpdate"], "session_info_update");
        assert_eq!(value["_meta"]["rustcode"]["status"], "needs_input");
        let self_settle = serde_json::to_value(status_update(Status::Done, true)).unwrap();
        assert_eq!(self_settle["_meta"]["rustcode"]["selfSettled"], true);
        let ordinary_settle = serde_json::to_value(status_update(Status::Done, false)).unwrap();
        assert!(
            ordinary_settle["_meta"]["rustcode"]
                .get("selfSettled")
                .is_none()
        );
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

    #[test]
    fn a_link_event_marks_an_idle_thread_for_review_once() {
        let id = crate::config::create_detached_session();
        let event = crate::pull_watch::WatchEvent {
            kind: "checks_failed".into(),
            message: "Checks failed: test".into(),
            at: String::new(),
        };
        on_pull_events(&id, &[]);
        assert_eq!(
            current(&id),
            Status::Idle,
            "nothing changed, nothing to say"
        );

        on_pull_events(&id, std::slice::from_ref(&event));
        assert_eq!(current(&id), Status::Review);

        // A settled thread stays settled: an old link does not reopen it.
        set(&id, "done").unwrap();
        on_pull_events(&id, std::slice::from_ref(&event));
        assert_eq!(current(&id), Status::Done);

        // A thread with a turn running is not marked: the turn ends as it
        // would, and the events wait for the thread's next `status`.
        set(&id, "idle").unwrap();
        {
            let _turn = turn(&id);
            on_pull_events(&id, std::slice::from_ref(&event));
            assert_eq!(current(&id), Status::Running);
        }
        assert_eq!(current(&id), Status::Idle);
    }
}
