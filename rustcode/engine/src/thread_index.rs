//! A SQLite index over the thread folders:
//! one file (`index.sqlite` in the config directory) with a `threads` table
//! and an FTS5 trigram index over what was said, so the app can search every
//! thread by its words, not only its title.
//!
//! The folders stay the source of truth. The index is built from them,
//! refreshed by folder modification time, and when it is missing or
//! unreadable it is deleted and built again: losing it loses nothing.
//! Migrations are numbered and recorded with their names; a recorded name
//! that does not match refuses to open rather than skip a step.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};

pub(crate) const INDEX_FILE: &str = "index.sqlite";

/// Longest message text kept for search; a huge tool dump adds little.
const MAX_TEXT_CHARS: usize = 6_000;

const MIGRATIONS: &[(i64, &str, &str)] = &[(
    1,
    "threads_and_message_search",
    "CREATE TABLE threads (
        id TEXT PRIMARY KEY,
        title TEXT NOT NULL,
        cwd TEXT,
        message_count INTEGER NOT NULL,
        folder_mtime INTEGER NOT NULL,
        indexed_at INTEGER NOT NULL
    );
    CREATE INDEX threads_mtime ON threads(folder_mtime);
    CREATE VIRTUAL TABLE messages_fts USING fts5(
        thread_id UNINDEXED,
        role UNINDEXED,
        text,
        tokenize = 'trigram'
    );",
)];

/// One thread as the folders describe it, for the index.
pub(crate) struct ThreadRecord {
    pub id: String,
    pub title: String,
    pub cwd: Option<String>,
    /// Seconds since the epoch.
    pub mtime: i64,
    /// `(role, text)` of each message worth searching.
    pub messages: Vec<(String, String)>,
}

/// A search answer: the thread and the words around the match.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Hit {
    pub id: String,
    pub title: String,
    pub snippet: String,
    pub mtime: i64,
}

pub(crate) fn index_path() -> Option<PathBuf> {
    crate::config::get_config_dir().map(|dir| dir.join(INDEX_FILE))
}

/// Opens (creating and migrating) the index; an unreadable file is rebuilt.
pub(crate) fn open(path: &Path) -> Result<Connection, String> {
    match open_once(path) {
        Ok(connection) => Ok(connection),
        Err(first) => {
            // The index is a cache of the folders: start it over.
            for suffix in ["", "-wal", "-shm"] {
                let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
            }
            open_once(path).map_err(|second| format!("{first}; rebuilding failed: {second}"))
        }
    }
}

fn open_once(path: &Path) -> Result<Connection, String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let connection = Connection::open(path).map_err(|error| error.to_string())?;
    connection
        .execute_batch(
            "PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA busy_timeout = 5000;",
        )
        .map_err(|error| error.to_string())?;
    migrate(&connection)?;
    Ok(connection)
}

fn migrate(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (id INTEGER PRIMARY KEY, name TEXT NOT NULL);",
        )
        .map_err(|error| error.to_string())?;
    for (id, name, sql) in MIGRATIONS {
        let recorded: Option<String> = connection
            .query_row(
                "SELECT name FROM schema_migrations WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        match recorded {
            Some(recorded) if recorded == *name => continue,
            Some(recorded) => {
                return Err(format!(
                    "index migration {id} is recorded as {recorded:?}, expected {name:?}"
                ));
            }
            None => {
                let transaction = connection
                    .unchecked_transaction()
                    .map_err(|error| error.to_string())?;
                transaction
                    .execute_batch(sql)
                    .map_err(|error| error.to_string())?;
                transaction
                    .execute(
                        "INSERT INTO schema_migrations (id, name) VALUES (?1, ?2)",
                        params![id, name],
                    )
                    .map_err(|error| error.to_string())?;
                transaction.commit().map_err(|error| error.to_string())?;
            }
        }
    }
    Ok(())
}

/// Brings the index up to date with the `listed` threads (id, folder mtime):
/// a thread whose folder changed since it was indexed is `read` and written
/// again, an unchanged one is not opened, and threads no longer listed are
/// dropped. Returns how many threads were (re)indexed.
pub(crate) fn refresh(
    connection: &Connection,
    listed: &[(String, i64)],
    mut read: impl FnMut(&str) -> Option<ThreadRecord>,
) -> Result<usize, String> {
    let mut written = 0;
    for (id, mtime) in listed {
        let known: Option<i64> = connection
            .query_row(
                "SELECT folder_mtime FROM threads WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        if known.is_some_and(|known| known >= *mtime) {
            continue;
        }
        let Some(record) = read(id) else { continue };
        write(connection, &record)?;
        written += 1;
    }
    // Threads whose folders are gone.
    let listed_ids: std::collections::HashSet<&str> =
        listed.iter().map(|(id, _)| id.as_str()).collect();
    let indexed: Vec<String> = {
        let mut statement = connection
            .prepare("SELECT id FROM threads")
            .map_err(|error| error.to_string())?;
        statement
            .query_map([], |row| row.get(0))
            .map_err(|error| error.to_string())?
            .filter_map(Result::ok)
            .collect()
    };
    for id in indexed
        .iter()
        .filter(|id| !listed_ids.contains(id.as_str()))
    {
        connection
            .execute("DELETE FROM threads WHERE id = ?1", [id])
            .map_err(|error| error.to_string())?;
        connection
            .execute("DELETE FROM messages_fts WHERE thread_id = ?1", [id])
            .map_err(|error| error.to_string())?;
    }
    Ok(written)
}

fn write(connection: &Connection, record: &ThreadRecord) -> Result<(), String> {
    let transaction = connection
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "INSERT INTO threads (id, title, cwd, message_count, folder_mtime, indexed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, strftime('%s','now'))
             ON CONFLICT(id) DO UPDATE SET title = ?2, cwd = ?3, message_count = ?4,
                folder_mtime = ?5, indexed_at = strftime('%s','now')",
            params![
                record.id,
                record.title,
                record.cwd,
                record.messages.len() as i64,
                record.mtime
            ],
        )
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "DELETE FROM messages_fts WHERE thread_id = ?1",
            [&record.id],
        )
        .map_err(|error| error.to_string())?;
    for (role, text) in &record.messages {
        let text: String = text.chars().take(MAX_TEXT_CHARS).collect();
        if text.trim().is_empty() {
            continue;
        }
        transaction
            .execute(
                "INSERT INTO messages_fts (thread_id, role, text) VALUES (?1, ?2, ?3)",
                params![record.id, role, text],
            )
            .map_err(|error| error.to_string())?;
    }
    transaction.commit().map_err(|error| error.to_string())
}

/// Threads whose messages or title contain `query` (three characters or
/// more, as the trigram index needs), newest first, one hit per thread.
pub(crate) fn search(
    connection: &Connection,
    query: &str,
    limit: usize,
) -> Result<Vec<Hit>, String> {
    let query = query.trim();
    if query.chars().count() < 3 {
        return Ok(Vec::new());
    }
    // A quoted FTS5 string: the words as written, quotes doubled.
    let phrase = format!("\"{}\"", query.replace('"', "\"\""));
    let mut statement = connection
        .prepare(
            "SELECT t.id, t.title, snippet(messages_fts, 2, '', '', '…', 64), t.folder_mtime
             FROM messages_fts JOIN threads t ON t.id = messages_fts.thread_id
             WHERE messages_fts MATCH ?1
             ORDER BY t.folder_mtime DESC
             LIMIT ?2",
        )
        .map_err(|error| error.to_string())?;
    let mut hits: Vec<Hit> = Vec::new();
    let rows = statement
        .query_map(params![phrase, (limit * 8) as i64], |row| {
            Ok(Hit {
                id: row.get(0)?,
                title: row.get(1)?,
                snippet: row.get(2)?,
                mtime: row.get(3)?,
            })
        })
        .map_err(|error| error.to_string())?;
    for hit in rows.filter_map(Result::ok) {
        if !hits.iter().any(|known| known.id == hit.id) {
            hits.push(hit);
        }
        if hits.len() >= limit {
            break;
        }
    }
    Ok(hits)
}

/// The thread folders as the session store lists them, with their mtimes.
fn listed_threads() -> Vec<(String, i64)> {
    crate::config::list_sessions()
        .into_iter()
        .filter_map(|meta| {
            let id = crate::config::session_id_from_path(&meta.path)?;
            let mtime = std::fs::metadata(&meta.path)
                .and_then(|metadata| metadata.modified())
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |since| since.as_secs() as i64);
            Some((id, mtime))
        })
        .collect()
}

fn read_thread(id: &str, mtime: i64) -> Option<ThreadRecord> {
    let (meta, history) = crate::config::load_session_by_id(id)?;
    let messages = history
        .iter()
        .filter(|message| message.role == "user" || message.role == "assistant")
        .map(|message| (message.role.clone(), message.content.clone()))
        .collect();
    Some(ThreadRecord {
        id: id.to_owned(),
        title: meta.title,
        cwd: meta.workspace_cwd.map(|cwd| cwd.display().to_string()),
        mtime,
        messages,
    })
}

/// Builds the index in the background when the engine starts, so the first
/// search does not wait for every folder to be read.
pub(crate) fn warm() {
    std::thread::spawn(|| {
        let Some(path) = index_path() else { return };
        let Ok(connection) = open(&path) else { return };
        let listed = listed_threads();
        let mtimes: std::collections::HashMap<&str, i64> = listed
            .iter()
            .map(|(id, mtime)| (id.as_str(), *mtime))
            .collect();
        let _ = refresh(&connection, &listed, |id| {
            read_thread(id, mtimes.get(id).copied().unwrap_or(0))
        });
    });
}

/// `_rustcode/threads/search`.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/threads/search", response = serde_json::Value)]
pub(crate) struct SearchRequest {
    pub query: String,
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Refreshes the index from the folders, then searches it.
pub(crate) fn answer_search(request: &SearchRequest) -> Result<Value, String> {
    let path = index_path().ok_or_else(|| "no config directory".to_owned())?;
    let connection = open(&path)?;
    let listed = listed_threads();
    let mtimes: std::collections::HashMap<&str, i64> = listed
        .iter()
        .map(|(id, mtime)| (id.as_str(), *mtime))
        .collect();
    refresh(&connection, &listed, |id| {
        read_thread(id, mtimes.get(id).copied().unwrap_or(0))
    })?;
    let hits = search(
        &connection,
        &request.query,
        request.limit.unwrap_or(20).clamp(1, 100),
    )?;
    Ok(json!({
        "results": hits.iter().map(|hit| json!({
            "session_id": hit.id,
            "title": hit.title,
            "snippet": hit.snippet,
            "updated_at": hit.mtime,
        })).collect::<Vec<_>>(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: &str, mtime: i64, messages: &[(&str, &str)]) -> ThreadRecord {
        ThreadRecord {
            id: id.into(),
            title: format!("title {id}"),
            cwd: Some("/repo".into()),
            mtime,
            messages: messages
                .iter()
                .map(|(r, t)| ((*r).into(), (*t).into()))
                .collect(),
        }
    }

    #[test]
    fn indexes_threads_and_finds_them_by_what_was_said() {
        let dir = tempfile::tempdir().unwrap();
        let connection = open(&dir.path().join(INDEX_FILE)).unwrap();
        let mut store = vec![
            record(
                "a",
                10,
                &[
                    ("user", "research the ostack cloud"),
                    ("assistant", "OStack is an MCP stack"),
                ],
            ),
            record("b", 20, &[("user", "fix the composer focus ring")]),
        ];
        let listed: Vec<_> = store.iter().map(|r| (r.id.clone(), r.mtime)).collect();
        let mut reads = 0;
        let written = refresh(&connection, &listed, |id| {
            reads += 1;
            store
                .iter()
                .position(|r| r.id == id)
                .map(|i| store.remove(i))
        })
        .unwrap();
        assert_eq!((written, reads), (2, 2));

        let hits = search(&connection, "mcp stack", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "a");
        assert!(
            hits[0].snippet.to_lowercase().contains("mcp stack"),
            "{}",
            hits[0].snippet
        );
        assert!(
            search(&connection, "fo", 10).unwrap().is_empty(),
            "under three characters finds nothing"
        );
        assert!(
            search(&connection, "\"quoted\" words", 10)
                .unwrap()
                .is_empty()
        );

        // Unchanged folders are not read again; a changed one is; a gone one is dropped.
        let mut reads = 0;
        let listed = vec![("a".to_owned(), 10), ("b".to_owned(), 30)];
        refresh(&connection, &listed, |id| {
            reads += 1;
            Some(record(id, 30, &[("user", "now about lighthouse scores")]))
        })
        .unwrap();
        assert_eq!(reads, 1);
        assert_eq!(search(&connection, "lighthouse", 10).unwrap()[0].id, "b");
        assert!(search(&connection, "focus ring", 10).unwrap().is_empty());
        refresh(&connection, &[("b".to_owned(), 30)], |_| None).unwrap();
        assert!(search(&connection, "mcp stack", 10).unwrap().is_empty());
    }

    #[test]
    fn a_broken_index_is_rebuilt_and_a_renamed_migration_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(INDEX_FILE);
        std::fs::write(&path, b"not a database").unwrap();
        let connection = open(&path).expect("the broken file is replaced");
        assert!(search(&connection, "anything", 5).unwrap().is_empty());

        connection
            .execute(
                "UPDATE schema_migrations SET name = 'something else' WHERE id = 1",
                [],
            )
            .unwrap();
        assert!(migrate(&connection).unwrap_err().contains("recorded as"));
    }
}
