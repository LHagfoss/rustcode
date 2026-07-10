/// macOS push notifications via alerter_rs (native macOS User Notifications).
///
/// This sends actual macOS Notification Center alerts, not terminal escape
/// sequences. Works whether the user runs rustcode in Ghostty, Terminal.app,
/// or anywhere else.
pub fn notify(title: &str, body: &str) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use alerter_rs::Alerter;
        if let Ok(handle) = Alerter::new(body).title(title).send_async() {
            handle.detach();
        }
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
