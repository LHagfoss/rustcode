//! Wire-level tests: explicit temporary settings avoid process-global config/env races.
use super::*;
use crate::orchestration::hub::CallAnswer;

struct FakeRunner;
impl outside::Runner for FakeRunner {
    fn run<'a>(
        &'a self,
        _: &'a str,
        _: &'a CancellationToken,
        _: &'a str,
        _: &'a Value,
    ) -> outside::BoxFuture<'a, CallAnswer> {
        Box::pin(async {
            CallAnswer {
                success: true,
                content: "fixture result".into(),
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
        let listener = bind_configured(&dir.join(PORT_FILE)).await.unwrap();
        let server = Arc::new(Server {
            base: format!("http://{}", listener.local_addr().unwrap()),
            public: public.map(|p| public_base(p).unwrap()),
            key: load_key(&dir.join(KEY_FILE)).unwrap(),
            sign_in_codes: dir.join("outside-codes.json"),
            failures: Mutex::new(Vec::new()),
            settings: outside::Settings {
                grants: dir.join("outside-agents.json"),
                data: None,
                approval_wait: Duration::from_secs(1),
            },
            runner: Arc::new(FakeRunner),
            codes: Mutex::new(HashMap::new()),
            slots: Arc::new(tokio::sync::Semaphore::new(32)),
        });
        let state = server.clone();
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let state = state.clone();
                tokio::spawn(serve(stream, state));
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
            let response = String::from_utf8(bytes).unwrap();
            let (head, body) = response.split_once("\r\n\r\n").unwrap();
            // A page or an empty redirect is not JSON; keep it as text.
            let value = serde_json::from_str(body).unwrap_or_else(|_| Value::String(body.to_owned()));
            (head.split_whitespace().nth(1).unwrap().parse().unwrap(), head.to_owned(), value)
        }).await.expect("HTTP request stalled")
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
            .any(|t| t["name"] == "whoami")
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
    let denied = f
        .rpc(
            &token,
            5,
            "tools/call",
            json!({"name":"start_session","arguments":{}}),
        )
        .await;
    assert_eq!(denied.2["result"]["isError"], true);
    assert!(
        denied.2["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("needs Delegate")
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
