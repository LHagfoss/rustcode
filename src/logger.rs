use serde_json::Value;

pub(crate) fn append_line(line: &str) {
    use std::io::Write;
    if let Some(log_dir) = crate::config::get_config_dir() {
        let log_path = log_dir.join("debug.log");
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
        {
            let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
            let _ = writeln!(f, "[{now}] {line}");
        }
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
