//! One connection to the user's data store, kept open for the life of the
//! process. Opening a connection per write cost about 5 ms: in WAL mode,
//! closing the last connection checkpoints and syncs the log every time, and
//! every open re-ran the migration checks. A kept connection writes a row in
//! well under a tenth of a millisecond.

use rusqlite::Connection;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock, TryLockError};

type Slot = Option<(PathBuf, Connection)>;

/// A connection to the store: the shared one, or one of its own when the
/// shared one is busy or `path` is another store (a test's).
pub enum Store {
    Own(Connection),
    Shared(MutexGuard<'static, Slot>),
}

impl Deref for Store {
    type Target = Connection;

    fn deref(&self) -> &Connection {
        match self {
            Self::Own(connection) => connection,
            Self::Shared(slot) => {
                &slot
                    .as_ref()
                    .expect("the shared store is open while borrowed")
                    .1
            }
        }
    }
}

fn slot() -> &'static Mutex<Slot> {
    static SLOT: OnceLock<Mutex<Slot>> = OnceLock::new();
    SLOT.get_or_init(Default::default)
}

/// The store at `path`: the kept connection when `path` is `shared` and it is
/// free, else a new one from `open`. Never waits for the kept connection, so
/// a caller already holding it gets a connection of its own, not a deadlock.
pub(super) fn connect(
    path: &Path,
    shared: Option<&Path>,
    open: impl Fn(&Path) -> Result<Connection, String>,
) -> Result<Store, String> {
    if shared != Some(path) {
        return open(path).map(Store::Own);
    }
    let mut slot = match slot().try_lock() {
        Ok(slot) => slot,
        Err(TryLockError::Poisoned(poisoned)) => {
            let mut slot = poisoned.into_inner();
            *slot = None;
            slot
        }
        Err(TryLockError::WouldBlock) => return open(path).map(Store::Own),
    };
    if slot.as_ref().is_none_or(|(kept, _)| kept != path) {
        *slot = Some((path.to_path_buf(), open(path)?));
    }
    Ok(Store::Shared(slot))
}

#[cfg(test)]
pub(super) fn forget() {
    *slot().lock().unwrap_or_else(|error| error.into_inner()) = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_plain(path: &Path) -> Result<Connection, String> {
        Connection::open(path).map_err(|error| error.to_string())
    }

    fn shared_once(path: &Path) -> Store {
        for _ in 0..1_000 {
            let store = connect(path, Some(path), open_plain).unwrap();
            if matches!(store, Store::Shared(_)) {
                return store;
            }
            drop(store);
            std::thread::yield_now();
        }
        panic!("the shared connection stayed busy");
    }

    #[test]
    fn the_shared_store_is_kept_and_a_busy_one_is_not_waited_for() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("data.sqlite");
        let other = directory.path().join("other.sqlite");

        let held = shared_once(&path);
        held.execute_batch("CREATE TABLE IF NOT EXISTS kept (n INTEGER)")
            .unwrap();
        let while_held = connect(&path, Some(&path), open_plain).unwrap();
        assert!(matches!(while_held, Store::Own(_)));
        drop(while_held);
        drop(held);

        let again = shared_once(&path);
        again.execute("INSERT INTO kept VALUES (1)", []).unwrap();
        drop(again);

        assert!(matches!(
            connect(&other, Some(&path), open_plain).unwrap(),
            Store::Own(_)
        ));
        assert!(matches!(
            connect(&path, None, open_plain).unwrap(),
            Store::Own(_)
        ));
        forget();
    }

    #[test]
    #[ignore = "a measurement: cargo test --lib data_store_write_throughput -- --ignored --nocapture"]
    fn data_store_write_throughput() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("data.sqlite");
        drop(crate::data_store::open(&path).unwrap());
        let insert = "INSERT INTO approvals (thread_id, source, tool_name, requested_at, \
                      decided_at, wait_ms, decision) VALUES ('t', 'builtin', 'x', 'a', 'b', 1, 'allow_once')";
        let rows = 1_000;

        let started = std::time::Instant::now();
        for _ in 0..rows {
            let store = connect(&path, None, super::super::open_connection).unwrap();
            store.execute(insert, []).unwrap();
        }
        let per_open = started.elapsed() / rows;

        let started = std::time::Instant::now();
        for _ in 0..rows {
            let store = shared_once(&path);
            store.execute(insert, []).unwrap();
        }
        let kept = started.elapsed() / rows;
        forget();

        println!(
            "data store write: {per_open:?} opening per write, {kept:?} on the kept connection"
        );
        assert!(kept * 10 <= per_open, "{kept:?} vs {per_open:?}");
    }
}
