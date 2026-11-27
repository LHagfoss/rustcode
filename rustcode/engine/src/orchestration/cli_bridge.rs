//! The orchestration tools from a thread's shell: `rc tools`, `rc secret`.
//!
//! An external harness thread already reaches these tools through the MCP
//! server the engine injects into it (`mcp_server`). Every listed MCP tool is
//! resent with each model request, so the engine also hands the harness's
//! process the same identity in its environment ([`shell_env`]): the thread
//! id, the hub's socket and the thread's token. A command the agent runs
//! inherits them, and [`Bridge`] sends its call through the same dispatch as
//! an MCP `tools/call`, so validation, the user's approval cards and the
//! private secret form behave exactly as they do there. A tool reached this
//! way costs no context until it is called.
//!
//! The token is the one the thread's MCP server already holds, and it lets
//! a command call every tool that thread can, the writes still behind the
//! user's approval. It is no longer confined to the MCP server: every
//! process the harness starts inherits it, an in-harness subagent and a
//! build script included, and it lasts until the thread's harness stops.
//! `rc terminal` drops it from the commands it keeps running, and a delegate
//! the engine starts gets a token of its own, since a harness never inherits
//! the engine's `RUSTCODE_*` variables (`harness::process`).

use std::path::PathBuf;

use serde_json::Value;

use super::mcp_server::{Identity, SocketForward};
use super::tool_socket::{
    ASK_USER_ENV, CallAnswer, Launch, ORCHESTRATION_VERSION_ENV, SMART_UI_ENV,
};

/// The thread a harness's commands belong to.
pub const THREAD_ENV: &str = "RUSTCODE_THREAD_ID";
/// The engine's orchestrator socket those commands call.
pub const SOCKET_ENV: &str = "RUSTCODE_ORCHESTRATOR_SOCKET";
/// The thread's token, under a name without `TOKEN`, `KEY` or `SECRET`:
/// Codex drops variables named like those from the shells it starts.
pub const CREDENTIAL_ENV: &str = "RUSTCODE_THREAD_CREDENTIAL";

/// What `rc tools` says outside a thread.
pub const NOT_IN_THREAD: &str = "error: this command works inside a RustCode thread, whose shell carries RUSTCODE_THREAD_ID, RUSTCODE_ORCHESTRATOR_SOCKET and RUSTCODE_THREAD_CREDENTIAL. Run it from an agent's shell in a thread that RustCode started.";

/// The variables a harness process gets so its commands can call the
/// thread's orchestration tools.
pub fn shell_env(launch: &Launch) -> Vec<(String, String)> {
    let flag = |name: &str| {
        launch
            .args
            .iter()
            .position(|arg| arg == name)
            .and_then(|at| launch.args.get(at + 1))
            .cloned()
    };
    let (Some(thread), Some(socket)) = (flag("--session"), flag("--socket")) else {
        return Vec::new();
    };
    let mut env = vec![
        (THREAD_ENV.to_owned(), thread),
        (SOCKET_ENV.to_owned(), socket),
        (CREDENTIAL_ENV.to_owned(), launch.token.clone()),
        (
            ORCHESTRATION_VERSION_ENV.to_owned(),
            launch.version.to_string(),
        ),
    ];
    if launch.smart_ui {
        env.push((SMART_UI_ENV.to_owned(), "1".to_owned()));
    }
    if launch.ask_user {
        env.push((ASK_USER_ENV.to_owned(), "1".to_owned()));
    }
    env
}

/// One thread's way to its orchestration tools from a shell command.
pub struct Bridge {
    identity: Identity,
    forward: SocketForward,
    version: u8,
    smart_ui: bool,
    ask_user: bool,
}

impl Bridge {
    /// The bridge for the thread whose shell this process runs in, read from
    /// [`shell_env`]'s variables. [`NOT_IN_THREAD`] when they are missing.
    pub fn from_env() -> Result<Self, String> {
        let read = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
        let (Some(session_id), Some(socket), Some(token)) =
            (read(THREAD_ENV), read(SOCKET_ENV), read(CREDENTIAL_ENV))
        else {
            return Err(NOT_IN_THREAD.to_owned());
        };
        let version = read(ORCHESTRATION_VERSION_ENV)
            .and_then(|value| value.parse::<u8>().ok())
            .map_or(super::VERSION, |version| version.min(super::VERSION));
        Ok(Self {
            identity: Identity {
                session_id,
                token,
                outside: false,
            },
            forward: SocketForward {
                socket: PathBuf::from(socket),
            },
            version,
            smart_ui: read(SMART_UI_ENV).is_some_and(|on| on == "1"),
            ask_user: read(ASK_USER_ENV).is_some_and(|on| on == "1"),
        })
    }

    /// The thread this bridge calls for.
    pub fn thread_id(&self) -> &str {
        &self.identity.session_id
    }

    /// Every tool the thread can call, as MCP tool entries under their public
    /// names, the ones its MCP list leaves out included.
    pub fn tools(&self) -> Vec<Value> {
        super::mcp_server::cli_tools(self.version, self.smart_ui, self.ask_user)
    }

    /// Call `name` (a public or internal tool name) with `args`, as the
    /// thread's MCP server would.
    pub async fn call(&self, name: &str, args: Value) -> CallAnswer {
        super::mcp_server::cli_call(
            &self.forward,
            &self.identity,
            name,
            args,
            self.version,
            self.smart_ui,
            self.ask_user,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_launch_gives_its_commands_the_thread_socket_and_token() {
        let launch = Launch::new(
            "/opt/rc".into(),
            "thread-1",
            std::path::Path::new("/run/orchestrator-1.sock"),
            "secret",
        )
        .with_smart_ui(true);
        let env = shell_env(&launch);
        let get = |name: &str| {
            env.iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.as_str())
        };
        assert_eq!(get(THREAD_ENV), Some("thread-1"));
        assert_eq!(get(SOCKET_ENV), Some("/run/orchestrator-1.sock"));
        assert_eq!(get(CREDENTIAL_ENV), Some("secret"));
        assert_eq!(get(SMART_UI_ENV), Some("1"));
        assert_eq!(get(ASK_USER_ENV), None);
    }

    #[test]
    fn the_cli_lists_the_tools_the_mcp_list_leaves_out() {
        let names: Vec<String> =
            super::super::mcp_server::cli_tools(super::super::VERSION, false, false)
                .into_iter()
                .filter_map(|entry| entry["name"].as_str().map(str::to_owned))
                .collect();
        assert!(names.iter().any(|name| name == "request_secret"));
        assert!(names.iter().any(|name| name == "worktree_status"));
    }
}
