use super::*;
use crate::data_store::ProviderRequestRow;

#[test]
fn request_workspace_uses_session_boundary_without_process_cwd_fallback() {
    let source = tempfile::tempdir().unwrap();
    let state =
        crate::app::AppState::new_with_workspace_session(source.path(), Some("request-source"));

    assert_eq!(
        request_workspace_root(&state).as_deref(),
        Some(source.path())
    );
}

#[test]
fn request_session_attribution_survives_active_session_switch() {
    assert_eq!(
        resolve_request_session_id(Some("request-session"), "new-active-session"),
        "request-session"
    );
    assert_eq!(
        resolve_request_session_id(None, "active-session"),
        "active-session"
    );
}

#[tokio::test(start_paused = true)]
async fn silent_sse_stream_returns_after_idle_timeout() {
    let (_writer, reader) = tokio::io::duplex(64);
    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    let read = read_sse_line(&mut reader, &mut line);
    tokio::pin!(read);

    tokio::task::yield_now().await;
    tokio::time::advance(retry::STREAM_IDLE_TIMEOUT + std::time::Duration::from_millis(1)).await;

    let error = read.await.expect_err("silent stream must not hang forever");
    assert!(error.to_string().contains("stream_idle_timeout"), "{error}");
}

#[tokio::test(start_paused = true)]
async fn silent_sse_stream_before_first_event_has_distinct_timeout() {
    let (_writer, reader) = tokio::io::duplex(64);
    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    let read = read_sse_line_with_state(&mut reader, &mut line, true, retry::FIRST_EVENT_TIMEOUT);
    tokio::pin!(read);

    tokio::task::yield_now().await;
    tokio::time::advance(retry::FIRST_EVENT_TIMEOUT + std::time::Duration::from_millis(1)).await;

    let error = read
        .await
        .expect_err("first event must have a bounded wait");
    assert_eq!(error.kind(), StreamFailureKind::FirstEventTimeout);
    assert_eq!(error.partial_event_bytes(), 0);
}

#[tokio::test]
async fn sse_data_before_idle_timeout_is_returned_normally() {
    let (mut writer, reader) = tokio::io::duplex(64);
    let mut reader = BufReader::new(reader);
    let writer_task = tokio::spawn(async move {
        tokio::io::AsyncWriteExt::write_all(&mut writer, b"data: ready\n")
            .await
            .unwrap();
    });
    let mut line = String::new();

    let bytes = read_sse_line(&mut reader, &mut line)
        .await
        .expect("data should arrive before the idle timeout");

    assert_eq!(bytes, "data: ready\n".len());
    assert_eq!(line, "data: ready\n");
    writer_task.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn partial_sse_bytes_reset_idle_timeout_until_line_completes() {
    let (mut writer, reader) = tokio::io::duplex(64);
    let read_task = tokio::spawn(async move {
        let mut reader = BufReader::new(reader);
        let mut line = String::new();
        let bytes = read_sse_line(&mut reader, &mut line)
            .await
            .map_err(|error| error.to_string())?;
        Ok::<_, String>((bytes, line))
    });

    tokio::io::AsyncWriteExt::write_all(&mut writer, b"data:")
        .await
        .unwrap();
    tokio::task::yield_now().await;
    tokio::time::advance(retry::STREAM_IDLE_TIMEOUT - std::time::Duration::from_secs(1)).await;
    tokio::io::AsyncWriteExt::write_all(&mut writer, b" still")
        .await
        .unwrap();
    tokio::task::yield_now().await;
    tokio::time::advance(retry::STREAM_IDLE_TIMEOUT - std::time::Duration::from_secs(1)).await;
    tokio::io::AsyncWriteExt::write_all(&mut writer, b" alive\n")
        .await
        .unwrap();

    let (bytes, line) = read_task.await.unwrap().unwrap();
    assert_eq!(bytes, "data: still alive\n".len());
    assert_eq!(line, "data: still alive\n");
}

#[tokio::test(start_paused = true)]
async fn partial_sse_bytes_then_stall_returns_idle_timeout() {
    let (mut writer, reader) = tokio::io::duplex(64);
    let read_task = tokio::spawn(async move {
        let mut reader = BufReader::new(reader);
        read_sse_line(&mut reader, &mut String::new()).await
    });

    tokio::io::AsyncWriteExt::write_all(&mut writer, b"data: partial")
        .await
        .unwrap();
    tokio::task::yield_now().await;
    tokio::time::advance(retry::STREAM_IDLE_TIMEOUT + std::time::Duration::from_millis(1)).await;

    let error = read_task
        .await
        .unwrap()
        .expect_err("partial line must time out after a stall");
    assert!(error.to_string().contains("stream_idle_timeout"), "{error}");
}

#[tokio::test]
async fn sse_progress_deadline_uses_first_event_budget_before_first_event() {
    let start = tokio::time::Instant::now();
    let first_event_deadline = start + retry::FIRST_EVENT_TIMEOUT;
    assert_eq!(
        sse_progress_deadline(first_event_deadline, start, 0),
        first_event_deadline
    );
}

#[tokio::test]
async fn sse_progress_deadline_tracks_last_progress_after_first_event() {
    let start = tokio::time::Instant::now();
    let first_event_deadline = start + retry::FIRST_EVENT_TIMEOUT;
    let later = start + std::time::Duration::from_secs(3600);
    assert_eq!(
        sse_progress_deadline(first_event_deadline, later, 2),
        later + retry::STREAM_IDLE_TIMEOUT
    );
}

#[tokio::test(start_paused = true)]
async fn keepalive_blank_lines_do_not_extend_first_event_deadline() {
    // Regression test for the hung-turn pattern: a provider or proxy that
    // emits periodic blank keep-alive lines must not postpone the
    // absolute first-event budget forever.
    let (mut writer, reader) = tokio::io::duplex(1024);
    let stream_start = tokio::time::Instant::now();
    let first_event_deadline = stream_start + retry::FIRST_EVENT_TIMEOUT;
    let read_task = tokio::spawn(async move {
        let mut reader = BufReader::new(reader);
        let mut events = 0usize;
        let last_progress = stream_start;
        loop {
            let deadline = sse_progress_deadline(first_event_deadline, last_progress, events);
            let mut line = String::new();
            match tokio::time::timeout_at(deadline, read_sse_line(&mut reader, &mut line)).await {
                Err(_) => return events,
                Ok(Ok(0)) => return events,
                Ok(Ok(_)) => {
                    if crate::network::parse_sse_line(line.trim()).is_some() {
                        events += 1;
                    }
                }
                Ok(Err(_)) => return events,
            }
        }
    });

    // Keep-alive blanks, each well inside the per-read budget.
    for _ in 0..3 {
        tokio::time::advance(retry::FIRST_EVENT_TIMEOUT / 4).await;
        tokio::task::yield_now().await;
        tokio::io::AsyncWriteExt::write_all(&mut writer, b"\n")
            .await
            .unwrap();
        tokio::task::yield_now().await;
    }
    // Push virtual time past the absolute first-event budget.
    tokio::time::advance(retry::FIRST_EVENT_TIMEOUT).await;
    tokio::task::yield_now().await;

    assert_eq!(
        read_task.await.unwrap(),
        0,
        "blank keep-alives must not count as progress"
    );
}

#[tokio::test]
async fn sse_eof_returns_final_unterminated_line() {
    let (mut writer, reader) = tokio::io::duplex(64);
    tokio::io::AsyncWriteExt::write_all(&mut writer, b"data: final")
        .await
        .unwrap();
    drop(writer);
    let mut reader = BufReader::new(reader);
    let mut line = String::new();

    let bytes = read_sse_line(&mut reader, &mut line).await.unwrap();

    assert_eq!(bytes, "data: final".len());
    assert_eq!(line, "data: final");
}

async fn stream_test_state(
    endpoint: &str,
) -> (
    std::sync::Arc<tokio::sync::Mutex<crate::app::AppState>>,
    String,
) {
    let state = std::sync::Arc::new(tokio::sync::Mutex::new(crate::app::AppState::new()));
    let session_id = {
        let mut state = state.lock().await;
        state.api_base_url = endpoint.to_owned();
        state.model_name = "stream-test".to_owned();
        state.config.models = vec![crate::config::ModelProfile {
            name: "stream-test".to_owned(),
            url: endpoint.to_owned(),
            model: "stream-test".to_owned(),
            context_window: Some(8_192),
            max_tokens: Some(1_024),
            ..Default::default()
        }];
        state.active_session_id.clone()
    };
    (state, session_id)
}

async fn read_http_request(socket: &mut tokio::net::TcpStream) -> Vec<u8> {
    use tokio::io::AsyncReadExt;

    let mut request = Vec::new();
    let mut chunk = [0_u8; 1024];
    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
        let bytes = socket.read(&mut chunk).await.expect("read request");
        if bytes == 0 {
            break;
        }
        request.extend_from_slice(&chunk[..bytes]);
    }
    request
}

async fn write_sse_response(socket: &mut tokio::net::TcpStream, status: &str, body: &[u8]) {
    use tokio::io::AsyncWriteExt;

    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    socket
        .write_all(header.as_bytes())
        .await
        .expect("write response headers");
    socket.write_all(body).await.expect("write response body");
}

#[tokio::test]
async fn stream_request_flushes_final_sse_content_and_records_usage() {
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!(
        "http://{}/v1/chat/completions",
        listener.local_addr().unwrap()
    );
    let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"final answer\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":3,\"total_tokens\":15}}\n\n",
            "data: [DONE]\n\n"
        )
        .as_bytes()
        .to_vec();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        read_http_request(&mut socket).await;
        write_sse_response(&mut socket, "200 OK", &body).await;
        socket.shutdown().await.unwrap();
    });

    let (state, session_id) = stream_test_state(&endpoint).await;
    let buffer = std::sync::Arc::new(tokio::sync::Mutex::new(StreamBuffer::new()));
    let finish = stream_request(
        &reqwest::Client::new(),
        state.clone(),
        tokio_util::sync::CancellationToken::new(),
        &endpoint,
        "stream-test",
        vec![serde_json::json!({"role": "user", "content": "hello"})],
        buffer.clone(),
        false,
        false,
        ThinkingMode::Normal,
        crate::tools::ToolSchemaPolicy::read_only_inspection(),
        Some(&session_id),
        None,
    )
    .await
    .expect("SSE response should complete");

    assert_eq!(finish.as_deref(), Some("stop"));
    assert_eq!(buffer.lock().await.content, "final answer");
    assert_eq!(
        buffer.lock().await.final_answer_boundary,
        super::super::stream::FinalAnswerBoundary::ReasoningClosed,
        "a terminal prose response without a reasoning delta still closes the answer boundary"
    );
    assert_eq!(
        buffer.lock().await.provider_final_answer_state,
        super::super::stream::ProviderFinalAnswerState::Terminal
    );
    assert_eq!(
        state.lock().await.current_token_usage,
        Some(crate::app::TokenUsage {
            prompt_tokens: 12,
            completion_tokens: 3,
            total_tokens: 15,
            ..Default::default()
        })
    );
    server.await.unwrap();
}

#[tokio::test]
async fn messages_sse_pairs_fragmented_native_calls_and_requires_message_stop() {
    use serde_json::json;
    use tokio::io::AsyncWriteExt;
    for complete in [true, false] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/v1/messages", listener.local_addr().unwrap());
        let mut events = vec![
            json!({"type":"message_start","message":{"usage":{"input_tokens":12,"output_tokens":0}}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"call_read","name":"read_file","input":{}}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"\"a.rs\"}"}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":9}}),
        ];
        if complete {
            events.push(json!({"type":"message_stop"}));
        }
        let body = events
            .iter()
            .map(|event| format!("data: {event}\n\n"))
            .collect::<String>()
            .into_bytes();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let request = read_http_request(&mut socket).await;
            write_sse_response(&mut socket, "200 OK", &body).await;
            socket.shutdown().await.unwrap();
            request
        });
        let (state, session) = stream_test_state(&endpoint).await;
        {
            let mut state = state.lock().await;
            let profile = &mut state.config.models[0];
            profile.api_protocol = Some(crate::config::ApiProtocol::AnthropicMessages);
            profile.tool_protocol = Some(crate::config::ToolProtocol::ApiNative);
        }
        let buffer = Arc::new(Mutex::new(StreamBuffer::new()));
        let result = stream_request(&reqwest::Client::new(), state.clone(), tokio_util::sync::CancellationToken::new(), &endpoint, "stream-test", vec![json!({"role":"system","content":"Helpful"}),json!({"role":"user","content":"Context notice"}),json!({"role":"user","content":[{"type":"text","text":"Inspect"},{"type":"image_url","image_url":{"url":"data:image/png;base64,aGVsbG8="}}]})], buffer.clone(), false, false, ThinkingMode::Normal,crate::tools::ToolSchemaPolicy::read_only_inspection(),Some(&session),None).await;
        if complete {
            assert_eq!(result.unwrap().as_deref(), Some("tool_calls"));
            let buffer = buffer.lock().await;
            assert_eq!(buffer.tool_call_ids, vec!["call_read"]);
            assert_eq!(buffer.native_tool_calls.len(), 1);
            assert_eq!(buffer.native_tool_calls[0].arguments["path"], "a.rs");
            assert_eq!(
                state
                    .lock()
                    .await
                    .current_token_usage
                    .as_ref()
                    .unwrap()
                    .total_tokens,
                21
            );
        } else {
            assert_eq!(result.unwrap_err().kind, StreamFailureKind::PrematureEof);
            assert!(buffer.lock().await.native_tool_calls.is_empty());
        }
        let request = String::from_utf8_lossy(&server.await.unwrap()).into_owned();
        assert!(request.contains("anthropic-version: 2023-06-01"));
        assert!(request.contains("\"system\""));
        let body = request.split_once("\r\n\r\n").unwrap().1;
        let payload: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(
            payload["messages"][0]["content"][0]["text"],
            "Context notice"
        );
        assert_eq!(payload["messages"][0]["content"][1]["text"], "Inspect");
        assert_eq!(payload["messages"][0]["content"][2]["type"], "image");
    }
}

#[tokio::test]
async fn responses_done_without_completed_is_a_premature_stream_failure() {
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/v1/responses", listener.local_addr().unwrap());
    let body = concat!(
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\n",
        "data: [DONE]\n\n"
    )
    .as_bytes()
    .to_vec();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        read_http_request(&mut socket).await;
        write_sse_response(&mut socket, "200 OK", &body).await;
        socket.shutdown().await.unwrap();
    });

    let (state, session_id) = stream_test_state(&endpoint).await;
    state.lock().await.config.models[0].api_protocol = Some(crate::config::ApiProtocol::Responses);
    let buffer = std::sync::Arc::new(tokio::sync::Mutex::new(StreamBuffer::new()));
    let error = stream_request(
        &reqwest::Client::new(),
        state,
        tokio_util::sync::CancellationToken::new(),
        &endpoint,
        "stream-test",
        vec![serde_json::json!({"role": "user", "content": "hello"})],
        buffer.clone(),
        false,
        false,
        ThinkingMode::Normal,
        crate::tools::ToolSchemaPolicy::read_only_inspection(),
        Some(&session_id),
        None,
    )
    .await
    .expect_err("[DONE] cannot substitute for Responses response.completed");

    assert_eq!(error.kind, StreamFailureKind::PrematureEof);
    assert!(
        error
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("before its completion event"))
    );
    assert_eq!(buffer.lock().await.content, "partial");
    server.await.unwrap();
}

#[tokio::test]
async fn responses_max_output_tokens_keeps_partial_text_and_returns_length() {
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/v1/responses", listener.local_addr().unwrap());
    let body = concat!(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial answer\"}\n\n",
            "data: {\"type\":\"response.incomplete\",\"response\":{\"incomplete_details\":{\"reason\":\"max_output_tokens\"},\"usage\":{\"input_tokens\":12,\"output_tokens\":20,\"total_tokens\":32}}}\n\n",
            "data: [DONE]\n\n"
        )
        .as_bytes()
        .to_vec();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        read_http_request(&mut socket).await;
        write_sse_response(&mut socket, "200 OK", &body).await;
        socket.shutdown().await.unwrap();
    });

    let (state, session_id) = stream_test_state(&endpoint).await;
    state.lock().await.config.models[0].api_protocol = Some(crate::config::ApiProtocol::Responses);
    let buffer = Arc::new(Mutex::new(StreamBuffer::new()));
    let finish = stream_request(
        &reqwest::Client::new(),
        state.clone(),
        tokio_util::sync::CancellationToken::new(),
        &endpoint,
        "stream-test",
        vec![serde_json::json!({"role": "user", "content": "hello"})],
        buffer.clone(),
        false,
        false,
        ThinkingMode::Normal,
        crate::tools::ToolSchemaPolicy::read_only_inspection(),
        Some(&session_id),
        None,
    )
    .await
    .expect("output-budget exhaustion is a recoverable terminal response");

    assert_eq!(finish.as_deref(), Some("length"));
    assert_eq!(buffer.lock().await.content, "partial answer");
    assert_eq!(
        state.lock().await.current_token_usage,
        Some(crate::app::TokenUsage {
            prompt_tokens: 12,
            completion_tokens: 20,
            total_tokens: 32,
            ..Default::default()
        })
    );
    server.await.unwrap();
}

// #2299: OpenAI reasoning summaries stream as one thought with a
// paragraph per part, also across reasoning items whose part index starts
// at 0 again; a part with no reasoning before it opens no thought, and
// the summary is marked so the message is labelled as one.
#[tokio::test]
async fn responses_reasoning_summaries_keep_paragraphs_and_are_marked() {
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/v1/responses", listener.local_addr().unwrap());
    let body = concat!(
            "data: {\"type\":\"response.reasoning_summary_part.added\",\"summary_index\":0}\n\n",
            "data: {\"type\":\"response.reasoning_summary_text.delta\",\"delta\":\"**Searching online**\"}\n\n",
            "data: {\"type\":\"response.reasoning_summary_part.added\",\"summary_index\":0}\n\n",
            "data: {\"type\":\"response.reasoning_summary_text.delta\",\"delta\":\"**Inspecting site**\"}\n\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Done.\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n",
        )
        .as_bytes()
        .to_vec();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        read_http_request(&mut socket).await;
        write_sse_response(&mut socket, "200 OK", &body).await;
        socket.shutdown().await.unwrap();
    });

    let (state, session_id) = stream_test_state(&endpoint).await;
    state.lock().await.config.models[0].api_protocol = Some(crate::config::ApiProtocol::Responses);
    let buffer = Arc::new(Mutex::new(StreamBuffer::new()));
    stream_request(
        &reqwest::Client::new(),
        state.clone(),
        tokio_util::sync::CancellationToken::new(),
        &endpoint,
        "stream-test",
        vec![serde_json::json!({"role": "user", "content": "hello"})],
        buffer.clone(),
        false,
        false,
        ThinkingMode::Normal,
        crate::tools::ToolSchemaPolicy::read_only_inspection(),
        Some(&session_id),
        None,
    )
    .await
    .expect("summary stream completes");

    assert_eq!(
        buffer.lock().await.content,
        "<think>\n**Searching online**\n\n**Inspecting site**\n</think>\n\nDone."
    );
    let mut state = state.lock().await;
    assert!(state.history.reasoning_summary());
    let message = crate::app::ChatMessage::new("assistant", buffer.lock().await.content.clone());
    state.history.push(message);
    assert!(state.history.last().unwrap().reasoning_summary);
    server.await.unwrap();
}

#[tokio::test]
async fn chatgpt_credentials_never_reach_a_handcrafted_endpoint() {
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/v1/responses", listener.local_addr().unwrap());
    let (state, session_id) = stream_test_state(&endpoint).await;
    {
        let mut state = state.lock().await;
        state.config.models[0].api_protocol = Some(crate::config::ApiProtocol::Responses);
        state.config.models[0].api_key = Some("must-never-be-used-as-fallback".into());
        state.config.models[0].credential = Some(crate::provider_auth::CredentialRef {
            provider: "openai".into(),
            account: "account-1".into(),
            method: crate::provider_auth::AuthMethod::ChatGpt,
        });
    }
    let result = stream_request(
        &reqwest::Client::new(),
        state,
        tokio_util::sync::CancellationToken::new(),
        &endpoint,
        "stream-test",
        vec![serde_json::json!({"role":"user","content":"hello"})],
        std::sync::Arc::new(tokio::sync::Mutex::new(StreamBuffer::new())),
        false,
        false,
        ThinkingMode::Normal,
        crate::tools::ToolSchemaPolicy::read_only_inspection(),
        Some(&session_id),
        None,
    )
    .await;
    let failure = result.expect_err("unsupported endpoint must fail locally");
    assert!(
        failure
            .detail
            .unwrap()
            .contains("public https://api.openai.com/v1/responses")
    );
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), listener.accept())
            .await
            .is_err(),
        "the local endpoint must never receive the ChatGPT token or legacy API key"
    );
}

#[tokio::test]
async fn ordinary_api_key_responses_keep_existing_auth_headers() {
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/v1/responses", listener.local_addr().unwrap());
    let body = concat!(
        "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n",
        "data: [DONE]\n\n"
    )
    .as_bytes()
    .to_vec();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let request = read_http_request(&mut socket).await;
        write_sse_response(&mut socket, "200 OK", &body).await;
        socket.shutdown().await.unwrap();
        request
    });
    let (state, session_id) = stream_test_state(&endpoint).await;
    {
        let mut state = state.lock().await;
        state.config.models[0].api_protocol = Some(crate::config::ApiProtocol::Responses);
        state.config.models[0].api_key = Some("legacy-key".into());
    }
    stream_request(
        &reqwest::Client::new(),
        state,
        tokio_util::sync::CancellationToken::new(),
        &endpoint,
        "stream-test",
        vec![serde_json::json!({"role":"user","content":"hello"})],
        std::sync::Arc::new(tokio::sync::Mutex::new(StreamBuffer::new())),
        false,
        false,
        ThinkingMode::Normal,
        crate::tools::ToolSchemaPolicy::read_only_inspection(),
        Some(&session_id),
        None,
    )
    .await
    .expect("ordinary API-key Responses request should complete");
    let request = String::from_utf8(server.await.unwrap())
        .unwrap()
        .to_ascii_lowercase();
    assert!(request.contains("authorization: bearer legacy-key"));
    assert!(!request.contains("x-api-key:"));
}

#[tokio::test]
async fn stream_request_rejects_subagent_tool_result_interleaving_before_send() {
    use tokio::io::AsyncWriteExt;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!(
        "http://{}/v1/chat/completions",
        listener.local_addr().unwrap()
    );
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        read_http_request(&mut socket).await;
        write_sse_response(
                &mut socket,
                "200 OK",
                b"data: {\"choices\":[{\"delta\":{\"content\":\"unexpected request\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
            )
            .await;
        socket.shutdown().await.unwrap();
    });
    let (state, session_id) = stream_test_state(&endpoint).await;
    {
        let mut state = state.lock().await;
        state.config.models[0].tool_protocol = Some(crate::config::ToolProtocol::ApiNative);
    }

    // This is the subagent transcript shape after it drops one call from a
    // batch: the in-flight call/result pair has a system notice between
    // them. Alignment carries that notice as a user message, which breaks
    // the provider's native tool-call transaction.
    let messages = vec![
        serde_json::json!({"role": "system", "content": "subagent instructions"}),
        serde_json::json!({"role": "user", "content": "inspect"}),
        serde_json::json!({
            "role": "assistant",
            "content": null,
            "tool_calls": [{
                "id": "call_kept",
                "type": "function",
                "function": {"name": "view_file", "arguments": "{}"}
            }]
        }),
        serde_json::json!({
            "role": "system",
            "content": "[Subagent tool batch kept 1 of 2 calls and dropped 1.]"
        }),
        serde_json::json!({
            "role": "tool", "tool_call_id": "call_kept", "content": "completed"
        }),
    ];

    let result = stream_request(
        &reqwest::Client::new(),
        state,
        tokio_util::sync::CancellationToken::new(),
        &endpoint,
        "stream-test",
        messages,
        std::sync::Arc::new(tokio::sync::Mutex::new(StreamBuffer::new())),
        true,
        true,
        ThinkingMode::Normal,
        crate::tools::ToolSchemaPolicy::subagent(),
        Some(&session_id),
        None,
    )
    .await;
    server.abort();
    let error = result.expect_err("interleaved native tool history must stop locally");

    assert_eq!(error.kind, StreamFailureKind::ProviderError);
    assert!(error.detail.as_deref().is_some_and(|detail| {
        detail.contains("Native tool history") && detail.contains("call_kept")
    }));
}

#[tokio::test]
async fn stream_request_times_out_when_successful_response_body_stays_silent() {
    use std::time::Duration;
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!(
        "http://{}/v1/chat/completions",
        listener.local_addr().unwrap()
    );
    let (accepted_tx, accepted_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        read_http_request(&mut socket).await;
        socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: keep-alive\r\n\r\n",
                )
                .await
                .expect("write successful response headers");
        accepted_tx.send(()).unwrap();
        let _ = release_rx.await;
        let _ = socket.shutdown().await;
    });

    let (state, session_id) = stream_test_state(&endpoint).await;
    let request_endpoint = endpoint.clone();
    let task = tokio::spawn(async move {
        stream_request_with_timeouts(
            &reqwest::Client::new(),
            state,
            tokio_util::sync::CancellationToken::new(),
            &request_endpoint,
            "stream-test",
            vec![serde_json::json!({"role": "user", "content": "hello"})],
            std::sync::Arc::new(tokio::sync::Mutex::new(StreamBuffer::new())),
            false,
            false,
            ThinkingMode::Normal,
            crate::tools::ToolSchemaPolicy::read_only_inspection(),
            Some(&session_id),
            None,
            Duration::from_millis(100),
            Duration::from_millis(100),
            None,
            false,
        )
        .await
    });

    tokio::time::timeout(Duration::from_secs(5), accepted_rx)
        .await
        .expect("provider must accept the request")
        .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("silent successful stream must respect the first-event deadline")
        .expect("stream request must not panic");
    let error = result.expect_err("silent successful response must time out");
    assert_eq!(error.kind, StreamFailureKind::FirstEventTimeout);

    release_tx.send(()).unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn stream_request_cancellation_interrupts_a_pending_response() {
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!(
        "http://{}/v1/chat/completions",
        listener.local_addr().unwrap()
    );
    let (accepted_tx, accepted_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        read_http_request(&mut socket).await;
        accepted_tx.send(()).unwrap();
        let _ = release_rx.await;
        let _ = socket.shutdown().await;
    });

    let (state, session_id) = stream_test_state(&endpoint).await;
    let cancel = tokio_util::sync::CancellationToken::new();
    let task_cancel = cancel.clone();
    let task = tokio::spawn(async move {
        let client = reqwest::Client::new();
        stream_request(
            &client,
            state,
            task_cancel,
            &endpoint,
            "stream-test",
            vec![serde_json::json!({"role": "user", "content": "hello"})],
            std::sync::Arc::new(tokio::sync::Mutex::new(StreamBuffer::new())),
            false,
            false,
            ThinkingMode::Normal,
            crate::tools::ToolSchemaPolicy::read_only_inspection(),
            Some(&session_id),
            None,
        )
        .await
    });
    accepted_rx.await.unwrap();
    cancel.cancel();
    let error = task.await.unwrap().expect_err("request should cancel");
    assert_eq!(error.kind, StreamFailureKind::Cancelled);
    release_tx.send(()).unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn stream_request_cancellation_preserves_observed_sse_progress() {
    use std::time::Duration;
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!(
        "http://{}/v1/chat/completions",
        listener.local_addr().unwrap()
    );
    let event_line = "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n";
    let keep_alive = ": keep-alive\n\n";
    let expected_bytes = keep_alive.len() + event_line.len();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        read_http_request(&mut socket).await;
        socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: keep-alive\r\n\r\n",
                )
                .await
                .expect("write streaming headers");
        // Provider-only keep-alive comment followed by one model event,
        // mirroring DeepSeek Responses emitting `: keep-alive` comments.
        socket
            .write_all(keep_alive.as_bytes())
            .await
            .expect("write keep-alive");
        socket
            .write_all(event_line.as_bytes())
            .await
            .expect("write model event");
        socket.flush().await.expect("flush SSE progress");
        let _ = release_rx.await;
        let _ = socket.shutdown().await;
    });

    let (state, session_id) = stream_test_state(&endpoint).await;
    let buffer = std::sync::Arc::new(tokio::sync::Mutex::new(StreamBuffer::new()));
    let task_buffer = buffer.clone();
    let cancel = tokio_util::sync::CancellationToken::new();
    let task_cancel = cancel.clone();
    let task = tokio::spawn(async move {
        let client = reqwest::Client::new();
        stream_request(
            &client,
            state,
            task_cancel,
            &endpoint,
            "stream-test",
            vec![serde_json::json!({"role": "user", "content": "hello"})],
            task_buffer,
            false,
            false,
            ThinkingMode::Normal,
            crate::tools::ToolSchemaPolicy::read_only_inspection(),
            Some(&session_id),
            None,
        )
        .await
    });
    // Wait until the model event is deterministically consumed before
    // cancelling, so the test does not race the stream reader.
    let observed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if buffer.lock().await.content.contains("hi") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(
        observed.is_ok(),
        "model event must be observed before cancel"
    );
    cancel.cancel();
    let error = task
        .await
        .expect("stream task must not panic")
        .expect_err("cancelled stream must error");
    assert_eq!(error.kind, StreamFailureKind::Cancelled);
    assert_eq!(error.events_received, 1);
    assert_eq!(error.bytes_received, expected_bytes);
    release_tx.send(()).unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn stream_request_retries_a_retryable_http_response_before_streaming() {
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!(
        "http://{}/v1/chat/completions",
        listener.local_addr().unwrap()
    );
    let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"retried\"},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n"
        )
        .as_bytes()
        .to_vec();
    let server = tokio::spawn(async move {
        let (mut first, _) = listener.accept().await.unwrap();
        read_http_request(&mut first).await;
        write_sse_response(&mut first, "503 Service Unavailable", b"busy").await;
        first.shutdown().await.unwrap();

        let (mut second, _) = listener.accept().await.unwrap();
        read_http_request(&mut second).await;
        write_sse_response(&mut second, "200 OK", &body).await;
        second.shutdown().await.unwrap();
    });

    let (state, session_id) = stream_test_state(&endpoint).await;
    let buffer = std::sync::Arc::new(tokio::sync::Mutex::new(StreamBuffer::new()));
    let finish = stream_request(
        &reqwest::Client::new(),
        state,
        tokio_util::sync::CancellationToken::new(),
        &endpoint,
        "stream-test",
        vec![serde_json::json!({"role": "user", "content": "hello"})],
        buffer.clone(),
        false,
        false,
        ThinkingMode::Normal,
        crate::tools::ToolSchemaPolicy::read_only_inspection(),
        Some(&session_id),
        None,
    )
    .await
    .expect("retry should reach the successful stream");

    assert_eq!(finish.as_deref(), Some("stop"));
    assert_eq!(buffer.lock().await.content, "retried");
    server.await.unwrap();
}

// ------------------------------------------------ provider attempt rows

/// The rows the data store holds for one thread, read the way
/// `docs/data-store.md` documents them.
fn provider_request_rows(thread_id: &str) -> Vec<serde_json::Value> {
    let sql = format!(
        "SELECT attempt, status_code, error_kind, provider, model, method, endpoint_host, \
             endpoint_path, round, turn_id, request_bytes, response_bytes, stream_event_count, \
             input_tokens, output_tokens, cached_tokens, headers_at, finished_at, latency_ms, \
             request_payload_json, response_summary_json, payload_truncated \
             FROM provider_requests WHERE thread_id = '{thread_id}' ORDER BY attempt"
    );
    crate::data_store::query(&sql, 100)
        .map(|result| result.rows)
        .unwrap_or_default()
}

/// A row is written off the async runtime, so wait for the write instead
/// of assuming it landed with the request.
async fn wait_for_provider_request_rows(
    thread_id: &str,
    expected: usize,
) -> Vec<serde_json::Value> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        let rows = provider_request_rows(thread_id);
        if rows.len() >= expected || tokio::time::Instant::now() >= deadline {
            return rows;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

fn attempt_context(thread_id: &str, capture_payloads: bool) -> ProviderAttemptContext {
    ProviderAttemptContext {
        thread_id: thread_id.to_owned(),
        turn_id: Some("turn-unit".to_owned()),
        round: Some(2),
        provider: Some("openai".to_owned()),
        model: "stream-test".to_owned(),
        endpoint_host: Some("api.example.com".to_owned()),
        endpoint_path: Some("/v1/chat/completions".to_owned()),
        capture_payloads,
        payload: capture_payloads.then(|| {
            serde_json::json!({
                "model": "stream-test",
                "messages": [{"role": "user", "content": "hi"},
                             {"role": "user", "content": "Authorization: Bearer sk-live-4f9"}],
            })
        }),
    }
}

#[test]
fn provider_endpoint_parts_keep_the_host_and_drop_the_query() {
    let (host, path) =
        endpoint_parts("https://gateway.example.com/v1/chat/completions?api-key=sk-live-4f9#frag");
    assert_eq!(host.as_deref(), Some("gateway.example.com"));
    assert_eq!(path.as_deref(), Some("/v1/chat/completions"));

    let (host, path) = endpoint_parts("not a url");
    assert_eq!(host, None);
    assert_eq!(path, None);
}

#[test]
fn provider_label_prefers_the_credential_then_the_engine_then_the_name() {
    let credential = crate::config::ModelProfile {
        name: "chatgpt".to_owned(),
        url: "https://chatgpt.com/backend-api/codex/responses".to_owned(),
        model: "gpt-5".to_owned(),
        engine: Some("openai".to_owned()),
        credential: Some(crate::provider_auth::CredentialRef {
            provider: "openai".to_owned(),
            account: "default".to_owned(),
            method: crate::provider_auth::AuthMethod::ChatGpt,
        }),
        ..Default::default()
    };
    assert_eq!(provider_label(Some(&credential)).as_deref(), Some("openai"));

    let local = crate::config::ModelProfile {
        engine: Some("ollama".to_owned()),
        ..credential.clone()
    };
    let local = crate::config::ModelProfile {
        credential: None,
        ..local
    };
    assert_eq!(provider_label(Some(&local)).as_deref(), Some("ollama"));

    let bare = crate::config::ModelProfile {
        credential: None,
        engine: None,
        ..credential
    };
    assert_eq!(provider_label(Some(&bare)).as_deref(), Some("chatgpt"));
    assert_eq!(provider_label(None), None);
}

#[test]
fn provider_attempt_error_kind_names_the_short_cause() {
    let token = tokio_util::sync::CancellationToken::new();
    let context = attempt_context("thread-kind", false);

    let unanswered = ProviderAttempt::start(&context, 1, 0, &token);
    assert_eq!(
        unanswered.derived_error_kind().as_deref(),
        Some("connect"),
        "a send that never reached the provider"
    );

    let mut rejected = ProviderAttempt::start(&context, 1, 0, &token);
    rejected.headers(500);
    assert_eq!(rejected.derived_error_kind().as_deref(), Some("http_500"));

    let mut balance = ProviderAttempt::start(&context, 1, 0, &token);
    balance.headers(402);
    balance.provider_message("Insufficient balance");
    assert_eq!(
        balance.derived_error_kind().as_deref(),
        Some("insufficient_balance")
    );
    let mut limited = ProviderAttempt::start(&context, 1, 0, &token);
    limited.headers(429);
    limited.provider_message("rate_limit_error");
    assert_eq!(limited.derived_error_kind().as_deref(), Some("rate_limit"));

    let mut interrupted = ProviderAttempt::start(&context, 1, 0, &token);
    interrupted.headers(200);
    assert_eq!(
        interrupted.derived_error_kind().as_deref(),
        Some("stream"),
        "headers arrived but the stream did not finish"
    );

    let mut timed_out = ProviderAttempt::start(&context, 1, 0, &token);
    timed_out.fail("timeout");
    assert_eq!(
        timed_out.derived_error_kind().as_deref(),
        Some("timeout"),
        "an exit that knows why keeps its own kind"
    );

    let cancelling = tokio_util::sync::CancellationToken::new();
    let mut cancelled = ProviderAttempt::start(&context, 1, 0, &cancelling);
    cancelled.headers(200);
    cancelling.cancel();
    assert_eq!(cancelled.derived_error_kind().as_deref(), Some("cancelled"));
}

#[test]
fn attempt_summary_is_small_redacted_and_clipped() {
    let row = crate::data_store::ProviderRequestRow {
        status_code: Some(401),
        stream_event_count: Some(7),
        ..ProviderRequestRow::default()
    };
    let (summary, clipped) = attempt_summary_json(
        &row,
        Some("stop".to_owned()),
        Some("  rejected: Authorization: Bearer sk-live-4f9  "),
    );
    let summary: serde_json::Value = serde_json::from_str(&summary).expect("valid JSON");
    assert_eq!(summary["status"], 401);
    assert_eq!(summary["finish_reason"], "stop");
    assert_eq!(summary["events"], 7);
    assert!(
        !summary["error"].as_str().unwrap().contains("sk-live-4f9"),
        "a bearer token never reaches the store"
    );
    assert!(!clipped);

    let long = "x".repeat(MAX_ATTEMPT_ERROR_BYTES * 2);
    let (summary, clipped) = attempt_summary_json(&row, None, Some(&long));
    let summary: serde_json::Value = serde_json::from_str(&summary).expect("valid JSON");
    assert!(clipped);
    assert_eq!(
        summary["error"].as_str().unwrap().len(),
        MAX_ATTEMPT_ERROR_BYTES
    );

    let (empty, _) = attempt_summary_json(&ProviderRequestRow::default(), None, Some("   "));
    assert_eq!(empty, "{}");
}

#[test]
fn provider_attempt_without_capture_stores_metadata_only() {
    let thread = format!("attempt-metadata-{}", crate::controller::next_turn_id());
    let context = attempt_context(&thread, false);
    {
        let mut attempt = ProviderAttempt::start(
            &context,
            3,
            4_096,
            &tokio_util::sync::CancellationToken::new(),
        );
        attempt.headers(200);
        attempt.progress(5, 8_192);
        attempt.usage(1_200, 300, Some(64));
        attempt.completed(Some("stop".to_owned()));
    }
    let rows = provider_request_rows(&thread);
    assert_eq!(rows.len(), 1, "one row per attempt: {rows:?}");
    let row = &rows[0];
    assert_eq!(row["attempt"], 3);
    assert_eq!(row["status_code"], 200);
    assert_eq!(row["error_kind"], serde_json::Value::Null);
    assert_eq!(row["provider"], "openai");
    assert_eq!(row["model"], "stream-test");
    assert_eq!(row["method"], "POST");
    assert_eq!(row["endpoint_host"], "api.example.com");
    assert_eq!(row["endpoint_path"], "/v1/chat/completions");
    assert_eq!(row["round"], 2);
    assert_eq!(row["turn_id"], "turn-unit");
    assert_eq!(row["request_bytes"], 4_096);
    assert_eq!(row["response_bytes"], 8_192);
    assert_eq!(row["stream_event_count"], 5);
    assert_eq!(row["input_tokens"], 1_200);
    assert_eq!(row["output_tokens"], 300);
    assert_eq!(row["cached_tokens"], 64);
    assert!(row["headers_at"].as_str().is_some());
    assert!(row["finished_at"].as_str().is_some());
    assert!(row["latency_ms"].as_i64().is_some());
    assert_eq!(row["request_payload_json"], serde_json::Value::Null);
    assert_eq!(row["response_summary_json"], serde_json::Value::Null);
    assert_eq!(row["payload_truncated"], 0);
}

#[test]
fn provider_attempt_with_capture_stores_a_redacted_payload_and_summary() {
    let thread = format!("attempt-capture-{}", crate::controller::next_turn_id());
    let context = attempt_context(&thread, true);
    {
        let mut attempt = ProviderAttempt::start(
            &context,
            1,
            128,
            &tokio_util::sync::CancellationToken::new(),
        );
        attempt.headers(401);
        attempt.progress(0, 96);
        attempt.provider_message("rejected: Authorization: Bearer sk-live-4f9");
    }
    let rows = provider_request_rows(&thread);
    assert_eq!(rows.len(), 1, "one row per attempt: {rows:?}");
    let row = &rows[0];
    assert_eq!(row["error_kind"], "http_401");
    let stored = row["request_payload_json"]
        .as_str()
        .expect("the request is captured");
    let payload: serde_json::Value = serde_json::from_str(stored).expect("valid JSON");
    assert!(payload["messages"].is_array());
    assert!(
        !stored.contains("sk-live-4f9"),
        "a bearer token in the captured request is redacted"
    );
    let summary = row["response_summary_json"]
        .as_str()
        .expect("the summary is captured");
    let summary: serde_json::Value = serde_json::from_str(summary).expect("valid JSON");
    assert_eq!(summary["status"], 401);
    assert!(!summary["error"].as_str().unwrap().contains("sk-live-4f9"));
    assert_eq!(row["payload_truncated"], 0);
}

#[tokio::test]
async fn provider_request_rows_record_a_retry_and_the_stream_after_it() {
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    // A query string is where a gateway key travels: the row must not
    // keep it.
    let endpoint = format!(
        "http://{}/v1/chat/completions?api-key=sk-live-4f9",
        listener.local_addr().unwrap()
    );
    let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"retried\"},\"finish_reason\":\"stop\"}]}\n\n",
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":11,\"completion_tokens\":5,\"total_tokens\":16}}\n\n",
            "data: [DONE]\n\n"
        )
        .as_bytes()
        .to_vec();
    let body_len = body.len() as i64;
    let server = tokio::spawn(async move {
        let (mut first, _) = listener.accept().await.unwrap();
        read_http_request(&mut first).await;
        write_sse_response(&mut first, "503 Service Unavailable", b"busy").await;
        first.shutdown().await.unwrap();

        let (mut second, _) = listener.accept().await.unwrap();
        read_http_request(&mut second).await;
        write_sse_response(&mut second, "200 OK", &body).await;
        second.shutdown().await.unwrap();
    });

    let (state, session_id) = stream_test_state(&endpoint).await;
    let buffer = std::sync::Arc::new(tokio::sync::Mutex::new(StreamBuffer::new()));
    let finish = stream_request(
        &reqwest::Client::new(),
        state,
        tokio_util::sync::CancellationToken::new(),
        &endpoint,
        "stream-test",
        vec![serde_json::json!({"role": "user", "content": "hello"})],
        buffer.clone(),
        false,
        false,
        ThinkingMode::Normal,
        crate::tools::ToolSchemaPolicy::read_only_inspection(),
        Some(&session_id),
        None,
    )
    .await
    .expect("retry should reach the successful stream");
    assert_eq!(finish.as_deref(), Some("stop"));
    assert_eq!(buffer.lock().await.content, "retried");
    server.await.unwrap();

    let rows = wait_for_provider_request_rows(&session_id, 2).await;
    assert_eq!(rows.len(), 2, "a retry is its own row: {rows:?}");
    let retried = &rows[0];
    assert_eq!(retried["attempt"], 1);
    assert_eq!(retried["status_code"], 503);
    assert_eq!(retried["error_kind"], "http_503");
    assert_eq!(retried["response_bytes"], 4, "the rejected body was read");
    assert_eq!(retried["method"], "POST");
    assert_eq!(retried["model"], "stream-test");
    assert_eq!(retried["endpoint_host"], "127.0.0.1");
    assert_eq!(retried["endpoint_path"], "/v1/chat/completions");
    assert!(
        retried["request_bytes"].as_i64().unwrap() > 0,
        "the serialized body length is recorded"
    );
    assert!(retried["headers_at"].as_str().is_some());
    assert!(retried["finished_at"].as_str().is_some());
    assert!(
        retried["latency_ms"].as_i64().unwrap() >= 0,
        "the attempt's own latency, not the backoff after it"
    );
    assert_eq!(
        retried["request_payload_json"],
        serde_json::Value::Null,
        "payload capture is off by default"
    );

    let streamed = &rows[1];
    assert_eq!(streamed["attempt"], 2);
    assert_eq!(streamed["status_code"], 200);
    assert_eq!(streamed["error_kind"], serde_json::Value::Null);
    assert_eq!(streamed["input_tokens"], 11);
    assert_eq!(streamed["output_tokens"], 5);
    assert!(
        streamed["stream_event_count"].as_i64().unwrap() >= 2,
        "the events that carried the answer and its usage"
    );
    assert!(
        streamed["response_bytes"].as_i64().unwrap() >= body_len - 1,
        "the bytes read are recorded"
    );
}

#[tokio::test]
async fn provider_request_row_captures_a_rejected_response_only_when_asked() {
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!(
        "http://{}/v1/chat/completions",
        listener.local_addr().unwrap()
    );
    let rejection =
        br#"{"error":{"message":"bad key: Authorization: Bearer sk-live-4f9"}}"#.to_vec();
    let server_rejection = rejection.clone();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        read_http_request(&mut socket).await;
        write_sse_response(&mut socket, "401 Unauthorized", &server_rejection).await;
        socket.shutdown().await.unwrap();
    });

    let (state, session_id) = stream_test_state(&endpoint).await;
    state.lock().await.config.capture_provider_payloads = true;
    let error = stream_request(
        &reqwest::Client::new(),
        state,
        tokio_util::sync::CancellationToken::new(),
        &endpoint,
        "stream-test",
        vec![serde_json::json!({"role": "user", "content": "hello"})],
        std::sync::Arc::new(tokio::sync::Mutex::new(StreamBuffer::new())),
        false,
        false,
        ThinkingMode::Normal,
        crate::tools::ToolSchemaPolicy::read_only_inspection(),
        Some(&session_id),
        None,
    )
    .await
    .expect_err("a 401 is not retried");
    assert_eq!(error.status, Some(401));
    server.await.unwrap();

    let rows = wait_for_provider_request_rows(&session_id, 1).await;
    assert_eq!(rows.len(), 1, "one rejected attempt is one row: {rows:?}");
    let row = &rows[0];
    assert_eq!(row["status_code"], 401);
    assert_eq!(row["error_kind"], "http_401");
    assert_eq!(row["response_bytes"], rejection.len());
    let payload = row["request_payload_json"]
        .as_str()
        .expect("capture is on, so the request is stored");
    serde_json::from_str::<serde_json::Value>(payload).expect("the captured request is JSON");
    let summary = row["response_summary_json"]
        .as_str()
        .expect("capture is on, so the summary is stored");
    assert!(
        !summary.contains("sk-live-4f9"),
        "the provider's error text is redacted: {summary}"
    );
    assert!(
        summary.contains("bad key"),
        "the summary keeps the provider's message: {summary}"
    );
}

#[tokio::test]
async fn provider_request_row_records_a_cancelled_stream() {
    use std::time::Duration;
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!(
        "http://{}/v1/chat/completions",
        listener.local_addr().unwrap()
    );
    let event_line = "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n";
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        read_http_request(&mut socket).await;
        socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: keep-alive\r\n\r\n",
                )
                .await
                .expect("write streaming headers");
        socket
            .write_all(event_line.as_bytes())
            .await
            .expect("write model event");
        socket.flush().await.expect("flush SSE progress");
        let _ = release_rx.await;
        let _ = socket.shutdown().await;
    });

    let (state, session_id) = stream_test_state(&endpoint).await;
    let recorded_thread = session_id.clone();
    let buffer = std::sync::Arc::new(tokio::sync::Mutex::new(StreamBuffer::new()));
    let task_buffer = buffer.clone();
    let cancel = tokio_util::sync::CancellationToken::new();
    let task_cancel = cancel.clone();
    let task = tokio::spawn(async move {
        stream_request(
            &reqwest::Client::new(),
            state,
            task_cancel,
            &endpoint,
            "stream-test",
            vec![serde_json::json!({"role": "user", "content": "hello"})],
            task_buffer,
            false,
            false,
            ThinkingMode::Normal,
            crate::tools::ToolSchemaPolicy::read_only_inspection(),
            Some(&session_id),
            None,
        )
        .await
    });
    let observed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if buffer.lock().await.content.contains("hi") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(observed.is_ok(), "the model event must arrive first");
    cancel.cancel();
    let error = task
        .await
        .expect("stream task must not panic")
        .expect_err("a cancelled stream errors");
    assert_eq!(error.kind, StreamFailureKind::Cancelled);
    release_tx.send(()).unwrap();
    server.await.unwrap();

    let rows = wait_for_provider_request_rows(&recorded_thread, 1).await;
    assert_eq!(
        rows.len(),
        1,
        "a cancelled attempt is still a row: {rows:?}"
    );
    let row = &rows[0];
    assert_eq!(row["status_code"], 200);
    assert_eq!(row["error_kind"], "cancelled");
    assert!(
        row["stream_event_count"].as_i64().unwrap() >= 1,
        "the events read before the cancel are recorded"
    );
    assert!(row["response_bytes"].as_i64().unwrap() > 0);
    assert!(row["finished_at"].as_str().is_some());
}

#[test]
fn parse_speculative_arguments_extracts_complete_and_partial_json() {
    // Complete JSON
    let complete = r#"{"TargetFile": "src/symbols.rs", "Instruction": "fix bug"}"#;
    let parsed = parse_speculative_arguments(complete);
    assert_eq!(parsed["TargetFile"], "src/symbols.rs");
    assert_eq!(parsed["Instruction"], "fix bug");

    // Partial JSON cut mid-string
    let partial_string = r#"{"TargetFile": "src/main.rs", "Instruction": "refactor"#;
    let parsed_partial = parse_speculative_arguments(partial_string);
    assert_eq!(parsed_partial["TargetFile"], "src/main.rs");

    // Early partial JSON
    let early_partial = r#"{"CommandLine": "cargo check --tests""#;
    let parsed_early = parse_speculative_arguments(early_partial);
    assert_eq!(parsed_early["CommandLine"], "cargo check --tests");

    // Grep pattern
    let grep_partial = r#"{"pattern": "Config", "path": "src/""#;
    let parsed_grep = parse_speculative_arguments(grep_partial);
    assert_eq!(parsed_grep["pattern"], "Config");
    assert_eq!(parsed_grep["path"], "src/");

    // Cut mid-key: closing the quote and brace cannot make this valid
    // JSON, so the values come from the pattern fallback.
    let mid_key = r#"{"path": "src/main.rs", "command": "cargo \"check\"", "sta"#;
    let parsed_mid_key = parse_speculative_arguments(mid_key);
    assert_eq!(parsed_mid_key["path"], "src/main.rs");
    assert_eq!(parsed_mid_key["command"], "cargo \"check\"");
    assert!(parsed_mid_key.get("sta").is_none());
}

#[test]
fn native_argument_markers_distinguish_syntax_from_shape() {
    let incomplete = parse_native_tool_arguments(r#"{"path":"src/main.rs""#);
    assert_eq!(
        incomplete["_invalid_arguments"]["kind"],
        "incomplete_syntax"
    );
    assert_eq!(incomplete["_invalid_arguments"]["execution"], "rejected");
    assert!(incomplete["_recovery"].as_str().is_some());

    let scalar = parse_native_tool_arguments("[]");
    assert_eq!(scalar["_invalid_arguments"]["kind"], "invalid_shape");
}

#[test]
fn parse_speculative_text_tool_call_extracts_in_flight_fences() {
    let in_flight = "Let me search the codebase:\n```tool\n{\"name\": \"grep\", \"arguments\": {\"pattern\": \"AppConfig\"";
    let (name, args) =
        parse_speculative_text_tool_call(in_flight).expect("should extract in-flight call");
    assert_eq!(name, "grep");
    assert_eq!(args["pattern"], "AppConfig");

    let completed =
        "Done:\n```tool\n{\"name\": \"grep\", \"arguments\": {}}\n```\nHere are the results:";
    assert!(
        parse_speculative_text_tool_call(completed).is_none(),
        "completed fence is not in-flight"
    );
}

#[test]
fn parse_speculative_native_tool_call_tracks_incremental_name_and_arguments() {
    assert!(parse_speculative_native_tool_call("[TOOL_CALLS]").is_none());

    let name_only = parse_speculative_native_tool_call("[TOOL_CALLS]grep")
        .expect("native tool name should be visible before arguments");
    assert_eq!(name_only.0, "grep");
    assert_eq!(name_only.1, serde_json::json!({}));

    let partial =
        parse_speculative_native_tool_call("[TOOL_CALLS]grep[ARGS]{\"pattern\": \"AppConfig\"")
            .expect("partial native arguments should be repaired");
    assert_eq!(partial.0, "grep");
    assert_eq!(partial.1["pattern"], "AppConfig");
}

#[test]
fn responses_input_preserves_messages_and_tool_transactions() {
    let messages = vec![
        serde_json::json!({"role": "system", "content": "You are RustCode."}),
        serde_json::json!({"role": "user", "content": "Inspect the project."}),
        serde_json::json!({
            "role": "assistant",
            "content": "",
            "tool_calls": [{
                "id": "call-1",
                "type": "function",
                "function": {
                    "name": "glob",
                    "arguments": "{\"pattern\":\"src/**\"}"
                }
            }]
        }),
        serde_json::json!({
            "role": "tool",
            "tool_call_id": "call-1",
            "content": "src/main.rs"
        }),
    ];

    let input = responses_input_from_messages(&messages);

    assert_eq!(input[0]["role"], "system");
    assert_eq!(input[1]["content"][0]["type"], "input_text");
    assert_eq!(input[2]["type"], "function_call");
    assert_eq!(input[2]["call_id"], "call-1");
    assert_eq!(input[2]["arguments"], "{\"pattern\":\"src/**\"}");
    assert_eq!(input[3]["type"], "function_call_output");
    assert_eq!(input[3]["call_id"], "call-1");
}

// Regression: only the text of a multi-part message was kept, so an
// attached image never reached a Responses-protocol vision model.
#[test]
fn responses_input_carries_image_parts_of_a_user_message() {
    let input = responses_input_from_messages(&[
        serde_json::json!({"role": "user", "content": [
            {"type": "text", "text": "who is this"},
            {"type": "image_url", "image_url": {"url": "data:image/png;base64,AAA"}},
        ]}),
        serde_json::json!({"role": "user", "content": [
            {"type": "image_url", "image_url": {"url": "data:image/png;base64,BBB"}},
        ]}),
        serde_json::json!({"role": "user", "content": "plain text"}),
    ]);
    assert_eq!(
        input,
        [
            serde_json::json!({"role": "user", "content": [
                {"type": "input_text", "text": "who is this"},
                {"type": "input_image", "image_url": "data:image/png;base64,AAA"},
            ]}),
            serde_json::json!({"role": "user", "content": [
                {"type": "input_image", "image_url": "data:image/png;base64,BBB"},
            ]}),
            serde_json::json!({"role": "user", "content": [
                {"type": "input_text", "text": "plain text"},
            ]}),
        ]
    );
}

#[test]
fn responses_input_keeps_assistant_prose_before_call_outputs() {
    let messages = vec![
        serde_json::json!({
            "role": "assistant",
            "content": "Continue from the real results.",
            "tool_calls": [
                {
                    "id": "call-a",
                    "type": "function",
                    "function": {"name": "view_file", "arguments": "{}"}
                },
                {
                    "id": "call-b",
                    "type": "function",
                    "function": {"name": "list_directory", "arguments": "{}"}
                }
            ]
        }),
        serde_json::json!({
            "role": "tool",
            "tool_call_id": "call-a",
            "content": "file result"
        }),
        serde_json::json!({
            "role": "tool",
            "tool_call_id": "call-b",
            "content": "directory result"
        }),
    ];

    let input = responses_input_from_messages(&messages);

    assert_eq!(input[0]["role"], "assistant");
    assert_eq!(input[1]["type"], "function_call");
    assert_eq!(input[1]["call_id"], "call-a");
    assert_eq!(input[2]["type"], "function_call");
    assert_eq!(input[2]["call_id"], "call-b");
    assert_eq!(input[3]["type"], "function_call_output");
    assert_eq!(input[3]["call_id"], "call-a");
    assert_eq!(input[4]["type"], "function_call_output");
    assert_eq!(input[4]["call_id"], "call-b");
}

#[test]
fn responses_input_holds_runtime_notices_outside_call_output_runs() {
    let messages = vec![
        serde_json::json!({
            "role": "assistant",
            "content": "reading",
            "tool_calls": [
                {
                    "id": "call-a",
                    "type": "function",
                    "function": {"name": "view_file", "arguments": "{}"}
                },
                {
                    "id": "call-b",
                    "type": "function",
                    "function": {"name": "list_directory", "arguments": "{}"}
                }
            ]
        }),
        serde_json::json!({
            "role": "user",
            "content": "<rustcode_runtime_notice>loop warning</rustcode_runtime_notice>",
        }),
        serde_json::json!({
            "role": "tool",
            "tool_call_id": "call-a",
            "content": "file result"
        }),
        serde_json::json!({
            "role": "tool",
            "tool_call_id": "call-b",
            "content": "directory result"
        }),
    ];

    let input = responses_input_from_messages(&messages);

    let first_call = input
        .iter()
        .position(|item| item["type"] == "function_call")
        .expect("call run");
    let last_output = input
        .iter()
        .rposition(|item| item["type"] == "function_call_output")
        .expect("output run");
    assert!(first_call < last_output);
    for item in &input[first_call..=last_output] {
        let item_type = item.get("type").and_then(|t| t.as_str()).unwrap_or("");
        assert!(
            item_type == "function_call" || item_type == "function_call_output",
            "transaction split by {item:?}"
        );
    }
    assert!(
        input[last_output + 1..]
            .iter()
            .any(|item| item["content"].to_string().contains("loop warning")),
        "buffered notice must survive after the run: {input:?}"
    );
}

#[test]
fn responses_tools_flatten_chat_function_schemas() {
    let schemas = vec![serde_json::json!({
        "type": "function",
        "function": {
            "name": "view_file",
            "description": "Read a file",
            "parameters": {"type": "object", "properties": {}},
            "strict": true
        }
    })];

    let tools = responses_tool_schemas(&schemas);

    assert_eq!(
        tools,
        vec![serde_json::json!({
            "type": "function",
            "name": "view_file",
            "description": "Read a file",
            "parameters": {"type": "object", "properties": {}},
            "strict": true
        })]
    );
}

#[test]
fn responses_tools_are_not_strict_unless_asked() {
    // An MCP tool's optional fields stay optional: the Responses default
    // would require them all.
    let schemas = vec![serde_json::json!({
        "type": "function",
        "function": {
            "name": "get_competency_goals",
            "parameters": {"type": "object", "properties": {
                "goal_number": {"type": "integer"}
            }}
        }
    })];
    assert_eq!(responses_tool_schemas(&schemas)[0]["strict"], false);
}

// #2582: only a ChatGPT-plan request of the session's own turn (or a
// subagent's) with fast mode on asks for the priority tier.
#[test]
fn fast_mode_sets_the_priority_tier_only_on_chatgpt_session_turns() {
    let tier = |chatgpt_plan, fast_mode, session_turn| {
        let mut payload = serde_json::json!({"model": "gpt-6-luna"});
        apply_fast_service_tier(&mut payload, chatgpt_plan, fast_mode, session_turn);
        payload.get("service_tier").cloned()
    };
    assert_eq!(tier(true, true, true), Some(serde_json::json!("priority")));
    assert_eq!(tier(true, false, true), None, "fast mode off");
    assert_eq!(tier(false, true, true), None, "not a ChatGPT plan");
    assert_eq!(tier(true, true, false), None, "a side request");
}

// #2608: a Fast request served at another tier is reported to the user
// once, until a response comes back at the requested tier again.
#[test]
fn a_fast_request_served_at_the_default_tier_is_reported_once() {
    let completed = |tier: &str| -> serde_json::Value {
        let line = format!(
            "data: {{\"type\":\"response.completed\",\"response\":{{\"status\":\"completed\",\"service_tier\":\"{tier}\"}}}}"
        );
        serde_json::from_str(parse_sse_line(&line).unwrap()).unwrap()
    };
    let priority = serde_json::json!(FAST_SERVICE_TIER);
    let mut state = crate::app::AppState::new();
    let mut serve = |requested: Option<&serde_json::Value>, tier: &str| {
        let event = completed(tier);
        state.note_served_service_tier(requested, served_service_tier(&event).unwrap());
        state.pending_service_tier_notice.take()
    };

    let notice = serve(Some(&priority), "default").expect("downgrade is reported");
    assert!(notice.contains("Fast mode was requested"), "{notice}");
    assert!(notice.contains("default tier"), "{notice}");
    assert_eq!(serve(Some(&priority), "default"), None, "only once");
    assert_eq!(serve(None, "default"), None, "Fast off asks for no tier");
    assert_eq!(serve(Some(&priority), "priority"), None, "served as asked");
    assert!(
        serve(Some(&priority), "default").is_some(),
        "a new downgrade after a served Fast response is reported again"
    );
    assert_eq!(
        state.active_transient_notice(),
        Some(notice.as_str()),
        "the terminal shows it in the footer"
    );
    assert!(
        state.history.is_empty(),
        "the notice is not part of the chat history"
    );

    let mut fresh = crate::app::AppState::new();
    fresh.note_served_service_tier(Some(&priority), &priority);
    fresh.note_served_service_tier(None, &serde_json::json!("default"));
    assert_eq!(fresh.pending_service_tier_notice, None);
    assert!(
        served_service_tier(&serde_json::json!({"response": {"service_tier": null}})).is_none()
    );
}

#[test]
fn chatgpt_plan_payload_is_stateless_and_roundtrips_tools() {
    let profile = crate::config::ModelProfile {
        name: "chatgpt".into(),
        url: "https://api.openai.com/v1/responses".into(),
        model: "gpt-6.1-sol".into(),
        api_protocol: Some(crate::config::ApiProtocol::Responses),
        credential: Some(crate::provider_auth::CredentialRef {
            provider: "openai".into(),
            account: "test-account".into(),
            method: crate::provider_auth::AuthMethod::ChatGpt,
        }),
        reasoning_effort: Some("high".into()),
        supports_reasoning_effort: Some(true),
        max_output_tokens: Some(4096),
        temperature: Some(0.7),
        top_p: Some(0.8),
        ..Default::default()
    };
    let messages = vec![
        serde_json::json!({"role":"system","content":"system instructions"}),
        serde_json::json!({"role":"developer","content":"developer instructions"}),
        serde_json::json!({"role":"user","content":"inspect"}),
        serde_json::json!({
            "role":"assistant",
            "tool_calls":[{"id":"call-1","type":"function","function":{"name":"mcp__files__read","arguments":r#"{"path":"a.rs"}"#}}]
        }),
        serde_json::json!({"role":"tool","tool_call_id":"call-1","content":"source"}),
    ];
    let schemas = vec![serde_json::json!({
        "type":"function",
        "function":{"name":"mcp__files__read","description":"Read a file","parameters":{"type":"object","properties":{}}}
    })];

    let payload = chatgpt_stream_payload(
        "gpt-6.1-sol",
        &messages,
        &schemas,
        Some(&profile),
        ThinkingMode::Normal,
    );

    assert_eq!(payload["stream"], true);
    assert_eq!(payload["store"], false);
    assert_eq!(payload["reasoning"]["effort"], "high");
    assert_eq!(payload["reasoning"]["summary"], "auto");
    for unsupported in [
        "max_output_tokens",
        "temperature",
        "top_p",
        "tools",
        "parallel_tool_calls",
        "previous_response_id",
    ] {
        assert!(
            payload.get(unsupported).is_none(),
            "unexpected {unsupported}: {payload}"
        );
    }
    let input = payload["input"].as_array().expect("stateless input");
    assert_eq!(input[0]["role"], "developer");
    assert_eq!(input[0]["content"][0]["text"], "system instructions");
    assert_eq!(input[1]["role"], "developer");
    let additional_tools = input
        .iter()
        .position(|item| item["type"] == "additional_tools")
        .expect("client-side tool declaration");
    let function_call = input
        .iter()
        .position(|item| item["type"] == "function_call")
        .expect("historical call");
    assert!(additional_tools < function_call);
    assert_eq!(
        input[additional_tools]["tools"][0]["name"],
        "mcp__files__read"
    );
    // #1796: the declaration holds the same position in the turn's first
    // request, which has no function call yet, so later rounds extend the
    // first request's prefix instead of rewriting it.
    let first_round = chatgpt_stream_payload(
        "gpt-6.1-sol",
        &messages[..3],
        &schemas,
        Some(&profile),
        ThinkingMode::Normal,
    );
    let first_input = first_round["input"].as_array().expect("stateless input");
    assert_eq!(additional_tools, 2);
    assert_eq!(first_input[..], input[..first_input.len()]);
    assert_eq!(input[function_call]["name"], "mcp__files__read");
    assert_eq!(input[function_call + 1]["type"], "function_call_output");
    assert_eq!(input[function_call + 1]["call_id"], "call-1");
}

#[test]
fn profile_selection_uses_selected_identity_and_rejects_credential_ambiguity() {
    let chatgpt = crate::config::ModelProfile {
        name: "chatgpt-plan".into(),
        url: "https://api.openai.com/v1/responses".into(),
        model: "shared-model".into(),
        api_protocol: Some(crate::config::ApiProtocol::Responses),
        credential: Some(crate::provider_auth::CredentialRef {
            provider: "openai".into(),
            account: "account-1".into(),
            method: crate::provider_auth::AuthMethod::ChatGpt,
        }),
        ..Default::default()
    };
    let api_key = crate::config::ModelProfile {
        name: "ordinary-responses".into(),
        api_key: Some("legacy-key".into()),
        credential: None,
        ..chatgpt.clone()
    };
    let profiles = vec![api_key.clone(), chatgpt.clone()];
    let endpoint = chatgpt.endpoint_url();

    assert_eq!(
        select_request_profile(&profiles, "chatgpt-plan", &endpoint, "shared-model")
            .unwrap()
            .unwrap()
            .name,
        "chatgpt-plan"
    );
    assert_eq!(
        select_request_profile(&profiles, "ordinary-responses", &endpoint, "shared-model")
            .unwrap()
            .unwrap()
            .name,
        "ordinary-responses"
    );
    assert!(
        select_request_profile(&profiles, "missing", &endpoint, "shared-model")
            .unwrap_err()
            .contains("select the intended profile")
    );
}

#[test]
fn responses_events_normalize_text_tools_usage_and_errors() {
    let mut call_ids = HashMap::new();
    let mut argument_deltas = HashSet::new();
    let text = normalize_responses_event(
        &serde_json::json!({
            "type": "response.output_text.delta",
            "delta": "hello"
        }),
        &mut call_ids,
        &mut argument_deltas,
    )
    .unwrap();
    assert_eq!(text["choices"][0]["delta"]["content"], "hello");

    // #2299: every summary part, including the first of a later
    // reasoning item (whose index starts at 0 again), is its own
    // paragraph; the stream drops the break before any reasoning text.
    for index in [0, 1] {
        let part = serde_json::json!({
            "type": "response.reasoning_summary_part.added",
            "summary_index": index
        });
        let separator =
            normalize_responses_event(&part, &mut call_ids, &mut argument_deltas).unwrap();
        let delta = &separator["choices"][0]["delta"];
        assert_eq!(delta["reasoning"], "\n\n");
        assert_eq!(delta["reasoning_separator"], true);
        assert_eq!(delta["reasoning_summary"], true);
    }
    let summary = normalize_responses_event(
        &serde_json::json!({
            "type": "response.reasoning_summary_text.delta",
            "delta": "**Inspecting site**"
        }),
        &mut call_ids,
        &mut argument_deltas,
    )
    .unwrap();
    assert_eq!(
        summary["choices"][0]["delta"]["reasoning"],
        "**Inspecting site**"
    );
    assert_eq!(summary["choices"][0]["delta"]["reasoning_summary"], true);
    let raw = normalize_responses_event(
        &serde_json::json!({
            "type": "response.reasoning_text.delta",
            "delta": "Let me check"
        }),
        &mut call_ids,
        &mut argument_deltas,
    )
    .unwrap();
    assert_eq!(raw["choices"][0]["delta"]["reasoning"], "Let me check");
    assert!(
        raw["choices"][0]["delta"]
            .get("reasoning_summary")
            .is_none()
    );

    let added = normalize_responses_event(
        &serde_json::json!({
            "type": "response.output_item.added",
            "output_index": 0,
            "item": {
                "type": "function_call",
                "id": "fc-item-1",
                "call_id": "call-1",
                "name": "glob"
            }
        }),
        &mut call_ids,
        &mut argument_deltas,
    )
    .unwrap();
    assert_eq!(
        added["choices"][0]["delta"]["tool_calls"][0]["id"],
        "call-1"
    );

    let arguments = normalize_responses_event(
        &serde_json::json!({
            "type": "response.function_call_arguments.delta",
            "output_index": 0,
            "item_id": "fc-item-1",
            "delta": "{}"
        }),
        &mut call_ids,
        &mut argument_deltas,
    )
    .unwrap();
    assert_eq!(
        arguments["choices"][0]["delta"]["tool_calls"][0]["function"]["arguments"],
        "{}"
    );
    assert!(
        normalize_responses_event(
            &serde_json::json!({
                "type": "response.function_call_arguments.done",
                "output_index": 0,
                "item_id": "fc-item-1",
                "arguments": "{}"
            }),
            &mut call_ids,
            &mut argument_deltas,
        )
        .is_none()
    );

    let completed = normalize_responses_event(
        &serde_json::json!({
            "type": "response.completed",
            "response": {"usage": {
                "input_tokens": 10,
                "output_tokens": 4,
                "total_tokens": 14
            }}
        }),
        &mut call_ids,
        &mut argument_deltas,
    )
    .unwrap();
    assert_eq!(completed["choices"][0]["finish_reason"], "stop");
    assert_eq!(completed["usage"]["prompt_tokens"], 10);

    let error = normalize_responses_event(
        &serde_json::json!({
            "type": "response.failed",
            "response": {"error": {
                "code": "subscription_sharing_usage_limit_exceeded",
                "message": "limit reached",
                "status_code": 429
            }}
        }),
        &mut call_ids,
        &mut argument_deltas,
    )
    .unwrap();
    assert_eq!(error["error"]["message"], "limit reached");
    assert_eq!(
        error["error"]["code"],
        "subscription_sharing_usage_limit_exceeded"
    );
    assert_eq!(error["error"]["status_code"], 429);

    // The ChatGPT backend nests the fields of a stream `error` event.
    let nested = normalize_responses_event(
        &serde_json::json!({
            "type": "error",
            "error": {
                "type": "service_unavailable_error",
                "code": "server_is_overloaded",
                "message": "Our servers are currently overloaded.",
                "param": null
            },
            "sequence_number": 2
        }),
        &mut call_ids,
        &mut argument_deltas,
    )
    .unwrap();
    assert_eq!(nested["error"]["code"], "server_is_overloaded");
    assert_eq!(
        nested["error"]["message"],
        "Our servers are currently overloaded."
    );
    let flat = normalize_responses_event(
        &serde_json::json!({"type": "error", "code": "bad", "message": "flat"}),
        &mut call_ids,
        &mut argument_deltas,
    )
    .unwrap();
    assert_eq!(flat["error"]["message"], "flat");

    let incomplete = normalize_responses_event(
        &serde_json::json!({
            "type": "response.incomplete",
            "response": {
                "incomplete_details": {"reason": "max_output_tokens"},
                "usage": {"input_tokens": 12, "output_tokens": 20, "total_tokens": 32}
            }
        }),
        &mut call_ids,
        &mut argument_deltas,
    )
    .unwrap();
    assert_eq!(incomplete["choices"][0]["finish_reason"], "length");
    assert_eq!(incomplete["usage"]["prompt_tokens"], 12);

    let incomplete = normalize_responses_event(
        &serde_json::json!({
            "type": "response.incomplete",
            "response": {"incomplete_details": {"reason": "content_filter"}}
        }),
        &mut call_ids,
        &mut argument_deltas,
    )
    .unwrap();
    assert_eq!(incomplete["error"]["code"], "response_incomplete");
}

#[test]
fn profile_generation_options_include_hard_thinking_budget() {
    let profile = crate::config::ModelProfile {
        enable_thinking: Some(true),
        reasoning_effort: Some("low".to_string()),
        thinking_budget: Some(4096),
        supports_reasoning_effort: Some(true),
        supports_thinking_budget: Some(true),
        ..crate::config::ModelProfile::default()
    };
    let mut payload = serde_json::json!({});

    apply_profile_generation_options(&mut payload, Some(&profile), ThinkingMode::Normal);

    assert_eq!(payload["enable_thinking"], true);
    assert_eq!(payload["chat_template_kwargs"]["enable_thinking"], true);
    assert_eq!(payload["reasoning_effort"], "low");
    assert_eq!(payload["thinking_budget"], 4096);
}

#[test]
fn absent_profile_generation_options_do_not_override_server_defaults() {
    let mut payload = serde_json::json!({});

    apply_profile_generation_options(
        &mut payload,
        Some(&crate::config::ModelProfile::default()),
        ThinkingMode::Normal,
    );

    assert!(payload.get("enable_thinking").is_none());
    assert!(payload.get("reasoning_effort").is_none());
    assert!(payload.get("thinking_budget").is_none());
}

#[test]
fn provider_without_thinking_controls_omits_disabled_thinking_fields() {
    let profile = crate::config::ModelProfile {
        url: "https://api.groq.com/openai/v1/chat/completions".to_string(),
        model: "groq/compound".to_string(),
        ..crate::config::ModelProfile::default()
    };
    let mut payload = serde_json::json!({});

    apply_profile_generation_options(&mut payload, Some(&profile), ThinkingMode::Disabled);

    assert!(payload.get("enable_thinking").is_none());
    assert!(payload.get("chat_template_kwargs").is_none());
}

#[test]
fn qwen_omlx_does_not_send_unverified_reasoning_extensions() {
    let profile = crate::config::ModelProfile {
        engine: Some("omlx".to_string()),
        enable_thinking: Some(true),
        reasoning_effort: Some("medium".to_string()),
        thinking_budget: Some(4096),
        supports_reasoning_effort: Some(false),
        supports_thinking_budget: Some(false),
        ..crate::config::ModelProfile::default()
    };
    let mut payload = serde_json::json!({});

    apply_profile_generation_options(&mut payload, Some(&profile), ThinkingMode::Normal);

    assert_eq!(payload["enable_thinking"], true);
    assert_eq!(payload["chat_template_kwargs"]["enable_thinking"], true);
    assert!(payload.get("reasoning_effort").is_none());
    assert!(payload.get("thinking_budget").is_none());
}

#[test]
fn legacy_reasoning_profile_still_sends_existing_wire_fields() {
    let profile = crate::config::ModelProfile {
        reasoning_effort: Some("medium".to_string()),
        thinking_budget: Some(4096),
        ..crate::config::ModelProfile::default()
    };
    let mut payload = serde_json::json!({});

    apply_profile_generation_options(&mut payload, Some(&profile), ThinkingMode::Normal);

    assert_eq!(payload["reasoning_effort"], "medium");
    assert_eq!(payload["thinking_budget"], 4096);
}

#[test]
fn native_tool_argument_accumulation_has_a_hard_byte_limit() {
    let mut arguments = String::new();
    assert!(!append_bounded_native_arguments(
        &mut arguments,
        "{\"path\":\""
    ));
    assert!(append_bounded_native_arguments(
        &mut arguments,
        &"repeat/".repeat(MAX_NATIVE_TOOL_ARGUMENT_BYTES)
    ));
    assert_eq!(arguments.len(), MAX_NATIVE_TOOL_ARGUMENT_BYTES);
}

#[test]
fn native_tool_calls_without_indices_use_distinct_ids() {
    let mut calls = ToolAccumulatorSet::default();
    calls.add_delta(&serde_json::json!({
        "id": "call-write",
        "function": {"name": "write_to_file", "arguments": "{\"path\":\"a\"}"}
    }));
    calls.add_delta(&serde_json::json!({
        "id": "call-run",
        "function": {"name": "run_command", "arguments": "{\"command\":\"pwd\"}"}
    }));

    assert_eq!(calls.calls.len(), 2);
    assert_eq!(calls.calls[0].id, "call-write");
    assert_eq!(calls.calls[0].name, "write_to_file");
    assert_eq!(calls.calls[1].id, "call-run");
    assert_eq!(calls.calls[1].name, "run_command");
    assert_eq!(calls.calls[0].arguments, r#"{"path":"a"}"#);
    assert_eq!(calls.calls[1].arguments, r#"{"command":"pwd"}"#);
}

#[test]
fn native_tool_call_fragments_by_id_append_to_one_call() {
    let mut calls = ToolAccumulatorSet::default();
    calls.add_delta(&serde_json::json!({
        "id": "call-1",
        "function": {"name": "run_command", "arguments": "{\"command\":"}
    }));
    calls.add_delta(&serde_json::json!({
        "id": "call-1",
        "function": {"arguments": "\"pwd\"}"}
    }));

    assert_eq!(calls.calls.len(), 1);
    assert_eq!(calls.calls[0].id, "call-1");
    assert_eq!(calls.calls[0].name, "run_command");
    assert_eq!(calls.calls[0].arguments, r#"{"command":"pwd"}"#);
}

#[test]
fn partial_native_call_checkpoint_preserves_identity_without_arguments() {
    let mut calls = ToolAccumulatorSet::default();
    calls.add_delta(&serde_json::json!({
        "index": 2,
        "id": "call-write",
        "function": {
            "name": "write_to_file",
            "arguments": "{\"path\":\"src/main.rs\",\"content\":\"secret"
        }
    }));

    let checkpoint = calls.checkpoints();
    assert_eq!(checkpoint.len(), 1);
    assert_eq!(checkpoint[0].index, Some(2));
    assert_eq!(checkpoint[0].call_id.as_deref(), Some("call-write"));
    assert_eq!(checkpoint[0].tool_name, "write_to_file");
    assert!(!checkpoint[0].arguments_complete);
    assert_eq!(checkpoint[0].argument_bytes, 39);
    assert!(!checkpoint[0].argument_fingerprint.is_empty());
    assert!(!checkpoint[0].diagnostic.is_empty());
    assert_ne!(checkpoint[0].diagnostic, "secret");
}

#[test]
fn native_checkpoint_fields_are_bounded() {
    let mut calls = ToolAccumulatorSet::default();
    calls.add_delta(&serde_json::json!({
        "id": "i".repeat(MAX_NATIVE_CHECKPOINT_TEXT_BYTES + 1),
        "function": {
            "name": "n".repeat(MAX_NATIVE_CHECKPOINT_TEXT_BYTES + 1),
            "arguments": "{"
        }
    }));

    let checkpoint = calls.checkpoints();
    assert_eq!(checkpoint.len(), 1);
    assert!(
        checkpoint[0]
            .call_id
            .as_ref()
            .is_some_and(|id| id.len() <= MAX_NATIVE_CHECKPOINT_TEXT_BYTES + "…".len())
    );
    assert!(checkpoint[0].tool_name.len() <= MAX_NATIVE_CHECKPOINT_TEXT_BYTES + "…".len());
}

#[test]
fn native_tool_call_late_index_keeps_id_accumulator() {
    let mut calls = ToolAccumulatorSet::default();
    calls.add_delta(&serde_json::json!({
        "id": "call-1",
        "function": {"name": "grep", "arguments": "{\"pattern\":"}
    }));
    calls.add_delta(&serde_json::json!({
        "index": 0,
        "id": "call-1",
        "function": {"arguments": "\"needle\"}"}
    }));

    assert_eq!(calls.calls.len(), 1);
    assert_eq!(calls.calls[0].arguments, r#"{"pattern":"needle"}"#);
    assert_eq!(calls.by_index.get(&0), Some(&0));
}

#[test]
fn native_tool_calls_keep_index_order_even_if_first_delta_is_late() {
    let mut calls = ToolAccumulatorSet::default();
    calls.add_delta(&serde_json::json!({
        "index": 1,
        "id": "call-1",
        "function": {"name": "second", "arguments": "{}"}
    }));
    calls.add_delta(&serde_json::json!({
        "index": 0,
        "id": "call-0",
        "function": {"name": "first", "arguments": "{}"}
    }));

    let mut order: Vec<_> = (0..calls.calls.len()).collect();
    order.sort_by_key(|&position| {
        let call = &calls.calls[position];
        (
            call.index.is_none(),
            call.index.unwrap_or(usize::MAX),
            position,
        )
    });
    assert_eq!(order, vec![1, 0]);
    assert_eq!(calls.calls[order[0]].name, "first");
    assert_eq!(calls.calls[order[1]].name, "second");
}

#[test]
fn native_tool_call_without_identity_fails_closed_per_delta() {
    let mut calls = ToolAccumulatorSet::default();
    calls.add_delta(&serde_json::json!({
        "function": {"name": "first", "arguments": "{}"}
    }));
    calls.add_delta(&serde_json::json!({
        "function": {"name": "second", "arguments": "{}"}
    }));

    assert_eq!(calls.calls.len(), 2);
    assert_eq!(calls.calls[0].name, "first");
    assert_eq!(calls.calls[1].name, "second");
}

#[test]
fn provider_trace_redacts_content_arguments_and_secrets() {
    let value = serde_json::json!({
        "authorization": "Bearer super-secret-token",
        "choices": [{
            "finish_reason": "tool_calls",
            "delta": {
                "content": "private prompt text",
                "tool_calls": [{
                    "index": 3,
                    "id": "call-3",
                    "function": {
                        "name": "run_command",
                        "arguments": "{\"command\":\"cat super-secret-file\"}"
                    }
                }]
            }
        }],
        "usage": {"prompt_tokens": 10, "completion_tokens": 4, "total_tokens": 14}
    });
    let mut trace = ProviderStreamTrace::new(true, "session-1", "model", 2);
    trace.record(321, &value);
    let summary = trace.summary().to_string();

    assert!(!summary.contains("super-secret-token"));
    assert!(!summary.contains("private prompt text"));
    assert!(!summary.contains("super-secret-file"));
    assert!(summary.contains("arguments_bytes"));
    assert!(summary.contains("tool_calls"));
    assert!(summary.contains("\"index\":3"));
    assert!(summary.contains("\"present\":true"));
}

#[test]
fn provider_trace_is_bounded_by_event_count_and_bytes() {
    let value = serde_json::json!({
        "choices": [{"delta": {"tool_calls": [{"index": 0, "id": "x", "function": {"name": "grep", "arguments": "{}"}}]}}]
    });
    let mut trace = ProviderStreamTrace::new(true, "session", "model", 1);
    for _ in 0..(MAX_PROVIDER_TRACE_EVENTS + 100) {
        trace.record(100, &value);
    }
    let summary = trace.summary();
    assert!(summary["event_count"].as_u64().unwrap() <= MAX_PROVIDER_TRACE_EVENTS as u64);
    assert!(summary["trace_bytes"].as_u64().unwrap() <= MAX_PROVIDER_TRACE_BYTES as u64);
    assert!(summary["dropped_events"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn native_schema_tokens_are_in_prompt_and_total_usage_estimates() {
    let messages = vec![serde_json::json!({
        "role": "system",
        "content": "You are RustCode."
    })];
    let schema = vec![serde_json::json!({
        "type": "function",
        "function": {
            "name": "inspect_workspace",
            "description": "Inspect files in the workspace",
            "parameters": {"type": "object"}
        }
    })];

    let without_schema = estimate_token_usage(&messages, "reply").await.unwrap();
    let with_schema = estimate_token_usage_with_tool_schemas(&messages, "reply", &schema)
        .await
        .unwrap();
    let schema_tokens = crate::network::compaction::estimate_tool_schema_tokens(&schema) as u32;

    assert!(schema_tokens > 0);
    assert_eq!(
        with_schema.prompt_tokens,
        without_schema.prompt_tokens + schema_tokens
    );
    assert_eq!(
        with_schema.total_tokens,
        without_schema.total_tokens + schema_tokens
    );
    assert_eq!(without_schema.completion_tokens, count_tokens("reply"));
    assert_eq!(
        without_schema.total_tokens,
        without_schema.prompt_tokens + without_schema.completion_tokens
    );

    let prompt_only = estimate_token_usage(&messages, "").await.unwrap();
    assert_eq!(prompt_only.completion_tokens, 0);
    assert_eq!(prompt_only.total_tokens, prompt_only.prompt_tokens);
    assert_eq!(prompt_only.prompt_tokens, without_schema.prompt_tokens);
    assert_eq!(
        with_schema.completion_tokens,
        without_schema.completion_tokens
    );
}

#[test]
fn prompt_estimation_delta_percent_makes_provider_drift_actionable() {
    assert_eq!(prompt_estimation_delta_percent(10_000, 11_500), Some(15));
    assert_eq!(prompt_estimation_delta_percent(10_000, 9_000), Some(-10));
    assert_eq!(prompt_estimation_delta_percent(0, 100), None);
}

#[test]
fn disabled_thinking_omits_reasoning_controls() {
    let profile = crate::config::ModelProfile {
        enable_thinking: Some(false),
        reasoning_effort: Some("medium".to_string()),
        thinking_budget: Some(4096),
        ..crate::config::ModelProfile::default()
    };
    let mut payload = serde_json::json!({});

    apply_profile_generation_options(&mut payload, Some(&profile), ThinkingMode::Normal);

    assert_eq!(payload["enable_thinking"], false);
    assert_eq!(payload["chat_template_kwargs"]["enable_thinking"], false);
    assert!(payload.get("reasoning_effort").is_none());
    assert!(payload.get("thinking_budget").is_none());
}

#[test]
fn request_override_disables_thinking_and_omits_reasoning_controls() {
    let profile = crate::config::ModelProfile {
        enable_thinking: Some(true),
        reasoning_effort: Some("high".to_string()),
        thinking_budget: Some(8192),
        ..crate::config::ModelProfile::default()
    };
    let mut payload = serde_json::json!({});

    apply_profile_generation_options(&mut payload, Some(&profile), ThinkingMode::Disabled);

    assert_eq!(payload["enable_thinking"], false);
    assert_eq!(payload["chat_template_kwargs"]["enable_thinking"], false);
    assert!(payload.get("reasoning_effort").is_none());
    assert!(payload.get("thinking_budget").is_none());
}

#[test]
fn recovery_disables_thinking_for_a_bounded_action_request() {
    let profile = crate::config::ModelProfile {
        enable_thinking: Some(true),
        reasoning_effort: Some("medium".to_string()),
        thinking_budget: Some(4096),
        supports_reasoning_effort: Some(true),
        supports_thinking_budget: Some(true),
        ..crate::config::ModelProfile::default()
    };
    let mut payload = serde_json::json!({});

    apply_profile_generation_options(&mut payload, Some(&profile), ThinkingMode::BoundedRecovery);

    assert_eq!(payload["enable_thinking"], false);
    assert_eq!(payload["chat_template_kwargs"]["enable_thinking"], false);
    assert!(payload.get("reasoning_effort").is_none());
    assert!(payload.get("thinking_budget").is_none());
    assert_eq!(
        clamp_request_max_tokens(16_000, ThinkingMode::BoundedRecovery),
        RECOVERY_MAX_TOKENS
    );
    assert_eq!(
        clamp_request_max_tokens(16_000, ThinkingMode::Normal),
        16_000
    );
}

#[test]
fn output_limit_policy_omits_unconfigured_normal_cap() {
    let profile = crate::config::ModelProfile {
        context_window: Some(128_000),
        ..crate::config::ModelProfile::default()
    };
    let mut payload = serde_json::json!({});
    apply_output_token_limit(
        &mut payload,
        profile.resolved_output_token_field(),
        profile.output_token_limit(false, false),
    );
    assert!(payload.get("max_tokens").is_none());
    assert!(payload.get("max_completion_tokens").is_none());
    assert!(payload.get("max_output_tokens").is_none());
}

#[test]
fn output_limit_policy_selects_explicit_openai_compatible_field() {
    let profile = crate::config::ModelProfile {
        context_window: Some(128_000),
        max_tokens: Some(16_000),
        output_token_field: Some(crate::config::OutputTokenField::MaxCompletionTokens),
        ..crate::config::ModelProfile::default()
    };
    let mut payload = serde_json::json!({});
    apply_output_token_limit(
        &mut payload,
        profile.resolved_output_token_field(),
        profile.output_token_limit(false, false),
    );
    assert_eq!(payload["max_completion_tokens"], 16_000);
    assert!(payload.get("max_tokens").is_none());
}

#[test]
fn tool_output_limit_is_present_when_profile_uses_provider_default() {
    let profile = crate::config::ModelProfile {
        context_window: Some(128_000),
        output_token_field: Some(crate::config::OutputTokenField::MaxOutputTokens),
        ..crate::config::ModelProfile::default()
    };
    let mut payload = serde_json::json!({});
    apply_output_token_limit(
        &mut payload,
        profile.resolved_output_token_field(),
        profile.output_token_limit(true, false),
    );
    assert_eq!(payload["max_output_tokens"], 8192);
    assert!(payload.get("max_tokens").is_none());
}

#[test]
fn verified_profile_tool_output_starts_at_safe_cap() {
    let profile = crate::config::ModelProfile {
        context_window: Some(128_000),
        max_tokens: Some(16_000),
        tool_max_tokens: Some(16_000),
        ..crate::config::ModelProfile::default()
    };
    let mut payload = serde_json::json!({});
    apply_output_token_limit(
        &mut payload,
        profile.resolved_output_token_field(),
        profile.output_token_limit(true, false),
    );
    assert_eq!(payload["max_tokens"], 8_192);
}

#[test]
fn verified_kat_adaptive_tool_limit_is_written_to_the_wire_field() {
    let profile = crate::config::ModelProfile {
        name: "kat-coder".to_string(),
        url: "https://tokmax.paral.no/v1/chat/completions".to_string(),
        model: "KAT-Coder-V2.5-Dev-OptiQ-4bit".to_string(),
        context_window: Some(262_144),
        max_tokens: Some(16_000),
        output_token_field: Some(crate::config::OutputTokenField::MaxOutputTokens),
        ..crate::config::ModelProfile::default()
    };
    let mut payload = serde_json::json!({});
    apply_output_token_limit(
        &mut payload,
        profile.resolved_output_token_field(),
        profile.verified_tool_output_ceiling(),
    );
    assert_eq!(payload["max_output_tokens"], 16_000);
    assert!(payload.get("max_tokens").is_none());
}

#[test]
fn native_google_payload_uses_max_output_tokens_capability_field() {
    let profile = crate::config::ModelProfile {
        url: "https://generativelanguage.googleapis.com/v1beta/models/gemini-3:generateContent"
            .to_string(),
        context_window: Some(128_000),
        ..crate::config::ModelProfile::default()
    };
    let mut payload = serde_json::json!({});
    apply_output_token_limit(
        &mut payload,
        profile.resolved_output_token_field(),
        profile.output_token_limit(true, false),
    );
    assert_eq!(payload["maxOutputTokens"], 8192);
    assert!(payload.get("max_tokens").is_none());
    assert!(payload.get("max_output_tokens").is_none());
}

#[test]
fn bounded_recovery_keeps_output_cap_separate_and_small() {
    let profile = crate::config::ModelProfile {
        context_window: Some(128_000),
        max_tokens: Some(16_000),
        tool_max_tokens: Some(16_000),
        ..crate::config::ModelProfile::default()
    };
    let limit = profile
        .output_token_limit(false, true)
        .map(|value| clamp_request_max_tokens(value, ThinkingMode::BoundedRecovery));
    assert_eq!(limit, Some(RECOVERY_MAX_TOKENS));
}

#[test]
fn bounded_recovery_clamp_needs_a_way_to_switch_reasoning_off() {
    // #2118: a profile that reasons by default (Gemini, DeepSeek) or only
    // sets an effort cannot turn reasoning off for the retry, so 1024
    // tokens would be spent reasoning.
    let effort_only = crate::config::ModelProfile {
        context_window: Some(128_000),
        reasoning_effort: Some("high".to_string()),
        supports_reasoning_effort: Some(true),
        ..crate::config::ModelProfile::default()
    };
    assert_eq!(
        request_output_token_limit(Some(&effort_only), true, ThinkingMode::BoundedRecovery, 1),
        Some(crate::config::DEFAULT_TOOL_ROUND_MAX_TOKENS)
    );
    let switchable = crate::config::ModelProfile {
        enable_thinking: Some(true),
        ..effort_only.clone()
    };
    assert_eq!(
        request_output_token_limit(Some(&switchable), true, ThinkingMode::BoundedRecovery, 1),
        Some(RECOVERY_MAX_TOKENS)
    );
}

#[test]
fn output_recovery_lifts_the_default_tool_round_cap() {
    let recovery = ThinkingMode::OutputRecovery {
        reduce_reasoning: false,
    };
    // Remote Chat Completions, nothing configured: 8192 on a tool round,
    // the full completion budget on the retry.
    let remote = crate::config::ModelProfile {
        url: "https://example.test/v1/chat/completions".to_string(),
        context_window: Some(128_000),
        ..crate::config::ModelProfile::default()
    };
    assert_eq!(
        request_output_token_limit(Some(&remote), true, ThinkingMode::Normal, 1),
        Some(8192)
    );
    assert_eq!(
        request_output_token_limit(Some(&remote), true, recovery, 1),
        Some(32_000)
    );
    // An Ollama profile defaults to 4096; the retry gets twice that.
    let local = crate::config::ModelProfile {
        engine: Some("ollama".to_string()),
        ..remote.clone()
    };
    assert_eq!(
        request_output_token_limit(Some(&local), true, ThinkingMode::Normal, 1),
        Some(4096)
    );
    assert_eq!(
        request_output_token_limit(Some(&local), true, recovery, 1),
        Some(8192)
    );
    // A configured cap is honoured, without the tool-round clamp.
    let configured = crate::config::ModelProfile {
        max_output_tokens: Some(16_000),
        ..remote.clone()
    };
    assert_eq!(
        request_output_token_limit(Some(&configured), true, recovery, 1),
        Some(16_000)
    );
    // An unconfigured Responses profile keeps the provider default.
    let responses = crate::config::ModelProfile {
        url: "https://api.openai.com/v1/responses".to_string(),
        api_protocol: Some(crate::config::ApiProtocol::Responses),
        ..remote
    };
    assert_eq!(
        request_output_token_limit(Some(&responses), true, recovery, 1),
        None
    );
}

#[test]
fn output_recovery_lowers_reasoning_effort_only_after_an_empty_stop() {
    let profile = crate::config::ModelProfile {
        reasoning_effort: Some("xhigh".to_string()),
        supports_reasoning_effort: Some(true),
        reasoning_efforts: Some(vec!["low".into(), "medium".into(), "xhigh".into()]),
        ..crate::config::ModelProfile::default()
    };
    let mut payload = serde_json::json!({});
    apply_responses_generation_options(
        &mut payload,
        Some(&profile),
        ThinkingMode::OutputRecovery {
            reduce_reasoning: false,
        },
    );
    assert_eq!(payload["reasoning"]["effort"], "xhigh");

    let mut payload = serde_json::json!({});
    apply_responses_generation_options(
        &mut payload,
        Some(&profile),
        ThinkingMode::OutputRecovery {
            reduce_reasoning: true,
        },
    );
    assert_eq!(payload["reasoning"]["effort"], "low");

    let mut payload = serde_json::json!({});
    apply_profile_generation_options(
        &mut payload,
        Some(&profile),
        ThinkingMode::OutputRecovery {
            reduce_reasoning: true,
        },
    );
    assert_eq!(payload["reasoning_effort"], "low");

    // Without `low` in the list the cheapest listed effort is used.
    let no_low = crate::config::ModelProfile {
        reasoning_efforts: Some(vec!["medium".into(), "high".into()]),
        ..profile
    };
    assert_eq!(
        request_reasoning_effort(
            &no_low,
            "high",
            ThinkingMode::OutputRecovery {
                reduce_reasoning: true
            }
        ),
        "medium"
    );
}

#[test]
fn wire_thinking_budget_leaves_answer_room_below_the_cap() {
    let mut payload = serde_json::json!({"thinking_budget": 16_000});
    bound_wire_thinking_budget(&mut payload, Some(8192));
    assert_eq!(payload["thinking_budget"], 6144);

    let mut payload = serde_json::json!({"thinking_budget": 2048});
    bound_wire_thinking_budget(&mut payload, Some(8192));
    assert_eq!(payload["thinking_budget"], 2048);

    let mut payload = serde_json::json!({"thinking_budget": 16_000});
    bound_wire_thinking_budget(&mut payload, None);
    assert_eq!(payload["thinking_budget"], 16_000);
}

#[test]
fn profile_sampling_options_are_sent_without_changing_unspecified_defaults() {
    let profile = crate::config::ModelProfile {
        temperature: Some(1.0),
        top_p: Some(0.95),
        top_k: Some(20),
        presence_penalty: Some(1.5),
        frequency_penalty: Some(0.0),
        force_sampling: Some(true),
        preserve_thinking: Some(true),
        ..crate::config::ModelProfile::default()
    };
    let mut payload = serde_json::json!({});

    apply_profile_sampling_options(&mut payload, Some(&profile));

    assert_eq!(payload["temperature"], 1.0);
    assert_eq!(payload["top_p"], 0.95);
    assert_eq!(payload["top_k"], 20);
    assert_eq!(payload["presence_penalty"], 1.5);
    assert_eq!(payload["force_sampling"], true);
    assert_eq!(payload["chat_template_kwargs"]["preserve_thinking"], true);
    assert!(payload.get("frequency_penalty").is_none());
}

#[test]
fn api_key_headers_default_to_authorization_only() {
    let req = apply_api_key_headers(
        reqwest::Client::new().post("http://localhost/v1/chat/completions"),
        "secret",
        false,
    )
    .build()
    .expect("test request builds");
    assert_eq!(
        req.headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok()),
        Some("Bearer secret")
    );
    assert!(req.headers().get("x-api-key").is_none());
}

#[test]
fn api_key_headers_include_x_api_key_when_profile_opts_in() {
    let profile = crate::config::ModelProfile {
        send_x_api_key: Some(true),
        ..crate::config::ModelProfile::default()
    };
    let req = apply_api_key_headers(
        reqwest::Client::new().post("http://localhost/v1/chat/completions"),
        "secret",
        profile.send_x_api_key_header(),
    )
    .build()
    .expect("test request builds");
    assert_eq!(
        req.headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok()),
        Some("Bearer secret")
    );
    assert_eq!(
        req.headers().get("x-api-key").and_then(|v| v.to_str().ok()),
        Some("secret")
    );
}

#[test]
fn frequency_penalty_is_only_sent_when_configured() {
    let url = "https://splash.paral.no/v1/chat/completions";
    // Splash-like profile: no sampling overrides at all.
    let plain = crate::config::ModelProfile::default();
    let mut payload = serde_json::json!({});
    apply_frequency_penalty(&mut payload, Some(&plain), url);
    assert!(payload.get("frequency_penalty").is_none());

    // `force_sampling = false` must not inject it either.
    let no_sampling = crate::config::ModelProfile {
        force_sampling: Some(false),
        ..crate::config::ModelProfile::default()
    };
    let mut payload = serde_json::json!({});
    apply_frequency_penalty(&mut payload, Some(&no_sampling), url);
    assert!(payload.get("frequency_penalty").is_none());

    // An explicitly configured value is still sent.
    let configured = crate::config::ModelProfile {
        frequency_penalty: Some(0.3),
        ..crate::config::ModelProfile::default()
    };
    let mut payload = serde_json::json!({});
    apply_frequency_penalty(&mut payload, Some(&configured), url);
    assert_eq!(payload["frequency_penalty"], 0.3);
}

#[test]
fn disabled_tools_omit_schema_and_tool_choice() {
    let mut payload = serde_json::json!({});
    let schema = vec![serde_json::json!({
        "type": "function",
        "function": {"name": "view_file"}
    })];

    apply_api_native_tools(&mut payload, schema, false);

    assert!(payload.get("tools").is_none());
    assert!(payload.get("tool_choice").is_none());
}

#[test]
fn enabled_tools_include_schema_and_auto_choice() {
    let mut payload = serde_json::json!({});
    let schema = vec![serde_json::json!({
        "type": "function",
        "function": {"name": "view_file"}
    })];

    apply_api_native_tools(&mut payload, schema, true);

    assert_eq!(payload["tools"].as_array().map(Vec::len), Some(1));
    assert_eq!(payload["tool_choice"], "auto");
}

#[test]
fn default_chat_completions_tools_allow_provider_parallel_calls() {
    let mut payload = serde_json::json!({
        "tools": [{"type": "function", "function": {"name": "view_file"}}]
    });

    apply_provider_parallel_tool_call_policy(
        &mut payload,
        crate::config::ApiProtocol::ChatCompletions,
        true,
    );

    assert_eq!(payload["parallel_tool_calls"], true);
}

#[test]
fn default_responses_tools_allow_provider_parallel_calls() {
    let mut payload = serde_json::json!({
        "tools": [{"type": "function", "name": "view_file"}]
    });

    apply_provider_parallel_tool_call_policy(
        &mut payload,
        crate::config::ApiProtocol::Responses,
        true,
    );

    assert_eq!(payload["parallel_tool_calls"], true);
}

#[test]
fn trusted_batching_profile_enables_provider_parallel_calls() {
    let mut payload = serde_json::json!({
        "tools": [{"type": "function", "function": {"name": "view_file"}}]
    });

    apply_provider_parallel_tool_call_policy(
        &mut payload,
        crate::config::ApiProtocol::ChatCompletions,
        true,
    );

    assert_eq!(payload["parallel_tool_calls"], true);
}

#[test]
fn parallel_call_hint_rejection_is_narrowly_classified() {
    assert!(is_parallel_tool_calls_rejection(
        400,
        "Unknown parameter: parallel_tool_calls"
    ));
    assert!(is_parallel_tool_calls_rejection(
        422,
        "parallel tool calls are not supported"
    ));
    assert!(!is_parallel_tool_calls_rejection(
        400,
        "Invalid tool_choice"
    ));
    assert!(!is_parallel_tool_calls_rejection(
        500,
        "Unknown parameter: parallel_tool_calls"
    ));
}

#[test]
fn openrouter_requests_carry_a_bounded_session_id() {
    let mut payload = serde_json::json!({});
    apply_openrouter_session_affinity(
        &mut payload,
        "https://openrouter.ai/api/v1/chat/completions",
        Some("  session-123  "),
    );
    assert_eq!(payload["session_id"], "session-123");

    let mut payload = serde_json::json!({});
    apply_openrouter_session_affinity(
        &mut payload,
        "https://provider.example/v1/chat/completions",
        Some("session-123"),
    );
    assert!(payload.get("session_id").is_none());

    let mut payload = serde_json::json!({});
    apply_openrouter_session_affinity(
        &mut payload,
        "https://openrouter.ai/api/v1/chat/completions",
        Some(&"x".repeat(300)),
    );
    assert_eq!(payload["session_id"].as_str().map(str::len), Some(256));
}

#[test]
fn provider_usage_preserves_cache_write_and_discount_metrics() {
    let value = serde_json::json!({
        "response": {
            "usage": {
                "input_tokens": 100,
                "output_tokens": 20,
                "input_tokens_details": {
                    "cached_tokens": 80,
                    "cache_write_tokens": 20
                },
                "cache_discount": 0.25
            }
        }
    });

    let usage = responses_usage(&value).expect("usage should normalize");
    assert_eq!(usage["prompt_tokens_details"]["cached_tokens"], 80);
    assert_eq!(usage["prompt_tokens_details"]["cache_write_tokens"], 20);
    assert_eq!(usage["cache_discount"], 0.25);
}

#[test]
fn provider_cache_observation_distinguishes_affinity_and_reported_state() {
    assert_eq!(
        provider_cache_observation(true, true, Some(80), Some(20)),
        ("hit", "provider_reported_cached_tokens")
    );
    assert_eq!(
        provider_cache_observation(true, true, Some(0), Some(0)),
        ("miss", "provider_reported_zero_cache_tokens")
    );
    assert_eq!(
        provider_cache_observation(true, true, None, None),
        ("unknown", "provider_did_not_report_cache_tokens")
    );
    assert_eq!(
        provider_cache_observation(true, false, None, None),
        ("not_requested", "openrouter_session_affinity_unavailable")
    );
    // #1796: a provider without session affinity reports its own cache.
    assert_eq!(
        provider_cache_observation(false, false, Some(0), None),
        ("miss", "provider_reported_zero_cache_tokens")
    );
    assert_eq!(
        provider_cache_observation(false, false, Some(2944), None),
        ("hit", "provider_reported_cached_tokens")
    );
}

#[test]
fn alternate_provider_cache_usage_names_are_normalized() {
    let usage = serde_json::json!({
        "prompt_tokens": 100,
        "completion_tokens": 10,
        "total_tokens": 110,
        "prompt_tokens_details": {
            "cache_read_input_tokens": 75,
            "cache_creation_input_tokens": 25
        }
    });
    assert_eq!(cache_usage_metrics(&usage).0, Some(75));
    assert_eq!(cache_usage_metrics(&usage).1, Some(25));
}

#[test]
fn recovery_tool_requests_allow_a_final_answer() {
    let mut payload = serde_json::json!({});
    let schema = vec![serde_json::json!({
        "type": "function",
        "function": {"name": "view_file"}
    })];

    apply_api_native_tools(&mut payload, schema, true);

    assert_eq!(payload["tool_choice"], "auto");
}

#[test]
fn reasoning_chunks_stop_at_the_configured_client_budget() {
    let first = bound_reasoning_chunk("a".repeat(12).as_str(), 0, Some(4));
    assert_eq!(first.estimated_tokens, 3);
    assert!(!first.budget_exhausted);

    let final_chunk = bound_reasoning_chunk("b".repeat(12).as_str(), 3, Some(4));
    assert_eq!(final_chunk.estimated_tokens, 1);
    assert_eq!(final_chunk.text.len(), 4);
    assert!(final_chunk.budget_exhausted);
}

#[test]
fn a_profile_without_thinking_does_not_cut_provider_reasoning() {
    // deepseek-flash reasons by default; a profile that sets no effort
    // derives a zero thinking budget, which used to end every round at
    // its first reasoning delta with an empty thought.
    let profile = crate::config::ModelProfile {
        name: "deepseek".into(),
        model: "deepseek-flash".into(),
        thinking_budget: Some(8192),
        ..Default::default()
    };
    assert_eq!(profile.context_budget().thinking_budget, 0);
    assert_eq!(client_thinking_budget(&profile), None);

    let explicit = crate::config::ModelProfile {
        client_reasoning_budget: Some(64),
        ..profile.clone()
    };
    assert_eq!(client_thinking_budget(&explicit), Some(64));

    let thinking = crate::config::ModelProfile {
        enable_thinking: Some(true),
        ..profile
    };
    assert!(client_thinking_budget(&thinking).is_some_and(|budget| budget > 0));
}

#[test]
fn absent_reasoning_budget_preserves_the_whole_chunk() {
    let chunk = bound_reasoning_chunk("reasoning", 0, None);
    assert_eq!(chunk.text, "reasoning");
    assert!(!chunk.budget_exhausted);
}

#[test]
fn thinking_estimate_accumulates_chars_not_per_chunk_ceils() {
    // Token-piece streams deliver thousands of few-char deltas. Summing
    // per-chunk ceils would charge 2000 * ceil(9 * 0.25) = 6000 tokens
    // here; deriving once from kept chars charges ceil(18000 * 0.25).
    let mut buffer = StreamBuffer::new();
    let budget = Some(8192);
    for _ in 0..2000 {
        let used = buffer.thought_tokens_estimate();
        let bounded = bound_reasoning_chunk("123456789", used, budget);
        assert!(!bounded.budget_exhausted);
        buffer.thought_chars += bounded.text.len();
        buffer.thought_tokens = buffer.thought_tokens_estimate();
    }
    assert_eq!(buffer.thought_chars, 18_000);
    assert_eq!(buffer.thought_tokens_estimate(), 4500);
    assert_eq!(buffer.thought_tokens, 4500);
}
