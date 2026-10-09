use super::misc::{MANAGE_MCP_SERVERS, manage_mcp_servers_in, mcp_servers_approval};
use super::{AuthorizationDecision, ToolCall, ToolSafety, authorize_tool_with_args};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

const SECRET: &str = "sk-live-0123456789";

/// The servers saved in `dir`, without the ones a fresh config starts with.
fn configured(dir: &Path) -> Vec<crate::config::McpServerConfig> {
    let defaults = crate::config::AppConfig::default().mcp_servers;
    crate::config::load_config_from(dir)
        .2
        .mcp_servers
        .into_iter()
        .filter(|server| !defaults.contains(server))
        .collect()
}

/// The `list` lines for servers these tests added.
fn listed(dir: &Path, workspace: &Path) -> Vec<String> {
    let defaults = crate::config::AppConfig::default().mcp_servers;
    manage_mcp_servers_in(&json!({"operation": "list"}), dir, workspace)
        .unwrap()
        .lines()
        .filter(|line| {
            !defaults
                .iter()
                .any(|server| line.starts_with(&format!("{}  ", server.name)))
        })
        .map(str::to_owned)
        .collect()
}

/// A stdio server that answers the handshake, lists one tool and answers one
/// call to it.
fn stub_server_script() -> String {
    [
        r#"read line; echo '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05","capabilities":{},"serverInfo":{"name":"stub","version":"1"}}}'"#,
        "read line",
        r#"read line; echo '{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"ping","description":"Answer pong","inputSchema":{"type":"object","properties":{}}}]}}'"#,
        r#"read line; echo '{"jsonrpc":"2.0","id":3,"result":{"content":[{"type":"text","text":"pong from the stub"}]}}'"#,
        "read line",
    ]
    .join("; ")
}

/// Run `body` the way a tool call runs: on a blocking thread of the session
/// runtime, against a registry no other test shares.
async fn in_session<T: Send + 'static>(
    registry: &crate::mcp::McpRegistry,
    body: impl FnOnce() -> T + Send + 'static,
) -> T {
    let registry = Arc::clone(registry);
    tokio::task::spawn_blocking(move || crate::mcp::DIRECT_MCP_REGISTRY.sync_scope(registry, body))
        .await
        .expect("tool body should not panic")
}

fn empty_registry() -> crate::mcp::McpRegistry {
    Arc::new(Mutex::new(HashMap::new()))
}

#[test]
fn every_operation_needs_approval_and_none_is_read_only() {
    assert!(MANAGE_MCP_SERVERS.requires_confirmation);
    assert_eq!(MANAGE_MCP_SERVERS.safety, ToolSafety::ControlPlane);
    assert!(
        super::TOOLS
            .iter()
            .any(|tool| tool.name == "manage_mcp_servers")
    );
    for operation in ["add", "list", "start", "remove"] {
        let call = ToolCall {
            name: "manage_mcp_servers".to_owned(),
            arguments: json!({"operation": operation, "name": "files"}),
            call_id: None,
        };
        assert_eq!(
            authorize_tool_with_args(
                &call.name,
                &call.arguments,
                crate::config::AgentMode::Build,
                false,
                false,
            ),
            AuthorizationDecision::RequireConfirmation,
            "{operation}"
        );
        assert!(
            matches!(
                authorize_tool_with_args(
                    &call.name,
                    &call.arguments,
                    crate::config::AgentMode::Plan,
                    false,
                    false,
                ),
                AuthorizationDecision::Deny(_)
            ),
            "{operation} must stay blocked in Plan mode"
        );
        // A read-only classification would skip the mutation budget, the
        // parallel scheduler's barrier and the read-only subagent boundary.
        assert!(!super::is_read_only_call(&call), "{operation}");
        assert!(!super::supports_parallel_execution(&call.name));
        assert!(!crate::network::loop_detect::is_read_only_call(
            &call.name,
            &call.arguments
        ));
    }
    assert!(super::validate_tool_calls(
        &[ToolCall {
            name: "manage_mcp_servers".to_owned(),
            arguments: json!({"operation": "add", "name": "x", "target": ["x"], "path": "/etc"}),
            call_id: None,
        }],
        1,
    )
    .is_err());
}

#[test]
fn add_list_and_remove_share_the_cli_validation() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let run = |args: Value| manage_mcp_servers_in(&args, dir.path(), workspace.path());

    for (rejected, reason) in [
        (json!({"operation": "add", "target": ["npx"]}), "name"),
        (
            json!({"operation": "add", "name": "files"}),
            "URL or a command",
        ),
        (
            json!({"operation": "add", "name": "bad name", "target": ["npx"], "start": false}),
            "may only contain",
        ),
        (
            json!({"operation": "add", "name": "api", "target": ["https://mcp.example.test"], "env": ["A=b"]}),
            "only applies to stdio",
        ),
        (
            json!({"operation": "add", "name": "api", "target": ["https://mcp.example.test", "extra"]}),
            "no further arguments",
        ),
        (
            json!({"operation": "add", "name": "files", "target": ["npx"], "headers": ["X-Key: v"]}),
            "only apply to remote",
        ),
        (
            json!({"operation": "add", "name": "api", "target": ["https://mcp.example.test"], "headers": ["no-separator"]}),
            "Name: value",
        ),
        (
            json!({"operation": "add", "name": "api", "target": ["mcp.example.test"], "transport": "http"}),
            "http:// or https://",
        ),
        (
            json!({"operation": "add", "name": "files", "target": ["npx"], "env": ["NOEQUALS"]}),
            "KEY=value",
        ),
        (json!({"operation": "start"}), "name"),
        (
            json!({"operation": "start", "name": "ghost"}),
            "no MCP server",
        ),
        (
            json!({"operation": "remove", "name": "ghost"}),
            "no MCP server",
        ),
        (
            json!({"operation": "restart", "name": "ghost"}),
            "unknown operation",
        ),
    ] {
        let error = run(rejected.clone()).unwrap_err();
        assert!(error.contains(reason), "{rejected}: {error}");
    }
    assert!(
        configured(dir.path()).is_empty(),
        "a rejected call saves nothing"
    );
    assert!(listed(dir.path(), workspace.path()).is_empty());

    let add = json!({
        "operation": "add", "name": "files", "target": ["npx", "-y", "files-mcp"],
        "env": ["API_KEY=first"], "start": false,
    });
    let added = run(add.clone()).unwrap();
    assert!(
        added.starts_with("Added files  stdio  npx -y files-mcp  (env: API_KEY)"),
        "{added}"
    );
    let saved = configured(dir.path());
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].command, "npx");
    assert_eq!(saved[0].args, ["-y", "files-mcp"]);
    assert_eq!(saved[0].env["API_KEY"], "first");

    let duplicate = run(add).unwrap_err();
    assert!(duplicate.contains("already exists"), "{duplicate}");
    assert!(duplicate.contains("\"replace\": true"), "{duplicate}");
    let replaced = run(json!({
        "operation": "add", "name": "files", "target": ["npx", "-y", "files-mcp"],
        "env": ["API_KEY=second"], "replace": true, "start": false,
    }))
    .unwrap();
    assert!(replaced.starts_with("Replaced files"), "{replaced}");
    assert_eq!(configured(dir.path())[0].env["API_KEY"], "second");

    assert_eq!(
        listed(dir.path(), workspace.path()),
        ["files  stdio  npx -y files-mcp  (env: API_KEY)  [not running]"]
    );
    assert_eq!(
        run(json!({"operation": "remove", "name": "files"})).unwrap(),
        "Removed 'files' from the user config."
    );
    assert!(configured(dir.path()).is_empty());
}

#[test]
fn a_server_the_project_overrides_is_saved_but_not_started() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let project = workspace.path().join(".rustcode");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("config.toml"), "mcp_servers = []\n").unwrap();

    // No session runtime exists here, so reaching the start would be an error.
    let output = manage_mcp_servers_in(
        &json!({"operation": "add", "name": "files", "target": ["npx"]}),
        dir.path(),
        workspace.path(),
    )
    .unwrap();
    assert!(output.contains("not active in this workspace"), "{output}");
    assert_eq!(configured(dir.path()).len(), 1);
}

#[test]
fn results_and_the_approval_prompt_hide_header_and_environment_values() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let remote = json!({
        "operation": "add", "name": "api", "target": ["https://mcp.example.test/mcp"],
        "headers": [format!("Authorization: Bearer {SECRET}"), "X-Org: acme-internal"],
        "start": false,
    });
    let stdio = json!({
        "operation": "add", "name": "files", "target": ["npx", "-y", "files-mcp"],
        "env": [format!("API_KEY={SECRET}")], "start": false,
    });

    for args in [&remote, &stdio] {
        let approval = mcp_servers_approval("manage_mcp_servers", args, Some(workspace.path()))
            .expect("the tool describes its own approval");
        let shown = format!(
            "{}\n{}\n{}",
            approval.label, approval.preview, approval.arguments
        );
        assert!(!shown.contains(SECRET), "{shown}");
        assert!(!shown.contains("acme-internal"), "{shown}");
        assert!(shown.contains("[REDACTED]"), "{shown}");

        let added = manage_mcp_servers_in(args, dir.path(), workspace.path()).unwrap();
        assert!(!added.contains(SECRET), "{added}");
    }
    let approval = mcp_servers_approval("manage_mcp_servers", &remote, None).unwrap();
    assert_eq!(
        approval.label,
        "add api  http  https://mcp.example.test/mcp  (headers: Authorization, X-Org)"
    );
    assert_eq!(
        approval.arguments["headers"],
        json!(["Authorization: [REDACTED]", "X-Org: [REDACTED]"])
    );
    assert_eq!(
        mcp_servers_approval("manage_mcp_servers", &stdio, None)
            .unwrap()
            .arguments["env"],
        json!(["API_KEY=[REDACTED]"])
    );
    assert!(mcp_servers_approval("run_command", &remote, None).is_none());

    // The values are stored: the server needs them to authenticate.
    let saved = configured(dir.path());
    assert_eq!(
        saved[0].headers["Authorization"],
        format!("Bearer {SECRET}")
    );
    assert_eq!(saved[1].env["API_KEY"], SECRET);

    assert_eq!(
        listed(dir.path(), workspace.path()),
        [
            "api  http  https://mcp.example.test/mcp  (headers: Authorization, X-Org)  [not running]",
            "files  stdio  npx -y files-mcp  (env: API_KEY)  [not running]",
        ]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_start_keeps_the_entry_and_does_not_quote_its_secrets() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let registry = empty_registry();
    let (config_dir, root) = (dir.path().to_owned(), workspace.path().to_owned());

    let error = in_session(&registry, move || {
        manage_mcp_servers_in(
            &json!({
                "operation": "add", "name": "leaky",
                "target": ["sh", "-c", "echo \"refusing key $API_KEY\" >&2; exit 3"],
                "env": [format!("API_KEY={SECRET}")],
            }),
            &config_dir,
            &root,
        )
    })
    .await
    .unwrap_err();

    assert!(error.contains("failed to start"), "{error}");
    assert!(error.contains("refusing key [REDACTED]"), "{error}");
    assert!(!error.contains(SECRET), "{error}");
    assert_eq!(
        configured(dir.path()).len(),
        1,
        "the entry stays for a retry"
    );
    assert!(registry.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_server_added_mid_session_is_started_and_its_tools_are_callable() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let registry = empty_registry();
    let generation = crate::mcp::mcp_generation();

    let (config_dir, root) = (dir.path().to_owned(), workspace.path().to_owned());
    let (added, listed, call_is_valid, called) = in_session(&registry, move || {
        let added = manage_mcp_servers_in(
            &json!({
                "operation": "add", "name": "stub",
                "target": ["sh", "-c", stub_server_script()],
            }),
            &config_dir,
            &root,
        );
        let listed = listed(&config_dir, &root);
        // The normal MCP path: the registry resolves the canonical name for
        // validation and dispatch, with no schema bound in advance.
        let call = ToolCall {
            name: "mcp__stub__ping".to_owned(),
            arguments: json!({}),
            call_id: None,
        };
        let call_is_valid = super::validate_tool_calls(std::slice::from_ref(&call), 1);
        let called = super::execute_with_metadata(&call.name, &call.arguments);
        (added, listed, call_is_valid, called)
    })
    .await;

    let added = added.unwrap();
    assert!(added.starts_with("Added stub  stdio  sh -c"), "{added}");
    assert!(
        added.ends_with("Started 'stub' in this session. Callable now: mcp__stub__ping."),
        "{added}"
    );
    assert_eq!(listed.len(), 1);
    assert!(listed[0].ends_with("[running]"), "{listed:?}");
    assert_eq!(call_is_valid, Ok(()));
    assert!(called.success, "{}", called.content);
    assert_eq!(called.content, "pong from the stub");
    assert_eq!(configured(dir.path())[0].name, "stub");
    assert!(
        crate::mcp::mcp_generation() > generation,
        "the tool schema must be rebuilt for the next request"
    );

    let (config_dir, root) = (dir.path().to_owned(), workspace.path().to_owned());
    let (restarted, removed) = in_session(&registry, move || {
        (
            manage_mcp_servers_in(
                &json!({"operation": "start", "name": "stub"}),
                &config_dir,
                &root,
            ),
            manage_mcp_servers_in(
                &json!({"operation": "remove", "name": "stub"}),
                &config_dir,
                &root,
            ),
        )
    })
    .await;
    assert!(restarted.unwrap().contains("mcp__stub__ping"));
    assert_eq!(
        removed.unwrap(),
        "Removed 'stub' from the user config and stopped it."
    );
    assert!(configured(dir.path()).is_empty());
    assert!(registry.lock().unwrap().is_empty());
}
