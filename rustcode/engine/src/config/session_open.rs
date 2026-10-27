//! Which process has a saved session open.
//!
//! Every process that runs a session (the terminal, an ACP server such as the
//! macOS app, a remote session owner) holds an advisory lock on `open.lock`
//! in the session's directory for as long as the session is loaded. The
//! kernel drops the lock when the process ends, crash included, so a lock
//! file left behind by a dead process reads as free. The remote gateway
//! probes it before it starts a second owner on a saved session: two
//! processes running the same session both write its history, and the last
//! writer wins.

use rustcode_session::SessionStore;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const OPEN_LOCK_FILE: &str = "open.lock";

/// The kind of process that holds a session open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionHolder {
    /// An ACP client, such as the RustCode app on the Mac.
    App,
    /// The terminal (or another single-session frontend).
    Terminal,
    /// A remote session owner started by the gateway.
    Remote,
}

#[derive(Serialize, Deserialize)]
struct OpenRecord {
    pid: u32,
    holder: SessionHolder,
}

struct Held {
    session_id: String,
    /// `None` while another process holds the lock; taken on a later hold.
    file: Option<File>,
    holders: HashSet<SessionHolder>,
}

fn registry() -> &'static Mutex<HashMap<PathBuf, Held>> {
    static HELD: OnceLock<Mutex<HashMap<PathBuf, Held>>> = OnceLock::new();
    HELD.get_or_init(|| Mutex::new(HashMap::new()))
}

fn primary() -> &'static Mutex<Option<(PathBuf, String)>> {
    static PRIMARY: OnceLock<Mutex<Option<(PathBuf, String)>>> = OnceLock::new();
    PRIMARY.get_or_init(|| Mutex::new(None))
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

/// The lock path of an existing session directory; `None` when the session
/// has no directory yet (nothing to protect, and nothing is created).
fn open_lock_path(config_dir: &Path, session_id: &str) -> Option<PathBuf> {
    if !rustcode_session::valid_session_id(session_id) {
        return None;
    }
    let directory = SessionStore::new(config_dir).session_dir(session_id);
    directory.is_dir().then(|| directory.join(OPEN_LOCK_FILE))
}

fn try_take(path: &Path, holder: SessionHolder) -> Option<File> {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .ok()?;
    file.try_lock().ok()?;
    // Who holds it, for the message the gateway gives. Best effort: the lock
    // alone is what counts.
    let record = serde_json::to_vec(&OpenRecord {
        pid: std::process::id(),
        holder,
    })
    .unwrap_or_default();
    let _ = file
        .set_len(0)
        .and_then(|()| file.rewind())
        .and_then(|()| file.write_all(&record));
    Some(file)
}

/// Mark `session_id` as open in this process on behalf of `holder`. Returns
/// whether this process now holds the session's lock; `false` when another
/// live process holds it or the session has no directory. A later call
/// retries a lock that was busy.
pub fn hold_session_open(config_dir: &Path, session_id: &str, holder: SessionHolder) -> bool {
    let Some(path) = open_lock_path(config_dir, session_id) else {
        return false;
    };
    let mut held = lock(registry());
    let entry = held.entry(path.clone()).or_insert_with(|| Held {
        session_id: session_id.to_owned(),
        file: None,
        holders: HashSet::new(),
    });
    entry.holders.insert(holder);
    if entry.file.is_none() {
        entry.file = try_take(&path, holder);
    }
    entry.file.is_some()
}

/// Drop `holder`'s mark on `session_id`; the lock is released once nobody in
/// this process holds the session any more.
pub fn release_session_open(session_id: &str, holder: SessionHolder) {
    lock(registry()).retain(|_, entry| {
        if entry.session_id == session_id {
            entry.holders.remove(&holder);
        }
        !entry.holders.is_empty()
    });
}

/// Make `session_id` the one session a single-session frontend has open,
/// releasing the previous one.
pub fn hold_primary_session(config_dir: &Path, session_id: &str) {
    let mut current = lock(primary());
    let next = (config_dir.to_path_buf(), session_id.to_owned());
    if current.as_ref() == Some(&next) {
        // Retry a lock that was busy when the session was first opened.
        hold_session_open(config_dir, session_id, SessionHolder::Terminal);
        return;
    }
    if let Some((_, previous)) = current.take() {
        release_session_open(&previous, SessionHolder::Terminal);
    }
    hold_session_open(config_dir, session_id, SessionHolder::Terminal);
    *current = Some(next);
}

/// Whether a live process has `session_id` open, and what kind of process it
/// is when it said so (`None` inside means it did not). This process counts.
pub fn session_open_by(config_dir: &Path, session_id: &str) -> Option<Option<SessionHolder>> {
    let path = open_lock_path(config_dir, session_id)?;
    if let Some(entry) = lock(registry()).get(&path)
        && entry.file.is_some()
    {
        return Some(entry.holders.iter().next().copied());
    }
    if !path.exists() {
        return None;
    }
    let mut file = OpenOptions::new().read(true).open(&path).ok()?;
    match file.try_lock() {
        // A lock nobody holds: its process ended. Dropping the file frees it.
        Ok(()) => None,
        Err(std::fs::TryLockError::WouldBlock) => {
            let mut text = String::new();
            let _ = file.read_to_string(&mut text);
            Some(
                serde_json::from_str::<OpenRecord>(&text)
                    .ok()
                    .map(|record| record.holder),
            )
        }
        Err(std::fs::TryLockError::Error(_)) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(config: &Path) -> String {
        let id = rustcode_session::next_session_id();
        SessionStore::new(config).ensure_session(&id);
        id
    }

    /// Another process holding the lock, as far as flock can tell: a second
    /// open file description, which conflicts even within one process.
    fn hold_elsewhere(config: &Path, id: &str, holder: SessionHolder) -> File {
        let path = open_lock_path(config, id).unwrap();
        try_take(&path, holder).expect("lock is free")
    }

    #[test]
    fn a_session_held_by_another_process_reads_as_open_by_it() {
        let config = tempfile::tempdir().unwrap();
        let id = session(config.path());
        assert_eq!(session_open_by(config.path(), &id), None);
        let _app = hold_elsewhere(config.path(), &id, SessionHolder::App);
        assert_eq!(
            session_open_by(config.path(), &id),
            Some(Some(SessionHolder::App))
        );
        // This process cannot take it while the other holds it.
        assert!(!hold_session_open(
            config.path(),
            &id,
            SessionHolder::Remote
        ));
        release_session_open(&id, SessionHolder::Remote);
    }

    #[test]
    fn a_lock_left_by_a_dead_process_is_free() {
        let config = tempfile::tempdir().unwrap();
        let id = session(config.path());
        drop(hold_elsewhere(config.path(), &id, SessionHolder::App));
        // The file and its record remain; nobody holds the lock.
        assert!(open_lock_path(config.path(), &id).unwrap().exists());
        assert_eq!(session_open_by(config.path(), &id), None);
        assert!(hold_session_open(config.path(), &id, SessionHolder::Remote));
        release_session_open(&id, SessionHolder::Remote);
    }

    #[test]
    fn the_lock_is_released_when_the_last_holder_lets_go() {
        let config = tempfile::tempdir().unwrap();
        let id = session(config.path());
        assert!(hold_session_open(config.path(), &id, SessionHolder::App));
        assert!(hold_session_open(
            config.path(),
            &id,
            SessionHolder::Terminal
        ));
        release_session_open(&id, SessionHolder::App);
        let path = open_lock_path(config.path(), &id).unwrap();
        assert!(try_take(&path, SessionHolder::App).is_none());
        release_session_open(&id, SessionHolder::Terminal);
        // A child another test forks while the lock is held shares its flock
        // until the child execs, so it can stay busy for a moment (#2360).
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while try_take(&path, SessionHolder::App).is_none() {
            assert!(
                std::time::Instant::now() < deadline,
                "the lock stayed busy after its last holder let go"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    #[test]
    fn a_busy_lock_is_taken_on_a_later_hold() {
        let config = tempfile::tempdir().unwrap();
        let id = session(config.path());
        let other = hold_elsewhere(config.path(), &id, SessionHolder::Remote);
        assert!(!hold_session_open(config.path(), &id, SessionHolder::App));
        drop(other);
        // A child another test forks while `other` is open shares its flock
        // until the child execs, so the lock can stay busy for a moment after
        // the drop. A later hold retries, as the real callers do.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !hold_session_open(config.path(), &id, SessionHolder::App) {
            assert!(
                std::time::Instant::now() < deadline,
                "lock still busy after release"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        release_session_open(&id, SessionHolder::App);
    }

    #[test]
    fn a_session_without_a_directory_is_never_locked() {
        let config = tempfile::tempdir().unwrap();
        let id = rustcode_session::next_session_id();
        assert!(!hold_session_open(config.path(), &id, SessionHolder::App));
        assert_eq!(session_open_by(config.path(), &id), None);
        assert!(!SessionStore::new(config.path()).session_dir(&id).exists());
    }
}
