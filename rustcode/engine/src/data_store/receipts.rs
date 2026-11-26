//! Command receipts and the outside-agent call audit.

use super::*;

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
    write_receipt(&connection, receipt).map(|_| ())
}

/// [`record_receipt`] against the store this machine uses.
pub fn record_receipt(receipt: &ReceiptRow) -> Result<(), String> {
    let path = data_path().ok_or_else(|| "no config directory".to_owned())?;
    record_receipt_at(&path, receipt)
}

/// Claim a retry key before creating a thread. Only the insert winner may start it.
pub(crate) fn claim_start_receipt(receipt: &ReceiptRow) -> Result<bool, String> {
    let path = data_path().ok_or_else(|| "no config directory".to_owned())?;
    let connection = open(&path)?;
    write_receipt(&connection, receipt)
}

/// Advance a claimed start from unknown outcome to known thread, then to an
/// accepted start. Match the original result as well as the key and hash.
pub(crate) fn advance_start_receipt(
    receipt: &mut ReceiptRow,
    result: &Value,
) -> Result<(), String> {
    let path = data_path().ok_or_else(|| "no config directory".to_owned())?;
    advance_start_receipt_at(&path, receipt, result)
}

pub(super) fn advance_start_receipt_at(
    path: &Path,
    receipt: &mut ReceiptRow,
    result: &Value,
) -> Result<(), String> {
    let result_json = result.to_string();
    let updated = open(path)?
        .execute(
            "UPDATE command_receipts SET result_json = ?6
             WHERE caller = ?1 AND client_request_id = ?2 AND action = ?3
               AND args_hash = ?4 AND result_json = ?5",
            params![
                receipt.caller,
                receipt.client_request_id,
                receipt.action,
                receipt.args_hash,
                receipt.result_json,
                result_json,
            ],
        )
        .map_err(|error| error.to_string())?;
    if updated != 1 {
        return Err("the start receipt changed or expired".to_owned());
    }
    receipt.result_json = result_json;
    Ok(())
}

pub(super) fn write_receipt(connection: &Connection, receipt: &ReceiptRow) -> Result<bool, String> {
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
        .map(|inserted| inserted == 1)
        .map_err(|error| error.to_string())
}

/// Drops receipts past the retry window, then the oldest until at most
/// `max_rows` are left. Called by the telemetry prune pass.
pub(super) fn prune_receipts(
    connection: &Connection,
    days: i64,
    max_rows: i64,
) -> Result<usize, String> {
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

/// How long the audit of outside-agent calls is kept.
pub const OUTSIDE_AGENT_CALL_RETENTION_DAYS: i64 = 90;

/// The longest `detail` an audit row keeps: a reason, never an argument.
pub(super) const MAX_OUTSIDE_DETAIL_CHARS: usize = 200;

/// One tool call an outside agent made (`docs/mcp-outside-agents.md`). Only
/// who, what and how it ended: arguments and answers are never stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutsideCallRow {
    pub client_id: String,
    pub client_name: String,
    pub platform: Option<String>,
    pub scope: String,
    pub tool: String,
    /// `ok`, `error`, `denied` (the grant's scope does not allow it),
    /// `declined` (the user said no in the app) or `unavailable`.
    pub outcome: String,
    pub duration_ms: Option<i64>,
    pub detail: Option<String>,
}

/// [`record_outside_call`] against one store file, so a test can use a temporary one.
pub fn record_outside_call_at(path: &Path, row: &OutsideCallRow) -> Result<(), String> {
    let connection = open(path)?;
    if row.client_id.trim().is_empty() || row.tool.trim().is_empty() {
        return Err("an outside-agent call needs a client and a tool".to_owned());
    }
    let at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let detail: Option<String> = row
        .detail
        .as_ref()
        .map(|detail| detail.chars().take(MAX_OUTSIDE_DETAIL_CHARS).collect());
    connection
        .execute(
            "INSERT INTO outside_agent_calls
                (at, client_id, client_name, platform, scope, tool, outcome, duration_ms, detail)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                at,
                row.client_id,
                row.client_name,
                row.platform,
                row.scope,
                row.tool,
                row.outcome,
                row.duration_ms,
                detail,
            ],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// Records an outside agent's call in the store this machine uses. A failure
/// is logged: the call already happened, and the audit must not change its answer.
pub fn record_outside_call(row: &OutsideCallRow) {
    let Some(path) = data_path() else {
        return;
    };
    if let Err(error) = record_outside_call_at(&path, row) {
        crate::dbg_log!("data store: outside-agent call not recorded: {error}");
    }
}

/// The newest outside-agent calls, newest first, optionally of one client.
pub fn recent_outside_calls_at(
    path: &Path,
    client_id: Option<&str>,
    limit: u32,
) -> Result<Vec<Value>, String> {
    let connection = open(path)?;
    let mut statement = connection
        .prepare(
            "SELECT at, client_id, client_name, scope, tool, outcome, duration_ms, detail
             FROM outside_agent_calls
             WHERE (?1 IS NULL OR client_id = ?1)
             ORDER BY id DESC LIMIT ?2",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(params![client_id, limit], |row| {
            Ok(json!({
                "at": row.get::<_, String>(0)?,
                "clientId": row.get::<_, String>(1)?,
                "clientName": row.get::<_, String>(2)?,
                "scope": row.get::<_, String>(3)?,
                "tool": row.get::<_, String>(4)?,
                "outcome": row.get::<_, String>(5)?,
                "durationMs": row.get::<_, Option<i64>>(6)?,
                "detail": row.get::<_, Option<String>>(7)?,
            }))
        })
        .map_err(|error| error.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}

/// [`recent_outside_calls_at`] against the store this machine uses.
pub fn recent_outside_calls(client_id: Option<&str>, limit: u32) -> Vec<Value> {
    data_path()
        .and_then(|path| recent_outside_calls_at(&path, client_id, limit).ok())
        .unwrap_or_default()
}

/// Drops audit rows past [`OUTSIDE_AGENT_CALL_RETENTION_DAYS`].
pub(super) fn prune_outside_calls(connection: &Connection) -> Result<usize, String> {
    let cutoff = rfc3339_days_ago(chrono::Utc::now(), OUTSIDE_AGENT_CALL_RETENTION_DAYS);
    connection
        .execute("DELETE FROM outside_agent_calls WHERE at < ?1", [cutoff])
        .map_err(|error| error.to_string())
}
