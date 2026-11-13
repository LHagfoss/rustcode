//! `_rustcode/mirror`: the durable copy of other environments' threads
//! (`docs/data-store.md` › Thread mirror).
//!
//! The app already reads every paired machine's thread list and the threads
//! the user opens. This is where it keeps that, on this computer's disk, so a
//! machine that is offline, wiped or forgotten still has its threads here, and
//! a wiped webview or a reinstalled app does not lose them.
//!
//! ```text
//! <config>/mirror/settings.json            the switch and the project opt-outs
//! <config>/mirror/<host>/index.json        one entry per thread the machine listed
//! <config>/mirror/<host>/threads/<id>.json the transcript last read from it
//! ```
//!
//! The origin machine stays the owner of a thread. The mirror is only ever
//! written from what that machine reported, never edited here, never moves or
//! deletes the original, and never deletes a mirrored thread either (a thread
//! the machine no longer lists stays: that is the point of a copy). Writes are
//! conflict-free because they commute: an incoming row replaces the stored one
//! only when it is at least as new by the origin's own `updatedAt` (ties break
//! on the row's canonical JSON), and an incoming transcript only when its
//! `revision` (when the app read it from the origin) is not older. Applying
//! the same reads twice, or in any order, leaves the same files.
//!
//! Replication is incremental: a transcript whose fingerprint (sha256 of its
//! JSON) is the stored one is not written again, and a row that did not change
//! is not either. It is throttled by the caller (the app writes a list at most
//! every few seconds and a transcript at most every two while it changes).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

/// One transcript larger than this is not mirrored (the app already caps what it sends).
const MAX_TRANSCRIPT_BYTES: usize = 32 * 1024 * 1024;
/// A request carries at most this many rows; the app sends its 200 newest.
const MAX_ROWS_PER_PUT: usize = 5_000;
/// Serialises writers (several windows, several connections) within one process.
static WRITE_LOCK: Mutex<()> = Mutex::new(());

/// `_rustcode/mirror`: one method for the mirror's needs, so the ACP router's
/// handler chain stays short.
#[derive(Debug, Clone, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[request(method = "_rustcode/mirror", response = serde_json::Value)]
pub(crate) struct MirrorRequest {
    #[serde(flatten)]
    pub action: MirrorAction,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "camelCase")]
pub(crate) enum MirrorAction {
    /// The switch, the project opt-outs and what the mirror holds.
    Status,
    /// Change the switch or the opt-outs; absent fields stay as they are.
    SetSettings {
        #[serde(default)]
        enabled: Option<bool>,
        #[serde(default, rename = "excludedProjects")]
        excluded_projects: Option<Vec<String>>,
    },
    /// Merge the rows a machine listed.
    PutList { host: String, sessions: Vec<Value> },
    /// Keep the transcript last read from a machine's thread.
    PutTranscript {
        host: String,
        id: String,
        cwd: String,
        revision: i64,
        transcript: Value,
    },
    /// What is mirrored: one machine's rows, or every machine's.
    List {
        #[serde(default)]
        host: Option<String>,
    },
    /// One mirrored transcript.
    Transcript { host: String, id: String },
}

/// What the user chose; the default copies everything.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MirrorSettings {
    pub enabled: bool,
    /// Project folders whose threads are not mirrored (a folder inside one counts).
    pub excluded_projects: Vec<String>,
}

impl Default for MirrorSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            excluded_projects: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct TranscriptMeta {
    revision: i64,
    fingerprint: String,
    bytes: u64,
    saved_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    row: Value,
    first_seen_at: i64,
    last_seen_at: i64,
    #[serde(default)]
    transcript: Option<TranscriptMeta>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Index {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    entries: BTreeMap<String, Entry>,
}

/// How a put went; counts add up to the rows or transcripts sent.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct PutOutcome {
    pub stored: usize,
    pub unchanged: usize,
    /// Older than what the mirror already holds.
    pub stale: usize,
    /// Not a usable row (no id or folder, or an id that is not a safe file name).
    pub skipped: usize,
    /// In a project the user opted out of.
    pub excluded: usize,
}

fn lock() -> std::sync::MutexGuard<'static, ()> {
    WRITE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A host or thread id that is safe as one path component.
fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn root_dir() -> Result<PathBuf, String> {
    crate::config::get_config_dir()
        .map(|dir| dir.join("mirror"))
        .ok_or_else(|| "could not determine the config directory".to_owned())
}

fn host_dir(root: &Path, host: &str) -> Result<PathBuf, String> {
    if !safe_name(host) {
        return Err(format!("'{host}' is not a machine id"));
    }
    Ok(root.join(host))
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
    }
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, bytes)
        .map_err(|error| format!("could not write {}: {error}", temporary.display()))?;
    crate::atomic_file::replace_file(&temporary, path)
        .map_err(|error| format!("could not write {}: {error}", path.display()))
}

fn read_json<T: for<'de> Deserialize<'de> + Default>(path: &Path) -> T {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn read_index(root: &Path, host: &str) -> Result<Index, String> {
    Ok(read_json(&host_dir(root, host)?.join("index.json")))
}

/// The index to change. One that exists but cannot be read is set aside as
/// `index.json.broken` first, so the write that follows never replaces the
/// only copy of what it could not parse.
fn read_index_for_write(root: &Path, host: &str) -> Result<Index, String> {
    let path = host_dir(root, host)?.join("index.json");
    if let Ok(bytes) = std::fs::read(&path)
        && serde_json::from_slice::<Index>(&bytes).is_err()
    {
        let _ = std::fs::write(path.with_extension("json.broken"), &bytes);
    }
    Ok(read_json(&path))
}

fn write_index(root: &Path, host: &str, index: &mut Index) -> Result<(), String> {
    index.version = 1;
    let bytes = serde_json::to_vec(index).map_err(|error| error.to_string())?;
    write_atomic(&host_dir(root, host)?.join("index.json"), &bytes)
}

// ---------------------------------------------------------------- settings

fn read_settings(root: &Path) -> MirrorSettings {
    std::fs::read(root.join("settings.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn set_settings(
    root: &Path,
    enabled: Option<bool>,
    opted_out: Option<Vec<String>>,
) -> Result<MirrorSettings, String> {
    let _guard = lock();
    let mut settings = read_settings(root);
    if let Some(enabled) = enabled {
        settings.enabled = enabled;
    }
    if let Some(opted_out) = opted_out {
        let mut seen = Vec::new();
        for path in opted_out {
            let path = path.trim().trim_end_matches(['/', '\\']).to_owned();
            if !path.is_empty() && !seen.contains(&path) {
                seen.push(path);
            }
        }
        settings.excluded_projects = seen;
    }
    let bytes = serde_json::to_vec_pretty(&settings).map_err(|error| error.to_string())?;
    write_atomic(&root.join("settings.json"), &bytes)?;
    Ok(settings)
}

/// Whether `cwd` is a folder the user opted out of, or inside one.
fn excluded(settings: &MirrorSettings, cwd: &str) -> bool {
    let thread = components(cwd);
    settings.excluded_projects.iter().any(|project| {
        let project = components(project);
        !project.is_empty() && thread.starts_with(&project)
    })
}

fn components(path: &str) -> Vec<String> {
    Path::new(&path.replace('\\', "/"))
        .components()
        .filter_map(|part| match part {
            Component::Normal(name) => Some(name.to_string_lossy().to_lowercase()),
            _ => None,
        })
        .collect()
}

// ---------------------------------------------------------------- merging

fn updated_at(row: &Value) -> i64 {
    row.get("updatedAt")
        .and_then(Value::as_f64)
        .map(|value| value as i64)
        .unwrap_or(0)
}

/// The total order rows merge by: the origin's `updatedAt`, then the row's JSON.
/// A total order makes the merge commutative and idempotent.
fn row_key(row: &Value) -> (i64, String) {
    (updated_at(row), row.to_string())
}

fn row_id(row: &Value) -> Option<&str> {
    row.get("id")
        .and_then(Value::as_str)
        .filter(|id| safe_name(id))
}

fn row_cwd(row: &Value) -> Option<&str> {
    row.get("cwd").and_then(Value::as_str)
}

fn fingerprint(value: &Value) -> (String, usize) {
    let bytes = serde_json::to_vec(value).unwrap_or_default();
    (hex::encode(Sha256::digest(&bytes)), bytes.len())
}

fn put_list_in(
    root: &Path,
    host: &str,
    sessions: &[Value],
    now: i64,
) -> Result<PutOutcome, String> {
    let settings = read_settings(root);
    if !settings.enabled {
        return Err("thread mirroring is off".to_owned());
    }
    if sessions.len() > MAX_ROWS_PER_PUT {
        return Err(format!("at most {MAX_ROWS_PER_PUT} rows at a time"));
    }
    let _guard = lock();
    let mut index = read_index_for_write(root, host)?;
    let mut outcome = PutOutcome::default();
    let mut changed = false;
    for row in sessions {
        let (Some(id), Some(cwd)) = (row_id(row), row_cwd(row)) else {
            outcome.skipped += 1;
            continue;
        };
        if excluded(&settings, cwd) {
            outcome.excluded += 1;
            continue;
        }
        match index.entries.get_mut(id) {
            None => {
                index.entries.insert(
                    id.to_owned(),
                    Entry {
                        row: row.clone(),
                        first_seen_at: now,
                        last_seen_at: now,
                        transcript: None,
                    },
                );
                outcome.stored += 1;
                changed = true;
            }
            Some(entry) if row_key(row) > row_key(&entry.row) => {
                entry.row = row.clone();
                entry.last_seen_at = entry.last_seen_at.max(now);
                outcome.stored += 1;
                changed = true;
            }
            Some(entry) if row_key(row) == row_key(&entry.row) => {
                outcome.unchanged += 1;
                if now > entry.last_seen_at {
                    entry.last_seen_at = now;
                    changed = true;
                }
            }
            Some(_) => outcome.stale += 1,
        }
    }
    if changed {
        write_index(root, host, &mut index)?;
    }
    Ok(outcome)
}

fn put_transcript_in(
    root: &Path,
    host: &str,
    id: &str,
    cwd: &str,
    revision: i64,
    transcript: &Value,
    now: i64,
) -> Result<PutOutcome, String> {
    let settings = read_settings(root);
    if !settings.enabled {
        return Err("thread mirroring is off".to_owned());
    }
    let mut outcome = PutOutcome::default();
    if !safe_name(id) {
        outcome.skipped = 1;
        return Ok(outcome);
    }
    if excluded(&settings, cwd) {
        outcome.excluded = 1;
        return Ok(outcome);
    }
    let (print, bytes) = fingerprint(transcript);
    if bytes > MAX_TRANSCRIPT_BYTES {
        return Err("that transcript is too large to mirror".to_owned());
    }
    let _guard = lock();
    let mut index = read_index_for_write(root, host)?;
    let entry = index.entries.entry(id.to_owned()).or_insert_with(|| Entry {
        row: json!({ "id": id, "cwd": cwd, "updatedAt": revision }),
        first_seen_at: now,
        last_seen_at: now,
        transcript: None,
    });
    if let Some(kept) = &entry.transcript {
        if revision < kept.revision {
            outcome.stale = 1;
            return Ok(outcome);
        }
        if kept.fingerprint == print {
            outcome.unchanged = 1;
            return Ok(outcome);
        }
    }
    let file = json!({ "revision": revision, "cwd": cwd, "transcript": transcript });
    let bytes_out = serde_json::to_vec(&file).map_err(|error| error.to_string())?;
    write_atomic(
        &host_dir(root, host)?
            .join("threads")
            .join(format!("{id}.json")),
        &bytes_out,
    )?;
    entry.last_seen_at = entry.last_seen_at.max(now);
    entry.transcript = Some(TranscriptMeta {
        revision,
        fingerprint: print,
        bytes: bytes_out.len() as u64,
        saved_at: now,
    });
    write_index(root, host, &mut index)?;
    outcome.stored = 1;
    Ok(outcome)
}

// ---------------------------------------------------------------- reading

fn hosts_in(root: &Path) -> Vec<String> {
    let mut hosts: Vec<String> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| safe_name(name))
        .collect();
    hosts.sort();
    hosts
}

fn entries_json(index: &Index) -> Vec<Value> {
    let mut entries: Vec<(&String, &Entry)> = index.entries.iter().collect();
    entries.sort_by(|a, b| {
        updated_at(&b.1.row)
            .cmp(&updated_at(&a.1.row))
            .then(a.0.cmp(b.0))
    });
    entries
        .into_iter()
        .map(|(_, entry)| {
            json!({
                "session": entry.row,
                "firstSeenAt": entry.first_seen_at,
                "lastSeenAt": entry.last_seen_at,
                "transcript": entry.transcript.as_ref().map(|t| json!({
                    "revision": t.revision,
                    "savedAt": t.saved_at,
                    "bytes": t.bytes,
                })),
            })
        })
        .collect()
}

fn list_in(root: &Path, host: Option<&str>) -> Result<Value, String> {
    let settings = read_settings(root);
    let mut hosts = Map::new();
    let names = match host {
        Some(host) => {
            host_dir(root, host)?;
            vec![host.to_owned()]
        }
        None => hosts_in(root),
    };
    for name in names {
        let index = read_index(root, &name)?;
        hosts.insert(name, json!({ "threads": entries_json(&index) }));
    }
    Ok(json!({ "enabled": settings.enabled, "hosts": hosts }))
}

fn transcript_in(root: &Path, host: &str, id: &str) -> Result<Value, String> {
    if !safe_name(id) {
        return Ok(json!({ "found": false }));
    }
    let path = host_dir(root, host)?
        .join("threads")
        .join(format!("{id}.json"));
    let Some(file) = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
    else {
        return Ok(json!({ "found": false }));
    };
    let saved_at = read_index(root, host)?
        .entries
        .get(id)
        .and_then(|entry| entry.transcript.as_ref().map(|t| t.saved_at));
    Ok(json!({
        "found": true,
        "revision": file.get("revision"),
        "cwd": file.get("cwd"),
        "savedAt": saved_at,
        "transcript": file.get("transcript"),
    }))
}

fn status_in(root: &Path) -> Result<Value, String> {
    let settings = read_settings(root);
    let mut hosts = Map::new();
    for name in hosts_in(root) {
        let index = read_index(root, &name)?;
        let transcripts = index
            .entries
            .values()
            .filter(|e| e.transcript.is_some())
            .count();
        let bytes: u64 = index
            .entries
            .values()
            .filter_map(|e| e.transcript.as_ref())
            .map(|t| t.bytes)
            .sum();
        hosts.insert(
            name,
            json!({ "threads": index.entries.len(), "transcripts": transcripts, "bytes": bytes }),
        );
    }
    Ok(json!({
        "enabled": settings.enabled,
        "excludedProjects": settings.excluded_projects,
        "hosts": hosts,
    }))
}

fn outcome_json(outcome: PutOutcome) -> Value {
    json!({
        "stored": outcome.stored,
        "unchanged": outcome.unchanged,
        "stale": outcome.stale,
        "skipped": outcome.skipped,
        "excluded": outcome.excluded,
    })
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn answer_in(root: &Path, action: MirrorAction, now: i64) -> Result<Value, String> {
    match action {
        MirrorAction::Status => status_in(root),
        MirrorAction::SetSettings {
            enabled,
            excluded_projects,
        } => {
            set_settings(root, enabled, excluded_projects)?;
            status_in(root)
        }
        MirrorAction::PutList { host, sessions } => {
            put_list_in(root, &host, &sessions, now).map(outcome_json)
        }
        MirrorAction::PutTranscript {
            host,
            id,
            cwd,
            revision,
            transcript,
        } => {
            put_transcript_in(root, &host, &id, &cwd, revision, &transcript, now).map(outcome_json)
        }
        MirrorAction::List { host } => list_in(root, host.as_deref()),
        MirrorAction::Transcript { host, id } => transcript_in(root, &host, &id),
    }
}

/// Answer an `_rustcode/mirror` request against this computer's mirror.
pub(crate) fn answer(request: MirrorRequest) -> Result<Value, String> {
    answer_in(&root_dir()?, request.action, now_ms())
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::JsonRpcMessage;

    fn temp_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("rustcode-tests").join(format!(
            "mirror-{}-{}",
            name,
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn row(id: &str, cwd: &str, updated_at: i64, title: &str) -> Value {
        json!({ "id": id, "cwd": cwd, "updatedAt": updated_at, "title": title })
    }

    fn snapshot(root: &Path, host: &str) -> Value {
        list_in(root, Some(host)).unwrap()["hosts"][host].clone()
    }

    #[test]
    fn the_request_is_a_rustcode_method_with_a_named_action() {
        assert!(MirrorRequest::matches_method("_rustcode/mirror"));
        let request: MirrorRequest = serde_json::from_value(
            json!({"action": "putList", "host": "h1", "sessions": [{"id": "a", "cwd": "/p"}]}),
        )
        .unwrap();
        assert!(matches!(request.action, MirrorAction::PutList { .. }));
        let request: MirrorRequest = serde_json::from_value(
            json!({"action": "setSettings", "excludedProjects": ["/p"], "enabled": false}),
        )
        .unwrap();
        assert_eq!(
            request.action,
            MirrorAction::SetSettings {
                enabled: Some(false),
                excluded_projects: Some(vec!["/p".to_owned()]),
            }
        );
    }

    #[test]
    fn a_list_is_kept_and_replicated_again_without_a_rewrite() {
        let root = temp_root("again");
        let rows = [
            row("a", "/work/a", 10, "first"),
            row("b", "/work/b", 20, "second"),
        ];
        let first = put_list_in(&root, "laptop", &rows, 100).unwrap();
        assert_eq!((first.stored, first.unchanged), (2, 0));
        let before = std::fs::metadata(root.join("laptop/index.json"))
            .unwrap()
            .modified()
            .unwrap();

        // The same read again changes nothing on disk except that nothing was newer.
        let second = put_list_in(&root, "laptop", &rows, 100).unwrap();
        assert_eq!((second.stored, second.unchanged), (0, 2));
        let after = std::fs::metadata(root.join("laptop/index.json"))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(before, after, "an unchanged list is not written again");

        let listed = snapshot(&root, "laptop");
        let threads = listed["threads"].as_array().unwrap();
        assert_eq!(threads.len(), 2);
        assert_eq!(threads[0]["session"]["id"], "b", "newest first");
    }

    #[test]
    fn a_newer_row_replaces_an_older_one_and_an_older_one_never_wins() {
        let root = temp_root("newer");
        put_list_in(&root, "m", &[row("a", "/p", 10, "old")], 1).unwrap();
        let newer = put_list_in(&root, "m", &[row("a", "/p", 30, "new")], 2).unwrap();
        assert_eq!(newer.stored, 1);
        let stale = put_list_in(&root, "m", &[row("a", "/p", 20, "middle")], 3).unwrap();
        assert_eq!(stale.stale, 1);
        let listed = snapshot(&root, "m");
        assert_eq!(listed["threads"][0]["session"]["title"], "new");
        assert_eq!(listed["threads"][0]["firstSeenAt"], 1);
        assert_eq!(listed["threads"][0]["lastSeenAt"], 2);
    }

    #[test]
    fn mirroring_is_conflict_free_whatever_order_reads_arrive_in() {
        let batches: Vec<Vec<Value>> = vec![
            vec![row("a", "/p", 10, "a1"), row("b", "/p", 5, "b1")],
            vec![row("a", "/p", 30, "a3"), row("c", "/p", 1, "c1")],
            vec![row("a", "/p", 20, "a2"), row("b", "/p", 7, "b2")],
            // Same time, different content: the tie breaks the same way every time.
            vec![row("c", "/p", 1, "c1-alt")],
        ];
        let orders: [&[usize]; 3] = [&[0, 1, 2, 3], &[3, 2, 1, 0], &[2, 0, 3, 1]];
        let mut results = Vec::new();
        for (n, order) in orders.iter().enumerate() {
            let root = temp_root(&format!("order{n}"));
            for &i in *order {
                // Each read is replayed twice: a retry must not matter either.
                put_list_in(&root, "m", &batches[i], 1).unwrap();
                put_list_in(&root, "m", &batches[i], 1).unwrap();
            }
            let listed = snapshot(&root, "m");
            let rows: Vec<Value> = listed["threads"]
                .as_array()
                .unwrap()
                .iter()
                .map(|thread| thread["session"].clone())
                .collect();
            results.push(rows);
        }
        assert_eq!(results[0], results[1]);
        assert_eq!(results[0], results[2]);
        let a = results[0].iter().find(|row| row["id"] == "a").unwrap();
        assert_eq!(a["title"], "a3", "the newest by the origin's clock wins");
    }

    #[test]
    fn a_thread_the_machine_stops_listing_stays_in_the_mirror() {
        let root = temp_root("kept");
        put_list_in(
            &root,
            "m",
            &[row("a", "/p", 1, "a"), row("b", "/p", 2, "b")],
            1,
        )
        .unwrap();
        // The machine was wiped: its next list is empty or lacks a thread.
        put_list_in(&root, "m", &[], 2).unwrap();
        put_list_in(&root, "m", &[row("b", "/p", 2, "b")], 3).unwrap();
        let listed = snapshot(&root, "m");
        assert_eq!(listed["threads"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn a_transcript_is_replicated_incrementally_by_revision_and_fingerprint() {
        let root = temp_root("transcript");
        let body = json!({ "items": [{ "kind": "user", "text": "hi" }] });
        let first = put_transcript_in(&root, "m", "a", "/p", 10, &body, 1).unwrap();
        assert_eq!(first.stored, 1);

        // The same content again is a no-op.
        let same = put_transcript_in(&root, "m", "a", "/p", 10, &body, 2).unwrap();
        assert_eq!((same.stored, same.unchanged), (0, 1));

        // The conversation grew on the origin: newer revision, new content.
        let grown = json!({ "items": [{ "kind": "user", "text": "hi" }, { "kind": "agent", "text": "hello" }] });
        let next = put_transcript_in(&root, "m", "a", "/p", 11, &grown, 3).unwrap();
        assert_eq!(next.stored, 1);

        // A late read of the older state must not roll the copy back.
        let late = put_transcript_in(&root, "m", "a", "/p", 10, &body, 4).unwrap();
        assert_eq!(late.stale, 1);

        let read = transcript_in(&root, "m", "a").unwrap();
        assert_eq!(read["found"], true);
        assert_eq!(read["revision"], 11);
        assert_eq!(read["transcript"], grown);
        assert_eq!(read["savedAt"], 3);

        // The thread has a row even though only its transcript was read.
        let listed = snapshot(&root, "m");
        assert_eq!(listed["threads"][0]["session"]["id"], "a");
        assert_eq!(listed["threads"][0]["transcript"]["revision"], 11);
    }

    #[test]
    fn a_missing_transcript_is_not_found_and_unsafe_names_are_refused() {
        let root = temp_root("names");
        assert_eq!(transcript_in(&root, "m", "nope").unwrap()["found"], false);
        assert!(transcript_in(&root, "../escape", "a").is_err());
        assert!(list_in(&root, Some("a/b")).is_err());
        let outcome = put_list_in(
            &root,
            "m",
            &[row("../x", "/p", 1, "x"), json!({"id": "a"})],
            1,
        )
        .unwrap();
        assert_eq!(outcome.skipped, 2);
        let outcome = put_transcript_in(&root, "m", "../x", "/p", 1, &json!({}), 1).unwrap();
        assert_eq!(outcome.skipped, 1);
        assert!(put_list_in(&root, "..", &[], 1).is_err());
        assert!(!root.join("x.json").exists());
    }

    #[test]
    fn the_switch_stops_new_copies_and_keeps_what_is_there() {
        let root = temp_root("switch");
        put_list_in(&root, "m", &[row("a", "/p", 1, "a")], 1).unwrap();
        answer_in(
            &root,
            MirrorAction::SetSettings {
                enabled: Some(false),
                excluded_projects: None,
            },
            2,
        )
        .unwrap();
        assert!(put_list_in(&root, "m", &[row("b", "/p", 2, "b")], 3).is_err());
        assert!(put_transcript_in(&root, "m", "a", "/p", 2, &json!({}), 3).is_err());
        // What was copied before can still be read.
        let listed = list_in(&root, None).unwrap();
        assert_eq!(listed["enabled"], false);
        assert_eq!(listed["hosts"]["m"]["threads"].as_array().unwrap().len(), 1);

        set_settings(&root, Some(true), None).unwrap();
        assert!(put_list_in(&root, "m", &[row("b", "/p", 2, "b")], 4).is_ok());
    }

    #[test]
    fn a_project_the_user_opted_out_of_is_not_copied() {
        let root = temp_root("optout");
        set_settings(
            &root,
            None,
            Some(vec![
                "/work/private/".to_owned(),
                " ".to_owned(),
                "/work/private".to_owned(),
            ]),
        )
        .unwrap();
        assert_eq!(read_settings(&root).excluded_projects, ["/work/private"]);
        let outcome = put_list_in(
            &root,
            "m",
            &[
                row("a", "/work/private", 1, "a"),
                row("b", "/work/private/sub/dir", 1, "b"),
                row("c", "/work/privateer", 1, "c"),
                row("d", "/work/open", 1, "d"),
            ],
            1,
        )
        .unwrap();
        assert_eq!((outcome.stored, outcome.excluded), (2, 2));
        let transcript =
            put_transcript_in(&root, "m", "a", "/work/private", 5, &json!({}), 1).unwrap();
        assert_eq!(transcript.excluded, 1);
        let ids: Vec<String> = snapshot(&root, "m")["threads"]
            .as_array()
            .unwrap()
            .iter()
            .map(|thread| thread["session"]["id"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&"c".to_owned()) && ids.contains(&"d".to_owned()));
    }

    #[test]
    fn each_machine_is_mirrored_under_its_own_folder() {
        let root = temp_root("hosts");
        put_list_in(&root, "laptop", &[row("a", "/p", 1, "a")], 1).unwrap();
        put_list_in(
            &root,
            "tower",
            &[row("a", "/p", 9, "same id, other machine")],
            1,
        )
        .unwrap();
        let all = list_in(&root, None).unwrap();
        assert_eq!(
            all["hosts"]["laptop"]["threads"][0]["session"]["title"],
            "a"
        );
        assert_eq!(
            all["hosts"]["tower"]["threads"][0]["session"]["title"],
            "same id, other machine"
        );
        let status = status_in(&root).unwrap();
        assert_eq!(status["hosts"]["laptop"]["threads"], 1);
        assert_eq!(status["hosts"]["tower"]["transcripts"], 0);
    }

    #[test]
    fn a_broken_index_reads_as_empty_instead_of_failing_the_app() {
        let root = temp_root("broken");
        put_list_in(&root, "m", &[row("a", "/p", 1, "a")], 1).unwrap();
        std::fs::write(root.join("m/index.json"), b"{ not json").unwrap();
        assert_eq!(snapshot(&root, "m")["threads"].as_array().unwrap().len(), 0);
        // The next write keeps the unreadable file instead of replacing the only copy.
        put_list_in(&root, "m", &[row("b", "/p", 2, "b")], 2).unwrap();
        assert_eq!(
            std::fs::read(root.join("m/index.json.broken")).unwrap(),
            b"{ not json"
        );
    }
}
