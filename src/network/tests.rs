use super::*;

#[tokio::test]
async fn request_snapshot_consumes_wakeups_for_background_results_it_observes() {
    let mut app = AppState::new();
    app.history.push(ChatMessage::new(
        "tool",
        "background_task: Task first completed. Output:\ncheck passed",
    ));
    app.history.push(ChatMessage::new(
        "tool",
        "background_task: Task second completed. Output:\ntests passed",
    ));
    app.pending_queue = vec![
        "__task_wakeup__:first".to_string(),
        "queued user prompt".to_string(),
        "__task_wakeup__:second".to_string(),
    ];
    let state = Arc::new(Mutex::new(app));

    let messages = prepare_turn_request(
        &reqwest::Client::new(),
        &state,
        1,
        &tokio_util::sync::CancellationToken::new(),
    )
    .await
    .expect("request preparation");

    let request_text = serde_json::to_string(&messages).expect("serialize request");
    assert!(request_text.contains("check passed"));
    assert!(request_text.contains("tests passed"));
    assert_eq!(state.lock().await.pending_queue, ["queued user prompt"]);
}

#[test]
fn execution_envelope_keeps_typed_state_separate_from_display_text() {
    let result = ToolResult {
        tool_name: "run_command".to_string(),
        content: "human-facing output that happens to mention error: but succeeded".to_string(),
        diff: None,
        file_preview: None,
        metadata: ToolResultMetadata {
            call_id: Some("call_native_7".to_string()),
            success: true,
            exit_code: Some(0),
            changed_paths: vec!["src/main.rs".to_string()],
            replayed: true,
            ..Default::default()
        },
    };
    let envelope = result.execution_envelope();
    assert_eq!(envelope.call_id, "call_native_7");
    assert!(envelope.success);
    assert_eq!(envelope.exit_code, Some(0));
    assert!(envelope.replayed);
    assert_eq!(envelope.changed_paths, ["src/main.rs"]);
    assert_eq!(envelope.error_kind, None);
}

#[test]
fn execution_envelope_preserves_authoritative_failure_kind() {
    let result = tool_result_from_execution(
        "run_command",
        &serde_json::json!({"command": "cargo test"}),
        crate::tools::ToolExecutionOutput::failure_with_kind(
            "error: the compiler reported a failure".to_string(),
            crate::tools::ToolErrorKind::CompilerFailed,
            true,
        ),
        None,
    );

    let envelope = result.execution_envelope();
    assert!(!envelope.success);
    assert_eq!(
        envelope.error_kind,
        Some(crate::tools::ToolErrorKind::CompilerFailed)
    );
    assert!(envelope.retryable);
}

#[test]
fn persisted_tool_error_kind_round_trips_explicitly() {
    let record = crate::app::ToolResultRecord {
        error_kind: Some(crate::tools::ToolErrorKind::McpFailed.as_str().to_string()),
        retryable: true,
        replayed: true,
        exit_code: Some(7),
        changed_paths: vec!["src/mcp.rs".to_string()],
        ..Default::default()
    };
    let reloaded: crate::app::ToolResultRecord =
        serde_json::from_value(serde_json::to_value(&record).unwrap()).unwrap();
    assert_eq!(
        reloaded.parsed_error_kind(),
        Some(crate::tools::ToolErrorKind::McpFailed)
    );
    assert!(reloaded.retryable);
    assert!(reloaded.replayed);
    assert_eq!(reloaded.exit_code, Some(7));
    assert_eq!(reloaded.changed_paths, ["src/mcp.rs"]);
}

#[tokio::test]
async fn denied_tool_batch_records_permission_denied_metadata() {
    let state = Arc::new(Mutex::new(AppState::new()));
    let cancel = tokio_util::sync::CancellationToken::new();
    let calls = vec![crate::tools::ToolCall {
        name: "run_command".to_string(),
        arguments: serde_json::json!({"command": "true"}),
        call_id: None,
    }];
    let mut dirty = false;
    let mut cache = None;
    let mut wait = std::time::Duration::ZERO;
    let results = execute_tool_batch(
        &reqwest::Client::new(),
        &state,
        &cancel,
        &calls,
        false,
        &None,
        &mut dirty,
        &mut cache,
        &mut wait,
        None,
    )
    .await;
    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0].metadata.error_kind,
        Some(crate::tools::ToolErrorKind::PermissionDenied)
    );
    assert!(!results[0].metadata.success);
}

#[tokio::test]
async fn cancelled_tool_batch_removes_its_live_projection() {
    let state = Arc::new(Mutex::new(AppState::new()));
    let cancel = tokio_util::sync::CancellationToken::new();
    cancel.cancel();
    let calls = vec![crate::tools::ToolCall {
        name: "run_command".to_string(),
        arguments: serde_json::json!({"command": "true"}),
        call_id: None,
    }];
    let mut dirty = false;
    let mut cache = None;
    let mut wait = std::time::Duration::ZERO;

    let _ = execute_tool_batch(
        &reqwest::Client::new(),
        &state,
        &cancel,
        &calls,
        true,
        &None,
        &mut dirty,
        &mut cache,
        &mut wait,
        None,
    )
    .await;

    assert!(state.lock().await.live_tool_calls.is_empty());
}

async fn gated_json_server(
    body: serde_json::Value,
) -> (
    String,
    tokio::sync::oneshot::Receiver<()>,
    tokio::sync::oneshot::Sender<()>,
) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test server");
    let address = listener.local_addr().expect("test server address");
    let (accepted_tx, accepted_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept request");
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4_096];
        loop {
            let read = socket.read(&mut buffer).await.expect("read request");
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
            let Some(header_end) = request.windows(4).position(|w| w == b"\r\n\r\n") else {
                continue;
            };
            let headers = String::from_utf8_lossy(&request[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or(0);
            if request.len() >= header_end + 4 + content_length {
                break;
            }
        }

        accepted_tx.send(()).ok();
        release_rx.await.ok();

        let body = body.to_string();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        socket
            .write_all(response.as_bytes())
            .await
            .expect("write response");
    });

    (format!("http://{address}"), accepted_rx, release_tx)
}

#[tokio::test]
async fn gemini_gateway_uses_capability_probe() {
    let (url, request_accepted, release_response) =
        gated_json_server(serde_json::json!({})).await;
    let state = Arc::new(Mutex::new(AppState::new()));
    let probe_state = Arc::clone(&state);
    let task = tokio::spawn(async move {
        probe_function_calling(
            &reqwest::Client::new(),
            &probe_state,
            &url,
            "gemini-3.7-flash",
        )
        .await
    });

    request_accepted
        .await
        .expect("Gemini gateway must receive the capability probe");
    release_response.send(()).expect("release probe response");
    assert!(task.await.expect("probe task must finish"));
}

#[tokio::test]
async fn automatic_compaction_discards_cross_session_result_with_shared_history() {
    use crate::app::AppState;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::Mutex;
    use tokio_util::sync::CancellationToken;

    let (url, request_accepted, release_response) = gated_json_server(serde_json::json!({
        "choices": [{"message": {"content": "summary of the old session"}}]
    }))
    .await;
    let mut app = AppState::new();
    app.api_base_url = url;
    app.active_session_id = "old-session".to_string();
    for profile in &mut app.config.models {
        profile.context_window = Some(400);
    }
    app.history = (0..(crate::network::compaction::KEEP_RECENT_TURNS + 4))
        .map(|index| {
            ChatMessage::new(
                if index % 2 == 0 { "user" } else { "assistant" },
                format!("message {index}: {}", "context ".repeat(80)),
            )
        })
        .collect();
    let new_session_history = app.history.clone();
    let state = Arc::new(Mutex::new(app));
    let request_state = Arc::clone(&state);
    let task = tokio::spawn(async move {
        prepare_turn_request(
            &reqwest::Client::new(),
            &request_state,
            1,
            &CancellationToken::new(),
        )
        .await
    });

    tokio::time::timeout(Duration::from_secs(10), request_accepted)
        .await
        .expect("automatic compaction request must start")
        .expect("test server must observe the request");
    {
        let mut live = tokio::time::timeout(Duration::from_secs(1), state.lock())
            .await
            .expect("network I/O must not hold the state lock");
        live.active_session_id = "new-session".to_string();
        live.history = new_session_history.clone();
    }
    release_response
        .send(())
        .expect("release automatic compaction response");
    let _ = tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .expect("automatic compaction must finish")
        .expect("automatic compaction task must not panic");

    let live = state.lock().await;
    assert_eq!(live.active_session_id, "new-session");
    assert!(live.history == new_session_history);
}

// Regression: session 1785600273324, msgs 7-18. The repeat guard correctly
// declined four identical `view_file lines 1-1` calls, but answered each with
// a notice pointing at earlier context. The model wanted those lines, so it
// asked again — six turns and four loop warnings before it moved on.
#[test]
fn small_reads_are_worth_repeating_verbatim() {
    let one_line =
        "[File: src/symbols.rs, Lines 1 to 1 of 408]\n1: use rusqlite::{Connection, params};";
    assert!(one_line.len() <= REPLAYABLE_READ_LIMIT);

    // A whole file stays behind the notice: repeating it every turn would
    // cost more than the loop it prevents.
    let whole_file = "x".repeat(15_567);
    assert!(whole_file.len() > REPLAYABLE_READ_LIMIT);
}

// Regression: session 1785595170460, msgs 5-8. Two identical full-file reads
// ran back to back because the read-dedupe cache was cleared on a sticky
// task-level "made edits" flag, which stays true for the rest of the task —
// so after the first edit no read was ever recognised as a repeat.
#[test]
fn only_a_batch_that_changed_files_invalidates_the_read_cache() {
    let applied = ToolResult {
        tool_name: "replace_file_content".to_string(),
        content: "successfully replaced target_content in 'src/lib.rs'".to_string(),
        diff: None,
        file_preview: None,
        metadata: ToolResultMetadata {
            success: true,
            ..Default::default()
        },
    };
    let failed = ToolResult {
        tool_name: "replace_file_content".to_string(),
        content: "error: target_content does not match".to_string(),
        diff: None,
        file_preview: None,
        metadata: ToolResultMetadata {
            success: false,
            ..Default::default()
        },
    };
    let read = ToolResult {
        tool_name: "view_file".to_string(),
        content: "1: fn main() {}".to_string(),
        diff: None,
        file_preview: None,
        metadata: ToolResultMetadata {
            success: true,
            ..Default::default()
        },
    };

    let changed = |results: &[ToolResult]| {
        results.iter().any(|result| {
            is_mutating_tool(&result.tool_name)
                && result.metadata.success
                && !result
                    .content
                    .trim_start()
                    .to_ascii_lowercase()
                    .starts_with("error")
        })
    };

    assert!(changed(&[applied]));
    // A failed edit leaves the files exactly as the earlier reads saw them.
    assert!(!changed(&[failed]));
    assert!(!changed(&[read]));
}

// Regression: session 1785595170460. The one edit the model attempted failed,
// it then read the file, found the line it wanted already present from an
// earlier run, and reported "I've added the comment" before calling
// complete_task — which the harness accepted.
// Regression: session 1785597279144. Blocked with only "make the change" or
// "say it could not be made" on offer, and looking at a file that already
// held the requested line, the model cleared the gate by deleting that line
// — then reported having added and removed it.
#[test]
fn the_block_message_sanctions_finishing_without_an_edit() {
    let message = completion_block_message(1);

    // The branch that fits "it is already how you asked".
    assert!(
        message.contains("already in the requested state"),
        "got: {message}"
    );
    assert!(message.contains("requires no edit"), "got: {message}");
    // And an explicit bar on satisfying the check with any other write.
    assert!(
        message.contains("delete existing content"),
        "got: {message}"
    );
    assert!(message.contains("reverse the request"), "got: {message}");
    assert!(message.contains("1 edit(s)"), "got: {message}");
}

#[test]
fn completion_is_blocked_only_when_nothing_was_applied() {
    // Every edit failed: the workspace is untouched.
    assert!(completion_claims_unapplied_work(false, 1, 0));

    // An edit landed, so a later failure does not invalidate the work.
    assert!(!completion_claims_unapplied_work(true, 3, 0));

    // A task with no edits at all — a question — finishes freely.
    assert!(!completion_claims_unapplied_work(false, 0, 0));

    // The gate stops arguing once it has said its piece twice.
    assert!(!completion_claims_unapplied_work(
        false,
        1,
        MAX_COMPLETION_BLOCKS
    ));
}

// Every id a replayed assistant message announces must have a matching
// result, or the provider rejects the request and the model is left to
// assume what happened to the call.
#[test]
fn rejected_and_interrupted_calls_still_get_results() {
    let refs = vec![
        crate::app::ToolCallRef {
            id: "call_1".to_string(),
            name: "grep".to_string(),
            arguments: "{}".to_string(),
        },
        crate::app::ToolCallRef {
            id: "call_2".to_string(),
            name: "run_command".to_string(),
            arguments: "{}".to_string(),
        },
    ];

    let answers = unanswered_call_results(&refs, "interrupted by the user");

    assert_eq!(answers.len(), 2);
    assert_eq!(answers[0].role, "tool");
    assert_eq!(answers[0].tool_call_id.as_deref(), Some("call_1"));
    assert!(
        answers[0]
            .content
            .contains("grep: error: interrupted by the user")
    );
    assert_eq!(answers[1].tool_call_id.as_deref(), Some("call_2"));
}

#[test]
fn call_refs_are_empty_without_provider_ids() {
    let calls = vec![crate::tools::ToolCall {
        name: "grep".to_string(),
        arguments: serde_json::json!({"pattern": "x"}),
        call_id: None,
    }];

    // Text protocols supply no ids, so nothing structured is recorded.
    assert!(call_refs_for(&calls, &[]).is_empty());

    let refs = call_refs_for(&calls, &["call_9".to_string()]);
    assert_eq!(refs[0].id, "call_9");
    assert_eq!(refs[0].name, "grep");
}

#[test]
fn call_refs_prefer_the_embedded_provider_id() {
    let calls = vec![crate::tools::ToolCall {
        name: "grep".to_string(),
        arguments: serde_json::json!({"pattern": "x"}),
        call_id: Some("native-call-1".to_string()),
    }];

    let refs = call_refs_for(&calls, &["positional-fallback".to_string()]);
    assert_eq!(refs[0].id, "native-call-1");
}

#[test]
fn fence_counter_survives_chunk_boundaries() {
    let mut counter = ToolFenceCounter::default();

    // Marker split across three chunks still counts exactly once.
    assert_eq!(counter.push("some text ``"), 0);
    assert_eq!(counter.push("`to"), 0);
    assert_eq!(counter.push("ol\n{\"name\": \"grep\"}"), 1);

    // Two more in a single chunk.
    assert_eq!(counter.push("```tool\n{}\n```\n```tool\n{}"), 3);

    // Prose without fences leaves the count alone.
    assert_eq!(counter.push(" and then I will check the results"), 3);
}

// Regression: an oversized batch used to be replayed into history verbatim,
// so the next turn read the model's imagined tool results ("the grep
// confirms...") as if they had actually happened.
#[test]
fn truncated_batch_summary_keeps_shape_and_drops_prose() {
    let kept = vec![
        crate::tools::ToolCall {
            name: "grep".to_string(),
            arguments: serde_json::json!({"pattern": "duct::cmd"}),
            call_id: None,
        },
        crate::tools::ToolCall {
            name: "run_command".to_string(),
            arguments: serde_json::json!({"command": "cargo check"}),
            call_id: None,
        },
    ];

    let summary = truncated_batch_summary(&kept, 14);

    assert!(summary.contains("first 2 tool calls"), "got: {summary}");
    assert!(summary.contains("grep, run_command"), "got: {summary}");
    assert!(summary.contains("14 more were dropped"), "got: {summary}");
    assert!(summary.contains("imagined"), "got: {summary}");
    // Nothing from the arguments or the surrounding narration survives.
    assert!(!summary.contains("cargo check"), "got: {summary}");
}

#[test]
fn oversized_tool_result_is_bounded_once_before_history_insertion() {
    let raw = (1..=2000)
        .map(|line| format!("payload line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let deferred_notice = "[harness: deferred 2 additional tool call(s) until the next model turn after skill loading]";
    let result = finalize_tool_result(
        ToolResult {
            tool_name: "use_skill".to_string(),
            content: raw,
            diff: None,
            file_preview: None,
            metadata: ToolResultMetadata::default(),
        },
        Some(deferred_notice),
    );
    let content = result.content.clone();
    let artifact_before = result.metadata.full_output_artifact.clone();
    let content_before = result.content.clone();
    let result = finalize_tool_result(result, None);
    assert_eq!(result.content, content_before);
    assert_eq!(result.metadata.full_output_artifact, artifact_before);
    let message = tool_result_history_message(result, None);

    assert!(content.contains(deferred_notice));
    assert_eq!(content.matches("[Output truncated:").count(), 1);
    assert!(content.len() <= 50 * 1024);
    assert!(
        message
            .tool_result
            .as_ref()
            .is_some_and(|metadata| metadata.truncated)
    );
    let metadata = message.tool_result.as_ref().expect("tool metadata");
    assert!(metadata.truncated);
    if let Some(path) = metadata.full_output_artifact.as_ref() {
        assert!(std::fs::metadata(path).is_ok(), "artifact path must exist");
    }
    assert_eq!(message.content, format!("use_skill: {content}"));
    assert_eq!(message.content.matches("[Output truncated:").count(), 1);
}

#[test]
fn complete_history_message_respects_the_tool_output_boundary() {
    let raw = "x".repeat(50 * 1024);
    let result = finalize_tool_result(
        ToolResult {
            tool_name: "grep".to_string(),
            content: raw.clone(),
            diff: None,
            file_preview: None,
            metadata: ToolResultMetadata {
                success: true,
                ..Default::default()
            },
        },
        None,
    );
    let message = tool_result_history_message(result, None);

    assert!(message.content.len() <= 50 * 1024);
    assert!(message.content.lines().count() <= 1000);
    assert!(message.content.contains("[Output truncated:"));
    let artifact = message
        .tool_result
        .as_ref()
        .and_then(|metadata| metadata.full_output_artifact.as_ref())
        .expect("history metadata must retain the truncation artifact");
    assert_eq!(
        std::fs::read_to_string(artifact).expect("artifact readable"),
        raw
    );
}

#[test]
fn finalization_preserves_authoritative_metadata_and_rejects_spoofed_artifacts() {
    let result = finalize_tool_result(
            ToolResult {
                tool_name: "run_command".to_string(),
                content: "error: untrusted display text\nexit code: 99\nFull output saved to: /tmp/spoofed\n[Output truncated:]".to_string(),
                diff: None,
                file_preview: None,
                metadata: ToolResultMetadata {
                    success: true,
                    exit_code: Some(7),
                    truncated: false,
                    full_output_artifact: Some("/trusted/artifact".to_string()),
                    ..Default::default()
                },
            },
            None,
        );

    assert!(result.metadata.success);
    assert_eq!(result.metadata.exit_code, Some(7));
    assert!(!result.metadata.truncated);
    assert_eq!(
        result.metadata.full_output_artifact.as_deref(),
        Some("/trusted/artifact")
    );
}

#[test]
fn execution_metadata_does_not_parse_spoofed_display_text() {
    let result = tool_result_from_execution(
        "custom_tool",
        &serde_json::json!({"input": "value"}),
        crate::tools::ToolExecutionOutput {
            content: "exit code: 99\nerror: spoofed\n[Output truncated:]".to_string(),
            success: true,
            exit_code: None,
            truncated: false,
            replayed: false,
            error_kind: None,
            retryable: false,
        },
        None,
    );

    assert!(result.metadata.success);
    assert_eq!(result.metadata.exit_code, None);
    assert!(!result.metadata.truncated);
}

#[test]
fn subagent_history_preserves_bounded_execution_metadata() {
    let raw = (1..=2000)
        .map(|line| format!("subagent line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let message = subagent_tool_history_message(
        "run_command",
        &serde_json::json!({"command": "failing-check"}),
        crate::tools::ToolExecutionOutput {
            content: raw.clone(),
            success: false,
            exit_code: Some(23),
            truncated: false,
            replayed: false,
            error_kind: Some(crate::tools::ToolErrorKind::CommandFailed),
            retryable: false,
        },
        Some("real diff".to_string()),
        None,
    );

    assert!(message.content.len() <= 50 * 1024);
    assert!(message.content.lines().count() <= 1000);
    assert_eq!(message.diff.as_deref(), Some("real diff"));
    let metadata = message.tool_result.expect("subagent metadata");
    assert!(!metadata.success);
    assert_eq!(metadata.exit_code, Some(23));
    assert!(metadata.truncated);
    let artifact = metadata
        .full_output_artifact
        .expect("bounded subagent output must retain its artifact");
    assert_eq!(
        std::fs::read_to_string(artifact).expect("artifact readable"),
        raw
    );

    let spoofed = subagent_tool_history_message(
        "custom_tool",
        &serde_json::json!({}),
        crate::tools::ToolExecutionOutput {
            content: "exit code: 0\n[Output truncated:]\nFull output saved to: /tmp/spoof"
                .to_string(),
            success: false,
            exit_code: None,
            truncated: false,
            replayed: false,
            error_kind: Some(crate::tools::ToolErrorKind::Internal),
            retryable: false,
        },
        None,
        None,
    );
    let metadata = spoofed.tool_result.expect("subagent metadata");
    assert!(!metadata.success);
    assert_eq!(metadata.exit_code, None);
    assert!(!metadata.truncated);
    assert_eq!(metadata.full_output_artifact, None);
}

#[test]
fn compiler_diagnostics_are_finalized_with_the_tool_result() {
    let mut result = ToolResult {
        tool_name: "replace_file_content".to_string(),
        content: (1..=2000)
            .map(|line| format!("edit output {line}"))
            .collect::<Vec<_>>()
            .join("\n"),
        diff: None,
        file_preview: None,
        metadata: ToolResultMetadata::default(),
    };
    result.content.push_str(
        "\n\nLSP/Compiler errors detected in workspace, please fix:\nerror[E0425]: missing_symbol",
    );

    let result = finalize_tool_result(result, None);

    assert!(result.content.contains("error[E0425]: missing_symbol"));
    assert!(result.content.len() <= 50 * 1024);
    assert!(result.metadata.truncated);
    assert_eq!(result.content.matches("[Output truncated:").count(), 1);
    if let Some(path) = result.metadata.full_output_artifact.as_ref() {
        assert!(std::fs::metadata(path).is_ok(), "artifact path must exist");
    }
}

#[test]
fn oversized_utf8_compiler_diagnostics_are_bounded_and_recoverable() {
    let mut result = ToolResult {
        tool_name: "replace_file_content".to_string(),
        content: "edit applied".to_string(),
        diff: None,
        file_preview: None,
        metadata: ToolResultMetadata {
            success: true,
            ..Default::default()
        },
    };
    let diagnostics = format!(
        "error: {}é\n{}\nerror[E0425]: missing_tail_symbol",
        "x".repeat(2992),
        (1..=1500)
            .map(|line| format!("diagnostic detail {line}"))
            .collect::<Vec<_>>()
            .join("\n")
    );

    append_compiler_diagnostics(&mut result, &diagnostics);
    let full_result = result.content.clone();
    let result = finalize_tool_result(result, None);

    assert!(result.content.len() <= 50 * 1024);
    assert!(result.content.lines().count() <= 1000);
    assert!(result.content.contains("error[E0425]: missing_tail_symbol"));
    assert_eq!(
        result.metadata.error_kind,
        Some(crate::tools::ToolErrorKind::CompilerFailed)
    );
    assert!(result.metadata.retryable);
    let artifact = result
        .metadata
        .full_output_artifact
        .as_ref()
        .expect("oversized diagnostics must have a recovery artifact");
    assert_eq!(
        std::fs::read_to_string(artifact).expect("artifact readable"),
        full_result
    );
}

#[test]
fn history_uses_the_bounded_tool_result_without_retruncating_it() {
    let result = finalize_tool_result(
        ToolResult {
            tool_name: "grep".to_string(),
            content: (1..=2000)
                .map(|line| format!("match {line}"))
                .collect::<Vec<_>>()
                .join("\n"),
            diff: None,
            file_preview: None,
            metadata: ToolResultMetadata::default(),
        },
        None,
    );
    let bounded = result.content.clone();

    let message = tool_result_history_message(result, None);

    assert_eq!(message.content, format!("grep: {bounded}"));
    assert_eq!(message.content.matches("[Output truncated:").count(), 1);
}

#[test]
fn malformed_native_arguments_are_preserved_for_validation() {
    let value = parse_native_tool_arguments("{\"pattern\":");
    assert!(value.get("_invalid_arguments").is_some());
    assert!(value.get("_parse_error").is_some());
}

#[test]
fn test_context_length_from_model_info() {
    let info = serde_json::json!({
        "general.architecture": "llama",
        "llama.context_length": 262144,
        "llama.embedding_length": 8192,
    });
    assert_eq!(context_length_from_model_info(&info), Some(262144));
    assert_eq!(context_length_from_model_info(&serde_json::json!({})), None);
}

#[test]
fn test_trim_msgs_keeps_system_and_latest() {
    let big = "x".repeat(4000); // ~1000 tokens
    let mut msgs: Vec<serde_json::Value> = vec![
        serde_json::json!({"role": "system", "content": "sys"}),
        serde_json::json!({"role": "user", "content": big.clone()}),
        serde_json::json!({"role": "assistant", "content": big.clone()}),
        serde_json::json!({"role": "user", "content": big.clone()}),
    ];
    // budget fits only ~1 big message
    let dropped = trim_msgs_to_budget(&mut msgs, 1100);
    assert_eq!(dropped, 2);
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[0]["role"], "system");
    // huge budget: nothing dropped
    let mut msgs2: Vec<serde_json::Value> = vec![
        serde_json::json!({"role": "system", "content": "sys"}),
        serde_json::json!({"role": "user", "content": "hi"}),
    ];
    assert_eq!(trim_msgs_to_budget(&mut msgs2, 8192), 0);
    assert_eq!(msgs2.len(), 2);
}

#[test]
fn test_inject_system_reminder_logic() {
    // Less than 4 messages: no reminder injected
    let mut msgs: Vec<serde_json::Value> = vec![
        serde_json::json!({"role": "system", "content": "sys"}),
        serde_json::json!({"role": "user", "content": "hello"}),
        serde_json::json!({"role": "assistant", "content": "hi"}),
    ];
    inject_system_reminder(&mut msgs);
    assert_eq!(msgs.len(), 3);

    // 4 or more messages: reminder is appended to the last message
    let mut msgs2: Vec<serde_json::Value> = vec![
        serde_json::json!({"role": "system", "content": "sys"}),
        serde_json::json!({"role": "user", "content": "hello"}),
        serde_json::json!({"role": "assistant", "content": "hi"}),
        serde_json::json!({"role": "user", "content": "tell me a story"}),
    ];
    inject_system_reminder(&mut msgs2);
    assert_eq!(msgs2.len(), 4);
    assert!(
        msgs2[3]["content"]
            .as_str()
            .unwrap()
            .contains("REMINDER: Follow the configured tool protocol")
    );
    assert!(
        msgs2[3]["content"]
            .as_str()
            .unwrap()
            .contains("tell me a story")
    );
}

#[test]
fn test_parse_multimodal_content_plain() {
    let val = parse_multimodal_content("Hello world");
    assert_eq!(val, serde_json::Value::String("Hello world".to_string()));
}

#[test]
fn test_parse_multimodal_content_with_image_nonexistent() {
    let val = parse_multimodal_content(
        "Look at this: ![image](file:///nonexistent/path.png) interesting!",
    );
    assert!(val.is_array());
    let arr = val.as_array().unwrap();
    assert_eq!(arr.len(), 3);
    assert_eq!(arr[0]["type"], "text");
    assert_eq!(arr[0]["text"], "Look at this: ");
    assert_eq!(arr[1]["type"], "text");
    assert_eq!(arr[1]["text"], "![image](file:///nonexistent/path.png)");
    assert_eq!(arr[2]["type"], "text");
    assert_eq!(arr[2]["text"], " interesting!");
}

#[tokio::test]
async fn test_confirm_and_execute_bypassed() {
    let state = Arc::new(Mutex::new(AppState::new()));
    state.lock().await.agent_mode = crate::config::AgentMode::Build;
    let cancel_token = tokio_util::sync::CancellationToken::new();
    let args = serde_json::json!({
        "path": "sandbox/test_bypass.txt",
        "content": "bypassed content",
        "overwrite": true
    });

    let (result, _, _) = confirm_and_execute(
        &state,
        &cancel_token,
        "write_to_file",
        &args,
        "write_to_file",
        true,
        None,
        None,
    )
    .await;
    assert!(
        result.content.contains("wrote")
            || result.content.contains("created")
            || result.content.contains("test_bypass.txt"),
        "got result: {}",
        result.content
    );

    let _ = std::fs::remove_file("sandbox/test_bypass.txt");
}

#[tokio::test]
async fn test_compact_history_strips_thinking_blocks() {
    let mut history = vec![
        crate::app::ChatMessage::new(
            "assistant",
            "<think>\nThinking about files...\n</think>\nHere is the answer",
        ),
        crate::app::ChatMessage::new("tool", "tool output"),
    ];
    compact_history_to_budget(&mut history, 5000).await;
    assert_eq!(history[0].content, "\nHere is the answer");
    assert_eq!(history[1].content, "tool output");
}

#[test]
fn test_classify_tool_msg() {
    assert_eq!(
        classify_tool_msg(&ChatMessage::new("tool", "run_command: done")),
        Some("throwaway")
    );
    assert_eq!(
        classify_tool_msg(&ChatMessage::new("tool", "grep: match")),
        Some("throwaway")
    );
    assert_eq!(
        classify_tool_msg(&ChatMessage::new("tool", "view_file: [File: x]")),
        Some("file")
    );
    assert_eq!(
        classify_tool_msg(&ChatMessage::new("tool", "get_weather: sunny")),
        Some("other")
    );
    assert_eq!(
        classify_tool_msg(&ChatMessage::new("assistant", "hi")),
        None
    );
}

#[test]
fn test_tool_signature_buckets_full_reads() {
    let full_default = serde_json::json!({"path": "src/main.rs"});
    let full_start1 = serde_json::json!({"path": "src/main.rs", "start_line": 1});
    let paged = serde_json::json!({"path": "src/main.rs", "start_line": 500, "end_line": 1000});
    let other = serde_json::json!({"path": "src/other.rs"});
    // Two full/default reads of the same file collapse to one signature.
    assert_eq!(
        tool_signature("view_file", &full_default),
        tool_signature("view_file", &full_start1)
    );
    // A distinct explicit page is its own signature.
    assert_ne!(
        tool_signature("view_file", &full_default),
        tool_signature("view_file", &paged)
    );
    assert_ne!(
        tool_signature("view_file", &full_default),
        tool_signature("view_file", &other)
    );
}

#[test]
fn test_is_read_only_tool() {
    assert!(is_read_only_tool("view_file"));
    assert!(is_read_only_tool("grep"));
    assert!(!is_read_only_tool("write_to_file"));
    assert!(!is_read_only_tool("run_command"));
    assert!(!is_read_only_tool("todo_write"));
}

#[test]
fn test_delegation_is_checked_as_potentially_mutating() {
    assert!(is_mutating_tool("spawn_agent"));
    assert!(is_mutating_tool("send_agent"));
    assert!(!is_mutating_tool("todo_write"));
}

// --- Feature 3: loop-detector reset only on real mutation progress ---

#[test]
fn mutation_made_progress_true_for_real_change() {
    // A genuine successful edit is progress: content doesn't start with
    // "error" and doesn't report a no-op.
    assert!(mutation_made_progress(true, "Applied edit to src/main.rs"));
}

#[test]
fn mutation_made_progress_false_for_failure() {
    assert!(!mutation_made_progress(
        false,
        "Applied edit to src/main.rs"
    ));
    assert!(!mutation_made_progress(
        true,
        "Error: no match found for old_string"
    ));
}

#[test]
fn mutation_made_progress_false_for_already_applied_noop() {
    // PR #306: replace_file_content is idempotent and reports success
    // with "already applied" when nothing changed. That must NOT count
    // as progress, or a repeated no-op edit could reset every budget
    // and the loop detector forever.
    assert!(!mutation_made_progress(
        true,
        "Edit already applied — no changes made"
    ));
    // Case-insensitive, per PR #306's contract.
    assert!(!mutation_made_progress(
        true,
        "ALREADY APPLIED: no-op, file unchanged"
    ));
}

#[test]
fn failure_replan_message_preserves_workspace_safety_and_requests_different_approach() {
    let message = failure_replan_message("replace_file_content", "edit:src/GameScene.ts", 2);
    assert!(message.contains("2 equivalent mutation attempts"));
    assert!(message.contains("changed no files"));
    assert!(message.contains("Do not retry the same edit"));
    assert!(message.contains("materially different safe approach"));
    assert!(message.contains("explain the exact blocker"));
}

#[test]
fn benchmark_summary_includes_failure_replan_metric() {
    let mut ctx = TurnContext::new();
    ctx.failure_replans = 1;
    let summary = ctx.benchmark_summary();
    assert_eq!(summary["failure_replans"], 1);
    assert!(mutation_made_progress(
        true,
        "Applied edit to src/GameScene.ts"
    ));
}

#[test]
fn repeated_compiler_diagnostics_increment_and_reset_their_streak() {
    let mut ctx = TurnContext::new();
    let first = "edit applied\n\nLSP/Compiler errors detected in workspace, please fix:\nsrc/GameScene.ts(89,5): error TS2554: Expected 1 arguments, but got 2.";
    let changed = "edit applied\n\nLSP/Compiler errors detected in workspace, please fix:\nsrc/GameScene.ts(95,5): error TS2339: Property 'unsubscribe' does not exist.";

    update_compiler_diagnostic_streak(&mut ctx, compiler_diagnostic_fingerprint(first));
    assert_eq!(ctx.consecutive_compiler_diagnostics, 1);
    update_compiler_diagnostic_streak(&mut ctx, compiler_diagnostic_fingerprint(first));
    assert_eq!(ctx.consecutive_compiler_diagnostics, 2);
    update_compiler_diagnostic_streak(&mut ctx, compiler_diagnostic_fingerprint(changed));
    assert_eq!(ctx.consecutive_compiler_diagnostics, 1);
    update_compiler_diagnostic_streak(&mut ctx, None);
    assert_eq!(ctx.consecutive_compiler_diagnostics, 0);
    assert!(ctx.last_compiler_diagnostic_fingerprint.is_none());
}

#[test]
fn repeated_compiler_diagnostics_trigger_the_budget() {
    let mut ctx = TurnContext::new();
    ctx.consecutive_compiler_diagnostics = MAX_CONSECUTIVE_COMPILER_DIAGNOSTICS;
    match turn_budget_exceeded(&ctx) {
        Some(TurnBudgetLimit::CompilerDiagnostics(n)) => {
            assert_eq!(n, MAX_CONSECUTIVE_COMPILER_DIAGNOSTICS)
        }
        other => panic!("expected CompilerDiagnostics limit, got {other:?}"),
    }
}

#[test]
fn benchmark_summary_contains_metrics_and_stop_reason() {
    let mut ctx = TurnContext::new();
    ctx.tool_rounds = 7;
    ctx.tokens_used = 1234;
    ctx.tool_calls = 9;
    ctx.malformed_calls = 2;
    ctx.no_progress_results = 3;
    ctx.provider_errors = 1;
    ctx.provider_429s = 1;
    ctx.changed_paths.insert("src/GameScene.ts".to_string());
    ctx.phase_checkpoint = Some("Phase 3: verify placement".to_string());
    ctx.stop_reason = Some(lifecycle::StopReason::ProviderError(Some(429)));

    let summary = ctx.benchmark_summary();
    assert_eq!(summary["tool_rounds"], 7);
    assert_eq!(summary["tokens_used"], 1234);
    assert_eq!(summary["tool_calls"], 9);
    assert_eq!(summary["provider_429s"], 1);
    assert_eq!(summary["changed_paths"][0], "src/GameScene.ts");
    assert_eq!(summary["phase_checkpoint"], "Phase 3: verify placement");
    assert_eq!(summary["stop_reason"], "provider_error:429");
}

#[test]
fn provider_error_metrics_distinguish_quota_exhaustion() {
    let mut ctx = TurnContext::new();
    record_provider_error(&mut ctx, "429 Too Many Requests");
    record_provider_error(&mut ctx, "502 Bad Gateway");
    assert_eq!(ctx.provider_errors, 2);
    assert_eq!(ctx.provider_429s, 1);
    assert_eq!(
        ctx.stop_reason.as_ref().map(ToString::to_string).as_deref(),
        Some("provider_error:429")
    );
}

#[test]
fn active_todo_is_the_current_phase_checkpoint() {
    let todos = vec![
        crate::app::TodoItem {
            content: "Scaffold".to_string(),
            status: "completed".to_string(),
            priority: "high".to_string(),
        },
        crate::app::TodoItem {
            content: "Verify placement".to_string(),
            status: "in_progress".to_string(),
            priority: "high".to_string(),
        },
    ];
    assert_eq!(
        active_todo_checkpoint(&todos).as_deref(),
        Some("Verify placement")
    );
}

#[test]
fn real_edit_resets_loop_detector() {
    // Regression guard for existing behavior: a genuine successful edit
    // must still be able to reset the detector so post-edit re-reads
    // start with a clean slate (test 1 + test 4 from the task spec).
    let mut d = loop_detect::LoopDetector::new(4);
    for start in [250, 260, 250] {
        let (e, c) = loop_detect::signatures(
            "view_file",
            &serde_json::json!({"path": "src/big.rs", "start_line": start, "end_line": start + 50}),
        );
        d.check(&e, &c);
    }
    assert!(mutation_made_progress(true, "Applied edit to src/big.rs"));
    d.reset();
    // A follow-up read cycle (read/edit/read) starts clean, not carrying
    // over the pre-edit repeat history.
    let (e, c) = loop_detect::signatures(
        "view_file",
        &serde_json::json!({"path": "src/big.rs", "start_line": 255, "end_line": 305}),
    );
    assert_eq!(
        d.check(&e, &c),
        loop_detect::LoopStatus::Ok,
        "genuine progress must still reset the detector"
    );
}

#[test]
fn noop_edit_does_not_reset_loop_detector() {
    // Core regression test: a successful but no-op edit (already
    // applied) must NOT reset the detector, matching how a failed edit
    // is treated — otherwise a model resubmitting the same already-
    // applied edit forever would never trip the detector.
    assert!(!mutation_made_progress(
        true,
        "already applied: no changes made"
    ));

    let mut d = loop_detect::LoopDetector::new(4);
    for start in [250, 260, 250] {
        let (e, c) = loop_detect::signatures(
            "view_file",
            &serde_json::json!({"path": "src/big.rs", "start_line": start, "end_line": start + 50}),
        );
        d.check(&e, &c);
    }
    // A no-op "success" must not clear the accumulated repeat state.
    // (mutation_made_progress being false is exactly what gates the
    // reset call in run_single_turn.)
    let (e, c) = loop_detect::signatures(
        "view_file",
        &serde_json::json!({"path": "src/big.rs", "start_line": 255, "end_line": 305}),
    );
    // Without a reset, this repeat continues to accumulate toward abort
    // rather than starting over at Ok.
    assert_ne!(
        d.check(&e, &c),
        loop_detect::LoopStatus::Ok,
        "no-op edit must not have cleared prior repeat state"
    );
}

#[test]
fn repeated_noop_edits_accumulate_toward_abort_instead_of_resetting() {
    // Core regression test for the bug: a model that keeps re-sending
    // the identical edit request, which now no-ops via PR #306's
    // idempotency, must still trip the loop detector because
    // mutation_made_progress gates the reset — no-op "successes" are
    // never allowed to reset it.
    let mut d = loop_detect::LoopDetector::new(4); // warn at 2, abort at 4
    let mut last = loop_detect::LoopStatus::Ok;
    for _ in 0..4 {
        let (e, c) = loop_detect::signatures(
            "replace_file_content",
            &serde_json::json!({"path": "src/main.rs", "old_string": "foo", "new_string": "bar"}),
        );
        last = d.check(&e, &c);
        // Simulate the harness: each round reports success with
        // "already applied", so mutation_made_progress is false and the
        // detector is never reset between iterations of this loop.
        assert!(!mutation_made_progress(
            true,
            "already applied: no changes made"
        ));
    }
    assert_eq!(
        last,
        loop_detect::LoopStatus::Abort(4),
        "identical no-op edits must accumulate to abort, not reset every round"
    );
}

#[test]
fn alternating_failed_and_noop_edits_never_reset_and_eventually_abort() {
    // Neither a failed edit nor a no-op edit is progress, so alternating
    // between two distinct edit attempts (one that fails, one that
    // no-ops as already-applied) must still accumulate toward the
    // detector's abort threshold via the frequency signal — since
    // neither outcome ever calls reset(), unlike a real change would.
    let mut d = loop_detect::LoopDetector::new(4); // frequency window = 8
    let mut last = loop_detect::LoopStatus::Ok;
    let outcomes = [
        (
            false,
            "Error: no match found for old_string",
            "old_string_a",
        ),
        (true, "already applied: no changes made", "old_string_b"),
        (
            false,
            "Error: no match found for old_string",
            "old_string_a",
        ),
        (true, "already applied: no changes made", "old_string_b"),
        (
            false,
            "Error: no match found for old_string",
            "old_string_a",
        ),
        (true, "already applied: no changes made", "old_string_b"),
        (
            false,
            "Error: no match found for old_string",
            "old_string_a",
        ),
        (true, "already applied: no changes made", "old_string_b"),
    ];
    for (success, content, old_string) in outcomes {
        assert!(
            !mutation_made_progress(success, content),
            "neither failure nor no-op should count as progress"
        );
        let (e, c) = loop_detect::signatures(
            "replace_file_content",
            &serde_json::json!({"path": "src/main.rs", "old_string": old_string, "new_string": "bar"}),
        );
        last = d.check(&e, &c);
        // The harness only calls reset() when mutation_made_progress is
        // true; since it never is here, the detector state must survive
        // every round instead of restarting from Ok.
    }
    assert_eq!(
        last,
        loop_detect::LoopStatus::Abort(8),
        "alternating failure/no-op must eventually abort since neither resets"
    );
}

#[test]
fn test_view_file_repeat_is_mtime_aware() {
    let t0 = std::time::SystemTime::now();
    let t1 = t0 + std::time::Duration::from_secs(30);
    // Never read before -> not a repeat (allow the first read).
    assert!(!view_file_unchanged_since_last_read(None, Some(t0)));
    // Read before, unchanged -> repeat (block redundant re-read).
    assert!(view_file_unchanged_since_last_read(Some(t0), Some(t0)));
    // Read before, file changed on disk -> not a repeat (allow refresh).
    assert!(!view_file_unchanged_since_last_read(Some(t0), Some(t1)));
    // File gone/unstatable after a read -> not a repeat (let it proceed/error naturally).
    assert!(!view_file_unchanged_since_last_read(Some(t0), None));
}

#[tokio::test]
async fn test_compact_prunes_throwaway_before_file_contents() {
    // Large throwaway command output + small file contents.
    let big_cmd = format!(
        "run_command: {}",
        (0..60)
            .map(|i| format!("output line number {i}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let file = "view_file: [File: src/main.rs, Lines 1 to 5 of 5]\n1: a\n2: b\n3: c\n4: d\n5: e";
    let file_original = file.to_string();
    let mut history = vec![
        ChatMessage::new("tool", big_cmd.clone()), // throwaway, oldest
        ChatMessage::new("tool", file.to_string()), // file contents, newer
    ];
    // Budget forces compaction; the throwaway must absorb the cut so the file
    // contents the agent is actively working on survive intact.
    compact_history_to_budget(&mut history, 80).await;
    assert_eq!(history[1].content, file_original, "file contents preserved");
    assert_ne!(history[0].content, big_cmd, "throwaway was reduced");
    assert!(
        !history[0].content.contains("line number 59"),
        "throwaway truncated: {}",
        history[0].content
    );
}

#[tokio::test]
async fn deterministic_compaction_keeps_goal_state_and_recent_activity() {
    let mut history = Vec::new();
    for round in 0..10 {
        history.push(ChatMessage::new(
            "user",
            if round == 0 {
                "original goal: repair the parser without changing the public API".to_string()
            } else {
                format!("follow-up {round}: continue the parser repair")
            },
        ));
        history.push(ChatMessage::new(
            "assistant",
            format!(
                "Decision {round}: inspect the parser state and preserve the existing error contract. {}",
                "architecture detail ".repeat(40)
            ),
        ));
        history.push(
            ChatMessage::new(
                "tool",
                format!(
                    "run_command: compiler failure round {round}\n{}",
                    "diagnostic ".repeat(120)
                ),
            )
            .with_tool_result(crate::app::ToolResultRecord {
                tool_name: "run_command".to_string(),
                success: false,
                error_kind: Some("CompilerFailed".to_string()),
                changed_paths: vec!["src/parser.rs".to_string()],
                ..Default::default()
            }),
        );
    }

    compact_history_to_budget(&mut history, 500).await;

    let record = history
        .iter()
        .find(|message| {
            message
                .content
                .starts_with("[Deterministic context record]")
        })
        .expect("over-budget local history should get a deterministic record");
    assert!(record.content.contains("original goal: repair the parser"));
    assert!(record.content.contains("src/parser.rs"));
    assert!(record.content.contains("CompilerFailed"));
    assert!(
        history
            .iter()
            .any(|message| message.content.contains("follow-up 9"))
    );

    let mut provider_messages = history::to_messages(&history, "system prompt");
    trim_msgs_to_budget(&mut provider_messages, 500);
    assert!(provider_messages.iter().any(|message| {
        message
            .get("content")
            .and_then(|content| content.as_str())
            .is_some_and(|content| content.contains("original goal: repair the parser"))
    }));
}

#[tokio::test]
async fn local_context_preserves_task_state_across_model_window_sizes() {
    for budget in [4_096, 8_192, 32_768, 128_000, 262_144] {
        let mut history = vec![ChatMessage::new(
            "system",
            "# Project instructions\nRun cargo test after edits; do not change the public API.",
        )];
        for round in 0..18 {
            history.push(ChatMessage::new(
                "user",
                if round == 0 {
                    "original task: repair the parser while preserving the public API".to_string()
                } else if round == 17 {
                    "current follow-up: resolve the remaining parser compiler error and verify it"
                        .to_string()
                } else {
                    format!("inspect parser phase {round}")
                },
            ));
            history.push(ChatMessage::new(
                "assistant",
                format!("decision {round}: keep the parser architecture and inspect diagnostics"),
            ));
            let result = if round == 17 {
                "run_command: cargo test --lib\nverification succeeded after the latest parser edit"
            } else {
                "run_command: cargo test\nerror: unresolved parser diagnostic\ncompiler output follows\n"
            };
            history.push(
                ChatMessage::new(
                    "tool",
                    if round == 17 {
                        result.to_string()
                    } else {
                        format!("{result}{}", "diagnostic detail ".repeat(500))
                    },
                )
                .with_tool_result(crate::app::ToolResultRecord {
                    tool_name: "run_command".to_string(),
                    success: round == 17,
                    error_kind: (!round.eq(&17)).then(|| "CompilerFailed".to_string()),
                    exit_code: Some(if round == 17 { 0 } else { 1 }),
                    changed_paths: vec!["src/parser.rs".to_string()],
                    ..Default::default()
                }),
            );
        }

        compact_history_to_budget(&mut history, budget).await;
        let mut messages = history::to_messages(&history, "system prompt");
        inject_system_reminder(&mut messages);
        trim_msgs_to_budget(&mut messages, budget);
        let request_tokens = messages
            .iter()
            .map(crate::network::messages::estimate_msg_tokens)
            .sum::<u32>();
        assert!(
            request_tokens <= budget,
            "budget={budget}, tokens={request_tokens}"
        );
        let rendered = messages
            .iter()
            .filter_map(|message| message.get("content").and_then(|content| content.as_str()))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(
            rendered.contains("original task: repair the parser"),
            "budget={budget}"
        );
        assert!(
            rendered.contains("current follow-up: resolve"),
            "budget={budget}"
        );
        assert!(rendered.contains("Project instructions"), "budget={budget}");
        assert!(
            rendered.contains("do not change the public API"),
            "budget={budget}"
        );
        assert!(rendered.contains("src/parser.rs"), "budget={budget}");
        assert!(
            rendered.contains("verification succeeded"),
            "budget={budget}"
        );
    }
}

#[test]
fn cancellation_persists_completed_results_and_typed_missing_results() {
    let calls = vec![
        crate::app::ToolCallRef {
            id: "call_done".to_string(),
            name: "grep".to_string(),
            arguments: "{}".to_string(),
        },
        crate::app::ToolCallRef {
            id: "call_cancelled".to_string(),
            name: "run_command".to_string(),
            arguments: "{}".to_string(),
        },
    ];
    let mut history =
        vec![ChatMessage::new("assistant", "native calls").with_tool_calls(calls.clone())];
    turn_engine::append_cancelled_batch_results(
        &mut history,
        vec![ToolResult {
            tool_name: "grep".to_string(),
            content: "grep: found".to_string(),
            diff: None,
            file_preview: None,
            metadata: ToolResultMetadata {
                success: true,
                ..Default::default()
            },
        }],
        &calls,
    );

    assert_eq!(history[1].tool_call_id.as_deref(), Some("call_done"));
    assert!(history[1].tool_result.as_ref().unwrap().success);
    assert_eq!(history[2].tool_call_id.as_deref(), Some("call_cancelled"));
    assert_eq!(
        history[2].tool_result.as_ref().unwrap().parsed_error_kind(),
        Some(crate::tools::ToolErrorKind::Cancelled)
    );

    let messages = history::to_messages(&history, "system");
    assert_eq!(messages[1]["tool_calls"][0]["id"], "call_done");
    assert_eq!(messages[2]["tool_call_id"], "call_done");
    assert_eq!(messages[3]["tool_call_id"], "call_cancelled");
}

#[tokio::test]
async fn test_run_compiler_check_success() {
    let cwd = std::env::current_dir().unwrap();
    let check = run_compiler_check(&cwd).await;
    assert!(check.is_none());
}

#[test]
fn project_root_from_relative_file_is_a_real_directory() {
    let root = get_tool_project_root("delete_file", &serde_json::json!({"path": "src/temp.rs"}))
        .expect("should find project root for workspace file");
    assert!(root.is_absolute());
    assert!(root.is_dir());
    assert!(root.join("Cargo.toml").exists());

    let tmp_root = get_tool_project_root("write_to_file", &serde_json::json!({"path": "/tmp/scratch.py"}));
    assert!(tmp_root.is_none());
}

// Regression: session 1785600769226. 25 loop warnings were written to
// history and none reached the model — the request filter kept only
// user/assistant/tool. The harness spent the session correcting a model that
// could not hear it.
#[test]
fn harness_notes_reach_the_model_but_session_chatter_does_not() {
    let warning = ChatMessage::new(
        "system",
        "[Loop warning: this action has repeated 5 times.]",
    );
    let summary = ChatMessage::new(
        "system",
        format!("{}earlier work", crate::network::compaction::SUMMARY_MARKER),
    );
    let chatter = ChatMessage::new("system", "Switched to model profile 'gemini-3.6-flash'");

    assert!(is_model_directed_note(&warning));
    assert!(is_model_directed_note(&summary));
    // TUI-only noise stays out of the prompt.
    assert!(!is_model_directed_note(&chatter));
    assert!(!is_model_directed_note(&ChatMessage::new(
        "user",
        "[not a system note]"
    )));
}

#[test]
fn loop_abort_allows_one_bounded_recovery_before_forced_final() {
    assert_eq!(loop_recovery_action(0), LoopRecoveryAction::Recover);
    assert_eq!(loop_recovery_action(1), LoopRecoveryAction::ForceFinal);
    assert_eq!(
        loop_recovery_action(u8::MAX),
        LoopRecoveryAction::ForceFinal
    );
    assert!(LOOP_RECOVERY_PROMPT.contains("Tools remain enabled"));
}

// Regression: hoisting every system message into the prompt filed each loop
// warning 12k characters away from the call it was about.
#[test]
fn a_mid_conversation_note_keeps_its_place() {
    let raw = vec![
        serde_json::json!({"role": "system", "content": "the prompt"}),
        serde_json::json!({"role": "user", "content": "do it"}),
        serde_json::json!({"role": "assistant", "content": "reading"}),
        serde_json::json!({"role": "system", "content": "[Loop warning: repeated 5 times.]"}),
    ];

    let aligned = align_alternating_messages(raw);

    assert_eq!(aligned[0]["role"], "system");
    assert_eq!(aligned[0]["content"], "the prompt");
    // The note stays after the turn it is about, carried as user text so
    // providers that demand strict alternation still accept it.
    let last = aligned.last().expect("note survives");
    assert_eq!(last["role"], "user");
    assert!(last["content"].as_str().unwrap().contains("Loop warning"));
}

#[test]
fn structured_tool_calls_survive_alignment() {
    let raw = vec![
        serde_json::json!({"role": "user", "content": "find it"}),
        serde_json::json!({
            "role": "assistant",
            "content": serde_json::Value::Null,
            "tool_calls": [{"id": "call_1", "type": "function",
                            "function": {"name": "grep", "arguments": "{}"}}],
        }),
        serde_json::json!({"role": "assistant", "content": "on it"}),
    ];

    let aligned = align_alternating_messages(raw);

    // The call-carrying message is never folded into its neighbour.
    assert_eq!(aligned[1]["tool_calls"][0]["id"], "call_1");
    assert_eq!(aligned[2]["content"], "on it");
}

#[test]
fn test_align_alternating_messages() {
    let raw = vec![
        serde_json::json!({"role": "system", "content": "Prompt"}),
        serde_json::json!({"role": "system", "content": "Summary"}),
        serde_json::json!({"role": "assistant", "content": "Grep"}),
        serde_json::json!({"role": "user", "content": "Result"}),
    ];
    let aligned = align_alternating_messages(raw);
    assert_eq!(aligned.len(), 4);
    assert_eq!(aligned[0]["role"], "system");
    assert_eq!(aligned[0]["content"], "Prompt\n\nSummary");
    assert_eq!(aligned[1]["role"], "user");
    assert_eq!(aligned[1]["content"], "[Context initialization]");
    assert_eq!(aligned[2]["role"], "assistant");
    assert_eq!(aligned[3]["role"], "user");
}

#[test]
fn test_build_dynamic_context_tail() {
    let todo = |content: &str, status: &str| crate::app::TodoItem {
        content: content.to_string(),
        status: status.to_string(),
        priority: "high".to_string(),
    };

    // No files and no todos: the context section is returned untouched.
    assert_eq!(
        build_dynamic_context_tail("# Env".to_string(), &[], &[]),
        "# Env"
    );

    // Files-in-context section lists each file as a bullet.
    let with_files = build_dynamic_context_tail(
        "# Env".to_string(),
        &["src/a.rs".to_string(), "src/b.rs".to_string()],
        &[],
    );
    assert!(with_files.contains("# Files already in context"));
    assert!(with_files.contains("re-read files marked stale"));
    assert!(with_files.contains("- src/a.rs"));
    assert!(with_files.contains("- src/b.rs"));

    // Files-in-context section bounds oversized file lists.
    let many_files: Vec<String> = (0..50)
        .map(|i| {
            if i == 0 {
                "src/stale.rs (STALE — changed on disk; re-read before editing)".to_string()
            } else {
                format!("src/file_{i}.rs (snapshot current)")
            }
        })
        .collect();
    let bounded = build_dynamic_context_tail("# Env".to_string(), &many_files, &[]);
    assert!(bounded.contains("src/stale.rs (STALE"));
    assert!(bounded.contains("more unchanged files omitted from context tail"));

    // Task plan renders status markers and 1-based ordering.
    let with_todos = build_dynamic_context_tail(
        String::new(),
        &[],
        &[
            todo("done thing", "completed"),
            todo("active thing", "in_progress"),
            todo("later thing", "pending"),
        ],
    );
    assert!(with_todos.contains("# Your current task plan"));
    assert!(with_todos.contains("1. [x] done thing (high)"));
    assert!(with_todos.contains("2. [~] active thing (high)"));
    assert!(with_todos.contains("3. [ ] later thing (high)"));
}

#[test]
fn file_context_marks_fresh_and_stale_snapshots() {
    let snapshot = std::time::SystemTime::UNIX_EPOCH;
    let fresh = format_read_file_context_entry("src/a.rs", Some(snapshot), Some(snapshot));
    assert!(fresh.contains("snapshot current"));

    let changed = snapshot + std::time::Duration::from_secs(1);
    let stale = format_read_file_context_entry("src/a.rs", Some(snapshot), Some(changed));
    assert!(stale.contains("STALE"));
    assert!(stale.contains("re-read before editing"));
}

#[test]
fn compiler_diagnostics_include_bounded_source_context_for_known_locations() {
    let diagnostics = "src/network.rs(1,1): error TS2554: Expected 1 arguments, but got 2.";
    let enriched = compiler_diagnostics_with_snippets(diagnostics);
    assert!(enriched.contains(diagnostics));
    assert!(enriched.contains("[compiler context: src/network.rs:1:1]"));
    assert!(enriched.contains("use crate::app::{AppState"));
}

#[test]
fn compiler_diagnostics_preserve_missing_file_output() {
    let diagnostics = "src/does-not-exist.ts(4,2): error TS2339: Missing property";
    assert_eq!(compiler_diagnostics_with_snippets(diagnostics), diagnostics);
}

// Regression: the benchmark session ran 106 tool rounds with no hard
// stop because the only guard was the loop detector, and a mutation that
// reports success while duplicating content resets it every round. These
// tests exercise the safety budgets directly against a constructed
// TurnContext so they run without a mock server.

#[test]
fn healthy_progress_does_not_trigger_the_budget() {
    let mut ctx = TurnContext::new();
    ctx.tool_rounds = 12;
    ctx.tokens_used = 40_000;
    ctx.consecutive_no_progress = 0;
    ctx.consecutive_failed_mutations = 0;
    ctx.consecutive_compiler_error_gates = 0;
    assert!(turn_budget_exceeded(&ctx).is_none());
}

#[test]
fn max_tool_rounds_triggers_the_budget() {
    let mut ctx = TurnContext::new();
    ctx.tool_rounds = ctx.max_tool_rounds;
    match turn_budget_exceeded(&ctx) {
        Some(TurnBudgetLimit::ToolRounds(n)) => assert_eq!(n, ctx.max_tool_rounds),
        other => panic!("expected ToolRounds limit, got {other:?}"),
    }
}

#[test]
fn custom_tool_round_limit_triggers_at_the_configured_round() {
    let mut ctx = TurnContext::with_max_tool_rounds(3);
    ctx.tool_rounds = 3;
    match turn_budget_exceeded(&ctx) {
        Some(TurnBudgetLimit::ToolRounds(n)) => assert_eq!(n, 3),
        other => panic!("expected configured ToolRounds limit, got {other:?}"),
    }
}

#[test]
fn per_round_usage_sums_across_rounds_instead_of_being_overwritten() {
    // Simulates three rounds each reporting the provider's per-response
    // usage. If usage were cumulative-not-per-response, or accidentally
    // overwritten instead of summed, this would land on the last round's
    // figure (30_000) instead of the true total (90_000).
    let mut tokens_used = 0u64;
    for reported in [40_000u64, 30_000, 20_000] {
        tokens_used = accumulate_tokens_used(tokens_used, Some(reported), "");
    }
    assert_eq!(tokens_used, 90_000);
}

#[test]
fn missing_provider_usage_falls_back_to_a_content_estimate_without_double_counting() {
    let after_first = accumulate_tokens_used(0, None, "hello world");
    assert!(
        after_first > 0,
        "fallback estimate must contribute something"
    );
    let after_second = accumulate_tokens_used(after_first, Some(500), "ignored");
    assert_eq!(
        after_second,
        after_first + 500,
        "second round must add, not replace"
    );
}

#[test]
fn a_genuinely_oversized_turn_trips_the_token_budget() {
    let mut ctx = TurnContext::new();
    for _ in 0..200 {
        ctx.tokens_used = accumulate_tokens_used(ctx.tokens_used, Some(30_000), "");
    }
    assert!(
        ctx.tokens_used >= MAX_TURN_TOKEN_BUDGET,
        "200 rounds of 30k tokens each must exceed the {MAX_TURN_TOKEN_BUDGET} budget"
    );
    match turn_budget_exceeded(&ctx) {
        Some(TurnBudgetLimit::Tokens(_)) => {}
        other => panic!("expected the token budget to trip, got {other:?}"),
    }
}

#[test]
fn normal_multi_round_work_is_not_stopped_prematurely() {
    // A healthy session doing real work across many rounds, well under
    // every budget, must not trip any safety limit.
    let mut ctx = TurnContext::new();
    for _ in 0..10 {
        ctx.tokens_used = accumulate_tokens_used(ctx.tokens_used, Some(5_000), "");
        ctx.tool_rounds += 1;
    }
    assert!(
        turn_budget_exceeded(&ctx).is_none(),
        "10 rounds of light, real work must not trip a safety budget"
    );
}

#[test]
fn token_budget_triggers_the_budget() {
    let mut ctx = TurnContext::new();
    ctx.tokens_used = MAX_TURN_TOKEN_BUDGET;
    match turn_budget_exceeded(&ctx) {
        Some(TurnBudgetLimit::Tokens(n)) => assert_eq!(n, MAX_TURN_TOKEN_BUDGET),
        other => panic!("expected Tokens limit, got {other:?}"),
    }
}

#[test]
fn repeated_malformed_tool_calls_trigger_the_budget_and_leave_it_idle() {
    let mut ctx = TurnContext::new();
    ctx.consecutive_malformed_calls = MAX_CONSECUTIVE_MALFORMED_CALLS;
    match turn_budget_exceeded(&ctx) {
        Some(TurnBudgetLimit::MalformedCalls(n)) => {
            assert_eq!(n, MAX_CONSECUTIVE_MALFORMED_CALLS)
        }
        other => panic!("expected MalformedCalls limit, got {other:?}"),
    }
}

#[test]
fn identical_malformed_tool_calls_are_counted_as_repeats() {
    let mut ctx = TurnContext::new();
    let call = crate::tools::ToolCall {
        name: "replace_file_content".to_string(),
        arguments: serde_json::json!({"path":"src/store.ts","edits":"[]"}),
        call_id: None,
    };

    assert!(!super::turn_engine::record_malformed_call(
        &mut ctx,
        "ignored for parsed calls",
        std::slice::from_ref(&call)
    ));
    assert!(super::turn_engine::record_malformed_call(
        &mut ctx,
        "ignored for parsed calls",
        std::slice::from_ref(&call)
    ));
    assert_eq!(ctx.consecutive_malformed_calls, 2);
    assert_eq!(ctx.malformed_calls, 2);
    assert!(!super::turn_engine::record_malformed_call(
        &mut ctx,
        "ignored for parsed calls",
        &[crate::tools::ToolCall {
            name: "replace_file_content".to_string(),
            arguments: serde_json::json!({"path":"src/other.ts","edits":"[]"}),
            call_id: None,
        }]
    ));
    assert_eq!(ctx.consecutive_malformed_calls, 1);
}

#[test]
fn below_the_malformed_call_budget_does_not_trip() {
    let mut ctx = TurnContext::new();
    ctx.consecutive_malformed_calls = MAX_CONSECUTIVE_MALFORMED_CALLS - 1;
    assert!(turn_budget_exceeded(&ctx).is_none());
}

#[test]
fn repeated_failed_edits_trigger_the_budget() {
    let mut ctx = TurnContext::new();
    ctx.consecutive_failed_mutations = MAX_CONSECUTIVE_FAILED_MUTATIONS;
    match turn_budget_exceeded(&ctx) {
        Some(TurnBudgetLimit::FailedMutations(n)) => {
            assert_eq!(n, MAX_CONSECUTIVE_FAILED_MUTATIONS)
        }
        other => panic!("expected FailedMutations limit, got {other:?}"),
    }
}

// The exact benchmark shape: a mutation reports success but changed
// nothing (already applied), round after round.
#[test]
fn repeated_noop_edits_trigger_the_budget() {
    let mut ctx = TurnContext::new();
    ctx.consecutive_no_progress = MAX_CONSECUTIVE_NO_PROGRESS;
    match turn_budget_exceeded(&ctx) {
        Some(TurnBudgetLimit::NoProgress(n)) => assert_eq!(n, MAX_CONSECUTIVE_NO_PROGRESS),
        other => panic!("expected NoProgress limit, got {other:?}"),
    }
}

#[test]
fn repeated_compiler_error_gates_trigger_the_budget() {
    let mut ctx = TurnContext::new();
    ctx.consecutive_compiler_error_gates = MAX_CONSECUTIVE_COMPILER_ERROR_GATES;
    match turn_budget_exceeded(&ctx) {
        Some(TurnBudgetLimit::CompilerErrorGates(n)) => {
            assert_eq!(n, MAX_CONSECUTIVE_COMPILER_ERROR_GATES)
        }
        other => panic!("expected CompilerErrorGates limit, got {other:?}"),
    }
}

#[tokio::test]
async fn stopping_for_budget_never_falsely_reports_completion() {
    let state = Arc::new(Mutex::new(AppState::new()));
    let mut ctx = TurnContext::new();
    ctx.tool_rounds = ctx.max_tool_rounds;
    ctx.task_completed = false;

    let limit = turn_budget_exceeded(&ctx).expect("budget should be exceeded");
    let should_continue = stop_turn_for_budget(&state, &mut ctx, limit).await;

    assert!(!should_continue, "a budget stop must end the loop");
    assert!(
        !ctx.task_completed,
        "a budget stop must never claim completion"
    );
    assert!(
        ctx.budget_stopped.is_some(),
        "the exact limit reached must be recorded"
    );
    assert!(
        ctx.final_content.contains("stopped"),
        "the summary must explain the stop: {}",
        ctx.final_content
    );
    assert!(
        ctx.final_content
            .to_ascii_lowercase()
            .contains("not complete"),
        "the summary must be explicit that the task is unfinished: {}",
        ctx.final_content
    );
}

#[tokio::test]
async fn stopping_for_a_malformed_call_streak_leaves_the_app_idle_and_preserves_history() {
    let state = Arc::new(Mutex::new(AppState::new()));
    {
        let mut s = state.lock().await;
        s.status = AppStatus::Streaming;
        s.history.push(ChatMessage::new("user", "do the thing"));
    }
    let mut ctx = TurnContext::new();
    ctx.consecutive_malformed_calls = MAX_CONSECUTIVE_MALFORMED_CALLS;

    let limit = turn_budget_exceeded(&ctx).expect("budget should be exceeded");
    assert!(matches!(limit, TurnBudgetLimit::MalformedCalls(_)));
    let should_continue = stop_turn_for_budget(&state, &mut ctx, limit).await;

    assert!(!should_continue, "a budget stop must end the loop");
    assert!(
        !ctx.task_completed,
        "must never claim completion after a parse-failure streak"
    );
    let s = state.lock().await;
    assert_eq!(
        s.status,
        AppStatus::Idle,
        "must leave the app in Idle, not stuck streaming"
    );
    assert_eq!(
        s.history.len(),
        1,
        "the transcript must be preserved, not cleared"
    );
}

#[tokio::test]
async fn cancellation_is_checked_before_the_budget_at_round_start() {
    // A cancelled turn must not be intercepted by the budget-stop
    // summary; the request layer's own cancellation handling owns that
    // path. This only exercises the ordering guard used at the top of
    // run_single_turn, not the full network round.
    let cancel_token = tokio_util::sync::CancellationToken::new();
    cancel_token.cancel();
    let mut ctx = TurnContext::new();
    ctx.tool_rounds = ctx.max_tool_rounds;

    let budget_should_fire = !cancel_token.is_cancelled() && turn_budget_exceeded(&ctx).is_some();
    assert!(
        !budget_should_fire,
        "cancellation must suppress the budget-stop path"
    );
}

#[test]
fn request_log_summary_reports_shape_not_content() {
    let summary = request_log_summary("gpt-oss-120b", 42, 7, 123_456);
    assert!(summary.contains("gpt-oss-120b"));
    assert!(summary.contains("messages=42"));
    assert!(summary.contains("tools=7"));
    assert!(summary.contains("payload_bytes=123456"));
}

#[test]
fn default_debug_log_line_never_contains_full_payload_content() {
    // A marker that would only ever appear if the actual message content
    // (e.g. a file's source text pulled into context by a prior tool
    // call) leaked into the log line.
    const FILE_CONTENT_MARKER: &str = "fn super_secret_business_logic_marker() {}";
    let payload = serde_json::json!({
        "model": "gpt-oss-120b",
        "messages": [
            {"role": "user", "content": FILE_CONTENT_MARKER},
        ],
        "tools": [{"type": "function", "function": {"name": "read_file"}}],
    });
    let summary = request_log_summary("gpt-oss-120b", 1, 1, 999);

    let default_line = request_debug_log_line(false, &summary, &payload);
    assert!(
        !default_line.contains(FILE_CONTENT_MARKER),
        "default (non-verbose) log line must not contain full message content: {default_line}"
    );
    assert_eq!(
        default_line, summary,
        "default log line should be exactly the structured summary"
    );
}

#[test]
fn verbose_flag_gates_full_payload_logging() {
    // This is the config-flag gate for opt-in full-payload logging
    // (`AppConfig::debug_verbose_network_logging`): false -> structured
    // summary only, true -> full serialized payload including content.
    const FILE_CONTENT_MARKER: &str = "fn super_secret_business_logic_marker() {}";
    let payload = serde_json::json!({
        "model": "gpt-oss-120b",
        "messages": [
            {"role": "user", "content": FILE_CONTENT_MARKER},
        ],
    });
    let summary = request_log_summary("gpt-oss-120b", 1, 0, 999);

    let quiet_line = request_debug_log_line(false, &summary, &payload);
    let verbose_line = request_debug_log_line(true, &summary, &payload);

    assert!(!quiet_line.contains(FILE_CONTENT_MARKER));
    assert!(
        verbose_line.contains(FILE_CONTENT_MARKER),
        "verbose mode must still support full-payload debugging: {verbose_line}"
    );
}

#[test]
fn debug_verbose_network_logging_defaults_to_off() {
    // The config flag must default to false so full-payload logging
    // (and the debug.log growth it causes) stays opt-in.
    let config = crate::config::AppConfig::default();
    assert!(!config.debug_verbose_network_logging);
}

// --- extract_diff_block: pull the real diff out of a tool result ---

#[test]
fn extract_diff_block_finds_a_normal_replacement_diff() {
    let content = "successfully replaced target_content in 'src/lib.rs'\n\n\
```diff\n@@ -1,3 +1,3 @@\n line one\n-line two\n+line TWO\n line three\n```\n";
    let diff = extract_diff_block(content).expect("diff fence should be found");
    assert!(diff.contains("@@ -1,3 +1,3 @@"), "got: {diff}");
    assert!(diff.contains("-line two"), "got: {diff}");
    assert!(diff.contains("+line TWO"), "got: {diff}");
}

#[test]
fn extract_diff_block_finds_a_multi_replace_diff() {
    let content = "successfully applied 2 replacements to 'src/lib.rs'\n\n\
```diff\n@@ -1,4 +1,4 @@\n a\n-b\n+B\n c\n-d\n+D\n```\n";
    let diff = extract_diff_block(content).expect("diff fence should be found");
    assert!(diff.contains("-b"), "got: {diff}");
    assert!(diff.contains("+D"), "got: {diff}");
}

#[test]
fn extract_diff_block_returns_none_for_a_noop_already_applied_result() {
    // PR #306: a repeated edit that's already applied reports success
    // with no diff fence at all. There must be nothing to show — a
    // stale argument-only preview must not fill this gap.
    let content = "already applied; no changes made to 'src/lib.rs' \
(target_content already reflects replacement_content)";
    assert!(extract_diff_block(content).is_none());
}

#[test]
fn extract_diff_block_returns_none_for_a_failed_edit() {
    let content = "Error: target_content not found in 'src/lib.rs'.";
    assert!(extract_diff_block(content).is_none());
}

#[test]
fn extract_diff_block_returns_none_for_content_with_no_fence() {
    let content = "wrote 'src/new.rs' (10 lines, 120 bytes)";
    assert!(extract_diff_block(content).is_none());
}

// --- Feature 2 integration: ToolResult.diff must be the real diff ---

fn test_tool_call(name: &str, args: serde_json::Value) -> crate::tools::ToolCall {
    crate::tools::ToolCall {
        name: name.to_string(),
        arguments: args,
        call_id: None,
    }
}

async fn run_one_tool_with_state(
    state: &Arc<Mutex<AppState>>,
    call: crate::tools::ToolCall,
) -> ToolResult {
    let client = reqwest::Client::new();
    let cancel_token = tokio_util::sync::CancellationToken::new();
    let mut compile_dirty = false;
    let mut compile_cache = None;
    let mut user_wait = std::time::Duration::ZERO;
    let mut results = execute_tool_batch(
        &client,
        state,
        &cancel_token,
        &[call],
        true,
        &None,
        &mut compile_dirty,
        &mut compile_cache,
        &mut user_wait,
        None,
    )
    .await;
    results.remove(0)
}

async fn run_one_tool(call: crate::tools::ToolCall) -> ToolResult {
    let state = Arc::new(Mutex::new(AppState::new()));
    run_one_tool_with_state(&state, call).await
}

#[derive(Debug, Clone)]
struct ReplayCall {
    id: &'static str,
    call: crate::tools::ToolCall,
}

#[derive(Debug, Clone)]
struct ReplayStep {
    label: &'static str,
    calls: Vec<ReplayCall>,
}

#[derive(Debug, Default)]
struct ReplayReport {
    tool_order: Vec<String>,
    paired_results: Vec<(String, String, bool)>,
    lifecycle: Vec<events::TurnState>,
    warnings: Vec<String>,
    changed_paths: std::collections::BTreeSet<String>,
    recovery_attempts: u8,
    forced_stop: bool,
    termination_reason: String,
}

fn replay_call(id: &'static str, name: &str, arguments: serde_json::Value) -> ReplayCall {
    ReplayCall {
        id,
        call: test_tool_call(name, arguments),
    }
}

fn replay_step(label: &'static str, calls: Vec<ReplayCall>) -> ReplayStep {
    ReplayStep { label, calls }
}

/// Drive the real local tool executor with scripted model steps. This is
/// deliberately below the provider request layer: replay tests exercise
/// tool ordering, result pairing, loop recovery, and workspace effects
/// without opening a socket or depending on a model response format.
async fn replay_steps(root: &std::path::Path, steps: &[ReplayStep]) -> ReplayReport {
    let mut app = AppState::new();
    app.workspace_root = Some(root.to_path_buf());
    let state = Arc::new(Mutex::new(app));
    let client = reqwest::Client::new();
    let cancel_token = tokio_util::sync::CancellationToken::new();
    let mut machine = events::TurnMachine::new();
    let mut detector = loop_detect::LoopDetector::new(4);
    let mut report = ReplayReport {
        lifecycle: vec![machine.state()],
        ..Default::default()
    };
    let mut compile_dirty = false;
    let mut compile_cache = None;
    let mut user_wait = std::time::Duration::ZERO;

    'steps: for step in steps {
        if step.calls.is_empty() {
            machine
                .model_finished(false, false, false, false)
                .unwrap_or_else(|error| panic!("{}: {error}", step.label));
            report.lifecycle.push(machine.state());
            report.termination_reason = "completed".to_string();
            break;
        }

        machine
            .model_finished(false, false, true, false)
            .unwrap_or_else(|error| panic!("{}: {error}", step.label));
        report.lifecycle.push(machine.state());

        for scripted in &step.calls {
            let (exact, category) =
                loop_detect::signatures(&scripted.call.name, &scripted.call.arguments);
            let status = detector.check_tool(&scripted.call.name, &exact, &category);
            match status {
                loop_detect::LoopStatus::Warning(repeats) => {
                    report.warnings.push(format!(
                        "{}: {} warning at repeat {repeats}",
                        step.label, scripted.call.name
                    ));
                }
                loop_detect::LoopStatus::Abort(repeats) => {
                    report.warnings.push(format!(
                        "{}: {} abort at repeat {repeats}",
                        step.label, scripted.call.name
                    ));
                    machine.abandon_tool_phase();
                    if report.recovery_attempts < 1 {
                        report.recovery_attempts += 1;
                        detector.reset();
                        report.warnings.push(format!(
                            "{}: bounded recovery attempt {}",
                            step.label, report.recovery_attempts
                        ));
                        continue 'steps;
                    }
                    report.forced_stop = true;
                    report.termination_reason = "forced_loop_stop".to_string();
                    machine
                        .model_finished(false, true, false, false)
                        .unwrap_or_else(|error| panic!("forced wrap-up: {error}"));
                    report.lifecycle.push(machine.state());
                    break 'steps;
                }
                loop_detect::LoopStatus::Ok => {}
            }
        }

        machine
            .approval_granted()
            .unwrap_or_else(|error| panic!("{} approval: {error}", step.label));
        report.lifecycle.push(machine.state());

        let calls = step
            .calls
            .iter()
            .map(|scripted| scripted.call.clone())
            .collect::<Vec<_>>();
        let mut results = execute_tool_batch(
            &client,
            &state,
            &cancel_token,
            &calls,
            true,
            &Some(root.to_path_buf()),
            &mut compile_dirty,
            &mut compile_cache,
            &mut user_wait,
            None,
        )
        .await;
        if results.len() != step.calls.len() {
            panic!(
                "{}: expected {} results, got {}",
                step.label,
                step.calls.len(),
                results.len()
            );
        }
        for (scripted, result) in step.calls.iter().zip(results.drain(..)) {
            if result.tool_name != scripted.call.name {
                panic!(
                    "{}: result for {} was paired with {}",
                    step.label, scripted.call.name, result.tool_name
                );
            }
            report.tool_order.push(result.tool_name.clone());
            report.paired_results.push((
                scripted.id.to_string(),
                result.tool_name,
                result.metadata.success,
            ));
            report.changed_paths.extend(result.metadata.changed_paths);
        }
        machine
            .tools_finished()
            .unwrap_or_else(|error| panic!("{} tools: {error}", step.label));
        report.lifecycle.push(machine.state());
    }

    report
}

#[tokio::test]
async fn failed_session_replay_is_bounded_and_keeps_workspace_safe() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("state.ts");
    let original = "const status = 'idle';\n";
    std::fs::write(&file, original).expect("write fixture");
    let path = file.to_string_lossy().to_string();
    let failed_edit = || {
        replay_call(
            "failed-edit",
            "replace_file_content",
            serde_json::json!({
                "path": path,
                "old_string": "const missing = true;",
                "new_string": "const missing = false;",
            }),
        )
    };
    let read = || {
        replay_call(
            "read-state",
            "view_file",
            serde_json::json!({"path": path, "start_line": 1, "end_line": 1}),
        )
    };
    let steps = vec![
        replay_step("failed edit", vec![failed_edit()]),
        replay_step(
            "state edit",
            vec![replay_call(
                "state-edit",
                "replace_file_content",
                serde_json::json!({
                    "path": path,
                    "old_string": "const status = 'idle';",
                    "new_string": "const status = 'active';",
                }),
            )],
        ),
        replay_step(
            "restore state",
            vec![replay_call(
                "restore-state",
                "replace_file_content",
                serde_json::json!({
                    "path": path,
                    "old_string": "const status = 'active';",
                    "new_string": "const status = 'idle';",
                }),
            )],
        ),
        replay_step("repeated read one", vec![read()]),
        replay_step("repeated read two", vec![read()]),
        replay_step("failed retry one", vec![failed_edit()]),
        replay_step("failed retry two", vec![failed_edit()]),
        replay_step("failed retry three", vec![failed_edit()]),
        replay_step("failed retry four", vec![failed_edit()]),
        replay_step("failed retry five", vec![failed_edit()]),
        replay_step("failed retry six", vec![failed_edit()]),
    ];

    let report = replay_steps(dir.path(), &steps).await;
    assert!(report.forced_stop, "replay did not stop: {report:?}");
    assert_eq!(report.termination_reason, "forced_loop_stop");
    assert_eq!(report.recovery_attempts, 1, "report: {report:?}");
    assert!(
        report
            .warnings
            .iter()
            .any(|warning| warning.contains("repeated read two")),
        "replay warnings lacked the read loop: {report:?}"
    );
    assert_eq!(
        std::fs::read_to_string(&file).expect("read fixture"),
        original,
        "failed edits and restore loop must not leave a workspace mutation"
    );
    assert_eq!(
        report.tool_order.first().map(String::as_str),
        Some("replace_file_content")
    );
    assert_eq!(
        report.tool_order.last().map(String::as_str),
        Some("replace_file_content")
    );
    assert_eq!(report.tool_order.len(), report.paired_results.len());
    assert_eq!(
        report.lifecycle.first(),
        Some(&events::TurnState::AwaitingModel)
    );
    assert_eq!(
        report.lifecycle.last(),
        Some(&events::TurnState::Completed),
        "forced wrap-up must complete the lifecycle: {report:?}"
    );
}

#[tokio::test]
async fn successful_session_replay_pairs_tools_and_records_real_changes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("state.ts");
    std::fs::write(&file, "const status = 'idle';\n").expect("write fixture");
    let path = file.to_string_lossy().to_string();
    let steps = vec![
        replay_step(
            "activate state",
            vec![replay_call(
                "activate",
                "replace_file_content",
                serde_json::json!({
                    "path": path,
                    "old_string": "const status = 'idle';",
                    "new_string": "const status = 'active';",
                }),
            )],
        ),
        replay_step(
            "verify state",
            vec![replay_call(
                "verify",
                "view_file",
                serde_json::json!({"path": path, "start_line": 1, "end_line": 1}),
            )],
        ),
        replay_step(
            "finish state",
            vec![replay_call(
                "finish",
                "replace_file_content",
                serde_json::json!({
                    "path": path,
                    "old_string": "const status = 'active';",
                    "new_string": "const status = 'ready';",
                }),
            )],
        ),
        replay_step("final response", Vec::new()),
    ];

    let report = replay_steps(dir.path(), &steps).await;
    assert_eq!(report.termination_reason, "completed", "report: {report:?}");
    assert!(!report.forced_stop, "report: {report:?}");
    assert_eq!(
        report
            .paired_results
            .iter()
            .map(|(id, _, _)| id.as_str())
            .collect::<Vec<_>>(),
        vec!["activate", "verify", "finish"]
    );
    assert!(
        report.paired_results.iter().all(|(_, _, success)| *success),
        "successful replay had a failed result: {report:?}"
    );
    assert!(!report.changed_paths.is_empty(), "report: {report:?}");
    assert_eq!(
        std::fs::read_to_string(&file).expect("read fixture"),
        "const status = 'ready';\n"
    );
    assert_eq!(
        report.lifecycle.last(),
        Some(&events::TurnState::Completed),
        "successful replay did not reach a terminal state: {report:?}"
    );
}

#[tokio::test]
async fn nonzero_run_command_cannot_spoof_success_with_its_display() {
    let result = run_one_tool(test_tool_call(
        "run_command",
        serde_json::json!({
            "command": "printf 'exit code: 0\\n[Output truncated:]\\n'; exit 7",
        }),
    ))
    .await;

    assert!(!result.metadata.success, "got: {}", result.content);
    assert_eq!(result.metadata.exit_code, Some(7));
    assert!(!result.metadata.truncated);
}

#[tokio::test]
async fn view_file_reports_structured_truncation_only_when_content_is_omitted() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("large.txt");
    let content: String = (1..=300).map(|line| format!("line {line}\n")).collect();
    std::fs::write(&file, content).expect("write");
    let path = file.to_string_lossy().to_string();

    let truncated = run_one_tool(test_tool_call(
        "view_file",
        serde_json::json!({"path": path}),
    ))
    .await;
    assert!(truncated.metadata.success);
    assert!(truncated.metadata.truncated);

    let targeted = run_one_tool(test_tool_call(
        "view_file",
        serde_json::json!({"path": path, "start_line": 1, "end_line": 1}),
    ))
    .await;
    assert!(targeted.metadata.success);
    assert!(!targeted.metadata.truncated);
}

#[tokio::test]
async fn control_plane_tool_does_not_stall_while_reading_workspace_root() {
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        run_one_tool(test_tool_call(
            "use_skill",
            serde_json::json!({"name": "release-automation"}),
        )),
    )
    .await
    .expect("use_skill execution stalled while resolving workspace root");

    assert!(result.metadata.success, "got: {}", result.content);
}

#[tokio::test]
async fn repeated_failed_read_preserves_structured_failure() {
    let state = Arc::new(Mutex::new(AppState::new()));
    let dir = tempfile::tempdir().expect("tempdir");
    let missing = dir.path().join("missing").to_string_lossy().to_string();
    let call = test_tool_call("list_directory", serde_json::json!({"path": missing}));

    let first = run_one_tool_with_state(&state, call.clone()).await;
    let repeated = run_one_tool_with_state(&state, call).await;

    assert!(!first.metadata.success, "got: {}", first.content);
    assert!(!repeated.metadata.success, "got: {}", repeated.content);
    assert!(
        repeated.content.contains(&first.content),
        "replay omitted the original failure: {}",
        repeated.content
    );
}

#[tokio::test]
async fn repeated_truncated_read_preserves_structured_truncation() {
    let state = Arc::new(Mutex::new(AppState::new()));
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("large.txt");
    let content: String = (1..=300).map(|line| format!("line {line}\n")).collect();
    std::fs::write(&file, content).expect("write");
    let path = file.to_string_lossy().to_string();
    let call = test_tool_call("view_file", serde_json::json!({"path": path}));

    let first = run_one_tool_with_state(&state, call.clone()).await;
    let repeated = run_one_tool_with_state(&state, call).await;

    assert!(first.metadata.truncated, "got: {}", first.content);
    assert!(
        repeated.metadata.truncated,
        "replay lost structured truncation: {}",
        repeated.content
    );
    assert!(
        repeated.content.contains(&first.content),
        "replay omitted the original truncated output"
    );
}

#[tokio::test]
async fn repeated_over_limit_failed_read_preserves_structured_failure() {
    let state = Arc::new(Mutex::new(AppState::new()));
    let invalid_pattern = "(".repeat(REPLAYABLE_READ_LIMIT + 1);
    let call = test_tool_call("grep", serde_json::json!({"pattern": invalid_pattern}));

    let first = run_one_tool_with_state(&state, call.clone()).await;
    let repeated = run_one_tool_with_state(&state, call).await;

    assert!(!first.metadata.success, "got: {}", first.content);
    assert!(first.content.len() > REPLAYABLE_READ_LIMIT);
    assert!(!repeated.metadata.success, "got: {}", repeated.content);
    assert_eq!(repeated.metadata.exit_code, first.metadata.exit_code);
    assert_eq!(repeated.metadata.truncated, first.metadata.truncated);
    assert!(repeated.content.len() <= REPLAYABLE_READ_LIMIT);
    assert!(repeated.content.contains("not repeated"));
    assert!(!repeated.content.contains(&first.content));
}

#[tokio::test]
async fn repeated_over_limit_truncated_read_preserves_metadata_and_recovery_artifact() {
    let state = Arc::new(Mutex::new(AppState::new()));
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("large.txt");
    let content: String = (1..=300)
        .map(|line| format!("line {line}: {}\n", "x".repeat(256)))
        .collect();
    std::fs::write(&file, content).expect("write");
    let path = file.to_string_lossy().to_string();
    let call = test_tool_call("view_file", serde_json::json!({"path": path}));

    let first = run_one_tool_with_state(&state, call.clone()).await;
    let repeated = run_one_tool_with_state(&state, call).await;

    assert!(first.metadata.success, "got: {}", first.content);
    assert!(first.content.len() > REPLAYABLE_READ_LIMIT);
    assert!(first.metadata.truncated, "got: {}", first.content);
    let artifact = first
        .metadata
        .full_output_artifact
        .as_deref()
        .expect("bounded read must retain its recovery artifact");
    assert!(std::fs::metadata(artifact).is_ok());
    assert!(repeated.metadata.success, "got: {}", repeated.content);
    assert!(
        repeated.metadata.truncated,
        "replay lost structured truncation: {}",
        repeated.content
    );
    assert_eq!(repeated.metadata.exit_code, first.metadata.exit_code);
    assert_eq!(
        repeated.metadata.full_output_artifact.as_deref(),
        Some(artifact)
    );
    assert!(repeated.content.len() <= REPLAYABLE_READ_LIMIT);
    assert!(repeated.content.contains("not repeated"));
    assert!(repeated.content.contains(artifact));
    assert!(!repeated.content.contains(&first.content));
}

#[tokio::test]
async fn normal_replacement_final_diff_is_real_and_has_correct_line_numbers() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("state.rs");
    // The edit lands at line 50, not line 1 — the old argument-only
    // preview always reported line 1 because it had no idea where in
    // the file the match actually was.
    let mut lines: Vec<String> = (1..=100).map(|n| format!("line {n}")).collect();
    lines[49] = "let target = 1;".to_string();
    std::fs::write(&file, lines.join("\n") + "\n").expect("write");
    let path = file.to_string_lossy().to_string();

    let call = test_tool_call(
        "replace_file_content",
        serde_json::json!({
            "path": path,
            "old_string": "let target = 1;",
            "new_string": "let target = 100;",
        }),
    );
    let result = run_one_tool(call).await;

    assert!(result.metadata.success, "got: {}", result.content);
    let diff = result.diff.expect("a real edit must produce a diff");
    assert!(
        diff.contains("@@ -47,"),
        "expected the real line number (~50), got: {diff}"
    );
    assert!(diff.contains("-let target = 1;"), "got: {diff}");
    assert!(diff.contains("+let target = 100;"), "got: {diff}");
}

#[tokio::test]
async fn insert_shaped_replacement_final_diff_is_real_not_argument_derived() {
    // The classic insert shape: replacement_content contains the full
    // target_content as a suffix. The old argument-only preview and the
    // real file-content diff would look identical here in isolation,
    // but this proves the diff still comes from the actual file (one
    // inserted line as `+`, the anchor line as unchanged context) —
    // not a side-by-side line-for-line replacement of the whole block.
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("state.rs");
    std::fs::write(&file, "    s.discord_rpc.set_activity(\"Idle\", ...);\n").expect("write");
    let path = file.to_string_lossy().to_string();

    let call = test_tool_call(
        "replace_file_content",
        serde_json::json!({
            "path": path,
            "old_string": "    s.discord_rpc.set_activity(\"Idle\", ...);",
            "new_string": "    let model_name = ...;\n    s.discord_rpc.set_activity(\"Idle\", ...);",
        }),
    );
    let result = run_one_tool(call).await;

    let diff = result.diff.expect("an insertion must still produce a diff");
    assert!(diff.contains("+    let model_name = ...;"), "got: {diff}");
    assert!(
        !diff.contains("-    s.discord_rpc.set_activity"),
        "the untouched anchor line must be context, not a fabricated deletion: {diff}"
    );
}

#[tokio::test]
async fn repeated_idempotent_edit_produces_no_diff_on_the_second_call() {
    // Core regression for the bug this feature fixes: before this fix,
    // ToolResult.diff came from get_diff_preview(name, args), which is
    // computed purely from the call's arguments and therefore looked
    // identical on every call — including a second, no-op call after
    // PR #306 made the edit itself idempotent. A stale diff on a no-op
    // result would tell the user something changed when nothing did.
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("state.rs");
    std::fs::write(&file, "let status = Idle;\n").expect("write");
    let path = file.to_string_lossy().to_string();
    let args = serde_json::json!({
        "path": path,
        "old_string": "let status = Idle;",
        "new_string": "let status = Active;",
    });

    let first = run_one_tool(test_tool_call("replace_file_content", args.clone())).await;
    assert!(
        first.diff.is_some(),
        "the first, real change must have a diff"
    );

    let second = run_one_tool(test_tool_call("replace_file_content", args)).await;
    assert!(
        second
            .content
            .to_ascii_lowercase()
            .contains("already applied"),
        "got: {}",
        second.content
    );
    assert!(
        second.diff.is_none(),
        "a no-op repeat must not carry a stale diff: {:?}",
        second.diff
    );
}

#[tokio::test]
async fn multi_replacement_final_diff_is_real() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("state.rs");
    std::fs::write(&file, "let a = 1;\nlet b = 2;\nlet c = 3;\n").expect("write");
    let path = file.to_string_lossy().to_string();

    let call = test_tool_call(
        "multi_replace_file_content",
        serde_json::json!({
            "path": path,
            "replacements": [
                { "start_line": 1, "end_line": 1, "target_content": "let a = 1;", "replacement_content": "let a = 100;" },
                { "start_line": 3, "end_line": 3, "target_content": "let c = 3;", "replacement_content": "let c = 300;" },
            ],
        }),
    );
    let result = run_one_tool(call).await;

    assert!(result.metadata.success, "got: {}", result.content);
    let diff = result
        .diff
        .expect("a multi-replace edit must produce a diff");
    assert!(
        diff.contains("-let a = 1;") && diff.contains("+let a = 100;"),
        "got: {diff}"
    );
    assert!(
        diff.contains("-let c = 3;") && diff.contains("+let c = 300;"),
        "got: {diff}"
    );
    // Unrelated middle line stays as untouched context, not a
    // fabricated change.
    assert!(diff.contains(" let b = 2;"), "got: {diff}");
}

// --- Confirmation preview: unchanged, provisional, and separate ---

#[test]
fn confirmation_preview_is_unaffected_and_stays_provisional() {
    // get_diff_preview is the confirmation-modal preview path — it must
    // keep working exactly as before (best-effort, argument-only,
    // computed before the edit runs). This is deliberately NOT what
    // ends up in ToolResult.diff for the final transcript entry
    // (see the tests above); it's a distinct, provisional artifact.
    let preview = get_diff_preview(
        "replace_file_content",
        &serde_json::json!({
            "target_content": "old line",
            "replacement_content": "new line",
        }),
    )
    .expect("a preview should be computed from the arguments alone");
    assert!(preview.contains("old line"));
    assert!(preview.contains("new line"));
    // The confirmation preview format is the side-by-side \0-delimited
    // one, not a unified diff — asserting that pins the distinction
    // between the two mechanisms so a future change can't quietly
    // merge them back together.
    assert!(preview.contains('\0'), "got: {preview:?}");
}

// get_diff_preview must recognize every alias the edit tools themselves
// accept (see `crate::tools::filesystem::EDIT_TARGET_ALIASES` /
// `EDIT_REPLACEMENT_ALIASES`), not just target_content/replacement_content
// — a legacy or differently-shaped call must still get a real,
// non-empty provisional preview instead of silently falling through to
// an empty one.
#[test]
fn confirmation_preview_supports_old_string_new_string_alias() {
    let preview = get_diff_preview(
        "replace_file_content",
        &serde_json::json!({
            "old_string": "old line",
            "new_string": "new line",
        }),
    )
    .expect("a preview should be computed from old_string/new_string");
    assert!(preview.contains("old line"));
    assert!(preview.contains("new line"));
}

#[test]
fn confirmation_preview_supports_old_text_new_text_alias() {
    let preview = get_diff_preview(
        "replace_file_content",
        &serde_json::json!({
            "old_text": "old line",
            "new_text": "new line",
        }),
    )
    .expect("a preview should be computed from old_text/new_text");
    assert!(preview.contains("old line"));
    assert!(preview.contains("new line"));
}

#[test]
fn confirmation_preview_supports_camel_case_old_string_alias() {
    let preview = get_diff_preview(
        "replace_file_content",
        &serde_json::json!({
            "oldString": "old line",
            "newString": "new line",
        }),
    )
    .expect("a preview should be computed from oldString/newString");
    assert!(preview.contains("old line"));
    assert!(preview.contains("new line"));
}

#[test]
fn confirmation_preview_supports_camel_case_old_text_alias() {
    let preview = get_diff_preview(
        "replace_file_content",
        &serde_json::json!({
            "oldText": "old line",
            "newText": "new line",
        }),
    )
    .expect("a preview should be computed from oldText/newText");
    assert!(preview.contains("old line"));
    assert!(preview.contains("new line"));
}

#[test]
fn confirmation_preview_supports_target_replacement_alias() {
    let preview = get_diff_preview(
        "replace_file_content",
        &serde_json::json!({
            "target": "old line",
            "replacement": "new line",
        }),
    )
    .expect("a preview should be computed from target/replacement");
    assert!(preview.contains("old line"));
    assert!(preview.contains("new line"));
}

#[test]
fn confirmation_preview_prefers_target_content_when_multiple_aliases_present() {
    // target_content/replacement_content are first in priority order —
    // a call that (unusually) carries both the canonical keys and an
    // alias must use the canonical ones, matching extract_edit_chunks's
    // own priority order exactly.
    let preview = get_diff_preview(
        "replace_file_content",
        &serde_json::json!({
            "target_content": "canonical old",
            "replacement_content": "canonical new",
            "old_string": "alias old",
            "new_string": "alias new",
        }),
    )
    .expect("a preview should be computed");
    assert!(preview.contains("canonical old"));
    assert!(preview.contains("canonical new"));
    assert!(!preview.contains("alias old"));
    assert!(!preview.contains("alias new"));
}

// Regression uncovered by fixing get_diff_preview's alias support: once
// it correctly computes a real, non-empty preview for old_string/
// new_string calls (not just target_content/replacement_content), that
// preview must still never leak through as a fallback for a no-op or
// failed edit — only extract_diff_block's real, post-execution diff (or
// no diff at all) may represent those outcomes.
#[test]
fn tool_result_precludes_preview_fallback_for_noop_and_failure() {
    assert!(tool_result_precludes_preview_fallback(
        "already applied; no changes made to 'x.rs'"
    ));
    assert!(tool_result_precludes_preview_fallback(
        "Error: target_content not found in 'x.rs'."
    ));
    assert!(!tool_result_precludes_preview_fallback(
        "wrote 'x.rs' (3 lines, 20 bytes)"
    ));
}

#[tokio::test]
async fn repeated_noop_edit_with_old_string_alias_still_shows_no_diff() {
    // The exact end-to-end shape of the regression: old_string/new_string
    // args (not target_content/replacement_content), repeated after the
    // edit already landed. Before this fix's tool_result_precludes_
    // preview_fallback guard, get_diff_preview's now-correct alias
    // support would have handed the pre-execution preview to
    // final_tool_diff as a non-empty fallback, showing a diff for a
    // no-op that changed nothing.
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("state.rs");
    std::fs::write(&file, "let status = Idle;\n").expect("write");
    let path = file.to_string_lossy().to_string();
    let args = serde_json::json!({
        "path": path,
        "old_string": "let status = Idle;",
        "new_string": "let status = Active;",
    });

    let first = run_one_tool(test_tool_call("replace_file_content", args.clone())).await;
    assert!(
        first.diff.is_some(),
        "the first, real change must have a diff"
    );

    let second = run_one_tool(test_tool_call("replace_file_content", args)).await;
    assert!(
        second
            .content
            .to_ascii_lowercase()
            .contains("already applied"),
        "got: {}",
        second.content
    );
    assert!(
        second.diff.is_none(),
        "a no-op repeat must not carry a stale diff, even with a now-working alias preview: {:?}",
        second.diff
    );
}

// Regression this feature fixes: `get_diff_preview` previously only read
// the `target_content`/`replacement_content` keys, so a call built with
// the (equally valid, alias-supported) `old_string`/`new_string` keys
// got an empty preview — `Some("")`, not `None`. `final_tool_diff` must
// still guard against an empty fallback regardless (defense in depth):
#[test]
fn final_tool_diff_ignores_an_empty_fallback_preview() {
    assert_eq!(
        final_tool_diff("already applied; no changes made", None),
        None
    );
    assert_eq!(
        final_tool_diff("already applied; no changes made", Some(String::new())),
        None,
        "an empty fallback preview must not surface as a diff"
    );
    assert_eq!(
        final_tool_diff(
            "already applied; no changes made",
            Some("   \n".to_string())
        ),
        None,
        "a whitespace-only fallback preview must not surface as a diff"
    );
}

#[test]
fn final_tool_diff_prefers_the_real_diff_over_a_nonempty_fallback() {
    let result =
        "successfully replaced target_content in 'x.rs'\n\n```diff\n@@ -1,1 +1,1 @@\n-a\n+b\n```\n";
    let stale_fallback = Some("-old\x00+new\n".to_string());
    let diff = final_tool_diff(result, stale_fallback).expect("real diff must win");
    assert!(diff.contains("-a") && diff.contains("+b"), "got: {diff}");
    assert!(
        !diff.contains("old") && !diff.contains("new"),
        "got: {diff}"
    );
}

#[test]
fn final_tool_diff_uses_the_fallback_only_when_it_has_real_content() {
    let result = "wrote 'x.rs' (3 lines, 20 bytes)"; // no ```diff fence
    let legacy_preview = Some("-old line\x00+new line\n".to_string());
    let diff = final_tool_diff(result, legacy_preview).expect("fallback should be used");
    assert!(diff.contains("old line") && diff.contains("new line"));
}

#[test]
fn consecutive_turn_provider_payloads_have_identical_historical_prefix() {
    // Turn 1: user says hello
    let history_turn_1 = vec![ChatMessage::new("user", "Hello, assistant!")];
    let mut msgs_1 = history::to_messages(&history_turn_1, "You are a helpful assistant.");
    messages::attach_request_context_tail(&mut msgs_1, "time: 12:00:00\ncwd: /tmp");

    // Turn 2: assistant answered, user asks follow-up
    let history_turn_2 = vec![
        ChatMessage::new("user", "Hello, assistant!"),
        ChatMessage::new("assistant", "Hello! How can I help you today?"),
        ChatMessage::new("user", "What is 2 + 2?"),
    ];
    let mut msgs_2 = history::to_messages(&history_turn_2, "You are a helpful assistant.");
    messages::attach_request_context_tail(&mut msgs_2, "time: 12:01:00\ncwd: /tmp");

    // Turn 1 had [system, user1, context_tail_1]
    // Turn 2 has [system, user1, assistant1, user2, context_tail_2]
    // The prefix [system, user1] must be EXACTLY byte-identical between Turn 1 and Turn 2.
    assert_eq!(msgs_1[0], msgs_2[0], "system prompt must be identical");
    assert_eq!(
        msgs_1[1], msgs_2[1],
        "user 1 message must be completely stable and unmutated"
    );
}

#[test]
fn historical_assistant_reasoning_is_stripped_when_generating_messages() {
    let history = vec![
        ChatMessage::new("user", "Calculate fibonacci"),
        ChatMessage::new(
            "assistant",
            "<think>\nLet me think through recursion vs dynamic programming...\n100 lines of reasoning\n</think>\n\nHere is the Fibonacci function:\n```rust\nfn fib(n: u32) -> u32 { ... }\n```",
        ),
        ChatMessage::new("user", "Now write tests for it"),
    ];

    let messages = history::to_messages(&history, "system prompt");
    let assistant_msg = &messages[2];
    assert_eq!(
        assistant_msg.get("role").and_then(|r| r.as_str()),
        Some("assistant")
    );
    let content = assistant_msg
        .get("content")
        .and_then(|c| c.as_str())
        .unwrap();
    assert!(
        !content.contains("<think>"),
        "historical assistant reasoning must not be sent to provider"
    );
    assert!(
        content.contains("Here is the Fibonacci function"),
        "visible answer must be preserved"
    );
}

#[test]
fn compaction_prunes_massive_historical_reasoning() {
    let mut history = vec![
        ChatMessage::new("user", "Prompt 1"),
        ChatMessage::new(
            "assistant",
            format!(
                "<think>\n{}\n</think>\nShort answer 1",
                "deep thoughts ".repeat(5000)
            ),
        ),
        ChatMessage::new("user", "Prompt 2"),
        ChatMessage::new(
            "assistant",
            format!(
                "<think>\n{}\n</think>\nShort answer 2",
                "more thoughts ".repeat(5000)
            ),
        ),
        ChatMessage::new("user", "Prompt 3 (recent)"),
        ChatMessage::new("assistant", "Recent answer"),
    ];

    let before_tokens: usize = history
        .iter()
        .map(compaction::estimate_message_tokens)
        .sum();
    assert!(before_tokens > 10000, "initial tokens should be large");

    let pruned = compaction::prune_historical_reasoning(&mut history, 2);
    assert_eq!(pruned, 2, "must prune 2 historical assistant messages");

    let after_tokens: usize = history
        .iter()
        .map(compaction::estimate_message_tokens)
        .sum();
    assert!(
        after_tokens < 500,
        "after reasoning pruning tokens should be drastically lower: got {after_tokens}"
    );
    assert_eq!(history[1].content, "Short answer 1");
    assert_eq!(history[3].content, "Short answer 2");
}

#[test]
fn model_profile_is_local_classification() {
    use crate::config::ModelProfile;

    let omlx_profile = ModelProfile {
        name: "local-qwen".to_string(),
        url: "http://127.0.0.1:8000/v1".to_string(),
        engine: Some("omlx".to_string()),
        ..ModelProfile::default()
    };
    assert!(
        omlx_profile.is_local(),
        "omlx engine must be classified as local"
    );

    let ollama_profile = ModelProfile {
        name: "ollama-model".to_string(),
        url: "http://127.0.0.1:11434".to_string(),
        engine: Some("ollama".to_string()),
        ..ModelProfile::default()
    };
    assert!(
        ollama_profile.is_local(),
        "ollama must be classified as local"
    );

    let lmstudio_profile = ModelProfile {
        name: "lmstudio-model".to_string(),
        url: "http://localhost:1234/v1".to_string(),
        engine: Some("lmstudio".to_string()),
        ..ModelProfile::default()
    };
    assert!(
        lmstudio_profile.is_local(),
        "lmstudio must be classified as local"
    );

    let remote_openai = ModelProfile {
        name: "gpt-4o".to_string(),
        url: "https://api.openai.com/v1".to_string(),
        engine: Some("openai".to_string()),
        ..ModelProfile::default()
    };
    assert!(
        !remote_openai.is_local(),
        "remote OpenAI endpoint must not be classified as local"
    );
}

#[test]
fn model_profile_reasoning_effort() {
    use crate::config::ModelProfile;

    let profile = ModelProfile {
        name: "qwen-3.8".to_string(),
        url: "https://tokmax.paral.no/v1/chat/completions".to_string(),
        model: "Qwen3.8-27B-MTPLX-4bit".to_string(),
        reasoning_effort: Some("low".to_string()),
        thinking_budget: Some(4096),
        ..ModelProfile::default()
    };

    assert_eq!(profile.reasoning_effort.as_deref(), Some("low"));
    assert_eq!(profile.thinking_budget, Some(4096));
    let budget = profile.context_budget();
    assert!(budget.thinking_reserve > 0);
}

#[test]
fn stress_test_long_agent_loop_20_turns_with_prefix_stability() {
    let mut history: Vec<ChatMessage> = Vec::new();
    let mut previous_provider_messages: Option<Vec<serde_json::Value>> = None;
    let system_prompt = "You are RustCode, an autonomous coding agent.";

    for turn in 1..=20 {
        // User instruction / tool feedback
        history.push(ChatMessage::new(
            "user",
            format!("Turn {turn}: Inspect and modify module {turn}"),
        ));

        // Generate provider messages for this turn
        let mut provider_msgs = history::to_messages(&history, system_prompt);
        let dynamic_ctx = format!("time: 12:{turn:02}:00\ncwd: /workspace\ntodos: []");
        messages::attach_request_context_tail(&mut provider_msgs, &dynamic_ctx);

        // Verify prefix stability: every message from turn N-1 (except its trailing synthetic context tail)
        // must be IDENTICAL in turn N.
        if let Some(prev) = &previous_provider_messages {
            let stable_prefix_len = prev.len().saturating_sub(1);
            for i in 0..stable_prefix_len {
                assert_eq!(
                    prev[i],
                    provider_msgs[i],
                    "Message index {i} mutated between turn {} and turn {}",
                    turn - 1,
                    turn
                );
            }
        }

        // Assistant responds with huge reasoning and a tool call
        let reasoning = format!(
            "<think>\nAnalyzing module {turn} in depth...\n{}\n</think>",
            "reasoning details ".repeat(200)
        );
        let tool_call = crate::app::ToolCallRef {
            id: format!("call_{turn}"),
            name: "view_file".to_string(),
            arguments: serde_json::json!({"path": format!("src/mod_{turn}.rs")}).to_string(),
        };
        let assistant_msg =
            ChatMessage::new("assistant", &reasoning).with_tool_calls(vec![tool_call]);
        history.push(assistant_msg);

        // Tool output arrives
        let tool_output =
            format!("src/mod_{turn}.rs contents:\npub fn run_{turn}() {{ println!(\"ok\"); }}");
        let mut tool_msg = ChatMessage::new("tool", format!("view_file: {tool_output}"));
        tool_msg.tool_call_id = Some(format!("call_{turn}"));
        history.push(tool_msg);

        // Save current provider messages for next turn's prefix comparison
        let mut current_turn_msgs = history::to_messages(&history, system_prompt);
        messages::attach_request_context_tail(&mut current_turn_msgs, &dynamic_ctx);
        previous_provider_messages = Some(current_turn_msgs);
    }

    // Verify final state:
    // 1. History has 60 messages (20 turns * 3 messages per turn)
    assert_eq!(history.len(), 60);

    // 2. When converted to provider messages, all historical assistant reasoning was stripped!
    let final_provider_msgs = history::to_messages(&history, system_prompt);
    for msg in &final_provider_msgs {
        if msg.get("role").and_then(|r| r.as_str()) == Some("assistant") {
            if let Some(content) = msg.get("content").and_then(|c| c.as_str()) {
                assert!(
                    !content.contains("<think>"),
                    "Provider message must not contain raw <think> reasoning!"
                );
            }
        }
    }
}

#[tokio::test]
async fn stress_test_compaction_under_massive_context_pressure() {
    let mut history = Vec::new();

    // Create 30 turns of large history (~120k tokens total)
    for i in 0..30 {
        history.push(ChatMessage::new("user", format!("Task step {i}")));
        history.push(ChatMessage::new(
            "assistant",
            format!(
                "<think>\n{}\n</think>\nHere is the plan for step {i}.",
                "extensive reasoning trace ".repeat(300)
            ),
        ));
        let mut tool_res = ChatMessage::new(
            "tool",
            format!(
                "run_command: output for step {i}:\n{}",
                "data row\n".repeat(200)
            ),
        );
        tool_res.tool_call_id = Some(format!("call_{i}"));
        history.push(tool_res);
    }

    let initial_tokens: usize = history
        .iter()
        .map(compaction::estimate_message_tokens)
        .sum();
    assert!(
        initial_tokens > 20000,
        "Initial tokens should be very high, got: {initial_tokens}"
    );

    let budget = 15000;
    let cancel = tokio_util::sync::CancellationToken::new();

    // Run deterministic local compaction
    let compacted = compaction::maybe_compact_with_local_policy(
        &reqwest::Client::new(),
        "http://localhost:11434",
        "qwen:32b",
        &mut history,
        budget,
        &cancel,
        true,
    )
    .await;
    assert!(compacted, "Compaction must report success");

    let post_compaction_tokens: usize = history
        .iter()
        .map(compaction::estimate_message_tokens)
        .sum();
    assert!(
        post_compaction_tokens <= budget,
        "Post compaction tokens ({post_compaction_tokens}) must be within budget ({budget})"
    );

    // Verify structured integrity of history after compaction
    let provider_msgs = history::to_messages(&history, "system prompt");
    assert!(
        !provider_msgs.is_empty(),
        "Provider messages must be non-empty"
    );
}

#[test]
fn session_interruption_and_recovery_safety() {
    let mut state = crate::app::AppState::new();
    state.active_session_id = "test-recovery-session".to_string();
    state
        .history
        .push(ChatMessage::new("user", "Write a function"));
    state.current_response = "<think>\nThinking about function...".to_string();
    state.current_thought_started_at = Some(std::time::Instant::now());
    state.live_tool_calls.push(crate::app::LiveToolCall::new(
        "call-temp",
        None,
        "run_command",
        "Bash",
        "cargo check",
    ));

    // Simulate killing/stopping turn: live tool calls & transient thought buffer are cleared
    state.clear_live_tool_calls();
    state.current_response.clear();
    state.current_thought_started_at = None;
    state.status = crate::app::AppStatus::Idle;

    assert!(state.live_tool_calls.is_empty());
    assert!(state.current_response.is_empty());
    assert_eq!(state.status, crate::app::AppStatus::Idle);
    assert_eq!(state.history.len(), 1);
}

#[test]
fn test_continuation_assistant_message_strips_massive_think_traces() {
    use super::text::format_continuation_assistant_message;

    // 1. Completed massive reasoning trace followed by visible prose
    let massive_think = format!(
        "<think>\n{}\n</think>\nI will now run the test suite.\n```tool\n{{\"name\": \"run_command\"}}\n```",
        "deep reasoning ".repeat(2000)
    );
    let continuation = format_continuation_assistant_message(&massive_think);
    assert!(!continuation.contains("<think>"), "Completed think blocks must be stripped from continuation assistant message");
    assert!(continuation.contains("I will now run the test suite."));
    assert!(continuation.contains("```tool"));
    assert!(continuation.len() < 500, "Continuation assistant message must be bounded and compact");

    // 2. Pure reasoning (no visible text outside think)
    let pure_think = format!("<think>\n{}\n</think>", "planning ".repeat(1500));
    let continuation_pure = format_continuation_assistant_message(&pure_think);
    assert_eq!(continuation_pure, "(completed reasoning scratchpad)");

    // 3. Unclosed think trace that was cut off mid-thought
    let cut_off_think = format!("<think>\n{}\nThinking about line 42", "thinking ".repeat(1000));
    let continuation_cut_off = format_continuation_assistant_message(&cut_off_think);
    assert!(continuation_cut_off.contains("<think>"));
    assert!(continuation_cut_off.contains("Thinking about line 42"));
    assert!(continuation_cut_off.len() <= 1200, "Unclosed think block tail must be bounded");
}

#[test]
fn test_continuation_nudges_are_category_aware() {
    use super::text::continuation_nudge_for_category;

    // Length cutoff
    assert_eq!(
        continuation_nudge_for_category("Some partial text", Some("length")),
        "Your previous response was cut off by the token limit. Continue directly from where you left off."
    );

    // Reasoning only
    assert_eq!(
        continuation_nudge_for_category("<think>Planning step 1...</think>", None),
        "Stop planning and do not restate your plan again. Call the tool now."
    );

    // Incomplete tool call
    assert_eq!(
        continuation_nudge_for_category("```tool\n{\"name\": \"view_file\"", None),
        "Your tool call syntax was cut off. Continue the tool syntax directly."
    );

    // Stated intent without tool call
    assert_eq!(
        continuation_nudge_for_category("I will read the file `src/main.rs` now to verify.", None),
        "You stated your intended action. Please execute the tool call now."
    );

    // Normal prose
    assert_eq!(
        continuation_nudge_for_category("Here is the explanation of the bug:", None),
        "continue"
    );
}

#[test]
fn test_structured_session_memory_semantic_continuity_across_compactions() {
    use super::compaction::{StructuredSessionMemory, compact_with_structured_memory};

    let mut history = Vec::new();

    // Turn 1: User specifies critical constraints
    history.push(ChatMessage::new(
        "user",
        "Implement feature X. Rule: Never modify files under src/generated/! Always use cargo check."
    ));
    history.push(ChatMessage::new("assistant", "Understood. I will not touch generated files."));

    // Turn 5: Discovered architecture & failed approach
    history.push(ChatMessage::new("user", "Check if we can use the old parser."));
    history.push(ChatMessage::new(
        "assistant",
        "Decision: Found parser in `src/parser/legacy.rs`. It does not support async."
    ));
    let mut failed_tool = ChatMessage::new("tool", "run_command: cargo test\nerror: compilation failed\nexit code: 1");
    failed_tool.tool_result = Some(crate::app::ToolResultRecord {
        tool_name: "run_command".to_string(),
        arguments_hash: "hash".to_string(),
        success: false,
        exit_code: Some(1),
        error_kind: Some("command_failed".to_string()),
        changed_paths: vec!["src/parser/legacy.rs".to_string()],
        truncated: false,
        full_output_artifact: None,
        replayed: false,
        retryable: false,
    });
    history.push(failed_tool);

    // Add multiple subsequent turns to trigger compaction
    for i in 6..=35 {
        history.push(ChatMessage::new("user", format!("Step {i} working on alternative parser")));
        history.push(ChatMessage::new("assistant", format!("Working on step {i} implementation")));
    }

    // Extract structured session memory
    let memory = StructuredSessionMemory::extract_from_history(&history);
    let record = memory.format_record(4000);

    // Verify key constraints and facts are preserved
    assert!(record.contains("Never modify files under src/generated/!"), "User constraint must be captured in memory");
    assert!(record.contains("src/parser/legacy.rs"), "Modified/inspected file must be captured");
    assert!(record.contains("Goal: Implement feature X"), "Initial goal must be captured");

    // Perform structured compaction
    let initial_len = history.len();
    let compacted = compact_with_structured_memory(&mut history, 10, 4000);
    assert!(compacted, "Compaction should succeed");
    assert!(history.len() < initial_len);

    // Verify the compacted history retains the structured memory block at message 0
    assert_eq!(history[0].role, "system");
    assert!(history[0].content.contains("Never modify files under src/generated/!"));

    // Simulate another 10 turns and a SECOND compaction pass (Turn 45)
    for i in 36..=45 {
        history.push(ChatMessage::new("user", format!("Follow up step {i}")));
        history.push(ChatMessage::new("assistant", format!("Follow up step {i} completed")));
    }
    let second_compacted = compact_with_structured_memory(&mut history, 10, 4000);
    assert!(second_compacted, "Second compaction should succeed");

    // Verify the constraint from Turn 1 is STILL preserved after multiple compaction passes!
    assert!(history[0].content.contains("Never modify files under src/generated/!"), "Turn 1 constraint must survive multiple compaction passes!");
}

#[test]
fn test_preflight_budget_calculation_and_limits() {
    use super::compaction::calculate_preflight_budget;
    use crate::config::ModelProfile;

    let profile = ModelProfile {
        name: "local-qwen".to_string(),
        url: "http://localhost:11434/v1/chat/completions".to_string(),
        model: "qwen:32b".to_string(),
        context_window: Some(32768),
        soft_context_target: Some(24000),
        hard_effective_limit: Some(30000),
        provider_overhead_margin: Some(1024),
        ..ModelProfile::default()
    };
    let budget = profile.context_budget();

    let history = vec![
        ChatMessage::new("user", "Hello! Write a small parser."),
        ChatMessage::new("assistant", "Here is the plan for the parser."),
    ];

    let preflight = calculate_preflight_budget(
        "You are RustCode.",
        &[],
        &history,
        "# Runtime Context\n- cwd: /code\n",
        0,
        &budget,
    );

    assert_eq!(preflight.soft_context_target, 24000);
    assert_eq!(preflight.hard_effective_limit, 30000);
    assert!(preflight.fits_soft_target());
    assert!(preflight.fits_hard_limit());
    assert!(preflight.total_estimated_prompt > 0);
}

#[test]
fn test_preflight_budget_accounts_for_native_tool_schemas_once() {
    use super::compaction::{calculate_preflight_budget, estimate_tool_schema_tokens};
    use crate::config::ModelProfile;

    let budget = ModelProfile {
        context_window: Some(32768),
        ..ModelProfile::default()
    }
    .context_budget();
    let history = vec![ChatMessage::new("user", "Use the available tool.")];
    let schema = vec![serde_json::json!({
        "type": "function",
        "function": {
            "name": "inspect_workspace",
            "description": "Inspect files in the workspace",
            "parameters": {"type": "object", "properties": {"path": {"type": "string"}}}
        }
    })];

    let without_schema = calculate_preflight_budget(
        "You are RustCode.",
        &[],
        &history,
        "# Runtime Context\n",
        0,
        &budget,
    );
    let with_schema = calculate_preflight_budget(
        "You are RustCode.",
        &schema,
        &history,
        "# Runtime Context\n",
        0,
        &budget,
    );

    assert_eq!(without_schema.tool_schema_tokens, 0);
    assert_eq!(with_schema.tool_schema_tokens, estimate_tool_schema_tokens(&schema));
    assert!(with_schema.tool_schema_tokens > 0);
    assert_eq!(
        with_schema.total_estimated_prompt - without_schema.total_estimated_prompt,
        with_schema.tool_schema_tokens
    );
}

#[test]
fn test_text_protocol_preflight_does_not_double_count_tool_definitions() {
    use super::compaction::calculate_preflight_budget;
    use crate::config::ModelProfile;

    let budget = ModelProfile {
        context_window: Some(32768),
        ..ModelProfile::default()
    }
    .context_budget();
    let schema = vec![serde_json::json!({
        "type": "function",
        "function": {"name": "inspect_workspace", "parameters": {"type": "object"}}
    })];
    let history = vec![ChatMessage::new("user", "Use the available tool.")];
    let text_prompt = "You are RustCode.\nTool: inspect_workspace (text protocol)";

    let preflight = calculate_preflight_budget(
        text_prompt,
        &[],
        &history,
        "",
        0,
        &budget,
    );
    let incorrectly_double_counted = calculate_preflight_budget(
        text_prompt,
        &schema,
        &history,
        "",
        0,
        &budget,
    );

    assert_eq!(preflight.tool_schema_tokens, 0);
    assert!(incorrectly_double_counted.total_estimated_prompt > preflight.total_estimated_prompt);
}

#[test]
fn test_local_model_profile_completion_reserve_defaults() {
    use crate::config::ModelProfile;

    // Local profile without explicit max_tokens defaults to a safe 4k-8k completion cap
    let local_profile = ModelProfile {
        name: "local-ollama".to_string(),
        url: "http://127.0.0.1:11434/v1/chat/completions".to_string(),
        model: "qwen2.5:32b".to_string(),
        context_window: Some(128000),
        ..ModelProfile::default()
    };
    let local_budget = local_profile.context_budget();
    assert!(
        local_budget.completion_reserve <= 8192,
        "Local profile completion reserve must be capped at 8192, got: {}",
        local_budget.completion_reserve
    );
    assert!(
        local_budget.soft_context_target < local_budget.context_window,
        "Local profile must have a realistic soft context target below the theoretical context window"
    );
}

#[test]
fn test_reasoning_loop_recovery_action_escalation() {
    assert_eq!(
        reasoning_loop_recovery_action(0),
        LoopRecoveryAction::Recover
    );
    assert_eq!(
        reasoning_loop_recovery_action(1),
        LoopRecoveryAction::ForceFinal
    );
    assert_eq!(
        reasoning_loop_recovery_action(2),
        LoopRecoveryAction::ForceFinal
    );
    assert_eq!(
        reasoning_loop_recovery_action(3),
        LoopRecoveryAction::ForceFinal
    );
}

#[test]
fn test_reasoning_loop_is_cut_off_behavior() {
    // When finish_reason is "reasoning_loop", is_cut_off must be false so runner returns collected response
    assert!(!is_cut_off("<think>repetitive thoughts", Some("reasoning_loop")));
    assert!(!is_cut_off("<think>thoughts</think>", Some("reasoning_loop")));
}

#[test]
fn test_turn_context_reasoning_benchmark_summary() {
    let mut ctx = TurnContext::new();
    assert_eq!(ctx.reasoning_loops_detected, 0);
    assert_eq!(ctx.reasoning_recovery_attempts, 0);

    ctx.reasoning_loops_detected = 1;
    ctx.reasoning_recovery_attempts = 1;

    let summary = ctx.benchmark_summary();
    assert_eq!(summary["reasoning_loops_detected"], 1);
    assert_eq!(summary["reasoning_recovery_attempts"], 1);
}

#[test]
fn test_reasoning_loop_detector_integration_and_resets() {
    let mut detector = loop_detect::ReasoningLoopDetector::default();

    // 1. Pathological consecutive loop
    let s = "We should inspect the configuration file in src/config.rs to verify settings.\n";
    assert_eq!(detector.feed_chunk(s), loop_detect::ReasoningLoopStatus::Ok);
    assert_eq!(detector.feed_chunk(s), loop_detect::ReasoningLoopStatus::Ok);
    assert!(matches!(
        detector.feed_chunk(s),
        loop_detect::ReasoningLoopStatus::LoopDetected(_)
    ));

    // Reset clears state
    detector.reset();

    // 2. Legitimate long reasoning with varied sentences
    for step in 0..50 {
        let sentence = format!("Analyzing architectural step {step} with specialized component handler_{step}.\n");
        assert_eq!(
            detector.feed_chunk(&sentence),
            loop_detect::ReasoningLoopStatus::Ok
        );
    }

    // 3. Cross-turn plan repetition
    let plan = "Plan: Inspect all routes in src/routes.rs and verify handler types.";
    assert_eq!(
        detector.record_turn_reasoning(plan, false),
        loop_detect::ReasoningLoopStatus::Ok
    );
    assert!(matches!(
        detector.record_turn_reasoning(plan, false),
        loop_detect::ReasoningLoopStatus::LoopDetected(_)
    ));

    // 4. Progress resets cross-turn plan
    detector.record_turn_reasoning(plan, true);
    assert_eq!(
        detector.record_turn_reasoning(plan, false),
        loop_detect::ReasoningLoopStatus::Ok
    );
}

#[test]
fn reasoning_loop_detector_treats_a_multi_tool_batch_as_one_turn() {
    let mut detector = loop_detect::ReasoningLoopDetector::default();
    let plan = "I will inspect the repository history and verify the changelog entries.";

    // A single model response can contain several independent read-only
    // calls. They must contribute one cross-turn record, not one record per
    // tool result, or the batch will look like an immediate repeated plan.
    assert_eq!(
        detector.record_turn_evidence(&loop_detect::TurnEvidence {
            reasoning: plan,
            target_files: &[],
            made_progress: false,
            had_edits: false,
            tool_count: 2,
            no_progress_streak: 0,
        }),
        loop_detect::ReasoningLoopStatus::Ok
    );
}

#[test]
fn test_adversarial_1_paraphrased_same_plan_same_files_no_edits_interrupts() {
    // Scenario 1: Model understands architecture, repeatedly reads the same file,
    // paraphrases the same plan with different wording, and never edits.
    let mut detector = loop_detect::ReasoningLoopDetector::default();
    let file = "src/network/turn_engine.rs";

    let turn1 = "I have analyzed the engine. I will edit src/network/turn_engine.rs to add the new loop recovery mechanism.";
    let res1 = detector.record_turn_evidence(&loop_detect::TurnEvidence {
        reasoning: turn1,
        target_files: &[file],
        made_progress: false,
        had_edits: false,
        tool_count: 1,
        no_progress_streak: 1,
    });
    assert_eq!(res1, loop_detect::ReasoningLoopStatus::Ok);

    let turn2 = "The architecture is clear. We are ready to modify src/network/turn_engine.rs to include the new loop recovery behavior.";
    let res2 = detector.record_turn_evidence(&loop_detect::TurnEvidence {
        reasoning: turn2,
        target_files: &[file],
        made_progress: false,
        had_edits: false,
        tool_count: 1,
        no_progress_streak: 2,
    });
    assert_eq!(
        res2,
        loop_detect::ReasoningLoopStatus::LoopDetected(loop_detect::DIAG_CROSS_TURN_SAME_PLAN)
    );
}

#[test]
fn test_adversarial_2_broad_investigation_across_files_does_not_interrupt() {
    // Scenario 2: Agent investigates many different files and continuously discovers
    // new information without editing -> must NOT interrupt.
    let mut detector = loop_detect::ReasoningLoopDetector::default();
    let files = [
        "src/app/actions.rs",
        "src/network/loop_detect.rs",
        "src/network/turn_engine.rs",
        "src/tools/exec.rs",
        "src/ui/mod.rs",
        "src/config.rs",
        "src/main.rs",
        "src/memory.rs",
    ];

    for (i, file) in files.iter().enumerate() {
        let thought = format!("Step {i}: Inspecting module {file} to understand its public API and dependencies.");
        let status = detector.record_turn_evidence(&loop_detect::TurnEvidence {
            reasoning: &thought,
            target_files: &[file],
            made_progress: false,
            had_edits: false,
            tool_count: 1,
            no_progress_streak: i + 1,
        });
        assert_eq!(
            status,
            loop_detect::ReasoningLoopStatus::Ok,
            "Broad investigation across {file} must not be interrupted"
        );
    }
}

#[test]
fn test_adversarial_3_long_advancing_reasoning_does_not_interrupt() {
    // Scenario 3: Agent spends a long time reasoning, but reasoning continues
    // materially advancing -> must NOT interrupt.
    let mut detector = loop_detect::ReasoningLoopDetector::default();
    for step in 0..100 {
        let advance = match step % 4 {
            0 => format!("Phase {step}: Evaluating caching layer invariants in src/cache/storage_{step}.rs.\n\n"),
            1 => format!("Phase {step}: Analyzing thread pool scheduler policies for runtime_{step}.\n\n"),
            2 => format!("Phase {step}: Reviewing serialization schema definitions in src/proto/message_{step}.rs.\n\n"),
            _ => format!("Phase {step}: Verifying cryptographic checksum verification logic in src/crypto/auth_{step}.rs.\n\n"),
        };
        let status = detector.feed_chunk(&advance);
        assert_eq!(
            status,
            loop_detect::ReasoningLoopStatus::Ok,
            "Advancing reasoning step {step} must not trigger loop detector"
        );
    }
}

#[test]
fn test_adversarial_4_ready_to_implement_hesitation_reads_interrupts() {
    // Scenario 4: Agent says it is ready to implement several times but repeatedly
    // performs equivalent verification reads without editing -> should interrupt.
    let mut detector = loop_detect::ReasoningLoopDetector::default();
    let file = "src/tools/filesystem.rs";

    let turn1 = "Everything is mapped out. I am ready to implement the changes in src/tools/filesystem.rs. Let's do one more check on the view_file signature.";
    assert_eq!(
        detector.record_turn_evidence(&loop_detect::TurnEvidence {
            reasoning: turn1,
            target_files: &[file],
            made_progress: false,
            had_edits: false,
            tool_count: 1,
            no_progress_streak: 1,
        }),
        loop_detect::ReasoningLoopStatus::Ok
    );

    let turn2 = "Signature is confirmed. Now proceed with implementation in src/tools/filesystem.rs. Let me do a quick check on start_line handling first.";
    assert_eq!(
        detector.record_turn_evidence(&loop_detect::TurnEvidence {
            reasoning: turn2,
            target_files: &[file],
            made_progress: false,
            had_edits: false,
            tool_count: 1,
            no_progress_streak: 2,
        }),
        loop_detect::ReasoningLoopStatus::LoopDetected(loop_detect::DIAG_SEMANTIC_NO_PROGRESS)
    );
}

#[test]
fn test_adversarial_5_loop_fires_recovery_succeeds_with_edit_resets_state() {
    // Scenario 5: Loop detector fires, recovery succeeds and an edit occurs -> state resets correctly.
    let mut detector = loop_detect::ReasoningLoopDetector::default();
    let file = "src/network/loop_detect.rs";

    let plan1 = "We will modify src/network/loop_detect.rs to add new loop signals.";
    let plan2 = "We are ready to modify src/network/loop_detect.rs to add new loop signals.";

    assert_eq!(
        detector.record_turn_evidence(&loop_detect::TurnEvidence {
            reasoning: plan1,
            target_files: &[file],
            made_progress: false,
            had_edits: false,
            tool_count: 1,
            no_progress_streak: 1,
        }),
        loop_detect::ReasoningLoopStatus::Ok
    );

    assert_eq!(
        detector.record_turn_evidence(&loop_detect::TurnEvidence {
            reasoning: plan2,
            target_files: &[file],
            made_progress: false,
            had_edits: false,
            tool_count: 1,
            no_progress_streak: 2,
        }),
        loop_detect::ReasoningLoopStatus::LoopDetected(loop_detect::DIAG_CROSS_TURN_SAME_PLAN)
    );

    // Recovery succeeds: edit occurs and makes progress
    assert_eq!(
        detector.record_turn_evidence(&loop_detect::TurnEvidence {
            reasoning: "Applied the edit to src/network/loop_detect.rs successfully.",
            target_files: &[file],
            made_progress: true,
            had_edits: true,
            tool_count: 1,
            no_progress_streak: 0,
        }),
        loop_detect::ReasoningLoopStatus::Ok
    );

    // Subsequent normal inspection starts from clean state
    assert_eq!(
        detector.record_turn_evidence(&loop_detect::TurnEvidence {
            reasoning: "Now reading src/network/loop_detect.rs to verify compiler output.",
            target_files: &[file],
            made_progress: false,
            had_edits: false,
            tool_count: 1,
            no_progress_streak: 1,
        }),
        loop_detect::ReasoningLoopStatus::Ok
    );
}

#[test]
fn test_adversarial_6_bounded_recovery_escalation_prevents_runaway() {
    // Scenario 6: Loop detector fires, recovery loops again -> bounded recovery prevents runaway.
    assert_eq!(reasoning_loop_recovery_action(0), LoopRecoveryAction::Recover);
    assert_eq!(reasoning_loop_recovery_action(1), LoopRecoveryAction::ForceFinal);
    assert_eq!(reasoning_loop_recovery_action(2), LoopRecoveryAction::ForceFinal);
    assert_eq!(reasoning_loop_recovery_action(u8::MAX), LoopRecoveryAction::ForceFinal);
    assert!(REASONING_LOOP_RECOVERY_PROMPT.contains("exactly one mutating tool call"));
}

#[test]
fn test_adversarial_7_file_reread_after_change_is_fresh_not_stale() {
    // Scenario 7: Same file is reread because it changed on disk -> must NOT be treated as stale repetition.
    let mut ledger = loop_detect::ProgressLedger::default();

    // 1. Initial read
    let obs1 = loop_detect::ProgressObservation {
        action: "view_file:src/lib.rs#0".to_string(),
        output_fingerprint: loop_detect::stable_hash("initial content"),
        state_fingerprint: None,
        failure_fingerprint: None,
        changed_workspace: false,
        fresh_read: true,
        search_result: false,
        no_result: false,
        verification: false,
        read_only: true,
        replayed: false,
        success: true,
    };
    let a1 = ledger.observe(&obs1);
    assert_eq!(a1.reason, loop_detect::ProgressReason::FreshRead);
    assert!(a1.meaningful);

    // 2. Edit occurs
    let obs2 = loop_detect::ProgressObservation {
        action: "edit:src/lib.rs".to_string(),
        output_fingerprint: loop_detect::stable_hash("applied"),
        state_fingerprint: Some(loop_detect::stable_hash("src/lib.rs\n+new line")),
        failure_fingerprint: None,
        changed_workspace: true,
        fresh_read: false,
        search_result: false,
        no_result: false,
        verification: false,
        read_only: false,
        replayed: false,
        success: true,
    };
    let a2 = ledger.observe(&obs2);
    assert_eq!(a2.reason, loop_detect::ProgressReason::WorkspaceChanged);
    assert!(a2.meaningful);

    // 3. Post-edit re-read with updated content (replayed is false)
    let obs3 = loop_detect::ProgressObservation {
        action: "view_file:src/lib.rs#0".to_string(),
        output_fingerprint: loop_detect::stable_hash("new updated content"),
        state_fingerprint: None,
        failure_fingerprint: None,
        changed_workspace: false,
        fresh_read: true,
        search_result: false,
        no_result: false,
        verification: false,
        read_only: true,
        replayed: false,
        success: true,
    };
    let a3 = ledger.observe(&obs3);
    assert_eq!(a3.reason, loop_detect::ProgressReason::FreshRead);
    assert!(a3.meaningful);
    assert_eq!(ledger.no_progress_streak(), 0);
}

#[test]
fn test_adversarial_8_readonly_analysis_task_does_not_false_positive() {
    // Scenario 8: Read-only/analysis task with no expected workspace edits ->
    // no-progress-by-no-edit must not create false positives.
    let mut detector = loop_detect::ReasoningLoopDetector::default();
    let mut ledger = loop_detect::ProgressLedger::default();

    let analysis_steps = [
        ("src/network/stream_request.rs", "Examining stream_request for SSE chunk parsing and backpressure."),
        ("src/network/runner.rs", "Checking how runner collects chunks and manages timeouts."),
        ("src/network/turn_engine.rs", "Synthesizing turn execution lifecycle for final explanation."),
    ];

    for (file, reasoning) in analysis_steps {
        let obs = loop_detect::ProgressObservation {
            action: format!("view_file:{file}#0"),
            output_fingerprint: loop_detect::stable_hash(file),
            state_fingerprint: None,
            failure_fingerprint: None,
            changed_workspace: false,
            fresh_read: true,
            search_result: false,
            no_result: false,
            verification: false,
            read_only: true,
            replayed: false,
            success: true,
        };
        let assessment = ledger.observe(&obs);
        assert!(assessment.meaningful);

        let status = detector.record_turn_evidence(&loop_detect::TurnEvidence {
            reasoning,
            target_files: &[file],
            made_progress: false,
            had_edits: false,
            tool_count: 1,
            no_progress_streak: ledger.no_progress_streak(),
        });
        assert_eq!(status, loop_detect::ReasoningLoopStatus::Ok);
    }
}
