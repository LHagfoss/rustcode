//! Pi takes no ACP `mcpServers`, so `pi-acp` drops the orchestrator server the
//! other harnesses get. `pi --extension` is how it gets those tools: `pi-acp`
//! starts `PI_ACP_PI_COMMAND`, here a small script that adds the extension,
//! and the extension starts `rustcode mcp-orchestrator` and the workspace's
//! MCP servers (Settings › MCP) itself.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::session::StartOptions;
use crate::config::McpServerConfig;
use crate::orchestration::tool_socket::{
    ASK_USER_ENV, DEFER_TOOLS_ENV, Launch, ORCHESTRATION_VERSION_ENV, ORCHESTRATOR_TOKEN_ENV,
    SERVER_NAME, SMART_UI_ENV,
};

/// The workspace's MCP servers, as JSON, for the extension to start (it takes the variable out of
/// the agent's shell, since remote servers' headers carry their sign-in).
const SERVERS_ENV: &str = "RUSTCODE_PI_MCP_SERVERS";

const EXTENSION: &str = include_str!("pi_extension.ts");
/// Keep both script formats testable on every host platform.
fn wrapper(windows: bool) -> (&'static str, &'static str) {
    if windows {
        (
            "pi-with-thread-tools.cmd",
            "@echo off\r\npi --extension \"%RUSTCODE_PI_EXTENSION%\" %*\r\n",
        )
    } else {
        (
            "pi-with-thread-tools",
            "#!/bin/sh\nexec pi --extension \"$RUSTCODE_PI_EXTENSION\" \"$@\"\n",
        )
    }
}

/// Write the extension and its script into `dir`, and give the script's path.
fn install(dir: &Path) -> std::io::Result<(PathBuf, PathBuf)> {
    std::fs::create_dir_all(dir)?;
    let extension = dir.join("rustcode-thread-tools.ts");
    std::fs::write(&extension, EXTENSION)?;
    let (name, script) = wrapper(cfg!(windows));
    let wrapper = dir.join(name);
    std::fs::write(&wrapper, script)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok((wrapper, extension))
}

/// The environment a Pi thread starts with: the extension that gives it the
/// orchestrator's tools and the workspace's MCP servers, as `StartOptions` names them.
pub(crate) fn thread_environment(options: &StartOptions) -> Vec<(String, String)> {
    if options.orchestrator.is_none() && options.mcp.is_empty() {
        return Vec::new();
    }
    match super::config::config_dir() {
        Ok(config) => environment(
            options.orchestrator.as_ref(),
            &options.mcp,
            &config.join("run").join("pi"),
        ),
        Err(_) => Vec::new(),
    }
}

/// The servers the extension starts beside the orchestrator, as JSON: a workspace server
/// named like the orchestrator gives way to it, as for the ACP harnesses.
fn servers_json(launch: Option<&Launch>, servers: &[McpServerConfig]) -> String {
    let listed: Vec<Value> = servers
        .iter()
        .filter(|server| launch.is_none() || server.name != SERVER_NAME)
        .map(
            |server| match server.url.as_deref().filter(|_| server.is_remote()) {
                Some(url) => json!({"name": server.name, "url": url, "headers": server.headers}),
                None => json!({
                    "name": server.name,
                    "command": server.command,
                    "args": server.args,
                    "env": server.env,
                }),
            },
        )
        .collect();
    Value::Array(listed).to_string()
}

/// The environment that gives a Pi thread the orchestrator's tools and the
/// workspace's MCP servers. Empty, so the thread runs without them, when the
/// files cannot be written.
fn environment(
    launch: Option<&Launch>,
    servers: &[McpServerConfig],
    dir: &Path,
) -> Vec<(String, String)> {
    let (wrapper, extension) = match install(dir) {
        Ok(paths) => paths,
        Err(error) => {
            crate::dbg_log!("harness pi: could not write the thread tools extension: {error}");
            return Vec::new();
        }
    };
    let mut env = vec![
        (
            "PI_ACP_PI_COMMAND".to_owned(),
            wrapper.to_string_lossy().into_owned(),
        ),
        (
            "RUSTCODE_PI_EXTENSION".to_owned(),
            extension.to_string_lossy().into_owned(),
        ),
        (SERVERS_ENV.to_owned(), servers_json(launch, servers)),
    ];
    let Some(launch) = launch else {
        return env;
    };
    env.extend([
        (
            "RUSTCODE_PI_MCP_COMMAND".to_owned(),
            launch.command.to_string_lossy().into_owned(),
        ),
        (
            "RUSTCODE_PI_MCP_ARGS".to_owned(),
            serde_json::to_string(&launch.args).unwrap_or_else(|_| "[]".to_owned()),
        ),
        (ORCHESTRATOR_TOKEN_ENV.to_owned(), launch.token.clone()),
        (
            ORCHESTRATION_VERSION_ENV.to_owned(),
            launch.version.to_string(),
        ),
    ]);
    let flags = [
        (SMART_UI_ENV, launch.smart_ui),
        (ASK_USER_ENV, launch.ask_user),
        (DEFER_TOOLS_ENV, launch.defer_tools),
    ];
    for (name, on) in flags {
        if on {
            env.push((name.to_owned(), "1".to_owned()));
        }
    }
    env
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapper_uses_windows_command_script_and_forwards_arguments() {
        assert_eq!(
            wrapper(true),
            (
                "pi-with-thread-tools.cmd",
                "@echo off\r\npi --extension \"%RUSTCODE_PI_EXTENSION%\" %*\r\n"
            )
        );
    }

    #[test]
    fn wrapper_preserves_unix_script_and_argument_quoting() {
        assert_eq!(
            wrapper(false),
            (
                "pi-with-thread-tools",
                "#!/bin/sh\nexec pi --extension \"$RUSTCODE_PI_EXTENSION\" \"$@\"\n"
            )
        );
    }

    #[test]
    fn environment_writes_the_files_and_passes_the_token_outside_the_args() {
        let dir = tempfile::tempdir().unwrap();
        let launch = Launch {
            command: "/bin/rustcode".into(),
            args: vec!["mcp-orchestrator".into(), "--session".into(), "t".into()],
            token: "secret".into(),
            smart_ui: false,
            ask_user: true,
            defer_tools: true,
            version: crate::orchestration::VERSION,
        };
        let env = environment(Some(&launch), &[], dir.path());
        let get = |name: &str| {
            env.iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.as_str())
        };
        assert_eq!(get(ORCHESTRATOR_TOKEN_ENV), Some("secret"));
        assert_eq!(get(ASK_USER_ENV), Some("1"));
        assert_eq!(get(DEFER_TOOLS_ENV), Some("1"));
        assert_eq!(get(SMART_UI_ENV), None);
        assert_eq!(
            get(ORCHESTRATION_VERSION_ENV),
            Some(crate::orchestration::VERSION.to_string().as_str())
        );
        assert!(!get("RUSTCODE_PI_MCP_ARGS").unwrap().contains("secret"));
        let installed_wrapper = Path::new(get("PI_ACP_PI_COMMAND").unwrap());
        let (name, script) = wrapper(cfg!(windows));
        assert_eq!(installed_wrapper.file_name().unwrap(), name);
        assert_eq!(std::fs::read_to_string(installed_wrapper).unwrap(), script);
        assert!(
            std::fs::read_to_string(get("RUSTCODE_PI_EXTENSION").unwrap())
                .unwrap()
                .contains("mcp__rustcode__")
        );
    }

    fn server(name: &str) -> McpServerConfig {
        serde_json::from_value(json!({"name": name, "command": "npx", "args": ["-y", name]}))
            .unwrap()
    }

    #[test]
    fn the_workspace_servers_reach_pi_without_the_orchestrator_name() {
        let dir = tempfile::tempdir().unwrap();
        let launch = Launch {
            command: "/bin/rustcode".into(),
            args: Vec::new(),
            token: "secret".into(),
            smart_ui: false,
            ask_user: false,
            defer_tools: false,
            version: crate::orchestration::VERSION,
        };
        let mut remote = server("sentry");
        remote.url = Some("https://mcp.sentry.dev/mcp".into());
        remote
            .headers
            .insert("Authorization".into(), "Bearer t".into());
        let servers = [server("rustcode"), server("context7"), remote];
        let env = environment(Some(&launch), &servers, dir.path());
        let listed: Value =
            serde_json::from_str(&env.iter().find(|(key, _)| key == SERVERS_ENV).unwrap().1)
                .unwrap();
        let names: Vec<_> = listed
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["context7", "sentry"]);
        assert_eq!(listed[0]["command"], "npx");
        assert_eq!(listed[1]["url"], "https://mcp.sentry.dev/mcp");
        assert_eq!(listed[1]["headers"]["Authorization"], "Bearer t");
    }

    #[test]
    fn a_thread_without_the_orchestrator_still_gets_its_servers() {
        let dir = tempfile::tempdir().unwrap();
        let env = environment(None, &[server("context7")], dir.path());
        let get = |name: &str| env.iter().any(|(key, _)| key == name);
        assert!(get("PI_ACP_PI_COMMAND") && get(SERVERS_ENV));
        assert!(!get(ORCHESTRATOR_TOKEN_ENV) && !get("RUSTCODE_PI_MCP_COMMAND"));
        assert!(thread_environment(&StartOptions::default()).is_empty());
    }
}
