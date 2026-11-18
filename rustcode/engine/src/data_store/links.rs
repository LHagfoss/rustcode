//! Thread links: the orchestration book the engine and the app share, and the links the engine runs.

use super::*;

// ---------------------------------------------------------------- thread links

/// Finished links the book keeps, and fan-outs: the limits the app prunes to
/// (`prune` in `state/orchestration.ts`), so a window that saves reads back
/// what it wrote.
pub(super) const KEPT_FINISHED_LINKS: usize = 50;
pub(super) const KEPT_FAN_OUTS: usize = 20;

/// What `orchestration_state` holds beside the links, under the names the book uses.
pub(super) const BOOK_STATE_KEYS: [&str; 3] = ["grants", "counters", "fanOuts"];

pub(super) fn is_active_link(state: Option<&str>) -> bool {
    matches!(state, Some("queued" | "working" | "needs_user"))
}

pub(super) fn empty_book() -> Value {
    json!({ "links": [], "grants": {}, "counters": {}, "fanOuts": [] })
}

/// Keys that are an object in every book; the rest (`fanOuts`) is a list.
pub(super) fn empty_state(key: &str) -> Value {
    if key == "fanOuts" {
        json!([])
    } else {
        json!({})
    }
}

/// The links the book keeps: every active one, and the newest
/// [`KEPT_FINISHED_LINKS`] finished ones, in the order they came.
pub(super) fn kept_links(links: &[Value]) -> Vec<Value> {
    let mut finished: Vec<&Value> = links
        .iter()
        .filter(|link| !is_active_link(link.get("state").and_then(Value::as_str)))
        .collect();
    finished.sort_by_key(|link| {
        std::cmp::Reverse(link.get("createdAt").and_then(Value::as_i64).unwrap_or(0))
    });
    let keep: HashSet<&str> = finished
        .iter()
        .take(KEPT_FINISHED_LINKS)
        .filter_map(|link| link.get("id").and_then(Value::as_str))
        .collect();
    links
        .iter()
        .filter(|link| {
            is_active_link(link.get("state").and_then(Value::as_str))
                || link
                    .get("id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| keep.contains(id))
        })
        .cloned()
        .collect()
}

/// The fan-outs that still have a link in the book, at most [`KEPT_FAN_OUTS`].
pub(super) fn kept_fan_outs(book: &serde_json::Map<String, Value>, links: &[Value]) -> Vec<Value> {
    let kept: HashSet<&str> = links
        .iter()
        .filter_map(|link| link.get("id").and_then(Value::as_str))
        .collect();
    let live = |fan_out: &Value| match fan_out.get("links").and_then(Value::as_array) {
        Some(ids) => ids
            .iter()
            .filter_map(Value::as_str)
            .any(|id| kept.contains(id)),
        None => false,
    };
    let mut out: Vec<Value> = book
        .get("fanOuts")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter(|fan_out| live(fan_out))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let extra = out.len().saturating_sub(KEPT_FAN_OUTS);
    out.drain(..extra);
    out
}

// ------------------------------------------------- links the engine runs

/// Links whose turn this process is running right now (`start_session`,
/// `message_session` and `fan_out` with no app attached,
/// `orchestration/local_threads.rs`). They carry `"engine": true` in the book,
/// and while one is live the app's whole-book save cannot take it over: the
/// app that attaches mid-turn did not start it and must not interrupt it.
pub(super) static ENGINE_LIVE: LazyLock<Mutex<HashSet<String>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

pub(super) fn engine_live() -> std::sync::MutexGuard<'static, HashSet<String>> {
    ENGINE_LIVE
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

/// This process now runs the turn `link` waits on.
pub fn engine_link_started(link: &str) {
    engine_live().insert(link.to_owned());
}

/// That turn ended (or could not start): the link is settled in the book.
pub fn engine_link_ended(link: &str) {
    engine_live().remove(link);
}

/// The links this process is running a turn for.
pub fn engine_live_links() -> Vec<String> {
    let mut live: Vec<String> = engine_live().iter().cloned().collect();
    live.sort();
    live
}

/// How often the engine stamps the links it runs, and how long without a stamp
/// before another reader takes such a link for one whose engine died.
pub const ENGINE_BEAT_EVERY: Duration = Duration::from_secs(10);
pub(super) const ENGINE_BEAT_STALE_MS: i64 = 45_000;

pub(super) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

/// Stamp every link this process runs with the time, so a reader in another
/// process (or this one after a restart) can tell a running engine's link from
/// one whose engine died with the turn still `working`.
pub fn beat_engine_links() {
    // Notes a sleep before any sweep does (`clock_recently_jumped`).
    clock_recently_jumped();
    let at = now_ms();
    for id in engine_live_links() {
        let _ = change_link(&id, |mut link| {
            link["engineBeat"] = json!(at);
            link
        });
    }
}

/// Names this process among the engines that run links: a link carries the
/// owner that opened it (`engineOwner`), so a reader can tell its own links
/// from another process's (the app's engine and the background server share
/// one store).
pub fn engine_owner() -> &'static str {
    static OWNER: LazyLock<String> = LazyLock::new(|| {
        format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or(0)
        )
    });
    &OWNER
}

/// When the engine last stamped `link` (or opened it).
pub(super) fn link_stamp(link: &Value) -> i64 {
    link.get("engineBeat")
        .and_then(Value::as_i64)
        .or_else(|| link.get("createdAt").and_then(Value::as_i64))
        .unwrap_or(0)
}

/// Whether an engine runs this link's turn right now: this process does and it
/// is in its live set, or whichever process does has stamped it within
/// [`ENGINE_BEAT_STALE_MS`]. Only an active `engine: true` link can be live.
pub(super) fn link_running(link: &Value, live: &HashSet<String>, at: i64) -> bool {
    if link.get("engine").and_then(Value::as_bool) != Some(true)
        || !is_active_link(link.get("state").and_then(Value::as_str))
    {
        return false;
    }
    if link_id(link).is_some_and(|id| live.contains(id)) {
        return true;
    }
    // Another process's stamp counts; so does a stamp of this one, whose live
    // set no longer has the link only when its turn just ended.
    at - link_stamp(link) <= ENGINE_BEAT_STALE_MS
}

/// An engine's link still `working` that no engine stamps any more: the engine
/// died mid-turn. A link this process runs, or any process stamped lately, never
/// counts.
pub(super) fn orphaned_link(link: &Value, live: &HashSet<String>, at: i64) -> bool {
    link.get("engine").and_then(Value::as_bool) == Some(true)
        && is_active_link(link.get("state").and_then(Value::as_str))
        && !link_running(link, live, at)
}

/// What the clock did between two looks, kept to tell a laptop's sleep from
/// time passing. The monotonic clock (`Instant`) stops while the machine
/// sleeps and the wall clock does not, so a wall interval much longer than
/// the monotonic one is a sleep or a clock jump: for a while after it, the
/// other engines have not had the chance to stamp their links, and nothing is
/// swept.
pub(super) struct ClockWatch {
    looked: Instant,
    wall: i64,
    quiet_until: Option<Instant>,
}

/// A wall interval this much longer than the monotonic one is a jump.
pub(super) const CLOCK_JUMP_MS: i64 = 5_000;
/// How long after a jump the sweep waits: two beats and some, so every live
/// engine has stamped again.
pub(super) const AFTER_JUMP_QUIET: Duration = Duration::from_secs(25);

impl ClockWatch {
    pub(super) fn new() -> Self {
        Self {
            looked: Instant::now(),
            wall: now_ms(),
            quiet_until: None,
        }
    }

    /// Look at the clocks and say whether a sweep should wait. `since_monotonic`
    /// and `since_wall` are the intervals since the last look.
    pub(super) fn observe(
        &mut self,
        since_monotonic: Duration,
        since_wall_ms: i64,
        now: Instant,
    ) -> bool {
        let gap = since_wall_ms - since_monotonic.as_millis() as i64;
        if gap.abs() >= CLOCK_JUMP_MS {
            self.quiet_until = Some(now + AFTER_JUMP_QUIET);
        }
        self.quiet_until.is_some_and(|until| now < until)
    }

    pub(super) fn look(&mut self) -> bool {
        let now = Instant::now();
        let wall = now_ms();
        let quiet = self.observe(now - self.looked, wall - self.wall, now);
        self.looked = now;
        self.wall = wall;
        quiet
    }
}

pub(super) fn clock_watch() -> &'static Mutex<ClockWatch> {
    static WATCH: LazyLock<Mutex<ClockWatch>> = LazyLock::new(|| Mutex::new(ClockWatch::new()));
    &WATCH
}

/// Whether the clocks say the machine just woke or jumped, which also counts
/// as a look for the next call. The beat loop looks too, so a long gap shows up
/// in whichever of them runs first after waking.
pub fn clock_recently_jumped() -> bool {
    clock_watch()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .look()
}

/// The link as the app would leave it after a restart.
pub(super) fn interrupted_orphan(mut link: Value, at: i64) -> Value {
    link["state"] = json!("interrupted");
    link["settledAt"] = json!(at);
    let mut events = link
        .get("events")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    events.push(json!({ "kind": "interrupted", "at": at }));
    link["events"] = Value::Array(events);
    link
}

/// Interrupt the links of engines that died mid-turn, in the default store.
pub fn interrupt_orphaned_links() -> usize {
    data_path()
        .filter(|path| path.is_file())
        .and_then(|path| interrupt_orphaned_links_at(&path).ok())
        .unwrap_or(0)
}

/// [`interrupt_orphaned_links`] against one store file.
pub fn interrupt_orphaned_links_at(path: &Path) -> Result<usize, String> {
    // Nothing is swept right after a sleep or a clock jump, until every engine
    // has had the chance to stamp again; this process stamps its own links
    // first, so a reader in another process finds them fresh. Its own live
    // links are never swept in any case.
    if clock_recently_jumped() {
        beat_engine_links();
        return Ok(0);
    }
    sweep_orphaned_links(path, now_ms())
}

pub(super) fn sweep_orphaned_links(path: &Path, at: i64) -> Result<usize, String> {
    let connection = open(path)?;
    let live = engine_live().clone();
    // Look first without the write lock: almost every call finds nothing.
    let stored = read_links(&connection)?;
    if !stored.iter().any(|link| orphaned_link(link, &live, at)) {
        return Ok(0);
    }
    let transaction = immediate(&connection)?;
    let mut changed = 0;
    for link in read_links(&transaction)? {
        if orphaned_link(&link, &live, at) {
            write_link(&transaction, &interrupted_orphan(link, at))?;
            changed += 1;
        }
    }
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(changed)
}

pub(super) fn is_final_link(state: Option<&str>) -> bool {
    matches!(state, Some("done" | "failed" | "cancelled"))
}

pub(super) fn link_id(link: &Value) -> Option<&str> {
    link.get("id").and_then(Value::as_str)
}

/// The app's whole-book save, kept from undoing what the engine wrote to a
/// link it runs: a live one stays as the engine has it (the app's copy of it is
/// from before the turn ended, or `interrupted` because the app attached
/// mid-turn), and a settled one is not taken back to a state that is not
/// final. A live link the app's copy lacks (it loaded the book before the
/// engine opened the link) is kept too.
pub(super) fn protect_engine_links(
    mut incoming: Vec<Value>,
    stored: &[Value],
    live: &HashSet<String>,
    at_ms: i64,
) -> Vec<Value> {
    for kept in stored {
        let Some(id) = link_id(kept) else {
            continue;
        };
        if kept.get("engine").and_then(Value::as_bool) != Some(true) {
            continue;
        }
        let state = kept.get("state").and_then(Value::as_str);
        let position = incoming.iter().position(|link| link_id(link) == Some(id));
        match position {
            Some(at) => {
                let theirs = incoming[at].get("state").and_then(Value::as_str);
                let settled_here = is_final_link(state) && !is_final_link(theirs);
                if link_running(kept, live, at_ms) || settled_here {
                    incoming[at] = kept.clone();
                }
            }
            // The app's copy predates the link (another window, an older load):
            // dropping it would lose a settled answer before its caller reads it.
            None => incoming.push(kept.clone()),
        }
    }
    incoming
}

/// A counter only goes up: the larger of the app's and the store's, so a link
/// id the engine opened is never handed out again.
pub(super) fn merge_counters(incoming: Option<&Value>, stored: &Value) -> Value {
    let mut out = incoming
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if let Some(stored) = stored.as_object() {
        for (who, count) in stored {
            let theirs = out.get(who).and_then(Value::as_u64).unwrap_or(0);
            let ours = count.as_u64().unwrap_or(0);
            out.insert(who.clone(), json!(theirs.max(ours)));
        }
    }
    Value::Object(out)
}

pub(super) fn read_links(transaction: &Connection) -> Result<Vec<Value>, String> {
    let mut statement = transaction
        .prepare_cached("SELECT body FROM thread_links ORDER BY started_at, id")
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?;
    let mut links = Vec::new();
    for body in rows {
        let body = body.map_err(|error| error.to_string())?;
        if let Some(link) = serde_json::from_str::<Value>(&body)
            .ok()
            .filter(Value::is_object)
        {
            links.push(link);
        }
    }
    Ok(links)
}

pub(super) fn read_state(transaction: &Transaction<'_>, key: &str) -> Result<Value, String> {
    let body: Option<String> = transaction
        .query_row(
            "SELECT body FROM orchestration_state WHERE key = ?1",
            params![key],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    Ok(body
        .and_then(|body| serde_json::from_str::<Value>(&body).ok())
        .unwrap_or_else(|| empty_state(key)))
}

pub(super) fn write_state(
    transaction: &Transaction<'_>,
    key: &str,
    value: &Value,
) -> Result<(), String> {
    transaction
        .execute(
            "INSERT OR REPLACE INTO orchestration_state (key, body) VALUES (?1, ?2)",
            params![key, value.to_string()],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// One link's row, as `save_link_book_at` writes it.
pub(super) fn write_link(transaction: &Transaction<'_>, link: &Value) -> Result<(), String> {
    let Some(id) = link_id(link) else {
        return Err("a link needs an id".to_owned());
    };
    transaction
        .execute(
            "INSERT OR REPLACE INTO thread_links
                (id, orchestrator, target, state, started_at, settled_at, harness, files_changed, body)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                id,
                link.get("orchestrator").and_then(Value::as_str),
                link.get("target").and_then(Value::as_str),
                link.get("state").and_then(Value::as_str),
                link.get("createdAt").and_then(Value::as_i64),
                link.get("settledAt").and_then(Value::as_i64),
                link.get("harness").and_then(Value::as_str),
                link.get("filesChanged").and_then(Value::as_i64),
                link.to_string(),
            ],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub(super) fn immediate(connection: &Connection) -> Result<Transaction<'_>, String> {
    Transaction::new_unchecked(connection, TransactionBehavior::Immediate)
        .map_err(|error| error.to_string())
}

/// Open a link for `orchestrator` in one transaction: take its next counter
/// value, let `make` build the link from it, and add the link. Nothing but this
/// link is written, so the app's links and grants are never read back and
/// replaced.
pub fn open_link(orchestrator: &str, make: impl FnOnce(u64) -> Value) -> Result<Value, String> {
    let path = data_path().ok_or_else(|| "no config directory".to_owned())?;
    open_link_at(&path, orchestrator, make)
}

/// [`open_link`] against one store file.
pub fn open_link_at(
    path: &Path,
    orchestrator: &str,
    make: impl FnOnce(u64) -> Value,
) -> Result<Value, String> {
    let connection = open(path)?;
    let transaction = immediate(&connection)?;
    let mut counters = read_state(&transaction, "counters")?;
    let next = counters
        .get(orchestrator)
        .and_then(Value::as_u64)
        .unwrap_or(0)
        + 1;
    counters[orchestrator] = json!(next);
    let link = make(next);
    write_link(&transaction, &link)?;
    write_state(&transaction, "counters", &counters)?;
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(link)
}

/// Change one link in place, in one transaction: `change` gets the stored link
/// and answers the new one. False when no such link is stored.
pub fn change_link(id: &str, change: impl FnOnce(Value) -> Value) -> Result<bool, String> {
    let path = data_path().ok_or_else(|| "no config directory".to_owned())?;
    change_link_at(&path, id, change)
}

/// [`change_link`] against one store file.
pub fn change_link_at(
    path: &Path,
    id: &str,
    change: impl FnOnce(Value) -> Value,
) -> Result<bool, String> {
    let connection = open(path)?;
    let transaction = immediate(&connection)?;
    let body: Option<String> = transaction
        .query_row(
            "SELECT body FROM thread_links WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some(link) = body
        .and_then(|body| serde_json::from_str::<Value>(&body).ok())
        .filter(Value::is_object)
    else {
        return Ok(false);
    };
    write_link(&transaction, &change(link))?;
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(true)
}

/// Add one fan-out group to the book, in one transaction.
pub fn add_fan_out(group: Value) -> Result<(), String> {
    let path = data_path().ok_or_else(|| "no config directory".to_owned())?;
    let connection = open(&path)?;
    let transaction = immediate(&connection)?;
    let mut groups = read_state(&transaction, "fanOuts")?;
    match groups.as_array_mut() {
        Some(list) => list.push(group),
        None => groups = json!([group]),
    }
    write_state(&transaction, "fanOuts", &groups)?;
    transaction.commit().map_err(|error| error.to_string())
}

/// Saves the app's link book (`LinkBook` in `state/orchestration.ts`): the
/// links in `thread_links`, grants, counters and fan-outs in
/// `orchestration_state`. One transaction replaces the whole book, so a second
/// window saving cannot leave half of each behind. Each link's `body` is the
/// whole record, so a field this release does not know survives a round trip.
///
/// The book is bounded here, not trusted: the same kept links and fan-outs the
/// app prunes to. And it is merged, not trusted blindly, with what the engine
/// itself wrote (`protect_engine_links`, `merge_counters`): the app's copy of a
/// link the engine runs, or of a counter the engine advanced, may be older.
pub fn save_link_book(book: &Value) -> Result<(), String> {
    let path = data_path().ok_or_else(|| "no config directory".to_owned())?;
    save_link_book_at(&path, book)
}

/// [`save_link_book`] against one store file, so a test can use a temporary one.
pub fn save_link_book_at(path: &Path, book: &Value) -> Result<(), String> {
    let object = book
        .as_object()
        .ok_or_else(|| "the link book must be a JSON object".to_owned())?;
    let connection = open(path)?;
    // Read and replace under one write lock: the engine's own writes wait.
    let transaction = immediate(&connection)?;
    let stored_links = read_links(&transaction)?;
    let stored_counters = read_state(&transaction, "counters")?;
    let live = engine_live().clone();
    let incoming = object
        .get("links")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let protected = protect_engine_links(incoming, &stored_links, &live, now_ms());
    let links = kept_links(&protected);
    let mut book_state = object.clone();
    // A fan-out of a link the engine runs is kept with it.
    let mut fan_outs: Vec<Value> = object
        .get("fanOuts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let stored_fan_outs = read_state(&transaction, "fanOuts")?;
    for group in stored_fan_outs
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let known = fan_outs
            .iter()
            .any(|seen| seen.get("id") == group.get("id"));
        let runs_live = group
            .get("links")
            .and_then(Value::as_array)
            .is_some_and(|ids| {
                ids.iter()
                    .filter_map(Value::as_str)
                    .any(|id| live.contains(id))
            });
        if !known && runs_live {
            fan_outs.push(group.clone());
        }
    }
    book_state.insert("fanOuts".to_owned(), Value::Array(fan_outs));
    let fan_outs = kept_fan_outs(&book_state, &links);
    transaction
        .execute("DELETE FROM thread_links", [])
        .map_err(|error| error.to_string())?;
    for link in &links {
        if link_id(link).is_some() {
            write_link(&transaction, link)?;
        }
    }
    for key in BOOK_STATE_KEYS {
        let value = match key {
            "fanOuts" => Value::Array(fan_outs.clone()),
            "counters" => merge_counters(object.get("counters"), &stored_counters),
            _ => object.get(key).cloned().unwrap_or_else(|| empty_state(key)),
        };
        write_state(&transaction, key, &value)?;
    }
    transaction.commit().map_err(|error| error.to_string())
}

/// Reads the link book back as `{ "stored": bool, "book": LinkBook }`.
/// `stored` is false when nothing was ever saved, so the app migrates its own
/// copy in once rather than replace a book it does not have with an empty one.
pub fn load_link_book() -> Result<Value, String> {
    let path = data_path().ok_or_else(|| "no config directory".to_owned())?;
    load_link_book_at(&path)
}

/// [`load_link_book`] against one store file, so a test can use a temporary one.
pub fn load_link_book_at(path: &Path) -> Result<Value, String> {
    if !path.is_file() {
        return Ok(json!({ "stored": false, "book": empty_book() }));
    }
    // An engine that died mid-turn leaves its link `working`: settle it before
    // reading, so every reader (the app, `wait_session`, the slot count) sees
    // it interrupted.
    let _ = interrupt_orphaned_links_at(path);
    let connection = open(path)?;
    let mut statement = connection
        .prepare(
            "SELECT id, orchestrator, target, state, started_at, settled_at, harness, files_changed, body
             FROM thread_links ORDER BY started_at, id",
        )
        .map_err(|error| error.to_string())?;
    let mut rows = statement.query([]).map_err(|error| error.to_string())?;
    let mut links: Vec<Value> = Vec::new();
    while let Some(row) = rows.next().map_err(|error| error.to_string())? {
        let body: String = row.get(8).map_err(|error| error.to_string())?;
        // The body is the whole record; the columns only back it up.
        let parsed = serde_json::from_str::<Value>(&body)
            .ok()
            .filter(Value::is_object);
        links.push(parsed.unwrap_or_else(|| {
            json!({
                "id": row.get::<_, Option<String>>(0).ok().flatten(),
                "orchestrator": row.get::<_, Option<String>>(1).ok().flatten(),
                "target": row.get::<_, Option<String>>(2).ok().flatten(),
                "state": row.get::<_, Option<String>>(3).ok().flatten(),
                "createdAt": row.get::<_, Option<i64>>(4).ok().flatten(),
                "settledAt": row.get::<_, Option<i64>>(5).ok().flatten(),
                "harness": row.get::<_, Option<String>>(6).ok().flatten(),
                "filesChanged": row.get::<_, Option<i64>>(7).ok().flatten(),
            })
        }));
    }
    drop(rows);
    drop(statement);
    let mut state = serde_json::Map::new();
    let mut stored = !links.is_empty();
    let mut statement = connection
        .prepare("SELECT key, body FROM orchestration_state")
        .map_err(|error| error.to_string())?;
    let mut rows = statement.query([]).map_err(|error| error.to_string())?;
    while let Some(row) = rows.next().map_err(|error| error.to_string())? {
        let key: String = row.get(0).map_err(|error| error.to_string())?;
        let body: String = row.get(1).map_err(|error| error.to_string())?;
        let value = serde_json::from_str::<Value>(&body).unwrap_or(Value::Null);
        state.insert(key, value);
        stored = true;
    }
    let mut book = empty_book();
    if let Some(object) = book.as_object_mut() {
        object.insert("links".to_owned(), Value::Array(links));
        for key in BOOK_STATE_KEYS {
            let value = state.remove(key).unwrap_or_else(|| empty_state(key));
            object.insert(key.to_owned(), value);
        }
    }
    // The links this process runs a turn for: the app that loads the book must
    // not take them for severed ones and interrupt them.
    // The links an engine runs the turn of, whichever process it is: this
    // one's live set, and any other's stamped lately.
    let live = engine_live().clone();
    let at = now_ms();
    let running: Vec<&str> = book["links"]
        .as_array()
        .map(|links| {
            links
                .iter()
                .filter(|link| link_running(link, &live, at))
                .filter_map(link_id)
                .collect()
        })
        .unwrap_or_default();
    Ok(json!({ "stored": stored, "book": book, "engineLive": running }))
}
