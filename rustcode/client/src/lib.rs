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
//! - [`machine`]: the ask-to-connect pairing flow, route selection and the one
//!   ACP read (`session/list`).
//! - [`discovery`]: Bonjour, for machines that take a desktop client.
//! - [`protocol`]: the remote protocol v1 frames this client sends, mirrored
//!   against the committed contract in `docs/remote-protocol/v1`.
//!
//! The crate never depends on the engine: the desktop app and the command
//! line both link it, and the engine links neither.

pub mod config;
pub mod discovery;
pub mod machine;
pub mod protocol;
pub mod registry;
pub mod tokens;
pub mod transport;

pub use discovery::{DiscoveredMachine, SERVICE_TYPE, browse, browse_async};
pub use machine::{
    Approved, Ask, ClientKind, Connector, Identity, Paired, Refusal, Session, Unreachable, Welcome,
    machine_from_pairing, platform,
};
pub use protocol::{
    AcpClientFrame, AcpServerFrame, CAPABILITY_TOKEN_ROTATION, ErrorCode, HandshakeRequest,
    HandshakeResponse, PAIRING_CODE_DIGITS, Secret,
};
pub use registry::{Machine, MachineRegistry, Route, RouteKind};
pub use tokens::{Access, TokenStore};
pub use transport::{AcpChannel, DEFAULT_PORT, MAX_FRAME_BYTES};
