//! Whether a thread's turn runs, or waits on the user, in some process.
//!
//! The engine that runs a turn holds an advisory lock on `turn.lock` in the
//! thread's directory for as long as a turn runs, and on `wait.lock` while an
//! approval or question waits. The kernel drops both when the process ends,
//! crash included, so a file left behind reads as free. The remote gateway
//! probes them to report `running` and `needs_input` for a thread no share
//! streams to it (`docs/remote-protocol/README.md`, *Saved sessions*).

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::path::Path;
use std::sync::{Mutex, OnceLock};

const TURN_LOCK_FILE: &str = "turn.lock";
const WAIT_LOCK_FILE: &str = "wait.lock";

/// The locks this process holds, per thread: `(turn, wait)`.
type Held = HashMap<String, (Option<File>, Option<File>)>;

fn held() -> &'static Mutex<Held> {
    static HELD: OnceLock<Mutex<Held>> = OnceLock::new();
    HELD.get_or_init(Default::default)
}

fn take(directory: &Path, file: &str) -> Option<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join(file))
        .ok()?;
    file.try_lock().ok()?;
    Some(file)
}

fn keep(slot: &mut Option<File>, wanted: bool, directory: Option<&Path>, file: &str) {
    if !wanted {
        *slot = None;
    } else if slot.is_none() {
        *slot = directory.and_then(|directory| take(directory, file));
    }
}

/// Hold or release `session_id`'s locks so they say whether a turn runs and
/// whether it waits on the user. A thread without a directory gets none.
pub(crate) fn publish(session_id: &str, running: bool, waiting: bool) {
    if !rustcode_session::valid_session_id(session_id) {
        return;
    }
    let mut held = held().lock().unwrap_or_else(|error| error.into_inner());
    if !running && !waiting {
        held.remove(session_id);
        return;
    }
    let directory = super::get_active_session_dir(session_id).filter(|dir| dir.is_dir());
    let entry = held.entry(session_id.to_owned()).or_default();
    keep(&mut entry.0, running, directory.as_deref(), TURN_LOCK_FILE);
    keep(&mut entry.1, waiting, directory.as_deref(), WAIT_LOCK_FILE);
}

fn busy(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    let Ok(file) = OpenOptions::new().read(true).open(path) else {
        return false;
    };
    matches!(file.try_lock(), Err(std::fs::TryLockError::WouldBlock))
}

/// `(running, waiting)` of the thread in `directory`, in whichever process
/// holds its locks.
pub fn activity_in(directory: &Path) -> (bool, bool) {
    (
        busy(&directory.join(TURN_LOCK_FILE)),
        busy(&directory.join(WAIT_LOCK_FILE)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_locks_follow_the_turn_and_free_when_it_ends() {
        let id = crate::sessions::create_detached_session();
        let directory = crate::sessions::get_active_session_dir(&id).unwrap();
        std::fs::create_dir_all(&directory).unwrap();
        assert_eq!(activity_in(&directory), (false, false));
        publish(&id, true, false);
        assert_eq!(activity_in(&directory), (true, false));
        publish(&id, true, true);
        assert_eq!(activity_in(&directory), (true, true));
        publish(&id, false, true);
        assert_eq!(activity_in(&directory), (false, true));
        publish(&id, false, false);
        assert_eq!(activity_in(&directory), (false, false));
    }
}
