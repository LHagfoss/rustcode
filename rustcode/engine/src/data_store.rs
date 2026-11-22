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

mod migrations;
pub mod redact;
use migrations::MIGRATIONS;
mod links;
pub use links::*;
mod receipts;
pub use receipts::*;

/// In the configuration directory, beside `index.sqlite`.
pub const DATA_FILE: &str = "data.sqlite";

/// Most rows one `data query` returns.
pub const MAX_ROWS: usize = 1000;

/// Wall-clock cap for one `data query`, enforced by the progress handler.
const QUERY_TIMEOUT: Duration = Duration::from_secs(2);

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
    /// An external harness's own name for the tool (`Bash`, `Read`,
    /// `mcp__rustcode__wait_session`, a Codex item type), where `tool_name`
    /// holds the ACP kind (`execute`, `other`). `None` for the built-in
    /// harness, whose `tool_name` is already the tool's name.
    pub harness_tool: Option<String>,
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
    /// The reasoning effort the turn ran with, when the thread has one.
    pub effort: Option<String>,
    /// The named harness account the turn ran on.
    pub account: Option<String>,
    /// Uncommitted changes in the thread's checkout when the turn ended
    /// (`git diff --numstat HEAD`): files, added lines and removed lines. The
    /// store fills these in when the caller leaves them empty.
    pub diff_files: Option<i64>,
    pub diff_added: Option<i64>,
    pub diff_removed: Option<i64>,
    /// Why a failed turn failed, as a short kind (`unknown_session`,
    /// `harness_exited`, `rate_limited`, …; see [`turn_failure_kind`]),
    /// never the error text.
    pub error_kind: Option<String>,
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
            "COALESCE(t.harness, 'unknown') AS harness, names.model AS model, \
             json_group_array(DISTINCT t.model) AS raw_models",
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
    // Resolve provider namespaces only for grouping; turns keep the harness's
    // exact reported model. Never guess what an old `default` alias meant.
    let model_names = if group_by == GroupBy::Model {
        "WITH RECURSIVE names(raw, model) AS (
         SELECT DISTINCT COALESCE(model, ''), COALESCE(model, '') FROM turns
         UNION ALL SELECT raw, substr(model, instr(model, '/') + 1) FROM names
         WHERE instr(model, '/') > 0) "
    } else {
        ""
    };
    let mut sql = format!(
        "{model_names}SELECT {key}, \
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
    if group_by == GroupBy::Model {
        sql.push_str(
            " JOIN names ON names.raw = COALESCE(t.model, '') AND instr(names.model, '/') = 0",
        );
    }
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
    let turn = &with_checkout_diff(turn);
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
    let turn = &with_checkout_diff(turn);
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
    remove_backfill(&transaction, &turn.thread_id)?;
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

fn remove_backfill(connection: &Connection, thread_id: &str) -> Result<(), String> {
    connection
        .execute(
            "DELETE FROM turns WHERE thread_id = ?1 AND status = 'backfill'",
            [thread_id],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
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
    remove_backfill(transaction, &turn.thread_id)?;
    let thread = live_thread(turn);
    upsert_thread(transaction, &thread)?;
    transaction
        .execute(
            "INSERT INTO turns (thread_id, started_at, finished_at, harness, model,
                    input_tokens, output_tokens, cached_tokens, cache_write_tokens,
                    cost_usd, tool_calls, status, duration_ms, time_to_first_token_ms,
                    model_time_ms, tool_time_ms, context_tokens, context_window, stop_reason,
                    effort, account, machine, rustcode_version, diff_files, diff_added,
                    diff_removed, error_kind)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                     ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27)
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
                stop_reason = COALESCE(excluded.stop_reason, turns.stop_reason),
                effort = COALESCE(excluded.effort, turns.effort),
                account = COALESCE(excluded.account, turns.account),
                machine = COALESCE(turns.machine, excluded.machine),
                rustcode_version = COALESCE(excluded.rustcode_version, turns.rustcode_version),
                diff_files = COALESCE(excluded.diff_files, turns.diff_files),
                diff_added = COALESCE(excluded.diff_added, turns.diff_added),
                diff_removed = COALESCE(excluded.diff_removed, turns.diff_removed),
                error_kind = excluded.error_kind",
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
                turn.effort.clone().or_else(|| thread.effort.clone()),
                turn.account,
                local_machine(),
                env!("CARGO_PKG_VERSION"),
                turn.diff_files,
                turn.diff_added,
                turn.diff_removed,
                turn.error_kind,
            ],
        )
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "UPDATE threads SET
                account = COALESCE(?2, account),
                machine = COALESCE(machine, ?3),
                created_with_version = COALESCE(created_with_version, ?4)
             WHERE id = ?1",
            params![
                turn.thread_id,
                turn.account,
                local_machine(),
                env!("CARGO_PKG_VERSION")
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
            .or_else(|| mcp_server_of(&call.tool_name).map(str::to_owned))
            .or_else(|| {
                call.harness_tool
                    .as_deref()
                    .and_then(mcp_server_of)
                    .map(str::to_owned)
            });
        let command_name = call.command_name.clone();
        transaction
            .execute(
                "INSERT INTO tool_calls (thread_id, turn_id, tool_name, success, exit_code, at,
                        started_at, duration_ms, output_bytes, truncated, mcp_server,
                        command_name, error_kind, harness_tool)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
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
                    call.harness_tool,
                ],
            )
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// Make sure a `threads` row exists for `thread_id` so an outcome recorded
/// before the first turn (or for an older thread) has a row to land on.
fn ensure_thread(connection: &Connection, thread_id: &str) -> Result<(), String> {
    connection
        .execute(
            "INSERT INTO threads (id) VALUES (?1) ON CONFLICT(id) DO NOTHING",
            [thread_id],
        )
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// What a thread's work ended as: its branch and the commit it stood on, kept
/// when its worktree is removed so the code can be found in git afterwards.
pub fn record_thread_outcome(thread_id: &str, branch: &str, final_commit: &str) {
    if thread_id.trim().is_empty() || rustcode_session::is_scratch_session_id(thread_id) {
        return;
    }
    let Some(path) = data_path() else { return };
    if let Err(error) = record_thread_outcome_at(&path, thread_id, branch, final_commit) {
        crate::dbg_log!("data store: outcome of {thread_id} not recorded: {error}");
    }
}

pub fn record_thread_outcome_at(
    path: &Path,
    thread_id: &str,
    branch: &str,
    final_commit: &str,
) -> Result<(), String> {
    let connection = open(path)?;
    ensure_thread(&connection, thread_id)?;
    connection
        .execute(
            "UPDATE threads SET branch = COALESCE(NULLIF(?2, ''), branch),
                    final_commit = COALESCE(NULLIF(?3, ''), final_commit)
             WHERE id = ?1",
            params![thread_id, branch, final_commit],
        )
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// The pull request a thread's branch is in, as the watch last read it.
/// `state` is `open`, `closed` or `merged`; a merged pull request's head is
/// the thread's final commit.
pub fn record_thread_pull_request_at(
    path: &Path,
    thread_id: &str,
    url: &str,
    state: &str,
    head: &str,
) -> Result<(), String> {
    let connection = open(path)?;
    ensure_thread(&connection, thread_id)?;
    update_thread_pull_request(&connection, thread_id, url, state, head)
}

fn update_thread_pull_request(
    connection: &Connection,
    thread_id: &str,
    url: &str,
    state: &str,
    head: &str,
) -> Result<(), String> {
    let merged_head = if state == "merged" { head } else { "" };
    connection
        .execute(
            "UPDATE threads SET pull_request = ?2, pull_request_state = ?3,
                    final_commit = COALESCE(NULLIF(?4, ''), final_commit)
             WHERE id = ?1",
            params![thread_id, url, state, merged_head],
        )
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// What one read of a thread's linked pull request showed (#3907): enough to
/// count its failed CI runs and fix rounds and to see it merge, close or be
/// reverted. Nothing here is a guess; a field GitHub did not answer is empty.
#[derive(Debug, Clone, Default)]
pub struct PullRequestOutcome {
    pub url: String,
    pub number: u64,
    pub title: String,
    /// `open`, `closed` or `merged`.
    pub state: String,
    /// The head commit the checks ran on.
    pub head: String,
    /// Whether a check on `head` failed.
    pub checks_failed: bool,
    pub merged_at: Option<String>,
    pub closed_at: Option<String>,
    /// The pull request this one reverts, from a `Reverts owner/name#N` body.
    pub reverts_number: Option<u64>,
    /// The title this one reverts, from a `Revert "<title>"` title.
    pub reverts_title: Option<String>,
}

/// Record a linked pull request on its thread: the `threads` columns and its
/// `thread_outcomes` row.
pub fn record_pull_request(thread_id: &str, outcome: &PullRequestOutcome) {
    if thread_id.trim().is_empty()
        || outcome.url.trim().is_empty()
        || rustcode_session::is_scratch_session_id(thread_id)
    {
        return;
    }
    let Some(path) = data_path() else { return };
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    if let Err(error) = record_pull_request_at(&path, thread_id, outcome, &now) {
        crate::dbg_log!("data store: pull request outcome of {thread_id} not recorded: {error}");
    }
}

/// `owner/name` of a pull request URL such as
/// `https://github.com/owner/name/pull/12`.
pub fn pull_request_repository(url: &str) -> Option<String> {
    let path = url.split_once("://").map_or(url, |(_, rest)| rest);
    let parts: Vec<&str> = path.split('/').collect();
    let pull = parts.iter().position(|part| *part == "pull")?;
    (pull >= 2 && !parts[pull - 2].is_empty() && !parts[pull - 1].is_empty())
        .then(|| format!("{}/{}", parts[pull - 2], parts[pull - 1]))
}

/// [`record_pull_request`] against one store file, at `now`. A new head after
/// the first one seen is a fix round; a head whose checks failed is one CI
/// failure, however many reads see it.
pub fn record_pull_request_at(
    path: &Path,
    thread_id: &str,
    outcome: &PullRequestOutcome,
    now: &str,
) -> Result<(), String> {
    let connection = open(path)?;
    ensure_thread(&connection, thread_id)?;
    update_thread_pull_request(
        &connection,
        thread_id,
        &outcome.url,
        &outcome.state,
        &outcome.head,
    )?;
    let Some(repo) = pull_request_repository(&outcome.url) else {
        return Ok(());
    };
    if outcome.number == 0 {
        return Ok(());
    }
    let number = outcome.number as i64;
    type Previous = (Option<String>, Option<String>, Option<i64>, Option<i64>);
    let previous: Option<Previous> = connection
        .query_row(
            "SELECT head, failed_head, ci_failures, fix_rounds FROM thread_outcomes
             WHERE thread_id = ?1 AND repo = ?2 AND pr_number = ?3",
            params![thread_id, repo, number],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let head = outcome.head.trim();
    let failed_now = outcome.checks_failed && !head.is_empty();
    let (ci_failures, fix_rounds, failed_head) = match previous {
        None => (
            Some(i64::from(failed_now)),
            Some(0),
            failed_now.then(|| head.to_owned()),
        ),
        Some((old_head, old_failed, ci_failures, fix_rounds)) => {
            let pushed = !head.is_empty()
                && old_head
                    .as_deref()
                    .is_some_and(|old| !old.is_empty() && old != head);
            let new_failure = failed_now && old_failed.as_deref() != Some(head);
            (
                ci_failures.map(|count| count + i64::from(new_failure)),
                fix_rounds.map(|count| count + i64::from(pushed)),
                if new_failure {
                    Some(head.to_owned())
                } else {
                    old_failed
                },
            )
        }
    };
    let merged_at = outcome
        .merged_at
        .clone()
        .filter(|at| !at.is_empty())
        .or_else(|| (outcome.state == "merged").then(|| now.to_owned()));
    let closed_at = outcome
        .closed_at
        .clone()
        .filter(|at| !at.is_empty())
        .or_else(|| (outcome.state != "open").then(|| now.to_owned()));
    connection
        .execute(
            "INSERT INTO thread_outcomes (thread_id, repo, pr_number, url, title, state,
                    first_seen_at, updated_at, merged_at, closed_at, closed_unmerged,
                    ci_failures, fix_rounds, head, failed_head)
             VALUES (?1, ?2, ?3, ?4, NULLIF(?5, ''), ?6, ?7, ?7, ?8, ?9, ?10, ?11, ?12,
                     NULLIF(?13, ''), ?14)
             ON CONFLICT(thread_id, repo, pr_number) DO UPDATE SET
                url = excluded.url,
                title = COALESCE(excluded.title, title),
                state = excluded.state,
                updated_at = excluded.updated_at,
                merged_at = COALESCE(merged_at, excluded.merged_at),
                closed_at = COALESCE(closed_at, excluded.closed_at),
                closed_unmerged = excluded.closed_unmerged,
                ci_failures = excluded.ci_failures,
                fix_rounds = excluded.fix_rounds,
                head = COALESCE(excluded.head, head),
                failed_head = excluded.failed_head",
            params![
                thread_id,
                repo,
                number,
                outcome.url,
                outcome.title,
                outcome.state,
                now,
                merged_at,
                closed_at,
                outcome.state == "closed",
                ci_failures,
                fix_rounds,
                head,
                failed_head,
            ],
        )
        .map_err(|error| error.to_string())?;
    // A merged revert marks what it reverted, in the same repository, for
    // every thread that recorded that pull request.
    if outcome.state == "merged" {
        let title = outcome
            .reverts_title
            .as_deref()
            .map(str::trim)
            .filter(|title| !title.is_empty());
        if outcome.reverts_number.is_some() || title.is_some() {
            connection
                .execute(
                    "UPDATE thread_outcomes SET reverted = 1, reverted_by = ?2
                     WHERE repo = ?1 AND pr_number <> ?2 AND state = 'merged'
                       AND (pr_number = ?3 OR (?4 IS NOT NULL AND title = ?4))",
                    params![
                        repo,
                        number,
                        outcome.reverts_number.map(|n| n as i64),
                        title
                    ],
                )
                .map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

/// When a thread was settled, or cleared when it is put back to active.
/// Settling is a status: nothing the thread recorded is removed.
pub fn record_thread_settled(thread_id: &str, settled: bool) {
    if thread_id.trim().is_empty() || rustcode_session::is_scratch_session_id(thread_id) {
        return;
    }
    let Some(path) = data_path() else { return };
    if let Err(error) = record_thread_settled_at(&path, thread_id, settled) {
        crate::dbg_log!("data store: settling {thread_id} not recorded: {error}");
    }
}

pub fn record_thread_settled_at(path: &Path, thread_id: &str, settled: bool) -> Result<(), String> {
    let connection = open(path)?;
    ensure_thread(&connection, thread_id)?;
    let at =
        settled.then(|| chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
    connection
        .execute(
            "UPDATE threads SET settled_at = ?2 WHERE id = ?1",
            params![thread_id, at],
        )
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// One approval a user was asked for and what they decided. Names the tool and
/// the decision; never the command, path or arguments it was about.
#[derive(Debug, Clone, Default)]
pub struct ApprovalRow {
    pub thread_id: String,
    /// `builtin`, `subagent` or `harness`.
    pub source: String,
    pub tool_name: Option<String>,
    pub requested_at: String,
    pub decided_at: Option<String>,
    pub wait_ms: Option<i64>,
    /// `allow_once`, `allow_hour`, `allow_always`, `reject` or `cancelled`.
    pub decision: String,
}

pub fn record_approval(row: &ApprovalRow) {
    if row.thread_id.trim().is_empty() || rustcode_session::is_scratch_session_id(&row.thread_id) {
        return;
    }
    let Some(path) = data_path() else { return };
    if let Err(error) = record_approval_at(&path, row) {
        crate::dbg_log!(
            "data store: approval of {} not recorded: {error}",
            row.thread_id
        );
    }
}

pub fn record_approval_at(path: &Path, row: &ApprovalRow) -> Result<(), String> {
    let connection = open(path)?;
    connection
        .execute(
            "INSERT INTO approvals (thread_id, source, tool_name, requested_at, decided_at,
                    wait_ms, decision)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                row.thread_id,
                row.source,
                row.tool_name,
                row.requested_at,
                row.decided_at,
                row.wait_ms,
                row.decision
            ],
        )
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// This machine's name, kept on each thread and turn so data from several
/// machines can be told apart. Read once: it does not change under a run.
fn local_machine() -> Option<String> {
    static NAME: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    NAME.get_or_init(|| {
        sysinfo::System::host_name()
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty())
    })
    .clone()
}

/// `turn` with its diff columns filled from the thread's checkout when the
/// caller left them empty. A folder that is not a git checkout, or a git that
/// does not answer, leaves them NULL rather than 0.
fn with_checkout_diff(turn: &TurnRow) -> TurnRow {
    let mut turn = turn.clone();
    if turn.diff_files.is_some() {
        return turn;
    }
    let cwd = crate::harness::store::load(&turn.thread_id)
        .map(|record| record.cwd)
        .or_else(|| {
            crate::sessions::load_session_workspace(&turn.thread_id).map(|workspace| workspace.cwd)
        });
    if let Some((files, added, removed)) = cwd.as_deref().and_then(checkout_diff) {
        turn.diff_files = Some(files);
        turn.diff_added = Some(added);
        turn.diff_removed = Some(removed);
    }
    turn
}

/// Files, added lines and removed lines of the uncommitted changes in `cwd`.
fn checkout_diff(cwd: &Path) -> Option<(i64, i64, i64)> {
    if !cwd.is_dir() {
        return None;
    }
    let output = rustcode_command::hidden_command("git")
        .args(["diff", "--numstat", "HEAD"])
        .current_dir(cwd)
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    Some(parse_numstat(&String::from_utf8_lossy(&output.stdout)))
}

/// Sums `git diff --numstat` lines; a binary file (`-\t-\t...`) counts as a
/// changed file with no lines.
fn parse_numstat(text: &str) -> (i64, i64, i64) {
    let (mut files, mut added, mut removed) = (0, 0, 0);
    for line in text.lines() {
        let mut parts = line.splitn(3, '\t');
        let (Some(add), Some(remove), Some(_)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        files += 1;
        added += add.parse::<i64>().unwrap_or(0);
        removed += remove.parse::<i64>().unwrap_or(0);
    }
    (files, added, removed)
}

/// Why a turn failed, as a short kind from its error text. The text itself
/// is never stored: it can hold paths, prompts or tokens.
pub fn turn_failure_kind(error: &str) -> String {
    let text = error.to_ascii_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|needle| text.contains(needle));
    // An HTTP status only as a status, never as digits inside an id.
    let status = |codes: &[u16]| {
        codes.iter().any(|code| {
            ["http ", "status ", "status code ", "status: ", "("]
                .iter()
                .any(|prefix| text.contains(&format!("{prefix}{code}")))
        })
    };
    let kind = if has(&[
        "unknown acp session",
        "session not found",
        "no such session",
    ]) {
        "unknown_session"
    } else if status(&[429]) || has(&["rate limit", "rate_limit", "too many requests"]) {
        "rate_limited"
    } else if has(&[
        "usage limit",
        "quota",
        "credit balance",
        "insufficient_quota",
    ]) {
        "usage_limit"
    } else if status(&[401, 403])
        || has(&[
            "unauthorized",
            "forbidden",
            "not logged in",
            "authentication",
            "token request failed",
            "invalid api key",
        ])
    {
        "auth"
    } else if has(&[
        "context window",
        "context length",
        "prompt is too long",
        "too many tokens",
    ]) {
        "context_overflow"
    } else if status(&[500, 502, 503, 504, 529])
        || has(&["overloaded", "internal server error", "server error"])
    {
        "provider_error"
    } else if has(&["timed out", "timeout"]) {
        "timeout"
    } else if has(&[
        "exited",
        "broken pipe",
        "connection closed",
        "process ended",
        "killed",
        "signal",
    ]) {
        "harness_exited"
    } else if has(&[
        "failed to start",
        "spawn",
        "not found in path",
        "no such file",
    ]) {
        "harness_start"
    } else if has(&[
        "tls",
        "certificate",
        "unknownissuer",
        "dns",
        "connection refused",
        "network",
    ]) {
        "network"
    } else {
        "other"
    };
    kind.to_owned()
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
    let dir = crate::sessions::get_active_session_dir(id);
    let cwd = record
        .as_ref()
        .map(|record| record.cwd.display().to_string())
        .or_else(|| {
            crate::sessions::load_session_workspace(id)
                .map(|workspace| workspace.cwd.display().to_string())
        });
    let metadata = crate::sessions::load_session_metadata(id);
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
            .or_else(|| crate::sessions::load_session_title(id)),
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
             SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 0, 'backfill'
             WHERE NOT EXISTS (SELECT 1 FROM turns WHERE thread_id = ?1 AND status != 'backfill')
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
/// deleted. Streaming-delta and status-ping payloads are dropped on every
/// pass (they are text the thread already holds). Everything else is kept
/// under the default `telemetry_retention = "forever"`. Only when
/// `telemetry_retention` names a number of days do payloads older than
/// [`PAYLOAD_RETENTION_DAYS`] go, raw `provider_requests` and
/// `harness_rpc_messages` rows older than that number of days, and the raw
/// tables are held under [`TELEMETRY_MAX_BYTES`] by dropping payloads before
/// rows. Command receipts
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
    // Age and size limits are opt-in: with `telemetry_retention = "forever"`
    // (the default) no stored payload is cleared or deleted because it is old
    // or because the file is large.
    if let Some(days) = row_retention_days {
        report.payloads_cleared += clear_payloads_before(connection, &payload_cutoff)?;
        let row_cutoff = rfc3339_days_ago(now, days);
        report.rows_deleted += delete_rows_before(connection, &row_cutoff)?;
    }
    report.rows_deleted += prune_receipts(
        connection,
        COMMAND_RECEIPT_RETENTION_DAYS,
        COMMAND_RECEIPT_MAX_ROWS,
    )?;
    report.rows_deleted += prune_outside_calls(connection)?;
    if row_retention_days.is_some() {
        trim_to_budget(connection, max_bytes, &mut report)?;
    }
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

/// What `rc usage --pull-requests` prints (#3907): per harness and
/// model, the pull requests its threads linked, how many merged, their CI
/// failures and fix rounds, and the cost and tokens per merged one, as the
/// `pr_outcomes_by_model` view has them.
pub const PULL_REQUEST_OUTCOMES_SQL: &str = "SELECT * FROM pr_outcomes_by_model \
     ORDER BY merged DESC, pull_requests DESC, harness, model";

pub fn pull_request_outcomes() -> Result<QueryResult, String> {
    query(PULL_REQUEST_OUTCOMES_SQL, 1000)
}

/// Per-table row counts and sizes, so growth itself can be tracked
/// (`rc data stats`). Table and view names come from `sqlite_master`;
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

/// The named account of `harness` the user's turns ran on most recently: what
/// a scheduled job with no account of its own runs on (issue: every run of a
/// job saved before it had an account started on the signed-out default login).
pub fn last_account(harness: &str) -> Option<String> {
    last_account_at(&data_path()?, harness).ok().flatten()
}

pub(crate) fn last_account_at(path: &Path, harness: &str) -> Result<Option<String>, String> {
    if !path.is_file() {
        return Ok(None);
    }
    let connection = open_readonly(path)?;
    use rusqlite::OptionalExtension;
    connection
        .query_row(
            "SELECT account FROM turns WHERE harness = ?1 AND account IS NOT NULL AND account <> '' ORDER BY started_at DESC, id DESC LIMIT 1",
            [harness],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(query_error)
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
#[path = "data_store_tests.rs"]
mod tests;
