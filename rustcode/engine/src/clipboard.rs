pub fn paste_image_from_clipboard() -> Option<String> {
    let attachments_dir = crate::config::get_config_dir()?.join("attachments");
    let _ = std::fs::create_dir_all(&attachments_dir);

    let filename = format!("clip_{}.png", chrono::Local::now().format("%Y%m%d_%H%M%S"));
    let file_path = attachments_dir.join(&filename);
    let file_path_str = file_path.to_string_lossy().to_string();

    let script = format!(
        "write (the clipboard as «class PNGf») to (open for access \"{}\" with write permission)",
        file_path_str
    );

    let output = std::process::Command::new("osascript")
        .args(["-e", &script])
        .output()
        .ok()?;

    if output.status.success() {
        Some(format!("![image](file://{})", file_path_str))
    } else {
        None
    }
}

pub fn read_text_from_clipboard() -> Option<String> {
    let output = std::process::Command::new("pbpaste")
        .env("LANG", "en_US.UTF-8")
        .env("LC_CTYPE", "en_US.UTF-8")
        .output()
        .ok()?;
    if output.status.success() {
        let text = std::str::from_utf8(&output.stdout).ok()?.to_string();
        if !text.is_empty() {
            return Some(text);
        }
    }
    None
}

fn clipboard_copy_log_summary(byte_count: usize) -> String {
    format!("[CLIPBOARD] Copying {byte_count} bytes to system clipboard")
}

/// A native clipboard write is confirmed; OSC 52 only confirms that the
/// terminal request was sent, since terminals do not acknowledge delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardCopyStatus {
    Confirmed,
    Requested,
    Failed,
}

pub fn copy_to_clipboard(text: &str) -> ClipboardCopyStatus {
    use std::io::Write;
    copy_to_clipboard_with(
        text,
        |clean| {
            use base64::Engine;
            let b64 = base64::engine::general_purpose::STANDARD.encode(clean.as_bytes());
            let osc52 = format!("\x1b]52;c;{b64}\x07");
            let mut stdout = std::io::stdout().lock();
            stdout.write_all(osc52.as_bytes()).is_ok() && stdout.flush().is_ok()
        },
        |clean| {
            let Ok(mut child) = std::process::Command::new("pbcopy")
                .env("LANG", "en_US.UTF-8")
                .env("LC_CTYPE", "en_US.UTF-8")
                .stdin(std::process::Stdio::piped())
                .spawn()
            else {
                dbg_log!("[CLIPBOARD] Failed to spawn pbcopy process");
                return false;
            };
            let wrote = child
                .stdin
                .take()
                .is_some_and(|mut stdin| stdin.write_all(clean.as_bytes()).is_ok());
            let status = child.wait();
            dbg_log!("[CLIPBOARD] pbcopy wait result: {:?}", status);
            wrote && status.is_ok_and(|status| status.success())
        },
    )
}

fn copy_to_clipboard_with(
    text: &str,
    send_terminal: impl FnOnce(&str) -> bool,
    write_native: impl FnOnce(&str) -> bool,
) -> ClipboardCopyStatus {
    let clean: String = text
        .chars()
        .filter(|&c| c != '\0' && c != '\u{feff}')
        .collect();
    if clean.trim().is_empty() {
        dbg_log!("[CLIPBOARD] Ignored copy request: text is empty or whitespace only");
        return ClipboardCopyStatus::Failed;
    }
    dbg_log!("{}", clipboard_copy_log_summary(clean.len()));
    let terminal_sent = send_terminal(&clean);
    let native_copied = write_native(&clean);
    match (native_copied, terminal_sent) {
        (true, _) => ClipboardCopyStatus::Confirmed,
        (false, true) => ClipboardCopyStatus::Requested,
        (false, false) => ClipboardCopyStatus::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::{ClipboardCopyStatus, clipboard_copy_log_summary, copy_to_clipboard_with};

    #[test]
    fn clipboard_status_distinguishes_native_terminal_and_failure() {
        let confirmed = copy_to_clipboard_with("text", |_| true, |_| true);
        let requested = copy_to_clipboard_with("text", |_| true, |_| false);
        let failed = copy_to_clipboard_with("text", |_| false, |_| false);
        assert_eq!(confirmed, ClipboardCopyStatus::Confirmed);
        assert_eq!(requested, ClipboardCopyStatus::Requested);
        assert_eq!(failed, ClipboardCopyStatus::Failed);
    }

    #[test]
    fn empty_copy_does_not_touch_either_backend() {
        let result = copy_to_clipboard_with(
            " \0\u{feff} ",
            |_| panic!("terminal backend must not be called"),
            |_| panic!("native backend must not be called"),
        );
        assert_eq!(result, ClipboardCopyStatus::Failed);
    }

    #[test]
    fn clipboard_copy_log_summary_contains_only_byte_count() {
        let clipboard_text = "secret-token-123";
        let summary = clipboard_copy_log_summary(clipboard_text.len());

        assert_eq!(
            summary,
            format!(
                "[CLIPBOARD] Copying {} bytes to system clipboard",
                clipboard_text.len()
            )
        );
        assert!(!summary.contains(clipboard_text));
    }
}
