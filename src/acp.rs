use agent_client_protocol::schema::v1::{
    AgentCapabilities, ContentBlock, ContentChunk, InitializeRequest, InitializeResponse,
    NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse, SessionNotification,
    SessionUpdate, StopReason, TextContent,
};
use agent_client_protocol::{Agent, Stdio};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

struct AcpSession {
    state: Arc<Mutex<crate::app::AppState>>,
    cwd: PathBuf,
}

type Sessions = Arc<Mutex<HashMap<String, AcpSession>>>;

pub async fn run_acp() -> Result<(), Box<dyn std::error::Error>> {
    let sessions: Sessions = Arc::new(Mutex::new(HashMap::new()));
    Agent
        .builder()
        .name("rustcode")
        .on_receive_request(
            async |request: InitializeRequest, responder, _connection| {
                responder.respond(
                    InitializeResponse::new(request.protocol_version)
                        .agent_capabilities(AgentCapabilities::new()),
                )
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let sessions = Arc::clone(&sessions);
                async move |request: NewSessionRequest, responder, _connection| {
                    let session_id = format!("rustcode-{}", uuid_like_id());
                    let mut state = crate::app::AppState::new();
                    state.raw_cli_mode = false;
                    state.workspace_root = Some(request.cwd.clone());
                    crate::tools::set_active_workspace_root(Some(request.cwd.clone()));
                    sessions.lock().await.insert(
                        session_id.clone(),
                        AcpSession {
                            state: Arc::new(Mutex::new(state)),
                            cwd: request.cwd,
                        },
                    );
                    responder.respond(NewSessionResponse::new(session_id))
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let sessions = Arc::clone(&sessions);
                async move |request: PromptRequest, responder, connection| {
                    let session_id = request.session_id.to_string();
                    let text = prompt_text(&request.prompt);
                    let session = sessions
                        .lock()
                        .await
                        .get(&session_id)
                        .map(|session| (Arc::clone(&session.state), session.cwd.clone()));
                    let Some((state, cwd)) = session else {
                        return Err(agent_client_protocol::Error::invalid_params()
                            .data(format!("unknown ACP session: {session_id}")));
                    };

                    crate::tools::set_active_session_id(Some(session_id.clone()));
                    crate::tools::set_active_workspace_root(Some(cwd));
                    state
                        .lock()
                        .await
                        .history
                        .push(crate::app::ChatMessage::new("user", text));

                    let client = reqwest::Client::builder()
                        .connect_timeout(std::time::Duration::from_secs(10))
                        .build()
                        .map_err(|error| {
                            agent_client_protocol::Error::internal_error().data(error.to_string())
                        })?;
                    let prose = crate::raw_cli::run_headless_turn(&client, state)
                        .await
                        .map_err(|error| {
                            agent_client_protocol::Error::internal_error().data(error.to_string())
                        })?;

                    if !prose.is_empty() {
                        connection.send_notification(SessionNotification::new(
                            session_id,
                            SessionUpdate::AgentMessageChunk(ContentChunk::new(
                                ContentBlock::Text(TextContent::new(prose)),
                            )),
                        ))?;
                    }
                    responder.respond(PromptResponse::new(StopReason::EndTurn))
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_to(Stdio::new())
        .await?;
    Ok(())
}

fn prompt_text(prompt: &[ContentBlock]) -> String {
    prompt
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect()
}

fn uuid_like_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    NEXT_ID.fetch_add(1, Ordering::Relaxed).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::{ContentBlock, TextContent};

    #[test]
    fn extracts_text_blocks_from_prompt() {
        let prompt = vec![
            ContentBlock::Text(TextContent::new("inspect the project")),
            ContentBlock::Text(TextContent::new(" and explain it")),
        ];

        assert_eq!(prompt_text(&prompt), "inspect the project and explain it");
    }

    #[test]
    fn session_state_can_keep_workspace_root() {
        let mut state = crate::app::AppState::new();
        state.workspace_root = Some(PathBuf::from("/tmp/project"));
        assert_eq!(state.workspace_root, Some(PathBuf::from("/tmp/project")));
    }
}
