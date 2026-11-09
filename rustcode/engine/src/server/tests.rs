use super::*;
use agent_client_protocol::schema::v1::{
    ContentBlock, InitializeRequest, LoadSessionRequest, NewSessionRequest, PromptRequest,
    SessionNotification, SessionUpdate, TextContent,
};
use agent_client_protocol::{Agent, ConnectionTo};

/// A client over the server's endpoint: every message chunk it is sent goes
/// to `said` as `("user" | "agent", text)`.
async fn socket_client(
    endpoint: String,
    said: tokio::sync::mpsc::UnboundedSender<(&'static str, String)>,
    connected: tokio::sync::oneshot::Sender<ConnectionTo<Agent>>,
    close: tokio::sync::oneshot::Receiver<()>,
) -> Result<(), agent_client_protocol::Error> {
    let stream = listen::connect(&endpoint)
        .await
        .map_err(|error| agent_client_protocol::Error::internal_error().data(error.to_string()))?;
    let (read, write) = tokio::io::split(stream);
    agent_client_protocol::Client
        .builder()
        .on_receive_notification(
            async move |notification: SessionNotification, _connection| {
                let chunk = match notification.update {
                    SessionUpdate::UserMessageChunk(chunk) => Some(("user", chunk)),
                    SessionUpdate::AgentMessageChunk(chunk) => Some(("agent", chunk)),
                    _ => None,
                };
                if let Some((who, chunk)) = chunk
                    && let ContentBlock::Text(text) = chunk.content
                {
                    let _ = said.send((who, text.text));
                }
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            async move |_request: agent_client_protocol::UntypedMessage, responder, _connection| {
                responder.respond(serde_json::json!({}))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(
            crate::acp::lines_transport(read, write),
            async move |connection| {
                let _ = connected.send(connection);
                let _ = close.await;
                Ok(())
            },
        )
        .await
}

fn on_a_large_stack(test: impl std::future::Future<Output = ()> + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(move || {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("runtime")
                .block_on(test)
        })
        .expect("test thread")
        .join()
        .expect("the test passes");
}

#[test]
fn two_clients_of_one_server_see_the_same_thread() {
    on_a_large_stack(two_clients_one_server());
}

async fn two_clients_one_server() {
    let run = tempfile::tempdir().expect("run dir");
    let endpoint = listen::endpoint(run.path());
    let mut listener = listen::Listener::bind(&endpoint).expect("listen");
    // Not `start`: it defers the configured MCP servers, which a turn in
    // another test would then start.
    let engine = AcpEngine::bare(false).expect("engine");
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let serving = tokio::spawn({
        let engine = engine.clone();
        async move {
            serve(&engine, &mut listener, async {
                let _ = stopped.await;
            })
            .await;
        }
    });

    let mut clients = Vec::new();
    for _ in 0..2 {
        let (said_tx, said) = tokio::sync::mpsc::unbounded_channel();
        let (ready_tx, ready) = tokio::sync::oneshot::channel();
        let (close, closed) = tokio::sync::oneshot::channel();
        let client = tokio::spawn(socket_client(endpoint.clone(), said_tx, ready_tx, closed));
        let connection = ready.await.expect("connects");
        connection
            .send_request(InitializeRequest::new(
                agent_client_protocol::schema::ProtocolVersion::V1,
            ))
            .block_task()
            .await
            .expect("initializes");
        clients.push((connection, said, close, client));
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while engine.clients() != 2 {
        assert!(std::time::Instant::now() < deadline, "both clients connect");
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    let cwd = tempfile::tempdir().expect("cwd");
    let cwd = std::fs::canonicalize(cwd.path()).expect("cwd");
    let session_id = clients[0]
        .0
        .send_request(NewSessionRequest::new(cwd.clone()))
        .block_task()
        .await
        .expect("a creates a thread")
        .session_id;
    clients[1]
        .0
        .send_request(LoadSessionRequest::new(session_id.clone(), cwd.clone()))
        .block_task()
        .await
        .expect("b opens the same thread");
    // A report answers without a model, through the thread's clients.
    clients[0]
        .0
        .send_request(PromptRequest::new(
            session_id.clone(),
            vec![ContentBlock::Text(TextContent::new("/context"))],
        ))
        .block_task()
        .await
        .expect("a's report");

    let mut said = Vec::new();
    for (_, mut received, close, client) in clients {
        let mut chunks = Vec::new();
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            while !chunks.iter().any(|(who, _)| *who == "agent") {
                chunks.push(received.recv().await.expect("client receives the reply"));
            }
        })
        .await
        .expect("client receives the reply before the deadline");
        let _ = close.send(());
        client.await.expect("client").expect("client ends cleanly");
        while let Ok(chunk) = received.try_recv() {
            chunks.push(chunk);
        }
        said.push(chunks);
    }
    let agent =
        |chunks: &[(&str, String)]| chunks.iter().filter(|(who, _)| *who == "agent").count();
    assert_eq!(agent(&said[0]), 1, "a got the reply: {:?}", said[0]);
    assert_eq!(agent(&said[1]), 1, "b got it too: {:?}", said[1]);
    assert!(
        said[1].contains(&("user", "/context".to_owned())),
        "b was shown a's prompt: {:?}",
        said[1]
    );

    let _ = stop.send(());
    serving.await.expect("the server stops");
    engine.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn the_socket_is_this_users_alone_and_goes_with_the_server() {
    use std::os::unix::fs::PermissionsExt;
    let run = tempfile::tempdir().expect("run dir");
    let endpoint = listen::endpoint(run.path());
    // A dead server's socket is replaced.
    std::fs::write(&endpoint, b"").expect("stale socket");
    let mut listener = listen::Listener::bind(&endpoint).expect("listen");
    let mode = std::fs::metadata(&endpoint)
        .expect("socket")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);

    let client = tokio::spawn({
        let endpoint = endpoint.clone();
        async move { listen::connect(&endpoint).await }
    });
    listener.accept().await.expect("this user is accepted");
    client.await.expect("client").expect("connects");

    drop(listener);
    assert!(!std::path::Path::new(&endpoint).exists());
    assert!(listen::connect(&endpoint).await.is_err());
}

#[tokio::test]
async fn the_bridge_relays_both_ways_and_waits_for_the_last_answer() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (bridge_side, mut server_side) = tokio::io::duplex(1024);
    let (mut client_in, bridge_in) = tokio::io::duplex(1024);
    let (bridge_out, mut client_out) = tokio::io::duplex(1024);
    let bridged = tokio::spawn(bridge(bridge_side, bridge_in, bridge_out));

    client_in.write_all(b"{\"id\":1}\n").await.expect("request");
    // The client is done before the answer comes.
    drop(client_in);
    let mut request = Vec::new();
    server_side
        .read_to_end(&mut request)
        .await
        .expect("the server reads to the end");
    assert_eq!(request, b"{\"id\":1}\n");
    server_side
        .write_all(b"{\"id\":1,\"result\":{}}\n")
        .await
        .expect("answer");
    drop(server_side);

    let mut answer = Vec::new();
    client_out.read_to_end(&mut answer).await.expect("answer");
    assert_eq!(answer, b"{\"id\":1,\"result\":{}}\n");
    bridged.await.expect("bridge").expect("relayed");
}

#[test]
fn the_status_request_is_answered_by_a_running_server() {
    on_a_large_stack(async {
        let run = tempfile::tempdir().expect("run dir");
        let endpoint = listen::endpoint(run.path());
        let mut listener = listen::Listener::bind(&endpoint).expect("listen");
        let engine = AcpEngine::bare(false).expect("engine");
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let serving = tokio::spawn({
            let engine = engine.clone();
            async move {
                serve(&engine, &mut listener, async {
                    let _ = stopped.await;
                })
                .await;
            }
        });

        // Without `initialize`: the asker is not a client.
        let stream = listen::connect(&endpoint).await.expect("connects");
        let status = crate::service::server_service::ask_status(stream)
            .await
            .expect("status");
        assert_eq!(status.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(status.pid, std::process::id());
        assert_eq!(status.clients, 0);
        assert_eq!(status.running_turns, 0);

        let _ = stop.send(());
        serving.await.expect("server stops");
    });
}
