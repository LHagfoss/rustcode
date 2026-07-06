//! Application state and input-handling modules.

pub mod suggestion;
mod state;

// Re-export public types so callers don't need to know the submodule layout.
pub use state::*;
