//! Configuration constants for the fmr TUI client.

/// Base URL of the Apple Foundation Models server.
pub const API_BASE_URL: &str = "http://127.0.0.1:1976/v1/chat/completions";

/// Model name sent in completion requests.
pub const MODEL_NAME: &str = "system";

/// Default system prompt instructing the model about available tools.
pub const TOOLS_SYSTEM_PROMPT: &str = "\
You have access to local tools. To call a tool, respond with [TOOL: tool_name] on its own line.\n\
Available tools:\n\
  get_time - Returns the current system date/time (runs `date`)\n\
  get_env  - Returns OS info (runs `uname -sr`)";

/// Maximum number of agent-tool-calling iterations per user prompt.
pub const MAX_AGENT_ITERATIONS: usize = 5;

/// A registered tool with its shell command mapping.
#[derive(Debug, Clone)]
pub struct ToolDef {
    pub name: &'static str,
    pub command: &'static [&'static str],
}

/// Registry of built-in tools the model can invoke.
pub const TOOLS: &[ToolDef] = &[
    ToolDef {
        name: "get_time",
        command: &["date"],
    },
    ToolDef {
        name: "get_env",
        command: &["uname", "-sr"],
    },
];

/// Apple's on-device foundation model context-window limit (tokens).
pub const MAX_CONTEXT_TOKENS: u32 = 2048;
