//! Socket-level tests of the gateway. Every wait is bounded, every listener
//! binds `127.0.0.1:0`, and every gateway lives in its own temporary
//! configuration directory.

use super::*;
use crate::remote_gateway::address::plan_listen;
use crate::remote_gateway::control;
use crate::remote_gateway::handshake::MAX_HANDSHAKE_FRAME_BYTES;
use crate::remote_gateway::pairing::{ATTEMPTS_PER_CHALLENGE, HOST_FAILURE_LIMIT, QrPayload};
use crate::remote_gateway::router::NoSessionsRouter;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::{WebSocketStream, client_async, tungstenite::Message};

const WAIT: Duration = Duration::from_secs(10);

async fn bounded<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(WAIT, future)
        .await
        .expect("test step timed out")
}

/// Records what reaches the router, so tests can prove nothing does before
/// authentication, and optionally echoes frames to every connected device.
#[derive(Default)]
struct RecordingRouter {
    broadcast: bool,
    frames: Mutex<Vec<(String, String)>>,
    sinks: Mutex<Vec<FrameSink>>,
    disconnected: Mutex<Vec<String>>,
}

impl FrameRouter for RecordingRouter {
    fn connected(&self, _device: &DeviceContext, sink: &FrameSink) {
        self.sinks.lock().unwrap().push(sink.clone());
    }

    fn frame(&self, device: &DeviceContext, frame: &str, sink: &FrameSink) {
        self.frames
            .lock()
            .unwrap()
            .push((device.device_id.clone(), frame.to_string()));
        if self.broadcast {
            for sink in self.sinks.lock().unwrap().iter() {
                let _ = sink.send(frame.to_string());
            }
        } else {
            NoSessionsRouter.frame(device, frame, sink);
        }
    }

    fn disconnected(&self, device: &DeviceContext) {
        self.disconnected
            .lock()
            .unwrap()
            .push(device.device_id.clone());
    }
}

struct Harness {
    _dir: tempfile::TempDir,
    lifecycle: RemoteLifecycle,
    addr: SocketAddr,
    router: Arc<RecordingRouter>,
    shutdown: CancellationToken,
    task: tokio::task::JoinHandle<Result<()>>,
}

impl Harness {
    async fn start() -> Self {
        Self::start_with(Limits::default(), RecordingRouter::default()).await
    }

    async fn start_with(limits: Limits, router: RecordingRouter) -> Self {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let lifecycle = RemoteLifecycle::new(dir.path());
        let router = Arc::new(router);
        let gateway = Gateway::bind(
            &lifecycle,
            GatewayConfig {
                plan: plan_listen("127.0.0.1", None, &[]).unwrap(),
                port: 0,
                router: router.clone(),
                sessions: None,
                limits,
            },
        )
        .await
        .unwrap();
        let addr = gateway.local_addr();
        let shutdown = gateway.shutdown_token();
        let task = tokio::spawn(gateway.run());
        Self {
            _dir: dir,
            lifecycle,
            addr,
            router,
            shutdown,
            task,
        }
    }

    async fn offer(&self) -> OfferDetails {
        bounded(self.lifecycle.pair()).await.unwrap()
    }

    async fn status(&self) -> GatewayStatus {
        bounded(self.lifecycle.status()).await.unwrap().unwrap()
    }

    /// Pair a fresh device and return its open connection and credentials.
    async fn paired_device(&self, name: &str) -> (Client, String, String) {
        let offer = self.offer().await;
        let mut client = Client::connect(self.addr).await;
        client
            .send(json!({
                "type": "pair",
                "protocol_version": 1,
                "method": "credential",
                "secret": offer.credential.expose(),
                "device_name": name,
            }))
            .await;
        let paired = client.next_json().await;
        assert_eq!(paired["type"], "paired", "{paired}");
        let id = paired["device_id"].as_str().unwrap().to_string();
        let token = paired["token"].as_str().unwrap().to_string();
        (client, id, token)
    }

    async fn stop(self) {
        self.shutdown.cancel();
        bounded(self.task).await.unwrap().unwrap();
    }
}

struct Client {
    socket: WebSocketStream<TcpStream>,
}

impl Client {
    async fn connect(addr: SocketAddr) -> Self {
        let stream = bounded(TcpStream::connect(addr)).await.unwrap();
        let (socket, _) = bounded(client_async(format!("ws://{addr}/"), stream))
            .await
            .expect("websocket upgrade");
        Self { socket }
    }

    async fn send(&mut self, value: Value) {
        self.send_text(value.to_string()).await;
    }

    async fn send_text(&mut self, text: String) {
        bounded(self.socket.send(Message::text(text)))
            .await
            .expect("send frame");
    }

    /// Next text frame as JSON; pings are skipped.
    async fn next_json(&mut self) -> Value {
        loop {
            match bounded(self.socket.next()).await {
                Some(Ok(Message::Text(text))) => {
                    return serde_json::from_str(text.as_str()).expect("JSON frame");
                }
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                other => panic!("expected a text frame, got {other:?}"),
            }
        }
    }

    /// The connection ends without another text frame.
    async fn expect_closed(&mut self) {
        loop {
            match bounded(self.socket.next()).await {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => return,
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                Some(Ok(other)) => panic!("expected the connection to close, got {other:?}"),
            }
        }
    }

    async fn expect_error(&mut self, code: &str) -> Value {
        let frame = self.next_json().await;
        assert_eq!(frame["type"], "error", "{frame}");
        assert_eq!(frame["code"], code, "{frame}");
        self.expect_closed().await;
        frame
    }

    async fn pair(addr: SocketAddr, method: &str, secret: &str) -> Value {
        let mut client = Self::connect(addr).await;
        client
            .send(json!({
                "type": "pair",
                "protocol_version": 1,
                "method": method,
                "secret": secret,
                "device_name": "phone",
            }))
            .await;
        client.next_json().await
    }

    async fn authenticate(addr: SocketAddr, device_id: &str, token: &str) -> (Self, Value) {
        let mut client = Self::connect(addr).await;
        client
            .send(json!({
                "type": "authenticate",
                "protocol_version": 1,
                "device_id": device_id,
                "token": token,
            }))
            .await;
        let frame = client.next_json().await;
        (client, frame)
    }
}

#[tokio::test]
async fn pairing_with_the_manual_code_yields_a_token_that_authenticates() {
    let harness = Harness::start().await;
    let offer = harness.offer().await;
    assert_eq!(offer.advertised_address, harness.addr.to_string());
    assert!(harness.status().await.pairing_open);

    let mut client = Client::connect(harness.addr).await;
    client
        .send(json!({
            "type": "pair",
            "protocol_version": 1,
            "method": "code",
            "secret": offer.code.expose(),
            "device_name": "Lars's iPhone",
        }))
        .await;
    let paired = client.next_json().await;
    assert_eq!(paired["type"], "paired", "{paired}");
    assert_eq!(paired["protocol_version"], 1);
    assert_eq!(paired["device_name"], "Lars's iPhone");
    assert_eq!(paired["gateway_id"], offer.gateway_id);
    let status = harness.status().await;
    assert_eq!(paired["instance_id"], status.instance_id);
    assert!(!status.pairing_open);
    let device_id = paired["device_id"].as_str().unwrap();
    let token = paired["token"].as_str().unwrap();

    // The pairing connection is authenticated; the stub router answers it.
    client.send(json!({"type": "list_sessions"})).await;
    let reply = client.next_json().await;
    assert_eq!(reply["type"], "error");
    assert_eq!(reply["code"], "not_implemented");

    // The token works on a new connection and only its digest is on disk.
    let (mut second, authenticated) = Client::authenticate(harness.addr, device_id, token).await;
    assert_eq!(authenticated["type"], "authenticated", "{authenticated}");
    assert_eq!(authenticated["device_id"], device_id);
    assert_eq!(authenticated["gateway_id"], offer.gateway_id);
    second.send(json!({"type": "anything"})).await;
    assert_eq!(second.next_json().await["code"], "not_implemented");

    let stored = std::fs::read_to_string(harness.lifecycle.devices_path()).unwrap();
    assert!(!stored.contains(token));
    assert!(stored.contains(device_id));
    let mode = std::fs::metadata(harness.lifecycle.devices_path())
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
    assert_eq!(harness.status().await.connections.len(), 2);
    harness.stop().await;
}

#[tokio::test]
async fn nothing_but_a_handshake_error_is_sent_before_authentication() {
    let harness = Harness::start().await;
    let (_paired, device_id, _token) = harness.paired_device("phone").await;

    // A session operation as first frame.
    let mut client = Client::connect(harness.addr).await;
    client
        .send(json!({"type": "list_sessions", "protocol_version": 1}))
        .await;
    client.expect_error("invalid_frame").await;

    // A wrong token, an unknown device and a newer protocol version.
    let (mut client, frame) = Client::authenticate(harness.addr, &device_id, "wrong-token").await;
    assert_eq!(frame["code"], "unauthorized", "{frame}");
    client.expect_closed().await;
    let (mut client, frame) = Client::authenticate(harness.addr, "ffffffffffffffff", "x").await;
    assert_eq!(frame["code"], "unauthorized", "{frame}");
    client.expect_closed().await;
    let mut client = Client::connect(harness.addr).await;
    client
        .send(json!({"type": "authenticate", "protocol_version": 2, "device_id": device_id, "token": "x"}))
        .await;
    client.expect_error("unsupported_version").await;

    // Binary and oversized first frames.
    let mut client = Client::connect(harness.addr).await;
    bounded(client.socket.send(Message::binary(vec![1u8, 2, 3])))
        .await
        .unwrap();
    client.expect_error("invalid_frame").await;
    let mut client = Client::connect(harness.addr).await;
    client
        .send_text("x".repeat(MAX_HANDSHAKE_FRAME_BYTES + 1))
        .await;
    client.expect_error("frame_too_large").await;

    // None of those sockets reached the router or the live registry.
    assert!(harness.router.frames.lock().unwrap().is_empty());
    assert_eq!(harness.router.sinks.lock().unwrap().len(), 1);
    assert_eq!(harness.status().await.connections.len(), 1);
    harness.stop().await;
}

#[tokio::test]
async fn browser_origins_are_refused_at_the_upgrade() {
    use tokio_tungstenite::tungstenite::{Error, client::IntoClientRequest};

    let harness = Harness::start().await;
    let mut request = format!("ws://{}/", harness.addr)
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("origin", "https://example.test".parse().unwrap());
    let stream = bounded(TcpStream::connect(harness.addr)).await.unwrap();
    match bounded(client_async(request, stream)).await {
        Err(Error::Http(response)) => assert_eq!(response.status(), 403),
        other => panic!("expected a refused upgrade, got {:?}", other.map(|_| ())),
    }
    harness.stop().await;
}

#[tokio::test]
async fn a_used_credential_or_code_cannot_pair_again() {
    let harness = Harness::start().await;
    let offer = harness.offer().await;
    let paired = Client::pair(harness.addr, "credential", offer.credential.expose()).await;
    assert_eq!(paired["type"], "paired", "{paired}");

    // Replay of the credential, and use of the code from the same challenge.
    let replay = Client::pair(harness.addr, "credential", offer.credential.expose()).await;
    assert_eq!(replay["code"], "pairing_failed", "{replay}");
    assert!(replay.get("token").is_none());
    let other = Client::pair(harness.addr, "code", offer.code.expose()).await;
    assert_eq!(other["code"], "pairing_failed", "{other}");
    assert_eq!(harness.lifecycle.devices().unwrap().len(), 1);

    // A new challenge invalidates the previous one even when unused.
    let stale = harness.offer().await;
    let fresh = harness.offer().await;
    let rejected = Client::pair(harness.addr, "code", stale.code.expose()).await;
    assert_eq!(rejected["code"], "pairing_failed", "{rejected}");
    let accepted = Client::pair(harness.addr, "code", fresh.code.expose()).await;
    assert_eq!(accepted["type"], "paired", "{accepted}");
    harness.stop().await;
}

#[tokio::test]
async fn attempt_limits_hold_across_rotating_connections() {
    let harness = Harness::start().await;
    let offer = harness.offer().await;
    // Five wrong codes, each from a brand-new socket.
    for _ in 0..ATTEMPTS_PER_CHALLENGE {
        let frame = Client::pair(harness.addr, "code", "0000-0000-wrong").await;
        assert_eq!(frame["code"], "pairing_failed", "{frame}");
    }
    // The challenge is gone: the right code and the right credential fail.
    let frame = Client::pair(harness.addr, "code", offer.code.expose()).await;
    assert_eq!(frame["code"], "pairing_failed", "{frame}");
    let frame = Client::pair(harness.addr, "credential", offer.credential.expose()).await;
    assert_eq!(frame["code"], "pairing_failed", "{frame}");

    // Keep guessing against fresh challenges until the host-wide budget is
    // spent; from then on even a correct code is refused, on any socket.
    let failures_so_far = usize::from(ATTEMPTS_PER_CHALLENGE) + 2;
    harness.offer().await;
    for _ in failures_so_far..HOST_FAILURE_LIMIT {
        let frame = Client::pair(harness.addr, "code", "wrong").await;
        assert_eq!(frame["code"], "pairing_failed", "{frame}");
    }
    let offer = harness.offer().await;
    for _ in 0..3 {
        let frame = Client::pair(harness.addr, "code", offer.code.expose()).await;
        assert_eq!(frame["code"], "rate_limited", "{frame}");
        assert!(frame["retry_after_secs"].as_u64().unwrap() > 0, "{frame}");
    }
    assert!(harness.lifecycle.devices().unwrap().is_empty());
    assert!(harness.router.sinks.lock().unwrap().is_empty());
    harness.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn racing_pairings_on_one_challenge_yield_exactly_one_device() {
    let harness = Harness::start().await;
    let offer = harness.offer().await;
    let mut racers = Vec::new();
    for _ in 0..4 {
        let addr = harness.addr;
        let credential = offer.credential.expose().to_string();
        racers.push(tokio::spawn(async move {
            Client::pair(addr, "credential", &credential).await
        }));
    }
    let mut paired = 0;
    for racer in racers {
        let frame = bounded(racer).await.unwrap();
        match frame["type"].as_str() {
            Some("paired") => paired += 1,
            _ => assert_eq!(frame["code"], "pairing_failed", "{frame}"),
        }
    }
    assert_eq!(paired, 1);
    assert_eq!(harness.lifecycle.devices().unwrap().len(), 1);
    harness.stop().await;
}

#[tokio::test]
async fn revoking_a_device_closes_its_connections_immediately() {
    let harness = Harness::start().await;
    let (mut first, device_id, token) = harness.paired_device("phone").await;
    let (mut second, frame) = Client::authenticate(harness.addr, &device_id, &token).await;
    assert_eq!(frame["type"], "authenticated");
    let (mut bystander, other_id, other_token) = harness.paired_device("tablet").await;

    let revocation = bounded(harness.lifecycle.revoke("phone")).await.unwrap();
    assert_eq!(revocation.device_id, device_id);
    assert_eq!(revocation.closed_connections, 2);
    assert!(revocation.gateway_running);
    // Gone from the live registry by the time revoke returns.
    let status = harness.status().await;
    assert_eq!(status.connections.len(), 1);
    assert_eq!(status.connections[0].device_id, other_id);

    first.expect_error("revoked").await;
    second.expect_error("revoked").await;
    let (mut retry, frame) = Client::authenticate(harness.addr, &device_id, &token).await;
    assert_eq!(frame["code"], "unauthorized", "{frame}");
    retry.expect_closed().await;
    assert!(
        harness
            .lifecycle
            .devices()
            .unwrap()
            .iter()
            .all(|device| device.id != device_id)
    );

    // The other device is untouched, live and for new connections.
    bystander.send(json!({"type": "ping"})).await;
    assert_eq!(bystander.next_json().await["code"], "not_implemented");
    let (_again, frame) = Client::authenticate(harness.addr, &other_id, &other_token).await;
    assert_eq!(frame["type"], "authenticated");
    assert!(bounded(harness.lifecycle.revoke("phone")).await.is_err());
    harness.stop().await;
}

#[tokio::test]
async fn silent_sockets_are_closed_at_the_handshake_deadline() {
    let limits = Limits {
        handshake_timeout: Duration::from_millis(200),
        ..Limits::default()
    };
    let harness = Harness::start_with(limits, RecordingRouter::default()).await;

    // Never upgrades: closed without a byte.
    let mut raw = bounded(TcpStream::connect(harness.addr)).await.unwrap();
    let mut buffer = [0u8; 64];
    assert_eq!(bounded(raw.read(&mut buffer)).await.unwrap_or(0), 0);

    // Trickles an HTTP request that never completes.
    let mut slow = bounded(TcpStream::connect(harness.addr)).await.unwrap();
    slow.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n")
        .await
        .unwrap();
    assert_eq!(bounded(slow.read(&mut buffer)).await.unwrap_or(0), 0);

    // Upgrades, then says nothing.
    let mut idle = Client::connect(harness.addr).await;
    idle.expect_error("handshake_timeout").await;
    harness.stop().await;
}

#[tokio::test]
async fn unauthenticated_sockets_are_bounded_and_do_not_starve_devices() {
    let limits = Limits {
        handshake_timeout: Duration::from_millis(600),
        max_unauthenticated: 3,
        max_unauthenticated_per_ip: 3,
        ..Limits::default()
    };
    let harness = Harness::start_with(limits, RecordingRouter::default()).await;
    let (mut device, device_id, token) = harness.paired_device("phone").await;

    // Fill every unauthenticated slot with sockets that never speak.
    let mut held = Vec::new();
    for _ in 0..3 {
        held.push(bounded(TcpStream::connect(harness.addr)).await.unwrap());
    }
    // Let the gateway accept them before probing the limit.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut refused = bounded(TcpStream::connect(harness.addr)).await.unwrap();
    let mut buffer = [0u8; 16];
    let closed_at = Instant::now();
    assert_eq!(bounded(refused.read(&mut buffer)).await.unwrap_or(0), 0);
    assert!(
        closed_at.elapsed() < Duration::from_millis(300),
        "an over-limit socket must be dropped at once, not at the deadline"
    );

    // The authenticated device is unaffected by the flood.
    device.send(json!({"type": "ping"})).await;
    assert_eq!(device.next_json().await["code"], "not_implemented");

    // Slots come back when the silent sockets hit the deadline.
    for socket in &mut held {
        assert_eq!(bounded(socket.read(&mut buffer)).await.unwrap_or(0), 0);
    }
    let (_client, frame) = Client::authenticate(harness.addr, &device_id, &token).await;
    assert_eq!(frame["type"], "authenticated", "{frame}");
    harness.stop().await;
}

#[tokio::test]
async fn one_address_cannot_take_every_unauthenticated_slot() {
    let limits = Limits {
        handshake_timeout: Duration::from_secs(5),
        max_unauthenticated: 8,
        max_unauthenticated_per_ip: 2,
        ..Limits::default()
    };
    let harness = Harness::start_with(limits, RecordingRouter::default()).await;
    let _held = [
        bounded(TcpStream::connect(harness.addr)).await.unwrap(),
        bounded(TcpStream::connect(harness.addr)).await.unwrap(),
    ];
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut refused = bounded(TcpStream::connect(harness.addr)).await.unwrap();
    let mut buffer = [0u8; 16];
    let read = tokio::time::timeout(Duration::from_secs(2), refused.read(&mut buffer)).await;
    assert_eq!(read.expect("dropped before the deadline").unwrap_or(0), 0);
    harness.stop().await;
}

#[tokio::test]
async fn oversized_frames_end_only_the_offending_connection() {
    let harness = Harness::start().await;
    let (mut offender, _, _) = harness.paired_device("phone").await;
    let (mut other, _, _) = harness.paired_device("tablet").await;

    // The gateway rejects the frame from its header and closes while the
    // device is still writing, so the write may fail and the error frame is
    // best effort: the device sees `frame_too_large` or a reset.
    let _ = bounded(
        offender
            .socket
            .send(Message::text("x".repeat(MAX_FRAME_BYTES + 1))),
    )
    .await;
    loop {
        match bounded(offender.socket.next()).await {
            Some(Ok(Message::Text(text))) => {
                let frame: Value = serde_json::from_str(text.as_str()).unwrap();
                assert_eq!(frame["code"], "frame_too_large", "{frame}");
            }
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
            None | Some(Err(_)) | Some(Ok(Message::Close(_))) => break,
            Some(Ok(other)) => panic!("unexpected frame {other:?}"),
        }
    }
    let deadline = Instant::now() + WAIT;
    while harness.status().await.connections.len() != 1 {
        assert!(Instant::now() < deadline, "offender still connected");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // The oversized frame never reached the router.
    assert!(harness.router.frames.lock().unwrap().is_empty());

    other.send_text("y".repeat(MAX_FRAME_BYTES / 2)).await;
    assert_eq!(other.next_json().await["code"], "not_implemented");
    harness.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stuck_device_is_dropped_without_delaying_others() {
    let limits = Limits {
        outbound_queue: 8,
        ..Limits::default()
    };
    let router = RecordingRouter {
        broadcast: true,
        ..RecordingRouter::default()
    };
    let harness = Harness::start_with(limits, router).await;
    // `stuck` authenticates and then never reads from its socket again.
    let (stuck, stuck_id, _) = harness.paired_device("stuck").await;
    let (mut healthy, healthy_id, _) = harness.paired_device("healthy").await;

    // Every frame is broadcast to both devices. The healthy one reads each
    // echo; the stuck one lets its socket buffers and then its queue fill.
    let payload = "z".repeat(64 * 1024);
    let started = Instant::now();
    for index in 0..400 {
        healthy
            .send(json!({"index": index, "payload": payload}))
            .await;
        assert_eq!(healthy.next_json().await["index"], index);
    }
    assert!(started.elapsed() < WAIT, "healthy device was delayed");

    // The stuck device was cut loose as a slow consumer.
    let deadline = Instant::now() + WAIT;
    loop {
        let status = harness.status().await;
        if status.connections.len() == 1 {
            assert_eq!(status.connections[0].device_id, healthy_id);
            break;
        }
        assert!(Instant::now() < deadline, "stuck device still connected");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        harness
            .router
            .disconnected
            .lock()
            .unwrap()
            .contains(&stuck_id)
    );
    drop(stuck);
    harness.stop().await;
}

#[tokio::test]
async fn idle_connections_are_closed() {
    let limits = Limits {
        ping_interval: Duration::from_millis(50),
        idle_timeout: Duration::from_millis(150),
        ..Limits::default()
    };
    let harness = Harness::start_with(limits, RecordingRouter::default()).await;
    let (device, _, _) = harness.paired_device("phone").await;
    // Not polling the client means its pongs are never sent.
    let deadline = Instant::now() + WAIT;
    while !harness.status().await.connections.is_empty() {
        assert!(Instant::now() < deadline, "idle device still connected");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    drop(device);
    harness.stop().await;
}

#[tokio::test]
async fn single_instance_stop_and_cleanup() {
    let harness = Harness::start().await;
    let lifecycle = harness.lifecycle.clone();
    let mode = |path: std::path::PathBuf| std::fs::metadata(path).unwrap().permissions().mode();
    assert_eq!(mode(lifecycle.socket_path()) & 0o777, 0o600);
    assert_eq!(mode(lifecycle.registration_path()) & 0o777, 0o600);

    // A second gateway for the same directory is refused.
    let second = Gateway::bind(
        &lifecycle,
        GatewayConfig {
            plan: plan_listen("127.0.0.1", None, &[]).unwrap(),
            port: 0,
            router: Arc::new(NoSessionsRouter),
            sessions: None,
            limits: Limits::default(),
        },
    )
    .await;
    let error = format!("{:#}", second.err().expect("second gateway must not bind"));
    assert!(error.contains("already running"), "{error}");

    let status = harness.status().await;
    assert_eq!(status.pid, std::process::id());
    assert_eq!(status.listen_address, harness.addr.to_string());
    assert_eq!(status.advertised_address, harness.addr.to_string());

    // A stale caller cannot stop this instance.
    let response = bounded(control::request(
        &lifecycle.socket_path(),
        &ControlRequest::Shutdown {
            instance_id: "someone-else".into(),
        },
    ))
    .await
    .unwrap();
    assert!(matches!(response, ControlResponse::Error { .. }));

    let (mut device, device_id, token) = harness.paired_device("phone").await;
    let stopped = bounded(lifecycle.stop()).await.unwrap().unwrap();
    assert_eq!(stopped.instance_id, status.instance_id);
    device.expect_error("shutting_down").await;
    bounded(harness.task).await.unwrap().unwrap();
    assert!(!lifecycle.socket_path().exists());
    assert!(!lifecycle.registration_path().exists());
    assert!(bounded(lifecycle.status()).await.unwrap().is_none());

    // Devices and the gateway identity survive a restart; the instance does
    // not. A child forked by a concurrent test can hold an inherited copy of
    // the lock descriptor until it execs, so poll instead of assuming one try.
    let deadline = Instant::now() + WAIT;
    let restarted = loop {
        let config = GatewayConfig {
            plan: plan_listen("127.0.0.1", None, &[]).unwrap(),
            port: 0,
            router: Arc::new(NoSessionsRouter),
            sessions: None,
            limits: Limits::default(),
        };
        match Gateway::bind(&lifecycle, config).await {
            Ok(gateway) => break gateway,
            Err(error) => {
                assert!(crate::daemon::lifecycle::is_lock_busy(&error), "{error:#}");
                assert!(Instant::now() < deadline, "owner lock not released");
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
    };
    let addr = restarted.local_addr();
    let shutdown = restarted.shutdown_token();
    let task = tokio::spawn(restarted.run());
    let (_client, frame) = Client::authenticate(addr, &device_id, &token).await;
    assert_eq!(frame["type"], "authenticated", "{frame}");
    assert_eq!(frame["gateway_id"], status.gateway_id);
    assert_ne!(frame["instance_id"], status.instance_id);
    shutdown.cancel();
    bounded(task).await.unwrap().unwrap();
}

#[tokio::test]
async fn aborted_gateway_releases_its_socket_registration_and_connections() {
    let harness = Harness::start().await;
    let (mut device, _, _) = harness.paired_device("phone").await;
    harness.task.abort();
    assert!(bounded(harness.task).await.unwrap_err().is_cancelled());
    assert!(!harness.lifecycle.socket_path().exists());
    assert!(!harness.lifecycle.registration_path().exists());
    device.expect_closed().await;
}

#[tokio::test]
async fn pairing_details_use_the_advertised_address_not_the_bind_address() {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let lifecycle = RemoteLifecycle::new(dir.path());
    let gateway = Gateway::bind(
        &lifecycle,
        GatewayConfig {
            plan: plan_listen("127.0.0.1", Some("workstation.netbird.test"), &[]).unwrap(),
            port: 0,
            router: Arc::new(NoSessionsRouter),
            sessions: None,
            limits: Limits::default(),
        },
    )
    .await
    .unwrap();
    let port = gateway.local_addr().port();
    let shutdown = gateway.shutdown_token();
    let task = tokio::spawn(gateway.run());

    let offer = bounded(lifecycle.pair()).await.unwrap();
    let expected = format!("workstation.netbird.test:{port}");
    assert_eq!(offer.advertised_address, expected);
    let payload: QrPayload = serde_json::from_str(offer.qr_payload.expose()).unwrap();
    assert_eq!(payload.protocol_version, 1);
    assert_eq!(payload.address, expected);
    assert_eq!(payload.gateway_id, offer.gateway_id);
    assert_eq!(payload.credential, offer.credential);
    let registration = GatewayRegistration::read(&lifecycle.registration_path()).unwrap();
    assert_eq!(registration.advertised_address, expected);
    // The registration holds no pairing secret.
    let on_disk = std::fs::read_to_string(lifecycle.registration_path()).unwrap();
    assert!(!on_disk.contains(offer.credential.expose()));

    shutdown.cancel();
    bounded(task).await.unwrap().unwrap();
}

#[tokio::test]
async fn websocket_pings_keep_an_otherwise_silent_connection_open() {
    let limits = Limits {
        ping_interval: Duration::from_millis(50),
        idle_timeout: Duration::from_millis(200),
        ..Limits::default()
    };
    let harness = Harness::start_with(limits, RecordingRouter::default()).await;
    let (mut client, _, _) = harness.paired_device("phone").await;
    // The device sends no text frame for five idle limits, only pings, and
    // does not read either, so it answers none of the gateway's own pings.
    for _ in 0..20 {
        bounded(client.socket.send(Message::Ping(Default::default())))
            .await
            .expect("ping");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    client.send(json!({"type": "still here"})).await;
    let answer = client.next_json().await;
    assert_eq!(answer["code"], "not_implemented", "{answer}");
    assert_eq!(harness.status().await.connections.len(), 1);
    harness.stop().await;
}

#[tokio::test]
async fn a_long_configuration_path_still_gets_working_sockets() {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let deep = dir.path().join("x".repeat(70)).join("y".repeat(70));
    assert!(deep.join("remote/control.sock").as_os_str().len() > 104);
    let lifecycle = RemoteLifecycle::new(&deep);
    let gateway = Gateway::bind(
        &lifecycle,
        GatewayConfig {
            plan: plan_listen("127.0.0.1", None, &[]).unwrap(),
            port: 0,
            router: Arc::new(NoSessionsRouter),
            sessions: None,
            limits: Limits::default(),
        },
    )
    .await
    .expect("the gateway binds although the path does not fit a socket address");
    let addr = gateway.local_addr();
    let shutdown = gateway.shutdown_token();
    let task = tokio::spawn(gateway.run());

    // The registration stays with the configuration and records where the
    // sockets went; both are private and owned by this user.
    let registration = GatewayRegistration::read(&lifecycle.registration_path()).unwrap();
    assert!(lifecycle.registration_path().starts_with(&deep));
    assert_eq!(registration.socket_path, lifecycle.socket_path());
    assert_eq!(
        registration.owner_socket_path,
        Some(lifecycle.owner_socket_path())
    );
    for socket in [lifecycle.socket_path(), lifecycle.owner_socket_path()] {
        assert!(!socket.starts_with(&deep), "{}", socket.display());
        assert_eq!(
            fs::metadata(&socket).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(socket.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
    // The control socket works: status, pairing, a device, stop.
    let offer = bounded(lifecycle.pair()).await.unwrap();
    let paired = Client::pair(addr, "code", offer.code.expose()).await;
    assert_eq!(paired["type"], "paired", "{paired}");
    assert!(bounded(lifecycle.status()).await.unwrap().is_some());
    shutdown.cancel();
    bounded(task).await.unwrap().unwrap();
    assert!(!lifecycle.socket_path().exists());
    assert!(!lifecycle.owner_socket_path().exists());
    let _ = fs::remove_dir_all(lifecycle.socket_path().parent().unwrap());
}
