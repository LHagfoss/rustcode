use agent_client_protocol::schema::v1::{
    ContentBlock, SessionNotification, SessionUpdate, StopReason,
};
use agent_client_protocol::{Client, ConnectionTo};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

use super::permissions::AcpPolicy;
use super::session::ScheduledSessionTurn;
use super::streaming::{AcpEventStream, acp_stop_reason};

pub(crate) fn prompt_text(prompt: &[ContentBlock]) -> String {
    prompt
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect()
}

pub(crate) async fn run_prompt(
    state: Arc<Mutex<crate::app::AppState>>,
    cwd: PathBuf,
    scheduled_turn: ScheduledSessionTurn,
    session_id: String,
    text: String,
    connection: ConnectionTo<Client>,
    client: Arc<reqwest::Client>,
    auto_approve: bool,
) -> Result<StopReason, agent_client_protocol::Error> {
    let turn = scheduled_turn.begin().await;
    crate::tools::set_active_session_id(Some(session_id.clone()));
    crate::tools::set_active_workspace_root(Some(cwd.clone()));
    let prompt_tokens = text.split_whitespace().collect::<Vec<_>>();
    if prompt_tokens.first() == Some(&"/memory") && prompt_tokens.len() > 1 {
        if let Some(message) = crate::memory::command(Some(&cwd), &prompt_tokens[1..]) {
            state
                .lock()
                .await
                .history
                .push(crate::app::ChatMessage::new("system", message.clone()));
            return Ok(StopReason::EndTurn);
        }
    }
    state
        .lock()
        .await
        .history
        .push(crate::app::ChatMessage::new("user", text.clone()));

    let stream_buffer = Arc::new(Mutex::new(crate::network::StreamBuffer::new()));
    let policy = Arc::new(AcpPolicy {
        connection: connection.clone(),
        session_id: session_id.clone(),
        auto_approve,
    });
    let (events, mut receiver) = crate::network::AgentUiEventSender::channel();
    let mut event_stream = AcpEventStream::new();
    let mut run = Box::pin(crate::network::ui_adapter::run_agent_turn_with_events(
        &client,
        &state,
        turn.cancel_token(),
        &policy,
        &stream_buffer,
        text,
        events,
    ));
    let context = loop {
        tokio::select! {
            context = &mut run => break context,
            event = receiver.recv() => {
                let Some(event) = event else { continue };
                send_updates(&connection, &session_id, event_stream.updates(event))?;
            }
        }
    };
    while let Ok(event) = receiver.try_recv() {
        send_updates(&connection, &session_id, event_stream.updates(event))?;
    }
    let harness_reason = context
        .lifecycle
        .stop_reason
        .as_ref()
        .map(ToString::to_string);
    Ok(acp_stop_reason(
        turn.cancel_token().is_cancelled(),
        harness_reason.as_deref(),
    ))
}

fn send_updates(
    connection: &ConnectionTo<Client>,
    session_id: &str,
    updates: Vec<SessionUpdate>,
) -> Result<(), agent_client_protocol::Error> {
    for update in updates {
        connection.send_notification(SessionNotification::new(session_id.to_owned(), update))?;
    }
    Ok(())
}
