use super::*;

fn stdio(name: &str, command: &str, enabled: bool) -> crate::config::McpServerConfig {
    serde_json::from_value(json!({"name": name, "command": command, "enabled": enabled}))
        .expect("server config")
}

#[test]
fn reconcile_starts_a_server_added_after_the_session_began() {
    // A shell `rc mcp add` wrote the config; the running session
    // never started it, so its tools could not be listed.
    let previous = [stdio("mail", "mail_mcp", true)];
    let current = [stdio("mail", "mail_mcp", true), stdio("ftp", "npx", true)];
    let running = HashSet::from(["mail".to_owned()]);
    let (start, stop) = plan_reconcile(&previous, &current, &running);
    assert_eq!(
        start.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
        ["ftp"]
    );
    assert!(stop.is_empty());
}

#[test]
fn reconcile_restarts_changed_and_stops_removed_or_disabled_servers() {
    let previous = [
        stdio("a", "old", true),
        stdio("b", "b", true),
        stdio("c", "c", true),
    ];
    let current = [stdio("a", "new", true), stdio("c", "c", false)];
    let running = HashSet::from(["a".to_owned(), "b".to_owned(), "c".to_owned()]);
    let (start, mut stop) = plan_reconcile(&previous, &current, &running);
    stop.sort();
    assert_eq!(
        start.iter().map(|s| s.command.as_str()).collect::<Vec<_>>(),
        ["new"]
    );
    assert_eq!(stop, ["b", "c"]);
}

#[test]
fn reconcile_leaves_unchanged_failed_and_already_started_servers_alone() {
    // `broken` failed at startup and is unchanged: no retry every turn.
    // `app` was added and started by the app before the turn reconciled.
    let previous = [stdio("broken", "x", true)];
    let current = [stdio("broken", "x", true), stdio("app", "y", true)];
    let running = HashSet::from(["app".to_owned()]);
    let (start, stop) = plan_reconcile(&previous, &current, &running);
    assert!(start.is_empty(), "{start:?}");
    assert!(stop.is_empty());
}

#[test]
fn reconcile_never_stops_a_server_it_did_not_configure() {
    // A server started outside the config (e.g. a job) is not ours to stop.
    let running = HashSet::from(["other".to_owned()]);
    let (_, stop) = plan_reconcile(&[], &[], &running);
    assert!(stop.is_empty());
}

/// Metadata captured verbatim from the live Vercel MCP OAuth endpoints on
/// 2026-09-30 (see #1536):
///
/// - `WWW-Authenticate` from `GET https://mcp.vercel.com` (401), value only
///   (the `www-authenticate:` header name is stripped)
/// - `GET https://mcp.vercel.com/.well-known/oauth-protected-resource`
/// - `GET https://vercel.com/.well-known/oauth-authorization-server`
///
/// The stub-based OAuth tests cannot see these shapes, so they are pinned
/// here: the authorization server advertises exactly one response mode
/// (`web_message.opener`), which can never deliver a code to a loopback
/// listener, and `offline_access` is the scope that buys a refresh token.
const VERCEL_CHALLENGE: &str = include_str!("fixtures/mcp_oauth_vercel/challenge.2026-09-30.txt");
const VERCEL_PROTECTED_RESOURCE: &str =
    include_str!("fixtures/mcp_oauth_vercel/protected_resource.2026-09-30.json");
const VERCEL_AUTHORIZATION_SERVER: &str =
    include_str!("fixtures/mcp_oauth_vercel/authorization_server.2026-09-30.json");

fn vercel_resource_meta() -> Value {
    serde_json::from_str(VERCEL_PROTECTED_RESOURCE).expect("captured resource metadata")
}

fn vercel_server_meta() -> Value {
    serde_json::from_str(VERCEL_AUTHORIZATION_SERVER).expect("captured server metadata")
}

fn query_param(url: &str, key: &str) -> Option<String> {
    let (_, query) = url.split_once('?')?;
    query.split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        (name == key)
            .then(|| urlencoding::decode(value).ok().map(|v| v.into_owned()))
            .flatten()
    })
}

fn stdio_server(name: &str) -> crate::config::McpServerConfig {
    crate::config::McpServerConfig {
        name: name.to_string(),
        command: "not-used".to_string(),
        args: Vec::new(),
        env: HashMap::new(),
        url: None,
        headers: HashMap::new(),
        client_id: None,
        enabled: true,
        always_include: false,
        timeout_secs: None,
    }
}

#[tokio::test]
async fn startup_helper_visits_enabled_servers_and_continues_after_failure() {
    let servers = vec![
        crate::config::McpServerConfig {
            name: "enabled-one".to_string(),
            command: "not-used".to_string(),
            args: Vec::new(),
            env: HashMap::new(),
            url: None,
            headers: HashMap::new(),
            client_id: None,
            enabled: true,
            always_include: false,
            timeout_secs: None,
        },
        crate::config::McpServerConfig {
            name: "disabled".to_string(),
            command: "not-used".to_string(),
            args: Vec::new(),
            env: HashMap::new(),
            url: None,
            headers: HashMap::new(),
            client_id: None,
            enabled: false,
            always_include: false,
            timeout_secs: None,
        },
        crate::config::McpServerConfig {
            name: "enabled-two".to_string(),
            command: "not-used".to_string(),
            args: Vec::new(),
            env: HashMap::new(),
            url: None,
            headers: HashMap::new(),
            client_id: None,
            enabled: true,
            always_include: false,
            timeout_secs: None,
        },
    ];
    let started = Arc::new(StdMutex::new(Vec::new()));
    let observed = Arc::clone(&started);

    let warnings = start_enabled_servers(&servers, move |name| {
        let observed = Arc::clone(&observed);
        async move {
            observed.lock().unwrap().push(name.clone());
            if name == "enabled-one" {
                Err("injected startup failure".to_string())
            } else {
                Ok(())
            }
        }
    })
    .await;

    assert_eq!(
        started.lock().unwrap().as_slice(),
        ["enabled-one", "enabled-two"]
    );
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("enabled-one"));
}

#[tokio::test]
async fn startup_helper_does_not_wait_forever_for_a_hanging_server() {
    let servers = vec![
        crate::config::McpServerConfig {
            name: "hanging".to_string(),
            command: "not-used".to_string(),
            args: Vec::new(),
            env: HashMap::new(),
            url: None,
            headers: HashMap::new(),
            client_id: None,
            enabled: true,
            always_include: false,
            timeout_secs: None,
        },
        crate::config::McpServerConfig {
            name: "reachable".to_string(),
            command: "not-used".to_string(),
            args: Vec::new(),
            env: HashMap::new(),
            url: None,
            headers: HashMap::new(),
            client_id: None,
            enabled: true,
            always_include: false,
            timeout_secs: None,
        },
    ];
    let started = Arc::new(StdMutex::new(Vec::new()));
    let observed = Arc::clone(&started);

    let completed = tokio::time::timeout(
        Duration::from_millis(100),
        start_enabled_servers_with_timeout(&servers, Duration::from_millis(10), move |name| {
            let observed = Arc::clone(&observed);
            async move {
                observed.lock().unwrap().push(name.clone());
                if name == "hanging" {
                    std::future::pending().await
                } else {
                    Ok(())
                }
            }
        }),
    )
    .await;

    assert!(completed.is_ok(), "startup must be bounded per server");
    let warnings = completed.unwrap();
    assert_eq!(started.lock().unwrap().as_slice(), ["hanging", "reachable"]);
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("hanging"));
}

#[tokio::test]
async fn test_mcp_client_handshake() {
    // A simple mock process that responds to 'initialize' request and 'tools/list' request
    let script = "read line; echo '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"protocolVersion\":\"2024-11-05\",\"capabilities\":{},\"serverInfo\":{\"name\":\"mock\",\"version\":\"1.0.0\"}}}'; read line; read line; echo '{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"tools\":[{\"name\":\"test_tool\",\"description\":\"a test tool\",\"inputSchema\":{}}]}}'";
    let client = McpClient::start(
        "mock_server".to_string(),
        "sh".to_string(),
        vec!["-c".to_string(), script.to_string()],
        HashMap::new(),
    )
    .await;

    assert!(client.is_ok());
    let client = client.unwrap();

    assert_eq!(client.name, "mock_server");
    let tools = client.get_tools().unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].get("name").unwrap().as_str().unwrap(), "test_tool");

    client.shutdown().await;
}

#[test]
fn mcp_app_visibility_defaults_and_fails_closed() {
    assert!(tool_visible_to(&json!({}), "model"));
    assert!(tool_visible_to(&json!({}), "app"));
    let app_only = json!({"_meta":{"ui":{"visibility":["app"]}}});
    assert!(!tool_visible_to(&app_only, "model"));
    assert!(tool_visible_to(&app_only, "app"));
    assert!(!tool_visible_to(
        &json!({"_meta":{"ui":{"visibility":"app"}}}),
        "app"
    ));
}

#[tokio::test]
async fn mcp_line_is_bounded_before_parsing() {
    let mut reader = BufReader::new(&b"12345\nnext\n"[..]);
    assert_eq!(
        bounded_mcp_line(&mut reader, 6).await.unwrap().unwrap(),
        b"12345\n"
    );
    assert_eq!(
        bounded_mcp_line(&mut reader, 4).await.unwrap_err().kind(),
        std::io::ErrorKind::InvalidData
    );
}

#[tokio::test]
async fn tool_app_reads_only_the_declared_resource() {
    let script = r#"read line; echo '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05","capabilities":{},"serverInfo":{"name":"mock","version":"1"}}}'; read line; read line; echo '{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"weather","inputSchema":{},"_meta":{"ui":{"resourceUri":"ui://weather/view"}}}]}}'; read line; case "$line" in *'ui://weather/view'*) echo '{"jsonrpc":"2.0","id":3,"result":{"contents":[{"uri":"ui://weather/view","mimeType":"text/html;profile=mcp-app","text":"<p>Weather</p>"}]}}';; esac"#;
    let client = McpClient::start(
        "mock".into(),
        "sh".into(),
        vec!["-c".into(), script.into()],
        HashMap::new(),
    )
    .await
    .unwrap();
    assert!(client.read_tool_app("other").await.unwrap().is_none());
    let app = client.read_tool_app("weather").await.unwrap().unwrap();
    assert_eq!(app.uri, "ui://weather/view");
    assert_eq!(app.html.as_deref(), Some("<p>Weather</p>"));
    client.shutdown().await;
}

#[tokio::test]
async fn app_calls_require_visibility_and_approval() {
    let script = r#"read line; echo '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05","capabilities":{},"serverInfo":{"name":"mock","version":"1"}}}'; read line; read line; echo '{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"app_tool","inputSchema":{},"_meta":{"ui":{"visibility":["app"]}}},{"name":"model_tool","inputSchema":{},"_meta":{"ui":{"visibility":["model"]}}}]}}'; read line; case "$line" in *'app_tool'*) echo '{"jsonrpc":"2.0","id":3,"result":{"content":[{"type":"text","text":"done"}]}}';; esac"#;
    let client = McpClient::start(
        "bound-server".into(),
        "sh".into(),
        vec!["-c".into(), script.into()],
        HashMap::new(),
    )
    .await
    .unwrap();
    assert!(
        client
            .call_app_tool("model_tool", json!({}), |_, _, _| async { true })
            .await
            .is_err()
    );
    assert!(
        client
            .call_app_tool("app_tool", json!({}), |_, _, _| async { false })
            .await
            .is_err()
    );
    let result = client
        .call_app_tool("app_tool", json!({}), |server, tool, _| {
            assert_eq!((server, tool), ("bound-server", "app_tool"));
            async { true }
        })
        .await
        .unwrap();
    assert_eq!(result["content"][0]["text"], "done");
    client.shutdown().await;
}

/// A mock server that answers the handshake, then answers tool call 3
/// after `body`, which may report progress for it first.
async fn slow_tool_server(name: &str, body: &str) -> Arc<McpClient> {
    let script = format!(
        "read line; echo '{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{\"protocolVersion\":\"2024-11-05\",\"capabilities\":{{}},\"serverInfo\":{{\"name\":\"mock\",\"version\":\"1.0.0\"}}}}}}'; read line; read line; echo '{{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{{\"tools\":[]}}}}'; read line; {body} echo '{{\"jsonrpc\":\"2.0\",\"id\":3,\"result\":{{\"content\":[]}}}}'; read line"
    );
    let client = McpClient::start(
        name.to_string(),
        "sh".to_string(),
        vec!["-c".to_string(), script],
        HashMap::new(),
    )
    .await
    .expect("mock server starts");
    client.set_request_timeout(Some(1));
    client
}

#[tokio::test]
async fn test_mcp_tool_call_progress_extends_timeout() {
    let progress = "echo '{\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\",\"params\":{\"progressToken\":3,\"progress\":1}}';";
    let body =
        format!("sleep 0.6; {progress} sleep 0.6; {progress} sleep 0.6; {progress} sleep 0.6;");
    let client = slow_tool_server("progress_server", &body).await;

    let result = client.call_tool("slow", json!({})).await;

    assert!(
        result.is_ok(),
        "progress must keep the call alive: {result:?}"
    );
    client.shutdown().await;
}

#[tokio::test]
async fn test_mcp_tool_call_frequent_progress_keeps_extending() {
    // More notifications than the old 60-extension cap, then a few more
    // spaced under the timeout: the call is alive the whole time.
    let progress = "echo '{\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\",\"params\":{\"progressToken\":3,\"progress\":1}}';";
    let body = format!(
        "i=0; while [ $i -lt 80 ]; do {progress} i=$((i+1)); done; \
             sleep 0.6; {progress} sleep 0.6; {progress} sleep 0.6; {progress} sleep 0.6;"
    );
    let client = slow_tool_server("chatty_server", &body).await;

    let result = client.call_tool("slow", json!({})).await;

    assert!(
        result.is_ok(),
        "frequent progress must not use up the allowance: {result:?}"
    );
    client.shutdown().await;
}

#[tokio::test]
async fn test_mcp_tool_call_times_out_without_progress() {
    let client = slow_tool_server("silent_server", "sleep 2;").await;

    let error = client
        .call_tool("slow", json!({}))
        .await
        .expect_err("a silent call past its timeout fails");

    assert!(error.contains("timed out after 1s"), "{error}");
    assert!(error.contains("timeout_secs"), "{error}");
    client.shutdown().await;
}

#[tokio::test]
async fn test_mcp_tool_call_timeout_cancels_on_the_server() {
    // The mock records the line after the tool call; on a timeout that
    // is the cancellation for request 3.
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("after");
    let body = format!(
        "read cancel; echo \"$cancel\" > {}; sleep 2;",
        log.display()
    );
    let client = slow_tool_server("cancel_server", &body).await;

    let error = client
        .call_tool("slow", json!({}))
        .await
        .expect_err("times out");
    assert!(error.contains("timed out"), "{error}");
    tokio::time::sleep(Duration::from_millis(300)).await;
    let line = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(line.contains("notifications/cancelled"), "{line}");
    assert!(line.contains("\"requestId\":3"), "{line}");
    client.shutdown().await;
}

#[tokio::test]
async fn test_mcp_client_passes_configured_environment() {
    let script = "test \"$MCP_TEST_ENV\" = forwarded || exit 3; read line; echo '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"protocolVersion\":\"2024-11-05\",\"capabilities\":{},\"serverInfo\":{\"name\":\"mock\",\"version\":\"1.0.0\"}}}'; read line; read line; echo '{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"tools\":[]}}'";
    let mut env = HashMap::new();
    env.insert("MCP_TEST_ENV".to_string(), "forwarded".to_string());

    let client = McpClient::start(
        "env_server".to_string(),
        "sh".to_string(),
        vec!["-c".to_string(), script.to_string()],
        env,
    )
    .await;

    assert!(client.is_ok());
    client.unwrap().shutdown().await;
}

#[tokio::test]
async fn test_mcp_client_reports_server_exit_without_waiting_for_request_timeout() {
    let result = tokio::time::timeout(
        Duration::from_secs(1),
        McpClient::start(
            "exited_server".to_string(),
            "sh".to_string(),
            vec!["-c".to_string(), "exit 0".to_string()],
            HashMap::new(),
        ),
    )
    .await
    .expect("server exit should be observed promptly");

    let error = match result {
        Err(error) => error,
        Ok(_) => panic!("an exited MCP server cannot complete startup"),
    };
    assert!(error.contains("Server closed connection"));
}

#[tokio::test]
async fn test_mcp_client_includes_server_stderr_when_startup_fails() {
    let result = tokio::time::timeout(
        Duration::from_secs(1),
        McpClient::start(
            "diagnostic_server".to_string(),
            "sh".to_string(),
            vec![
                "-c".to_string(),
                "echo 'IMAP_USER not set' >&2; exit 1".to_string(),
            ],
            HashMap::new(),
        ),
    )
    .await
    .expect("server startup failure should be observed promptly");

    let error = match result {
        Err(error) => error,
        Ok(_) => panic!("a server that exits cannot complete startup"),
    };
    assert!(error.contains("Server closed connection"));
    assert!(error.contains("IMAP_USER not set"));
}

#[test]
fn owned_server_rejects_entries_without_a_transport() {
    let mut server = stdio_server("half-configured");
    server.command = String::new();
    let error = server
        .validate()
        .expect_err("neither command nor url must fail");
    assert!(error.contains("half-configured"));
    assert!(error.contains("command"));
    assert!(error.contains("url"));
}

#[test]
fn remote_entries_validate_and_win_over_command() {
    let mut server = stdio_server("vercel");
    server.url = Some("https://mcp.vercel.com".to_string());
    assert!(server.is_remote());
    assert!(server.validate().is_ok());
    assert!(!stdio_server("plain").is_remote());
}

#[test]
fn oauth_tokens_round_trip_through_durable_storage() {
    let dir = tempfile::tempdir().expect("oauth token dir");
    let token = OAuthToken {
        access_token: "access-123".to_string(),
        refresh_token: Some("refresh-456".to_string()),
        client_id: Some("client-789".to_string()),
        token_endpoint: Some("https://auth.example.com/token".to_string()),
        expires_at: Some(now_unix_secs() + 3600),
        resource: Some("https://mcp.example.com/mcp".to_string()),
    };
    save_oauth_token_to_dir(dir.path(), "vercel", &token).expect("save token");
    let path = oauth_token_path_in_dir(dir.path(), "vercel");
    assert!(path.ends_with("mcp-oauth/vercel.json"));
    let loaded = load_oauth_token_from_dir(dir.path(), "vercel").expect("load token");
    assert_eq!(loaded, token);
    assert!(loaded.usable());

    let expired = OAuthToken {
        expires_at: Some(now_unix_secs().saturating_sub(10)),
        ..token
    };
    assert!(!expired.usable());
}

#[derive(Clone, Copy)]
enum MockHttpMode {
    Json,
    Sse,
    Unauthorized,
    Oversized,
    /// Legacy SSE server: the Streamable HTTP endpoint 404s, `GET /mcp`
    /// yields an `event: endpoint` handshake, and JSON-RPC flows over
    /// the advertised `/rpc` path.
    LegacySse,
    /// Minimal OAuth token endpoint for refresh/exchange round-trips.
    TokenEndpoint,
    /// RFC 8628 token endpoint: `authorization_pending`, then `slow_down`,
    /// then a token — the sequence a real device login produces.
    DeviceToken,
    /// Device login the user declined.
    DeviceDenied,
    /// Device code that expired before approval.
    DeviceExpired,
}

/// Token-endpoint answers for the device-grant mocks, keyed by poll count.
fn device_token_payload(mode: MockHttpMode, poll: usize) -> (u16, Value) {
    match mode {
        MockHttpMode::DeviceToken if poll == 0 => (400, json!({"error": "authorization_pending"})),
        MockHttpMode::DeviceToken if poll == 1 => (400, json!({"error": "slow_down"})),
        MockHttpMode::DeviceToken => (
            200,
            json!({
                "access_token": "device-access",
                "refresh_token": "device-refresh",
                "token_type": "Bearer",
                "expires_in": 3600,
            }),
        ),
        MockHttpMode::DeviceDenied => (400, json!({"error": "access_denied"})),
        MockHttpMode::DeviceExpired => (400, json!({"error": "expired_token"})),
        _ => (200, json!({"access_token": "unused"})),
    }
}

fn find_headers_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|pos| pos + 4)
}

/// Minimal single-request-per-connection HTTP mock speaking just enough
/// JSON-RPC to exercise the remote handshake, tool listing, and calls.
async fn spawn_mock_http_mcp(mode: MockHttpMode) -> (String, tokio::task::JoinHandle<()>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock MCP server");
    let url = format!("http://{}/mcp", listener.local_addr().expect("mock addr"));
    // Device-grant mocks answer a different body on each poll, so the
    // poll index is shared across the connections the client opens.
    let device_polls = Arc::new(StdMutex::new(0usize));
    let device_polls_conn = Arc::clone(&device_polls);
    let handle = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let device_polls = Arc::clone(&device_polls_conn);
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    match socket.read(&mut chunk).await {
                        Ok(0) => break,
                        Ok(n) => {
                            buf.extend_from_slice(&chunk[..n]);
                            if find_headers_end(&buf).is_some() || buf.len() > 65_536 {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
                let head_end = find_headers_end(&buf).unwrap_or(buf.len());
                let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                let content_length = head
                    .lines()
                    .skip(1)
                    .filter_map(|line| line.split_once(':'))
                    .find(|(key, _)| key.trim().eq_ignore_ascii_case("content-length"))
                    .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                let mut body = buf[head_end..].to_vec();
                while body.len() < content_length {
                    match socket.read(&mut chunk).await {
                        Ok(0) => break,
                        Ok(n) => body.extend_from_slice(&chunk[..n]),
                        Err(_) => break,
                    }
                }
                let request: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                let id = request.get("id").cloned().unwrap_or(Value::Null);
                let method = request
                    .get("method")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let mut request_line = head.lines().next().unwrap_or_default().split_whitespace();
                let http_method = request_line.next().unwrap_or_default();
                let request_path = request_line.next().unwrap_or("/");

                let poll = {
                    let mut count = device_polls.lock().expect("device poll counter");
                    *count += 1;
                    *count - 1
                };
                let (status, content_type, extra, payload) = match mode {
                    MockHttpMode::Unauthorized => (
                        401,
                        "application/json",
                        "WWW-Authenticate: Bearer error=\"invalid_token\"\r\n".to_string(),
                        json!({"error": "unauthorized"}).to_string(),
                    ),
                    MockHttpMode::TokenEndpoint => (
                        200,
                        "application/json",
                        String::new(),
                        json!({
                            "access_token": "refreshed-access",
                            "token_type": "Bearer",
                            "expires_in": 3600,
                        })
                        .to_string(),
                    ),
                    MockHttpMode::DeviceToken
                    | MockHttpMode::DeviceDenied
                    | MockHttpMode::DeviceExpired => {
                        let (status, payload) = device_token_payload(mode, poll);
                        (
                            status,
                            "application/json",
                            String::new(),
                            payload.to_string(),
                        )
                    }
                    MockHttpMode::LegacySse if http_method == "GET" => (
                        200,
                        "text/event-stream",
                        String::new(),
                        "event: endpoint\ndata: /rpc\n\n".to_string(),
                    ),
                    MockHttpMode::LegacySse if request_path != "/rpc" => (
                        404,
                        "application/json",
                        String::new(),
                        json!({"error": "not found"}).to_string(),
                    ),
                    _ if id.is_null() => {
                        // JSON-RPC notifications get 202 with no body.
                        (202, "application/json", String::new(), String::new())
                    }
                    _ => {
                        let response = match method.as_str() {
                            "initialize" => json!({
                                "jsonrpc": "2.0", "id": id,
                                "result": {
                                    "protocolVersion": "2024-11-05",
                                    "capabilities": {},
                                    "serverInfo": {"name": "mock-http", "version": "1.0.0"},
                                },
                            }),
                            "tools/list" => json!({
                                "jsonrpc": "2.0", "id": id,
                                "result": { "tools": mock_http_tools(mode) },
                            }),
                            "tools/call" => {
                                let tool = request
                                    .get("params")
                                    .and_then(|p| p.get("name"))
                                    .and_then(Value::as_str)
                                    .unwrap_or("unknown");
                                json!({
                                    "jsonrpc": "2.0", "id": id,
                                    "result": {
                                        "content": [
                                            {"type": "text", "text": format!("called {tool}")},
                                        ],
                                    },
                                })
                            }
                            _ => json!({
                                "jsonrpc": "2.0", "id": id,
                                "error": {"code": -32601, "message": "Method not found"},
                            }),
                        };
                        let body = response.to_string();
                        let (content_type, body) = match mode {
                            MockHttpMode::Sse => (
                                "text/event-stream",
                                format!("event: message\ndata: {body}\n\n"),
                            ),
                            _ => ("application/json", body),
                        };
                        (
                            200,
                            content_type,
                            "mcp-session-id: mock-session\r\n".to_string(),
                            body,
                        )
                    }
                };
                let reason = match status {
                    200 => "OK",
                    202 => "Accepted",
                    401 => "Unauthorized",
                    404 => "Not Found",
                    _ => "Error",
                };
                let response = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\n\
                         Content-Length: {}\r\nConnection: close\r\n{extra}\r\n{payload}",
                    payload.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
            });
        }
    });
    (url, handle)
}

fn mock_http_tools(mode: MockHttpMode) -> Vec<Value> {
    match mode {
        MockHttpMode::Oversized => {
            // Two tools whose combined schema bytes exceed the native
            // budget, so an always_include reservation must be rejected.
            let half = "x".repeat(crate::tools::schema::MAX_MCP_NATIVE_SCHEMA_BYTES / 2);
            vec![
                json!({
                    "name": "big_first",
                    "description": format!("Remote tool {half}"),
                    "inputSchema": {"type": "object", "properties": {}},
                }),
                json!({
                    "name": "big_second",
                    "description": format!("Remote tool {half}"),
                    "inputSchema": {"type": "object", "properties": {}},
                }),
            ]
        }
        _ => vec![json!({
            "name": "http_tool",
            "description": "a remote test tool",
            "inputSchema": {"type": "object", "properties": {}},
        })],
    }
}

#[tokio::test]
async fn remote_http_happy_path_lists_and_calls_tools() {
    let (url, _server) = spawn_mock_http_mcp(MockHttpMode::Json).await;
    let client = McpClient::start_remote("http-happy".to_string(), url, HashMap::new())
        .await
        .expect("remote server should start");
    let tools = client.get_tools().expect("tools");
    assert_eq!(tools.len(), 1);
    assert_eq!(
        tools[0].get("name").and_then(Value::as_str),
        Some("http_tool")
    );
    let result = client
        .call_tool("http_tool", json!({}))
        .await
        .expect("tool call");
    let text = result
        .get("content")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("text"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(text.contains("http_tool"), "unexpected result: {result}");
    client.shutdown().await;
}

#[tokio::test]
async fn remote_http_sse_wrapped_responses_parse() {
    let (url, _server) = spawn_mock_http_mcp(MockHttpMode::Sse).await;
    let client = McpClient::start_remote("http-sse".to_string(), url, HashMap::new())
        .await
        .expect("SSE-wrapped server should start");
    let tools = client.get_tools().expect("tools");
    assert_eq!(tools.len(), 1);
    assert_eq!(
        tools[0].get("name").and_then(Value::as_str),
        Some("http_tool")
    );
    client.shutdown().await;
}

#[tokio::test]
async fn remote_http_legacy_sse_fallback_lists_and_calls_tools() {
    let (url, _server) = spawn_mock_http_mcp(MockHttpMode::LegacySse).await;
    let client = McpClient::start_remote("http-legacy".to_string(), url, HashMap::new())
        .await
        .expect("legacy SSE server should start via fallback");
    let tools = client.get_tools().expect("tools");
    assert_eq!(tools.len(), 1);
    assert_eq!(
        tools[0].get("name").and_then(Value::as_str),
        Some("http_tool")
    );
    let result = client
        .call_tool("http_tool", json!({}))
        .await
        .expect("tool call over legacy endpoint");
    let text = result
        .get("content")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("text"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(text.contains("http_tool"), "unexpected result: {result}");
    client.shutdown().await;
}

#[tokio::test]
async fn oauth_refresh_exchanges_a_new_access_token() {
    let (url, _server) = spawn_mock_http_mcp(MockHttpMode::TokenEndpoint).await;
    let http = reqwest::Client::new();
    let token = refresh_access_token(
        &http,
        &url,
        Some("client-1"),
        "old-refresh",
        Some("https://mcp.example.com/mcp"),
    )
    .await
    .expect("refresh should succeed");
    assert_eq!(token.access_token, "refreshed-access");
    // The mock omits `refresh_token`, so the previous one is preserved
    // for the next refresh.
    assert_eq!(token.refresh_token.as_deref(), Some("old-refresh"));
    assert_eq!(token.client_id.as_deref(), Some("client-1"));
    assert_eq!(token.token_endpoint.as_deref(), Some(url.as_str()));
    assert_eq!(
        token.resource.as_deref(),
        Some("https://mcp.example.com/mcp")
    );
    assert!(token.usable());
}

#[test]
fn sse_endpoint_helpers_parse_and_join() {
    let stream = ": keep-alive\n\nevent: message\ndata: {\"id\":1}\n\nevent: endpoint\ndata: /rpc?session=abc\n\n";
    assert_eq!(
        parse_sse_endpoint_event(stream).as_deref(),
        Some("/rpc?session=abc")
    );
    assert_eq!(
        parse_sse_endpoint_event("event: message\ndata: {}\n\n"),
        None
    );
    assert_eq!(
        join_endpoint("http://127.0.0.1:9/mcp", "/rpc?session=abc"),
        "http://127.0.0.1:9/rpc?session=abc"
    );
    assert_eq!(
        join_endpoint("http://127.0.0.1:9/mcp", "https://other.example/rpc"),
        "https://other.example/rpc"
    );
    assert_eq!(
        join_endpoint("http://127.0.0.1:9/mcp/", "rpc"),
        "http://127.0.0.1:9/mcp/rpc"
    );
    assert_eq!(
        http_origin("https://mcp.sentry.dev/mcp?x=1"),
        "https://mcp.sentry.dev"
    );
    assert_eq!(
            parse_challenge_param(
                "Bearer error=\"invalid_token\", resource_metadata=\"https://example.com/.well-known/oauth-protected-resource\"",
                "resource_metadata"
            )
            .as_deref(),
            Some("https://example.com/.well-known/oauth-protected-resource")
        );
    let candidates =
        oauth_server_metadata_urls("https://auth.example.com", "https://mcp.example.com/mcp");
    assert!(candidates[0].contains("oauth-authorization-server"));
    assert!(
        candidates
            .iter()
            .any(|u| u.contains("openid-configuration"))
    );
}

#[tokio::test]
async fn remote_http_connection_failure_names_the_server() {
    let result = McpClient::start_remote(
        "http-down".to_string(),
        "http://127.0.0.1:9/mcp".to_string(),
        HashMap::new(),
    )
    .await;
    let error = match result {
        Err(error) => error,
        Ok(_) => panic!("unreachable server must fail"),
    };
    assert!(error.contains("http-down"), "unexpected error: {error}");
    assert!(
        error.to_lowercase().contains("connect"),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn remote_http_auth_required_without_a_browser_flow() {
    let (url, _server) = spawn_mock_http_mcp(MockHttpMode::Unauthorized).await;
    let result = McpClient::start_remote("http-auth".to_string(), url, HashMap::new()).await;
    let error = match result {
        Err(error) => error,
        Ok(_) => panic!("401-only server must fail without a browser flow"),
    };
    assert!(error.contains("http-auth"), "unexpected error: {error}");
    assert!(
        error.to_lowercase().contains("authentication"),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn remote_http_schema_budget_overflow_rejects_the_reservation() {
    let (url, _server) = spawn_mock_http_mcp(MockHttpMode::Oversized).await;
    let client = McpClient::start_remote("http-big".to_string(), url, HashMap::new())
        .await
        .expect("oversized remote server should still start");
    let tools = client.get_tools().expect("tools");
    assert_eq!(tools.len(), 2);
    // Flatten exactly the way the native schema builder consumes live
    // registry tools, then run the real reservation/budget selection.
    let flattened: Vec<(String, String, Value)> = tools
        .iter()
        .map(|tool| {
            (
                tool.get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                tool.get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                tool.get("inputSchema").cloned().unwrap_or(json!({})),
            )
        })
        .collect();
    let owners = vec![client.name.clone(), client.name.clone()];
    let messages = vec![json!({"role": "user", "content": "use the big tools"})];
    let (selected, stats) =
        crate::tools::schema::select_mcp_tools_for_context_with_sticky_and_reservations_in_phase(
            &flattened,
            &owners,
            std::slice::from_ref(&client.name),
            &messages,
            &[],
            crate::tools::schema::ToolSchemaPhase::Established,
        );
    assert!(selected.is_empty());
    assert_eq!(stats.rejected_reservations.len(), 1);
    assert_eq!(stats.rejected_reservations[0], client.name);
    assert!(stats.schema_budget_exhausted);
    assert!(stats.mcp_schema_bytes <= stats.mcp_schema_budget_bytes);
    client.shutdown().await;
}

#[tokio::test]
async fn owned_server_refuses_disabled_remote_servers_without_network_use() {
    let workspace = tempfile::tempdir().expect("workspace");
    let server = crate::config::McpServerConfig {
        name: "http-off".to_string(),
        command: String::new(),
        args: Vec::new(),
        env: HashMap::new(),
        url: Some("http://127.0.0.1:9/mcp".to_string()),
        headers: HashMap::new(),
        client_id: None,
        enabled: false,
        always_include: false,
        timeout_secs: None,
    };
    let result = start_owned_server(&server, workspace.path()).await;
    let error = match result {
        Err(error) => error,
        Ok(_) => panic!("disabled server must not start"),
    };
    assert!(error.contains("disabled"), "unexpected error: {error}");
}

#[tokio::test]
async fn owned_server_starts_remote_servers_within_startup_timeout() {
    let workspace = tempfile::tempdir().expect("workspace");
    let (url, _server) = spawn_mock_http_mcp(MockHttpMode::Json).await;
    let server = crate::config::McpServerConfig {
        name: "http-owned".to_string(),
        command: String::new(),
        args: Vec::new(),
        env: HashMap::new(),
        url: Some(url),
        headers: HashMap::new(),
        client_id: None,
        enabled: true,
        always_include: false,
        timeout_secs: None,
    };
    let result = start_owned_server(&server, workspace.path()).await;
    let client = match result {
        Ok(client) => client,
        Err(error) => panic!("owned remote server should start: {error}"),
    };
    assert_eq!(client.get_tools().expect("tools").len(), 1);
    client.shutdown().await;
}

#[test]
fn live_vercel_challenge_points_at_the_captured_resource_metadata() {
    let resource_meta_url =
        parse_challenge_param(VERCEL_CHALLENGE, "resource_metadata").expect("challenge");
    assert_eq!(
        resource_meta_url,
        "https://mcp.vercel.com/.well-known/oauth-protected-resource"
    );
    let meta = vercel_resource_meta();
    assert_eq!(meta["authorization_servers"][0], "https://vercel.com");
    assert!(
        oauth_server_metadata_urls("https://vercel.com", "https://mcp.vercel.com")[0]
            .contains("oauth-authorization-server"),
        "the captured challenge must resolve to the captured AS metadata"
    );
}

#[test]
fn live_vercel_scope_requests_resource_permissions_and_refresh() {
    let resource_meta = vercel_resource_meta();
    let server_meta = vercel_server_meta();
    // The resource only advertises `openid`; the AS also knows
    // `email`/`profile`, which are not this server's scopes, so they are
    // not requested. `offline_access` is added because the AS advertises
    // it — that is what makes it issue a refresh token, without which the
    // login cannot survive a restart (#1536).
    assert_eq!(
        requested_scopes(Some(&resource_meta), &server_meta).as_deref(),
        Some("openid offline_access")
    );
    // No metadata about scopes at all: omit the parameter rather than send
    // an empty `scope=`.
    let bare = json!({"authorization_endpoint": "https://as.example/authorize"});
    assert_eq!(requested_scopes(None, &bare), None);
    assert_eq!(requested_scopes(Some(&json!({})), &bare), None);
    // With no resource scopes, request only refresh access, rather than
    // all permissions advertised for unrelated resources by the AS.
    assert_eq!(
        requested_scopes(None, &server_meta).as_deref(),
        Some("offline_access")
    );
}

#[test]
fn live_vercel_login_plan_is_the_device_grant_with_a_refresh_capable_scope() {
    // The plan is what `run_oauth_flow` executes: the grant it runs and
    // the scopes it asks for are read off the captured metadata, never
    // assumed. Against the pre-fix client both were absent — it always ran
    // a loopback redirect with no `scope`, which Vercel cannot complete.
    let plan = plan_oauth_login(Some(&vercel_resource_meta()), &vercel_server_meta())
        .expect("the captured metadata must yield a usable login");
    assert_eq!(plan.flow, OAuthFlow::Device);
    assert_eq!(plan.scope.as_deref(), Some("openid offline_access"));
    assert_eq!(
        plan.registration_grants,
        ["authorization_code", DEVICE_CODE_GRANT, "refresh_token"],
        "registration must offer the grant actually being used"
    );
    // A server that does support the default `query` mode keeps the
    // loopback flow, with `query` named explicitly rather than implicit.
    let mut with_query = vercel_server_meta();
    with_query["response_modes_supported"] = json!(["query", "fragment"]);
    let plan = plan_oauth_login(Some(&vercel_resource_meta()), &with_query).expect("plan");
    assert_eq!(plan.flow, OAuthFlow::Loopback(Some("query")));
    assert_eq!(
        plan.registration_grants,
        ["authorization_code", DEVICE_CODE_GRANT, "refresh_token"]
    );
}

#[test]
fn live_vercel_advertises_only_a_response_mode_a_loopback_cannot_receive() {
    let server_meta = vercel_server_meta();
    assert_eq!(
        server_meta["response_modes_supported"],
        json!(["web_message.opener"])
    );
    assert_eq!(loopback_response_mode(&server_meta), None);
    // ...and it does advertise the device grant, so that is the flow the
    // client must build instead of a loopback redirect.
    assert!(device_grant_supported(&server_meta));
    assert_eq!(select_oauth_flow(&server_meta), Ok(OAuthFlow::Device));
    let fragment_only = json!({"response_modes_supported": ["fragment"]});
    assert_eq!(
        select_oauth_flow(&fragment_only),
        Ok(OAuthFlow::Loopback(None))
    );
    // No `response_modes_supported` at all: RFC 6749 defaults to `query`.
    assert_eq!(
        select_oauth_flow(&json!({})),
        Ok(OAuthFlow::Loopback(Some("query")))
    );
    // Redirect-incapable *and* no device grant: an actionable error, not
    // a silently broken login.
    let unusable = json!({"response_modes_supported": ["web_message.opener"], "grant_types_supported": ["client_credentials"]});
    assert!(!device_grant_supported(&unusable));
    let error = plan_oauth_login(Some(&json!({})), &unusable).expect_err("no viable grant");
    assert!(error.contains("no usable"), "unexpected: {error}");
    assert!(
        error.contains("device authorization"),
        "unexpected: {error}"
    );
}

#[test]
fn authorization_request_built_for_live_vercel_names_scope_and_response_mode() {
    // Test a server advertising query mode, using the captured endpoints
    // and scopes. This does not establish Vercel's acceptance of query.
    let mut server_meta = vercel_server_meta();
    server_meta["response_modes_supported"] = json!(["query"]);
    let plan = plan_oauth_login(Some(&vercel_resource_meta()), &server_meta).expect("plan");
    let OAuthFlow::Loopback(response_mode) = plan.flow else {
        panic!("expected the loopback flow")
    };
    let url = build_authorization_url(&AuthorizationRequest {
        authorization_endpoint: server_meta["authorization_endpoint"].as_str().unwrap(),
        client_id: server_meta["registration_endpoint"]
            .as_str()
            .map(|_| "vercel-client"),
        redirect_uri: "http://127.0.0.1:51234/callback",
        code_challenge: "challenge-value",
        state: "state-value",
        response_mode,
        scope: plan.scope.as_deref(),
        resource: "https://mcp.vercel.com",
    });

    assert!(url.starts_with("https://vercel.com/oauth/authorize?"));
    assert_eq!(
        query_param(&url, "scope").as_deref(),
        Some("openid offline_access"),
        "the request must name scopes the server advertises: {url}"
    );
    assert_eq!(
        query_param(&url, "response_mode").as_deref(),
        Some("query"),
        "the request must name an advertised response mode: {url}"
    );
    assert_eq!(query_param(&url, "response_type").as_deref(), Some("code"));
    assert_eq!(
        query_param(&url, "resource").as_deref(),
        Some("https://mcp.vercel.com")
    );
    assert_eq!(
        query_param(&url, "redirect_uri").as_deref(),
        Some("http://127.0.0.1:51234/callback")
    );
    assert_eq!(
        query_param(&url, "code_challenge_method").as_deref(),
        Some("S256")
    );
    assert_eq!(
        query_param(&url, "client_id").as_deref(),
        Some("vercel-client")
    );
    // `scope` is omitted entirely when the metadata yields none.
    let bare = build_authorization_url(&AuthorizationRequest {
        authorization_endpoint: "https://as.example/authorize",
        client_id: None,
        redirect_uri: "http://127.0.0.1:1/cb",
        code_challenge: "c",
        state: "s",
        response_mode: Some("query"),
        scope: None,
        resource: "https://mcp.example",
    });
    assert!(!bare.contains("scope="), "unexpected empty scope: {bare}");
}

#[test]
fn device_request_built_for_live_vercel_names_the_scope() {
    let server_meta = vercel_server_meta();
    let device_endpoint = server_meta["device_authorization_endpoint"]
        .as_str()
        .expect("captured device endpoint");
    assert_eq!(
        device_endpoint,
        "https://api.vercel.com/login/oauth/device-authorization"
    );
    let plan = plan_oauth_login(Some(&vercel_resource_meta()), &server_meta).expect("plan");
    let params = device_authorization_params(Some("vercel-client"), plan.scope.as_deref());
    let body = params
        .iter()
        .map(|(key, value)| format!("{key}={}", urlencoding::encode(value)))
        .collect::<Vec<_>>()
        .join("&");
    assert_eq!(
        body, "client_id=vercel-client&scope=openid%20offline_access",
        "the device authorization request must name the derived scope"
    );
    // The device token request needs no redirect URI at all.
    let token_params = device_token_params(Some("vercel-client"), "device-code");
    assert_eq!(
        token_params.iter().map(|(key, _)| *key).collect::<Vec<_>>(),
        ["grant_type", "device_code", "client_id"]
    );
    assert_eq!(token_params[0].1, DEVICE_CODE_GRANT);
}

#[test]
fn device_authorization_response_is_parsed_with_rfc8628_defaults() {
    let parsed = parse_device_authorization(
        &json!({
            "device_code": "dev-code",
            "user_code": "WDJB-MJHT",
            "verification_uri": "https://vercel.com/device",
            "verification_uri_complete": "https://vercel.com/device?user_code=WDJB-MJHT",
        }),
        "vercel",
    )
    .expect("device response");
    assert_eq!(parsed.user_code, "WDJB-MJHT");
    assert_eq!(parsed.interval, Duration::from_secs(5));
    assert_eq!(parsed.expires_in, Duration::from_secs(900));
    assert!(
        parsed
            .verification_uri_complete
            .as_deref()
            .is_some_and(|uri| uri.contains("WDJB-MJHT"))
    );
    // `verification_url` is accepted as an alias, and explicit timings win.
    let parsed = parse_device_authorization(
        &json!({
            "device_code": "d",
            "user_code": "u",
            "verification_url": "https://as.example/device",
            "interval": 2,
            "expires_in": 60,
        }),
        "vercel",
    )
    .expect("device response");
    assert_eq!(parsed.verification_uri, "https://as.example/device");
    assert_eq!(parsed.interval, Duration::from_secs(2));
    assert_eq!(parsed.expires_in, Duration::from_secs(60));
    let error = parse_device_authorization(&json!({"user_code": "u"}), "vercel")
        .expect_err("device_code is required");
    assert!(error.contains("device_code"), "unexpected: {error}");
}

#[tokio::test]
async fn device_poll_waits_out_pending_and_slow_down_then_returns_the_token() {
    let (url, _server) = spawn_mock_http_mcp(MockHttpMode::DeviceToken).await;
    let body = poll_device_token(
        &reqwest::Client::new(),
        "vercel",
        &url,
        Some("client-1"),
        "dev-code",
        Duration::from_millis(10),
        Duration::from_secs(30),
    )
    .await
    .expect("device token");
    assert_eq!(body["access_token"], "device-access");
    assert_eq!(body["refresh_token"], "device-refresh");
}

#[tokio::test]
async fn device_poll_stops_on_a_denied_or_expired_code() {
    let (denied, _server) = spawn_mock_http_mcp(MockHttpMode::DeviceDenied).await;
    let error = poll_device_token(
        &reqwest::Client::new(),
        "vercel",
        &denied,
        None,
        "dev-code",
        Duration::from_millis(10),
        Duration::from_secs(30),
    )
    .await
    .expect_err("denied device login");
    assert!(error.contains("denied"), "unexpected: {error}");

    let (expired, _server) = spawn_mock_http_mcp(MockHttpMode::DeviceExpired).await;
    let error = poll_device_token(
        &reqwest::Client::new(),
        "vercel",
        &expired,
        None,
        "dev-code",
        Duration::from_millis(10),
        Duration::from_secs(30),
    )
    .await
    .expect_err("expired device code");
    assert!(error.contains("expired"), "unexpected: {error}");
}

#[tokio::test]
async fn loopback_callback_requires_query_parameters_from_real_http_clients() {
    for fragment in [false, true] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let sender = tokio::spawn(async move {
            let separator = if fragment { '#' } else { '?' };
            reqwest::get(format!(
                "http://{addr}/callback{separator}code=abc&state=xyz"
            ))
            .await
            .unwrap();
        });
        let result = wait_for_oauth_code(listener, "stub").await;
        sender.await.unwrap();
        if fragment {
            assert!(result.unwrap_err().contains("no authorization code"));
        } else {
            assert_eq!(result.unwrap(), ("abc".into(), "xyz".into()));
        }
    }
}
