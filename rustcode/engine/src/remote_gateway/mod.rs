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

pub mod address;
pub mod handshake;
pub mod hub;
pub mod owner_ipc;
pub mod pairing;
pub mod qr;
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
pub mod owner_client;
#[cfg(unix)]
mod transport;

#[cfg(all(test, unix))]
mod e2e_tests;

/// Default TCP port of the gateway's WebSocket listener.
pub const DEFAULT_PORT: u16 = 17879;

#[cfg(unix)]
pub mod workspace;

#[cfg(unix)]
mod discovery;

#[cfg(unix)]
pub mod service;
