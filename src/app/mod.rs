//! Application state and input-handling modules.

mod state;
pub mod suggestion;

// Re-export public types so callers don't need to know the submodule layout.
pub use state::*;
