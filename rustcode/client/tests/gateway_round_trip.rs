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
    ClientKind, Connector, MachineRegistry, Route, RouteKind, TokenStore, transport,
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
