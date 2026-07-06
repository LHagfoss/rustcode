//! Configuration constants for the fmr TUI client.

/// Base URL of the Apple Foundation Models server.
pub const API_BASE_URL: &str = "http://127.0.0.1:1976/v1/chat/completions";

/// Model name sent in completion requests.
pub const MODEL_NAME: &str = "system";

/// Apple's on-device foundation model context-window limit (tokens).
pub const MAX_CONTEXT_TOKENS: u32 = 2048;
