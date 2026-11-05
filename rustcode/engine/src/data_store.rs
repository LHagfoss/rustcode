//! A SQLite store of what the threads did, for questions the folders cannot
//! answer cheaply: tokens per harness and model per day, how many turns and
//! tool calls a thread spent, which tools failed.
//!
//! **Metadata only.** No message text, tool arguments or tool output is kept
//! here; those stay in each thread's `history.json`. Because the per-turn
//! token deltas of an external harness cannot be recovered once its session
//! is over, this file is not a disposable cache like `index.sqlite`: it is
//! never deleted or rebuilt on a read error.
//!
//! Writes never fail a turn: [`record_turn`] logs and returns. Reads go
//! through [`query`], a read-only connection with a two-second cap, a single
//! statement, no `ATTACH` and at most [`MAX_ROWS`] rows.
//!
//! The reference point for the schema is `docs/data-store.md`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rusqlite::limits::Limit;
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde_json::{Value, json};

/// In the configuration directory, beside `index.sqlite`.
pub const DATA_FILE: &str = "data.sqlite";

/// Most rows one `data query` returns.
pub const MAX_ROWS: usize = 1000;

/// Wall-clock cap for one `data query`, enforced by the progress handler.
const QUERY_TIMEOUT: Duration = Duration::from_secs(2);

const MIGRATIONS: &[(i64, &str, &str)] = &[
    (
        1,
        "threads_turns_and_tool_calls",
        "CREATE TABLE threads (
        id TEXT PRIMARY KEY,
        title TEXT NOT NULL DEFAULT '',
        project TEXT,
        cwd TEXT,
        harness TEXT,
        model TEXT,
        effort TEXT,
        created_at TEXT,
        updated_at TEXT
    );
    CREATE TABLE turns (
        id INTEGER PRIMARY KEY,
        thread_id TEXT NOT NULL,
        started_at TEXT NOT NULL,
        finished_at TEXT,
        harness TEXT,
        model TEXT,
        input_tokens INTEGER,
        output_tokens INTEGER,
        cached_tokens INTEGER,
        cache_write_tokens INTEGER,
        cost_usd REAL,
        tool_calls INTEGER NOT NULL DEFAULT 0,
        status TEXT NOT NULL,
        UNIQUE(thread_id, started_at)
    );
    CREATE INDEX turns_day ON turns(started_at);
    CREATE INDEX turns_thread ON turns(thread_id);
    CREATE TABLE tool_calls (
        id INTEGER PRIMARY KEY,
        thread_id TEXT NOT NULL,
        turn_id INTEGER,
        tool_name TEXT NOT NULL,
        success INTEGER,
        exit_code INTEGER,
        at TEXT
    );
    CREATE INDEX tool_calls_thread ON tool_calls(thread_id);
    CREATE TABLE store_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
    CREATE VIEW daily_usage AS
        SELECT date(started_at) AS day, harness, model,
               SUM(input_tokens) AS input_tokens,
               SUM(output_tokens) AS output_tokens,
               SUM(cached_tokens) AS cached_tokens,
               SUM(cache_write_tokens) AS cache_write_tokens,
               SUM(cost_usd) AS cost_usd,
               COUNT(*) AS turns
        FROM turns
        GROUP BY day, harness, model;",
    ),
    (
        2,
        "thread_links_and_orchestration_state",
        "CREATE TABLE thread_links (
        id TEXT PRIMARY KEY,
        orchestrator TEXT,
        target TEXT,
        state TEXT,
        started_at INTEGER,
        settled_at INTEGER,
        harness TEXT,
        files_changed INTEGER,
        body TEXT NOT NULL
    );
    CREATE INDEX thread_links_target ON thread_links(target);
    CREATE TABLE orchestration_state (key TEXT PRIMARY KEY, body TEXT NOT NULL);",
    ),
];

/// One turn's token totals. Each is optional: a harness that reports no count
/// stores NULL and the number is never guessed. `cached` is the prompt cache
/// that was read; `cache_write` the prompt cache that was written.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct TokenTotals {
    pub input: Option<i64>,
    pub output: Option<i64>,
    pub cached: Option<i64>,
    pub cache_write: Option<i64>,
}

impl TokenTotals {
    pub fn is_empty(&self) -> bool {
        self.input.is_none()
            && self.output.is_none()
            && self.cached.is_none()
            && self.cache_write.is_none()
    }

    /// Adds another count to this one, keeping NULL only when both are NULL.
    pub fn add(&mut self, other: &TokenTotals) {
        self.input = sum(self.input, other.input);
        self.output = sum(self.output, other.output);
        self.cached = sum(self.cached, other.cached);
        self.cache_write = sum(self.cache_write, other.cache_write);
    }
}

fn sum(left: Option<i64>, right: Option<i64>) -> Option<i64> {
    match (left, right) {
        (None, None) => None,
        (left, right) => Some(left.unwrap_or(0) + right.unwrap_or(0)),
    }
}

/// A session's cumulative usage: token totals, the cost the harness reported
/// for them (USD), and when its last entry was.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct SessionUsage {
    pub tokens: TokenTotals,
    pub cost_usd: Option<f64>,
    /// The last entry's timestamp, RFC 3339 UTC, when the source carries one.
    pub at: Option<String>,
}

impl SessionUsage {
    /// The growth over `baseline`, which is what one turn spent. A counter
    /// that went backwards means a new session log (a harness switch or a
    /// restarted harness): its whole total is this turn's, not a negative
    /// number. A counter that is absent stays absent.
    pub fn growth(self, baseline: SessionUsage) -> SessionUsage {
        let grow = |now: Option<i64>, base: Option<i64>| match (now, base) {
            (None, _) => None,
            (Some(now), Some(base)) if now >= base => Some(now - base),
            (Some(now), _) => Some(now),
        };
        let cost = match (self.cost_usd, baseline.cost_usd) {
            (None, _) => None,
            (Some(now), Some(base)) if now >= base => Some(now - base),
            (Some(now), _) => Some(now),
        };
        SessionUsage {
            tokens: TokenTotals {
                input: grow(self.tokens.input, baseline.tokens.input),
                output: grow(self.tokens.output, baseline.tokens.output),
                cached: grow(self.tokens.cached, baseline.tokens.cached),
                cache_write: grow(self.tokens.cache_write, baseline.tokens.cache_write),
            },
            cost_usd: cost,
            at: self.at,
        }
    }
}

/// The cumulative totals in a harness `usage` value, when it reports them.
///
/// RustCode's harnesses put session totals in `_meta.rustcode` (see
/// `acp/usage.rs`); the standard `usage_update` fields only describe the
/// context window, so a harness that sends no `_meta` yields `None` rather
/// than a made-up number.
pub fn totals(usage: &Value) -> Option<TokenTotals> {
    let meta = usage.get("_meta")?.get("rustcode")?;
    let count = |key: &str| meta.get(key).and_then(Value::as_i64);
    let totals = TokenTotals {
        input: count("inputTokens"),
        output: count("outputTokens"),
        cached: count("cachedTokens"),
        cache_write: count("cacheWriteTokens"),
    };
    (!totals.is_empty()).then_some(totals)
}

/// The cost a `usage` value reports, in USD. A cost in another currency is
/// not converted and not stored: an approximate dollar figure is worse than
/// a blank one.
pub fn cost_usd(usage: &Value) -> Option<f64> {
    let cost = usage.get("cost")?;
    let currency = cost.get("currency").and_then(Value::as_str)?;
    if !currency.eq_ignore_ascii_case("usd") {
        return None;
    }
    cost.get("amount")
        .and_then(Value::as_f64)
        .filter(|amount| amount.is_finite())
}

/// The growth from `before` to `now`, which is what one turn spent. Counters
/// that went backwards (a harness restarted its own session) yield zero for
/// the missing side rather than a negative number.
pub fn delta(now: TokenTotals, before: Option<TokenTotals>) -> TokenTotals {
    let Some(before) = before else {
        return now;
    };
    let subtract = |now: Option<i64>, before: Option<i64>| match (now, before) {
        (Some(now), Some(before)) => (now > before).then_some(now - before),
        (now, _) => now,
    };
    TokenTotals {
        input: subtract(now.input, before.input),
        output: subtract(now.output, before.output),
        cached: subtract(now.cached, before.cached),
        cache_write: subtract(now.cache_write, before.cache_write),
    }
}

/// A thread row, as far as the store is allowed to care.
#[derive(Debug, Clone, Default)]
pub struct ThreadRow {
    pub id: String,
    pub title: Option<String>,
    pub project: Option<String>,
    pub cwd: Option<String>,
    pub harness: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

/// One tool call, without its arguments or output.
#[derive(Debug, Clone, Default)]
pub struct ToolCallRow {
    pub tool_name: String,
    pub success: Option<bool>,
    pub exit_code: Option<i64>,
    pub at: Option<String>,
}

/// One turn to record. `started_at` and `finished_at` are RFC 3339 in UTC,
/// which sorts and groups by day; `(thread_id, started_at)` is the identity,
/// so recording the same turn twice updates it instead of duplicating it.
#[derive(Debug, Clone, Default)]
pub struct TurnRow {
    pub thread_id: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub harness: Option<String>,
    pub model: Option<String>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cached_tokens: Option<i64>,
    pub cache_write_tokens: Option<i64>,
    pub cost_usd: Option<f64>,
    pub status: String,
    pub tool_calls: Vec<ToolCallRow>,
}

/// What a backfill found.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BackfillReport {
    pub threads: usize,
    pub turns: usize,
    pub errors: usize,
}

/// The columns and rows of one read-only query.
#[derive(Debug, Clone, serde::Serialize)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Value>,
    pub truncated: bool,
}

/// How `thread_usage` groups its rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupBy {
    Harness,
    Model,
    Day,
    Project,
    Thread,
}

impl GroupBy {
    pub const ALL: [&'static str; 5] = ["harness", "model", "day", "project", "thread"];

    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "" | "harness" => Some(Self::Harness),
            "model" => Some(Self::Model),
            "day" => Some(Self::Day),
            "project" => Some(Self::Project),
            "thread" => Some(Self::Thread),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Harness => "harness",
            Self::Model => "model",
            Self::Day => "day",
            Self::Project => "project",
            Self::Thread => "thread",
        }
    }
}

/// A day for `thread_usage`'s `since`: `YYYY-MM-DD` or `YYYY-MM`.
pub fn parse_since(value: &str) -> Result<String, String> {
    let value = value.trim();
    let day = if value.len() == 7 {
        chrono::NaiveDate::parse_from_str(&format!("{value}-01"), "%Y-%m-%d")
    } else {
        chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
    };
    day.map(|day| {
        if value.len() == 7 {
            value.to_owned()
        } else {
            day.format("%Y-%m-%d").to_string()
        }
    })
    .map_err(|_| format!("since must be a day or month, YYYY-MM-DD or YYYY-MM, not {value:?}"))
}

/// Tokens and reported cost over `turns`, grouped as asked, for `thread_usage`.
/// Read-only, like [`query`], and bounded to [`MAX_ROWS`] groups. A cost is
/// only ever the USD a harness reported: nothing here prices a request.
pub fn usage(
    since: Option<&str>,
    group_by: GroupBy,
    project: Option<&str>,
) -> Result<Value, String> {
    let path = data_path().ok_or_else(|| "no config directory".to_owned())?;
    usage_at(&path, since, group_by, project)
}

/// [`usage`] against one store file, so a test can read a temporary one.
pub fn usage_at(
    path: &Path,
    since: Option<&str>,
    group_by: GroupBy,
    project: Option<&str>,
) -> Result<Value, String> {
    // The key column is the one grouped on, so a row says what it is.
    let (key, group) = match group_by {
        GroupBy::Harness => ("COALESCE(t.harness, 'unknown') AS harness", "1"),
        GroupBy::Model => (
            "COALESCE(t.harness, 'unknown') AS harness, COALESCE(t.model, '') AS model",
            "1, 2",
        ),
        GroupBy::Day => ("date(t.started_at) AS day", "1"),
        GroupBy::Project => ("COALESCE(th.project, th.cwd, '') AS project", "1"),
        GroupBy::Thread => (
            "t.thread_id AS thread, th.title AS title, \
             COALESCE(th.project, th.cwd, '') AS project, COALESCE(th.harness, t.harness) AS harness",
            "1",
        ),
    };
    let mut sql = format!(
        "SELECT {key}, \
                SUM(t.input_tokens) AS input_tokens, SUM(t.output_tokens) AS output_tokens, \
                SUM(t.cached_tokens) AS cached_tokens, \
                SUM(t.cache_write_tokens) AS cache_write_tokens, SUM(t.cost_usd) AS cost_usd, \
                COUNT(*) AS turns, \
                SUM(CASE WHEN t.input_tokens IS NULL AND t.output_tokens IS NULL THEN 1 ELSE 0 END) \
                    AS turns_without_tokens \
         FROM turns t LEFT JOIN threads th ON th.id = t.thread_id"
    );
    let mut filters: Vec<String> = Vec::new();
    if let Some(since) = since.filter(|since| !since.trim().is_empty()) {
        // RFC 3339 in UTC sorts by string, so a day or a month both work and
        // the column's index still can.
        filters.push(format!("t.started_at >= '{}'", since.replace('\'', "")));
    }
    if let Some(project) = project.filter(|project| !project.trim().is_empty()) {
        filters.push(format!(
            "COALESCE(th.project, th.cwd, '') = '{}'",
            project.replace('\'', "''")
        ));
    }
    if !filters.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&filters.join(" AND "));
    }
    sql.push_str(&format!(
        " GROUP BY {group} \
         ORDER BY (COALESCE(SUM(t.input_tokens), 0) + COALESCE(SUM(t.output_tokens), 0)) DESC, 1 \
         LIMIT {}",
        MAX_ROWS
    ));
    let result = query_path(path, &sql, MAX_ROWS)?;
    let number = |row: &Value, key: &str| row.get(key).and_then(Value::as_f64);
    let sum = |key: &str| {
        result
            .rows
            .iter()
            .filter_map(|row| number(row, key))
            .sum::<f64>()
    };
    let any_cost = result
        .rows
        .iter()
        .any(|row| number(row, "cost_usd").is_some());
    let totals = json!({
        "input_tokens": sum("input_tokens"),
        "output_tokens": sum("output_tokens"),
        "cached_tokens": sum("cached_tokens"),
        "cache_write_tokens": sum("cache_write_tokens"),
        "cost_usd": any_cost.then(|| sum("cost_usd")),
        "turns": sum("turns"),
    });
    Ok(json!({
        "group_by": group_by.name(),
        "since": since,
        "project": project,
        "rows": result.rows,
        "totals": totals,
        "truncated": result.truncated,
        "note": "tokens are what each harness reported; cost is the USD a harness reported, never a price RustCode computed",
    }))
}

pub fn data_path() -> Option<PathBuf> {
    crate::config::get_config_dir().map(|dir| dir.join(DATA_FILE))
}

/// Opens (creating and migrating) the store. Used by writers and by the CLI's
/// one-time backfill; reads use [`query`] instead.
pub fn open(path: &Path) -> Result<Connection, String> {
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
                    "data store migration {id} is recorded as {recorded:?}, expected {name:?}"
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

// ---------------------------------------------------------------- thread links

/// Finished links the book keeps, and fan-outs: the limits the app prunes to
/// (`prune` in `state/orchestration.ts`), so a window that saves reads back
/// what it wrote.
const KEPT_FINISHED_LINKS: usize = 50;
const KEPT_FAN_OUTS: usize = 20;

/// What `orchestration_state` holds beside the links, under the names the book uses.
const BOOK_STATE_KEYS: [&str; 3] = ["grants", "counters", "fanOuts"];

fn is_active_link(state: Option<&str>) -> bool {
    matches!(state, Some("queued" | "working" | "needs_user"))
}

fn empty_book() -> Value {
    json!({ "links": [], "grants": {}, "counters": {}, "fanOuts": [] })
}

/// Keys that are an object in every book; the rest (`fanOuts`) is a list.
fn empty_state(key: &str) -> Value {
    if key == "fanOuts" {
        json!([])
    } else {
        json!({})
    }
}

/// The links the book keeps: every active one, and the newest
/// [`KEPT_FINISHED_LINKS`] finished ones, in the order they came.
fn kept_links(links: &[Value]) -> Vec<Value> {
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
fn kept_fan_outs(book: &serde_json::Map<String, Value>, links: &[Value]) -> Vec<Value> {
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

/// Saves the app's link book (`LinkBook` in `state/orchestration.ts`): the
/// links in `thread_links`, grants, counters and fan-outs in
/// `orchestration_state`. One transaction replaces the whole book, so a second
/// window saving cannot leave half of each behind. Each link's `body` is the
/// whole record, so a field this release does not know survives a round trip.
///
/// The book is bounded here, not trusted: the same kept links and fan-outs the
/// app prunes to.
pub fn save_link_book(book: &Value) -> Result<(), String> {
    let path = data_path().ok_or_else(|| "no config directory".to_owned())?;
    save_link_book_at(&path, book)
}

/// [`save_link_book`] against one store file, so a test can use a temporary one.
pub fn save_link_book_at(path: &Path, book: &Value) -> Result<(), String> {
    let object = book
        .as_object()
        .ok_or_else(|| "the link book must be a JSON object".to_owned())?;
    let links = kept_links(
        object
            .get("links")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default(),
    );
    let fan_outs = kept_fan_outs(object, &links);
    let connection = open(path)?;
    let transaction = connection
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    transaction
        .execute("DELETE FROM thread_links", [])
        .map_err(|error| error.to_string())?;
    {
        let mut insert = transaction
            .prepare_cached(
                "INSERT OR REPLACE INTO thread_links
                    (id, orchestrator, target, state, started_at, settled_at, harness, files_changed, body)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )
            .map_err(|error| error.to_string())?;
        for link in &links {
            let Some(id) = link.get("id").and_then(Value::as_str) else {
                continue;
            };
            let body = link.to_string();
            insert
                .execute(params![
                    id,
                    link.get("orchestrator").and_then(Value::as_str),
                    link.get("target").and_then(Value::as_str),
                    link.get("state").and_then(Value::as_str),
                    link.get("createdAt").and_then(Value::as_i64),
                    link.get("settledAt").and_then(Value::as_i64),
                    link.get("harness").and_then(Value::as_str),
                    link.get("filesChanged").and_then(Value::as_i64),
                    body,
                ])
                .map_err(|error| error.to_string())?;
        }
    }
    for key in BOOK_STATE_KEYS {
        let value = if key == "fanOuts" {
            Value::Array(fan_outs.clone())
        } else {
            object.get(key).cloned().unwrap_or_else(|| empty_state(key))
        };
        transaction
            .execute(
                "INSERT OR REPLACE INTO orchestration_state (key, body) VALUES (?1, ?2)",
                params![key, value.to_string()],
            )
            .map_err(|error| error.to_string())?;
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
    Ok(json!({ "stored": stored, "book": book }))
}

/// Records a turn (and its tool calls) in the store; a failure is logged and
/// swallowed, because the turn itself already succeeded or failed on its own.
pub fn record_turn(turn: &TurnRow) {
    if turn.thread_id.trim().is_empty() || turn.started_at.trim().is_empty() {
        return;
    }
    // A title, compaction or recap request is not a thread.
    if rustcode_session::is_scratch_session_id(&turn.thread_id) {
        return;
    }
    let Some(path) = data_path() else { return };
    let written = open(&path).and_then(|connection| write_turn(&connection, turn));
    if let Err(error) = written {
        crate::dbg_log!(
            "data store: turn of {} not recorded: {error}",
            turn.thread_id
        );
    }
}

/// Records a turn whose harness reported *cumulative* session usage (its
/// `usage_update`, or its own log): the row keeps the growth over what this
/// thread already has for that harness, including a backfilled summary. Two
/// opens of the same session therefore cannot double count, and a thread that
/// switched harness starts its new harness at zero.
pub fn record_cumulative_turn(turn: &TurnRow, cumulative: SessionUsage) {
    if turn.thread_id.trim().is_empty() || turn.started_at.trim().is_empty() {
        return;
    }
    if rustcode_session::is_scratch_session_id(&turn.thread_id) {
        return;
    }
    let Some(path) = data_path() else { return };
    let written =
        open(&path).and_then(|connection| write_cumulative_turn(&connection, turn, cumulative));
    if let Err(error) = written {
        crate::dbg_log!(
            "data store: turn of {} not recorded: {error}",
            turn.thread_id
        );
    }
}

/// [`record_cumulative_turn`] against an open connection (tests, backfill).
fn write_cumulative_turn(
    connection: &Connection,
    turn: &TurnRow,
    cumulative: SessionUsage,
) -> Result<(), String> {
    let harness = turn.harness.clone().unwrap_or_default();
    let baseline = recorded(connection, &turn.thread_id, &harness)?;
    let spent = cumulative.growth(baseline);
    let mut row = turn.clone();
    row.input_tokens = spent.tokens.input;
    row.output_tokens = spent.tokens.output;
    row.cached_tokens = spent.tokens.cached;
    row.cache_write_tokens = spent.tokens.cache_write;
    row.cost_usd = spent.cost_usd;
    write_turn(connection, &row)
}

/// What the store already holds for one thread on one harness: the baseline a
/// cumulative session total is measured against.
pub fn recorded(
    connection: &Connection,
    thread_id: &str,
    harness: &str,
) -> Result<SessionUsage, String> {
    connection
        .query_row(
            "SELECT COALESCE(SUM(input_tokens), 0), COALESCE(SUM(output_tokens), 0),
                    COALESCE(SUM(cached_tokens), 0), COALESCE(SUM(cache_write_tokens), 0),
                    COALESCE(SUM(cost_usd), 0.0), COUNT(*)
             FROM turns WHERE thread_id = ?1 AND harness = ?2",
            params![thread_id, harness],
            |row| {
                let input: i64 = row.get(0)?;
                let output: i64 = row.get(1)?;
                let cached: i64 = row.get(2)?;
                let cache_write: i64 = row.get(3)?;
                let cost: f64 = row.get(4)?;
                let turns: i64 = row.get(5)?;
                Ok((input, output, cached, cache_write, cost, turns))
            },
        )
        .map(
            |(input, output, cached, cache_write, cost, turns)| SessionUsage {
                tokens: TokenTotals {
                    input: (turns > 0).then_some(input),
                    output: (turns > 0).then_some(output),
                    cached: (turns > 0).then_some(cached),
                    cache_write: (turns > 0).then_some(cache_write),
                },
                cost_usd: (turns > 0).then_some(cost),
                at: None,
            },
        )
        .map_err(|error| error.to_string())
}

fn write_turn(connection: &Connection, turn: &TurnRow) -> Result<(), String> {
    let transaction = connection
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    let thread = live_thread(turn);
    upsert_thread(&transaction, &thread)?;
    transaction
        .execute(
            "INSERT INTO turns (thread_id, started_at, finished_at, harness, model,
                    input_tokens, output_tokens, cached_tokens, cache_write_tokens,
                    cost_usd, tool_calls, status)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             ON CONFLICT(thread_id, started_at) DO UPDATE SET
                finished_at = excluded.finished_at,
                harness = COALESCE(excluded.harness, turns.harness),
                model = COALESCE(excluded.model, turns.model),
                input_tokens = excluded.input_tokens,
                output_tokens = excluded.output_tokens,
                cached_tokens = excluded.cached_tokens,
                cache_write_tokens = excluded.cache_write_tokens,
                cost_usd = excluded.cost_usd,
                tool_calls = excluded.tool_calls,
                status = excluded.status",
            params![
                turn.thread_id,
                turn.started_at,
                turn.finished_at,
                turn.harness,
                turn.model,
                turn.input_tokens,
                turn.output_tokens,
                turn.cached_tokens,
                turn.cache_write_tokens,
                turn.cost_usd,
                turn.tool_calls.len() as i64,
                turn.status,
            ],
        )
        .map_err(|error| error.to_string())?;
    let turn_id: i64 = transaction
        .query_row(
            "SELECT id FROM turns WHERE thread_id = ?1 AND started_at = ?2",
            params![turn.thread_id, turn.started_at],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    // A re-recorded turn (a retried write) replaces its calls instead of
    // adding a second copy.
    transaction
        .execute("DELETE FROM tool_calls WHERE turn_id = ?1", [turn_id])
        .map_err(|error| error.to_string())?;
    for call in &turn.tool_calls {
        transaction
            .execute(
                "INSERT INTO tool_calls (thread_id, turn_id, tool_name, success, exit_code, at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    turn.thread_id,
                    turn_id,
                    call.tool_name,
                    call.success,
                    call.exit_code,
                    call.at,
                ],
            )
            .map_err(|error| error.to_string())?;
    }
    transaction.commit().map_err(|error| error.to_string())
}

/// What is known about the thread outside the store: its title, where it
/// lives, and the harness and model that ran. Missing pieces stay NULL.
fn live_thread(turn: &TurnRow) -> ThreadRow {
    let id = turn.thread_id.as_str();
    let record = crate::harness::store::load(id);
    let dir = crate::config::get_active_session_dir(id);
    let cwd = record
        .as_ref()
        .map(|record| record.cwd.display().to_string())
        .or_else(|| {
            crate::config::load_session_workspace(id)
                .map(|workspace| workspace.cwd.display().to_string())
        });
    let created_at = record
        .as_ref()
        .map(|record| record.created_at.clone())
        .or_else(|| {
            crate::config::load_session_metadata(id)
                .map(|metadata| millis_to_rfc3339(metadata.created_at_ms as i64))
        });
    ThreadRow {
        id: id.to_owned(),
        title: record
            .as_ref()
            .and_then(|record| record.title.clone())
            .or_else(|| crate::config::load_session_title(id)),
        project: project_of(dir.as_deref(), cwd.as_deref()),
        cwd,
        harness: turn.harness.clone(),
        model: turn.model.clone(),
        effort: record.as_ref().and_then(|record| record.effort.clone()),
        created_at,
        updated_at: turn.finished_at.clone(),
    }
}

fn upsert_thread(connection: &Connection, thread: &ThreadRow) -> Result<(), String> {
    connection
        .execute(
            "INSERT INTO threads (id, title, project, cwd, harness, model, effort,
                    created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(id) DO UPDATE SET
                title = COALESCE(NULLIF(excluded.title, ''), threads.title),
                project = COALESCE(excluded.project, threads.project),
                cwd = COALESCE(excluded.cwd, threads.cwd),
                harness = COALESCE(excluded.harness, threads.harness),
                model = COALESCE(excluded.model, threads.model),
                effort = COALESCE(excluded.effort, threads.effort),
                created_at = COALESCE(threads.created_at, excluded.created_at),
                updated_at = COALESCE(excluded.updated_at, threads.updated_at)",
            params![
                thread.id,
                thread.title.clone().unwrap_or_default(),
                thread.project,
                thread.cwd,
                thread.harness,
                thread.model,
                thread.effort,
                thread.created_at,
                thread.updated_at,
            ],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// The project a thread belongs to: the repository of its worktree when it
/// has one, else the folder it runs in.
fn project_of(dir: Option<&Path>, cwd: Option<&str>) -> Option<String> {
    let from_worktree = dir
        .and_then(|dir| std::fs::read_to_string(dir.join("worktree.json")).ok())
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| value.get("repo").and_then(Value::as_str).map(str::to_owned));
    from_worktree.or_else(|| cwd.map(str::to_owned))
}

// ---------------------------------------------------------------- backfill

/// One thread folder's metadata, read without loading its history.
#[derive(Default)]
struct FolderFacts {
    title: Option<String>,
    project: Option<String>,
    cwd: Option<String>,
    harness: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    native_session_id: Option<String>,
    /// The named account the thread ran on (`harness.json` `account`).
    account: Option<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
    /// The thread ran on an external harness, so its usage is a session total.
    external: bool,
    usage: Option<Value>,
}

#[derive(serde::Deserialize)]
struct HarnessFile {
    harness: String,
    #[serde(default)]
    account: Option<String>,
    #[serde(default)]
    native_session_id: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    effort: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
    #[serde(default)]
    usage: Option<Value>,
}

#[derive(serde::Deserialize)]
struct CreatedFile {
    #[serde(default)]
    created_at_ms: Option<i64>,
}

#[derive(serde::Deserialize)]
struct CwdFile {
    #[serde(default)]
    cwd: Option<String>,
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn text_file(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

fn modified_rfc3339(path: &Path) -> Option<String> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    let millis = modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis() as i64;
    Some(millis_to_rfc3339(millis))
}

fn millis_to_rfc3339(millis: i64) -> String {
    chrono::DateTime::from_timestamp_millis(millis)
        .unwrap_or_else(chrono::Utc::now)
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn facts(dir: &Path) -> FolderFacts {
    let harness_file: Option<HarnessFile> = read_json(&dir.join("harness.json"));
    let created: Option<CreatedFile> = read_json(&dir.join("metadata.json"));
    let workspace: Option<CwdFile> = read_json(&dir.join("workspace.json"));
    let cwd = harness_file
        .as_ref()
        .and_then(|file| file.cwd.clone())
        .or_else(|| workspace.and_then(|file| file.cwd));
    let created_at = harness_file
        .as_ref()
        .and_then(|file| file.created_at.clone())
        .or_else(|| {
            created
                .and_then(|file| file.created_at_ms)
                .map(millis_to_rfc3339)
        })
        .or_else(|| modified_rfc3339(&dir.join("history.json")));
    FolderFacts {
        title: harness_file
            .as_ref()
            .and_then(|file| file.title.clone())
            .or_else(|| text_file(&dir.join("title.txt"))),
        project: project_of(Some(dir), cwd.as_deref()),
        cwd,
        harness: harness_file
            .as_ref()
            .map(|file| file.harness.clone())
            .or_else(|| Some(crate::harness::BUILTIN_ID.to_owned())),
        model: harness_file.as_ref().and_then(|file| file.model.clone()),
        effort: harness_file.as_ref().and_then(|file| file.effort.clone()),
        native_session_id: harness_file
            .as_ref()
            .and_then(|file| file.native_session_id.clone()),
        account: harness_file.as_ref().and_then(|file| file.account.clone()),
        external: harness_file.is_some(),
        usage: harness_file.as_ref().and_then(|file| file.usage.clone()),
        created_at,
        updated_at: harness_file
            .and_then(|file| file.updated_at)
            .or_else(|| modified_rfc3339(&dir.join("history.json"))),
    }
}

/// Thread folders under `config_dir/threads`, each one holding a
/// `history.json`. Not following symlinks: `sessions` links to `threads` on
/// older layouts and would otherwise be counted twice.
fn thread_folders(config_dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![(config_dir.join("threads"), 0usize)];
    while let Some((dir, depth)) = pending.pop() {
        if depth > 6 {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_symlink() {
                continue;
            }
            if !kind.is_dir() {
                continue;
            }
            if path.join("history.json").is_file() {
                found.push(path);
            } else {
                pending.push((path, depth + 1));
            }
        }
    }
    found
}

/// Fills the store from the thread folders: a `threads` row for every thread,
/// and one summary turn per thread that ran on an external harness, from its
/// own session log or its saved session usage, marked `backfill`. Safe to run
/// again: thread rows are updated in place, and a summary turn is rewritten
/// only while its status is still `backfill` — a live turn is never
/// overwritten or duplicated.
pub fn backfill(connection: &Connection, config_dir: &Path) -> Result<BackfillReport, String> {
    let mut report = BackfillReport::default();
    let transaction = connection
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    for dir in thread_folders(config_dir) {
        let Some(id) = dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
        else {
            continue;
        };
        if !rustcode_session::valid_session_id(&id) {
            continue;
        }
        let facts = facts(&dir);
        let thread = ThreadRow {
            id: id.clone(),
            title: facts.title.clone(),
            project: facts.project.clone(),
            cwd: facts.cwd.clone(),
            harness: facts.harness.clone(),
            model: facts.model.clone(),
            effort: facts.effort.clone(),
            created_at: facts.created_at.clone(),
            updated_at: facts.updated_at.clone(),
        };
        if let Err(error) = upsert_thread(&transaction, &thread) {
            crate::dbg_log!("data store: thread {id} not recorded: {error}");
            report.errors += 1;
            continue;
        }
        report.threads += 1;
        if !facts.external {
            continue;
        }
        let Some(started_at) = facts.created_at.clone() else {
            continue;
        };
        // The harness's own log has the real token counts when it keeps one
        // (Claude Code, Pi); a harness whose only usage is the context gauge
        // falls back to the session totals in `_meta.rustcode`, and keeps NULL
        // when it reports neither.
        let harness = facts.harness.as_deref().unwrap_or_default();
        let account_dir = facts.account.as_deref().and_then(|account| {
            let dir = crate::harness::config::config_dir().ok()?;
            crate::harness::config::load_accounts_in(&dir)
                .into_iter()
                .find(|known| known.id == account && known.harness == harness)
                .map(|known| known.resolved_dir())
        });
        let from_log = crate::harness_usage::session_usage(
            harness,
            facts.native_session_id.as_deref(),
            facts.cwd.as_deref().map(Path::new),
            account_dir.as_deref(),
        );
        let totals = from_log
            .as_ref()
            .map(|usage| usage.tokens)
            .filter(|tokens| !tokens.is_empty())
            .or_else(|| facts.usage.as_ref().and_then(totals))
            .unwrap_or_default();
        // The harness's reported USD cost first (it may include turns the log
        // does not), else the cost its log priced.
        let cost = facts
            .usage
            .as_ref()
            .and_then(cost_usd)
            .or_else(|| from_log.as_ref().and_then(|usage| usage.cost_usd));
        let finished_at = from_log
            .as_ref()
            .and_then(|usage| usage.at.clone())
            .or_else(|| facts.updated_at.clone());
        let written = transaction.execute(
            "INSERT INTO turns (thread_id, started_at, finished_at, harness, model,
                    input_tokens, output_tokens, cached_tokens, cache_write_tokens,
                    cost_usd, tool_calls, status)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 0, 'backfill')
             ON CONFLICT(thread_id, started_at) DO UPDATE SET
                finished_at = excluded.finished_at,
                input_tokens = excluded.input_tokens,
                output_tokens = excluded.output_tokens,
                cached_tokens = excluded.cached_tokens,
                cache_write_tokens = excluded.cache_write_tokens,
                cost_usd = excluded.cost_usd
             WHERE turns.status = 'backfill'",
            params![
                id,
                started_at,
                finished_at,
                facts.harness,
                facts.model,
                totals.input,
                totals.output,
                totals.cached,
                totals.cache_write,
                cost,
            ],
        );
        match written {
            Ok(1) => report.turns += 1,
            Ok(_) => {}
            Err(error) => {
                crate::dbg_log!("data store: backfill turn of {id} not recorded: {error}");
                report.errors += 1;
            }
        }
    }
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(report)
}

/// The backfill's own version. Bumped when a later release can read more than
/// the previous one (v2: token counts from Claude Code and Pi logs, cost):
/// the pass runs again and rewrites only rows still marked `backfill`.
const BACKFILL_VERSION: i64 = 2;

/// Opens the store and backfills it once, on first use. Called at engine
/// start (in the background) and by the `usage` / `data` commands.
pub fn ensure_ready() -> Result<PathBuf, String> {
    let path = data_path().ok_or_else(|| "no config directory".to_owned())?;
    let connection = open(&path)?;
    let done: Option<String> = connection
        .query_row(
            "SELECT value FROM store_meta WHERE key = 'backfill_version'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let current = done
        .as_deref()
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(0);
    if current < BACKFILL_VERSION {
        let config_dir = path
            .parent()
            .ok_or_else(|| "the data store has no parent directory".to_owned())?;
        let report = backfill(&connection, config_dir)?;
        connection
            .execute(
                "INSERT INTO store_meta (key, value) VALUES ('backfill_version', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [BACKFILL_VERSION.to_string()],
            )
            .map_err(|error| error.to_string())?;
        crate::logger::operational_event(
            "data_store.backfill",
            json!({
                "threads": report.threads,
                "turns": report.turns,
                "errors": report.errors,
            }),
        );
    }
    Ok(path)
}

/// Backfills in the background at engine start, so nothing waits for it.
pub fn warm() {
    std::thread::spawn(|| {
        if let Err(error) = ensure_ready() {
            crate::dbg_log!("data store: not ready: {error}");
        }
    });
}

// ------------------------------------------------------------------ reads

/// Opens the store read-only with the guards every query runs under: no
/// writes, no `ATTACH`, and a progress handler that stops after two seconds.
pub fn open_readonly(path: &Path) -> Result<Connection, String> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| format!("{}: {error}", path.display()))?;
    connection
        .execute_batch("PRAGMA query_only = ON;")
        .map_err(|error| error.to_string())?;
    let _ = connection.set_limit(Limit::SQLITE_LIMIT_ATTACHED, 0);
    let started = Instant::now();
    connection.progress_handler(10_000, Some(move || started.elapsed() >= QUERY_TIMEOUT));
    Ok(connection)
}

/// Runs one read-only statement against the store and returns its rows.
///
/// `limit` is clamped to [`MAX_ROWS`]. A second statement, a write, `ATTACH`
/// and a read of another database file are refused; a query that runs longer
/// than two seconds is stopped.
pub fn query(sql: &str, limit: usize) -> Result<QueryResult, String> {
    let path = data_path().ok_or_else(|| "no config directory".to_owned())?;
    query_path(&path, sql, limit)
}

fn query_path(path: &Path, sql: &str, limit: usize) -> Result<QueryResult, String> {
    if sql.trim().is_empty() {
        return Err("no SQL given".to_owned());
    }
    if !single_statement(sql) {
        return Err("one statement only".to_owned());
    }
    if !path.is_file() {
        return Err(format!(
            "no data store yet at {}; open the app once or run a turn so it is created",
            path.display()
        ));
    }
    let connection = open_readonly(path)?;
    let mut statement = connection
        .prepare(sql)
        .map_err(|error| format!("SQL: {error}"))?;
    let columns: Vec<String> = statement
        .column_names()
        .into_iter()
        .map(str::to_owned)
        .collect();
    let cap = limit.clamp(1, MAX_ROWS);
    let mut result = QueryResult {
        columns: columns.clone(),
        rows: Vec::new(),
        truncated: false,
    };
    let mut rows = statement.query([]).map_err(query_error)?;
    while let Some(row) = rows.next().map_err(query_error)? {
        if result.rows.len() >= cap {
            result.truncated = true;
            break;
        }
        let mut object = serde_json::Map::with_capacity(columns.len());
        for (index, name) in columns.iter().enumerate() {
            let value = row.get_ref(index).map_err(query_error)?;
            object.insert(name.clone(), value_json(value));
        }
        result.rows.push(Value::Object(object));
    }
    Ok(result)
}

fn query_error(error: rusqlite::Error) -> String {
    let text = error.to_string();
    if text.contains("interrupted") {
        format!("query stopped after {} s", QUERY_TIMEOUT.as_secs())
    } else {
        text
    }
}

fn value_json(value: ValueRef<'_>) -> Value {
    match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(number) => json!(number),
        ValueRef::Real(number) => json!(number),
        ValueRef::Text(text) => json!(String::from_utf8_lossy(text)),
        ValueRef::Blob(bytes) => json!(format!("<{} bytes>", bytes.len())),
    }
}

/// True when `sql` holds one statement: semicolons inside quotes and comments
/// do not count, and anything but whitespace or a comment after the
/// terminating `;` is a second statement.
fn single_statement(sql: &str) -> bool {
    let mut chars = sql.chars().peekable();
    let mut ended = false;
    while let Some(ch) = chars.next() {
        if ended {
            if ch.is_whitespace() || ch == ';' {
                continue;
            }
            return false;
        }
        match ch {
            '\'' | '"' => {
                let quote = ch;
                loop {
                    match chars.next() {
                        None => return false,
                        Some(inner) if inner == quote => {
                            if chars.peek() == Some(&quote) {
                                chars.next();
                                continue;
                            }
                            break;
                        }
                        Some(_) => {}
                    }
                }
            }
            '-' if chars.peek() == Some(&'-') => {
                for inner in chars.by_ref() {
                    if inner == '\n' {
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut closed = false;
                while let Some(inner) = chars.next() {
                    if inner == '*' && chars.peek() == Some(&'/') {
                        chars.next();
                        closed = true;
                        break;
                    }
                }
                if !closed {
                    return false;
                }
            }
            ';' => ended = true,
            _ => {}
        }
    }
    !sql.trim().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(thread: &str, started: &str, tokens: (i64, i64), status: &str) -> TurnRow {
        TurnRow {
            thread_id: thread.to_owned(),
            started_at: started.to_owned(),
            finished_at: Some(started.to_owned()),
            harness: Some("codex".to_owned()),
            model: Some("gpt-6-luna".to_owned()),
            input_tokens: Some(tokens.0),
            output_tokens: Some(tokens.1),
            cached_tokens: None,
            cache_write_tokens: None,
            cost_usd: None,
            status: status.to_owned(),
            tool_calls: vec![ToolCallRow {
                tool_name: "execute".to_owned(),
                success: Some(false),
                exit_code: Some(1),
                at: Some(started.to_owned()),
            }],
        }
    }

    /// A store with two threads in two projects and three turns, one of them
    /// with no reported tokens and none with a cost.
    fn usage_store(dir: &Path) -> PathBuf {
        let path = dir.join(DATA_FILE);
        let connection = open(&path).unwrap();
        for (id, project) in [("a", "/repo/a"), ("b", "/repo/b")] {
            upsert_thread(
                &connection,
                &ThreadRow {
                    id: id.to_owned(),
                    title: Some(format!("Thread {id}")),
                    project: Some(project.to_owned()),
                    cwd: Some(project.to_owned()),
                    harness: Some("codex".to_owned()),
                    model: Some("gpt-6-luna".to_owned()),
                    effort: None,
                    created_at: Some("2026-11-01T10:00:00Z".to_owned()),
                    updated_at: Some("2026-11-01T10:00:00Z".to_owned()),
                },
            )
            .unwrap();
        }
        for spent in [
            turn("a", "2026-11-01T10:00:00Z", (100, 10), "completed"),
            turn("b", "2026-11-02T10:00:00Z", (200, 20), "completed"),
        ] {
            write_turn(&connection, &spent).unwrap();
        }
        // A turn whose harness reported nothing at all.
        write_turn(
            &connection,
            &TurnRow {
                thread_id: "b".to_owned(),
                started_at: "2026-11-03T10:00:00Z".to_owned(),
                finished_at: Some("2026-11-03T10:00:01Z".to_owned()),
                harness: Some("claude-code".to_owned()),
                model: Some("sonnet".to_owned()),
                input_tokens: None,
                output_tokens: None,
                cached_tokens: None,
                cache_write_tokens: None,
                cost_usd: None,
                status: "completed".to_owned(),
                tool_calls: Vec::new(),
            },
        )
        .unwrap();
        path
    }

    fn rows(answer: &Value) -> Vec<Value> {
        answer["rows"].as_array().cloned().unwrap_or_default()
    }

    #[test]
    fn usage_groups_totals_and_scopes_by_project() {
        let dir = tempfile::tempdir().unwrap();
        let path = usage_store(dir.path());

        let by_harness = usage_at(&path, None, GroupBy::Harness, None).unwrap();
        let listed = rows(&by_harness);
        let harnesses: Vec<&str> = listed
            .iter()
            .map(|row| row["harness"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(harnesses, ["codex", "claude-code"], "{by_harness}");
        let codex = &rows(&by_harness)[0];
        assert_eq!(codex["input_tokens"], 300);
        assert_eq!(codex["output_tokens"], 30);
        assert_eq!(codex["turns"], 2);
        assert_eq!(codex["turns_without_tokens"], 0);
        // A harness that reported nothing is NULL, never a made-up zero.
        let claude = &rows(&by_harness)[1];
        assert!(claude["input_tokens"].is_null(), "{claude}");
        assert_eq!(claude["turns_without_tokens"], 1);
        assert!(
            by_harness["totals"]["cost_usd"].is_null(),
            "nothing was priced"
        );
        assert_eq!(by_harness["totals"]["input_tokens"], 300.0);
        assert_eq!(by_harness["totals"]["turns"], 3.0);
        assert!(
            by_harness["note"]
                .as_str()
                .unwrap()
                .contains("never a price")
        );

        // One project, read from `threads`.
        let scoped = usage_at(&path, None, GroupBy::Project, Some("/repo/a")).unwrap();
        assert_eq!(rows(&scoped).len(), 1);
        assert_eq!(rows(&scoped)[0]["project"], "/repo/a");
        assert_eq!(rows(&scoped)[0]["input_tokens"], 100);

        // `since` is "from that day on", and a month works too, comparing the
        // RFC 3339 timestamp's prefix.
        let day = usage_at(&path, Some("2026-11-02"), GroupBy::Day, None).unwrap();
        assert_eq!(rows(&day).len(), 2, "{day}");
        assert_eq!(rows(&day)[0]["day"], "2026-11-02");
        let month = usage_at(&path, Some("2026-11"), GroupBy::Day, None).unwrap();
        assert_eq!(rows(&month).len(), 3);
        let later = usage_at(&path, Some("2026-12"), GroupBy::Day, None).unwrap();
        assert!(rows(&later).is_empty());

        // Per thread, with its title so a row says which thread it is.
        let threads = usage_at(&path, None, GroupBy::Thread, None).unwrap();
        let first = &rows(&threads)[0];
        assert_eq!(first["thread"], "b");
        assert_eq!(first["title"], "Thread b");
        assert_eq!(first["project"], "/repo/b");
    }

    #[test]
    fn query_answers_which_tools_fail() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DATA_FILE);
        let connection = open(&path).unwrap();
        upsert_thread(
            &connection,
            &ThreadRow {
                id: "a".to_owned(),
                title: Some("Thread a".to_owned()),
                project: Some("/repo/a".to_owned()),
                cwd: Some("/repo/a".to_owned()),
                harness: Some("codex".to_owned()),
                model: None,
                effort: None,
                created_at: None,
                updated_at: None,
            },
        )
        .unwrap();
        let mut first = turn("a", "2026-11-01T10:00:00Z", (10, 1), "completed");
        first.tool_calls = vec![
            ToolCallRow {
                tool_name: "run_command".to_owned(),
                success: Some(false),
                exit_code: Some(1),
                at: None,
            },
            ToolCallRow {
                tool_name: "run_command".to_owned(),
                success: Some(true),
                exit_code: Some(0),
                at: None,
            },
            ToolCallRow {
                tool_name: "replace_file_content".to_owned(),
                success: Some(false),
                exit_code: None,
                at: None,
            },
        ];
        write_turn(&connection, &first).unwrap();
        // The question an agent asks of `query_data`, through the same guards
        // the tool uses.
        let answer = query_path(
            &path,
            "SELECT tool_name, SUM(success = 0) AS failures, COUNT(*) AS calls \
             FROM tool_calls GROUP BY tool_name HAVING failures > 0 \
             ORDER BY failures DESC, calls DESC",
            MAX_ROWS,
        )
        .unwrap();
        assert_eq!(answer.columns, ["tool_name", "failures", "calls"]);
        assert_eq!(
            answer.rows,
            [
                json!({"tool_name": "run_command", "failures": 1, "calls": 2}),
                json!({"tool_name": "replace_file_content", "failures": 1, "calls": 1}),
            ]
        );
    }

    #[test]
    fn usage_reports_a_missing_store_and_a_bad_day() {
        let dir = tempfile::tempdir().unwrap();
        let error =
            usage_at(&dir.path().join(DATA_FILE), None, GroupBy::Harness, None).unwrap_err();
        assert!(error.contains("no data store yet"), "{error}");
        assert!(parse_since("last week").is_err());
        assert_eq!(parse_since("2026-11").unwrap(), "2026-11");
        assert_eq!(parse_since("2026-11-05").unwrap(), "2026-11-05");
        assert_eq!(GroupBy::parse("day"), Some(GroupBy::Day));
        assert_eq!(GroupBy::parse(""), Some(GroupBy::Harness));
        assert_eq!(GroupBy::parse("tool"), None);
    }

    /// A book with a settled link and then a working one (the order the app
    /// appends them in), and the state beside them.
    fn book_json() -> Value {
        json!({
            "links": [
                {
                    "id": "a/1",
                    "orchestrator": "a",
                    "target": "b",
                    "orchestratorTitle": "A",
                    "targetTitle": "B",
                    "message": "count them",
                    "state": "done",
                    "answer": "twelve",
                    "createdAt": 1_761_000_000_000i64,
                    "settledAt": 1_761_000_100_000i64,
                    "notifiedAt": 1_761_000_200_000i64,
                },
                {
                    "id": "a/2",
                    "orchestrator": "a",
                    "target": "c",
                    "orchestratorTitle": "A",
                    "targetTitle": "C",
                    "message": "review the diff",
                    "state": "working",
                    "started": true,
                    "harness": "codex",
                    "filesChanged": 2,
                    "createdAt": 1_762_000_000_000i64,
                },
            ],
            "grants": { "a": ["start", "b"] },
            "counters": { "a": 2 },
            "fanOuts": [{
                "id": "fan-1",
                "orchestrator": "a",
                "title": "Both ways",
                "prompt": "do it",
                "links": ["a/2"],
                "createdAt": 1_762_000_000_000i64,
            }],
        })
    }

    #[test]
    fn the_link_book_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DATA_FILE);
        // Nothing saved yet: the app must know to migrate its own copy in.
        let none = load_link_book_at(&path).unwrap();
        assert_eq!(none["stored"], false);
        assert_eq!(none["book"]["links"], json!([]));

        let book = book_json();
        save_link_book_at(&path, &book).unwrap();
        let loaded = load_link_book_at(&path).unwrap();
        assert_eq!(loaded["stored"], true);
        assert_eq!(loaded["book"], book, "{loaded}");

        // Denormalised columns answer a query without opening a body.
        let connection = open(&path).unwrap();
        let summary: (String, Option<String>, Option<i64>) = connection
            .query_row(
                "SELECT state, harness, files_changed FROM thread_links WHERE id = 'a/2'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            summary,
            ("working".to_owned(), Some("codex".to_owned()), Some(2))
        );
        // The book is replaced, not appended to.
        save_link_book_at(
            &path,
            &json!({ "links": [], "grants": {}, "counters": {}, "fanOuts": [] }),
        )
        .unwrap();
        let empty = load_link_book_at(&path).unwrap();
        assert_eq!(empty["book"]["links"], json!([]));
        assert_eq!(empty["stored"], true, "the state rows are still there");
    }

    #[test]
    fn the_link_book_tolerates_fields_it_does_not_know() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DATA_FILE);
        let mut book = book_json();
        book["future"] = json!({ "kept": false });
        book["links"][1]["effort"] = json!("high");
        book["links"][1]["laterField"] = json!([1, 2, 3]);
        save_link_book_at(&path, &book).unwrap();
        let loaded = load_link_book_at(&path).unwrap();
        // A link's own fields travel in `body`, which is the whole record.
        assert_eq!(loaded["book"]["links"][1]["laterField"], json!([1, 2, 3]));
        assert_eq!(loaded["book"]["links"][1]["effort"], json!("high"));
        // A top-level key this release has no column for is dropped, never an error.
        assert!(loaded["book"].get("future").is_none());
        assert_eq!(loaded["book"]["grants"], json!({ "a": ["start", "b"] }));
    }

    #[test]
    fn the_link_book_is_bounded_like_the_app_prunes_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DATA_FILE);
        let mut links: Vec<Value> = (0..60)
            .map(|n| {
                json!({
                    "id": format!("a/{n}"),
                    "orchestrator": "a",
                    "target": format!("t{n}"),
                    "state": "done",
                    "createdAt": 1_000 + n,
                })
            })
            .collect();
        // An active link is kept whatever its age.
        links.push(json!({
            "id": "a/old",
            "orchestrator": "a",
            "target": "old",
            "state": "working",
            "createdAt": 1,
        }));
        // Every fan-out names the active link, so all of them still have one.
        let fan_outs: Vec<Value> = (0..25)
            .map(|n| json!({ "id": format!("fan-{n}"), "orchestrator": "a", "links": ["a/old"] }))
            .collect();
        save_link_book_at(
            &path,
            &json!({ "links": links, "grants": {}, "counters": {}, "fanOuts": fan_outs }),
        )
        .unwrap();
        let loaded = load_link_book_at(&path).unwrap();
        let kept = loaded["book"]["links"].as_array().unwrap();
        assert_eq!(kept.len(), KEPT_FINISHED_LINKS + 1);
        assert!(kept.iter().any(|link| link["id"] == "a/old"));
        assert!(
            !kept.iter().any(|link| link["id"] == "a/0"),
            "the oldest finished link goes"
        );
        assert_eq!(
            loaded["book"]["fanOuts"].as_array().unwrap().len(),
            KEPT_FAN_OUTS
        );
    }

    #[test]
    fn a_store_from_the_previous_version_gets_the_link_tables() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DATA_FILE);
        // A store as release 1 left it: migration 1 recorded, nothing else.
        {
            let connection = Connection::open(&path).unwrap();
            connection
                .execute_batch(
                    "CREATE TABLE schema_migrations (id INTEGER PRIMARY KEY, name TEXT NOT NULL);",
                )
                .unwrap();
            let (id, name, sql) = MIGRATIONS[0];
            connection.execute_batch(sql).unwrap();
            connection
                .execute(
                    "INSERT INTO schema_migrations (id, name) VALUES (?1, ?2)",
                    params![id, name],
                )
                .unwrap();
        }
        let loaded = load_link_book_at(&path).unwrap();
        assert_eq!(loaded["stored"], false);
        save_link_book_at(&path, &book_json()).unwrap();
        let again = load_link_book_at(&path).unwrap();
        assert_eq!(again["book"]["links"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn migrations_apply_once_and_a_renamed_one_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DATA_FILE);
        let first = open(&path).unwrap();
        let columns: Vec<String> = first
            .prepare("PRAGMA table_info(tool_calls)")
            .unwrap()
            .query_map([], |row| row.get(1))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert_eq!(
            columns,
            [
                "id",
                "thread_id",
                "turn_id",
                "tool_name",
                "success",
                "exit_code",
                "at"
            ]
        );
        drop(first);
        open(&path).expect("a second open does not migrate again");

        let connection = open(&path).unwrap();
        connection
            .execute(
                "UPDATE schema_migrations SET name = 'something else' WHERE id = 1",
                [],
            )
            .unwrap();
        assert!(migrate(&connection).unwrap_err().contains("recorded as"));
    }

    #[test]
    fn a_turn_is_updated_in_place_and_its_calls_are_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let connection = open(&dir.path().join(DATA_FILE)).unwrap();
        let started = "2026-11-04T10:00:00.000Z";
        write_turn(&connection, &turn("t1", started, (100, 20), "completed")).unwrap();
        write_turn(&connection, &turn("t1", started, (140, 30), "cancelled")).unwrap();

        let (turns, status, input, calls): (i64, String, i64, i64) = connection
            .query_row(
                "SELECT (SELECT COUNT(*) FROM turns), status, input_tokens,
                        (SELECT COUNT(*) FROM tool_calls)
                 FROM turns",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            (turns, status.as_str(), input, calls),
            (1, "cancelled", 140, 1)
        );

        let per_day: (String, i64, i64) = connection
            .query_row(
                "SELECT day, input_tokens, turns FROM daily_usage",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(per_day, ("2026-11-04".to_owned(), 140, 1));
    }

    #[test]
    fn session_totals_become_per_turn_deltas() {
        let before = totals(&json!({
            "_meta": {"rustcode": {"inputTokens": 1000, "outputTokens": 100, "cachedTokens": 500}},
        }))
        .unwrap();
        assert_eq!(
            before,
            TokenTotals {
                input: Some(1000),
                output: Some(100),
                cached: Some(500),
                cache_write: None,
            }
        );
        let now = totals(&json!({
            "_meta": {"rustcode": {"inputTokens": 1600, "outputTokens": 180, "cachedTokens": 900}},
        }))
        .unwrap();
        assert_eq!(
            delta(now, Some(before)),
            TokenTotals {
                input: Some(600),
                output: Some(80),
                cached: Some(400),
                cache_write: None,
            }
        );
        assert_eq!(
            delta(now, None),
            now,
            "first sight of a counter is its own turn"
        );
        // A harness that restarted its own session: no negative numbers.
        assert_eq!(
            delta(
                TokenTotals {
                    input: Some(5),
                    output: None,
                    cached: None,
                    cache_write: None,
                },
                Some(before)
            ),
            TokenTotals {
                input: None,
                output: None,
                cached: None,
                cache_write: None,
            }
        );
        assert!(
            totals(&json!({"used": 10, "size": 100})).is_none(),
            "a context window alone is not a token total"
        );
    }

    fn fake_config(dir: &Path) {
        let thread = |id: &str, harness: Option<&str>, usage: Option<Value>, native: bool| {
            let folder = dir.join("threads/2026/11/04").join(id);
            std::fs::create_dir_all(&folder).unwrap();
            std::fs::write(folder.join("history.json"), "[]").unwrap();
            std::fs::write(folder.join("title.txt"), format!("title {id}")).unwrap();
            std::fs::write(
                folder.join("workspace.json"),
                r#"{"cwd":"/repo","additional_directories":[]}"#,
            )
            .unwrap();
            if let Some(harness) = harness {
                let mut record = json!({
                    "harness": harness,
                    "cwd": "/repo",
                    "title": format!("harness title {id}"),
                    "model": "gpt-6-luna",
                    "created_at": "2026-11-04T09:00:00+00:00",
                    "updated_at": "2026-11-04T10:00:00+00:00",
                });
                if let Some(usage) = usage {
                    record["usage"] = usage;
                }
                std::fs::write(
                    folder.join("harness.json"),
                    serde_json::to_string(&record).unwrap(),
                )
                .unwrap();
            }
            if native {
                std::fs::write(
                    folder.join("metadata.json"),
                    r#"{"schema_version":1,"id":"x","created_at_ms":1793784000000}"#,
                )
                .unwrap();
            }
        };
        thread(
            "01a10000-0000-0000-0000-000000000001",
            Some("codex"),
            Some(json!({"used": 10, "size": 1000, "_meta": {"rustcode": {
                "inputTokens": 700, "outputTokens": 70, "cachedTokens": 300}}})),
            false,
        );
        thread("01a10000-0000-0000-0000-000000000002", None, None, true);
    }

    #[test]
    fn backfill_is_idempotent_and_one_summary_turn_per_external_thread() {
        let config = tempfile::tempdir().unwrap();
        fake_config(config.path());
        let connection = open(&config.path().join(DATA_FILE)).unwrap();

        let first = backfill(&connection, config.path()).unwrap();
        assert_eq!(
            first,
            BackfillReport {
                threads: 2,
                turns: 1,
                errors: 0
            }
        );
        // A later pass rewrites the same summary row rather than adding a
        // second one, which is what lets v2 refresh a v1 backfill.
        let second = backfill(&connection, config.path()).unwrap();
        assert_eq!(second.threads, 2);
        assert_eq!(second.errors, 0);

        let (threads, turns, status, input, title): (i64, i64, String, i64, String) = connection
            .query_row(
                "SELECT (SELECT COUNT(*) FROM threads), (SELECT COUNT(*) FROM turns),
                        t.status, t.input_tokens, th.title
                 FROM turns t JOIN threads th ON th.id = t.thread_id",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!((threads, turns), (2, 1));
        assert_eq!(status, "backfill");
        assert_eq!(input, 700);
        assert_eq!(title, "harness title 01a10000-0000-0000-0000-000000000001");

        let native: (String, String, Option<String>) = connection
            .query_row(
                "SELECT harness, title, created_at FROM threads
                 WHERE id = '01a10000-0000-0000-0000-000000000002'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(native.0, "rustcode");
        assert!(native.1.ends_with("02"));
        assert!(native.2.is_some(), "metadata.json supplies created_at");
    }

    #[test]
    fn cost_is_a_delta_and_only_usd_is_kept() {
        let usd = json!({"used": 10, "size": 100, "cost": {"amount": 24.5, "currency": "USD"}});
        assert_eq!(cost_usd(&usd), Some(24.5));
        let eur = json!({"cost": {"amount": 24.5, "currency": "EUR"}});
        assert_eq!(cost_usd(&eur), None, "another currency is not converted");
        assert_eq!(cost_usd(&json!({"used": 10})), None);

        let usage = |cost: f64| SessionUsage {
            tokens: TokenTotals::default(),
            cost_usd: Some(cost),
            at: None,
        };
        assert_eq!(usage(0.75).growth(usage(0.5)).cost_usd, Some(0.25));
        assert_eq!(
            usage(0.2).growth(usage(0.5)).cost_usd,
            Some(0.2),
            "a restarted session reports its whole cost again"
        );
        assert_eq!(
            usage(0.2).growth(SessionUsage::default()).cost_usd,
            Some(0.2)
        );
    }

    #[test]
    fn the_first_turn_of_a_new_thread_is_recorded() {
        let config = tempfile::tempdir().unwrap();
        let connection = open(&config.path().join(DATA_FILE)).unwrap();
        let thread = "01a10000-0000-0000-0000-00000000000f";
        let turn = TurnRow {
            thread_id: thread.to_owned(),
            started_at: "2026-11-04T11:00:00.000Z".to_owned(),
            finished_at: Some("2026-11-04T11:00:05.000Z".to_owned()),
            harness: Some("pi".to_owned()),
            status: "completed".to_owned(),
            ..TurnRow::default()
        };
        // No row yet, so the baseline has nothing to sum.
        write_cumulative_turn(
            &connection,
            &turn,
            SessionUsage {
                tokens: TokenTotals {
                    input: Some(50),
                    ..TokenTotals::default()
                },
                ..SessionUsage::default()
            },
        )
        .unwrap();
        let (turns, input, threads): (i64, i64, i64) = connection
            .query_row(
                "SELECT COUNT(*), SUM(input_tokens),
                        (SELECT COUNT(*) FROM threads WHERE id = ?1)
                 FROM turns WHERE thread_id = ?1",
                [thread],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!((turns, input, threads), (1, 50, 1));
    }

    #[test]
    fn a_live_turn_after_backfill_counts_only_the_growth() {
        let config = tempfile::tempdir().unwrap();
        fake_config(config.path());
        let connection = open(&config.path().join(DATA_FILE)).unwrap();
        backfill(&connection, config.path()).unwrap();
        drop(connection);

        let thread = "01a10000-0000-0000-0000-000000000001";
        let cumulative = |input: i64, cost: f64| SessionUsage {
            tokens: TokenTotals {
                input: Some(input),
                output: Some(input / 10),
                cached: None,
                cache_write: None,
            },
            cost_usd: Some(cost),
            at: None,
        };
        let live = |started: &str| TurnRow {
            thread_id: thread.to_owned(),
            started_at: started.to_owned(),
            finished_at: Some(started.to_owned()),
            harness: Some("codex".to_owned()),
            model: Some("gpt-6-luna".to_owned()),
            status: "completed".to_owned(),
            ..TurnRow::default()
        };
        // The summary already holds 700 in / 70 out and no cost.
        let connection = open(&config.path().join(DATA_FILE)).unwrap();
        write_cumulative_turn(
            &connection,
            &live("2026-11-04T11:00:00.000Z"),
            cumulative(700, 0.4),
        )
        .unwrap();
        write_cumulative_turn(
            &connection,
            &live("2026-11-04T12:00:00.000Z"),
            cumulative(900, 0.6),
        )
        .unwrap();
        let (turns, input, output, cost): (i64, i64, i64, f64) = connection
            .query_row(
                "SELECT COUNT(*), SUM(input_tokens), SUM(output_tokens), SUM(cost_usd)
                 FROM turns WHERE thread_id = ?1",
                [thread],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(turns, 3, "one backfill row and two live rows");
        assert_eq!(input, 900, "700 backfilled plus 0 plus 200");
        assert_eq!(output, 90);
        assert!((cost - 0.6).abs() < 1e-9, "cost adds up to the last total");
        let backfill: (i64, String) = connection
            .query_row(
                "SELECT input_tokens, status FROM turns WHERE started_at LIKE '2026-11-04T09:00%'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(backfill, (700, "backfill".to_owned()));
    }

    #[test]
    fn backfill_reads_a_claude_log_for_tokens_and_the_harness_cost_for_usd() {
        let config = tempfile::tempdir().unwrap();
        let claude = tempfile::tempdir().unwrap();
        let thread = "01a10000-0000-0000-0000-000000000009";
        let folder = config.path().join("threads/2026/11/04").join(thread);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("history.json"), "[]").unwrap();
        std::fs::write(folder.join("title.txt"), "claude thread").unwrap();
        std::fs::write(
            folder.join("workspace.json"),
            r#"{"cwd":"/tmp/claude-project","additional_directories":[]}"#,
        )
        .unwrap();
        std::fs::write(
            folder.join("harness.json"),
            serde_json::to_string(&json!({
                "harness": "claude-code",
                "native_session_id": "session-1",
                "cwd": "/tmp/claude-project",
                "model": "claude-opus-5-5",
                "usage": {"used": 10, "size": 1000,
                          "cost": {"amount": 1.25, "currency": "USD"}},
                "created_at": "2026-11-04T09:00:00+00:00",
                "updated_at": "2026-11-04T10:00:00+00:00",
            }))
            .unwrap(),
        )
        .unwrap();
        // The log sits where Claude Code puts it: projects/<slug>/<id>.jsonl,
        // with the same message twice as it spans two content blocks.
        let projects = claude.path().join("projects/-tmp-claude-project");
        std::fs::create_dir_all(&projects).unwrap();
        let line = r#"{"type":"assistant","timestamp":"2026-11-04T09:30:00.000Z","message":{"id":"msg_1","model":"claude-opus-5-5","usage":{"input_tokens":10,"output_tokens":100,"cache_read_input_tokens":1000,"cache_creation_input_tokens":50}}}"#;
        std::fs::write(
            projects.join("session-1.jsonl"),
            format!("{line}\n{line}\n"),
        )
        .unwrap();
        std::fs::write(
            projects.join("other.jsonl"),
            r#"{"type":"assistant","message":{"id":"x","usage":{"input_tokens":9999}}}"#,
        )
        .unwrap();

        let previous = std::env::var_os("CLAUDE_CONFIG_DIR");
        // Only this test reads `CLAUDE_CONFIG_DIR`; nothing else in the crate
        // looks at the Claude folders.
        unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", claude.path()) };
        let connection = open(&config.path().join(DATA_FILE)).unwrap();
        let report = backfill(&connection, config.path()).unwrap();
        match previous {
            Some(value) => unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", value) },
            None => unsafe { std::env::remove_var("CLAUDE_CONFIG_DIR") },
        }
        assert_eq!(report.errors, 0);

        let (input, output, cached, cache_write, cost, finished): (
            i64,
            i64,
            i64,
            i64,
            f64,
            String,
        ) = connection
            .query_row(
                "SELECT input_tokens, output_tokens, cached_tokens, cache_write_tokens,
                        cost_usd, finished_at
                 FROM turns WHERE thread_id = ?1",
                [thread],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!((input, output, cached, cache_write), (10, 100, 1000, 50));
        assert!((cost - 1.25).abs() < 1e-9);
        assert_eq!(finished, "2026-11-04T09:30:00.000Z", "the log's last stamp");
    }

    #[test]
    fn a_write_attach_second_statement_and_another_file_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DATA_FILE);
        let other = dir.path().join("other.sqlite");
        let connection = open(&path).unwrap();
        write_turn(
            &connection,
            &turn("t1", "2026-11-04T10:00:00.000Z", (5, 5), "completed"),
        )
        .unwrap();
        open(&other).unwrap();
        drop(connection);

        let result = query_path(&path, "SELECT COUNT(*) AS n FROM turns", 10).unwrap();
        assert_eq!(result.rows[0]["n"], 1);

        for sql in [
            "INSERT INTO threads (id) VALUES ('x')",
            "DELETE FROM turns",
            "DROP TABLE turns",
        ] {
            assert!(query_path(&path, sql, 10).is_err(), "{sql} must be refused");
        }
        let attach = format!("ATTACH DATABASE '{}' AS other", other.display());
        assert!(query_path(&path, &attach, 10).is_err(), "ATTACH is refused");
        assert!(
            query_path(&path, "SELECT * FROM other.turns", 10).is_err(),
            "another database cannot be reached"
        );
        assert!(
            query_path(&path, "SELECT 1; SELECT 2", 10).is_err(),
            "one statement only"
        );
        assert!(
            query_path(&path, "SELECT ';' AS semi;", 10).is_ok(),
            "a semicolon inside a string is not a separator"
        );
        assert!(
            query_path(&path, "SELECT 1 -- ;\n", 10).is_ok(),
            "a semicolon inside a comment is not a separator"
        );
    }

    #[test]
    fn rows_are_capped_and_an_endless_query_is_stopped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DATA_FILE);
        open(&path).unwrap();

        let many = query_path(
            &path,
            "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c LIMIT 1200)
             SELECT x FROM c",
            5000,
        )
        .unwrap();
        assert_eq!(many.rows.len(), MAX_ROWS);
        assert!(many.truncated);

        let started = Instant::now();
        let error = query_path(
            &path,
            "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c)
             SELECT count(*) FROM c",
            10,
        )
        .unwrap_err();
        assert!(error.contains("stopped"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn a_missing_store_says_how_to_create_it() {
        let dir = tempfile::tempdir().unwrap();
        let error = query_path(&dir.path().join(DATA_FILE), "SELECT 1", 10).unwrap_err();
        assert!(error.contains("no data store yet"), "{error}");
    }
}
