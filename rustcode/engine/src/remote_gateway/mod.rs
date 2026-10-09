//! Remote gateway foundation (issue #1909, first half).
//!
//! A separate host process with one explicitly configured WebSocket listener
//! for paired devices and one private control socket for the local CLI. It
//! pairs devices, authenticates them and holds their connections; it never
//! executes agent turns and shares no session yet. Session registry, routing,
//! receipts and replay plug in behind [`router::FrameRouter`].
//!
//! This tree is transport and trust only. The versioned remote protocol
//! (issue #1907: envelope, session operations, schema) lives in its own
//! `remote` module; the handshake frames in [`handshake`] are the one piece
//! of wire format defined here and are meant to move into that envelope.
//!
//! The existing `rustcode serve` transport (`crate::serve`) is unrelated and
//! unchanged.

pub mod address;
pub mod handshake;
pub mod pairing;
pub mod router;

#[cfg(unix)]
pub mod command;
#[cfg(unix)]
pub mod control;
#[cfg(unix)]
pub mod devices;
#[cfg(unix)]
pub mod gateway;
#[cfg(unix)]
pub mod lifecycle;
#[cfg(unix)]
mod transport;

/// Default TCP port of the gateway's WebSocket listener.
pub const DEFAULT_PORT: u16 = 17879;
