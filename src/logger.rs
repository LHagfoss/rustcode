use serde_json::Value;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

/// debug.log is append-only and never trimmed per write (that would make
/// every log line pay for a size check). Instead, cap it cheaply once per
/// process start: if it's grown past this, move it aside so the session
/// starts with a fresh file instead of growing the old one forever.
const MAX_DEBUG_LOG_BYTES: u64 = 50 * 1024 * 1024;

/// Rotate `debug.log` out of the way if it has grown past the size cap.
/// Call once at process start — not on every write, since the whole point is
/// to keep per-write logging cheap (a single `metadata()` stat per session,
/// not per line).
pub(crate) fn rotate_if_oversized() {
    if let Some(log_dir) = crate::config::get_config_dir() {
        rotate_log_dir_if_oversized(&log_dir, MAX_DEBUG_LOG_BYTES);
        if let Some(session_id) = active_session_id() {
            let session_dir =
                rustcode_session::SessionStore::new(&log_dir).session_dir(&session_id);
            rotate_log_dir_if_oversized(&session_dir.join("logs"), MAX_DEBUG_LOG_BYTES);
        }
    }
}

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
    if let Some(log_dir) = crate::config::get_config_dir() {
        let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
        let formatted = format!("[{now}] {line}");
        append_line_to_path(&log_dir.join("debug.log"), &formatted);

        if let Some(session_id) = active_session_id() {
            let session_dir =
                rustcode_session::SessionStore::new(&log_dir).session_dir(&session_id);
            let logs_dir = session_dir.join("logs");
            let _ = std::fs::create_dir_all(&logs_dir);
            rotate_log_dir_if_oversized(&logs_dir, MAX_DEBUG_LOG_BYTES);
            append_line_to_path(&logs_dir.join("debug.log"), &formatted);
        }
    }
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

/// Write metadata-only lifecycle events to the existing debug log.
pub(crate) fn operational_event(event: &str, fields: Value) {
    let payload = serde_json::json!({"event": event, "fields": fields});
    append_line(&format!("[op] {payload}"));
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
    fn appends_session_log_lines_without_changing_global_format() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("sessions/2026/09/11/abc/logs/debug.log");
        append_line_to_path(&path, "[time] [op] {\"event\":\"turn.start\"}");
        let contents = std::fs::read_to_string(path).expect("session log");
        assert!(contents.contains("turn.start"));
    }
}
