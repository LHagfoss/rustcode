#[macro_export]
macro_rules! dbg_log {
    ($($arg:tt)*) => {{
        use std::io::Write;
        if let Some(log_dir) = crate::config::get_config_dir() {
            let log_path = log_dir.join("debug.log");
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path)
            {
                let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
                let _ = writeln!(f, "[{now}] {}", format!($($arg)*));
            }
        }
    }};
}
