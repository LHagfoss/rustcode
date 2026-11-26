use std::sync::Arc;
use std::time::Duration;

use agent_client_protocol::schema::v1::{
    AgentCapabilities, InitializeRequest, InitializeResponse, RequestPermissionOutcome,
    RequestPermissionRequest, RequestPermissionResponse, SelectedPermissionOutcome, ToolCallUpdate,
    ToolCallUpdateFields,
};
use agent_client_protocol::{Agent, Channel, Client, Error};

use super::{ClientInfo, Clients, Peer};

enum PermissionReply {
    Select(&'static str),
    Error(Error),
    DropConnection,
}

fn sample_permission(session_id: &str) -> RequestPermissionRequest {
    RequestPermissionRequest::new(
        session_id.to_owned(),
        ToolCallUpdate::new("call-1", ToolCallUpdateFields::new().title("Run command")),
        Vec::new(),
    )
}

async fn connect_permission_client(
    clients: &Clients,
    reply: PermissionReply,
) -> (
    Peer,
    tokio::sync::oneshot::Receiver<RequestPermissionRequest>,
    tokio::sync::oneshot::Sender<()>,
) {
    let id = clients.new_id();
    let (engine, transport) = Channel::duplex();
    let (peer_tx, peer_rx) = tokio::sync::oneshot::channel();
    let peer_tx = Arc::new(std::sync::Mutex::new(Some(peer_tx)));
    let (asked_tx, asked_rx) = tokio::sync::oneshot::channel();
    let asked_tx = Arc::new(std::sync::Mutex::new(Some(asked_tx)));
    let (close_tx, close_rx) = tokio::sync::oneshot::channel();
    let (drop_tx, drop_rx) = tokio::sync::oneshot::channel();
    let drop_tx = Arc::new(std::sync::Mutex::new(Some(drop_tx)));
    let reply = Arc::new(std::sync::Mutex::new(Some(reply)));
    let server_clients = clients.clone();

    tokio::spawn(async move {
        let _ = Agent
            .builder()
            .on_receive_request(
                async move |_: InitializeRequest, responder, connection| {
                    server_clients.connect(id, &connection, ClientInfo::default());
                    if let Some(peer_tx) = peer_tx
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .take()
                    {
                        let _ = peer_tx.send(server_clients.peer(id, &connection));
                    }
                    responder.respond(
                        InitializeResponse::new(agent_client_protocol::schema::ProtocolVersion::V1)
                            .agent_capabilities(AgentCapabilities::new()),
                    )
                },
                agent_client_protocol::on_receive_request!(),
            )
            .connect_to(engine)
            .await;
    });

    tokio::spawn(async move {
        let _ = Client
            .builder()
            .on_receive_request(
                async move |request: RequestPermissionRequest, responder, _connection| {
                    if let Some(asked_tx) = asked_tx
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .take()
                    {
                        let _ = asked_tx.send(request);
                    }
                    let action = reply
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .take()
                        .unwrap_or(PermissionReply::DropConnection);
                    match action {
                        PermissionReply::Select(option) => responder.respond(
                            RequestPermissionResponse::new(RequestPermissionOutcome::Selected(
                                SelectedPermissionOutcome::new(option),
                            )),
                        ),
                        PermissionReply::Error(error) => responder.respond_with_error(error),
                        PermissionReply::DropConnection => {
                            if let Some(drop_tx) = drop_tx
                                .lock()
                                .unwrap_or_else(|error| error.into_inner())
                                .take()
                            {
                                let _ = drop_tx.send(());
                            }
                            std::future::pending::<Result<(), Error>>().await
                        }
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .connect_with(transport, async move |connection| {
                connection
                    .send_request(InitializeRequest::new(
                        agent_client_protocol::schema::ProtocolVersion::V1,
                    ))
                    .block_task()
                    .await?;
                tokio::select! {
                    _ = close_rx => {}
                    _ = drop_rx => {}
                }
                Ok(())
            })
            .await;
    });

    (peer_rx.await.expect("initialized"), asked_rx, close_tx)
}

fn selected_option(response: RequestPermissionResponse) -> Option<String> {
    match response.outcome {
        RequestPermissionOutcome::Selected(selected) => Some(selected.option_id.0.to_string()),
        _ => None,
    }
}

#[tokio::test]
async fn approval_waits_for_a_client_to_attach_and_returns_its_answer() {
    let clients = Clients::default();
    let session = clients.session("detached");
    let task = tokio::spawn({
        let session = session.clone();
        async move {
            session
                .request_approval(sample_permission("detached"))
                .await
        }
    });

    tokio::time::sleep(Duration::from_millis(25)).await;
    assert!(
        !task.is_finished(),
        "approval must wait when no client is attached"
    );

    let (peer, asked_rx, _close) =
        connect_permission_client(&clients, PermissionReply::Select("allow_once")).await;
    peer.attach("detached");

    let asked = tokio::time::timeout(Duration::from_secs(5), asked_rx)
        .await
        .expect("delivered")
        .expect("request");
    assert_eq!(asked.session_id.0.as_ref(), "detached");
    let response = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("answered")
        .expect("join")
        .expect("ok");
    assert_eq!(selected_option(response).as_deref(), Some("allow_once"));
}

#[tokio::test]
async fn approval_wakes_on_resume_open_and_attach_open() {
    let clients = Clients::default();
    let session = clients.session("resumed");
    let cursor = clients.head("resumed");

    let resume_task = tokio::spawn({
        let session = session.clone();
        async move { session.request_approval(sample_permission("resumed")).await }
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    let (resumer, _asked, _close) =
        connect_permission_client(&clients, PermissionReply::Select("allow_resumed")).await;
    resumer
        .resume_open("resumed", cursor)
        .expect("send")
        .expect("resumed");
    let response = tokio::time::timeout(Duration::from_secs(5), resume_task)
        .await
        .expect("answered")
        .expect("join")
        .expect("ok");
    assert_eq!(selected_option(response).as_deref(), Some("allow_resumed"));

    let open_session = clients.session("reopened");
    let open_task = tokio::spawn({
        let session = open_session.clone();
        async move {
            session
                .request_approval(sample_permission("reopened"))
                .await
        }
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    let (loader, _asked, _close) =
        connect_permission_client(&clients, PermissionReply::Select("allow_reopened")).await;
    loader
        .attach_open("reopened", None, Vec::new(), false)
        .expect("send")
        .expect("attached");
    let response = tokio::time::timeout(Duration::from_secs(5), open_task)
        .await
        .expect("answered")
        .expect("join")
        .expect("ok");
    assert_eq!(selected_option(response).as_deref(), Some("allow_reopened"));
}

#[tokio::test]
async fn approval_redelivers_after_client_disconnects_mid_request() {
    let clients = Clients::default();
    let (first, first_asked, _first_close) =
        connect_permission_client(&clients, PermissionReply::DropConnection).await;
    first.attach("dropping");

    let session = clients.session("dropping");
    let task = tokio::spawn(async move {
        session
            .request_approval(sample_permission("dropping"))
            .await
    });

    tokio::time::timeout(Duration::from_secs(5), first_asked)
        .await
        .expect("first client received request")
        .expect("request");
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(
        !task.is_finished(),
        "approval must keep waiting after the first client drops"
    );

    let (second, second_asked, _second_close) =
        connect_permission_client(&clients, PermissionReply::Select("allow_after_drop")).await;
    second.attach("dropping");

    tokio::time::timeout(Duration::from_secs(5), second_asked)
        .await
        .expect("second client received redelivered request")
        .expect("request");
    let response = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("answered")
        .expect("join")
        .expect("ok");
    assert_eq!(
        selected_option(response).as_deref(),
        Some("allow_after_drop")
    );
}

#[tokio::test]
async fn approval_fails_over_to_another_attached_client_when_newest_drops() {
    let clients = Clients::default();
    let (older, older_asked, _older_close) =
        connect_permission_client(&clients, PermissionReply::Select("allow_older")).await;
    older.attach("two-clients");

    let (newer, newer_asked, _newer_close) =
        connect_permission_client(&clients, PermissionReply::DropConnection).await;
    newer.attach("two-clients");

    let session = clients.session("two-clients");
    let task = tokio::spawn(async move {
        session
            .request_approval(sample_permission("two-clients"))
            .await
    });

    tokio::time::timeout(Duration::from_secs(5), newer_asked)
        .await
        .expect("newest client asked first")
        .expect("request");
    tokio::time::timeout(Duration::from_secs(5), older_asked)
        .await
        .expect("older client asked on failover")
        .expect("request");

    let response = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("answered")
        .expect("join")
        .expect("ok");
    assert_eq!(selected_option(response).as_deref(), Some("allow_older"));
}

#[tokio::test]
async fn approval_returns_client_rpc_error_without_waiting() {
    let clients = Clients::default();
    let (peer, _asked, _close) =
        connect_permission_client(&clients, PermissionReply::Error(Error::method_not_found()))
            .await;
    peer.attach("rpc-error");

    let session = clients.session("rpc-error");
    let error = tokio::time::timeout(
        Duration::from_secs(5),
        session.request_approval(sample_permission("rpc-error")),
    )
    .await
    .expect("returned promptly")
    .expect_err("rpc error propagates");
    assert_eq!(error.code, agent_client_protocol::ErrorCode::MethodNotFound);
}

#[tokio::test]
async fn approval_aborts_on_cancel_forget_and_shutdown() {
    let clients = Clients::default();

    let cancelled_session = clients.session("cancelled");
    let cancel_task = tokio::spawn({
        let session = cancelled_session.clone();
        async move {
            session
                .request_approval(sample_permission("cancelled"))
                .await
        }
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    clients.cancel_approvals("cancelled");
    assert!(
        tokio::time::timeout(Duration::from_secs(5), cancel_task)
            .await
            .expect("aborted")
            .expect("join")
            .is_err()
    );

    let (peer, _asked, _close) =
        connect_permission_client(&clients, PermissionReply::Select("allow_next")).await;
    peer.attach("cancelled");
    let next = tokio::time::timeout(
        Duration::from_secs(5),
        cancelled_session.request_approval(sample_permission("cancelled")),
    )
    .await
    .expect("subsequent approval succeeds")
    .expect("ok");
    assert_eq!(selected_option(next).as_deref(), Some("allow_next"));

    let forgotten_session = clients.session("forgotten");
    let forget_task = tokio::spawn(async move {
        forgotten_session
            .request_approval(sample_permission("forgotten"))
            .await
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    clients.forget_session("forgotten");
    assert!(
        tokio::time::timeout(Duration::from_secs(5), forget_task)
            .await
            .expect("aborted")
            .expect("join")
            .is_err()
    );

    let shutdown_session = clients.session("shutdown");
    let shutdown_task = tokio::spawn({
        let session = shutdown_session.clone();
        async move {
            session
                .request_approval(sample_permission("shutdown"))
                .await
        }
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    clients.shutdown();
    assert!(
        tokio::time::timeout(Duration::from_secs(5), shutdown_task)
            .await
            .expect("aborted")
            .expect("join")
            .is_err()
    );
    assert!(
        tokio::time::timeout(
            Duration::from_secs(5),
            shutdown_session.request_approval(sample_permission("shutdown")),
        )
        .await
        .expect("returns immediately after shutdown")
        .is_err()
    );
}
