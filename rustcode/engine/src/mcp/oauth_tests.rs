use super::*;
use tokio::io::AsyncReadExt;

#[derive(Clone, Copy)]
enum ServerMode {
    Downgrade,
    RejectDevice,
    RejectRegistration,
    MissingClientId,
    UnsupportedClientGrants,
    DeviceOnly,
    Denied,
    WrongState,
}

struct OAuthStub {
    base: String,
    requests: Arc<StdMutex<Vec<(String, String)>>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for OAuthStub {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn oauth_stub(mode: ServerMode) -> OAuthStub {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let requests = Arc::new(StdMutex::new(Vec::new()));
    let captured = requests.clone();
    let origin = base.clone();
    let task = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let origin = origin.clone();
            let captured = captured.clone();
            tokio::spawn(async move {
                let mut bytes = Vec::new();
                let mut chunk = [0u8; 4096];
                let (header_end, content_length) = loop {
                    let read = socket.read(&mut chunk).await.unwrap();
                    if read == 0 {
                        return;
                    }
                    bytes.extend_from_slice(&chunk[..read]);
                    if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..end]);
                        let len = headers
                            .lines()
                            .find_map(|line| {
                                let (key, value) = line.split_once(':')?;
                                key.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        break (end + 4, len);
                    }
                };
                while bytes.len() < header_end + content_length {
                    let read = socket.read(&mut chunk).await.unwrap();
                    if read == 0 {
                        return;
                    }
                    bytes.extend_from_slice(&chunk[..read]);
                }
                let head = String::from_utf8_lossy(&bytes[..header_end]);
                let path = head
                    .lines()
                    .next()
                    .unwrap()
                    .split_whitespace()
                    .nth(1)
                    .unwrap()
                    .to_string();
                let body = String::from_utf8_lossy(&bytes[header_end..]).into_owned();
                captured.lock().unwrap().push((path.clone(), body.clone()));
                let mut extra = String::new();
                let (status, payload) = match path.split('?').next().unwrap() {
                    "/mcp" => {
                        extra = format!(
                            "WWW-Authenticate: Bearer resource_metadata=\"{origin}/resource\", scope=\"files:read\"\r\n"
                        );
                        (401, json!({"error":"unauthorized"}))
                    }
                    "/resource" => (
                        200,
                        json!({"resource":format!("{origin}/mcp"),"authorization_servers":[origin],"scopes_supported":["fallback:scope"]}),
                    ),
                    "/.well-known/oauth-authorization-server" => (
                        200,
                        json!({
                            "issuer":origin,"authorization_endpoint":format!("{origin}/authorize"),
                            "token_endpoint":format!("{origin}/token"),"registration_endpoint":format!("{origin}/register"),
                            "device_authorization_endpoint":format!("{origin}/device"),
                            "response_types_supported":["code"],"response_modes_supported":["web_message.opener"],
                            "grant_types_supported":if matches!(mode, ServerMode::DeviceOnly) { vec![DEVICE_CODE_GRANT,"refresh_token"] } else { vec!["authorization_code", DEVICE_CODE_GRANT,"refresh_token"] },
                            "scopes_supported":["openid","offline_access"]
                        }),
                    ),
                    "/register" if matches!(mode, ServerMode::RejectRegistration) => (
                        400,
                        json!({"error":"invalid_client_metadata","error_description":"native application_type required"}),
                    ),
                    "/register" if matches!(mode, ServerMode::MissingClientId) => {
                        (201, json!({"grant_types":["authorization_code"]}))
                    }
                    "/register" if matches!(mode, ServerMode::UnsupportedClientGrants) => (
                        201,
                        json!({"client_id":"registered-client","grant_types":["client_credentials"]}),
                    ),
                    "/register" if matches!(mode, ServerMode::DeviceOnly) => (
                        201,
                        json!({"client_id":"registered-client","grant_types":[DEVICE_CODE_GRANT,"refresh_token"]}),
                    ),
                    "/register" => (
                        201,
                        json!({"client_id":"registered-client","grant_types":if matches!(mode, ServerMode::Downgrade) { vec!["authorization_code","refresh_token"] } else {vec!["authorization_code",DEVICE_CODE_GRANT,"refresh_token"]}}),
                    ),
                    "/device" if matches!(mode, ServerMode::Denied) => (
                        200,
                        json!({"device_code":"device","user_code":"ABC","verification_uri":format!("{origin}/verify"),"expires_in":60,"interval":1}),
                    ),
                    "/verify" => (200, json!({})),
                    "/device" => (
                        400,
                        json!({"error":"unauthorized_client","error_description":"client cannot use device grant"}),
                    ),
                    "/authorize" => {
                        let url = reqwest::Url::parse(&format!("{origin}{path}")).unwrap();
                        let params: HashMap<_, _> = url.query_pairs().into_owned().collect();
                        if params.contains_key("response_mode") {
                            (400, json!({"error":"invalid_request"}))
                        } else {
                            let redirect = params.get("redirect_uri").unwrap();
                            let state = if matches!(mode, ServerMode::WrongState) {
                                "other-state"
                            } else {
                                params.get("state").unwrap()
                            };
                            extra =
                                format!("Location: {redirect}?code=auth-code&state={state}\r\n");
                            (302, json!({}))
                        }
                    }
                    "/token" if matches!(mode, ServerMode::Denied) => {
                        (400, json!({"error":"access_denied"}))
                    }
                    "/token" => (
                        200,
                        json!({"access_token":"access","refresh_token":"refresh","expires_in":3600}),
                    ),
                    _ => (404, json!({"error":"not_found"})),
                };
                let payload = payload.to_string();
                socket.write_all(format!("HTTP/1.1 {status} Response\r\nContent-Type: application/json\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{payload}",payload.len()).as_bytes()).await.unwrap();
            });
        }
    });
    OAuthStub {
        base,
        requests,
        task,
    }
}

fn oauth_client(stub: &OAuthStub) -> McpClient {
    McpClient {
        name: "stub-oauth".into(),
        transport: Transport::Remote {
            state: Mutex::new(Box::new(RemoteState {
                url: format!("{}/mcp", stub.base),
                headers: HashMap::new(),
                http: reqwest::Client::new(),
                session_id: None,
                legacy_endpoint: None,
                auth: None,
                client_id: None,
            })),
        },
        pending: Arc::new(Mutex::new(HashMap::new())),
        next_id: Arc::new(Mutex::new(1)),
        tools: Arc::new(StdMutex::new(Vec::new())),
        child: Arc::new(Mutex::new(None)),
        stderr_diagnostics: Arc::new(StdMutex::new(Vec::new())),
        stderr_finished: Arc::new(Notify::new()),
    }
}

#[test]
fn oauth_preserves_resource_scopes_not_advertised_by_as() {
    let resource = json!({"scopes_supported":["files:read"]});
    assert_eq!(
        requested_scopes(Some(&resource), &json!({})),
        Some("files:read".into())
    );
    assert_eq!(
        requested_scopes(
            Some(&resource),
            &json!({"scopes_supported":["openid","offline_access"]})
        ),
        Some("files:read offline_access".into())
    );
    assert_eq!(
        requested_scopes(None, &json!({"scopes_supported":["admin:write"]})),
        None
    );
}

#[tokio::test]
async fn oauth_registration_rejection_surfaces_the_original_error() {
    let stub = oauth_stub(ServerMode::RejectRegistration).await;
    let error = oauth_client(&stub)
        .run_oauth_flow(&reqwest::Client::new())
        .await
        .unwrap_err();
    assert!(
        error.contains("registration") && error.contains("native application_type required"),
        "{error}"
    );
    assert!(
        !stub
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|(path, _)| path == "/device")
    );
}

#[tokio::test]
async fn oauth_registered_grants_override_discovery() {
    let stub = oauth_stub(ServerMode::Downgrade).await;
    let browser = |url: &str| {
        let url = url.to_string();
        tokio::spawn(async move {
            reqwest::get(url).await.unwrap();
        });
    };
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        oauth_client(&stub).run_oauth_flow_with(&reqwest::Client::new(), &browser, None),
    )
    .await;
    assert!(matches!(result, Ok(Ok(_))), "{result:?}");
    assert!(
        !stub
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|(path, _)| path == "/device")
    );
}

#[tokio::test]
async fn oauth_rejected_device_falls_back_and_persists_refreshable_token() {
    let stub = oauth_stub(ServerMode::RejectDevice).await;
    let client = oauth_client(&stub);
    let browser = |url: &str| {
        let url = url.to_string();
        tokio::spawn(async move {
            reqwest::get(url).await.unwrap();
        });
    };
    let token = tokio::time::timeout(
        Duration::from_secs(2),
        client.run_oauth_flow_with(&reqwest::Client::new(), &browser, None),
    )
    .await
    .unwrap()
    .unwrap();
    let requests = stub.requests.lock().unwrap().clone();
    assert!(requests.iter().any(|(path, _)| path == "/device"));
    let registered: Value = serde_json::from_str(
        &requests
            .iter()
            .find(|(path, _)| path == "/register")
            .unwrap()
            .1,
    )
    .unwrap();
    assert_eq!(registered["application_type"], "native");
    assert_eq!(registered["response_types"], json!(["code"]));
    assert_eq!(
        registered["grant_types"],
        json!(["authorization_code", DEVICE_CODE_GRANT, "refresh_token"])
    );
    let auth = &requests
        .iter()
        .find(|(path, _)| path.starts_with("/authorize?"))
        .unwrap()
        .0;
    let params: HashMap<_, _> = reqwest::Url::parse(&format!("{}{auth}", stub.base))
        .unwrap()
        .query_pairs()
        .into_owned()
        .collect();
    assert!(!params.contains_key("response_mode"));
    assert_eq!(params["scope"], "files:read offline_access");
    assert_eq!(params["client_id"], "registered-client");
    assert_eq!(params["code_challenge_method"], "S256");
    let form = &requests
        .iter()
        .find(|(path, _)| path == "/token")
        .unwrap()
        .1;
    assert!(
        form.contains("grant_type=authorization_code")
            && form.contains("code_verifier=")
            && form.contains("client_id=registered-client"),
        "{form}"
    );
    let exchange: HashMap<_, _> = reqwest::Url::parse(&format!("http://test/?{form}"))
        .unwrap()
        .query_pairs()
        .into_owned()
        .collect();
    use base64::Engine as _;
    use sha2::{Digest, Sha256};
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(Sha256::digest(exchange["code_verifier"].as_bytes()));
    assert_eq!(params["code_challenge"], challenge);
    assert_eq!(exchange["redirect_uri"], params["redirect_uri"]);
    assert_eq!(registered["redirect_uris"][0], params["redirect_uri"]);
    assert_eq!(registered["token_endpoint_auth_method"], "none");
    assert_eq!(exchange["resource"], format!("{}/mcp", stub.base));
    let dir = tempfile::tempdir().unwrap();
    save_oauth_token_to_dir(dir.path(), "stub", &token).unwrap();
    let mut loaded = load_oauth_token_from_dir(dir.path(), "stub").unwrap();
    assert_eq!(loaded, token);
    loaded.expires_at = Some(0);
    let refreshed = refresh_access_token(
        &reqwest::Client::new(),
        loaded.token_endpoint.as_deref().unwrap(),
        loaded.client_id.as_deref(),
        loaded.refresh_token.as_deref().unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(refreshed.refresh_token.as_deref(), Some("refresh"));
    let requests = stub.requests.lock().unwrap();
    let refresh = &requests
        .iter()
        .rev()
        .find(|(path, _)| path == "/token")
        .unwrap()
        .1;
    assert!(
        refresh.contains("grant_type=refresh_token")
            && refresh.contains("refresh_token=refresh")
            && refresh.contains("client_id=registered-client"),
        "{refresh}"
    );
}

#[tokio::test]
async fn oauth_pre_registered_client_skips_registration() {
    let stub = oauth_stub(ServerMode::RejectRegistration).await;
    let client = oauth_client(&stub);
    if let Transport::Remote { state } = &client.transport {
        state.lock().await.client_id = Some("pre-registered".into());
    }
    let browser = |url: &str| {
        let url = url.to_string();
        tokio::spawn(async move {
            reqwest::get(url).await.unwrap();
        });
    };
    let token = tokio::time::timeout(
        Duration::from_secs(2),
        client.run_oauth_flow_with(&reqwest::Client::new(), &browser, None),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(token.client_id.as_deref(), Some("pre-registered"));
    assert!(
        !stub
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|(path, _)| path == "/register")
    );
}

#[tokio::test]
async fn oauth_no_browser_still_discovers_logs_in_and_reuses_persisted_tokens() {
    // Other OAuth tests use an injected URL handler; no other test mutates
    // browser launch configuration. Preserve a caller's original setting.
    struct RestoreBrowser(Option<std::ffi::OsString>);
    impl Drop for RestoreBrowser {
        fn drop(&mut self) {
            unsafe {
                match &self.0 {
                    Some(value) => std::env::set_var("RUSTCODE_MCP_NO_BROWSER", value),
                    None => std::env::remove_var("RUSTCODE_MCP_NO_BROWSER"),
                }
            }
        }
    }
    let _restore = RestoreBrowser(std::env::var_os("RUSTCODE_MCP_NO_BROWSER"));
    unsafe {
        std::env::set_var("RUSTCODE_MCP_NO_BROWSER", "1");
    }
    let stub = oauth_stub(ServerMode::Downgrade).await;
    let client = oauth_client(&stub);
    let manual_browser = |url: &str| {
        assert!(browser_launch_suppressed());
        let url = url.to_string();
        tokio::spawn(async move {
            reqwest::get(url).await.unwrap();
        });
    };
    assert!(
        tokio::time::timeout(
            Duration::from_secs(2),
            client.ensure_remote_auth_with(&manual_browser, None)
        )
        .await
        .unwrap()
        .unwrap()
    );
    let token = load_oauth_token("stub-oauth").expect("persisted login token");
    assert_eq!(token.refresh_token.as_deref(), Some("refresh"));
    let restarted = oauth_client(&stub);
    if let Transport::Remote { state } = &restarted.transport {
        state.lock().await.auth = Some(token.clone());
    }
    let previous = stub.requests.lock().unwrap().len();
    assert!(
        restarted
            .ensure_remote_auth_with(&|_| panic!("usable persisted token must not log in"), None)
            .await
            .unwrap()
    );
    assert_eq!(stub.requests.lock().unwrap().len(), previous);
    if let Transport::Remote { state } = &restarted.transport {
        state.lock().await.auth.as_mut().unwrap().expires_at = Some(0);
    }
    assert!(
        restarted
            .ensure_remote_auth_with(&|_| panic!("refresh must not log in"), None)
            .await
            .unwrap()
    );
    let requests = stub.requests.lock().unwrap();
    assert_eq!(requests.len(), previous + 1);
    assert_eq!(requests.last().unwrap().0, "/token");
    assert!(
        requests
            .last()
            .unwrap()
            .1
            .contains("grant_type=refresh_token")
    );
}

#[tokio::test]
async fn oauth_registration_missing_id_or_usable_grants_fails_before_login() {
    for (mode, expected) in [
        (ServerMode::MissingClientId, "returned no client_id"),
        (ServerMode::UnsupportedClientGrants, "no usable"),
    ] {
        let stub = oauth_stub(mode).await;
        let error = oauth_client(&stub)
            .run_oauth_flow_with(
                &reqwest::Client::new(),
                &|_| panic!("invalid registration must not log in"),
                None,
            )
            .await
            .unwrap_err();
        assert!(error.contains(expected), "{error}");
        assert!(
            !stub
                .requests
                .lock()
                .unwrap()
                .iter()
                .any(|(path, _)| path == "/device")
        );
    }
}

#[tokio::test]
async fn oauth_device_only_registration_has_no_code_response_type_or_fallback() {
    let stub = oauth_stub(ServerMode::DeviceOnly).await;
    let error = oauth_client(&stub)
        .run_oauth_flow_with(
            &reqwest::Client::new(),
            &|_| panic!("ungranted code fallback must not run"),
            None,
        )
        .await
        .unwrap_err();
    assert!(error.contains("client cannot use device grant"), "{error}");
    let requests = stub.requests.lock().unwrap();
    let registration: Value = serde_json::from_str(
        &requests
            .iter()
            .find(|(path, _)| path == "/register")
            .unwrap()
            .1,
    )
    .unwrap();
    assert_eq!(
        registration["grant_types"],
        json!([DEVICE_CODE_GRANT, "refresh_token"])
    );
    assert_eq!(registration["response_types"], json!([]));
    assert_eq!(registration["application_type"], "native");
}

#[tokio::test]
async fn oauth_device_denial_is_terminal() {
    let stub = oauth_stub(ServerMode::Denied).await;
    let error = oauth_client(&stub)
        .run_oauth_flow_with(&reqwest::Client::new(), &|_| {}, None)
        .await
        .unwrap_err();
    assert!(error.contains("device login was denied"), "{error}");
    assert!(
        !stub
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|(path, _)| path.starts_with("/authorize"))
    );
}

#[tokio::test]
async fn oauth_callback_wrong_state_never_exchanges_a_token() {
    let stub = oauth_stub(ServerMode::WrongState).await;
    let browser = |url: &str| {
        let url = url.to_string();
        tokio::spawn(async move {
            reqwest::get(url).await.unwrap();
        });
    };
    let error = oauth_client(&stub)
        .run_oauth_flow_with(&reqwest::Client::new(), &browser, None)
        .await
        .unwrap_err();
    assert!(error.contains("state mismatch"), "{error}");
    assert!(
        !stub
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|(path, _)| path == "/token")
    );
}

#[test]
fn oauth_nonsecret_client_id_round_trips_in_server_config() {
    let server: crate::config::McpServerConfig=serde_json::from_value(json!({"name":"pre-registered","url":"https://mcp.example","client_id":"https://client.example/metadata.json"})).unwrap();
    assert_eq!(
        serde_json::to_value(server).unwrap()["client_id"],
        "https://client.example/metadata.json"
    );
    let legacy: crate::config::McpServerConfig =
        serde_json::from_value(json!({"name":"legacy","url":"https://mcp.example"})).unwrap();
    assert!(legacy.client_id.is_none());
}

#[tokio::test]
async fn oauth_original_request_scope_challenge_takes_precedence_over_probe() {
    let stub = oauth_stub(ServerMode::Downgrade).await;
    let browser = |url: &str| {
        let url = url.to_string();
        tokio::spawn(async move {
            reqwest::get(url).await.unwrap();
        });
    };
    let challenge = format!(
        "Bearer resource_metadata=\"{}/resource\", scope=\"files:write\"",
        stub.base
    );
    let token = oauth_client(&stub)
        .run_oauth_flow_with(&reqwest::Client::new(), &browser, Some(&challenge))
        .await
        .unwrap();
    assert_eq!(token.access_token, "access");
    let requests = stub.requests.lock().unwrap();
    assert!(!requests.iter().any(|(path, _)| path == "/mcp"));
    let auth = &requests
        .iter()
        .find(|(path, _)| path.starts_with("/authorize?"))
        .unwrap()
        .0;
    let params: HashMap<_, _> = reqwest::Url::parse(&format!("{}{auth}", stub.base))
        .unwrap()
        .query_pairs()
        .into_owned()
        .collect();
    assert_eq!(params["scope"], "files:write offline_access");
}
