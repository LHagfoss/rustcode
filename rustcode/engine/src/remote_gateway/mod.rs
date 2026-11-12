//! The remote gateway (issue #1909).
//!
//! A separate host process with one explicitly configured WebSocket listener
//! for paired devices and two private local sockets: a control socket for
//! the CLI and an owner socket on which terminals register the sessions they
//! share. It pairs devices, authenticates them, keeps the live registry of
//! shared sessions ([`hub`]) and routes each operation to the terminal that
//! owns the session. It never executes agent turns.
//!
//! The session operations and their schema live in the `remote` module; the
//! handshake frames in [`handshake`] are defined here and committed as part
//! of the same versioned contract.
//!
//! The existing `rustcode serve` transport (`crate::serve`) is unrelated and
//! unchanged.

mod acp_relay;
pub mod address;
pub mod command;
pub mod connect;
pub mod connect_code;
pub mod control;
pub mod devices;
pub mod discovery;
pub mod gateway;
pub mod handshake;
pub mod host;
pub mod hub;
pub mod lifecycle;
pub mod local_socket;
pub mod orphan;
pub mod owner_client;
pub mod owner_ipc;
pub mod pairing;
pub mod pairing_watch;
pub mod qr;
pub mod router;
pub mod service;
mod transport;
pub mod wire_names;
pub mod workspace;

// End-to-end through Unix sockets, Unix permissions and process signals.
#[cfg(all(test, unix))]
mod e2e_tests;

/// Default TCP port of the gateway's WebSocket listener.
pub const DEFAULT_PORT: u16 = 17879;
