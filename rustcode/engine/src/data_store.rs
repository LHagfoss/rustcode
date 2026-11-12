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
//! The request and protocol tables ([`record_provider_request`],
//! [`record_rpc_message`], [`record_operational_event`]) record identifiers,
//! timings and counts by default. A payload is stored only when the user turned
//! on `capture_provider_payloads` or `capture_harness_rpc_payloads`, and even
//! then it is redacted and clipped ([`redact`]) before it is written, kept for
//! [`PAYLOAD_RETENTION_DAYS`] days, and pruned to a byte budget
//! ([`prune`]).
//!
//! Writes never fail a turn: [`record_turn`] logs and returns. Reads go
//! through [`query`], a read-only connection with a two-second cap, a single
//! statement, no `ATTACH` and at most [`MAX_ROWS`] rows.
//!
//! The reference point for the schema is `docs/data-store.md`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use rusqlite::limits::Limit;
use rusqlite::types::ValueRef;
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params,
};
use serde_json::{Value, json};

pub mod redact;

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
    (
        3,
        "thread_parents",
        "ALTER TABLE threads ADD COLUMN parent_id TEXT;
    ALTER TABLE threads ADD COLUMN parent_title TEXT;
    CREATE INDEX threads_parent ON threads(parent_id);",
    ),
    (
        4,
        "request_and_protocol_telemetry",
        "CREATE TABLE provider_requests (
        id INTEGER PRIMARY KEY,
        thread_id TEXT NOT NULL,
        turn_id TEXT,
        round INTEGER,
        provider TEXT,
        model TEXT,
        endpoint_host TEXT,
        endpoint_path TEXT,
        method TEXT,
        attempt INTEGER,
        started_at TEXT NOT NULL,
        headers_at TEXT,
        finished_at TEXT,
        latency_ms INTEGER,
        status_code INTEGER,
        request_bytes INTEGER,
        response_bytes INTEGER,
        stream_event_count INTEGER,
        input_tokens INTEGER,
        output_tokens INTEGER,
        cached_tokens INTEGER,
        error_kind TEXT,
        request_payload_json TEXT,
        response_summary_json TEXT,
        payload_truncated INTEGER NOT NULL DEFAULT 0,
        redaction_version INTEGER NOT NULL DEFAULT 0
    );
    CREATE INDEX provider_requests_thread ON provider_requests(thread_id, started_at);
    CREATE INDEX provider_requests_started ON provider_requests(started_at);
    CREATE TABLE harness_rpc_messages (
        id INTEGER PRIMARY KEY,
        thread_id TEXT NOT NULL,
        turn_id TEXT,
        harness TEXT,
        protocol TEXT,
        direction TEXT NOT NULL,
        message_kind TEXT NOT NULL,
        rpc_id TEXT,
        method TEXT,
        at TEXT NOT NULL,
        latency_ms INTEGER,
        byte_size INTEGER,
        payload_json TEXT,
        payload_truncated INTEGER NOT NULL DEFAULT 0,
        redaction_version INTEGER NOT NULL DEFAULT 0
    );
    CREATE INDEX harness_rpc_messages_thread ON harness_rpc_messages(thread_id, at);
    CREATE INDEX harness_rpc_messages_at ON harness_rpc_messages(at);
    CREATE TABLE operational_events (
        id INTEGER PRIMARY KEY,
        thread_id TEXT NOT NULL,
        turn_id TEXT,
        sequence INTEGER NOT NULL,
        event_type TEXT NOT NULL,
        at TEXT NOT NULL,
        fields_json TEXT,
        original_bytes INTEGER,
        captured_bytes INTEGER,
        payload_truncated INTEGER NOT NULL DEFAULT 0,
        redaction_version INTEGER NOT NULL DEFAULT 0,
        UNIQUE(thread_id, sequence)
    );
    CREATE INDEX operational_events_thread ON operational_events(thread_id, at);
    CREATE INDEX operational_events_at ON operational_events(at);",
    ),
    (
        5,
        "command_receipts",
        "CREATE TABLE command_receipts (
        caller TEXT NOT NULL,
        client_request_id TEXT NOT NULL,
        action TEXT NOT NULL,
        args_hash TEXT NOT NULL,
        result_json TEXT NOT NULL,
        created_at TEXT NOT NULL,
        PRIMARY KEY (caller, client_request_id)
    );
    CREATE INDEX command_receipts_created ON command_receipts(created_at);",
    ),
    (
        6,
        "usage_cursors",
        "CREATE TABLE usage_cursors (
        thread_id TEXT NOT NULL,
        harness TEXT NOT NULL,
        series TEXT NOT NULL DEFAULT '',
        input_tokens INTEGER,
        output_tokens INTEGER,
        cached_tokens INTEGER,
        cache_write_tokens INTEGER,
        cost_usd REAL,
        updated_at TEXT,
        PRIMARY KEY (thread_id, harness, series)
    );",
    ),
    (
        7,
        "durable_telemetry",
        "ALTER TABLE turns ADD COLUMN duration_ms INTEGER;
    ALTER TABLE turns ADD COLUMN time_to_first_token_ms INTEGER;
    ALTER TABLE turns ADD COLUMN model_time_ms INTEGER;
    ALTER TABLE turns ADD COLUMN tool_time_ms INTEGER;
    ALTER TABLE turns ADD COLUMN context_tokens INTEGER;
    ALTER TABLE turns ADD COLUMN context_window INTEGER;
    ALTER TABLE turns ADD COLUMN stop_reason TEXT;
    ALTER TABLE tool_calls ADD COLUMN started_at TEXT;
    ALTER TABLE tool_calls ADD COLUMN duration_ms INTEGER;
    ALTER TABLE tool_calls ADD COLUMN output_bytes INTEGER;
    ALTER TABLE tool_calls ADD COLUMN truncated INTEGER;
    ALTER TABLE tool_calls ADD COLUMN mcp_server TEXT;
    ALTER TABLE tool_calls ADD COLUMN command_name TEXT;
    ALTER TABLE tool_calls ADD COLUMN error_kind TEXT;
    CREATE INDEX tool_calls_name ON tool_calls(tool_name);
    CREATE INDEX harness_rpc_messages_method ON harness_rpc_messages(method);
    CREATE TABLE compactions (
        id INTEGER PRIMARY KEY,
        thread_id TEXT NOT NULL,
        turn_id TEXT,
        harness TEXT,
        model TEXT,
        at TEXT NOT NULL,
        duration_ms INTEGER,
        trigger TEXT NOT NULL,
        mode TEXT,
        tokens_before INTEGER,
        tokens_after INTEGER,
        messages_before INTEGER,
        messages_after INTEGER,
        idle_secs_before INTEGER,
        count INTEGER,
        success INTEGER NOT NULL DEFAULT 1,
        error_kind TEXT,
        external_id TEXT,
        source TEXT,
        UNIQUE(thread_id, external_id)
    );
    CREATE INDEX compactions_thread ON compactions(thread_id, at);
    CREATE INDEX compactions_at ON compactions(at);
    CREATE VIEW daily_compactions AS
        SELECT date(at) AS day, harness, trigger,
               COUNT(*) AS compactions,
               SUM(CASE WHEN success = 0 THEN 1 ELSE 0 END) AS failed,
               SUM(tokens_before) AS tokens_before,
               SUM(tokens_after) AS tokens_after,
               SUM(CASE WHEN tokens_before IS NOT NULL AND tokens_after IS NOT NULL
                        THEN tokens_before - tokens_after END) AS reclaimed_tokens,
               AVG(duration_ms) AS mean_duration_ms
        FROM compactions
        GROUP BY day, harness, trigger;
    CREATE VIEW tool_failure_rates AS
        SELECT date(at) AS day, tool_name,
               COUNT(*) AS calls,
               SUM(CASE WHEN success = 0 THEN 1 ELSE 0 END) AS failures,
               CASE WHEN COUNT(*) > 0
                    THEN 1.0 * SUM(CASE WHEN success = 0 THEN 1 ELSE 0 END) / COUNT(*)
               END AS failure_rate,
               AVG(duration_ms) AS mean_duration_ms,
               SUM(CASE WHEN truncated = 1 THEN 1 ELSE 0 END) AS truncated
        FROM tool_calls
        WHERE at IS NOT NULL
        GROUP BY day, tool_name;
    CREATE VIEW turn_latency_daily AS
        SELECT date(started_at) AS day, harness, model,
               COUNT(*) AS turns,
               AVG(duration_ms) AS mean_duration_ms,
               AVG(time_to_first_token_ms) AS mean_time_to_first_token_ms,
               AVG(model_time_ms) AS mean_model_time_ms,
               AVG(tool_time_ms) AS mean_tool_time_ms,
               AVG(context_tokens) AS mean_context_tokens
        FROM turns
        WHERE duration_ms IS NOT NULL
        GROUP BY day, harness, model;",
    ),
    (
        8,
        "tool_calls_turn_index",
        "CREATE INDEX IF NOT EXISTS tool_calls_turn ON tool_calls(turn_id);",
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
    /// The thread that started this one, from its metadata. Kept so a usage
    /// read can roll a thread's delegates up without the app's session list.
    pub parent_id: Option<String>,
    /// The parent's title, when only a title was recovered (older threads).
    pub parent_title: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

/// One tool call, without its arguments or output.
#[derive(Debug, Clone, Default)]
pub struct ToolCallRow {
    pub tool_name: String,
    pub success: Option<bool>,
    pub exit_code: Option<i64>,
    /// When the call finished, from the transcript's message timestamp.
    pub at: Option<String>,
    /// When the call started, when a duration could be subtracted from `at`.
    pub started_at: Option<String>,
    pub duration_ms: Option<i64>,
    /// Bytes of the result text, not the text itself.
    pub output_bytes: Option<i64>,
    pub truncated: Option<bool>,
    /// The MCP server in an `mcp__<server>__<tool>` name.
    pub mcp_server: Option<String>,
    /// The first word of a shell command (`cargo`, `git`, `bun`), never the
    /// whole command.
    pub command_name: Option<String>,
    pub error_kind: Option<String>,
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
    /// Wall time of the whole turn, when the caller measured it.
    pub duration_ms: Option<i64>,
    pub time_to_first_token_ms: Option<i64>,
    pub model_time_ms: Option<i64>,
    pub tool_time_ms: Option<i64>,
    /// Tokens the model's context held at the turn's last measurement.
    pub context_tokens: Option<i64>,
    /// The model's context window in tokens, as the profile or harness said.
    pub context_window: Option<i64>,
    /// How the model's turn ended (`completed`, `cancelled`, `max_tokens`,
    /// `refused`, `recovery_failed`, …), when the caller knows more than
    /// `status`.
    pub stop_reason: Option<String>,
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
        // A thread's display harness may be its latest one, but pricing needs
        // the pair its turns actually ran. `model`/`model_harness` are present
        // only when every turn shared one harness and model; otherwise a row
        // with no reported cost stays unpriced rather than applying one rate
        // to another model's turns.
        GroupBy::Thread => (
            "t.thread_id AS thread, th.title AS title, \
             COALESCE(th.project, th.cwd, '') AS project, COALESCE(th.harness, t.harness) AS harness, \
             CASE WHEN COUNT(DISTINCT COALESCE(t.harness, '') || '/' || COALESCE(t.model, '')) = 1 \
                  THEN MAX(t.model) END AS model, \
             CASE WHEN COUNT(DISTINCT COALESCE(t.harness, '') || '/' || COALESCE(t.model, '')) = 1 \
                  THEN MAX(t.harness) END AS model_harness, \
             th.parent_id AS parent_id, th.parent_title AS parent_title",
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
                    AS turns_without_tokens, \
                SUM(CASE WHEN t.input_tokens IS NULL OR t.output_tokens IS NULL \
                    OR t.cached_tokens IS NULL OR t.cache_write_tokens IS NULL THEN 1 ELSE 0 END) \
                    AS turns_without_any_tokens, \
                SUM(CASE WHEN t.input_tokens IS NULL THEN 1 ELSE 0 END) \
                    AS turns_without_input_tokens, \
                SUM(CASE WHEN t.output_tokens IS NULL THEN 1 ELSE 0 END) \
                    AS turns_without_output_tokens, \
                SUM(CASE WHEN t.cached_tokens IS NULL THEN 1 ELSE 0 END) \
                    AS turns_without_cached_tokens, \
                SUM(CASE WHEN t.cache_write_tokens IS NULL THEN 1 ELSE 0 END) \
                    AS turns_without_cache_write_tokens, \
                SUM(CASE WHEN t.cost_usd IS NULL THEN 1 ELSE 0 END) AS turns_without_cost \
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
    // A column no row reported stays absent, never 0: an all-NULL group has no
    // total, not a total of nothing. Counts below always have a value.
    let sum_optional = |key: &str| {
        let mut total = 0.0;
        let mut any = false;
        for row in &result.rows {
            if let Some(value) = number(row, key) {
                any = true;
                total += value;
            }
        }
        any.then_some(total)
    };
    let totals = json!({
        "input_tokens": sum_optional("input_tokens"),
        "output_tokens": sum_optional("output_tokens"),
        "cached_tokens": sum_optional("cached_tokens"),
        "cache_write_tokens": sum_optional("cache_write_tokens"),
        "cost_usd": sum_optional("cost_usd"),
        "turns": sum("turns"),
        "turns_without_tokens": sum("turns_without_tokens"),
        "turns_without_any_tokens": sum("turns_without_any_tokens"),
        "turns_without_input_tokens": sum("turns_without_input_tokens"),
        "turns_without_output_tokens": sum("turns_without_output_tokens"),
        "turns_without_cached_tokens": sum("turns_without_cached_tokens"),
        "turns_without_cache_write_tokens": sum("turns_without_cache_write_tokens"),
        "turns_without_cost": sum("turns_without_cost"),
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
    // Retention is checked here rather than after every write: every writer
    // opens the store, and the pass itself runs at most once an hour.
    maybe_prune(&connection);
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

// ------------------------------------------------- links the engine runs

/// Links whose turn this process is running right now (`start_session`,
/// `message_session` and `fan_out` with no app attached,
/// `orchestration/local_threads.rs`). They carry `"engine": true` in the book,
/// and while one is live the app's whole-book save cannot take it over: the
/// app that attaches mid-turn did not start it and must not interrupt it.
static ENGINE_LIVE: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

fn engine_live() -> std::sync::MutexGuard<'static, HashSet<String>> {
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
const ENGINE_BEAT_STALE_MS: i64 = 45_000;

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

/// Stamp every link this process runs with the time, so a reader in another
/// process (or this one after a restart) can tell a running engine's link from
/// one whose engine died with the turn still `working`.
pub fn beat_engine_links() {
    let at = now_ms();
    for id in engine_live_links() {
        let _ = change_link(&id, |mut link| {
            link["engineBeat"] = json!(at);
            link
        });
    }
}

/// An engine's link still `working` that no live engine stamps any more: the
/// engine died mid-turn. This process's own live links never count.
fn orphaned_link(link: &Value, live: &HashSet<String>, at: i64) -> bool {
    if link.get("engine").and_then(Value::as_bool) != Some(true)
        || !is_active_link(link.get("state").and_then(Value::as_str))
        || link_id(link).is_some_and(|id| live.contains(id))
    {
        return false;
    }
    let stamped = link
        .get("engineBeat")
        .and_then(Value::as_i64)
        .or_else(|| link.get("createdAt").and_then(Value::as_i64))
        .unwrap_or(0);
    at - stamped > ENGINE_BEAT_STALE_MS
}

/// The link as the app would leave it after a restart.
fn interrupted_orphan(mut link: Value, at: i64) -> Value {
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
    let connection = open(path)?;
    let live = engine_live().clone();
    let at = now_ms();
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

fn is_final_link(state: Option<&str>) -> bool {
    matches!(state, Some("done" | "failed" | "cancelled"))
}

fn link_id(link: &Value) -> Option<&str> {
    link.get("id").and_then(Value::as_str)
}

/// The app's whole-book save, kept from undoing what the engine wrote to a
/// link it runs: a live one stays as the engine has it (the app's copy of it is
/// from before the turn ended, or `interrupted` because the app attached
/// mid-turn), and a settled one is not taken back to a state that is not
/// final. A live link the app's copy lacks (it loaded the book before the
/// engine opened the link) is kept too.
fn protect_engine_links(
    mut incoming: Vec<Value>,
    stored: &[Value],
    live: &HashSet<String>,
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
                if live.contains(id) || settled_here {
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
fn merge_counters(incoming: Option<&Value>, stored: &Value) -> Value {
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

fn read_links(transaction: &Connection) -> Result<Vec<Value>, String> {
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

fn read_state(transaction: &Transaction<'_>, key: &str) -> Result<Value, String> {
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

fn write_state(transaction: &Transaction<'_>, key: &str, value: &Value) -> Result<(), String> {
    transaction
        .execute(
            "INSERT OR REPLACE INTO orchestration_state (key, body) VALUES (?1, ?2)",
            params![key, value.to_string()],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// One link's row, as `save_link_book_at` writes it.
fn write_link(transaction: &Transaction<'_>, link: &Value) -> Result<(), String> {
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

fn immediate(connection: &Connection) -> Result<Transaction<'_>, String> {
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
    let protected = protect_engine_links(incoming, &stored_links, &live);
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
    Ok(json!({ "stored": stored, "book": book, "engineLive": engine_live_links() }))
}

// ------------------------------------------------------------- command receipts

/// One receipt: what a caller's earlier orchestration call with the same retry
/// key did. It keeps the action, a hash of the arguments and the answer —
/// never the arguments themselves, so a message or a prompt is not stored.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReceiptRow {
    /// The thread that made the call.
    pub caller: String,
    /// The caller's retry key: the `client_request_id` it sent.
    pub client_request_id: String,
    /// The orchestration tool: `start_session`, `message_session` or `fan_out`.
    pub action: String,
    /// sha256 of the action and the call's canonical arguments JSON.
    pub args_hash: String,
    /// The answer the call gave, as JSON.
    pub result_json: String,
}

/// What a caller's earlier call under the same retry key left behind.
#[derive(Debug, Clone, PartialEq)]
pub enum Receipt {
    /// Nothing under that key: never stored, or aged out of the retry window.
    /// The call may run.
    None,
    /// The same call: this is what it answered, and it must not run again.
    Replay { result: Value, created_at: String },
    /// The key was used for another action or for other arguments.
    Conflict,
}

/// [`check_receipt`] against one store file, so a test can use a temporary one.
pub fn check_receipt_at(
    path: &Path,
    caller: &str,
    client_request_id: &str,
    action: &str,
    args_hash: &str,
) -> Result<Receipt, String> {
    let connection = open(path)?;
    let row: Option<(String, String, String, String)> = connection
        .query_row(
            "SELECT action, args_hash, result_json, created_at FROM command_receipts
             WHERE caller = ?1 AND client_request_id = ?2",
            params![caller, client_request_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some((stored_action, stored_hash, result, created_at)) = row else {
        return Ok(Receipt::None);
    };
    // The action is compared as well as the hash: the hash covers the action
    // already, and a stored row from an older release may not.
    if stored_action != action || stored_hash != args_hash {
        return Ok(Receipt::Conflict);
    }
    Ok(Receipt::Replay {
        result: serde_json::from_str(&result).unwrap_or(Value::Null),
        created_at,
    })
}

/// [`check_receipt`] against the store this machine uses.
pub fn check_receipt(
    caller: &str,
    client_request_id: &str,
    action: &str,
    args_hash: &str,
) -> Result<Receipt, String> {
    let path = data_path().ok_or_else(|| "no config directory".to_owned())?;
    check_receipt_at(&path, caller, client_request_id, action, args_hash)
}

/// [`record_receipt`] against one store file, so a test can use a temporary one.
pub fn record_receipt_at(path: &Path, receipt: &ReceiptRow) -> Result<(), String> {
    let connection = open(path)?;
    write_receipt(&connection, receipt)
}

/// [`record_receipt`] against the store this machine uses.
pub fn record_receipt(receipt: &ReceiptRow) -> Result<(), String> {
    let path = data_path().ok_or_else(|| "no config directory".to_owned())?;
    record_receipt_at(&path, receipt)
}

fn write_receipt(connection: &Connection, receipt: &ReceiptRow) -> Result<(), String> {
    if receipt.caller.trim().is_empty() || receipt.client_request_id.trim().is_empty() {
        return Err("a receipt needs a caller and a client_request_id".to_owned());
    }
    if receipt.result_json.len() > MAX_RECEIPT_RESULT_BYTES {
        return Err(format!(
            "the receipt's result is {} bytes; at most {MAX_RECEIPT_RESULT_BYTES} are kept",
            receipt.result_json.len()
        ));
    }
    let created_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    // The first receipt under a key stays: a second write is either a retry
    // that slipped past the check or a race, and the call that already
    // happened is the one to replay.
    connection
        .execute(
            "INSERT INTO command_receipts
                (caller, client_request_id, action, args_hash, result_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(caller, client_request_id) DO NOTHING",
            params![
                receipt.caller,
                receipt.client_request_id,
                receipt.action,
                receipt.args_hash,
                receipt.result_json,
                created_at,
            ],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// Drops receipts past the retry window, then the oldest until at most
/// `max_rows` are left. Called by the telemetry prune pass.
fn prune_receipts(connection: &Connection, days: i64, max_rows: i64) -> Result<usize, String> {
    let cutoff = rfc3339_days_ago(chrono::Utc::now(), days);
    let mut deleted = connection
        .execute(
            "DELETE FROM command_receipts WHERE created_at < ?1",
            [cutoff],
        )
        .map_err(|error| error.to_string())?;
    deleted += connection
        .execute(
            "DELETE FROM command_receipts WHERE rowid NOT IN
                (SELECT rowid FROM command_receipts ORDER BY created_at DESC, rowid DESC LIMIT ?1)",
            [max_rows],
        )
        .map_err(|error| error.to_string())?;
    Ok(deleted)
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
/// `usage_update`, or its own log): the row keeps the growth over the last
/// total the same session series reported, not over the sum of what was
/// already recorded. `series` names that series (the harness's native session
/// id, with the account it ran as, see [`record_cumulative_turn`]'s caller): a
/// different one is a restarted or switched harness whose whole total is this
/// turn's.
///
/// A turn that reports the same total as the one before it records zero, and a
/// counter that dipped alone records zero rather than a whole session. Only a
/// new series, or every reported counter dropping together, is a restart.
pub fn record_cumulative_turn(turn: &TurnRow, cumulative: SessionUsage, series: Option<&str>) {
    if turn.thread_id.trim().is_empty() || turn.started_at.trim().is_empty() {
        return;
    }
    if rustcode_session::is_scratch_session_id(&turn.thread_id) {
        return;
    }
    let Some(path) = data_path() else { return };
    let written = open(&path)
        .and_then(|connection| write_cumulative_turn(&connection, turn, cumulative, series));
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
    series: Option<&str>,
) -> Result<(), String> {
    let harness = turn.harness.clone().unwrap_or_default();
    let series = series.unwrap_or_default();
    // IMMEDIATE, not the default DEFERRED: the cursor is read before the turn
    // is written, and a read snapshot another writer commits past cannot be
    // upgraded — SQLite answers `SQLITE_BUSY_SNAPSHOT` without retrying it, so
    // a concurrent turn would be lost instead of waiting its turn.
    let transaction = Transaction::new_unchecked(connection, TransactionBehavior::Immediate)
        .map_err(|error| error.to_string())?;
    let baseline = baseline(&transaction, &turn.thread_id, &harness, series, &cumulative)?;
    let (spent, next) = advance(&cumulative, baseline.as_ref());
    let mut row = turn.clone();
    row.input_tokens = spent.tokens.input;
    row.output_tokens = spent.tokens.output;
    row.cached_tokens = spent.tokens.cached;
    row.cache_write_tokens = spent.tokens.cache_write;
    row.cost_usd = spent.cost_usd;
    write_turn_in(&transaction, &row)?;
    move_cursor(&transaction, &turn.thread_id, &harness, series, &next, turn)?;
    transaction.commit().map_err(|error| error.to_string())
}

/// The total this turn's reading is measured against.
///
/// - The last total the same session series reported, when the store has it.
/// - `None` when the store holds cursors for this thread and harness but not
///   this series: a restarted or switched harness, whose own total is this
///   turn's.
/// - Otherwise the thread's rows predate cursors (an older release, or a
///   backfill) and their sum is the baseline its last row was written against.
///   That sum can already be inflated (#3169), so the lower of it and this
///   reading is used: the first turn after the release counts nothing it had
///   already counted, rather than adding one more whole total on top. The cost
///   is that a restart that happened between those rows and this reading is not
///   counted, which undercounts one turn instead of double counting a session.
fn baseline(
    connection: &Connection,
    thread_id: &str,
    harness: &str,
    series: &str,
    cumulative: &SessionUsage,
) -> Result<Option<SessionUsage>, String> {
    if let Some(stored) = stored_cursor(connection, thread_id, harness, series)? {
        return Ok(Some(stored));
    }
    if has_cursor(connection, thread_id, harness)? {
        return Ok(None);
    }
    let sum = recorded(connection, thread_id, harness)?;
    Ok(Some(lower_of(sum, cumulative)))
}

/// The cursor row for one session series, when the store has it.
fn stored_cursor(
    connection: &Connection,
    thread_id: &str,
    harness: &str,
    series: &str,
) -> Result<Option<SessionUsage>, String> {
    let stored: Option<(
        Option<i64>,
        Option<i64>,
        Option<i64>,
        Option<i64>,
        Option<f64>,
    )> = connection
        .query_row(
            "SELECT input_tokens, output_tokens, cached_tokens, cache_write_tokens, cost_usd
                 FROM usage_cursors WHERE thread_id = ?1 AND harness = ?2 AND series = ?3",
            params![thread_id, harness, series],
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
        .optional()
        .map_err(|error| error.to_string())?;
    Ok(stored.map(
        |(input, output, cached, cache_write, cost_usd)| SessionUsage {
            tokens: TokenTotals {
                input,
                output,
                cached,
                cache_write,
            },
            cost_usd,
            at: None,
        },
    ))
}

/// Whether any series of this thread and harness has a cursor.
fn has_cursor(connection: &Connection, thread_id: &str, harness: &str) -> Result<bool, String> {
    connection
        .query_row(
            "SELECT 1 FROM usage_cursors WHERE thread_id = ?1 AND harness = ?2 LIMIT 1",
            params![thread_id, harness],
            |_| Ok(()),
        )
        .optional()
        .map(|found| found.is_some())
        .map_err(|error| error.to_string())
}

/// Per counter, the lower of a thread's recorded sum and a reading of it.
fn lower_of(sum: SessionUsage, cumulative: &SessionUsage) -> SessionUsage {
    let lower = |sum: Option<i64>, now: Option<i64>| match (sum, now) {
        (Some(sum), Some(now)) => Some(sum.min(now)),
        (sum, _) => sum,
    };
    SessionUsage {
        tokens: TokenTotals {
            input: lower(sum.tokens.input, cumulative.tokens.input),
            output: lower(sum.tokens.output, cumulative.tokens.output),
            cached: lower(sum.tokens.cached, cumulative.tokens.cached),
            cache_write: lower(sum.tokens.cache_write, cumulative.tokens.cache_write),
        },
        cost_usd: match (sum.cost_usd, cumulative.cost_usd) {
            (Some(sum), Some(now)) => Some(sum.min(now)),
            (sum, _) => sum,
        },
        at: None,
    }
}

/// What one turn spent, and where its cursor goes next.
///
/// `None` is a series the store has not seen: it reports a whole session total,
/// which is this turn's, and the cursor starts there. Within one series a
/// counter that is gone stays gone, a counter that rose grew by the difference,
/// and a counter that fell is a restart when every reported counter fell with
/// it (a new session in the same log) — otherwise a log the harness rewrote
/// (rotated, truncated, an account directory that went missing): that counter
/// records nothing and keeps the higher cursor, so the next reading above it
/// grows from there. Recording the whole total then would re-count a session
/// whose other counters say it never restarted.
fn advance(
    cumulative: &SessionUsage,
    baseline: Option<&SessionUsage>,
) -> (SessionUsage, SessionUsage) {
    let Some(baseline) = baseline else {
        return (cumulative.clone(), cumulative.clone());
    };
    let restart = all_reported_dropped(cumulative, baseline);
    let step = |now: Option<i64>, base: Option<i64>| match (now, base) {
        (None, base) => (None, base),
        (Some(now), None) => (Some(now), Some(now)),
        (Some(now), Some(_)) if restart => (Some(now), Some(now)),
        (Some(now), Some(base)) if now >= base => (Some(now - base), Some(now)),
        (Some(_), Some(base)) => (Some(0), Some(base)),
    };
    let cost = match (cumulative.cost_usd, baseline.cost_usd) {
        (None, base) => (None, base),
        (Some(now), None) => (Some(now), Some(now)),
        (Some(now), Some(_)) if restart => (Some(now), Some(now)),
        (Some(now), Some(base)) if now >= base => (Some(now - base), Some(now)),
        (Some(_), Some(base)) => (Some(0.0), Some(base)),
    };
    let (input, next_input) = step(cumulative.tokens.input, baseline.tokens.input);
    let (output, next_output) = step(cumulative.tokens.output, baseline.tokens.output);
    let (cached, next_cached) = step(cumulative.tokens.cached, baseline.tokens.cached);
    let (cache_write, next_cache_write) =
        step(cumulative.tokens.cache_write, baseline.tokens.cache_write);
    (
        SessionUsage {
            tokens: TokenTotals {
                input,
                output,
                cached,
                cache_write,
            },
            cost_usd: cost.0,
            at: cumulative.at.clone(),
        },
        SessionUsage {
            tokens: TokenTotals {
                input: next_input,
                output: next_output,
                cached: next_cached,
                cache_write: next_cache_write,
            },
            cost_usd: cost.1,
            at: cumulative.at.clone(),
        },
    )
}

/// Whether every counter both sides reported fell: a session that restarted
/// inside the same log. A single counter that fell while another rose is a log
/// the harness rewrote, not a restart.
fn all_reported_dropped(cumulative: &SessionUsage, baseline: &SessionUsage) -> bool {
    let pairs = [
        (cumulative.tokens.input, baseline.tokens.input),
        (cumulative.tokens.output, baseline.tokens.output),
        (cumulative.tokens.cached, baseline.tokens.cached),
        (cumulative.tokens.cache_write, baseline.tokens.cache_write),
    ];
    let (mut reported, mut dropped) = (0, 0);
    for (now, base) in pairs {
        if let (Some(now), Some(base)) = (now, base) {
            reported += 1;
            dropped += i64::from(now < base);
        }
    }
    let (cost_reported, cost_dropped) = match (cumulative.cost_usd, baseline.cost_usd) {
        (Some(now), Some(base)) => (1, i64::from(now < base)),
        _ => (0, 0),
    };
    reported += cost_reported;
    dropped += cost_dropped;
    reported > 0 && dropped == reported
}

/// Writes the cursor this turn leaves behind, in the same transaction as the
/// turn, so a failed write leaves it where it was.
fn move_cursor(
    connection: &Connection,
    thread_id: &str,
    harness: &str,
    series: &str,
    next: &SessionUsage,
    turn: &TurnRow,
) -> Result<(), String> {
    connection
        .execute(
            "INSERT INTO usage_cursors (thread_id, harness, series, input_tokens,
                    output_tokens, cached_tokens, cache_write_tokens, cost_usd, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(thread_id, harness, series) DO UPDATE SET
                input_tokens = excluded.input_tokens,
                output_tokens = excluded.output_tokens,
                cached_tokens = excluded.cached_tokens,
                cache_write_tokens = excluded.cache_write_tokens,
                cost_usd = excluded.cost_usd,
                updated_at = excluded.updated_at",
            params![
                thread_id,
                harness,
                series,
                next.tokens.input,
                next.tokens.output,
                next.tokens.cached,
                next.tokens.cache_write,
                next.cost_usd,
                turn.finished_at
                    .clone()
                    .or_else(|| Some(turn.started_at.clone())),
            ],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// What the store already holds for one thread on one harness: the sum of its
/// recorded rows, which is the baseline a thread recorded before cursors
/// existed was written against. [`baseline`] takes the lower of it and a live
/// reading, because such a sum can already be inflated.
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
    write_turn_in(&transaction, turn)?;
    transaction.commit().map_err(|error| error.to_string())
}

/// [`write_turn`] inside a transaction the caller already opened, so a turn and
/// its usage cursor move together.
fn write_turn_in(transaction: &Connection, turn: &TurnRow) -> Result<(), String> {
    let thread = live_thread(turn);
    upsert_thread(transaction, &thread)?;
    transaction
        .execute(
            "INSERT INTO turns (thread_id, started_at, finished_at, harness, model,
                    input_tokens, output_tokens, cached_tokens, cache_write_tokens,
                    cost_usd, tool_calls, status, duration_ms, time_to_first_token_ms,
                    model_time_ms, tool_time_ms, context_tokens, context_window, stop_reason)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                     ?15, ?16, ?17, ?18, ?19)
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
                status = excluded.status,
                duration_ms = COALESCE(excluded.duration_ms, turns.duration_ms),
                time_to_first_token_ms = COALESCE(
                    excluded.time_to_first_token_ms, turns.time_to_first_token_ms),
                model_time_ms = COALESCE(excluded.model_time_ms, turns.model_time_ms),
                tool_time_ms = COALESCE(excluded.tool_time_ms, turns.tool_time_ms),
                context_tokens = COALESCE(excluded.context_tokens, turns.context_tokens),
                context_window = COALESCE(excluded.context_window, turns.context_window),
                stop_reason = COALESCE(excluded.stop_reason, turns.stop_reason)",
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
                turn.duration_ms,
                turn.time_to_first_token_ms,
                turn.model_time_ms,
                turn.tool_time_ms,
                turn.context_tokens,
                turn.context_window,
                turn.stop_reason,
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
        let mcp_server = call
            .mcp_server
            .clone()
            .or_else(|| mcp_server_of(&call.tool_name).map(str::to_owned));
        let command_name = call.command_name.clone();
        transaction
            .execute(
                "INSERT INTO tool_calls (thread_id, turn_id, tool_name, success, exit_code, at,
                        started_at, duration_ms, output_bytes, truncated, mcp_server,
                        command_name, error_kind)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    turn.thread_id,
                    turn_id,
                    call.tool_name,
                    call.success,
                    call.exit_code,
                    call.at,
                    call.started_at,
                    call.duration_ms,
                    call.output_bytes,
                    call.truncated,
                    mcp_server,
                    command_name,
                    call.error_kind,
                ],
            )
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// The server in an `mcp__<server>__<tool>` tool name, if it is one.
fn mcp_server_of(tool_name: &str) -> Option<&str> {
    let rest = tool_name.strip_prefix("mcp__")?;
    let (server, _) = rest.split_once("__")?;
    (!server.is_empty()).then_some(server)
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
    let metadata = crate::config::load_session_metadata(id);
    let created_at = record
        .as_ref()
        .map(|record| record.created_at.clone())
        .or_else(|| {
            metadata
                .as_ref()
                .map(|metadata| millis_to_rfc3339(metadata.created_at_ms as i64))
        });
    // Only a delegate rolls up into its parent's statistics: a fork or a side
    // chat is the user's own thread, with its own cost.
    let parent = metadata
        .as_ref()
        .and_then(|metadata| metadata.parent.as_ref())
        .filter(|parent| parent.relationship() == rustcode_session::SessionRelationship::Delegate);
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
        parent_id: parent.and_then(|parent| parent.id.clone()),
        parent_title: parent.and_then(|parent| parent.title.clone()),
        created_at,
        updated_at: turn.finished_at.clone(),
    }
}

fn upsert_thread(connection: &Connection, thread: &ThreadRow) -> Result<(), String> {
    connection
        .execute(
            "INSERT INTO threads (id, title, project, cwd, harness, model, effort,
                    parent_id, parent_title, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(id) DO UPDATE SET
                title = COALESCE(NULLIF(excluded.title, ''), threads.title),
                project = COALESCE(excluded.project, threads.project),
                cwd = COALESCE(excluded.cwd, threads.cwd),
                harness = COALESCE(excluded.harness, threads.harness),
                model = COALESCE(excluded.model, threads.model),
                effort = COALESCE(excluded.effort, threads.effort),
                parent_id = COALESCE(excluded.parent_id, threads.parent_id),
                parent_title = COALESCE(excluded.parent_title, threads.parent_title),
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
                thread.parent_id,
                thread.parent_title,
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
    parent_id: Option<String>,
    parent_title: Option<String>,
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
    #[serde(default)]
    parent: Option<rustcode_session::SessionParent>,
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
    let parent = created
        .as_ref()
        .and_then(|file| file.parent.clone())
        .filter(|parent| parent.relationship() == rustcode_session::SessionRelationship::Delegate);
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
        parent_id: parent.as_ref().and_then(|parent| parent.id.clone()),
        parent_title: parent.as_ref().and_then(|parent| parent.title.clone()),
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
            parent_id: facts.parent_id.clone(),
            parent_title: facts.parent_title.clone(),
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
/// the previous one (v2: token counts from Claude Code and Pi logs, cost;
/// v3: parent links): the pass runs again and rewrites only rows still marked
/// `backfill`.
const BACKFILL_VERSION: i64 = 3;

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

/// One `store_meta` value, for a pass that must remember it ran once
/// (`docs/data-store.md` › Schema).
pub fn meta(key: &str) -> Option<String> {
    let path = data_path()?;
    let connection = open(&path).ok()?;
    connection
        .query_row(
            "SELECT value FROM store_meta WHERE key = ?1",
            [key],
            |row| row.get(0),
        )
        .optional()
        .ok()
        .flatten()
}

/// Remember that a one-time pass ran (`meta`).
pub fn set_meta(key: &str, value: &str) -> Result<(), String> {
    let path = data_path().ok_or_else(|| "no config directory".to_owned())?;
    let connection = open(&path)?;
    connection
        .execute(
            "INSERT INTO store_meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )
        .map(|_| ())
        .map_err(|error| error.to_string())
}

// ---------------------------------------------------------------- telemetry

/// The most of one provider request or response payload a row keeps. Applied
/// again at the insert, after redaction, so a caller's mistake cannot write
/// more than this.
pub const MAX_PROVIDER_PAYLOAD_BYTES: usize = 64 * 1024;

/// The most of one JSON-RPC message a row keeps.
pub const MAX_RPC_PAYLOAD_BYTES: usize = 16 * 1024;

/// The most of one `operational_events.fields_json` a row keeps.
pub const MAX_EVENT_FIELDS_BYTES: usize = 8 * 1024;

/// How long a row keeps its payload. After this the payload columns are set
/// to NULL and the metadata row stays.
pub const PAYLOAD_RETENTION_DAYS: i64 = 7;

/// Payloads of streaming deltas and repeated status pings are dropped on
/// every pass, whatever their age: they are the bulk of the store and carry
/// no question an agent asks. The metadata row stays.
pub const NOISE_METHODS: [&str; 2] = [
    "account/rateLimits/updated",
    "mcpServer/startupStatus/updated",
];

/// A streaming row's payload above this size is a raw frame body, not the
/// `{coalesced, update, bytes}` metadata a recorder writes for a folded chunk
/// (`harness/rpc_capture.rs`). Only the raw body is noise; the metadata is how
/// the folded row can be read at all and is kept.
pub const STREAMED_PAYLOAD_BODY_BYTES: i64 = 512;

/// How long a command receipt stays usable: an orchestration call retried
/// inside this window is answered from its receipt, and one retried later is
/// a new call, like one that carried no retry key.
pub const COMMAND_RECEIPT_RETENTION_DAYS: i64 = 7;

/// The most receipts the store keeps. The oldest go first, so the retry
/// window is bounded in rows as well as in days.
pub const COMMAND_RECEIPT_MAX_ROWS: i64 = 10_000;

/// The most of one receipt's recorded result a row keeps: a `start_session`
/// or `fan_out` answer is a few hundred bytes, so more is a mistake.
pub const MAX_RECEIPT_RESULT_BYTES: usize = 16 * 1024;

/// What `provider_requests` and `harness_rpc_messages` may hold together in
/// payload bytes before the oldest payloads are dropped.
pub const TELEMETRY_MAX_BYTES: i64 = 256 * 1024 * 1024;

/// The `direction` of a message that came from the harness.
pub const DIRECTION_HARNESS_TO_RUSTCODE: &str = "harness_to_rustcode";
/// The `direction` of a message RustCode sent to the harness.
pub const DIRECTION_RUSTCODE_TO_HARNESS: &str = "rustcode_to_harness";
/// The `message_kind` of a JSON-RPC request.
pub const KIND_REQUEST: &str = "request";
/// The `message_kind` of a JSON-RPC response.
pub const KIND_RESPONSE: &str = "response";
/// The `message_kind` of a JSON-RPC notification.
pub const KIND_NOTIFICATION: &str = "notification";

/// `store_meta` key holding when the last pass ran, in Unix seconds.
const PRUNE_META_KEY: &str = "telemetry_pruned_at";
const PRUNE_INTERVAL_SECS: i64 = 60 * 60;

/// One compaction of a thread's history, from whichever side ran it.
///
/// `trigger` says why (`auto_context`, `manual`, `cache_expiry`,
/// `harness_auto`); `mode` how (`ai_summary`, `deterministic_record`,
/// `harness_summary`). Token counts stay `NULL` when the source did not say,
/// never `0`. `external_id` is the harness's own id for the event, so a
/// repeated protocol update updates its row instead of adding another one.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CompactionRow {
    pub thread_id: String,
    pub turn_id: Option<String>,
    pub harness: Option<String>,
    pub model: Option<String>,
    pub at: String,
    pub duration_ms: Option<i64>,
    pub trigger: String,
    pub mode: Option<String>,
    pub tokens_before: Option<i64>,
    pub tokens_after: Option<i64>,
    pub messages_before: Option<i64>,
    pub messages_after: Option<i64>,
    pub idle_secs_before: Option<i64>,
    /// How many compactions the session had performed, this one included.
    pub count: Option<i64>,
    pub success: bool,
    pub error_kind: Option<String>,
    pub external_id: Option<String>,
    /// Where the record came from: `rustcode`, `codex`, `claude-code`, …
    pub source: Option<String>,
}

/// One built-in provider HTTP attempt: what was asked, where, how long it
/// took, and — only when `capture_provider_payloads` is on — a redacted,
/// capped copy of the request.
///
/// `endpoint_host` and `endpoint_path` are stored without the query or
/// fragment, which can carry a key. The payload fields are stored as given:
/// redact them with [`redact::prepare_payload`] before setting them, and the
/// insert clips them again if they are still over the cap. The insert also
/// redacts them again, so a caller that forgot cannot write a secret.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProviderRequestRow {
    pub thread_id: String,
    pub turn_id: Option<String>,
    /// The request round within the turn, when the caller counts them.
    pub round: Option<i64>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub endpoint_host: Option<String>,
    pub endpoint_path: Option<String>,
    pub method: Option<String>,
    /// 1 for the first send, 2 for the first retry, and so on.
    pub attempt: Option<i64>,
    pub started_at: String,
    /// When the response headers arrived, if they did.
    pub headers_at: Option<String>,
    pub finished_at: Option<String>,
    pub latency_ms: Option<i64>,
    pub status_code: Option<i64>,
    pub request_bytes: Option<i64>,
    pub response_bytes: Option<i64>,
    pub stream_event_count: Option<i64>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cached_tokens: Option<i64>,
    /// A short classification (`timeout`, `http_500`, `stream_failure`), never
    /// the provider's message text.
    pub error_kind: Option<String>,
    pub request_payload_json: Option<String>,
    /// A summary of the response, not the raw body.
    pub response_summary_json: Option<String>,
    /// Set by the clip; a caller that clipped already may set it too.
    pub payload_truncated: bool,
}

/// One JSON-RPC message that crossed RustCode's ACP or Codex stdio boundary.
/// These are harness-to-RustCode protocol messages: an external harness's own
/// provider HTTP traffic never passes through here.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RpcMessageRow {
    pub thread_id: String,
    pub turn_id: Option<String>,
    pub harness: Option<String>,
    /// `acp` or `codex-app-server`, when the caller knows.
    pub protocol: Option<String>,
    /// [`DIRECTION_HARNESS_TO_RUSTCODE`] or [`DIRECTION_RUSTCODE_TO_HARNESS`].
    pub direction: String,
    /// [`KIND_REQUEST`], [`KIND_RESPONSE`] or [`KIND_NOTIFICATION`].
    pub message_kind: String,
    /// The JSON-RPC id as text, so an integer and a string id both fit.
    pub rpc_id: Option<String>,
    pub method: Option<String>,
    pub at: String,
    /// A request's answer time, when the caller could pair them.
    pub latency_ms: Option<i64>,
    pub byte_size: Option<i64>,
    pub payload_json: Option<String>,
    pub payload_truncated: bool,
}

/// One allowlisted operational event, projected for an agent to read.
///
/// The sequence is per thread and assigned by the store: it is the cursor a
/// reader pages with, so it is monotonic even when two writers race.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OperationalEventRow {
    pub thread_id: String,
    pub turn_id: Option<String>,
    pub event_type: String,
    pub at: String,
    /// Safe, bounded fields only: a provider error's text does not belong here.
    pub fields_json: Option<String>,
    pub original_bytes: Option<i64>,
    /// Set by the caller, or by the clip when the fields were too long.
    pub captured_bytes: Option<i64>,
    pub payload_truncated: bool,
}

/// Records one provider attempt. A failure is logged and swallowed: a turn
/// must not fail because telemetry storage is locked or full.
pub fn record_provider_request(row: &ProviderRequestRow) {
    if !wants_row(&row.thread_id, &row.started_at) {
        return;
    }
    let Some(path) = data_path() else { return };
    if let Err(error) = record_provider_request_at(&path, row) {
        crate::dbg_log!(
            "data store: provider request of {} not recorded: {error}",
            row.thread_id
        );
    }
}

/// [`record_provider_request`] against one store file (tests, backfill).
pub fn record_provider_request_at(path: &Path, row: &ProviderRequestRow) -> Result<(), String> {
    let connection = open(path)?;
    write_provider_request(&connection, row)
}

fn write_provider_request(connection: &Connection, row: &ProviderRequestRow) -> Result<(), String> {
    if !wants_row(&row.thread_id, &row.started_at) {
        return Ok(());
    }
    let (request_payload, request_clipped) = capped_payload(
        row.request_payload_json.as_deref(),
        MAX_PROVIDER_PAYLOAD_BYTES,
    );
    let (response_summary, response_clipped) = capped_payload(
        row.response_summary_json.as_deref(),
        MAX_PROVIDER_PAYLOAD_BYTES,
    );
    let payload_truncated = row.payload_truncated || request_clipped || response_clipped;
    let redaction_version =
        redaction_version(request_payload.as_deref(), response_summary.as_deref());
    connection
        .execute(
            "INSERT INTO provider_requests (thread_id, turn_id, round, provider, model,
                    endpoint_host, endpoint_path, method, attempt, started_at, headers_at,
                    finished_at, latency_ms, status_code, request_bytes, response_bytes,
                    stream_event_count, input_tokens, output_tokens, cached_tokens, error_kind,
                    request_payload_json, response_summary_json, payload_truncated,
                    redaction_version)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
                    ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25)",
            params![
                row.thread_id,
                row.turn_id,
                row.round,
                row.provider,
                row.model,
                row.endpoint_host,
                row.endpoint_path,
                row.method,
                row.attempt,
                at_rfc3339(&row.started_at),
                row.headers_at.as_deref().map(at_rfc3339),
                row.finished_at.as_deref().map(at_rfc3339),
                row.latency_ms,
                row.status_code,
                row.request_bytes,
                row.response_bytes,
                row.stream_event_count,
                row.input_tokens,
                row.output_tokens,
                row.cached_tokens,
                row.error_kind,
                request_payload,
                response_summary,
                payload_truncated,
                redaction_version,
            ],
        )
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// Records one JSON-RPC message. Logged and swallowed, like the others.
pub fn record_rpc_message(row: &RpcMessageRow) {
    record_rpc_messages(std::slice::from_ref(row));
}

/// Records messages that crossed the harness boundary in one transaction, so
/// a busy turn pays for one open and one commit. Rows with no thread, no
/// direction or no kind are skipped; a failure is logged and swallowed.
pub fn record_rpc_messages(rows: &[RpcMessageRow]) {
    let rows: Vec<&RpcMessageRow> = rows
        .iter()
        .filter(|row| wants_row(&row.thread_id, &row.at))
        .filter(|row| !row.direction.trim().is_empty() && !row.message_kind.trim().is_empty())
        .collect();
    if rows.is_empty() {
        return;
    }
    let Some(path) = data_path() else { return };
    if let Err(error) = record_rpc_messages_at(&path, &rows) {
        crate::dbg_log!(
            "data store: {} rpc message(s) not recorded: {error}",
            rows.len()
        );
    }
}

/// [`record_rpc_messages`] against one store file (tests).
pub fn record_rpc_messages_at(path: &Path, rows: &[&RpcMessageRow]) -> Result<usize, String> {
    if rows.is_empty() {
        return Ok(0);
    }
    let connection = open(path)?;
    write_rpc_messages(&connection, rows)
}

fn write_rpc_messages(connection: &Connection, rows: &[&RpcMessageRow]) -> Result<usize, String> {
    let transaction = connection
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    let mut written = 0usize;
    {
        let mut insert = transaction
            .prepare_cached(
                "INSERT INTO harness_rpc_messages (thread_id, turn_id, harness, protocol,
                        direction, message_kind, rpc_id, method, at, latency_ms, byte_size,
                        payload_json, payload_truncated, redaction_version)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            )
            .map_err(|error| error.to_string())?;
        for row in rows {
            if !wants_row(&row.thread_id, &row.at)
                || row.direction.trim().is_empty()
                || row.message_kind.trim().is_empty()
            {
                continue;
            }
            let (payload, clipped) =
                capped_payload(row.payload_json.as_deref(), MAX_RPC_PAYLOAD_BYTES);
            let redaction_version = redaction_version(payload.as_deref(), None);
            insert
                .execute(params![
                    row.thread_id,
                    row.turn_id,
                    row.harness,
                    row.protocol,
                    row.direction,
                    row.message_kind,
                    row.rpc_id,
                    row.method,
                    at_rfc3339(&row.at),
                    row.latency_ms,
                    row.byte_size,
                    payload,
                    row.payload_truncated || clipped,
                    redaction_version,
                ])
                .map_err(|error| error.to_string())?;
            written += 1;
        }
    }
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(written)
}

/// Records one operational event and answers its per-thread sequence. Logged
/// and swallowed: returns 0 when it was not written.
pub fn record_operational_event(row: &OperationalEventRow) -> i64 {
    if !wants_row(&row.thread_id, &row.at) || row.event_type.trim().is_empty() {
        return 0;
    }
    let Some(path) = data_path() else { return 0 };
    match record_operational_event_at(&path, row) {
        Ok(sequence) => sequence,
        Err(error) => {
            crate::dbg_log!(
                "data store: {} event of {} not recorded: {error}",
                row.event_type,
                row.thread_id
            );
            0
        }
    }
}

/// [`record_operational_event`] against one store file (tests), returning the
/// sequence it was given.
pub fn record_operational_event_at(path: &Path, row: &OperationalEventRow) -> Result<i64, String> {
    let connection = open(path)?;
    write_operational_event(&connection, row)
}

fn write_operational_event(
    connection: &Connection,
    row: &OperationalEventRow,
) -> Result<i64, String> {
    if !wants_row(&row.thread_id, &row.at) || row.event_type.trim().is_empty() {
        return Ok(0);
    }
    let (fields, clipped) = capped_payload(row.fields_json.as_deref(), MAX_EVENT_FIELDS_BYTES);
    let captured_bytes = fields
        .as_deref()
        .map(|fields| fields.len() as i64)
        .or(row.captured_bytes);
    let redaction_version = redaction_version(fields.as_deref(), None);
    // The sequence is assigned by the same statement that reads it, so two
    // writers cannot pick the same one and a retry cannot skip a number that
    // another writer took.
    connection
        .execute(
            "INSERT INTO operational_events (thread_id, turn_id, sequence, event_type, at,
                    fields_json, original_bytes, captured_bytes, payload_truncated,
                    redaction_version)
             SELECT ?1, ?2, COALESCE(MAX(sequence), 0) + 1, ?3, ?4, ?5, ?6, ?7, ?8, ?9
             FROM operational_events WHERE thread_id = ?1",
            params![
                row.thread_id,
                row.turn_id,
                row.event_type,
                at_rfc3339(&row.at),
                fields,
                row.original_bytes,
                captured_bytes,
                row.payload_truncated || clipped,
                redaction_version,
            ],
        )
        .map_err(|error| error.to_string())?;
    let id = connection.last_insert_rowid();
    connection
        .query_row(
            "SELECT sequence FROM operational_events WHERE id = ?1",
            [id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())
}

/// Records one compaction off the caller's path, logging and swallowing a
/// failure like every other telemetry write: a compaction that already
/// happened must not fail because its row could not be written.
pub fn record_compaction(row: &CompactionRow) {
    if !wants_row(&row.thread_id, &row.at) || row.trigger.trim().is_empty() {
        return;
    }
    let Some(path) = data_path() else { return };
    let row = row.clone();
    // Opening the store and inserting must not stall a turn, as with an
    // operational event (`crate::logger`).
    let write = move || {
        if let Err(error) = record_compaction_at(&path, &row) {
            crate::dbg_log!(
                "data store: compaction of {} not recorded: {error}",
                row.thread_id
            );
        }
    };
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            let _ = handle.spawn_blocking(write);
        }
        Err(_) => write(),
    }
}

/// [`record_compaction`] against one store file (tests), answering the row id.
pub fn record_compaction_at(path: &Path, row: &CompactionRow) -> Result<i64, String> {
    let connection = open(path)?;
    write_compaction(&connection, row)
}

fn write_compaction(connection: &Connection, row: &CompactionRow) -> Result<i64, String> {
    if !wants_row(&row.thread_id, &row.at) || row.trigger.trim().is_empty() {
        return Ok(0);
    }
    connection
        .execute(
            "INSERT INTO compactions (thread_id, turn_id, harness, model, at, duration_ms,
                    trigger, mode, tokens_before, tokens_after, messages_before, messages_after,
                    idle_secs_before, count, success, error_kind, external_id, source)
             VALUES (?1, ?2,
                    COALESCE(?3, (SELECT harness FROM threads WHERE id = ?1)),
                    COALESCE(?4, (SELECT model FROM threads WHERE id = ?1)),
                    ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)
             ON CONFLICT(thread_id, external_id) DO UPDATE SET
                at = excluded.at,
                harness = COALESCE(excluded.harness, compactions.harness),
                model = COALESCE(excluded.model, compactions.model),
                duration_ms = COALESCE(excluded.duration_ms, compactions.duration_ms),
                trigger = excluded.trigger,
                mode = COALESCE(excluded.mode, compactions.mode),
                tokens_before = COALESCE(excluded.tokens_before, compactions.tokens_before),
                tokens_after = COALESCE(excluded.tokens_after, compactions.tokens_after),
                messages_before = COALESCE(excluded.messages_before, compactions.messages_before),
                messages_after = COALESCE(excluded.messages_after, compactions.messages_after),
                count = COALESCE(excluded.count, compactions.count),
                success = excluded.success,
                error_kind = COALESCE(excluded.error_kind, compactions.error_kind),
                source = COALESCE(excluded.source, compactions.source)",
            params![
                row.thread_id,
                row.turn_id,
                row.harness,
                row.model,
                at_rfc3339(&row.at),
                row.duration_ms,
                row.trigger,
                row.mode,
                row.tokens_before,
                row.tokens_after,
                row.messages_before,
                row.messages_after,
                row.idle_secs_before,
                row.count,
                row.success,
                row.error_kind,
                row.external_id,
                row.source,
            ],
        )
        .map_err(|error| error.to_string())?;
    Ok(connection.last_insert_rowid())
}

/// A row is only worth storing when it names a thread and a time.
fn wants_row(thread_id: &str, at: &str) -> bool {
    !thread_id.trim().is_empty()
        && !at.trim().is_empty()
        && !rustcode_session::is_scratch_session_id(thread_id)
}

/// `at` in the one format the store writes: RFC 3339 in UTC with
/// milliseconds, so string order is time order and pruning compares strings.
fn at_rfc3339(at: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(at)
        .map(|at| {
            at.with_timezone(&chrono::Utc)
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
        })
        .unwrap_or_else(|_| at.to_owned())
}

/// The `redaction_version` a row carries: the rules used, or 0 when it holds
/// no payload at all.
fn redaction_version(payload: Option<&str>, other: Option<&str>) -> i64 {
    if payload.is_some() || other.is_some() {
        redact::REDACTION_VERSION
    } else {
        0
    }
}

/// An optional payload held to `max` bytes. A payload a caller already
/// clipped passes through; one that is still too long is clipped rather than
/// stored (or dropped), and the flag says so.
/// An optional payload held to `max` bytes and redacted on the way in.
///
/// Redaction is applied here as well as by [`redact::prepare_payload`]: it is
/// idempotent, and the second pass means a caller that forgot cannot write a
/// secret to disk. A payload that is not JSON keeps its text, redacted and cut.
/// The flag says it was clipped.
fn capped_payload(payload: Option<&str>, max: usize) -> (Option<String>, bool) {
    let Some(text) = payload.map(str::trim).filter(|text| !text.is_empty()) else {
        return (None, false);
    };
    match serde_json::from_str::<Value>(text) {
        Ok(value) => {
            let redacted = redact::redact(&value);
            let text = redacted.to_string();
            if text.len() <= max {
                (Some(text), false)
            } else {
                let (clipped, _) = redact::clip_json(&redacted, max);
                (Some(clipped), true)
            }
        }
        Err(_) => {
            let text = redact::redact_bearer(text);
            let clipped = text.len() > max;
            (Some(redact::clip_text(&text, max)), clipped)
        }
    }
}

/// What one retention pass did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct PruneReport {
    /// Payload columns set to NULL.
    pub payloads_cleared: usize,
    /// Rows deleted.
    pub rows_deleted: usize,
    /// Payload bytes before the pass.
    pub bytes_before: i64,
    /// Payload bytes after it.
    pub bytes_after: i64,
}

/// [`prune`] against one store file, answering what it did.
pub fn prune_telemetry_at(path: &Path) -> Result<PruneReport, String> {
    let connection = open(path)?;
    prune(&connection)
}

/// The retention policy: structured rows are the history and are never
/// deleted; only the bulky raw protocol payloads age out. Payloads older than
/// [`PAYLOAD_RETENTION_DAYS`] are dropped, streaming-delta and status-ping
/// payloads are dropped on every pass, and the raw tables are held under
/// [`TELEMETRY_MAX_BYTES`] by dropping payloads before rows. When
/// `telemetry_retention` in `config.toml` names a number of days, raw
/// `provider_requests` and `harness_rpc_messages` rows older than that are
/// deleted too; the default `forever` keeps their metadata. Command receipts
/// have their own, shorter window ([`COMMAND_RECEIPT_RETENTION_DAYS`],
/// [`COMMAND_RECEIPT_MAX_ROWS`]): they exist to make a retry safe for a while,
/// not to keep history. The file is then checkpointed (and vacuumed only when
/// the database already vacuums incrementally) so deleting rows actually frees
/// space.
pub fn prune(connection: &Connection) -> Result<PruneReport, String> {
    prune_with(
        connection,
        TELEMETRY_MAX_BYTES,
        configured_row_retention_days(),
    )
}

/// `telemetry_retention` from `config.toml`: `None` for `forever` (the
/// default), else the number of days a raw protocol row is kept.
pub fn configured_row_retention_days() -> Option<i64> {
    let retention = crate::config::load_config().2.telemetry_retention;
    parse_retention_days(&retention)
}

fn parse_retention_days(value: &str) -> Option<i64> {
    let value = value.trim();
    if value.is_empty() || value.eq_ignore_ascii_case("forever") {
        return None;
    }
    value.parse::<i64>().ok().filter(|days| *days > 0)
}

/// [`prune`] with an explicit payload budget and raw-row retention, so a test
/// can prove the byte cap without writing 256 MiB.
fn prune_with(
    connection: &Connection,
    max_bytes: i64,
    row_retention_days: Option<i64>,
) -> Result<PruneReport, String> {
    let now = chrono::Utc::now();
    let payload_cutoff = rfc3339_days_ago(now, PAYLOAD_RETENTION_DAYS);
    let mut report = PruneReport {
        bytes_before: telemetry_bytes(connection)?,
        ..PruneReport::default()
    };
    report.payloads_cleared += clear_noise_payloads(connection)?;
    report.payloads_cleared += clear_payloads_before(connection, &payload_cutoff)?;
    if let Some(days) = row_retention_days {
        let row_cutoff = rfc3339_days_ago(now, days);
        report.rows_deleted += delete_rows_before(connection, &row_cutoff)?;
    }
    report.rows_deleted += prune_receipts(
        connection,
        COMMAND_RECEIPT_RETENTION_DAYS,
        COMMAND_RECEIPT_MAX_ROWS,
    )?;
    trim_to_budget(connection, max_bytes, &mut report)?;
    report.bytes_after = telemetry_bytes(connection)?;
    compact(connection);
    Ok(report)
}

fn rfc3339_days_ago(now: chrono::DateTime<chrono::Utc>, days: i64) -> String {
    (now - chrono::Duration::days(days)).to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Payload bytes the raw protocol tables hold, as the store estimates them:
/// the length of every payload column. Structured rows are not counted,
/// because this budget never deletes them.
fn telemetry_bytes(connection: &Connection) -> Result<i64, String> {
    connection
        .query_row(
            "SELECT COALESCE((SELECT SUM(LENGTH(COALESCE(request_payload_json, '')) +
                                       LENGTH(COALESCE(response_summary_json, '')))
                               FROM provider_requests), 0)
                  + COALESCE((SELECT SUM(LENGTH(COALESCE(payload_json, '')))
                              FROM harness_rpc_messages), 0)",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())
}

/// Drops the raw bodies of streaming deltas and repeated status pings,
/// whatever their age. The metadata row (method, at, latency, size) stays, and
/// so does a folded chunk's small payload.
fn clear_noise_payloads(connection: &Connection) -> Result<usize, String> {
    let mut cleared = connection
        .execute(
            "UPDATE harness_rpc_messages SET payload_json = NULL
             WHERE payload_json IS NOT NULL
               AND ((method GLOB '*/outputDelta' AND LENGTH(payload_json) > ?1)
                    OR method = ?2 OR method = ?3)",
            params![
                STREAMED_PAYLOAD_BODY_BYTES,
                NOISE_METHODS[0],
                NOISE_METHODS[1]
            ],
        )
        .map_err(|error| error.to_string())?;
    // A streaming chunk's row can carry the chunk text itself: a store from
    // before chunks were folded, or a recorder that forwarded one as a frame.
    // A captured tool-call payload is not that, whatever its size, and stays.
    cleared += connection
        .execute(
            "UPDATE harness_rpc_messages SET payload_json = NULL
             WHERE payload_json IS NOT NULL AND method = 'session/update'
               AND LENGTH(payload_json) > ?1
               AND (payload_json LIKE '%\"sessionUpdate\":\"agent_message_chunk\"%'
                    OR payload_json LIKE '%\"sessionUpdate\":\"agent_thought_chunk\"%'
                    OR payload_json LIKE '%\"sessionUpdate\":\"user_message_chunk\"%')",
            [STREAMED_PAYLOAD_BODY_BYTES],
        )
        .map_err(|error| error.to_string())?;
    Ok(cleared)
}

fn clear_payloads_before(connection: &Connection, cutoff: &str) -> Result<usize, String> {
    let mut cleared = connection
        .execute(
            "UPDATE provider_requests SET request_payload_json = NULL, response_summary_json = NULL
             WHERE started_at < ?1
               AND (request_payload_json IS NOT NULL OR response_summary_json IS NOT NULL)",
            [cutoff],
        )
        .map_err(|error| error.to_string())?;
    cleared += connection
        .execute(
            "UPDATE harness_rpc_messages SET payload_json = NULL
             WHERE at < ?1 AND payload_json IS NOT NULL",
            [cutoff],
        )
        .map_err(|error| error.to_string())?;
    Ok(cleared)
}

/// Deletes only raw protocol rows. `turns`, `tool_calls`, `threads`,
/// `thread_links`, `operational_events` and `compactions` are the history and
/// are never deleted by the retention pass.
fn delete_rows_before(connection: &Connection, cutoff: &str) -> Result<usize, String> {
    let mut deleted = connection
        .execute(
            "DELETE FROM provider_requests WHERE started_at < ?1",
            [cutoff],
        )
        .map_err(|error| error.to_string())?;
    deleted += connection
        .execute("DELETE FROM harness_rpc_messages WHERE at < ?1", [cutoff])
        .map_err(|error| error.to_string())?;
    Ok(deleted)
}

/// Drops payloads first, then raw rows, until the payload bytes fit the
/// budget. Bounded: a batch per step and a finite number of steps, so a
/// pathological store cannot hold the caller for long.
fn trim_to_budget(
    connection: &Connection,
    max_bytes: i64,
    report: &mut PruneReport,
) -> Result<(), String> {
    const BATCH: i64 = 256;
    const MAX_PASSES: usize = 64;
    let mut total = telemetry_bytes(connection)?;
    for _ in 0..MAX_PASSES {
        if total <= max_bytes {
            return Ok(());
        }
        let cleared = clear_oldest_payloads(connection, BATCH)?;
        if cleared > 0 {
            report.payloads_cleared += cleared;
            total = telemetry_bytes(connection)?;
            continue;
        }
        let deleted = delete_oldest_rows(connection, BATCH)?;
        if deleted == 0 {
            return Ok(());
        }
        report.rows_deleted += deleted;
        total = telemetry_bytes(connection)?;
    }
    Ok(())
}

fn clear_oldest_payloads(connection: &Connection, batch: i64) -> Result<usize, String> {
    let mut cleared = connection
        .execute(
            "UPDATE provider_requests SET request_payload_json = NULL, response_summary_json = NULL
             WHERE id IN (SELECT id FROM provider_requests
                          WHERE request_payload_json IS NOT NULL
                             OR response_summary_json IS NOT NULL
                          ORDER BY started_at, id LIMIT ?1)",
            [batch],
        )
        .map_err(|error| error.to_string())?;
    cleared += connection
        .execute(
            "UPDATE harness_rpc_messages SET payload_json = NULL
             WHERE id IN (SELECT id FROM harness_rpc_messages
                          WHERE payload_json IS NOT NULL ORDER BY at, id LIMIT ?1)",
            [batch],
        )
        .map_err(|error| error.to_string())?;
    Ok(cleared)
}

fn delete_oldest_rows(connection: &Connection, batch: i64) -> Result<usize, String> {
    let mut deleted = connection
        .execute(
            "DELETE FROM provider_requests WHERE id IN
                (SELECT id FROM provider_requests ORDER BY started_at, id LIMIT ?1)",
            [batch],
        )
        .map_err(|error| error.to_string())?;
    deleted += connection
        .execute(
            "DELETE FROM harness_rpc_messages WHERE id IN
                (SELECT id FROM harness_rpc_messages ORDER BY at, id LIMIT ?1)",
            [batch],
        )
        .map_err(|error| error.to_string())?;
    Ok(deleted)
}

/// Writes the WAL back into the file, and reclaims pages only when the
/// database already vacuums incrementally: switching `auto_vacuum` on an
/// existing file needs a full `VACUUM`, which is not something a background
/// pass should start on a user's store.
fn compact(connection: &Connection) {
    let _ = connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
    let mode: i64 = connection
        .query_row("PRAGMA auto_vacuum", [], |row| row.get(0))
        .unwrap_or(0);
    if mode == 2 {
        let _ = connection.execute_batch("PRAGMA incremental_vacuum;");
    }
}

/// Runs [`prune`] at most [`PRUNE_INTERVAL_SECS`], stamped in `store_meta`.
/// Called from [`open`], so every writer checks retention without a timer.
fn maybe_prune(connection: &Connection) {
    let now = chrono::Utc::now().timestamp();
    let last: Option<String> = connection
        .query_row(
            "SELECT value FROM store_meta WHERE key = ?1",
            [PRUNE_META_KEY],
            |row| row.get(0),
        )
        .optional()
        .ok()
        .flatten();
    let last = last
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(0);
    if last > 0 && now.saturating_sub(last) < PRUNE_INTERVAL_SECS {
        return;
    }
    match prune(connection) {
        Ok(report) => {
            if report.payloads_cleared > 0 || report.rows_deleted > 0 {
                crate::dbg_log!(
                    "data store: telemetry pruned: {} payload(s) cleared, {} row(s) deleted, \
                     {} bytes",
                    report.payloads_cleared,
                    report.rows_deleted,
                    report.bytes_before - report.bytes_after
                );
            }
        }
        // The stamp is still written: a store that cannot be pruned must not
        // be retried on every single write.
        Err(error) => crate::dbg_log!("data store: telemetry not pruned: {error}"),
    }
    let _ = connection.execute(
        "INSERT INTO store_meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![PRUNE_META_KEY, now.to_string()],
    );
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

/// Per-table row counts and sizes, so growth itself can be tracked
/// (`rustcode data stats`). Table and view names come from `sqlite_master`;
/// a size is `dbstat`'s for the table together with its indexes (they grow
/// with it), when SQLite has that virtual table, and `NULL` otherwise, never
/// a guess.
pub fn stats() -> Result<Value, String> {
    let path = data_path().ok_or_else(|| "no config directory".to_owned())?;
    stats_at(&path)
}

/// [`stats`] against one store file, so a test can read a temporary one.
pub fn stats_at(path: &Path) -> Result<Value, String> {
    if !path.is_file() {
        return Err(format!(
            "no data store yet at {}; open the app once or run a turn so it is created",
            path.display()
        ));
    }
    let connection = open(path)?;
    let objects: Vec<(String, String)> = {
        let mut statement = connection
            .prepare(
                "SELECT type, name FROM sqlite_master
                 WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite_%'
                   AND name <> 'schema_migrations'
                 ORDER BY type, name",
            )
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(|error| error.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?
    };
    // `tbl_name` is the table an index belongs to, so summing by it counts a
    // table's own pages and its indexes: an index is most of a narrow table's
    // growth, and leaving it out would understate what the store holds.
    let sizes: std::collections::HashMap<String, i64> = connection
        .prepare(
            "SELECT master.tbl_name, SUM(dbstat.pgsize)
             FROM dbstat JOIN sqlite_master AS master ON master.name = dbstat.name
             GROUP BY master.tbl_name",
        )
        .and_then(|mut statement| {
            let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
            rows.collect::<Result<Vec<(String, i64)>, _>>()
        })
        .map(|rows| rows.into_iter().collect())
        .unwrap_or_default();
    let mut out = Vec::with_capacity(objects.len());
    for (kind, name) in objects {
        let quoted = format!("\"{}\"", name.replace('"', "\"\""));
        let rows: i64 = connection
            .query_row(&format!("SELECT COUNT(*) FROM {quoted}"), [], |row| {
                row.get(0)
            })
            .unwrap_or(-1);
        out.push(json!({
            "type": kind,
            "name": name,
            "rows": (rows >= 0).then_some(rows),
            "bytes": sizes.get(&name),
        }));
    }
    Ok(json!({
        "path": path.display().to_string(),
        "total_bytes": std::fs::metadata(path).map(|meta| meta.len()).ok(),
        "objects": out,
    }))
}

/// All-time input and output tokens the local store recorded, without adding
/// cached-token columns that may already be included in input totals.
pub(crate) fn total_reported_tokens() -> Result<Option<u64>, String> {
    let result = query(
        "SELECT CASE WHEN SUM(input_tokens) IS NULL AND SUM(output_tokens) IS NULL THEN NULL \
         ELSE COALESCE(SUM(input_tokens), 0) + COALESCE(SUM(output_tokens), 0) END \
         AS total_tokens FROM turns",
        1,
    )?;
    Ok(result
        .rows
        .first()
        .and_then(|row| row.get("total_tokens"))
        .and_then(serde_json::Value::as_u64))
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

/// How many items one `read_activity` page holds by default, and at most.
pub const DEFAULT_ACTIVITY_ITEMS: usize = 50;
pub const MAX_ACTIVITY_ITEMS: usize = 200;

/// The three sources an activity page merges, in cursor tie-break order.
const ACTIVITY_SOURCES: [&str; 3] = ["event", "provider_request", "rpc_message"];

/// One page of a thread's recorded activity: its allowlisted operational
/// events, built-in provider attempts and harness protocol messages, merged
/// in time order. Payload columns are never selected: these are summaries.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct ActivityPage {
    pub items: Vec<Value>,
    /// The opaque cursor token of the last item; pass it back as `after`. At
    /// most one `|`-free `at`, a source name and a row id, so it stays short.
    pub next: Option<String>,
    pub has_more: bool,
}

/// One thread's activity, oldest first, from the store the app reads.
pub fn activity(
    thread_id: &str,
    after: Option<&str>,
    limit: usize,
) -> Result<ActivityPage, String> {
    activity_ordered(thread_id, after, limit, false)
}

pub fn activity_ordered(
    thread_id: &str,
    after: Option<&str>,
    limit: usize,
    newest: bool,
) -> Result<ActivityPage, String> {
    let path = data_path().ok_or_else(|| "no config directory".to_owned())?;
    activity_at_ordered(&path, thread_id, after, limit, newest)
}

/// [`activity`] against one store file (tests). A store that does not exist
/// yet is an empty page, not an error: a new thread has done nothing.
pub fn activity_at(
    path: &Path,
    thread_id: &str,
    after: Option<&str>,
    limit: usize,
) -> Result<ActivityPage, String> {
    activity_at_ordered(path, thread_id, after, limit, false)
}

pub fn activity_at_ordered(
    path: &Path,
    thread_id: &str,
    after: Option<&str>,
    limit: usize,
    newest: bool,
) -> Result<ActivityPage, String> {
    let thread_id = thread_id.trim();
    if thread_id.is_empty() {
        return Err("session_id is required".to_owned());
    }
    let (after_at, after_source, after_row) = activity_cursor(after)?;
    let limit = limit.clamp(1, MAX_ACTIVITY_ITEMS);
    if !path.is_file() {
        return Ok(ActivityPage::default());
    }
    let connection = open_readonly(path)?;
    let newest_sql = newest.then(|| {
        ACTIVITY_SQL
            .replace(
                "(at, source, row_id) > (?2, ?3, ?4)",
                "(?2 = '' OR (at, source, row_id) < (?2, ?3, ?4))",
            )
            .replace(
                "ORDER BY at, source, row_id",
                "ORDER BY at DESC, source DESC, row_id DESC",
            )
    });
    let mut statement = connection
        .prepare(newest_sql.as_deref().unwrap_or(ACTIVITY_SQL))
        .map_err(|error| error.to_string())?;
    // One more than the page, so `has_more` needs no second count.
    let mut rows = statement
        .query(params![
            thread_id,
            after_at,
            after_source,
            after_row,
            (limit + 1) as i64
        ])
        .map_err(query_error)?;
    let mut page: Vec<(Value, String)> = Vec::with_capacity(limit + 1);
    while let Some(row) = rows.next().map_err(query_error)? {
        let at: String = row.get("at").map_err(query_error)?;
        let source: String = row.get("source").map_err(query_error)?;
        let row_id: i64 = row.get("row_id").map_err(query_error)?;
        let item = activity_item(row, &source, &at).map_err(query_error)?;
        page.push((item, format!("{at}|{source}|{row_id}")));
    }
    let has_more = page.len() > limit;
    page.truncate(limit);
    let next = page.last().map(|(_, cursor)| cursor.clone());
    Ok(ActivityPage {
        items: page.into_iter().map(|(item, _)| item).collect(),
        next,
        has_more,
    })
}

pub fn last_provider_error(thread_id: &str) -> Result<Option<Value>, String> {
    let path = data_path().ok_or_else(|| "no config directory".to_owned())?;
    last_provider_error_at(&path, thread_id)
}

fn last_provider_error_at(path: &Path, thread_id: &str) -> Result<Option<Value>, String> {
    if !path.is_file() {
        return Ok(None);
    }
    let connection = open_readonly(path)?;
    use rusqlite::OptionalExtension;
    connection.query_row(
        "SELECT started_at, provider, model, status_code, error_kind, attempt FROM provider_requests WHERE thread_id = ?1 AND (error_kind IS NOT NULL OR status_code >= 400) ORDER BY started_at DESC, id DESC LIMIT 1",
        [thread_id],
        |row| Ok(json!({"at": row.get::<_, String>(0)?, "provider": row.get::<_, Option<String>>(1)?, "model": row.get::<_, Option<String>>(2)?, "status_code": row.get::<_, Option<i64>>(3)?, "error_kind": row.get::<_, Option<String>>(4)?, "attempt": row.get::<_, Option<i64>>(5)?})),
    ).optional().map_err(query_error)
}

/// The cursor a page starts after: the `(at, source, row id)` of its last item,
/// or the beginning of the thread when `after` is absent.
fn activity_cursor(after: Option<&str>) -> Result<(String, String, i64), String> {
    let Some(after) = after.map(str::trim).filter(|after| !after.is_empty()) else {
        return Ok((String::new(), String::new(), 0));
    };
    let parts: Vec<&str> = after.split('|').collect();
    let invalid = || "after is not the next token an earlier read_activity returned".to_owned();
    let [at, source, row] = parts.as_slice() else {
        return Err(invalid());
    };
    if !ACTIVITY_SOURCES.contains(source) {
        return Err(invalid());
    }
    let row = row.parse::<i64>().map_err(|_| invalid())?;
    Ok((at.to_string(), source.to_string(), row))
}

/// The one statement behind [`activity`]: the three tables with the columns
/// each source needs, and a row-value comparison for the cursor, so a page can
/// neither repeat nor skip an item when two rows share a timestamp.
const ACTIVITY_SQL: &str = "
WITH merged AS (
    SELECT 'event' AS source, id AS row_id, at AS at, event_type, fields_json,
           NULL AS provider, NULL AS model, NULL AS status_code, NULL AS latency_ms,
           NULL AS error_kind, NULL AS input_tokens, NULL AS output_tokens,
           NULL AS cached_tokens, NULL AS direction, NULL AS message_kind,
           NULL AS method, NULL AS byte_size, NULL AS attempt
      FROM operational_events WHERE thread_id = ?1
    UNION ALL
    SELECT 'provider_request', id, started_at, NULL, NULL, provider, model, status_code,
           latency_ms, error_kind, input_tokens, output_tokens, cached_tokens,
           NULL, NULL, NULL, NULL, attempt
      FROM provider_requests WHERE thread_id = ?1
    UNION ALL
    SELECT 'rpc_message', id, at, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL,
           NULL, direction, message_kind, method, byte_size, NULL
      FROM harness_rpc_messages WHERE thread_id = ?1
)
SELECT source, row_id, at, event_type, fields_json, provider, model, status_code,
       latency_ms, error_kind, input_tokens, output_tokens, cached_tokens, direction,
       message_kind, method, byte_size, attempt
  FROM merged
 WHERE (at, source, row_id) > (?2, ?3, ?4)
 ORDER BY at, source, row_id
 LIMIT ?5";

/// One merged row as the item `read_activity` returns. `fields_json` was already
/// redacted and bounded when it was written; it is re-parsed so the answer is
/// JSON, never a string a reader has to decode.
fn activity_item(row: &rusqlite::Row<'_>, source: &str, at: &str) -> rusqlite::Result<Value> {
    let mut item = serde_json::Map::new();
    item.insert("source".to_owned(), json!(source));
    item.insert("at".to_owned(), json!(at));
    let text = |name: &str| row.get::<_, Option<String>>(name);
    let number = |name: &str| row.get::<_, Option<i64>>(name);
    match source {
        "event" => {
            item.insert(
                "type".to_owned(),
                json!(text("event_type")?.unwrap_or_default()),
            );
            if let Some(fields) = text("fields_json")? {
                let fields = serde_json::from_str::<Value>(&fields).unwrap_or(json!(fields));
                item.insert("fields".to_owned(), fields);
            }
        }
        "provider_request" => {
            insert_text(&mut item, "provider", text("provider")?);
            insert_text(&mut item, "model", text("model")?);
            insert_text(&mut item, "error_kind", text("error_kind")?);
            insert_number(&mut item, "status_code", number("status_code")?);
            insert_number(&mut item, "latency_ms", number("latency_ms")?);
            insert_number(&mut item, "input_tokens", number("input_tokens")?);
            insert_number(&mut item, "output_tokens", number("output_tokens")?);
            insert_number(&mut item, "cached_tokens", number("cached_tokens")?);
            insert_number(&mut item, "attempt", number("attempt")?);
        }
        "rpc_message" => {
            insert_text(&mut item, "direction", text("direction")?);
            insert_text(&mut item, "kind", text("message_kind")?);
            insert_text(&mut item, "method", text("method")?);
            insert_number(&mut item, "latency_ms", number("latency_ms")?);
            insert_number(&mut item, "byte_size", number("byte_size")?);
        }
        _ => {}
    }
    Ok(Value::Object(item))
}

fn insert_text(item: &mut serde_json::Map<String, Value>, key: &str, value: Option<String>) {
    if let Some(value) = value {
        item.insert(key.to_owned(), json!(value));
    }
}

fn insert_number(item: &mut serde_json::Map<String, Value>, key: &str, value: Option<i64>) {
    if let Some(value) = value {
        item.insert(key.to_owned(), json!(value));
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

    #[test]
    fn a_one_time_pass_remembers_that_it_ran() {
        assert_eq!(meta("test_one_time_pass"), None);
        set_meta("test_one_time_pass", "1").unwrap();
        assert_eq!(meta("test_one_time_pass").as_deref(), Some("1"));
        set_meta("test_one_time_pass", "2").unwrap();
        assert_eq!(meta("test_one_time_pass").as_deref(), Some("2"));
    }

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
            duration_ms: None,
            time_to_first_token_ms: None,
            model_time_ms: None,
            tool_time_ms: None,
            context_tokens: None,
            context_window: None,
            stop_reason: None,
            tool_calls: vec![ToolCallRow {
                tool_name: "execute".to_owned(),
                success: Some(false),
                exit_code: Some(1),
                at: Some(started.to_owned()),
                ..Default::default()
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
                    parent_id: (id == "b").then(|| "a".to_owned()),
                    parent_title: (id == "b").then(|| "Thread a".to_owned()),
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
                duration_ms: None,
                time_to_first_token_ms: None,
                model_time_ms: None,
                tool_time_ms: None,
                context_tokens: None,
                context_window: None,
                stop_reason: None,
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
        assert_eq!(codex["turns_without_input_tokens"], 0);
        assert_eq!(codex["turns_without_output_tokens"], 0);
        // Cache columns are NULL on both codex turns; no count is invented.
        assert_eq!(codex["turns_without_cached_tokens"], 2);
        assert_eq!(codex["turns_without_cache_write_tokens"], 2);
        assert_eq!(codex["turns_without_any_tokens"], 2);
        // No turn in this store reported a cost, so both of codex's are missing one.
        assert_eq!(codex["turns_without_cost"], 2);
        // A harness that reported nothing is NULL, never a made-up zero.
        let claude = &rows(&by_harness)[1];
        assert!(claude["input_tokens"].is_null(), "{claude}");
        assert_eq!(claude["turns_without_tokens"], 1);
        assert_eq!(claude["turns_without_input_tokens"], 1);
        assert_eq!(claude["turns_without_output_tokens"], 1);
        assert_eq!(claude["turns_without_cached_tokens"], 1);
        assert_eq!(claude["turns_without_cache_write_tokens"], 1);
        assert_eq!(claude["turns_without_any_tokens"], 1);
        assert_eq!(claude["turns_without_cost"], 1);
        assert!(
            by_harness["totals"]["cost_usd"].is_null(),
            "nothing was priced"
        );
        // Every cache count in the store is NULL: the total stays absent, not 0.
        assert!(
            by_harness["totals"]["cached_tokens"].is_null(),
            "{by_harness}"
        );
        assert!(
            by_harness["totals"]["cache_write_tokens"].is_null(),
            "{by_harness}"
        );
        assert_eq!(by_harness["totals"]["input_tokens"], 300.0);
        assert_eq!(by_harness["totals"]["turns"], 3.0);
        assert_eq!(by_harness["totals"]["turns_without_tokens"], 1.0);
        assert_eq!(by_harness["totals"]["turns_without_any_tokens"], 3.0);
        assert_eq!(by_harness["totals"]["turns_without_input_tokens"], 1.0);
        assert_eq!(by_harness["totals"]["turns_without_output_tokens"], 1.0);
        assert_eq!(by_harness["totals"]["turns_without_cached_tokens"], 3.0);
        assert_eq!(
            by_harness["totals"]["turns_without_cache_write_tokens"],
            3.0
        );
        assert_eq!(by_harness["totals"]["turns_without_cost"], 3.0);
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
        // The thread ran two harness/model pairs (codex/gpt-6-luna and
        // claude-code/sonnet), so it has no single pair to price and the key
        // stays NULL rather than naming the latest one.
        assert!(first["model"].is_null(), "{first}");
        assert!(first["model_harness"].is_null(), "{first}");
        assert_eq!(first["parent_id"], "a");
        assert_eq!(first["parent_title"], "Thread a");
    }

    #[test]
    fn a_thread_is_priced_only_when_its_turns_share_one_harness_and_model() {
        let dir = tempfile::tempdir().unwrap();
        let path = usage_store(dir.path());
        let answer = usage_at(&path, None, GroupBy::Thread, None).unwrap();
        let listed = rows(&answer);
        let row = |id: &str| {
            listed
                .iter()
                .find(|row| row["thread"] == id)
                .unwrap_or_else(|| panic!("no row for {id} in {answer}"))
        };

        // "a" ran one turn, so its harness and model are the pair to price.
        let single = row("a");
        assert_eq!(single["model"], "gpt-6-luna", "{single}");
        assert_eq!(single["model_harness"], "codex", "{single}");
        // Its display harness still comes from `threads`/the latest turn.
        assert_eq!(single["harness"], "codex", "{single}");

        // "b" mixes codex/gpt-6-luna with claude-code/sonnet, so neither key
        // is named: one rate would be wrong for the other harness's turns.
        let mixed = row("b");
        assert!(mixed["model"].is_null(), "{mixed}");
        assert!(mixed["model_harness"].is_null(), "{mixed}");
        // The display harness is unchanged (the store's latest one).
        assert_eq!(mixed["harness"], "claude-code", "{mixed}");
    }

    #[test]
    fn usage_counts_each_missing_token_column() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DATA_FILE);
        let connection = open(&path).unwrap();
        upsert_thread(
            &connection,
            &ThreadRow {
                id: "t".to_owned(),
                title: Some("Thread t".to_owned()),
                project: Some("/repo/t".to_owned()),
                cwd: Some("/repo/t".to_owned()),
                harness: Some("codex".to_owned()),
                model: Some("gpt-6-luna".to_owned()),
                effort: None,
                parent_id: None,
                parent_title: None,
                created_at: None,
                updated_at: None,
            },
        )
        .unwrap();
        let spent = |started: &str,
                     input: Option<i64>,
                     output: Option<i64>,
                     cached: Option<i64>,
                     cache_write: Option<i64>| TurnRow {
            thread_id: "t".to_owned(),
            started_at: started.to_owned(),
            finished_at: Some(started.to_owned()),
            harness: Some("codex".to_owned()),
            model: Some("gpt-6-luna".to_owned()),
            input_tokens: input,
            output_tokens: output,
            cached_tokens: cached,
            cache_write_tokens: cache_write,
            cost_usd: None,
            status: "completed".to_owned(),
            duration_ms: None,
            time_to_first_token_ms: None,
            model_time_ms: None,
            tool_time_ms: None,
            context_tokens: None,
            context_window: None,
            stop_reason: None,
            tool_calls: Vec::new(),
        };
        for turn in [
            spent("2026-11-01T10:00:00Z", Some(10), Some(2), Some(3), Some(1)),
            spent("2026-11-02T10:00:00Z", Some(5), None, None, None),
            spent("2026-11-03T10:00:00Z", None, Some(4), None, Some(2)),
            spent("2026-11-04T10:00:00Z", None, None, None, None),
        ] {
            write_turn(&connection, &turn).unwrap();
        }

        let answer = usage_at(&path, None, GroupBy::Harness, None).unwrap();
        let row = &rows(&answer)[0];
        assert_eq!(row["turns"], 4);
        assert_eq!(row["turns_without_tokens"], 1, "only the all-NULL turn");
        assert_eq!(row["turns_without_any_tokens"], 3);
        assert_eq!(row["turns_without_input_tokens"], 2);
        assert_eq!(row["turns_without_output_tokens"], 2);
        assert_eq!(row["turns_without_cached_tokens"], 3);
        assert_eq!(row["turns_without_cache_write_tokens"], 2);
        assert_eq!(answer["totals"]["input_tokens"], 15.0);
        assert_eq!(answer["totals"]["output_tokens"], 6.0);
        assert_eq!(answer["totals"]["cached_tokens"], 3.0);
        assert_eq!(answer["totals"]["cache_write_tokens"], 3.0);
        assert_eq!(answer["totals"]["turns_without_any_tokens"], 3.0);
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
                parent_id: None,
                parent_title: None,
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
                ..Default::default()
            },
            ToolCallRow {
                tool_name: "run_command".to_owned(),
                success: Some(true),
                exit_code: Some(0),
                at: None,
                ..Default::default()
            },
            ToolCallRow {
                tool_name: "replace_file_content".to_owned(),
                success: Some(false),
                exit_code: None,
                at: None,
                ..Default::default()
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

    fn engine_link(id: &str, orchestrator: &str, state: &str) -> Value {
        json!({
            "id": id,
            "orchestrator": orchestrator,
            "target": "delegate",
            "state": state,
            "engine": true,
            "createdAt": 1_762_000_000_000i64,
        })
    }

    #[test]
    fn the_apps_save_cannot_undo_a_link_the_engine_runs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DATA_FILE);
        let caller = "caller-live-link";
        let opened = open_link_at(&path, caller, |n| {
            engine_link(&format!("{caller}/{n}"), caller, "working")
        })
        .unwrap();
        let id = opened["id"].as_str().unwrap().to_owned();
        assert_eq!(id, format!("{caller}/1"));
        engine_link_started(&id);

        // An app that attached mid-turn interrupts the link in its copy and
        // saves a book that never saw the counter move.
        let mut mine = engine_link(&id, caller, "interrupted");
        mine["answer"] = json!("stale");
        save_link_book_at(
            &path,
            &json!({ "links": [mine], "grants": {}, "counters": {}, "fanOuts": [] }),
        )
        .unwrap();
        let loaded = load_link_book_at(&path).unwrap();
        assert_eq!(loaded["book"]["links"][0]["state"], json!("working"));
        assert!(loaded["book"]["links"][0].get("answer").is_none());
        assert_eq!(loaded["book"]["counters"][caller], json!(1));

        // A book saved before the engine opened the link keeps it too.
        save_link_book_at(&path, &book_json()).unwrap();
        let loaded = load_link_book_at(&path).unwrap();
        let ids: Vec<&str> = loaded["book"]["links"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|link| link["id"].as_str())
            .collect();
        assert!(ids.contains(&id.as_str()), "{ids:?}");

        // The turn ends: the engine settles the link, and the app's older
        // copy no longer takes it back.
        assert!(
            change_link_at(&path, &id, |mut link| {
                link["state"] = json!("done");
                link["answer"] = json!("finished");
                link
            })
            .unwrap()
        );
        engine_link_ended(&id);
        save_link_book_at(
            &path,
            &json!({
                "links": [engine_link(&id, caller, "interrupted")],
                "grants": {}, "counters": {}, "fanOuts": [],
            }),
        )
        .unwrap();
        let loaded = load_link_book_at(&path).unwrap();
        assert_eq!(loaded["book"]["links"][0]["state"], json!("done"));
        assert_eq!(loaded["book"]["links"][0]["answer"], json!("finished"));
    }

    #[test]
    fn a_link_the_engine_lost_to_a_restart_is_the_apps_to_interrupt() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DATA_FILE);
        let caller = "caller-dead-engine";
        open_link_at(&path, caller, |n| {
            engine_link(&format!("{caller}/{n}"), caller, "working")
        })
        .unwrap();
        // Never marked live in this process: its engine is gone.
        save_link_book_at(
            &path,
            &json!({
                "links": [engine_link(&format!("{caller}/1"), caller, "interrupted")],
                "grants": {}, "counters": {}, "fanOuts": [],
            }),
        )
        .unwrap();
        let loaded = load_link_book_at(&path).unwrap();
        assert_eq!(loaded["book"]["links"][0]["state"], json!("interrupted"));
    }

    #[test]
    fn a_link_whose_engine_died_is_interrupted_on_the_next_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DATA_FILE);
        let dead = "caller-dead-read";
        // Stamped long ago and never live in this process: its engine is gone.
        open_link_at(&path, dead, |n| {
            engine_link(&format!("{dead}/{n}"), dead, "working")
        })
        .unwrap();
        let live_caller = "caller-live-read";
        let live = open_link_at(&path, live_caller, |n| {
            engine_link(&format!("{live_caller}/{n}"), live_caller, "working")
        })
        .unwrap();
        engine_link_started(live["id"].as_str().unwrap());
        // One a running engine stamped a moment ago is not orphaned either.
        let fresh = open_link_at(&path, "caller-fresh-read", |n| {
            let mut link = engine_link(
                &format!("caller-fresh-read/{n}"),
                "caller-fresh-read",
                "working",
            );
            link["engineBeat"] = json!(now_ms());
            link
        })
        .unwrap();

        let loaded = load_link_book_at(&path).unwrap();
        let state = |id: &str| {
            loaded["book"]["links"]
                .as_array()
                .unwrap()
                .iter()
                .find(|link| link["id"] == json!(id))
                .map(|link| link["state"].clone())
        };
        assert_eq!(state(&format!("{dead}/1")), Some(json!("interrupted")));
        assert_eq!(state(live["id"].as_str().unwrap()), Some(json!("working")));
        assert_eq!(state(fresh["id"].as_str().unwrap()), Some(json!("working")));
        engine_link_ended(live["id"].as_str().unwrap());
    }

    #[test]
    fn a_settled_engine_link_survives_an_app_copy_that_never_saw_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DATA_FILE);
        let caller = "caller-settled-kept";
        let opened = open_link_at(&path, caller, |n| {
            engine_link(&format!("{caller}/{n}"), caller, "done")
        })
        .unwrap();
        // A second window's book from before the link existed.
        save_link_book_at(&path, &book_json()).unwrap();
        let loaded = load_link_book_at(&path).unwrap();
        let kept = loaded["book"]["links"]
            .as_array()
            .unwrap()
            .iter()
            .any(|link| link["id"] == opened["id"]);
        assert!(kept, "{loaded}");
    }

    #[test]
    fn two_openers_never_share_a_link_id() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DATA_FILE);
        let first = open_link_at(&path, "same", |n| {
            engine_link(&format!("same/{n}"), "same", "done")
        })
        .unwrap();
        // The app saves a counter that is behind; the next id still moves on.
        save_link_book_at(
            &path,
            &json!({ "links": [], "grants": {}, "counters": { "same": 0 }, "fanOuts": [] }),
        )
        .unwrap();
        let second = open_link_at(&path, "same", |n| {
            engine_link(&format!("same/{n}"), "same", "done")
        })
        .unwrap();
        assert_ne!(first["id"], second["id"]);
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
                "at",
                "started_at",
                "duration_ms",
                "output_bytes",
                "truncated",
                "mcp_server",
                "command_name",
                "error_kind"
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
                    r#"{"schema_version":1,"id":"x","created_at_ms":1793784000000,
                        "parent":{"id":"01a10000-0000-0000-0000-000000000001","title":"Parent thread"}}"#,
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

        let native: (
            String,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
        ) = connection
            .query_row(
                "SELECT harness, title, created_at, parent_id, parent_title FROM threads
                 WHERE id = '01a10000-0000-0000-0000-000000000002'",
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
        assert_eq!(native.0, "rustcode");
        assert!(native.1.ends_with("02"));
        assert!(native.2.is_some(), "metadata.json supplies created_at");
        assert_eq!(
            native.3.as_deref(),
            Some("01a10000-0000-0000-0000-000000000001")
        );
        assert_eq!(native.4.as_deref(), Some("Parent thread"));
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
        assert_eq!(
            advance(&usage(0.75), Some(&usage(0.5))).0.cost_usd,
            Some(0.25)
        );
        assert_eq!(
            advance(&usage(0.2), Some(&usage(0.5))).0.cost_usd,
            Some(0.2),
            "a restarted session reports its whole cost again"
        );
        assert_eq!(advance(&usage(0.2), None).0.cost_usd, Some(0.2));
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
            Some("session-a"),
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
    fn a_restarted_harness_counts_each_turn_once() {
        let config = tempfile::tempdir().unwrap();
        let connection = open(&config.path().join(DATA_FILE)).unwrap();
        let thread = "01a10000-0000-0000-0000-000000000010";
        // Claude Code reports session totals: cost, and one token counter per
        // kind, all of which restart together when its process does.
        let cumulative = |cost: f64, input: i64| SessionUsage {
            tokens: TokenTotals {
                input: Some(input),
                output: Some(input / 10),
                cached: Some(input * 5),
                cache_write: Some(input / 2),
            },
            cost_usd: Some(cost),
            at: None,
        };
        let turn = |started: &str, status: &str| TurnRow {
            thread_id: thread.to_owned(),
            started_at: started.to_owned(),
            finished_at: Some(started.to_owned()),
            harness: Some("claude-code".to_owned()),
            status: status.to_owned(),
            ..TurnRow::default()
        };
        let record = |started: &str, status: &str, cost: f64, input: i64| {
            write_cumulative_turn(
                &connection,
                &turn(started, status),
                cumulative(cost, input),
                Some("session-a"),
            )
            .unwrap();
        };
        record("2026-11-04T11:00:00.000Z", "completed", 10.0, 1000);
        record("2026-11-04T11:05:00.000Z", "completed", 15.0, 1500);
        // The harness restarted: its session counter starts again at 2.
        record("2026-11-05T09:00:00.000Z", "completed", 2.0, 200);
        record("2026-11-05T09:05:00.000Z", "completed", 4.0, 400);
        // A failed turn reports the previous total again: no change, no row value.
        record("2026-11-05T09:06:00.000Z", "failed", 4.0, 400);

        let rows: Vec<(
            String,
            Option<f64>,
            Option<i64>,
            Option<i64>,
            Option<i64>,
            Option<i64>,
        )> = {
            let mut statement = connection
                .prepare(
                    "SELECT status, cost_usd, input_tokens, output_tokens,
                                cached_tokens, cache_write_tokens
                         FROM turns WHERE thread_id = ?1 ORDER BY started_at",
                )
                .unwrap();
            statement
                .query_map([thread], |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                })
                .unwrap()
                .filter_map(Result::ok)
                .collect()
        };
        let spent: Vec<(String, Option<f64>, Option<i64>)> = rows
            .iter()
            .map(|(status, cost, input, _, _, _)| (status.clone(), *cost, *input))
            .collect();
        assert_eq!(
            spent,
            [
                ("completed".to_owned(), Some(10.0), Some(1000)),
                ("completed".to_owned(), Some(5.0), Some(500)),
                ("completed".to_owned(), Some(2.0), Some(200)),
                ("completed".to_owned(), Some(2.0), Some(200)),
                ("failed".to_owned(), Some(0.0), Some(0)),
            ]
        );
        // Each token counter grows the same way, and the summed rows are what
        // the harness actually spent: 19 dollars, 1900 input tokens.
        let (output, cached, cache_write): (i64, i64, i64) = connection
            .query_row(
                "SELECT SUM(output_tokens), SUM(cached_tokens), SUM(cache_write_tokens)
                 FROM turns WHERE thread_id = ?1",
                [thread],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!((output, cached, cache_write), (190, 9500, 950));
        let total: f64 = connection
            .query_row(
                "SELECT SUM(cost_usd) FROM turns WHERE thread_id = ?1",
                [thread],
                |row| row.get(0),
            )
            .unwrap();
        assert!((total - 19.0).abs() < 1e-9, "{total} is 10+5+2+2+0");
        // The cursor holds the last total, not the sum of the rows.
        let cursor: (f64, i64) = connection
            .query_row(
                "SELECT cost_usd, input_tokens FROM usage_cursors
                 WHERE thread_id = ?1 AND harness = 'claude-code' AND series = 'session-a'",
                [thread],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!((cursor.0 - 4.0).abs() < 1e-9);
        assert_eq!(cursor.1, 400);

        // A thread that switches harness starts its new harness at zero.
        write_cumulative_turn(
            &connection,
            &TurnRow {
                harness: Some("codex".to_owned()),
                ..turn("2026-11-05T10:00:00.000Z", "completed")
            },
            cumulative(3.0, 30),
            Some("session-a"),
        )
        .unwrap();
        let switched: (Option<f64>, Option<i64>) = connection
            .query_row(
                "SELECT cost_usd, input_tokens FROM turns
                 WHERE thread_id = ?1 AND harness = 'codex'",
                [thread],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(switched, (Some(3.0), Some(30)));
    }

    #[test]
    fn returning_to_an_earlier_session_counts_only_its_own_growth() {
        let config = tempfile::tempdir().unwrap();
        let connection = open(&config.path().join(DATA_FILE)).unwrap();
        let thread = "01a10000-0000-0000-0000-000000000012";
        let cumulative = |cost: f64| SessionUsage {
            tokens: TokenTotals {
                input: Some(cost as i64 * 100),
                ..TokenTotals::default()
            },
            cost_usd: Some(cost),
            at: None,
        };
        let record = |started: &str, cost: f64, series: &str| {
            write_cumulative_turn(
                &connection,
                &TurnRow {
                    thread_id: thread.to_owned(),
                    started_at: started.to_owned(),
                    finished_at: Some(started.to_owned()),
                    harness: Some("claude-code".to_owned()),
                    status: "completed".to_owned(),
                    ..TurnRow::default()
                },
                cumulative(cost),
                Some(series),
            )
            .unwrap();
        };
        record("2026-11-04T11:00:00.000Z", 10.0, "session-a");
        record("2026-11-04T11:05:00.000Z", 15.0, "session-a");
        // Resume in another session: its own total, not 2 minus 15.
        record("2026-11-04T12:00:00.000Z", 2.0, "session-b");
        // Back in the first session: the difference against *its* last total,
        // never against the other session's 16.
        record("2026-11-04T13:00:00.000Z", 16.0, "session-a");

        let rows: Vec<Option<f64>> = {
            let mut statement = connection
                .prepare("SELECT cost_usd FROM turns WHERE thread_id = ?1 ORDER BY started_at")
                .unwrap();
            statement
                .query_map([thread], |row| row.get(0))
                .unwrap()
                .filter_map(Result::ok)
                .collect()
        };
        assert_eq!(
            rows,
            [Some(10.0), Some(5.0), Some(2.0), Some(1.0)],
            "each series is measured against its own last total"
        );
        let cursors: Vec<(String, f64)> = {
            let mut statement = connection
                .prepare(
                    "SELECT series, cost_usd FROM usage_cursors
                     WHERE thread_id = ?1 ORDER BY series",
                )
                .unwrap();
            statement
                .query_map([thread], |row| Ok((row.get(0)?, row.get(1)?)))
                .unwrap()
                .filter_map(Result::ok)
                .collect()
        };
        assert_eq!(
            cursors,
            [
                ("session-a".to_owned(), 16.0),
                ("session-b".to_owned(), 2.0)
            ]
        );
    }

    #[test]
    fn a_counter_that_dips_alone_records_no_growth() {
        let config = tempfile::tempdir().unwrap();
        let connection = open(&config.path().join(DATA_FILE)).unwrap();
        let thread = "01a10000-0000-0000-0000-000000000013";
        let record = |started: &str, cost: f64, input: i64, output: i64, cached: i64| {
            write_cumulative_turn(
                &connection,
                &TurnRow {
                    thread_id: thread.to_owned(),
                    started_at: started.to_owned(),
                    finished_at: Some(started.to_owned()),
                    harness: Some("claude-code".to_owned()),
                    status: "completed".to_owned(),
                    ..TurnRow::default()
                },
                SessionUsage {
                    tokens: TokenTotals {
                        input: Some(input),
                        output: Some(output),
                        cached: Some(cached),
                        cache_write: Some(input / 2),
                    },
                    cost_usd: Some(cost),
                    at: None,
                },
                Some("session-a"),
            )
            .unwrap();
        };
        record("2026-11-04T11:00:00.000Z", 10.0, 1000, 100, 500);
        // A rotated or truncated log, or an account directory that went
        // missing: one counter goes back while the others keep rising. That is
        // not a restart, so the dipped counter records nothing instead of a
        // whole session, and its cursor keeps the higher total.
        record("2026-11-04T11:05:00.000Z", 11.0, 400, 130, 600);
        // Once the log is whole again, growth resumes from the kept total.
        record("2026-11-04T11:10:00.000Z", 11.5, 1100, 150, 700);

        let rows: Vec<(Option<f64>, Option<i64>, Option<i64>, Option<i64>)> = {
            let mut statement = connection
                .prepare(
                    "SELECT cost_usd, input_tokens, output_tokens, cached_tokens
                     FROM turns WHERE thread_id = ?1 ORDER BY started_at",
                )
                .unwrap();
            statement
                .query_map([thread], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                })
                .unwrap()
                .filter_map(Result::ok)
                .collect()
        };
        assert_eq!(
            rows,
            [
                (Some(10.0), Some(1000), Some(100), Some(500)),
                (Some(1.0), Some(0), Some(30), Some(100)),
                (Some(0.5), Some(100), Some(20), Some(100)),
            ]
        );
        let cursor: (Option<i64>, Option<i64>) = connection
            .query_row(
                "SELECT input_tokens, cached_tokens FROM usage_cursors
                 WHERE thread_id = ?1 AND series = 'session-a'",
                [thread],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(cursor, (Some(1100), Some(700)));
    }

    #[test]
    fn a_cumulative_turn_survives_a_commit_from_another_writer() {
        let config = tempfile::tempdir().unwrap();
        let path = config.path().join(DATA_FILE);
        let connection = open(&path).unwrap();
        // Another writer holds the write lock and commits while this turn is
        // between its reads and its write. A DEFERRED transaction would answer
        // SQLITE_BUSY_SNAPSHOT there and lose the turn; IMMEDIATE takes the
        // lock first and waits for its turn.
        let other_path = path.clone();
        let (locked, ready) = std::sync::mpsc::channel();
        let other = std::thread::spawn(move || {
            let other = open(&other_path).unwrap();
            other.execute_batch("BEGIN IMMEDIATE").unwrap();
            other
                .execute("INSERT INTO threads (id) VALUES ('another-writer')", [])
                .unwrap();
            locked.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(300));
            other.execute_batch("COMMIT").unwrap();
        });
        ready.recv().unwrap();
        let thread = "01a10000-0000-0000-0000-000000000014";
        write_cumulative_turn(
            &connection,
            &TurnRow {
                thread_id: thread.to_owned(),
                started_at: "2026-11-04T11:00:00.000Z".to_owned(),
                finished_at: Some("2026-11-04T11:00:01.000Z".to_owned()),
                harness: Some("claude-code".to_owned()),
                status: "completed".to_owned(),
                ..TurnRow::default()
            },
            SessionUsage {
                tokens: TokenTotals {
                    input: Some(50),
                    ..TokenTotals::default()
                },
                ..SessionUsage::default()
            },
            Some("session-a"),
        )
        .expect("the turn waits for the other writer instead of failing");
        other.join().unwrap();
        let (turns, input): (i64, i64) = connection
            .query_row(
                "SELECT COUNT(*), SUM(input_tokens) FROM turns WHERE thread_id = ?1",
                [thread],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!((turns, input), (1, 50));
    }

    #[test]
    fn a_thread_recorded_before_cursors_does_not_add_an_inflated_total() {
        let config = tempfile::tempdir().unwrap();
        let connection = open(&config.path().join(DATA_FILE)).unwrap();
        let thread = "01a10000-0000-0000-0000-000000000011";
        // An older release recorded this thread's rows, and the sum it was
        // measured against is inflated (830: 15 turns of a runaway total).
        write_turn(
            &connection,
            &TurnRow {
                thread_id: thread.to_owned(),
                started_at: "2026-11-04T11:00:00.000Z".to_owned(),
                harness: Some("claude-code".to_owned()),
                input_tokens: Some(830),
                cost_usd: Some(830.0),
                status: "completed".to_owned(),
                ..TurnRow::default()
            },
        )
        .unwrap();
        let cumulative = |cost: f64| SessionUsage {
            tokens: TokenTotals {
                input: Some(cost as i64),
                ..TokenTotals::default()
            },
            cost_usd: Some(cost),
            at: None,
        };
        let live = |started: &str| TurnRow {
            thread_id: thread.to_owned(),
            started_at: started.to_owned(),
            finished_at: Some(started.to_owned()),
            harness: Some("claude-code".to_owned()),
            status: "completed".to_owned(),
            ..TurnRow::default()
        };
        // The next total is below the sum, so the sum is the inflated side:
        // the lower of the two is the baseline. This turn counts nothing — the
        // rows already hold at least this total — and the cursor starts at the
        // reading, so the turns after it are exact.
        write_cumulative_turn(
            &connection,
            &live("2026-11-04T11:05:00.000Z"),
            cumulative(10.0),
            Some("session-a"),
        )
        .unwrap();
        write_cumulative_turn(
            &connection,
            &live("2026-11-04T11:10:00.000Z"),
            cumulative(12.0),
            Some("session-a"),
        )
        .unwrap();
        write_cumulative_turn(
            &connection,
            &live("2026-11-04T11:15:00.000Z"),
            cumulative(15.0),
            Some("session-a"),
        )
        .unwrap();
        let rows: Vec<Option<f64>> = {
            let mut statement = connection
                .prepare("SELECT cost_usd FROM turns WHERE thread_id = ?1 ORDER BY started_at")
                .unwrap();
            statement
                .query_map([thread], |row| row.get(0))
                .unwrap()
                .filter_map(Result::ok)
                .collect()
        };
        assert_eq!(
            rows,
            [Some(830.0), Some(0.0), Some(2.0), Some(3.0)],
            "the old rows stay, and no total already in them is added again"
        );
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
            Some("session-a"),
        )
        .unwrap();
        write_cumulative_turn(
            &connection,
            &live("2026-11-04T12:00:00.000Z"),
            cumulative(900, 0.6),
            Some("session-a"),
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

    // ------------------------------------------------------ request telemetry

    fn days_ago(days: i64) -> String {
        (chrono::Utc::now() - chrono::Duration::days(days))
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    }

    fn minutes_ago(minutes: i64) -> String {
        (chrono::Utc::now() - chrono::Duration::minutes(minutes))
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    }

    fn count_rows(path: &Path, table: &str) -> i64 {
        let sql = format!("SELECT COUNT(*) AS n FROM {table}");
        query_path(path, &sql, 10).unwrap().rows[0]["n"]
            .as_i64()
            .unwrap()
    }

    fn one_row(path: &Path, sql: &str) -> Value {
        let rows = query_path(path, sql, 10).unwrap().rows;
        assert_eq!(rows.len(), 1, "{sql}: {rows:?}");
        rows.into_iter().next().unwrap()
    }

    fn provider_row(thread: &str, started_at: &str, payload: Option<&str>) -> ProviderRequestRow {
        ProviderRequestRow {
            thread_id: thread.to_owned(),
            turn_id: Some("turn-1".to_owned()),
            round: Some(2),
            provider: Some("openai".to_owned()),
            model: Some("gpt-6-luna".to_owned()),
            endpoint_host: Some("api.openai.com".to_owned()),
            endpoint_path: Some("/v1/chat/completions".to_owned()),
            method: Some("POST".to_owned()),
            attempt: Some(1),
            started_at: started_at.to_owned(),
            headers_at: Some(started_at.to_owned()),
            finished_at: Some(started_at.to_owned()),
            latency_ms: Some(412),
            status_code: Some(200),
            request_bytes: Some(1024),
            response_bytes: Some(2048),
            stream_event_count: Some(37),
            input_tokens: Some(100),
            output_tokens: Some(20),
            cached_tokens: None,
            error_kind: None,
            request_payload_json: payload.map(str::to_owned),
            response_summary_json: None,
            payload_truncated: false,
        }
    }

    fn rpc_row(thread: &str, at: &str, payload: Option<&str>) -> RpcMessageRow {
        RpcMessageRow {
            thread_id: thread.to_owned(),
            turn_id: Some("turn-1".to_owned()),
            harness: Some("codex".to_owned()),
            protocol: Some("acp".to_owned()),
            direction: DIRECTION_RUSTCODE_TO_HARNESS.to_owned(),
            message_kind: KIND_REQUEST.to_owned(),
            rpc_id: Some("7".to_owned()),
            method: Some("session/prompt".to_owned()),
            at: at.to_owned(),
            latency_ms: Some(30),
            byte_size: Some(512),
            payload_json: payload.map(str::to_owned),
            payload_truncated: false,
        }
    }

    fn event_row(thread: &str, at: &str, fields: Option<&str>) -> OperationalEventRow {
        OperationalEventRow {
            thread_id: thread.to_owned(),
            turn_id: Some("turn-1".to_owned()),
            event_type: "turn.summary".to_owned(),
            at: at.to_owned(),
            fields_json: fields.map(str::to_owned),
            original_bytes: fields.map(|fields| fields.len() as i64),
            captured_bytes: None,
            payload_truncated: false,
        }
    }

    fn compaction_row(thread: &str, at: &str, trigger: &str) -> CompactionRow {
        CompactionRow {
            thread_id: thread.to_owned(),
            turn_id: None,
            harness: Some("claude-code".to_owned()),
            model: None,
            at: at.to_owned(),
            duration_ms: Some(2_400),
            trigger: trigger.to_owned(),
            mode: Some("harness_summary".to_owned()),
            tokens_before: Some(120_000),
            tokens_after: Some(18_000),
            messages_before: None,
            messages_after: None,
            idle_secs_before: None,
            count: Some(1),
            success: true,
            error_kind: None,
            external_id: Some("compact-1".to_owned()),
            source: Some("claude-code".to_owned()),
        }
    }

    #[test]
    fn migration_adds_turn_metrics_and_the_compactions_table() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DATA_FILE);
        let connection = open(&path).unwrap();
        let columns = |table: &str| -> Vec<String> {
            connection
                .prepare(&format!("PRAGMA table_info({table})"))
                .unwrap()
                .query_map([], |row| row.get(1))
                .unwrap()
                .filter_map(Result::ok)
                .collect()
        };
        let turns = columns("turns");
        for column in [
            "duration_ms",
            "time_to_first_token_ms",
            "model_time_ms",
            "tool_time_ms",
            "context_tokens",
            "context_window",
            "stop_reason",
        ] {
            assert!(turns.iter().any(|name| name == column), "{column}");
        }
        let compactions = columns("compactions");
        for column in [
            "thread_id",
            "harness",
            "trigger",
            "mode",
            "tokens_before",
            "tokens_after",
            "success",
            "external_id",
            "source",
        ] {
            assert!(compactions.iter().any(|name| name == column), "{column}");
        }
        let views: Vec<String> = connection
            .prepare("SELECT name FROM sqlite_master WHERE type = 'view'")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        for view in [
            "daily_usage",
            "daily_compactions",
            "tool_failure_rates",
            "turn_latency_daily",
        ] {
            assert!(views.iter().any(|name| name == view), "{view}");
        }
    }

    #[test]
    fn stats_count_rows_and_a_tables_indexes_with_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DATA_FILE);
        let connection = open(&path).unwrap();
        write_turn(
            &connection,
            &turn("t", "2026-11-01T10:00:00Z", (10, 2), "completed"),
        )
        .unwrap();
        let index_bytes: i64 = connection
            .query_row(
                "SELECT COALESCE(SUM(pgsize), 0) FROM dbstat WHERE name = 'turns_day'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        drop(connection);

        let stats = stats_at(&path).unwrap();
        let object = |name: &str| {
            stats["objects"]
                .as_array()
                .unwrap()
                .iter()
                .find(|object| object["name"] == name)
                .cloned()
                .unwrap_or_else(|| panic!("{name}"))
        };
        assert_eq!(object("turns")["rows"], 1);
        assert_eq!(object("compactions")["rows"], 0);
        assert_eq!(object("daily_usage")["rows"], 1, "a view reports its rows");
        let bytes = object("turns")["bytes"].as_i64().expect("dbstat sizes");
        assert!(
            bytes > index_bytes,
            "the table's pages and its indexes both count: {bytes} <= {index_bytes}"
        );
    }

    #[test]
    fn a_compaction_round_trips_and_a_repeated_update_keeps_one_row() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DATA_FILE);
        let connection = open(&path).unwrap();
        upsert_thread(
            &connection,
            &ThreadRow {
                id: "t".to_owned(),
                harness: Some("claude-code".to_owned()),
                model: Some("sonnet".to_owned()),
                ..ThreadRow::default()
            },
        )
        .unwrap();
        let at = "2026-11-01T10:00:00Z";
        record_compaction_at(&path, &compaction_row("t", at, "manual")).unwrap();
        let row = one_row(&path, "SELECT * FROM compactions");
        assert_eq!(row["thread_id"], "t");
        assert_eq!(row["harness"], "claude-code");
        assert_eq!(row["model"], "sonnet", "filled from the thread row");
        assert_eq!(row["trigger"], "manual");
        assert_eq!(row["mode"], "harness_summary");
        assert_eq!(row["tokens_before"], 120_000);
        assert_eq!(row["tokens_after"], 18_000);
        assert_eq!(row["duration_ms"], 2_400);
        assert_eq!(row["success"], 1);
        assert_eq!(row["at"], "2026-11-01T10:00:00.000Z");

        // The same harness event again (a duplicate terminal update) updates
        // its row instead of adding a second one.
        let mut again = compaction_row("t", "2026-11-01T10:00:01Z", "manual");
        again.tokens_after = Some(12_000);
        record_compaction_at(&path, &again).unwrap();
        assert_eq!(count_rows(&path, "compactions"), 1);
        let row = one_row(&path, "SELECT tokens_after FROM compactions");
        assert_eq!(row["tokens_after"], 12_000);

        // The one-SELECT acceptance question, through the same guards as
        // `query_data`.
        let daily = one_row(
            &path,
            "SELECT compactions, reclaimed_tokens FROM daily_compactions",
        );
        assert_eq!(daily["compactions"], 1);
        assert_eq!(daily["reclaimed_tokens"], 108_000);
    }

    #[test]
    fn turn_and_tool_metrics_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DATA_FILE);
        let connection = open(&path).unwrap();
        let mut row = turn("t", "2026-11-01T10:00:00Z", (100, 20), "completed");
        row.duration_ms = Some(9_500);
        row.time_to_first_token_ms = Some(700);
        row.model_time_ms = Some(7_000);
        row.tool_time_ms = Some(1_500);
        row.context_tokens = Some(48_000);
        row.context_window = Some(200_000);
        row.stop_reason = Some("end_turn".to_owned());
        row.tool_calls[0].duration_ms = Some(250);
        row.tool_calls[0].truncated = Some(true);
        row.tool_calls[0].command_name = Some("cargo".to_owned());
        row.tool_calls[0].error_kind = Some("exit_code".to_owned());
        row.tool_calls[0].output_bytes = Some(512);
        row.tool_calls.push(ToolCallRow {
            tool_name: "mcp__rustcode__thread_usage".to_owned(),
            ..Default::default()
        });
        write_turn(&connection, &row).unwrap();

        let stored = one_row(&path, "SELECT * FROM turns");
        assert_eq!(stored["duration_ms"], 9_500);
        assert_eq!(stored["time_to_first_token_ms"], 700);
        assert_eq!(stored["model_time_ms"], 7_000);
        assert_eq!(stored["tool_time_ms"], 1_500);
        assert_eq!(stored["context_tokens"], 48_000);
        assert_eq!(stored["context_window"], 200_000);
        assert_eq!(stored["stop_reason"], "end_turn");
        let tools = query_path(
            &path,
            "SELECT tool_name, mcp_server, command_name, duration_ms, truncated, error_kind \
             FROM tool_calls ORDER BY id",
            10,
        )
        .unwrap()
        .rows;
        assert_eq!(tools[0]["command_name"], "cargo");
        assert_eq!(tools[0]["duration_ms"], 250);
        assert_eq!(tools[0]["truncated"], 1);
        assert_eq!(tools[0]["error_kind"], "exit_code");
        assert!(tools[0]["mcp_server"].is_null());
        assert_eq!(tools[1]["mcp_server"], "rustcode");
        let latency = one_row(
            &path,
            "SELECT turns, mean_duration_ms FROM turn_latency_daily",
        );
        assert_eq!(latency["turns"], 1);
        assert_eq!(latency["mean_duration_ms"], 9_500.0);
    }

    #[test]
    fn the_telemetry_tables_migrate_again_without_losing_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DATA_FILE);
        let connection = open(&path).unwrap();
        write_turn(
            &connection,
            &turn("t", "2026-11-01T10:00:00Z", (10, 2), "completed"),
        )
        .unwrap();
        record_provider_request_at(&path, &provider_row("t", "2026-11-01T10:00:00Z", None))
            .unwrap();

        // Opening again runs `migrate` on a store that already has the tables,
        // which must find its own work done rather than fail on CREATE TABLE.
        let connection = open(&path).unwrap();
        migrate(&connection).unwrap();
        let recorded: i64 = connection
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(recorded, MIGRATIONS.len() as i64);
        assert_eq!(count_rows(&path, "turns"), 1, "the old rows survive");
        assert_eq!(count_rows(&path, "provider_requests"), 1);
        assert_eq!(count_rows(&path, "harness_rpc_messages"), 0);
        assert_eq!(count_rows(&path, "usage_cursors"), 0);
        assert_eq!(count_rows(&path, "operational_events"), 0);
        let kept = one_row(&path, "SELECT status FROM turns");
        assert_eq!(kept["status"], "completed");
    }

    #[test]
    fn a_store_at_migration_seven_gets_the_tool_call_turn_index() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DATA_FILE);
        let indexes = |path: &Path| -> Vec<String> {
            let connection = Connection::open(path).unwrap();
            connection
                .prepare("PRAGMA index_list(tool_calls)")
                .unwrap()
                .query_map([], |row| row.get::<_, String>(1))
                .unwrap()
                .filter_map(Result::ok)
                .collect()
        };
        // A store as migration 7 left it: every earlier step recorded, rows
        // written, and no index on `tool_calls.turn_id`.
        {
            let connection = Connection::open(&path).unwrap();
            connection
                .execute_batch(
                    "CREATE TABLE schema_migrations (id INTEGER PRIMARY KEY, name TEXT NOT NULL);",
                )
                .unwrap();
            for (id, name, sql) in MIGRATIONS.iter().filter(|(id, _, _)| *id <= 7) {
                connection.execute_batch(sql).unwrap();
                connection
                    .execute(
                        "INSERT INTO schema_migrations (id, name) VALUES (?1, ?2)",
                        params![id, name],
                    )
                    .unwrap();
            }
            write_turn(
                &connection,
                &turn("t", "2026-11-01T10:00:00Z", (10, 2), "completed"),
            )
            .unwrap();
        }
        assert!(
            !indexes(&path).iter().any(|name| name == "tool_calls_turn"),
            "the migration-7 store has no such index"
        );

        // Opening runs migration 8 on top of 7, and the index it adds is the
        // one an upsert's `DELETE FROM tool_calls WHERE turn_id = ?` needs.
        let connection = open(&path).unwrap();
        assert!(indexes(&path).iter().any(|name| name == "tool_calls_turn"));
        let recorded: i64 = connection
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(recorded, MIGRATIONS.len() as i64);
        assert_eq!(count_rows(&path, "turns"), 1, "the old rows survive");
        assert_eq!(count_rows(&path, "tool_calls"), 1, "the old rows survive");
        // `IF NOT EXISTS` keeps the step harmless on a store that already has
        // the index some other way.
        let last = MIGRATIONS.iter().find(|(id, _, _)| *id == 8).unwrap().2;
        connection.execute_batch(last).unwrap();
    }

    #[test]
    fn telemetry_rows_round_trip_without_a_payload_by_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DATA_FILE);
        let at = "2026-11-01T10:00:00Z";
        record_provider_request_at(&path, &provider_row("t", at, None)).unwrap();
        let written = record_rpc_messages_at(&path, &[&rpc_row("t", at, None)]).unwrap();
        assert_eq!(written, 1);
        let sequence = record_operational_event_at(&path, &event_row("t", at, None)).unwrap();
        assert_eq!(sequence, 1);

        let request = one_row(&path, "SELECT * FROM provider_requests");
        assert_eq!(request["thread_id"], "t");
        assert_eq!(request["turn_id"], "turn-1");
        assert_eq!(request["round"], 2);
        assert_eq!(request["provider"], "openai");
        assert_eq!(request["endpoint_host"], "api.openai.com");
        assert_eq!(request["endpoint_path"], "/v1/chat/completions");
        assert_eq!(request["attempt"], 1);
        assert_eq!(request["latency_ms"], 412);
        assert_eq!(request["status_code"], 200);
        assert_eq!(request["stream_event_count"], 37);
        assert_eq!(request["input_tokens"], 100);
        // Timestamps are normalised, so string order is time order.
        assert_eq!(request["started_at"], "2026-11-01T10:00:00.000Z");
        // Metadata by default: no payload, no redaction to record.
        assert!(request["request_payload_json"].is_null(), "{request}");
        assert_eq!(request["payload_truncated"], 0);
        assert_eq!(request["redaction_version"], 0);

        let rpc = one_row(&path, "SELECT * FROM harness_rpc_messages");
        assert_eq!(rpc["direction"], DIRECTION_RUSTCODE_TO_HARNESS);
        assert_eq!(rpc["message_kind"], KIND_REQUEST);
        assert_eq!(rpc["rpc_id"], "7");
        assert_eq!(rpc["method"], "session/prompt");
        assert_eq!(rpc["byte_size"], 512);
        assert!(rpc["payload_json"].is_null(), "{rpc}");

        let event = one_row(&path, "SELECT * FROM operational_events");
        assert_eq!(event["event_type"], "turn.summary");
        assert_eq!(event["sequence"], 1);
        assert!(event["fields_json"].is_null(), "{event}");
        assert_eq!(event["redaction_version"], 0);
    }

    #[test]
    fn a_captured_payload_is_redacted_and_capped_before_it_is_stored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DATA_FILE);
        let at = "2026-11-01T10:00:00Z";
        let payload = json!({
            "model": "gpt-6-luna",
            "headers": {"authorization": "Bearer sk-live-123", "accept": "text/event-stream"},
            "pad": "z".repeat(4000),
        });
        let (text, truncated) = redact::prepare_payload(&payload, 512);
        assert!(truncated, "a 4 KiB pad is clipped");
        let mut row = provider_row("t", at, Some(&text));
        row.payload_truncated = truncated;
        record_provider_request_at(&path, &row).unwrap();

        let stored = one_row(&path, "SELECT * FROM provider_requests");
        let kept = stored["request_payload_json"].as_str().unwrap();
        assert!(
            kept.len() <= MAX_PROVIDER_PAYLOAD_BYTES,
            "{} bytes",
            kept.len()
        );
        assert!(!kept.contains("sk-live-123"), "{kept}");
        assert_eq!(stored["payload_truncated"], 1);
        assert_eq!(stored["redaction_version"], redact::REDACTION_VERSION);
        let parsed: Value = serde_json::from_str(kept).expect("the stored payload is valid JSON");
        assert_eq!(parsed["model"], "gpt-6-luna");
        assert_eq!(parsed["headers"]["authorization"], redact::REDACTED);

        // A caller that skipped the cap is still capped, and told so.
        let long = format!(
            "{{\"pad\":\"{}\"}}",
            "z".repeat(MAX_PROVIDER_PAYLOAD_BYTES + 200)
        );
        record_provider_request_at(&path, &provider_row("u", at, Some(&long))).unwrap();
        let stored = one_row(
            &path,
            "SELECT * FROM provider_requests WHERE thread_id = 'u'",
        );
        let kept = stored["request_payload_json"].as_str().unwrap();
        assert!(
            kept.len() <= MAX_PROVIDER_PAYLOAD_BYTES,
            "{} bytes",
            kept.len()
        );
        assert_eq!(stored["payload_truncated"], 1);
        assert!(serde_json::from_str::<Value>(kept).is_ok(), "{kept}");

        // The insert redacts again, so a caller that skipped it cannot write
        // a secret.
        let raw = r#"{"headers": {"Authorization": "Bearer sk-raw-1"}, "max_tokens": 64}"#;
        record_provider_request_at(&path, &provider_row("v", at, Some(raw))).unwrap();
        let stored = one_row(
            &path,
            "SELECT * FROM provider_requests WHERE thread_id = 'v'",
        );
        let kept = stored["request_payload_json"].as_str().unwrap();
        assert!(!kept.contains("sk-raw-1"), "{kept}");
        assert_eq!(stored["redaction_version"], redact::REDACTION_VERSION);
        let parsed: Value = serde_json::from_str(kept).unwrap();
        assert_eq!(parsed["headers"]["Authorization"], redact::REDACTED);
        // A token limit is a number, not a secret, so it is kept.
        assert_eq!(parsed["max_tokens"], 64);
    }

    #[test]
    fn every_telemetry_payload_is_held_to_its_own_cap() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DATA_FILE);
        let at = "2026-11-01T10:00:00Z";
        let long = |field: &str| format!("{{\"{field}\":\"{}\"}}", "z".repeat(32 * 1024));

        record_rpc_messages_at(&path, &[&rpc_row("t", at, Some(&long("params")))]).unwrap();
        let stored = one_row(&path, "SELECT * FROM harness_rpc_messages");
        let kept = stored["payload_json"].as_str().unwrap();
        assert!(kept.len() <= MAX_RPC_PAYLOAD_BYTES, "{} bytes", kept.len());
        assert_eq!(stored["payload_truncated"], 1);
        assert_eq!(stored["redaction_version"], redact::REDACTION_VERSION);
        assert!(serde_json::from_str::<Value>(kept).is_ok(), "{kept}");

        record_operational_event_at(&path, &event_row("t", at, Some(&long("fields")))).unwrap();
        let stored = one_row(&path, "SELECT * FROM operational_events");
        let kept = stored["fields_json"].as_str().unwrap();
        assert!(kept.len() <= MAX_EVENT_FIELDS_BYTES, "{} bytes", kept.len());
        assert_eq!(stored["payload_truncated"], 1);
        assert_eq!(stored["captured_bytes"], kept.len() as i64);
        assert!(serde_json::from_str::<Value>(kept).is_ok(), "{kept}");
    }

    #[test]
    fn operational_events_number_themselves_per_thread() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DATA_FILE);
        let at = "2026-11-01T10:00:00Z";
        for expected in 1..=3 {
            let sequence =
                record_operational_event_at(&path, &event_row("a", at, Some("{\"n\":1}"))).unwrap();
            assert_eq!(sequence, expected);
        }
        let other = record_operational_event_at(&path, &event_row("b", at, None)).unwrap();
        assert_eq!(other, 1, "each thread has its own sequence");
        // A scratch id is not a thread, so nothing is stored for it.
        assert_eq!(
            record_operational_event_at(&path, &event_row("session-title-1700000000000", at, None))
                .unwrap(),
            0,
            "a title request is not a thread"
        );
        let sequences = query_path(
            &path,
            "SELECT thread_id, sequence FROM operational_events ORDER BY thread_id, sequence",
            10,
        )
        .unwrap()
        .rows;
        assert_eq!(sequences.len(), 4, "{sequences:?}");
    }

    #[test]
    fn pruning_drops_old_raw_payloads_and_keeps_structured_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DATA_FILE);
        let connection = open(&path).unwrap();
        let payload = "{\"a\":1}";
        write_provider_request(&connection, &provider_row("t", &days_ago(8), Some(payload)))
            .unwrap();
        write_rpc_messages(&connection, &[&rpc_row("t", &days_ago(8), Some(payload))]).unwrap();
        write_operational_event(&connection, &event_row("t", &days_ago(8), Some(payload))).unwrap();
        write_turn(
            &connection,
            &turn("t", &days_ago(400), (10, 2), "completed"),
        )
        .unwrap();
        // Older than the payload window.
        write_provider_request(
            &connection,
            &provider_row("t", &days_ago(40), Some(payload)),
        )
        .unwrap();
        // Fresh: untouched.
        write_provider_request(&connection, &provider_row("t", &days_ago(0), Some(payload)))
            .unwrap();

        let report = prune_with(&connection, TELEMETRY_MAX_BYTES, None).unwrap();
        assert!(report.payloads_cleared >= 2, "{report:?}");
        assert_eq!(
            report.rows_deleted, 0,
            "forever deletes no rows: {report:?}"
        );

        let requests = query_path(
            &path,
            "SELECT started_at, request_payload_json FROM provider_requests ORDER BY started_at",
            10,
        )
        .unwrap()
        .rows;
        assert_eq!(requests.len(), 3, "every raw row stays: {requests:?}");
        assert!(
            requests[0]["request_payload_json"].is_null(),
            "the 40-day payload is gone: {requests:?}"
        );
        assert!(
            requests[1]["request_payload_json"].is_null(),
            "the 8-day payload is gone: {requests:?}"
        );
        assert_eq!(
            requests[2]["request_payload_json"], payload,
            "today keeps its payload"
        );

        let rpc = one_row(&path, "SELECT payload_json FROM harness_rpc_messages");
        assert!(rpc["payload_json"].is_null(), "{rpc}");
        // Operational events are structured rows: their fields are history.
        let event = one_row(
            &path,
            "SELECT fields_json, captured_bytes FROM operational_events",
        );
        assert_eq!(event["fields_json"], payload, "{event}");
        // A 400-day-old turn is still queryable.
        assert_eq!(count_rows(&path, "turns"), 1);
    }

    #[test]
    fn a_folded_streaming_row_keeps_its_metadata_and_a_captured_tool_call_its_body() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DATA_FILE);
        let connection = open(&path).unwrap();
        let coalesced = "{\"coalesced\":42,\"update\":\"command_output\",\"bytes\":900}";
        let raw_body = format!("{{\"delta\":\"{}\"}}", "x".repeat(900));
        let chunk_text = format!(
            "{{\"params\":{{\"update\":{{\"sessionUpdate\":\"agent_message_chunk\",\"content\":\"{}\"}}}}}}",
            "y".repeat(900)
        );
        let captured_tool_call = format!(
            "{{\"params\":{{\"update\":{{\"sessionUpdate\":\"tool_call_update\",\"rawOutput\":{{\"text\":\"{}\"}}}}}}}}",
            "z".repeat(900)
        );
        let row = |method: &str, payload: &str| {
            let mut row = rpc_row("t", &days_ago(0), Some(payload));
            row.method = Some(method.to_owned());
            row
        };
        write_rpc_messages(
            &connection,
            &[
                &row("item/commandExecution/outputDelta", coalesced),
                &row("item/commandExecution/outputDelta", &raw_body),
                &row("session/update", &chunk_text),
                &row("session/update", &captured_tool_call),
            ],
        )
        .unwrap();

        let report = prune_with(&connection, TELEMETRY_MAX_BYTES, None).unwrap();
        assert_eq!(
            report.payloads_cleared, 2,
            "the raw bodies only: {report:?}"
        );
        let payloads: Vec<Value> = query_path(
            &path,
            "SELECT payload_json FROM harness_rpc_messages ORDER BY id",
            10,
        )
        .unwrap()
        .rows
        .into_iter()
        .map(|row| row["payload_json"].clone())
        .collect();
        assert_eq!(
            payloads[0], coalesced,
            "a folded chunk keeps its counts: {payloads:?}"
        );
        assert!(payloads[1].is_null(), "the raw delta body is gone");
        assert!(payloads[2].is_null(), "the chunk text is gone");
        assert_eq!(
            payloads[3], captured_tool_call,
            "a captured tool call is not noise: {payloads:?}"
        );
        assert_eq!(count_rows(&path, "harness_rpc_messages"), 4);
    }

    #[test]
    fn a_configured_retention_deletes_only_raw_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DATA_FILE);
        let connection = open(&path).unwrap();
        write_provider_request(&connection, &provider_row("t", &days_ago(40), None)).unwrap();
        write_rpc_messages(&connection, &[&rpc_row("t", &days_ago(40), None)]).unwrap();
        write_operational_event(&connection, &event_row("t", &days_ago(40), None)).unwrap();
        write_turn(&connection, &turn("t", &days_ago(40), (10, 2), "completed")).unwrap();

        let report = prune_with(&connection, TELEMETRY_MAX_BYTES, Some(30)).unwrap();
        assert_eq!(report.rows_deleted, 2, "only the raw rows: {report:?}");
        assert_eq!(count_rows(&path, "provider_requests"), 0);
        assert_eq!(count_rows(&path, "harness_rpc_messages"), 0);
        assert_eq!(count_rows(&path, "operational_events"), 1);
        assert_eq!(count_rows(&path, "turns"), 1);
    }

    #[test]
    fn telemetry_retention_parses_forever_and_days() {
        assert_eq!(super::parse_retention_days("forever"), None);
        assert_eq!(super::parse_retention_days(""), None);
        assert_eq!(super::parse_retention_days(" 30 "), Some(30));
        assert_eq!(super::parse_retention_days("0"), None);
        assert_eq!(super::parse_retention_days("later"), None);
    }

    #[test]
    fn the_byte_budget_drops_the_oldest_payloads_first() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DATA_FILE);
        let connection = open(&path).unwrap();
        let payload = format!("{{\"pad\":\"{}\"}}", "p".repeat(1024));
        // 300 rows of ~1 KiB, one minute apart so none is old enough for the
        // age policy, newest last.
        for minutes in (0..300).rev() {
            write_provider_request(
                &connection,
                &provider_row("t", &minutes_ago(minutes), Some(&payload)),
            )
            .unwrap();
        }
        let budget = 100 * 1024;
        let report = prune_with(&connection, budget, None).unwrap();
        assert!(report.bytes_before > budget, "{report:?}");
        assert!(report.bytes_after <= budget, "{report:?}");
        assert!(report.payloads_cleared > 0, "{report:?}");
        assert_eq!(
            report.rows_deleted, 0,
            "payloads go before rows: {report:?}"
        );
        assert_eq!(count_rows(&path, "provider_requests"), 300);

        let newest = one_row(
            &path,
            "SELECT request_payload_json FROM provider_requests ORDER BY started_at DESC LIMIT 1",
        );
        assert_eq!(
            newest["request_payload_json"], payload,
            "the newest payload stays"
        );
        let oldest = one_row(
            &path,
            "SELECT request_payload_json FROM provider_requests ORDER BY started_at LIMIT 1",
        );
        assert!(
            oldest["request_payload_json"].is_null(),
            "the oldest payload goes"
        );
    }

    // -------------------------------------------------------------- activity

    #[test]
    fn newest_activity_pages_include_provider_failures_without_gaps() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DATA_FILE);
        mixed_activity(&path, "t", 4);
        let mut items = Vec::new();
        let mut after = None;
        loop {
            let page = activity_at_ordered(&path, "t", after.as_deref(), 2, true).unwrap();
            items.extend(page.items);
            after = page.next;
            if !page.has_more {
                break;
            }
        }
        assert_eq!(items.len(), 12);
        let keys: Vec<_> = items
            .iter()
            .map(|i| (i["at"].as_str().unwrap(), i["source"].as_str().unwrap()))
            .collect();
        assert!(keys.windows(2).all(|w| w[0] > w[1]));
        assert!(items.iter().any(|i| i["error_kind"] == "http_500"));
        assert!(items.iter().any(|i| i["status_code"] == 500));
        assert_eq!(
            last_provider_error_at(&path, "t").unwrap().unwrap()["error_kind"],
            "http_500"
        );
    }

    /// One row in each telemetry table per minute, oldest first, so a page
    /// boundary can fall inside, between and across sources.
    fn mixed_activity(path: &Path, thread: &str, minutes: usize) {
        for minute in (0..minutes).rev() {
            let at = minutes_ago(minute as i64);
            let mut request = provider_row(thread, &at, None);
            request.error_kind = (minute % 3 == 0).then(|| "http_500".to_owned());
            if request.error_kind.is_some() {
                request.status_code = Some(500);
            }
            write_provider_request(&open(path).unwrap(), &request).unwrap();
            record_rpc_messages_at(path, &[&rpc_row(thread, &at, None)]).unwrap();
            record_operational_event_at(
                path,
                &OperationalEventRow {
                    event_type: "turn.start".to_owned(),
                    ..event_row(thread, &at, Some("{\"tool_rounds\":1}"))
                },
            )
            .unwrap();
        }
    }

    #[test]
    fn activity_pages_have_no_gaps_or_duplicates_across_sources() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DATA_FILE);
        mixed_activity(&path, "t", 4);
        mixed_activity(&path, "other", 2);

        let mut seen: Vec<(String, String)> = Vec::new();
        let mut after: Option<String> = None;
        loop {
            let page = activity_at(&path, "t", after.as_deref(), 5).unwrap();
            assert!(page.items.len() <= 5);
            for item in &page.items {
                let at = item["at"].as_str().unwrap().to_owned();
                let source = item["source"].as_str().unwrap().to_owned();
                seen.push((at, source));
            }
            if let Some(next) = page.next {
                after = Some(next);
            }
            if !page.has_more {
                break;
            }
        }
        // 4 minutes x 3 sources, this thread only.
        assert_eq!(seen.len(), 12, "{seen:?}");
        let mut unique = seen.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), seen.len(), "a page repeated an item");
        let mut sorted = seen.clone();
        sorted.sort();
        assert_eq!(sorted, seen, "pages are in time order");
        assert!(
            seen.iter().all(|(_, source)| source != ""),
            "every item names its source"
        );
    }

    #[test]
    fn activity_limits_are_clamped_and_a_bad_cursor_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DATA_FILE);
        mixed_activity(&path, "t", 3);

        assert_eq!(activity_at(&path, "t", None, 0).unwrap().items.len(), 1);
        // More rows than one capped page, so the cap is visible and a page says
        // there is more instead of silently stopping at 200.
        mixed_activity(&path, "t", 80);
        let capped = activity_at(&path, "t", None, MAX_ACTIVITY_ITEMS + 1).unwrap();
        assert_eq!(capped.items.len(), MAX_ACTIVITY_ITEMS);
        assert!(capped.has_more);

        let error = activity_at(&path, "t", Some("nonsense"), 5).unwrap_err();
        assert!(error.contains("next token"), "{error}");
        let error = activity_at(&path, "t", Some("2026-11-01T00:00:00Z|nope|1"), 5).unwrap_err();
        assert!(error.contains("next token"), "{error}");
        assert!(activity_at(&path, "  ", None, 5).is_err());
    }

    #[test]
    fn activity_returns_summaries_and_never_a_payload() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DATA_FILE);
        let at = "2026-11-01T10:00:00Z";
        record_provider_request_at(
            &path,
            &provider_row("t", at, Some("{\"messages\":\"SECRET-PROMPT\"}")),
        )
        .unwrap();
        record_rpc_messages_at(
            &path,
            &[&rpc_row("t", at, Some("{\"params\":\"SECRET-RPC\"}"))],
        )
        .unwrap();
        record_operational_event_at(&path, &event_row("t", at, Some("{\"n\":1}"))).unwrap();

        let page = activity_at(&path, "t", None, 10).unwrap();
        assert_eq!(page.items.len(), 3);
        let rendered = serde_json::to_string(&page.items).unwrap();
        assert!(!rendered.contains("SECRET"), "{rendered}");
        let request = page
            .items
            .iter()
            .find(|item| item["source"] == "provider_request")
            .unwrap();
        assert_eq!(request["provider"], "openai");
        assert_eq!(request["model"], "gpt-6-luna");
        assert_eq!(request["status_code"], 200);
        assert_eq!(request["latency_ms"], 412);
        assert_eq!(request["input_tokens"], 100);
        assert!(request.get("request_payload_json").is_none());
        let rpc = page
            .items
            .iter()
            .find(|item| item["source"] == "rpc_message")
            .unwrap();
        assert_eq!(rpc["direction"], DIRECTION_RUSTCODE_TO_HARNESS);
        assert_eq!(rpc["kind"], KIND_REQUEST);
        assert_eq!(rpc["method"], "session/prompt");
        assert_eq!(rpc["byte_size"], 512);
        assert!(rpc.get("payload_json").is_none());
        let event = page
            .items
            .iter()
            .find(|item| item["source"] == "event")
            .unwrap();
        assert_eq!(event["type"], "turn.summary");
        assert_eq!(event["fields"]["n"], 1);

        // A thread nothing was recorded for is an empty page, and so is a
        // store that does not exist yet.
        assert_eq!(
            activity_at(&path, "nobody", None, 10).unwrap().items.len(),
            0
        );
        assert_eq!(
            activity_at(&dir.path().join("missing.sqlite"), "t", None, 10)
                .unwrap()
                .items
                .len(),
            0
        );
    }

    fn receipt(caller: &str, key: &str, action: &str, hash: &str) -> ReceiptRow {
        ReceiptRow {
            caller: caller.to_owned(),
            client_request_id: key.to_owned(),
            action: action.to_owned(),
            args_hash: hash.to_owned(),
            result_json: json!({ "session_id": "child", "link_id": "a/1" }).to_string(),
        }
    }

    #[test]
    fn a_receipt_round_trips_and_a_changed_call_is_a_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DATA_FILE);

        assert_eq!(
            check_receipt_at(&path, "a", "k1", "start_session", "h1").unwrap(),
            Receipt::None
        );
        record_receipt_at(&path, &receipt("a", "k1", "start_session", "h1")).unwrap();

        // The same call again: the recorded answer, and nothing stored but the
        // action, the hash and the result.
        let again = check_receipt_at(&path, "a", "k1", "start_session", "h1").unwrap();
        let Receipt::Replay { result, .. } = again else {
            panic!("expected a replay, got {again:?}");
        };
        assert_eq!(result["session_id"], "child");
        assert_eq!(result["link_id"], "a/1");
        let row = one_row(
            &path,
            "SELECT action, args_hash, result_json FROM command_receipts",
        );
        assert_eq!(row["action"], "start_session");
        assert_eq!(row["args_hash"], "h1");

        // The same key for other arguments or another action is refused.
        assert_eq!(
            check_receipt_at(&path, "a", "k1", "start_session", "h2").unwrap(),
            Receipt::Conflict
        );
        assert_eq!(
            check_receipt_at(&path, "a", "k1", "fan_out", "h1").unwrap(),
            Receipt::Conflict
        );
        // Another caller, or no key, is another call.
        assert_eq!(
            check_receipt_at(&path, "b", "k1", "start_session", "h1").unwrap(),
            Receipt::None
        );
        assert_eq!(
            check_receipt_at(&path, "a", "k2", "start_session", "h1").unwrap(),
            Receipt::None
        );

        // A second write under a key keeps the first receipt.
        let mut second = receipt("a", "k1", "start_session", "h1");
        second.result_json = json!({ "session_id": "other" }).to_string();
        record_receipt_at(&path, &second).unwrap();
        let Receipt::Replay { result, .. } =
            check_receipt_at(&path, "a", "k1", "start_session", "h1").unwrap()
        else {
            panic!("the first receipt must stay");
        };
        assert_eq!(result["session_id"], "child");

        // A receipt that carries no caller or key is refused, never written.
        assert!(record_receipt_at(&path, &receipt("", "k1", "start_session", "h1")).is_err());
        assert_eq!(count_rows(&path, "command_receipts"), 1);
    }

    #[test]
    fn receipts_age_out_of_the_retry_window_and_stay_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DATA_FILE);
        let connection = open(&path).unwrap();
        // One inside the window, one past it, inserted with its own timestamp.
        for (key, created) in [("fresh", days_ago(1)), ("old", days_ago(9))] {
            connection
                .execute(
                    "INSERT INTO command_receipts
                        (caller, client_request_id, action, args_hash, result_json, created_at)
                     VALUES ('a', ?1, 'message_session', 'h1', '{}', ?2)",
                    params![key, created],
                )
                .unwrap();
        }

        let report = prune_with(&connection, TELEMETRY_MAX_BYTES, None).unwrap();
        assert_eq!(report.rows_deleted, 1, "{report:?}");
        assert_eq!(
            check_receipt_at(&path, "a", "old", "message_session", "h1").unwrap(),
            Receipt::None,
            "past the window a call is new again"
        );
        assert!(matches!(
            check_receipt_at(&path, "a", "fresh", "message_session", "h1").unwrap(),
            Receipt::Replay { .. }
        ));

        // The row cap drops the oldest keepers of the window, not the newest.
        for key in ["k1", "k2", "k3"] {
            record_receipt_at(&path, &receipt("a", key, "fan_out", "h1")).unwrap();
        }
        let deleted = prune_receipts(&connection, COMMAND_RECEIPT_RETENTION_DAYS, 2).unwrap();
        assert_eq!(deleted, 2, "four receipts, two kept");
        assert_eq!(count_rows(&path, "command_receipts"), 2);
        let mut keys: Vec<String> =
            query_path(&path, "SELECT client_request_id FROM command_receipts", 10)
                .unwrap()
                .rows
                .iter()
                .map(|row| {
                    row["client_request_id"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned()
                })
                .collect();
        keys.sort();
        assert_eq!(keys, ["k2", "k3"], "the newest two stay");
    }
}
