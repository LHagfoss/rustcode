use serde_json::Value;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

/// Keep each active debug log bounded. The current and previous files are
/// retained (`debug.log` and `debug.log.1`).
const MAX_DEBUG_LOG_BYTES: u64 = 50 * 1024 * 1024;

pub(crate) fn set_active_session_id(session_id: Option<&str>) {
    let mut active = active_session_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    *active = session_id.map(str::to_owned).filter(|id| !id.is_empty());
}

fn active_session_id() -> Option<String> {
    active_session_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
}

fn active_session_lock() -> &'static Mutex<Option<String>> {
    static ACTIVE: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    ACTIVE.get_or_init(|| Mutex::new(None))
}

/// Rotate `debug.log` out of the way if it has grown past the size cap.
/// Also called before writes so a long-lived process cannot grow the global
/// or session log beyond the cap by accumulating lines between restarts.
pub(crate) fn rotate_if_oversized() {
    if let Some(config_dir) = crate::config::get_config_dir() {
        rotate_log_dir_if_oversized(&config_dir, MAX_DEBUG_LOG_BYTES);
        if let Some(session_id) = active_session_id() {
            let session_dir =
                rustcode_session::SessionStore::new(&config_dir).session_dir(&session_id);
            rotate_log_dir_if_oversized(&session_dir.join("logs"), MAX_DEBUG_LOG_BYTES);
        }
    }
}

fn rotate_log_dir_if_oversized(log_dir: &std::path::Path, limit_bytes: u64) {
    let log_path = log_dir.join("debug.log");
    let Ok(meta) = std::fs::metadata(&log_path) else {
        return;
    };
    if meta.len() > limit_bytes {
        let rotated_path = log_dir.join("debug.log.1");
        // Best-effort: if the rename fails (e.g. permissions), just keep
        // appending to the oversized file rather than losing log data.
        let _ = std::fs::rename(&log_path, &rotated_path);
    }
}

pub(crate) fn append_line(line: &str) {
    append_line_with_session(line, None);
}

fn append_line_with_session(line: &str, session_id: Option<&str>) {
    if let Some(log_dir) = crate::config::get_config_dir() {
        let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
        let formatted = format!("[{now}] {line}");
        let session_id = session_id.map(str::to_owned).or_else(active_session_id);
        append_line_to_logs(&log_dir, session_id.as_deref(), &formatted);
    }
}

/// Write every line to the bounded config-level log, and duplicate it under
/// its owning session when the logger has an explicit or active session id.
fn append_line_to_logs(config_dir: &Path, session_id: Option<&str>, line: &str) {
    append_line_to_logs_with_limit(config_dir, session_id, line, MAX_DEBUG_LOG_BYTES);
}

fn append_line_to_logs_with_limit(
    config_dir: &Path,
    session_id: Option<&str>,
    line: &str,
    limit_bytes: u64,
) {
    rotate_log_dir_if_oversized(config_dir, limit_bytes);
    append_line_to_path(&config_dir.join("debug.log"), line);

    if let Some(session_id) = session_id {
        let session_dir = rustcode_session::SessionStore::new(config_dir).session_dir(session_id);
        let logs_dir = session_dir.join("logs");
        rotate_log_dir_if_oversized(&logs_dir, limit_bytes);
        append_line_to_path(&logs_dir.join("debug.log"), line);
    }
}

fn attributed_fields(mut fields: Value) -> (Value, Option<String>) {
    let session_id = fields
        .get("session_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .or_else(active_session_id);
    if let Some(object) = fields.as_object_mut()
        && !object.contains_key("session_id")
        && let Some(session_id) = session_id.as_ref()
    {
        object.insert("session_id".to_owned(), Value::String(session_id.clone()));
    }
    (fields, session_id)
}

fn append_line_to_path(path: &Path, line: &str) {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{line}");
    }
}

/// Install a panic hook that preserves panic evidence in the global and, when
/// available, session-scoped logs.
/// Issue #1226: two sessions froze/died mid-stream with zero log evidence.
/// Unwind panics leave no macOS crash report, and a panic in a spawned task
/// is silently dropped unless its JoinHandle is observed — so without this
/// hook the next occurrence would be just as undiagnosable.
pub(crate) fn install_panic_hook() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let payload = info
                .payload()
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| info.payload().downcast_ref::<String>().map(String::as_str))
                .unwrap_or("<non-string panic payload>");
            let location = info
                .location()
                .map(|loc| format!("{}:{}:{}", loc.file(), loc.line(), loc.column()))
                .unwrap_or_else(|| "<unknown location>".to_owned());
            // Best-effort synchronous write: async logging may be gone, and
            // append_line is lock-poison-safe, so this cannot deadlock.
            append_line(&format!("[PANIC] {payload} at {location}"));
            previous(info);
        }));
    });
}

/// Write metadata-only lifecycle events to the global and owning session logs.
pub(crate) fn operational_event(event: &str, fields: Value) {
    // Keep every operational event attributable even when a call site is in a
    // low-level stream/parser helper that does not otherwise carry session
    // state. Explicit ownership wins so a stale task cannot be attributed to
    // whichever session happens to be active when it unwinds.
    let (fields, session_id) = attributed_fields(fields);
    let payload = serde_json::json!({"event": event, "fields": fields});
    append_line_with_session(&format!("[op] {payload}"), session_id.as_deref());
}

#[macro_export]
macro_rules! dbg_log {
    ($($arg:tt)*) => {{
        $crate::logger::append_line(&format!($($arg)*));
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_triggers_when_log_exceeds_the_cap() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log_path = dir.path().join("debug.log");
        std::fs::write(&log_path, vec![b'x'; 200]).expect("write oversized log");

        // Small threshold so the test doesn't need to write anywhere near
        // the real 50MB cap.
        rotate_log_dir_if_oversized(dir.path(), 100);

        assert!(
            !log_path.exists(),
            "oversized debug.log should have been rotated away"
        );
        assert!(
            dir.path().join("debug.log.1").exists(),
            "rotated log should be preserved as debug.log.1"
        );
    }

    #[test]
    fn rotation_leaves_small_logs_untouched() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log_path = dir.path().join("debug.log");
        std::fs::write(&log_path, vec![b'x'; 10]).expect("write small log");

        rotate_log_dir_if_oversized(dir.path(), 100);

        assert!(
            log_path.exists(),
            "a log under the size cap must not be rotated"
        );
        assert!(!dir.path().join("debug.log.1").exists());
    }

    #[test]
    fn rotation_is_a_no_op_when_no_log_exists_yet() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Must not panic or create anything when there's no debug.log yet
        // (e.g. the very first run of a fresh install).
        rotate_log_dir_if_oversized(dir.path(), 100);
        assert!(!dir.path().join("debug.log").exists());
        assert!(!dir.path().join("debug.log.1").exists());
    }

    #[test]
    fn writes_global_lines_when_no_session_is_active() {
        let dir = tempfile::tempdir().expect("tempdir");
        append_line_to_logs(dir.path(), None, "[time] early startup");
        let contents = std::fs::read_to_string(dir.path().join("debug.log")).expect("global log");
        assert!(contents.contains("early startup"));
        assert!(!dir.path().join("sessions").exists());
    }

    #[test]
    fn writes_session_lines_to_both_global_and_session_logs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let session_id = "2026-09-11T00:00:00Z-test-session";
        append_line_to_logs(
            dir.path(),
            Some(session_id),
            "[time] [op] {\"event\":\"turn.start\"}",
        );
        let global = std::fs::read_to_string(dir.path().join("debug.log")).expect("global log");
        let session_path = rustcode_session::SessionStore::new(dir.path())
            .session_dir(session_id)
            .join("logs/debug.log");
        let session = std::fs::read_to_string(session_path).expect("session log");
        assert!(global.contains("turn.start"));
        assert!(session.contains("turn.start"));
    }

    #[test]
    fn rotates_global_and_session_logs_before_appending() {
        let dir = tempfile::tempdir().expect("tempdir");
        let session_id = "2026-09-11T00:00:00Z-test-session";
        let session_dir = rustcode_session::SessionStore::new(dir.path()).session_dir(session_id);
        let session_logs = session_dir.join("logs");
        std::fs::create_dir_all(&session_logs).expect("create session log directory");
        std::fs::write(dir.path().join("debug.log"), b"oversized global log")
            .expect("write global log");
        std::fs::write(session_logs.join("debug.log"), b"oversized session log")
            .expect("write session log");

        append_line_to_logs_with_limit(dir.path(), Some(session_id), "new line", 8);

        assert_eq!(
            std::fs::read_to_string(dir.path().join("debug.log.1")).expect("rotated global log"),
            "oversized global log"
        );
        assert_eq!(
            std::fs::read_to_string(session_logs.join("debug.log.1")).expect("rotated session log"),
            "oversized session log"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("debug.log")).expect("current global log"),
            "new line\n"
        );
        assert_eq!(
            std::fs::read_to_string(session_logs.join("debug.log")).expect("current session log"),
            "new line\n"
        );
    }

    #[test]
    fn operational_fields_keep_explicit_session_ownership() {
        let (fields, session_id) = attributed_fields(serde_json::json!({
            "session_id": "older-session",
            "round": 2,
        }));
        assert_eq!(session_id.as_deref(), Some("older-session"));
        assert_eq!(fields["session_id"], "older-session");
    }
}
