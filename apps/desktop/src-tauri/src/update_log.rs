//! A small record of what the updater and the system setup did, in
//! `updates.log` next to the engine's `debug.log`: one line per event, so a
//! failed update leaves evidence a person (or an agent reading a bug report)
//! can read without a debugger. It holds versions, step names and outcomes,
//! never file contents or tokens. Best effort: a log that cannot be written
//! never fails the update.

use std::io::Write;
use std::path::Path;

/// The file stops growing here: older lines go once it is longer.
const MAX_LINES: usize = 400;

/// Appends `event detail` with a UTC time. `event` is a dotted name such as
/// `update.failed`; `detail` is one line (newlines are flattened).
pub fn record(event: &str, detail: &str) {
    let Some(dir) = crate::diagnostics::config_dir() else {
        return;
    };
    let _ = append(
        &dir.join("updates.log"),
        &line(chrono::Utc::now().to_rfc3339(), event, detail),
    );
}

/// One log line, `time event detail`, on one physical line.
fn line(time: String, event: &str, detail: &str) -> String {
    let detail: String = detail
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    format!("{time} {event} {}", detail.trim())
        .trim_end()
        .to_owned()
}

fn append(path: &Path, line: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let kept = std::fs::read_to_string(path)
        .ok()
        .filter(|text| text.lines().count() >= MAX_LINES)
        .map(|text| trimmed(&text, MAX_LINES - 1));
    match kept {
        Some(text) => std::fs::write(path, format!("{text}{line}\n")),
        None => {
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)?;
            writeln!(file, "{line}")
        }
    }
}

/// The last `keep` lines of `text`, each ending in a newline.
fn trimmed(text: &str, keep: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(keep);
    lines[start..].iter().map(|l| format!("{l}\n")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_is_one_physical_line() {
        let text = line(
            "2026-11-13T10:00:00Z".into(),
            "update.failed",
            "bad\nthing\r\n",
        );
        assert_eq!(text, "2026-11-13T10:00:00Z update.failed bad thing");
        assert!(!text.contains('\n'));
        assert_eq!(line("t".into(), "update.started", ""), "t update.started");
    }

    #[test]
    fn the_log_keeps_only_the_newest_lines() {
        let text: String = (0..10).map(|n| format!("line {n}\n")).collect();
        assert_eq!(trimmed(&text, 3), "line 7\nline 8\nline 9\n");
        assert_eq!(trimmed("a\nb\n", 5), "a\nb\n");
    }

    #[test]
    fn appending_past_the_cap_drops_the_oldest() {
        let dir = std::env::temp_dir().join(format!("rustcode-update-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("updates.log");
        for n in 0..MAX_LINES + 5 {
            append(&path, &format!("entry {n}")).unwrap();
        }
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), MAX_LINES);
        assert!(text.lines().next().unwrap().starts_with("entry "));
        assert_eq!(
            text.lines().last().unwrap(),
            format!("entry {}", MAX_LINES + 4)
        );
        assert!(!text.contains("entry 0\n"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
