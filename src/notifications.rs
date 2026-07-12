use std::io::{self, IsTerminal, Write};

/// Sends desktop notifications via terminal escape sequences (OSC 777).
///
/// Works in modern terminal emulators like Ghostty, WezTerm, etc.,
/// which support the OSC 777 notification sequence.
pub fn notify(title: &str, body: &str) -> std::io::Result<()> {
    if io::stdout().is_terminal() {
        let clean_title = title.replace(|c| c == ';' || c == '\n' || c == '\r', " ");
        let clean_body = body.replace(|c| c == ';' || c == '\n' || c == '\r', " ");
        let mut stdout = io::stdout();
        write!(
            stdout,
            "\x1b]777;notify;{};{}\x1b\\",
            clean_title, clean_body
        )?;
        stdout.flush()?;
    }
    Ok(())
}

/// Convenience: notify that a tool needs user confirmation.
pub fn notify_pending_confirmation(details: &str) -> std::io::Result<()> {
    notify("rustcode", details)
}

#[derive(Debug)]
pub enum FinishedStatus {
    Success,
    Cancelled,
    Denied,
    Error(String),
}

/// Convenience: notify that the harness finished (success or error).
pub fn notify_finished(status: FinishedStatus) -> std::io::Result<()> {
    match status {
        FinishedStatus::Success => {
            notify("rustcode", "Task complete.")?;
        }
        FinishedStatus::Cancelled | FinishedStatus::Denied => {
            notify("rustcode", "Operation cancelled or denied.")?;
        }
        FinishedStatus::Error(msg) => {
            notify("rustcode", &msg)?;
        }
    }
    Ok(())
}
