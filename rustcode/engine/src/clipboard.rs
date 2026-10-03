pub fn paste_image_from_clipboard() -> Option<String> {
    let attachments_dir = crate::config::get_config_dir()?.join("attachments");
    let _ = std::fs::create_dir_all(&attachments_dir);

    let filename = format!("clip_{}.png", chrono::Local::now().format("%Y%m%d_%H%M%S"));
    let file_path = attachments_dir.join(&filename);
    let file_path_str = file_path.to_string_lossy().to_string();

    // The pasteboard usually already holds PNG data (screenshots do), which
    // is read in-process; launching `osascript` costs a visible pause.
    #[cfg(target_os = "macos")]
    if let Some(png) = pasteboard::png_data()
        && std::fs::write(&file_path, png).is_ok()
    {
        return Some(format!("![image](file://{})", file_path_str));
    }

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

#[cfg(target_os = "macos")]
mod pasteboard {
    use std::ffi::{CStr, c_char, c_void};

    type Id = *mut c_void;

    #[link(name = "AppKit", kind = "framework")]
    unsafe extern "C" {}

    #[link(name = "objc")]
    unsafe extern "C" {
        fn objc_getClass(name: *const c_char) -> Id;
        fn sel_registerName(name: *const c_char) -> Id;
        fn objc_msgSend();
        fn objc_autoreleasePoolPush() -> *mut c_void;
        fn objc_autoreleasePoolPop(pool: *mut c_void);
    }

    /// PNG bytes on the general pasteboard, or `None` when it holds no PNG.
    pub(super) fn png_data() -> Option<Vec<u8>> {
        // SAFETY: every message goes to a non-null receiver with the selector
        // and signature AppKit documents for it, and the returned bytes are
        // copied before the autorelease pool that owns them is drained.
        unsafe {
            let send0: unsafe extern "C" fn(Id, Id) -> Id =
                std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
            let send1: unsafe extern "C" fn(Id, Id, Id) -> Id =
                std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
            let send_cstr: unsafe extern "C" fn(Id, Id, *const c_char) -> Id =
                std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
            let send_len: unsafe extern "C" fn(Id, Id) -> usize =
                std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
            let sel = |name: &CStr| sel_registerName(name.as_ptr());

            let pool = objc_autoreleasePoolPush();
            let png = (|| {
                let pasteboard_class = objc_getClass(c"NSPasteboard".as_ptr());
                let string_class = objc_getClass(c"NSString".as_ptr());
                if pasteboard_class.is_null() || string_class.is_null() {
                    return None;
                }
                let pasteboard = send0(pasteboard_class, sel(c"generalPasteboard"));
                let kind = send_cstr(
                    string_class,
                    sel(c"stringWithUTF8String:"),
                    c"public.png".as_ptr(),
                );
                if pasteboard.is_null() || kind.is_null() {
                    return None;
                }
                let data = send1(pasteboard, sel(c"dataForType:"), kind);
                if data.is_null() {
                    return None;
                }
                let bytes = send0(data, sel(c"bytes")) as *const u8;
                let len = send_len(data, sel(c"length"));
                if bytes.is_null() || len == 0 {
                    return None;
                }
                Some(std::slice::from_raw_parts(bytes, len).to_vec())
            })();
            objc_autoreleasePoolPop(pool);
            png
        }
    }
}

/// Load the pasteboard framework off the UI path. The first in-process read
/// pays a one-time ~200ms framework start-up; doing it at launch keeps the
/// first image paste as quick as the later ones.
pub fn warm_image_paste() {
    #[cfg(target_os = "macos")]
    std::thread::spawn(|| {
        let _ = pasteboard::png_data();
    });
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

    /// Reads the real pasteboard, so it only runs on request:
    /// `cargo test native_pasteboard -- --ignored --nocapture`.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore]
    fn native_pasteboard_png_read_returns_png_bytes_or_none() {
        let started = std::time::Instant::now();
        let png = super::pasteboard::png_data();
        println!(
            "pasteboard png: {:?} bytes in {:?}",
            png.as_ref().map(Vec::len),
            started.elapsed()
        );
        if let Some(png) = png {
            assert!(png.starts_with(b"\x89PNG"));
        }
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
