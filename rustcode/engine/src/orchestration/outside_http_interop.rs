//! Wire-level tests: explicit temporary settings avoid process-global config/env races.
use super::*;
use crate::orchestration::tool_socket::CallAnswer;

struct FakeRunner;
impl outside::Runner for FakeRunner {
    fn run<'a>(
        &'a self,
        _: &'a str,
        _: &'a CancellationToken,
        tool: &'a str,
        args: &'a Value,
    ) -> outside::BoxFuture<'a, CallAnswer> {
        Box::pin(async move {
            CallAnswer {
                success: true,
                content: json!({"tool": tool, "session_id": args["session_id"]}).to_string(),
            }
        })
    }
    fn context<'a>(
        &'a self,
        _: &'a str,
        _: &'a CancellationToken,
    ) -> outside::BoxFuture<'a, Value> {
        Box::pin(async { json!({}) })
    }
}
struct Fixture {
    server: Arc<Server>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Fixture {
    async fn start(dir: &std::path::Path) -> Self {
        Self::start_with(dir, None).await
    }
    async fn start_with(dir: &std::path::Path, public: Option<&str>) -> Self {
        Self::start_web(dir, public, None).await
    }
    async fn start_web(
        dir: &std::path::Path,
        public: Option<&str>,
        web: Option<std::path::PathBuf>,
    ) -> Self {
        let listener = bind_configured(&dir.join(PORT_FILE), Duration::ZERO)
            .await
            .unwrap();
        let server = Arc::new(Server {
            base: format!("http://{}", listener.local_addr().unwrap()),
            public: public.map(|p| public_base(p).unwrap()),
            key: load_key(&dir.join(KEY_FILE)).unwrap(),
            sign_in_codes: dir.join("outside-codes.json"),
            passkeys: dir.join("owner-passkeys.json"),
            passkey_enrollments: dir.join("owner-passkey-enrollments.json"),
            web_sessions: dir.join("owner-web-sessions.json"),
            web,
            machine: "test-machine".to_owned(),
            config: dir.to_path_buf(),
            failures: Mutex::new(Vec::new()),
            settings: outside::Settings {
                grants: dir.join("outside-agents.json"),
                data: None,
                approval_wait: Duration::from_secs(1),
                needs_approver: false,
            },
            runner: Arc::new(FakeRunner),
            codes: Mutex::new(HashMap::new()),
            slots: Arc::new(tokio::sync::Semaphore::new(32)),
        });
        let state = server.clone();
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let state = state.clone();
                tokio::spawn(handle(stream, state, None));
            }
        });
        Self { server, task }
    }
    async fn request(
        &self,
        method: &str,
        target: &str,
        token: &str,
        body: &str,
    ) -> (u16, String, Value) {
        let authority = self.server.base.trim_start_matches("http://").to_owned();
        self.request_to(&authority, method, target, token, body)
            .await
    }
    /// The same request, naming `host` in its Host header (as a tunnel would).
    async fn request_to(
        &self,
        host: &str,
        method: &str,
        target: &str,
        token: &str,
        body: &str,
    ) -> (u16, String, Value) {
        tokio::time::timeout(Duration::from_secs(5), async {
            let authority = self.server.base.trim_start_matches("http://");
            let mut stream = TcpStream::connect(authority).await.unwrap();
            let content_type = if target == "/mcp" || target == "/register" { "application/json" } else { "application/x-www-form-urlencoded" };
            let request = format!("{method} {target} HTTP/1.1\r\nHost: {host}\r\nAuthorization: Bearer {token}\r\nContent-Type: {content_type}\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\n\r\n{body}", body.len());
            stream.write_all(request.as_bytes()).await.unwrap();
            let mut bytes = Vec::new();
            stream.read_to_end(&mut bytes).await.unwrap();
            let response = String::from_utf8_lossy(&bytes).into_owned();
            let (head, body) = response.split_once("\r\n\r\n").unwrap();
            // A page or an empty redirect is not JSON; keep it as text.
            let value = serde_json::from_str(body).unwrap_or_else(|_| Value::String(body.to_owned()));
            (head.split_whitespace().nth(1).unwrap().parse().unwrap(), head.to_owned(), value)
        }).await.expect("HTTP request stalled")
    }
    /// A request with exactly the headers given (no bearer token), as a
    /// browser sends one: `extra` is header lines, each ending in `\r\n`.
    async fn browser(
        &self,
        method: &str,
        target: &str,
        extra: &str,
        body: &str,
    ) -> (u16, String, String) {
        tokio::time::timeout(Duration::from_secs(5), async {
            let authority = self.server.base.trim_start_matches("http://");
            let mut stream = TcpStream::connect(authority).await.unwrap();
            let request = format!(
                "{method} {target} HTTP/1.1\r\nHost: rustcode.example.com\r\n{extra}Content-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(request.as_bytes()).await.unwrap();
            let mut bytes = Vec::new();
            stream.read_to_end(&mut bytes).await.unwrap();
            let response = String::from_utf8_lossy(&bytes).into_owned();
            let (head, body) = response.split_once("\r\n\r\n").unwrap();
            (
                head.split_whitespace().nth(1).unwrap().parse().unwrap(),
                head.to_owned(),
                body.to_owned(),
            )
        })
        .await
        .expect("HTTP request stalled")
    }
    async fn rpc(&self, token: &str, id: u32, method: &str, params: Value) -> (u16, String, Value) {
        self.request(
            "POST",
            "/mcp",
            token,
            &json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params}).to_string(),
        )
        .await
    }
    async fn code(&self, allow: bool) -> Option<String> {
        let client = format!("interop-{}", &secret()[..24]);
        let query = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs([
                ("response_type", "code"),
                ("client_id", &client),
                ("redirect_uri", "http://127.0.0.1:12345/callback"),
                ("scope", "read_only"),
                ("resource", &format!("{}/mcp", self.server.base)),
                ("state", "state with spaces"),
                ("code_challenge_method", "S256"),
                (
                    "code_challenge",
                    "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM",
                ),
            ])
            .finish();
        let approve = async {
            tokio::time::timeout(Duration::from_secs(3), async {
                loop {
                    if let Some(pending) = outside::pending_view()
                        .iter()
                        .find(|p| p["clientName"] == client)
                    {
                        assert!(
                            pending["summary"]
                                .as_str()
                                .unwrap()
                                .contains("127.0.0.1:12345")
                        );
                        assert!(outside::answer_pending(
                            pending["id"].as_str().unwrap(),
                            allow
                        ));
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
        };
        let target = format!("/authorize?{query}");
        let (response, ()) = tokio::join!(self.request("GET", &target, "", ""), approve);
        assert_eq!(response.0, 302);
        let location = response
            .1
            .lines()
            .find_map(|l| l.strip_prefix("Location: "))
            .unwrap();
        let url = url::Url::parse(location).unwrap();
        let pairs: HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(pairs["state"], "state with spaces");
        if !allow {
            assert_eq!(pairs["error"], "access_denied");
        }
        pairs
            .get("code")
            .map(|code| format!("code={code}&client_id={client}"))
    }
    async fn exchange(&self, code: &str, verifier: &str) -> (u16, String, Value) {
        let form = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs([
                ("grant_type", "authorization_code"),
                ("redirect_uri", "http://127.0.0.1:12345/callback"),
                ("resource", &format!("{}/mcp", self.server.base)),
                ("code_verifier", verifier),
            ])
            .finish();
        self.request("POST", "/token", "", &format!("{code}&{form}"))
            .await
    }
    async fn token(&self) -> String {
        let code = self.code(true).await.unwrap();
        let response = self
            .exchange(&code, "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk")
            .await;
        assert_eq!(response.0, 200);
        assert_eq!(response.2["expires_in"], 3600);
        assert_eq!(response.2["scope"], "read_only");
        assert_eq!(
            self.exchange(&code, "wrong").await.0,
            400,
            "code is single use"
        );
        response.2["access_token"].as_str().unwrap().into()
    }
}

#[tokio::test]
async fn outside_http_interop() {
    let dir = tempfile::tempdir().unwrap();
    // Seed a free remembered port so tests never compete for the user's default port.
    let seed = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    std::fs::write(
        dir.path().join(PORT_FILE),
        seed.local_addr().unwrap().port().to_string(),
    )
    .unwrap();
    drop(seed);
    let f = Fixture::start(dir.path()).await;
    let (status, head, _) = f.rpc("", 1, "initialize", json!({})).await;
    assert_eq!(status, 401);
    assert!(head.contains("WWW-Authenticate: Bearer resource_metadata="));
    let resource = f
        .request("GET", "/.well-known/oauth-protected-resource/mcp", "", "")
        .await;
    assert_eq!(resource.0, 200);
    assert_eq!(resource.2["resource"], format!("{}/mcp", f.server.base));
    assert_eq!(resource.2["authorization_servers"][0], f.server.base);
    let metadata = f
        .request("GET", "/.well-known/oauth-authorization-server", "", "")
        .await;
    assert_eq!(
        metadata.2["authorization_endpoint"],
        format!("{}/authorize", f.server.base)
    );
    assert_eq!(
        metadata.2["token_endpoint"],
        format!("{}/token", f.server.base)
    );
    assert_eq!(
        metadata.2["code_challenge_methods_supported"],
        json!(["S256"])
    );
    assert!(f.code(false).await.is_none());
    let wrong = f.code(true).await.unwrap();
    assert_eq!(
        f.exchange(&wrong, &"a".repeat(43)).await.2["error"],
        "invalid_grant"
    );
    let token = f.token().await;
    assert!(outside::authenticate_at(&f.server.settings.grants, &token).is_none());
    let init = f.rpc(&token, 2, "initialize", json!({"protocolVersion":"2025-03-26", "capabilities":{}, "clientInfo":{"name":"fixture","version":"1"}})).await;
    assert_eq!(init.0, 200);
    assert_eq!(init.2["result"]["serverInfo"]["name"], "rustcode");
    let listed = f.rpc(&token, 3, "tools/list", json!({})).await;
    assert!(
        listed.2["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["name"] == "read_thread"
                && t["inputSchema"]["required"] == json!(["thread_id"]))
    );
    let call = f
        .rpc(
            &token,
            4,
            "tools/call",
            json!({"name":"whoami","arguments":{}}),
        )
        .await;
    assert_eq!(call.2["result"]["isError"], false);
    for (tool, field) in [("read_thread", "thread_id"), ("read_session", "session_id")] {
        let reply = f
            .rpc(
                &token,
                12,
                "tools/call",
                json!({
                    "name": tool, "arguments": { (field): "thread-1" }
                }),
            )
            .await;
        assert_eq!(reply.2["result"]["isError"], false);
        let body: Value =
            serde_json::from_str(reply.2["result"]["content"][0]["text"].as_str().unwrap())
                .unwrap();
        assert_eq!(body, json!({"tool": "read_session", (field): "thread-1"}));
    }
    let denied = f
        .rpc(
            &token,
            5,
            "tools/call",
            json!({"name":"start_thread","arguments":{}}),
        )
        .await;
    assert_eq!(denied.2["result"]["isError"], true);
    assert!(
        denied.2["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("start_thread needs Delegate")
    );
    let (a, b) = tokio::join!(
        f.rpc(&token, 6, "tools/call", json!({"name":"whoami"})),
        f.rpc(&token, 7, "tools/call", json!({"name":"whoami"}))
    );
    assert_eq!(a.2["id"], 6);
    assert_eq!(b.2["id"], 7);
    assert_eq!(a.2["result"]["isError"], false);
    assert_eq!(b.2["result"]["isError"], false);
    let (_, stdio) = outside::create_at(
        &f.server.settings.grants,
        "stdio",
        "other",
        outside::Scope::ReadOnly,
        None,
    )
    .unwrap();
    assert_eq!(f.rpc(&stdio, 8, "tools/list", json!({})).await.0, 401);
    let grant =
        outside::authenticate_transport_at(&f.server.settings.grants, &token, true).unwrap();
    outside::revoke_at(&f.server.settings.grants, &grant.id).unwrap();
    assert_eq!(f.rpc(&token, 9, "tools/list", json!({})).await.0, 401);
    let expiring = f.token().await;
    assert_eq!(f.rpc(&expiring, 10, "initialize", json!({})).await.0, 200);
    let path = &f.server.settings.grants;
    let mut data: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    for client in data["clients"].as_array_mut().unwrap() {
        client["expires_at"] = json!(0);
    }
    std::fs::write(path, serde_json::to_vec(&data).unwrap()).unwrap();
    assert_eq!(f.rpc(&expiring, 10, "tools/list", json!({})).await.0, 401);
    let base = f.server.base.clone();
    f.task.abort();
    while !f.task.is_finished() {
        tokio::task::yield_now().await;
    }
    drop(f);
    // The full suite binds ports of its own: wait until the remembered one is free again, so the
    // restart is not pushed to the next port by another test holding it for a moment.
    let port: u16 = base.rsplit(':').next().unwrap().parse().unwrap();
    for _ in 0..200 {
        if TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
            .await
            .is_ok()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let restarted = Fixture::start(dir.path()).await;
    assert_eq!(restarted.server.base, base);
    let token = restarted.token().await;
    assert_eq!(
        restarted.rpc(&token, 11, "initialize", json!({})).await.0,
        200
    );
}

fn location(head: &str) -> HashMap<String, String> {
    let location = head
        .lines()
        .find_map(|l| l.strip_prefix("Location: "))
        .expect("a redirect");
    let url = url::Url::parse(location).unwrap();
    assert_eq!(url.host_str(), Some("chatgpt.com"), "{location}");
    url.query_pairs().into_owned().collect()
}

#[tokio::test]
async fn a_hosted_agent_registers_signs_in_with_a_code_and_refreshes() {
    let dir = tempfile::tempdir().unwrap();
    let seed = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    std::fs::write(
        dir.path().join(PORT_FILE),
        seed.local_addr().unwrap().port().to_string(),
    )
    .unwrap();
    drop(seed);
    let f = Fixture::start_with(dir.path(), Some("https://rustcode.example.com/")).await;
    let public = "rustcode.example.com";
    let base = "https://rustcode.example.com";

    // Only this computer's address and the configured host are answered.
    let other = f
        .request_to(
            "evil.example",
            "GET",
            "/.well-known/oauth-authorization-server",
            "",
            "",
        )
        .await;
    assert_eq!(other.0, 403);
    let metadata = f
        .request_to(
            public,
            "GET",
            "/.well-known/oauth-authorization-server",
            "",
            "",
        )
        .await;
    assert_eq!(metadata.2["issuer"], base);
    assert_eq!(
        metadata.2["registration_endpoint"],
        format!("{base}/register")
    );
    let local = f
        .request("GET", "/.well-known/oauth-authorization-server", "", "")
        .await;
    assert_eq!(
        local.2["issuer"], f.server.base,
        "loopback keeps its own issuer"
    );
    let unauthorized = f.request_to(public, "POST", "/mcp", "", "{}").await;
    assert_eq!(unauthorized.0, 401);
    assert!(unauthorized.1.contains(&format!(
        "resource_metadata=\"{base}/.well-known/oauth-protected-resource/mcp\""
    )));

    // Registration stores nothing; the id carries it, signed.
    let callback = "https://chatgpt.com/connector_platform_oauth_redirect";
    let registered = f
        .request_to(
            public,
            "POST",
            "/register",
            "",
            &json!({"client_name":"ChatGPT","redirect_uris":[callback]}).to_string(),
        )
        .await;
    assert_eq!(registered.0, 201, "{:?}", registered.2);
    let client = registered.2["client_id"].as_str().unwrap().to_owned();
    assert!(client.starts_with(CLIENT_PREFIX));
    let refused = f
        .request_to(
            public,
            "POST",
            "/register",
            "",
            &json!({"redirect_uris":["ftp://x"]}).to_string(),
        )
        .await;
    assert_eq!(refused.0, 400);

    let form_for = |client: &str, redirect: &str, resource: &str| {
        url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs([
                ("response_type", "code"),
                ("client_id", client),
                ("redirect_uri", redirect),
                ("scope", "delegate"),
                ("resource", resource),
                ("state", "s1"),
                ("code_challenge_method", "S256"),
                (
                    "code_challenge",
                    "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM",
                ),
            ])
            .finish()
    };
    let form = |client: &str, redirect: &str| form_for(client, redirect, &format!("{base}/mcp"));
    let query = form(&client, callback);
    let page = f
        .request_to(public, "GET", &format!("/authorize?{query}"), "", "")
        .await;
    assert_eq!(page.0, 200);
    let html = page.2.as_str().unwrap();
    assert!(
        html.contains("ChatGPT") && html.contains("chatgpt.com"),
        "{html}"
    );
    assert!(page.1.contains("frame-ancestors 'none'"));
    // A callback the client did not register, a tampered id, or an https
    // callback over loopback are refused without redirecting anywhere.
    let other_callback = form(&client, "https://evil.example/cb");
    assert_eq!(
        f.request_to(
            public,
            "GET",
            &format!("/authorize?{other_callback}"),
            "",
            ""
        )
        .await
        .0,
        400
    );
    let tampered = form(&format!("{client}x"), callback);
    assert_eq!(
        f.request_to(public, "GET", &format!("/authorize?{tampered}"), "", "")
            .await
            .0,
        400
    );
    let over_loopback = form_for(&client, callback, &format!("{}/mcp", f.server.base));
    assert_eq!(
        f.request("GET", &format!("/authorize?{over_loopback}"), "", "")
            .await
            .0,
        400
    );

    // A wrong code shows the page again; a code sets the ceiling.
    let wrong = f
        .request_to(
            public,
            "POST",
            "/authorize",
            "",
            &format!("{query}&decision=approve&code=AAAA-AAAA"),
        )
        .await;
    assert_eq!(wrong.0, 200);
    assert!(wrong.2.as_str().unwrap().contains("not valid"));
    let code = outside::create_sign_in_code_at(
        &f.server.sign_in_codes,
        outside::Scope::ReadOnly,
        Some(false),
    )
    .unwrap();
    let approved = f
        .request_to(
            public,
            "POST",
            "/authorize",
            "",
            &format!("{query}&decision=approve&code={}", code.to_lowercase()),
        )
        .await;
    assert_eq!(approved.0, 303);
    let pairs = location(&approved.1);
    assert_eq!(pairs["state"], "s1");
    assert_eq!(pairs["iss"], base);
    let again = f
        .request_to(
            public,
            "POST",
            "/authorize",
            "",
            &format!("{query}&decision=approve&code={code}"),
        )
        .await;
    assert!(
        again.2.as_str().unwrap().contains("not valid"),
        "a code works once"
    );
    let declined = f
        .request_to(
            public,
            "POST",
            "/authorize",
            "",
            &format!("{query}&decision=deny"),
        )
        .await;
    assert_eq!(location(&declined.1)["error"], "access_denied");

    let exchange = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs([
            ("grant_type", "authorization_code"),
            ("code", &pairs["code"]),
            ("client_id", &client),
            ("redirect_uri", callback),
            ("resource", &format!("{base}/mcp")),
            (
                "code_verifier",
                "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk",
            ),
        ])
        .finish();
    let tokens = f.request_to(public, "POST", "/token", "", &exchange).await;
    assert_eq!(tokens.0, 200, "{:?}", tokens.2);
    assert_eq!(
        tokens.2["scope"], "read_only",
        "the code's level caps the request"
    );
    let access = tokens.2["access_token"].as_str().unwrap().to_owned();
    let refresh = tokens.2["refresh_token"].as_str().unwrap().to_owned();
    let grant =
        outside::authenticate_transport_at(&f.server.settings.grants, &access, true).unwrap();
    assert_eq!(grant.name, "ChatGPT");
    assert!(!grant.ask);
    let init = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}).to_string();
    assert_eq!(
        f.request_to(public, "POST", "/mcp", &access, &init).await.0,
        200
    );

    // Refreshing rotates both tokens on the same grant; the old ones stop.
    let refresh_form = |token: &str, client: &str| {
        url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs([
                ("grant_type", "refresh_token"),
                ("refresh_token", token),
                ("client_id", client),
            ])
            .finish()
    };
    assert_eq!(
        f.request_to(
            public,
            "POST",
            "/token",
            "",
            &refresh_form(&refresh, "someone-else")
        )
        .await
        .0,
        400,
        "a refresh token is bound to its client"
    );
    let renewed = f
        .request_to(
            public,
            "POST",
            "/token",
            "",
            &refresh_form(&refresh, &client),
        )
        .await;
    assert_eq!(renewed.0, 200, "{:?}", renewed.2);
    let new_access = renewed.2["access_token"].as_str().unwrap();
    assert_ne!(new_access, access);
    assert_eq!(
        f.request_to(public, "POST", "/mcp", new_access, &init)
            .await
            .0,
        200
    );
    assert_eq!(
        f.request_to(public, "POST", "/mcp", &access, &init).await.0,
        401
    );
    assert_eq!(
        f.request_to(
            public,
            "POST",
            "/token",
            "",
            &refresh_form(&refresh, &client)
        )
        .await
        .0,
        400,
        "a used refresh token never works again"
    );
    assert_eq!(outside::list_at(&f.server.settings.grants).len(), 1);
}

#[tokio::test]
async fn a_loop_webhook_takes_only_a_post_from_the_public_host() {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let f = Fixture::start_with(dir.path(), Some("https://rustcode.example.com")).await;
    let public = "rustcode.example.com";
    assert_eq!(
        f.request_to(public, "GET", "/hooks/loop-1/secret", "", "")
            .await
            .0,
        405
    );
    assert_eq!(
        f.request_to("evil.example.com", "POST", "/hooks/loop-1/secret", "", "{}")
            .await
            .0,
        403
    );
}

#[test]
fn public_url_must_be_a_bare_https_origin() {
    assert_eq!(
        public_base("https://rustcode.example.com").unwrap(),
        "https://rustcode.example.com"
    );
    assert_eq!(
        public_base(" https://rustcode.example.com:8443/ ").unwrap(),
        "https://rustcode.example.com:8443"
    );
    for wrong in [
        "http://rustcode.example.com",
        "https://rustcode.example.com/mcp",
        "https://user@rustcode.example.com",
        "https://rustcode.example.com/?x=1",
        "rustcode.example.com",
    ] {
        assert!(public_base(wrong).is_err(), "{wrong}");
    }
}

#[test]
fn loopback_callbacks_match_on_any_port_and_others_exactly() {
    let presented = url::Url::parse("http://127.0.0.1:5555/callback").unwrap();
    assert!(redirect_matches(
        "http://127.0.0.1:1234/callback",
        &presented
    ));
    assert!(!redirect_matches("http://127.0.0.1:1234/other", &presented));
    let https = url::Url::parse("https://chatgpt.com/cb").unwrap();
    assert!(redirect_matches("https://chatgpt.com/cb", &https));
    assert!(!redirect_matches("https://chatgpt.com/cb2", &https));
    assert_eq!(
        requested_scope("openid delegate read_only"),
        Some(outside::Scope::Delegate)
    );
    assert_eq!(requested_scope(""), None);
}

#[tokio::test]
async fn a_hosted_agent_is_approved_in_the_app_after_the_words_match() {
    let _serial = super::outside_signin::reset_for_tests();
    let dir = tempfile::tempdir().unwrap();
    let f = Fixture::start_with(dir.path(), Some("https://rustcode.example.com")).await;
    let public = "rustcode.example.com";
    let base = "https://rustcode.example.com";
    let callback = "https://chatgpt.com/connector_platform_oauth_redirect";
    let registered = f
        .request_to(
            public,
            "POST",
            "/register",
            "",
            &json!({"client_name":"ChatGPT","redirect_uris":[callback]}).to_string(),
        )
        .await;
    let client = registered.2["client_id"].as_str().unwrap().to_owned();
    let query = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs([
            ("response_type", "code"),
            ("client_id", client.as_str()),
            ("redirect_uri", callback),
            ("scope", "read_only"),
            ("resource", &format!("{base}/mcp")),
            ("state", "s2"),
            ("code_challenge_method", "S256"),
            (
                "code_challenge",
                "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM",
            ),
        ])
        .finish();
    let wait_target = |head: &str| {
        head.lines()
            .find_map(|l| l.strip_prefix("Location: "))
            .expect("a redirect")
            .to_owned()
    };

    // The page offers the levels, the requested one chosen, and both ways in.
    let page = f
        .request_to(public, "GET", &format!("/authorize?{query}"), "", "")
        .await;
    let html = page.2.as_str().unwrap();
    assert!(html.contains(r#"value="read_only" checked"#), "{html}");
    assert!(html.contains("Approve in RustCode") && html.contains("test-machine"));
    let logo = f.request_to(public, "GET", "/logo.png", "", "").await;
    assert_eq!(logo.0, 200);
    assert!(logo.1.contains("image/png"));

    // Approving in the app: the page waits with the words the app shows.
    let started = f
        .request_to(
            public,
            "POST",
            "/authorize",
            "",
            &format!("{query}&access=manage&decision=app"),
        )
        .await;
    assert_eq!(started.0, 303, "{:?}", started.2);
    let wait = wait_target(&started.1);
    assert!(wait.starts_with("/authorize/wait?id="), "{wait}");
    let listed = super::outside_signin::view();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["clientName"], "ChatGPT");
    assert_eq!(listed[0]["destination"], "chatgpt.com");
    assert_eq!(listed[0]["scope"], "manage", "the level picked on the page");
    let waiting = f.request_to(public, "GET", &wait, "", "").await;
    assert_eq!(waiting.0, 200);
    let words = listed[0]["words"].as_str().unwrap();
    assert!(waiting.2.as_str().unwrap().contains(words));
    assert_eq!(
        f.request("GET", &wait, "", "").await.0,
        404,
        "the waiting page is only on the public address"
    );
    let forged = wait.replace("key=", "key=0");
    assert_eq!(f.request_to(public, "GET", &forged, "", "").await.0, 410);

    // The owner lowers the level and approves; the page hands the agent its code.
    let id = listed[0]["id"].as_str().unwrap();
    assert!(super::outside_signin::answer(
        id,
        true,
        Some(outside::Scope::Delegate),
        Some(false)
    ));
    let approved = f.request_to(public, "GET", &wait, "", "").await;
    assert_eq!(approved.0, 303);
    let pairs = location(&approved.1);
    assert_eq!(pairs["state"], "s2");
    let exchange = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs([
            ("grant_type", "authorization_code"),
            ("code", &pairs["code"]),
            ("client_id", &client),
            ("redirect_uri", callback),
            (
                "code_verifier",
                "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk",
            ),
        ])
        .finish();
    let tokens = f.request_to(public, "POST", "/token", "", &exchange).await;
    assert_eq!(tokens.0, 200, "{:?}", tokens.2);
    assert_eq!(tokens.2["scope"], "delegate", "the owner's level wins");
    assert_eq!(
        f.request_to(public, "GET", &wait, "", "").await.0,
        410,
        "an answered sign-in is spent"
    );

    // Declined in the app, or cancelled on the page: the agent hears no.
    let declined = f
        .request_to(
            public,
            "POST",
            "/authorize",
            "",
            &format!("{query}&decision=app"),
        )
        .await;
    let wait = wait_target(&declined.1);
    let id = super::outside_signin::view()[0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(super::outside_signin::answer(&id, false, None, None));
    let answer = f.request_to(public, "GET", &wait, "", "").await;
    assert_eq!(location(&answer.1)["error"], "access_denied");
    let cancelled = f
        .request_to(
            public,
            "POST",
            "/authorize",
            "",
            &format!("{query}&decision=app"),
        )
        .await;
    let wait = wait_target(&cancelled.1);
    let answer = f
        .request_to(public, "GET", &format!("{wait}&cancel=1"), "", "")
        .await;
    assert_eq!(location(&answer.1)["error"], "access_denied");
    assert!(super::outside_signin::view().is_empty());
}

/// One tunnel serves both audiences (#3955): `/ws` on the public host is the
/// remote gateway's WebSocket, relayed to its loopback ingress, with the
/// gateway's own browser-origin rule; any other host is refused, and without
/// a running gateway the device is told so.
#[tokio::test]
async fn ws_on_the_public_host_reaches_the_remote_gateway() {
    use crate::remote_gateway::{
        address::plan_listen,
        gateway::{Gateway, GatewayConfig, Limits},
        lifecycle::RemoteLifecycle,
        router::NoSessionsRouter,
    };
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::{client_async, tungstenite::Message};

    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let f = Fixture::start_with(dir.path(), Some("https://rustcode.example.com")).await;
    let authority = f.server.base.trim_start_matches("http://").to_owned();
    let upgrade = |host: &str, origin: Option<&str>| {
        let origin = origin.map_or(String::new(), |o| format!("Origin: {o}\r\n"));
        format!(
            "GET /ws HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n{origin}\r\n"
        )
    };
    let status_of = |request: String| {
        let authority = authority.clone();
        async move {
            tokio::time::timeout(Duration::from_secs(5), async {
                let mut stream = TcpStream::connect(&authority).await.unwrap();
                stream.write_all(request.as_bytes()).await.unwrap();
                let mut head = vec![0; 512];
                let n = stream.read(&mut head).await.unwrap();
                let text = String::from_utf8_lossy(&head[..n]).into_owned();
                text.split_whitespace()
                    .nth(1)
                    .unwrap()
                    .parse::<u16>()
                    .unwrap()
            })
            .await
            .expect("HTTP request stalled")
        }
    };

    // No gateway runs for this configuration yet.
    assert_eq!(status_of(upgrade("rustcode.example.com", None)).await, 503);
    // A host this listener does not answer for, and a plain GET.
    assert_eq!(status_of(upgrade("evil.example", None)).await, 403);
    assert_eq!(
        f.request_to("rustcode.example.com", "GET", "/ws", "", "")
            .await
            .0,
        426
    );

    let lifecycle = RemoteLifecycle::new(dir.path());
    let gateway = Gateway::bind(
        &lifecycle,
        GatewayConfig {
            replan: None,
            plan: plan_listen("127.0.0.1", None, &[]).unwrap(),
            port: 0,
            router: Arc::new(NoSessionsRouter),
            sessions: None,
            limits: Limits::default(),
        },
    )
    .await
    .unwrap();
    let shutdown = gateway.shutdown_token();
    let running = tokio::spawn(gateway.run());

    // A browser page cannot use the tunnel to reach the gateway.
    assert_eq!(
        status_of(upgrade(
            "rustcode.example.com",
            Some("https://rustcode.example.com")
        ))
        .await,
        403
    );

    // A native client gets the gateway's handshake answers through the relay.
    let stream = TcpStream::connect(&authority).await.unwrap();
    let (mut socket, _) = tokio::time::timeout(
        Duration::from_secs(5),
        client_async("ws://rustcode.example.com/ws", stream),
    )
    .await
    .unwrap()
    .expect("the relayed upgrade succeeds");
    socket
        .send(Message::text(
            json!({
                "type": "pair",
                "protocol_version": 1,
                "method": "credential",
                "secret": "not-a-real-credential",
                "device_name": "phone",
            })
            .to_string(),
        ))
        .await
        .unwrap();
    let answer = loop {
        match tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await
            .unwrap()
        {
            Some(Ok(Message::Text(text))) => break serde_json::from_str::<Value>(&text).unwrap(),
            Some(Ok(_)) => continue,
            other => panic!("expected a handshake answer, got {other:?}"),
        }
    };
    assert_eq!(answer["type"], "error", "{answer}");

    shutdown.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), running).await;
}

/// The owner's passkey end to end over HTTP (#3954): a link from the CLI opens
/// the page that adds it, the consent page then offers it with a script its
/// nonce allows, and its answer approves the sign-in at the chosen level,
/// once.
#[tokio::test]
async fn the_owners_passkey_is_added_from_a_link_and_approves_a_sign_in() {
    use crate::orchestration::outside_passkey::{self, fake::Authenticator};
    let dir = tempfile::tempdir().unwrap();
    let f = Fixture::start_with(dir.path(), Some("https://rustcode.example.com")).await;
    let public = "rustcode.example.com";
    let base = "https://rustcode.example.com";
    let encode = |pairs: &[(&str, &str)]| {
        url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(pairs)
            .finish()
    };

    // Without a passkey the consent page runs no script.
    let callback = "https://chatgpt.com/connector_platform_oauth_redirect";
    let registered = f
        .request_to(
            public,
            "POST",
            "/register",
            "",
            &json!({"client_name":"ChatGPT","redirect_uris":[callback]}).to_string(),
        )
        .await;
    let client = registered.2["client_id"].as_str().unwrap().to_owned();
    let mcp = format!("{base}/mcp");
    let query = encode(&[
        ("response_type", "code"),
        ("client_id", &client),
        ("redirect_uri", callback),
        ("scope", "delegate"),
        ("resource", &mcp),
        ("state", "s1"),
        ("code_challenge_method", "S256"),
        (
            "code_challenge",
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM",
        ),
    ]);
    let page = f
        .request_to(public, "GET", &format!("/authorize?{query}"), "", "")
        .await;
    assert!(!page.2.as_str().unwrap().contains("Approve with passkey"));
    assert!(!page.1.contains("script-src"));

    // The link works on the public address only, and once.
    let ticket = outside_passkey::create_enrollment_at(
        &f.server.passkey_enrollments,
        &f.server.passkeys,
        "Phone",
    )
    .unwrap();
    let local = f
        .request("GET", &format!("/owner/passkey?ticket={ticket}"), "", "")
        .await;
    assert_ne!(
        local.0, 200,
        "the loopback address shows no enrollment page"
    );
    let enroll = f
        .request_to(
            public,
            "GET",
            &format!("/owner/passkey?ticket={ticket}"),
            "",
            "",
        )
        .await;
    assert_eq!(enroll.0, 200);
    let html = enroll.2.as_str().unwrap().to_owned();
    assert!(html.contains("Add passkey"), "{html}");
    assert!(enroll.1.contains("script-src 'nonce-"), "{}", enroll.1);
    let offer = outside_passkey::enrollment_at(&f.server.passkey_enrollments, &ticket).unwrap();
    assert!(html.contains(&offer.challenge));
    let mut phone = Authenticator::new();
    let (client_data, attestation) = phone.create(&offer.challenge, base);
    let added = f
        .request_to(
            public,
            "POST",
            "/owner/passkey",
            "",
            &encode(&[
                ("ticket", &ticket),
                ("pk_client", &client_data),
                ("pk_attestation", &attestation),
            ]),
        )
        .await;
    assert!(
        added.2.as_str().unwrap().contains("Passkey added"),
        "{:?}",
        added.2
    );
    assert_eq!(outside_passkey::list_at(&f.server.passkeys).len(), 1);

    // Now the page offers it, and its answer approves Read-only when chosen.
    let page = f
        .request_to(public, "GET", &format!("/authorize?{query}"), "", "")
        .await;
    let html = page.2.as_str().unwrap().to_owned();
    assert!(html.contains("Approve with passkey"), "{html}");
    assert!(page.1.contains("script-src 'nonce-"));
    let challenge = html
        .split("challenge:b(\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("the page carries a challenge")
        .to_owned();
    let answer = phone.get(&challenge, base);
    let form = format!(
        "{query}&access=read_only&{}",
        encode(&[
            ("decision", "passkey"),
            ("pk_id", &answer[0]),
            ("pk_client", &answer[1]),
            ("pk_auth", &answer[2]),
            ("pk_sig", &answer[3]),
        ])
    );
    let approved = f.request_to(public, "POST", "/authorize", "", &form).await;
    assert_eq!(approved.0, 303, "{:?}", approved.2);
    let pairs = location(&approved.1);
    assert_eq!(pairs["state"], "s1");
    let code = pairs["code"].clone();
    let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    let tokens = f
        .request_to(
            public,
            "POST",
            "/token",
            "",
            &encode(&[
                ("grant_type", "authorization_code"),
                ("code", &code),
                ("client_id", &client),
                ("redirect_uri", callback),
                ("code_verifier", verifier),
                ("resource", &mcp),
            ]),
        )
        .await;
    assert_eq!(tokens.0, 200, "{:?}", tokens.2);
    assert_eq!(tokens.2["scope"], "read_only");

    // The same answer again is refused and shows the page with why.
    let replayed = f.request_to(public, "POST", "/authorize", "", &form).await;
    assert_eq!(replayed.0, 200);
    assert!(
        replayed.2.as_str().unwrap().contains("used already"),
        "{:?}",
        replayed.2
    );
}

/// The web app (#3958): passkey login, the cookie, the app's files, the
/// socket ticket, the refused cross-use with `/mcp`, and signing out.
#[tokio::test]
async fn the_owner_signs_in_to_the_web_app_with_a_passkey() {
    use crate::orchestration::outside_passkey::{self, fake::Authenticator};
    use crate::orchestration::outside_web;
    let dir = tempfile::tempdir().unwrap();
    let web = dir.path().join("web");
    std::fs::create_dir_all(web.join("assets")).unwrap();
    std::fs::write(web.join("index.html"), "<html>the app</html>").unwrap();
    std::fs::write(web.join("assets/index-abc.js"), "console.log(1)").unwrap();
    let base = "https://rustcode.example.com";
    let f = Fixture::start_web(dir.path(), Some(base), Some(web)).await;
    let origin = format!("Origin: {base}\r\n");
    let encode = |pairs: &[(&str, &str)]| {
        url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(pairs)
            .finish()
    };

    // Not signed in: the app sends a browser to the login page, and its
    // files are not served.
    let home = f.browser("GET", "/", "", "").await;
    assert_eq!(home.0, 303);
    assert!(home.1.contains("Location: /login"), "{}", home.1);
    assert_eq!(
        f.browser("GET", "/assets/index-abc.js", "", "").await.0,
        401
    );
    // Discovery stays the OAuth server's: no login redirect there.
    assert_eq!(
        f.browser("GET", "/.well-known/openid-configuration", "", "")
            .await
            .0,
        404
    );
    // The loopback address has no app.
    assert_ne!(f.request("GET", "/login", "", "").await.0, 200);

    // With no passkey the login page says how to add one, and runs nothing.
    let login = f.browser("GET", "/login", "", "").await;
    assert_eq!(login.0, 200);
    assert!(login.2.contains("Add a passkey first"), "{}", login.2);
    assert!(!login.1.contains("script-src"));

    let ticket = outside_passkey::create_enrollment_at(
        &f.server.passkey_enrollments,
        &f.server.passkeys,
        "Laptop",
    )
    .unwrap();
    let offer = outside_passkey::enrollment_at(&f.server.passkey_enrollments, &ticket).unwrap();
    let mut laptop = Authenticator::new();
    let (client_data, attestation) = laptop.create(&offer.challenge, base);
    let passkey = outside_passkey::register_at(
        &f.server.passkeys,
        &f.server.passkey_enrollments,
        base,
        &outside_passkey::Registration {
            ticket: &ticket,
            client_data: &client_data,
            attestation: &attestation,
        },
    )
    .unwrap();

    // The login page asks for it.
    let login = f.browser("GET", "/login", "", "").await;
    assert!(login.2.contains("Sign in with passkey"), "{}", login.2);
    assert!(login.1.contains("script-src 'nonce-"), "{}", login.1);
    let challenge = login
        .2
        .split("challenge:b(\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("the page carries a challenge")
        .to_owned();
    let answer = laptop.get(&challenge, base);
    let form = encode(&[
        ("pk_id", &answer[0]),
        ("pk_client", &answer[1]),
        ("pk_auth", &answer[2]),
        ("pk_sig", &answer[3]),
    ]);
    // A post from another origin, or with an outside agent's token, is refused.
    assert_eq!(
        f.browser("POST", "/login", "Origin: https://evil.example\r\n", &form)
            .await
            .0,
        403
    );
    assert_eq!(f.browser("POST", "/login", "", &form).await.0, 403);
    let bearer = format!("{origin}Authorization: Bearer rc_out_x\r\n");
    let refused = f.browser("POST", "/login", &bearer, &form).await;
    assert_eq!(refused.0, 401);
    assert!(refused.2.contains("wrong_audience"), "{}", refused.2);

    let signed_in = f
        .browser(
            "POST",
            "/login",
            &format!("{origin}User-Agent: Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) Version/18.0 Safari/605.1.15\r\n"),
            &form,
        )
        .await;
    assert_eq!(signed_in.0, 303, "{}", signed_in.2);
    let set = signed_in
        .1
        .lines()
        .find_map(|line| line.strip_prefix("Set-Cookie: "))
        .expect("a session cookie")
        .to_owned();
    for part in [
        "__Host-rustcode-session=",
        "HttpOnly",
        "Secure",
        "SameSite=Strict",
        "Path=/",
    ] {
        assert!(set.contains(part), "{set}");
    }
    let secret = set
        .split(';')
        .next()
        .and_then(|pair| pair.split_once('='))
        .unwrap()
        .1
        .to_owned();
    let cookie = format!("Cookie: {}={secret}\r\n", outside_web::COOKIE);
    let sessions = outside_web::list_at(&f.server.web_sessions);
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].name, "Safari on macOS");
    assert_eq!(sessions[0].passkey, passkey.id);
    // The same answer cannot sign in a second browser.
    let replayed = f.browser("POST", "/login", &origin, &form).await;
    assert_eq!(replayed.0, 401);

    // Signed in: the app and its files.
    let home = f.browser("GET", "/", &cookie, "").await;
    assert_eq!(home.0, 200);
    assert_eq!(home.2, "<html>the app</html>");
    assert!(home.1.contains("frame-ancestors 'none'"));
    let script = f.browser("GET", "/assets/index-abc.js", &cookie, "").await;
    assert_eq!(script.0, 200);
    assert!(script.1.contains("immutable"));
    assert!(script.1.contains("text/javascript"));
    assert_eq!(
        f.browser("GET", "/settings/outside", &cookie, "").await.2,
        "<html>the app</html>"
    );
    assert_eq!(
        f.browser("GET", "/../outside-agents.json", &cookie, "")
            .await
            .0,
        404
    );
    assert_eq!(f.browser("GET", "/login", &cookie, "").await.0, 303);

    // A ticket for the socket, from the page's own origin only.
    assert_eq!(f.browser("POST", "/app/ticket", &cookie, "").await.0, 403);
    assert_eq!(f.browser("POST", "/app/ticket", &origin, "").await.0, 401);
    let issued = f
        .browser("POST", "/app/ticket", &format!("{origin}{cookie}"), "")
        .await;
    assert_eq!(issued.0, 200, "{}", issued.2);
    let issued: Value = serde_json::from_str(&issued.2).unwrap();
    assert_eq!(issued["session"], sessions[0].id.as_str());
    let with_bearer = f
        .browser(
            "POST",
            "/app/ticket",
            &format!("{origin}{cookie}Authorization: Bearer rc_out_x\r\n"),
            "",
        )
        .await;
    assert_eq!(with_bearer.0, 401);
    assert!(with_bearer.2.contains("wrong_audience"));

    // The web session does not reach /mcp.
    let mcp = f
        .browser(
            "POST",
            "/mcp",
            &cookie,
            &json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}).to_string(),
        )
        .await;
    assert_eq!(mcp.0, 401);
    assert!(mcp.2.contains("wrong_audience"), "{}", mcp.2);

    // A socket needs the page's origin and an unspent ticket.
    let upgrade = |origin: &str| {
        format!(
            "Upgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n{origin}"
        )
    };
    let ticket = issued["ticket"].as_str().unwrap();
    let foreign = f
        .browser(
            "GET",
            &format!("/app/acp?ticket={ticket}"),
            &upgrade("Origin: https://evil.example\r\n"),
            "",
        )
        .await;
    assert_eq!(foreign.0, 403);
    assert_eq!(
        f.browser("GET", "/app/acp?ticket=made-up", &upgrade(&origin), "")
            .await
            .0,
        401
    );

    // Signing out in Settings ends the session at once.
    outside_web::remove_at(&f.server.web_sessions, &sessions[0].id).unwrap();
    assert_eq!(f.browser("GET", "/", &cookie, "").await.0, 303);
    let after = f
        .browser("POST", "/app/ticket", &format!("{origin}{cookie}"), "")
        .await;
    assert_eq!(after.0, 401);
    // The browser's own Sign out clears its cookie.
    let out = f
        .browser("POST", "/logout", &format!("{origin}{cookie}"), "")
        .await;
    assert_eq!(out.0, 303);
    assert!(out.1.contains("Max-Age=0"), "{}", out.1);
}

/// The web app's socket reaches this machine's server, and closes when the
/// browser is signed out.
#[cfg(unix)]
#[tokio::test]
async fn the_web_apps_socket_relays_acp_until_signed_out() {
    use crate::orchestration::outside_web;
    use futures_util::{SinkExt, StreamExt};
    use tokio::io::{AsyncBufReadExt, BufReader};
    use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};
    let dir = tempfile::tempdir().unwrap();
    let base = "https://rustcode.example.com";
    let f = Fixture::start_web(dir.path(), Some(base), None).await;

    // A server that answers `initialize`.
    let run = dir.path().join("run");
    std::fs::create_dir_all(&run).unwrap();
    let endpoint = run.join("acp-web.sock");
    let listener = tokio::net::UnixListener::bind(&endpoint).unwrap();
    std::fs::write(
        run.join("server.json"),
        serde_json::to_vec(&crate::server::ServerInfo::now(
            endpoint.to_string_lossy().into(),
        ))
        .unwrap(),
    )
    .unwrap();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let (read, mut write) = tokio::io::split(stream);
        let mut lines = BufReader::new(read).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let request: Value = serde_json::from_str(&line).unwrap();
            let answer = json!({"jsonrpc":"2.0","id":request["id"],"result":{"agent":"rustcode"}});
            write
                .write_all(format!("{answer}\n").as_bytes())
                .await
                .unwrap();
        }
    });

    let (session, _) =
        outside_web::create_at(&f.server.web_sessions, "Firefox on Linux", "pk").unwrap();
    let ticket = outside_web::new_ticket(&session);
    let mut request = format!("ws://rustcode.example.com/app/acp?ticket={ticket}")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("Origin", base.parse().unwrap());
    let stream = TcpStream::connect(f.server.base.trim_start_matches("http://"))
        .await
        .unwrap();
    let (mut socket, _) = tokio::time::timeout(
        Duration::from_secs(5),
        tokio_tungstenite::client_async(request, stream),
    )
    .await
    .unwrap()
    .unwrap();
    let opened = tokio::time::timeout(Duration::from_secs(15), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(opened.to_text().unwrap(), r#"{"type":"acp_opened"}"#);
    socket
        .send(Message::text(
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}).to_string(),
        ))
        .await
        .unwrap();
    let answer = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let answer: Value = serde_json::from_str(answer.to_text().unwrap()).unwrap();
    assert_eq!(answer["result"]["agent"], "rustcode");
    // A method a paired desktop may not call is refused here too.
    socket
        .send(Message::text(
            json!({"jsonrpc":"2.0","id":2,"method":"fs/write_text_file","params":{}}).to_string(),
        ))
        .await
        .unwrap();
    let refused = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        refused.to_text().unwrap().contains("not permitted"),
        "{refused}"
    );
    // The ticket worked once.
    let mut again = format!("ws://rustcode.example.com/app/acp?ticket={ticket}")
        .into_client_request()
        .unwrap();
    again.headers_mut().insert("Origin", base.parse().unwrap());
    let stream = TcpStream::connect(f.server.base.trim_start_matches("http://"))
        .await
        .unwrap();
    assert!(
        tokio_tungstenite::client_async(again, stream)
            .await
            .is_err()
    );

    // Signed out: the open socket closes with why.
    outside_web::remove_at(&f.server.web_sessions, &session).unwrap();
    let closed = loop {
        match tokio::time::timeout(Duration::from_secs(15), socket.next())
            .await
            .expect("the socket stayed open after sign-out")
        {
            Some(Ok(Message::Close(frame))) => break frame,
            Some(Ok(_)) => continue,
            _ => break None,
        }
    };
    assert_eq!(closed.unwrap().reason.as_str(), "signed out");
}
