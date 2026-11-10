//! The whole path from this crate against the real gateway: `connect_request`,
//! the owner accepting on the machine, `pair` with the code, `authenticate`
//! on a fresh socket, `acp_open`, and one ACP round trip (`initialize`,
//! `session/list`).
//!
//! The gateway, its session hub and its control socket are the ones that
//! ship; the ACP server behind the gateway is a stub, because what is under
//! test here is the client and the relay, not a model turn. The ACP half needs
//! Unix sockets, which the engine's server listens on; the pairing half runs
//! on every platform.

use std::path::Path;
use std::time::Duration;

use rustcode::remote_gateway::address::plan_listen;
use rustcode::remote_gateway::gateway::{Gateway, GatewayConfig, Limits};
use rustcode::remote_gateway::hub::{HubLimits, SessionHub};
use rustcode::remote_gateway::lifecycle::RemoteLifecycle;
use rustcode_client::machine::machine_from_pairing;
use rustcode_client::{
    Access, ClientKind, Connector, Machine, MachineRegistry, Paired, Route, RouteKind, Secret,
    TokenStore, transport,
};

const WAIT: Duration = Duration::from_secs(15);

struct Host {
    dir: tempfile::TempDir,
    lifecycle: RemoteLifecycle,
    shutdown: tokio_util::sync::CancellationToken,
    task: tokio::task::JoinHandle<anyhow::Result<()>>,
    addr: std::net::SocketAddr,
}

impl Host {
    /// A gateway on a loopback port in its own temporary configuration
    /// directory, exactly as `remote_gateway/e2e_tests.rs` starts one.
    async fn start() -> Self {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let lifecycle = RemoteLifecycle::new(dir.path());
        let hub = SessionHub::start(HubLimits::default());
        let gateway = Gateway::bind(
            &lifecycle,
            GatewayConfig {
                replan: None,
                plan: plan_listen("127.0.0.1", None, &[]).unwrap(),
                port: 0,
                router: hub.clone(),
                sessions: Some(hub),
                limits: Limits::default(),
            },
        )
        .await
        .expect("the gateway binds");
        let addr = gateway.local_addr();
        let shutdown = gateway.shutdown_token();
        let task = tokio::spawn(gateway.run());
        Self {
            dir,
            lifecycle,
            shutdown,
            task,
            addr,
        }
    }

    fn address(&self) -> String {
        self.addr.to_string()
    }

    fn config_dir(&self) -> &Path {
        self.dir.path()
    }

    async fn stop(self) {
        self.shutdown.cancel();
        tokio::time::timeout(WAIT, self.task)
            .await
            .expect("the gateway stops")
            .expect("the gateway task does not panic")
            .expect("the gateway shuts down cleanly");
    }

    /// Accept the one open connection request the way the machine's owner
    /// does, and return the code it shows.
    async fn accept_connect_request(&self) -> String {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let status = self.lifecycle.status().await.unwrap().expect("running");
            if let Some(request) = status.connect_requests.first() {
                let answer = self
                    .lifecycle
                    .answer_connect_request(&request.id, true)
                    .await
                    .expect("the owner accepts");
                assert!(answer.accepted);
                return answer
                    .code
                    .expect("an accepted request has a code")
                    .expose()
                    .to_owned();
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "the connection request never reached the gateway"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

#[tokio::test]
async fn a_machine_is_asked_paired_and_authenticated_then_speaks_acp() {
    let host = Host::start().await;
    let config = tempfile::tempdir_in("/tmp").unwrap();
    let connector = Connector::new(config.path(), ClientKind::Desktop).unwrap();

    // 1. connect_request: the request is in front of the machine's owner.
    let ask = connector
        .ask(&host.address())
        .await
        .expect("the request is taken");
    assert_eq!(ask.request_id().len(), 16);
    let code = host.accept_connect_request().await;

    // 2. connect_approved, then pair with the code the owner read off.
    let approved = tokio::time::timeout(WAIT, ask.wait(WAIT))
        .await
        .expect("the owner answers in time")
        .expect("the answer arrives")
        .expect("the request was approved");
    assert_eq!(approved.code_length(), 8);
    let paired = approved.pair(&code).await.expect("the code is accepted");
    assert_eq!(paired.session.welcome().gateway_id.len(), 32);
    assert_eq!(paired.session.welcome().device_id.len(), 16);
    assert!(paired.secret.expose().len() >= 32, "a real device token");

    // 3. The machine is saved and its token kept; this is what `machines add`
    // does with the same answer.
    let mut registry = MachineRegistry::load(config.path()).unwrap();
    let machine = machine_from_pairing(paired.session.welcome(), Route::new(host.address()), None);
    registry.insert(machine.clone()).unwrap();
    let tokens = TokenStore::in_file(config.path()).unwrap();
    tokens.set(&machine.host_id, &paired.secret).unwrap();
    drop(paired);

    // 4. authenticate on a fresh socket, as a later `machines list` does.
    let (session, route) = connector
        .connect(&machine, &tokens)
        .await
        .expect("the saved token authenticates");
    assert_eq!(route.address, host.address());
    assert_eq!(session.welcome().gateway_id, machine.host_id);
    assert_eq!(session.welcome().device_id, machine.device_id);

    // 5. acp_open, then one ACP round trip. Unix only: the engine's server
    // listens on a Unix socket, and Windows uses a named pipe.
    #[cfg(unix)]
    {
        let server_task = acp_stub(host.config_dir()).await;
        let mut channel = session.acp().await.expect("ACP access is granted");
        let answer = channel
            .request(1, "initialize", serde_json::json!({"protocolVersion": 1}))
            .await
            .expect("initialize is answered");
        assert_eq!(answer["result"]["protocolVersion"], 1);
        let titles = rustcode_client::machine::thread_titles(&mut channel)
            .await
            .expect("session/list is answered");
        assert_eq!(titles, ["Fix the composer focus"]);
        // Close this side first: the stub ends when the relay does.
        drop(channel);
        server_task.await.expect("the stub server finishes");
    }
    #[cfg(not(unix))]
    drop(session);

    // The machine survives a reload with its route and device record.
    let reloaded = MachineRegistry::load(config.path()).unwrap();
    let saved = reloaded.get(&machine.host_id).expect("saved");
    assert_eq!(saved.device_id, machine.device_id);
    assert_eq!(saved.routes, machine.routes);

    // 6. A token that was revoked on the machine is refused, and says so.
    tokens
        .set(
            &machine.host_id,
            &rustcode_client::Secret::new("not-the-token"),
        )
        .unwrap();
    let error = connector
        .connect(&machine, &tokens)
        .await
        .expect_err("a wrong token is refused");
    assert!(
        format!("{error:#}").contains("unauthorized") || format!("{error:#}").contains("refused"),
        "{error:#}"
    );

    host.stop().await;
}

/// A stub ACP server the gateway relays to, described in `run/server.json`
/// the way the engine's server describes itself. The returned task is the
/// server: it ends when the channel closes.
#[cfg(unix)]
async fn acp_stub(config_dir: &Path) -> tokio::task::JoinHandle<()> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let run = config_dir.join("run");
    std::fs::create_dir_all(&run).unwrap();
    let endpoint = run.join("stub.sock");
    let listener = tokio::net::UnixListener::bind(&endpoint).unwrap();
    std::fs::write(
        run.join("server.json"),
        serde_json::to_vec(&serde_json::json!({
            "pid": std::process::id(),
            "version": "test",
            "endpoint": endpoint.to_string_lossy(),
            "started_at": 0,
        }))
        .unwrap(),
    )
    .unwrap();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let (read, mut write) = stream.into_split();
        let mut lines = BufReader::new(read).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let frame: serde_json::Value = serde_json::from_str(&line).unwrap();
            let answer = match frame["method"].as_str() {
                Some("initialize") => serde_json::json!({
                    "jsonrpc": "2.0", "id": frame["id"],
                    "result": {"protocolVersion": 1, "agentCapabilities": {}}
                }),
                Some("session/list") => serde_json::json!({
                    "jsonrpc": "2.0", "id": frame["id"],
                    "result": {"sessions": [{
                        "sessionId": "0190f000-0000-7000-8000-000000000001",
                        "cwd": "/tmp",
                        "title": "Fix the composer focus"
                    }]}
                }),
                other => panic!("unexpected ACP method {other:?}"),
            };
            write
                .write_all(format!("{answer}\n").as_bytes())
                .await
                .unwrap();
        }
    })
}

/// `rustcode machines list --threads` reaches the same read through
/// [`Connector::thread_titles`]; this checks the wiring without a second
/// gateway by using the pieces the test above already built.
#[cfg(unix)]
#[tokio::test]
async fn thread_titles_read_the_machines_threads() {
    let host = Host::start().await;
    let config = tempfile::tempdir_in("/tmp").unwrap();
    let connector = Connector::new(config.path(), ClientKind::Desktop).unwrap();
    let ask = connector.ask(&host.address()).await.unwrap();
    let code = host.accept_connect_request().await;
    let approved = ask.wait(WAIT).await.unwrap().unwrap();
    let paired = approved.pair(&code).await.unwrap();
    let mut registry = MachineRegistry::load(config.path()).unwrap();
    let machine = machine_from_pairing(paired.session.welcome(), Route::new(host.address()), None);
    registry.insert(machine.clone()).unwrap();
    let tokens = TokenStore::in_file(config.path()).unwrap();
    tokens.set(&machine.host_id, &paired.secret).unwrap();
    drop(paired);
    let server = acp_stub(host.config_dir()).await;
    let titles = connector.thread_titles(&machine, &tokens).await.unwrap();
    assert_eq!(titles, ["Fix the composer focus"]);
    server.await.unwrap();
    host.stop().await;
}

/// A route that does not answer as the machine we saved is refused, not
/// trusted: the machine is keyed by `host_id`, not by address.
#[tokio::test]
async fn another_machines_address_is_not_taken_for_this_one() {
    let host = Host::start().await;
    let config = tempfile::tempdir_in("/tmp").unwrap();
    let connector = Connector::new(config.path(), ClientKind::Desktop).unwrap();
    let ask = connector.ask(&host.address()).await.unwrap();
    let code = host.accept_connect_request().await;
    let paired = ask
        .wait(WAIT)
        .await
        .unwrap()
        .unwrap()
        .pair(&code)
        .await
        .unwrap();
    let mut machine =
        machine_from_pairing(paired.session.welcome(), Route::new(host.address()), None);
    let tokens = TokenStore::in_file(config.path()).unwrap();
    tokens.set(&machine.host_id, &paired.secret).unwrap();
    // The same address saved under another machine's identity, with the
    // token this machine issued.
    machine.host_id = "gw-somewhere-else".into();
    tokens.set(&machine.host_id, &paired.secret).unwrap();
    drop(paired);
    let error = connector
        .connect(&machine, &tokens)
        .await
        .expect_err("a mismatched machine is refused");
    assert!(format!("{error:#}").contains("answers as"), "{error:#}");
    host.stop().await;
}

/// A route that would carry the token in the clear is skipped, not dialled:
/// the LAN route behind it is used instead. `203.0.113.9` (TEST-NET-3) answers
/// nothing, so trying it would cost that route's timeout.
#[tokio::test]
async fn a_public_route_is_skipped_for_a_lan_one() {
    let host = Host::start().await;
    let config = tempfile::tempdir_in("/tmp").unwrap();
    let connector = Connector::new(config.path(), ClientKind::Desktop).unwrap();
    let ask = connector.ask(&host.address()).await.unwrap();
    let code = host.accept_connect_request().await;
    let paired = ask
        .wait(WAIT)
        .await
        .unwrap()
        .unwrap()
        .pair(&code)
        .await
        .unwrap();
    let mut machine =
        machine_from_pairing(paired.session.welcome(), Route::new(host.address()), None);
    machine
        .routes
        .insert(0, Route::with_kind(RouteKind::Public, "203.0.113.9:17879"));
    let tokens = TokenStore::in_file(config.path()).unwrap();
    tokens.set(&machine.host_id, &paired.secret).unwrap();
    drop(paired);
    let started = std::time::Instant::now();
    let (session, route) = connector
        .connect(&machine, &tokens)
        .await
        .expect("the LAN route is used");
    assert_eq!(route.address, host.address());
    assert_eq!(route.kind, RouteKind::Lan);
    assert_eq!(session.welcome().gateway_id, machine.host_id);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "the public route was dialled: {:?}",
        started.elapsed()
    );
    host.stop().await;
}

/// A saved machine without a token says to pair again instead of dialling.
#[tokio::test]
async fn a_machine_without_a_token_asks_for_a_new_pairing() {
    let host = Host::start().await;
    let config = tempfile::tempdir_in("/tmp").unwrap();
    let connector = Connector::new(config.path(), ClientKind::Cli).unwrap();
    let machine = rustcode_client::Machine::new(
        "gw-0123456789abcdef",
        "Studio",
        "macos",
        "0011223344556677",
        Route::new(host.address()),
        chrono::Utc::now(),
    );
    let error = connector
        .connect(&machine, &TokenStore::in_file(config.path()).unwrap())
        .await
        .expect_err("no token, no connection");
    assert!(
        format!("{error:#}").contains("pair with it again"),
        "{error:#}"
    );
    host.stop().await;
}

/// The transport refuses a TLS address with a reason rather than hanging.
#[test]
fn a_wss_address_is_refused() {
    let error = transport::websocket_url("wss://studio.local:17879").unwrap_err();
    assert!(
        format!("{error:#}").contains("no TLS listener"),
        "{error:#}"
    );
}

/// Pair this client with the host the way the app does: ask, take the code the
/// owner reads off the machine, then keep what the pairing handed out.
async fn pair(
    host: &Host,
    connector: &Connector,
    config_dir: &Path,
) -> (Machine, TokenStore, Paired) {
    let ask = connector
        .ask(&host.address())
        .await
        .expect("the request is taken");
    let code = host.accept_connect_request().await;
    let approved = ask
        .wait(WAIT)
        .await
        .expect("the owner answers in time")
        .expect("the request was approved");
    let paired = approved.pair(&code).await.expect("the code is accepted");
    let machine = machine_from_pairing(paired.session.welcome(), Route::new(host.address()), None);
    let tokens = TokenStore::in_file(config_dir).unwrap();
    paired
        .keep(&tokens, &machine.host_id)
        .expect("the pairing tokens are stored");
    (machine, tokens, paired)
}

/// The rotating pairing, end to end against the real gateway: the access token
/// reconnects without rotating, an expired access token and one the machine
/// answers `access_expired` for both fall back to the refresh token, every
/// rotation is stored before it is acknowledged, and the refresh token the
/// pairing issued is a replay afterwards.
#[tokio::test]
async fn a_rotating_pairing_reconnects_with_access_then_rotates() {
    let host = Host::start().await;
    let config = tempfile::tempdir_in("/tmp").unwrap();
    let connector = Connector::new(config.path(), ClientKind::Desktop).unwrap();
    let (machine, tokens, paired) = pair(&host, &connector, config.path()).await;

    // The client asked for rotation, so it got a refresh token and a
    // short-lived access token instead of the static token, and no 30-day
    // expiry: the pairing lasts until it is revoked.
    assert!(
        paired.session.welcome().expires_at.is_none(),
        "a rotating pairing never expires"
    );
    let access = paired.access().expect("an access token comes with it");
    let first_access = access.token().expose().to_owned();
    let first_refresh = tokens
        .get(&machine.host_id)
        .unwrap()
        .expect("the refresh token is stored durably");
    assert_eq!(first_refresh.expose(), paired.secret.expose());
    assert!(host.lifecycle.devices().unwrap()[0].rotates());

    // 1. Inside the access token's 15 minutes, a reconnect presents it and
    //    rotates nothing.
    let (session, _) = connector
        .connect(&machine, &tokens)
        .await
        .expect("the access token reconnects");
    assert_eq!(
        tokens.get(&machine.host_id).unwrap().unwrap().expose(),
        first_refresh.expose(),
        "an access-token reconnect replaces no refresh token"
    );
    assert_eq!(
        session.access().expect("still valid").token().expose(),
        first_access
    );
    drop(session);

    // 2. An access token whose expiry has passed is not presented at all: the
    //    refresh token reconnects, the new one is stored, and the rotation is
    //    acknowledged before this call returns (the next connect below would
    //    otherwise present the unacknowledged token and get the same answer).
    tokens.keep_access(
        &machine.host_id,
        Access::new(
            Secret::new(first_access.clone()),
            chrono::Utc::now() - chrono::Duration::seconds(1),
        ),
    );
    let (session, _) = connector
        .connect(&machine, &tokens)
        .await
        .expect("the refresh token reconnects");
    let second_refresh = tokens.get(&machine.host_id).unwrap().unwrap();
    assert_ne!(
        second_refresh.expose(),
        first_refresh.expose(),
        "every accepted refresh token rotates"
    );
    assert_ne!(
        session
            .access()
            .expect("a rotation issues one")
            .token()
            .expose(),
        first_access
    );
    drop(session);

    // 3. An access token the machine no longer takes is answered
    //    `access_expired`; the client falls back to the refresh token on the
    //    same route rather than pairing again.
    tokens.keep_access(
        &machine.host_id,
        Access::new(
            Secret::new("an-access-token-this-machine-never-minted"),
            chrono::Utc::now() + chrono::Duration::minutes(10),
        ),
    );
    let (session, _) = connector
        .connect(&machine, &tokens)
        .await
        .expect("access_expired falls back to the refresh token");
    let third_refresh = tokens.get(&machine.host_id).unwrap().unwrap();
    assert_ne!(third_refresh.expose(), second_refresh.expose());
    assert!(session.access().is_some());
    drop(session);

    // 4. The refresh token the pairing issued was retired when its rotation was
    //    acknowledged: presenting it again is a replay, and the machine revokes
    //    the pairing. The client drops the dead token and says to pair again.
    tokens.set(&machine.host_id, &first_refresh).unwrap();
    assert!(
        tokens.access(&machine.host_id).is_none(),
        "a replaced durable token takes the access token with it"
    );
    let error = connector
        .connect(&machine, &tokens)
        .await
        .expect_err("a retired refresh token is a replay");
    let message = format!("{error:#}");
    assert!(message.contains("token_replayed"), "{message}");
    assert!(message.contains("pairing revoked"), "{message}");
    assert!(
        tokens.get(&machine.host_id).unwrap().is_none(),
        "the client drops a token the machine revoked"
    );

    host.stop().await;
}

/// A device the machine revoked is told to pair again rather than to keep
/// retrying a token it will never accept.
#[tokio::test]
async fn a_revoked_device_is_told_to_pair_again() {
    let host = Host::start().await;
    let config = tempfile::tempdir_in("/tmp").unwrap();
    let connector = Connector::new(config.path(), ClientKind::Desktop).unwrap();
    let (machine, tokens, _paired) = pair(&host, &connector, config.path()).await;

    let revoked = host
        .lifecycle
        .revoke(&machine.device_id)
        .await
        .expect("the owner revokes the device");
    assert_eq!(revoked.device_id, machine.device_id);

    let error = connector
        .connect(&machine, &tokens)
        .await
        .expect_err("a revoked device is refused");
    let message = format!("{error:#}");
    assert!(message.contains("unauthorized"), "{message}");
    assert!(message.contains("pair with the machine again"), "{message}");

    host.stop().await;
}

/// A gateway from before rotating tokens does not know the `refresh_token`
/// field and answers `invalid_frame`. The static token of a pairing made
/// before this client learned to rotate then reconnects with the old form, so
/// an existing pairing keeps working until it is paired again.
#[tokio::test]
async fn an_older_gateway_reads_the_static_token_after_the_refresh_form() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    const HOST_ID: &str = "gw-static-00112233";
    const DEVICE_ID: &str = "0011223344556677";
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        // The rotating form first: this gateway cannot read it, exactly as one
        // from before rotating tokens answers it.
        {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            let Some(Ok(Message::Text(frame))) = socket.next().await else {
                panic!("the client sends a handshake frame first");
            };
            assert!(frame.contains("\"refresh_token\""), "{frame}");
            assert!(frame.contains("\"token_rotation\""), "{frame}");
            socket
                .send(Message::text(
                    serde_json::json!({
                        "type": "error",
                        "code": "invalid_frame",
                        "message": "first frame must be a pair or authenticate request",
                    })
                    .to_string(),
                ))
                .await
                .unwrap();
        }
        // The pre-rotation form, which it is the gateway to read.
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        let Some(Ok(Message::Text(frame))) = socket.next().await else {
            panic!("the client sends a handshake frame first");
        };
        assert!(frame.contains("\"token\""), "{frame}");
        assert!(!frame.contains("refresh_token"), "{frame}");
        assert!(!frame.contains("capabilities"), "{frame}");
        socket
            .send(Message::text(
                serde_json::json!({
                    "type": "authenticated",
                    "protocol_version": 1,
                    "gateway_id": HOST_ID,
                    "instance_id": "inst-1",
                    "device_id": DEVICE_ID,
                    "device_name": "PC",
                })
                .to_string(),
            ))
            .await
            .unwrap();
        // Hold the connection the way a gateway that keeps it open does.
        while let Some(Ok(_)) = socket.next().await {}
    });

    let config = tempfile::tempdir_in("/tmp").unwrap();
    let connector = Connector::new(config.path(), ClientKind::Cli).unwrap();
    let machine = Machine::new(
        HOST_ID,
        "Studio",
        "macos",
        DEVICE_ID,
        Route::new(addr.to_string()),
        chrono::Utc::now(),
    );
    let tokens = TokenStore::in_file(config.path()).unwrap();
    tokens
        .set(&machine.host_id, &Secret::new("a-static-device-token"))
        .unwrap();

    let (session, _) = connector
        .connect(&machine, &tokens)
        .await
        .expect("the static token still authenticates");
    assert_eq!(session.welcome().gateway_id, HOST_ID);
    assert_eq!(session.welcome().device_id, DEVICE_ID);
    drop(session);
    server.abort();
}

/// An endpoint on a saved machine's address that answers as another gateway,
/// and hands out a refresh token of its own, must not replace the saved
/// credential or be told the rotation landed: the identity is checked before
/// anything it sent is stored or acknowledged.
#[tokio::test]
async fn a_spoofed_gateway_cannot_overwrite_the_saved_token_or_get_an_ack() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    const HOST_ID: &str = "gw-real-00112233";
    const DEVICE_ID: &str = "0011223344556677";
    for (gateway_id, device_id) in [
        ("gw-attacker-9988", DEVICE_ID),
        (HOST_ID, "ffffffffffffffff"),
    ] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (seen_tx, seen_rx) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            let Some(Ok(Message::Text(_))) = socket.next().await else {
                panic!("the client sends a handshake frame first");
            };
            socket
                .send(Message::text(
                    serde_json::json!({
                        "type": "authenticated",
                        "protocol_version": 1,
                        "gateway_id": gateway_id,
                        "instance_id": "inst-1",
                        "device_id": device_id,
                        "device_name": "PC",
                        "refresh_token": "attacker-refresh",
                        "access_token": "attacker-access",
                        "access_expires_at": "2099-01-01T00:00:00Z",
                        "rotation_id": "5be1c4a07d2e9f63",
                    })
                    .to_string(),
                ))
                .await
                .unwrap();
            // What follows from the client: a closed socket, not a `token_ack`.
            let next = tokio::time::timeout(Duration::from_secs(5), socket.next()).await;
            let acked =
                matches!(next, Ok(Some(Ok(Message::Text(frame)))) if frame.contains("token_ack"));
            let _ = seen_tx.send(acked);
        });

        let config = tempfile::tempdir_in("/tmp").unwrap();
        let connector = Connector::new(config.path(), ClientKind::Cli).unwrap();
        let machine = Machine::new(
            HOST_ID,
            "Studio",
            "macos",
            DEVICE_ID,
            Route::new(addr.to_string()),
            chrono::Utc::now(),
        );
        let tokens = TokenStore::in_file(config.path()).unwrap();
        tokens
            .set(&machine.host_id, &Secret::new("the-real-refresh"))
            .unwrap();

        let error = connector
            .connect(&machine, &tokens)
            .await
            .expect_err("a mismatched identity is not a connection");
        let message = format!("{error:#}");
        assert!(
            !message.contains("attacker-refresh") && !message.contains("attacker-access"),
            "{message}"
        );
        assert_eq!(
            tokens.get(&machine.host_id).unwrap().unwrap().expose(),
            "the-real-refresh",
            "the saved credential is untouched ({gateway_id}, {device_id})"
        );
        assert!(
            tokens.access(&machine.host_id).is_none(),
            "the endpoint's access token is not kept"
        );
        assert!(
            !seen_rx.await.unwrap(),
            "no token_ack goes to an endpoint that is not the machine"
        );
        server.abort();
    }
}

/// Two routes to one machine race, and only one presents the rotating
/// refresh token. Presenting it twice at once looks like a replay to the
/// machine, which revokes the pairing, so the race must serialize the
/// credential and let the loser be cancelled.
#[tokio::test]
async fn racing_routes_present_the_refresh_token_once() {
    let host = Host::start().await;
    let config = tempfile::tempdir_in("/tmp").unwrap();
    let connector = Connector::new(config.path(), ClientKind::Desktop).unwrap();
    let (mut machine, tokens, paired) = pair(&host, &connector, config.path()).await;
    drop(paired);
    // The same gateway under two names, raced as two LAN routes: the second
    // socket opens, then waits for the credential the first one presents.
    machine.routes = vec![
        Route::new(host.address()),
        Route::new(format!("localhost:{}", host.addr.port())),
    ];
    let connected = connector
        .connect_raced(&machine, &tokens)
        .await
        .expect("one of the two routes wins");
    drop(connected);
    // Present the durable token again. A concurrent replay would have revoked
    // the family, and this would fail as `revoked`.
    tokens.forget_access(&machine.host_id);
    connector
        .connect(&machine, &tokens)
        .await
        .expect("the pairing survived the race");
    host.stop().await;
}
