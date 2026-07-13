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

pub fn copy_to_clipboard(text: &str) -> bool {
    use std::io::Write;
    if let Ok(mut child) = std::process::Command::new("pbcopy")
        .env("LANG", "en_US.UTF-8")
        .env("LC_CTYPE", "en_US.UTF-8")
        .stdin(std::process::Stdio::piped())
        .spawn()
    {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
        }
        child.wait().is_ok()
    } else {
        false
    }
}
