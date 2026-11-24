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
