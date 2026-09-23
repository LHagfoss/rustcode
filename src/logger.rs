use serde_json::Value;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

/// Keep each active debug log bounded. The current and previous files are
/// retained (`debug.log` and `debug.log.1`).
const MAX_DEBUG_LOG_BYTES: u64 = 50 * 1024 * 1024;
const OVERSIZED_LINE_MARKER: &str = "[logger] dropped oversized line";
const ROTATION_FAILURE_MARKER: &str = "[logger] prior log truncated because rotation failed";
const DEBUG_LOG_LOCK_NAME: &str = "debug.log.lock";

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
    let _guard = log_write_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if let Some(config_dir) = crate::config::get_config_dir() {
        let Some(_file_lock) = acquire_log_file_lock(&config_dir) else {
            return;
        };
        rotate_log_dir_if_oversized(&config_dir, MAX_DEBUG_LOG_BYTES);
        if let Some(session_id) = active_session_id() {
            let session_dir =
                rustcode_session::SessionStore::new(&config_dir).session_dir(&session_id);
            rotate_log_dir_if_oversized(&session_dir.join("logs"), MAX_DEBUG_LOG_BYTES);
        }
    }
}

fn log_write_lock() -> &'static Mutex<()> {
    // The persistent sibling lock file serializes processes; this mutex also
    // serializes threads before they contend for that OS-level lock.
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn acquire_log_file_lock(config_dir: &Path) -> Option<std::fs::File> {
    use std::fs::OpenOptions;

    std::fs::create_dir_all(config_dir).ok()?;
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(config_dir.join(DEBUG_LOG_LOCK_NAME))
        .ok()?;
    lock.lock().ok()?;
    Some(lock)
}

fn rotate_log_dir_if_oversized(log_dir: &std::path::Path, limit_bytes: u64) {
    let log_path = log_dir.join("debug.log");
    let rotated_path = log_dir.join("debug.log.1");
    let _ = bound_log_file(&rotated_path, limit_bytes);
    let Ok(meta) = std::fs::metadata(&log_path) else {
        return;
    };
    if meta.len() > limit_bytes {
        if bound_log_file(&log_path, limit_bytes).is_ok() {
            let _ = replace_rotated_log(&log_path, &rotated_path);
        }
    }
}

fn bound_log_file(path: &Path, limit_bytes: u64) -> std::io::Result<()> {
    use std::io::{Read, Seek, SeekFrom, Write};

    let Ok(metadata) = std::fs::metadata(path) else {
        return Ok(());
    };
    if metadata.len() <= limit_bytes {
        return Ok(());
    }

    let marker = format!("[logger] earlier log bytes discarded to fit {limit_bytes}-byte cap\n");
    let marker_bytes = marker
        .len()
        .min(usize::try_from(limit_bytes).unwrap_or(usize::MAX));
    let tail_bytes = limit_bytes.saturating_sub(marker_bytes as u64);
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?;
    let mut tail = Vec::with_capacity(tail_bytes as usize);
    if tail_bytes > 0 {
        file.seek(SeekFrom::End(-(tail_bytes as i64)))?;
        file.read_to_end(&mut tail)?;
        if let Some(line_end) = tail.iter().position(|byte| *byte == b'\n')
            && line_end + 1 < tail.len()
        {
            tail.drain(..=line_end);
        }
    }

    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&marker.as_bytes()[..marker_bytes])?;
    file.write_all(&tail)?;
    file.flush()
}

fn replace_rotated_log(log_path: &Path, rotated_path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(rotated_path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    std::fs::rename(log_path, rotated_path)
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
    if limit_bytes == 0 {
        return;
    }

    let _guard = log_write_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let Some(_file_lock) = acquire_log_file_lock(config_dir) else {
        return;
    };
    let line = capped_log_line(line, limit_bytes);
    let append_bytes = line.len().saturating_add(1) as u64;
    append_bounded_line(config_dir, &line, append_bytes, limit_bytes);

    if let Some(session_id) = session_id {
        let session_dir = rustcode_session::SessionStore::new(config_dir).session_dir(session_id);
        let logs_dir = session_dir.join("logs");
        append_bounded_line(&logs_dir, &line, append_bytes, limit_bytes);
    }
}

fn capped_log_line<'a>(line: &'a str, limit_bytes: u64) -> std::borrow::Cow<'a, str> {
    let max_content_bytes = usize::try_from(limit_bytes.saturating_sub(1)).unwrap_or(usize::MAX);
    if line.len() <= max_content_bytes {
        return std::borrow::Cow::Borrowed(line);
    }

    let marker = format!(
        "{OVERSIZED_LINE_MARKER} ({} bytes)",
        line.len().saturating_add(1)
    );
    if marker.len() <= max_content_bytes {
        return std::borrow::Cow::Owned(marker);
    }

    // The marker is ASCII, so slicing at any byte boundary stays valid UTF-8.
    std::borrow::Cow::Owned(marker[..max_content_bytes].to_owned())
}

fn append_bounded_line(log_dir: &Path, line: &str, append_bytes: u64, limit_bytes: u64) {
    let log_path = log_dir.join("debug.log");
    let rotated_path = log_dir.join("debug.log.1");
    let _ = bound_log_file(&rotated_path, limit_bytes);
    let current_bytes = std::fs::metadata(&log_path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    if current_bytes.saturating_add(append_bytes) > limit_bytes {
        if current_bytes > limit_bytes && bound_log_file(&log_path, limit_bytes).is_err() {
            let _ = truncate_and_append_bounded_line(&log_path, line, append_bytes, limit_bytes);
            return;
        }
        if replace_rotated_log(&log_path, &rotated_path).is_err() {
            // Rotation can fail when the archive path is unavailable. Preserve
            // the new diagnostic by truncating the active log and writing a
            // marker plus the line when both fit; otherwise keep just the line.
            let _ = truncate_and_append_bounded_line(&log_path, line, append_bytes, limit_bytes);
            return;
        }
    }
    append_line_to_path(&log_path, line);
}

fn truncate_and_append_bounded_line(
    path: &Path,
    line: &str,
    append_bytes: u64,
    limit_bytes: u64,
) -> std::io::Result<()> {
    use std::io::Write;

    let marker_bytes = ROTATION_FAILURE_MARKER.len().saturating_add(1) as u64;
    let include_marker = marker_bytes.saturating_add(append_bytes) <= limit_bytes;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)?;
    if include_marker {
        writeln!(file, "{ROTATION_FAILURE_MARKER}")?;
    }
    writeln!(file, "{line}")?;
    Ok(())
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

#[macro_export]
macro_rules! dbg_log_for_session {
    ($session_id:expr, $($arg:tt)*) => {{
        $crate::logger::append_line_for_session(&$session_id, &format!($($arg)*));
    }};
}

pub(crate) fn append_line_for_session(session_id: &str, line: &str) {
    append_line_with_session(line, Some(session_id));
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
        assert!(
            std::fs::metadata(dir.path().join("debug.log.1"))
                .expect("rotated metadata")
                .len()
                <= 100,
            "oversized legacy logs must not leave an oversized archive"
        );
    }

    #[test]
    fn startup_bounds_an_oversized_existing_archive() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log_path = dir.path().join("debug.log");
        let archive_path = dir.path().join("debug.log.1");
        std::fs::write(&log_path, "small active log\n").expect("write active log");
        std::fs::write(
            &archive_path,
            format!("{}latest archived line\n", "x".repeat(150)),
        )
        .expect("write oversized archive");

        rotate_log_dir_if_oversized(dir.path(), 100);

        let archive = std::fs::read(&archive_path).expect("bounded archive");
        assert!(archive.len() <= 100);
        assert!(archive.ends_with(b"latest archived line\n"));
        assert!(log_path.exists(), "small active log should remain in place");
    }

    #[test]
    fn archive_cap_smaller_than_marker_remains_strict() {
        let dir = tempfile::tempdir().expect("tempdir");
        let archive_path = dir.path().join("debug.log.1");
        std::fs::write(&archive_path, vec![b'x'; 100]).expect("write archive");

        bound_log_file(&archive_path, 8).expect("bound tiny archive");

        assert_eq!(std::fs::metadata(archive_path).expect("metadata").len(), 8);
    }

    #[test]
    fn rotation_replaces_an_existing_archive_on_all_platforms() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log_path = dir.path().join("debug.log");
        let archive_path = dir.path().join("debug.log.1");
        std::fs::write(&log_path, format!("{}new active tail\n", "n".repeat(110)))
            .expect("write oversized active log");
        std::fs::write(&archive_path, "old archive segment\n").expect("write old archive");

        rotate_log_dir_if_oversized(dir.path(), 100);

        let archive = std::fs::read(&archive_path).expect("new archive segment");
        assert!(archive.len() <= 100);
        assert!(archive.ends_with(b"new active tail\n"));
        assert!(!log_path.exists());
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
    fn appends_at_exact_cap_boundary_for_global_and_session_logs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let session_id = "2026-09-11T00:00:00Z-test-session";
        let session_dir = rustcode_session::SessionStore::new(dir.path()).session_dir(session_id);
        let session_logs = session_dir.join("logs");
        std::fs::create_dir_all(&session_logs).expect("create session log directory");
        std::fs::write(dir.path().join("debug.log"), b"old\n").expect("write global log");
        std::fs::write(session_logs.join("debug.log"), b"old\n").expect("write session log");

        append_line_to_logs_with_limit(dir.path(), Some(session_id), "new", 8);

        assert_eq!(
            std::fs::read_to_string(dir.path().join("debug.log")).expect("current global log"),
            "old\nnew\n"
        );
        assert_eq!(
            std::fs::read_to_string(session_logs.join("debug.log")).expect("current session log"),
            "old\nnew\n"
        );
        assert_eq!(
            std::fs::metadata(dir.path().join("debug.log"))
                .unwrap()
                .len(),
            8
        );
        assert_eq!(
            std::fs::metadata(session_logs.join("debug.log"))
                .unwrap()
                .len(),
            8
        );
        assert!(!dir.path().join("debug.log.1").exists());
        assert!(!session_logs.join("debug.log.1").exists());
    }

    #[test]
    fn rotates_before_an_append_that_would_cross_cap_for_both_logs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let session_id = "2026-09-11T00:00:00Z-test-session";
        let session_dir = rustcode_session::SessionStore::new(dir.path()).session_dir(session_id);
        let session_logs = session_dir.join("logs");
        std::fs::create_dir_all(&session_logs).expect("create session log directory");
        std::fs::write(dir.path().join("debug.log"), b"old!\n").expect("write global log");
        std::fs::write(session_logs.join("debug.log"), b"old!\n").expect("write session log");

        append_line_to_logs_with_limit(dir.path(), Some(session_id), "new", 8);

        assert_eq!(
            std::fs::read_to_string(dir.path().join("debug.log.1")).expect("rotated global log"),
            "old!\n"
        );
        assert_eq!(
            std::fs::read_to_string(session_logs.join("debug.log.1")).expect("rotated session log"),
            "old!\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("debug.log")).unwrap(),
            "new\n"
        );
        assert_eq!(
            std::fs::read_to_string(session_logs.join("debug.log")).unwrap(),
            "new\n"
        );
        assert_eq!(
            std::fs::metadata(dir.path().join("debug.log"))
                .unwrap()
                .len(),
            4
        );
        assert_eq!(
            std::fs::metadata(session_logs.join("debug.log"))
                .unwrap()
                .len(),
            4
        );
    }

    #[test]
    fn replaces_oversized_line_with_marker_that_fits_both_log_caps() {
        let dir = tempfile::tempdir().expect("tempdir");
        let session_id = "2026-09-11T00:00:00Z-test-session";
        let oversized = "x".repeat(300);

        append_line_to_logs_with_limit(dir.path(), Some(session_id), &oversized, 64);

        let global = std::fs::read_to_string(dir.path().join("debug.log")).expect("global log");
        let session_path = rustcode_session::SessionStore::new(dir.path())
            .session_dir(session_id)
            .join("logs/debug.log");
        let session = std::fs::read_to_string(session_path).expect("session log");
        assert!(global.starts_with(OVERSIZED_LINE_MARKER));
        assert!(session.starts_with(OVERSIZED_LINE_MARKER));
        assert!(!global.contains(&oversized));
        assert!(!session.contains(&oversized));
        assert!(global.len() <= 64);
        assert!(session.len() <= 64);
    }

    #[test]
    fn keeps_new_line_when_rotation_fails_for_both_log_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let session_id = "2026-09-11T00:00:00Z-test-session";
        let session_dir = rustcode_session::SessionStore::new(dir.path()).session_dir(session_id);
        let session_logs = session_dir.join("logs");
        std::fs::create_dir_all(&session_logs).expect("create session log directory");
        let global_archive = dir.path().join("debug.log.1");
        let session_archive = session_logs.join("debug.log.1");
        std::fs::create_dir(&global_archive).expect("block global archive path");
        std::fs::create_dir(&session_archive).expect("block session archive path");
        std::fs::write(dir.path().join("debug.log"), vec![b'g'; 75]).expect("write global log");
        std::fs::write(session_logs.join("debug.log"), vec![b's'; 75]).expect("write session log");

        append_line_to_logs_with_limit(dir.path(), Some(session_id), "latest", 80);

        let global = std::fs::read_to_string(dir.path().join("debug.log")).expect("global log");
        let session = std::fs::read_to_string(session_logs.join("debug.log")).expect("session log");
        assert!(global.starts_with(ROTATION_FAILURE_MARKER));
        assert!(global.ends_with("latest\n"));
        assert!(session.starts_with(ROTATION_FAILURE_MARKER));
        assert!(session.ends_with("latest\n"));
        assert!(global.len() <= 80);
        assert!(session.len() <= 80);
        assert!(global_archive.is_dir());
        assert!(session_archive.is_dir());
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
