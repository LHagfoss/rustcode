//! The RustCode machine client.
//!
//! One implementation of "reach another computer's RustCode and speak to it",
//! shared by the desktop app's Rust shell and `rustcode machines`, so a
//! machine is saved, paired and connected to the same way wherever it is used.
//!
//! - [`registry`]: saved machines keyed by `host_id`, with their routes, in
//!   `machines.json` (`0600` on Unix).
//! - [`tokens`]: the device token in the operating system credential store,
//!   never in the provider credential files, and the rotating pairing's
//!   short-lived access token in memory only.
//! - [`transport`]: the WebSocket to a machine's gateway, its handshake and
//!   the ACP channel it becomes.
//! - [`routes`]: the order routes are tried in, which of them this computer is
//!   on an overlay for, the race between them and the hint a route that does
//!   not answer earns.
//! - [`machine`]: the ask-to-connect pairing flow, route selection and the one
//!   ACP read (`session/list`).
//! - [`reconnect`]: the backoff a client waits out while a machine is away, the
//!   route that worked last first, the network change or wake from sleep that
//!   cuts the wait short, and why the last attempt failed.
//! - [`connect_code`]: the connect codes another computer's owner creates
//!   there and this computer redeems.
//! - [`discovery`]: Bonjour, for machines that take a desktop client.
//! - [`ssh`]: connect over SSH: install RustCode on a computer this one
//!   reaches with `ssh`, start its server and pair with it in one step.
//! - [`protocol`]: the remote protocol v1 frames this client sends, mirrored
//!   against the committed contract in `docs/remote-protocol/v1`.
//!
//! The crate never depends on the engine: the desktop app and the command
//! line both link it, and the engine links neither.

pub mod config;
pub mod connect_code;
pub mod discovery;
pub mod machine;
pub mod protocol;
pub mod reconnect;
pub mod registry;
pub mod routes;
pub mod ssh;
pub mod tokens;
pub mod transport;

pub use connect_code::ConnectCode;
pub use discovery::{DiscoveredMachine, SERVICE_TYPE, browse, browse_async};
pub use machine::{
    Approved, Ask, ClientKind, Connector, Identity, NotThisMachine, Paired, PairingLost,
    PairingLostReason, Refusal, Session, Unreachable, Welcome, machine_from_pairing,
    needs_new_pairing, platform,
};
pub use protocol::{
    AcpClientFrame, AcpServerFrame, CAPABILITY_TOKEN_ROTATION, ErrorCode, HandshakeRequest,
    HandshakeResponse, PAIRING_CODE_DIGITS, Secret,
};
pub use reconnect::{
    Cause, SILENCE_LIMIT, answer_deadline, backoff_cap, backoff_delay, network_changed,
    prefer_last_working, slept,
};
pub use registry::{
    Machine, MachineRegistry, OverlayProvider, ProbeOutcome, Route, RouteChange, RouteKind,
    RouteProbe,
};
pub use routes::{
    AddressSpace, DialFailure, LAN_GRACE, RACE_DEADLINE, Race, RaceSettings, RouteFailure,
    RouteReport, address_space, local_addresses, on_local_overlay, ordered_for_this_computer,
};
pub use tokens::{Access, TokenStore};
pub use transport::{AcpChannel, DEFAULT_PORT, MAX_FRAME_BYTES};
