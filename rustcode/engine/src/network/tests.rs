use super::turn_engine::{save_turn_context_after_run, take_turn_context_for_prompt};
use super::*;

#[test]
fn zoom_context_dispatch_appends_recalled_data_without_rewriting_the_request_prefix() {
    let original = vec![ChatMessage::new("user", "Correction: use port 5433.")];
    let archive = crate::config::archive_history_prefix(&original).unwrap();
    let session = rustcode_session::next_session_id();
    let mut transcript = vec![
        compaction::durable_compaction_record_message("Database setup discussed", &[], &archive),
        ChatMessage::new("user", "Recover my exact correction"),
    ];
    crate::config::save_session_history(&session, &transcript);
    crate::tools::set_active_session_id(Some(session));
    let mut prefix = RequestPrefixCache::default();
    let initial = history::to_messages(&transcript, "static system prompt");
    let request = prefix.compose(&initial, "runtime context one");
    prefix.record(initial.clone(), &request, "runtime context one");

    for (round, args) in [serde_json::json!({}), serde_json::json!({"message":1})]
        .into_iter()
        .enumerate()
    {
        let call_id = format!("recall-{round}");
        let result = crate::tools::execute_with_metadata("zoom_context", &args);
        assert!(result.success, "{}", result.content);
        let recalled: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        if round == 0 {
            assert_eq!(
                result.completeness,
                rustcode_core::ToolResultCompleteness::UserLimited
            );
            assert_eq!(recalled["messages"][0]["message"], 1);
        } else {
            assert_eq!(recalled["text"], "Correction: use port 5433.");
            assert_eq!(
                result.completeness,
                rustcode_core::ToolResultCompleteness::Complete
            );
        }
        transcript.push(ChatMessage::new("assistant", "").with_tool_calls(vec![
            crate::app::ToolCallRef {
                id: call_id.clone(),
                name: "zoom_context".into(),
                arguments: args.to_string(),
            },
        ]));
        transcript.push(ChatMessage::new("tool", result.content).answering(Some(call_id)));
        let rendered = history::to_messages(&transcript, "static system prompt");
        let next_request = prefix.compose(&rendered, "runtime context two");
        assert_eq!(prefix.last_decision(), PrefixCacheDecision::Reused);
        assert_eq!(&next_request[..initial.len()], initial.as_slice());
        history::validate_native_tool_messages(&next_request).unwrap();
        prefix.record(rendered, &next_request, "runtime context two");
    }
    crate::tools::set_active_session_id(None);
}

#[test]
fn app_state_separates_source_from_active_workspace_root() {
    let workspace = tempfile::tempdir().expect("temporary workspace");
    let state = AppState::new_with_workspace_session(workspace.path(), Some("workspace-root-test"));

    assert_eq!(state.workspace_root, None);
    assert_eq!(
        state.effective_workspace_root().as_deref(),
        Some(workspace.path())
    );
}

#[test]
fn ordinary_app_state_keeps_the_source_separate_from_active_workspace() {
    let source = std::env::current_dir().expect("current source workspace");
    let state = AppState::new();

    assert_eq!(state.workspace_root, None);
    assert_eq!(
        state.effective_workspace_root().as_deref(),
        Some(source.as_path())
    );
}

#[test]
fn active_workspace_overrides_source_and_cleanup_falls_back_to_source() {
    let source = tempfile::tempdir().expect("source workspace");
    let isolated = tempfile::tempdir().expect("isolated workspace");
    let mut state =
        AppState::new_with_workspace_session(source.path(), Some("workspace-root-test"));
    state.workspace_root = Some(isolated.path().to_path_buf());

    assert_eq!(
        state.effective_workspace_root().as_deref(),
        Some(isolated.path())
    );

    state.workspace_root = None;
    assert_eq!(
        state.effective_workspace_root().as_deref(),
        Some(source.path())
    );
}

#[test]
fn deleted_active_workspace_does_not_fall_back_to_source() {
    let source = tempfile::tempdir().expect("source workspace");
    let deleted = tempfile::tempdir().expect("isolated workspace");
    let deleted_path = deleted.path().to_path_buf();
    let mut state =
        AppState::new_with_workspace_session(source.path(), Some("workspace-root-test"));
    state.workspace_root = Some(deleted_path.clone());
    drop(deleted);

    assert_eq!(state.effective_workspace_root(), Some(deleted_path));
}

#[test]
fn request_context_uses_effective_boundary_and_preserves_task_scope() {
    let source = tempfile::tempdir().expect("source workspace");
    let active = tempfile::tempdir().expect("active workspace");
    let task_scope = tempfile::tempdir().expect("task working directory");
    let mut state = AppState::new_with_workspace_session(source.path(), Some("request-roots"));
    state.task_working_directory = Some(task_scope.path().to_path_buf());

    let (boundary, scope) = super::request_context_roots(&state);
    assert_eq!(boundary.as_deref(), Some(source.path()));
    assert_eq!(scope.as_deref(), Some(task_scope.path()));

    state.workspace_root = Some(active.path().to_path_buf());
    let (boundary, scope) = super::request_context_roots(&state);
    assert_eq!(boundary.as_deref(), Some(active.path()));
    assert_eq!(scope.as_deref(), Some(task_scope.path()));
}

#[test]
fn request_history_uses_full_transcript_until_soft_target_pressure_and_keeps_tool_pairs_valid() {
    let history = vec![
        ChatMessage::new("user", "first task"),
        ChatMessage::new("assistant", "older call context").with_tool_calls(vec![
            crate::app::ToolCallRef {
                id: "older-call".into(),
                name: "run_command".into(),
                arguments: r#"{"command":"npm run build"}"#.into(),
            },
        ]),
        ChatMessage::new("tool", "run_command: earlier build output")
            .answering(Some("older-call".into())),
        ChatMessage::new("assistant", "first task done"),
        ChatMessage::new("user", "second task"),
        ChatMessage::new("assistant", "second task done"),
        ChatMessage::new("user", "current task"),
        ChatMessage::new("assistant", "current call context").with_tool_calls(vec![
            crate::app::ToolCallRef {
                id: "current-call".into(),
                name: "view_file".into(),
                arguments: r#"{"path":"src/lib.rs"}"#.into(),
            },
        ]),
        ChatMessage::new("tool", "view_file: current inspection output")
            .answering(Some("current-call".into())),
    ];
    let instructions = history::RequestInstructions::new("base", None);
    let full = history::to_messages_for_request_with_scope(
        &history,
        instructions,
        history::RequestHistoryScope::Full,
    );
    let mut budget = crate::config::ModelProfile {
        name: "projection-test".into(),
        model: "projection-test".into(),
        context_window: Some(100_000),
        max_output_tokens: Some(1_000),
        ..Default::default()
    }
    .context_budget();
    budget.soft_context_target = u32::MAX;
    let full_preflight =
        compaction::calculate_preflight_budget_for_projection(&full, &[], 0, &budget);

    assert_eq!(
        history_scope_for_preflight(&full_preflight),
        history::RequestHistoryScope::Full
    );
    history::validate_native_tool_messages(&full).expect("full history keeps valid tool pairs");
    assert!(
        serde_json::to_string(&full)
            .unwrap()
            .contains("earlier build output")
    );

    budget.soft_context_target = u32::try_from(full_preflight.total_estimated_prompt)
        .expect("fixture prompt fits in u32")
        .saturating_sub(1);
    let pressured_preflight =
        compaction::calculate_preflight_budget_for_projection(&full, &[], 0, &budget);
    let pressured_scope = history_scope_for_preflight(&pressured_preflight);
    assert_eq!(pressured_scope, history::RequestHistoryScope::RecentTurns);

    let pressured =
        history::to_messages_for_request_with_scope(&history, instructions, pressured_scope);
    history::validate_native_tool_messages(&pressured)
        .expect("recent-turn projection keeps valid tool pairs");
    let rendered = serde_json::to_string(&pressured).unwrap();
    assert!(!rendered.contains("older-call"));
    assert!(!rendered.contains("earlier build output"));
    assert!(rendered.contains("current-call"));
    assert!(rendered.contains("current inspection output"));
}

#[tokio::test]
async fn context_detection_does_not_probe_unverified_omlx_endpoints() {
    use std::time::Duration;
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let chat_url = format!("http://{address}/v1/chat/completions");
    let client = reqwest::Client::new();

    assert_eq!(
        fetch_context_window(&client, &chat_url, "model", Some("omlx")).await,
        None
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(100), listener.accept())
            .await
            .is_err(),
        "oMLX must use an explicit profile/provider context limit"
    );
}

#[test]
fn model_result_contract_distinguishes_read_completeness_states() {
    let complete = tool_result_from_execution(
        "view_file",
        &serde_json::json!({"path": "src/lib.rs", "start_line": 1, "end_line": 15}),
        crate::tools::ToolExecutionOutput::success(
            "[File: src/lib.rs, Lines 1 to 15 of 15]\n1: fn main() {}".to_string(),
        ),
        None,
    );
    assert_eq!(
        complete.metadata.completeness,
        rustcode_core::ToolResultCompleteness::Complete
    );

    let user_limited = tool_result_from_execution(
        "view_file",
        &serde_json::json!({"path": "src/lib.rs", "start_line": 1, "end_line": 1}),
        crate::tools::ToolExecutionOutput {
            completeness: rustcode_core::ToolResultCompleteness::UserLimited,
            ..crate::tools::ToolExecutionOutput::success(
                "[File: src/lib.rs, Lines 1 to 1 of 15]\n1: fn main() {}\nunchanged wording"
                    .to_string(),
            )
        },
        None,
    );
    assert_eq!(
        user_limited.metadata.completeness,
        rustcode_core::ToolResultCompleteness::UserLimited
    );

    let line_truncated = tool_result_from_execution(
        "view_file",
        &serde_json::json!({"path": "src/lib.rs"}),
        crate::tools::ToolExecutionOutput {
            content: "[Truncated: lines 801-1000 of 1000]".to_string(),
            truncated: true,
            completeness: rustcode_core::ToolResultCompleteness::LineTruncated,
            ..crate::tools::ToolExecutionOutput::success(String::new())
        },
        None,
    );
    assert_eq!(
        line_truncated.metadata.completeness,
        rustcode_core::ToolResultCompleteness::LineTruncated
    );

    let byte_truncated = tool_result_from_execution(
        "run_command",
        &serde_json::json!({"command": "yes"}),
        crate::tools::ToolExecutionOutput {
            content: "[Output truncated: 100 bytes total]".to_string(),
            truncated: true,
            completeness: rustcode_core::ToolResultCompleteness::ByteTruncated,
            ..crate::tools::ToolExecutionOutput::success(String::new())
        },
        None,
    );
    assert_eq!(
        byte_truncated.metadata.completeness,
        rustcode_core::ToolResultCompleteness::ByteTruncated
    );
}

#[test]
fn inspection_result_contract_persists_ranges_and_canonical_fingerprint() {
    let result = tool_result_from_execution(
        "view_file",
        &serde_json::json!({"path": "src/lib.rs", "start_line": 1}),
        crate::tools::ToolExecutionOutput {
            content: "[File: src/lib.rs, Lines 1 to 800 of 1000, Bytes offset: 0]\nsource\n[Truncated: lines 801-1000 of 1000]".to_string(),
            truncated: true,
            completeness: rustcode_core::ToolResultCompleteness::LineTruncated,
            ..crate::tools::ToolExecutionOutput::success(String::new())
        },
        None,
    );
    let inspection = result
        .metadata
        .inspection
        .as_ref()
        .expect("inspection metadata");
    assert_eq!(inspection.requested_path.as_deref(), Some("src/lib.rs"));
    assert_eq!(
        inspection.requested_range,
        Some(rustcode_core::InspectionRange {
            start: Some(1),
            end: None,
        })
    );
    assert_eq!(inspection.returned_path.as_deref(), Some("src/lib.rs"));
    assert_eq!(
        inspection.returned_range,
        Some(rustcode_core::InspectionRange {
            start: Some(1),
            end: Some(800),
        })
    );
    assert!(!inspection.complete);
    assert_eq!(
        inspection.next_range,
        Some(rustcode_core::InspectionRange {
            start: Some(801),
            end: Some(1000),
        })
    );
    assert_eq!(inspection.fingerprint, "read:src/lib.rs#0");

    let message = tool_result_history_message(result, None);
    assert!(!message.content.contains("requested_path"));
    let model = history::to_messages(&[message], "system");
    let payload = model[1]["content"].as_str().expect("model result");
    assert!(payload.contains("requested_path"));
    assert!(payload.contains("next_range"));
    assert!(payload.contains("fingerprint"));
}

#[test]
fn final_truncation_recomputes_mid_file_inspection_ranges() {
    let args = serde_json::json!({
        "path": "src/large.ts",
        "start_line": 1,
        "end_line": 140
    });
    let content = format!(
        "[File: src/large.ts, Lines 1 to 140 of 140, Bytes offset: 0]\n{}",
        (1..=140)
            .map(|line| format!("{line}: {}", "x".repeat(500)))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let result = finalize_tool_result(
        tool_result_from_execution(
            "view_file",
            &args,
            crate::tools::ToolExecutionOutput::success(content),
            None,
        ),
        None,
    );
    let inspection = result.metadata.inspection.expect("inspection metadata");

    assert!(result.metadata.truncated);
    assert!(result.metadata.payload_truncated);
    assert_eq!(
        result.metadata.completeness,
        rustcode_core::ToolResultCompleteness::Complete,
        "transport clipping must not overwrite source completeness"
    );
    assert!(!inspection.complete);
    assert!(
        inspection
            .delivered_ranges
            .iter()
            .any(|range| range.end.unwrap() < 140),
        "returned metadata must not claim the original range: {inspection:?}"
    );
    assert_ne!(
        inspection.returned_range,
        Some(rustcode_core::InspectionRange {
            start: Some(1),
            end: Some(140),
        })
    );
    let next = inspection.next_range.expect("actionable omitted range");
    assert!(next.start.unwrap() <= next.end.unwrap());
    assert!(next.end.unwrap() < 140);
}

#[test]
fn final_truncation_recomputes_mid_line_range_and_survives_replay() {
    let args = serde_json::json!({"path": "src/long.ts", "start_line": 1});
    let content = format!(
        "[File: src/long.ts, Lines 1 to 1 of 1, Bytes offset: 0]\n1: {}",
        "x".repeat(100_000)
    );
    let result = finalize_tool_result(
        tool_result_from_execution(
            "view_file",
            &args,
            crate::tools::ToolExecutionOutput::success(content),
            None,
        ),
        None,
    );
    let inspection = result
        .metadata
        .inspection
        .as_ref()
        .expect("inspection metadata")
        .clone();

    assert!(result.metadata.truncated);
    assert!(result.metadata.payload_truncated);
    assert_eq!(
        result.metadata.completeness,
        rustcode_core::ToolResultCompleteness::Complete
    );
    assert!(!inspection.complete);
    assert!(
        inspection.returned_range.is_none(),
        "a line cut mid-content is not a complete returned range: {inspection:?}"
    );
    assert_eq!(
        inspection.next_range,
        Some(rustcode_core::InspectionRange {
            start: Some(1),
            end: Some(1),
        })
    );

    let artifact = result.metadata.full_output_artifact.clone();
    if let Some(path) = artifact.as_deref() {
        assert!(
            std::fs::metadata(path).is_ok(),
            "artifact must be preserved"
        );
    }
    let message = tool_result_history_message(result, None);
    let record = message.tool_result.as_ref().expect("persisted metadata");
    assert_eq!(record.inspection.as_ref(), Some(&inspection));
    assert_eq!(record.full_output_artifact, artifact);
    let replay = history::to_messages(&[message], "system");
    let payload = replay[1]["content"].as_str().expect("model result");
    assert!(payload.contains("next_range"));
    assert!(payload.contains("\"start\":1"));
}

#[test]
fn equivalent_inspection_tools_share_a_fingerprint() {
    let view = tool_result_from_execution(
        "view_file",
        &serde_json::json!({"path": "src/lib.rs", "start_line": 1}),
        crate::tools::ToolExecutionOutput::success(
            "[File: src/lib.rs, Lines 1 to 2 of 2]\ncode".into(),
        ),
        None,
    );
    let shell = tool_result_from_execution(
        "run_command",
        &serde_json::json!({"command": "sed -n '1,2p' src/lib.rs"}),
        crate::tools::ToolExecutionOutput::success("code".into()),
        None,
    );
    assert_eq!(
        view.metadata.inspection.as_ref().unwrap().fingerprint,
        shell.metadata.inspection.as_ref().unwrap().fingerprint
    );
}

#[test]
fn deterministic_compaction_persists_the_retained_suffix_anchor() {
    let mut history = vec![
        ChatMessage::new("user", "old task"),
        ChatMessage::new("assistant", "old progress ".repeat(300)),
        ChatMessage::new("user", "recent task"),
        ChatMessage::new("assistant", "recent progress"),
    ];

    let original = history.clone();
    assert!(compact_history_deterministically(&mut history, 100));
    assert!(history[0].compaction_boundary.is_some());
    let expected_anchor = history
        .get(1)
        .map(crate::app::CompactionEntry::from_message);
    assert_eq!(
        history[0]
            .compaction_boundary
            .as_ref()
            .and_then(|boundary| boundary.first_retained_entry.as_ref()),
        expected_anchor.as_ref()
    );
    let boundary = history[0]
        .compaction_boundary
        .as_ref()
        .expect("typed compaction boundary");
    let archive_path = boundary
        .history_archive
        .as_ref()
        .expect("immutable history archive");
    let archived = std::fs::read_to_string(archive_path).expect("archived JSONL history");
    let archived_messages = archived
        .lines()
        .map(|line| serde_json::from_str::<ChatMessage>(line).expect("original chat message"))
        .collect::<Vec<_>>();
    let boundary_index = original
        .iter()
        .position(|message| {
            Some(&crate::app::CompactionEntry::from_message(message))
                == boundary.first_retained_entry.as_ref()
        })
        .expect("retained anchor from original history");
    assert_eq!(archived_messages, original[..boundary_index]);
    assert_eq!(
        boundary.history_archive.as_deref(),
        Some(archive_path.as_str())
    );
}

#[test]
fn deterministic_compaction_keeps_history_when_archive_fails() {
    let mut history = vec![
        ChatMessage::new("user", "old task"),
        ChatMessage::new("assistant", "old progress ".repeat(300)),
        ChatMessage::new("user", "recent task"),
        ChatMessage::new("assistant", "recent progress"),
    ];
    let original = history.clone();

    assert!(!compact_history_deterministically_with_archive(
        &mut history,
        100,
        |_| Err("archive unavailable".to_string()),
    ));
    assert_eq!(history, original);
}

#[test]
fn structured_tool_replay_keeps_metadata_out_of_ui_content_but_in_model_payload() {
    let message = tool_result_history_message(
        ToolResult {
            tool_name: "view_file".to_string(),
            content: "[File: x, Lines 1 to 15 of 15]".to_string(),
            diff: None,
            file_preview: None,
            metadata: ToolResultMetadata {
                call_id: Some("call-1".to_string()),
                arguments_hash: "hash".to_string(),
                success: true,
                completeness: rustcode_core::ToolResultCompleteness::Complete,
                ..Default::default()
            },
        },
        Some("call-1".to_string()),
    );
    assert!(!message.content.contains("result_metadata"));
    let history = vec![
        ChatMessage::new("user", "read x"),
        ChatMessage::new("assistant", "").with_tool_calls(vec![crate::app::ToolCallRef {
            id: "call-1".to_string(),
            name: "view_file".to_string(),
            arguments: "{\"path\":\"x\"}".to_string(),
        }]),
        message,
    ];
    let messages = history::to_messages(&history, "system");
    let model_content = messages[3]["content"].as_str().expect("tool content");
    assert!(model_content.contains("result_metadata"));
    assert!(model_content.contains("arguments_hash"));
}

#[test]
fn bounded_results_keep_incomplete_marker_and_completeness_through_replay() {
    let result = finalize_tool_result(
        ToolResult {
            tool_name: "view_file".to_string(),
            content: "partial source range".to_string(),
            diff: None,
            file_preview: None,
            metadata: ToolResultMetadata {
                success: true,
                truncated: true,
                ..Default::default()
            },
        },
        None,
    );
    assert_eq!(
        result.metadata.completeness,
        rustcode_core::ToolResultCompleteness::ByteTruncated
    );
    assert!(result.content.contains("tool_result_incomplete:"));

    let message = tool_result_history_message(result, None);
    let record = message.tool_result.as_ref().expect("typed metadata");
    assert!(record.truncated);
    assert_eq!(
        record.resolved_completeness(),
        rustcode_core::ToolResultCompleteness::ByteTruncated
    );
    let replay = history::to_messages(&[message], "system");
    let model_content = replay[1]["content"].as_str().expect("model result");
    assert!(model_content.contains("tool_result_incomplete:"));
    assert!(model_content.contains("byte_truncated"));
}

#[test]
fn background_wakeup_reuses_the_logical_turn_context_after_orchestrator_yields() {
    let mut state = AppState::new();
    let mut context = TurnContext::with_max_tool_rounds(7);
    context.budget.tool_rounds = 4;
    context.budget.round_budget_notice_sent = true;
    context.progress.failed_mutations = 2;
    context.progress.consecutive_failed_mutations = 2;
    context
        .progress
        .changed_paths
        .insert("README.md".to_string());
    context.progress.grounded_artifact = Some(super::turn_engine::GroundedArtifactEvidence {
        path: "README.md".to_string(),
        write_lines: Some(10),
        write_bytes: Some(120),
        read_range: Some((1, 10)),
        repair_attempts: 1,
    });
    context
        .recovery
        .loop_detector
        .record_failed_tool("edit:readme:1", "edit:README.md");
    context
        .recovery
        .loop_detector
        .check("inspect:README.md", "inspect:README.md");
    context
        .recovery
        .loop_detector
        .check("inspect:README.md", "inspect:README.md");
    let progress_observation = loop_detect::ProgressObservation {
        action: "run_command:markdownlint".to_string(),
        output_fingerprint: 11,
        state_fingerprint: None,
        failure_fingerprint: Some(22),
        changed_workspace: false,
        fresh_read: false,
        search_result: false,
        no_result: false,
        verification: true,
        read_only: true,
        replayed: false,
        success: false,
    };
    context.progress.ledger.observe(&progress_observation);
    context.progress.ledger.observe(&progress_observation);
    context.verification.ledger.record_edit();
    context
        .verification
        .ledger
        .record_explicit_command("markdownlint README.md", Some(1));
    context.lifecycle.stop_reason = Some(lifecycle::StopReason::BackgroundPending);

    save_turn_context_after_run(&mut state, context, true);

    let mut resumed = take_turn_context_for_prompt(&mut state, true, 99);

    assert_eq!(resumed.budget.max_tool_rounds, usize::MAX);
    assert_eq!(resumed.budget.max_total_tool_rounds, usize::MAX);
    assert_eq!(resumed.budget.tool_rounds, 4);
    assert!(!resumed.budget.round_budget_notice_sent);
    assert_eq!(resumed.progress.failed_mutations, 2);
    assert_eq!(resumed.progress.consecutive_failed_mutations, 2);
    assert_eq!(
        resumed.progress.changed_paths.iter().collect::<Vec<_>>(),
        ["README.md"]
    );
    assert_eq!(
        resumed
            .progress
            .grounded_artifact
            .as_ref()
            .map(|evidence| evidence.path.as_str()),
        Some("README.md")
    );
    assert_eq!(
        resumed
            .recovery
            .loop_detector
            .check("inspect:README.md", "inspect:README.md"),
        loop_detect::LoopStatus::Warning(3),
        "ordinary loop history must continue across a background wakeup"
    );
    assert_eq!(
        resumed
            .recovery
            .loop_detector
            .record_failed_tool("edit:readme:2", "edit:README.md"),
        loop_detect::LoopStatus::Abort(2),
        "failed-mutation repetition must continue across a background wakeup"
    );
    assert_eq!(resumed.progress.ledger.no_progress_streak(), 2);
    assert_eq!(
        resumed
            .verification
            .ledger
            .explicit_last_failure()
            .map(|evidence| evidence.command.as_str()),
        Some("markdownlint README.md")
    );
}

#[test]
fn new_user_prompt_starts_fresh_turn_context() {
    let mut state = AppState::new();
    let mut context = TurnContext::new();
    context.budget.round_budget_notice_sent = true;
    context.progress.failed_mutations = 3;
    context
        .progress
        .changed_paths
        .insert("src/lib.rs".to_string());
    context.lifecycle.stop_reason = Some(lifecycle::StopReason::BackgroundPending);
    save_turn_context_after_run(&mut state, context, true);

    let fresh = take_turn_context_for_prompt(&mut state, false, 9);

    assert_eq!(fresh.budget.max_tool_rounds, usize::MAX);
    assert_eq!(fresh.budget.max_total_tool_rounds, usize::MAX);
    assert_eq!(fresh.budget.tool_rounds, 0);
    assert!(!fresh.budget.round_budget_notice_sent);
    assert_eq!(fresh.progress.failed_mutations, 0);
    assert!(fresh.progress.changed_paths.is_empty());
    assert!(fresh.verification.ledger.explicit_last_failure().is_none());
    assert!(state.background_turn_context.is_none());
}

#[test]
fn explicit_verification_hydrates_the_terminal_background_result() {
    let history = vec![
        ChatMessage::new("user", "Run `custom-tool --strict` and report the result."),
        ChatMessage::new("tool", "background task failed").with_tool_result(
            crate::app::ToolResultRecord {
                workspace_generation: None,
                workspace_epoch: None,
                evidence_hash: None,
                tool_name: "background_task".to_string(),
                pending: false,
                command: Some("custom-tool --strict".to_string()),
                success: false,
                exit_code: Some(7),
                ..Default::default()
            },
        ),
    ];
    let mut ledger = verification::VerificationLedger::default();

    crate::network::turn_engine::hydrate_explicit_verification_from_history(
        &mut ledger,
        &history,
        0,
    );

    assert_eq!(
        ledger
            .explicit_last_failure()
            .map(|evidence| evidence.command.as_str()),
        Some("custom-tool --strict")
    );
}

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
            command_status: Some(rustcode_core::CommandResultMetadata {
                completed: true,
                exit_code: Some(0),
                bytes_returned: 12,
                total_output_bytes: Some(12),
                ..Default::default()
            }),
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
    assert_eq!(
        envelope
            .command_status
            .as_ref()
            .map(|status| status.bytes_returned),
        Some(12)
    );
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
        workspace_generation: None,
        workspace_epoch: None,
        evidence_hash: None,
        error_kind: Some(crate::tools::ToolErrorKind::McpFailed.as_str().to_string()),
        retryable: true,
        replayed: true,
        pending: true,
        command: Some("long-running-check".to_string()),
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
    assert!(reloaded.pending);
    assert_eq!(reloaded.command.as_deref(), Some("long-running-check"));
    assert_eq!(reloaded.exit_code, Some(7));
    assert_eq!(reloaded.changed_paths, ["src/mcp.rs"]);
}

#[test]
fn deferred_tool_calls_are_distinct_and_not_retryable() {
    let calls = vec![crate::app::ToolCallRef {
        id: "call_deferred".into(),
        name: "run_command".into(),
        arguments: r#"{"command":"echo deferred"}"#.into(),
    }];

    let deferred = unanswered_call_results_with_kind(
        &calls,
        "intentionally deferred by the scheduler",
        crate::tools::ToolErrorKind::Deferred,
    );
    let record = deferred[0].tool_result.as_ref().unwrap();

    assert_eq!(
        record.parsed_error_kind(),
        Some(crate::tools::ToolErrorKind::Deferred)
    );
    assert!(!record.retryable);
    assert!(deferred[0].content.contains("intentionally deferred"));

    let internal = unanswered_call_results(&calls, "harness failure");
    assert_eq!(
        internal[0]
            .tool_result
            .as_ref()
            .unwrap()
            .parsed_error_kind(),
        Some(crate::tools::ToolErrorKind::Internal)
    );
    assert!(internal[0].tool_result.as_ref().unwrap().retryable);
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
async fn session_title_tool_is_rejected_after_first_turn_without_running() {
    let state = Arc::new(Mutex::new(AppState::new()));
    let session_id = state.lock().await.active_session_id.clone();
    {
        let mut state = state.lock().await;
        state.session_title_cache = Some((session_id, Some("existing".to_string())));
        state.session_title_tool_available = false;
    }
    let cancel = tokio_util::sync::CancellationToken::new();
    let calls = vec![crate::tools::ToolCall {
        name: "set_session_title".to_string(),
        arguments: serde_json::json!({"title": "should not run"}),
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
        true,
        &None,
        &mut dirty,
        &mut cache,
        &mut wait,
        None,
    )
    .await;

    assert!(!results[0].metadata.success);
    assert_eq!(
        results[0].metadata.error_kind,
        Some(crate::tools::ToolErrorKind::UnavailableDependency)
    );
    assert!(
        results[0]
            .content
            .contains("only available during the first turn")
    );
    assert!(state.lock().await.session_title_cache.is_some());
}

#[tokio::test]
async fn successful_first_turn_session_title_invalidates_ui_cache() {
    let state = Arc::new(Mutex::new(AppState::new()));
    let session_id = state.lock().await.active_session_id.clone();
    {
        let mut state = state.lock().await;
        state.session_title_cache = Some((session_id, Some("old title".to_string())));
        state.session_title_tool_available = true;
    }
    let cancel = tokio_util::sync::CancellationToken::new();
    let calls = vec![crate::tools::ToolCall {
        name: "set_session_title".to_string(),
        arguments: serde_json::json!({"title": "Focused session title"}),
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
        true,
        &None,
        &mut dirty,
        &mut cache,
        &mut wait,
        None,
    )
    .await;

    assert!(results[0].metadata.success, "{results:?}");
    let state = state.lock().await;
    assert!(!state.session_title_tool_available);
    assert!(state.session_title_cache.is_none());
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

async fn streaming_provider_server() -> (String, tokio::sync::oneshot::Receiver<Vec<u8>>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind streaming test server");
    let address = listener
        .local_addr()
        .expect("streaming test server address");
    let (accepted_tx, accepted_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept streaming request");
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4_096];
        loop {
            let read = socket
                .read(&mut buffer)
                .await
                .expect("read streaming request");
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
        accepted_tx.send(request).ok();
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"wakeup request reached provider\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n"
        );
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        socket
            .write_all(response.as_bytes())
            .await
            .expect("write streaming response");
    });
    (format!("http://{address}"), accepted_rx)
}

struct AcpPromptTestPolicy;

impl policy::TurnPolicy for AcpPromptTestPolicy {
    async fn should_approve(
        &self,
        _state: &Arc<tokio::sync::Mutex<crate::app::AppState>>,
        _tool_calls: &[crate::tools::ToolCall],
    ) -> bool {
        true
    }

    fn should_verify_completion(&self) -> bool {
        false
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn acp_prompt_turn_releases_state_and_delivers_provider_completion() {
    use crate::app::ChatMessage;
    use crate::config::{ApiProtocol, ModelProfile};
    use std::time::Duration;
    use tokio::sync::Mutex;
    use tokio_util::sync::CancellationToken;

    let (provider_url, provider_request) = streaming_provider_server().await;
    let endpoint = format!("{provider_url}/v1/chat/completions");
    let mut app = crate::app::AppState::new();
    app.api_base_url = endpoint.clone();
    app.model_name = "acp-prompt-test".to_owned();
    app.config.models = vec![ModelProfile {
        name: app.model_name.clone(),
        url: endpoint.clone(),
        model: app.model_name.clone(),
        api_protocol: Some(ApiProtocol::ChatCompletions),
        context_window: Some(8_192),
        ..ModelProfile::default()
    }];
    app.record_function_calling_support(&endpoint, false);
    app.history
        .push(ChatMessage::new("user", "Reply with exactly: PONG"));
    let state = Arc::new(Mutex::new(app));
    let (sender, mut receiver) = ui_adapter::AgentUiEventSender::channel();
    let client = reqwest::Client::new();
    let cancellation = CancellationToken::new();
    let policy = Arc::new(AcpPromptTestPolicy);
    let stream_buffer = Arc::new(Mutex::new(StreamBuffer::new()));
    let context = tokio::time::timeout(
        Duration::from_secs(5),
        ui_adapter::run_agent_turn_with_events_for_acp(
            &client,
            &state,
            &cancellation,
            &policy,
            &stream_buffer,
            "Reply with exactly: PONG".to_owned(),
            sender,
        ),
    )
    .await
    .expect("ACP prompt must not hold AppState locked while waiting for its turn");

    provider_request
        .await
        .expect("mock provider must receive the ACP prompt");
    assert_eq!(
        context.response.final_content,
        "wakeup request reached provider"
    );
    assert!(matches!(
        receiver.recv().await,
        Some(ui_adapter::AgentUiEvent::PromptStarted { .. })
    ));
    let mut completed_content = None;
    while let Some(event) = receiver.recv().await {
        if let ui_adapter::AgentUiEvent::TurnFinished { content, .. } = event {
            completed_content = Some(content);
            break;
        }
    }
    assert_eq!(
        completed_content.as_deref(),
        Some("wakeup request reached provider"),
        "ACP must emit a terminal event carrying the provider output"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn acp_continuation_releases_state_and_delivers_provider_completion() {
    use crate::app::ChatMessage;
    use crate::config::{ApiProtocol, ModelProfile};
    use std::time::Duration;
    use tokio::sync::Mutex;
    use tokio_util::sync::CancellationToken;

    let (provider_url, provider_request) = streaming_provider_server().await;
    let endpoint = format!("{provider_url}/v1/chat/completions");
    let mut app = crate::app::AppState::new();
    app.api_base_url = endpoint.clone();
    app.model_name = "acp-continuation-test".to_owned();
    app.config.models = vec![ModelProfile {
        name: app.model_name.clone(),
        url: endpoint.clone(),
        model: app.model_name.clone(),
        api_protocol: Some(ApiProtocol::ChatCompletions),
        context_window: Some(8_192),
        ..ModelProfile::default()
    }];
    app.record_function_calling_support(&endpoint, false);
    app.history
        .push(ChatMessage::new("user", "Continue the existing task"));
    let state = Arc::new(Mutex::new(app));
    let (sender, mut receiver) = ui_adapter::AgentUiEventSender::channel();
    let client = reqwest::Client::new();
    let cancellation = CancellationToken::new();
    let policy = Arc::new(AcpPromptTestPolicy);
    let stream_buffer = Arc::new(Mutex::new(StreamBuffer::new()));
    let context = TurnContext::with_budgets(8, 16);
    let context = tokio::time::timeout(
        Duration::from_secs(5),
        ui_adapter::run_agent_turn_with_events_and_context_for_acp(
            &client,
            &state,
            &cancellation,
            &policy,
            &stream_buffer,
            String::new(),
            sender,
            context,
        ),
    )
    .await
    .expect("ACP continuation must release AppState while waiting for its turn");

    provider_request
        .await
        .expect("mock provider must receive the ACP continuation");
    assert_eq!(
        context.response.final_content,
        "wakeup request reached provider"
    );
    assert!(matches!(
        receiver.recv().await,
        Some(ui_adapter::AgentUiEvent::PromptStarted { prompt , ..}) if prompt.is_empty()
    ));
    let mut completed_content = None;
    while let Some(event) = receiver.recv().await {
        if let ui_adapter::AgentUiEvent::TurnFinished { content, .. } = event {
            completed_content = Some(content);
            break;
        }
    }
    assert_eq!(
        completed_content.as_deref(),
        Some("wakeup request reached provider"),
        "ACP continuation must emit a terminal event carrying the provider output"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn acp_loaded_session_continues_with_its_persisted_transcript() {
    use crate::app::ChatMessage;
    use crate::config::{ApiProtocol, ModelProfile};
    use std::time::Duration;
    use tokio::sync::Mutex;
    use tokio_util::sync::CancellationToken;

    let (provider_url, provider_request) = streaming_provider_server().await;
    let endpoint = format!("{provider_url}/v1/chat/completions");
    let mut app = crate::acp::build_loaded_app_state(
        "loaded-session-123",
        &std::env::current_dir().unwrap(),
        vec![
            ChatMessage::new("user", "Earlier user request"),
            ChatMessage::new("assistant", "Earlier assistant answer"),
        ],
    );
    app.api_base_url = endpoint.clone();
    app.model_name = "acp-loaded-session-test".to_owned();
    app.config.models = vec![ModelProfile {
        name: app.model_name.clone(),
        url: endpoint.clone(),
        model: app.model_name.clone(),
        api_protocol: Some(ApiProtocol::ChatCompletions),
        context_window: Some(8_192),
        ..ModelProfile::default()
    }];
    app.record_function_calling_support(&endpoint, false);
    let state = Arc::new(Mutex::new(app));
    let (sender, _) = ui_adapter::AgentUiEventSender::channel();
    let client = reqwest::Client::new();
    let cancellation = CancellationToken::new();
    let policy = Arc::new(AcpPromptTestPolicy);
    let stream_buffer = Arc::new(Mutex::new(StreamBuffer::new()));
    let context = TurnContext::with_budgets(8, 16);
    tokio::time::timeout(
        Duration::from_secs(5),
        ui_adapter::run_agent_turn_with_events_and_context_for_acp(
            &client,
            &state,
            &cancellation,
            &policy,
            &stream_buffer,
            String::new(),
            sender,
            context,
        ),
    )
    .await
    .expect("loaded ACP session prompt must complete");
    let request = provider_request.await.expect("provider request");
    let request_text = String::from_utf8(request).expect("HTTP request UTF-8");
    let body = request_text
        .split_once("\r\n\r\n")
        .expect("HTTP request body")
        .1;
    let request_json: serde_json::Value = serde_json::from_str(body).expect("provider JSON");
    let messages = request_json["messages"].as_array().expect("messages");
    assert!(messages.iter().any(|message| {
        message["role"] == "user" && message["content"] == "Earlier user request"
    }));
    assert!(
        messages.iter().any(|message| {
            message["role"] == "assistant"
                && message["content"]
                    .as_str()
                    .is_some_and(|content| content.starts_with("Earlier assistant answer"))
        }),
        "loaded assistant transcript should be sent to the provider"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn background_wakeup_releases_state_during_native_schema_selection_and_starts_provider_request()
 {
    use crate::app::state::AppState;
    use crate::config::{ApiProtocol, ModelProfile, ToolProtocol};
    use crate::network::policy::InteractivePolicy;
    use crate::tools::install_native_schema_test_gate;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::Mutex;
    use tokio_util::sync::CancellationToken;

    let marker = "issue-1279-native-schema-gate";
    let (provider_url, provider_request) = streaming_provider_server().await;
    let mut app = AppState::new();
    app.api_base_url = provider_url.clone();
    app.model_name = "issue-1279-model".to_string();
    app.config.models = vec![ModelProfile {
        name: app.model_name.clone(),
        url: provider_url,
        model: app.model_name.clone(),
        api_protocol: Some(ApiProtocol::ChatCompletions),
        tool_protocol: Some(ToolProtocol::ApiNative),
        context_window: Some(32_768),
        ..ModelProfile::default()
    }];
    app.history.push(ChatMessage::new("user", marker));
    app.pending_queue = vec!["__task_wakeup__:issue-1279".to_string()];
    let state = Arc::new(Mutex::new(app));
    let gate = install_native_schema_test_gate(marker, 3);
    let lease = state
        .lock()
        .await
        .claim_orchestrator()
        .expect("test orchestrator lease");
    let (ui_events, _ui_event_receiver) = ui_adapter::AgentUiEventSender::channel();
    let task = tokio::spawn(crate::network::process_queue_orchestrator_with_ui_events(
        reqwest::Client::new(),
        Arc::clone(&state),
        CancellationToken::new(),
        Arc::new(InteractivePolicy),
        ui_events,
        lease,
    ));

    tokio::time::timeout(Duration::from_secs(5), async {
        while !gate.is_entered() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("provider-startup schema selection must reach the deterministic gate");

    let lock_acquired = tokio::time::timeout(Duration::from_millis(100), async {
        let _guard = state.lock().await;
    })
    .await
    .is_ok();
    gate.release();

    let provider_started = tokio::time::timeout(Duration::from_secs(10), provider_request)
        .await
        .is_ok();
    tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .expect("wakeup orchestrator must finish")
        .expect("wakeup orchestrator must not panic");

    assert!(
        lock_acquired,
        "AppState lock remained held while provider schema computation was gated"
    );
    assert!(provider_started, "provider must observe the wakeup request");
    let state = state.lock().await;
    assert!(state.pending_queue.is_empty());
    assert!(!state.orchestrator_running);
}

#[tokio::test]
async fn gemini_gateway_uses_capability_probe() {
    let (url, request_accepted, release_response) = gated_json_server(serde_json::json!({})).await;
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
        // Leave enough room for the compacted summary and final request so
        // this test continues to exercise session isolation rather than the
        // intentional over-budget preflight checkpoint.
        profile.context_window = Some(2_000);
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
        // Round 0 is the turn boundary, the only point the summarizer runs.
        prepare_turn_request(
            &reqwest::Client::new(),
            &request_state,
            0,
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
    // The new session's own history is untouched. At a turn boundary the
    // request it builds may still overflow and append its one-time notice.
    assert!(live.history[..new_session_history.len()] == new_session_history[..]);
    assert!(
        live.history[new_session_history.len()..]
            .iter()
            .all(|message| message.role == "system"
                && message.content.starts_with("context window full:"))
    );
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
    let whole_file = "x".repeat(50_000);
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
fn output_limited_call_results_are_retryable_and_bounded() {
    let refs = vec![crate::app::ToolCallRef {
        id: "call_length".to_string(),
        name: "write_to_file".to_string(),
        arguments: "{}".to_string(),
    }];
    let answers = unanswered_call_results_with_kind(
        &refs,
        "provider stopped at the output limit; the call was not executed",
        crate::tools::ToolErrorKind::OutputLimit,
    );

    assert_eq!(answers.len(), 1);
    assert_eq!(answers[0].tool_call_id.as_deref(), Some("call_length"));
    assert!(answers[0].tool_result.as_ref().is_some_and(|record| {
        record.error_kind.as_deref() == Some("OutputLimit") && record.retryable
    }));
    assert!(answers[0].content.len() < 200);
}

#[tokio::test]
async fn output_truncated_native_response_never_executes_salvaged_call() {
    let dir = tempfile::tempdir().expect("tempdir");
    let target = dir.path().join("must-not-exist.txt");
    let state = Arc::new(Mutex::new(AppState::new()));
    {
        let mut state = state.lock().await;
        state.auto_confirm = true;
        let api_base_url = state.api_base_url.clone();
        state.record_function_calling_support(&api_base_url, true);
    }
    let policy = Arc::new(super::policy::InteractivePolicy);
    let cancel_token = tokio_util::sync::CancellationToken::new();
    let mut ctx = TurnContext::new();

    let outcome = super::turn_engine::tools::handle_tool_response(
        &reqwest::Client::new(),
        &state,
        &cancel_token,
        &policy,
        &mut ctx,
        Some("tool_arguments_limit"),
        0,
        None,
        None,
        None,
        vec![crate::tools::ToolCallEnvelope {
            call_id: "call_partial_write".to_string(),
            tool_name: "write_to_file".to_string(),
            arguments: serde_json::json!({
                "path": target,
                "content": "this response was truncated"
            }),
        }],
        "",
    )
    .await;

    assert_eq!(
        outcome,
        super::turn_engine::tools::ToolHandlingOutcome::Continue
    );
    assert!(
        !target.exists(),
        "a salvaged truncated call must not execute"
    );
    let history = state.lock().await;
    let result = history
        .history
        .iter()
        .find_map(|message| message.tool_result.as_ref())
        .expect("typed not-executed result");
    assert_eq!(result.error_kind.as_deref(), Some("OutputLimit"));
    assert!(result.retryable);
    assert!(
        history
            .history
            .iter()
            .any(|message| message.content.contains("No tool ran"))
    );
}

#[tokio::test]
async fn repeated_truncated_native_calls_get_split_guidance_then_an_output_limit_stop() {
    // #2118: a cut-off tool call is re-requested with more room and advice to
    // split large writes; a run of them ends with a notice naming the cap.
    let dir = tempfile::tempdir().expect("tempdir");
    let target = dir.path().join("must-not-exist.txt");
    let state = Arc::new(Mutex::new(AppState::new()));
    {
        let mut state = state.lock().await;
        state.auto_confirm = true;
        let api_base_url = state.api_base_url.clone();
        state.record_function_calling_support(&api_base_url, true);
    }
    let policy = Arc::new(super::policy::InteractivePolicy);
    let cancel_token = tokio_util::sync::CancellationToken::new();
    let mut ctx = TurnContext::new();
    ctx.recovery.output_limit.last_request_limit = Some(8192);
    let call = || {
        vec![crate::tools::ToolCallEnvelope {
            call_id: "call_partial_write".to_string(),
            tool_name: "write_to_file".to_string(),
            arguments: serde_json::json!({"path": target, "content": "cut off"}),
        }]
    };
    for attempt in 1..=3 {
        let outcome = super::turn_engine::tools::handle_tool_response(
            &reqwest::Client::new(),
            &state,
            &cancel_token,
            &policy,
            &mut ctx,
            Some("length"),
            0,
            None,
            None,
            None,
            call(),
            "",
        )
        .await;
        assert_eq!(
            outcome,
            super::turn_engine::tools::ToolHandlingOutcome::Continue,
            "attempt {attempt}"
        );
        assert!(ctx.recovery.output_limit.pending.is_some());
        ctx.recovery.output_limit.pending = None;
    }
    assert!(state.lock().await.history.iter().any(|message| {
        message.role == "system" && message.content.contains("write_file_chunk")
    }));

    let outcome = super::turn_engine::tools::handle_tool_response(
        &reqwest::Client::new(),
        &state,
        &cancel_token,
        &policy,
        &mut ctx,
        Some("length"),
        0,
        None,
        None,
        None,
        call(),
        "",
    )
    .await;
    assert_eq!(
        outcome,
        super::turn_engine::tools::ToolHandlingOutcome::Stop
    );
    assert!(!target.exists(), "a truncated call never runs");
    let reason = ctx.lifecycle.stop_reason.clone().expect("stop reason");
    assert!(reason.to_string().starts_with("output_limit:"), "{reason}");
    assert!(
        ctx.response.final_content.contains("8192"),
        "{}",
        ctx.response.final_content
    );
}

#[tokio::test]
async fn output_truncated_text_response_never_executes_salvaged_call() {
    let dir = tempfile::tempdir().expect("tempdir");
    let target = dir.path().join("must-not-exist.txt");
    let state = Arc::new(Mutex::new(AppState::new()));
    {
        let mut state = state.lock().await;
        let api_base_url = state.api_base_url.clone();
        state.record_function_calling_support(&api_base_url, false);
    }
    let policy = Arc::new(super::policy::InteractivePolicy);
    let cancel_token = tokio_util::sync::CancellationToken::new();
    let mut ctx = TurnContext::new();
    ctx.response.final_content = format!(
        "```tool\n{{\"name\":\"write_to_file\",\"arguments\":{{\"path\":{},\"content\":\"truncated\"}}}}\n```",
        serde_json::to_string(&target).expect("serialize path")
    );
    ctx.response.streamed_call_ids = vec!["text_call_partial".to_string()];

    let outcome = super::turn_engine::tools::handle_tool_response(
        &reqwest::Client::new(),
        &state,
        &cancel_token,
        &policy,
        &mut ctx,
        Some("length"),
        0,
        None,
        None,
        None,
        Vec::new(),
        "",
    )
    .await;

    assert_eq!(
        outcome,
        super::turn_engine::tools::ToolHandlingOutcome::Continue
    );
    assert!(
        !target.exists(),
        "a salvaged truncated text call must not execute"
    );
    let history = state.lock().await;
    let result = history
        .history
        .iter()
        .find_map(|message| message.tool_result.as_ref())
        .expect("typed not-executed result");
    assert_eq!(result.error_kind.as_deref(), Some("OutputLimit"));
    assert!(result.retryable);
    assert!(
        history
            .history
            .iter()
            .any(|message| message.content.contains("No tool ran"))
    );
}

#[tokio::test]
async fn grounded_recovery_targets_failed_repair_after_complete_malformed_write_read() {
    let dir = tempfile::tempdir().expect("tempdir");
    let target = dir.path().join("index.html");
    let content = format!(
        "{}l.x += l<think>\n",
        (1..=698)
            .map(|line| format!("line {line}\n"))
            .collect::<String>()
    );
    let state = Arc::new(Mutex::new(AppState::new()));
    {
        let mut state = state.lock().await;
        state.auto_confirm = true;
        state.workspace_root = Some(dir.path().to_path_buf());
        let api_base_url = state.api_base_url.clone();
        state.record_function_calling_support(&api_base_url, true);
    }
    let policy = Arc::new(super::policy::InteractivePolicy);
    let cancel_token = tokio_util::sync::CancellationToken::new();
    let client = reqwest::Client::new();
    let mut ctx = TurnContext::new();

    let native_call = |call_id: &str, tool_name: &str, arguments: serde_json::Value| {
        vec![crate::tools::ToolCallEnvelope {
            call_id: call_id.to_string(),
            tool_name: tool_name.to_string(),
            arguments,
        }]
    };

    ctx.response.final_content = "Writing the artifact.".to_string();
    super::turn_engine::tools::handle_tool_response(
        &client,
        &state,
        &cancel_token,
        &policy,
        &mut ctx,
        Some("tool_calls"),
        0,
        None,
        None,
        None,
        native_call(
            "write-1",
            "write_to_file",
            serde_json::json!({"path": target, "content": content}),
        ),
        "",
    )
    .await;

    ctx.response.final_content =
        "The complete read shows a malformed artifact at the end.".to_string();
    super::turn_engine::tools::handle_tool_response(
        &client,
        &state,
        &cancel_token,
        &policy,
        &mut ctx,
        Some("tool_calls"),
        0,
        None,
        None,
        None,
        native_call("read-1", "view_file", serde_json::json!({"path": target})),
        "",
    )
    .await;

    ctx.response.final_content = "Attempting the repair.".to_string();
    let outcome = super::turn_engine::tools::handle_tool_response(
        &client,
        &state,
        &cancel_token,
        &policy,
        &mut ctx,
        Some("tool_calls"),
        0,
        None,
        None,
        None,
        native_call(
            "repair-1",
            "replace_file_content",
            serde_json::json!({
                "path": target,
                "old_string": "the malformed suffix",
                "new_string": "the malformed suffix"
            }),
        ),
        "",
    )
    .await;

    assert_eq!(
        outcome,
        super::turn_engine::tools::ToolHandlingOutcome::Continue
    );
    assert_eq!(ctx.metrics.grounded_recoveries, 1);
    assert_eq!(ctx.progress.failed_mutations, 1);
    assert_eq!(ctx.progress.consecutive_no_progress, 1);
    let history = state.lock().await;
    let recovery = history
        .history
        .iter()
        .find(|message| message.content.starts_with("[Evidence-based recovery:"))
        .expect("grounded recovery notice");
    assert!(recovery.content.contains("index.html"));
    assert!(
        recovery
            .content
            .contains(&format!("699 lines / {} bytes", content.len()))
    );
    assert!(recovery.content.contains("complete read of lines 1-699"));
    assert!(
        recovery
            .content
            .contains("last repair attempt made no change")
    );
    assert!(
        recovery
            .content
            .contains("old_string and new_string are identical")
    );
    assert!(
        recovery
            .content
            .contains("exactly one narrow replace_file_content")
    );
    assert!(recovery.content.contains("Do not reread the whole file"));
    assert_eq!(std::fs::read_to_string(&target).expect("artifact"), content);
}

#[tokio::test]
async fn mixed_batch_validation_errors_are_isolated_to_the_failing_call_id() {
    let state = Arc::new(Mutex::new(AppState::new()));
    {
        let mut state = state.lock().await;
        state.auto_confirm = true;
        let api_base_url = state.api_base_url.clone();
        state.record_function_calling_support(&api_base_url, true);
    }
    let policy = Arc::new(super::policy::InteractivePolicy);
    let cancel_token = tokio_util::sync::CancellationToken::new();
    let mut ctx = TurnContext::new();

    super::turn_engine::tools::handle_tool_response(
        &reqwest::Client::new(),
        &state,
        &cancel_token,
        &policy,
        &mut ctx,
        Some("tool_calls"),
        0,
        None,
        None,
        None,
        vec![
            crate::tools::ToolCallEnvelope {
                call_id: "call_invalid".to_string(),
                tool_name: "grep".to_string(),
                arguments: serde_json::json!({}),
            },
            crate::tools::ToolCallEnvelope {
                call_id: "call_valid".to_string(),
                tool_name: "grep".to_string(),
                arguments: serde_json::json!({"pattern": "TODO"}),
            },
        ],
        "",
    )
    .await;

    let history = state.lock().await;
    let tool_results = history
        .history
        .iter()
        .filter(|message| message.role == "tool")
        .collect::<Vec<_>>();
    assert_eq!(tool_results.len(), 2);
    assert_eq!(
        tool_results[0].tool_call_id.as_deref(),
        Some("call_invalid")
    );
    assert!(
        tool_results[0]
            .content
            .contains("invalid arguments for 'grep'")
    );
    assert_eq!(tool_results[1].tool_call_id.as_deref(), Some("call_valid"));
    assert!(
        !tool_results[1]
            .content
            .contains("invalid arguments for 'grep'")
    );
}

#[tokio::test]
async fn multi_call_response_executes_all_valid_reads() {
    let state = Arc::new(Mutex::new(AppState::new()));
    {
        let mut state = state.lock().await;
        state.auto_confirm = true;
        let api_base_url = state.api_base_url.clone();
        state.record_function_calling_support(&api_base_url, true);
    }
    let policy = Arc::new(super::policy::InteractivePolicy);
    let cancel_token = tokio_util::sync::CancellationToken::new();
    let mut ctx = TurnContext::new();

    super::turn_engine::tools::handle_tool_response(
        &reqwest::Client::new(),
        &state,
        &cancel_token,
        &policy,
        &mut ctx,
        Some("tool_calls"),
        0,
        None,
        None,
        None,
        vec![
            crate::tools::ToolCallEnvelope {
                call_id: "call_first".to_string(),
                tool_name: "get_time".to_string(),
                arguments: serde_json::json!({}),
            },
            crate::tools::ToolCallEnvelope {
                call_id: "call_second".to_string(),
                tool_name: "grep".to_string(),
                arguments: serde_json::json!({"pattern": "Cargo.toml"}),
            },
        ],
        "",
    )
    .await;

    let state = state.lock().await;
    assert_eq!(ctx.metrics.tool_calls, 2);
    let assistant = state
        .history
        .iter()
        .find(|message| message.role == "assistant" && !message.tool_calls.is_empty())
        .expect("the complete model call list is persisted");
    assert_eq!(
        assistant
            .tool_calls
            .iter()
            .map(|call| call.id.as_str())
            .collect::<Vec<_>>(),
        ["call_first", "call_second"]
    );

    let results = state
        .history
        .iter()
        .filter(|message| message.role == "tool")
        .collect::<Vec<_>>();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].tool_call_id.as_deref(), Some("call_first"));
    assert!(
        results[0]
            .tool_result
            .as_ref()
            .is_some_and(|result| result.success)
    );
    assert_eq!(results[1].tool_call_id.as_deref(), Some("call_second"));
    let second = results[1].tool_result.as_ref().expect("second result");
    assert!(
        second.success,
        "both valid reads execute: {:?}",
        results[1].content
    );

    let messages = history::to_messages(&state.history, "system");
    let rendered_ids = messages
        .iter()
        .filter_map(|message| message.get("tool_call_id").and_then(|id| id.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(rendered_ids, ["call_first", "call_second"]);
}

#[tokio::test]
async fn evidence_recovery_suppresses_duplicate_loop_warnings() {
    // Search an empty directory so the outputs are hermetic: the pattern can
    // never match the test source itself (which contains the pattern text).
    // The directory must live inside the workspace root: an out-of-workspace
    // path is a harness boundary rejection, which rides the infrastructure
    // guard instead of evidence-based recovery (see the boundary test below).
    let state = Arc::new(Mutex::new(AppState::new()));
    let workspace = {
        let state = state.lock().await;
        state
            .effective_workspace_root()
            .expect("test workspace root")
    };
    let dir = tempfile::tempdir_in(workspace).expect("temp search root");
    {
        let mut state = state.lock().await;
        state.auto_confirm = true;
        let api_base_url = state.api_base_url.clone();
        state.record_function_calling_support(&api_base_url, true);
    }
    let policy = Arc::new(super::policy::InteractivePolicy);
    let cancel_token = tokio_util::sync::CancellationToken::new();
    let client = reqwest::Client::new();
    let mut ctx = TurnContext::new();
    let root = dir.path().to_string_lossy().to_string();

    // Four no-match searches with the same pattern but alternating flags:
    // same call category (pre-execution repetition warning parks) and same
    // stagnant output, while the progress ledger hits its recovery streak.
    // The evidence round must speak once, not stack both warnings with it.
    for round in 0..4 {
        super::turn_engine::tools::handle_tool_response(
            &client,
            &state,
            &cancel_token,
            &policy,
            &mut ctx,
            Some("tool_calls"),
            0,
            None,
            None,
            None,
            vec![crate::tools::ToolCallEnvelope {
                call_id: format!("call-nomatch-{round}"),
                tool_name: "grep".to_string(),
                arguments: serde_json::json!({
                    "pattern": "zzz-no-match-xyz-123",
                    "path": root,
                    "ignore_case": round % 2 == 0,
                }),
            }],
            "",
        )
        .await;
        ctx.response.final_content = "Searching for references.".to_string();
    }

    let history = state.lock().await;
    let bodies: Vec<&str> = history
        .history
        .iter()
        .map(|message| message.content.as_str())
        .collect();
    assert!(
        bodies
            .iter()
            .any(|body| body.contains("[Evidence-based recovery:")),
        "expected evidence recovery after repeated no-progress searches: {bodies:?}"
    );
    assert!(
        !bodies
            .iter()
            .any(|body| body.contains("has repeated 4 times")),
        "parked call-repetition warning must not stack with recovery: {bodies:?}"
    );
    assert!(
        !bodies
            .iter()
            .any(|body| body.contains("last 4 tool results")),
        "output-stagnation warning must not stack with recovery: {bodies:?}"
    );
}

#[tokio::test]
async fn workspace_boundary_rejections_skip_evidence_recovery_for_infra_guard() {
    // An out-of-workspace search path is a harness guardrail hit, not model
    // stagnation: repeated rejections must reset the evidence streak and stop
    // via the workspace-boundary guard instead of injecting evidence-based
    // recovery.
    let dir = tempfile::tempdir().expect("temp search root outside workspace");
    let state = Arc::new(Mutex::new(AppState::new()));
    {
        let mut state = state.lock().await;
        state.auto_confirm = false;
        state.config.sandbox_mode = crate::config::SandboxMode::WorkspaceWrite;
        let api_base_url = state.api_base_url.clone();
        state.record_function_calling_support(&api_base_url, true);
    }
    let policy = Arc::new(super::policy::InteractivePolicy);
    let cancel_token = tokio_util::sync::CancellationToken::new();
    let client = reqwest::Client::new();
    let mut ctx = TurnContext::new();
    let root = dir.path().to_string_lossy().to_string();
    assert!(
        !root.starts_with(
            state
                .lock()
                .await
                .effective_workspace_root()
                .expect("test workspace root")
                .to_string_lossy()
                .as_ref()
        ),
        "this scenario needs an out-of-workspace path"
    );

    for round in 0..4 {
        super::turn_engine::tools::handle_tool_response(
            &client,
            &state,
            &cancel_token,
            &policy,
            &mut ctx,
            Some("tool_calls"),
            0,
            None,
            None,
            None,
            vec![crate::tools::ToolCallEnvelope {
                call_id: format!("call-boundary-{round}"),
                tool_name: "grep".to_string(),
                arguments: serde_json::json!({
                    "pattern": "zzz-no-match-xyz-123",
                    "path": root,
                }),
            }],
            "",
        )
        .await;
        ctx.response.final_content = "Searching for references.".to_string();
    }

    let history = state.lock().await;
    let bodies: Vec<&str> = history
        .history
        .iter()
        .map(|message| message.content.as_str())
        .collect();
    assert!(
        !bodies
            .iter()
            .any(|body| body.contains("[Evidence-based recovery:")),
        "boundary rejections must not inject evidence recovery: {bodies:?}"
    );
    assert!(
        bodies
            .iter()
            .any(|body| body.contains("[Workspace boundary guard:")),
        "boundary rejections must stop via the infra guard: {bodies:?}"
    );
    assert_eq!(ctx.metrics.evidence_recoveries, 0);
    assert_eq!(ctx.progress.ledger.no_progress_streak(), 0);
}

/// The exact tool-call sequence of session `01a0eed269ed-7000-9dda-4d40-4d4078a20034`,
/// the turn that motivated #1513: a `grep` the workspace boundary refused, the
/// shell fallback that correctly reported no match, a second differently-worded
/// shell search that also reported no match, and the read that ended the turn.
/// Captured verbatim from the session's `turn.progress`, `turn.recovery_decision`
/// and `turn.summary` events so the regression is pinned to a real session
/// rather than a synthetic one. Issue #1532.
const LOOP_GUARD_SESSION_REPLAY: &str = include_str!("fixtures/loop_guard_session_01a0eed26.json");

#[derive(serde::Deserialize)]
struct SessionReplayBaselineSummary {
    tool_rounds: usize,
    tool_calls: usize,
    evidence_recoveries: usize,
    tokens_used: u64,
}

#[derive(serde::Deserialize)]
struct SessionReplayBaselineProgress {
    tool: String,
    streak: usize,
}

#[derive(serde::Deserialize)]
struct SessionReplayBaseline {
    turn_summary: SessionReplayBaselineSummary,
    turn_progress: Vec<SessionReplayBaselineProgress>,
    recovery_decisions: Vec<serde_json::Value>,
}

#[derive(serde::Deserialize)]
struct SessionReplayArtifact {
    filename: String,
    /// Server names the turn's last shell read has to surface for the loop
    /// guard to see the round as `new_information`.
    terminal_shell_markers: Vec<String>,
    content: String,
}

#[derive(serde::Deserialize)]
struct SessionReplayExpected {
    success: bool,
    exit_code: Option<i32>,
}

#[derive(serde::Deserialize)]
struct SessionReplayCall {
    tool: String,
    arguments: serde_json::Value,
    expected: SessionReplayExpected,
}

#[derive(serde::Deserialize)]
struct SessionReplayRound {
    baseline_tool: String,
    calls: Vec<SessionReplayCall>,
}

#[derive(serde::Deserialize)]
struct SessionReplayFixture {
    source_session_id: String,
    task: String,
    baseline: SessionReplayBaseline,
    artifact: SessionReplayArtifact,
    rounds: Vec<SessionReplayRound>,
}

/// Substitute the replay's placeholders in a recorded call's arguments. The
/// baseline referenced an absolute path under the user's `~/.config`; the replay
/// points at a temp artifact outside the workspace root so the read-only tools
/// are refused by the same boundary guardrail the baseline hit.
fn expand_replay_arguments(value: &serde_json::Value, artifact: &str, workspace: &str) -> String {
    serde_json::to_string(value)
        .expect("replay arguments serialize")
        .replace("{{ARTIFACT}}", artifact)
        .replace("{{WORKSPACE}}", workspace)
}

#[tokio::test]
async fn session_01a0eed26_replay_injects_no_evidence_recovery() {
    // Replays the #1513 session against the current loop guard. The baseline
    // pooled four failures across two different tools into a `repeated_failure`
    // streak of 4 and paid two evidence-recovery injections for it; the fix must
    // make that exact sequence produce no recovery at all.
    let fixture: SessionReplayFixture =
        serde_json::from_str(LOOP_GUARD_SESSION_REPLAY).expect("replay fixture parses");
    let baseline = &fixture.baseline.turn_summary;
    let baseline_report = format!(
        "session {}: {} tool_rounds, {} tool_calls, {} evidence_recoveries, {} tokens",
        fixture.source_session_id,
        baseline.tool_rounds,
        baseline.tool_calls,
        baseline.evidence_recoveries,
        baseline.tokens_used,
    );
    assert_eq!(
        fixture.baseline.recovery_decisions.len(),
        baseline.evidence_recoveries,
        "fixture must record the baseline's recovery injections"
    );
    assert_eq!(
        fixture
            .baseline
            .turn_progress
            .iter()
            .map(|entry| entry.streak)
            .max()
            .expect("baseline has progress records"),
        4,
        "fixture must record the baseline streak that crossed the recovery threshold"
    );
    assert!(
        fixture
            .baseline
            .turn_progress
            .iter()
            .any(|entry| entry.tool == "grep")
            && fixture
                .baseline
                .turn_progress
                .iter()
                .any(|entry| entry.tool == "run_command"),
        "the pooled baseline streak must span both tools, otherwise this replay is not the regression"
    );

    let state = Arc::new(Mutex::new(AppState::new()));
    let workspace = {
        let state = state.lock().await;
        state
            .effective_workspace_root()
            .expect("test workspace root")
    };
    let workspace_dir = tempfile::tempdir_in(&workspace).expect("temp workspace read root");
    // The baseline artifact lived in the harness tool-output directory, i.e.
    // outside the workspace root, which is precisely why `grep` could not
    // reach it. Keep that property: the replay must not be rescued by making
    // the path readable.
    let artifact_dir = tempfile::tempdir().expect("temp artifact root outside workspace");
    let artifact = artifact_dir
        .path()
        .join(&fixture.artifact.filename)
        .to_string_lossy()
        .to_string();
    assert!(
        !artifact.starts_with(workspace.to_string_lossy().as_ref()),
        "the replay artifact must stay outside the workspace root"
    );
    std::fs::write(&artifact, &fixture.artifact.content).expect("write replay artifact");

    {
        let mut state = state.lock().await;
        state.auto_confirm = true;
        // Shell rounds use native process permissions so sandbox backend behavior
        // cannot change the recorded artifact/exit-status inputs. Native searches
        // explicitly opt into the original workspace boundary below.
        state.config.sandbox_mode = crate::config::SandboxMode::Trusted;
        let api_base_url = state.api_base_url.clone();
        state.record_function_calling_support(&api_base_url, true);
    }
    let policy = Arc::new(super::policy::InteractivePolicy);
    let cancel_token = tokio_util::sync::CancellationToken::new();
    let client = reqwest::Client::new();
    let mut ctx = TurnContext::new();
    let mut streaks = Vec::new();
    let mut calls = 0usize;

    for round in &fixture.rounds {
        {
            let mut state = state.lock().await;
            let native_search = round.calls.iter().all(|call| call.tool == "grep");
            state.auto_confirm = !native_search;
            state.config.sandbox_mode = if native_search {
                crate::config::SandboxMode::WorkspaceWrite
            } else {
                crate::config::SandboxMode::Trusted
            };
        }
        let envelopes = round
            .calls
            .iter()
            .map(|call| {
                calls += 1;
                crate::tools::ToolCallEnvelope {
                    call_id: format!("replay-{calls}"),
                    tool_name: call.tool.clone(),
                    arguments: serde_json::from_str(&expand_replay_arguments(
                        &call.arguments,
                        &artifact,
                        &workspace_dir.path().to_string_lossy(),
                    ))
                    .expect("replay arguments parse"),
                }
            })
            .collect::<Vec<_>>();
        super::turn_engine::tools::handle_tool_response(
            &client,
            &state,
            &cancel_token,
            &policy,
            &mut ctx,
            Some("tool_calls"),
            0,
            None,
            None,
            None,
            envelopes,
            "",
        )
        .await;
        ctx.response.final_content = format!(
            "Continuing the {} recovery for {}.",
            round.baseline_tool, fixture.task
        );
        streaks.push((
            round.baseline_tool.clone(),
            ctx.progress.ledger.no_progress_streak(),
            ctx.recovery.infrastructure_failures.streak(),
        ));
    }

    // The fixture must still reproduce the baseline's decision inputs, otherwise
    // a green result would prove nothing about that session.
    assert_eq!(
        calls, baseline.tool_calls,
        "replay must issue every tool call the baseline turn made ({baseline_report})"
    );
    let history = state.lock().await;
    let recorded = history
        .history
        .iter()
        .filter(|message| message.tool_result.is_some())
        .collect::<Vec<_>>();
    assert_eq!(
        recorded.len(),
        calls,
        "every replayed call must produce one recorded result ({baseline_report})"
    );
    for (call, message) in fixture
        .rounds
        .iter()
        .flat_map(|round| &round.calls)
        .zip(&recorded)
    {
        let record = message
            .tool_result
            .as_ref()
            .expect("filtered to recorded tool results");
        assert_eq!(
            record.tool_name, call.tool,
            "replayed tool order must match the baseline"
        );
        // Success and exit status are asserted together and the recorded body
        // rides along in the message: a host that diverges (no `grep`, a
        // different default, a denied read) can only be told apart from a real
        // loop-guard regression by the text the tool actually returned.
        assert!(
            record.success == call.expected.success && record.exit_code == call.expected.exit_code,
            "replayed result for {} must match the baseline (expected success={} exit_code={:?}, recorded success={} exit_code={:?} error_kind={:?}): {}",
            call.tool,
            call.expected.success,
            call.expected.exit_code,
            record.success,
            record.exit_code,
            record.error_kind,
            message.content,
        );
    }

    // The turn's last shell read is what the guard actually classifies: it is
    // `new_information` because of the server names in the body, not because
    // of its exit status. Assert those names, so a host whose shell could not
    // read the artifact (a sandbox that hides the temp directory, a missing
    // `grep`) fails on the round that lost its evidence rather than on an exit
    // code that a coincidence could satisfy.
    let terminal = recorded
        .last()
        .expect("the replay recorded a terminal round");
    for marker in &fixture.artifact.terminal_shell_markers {
        assert!(
            terminal.content.contains(marker.as_str()),
            "the terminal shell read must surface '{marker}' for the guard to see new information, got: {}",
            terminal.content
        );
    }

    let bodies = history
        .history
        .iter()
        .map(|message| message.content.as_str())
        .collect::<Vec<_>>();
    assert!(
        !bodies
            .iter()
            .any(|body| body.contains("[Evidence-based recovery:")),
        "the #1513 session must not reproduce an evidence-recovery injection ({baseline_report}): {bodies:?}"
    );
    assert_eq!(
        ctx.metrics.evidence_recoveries, 0,
        "evidence_recoveries must go {} -> 0 for this exact sequence ({baseline_report})",
        baseline.evidence_recoveries
    );

    // The streak must not form across the tool boundary: the boundary
    // rejections reset it, and the two distinct shell searches restart it
    // instead of pooling into one corroborated miss.
    let shell_streaks = streaks
        .iter()
        .filter(|(tool, _, _)| tool == "run_command")
        .map(|(_, ledger, _)| *ledger)
        .collect::<Vec<_>>();
    assert_eq!(
        shell_streaks,
        vec![1, 1, 0],
        "distinct failing searches must restart the streak, not extend it (baseline pooled them into 3, 4)"
    );
    let boundary_round = streaks
        .iter()
        .find(|(tool, _, _)| tool == "grep")
        .expect("the baseline hit the workspace boundary");
    assert_eq!(
        boundary_round.1, 0,
        "workspace-boundary rejections must reset the progress streak, not feed it"
    );
    assert_eq!(
        boundary_round.2, 2,
        "both boundary rejections must be counted by the infrastructure guard"
    );
    let peak = streaks
        .iter()
        .map(|(_, ledger, _)| *ledger)
        .max()
        .expect("replay recorded streaks");
    assert!(
        peak < ctx.progress.evidence_recovery_streak,
        "peak streak {peak} reached the recovery threshold {}",
        ctx.progress.evidence_recovery_streak
    );
    assert_eq!(
        ctx.progress.ledger.no_progress_streak(),
        0,
        "the turn ended on a meaningful read"
    );
    assert_eq!(
        ctx.recovery.infrastructure_failures.streak(),
        0,
        "the terminal successful read cleared the infrastructure streak"
    );
}

#[tokio::test]
async fn session_01a0eed26_replay_still_catches_a_genuinely_repeating_search() {
    // The counterpart to the replay above, and the guard against the per-tool
    // scoping being too permissive: the *same* failing search from the #1513
    // session, re-issued verbatim against the same unreachable artifact, is a
    // real loop and must still reach evidence-based recovery. The replay is
    // green precisely because the baseline changed its search between rounds.
    let fixture: SessionReplayFixture =
        serde_json::from_str(LOOP_GUARD_SESSION_REPLAY).expect("replay fixture parses");
    let looping = &fixture.rounds[2].calls[0];

    let state = Arc::new(Mutex::new(AppState::new()));
    let workspace = {
        let state = state.lock().await;
        state
            .effective_workspace_root()
            .expect("test workspace root")
    };
    let artifact_dir = tempfile::tempdir().expect("temp artifact root outside workspace");
    let artifact = artifact_dir
        .path()
        .join(&fixture.artifact.filename)
        .to_string_lossy()
        .to_string();
    std::fs::write(&artifact, &fixture.artifact.content).expect("write replay artifact");

    {
        let mut state = state.lock().await;
        state.auto_confirm = true;
        let api_base_url = state.api_base_url.clone();
        state.record_function_calling_support(&api_base_url, true);
    }
    let policy = Arc::new(super::policy::InteractivePolicy);
    let cancel_token = tokio_util::sync::CancellationToken::new();
    let client = reqwest::Client::new();
    let mut ctx = TurnContext::new();
    let mut peak = 0usize;
    let mut recoveries = 0usize;

    for round in 0..ctx.progress.evidence_recovery_streak + 1 {
        super::turn_engine::tools::handle_tool_response(
            &client,
            &state,
            &cancel_token,
            &policy,
            &mut ctx,
            Some("tool_calls"),
            0,
            None,
            None,
            None,
            vec![crate::tools::ToolCallEnvelope {
                call_id: format!("looping-{round}"),
                tool_name: looping.tool.clone(),
                arguments: serde_json::from_str(&expand_replay_arguments(
                    &looping.arguments,
                    &artifact,
                    &workspace.to_string_lossy(),
                ))
                .expect("replay arguments parse"),
            }],
            "",
        )
        .await;
        ctx.response.final_content = "Still looking for the server registry.".to_string();
        peak = peak.max(ctx.progress.ledger.no_progress_streak());
        recoveries = ctx.metrics.evidence_recoveries;
    }

    assert!(
        peak >= ctx.progress.evidence_recovery_streak,
        "repeating one failing search must reach the recovery threshold {} (peak {peak})",
        ctx.progress.evidence_recovery_streak
    );
    assert!(
        recoveries > 0,
        "a genuinely repeating search must inject a recovery notice"
    );
    let history = state.lock().await;
    assert!(
        history
            .history
            .iter()
            .any(|message| message.content.contains("[Evidence-based recovery:")),
        "a genuinely repeating search must still produce a model-visible recovery notice"
    );
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
fn truncated_batch_summary_names_selectively_dropped_calls_without_arguments() {
    let kept = vec![crate::tools::ToolCall {
        name: "grep".to_owned(),
        arguments: serde_json::json!({"pattern": "needle"}),
        call_id: None,
    }];
    let dropped = vec![crate::tools::ToolCall {
        name: "write_to_file".to_owned(),
        arguments: serde_json::json!({"path": "secret.txt", "content": "secret"}),
        call_id: None,
    }];

    let summary = super::truncated_batch_summary_with_dropped(&kept, &dropped);

    assert!(
        summary.contains("1 were dropped (write_to_file)"),
        "got: {summary}"
    );
    assert!(!summary.contains("secret.txt"), "got: {summary}");
    assert!(!summary.contains("secret"), "got: {summary}");
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
            pending: false,
            command: None,
            exit_code: None,
            truncated: false,
            completeness: rustcode_core::ToolResultCompleteness::Complete,
            replayed: false,
            error_kind: None,
            retryable: false,
            command_status: None,
        },
        None,
    );

    assert!(result.metadata.success);
    assert_eq!(result.metadata.exit_code, None);
    assert!(!result.metadata.truncated);
}

#[test]
fn failed_mutation_does_not_claim_requested_path_changed() {
    let result = tool_result_from_execution(
        "write_to_file",
        &serde_json::json!({"path": "public/sounds/projectile_shot.txt"}),
        crate::tools::ToolExecutionOutput::failure_with_kind(
            "error: write was interrupted".to_string(),
            crate::tools::ToolErrorKind::CommandFailed,
            false,
        ),
        None,
    );

    assert!(!result.metadata.success);
    assert!(result.metadata.changed_paths.is_empty());
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
            pending: false,
            command: None,
            exit_code: Some(23),
            truncated: false,
            completeness: rustcode_core::ToolResultCompleteness::Complete,
            replayed: false,
            error_kind: Some(crate::tools::ToolErrorKind::CommandFailed),
            retryable: false,
            command_status: None,
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
            pending: false,
            command: None,
            exit_code: None,
            truncated: false,
            completeness: rustcode_core::ToolResultCompleteness::Complete,
            replayed: false,
            error_kind: Some(crate::tools::ToolErrorKind::Internal),
            retryable: false,
            command_status: None,
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
    // Diagnostics are reported next to a successful edit, not as its status.
    assert!(result.metadata.success);
    assert_eq!(result.metadata.error_kind, None);
    assert!(!result.metadata.retryable);
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
fn malformed_native_arguments_are_bounded_for_validation() {
    let raw = format!("{{\"pattern\":\"{}", "repeat/".repeat(4_000));
    let value = parse_native_tool_arguments(&raw);
    let invalid = &value["_invalid_arguments"];
    assert_eq!(invalid["original_bytes"], raw.len());
    assert_eq!(invalid["truncated"], true);
    assert!(invalid["preview"].as_str().unwrap().len() <= 1024);
    assert!(value.to_string().len() < 2_000);
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

    // 4 or more messages: the reminder goes in a request-local context tail,
    // never into a conversation message the prefix cache could freeze.
    let mut msgs2: Vec<serde_json::Value> = vec![
        serde_json::json!({"role": "system", "content": "sys"}),
        serde_json::json!({"role": "user", "content": "hello"}),
        serde_json::json!({"role": "assistant", "content": "hi"}),
        serde_json::json!({"role": "user", "content": "tell me a story"}),
    ];
    inject_system_reminder(&mut msgs2);
    assert_eq!(msgs2.len(), 5);
    assert_eq!(msgs2[3]["content"], "tell me a story");
    let tail = msgs2[4]["content"].as_str().unwrap();
    assert!(tail.starts_with("<rustcode_context>"));
    assert!(tail.contains("REMINDER: Follow the configured tool protocol"));

    // An existing context tail takes the reminder instead.
    let mut msgs3 = msgs2[..4].to_vec();
    attach_request_context_tail(&mut msgs3, "# Environment");
    inject_system_reminder(&mut msgs3);
    assert_eq!(msgs3.len(), 5);
    assert!(
        msgs3[4]["content"]
            .as_str()
            .unwrap()
            .contains("REMINDER: Follow the configured tool protocol")
    );
}

// Session 01a16b37: the reminder was appended to the last message, the prefix
// cache reused it for the rest of the turn, and the next turn rebuilt the
// prefix without it, so its first request missed the provider cache from that
// message on.
#[test]
fn reminder_never_enters_the_reused_request_prefix() {
    let history = vec![
        serde_json::json!({"role": "system", "content": "sys"}),
        serde_json::json!({"role": "user", "content": "build it"}),
        serde_json::json!({"role": "assistant", "content": "reading"}),
        serde_json::json!({"role": "user", "content": "<tool_result>\nview_file: 1: x\n</tool_result>"}),
    ];
    let mut cache = RequestPrefixCache::default();
    let mut first = cache.compose(&history, "# Environment");
    inject_system_reminder(&mut first);
    cache.record(history.clone(), &first, "# Environment");

    let mut grown = history.clone();
    grown.push(serde_json::json!({"role": "assistant", "content": "editing"}));
    let mut second = cache.compose(&grown, "# Environment");
    inject_system_reminder(&mut second);

    // A cold rebuild (the next turn) renders the same prefix.
    let mut cold = RequestPrefixCache::default().compose(&grown, "# Environment");
    inject_system_reminder(&mut cold);
    assert_eq!(second, cold);
    let reminders = serde_json::to_string(&second)
        .unwrap()
        .matches("REMINDER: Follow the configured tool protocol")
        .count();
    assert_eq!(reminders, 1);
}

#[test]
fn test_parse_multimodal_content_plain() {
    let val = parse_multimodal_content("Hello world");
    assert_eq!(val, serde_json::Value::String("Hello world".to_string()));
}

#[test]
fn test_parse_multimodal_content_expands_complete_paste_payload_once() {
    let payload = "first --> second";
    let marker = format!(
        "before <!--PASTE:{}:{}--> after",
        payload.chars().count(),
        payload
    );
    let val = parse_multimodal_content(&marker);
    assert_eq!(
        val,
        serde_json::Value::String("before first --> second after".to_string())
    );
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

#[test]
fn test_parse_multimodal_content_downscales_large_screenshots() {
    use base64::{Engine as _, engine::general_purpose};
    let dir = tempfile::tempdir().expect("temporary directory");
    // Noise compresses badly, so the PNG is well past the send-as-is size.
    let path = dir.path().join("shot.png");
    let mut seed = 1u32;
    let image = image::RgbImage::from_fn(3000, 1800, |_, _| {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        image::Rgb([(seed >> 24) as u8, (seed >> 16) as u8, (seed >> 8) as u8])
    });
    image.save(&path).expect("write png");
    let original = std::fs::metadata(&path).unwrap().len() as usize;

    let text = format!("see ![image](file://{})", path.display());
    let first = parse_multimodal_content(&text);
    let url = first[1]["image_url"]["url"].as_str().expect("image part");
    let data = url
        .strip_prefix("data:image/jpeg;base64,")
        .expect("re-encoded as JPEG");
    let bytes = general_purpose::STANDARD.decode(data).unwrap();
    assert!(bytes.len() < original);
    let sent = image::load_from_memory(&bytes).unwrap();
    assert_eq!(sent.width().max(sent.height()), 1568);
    // A replay of the same attachment gets the same bytes.
    assert_eq!(parse_multimodal_content(&text), first);
}

#[tokio::test]
async fn test_confirm_and_execute_bypassed() {
    // Write into a throwaway workspace: the repository checkout must not
    // gain (or lose) files because the suite ran.
    let workspace = tempfile::tempdir().expect("temporary workspace");
    let state = Arc::new(Mutex::new(AppState::new_with_workspace_session(
        workspace.path(),
        Some("confirm-bypass-test"),
    )));
    state.lock().await.agent_mode = crate::config::AgentMode::Build;
    let cancel_token = tokio_util::sync::CancellationToken::new();
    let client = reqwest::Client::new();
    let args = serde_json::json!({
        "path": "test_bypass.txt",
        "content": "bypassed content",
        "overwrite": true
    });

    let (result, _, _) = confirm_and_execute(
        &client,
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

    assert_eq!(
        std::fs::read_to_string(workspace.path().join("test_bypass.txt"))
            .ok()
            .as_deref(),
        Some("bypassed content"),
        "the write lands in the session workspace"
    );
    assert!(!std::path::Path::new("sandbox/test_bypass.txt").exists());
}

#[tokio::test]
async fn question_prompt_transitions_invalidate_render_metrics_once() {
    let state = Arc::new(Mutex::new(AppState::new()));
    let cancel_token = tokio_util::sync::CancellationToken::new();
    let initial_revision = state.lock().await.render_revision;
    let state_for_task = Arc::clone(&state);
    let cancel_for_task = cancel_token.clone();
    let task = tokio::spawn(async move {
        super::tool_exec::ask_user_question(
            &state_for_task,
            &cancel_for_task,
            &serde_json::json!({
                "question": "Continue?",
                "options": ["Proceed", "Cancel"]
            }),
        )
        .await
    });

    let awaiting_revision = tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            let s = state.lock().await;
            if s.pending_question.is_some() {
                break s.render_revision;
            }
            drop(s);
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("question prompt should become visible");
    assert_eq!(
        awaiting_revision,
        initial_revision.wrapping_add(1),
        "publishing a question should invalidate one render revision"
    );

    let response = state
        .lock()
        .await
        .question_response
        .take()
        .expect("question response channel");
    response
        .send("Proceed".to_owned())
        .expect("question task alive");
    task.await.expect("question task should finish");

    let s = state.lock().await;
    assert!(s.pending_question.is_none());
    assert_eq!(s.status, AppStatus::Streaming);
    assert_eq!(
        s.render_revision,
        awaiting_revision.wrapping_add(1),
        "clearing a question should invalidate one additional render revision"
    );
}

#[tokio::test]
async fn question_answered_after_a_delay_is_user_wait_not_tool_time() {
    let delay = std::time::Duration::from_millis(300);
    let state = Arc::new(Mutex::new(AppState::new()));
    let turn_started = std::time::Instant::now();
    state.lock().await.current_turn_started_at = Some(turn_started);
    let answerer = tokio::spawn({
        let state = Arc::clone(&state);
        async move {
            let response = loop {
                if let Some(response) = state.lock().await.question_response.take() {
                    break response;
                }
                tokio::task::yield_now().await;
            };
            tokio::time::sleep(delay).await;
            response
                .send("User selected: Proceed".to_owned())
                .expect("question task alive");
        }
    });

    let client = reqwest::Client::new();
    let cancel_token = tokio_util::sync::CancellationToken::new();
    let mut compile_dirty = false;
    let mut compile_cache = None;
    let mut user_wait = std::time::Duration::ZERO;
    let batch_started = std::time::Instant::now();
    let results = execute_tool_batch(
        &client,
        &state,
        &cancel_token,
        &[crate::tools::ToolCall {
            name: "ask_question".to_owned(),
            arguments: serde_json::json!({
                "question": "Continue?",
                "options": ["Proceed", "Cancel"]
            }),
            call_id: Some("question".to_owned()),
        }],
        true,
        &None,
        &mut compile_dirty,
        &mut compile_cache,
        &mut user_wait,
        None,
    )
    .await;
    let batch_us = crate::benchmark::elapsed_us(batch_started);
    answerer.await.expect("answer task should finish");

    assert!(results[0].metadata.success);
    assert!(user_wait >= delay, "{user_wait:?}");
    let delay_us = crate::benchmark::duration_us(delay);
    assert!(
        results[0].metadata.execution_us < delay_us,
        "the wait must not be execution time: {}",
        results[0].metadata.execution_us
    );

    // What the turn records for this batch (`turn.performance`).
    let mut performance = crate::benchmark::TurnPerformance {
        wall_us: batch_us,
        ..Default::default()
    };
    performance.record_tool_batch(
        batch_us,
        results[0].metadata.execution_us,
        crate::benchmark::duration_us(user_wait),
    );
    assert!(performance.user_wait_us >= delay_us);
    // The wait is taken out of tool time exactly. Comparing what is left
    // against the delay flaked when a loaded test run starved the batch.
    assert_eq!(
        performance.tool_wall_us + performance.user_wait_us,
        batch_us
    );
    assert!(performance.tool_work_us < performance.user_wait_us);
    assert_eq!(performance.harness_us(), 0);

    // The `Working` row clock and `Worked for` read this start: the wait is
    // taken out of it.
    let clock_start = state
        .lock()
        .await
        .current_turn_started_at
        .expect("turn clock");
    assert!(clock_start >= turn_started + delay);
    assert!(clock_start <= std::time::Instant::now());
}

#[tokio::test]
async fn tool_confirmation_cleanup_invalidates_render_metrics_once() {
    let mut app = AppState::new();
    app.agent_mode = crate::config::AgentMode::Build;
    let state = Arc::new(Mutex::new(app));
    let cancel_token = tokio_util::sync::CancellationToken::new();
    let client = reqwest::Client::new();
    let initial_revision = state.lock().await.render_revision;
    let state_for_task = Arc::clone(&state);
    let cancel_for_task = cancel_token.clone();
    let task = tokio::spawn(async move {
        super::tool_exec::confirm_and_execute(
            &client,
            &state_for_task,
            &cancel_for_task,
            "write_to_file",
            &serde_json::json!({
                "path": "sandbox/render-metrics-confirmation.txt",
                "content": "content"
            }),
            "write_to_file",
            false,
            None,
            None,
        )
        .await
    });

    let awaiting_revision = tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            let s = state.lock().await;
            if s.pending_tool_confirmation.is_some() {
                break s.render_revision;
            }
            drop(s);
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("confirmation should become visible");
    assert_eq!(
        awaiting_revision,
        initial_revision.wrapping_add(1),
        "publishing a tool confirmation should invalidate one render revision"
    );

    let response = state
        .lock()
        .await
        .tool_confirmation_response
        .take()
        .expect("confirmation response channel");
    response
        .send(crate::app::ToolConfirmationResponse::Deny)
        .expect("confirmation task alive");
    let _ = task.await.expect("confirmation task should finish");

    let s = state.lock().await;
    assert!(s.pending_tool_confirmation.is_none());
    assert_eq!(s.status, AppStatus::Streaming);
    assert_eq!(
        s.render_revision,
        awaiting_revision.wrapping_add(1),
        "clearing a tool confirmation should invalidate one additional render revision"
    );
}

#[tokio::test]
async fn interactive_confirmation_publication_invalidates_render_metrics_once() {
    use crate::network::policy::TurnPolicy;

    let mut app = AppState::new();
    app.agent_mode = crate::config::AgentMode::Build;
    let state = Arc::new(Mutex::new(app));
    let initial_revision = state.lock().await.render_revision;
    let calls = vec![crate::tools::ToolCall {
        name: "write_to_file".to_owned(),
        arguments: serde_json::json!({
            "path": "sandbox/render-metrics.txt",
            "content": "content"
        }),
        call_id: None,
    }];
    let state_for_task = Arc::clone(&state);
    let task = tokio::spawn(async move {
        super::policy::InteractivePolicy
            .should_approve(&state_for_task, &calls)
            .await
    });

    let awaiting_revision = tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            let s = state.lock().await;
            if s.pending_tool_confirmation.is_some() {
                break s.render_revision;
            }
            drop(s);
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("confirmation should become visible");
    assert_eq!(
        awaiting_revision,
        initial_revision.wrapping_add(1),
        "publishing a confirmation should invalidate one render revision"
    );

    let response = state
        .lock()
        .await
        .tool_confirmation_response
        .take()
        .expect("confirmation response channel");
    response
        .send(crate::app::ToolConfirmationResponse::Deny)
        .expect("confirmation task alive");
    assert!(!task.await.expect("confirmation task should finish"));
}

#[tokio::test]
async fn test_compact_history_strips_thinking_blocks() {
    // #985 immutable-history contract: `<think>` blocks stay verbatim in
    // storage and are stripped only at request-render time
    // (`history::to_messages`), so compaction must not rewrite them.
    let mut history = vec![
        crate::app::ChatMessage::new(
            "assistant",
            "<think>\nThinking about files...\n</think>\nHere is the answer",
        ),
        crate::app::ChatMessage::new("tool", "tool output"),
    ];
    compact_history_to_budget(&mut history, 5000).await;
    assert_eq!(
        history[0].content,
        "<think>\nThinking about files...\n</think>\nHere is the answer"
    );
    assert_eq!(history[1].content, "tool output");
    let rendered = history::to_messages(&history, "system");
    assert!(
        rendered
            .iter()
            .any(|m| m.get("content").and_then(|c| c.as_str()) == Some("Here is the answer")),
        "rendered request must strip the think block without touching storage"
    );
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
fn test_tool_signature_normalizes_equivalent_shell_reads() {
    let view = serde_json::json!({"path": "js/main.js", "start_line": 1, "end_line": 40});
    let sed = serde_json::json!({"command": "sed -n '1,40p' js/main.js"});
    let awk = serde_json::json!({"command": "awk 'NR>=1 && NR<=40' js/main.js"});
    let unrelated = serde_json::json!({"command": "sed -n '41,80p' js/main.js"});

    assert_eq!(
        tool_signature("view_file", &view),
        tool_signature("run_command", &sed)
    );
    assert_eq!(
        tool_signature("run_command", &sed),
        tool_signature("run_command", &awk)
    );
    assert_ne!(
        tool_signature("run_command", &sed),
        tool_signature("run_command", &unrelated)
    );
}

#[test]
fn shell_file_reads_are_repeatable_read_only_calls() {
    for command in [
        "cat js/main.js",
        "sed -n '1,40p' js/main.js",
        "awk 'NR>=1 && NR<=40' js/main.js",
    ] {
        let args = serde_json::json!({"command": command});
        assert!(
            loop_detect::is_read_only_call("run_command", &args),
            "expected read-only classification for {command}"
        );
    }
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
    assert!(is_mutating_tool("write_file_chunk"));
    assert!(!is_mutating_tool("todo_write"));
}

#[path = "tests/tool_replay.rs"]
mod tool_replay;

#[path = "tests/budget.rs"]
mod budget;

#[path = "tests/loop_detection.rs"]
mod loop_detection;
