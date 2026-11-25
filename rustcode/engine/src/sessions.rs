//! The session store: each thread's saved history, settings and metadata on
//! disk, its export, and which process holds it open. Kept apart from
//! [`crate::config`], which is the user's settings, so reading settings does not
//! pull in the agent's history types.

pub(crate) mod activity;
mod export;
mod open;
mod store;
pub use export::{downloads_dir, export_session};
pub(crate) use open::session_open_elsewhere;
pub use open::{
    SessionHolder, hold_primary_session, hold_session_open, release_session_open, session_open_by,
};
pub use store::session_title;
pub use store::*;

#[cfg(test)]
pub(crate) use store::{next_session_id_value, queue_history_write, write_history_file};

/// Record a thread whose history stops being saved (a full disk, a read-only
/// folder) in its log and as a `history.write_failed` operational event.
pub fn report_history_write_failures() {
    rustcode_session::on_history_write_failure(|path, error| {
        let thread = path
            .parent()
            .and_then(std::path::Path::file_name)
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or_default();
        crate::logger::operational_event(
            "history.write_failed",
            serde_json::json!({
                "session_id": thread,
                "error_kind": format!("{:?}", error.kind()),
            }),
        );
    });
}
