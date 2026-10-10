//! Remote protocol v1 for attaching a client to a running session (issue #1907).
//!
//! Wire types, bounded projections, the identity-checked operations a session
//! owner applies, and the owner's side of the link to the gateway ([`owner`],
//! [`publisher`]). There is no listener, gateway or pairing here; the terminal
//! bridge and the gateway build on these types. The experimental
//! [`crate::serve`] protocol is separate and unchanged.
//!
//! The Rust types are the source of truth. The JSON Schema generated from
//! them and one golden frame per shape are committed under
//! `docs/remote-protocol/v1/`; `contract::tests` fails when they drift.

pub mod contract;
#[cfg(unix)]
mod images;
pub mod ops;
pub mod owner;
pub mod projection;
pub mod protocol;
pub mod publisher;

pub use ops::{
    OwnerFollowUp, SessionMutation, SessionRegistration, apply_session_mutation, read_session,
};
pub use projection::{
    ProjectionContext, ProjectionLimits, bound_text, content_chunk, project_event,
    project_history_page, project_session_info, project_snapshot, resolve_content,
};
pub use protocol::*;
pub use publisher::SessionPublisher;
