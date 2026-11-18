//! Tool execution and replay: real diffs, read caching and failed-session replays.

use super::*;

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
    if !crate::tools::exec::sandbox::runtime_tests_available() {
        return;
    }
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
    let state = Arc::new(Mutex::new(AppState::new()));
    state.lock().await.workspace_root = Some(dir.path().to_path_buf());
    let file = dir.path().join("large.txt");
    let content: String = (1..=1000).map(|line| format!("line {line}\n")).collect();
    std::fs::write(&file, content).expect("write");
    let path = file.to_string_lossy().to_string();

    let truncated = run_one_tool_with_state(
        &state,
        test_tool_call("view_file", serde_json::json!({"path": path})),
    )
    .await;
    assert!(truncated.metadata.success);
    assert!(truncated.metadata.truncated);

    let targeted = run_one_tool_with_state(
        &state,
        test_tool_call(
            "view_file",
            serde_json::json!({"path": path, "start_line": 1, "end_line": 1}),
        ),
    )
    .await;
    assert!(targeted.metadata.success);
    assert!(!targeted.metadata.truncated);
}

#[tokio::test]
async fn control_plane_tool_does_not_stall_while_reading_workspace_root() {
    // A workspace-local skill root keeps this hermetic: discovery no longer
    // reads the developer's real home skill directory.
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    let skill_dir = workspace.path().join(".rustcode/skills/stall-check");
    std::fs::create_dir_all(&skill_dir).expect("skill dir");
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: stall-check\ndescription: Skill load timing check\n---\nBody",
    )
    .expect("skill file");
    let state = Arc::new(Mutex::new(AppState::new_with_workspace_session(
        workspace.path(),
        None,
    )));

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        run_one_tool_with_state(
            &state,
            test_tool_call("use_skill", serde_json::json!({"name": "stall-check"})),
        ),
    )
    .await
    .expect("use_skill execution stalled while resolving workspace root");

    assert!(result.metadata.success, "got: {}", result.content);
}

#[tokio::test]
async fn repeated_failed_read_reexecutes_and_preserves_structured_failure() {
    let state = Arc::new(Mutex::new(AppState::new()));
    let dir = tempfile::tempdir().expect("tempdir");
    let missing = dir.path().join("missing").to_string_lossy().to_string();
    let call = test_tool_call("list_directory", serde_json::json!({"path": missing}));

    let first = run_one_tool_with_state(&state, call.clone()).await;
    let repeated = run_one_tool_with_state(&state, call).await;

    assert!(!first.metadata.success, "got: {}", first.content);
    assert!(!repeated.metadata.success, "got: {}", repeated.content);
    assert!(!repeated.metadata.replayed);
    assert_eq!(repeated.metadata.error_kind, first.metadata.error_kind);
    assert!(!repeated.content.contains("Unchanged read replay"));
}

#[tokio::test]
async fn repeated_truncated_read_reexecutes_with_structured_truncation() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = Arc::new(Mutex::new(AppState::new()));
    state.lock().await.workspace_root = Some(dir.path().to_path_buf());
    let file = dir.path().join("large.txt");
    let content: String = (1..=850).map(|line| format!("{line}\n")).collect();
    std::fs::write(&file, content).expect("write");
    let path = file.to_string_lossy().to_string();
    let call = test_tool_call("view_file", serde_json::json!({"path": path}));

    let first = run_one_tool_with_state(&state, call.clone()).await;
    let repeated = run_one_tool_with_state(&state, call).await;

    assert!(first.metadata.truncated, "got: {}", first.content);
    assert!(!repeated.metadata.replayed);
    assert!(
        repeated.metadata.truncated,
        "replay lost structured truncation: {}",
        repeated.content
    );
    assert!(!repeated.content.contains("Unchanged read replay"));
    assert!(repeated.content.contains("[tool_result_incomplete:"));
    assert_eq!(repeated.metadata.completeness, first.metadata.completeness);
}

#[tokio::test]
async fn repeated_unchanged_small_view_file_replays_cached_body() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = Arc::new(Mutex::new(AppState::new()));
    state.lock().await.workspace_root = Some(dir.path().to_path_buf());
    let file = dir.path().join("small.txt");
    std::fs::write(&file, "first line\nrecoverable body\n").expect("write");
    let path = file.to_string_lossy().to_string();
    let call = test_tool_call("view_file", serde_json::json!({"path": path}));

    let first = run_one_tool_with_state(&state, call.clone()).await;
    let repeated = run_one_tool_with_state(&state, call).await;

    assert!(first.metadata.success, "got: {}", first.content);
    assert!(!first.metadata.replayed);
    assert!(repeated.metadata.success, "got: {}", repeated.content);
    assert!(repeated.metadata.replayed);
    assert!(!repeated.content.contains("recoverable body"));
    assert!(repeated.content.contains("Unchanged read replay"));
    assert!(repeated.content.len() <= 50 * 1024);
    assert_eq!(repeated.metadata.inspection, first.metadata.inspection);
}

#[tokio::test]
async fn view_file_subrange_reuses_complete_cached_read() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = Arc::new(Mutex::new(AppState::new()));
    state.lock().await.workspace_root = Some(dir.path().to_path_buf());
    let file = dir.path().join("source.js");
    std::fs::write(&file, "first\nsecond\nthird\n").expect("write");
    let path = file.to_string_lossy().to_string();
    let full = test_tool_call(
        "view_file",
        serde_json::json!({"path": path, "start_line": 1, "end_line": 3}),
    );
    let subrange = test_tool_call(
        "view_file",
        serde_json::json!({"path": path, "start_line": 1, "end_line": 2}),
    );

    let first = run_one_tool_with_state(&state, full).await;
    let repeated = run_one_tool_with_state(&state, subrange).await;

    assert!(first.metadata.success, "got: {}", first.content);
    assert!(repeated.metadata.success, "got: {}", repeated.content);
    assert!(repeated.metadata.replayed, "got: {}", repeated.content);
    assert!(repeated.content.contains("Lines 1 to 2 of 3"));
    assert!(repeated.content.contains("2: second"));
    assert!(!repeated.content.contains("Unchanged read replay"));
    assert!(!repeated.content.contains("3: third"));
    assert_eq!(
        repeated
            .metadata
            .inspection
            .as_ref()
            .and_then(|inspection| inspection.requested_range.clone()),
        Some(rustcode_core::InspectionRange {
            start: Some(1),
            end: Some(2),
        })
    );
    assert_eq!(
        repeated
            .metadata
            .inspection
            .as_ref()
            .and_then(|inspection| inspection.returned_range.clone()),
        Some(rustcode_core::InspectionRange {
            start: Some(1),
            end: Some(2),
        })
    );
}

#[tokio::test]
async fn view_file_subrange_without_end_line_reexecutes_when_cache_is_finite() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = Arc::new(Mutex::new(AppState::new()));
    state.lock().await.workspace_root = Some(dir.path().to_path_buf());
    let file = dir.path().join("source.js");
    std::fs::write(&file, "first\nsecond\nthird\n").expect("write");
    let path = file.to_string_lossy().to_string();
    let full = test_tool_call(
        "view_file",
        serde_json::json!({"path": path, "start_line": 1, "end_line": 3}),
    );
    let through_end = test_tool_call(
        "view_file",
        serde_json::json!({"path": path, "start_line": 2}),
    );

    let first = run_one_tool_with_state(&state, full).await;
    let repeated = run_one_tool_with_state(&state, through_end).await;

    assert!(first.metadata.success, "got: {}", first.content);
    assert!(repeated.metadata.success, "got: {}", repeated.content);
    assert!(!repeated.metadata.replayed, "got: {}", repeated.content);
    assert!(repeated.content.contains("2: second"));
    assert!(repeated.content.contains("3: third"));
}

#[tokio::test]
async fn view_file_subrange_with_different_content_offset_reexecutes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = Arc::new(Mutex::new(AppState::new()));
    state.lock().await.workspace_root = Some(dir.path().to_path_buf());
    let file = dir.path().join("source.js");
    std::fs::write(&file, "first\nsecond\nthird\n").expect("write");
    let path = file.to_string_lossy().to_string();
    let full = test_tool_call(
        "view_file",
        serde_json::json!({"path": path, "start_line": 1, "end_line": 3}),
    );
    let offset = test_tool_call(
        "view_file",
        serde_json::json!({
            "path": path,
            "content_offset": 6,
            "start_line": 2,
            "end_line": 3
        }),
    );

    let first = run_one_tool_with_state(&state, full).await;
    let repeated = run_one_tool_with_state(&state, offset).await;

    assert!(first.metadata.success, "got: {}", first.content);
    assert!(repeated.metadata.success, "got: {}", repeated.content);
    assert!(!repeated.metadata.replayed, "got: {}", repeated.content);
    assert!(repeated.content.contains("2: second"));
    assert!(repeated.content.contains("3: third"));
}

#[tokio::test]
async fn repeated_unchanged_large_cached_view_file_stays_bounded() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = Arc::new(Mutex::new(AppState::new()));
    state.lock().await.workspace_root = Some(dir.path().to_path_buf());
    let file = dir.path().join("source.rs");
    let content: String = (1..=800)
        .map(|line| format!("line {line}: {}\n", "x".repeat(13)))
        .collect();
    std::fs::write(&file, content).expect("write");
    let path = file.to_string_lossy().to_string();
    let call = test_tool_call(
        "view_file",
        serde_json::json!({"path": path, "start_line": 1, "end_line": 800}),
    );

    let first = run_one_tool_with_state(&state, call.clone()).await;
    let repeated = run_one_tool_with_state(&state, call).await;

    assert!(first.metadata.success, "got: {}", first.content);
    assert!(!first.metadata.replayed);
    assert!(
        first.content.len() > 20_000,
        "fixture did not exercise the regression"
    );
    assert!(
        first.content.len() <= REPLAYABLE_READ_LIMIT,
        "fixture must remain cacheable: {} bytes",
        first.content.len()
    );
    assert!(repeated.metadata.success, "got: {}", repeated.content);
    assert!(repeated.metadata.replayed);
    assert!(
        repeated.content.len() <= 50 * 1024,
        "replayed read exceeded the context limit: {} bytes",
        repeated.content.len()
    );
    assert!(!repeated.content.contains("line 200:"));
    assert!(repeated.content.contains("fingerprint="));
    assert!(repeated.content.contains("Lines 1 to 800"));
    assert!(repeated.content.contains("earlier result"));
    assert!(repeated.content.contains("start_line/end_line"));
    assert_eq!(
        repeated
            .metadata
            .inspection
            .as_ref()
            .and_then(|inspection| inspection.returned_range.clone()),
        first
            .metadata
            .inspection
            .as_ref()
            .and_then(|inspection| inspection.returned_range.clone())
    );

    // History stores the original body and the compact replay as separate
    // durable results; request rendering must retain the canonical first body
    // and the replay metadata.
    let first_history = tool_result_history_message(first.clone(), None);
    let repeated_history = tool_result_history_message(repeated.clone(), None);
    assert!(first_history.content.contains("line 200:"));
    assert!(!repeated_history.content.contains("line 200:"));
    assert!(
        repeated_history
            .tool_result
            .as_ref()
            .expect("metadata")
            .replayed
    );
    let rendered = history::to_messages(&[first_history, repeated_history], "system");
    let rendered = serde_json::to_string(&rendered).expect("render history");
    assert!(rendered.contains("line 200:"));
    assert!(rendered.contains("Unchanged read replay"));
}

#[tokio::test]
async fn repeated_over_limit_failed_read_reexecutes() {
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
    assert!(!repeated.metadata.replayed);
    assert!(!repeated.content.contains("Unchanged read replay"));
}

#[tokio::test]
async fn repeated_over_limit_truncated_read_reexecutes_with_recovery_artifact() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = Arc::new(Mutex::new(AppState::new()));
    state.lock().await.workspace_root = Some(dir.path().to_path_buf());
    let file = dir.path().join("large.txt");
    let content: String = (1..=850)
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
    assert!(!repeated.metadata.replayed);
    assert!(repeated.metadata.full_output_artifact.is_some());
    assert!(!repeated.content.contains("Unchanged read replay"));
}

#[tokio::test]
async fn normal_replacement_final_diff_is_real_and_has_correct_line_numbers() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = Arc::new(Mutex::new(AppState::new()));
    state.lock().await.workspace_root = Some(dir.path().to_path_buf());
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
    let result = run_one_tool_with_state(&state, call).await;

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
async fn approved_text_file_deletion_records_removed_lines() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = Arc::new(Mutex::new(AppState::new()));
    state.lock().await.workspace_root = Some(dir.path().to_path_buf());
    let file = dir.path().join("remove.txt");
    std::fs::write(&file, "first line\nsecond line\n").expect("write source file");

    let result = run_one_tool_with_state(
        &state,
        test_tool_call(
            "delete_file",
            serde_json::json!({"path": file.to_string_lossy()}),
        ),
    )
    .await;

    assert!(result.metadata.success, "got: {}", result.content);
    let diff = result.diff.expect("deletion should retain a text diff");
    assert!(diff.contains("-first line"), "got: {diff}");
    assert!(diff.contains("-second line"), "got: {diff}");
    assert!(!file.exists(), "the approved deletion still executes");
}

#[test]
fn delete_file_is_not_read_for_the_pre_approval_confirmation_preview() {
    assert!(get_diff_preview("delete_file", &serde_json::json!({"path":"secret.txt"})).is_none());
}

#[tokio::test]
async fn approved_text_copy_diff_covers_new_and_overwritten_files() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = Arc::new(Mutex::new(AppState::new()));
    state.lock().await.workspace_root = Some(dir.path().to_path_buf());
    let source = dir.path().join("source.txt");
    let target = dir.path().join("target.txt");
    std::fs::write(&source, "copied line\n").expect("write source");

    let call = || {
        test_tool_call(
            "copy_file",
            serde_json::json!({"src":source.to_string_lossy(), "dest":target.to_string_lossy()}),
        )
    };
    let created = run_one_tool_with_state(&state, call()).await;
    assert!(created.metadata.success, "got: {}", created.content);
    let created_diff = created
        .diff
        .expect("new text copy should have an added-line diff");
    assert!(created_diff.contains("+copied line"), "got: {created_diff}");

    std::fs::write(&source, "replacement line\n").expect("update source");
    let overwritten = run_one_tool_with_state(&state, call()).await;
    assert!(overwritten.metadata.success, "got: {}", overwritten.content);
    let overwritten_diff = overwritten.diff.expect("overwrite should have a text diff");
    assert!(
        overwritten_diff.contains("-copied line"),
        "got: {overwritten_diff}"
    );
    assert!(
        overwritten_diff.contains("+replacement line"),
        "got: {overwritten_diff}"
    );
}

#[tokio::test]
async fn approved_text_chunk_write_records_the_appended_lines() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = Arc::new(Mutex::new(AppState::new()));
    state.lock().await.workspace_root = Some(dir.path().to_path_buf());
    let file = dir.path().join("chunk.txt");
    std::fs::write(&file, "first").expect("write initial content");

    let result = run_one_tool_with_state(
        &state,
        test_tool_call(
            "write_file_chunk",
            serde_json::json!({
                "path": file.to_string_lossy(),
                "content": " second",
                "offset": 5,
                "expected_size": 5,
            }),
        ),
    )
    .await;

    assert!(result.metadata.success, "got: {}", result.content);
    let diff = result.diff.expect("text chunk write should have a diff");
    assert!(diff.contains("-first"), "got: {diff}");
    assert!(diff.contains("+first second"), "got: {diff}");
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
    let state = Arc::new(Mutex::new(AppState::new()));
    state.lock().await.workspace_root = Some(dir.path().to_path_buf());
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
    let result = run_one_tool_with_state(&state, call).await;

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
    let state = Arc::new(Mutex::new(AppState::new()));
    state.lock().await.workspace_root = Some(dir.path().to_path_buf());
    let file = dir.path().join("state.rs");
    std::fs::write(&file, "let status = Idle;\n").expect("write");
    let path = file.to_string_lossy().to_string();
    let args = serde_json::json!({
        "path": path,
        "old_string": "let status = Idle;",
        "new_string": "let status = Active;",
    });

    let first =
        run_one_tool_with_state(&state, test_tool_call("replace_file_content", args.clone())).await;
    assert!(
        first.diff.is_some(),
        "the first, real change must have a diff"
    );

    let second =
        run_one_tool_with_state(&state, test_tool_call("replace_file_content", args)).await;
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
    let state = Arc::new(Mutex::new(AppState::new()));
    state.lock().await.workspace_root = Some(dir.path().to_path_buf());
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
    let result = run_one_tool_with_state(&state, call).await;

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
    let state = Arc::new(Mutex::new(AppState::new()));
    state.lock().await.workspace_root = Some(dir.path().to_path_buf());
    let file = dir.path().join("state.rs");
    std::fs::write(&file, "let status = Idle;\n").expect("write");
    let path = file.to_string_lossy().to_string();
    let args = serde_json::json!({
        "path": path,
        "old_string": "let status = Idle;",
        "new_string": "let status = Active;",
    });

    let first =
        run_one_tool_with_state(&state, test_tool_call("replace_file_content", args.clone())).await;
    assert!(
        first.diff.is_some(),
        "the first, real change must have a diff"
    );

    let second =
        run_one_tool_with_state(&state, test_tool_call("replace_file_content", args)).await;
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
fn skill_routing_hint_stays_in_dynamic_tail_and_preserves_static_system_prompt() {
    let skills = [crate::skills::SkillMetadata {
        name: "solidtime".to_string(),
        description: "Solidtime workflow".to_string(),
        path: std::path::PathBuf::from("/skills/solidtime"),
        triggers: Vec::new(),
        keywords: Vec::new(),
        priority: 0,
    }];
    let hint = crate::skills::skill_routing_hint("Check Solidtime this week.", &skills, &[])
        .expect("named skill route");
    let mut dynamic_context = "# Environment\nworkspace".to_string();
    prepend_skill_routing_hint(&mut dynamic_context, Some(&hint));

    let mut messages = history::to_messages(
        &[ChatMessage::new("user", "Check Solidtime this week.")],
        "static system prompt",
    );
    messages::attach_request_context_tail(&mut messages, &dynamic_context);

    assert_eq!(messages[0]["content"], "static system prompt");
    let tail = messages.last().expect("dynamic context tail")["content"]
        .as_str()
        .expect("tail text");
    assert!(tail.contains("use_skill"));
    assert!(tail.find("Priority skill route") < tail.find("# Environment"));
}

#[tokio::test]
async fn repeated_use_skill_returns_actionable_success_after_same_turn_load() {
    let state = Arc::new(Mutex::new(AppState::new()));
    {
        let mut state = state.lock().await;
        state
            .history
            .push(ChatMessage::new("user", "Use synthetic-skill"));
        state.history.push(
            ChatMessage::new(
                "tool",
                "use_skill: <skill_content name=\"synthetic-skill\">\ninstructions",
            )
            .with_tool_result(crate::app::ToolResultRecord {
                workspace_generation: None,
                workspace_epoch: None,
                evidence_hash: None,
                tool_name: "use_skill".to_string(),
                success: true,
                ..Default::default()
            }),
        );
    }

    let result = run_one_tool_with_state(
        &state,
        test_tool_call("use_skill", serde_json::json!({"name": "synthetic-skill"})),
    )
    .await;

    assert!(result.metadata.success, "got: {}", result.content);
    assert!(!result.metadata.replayed);
    assert!(result.metadata.inspection.is_none());
    assert!(result.content.contains("already loaded"));
    assert!(
        result
            .content
            .contains("do not call `use_skill` for it again")
    );
}

// Session 01a130ea (#1970): a skill loaded in the previous user turn is still
// in context, so a new turn gets the short pointer, not the full body again.
#[tokio::test]
async fn use_skill_from_the_previous_turn_returns_a_pointer_not_the_body() {
    let state = Arc::new(Mutex::new(AppState::new()));
    {
        let mut state = state.lock().await;
        state.history.push(ChatMessage::new("user", "Log my hours"));
        state.history.push(
            ChatMessage::new(
                "tool",
                "use_skill: <skill_content name=\"synthetic-skill\">\ninstructions",
            )
            .with_tool_result(crate::app::ToolResultRecord {
                tool_name: "use_skill".to_string(),
                success: true,
                ..Default::default()
            }),
        );
        state.history.push(ChatMessage::new("assistant", "Logged."));
        state
            .history
            .push(ChatMessage::new("user", "Now check the active timer"));
    }

    let result = run_one_tool_with_state(
        &state,
        test_tool_call("use_skill", serde_json::json!({"name": "synthetic-skill"})),
    )
    .await;

    assert!(result.metadata.success, "got: {}", result.content);
    assert!(
        result.content.contains("already loaded"),
        "{}",
        result.content
    );
    assert!(
        !result.content.contains("<skill_content"),
        "{}",
        result.content
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
    let history = vec![
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

    // #985: storage is append-only. Nothing rewrites retained messages;
    // the request render is what drops the scratchpads.

    // The request render still excludes the scratchpads while preserving the
    // visible answers, so provider pressure is handled without touching
    // history.
    let messages = crate::network::history::to_messages(&history, "system");
    let rendered = serde_json::to_string(&messages).unwrap();
    assert!(
        !rendered.contains("deep thoughts"),
        "historical assistant reasoning must not be sent to provider"
    );
    assert!(
        !rendered.contains("more thoughts"),
        "historical assistant reasoning must not be sent to provider"
    );
    assert!(
        rendered.contains("Short answer 1") && rendered.contains("Short answer 2"),
        "visible answers must be preserved"
    );
    assert!(
        history[1].content.contains("<think>") && history[3].content.contains("<think>"),
        "stored scratchpads stay verbatim for the transcript"
    );
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
        supports_reasoning_effort: Some(true),
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
    state.replace_current_response("<think>\nThinking about function...");
    state.current_thought_started_at = Some(std::time::Instant::now());
    std::sync::Arc::make_mut(&mut state.live_tool_calls).push(crate::app::LiveToolCall::new(
        "call-temp",
        None,
        "run_command",
        "Bash",
        "cargo check",
    ));

    // Simulate killing/stopping turn: live tool calls & transient thought buffer are cleared
    state.clear_live_tool_calls();
    state.clear_current_response();
    state.current_thought_started_at = None;
    state.status = crate::app::AppStatus::Idle;

    assert!(state.live_tool_calls.is_empty());
    assert!(state.current_response.is_empty());
    assert_eq!(state.status, crate::app::AppStatus::Idle);
    assert_eq!(state.history.len(), 1);
}

#[test]
fn test_continuation_assistant_message_strips_massive_think_traces() {
    use rustcode_tool_protocol::text::format_continuation_assistant_message;

    // 1. Completed massive reasoning trace followed by visible prose
    let massive_think = format!(
        "<think>\n{}\n</think>\nI will now run the test suite.\n```tool\n{{\"name\": \"run_command\"}}\n```",
        "deep reasoning ".repeat(2000)
    );
    let continuation = format_continuation_assistant_message(&massive_think);
    assert!(
        !continuation.contains("<think>"),
        "Completed think blocks must be stripped from continuation assistant message"
    );
    assert!(continuation.contains("I will now run the test suite."));
    assert!(continuation.contains("```tool"));
    assert!(
        continuation.len() < 500,
        "Continuation assistant message must be bounded and compact"
    );

    // 2. Pure reasoning (no visible text outside think)
    let pure_think = format!("<think>\n{}\n</think>", "planning ".repeat(1500));
    let continuation_pure = format_continuation_assistant_message(&pure_think);
    assert_eq!(continuation_pure, "(completed reasoning scratchpad)");

    // 3. Unclosed think trace that was cut off mid-thought
    let cut_off_think = format!(
        "<think>\n{}\nThinking about line 42",
        "thinking ".repeat(1000)
    );
    let continuation_cut_off = format_continuation_assistant_message(&cut_off_think);
    assert!(continuation_cut_off.contains("<think>"));
    assert!(continuation_cut_off.contains("Thinking about line 42"));
    assert!(
        continuation_cut_off.len() <= 1200,
        "Unclosed think block tail must be bounded"
    );
}

#[test]
fn test_continuation_nudges_are_category_aware() {
    use rustcode_tool_protocol::text::continuation_nudge_for_category;

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
        "Your tool call syntax was incomplete, so no tool was executed. Continue from the exact cutoff without restarting or repeating earlier arguments. Keep the remainder bounded; use a smaller follow-up edit if needed."
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
    history.push(ChatMessage::new(
        "assistant",
        "Understood. I will not touch generated files.",
    ));

    // Turn 5: Discovered architecture & failed approach
    history.push(ChatMessage::new(
        "user",
        "Check if we can use the old parser.",
    ));
    history.push(ChatMessage::new(
        "assistant",
        "Decision: Found parser in `src/parser/legacy.rs`. It does not support async.",
    ));
    let mut failed_tool = ChatMessage::new(
        "tool",
        "run_command: cargo test\nerror: compilation failed\nexit code: 1",
    );
    failed_tool.tool_result = Some(crate::app::ToolResultRecord {
        workspace_generation: None,
        workspace_epoch: None,
        evidence_hash: None,
        tool_name: "run_command".to_string(),
        arguments_hash: "hash".to_string(),
        success: false,
        pending: false,
        command: None,
        exit_code: Some(1),
        error_kind: Some("command_failed".to_string()),
        changed_paths: vec!["src/parser/legacy.rs".to_string()],
        truncated: false,
        payload_truncated: false,
        completeness: rustcode_core::ToolResultCompleteness::Complete,
        full_output_artifact: None,
        replayed: false,
        retryable: false,
        inspection: None,
        command_status: None,
        mcp_app: None,
    });
    history.push(failed_tool);

    // Add multiple subsequent turns to trigger compaction
    for i in 6..=35 {
        history.push(ChatMessage::new(
            "user",
            format!("Step {i} working on alternative parser"),
        ));
        history.push(ChatMessage::new(
            "assistant",
            format!("Working on step {i} implementation"),
        ));
    }

    // Extract structured session memory
    let memory = StructuredSessionMemory::extract_from_history(&history);
    let record = memory.format_record(4000);

    // Verify key constraints and facts are preserved
    assert!(
        record.contains("Never modify files under src/generated/!"),
        "User constraint must be captured in memory"
    );
    assert!(
        record.contains("src/parser/legacy.rs"),
        "Modified/inspected file must be captured"
    );
    assert!(
        record.contains("Goal: Implement feature X"),
        "Initial goal must be captured"
    );

    // Perform structured compaction
    let initial_len = history.len();
    let compacted = compact_with_structured_memory(&mut history, 10, 4000);
    assert!(compacted, "Compaction should succeed");
    assert!(history.len() < initial_len);

    // Verify the compacted history retains the structured memory block at message 0
    assert_eq!(history[0].role, "system");
    assert!(
        history[0]
            .content
            .contains("Never modify files under src/generated/!")
    );

    // Simulate another 10 turns and a SECOND compaction pass (Turn 45)
    for i in 36..=45 {
        history.push(ChatMessage::new("user", format!("Follow up step {i}")));
        history.push(ChatMessage::new(
            "assistant",
            format!("Follow up step {i} completed"),
        ));
    }
    let second_compacted = compact_with_structured_memory(&mut history, 10, 4000);
    assert!(second_compacted, "Second compaction should succeed");

    // Verify the constraint from Turn 1 is STILL preserved after multiple compaction passes!
    assert!(
        history[0]
            .content
            .contains("Never modify files under src/generated/!"),
        "Turn 1 constraint must survive multiple compaction passes!"
    );
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
    assert_eq!(
        with_schema.tool_schema_tokens,
        estimate_tool_schema_tokens(&schema)
    );
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

    let preflight = calculate_preflight_budget(text_prompt, &[], &history, "", 0, &budget);
    let incorrectly_double_counted =
        calculate_preflight_budget(text_prompt, &schema, &history, "", 0, &budget);

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
fn test_reasoning_loop_recovery_is_bounded() {
    assert_eq!(
        reasoning_loop_recovery_action(0),
        LoopRecoveryAction::Recover
    );
    assert_eq!(
        reasoning_loop_recovery_action(1),
        LoopRecoveryAction::Recover
    );
    assert_eq!(
        reasoning_loop_recovery_action(2),
        LoopRecoveryAction::Recover
    );
    assert_eq!(
        reasoning_loop_recovery_action(3),
        LoopRecoveryAction::Recover
    );
    assert_eq!(
        reasoning_loop_recovery_action(MAX_READ_ONLY_LOOP_RECOVERY_ROUNDS),
        LoopRecoveryAction::ForceFinal
    );
}

#[test]
fn test_reasoning_loop_is_cut_off_behavior() {
    // When finish_reason is "reasoning_loop", is_cut_off must be false so runner returns collected response
    assert!(!is_cut_off(
        "<think>repetitive thoughts",
        Some("reasoning_loop")
    ));
    assert!(!is_cut_off(
        "<think>thoughts</think>",
        Some("reasoning_loop")
    ));
    assert!(!is_cut_off(
        "<think>reasoning reached its client budget</think>",
        Some("reasoning_budget")
    ));
}

#[test]
fn test_turn_context_reasoning_benchmark_summary() {
    let mut ctx = TurnContext::new();
    assert_eq!(ctx.recovery.reasoning_loops_detected, 0);
    assert_eq!(ctx.recovery.reasoning_recovery_attempts, 0);

    ctx.recovery.reasoning_loops_detected = 1;
    ctx.recovery.reasoning_recovery_attempts = 1;

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
        let sentence = format!(
            "Analyzing architectural step {step} with specialized component handler_{step}.\n"
        );
        assert_eq!(
            detector.feed_chunk(&sentence),
            loop_detect::ReasoningLoopStatus::Ok
        );
    }

    // 3. Cross-turn plan repetition with confirmed ledger stagnation.
    // A bare repeated plan without stagnation is legitimate re-inspection
    // (#984), so this path goes through evidence with streaks.
    let plan = "Plan: Inspect all routes in src/routes.rs and verify handler types.";
    assert_eq!(
        detector.record_turn_evidence(&loop_detect::TurnEvidence {
            reasoning: plan,
            target_files: &[],
            made_progress: false,
            had_edits: false,
            tool_count: 1,
            no_progress_streak: 1,
        }),
        loop_detect::ReasoningLoopStatus::Ok
    );
    assert!(matches!(
        detector.record_turn_evidence(&loop_detect::TurnEvidence {
            reasoning: plan,
            target_files: &[],
            made_progress: false,
            had_edits: false,
            tool_count: 1,
            no_progress_streak: 2,
        }),
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
        let thought = format!(
            "Step {i}: Inspecting module {file} to understand its public API and dependencies."
        );
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
            0 => format!(
                "Phase {step}: Evaluating caching layer invariants in src/cache/storage_{step}.rs.\n\n"
            ),
            1 => format!(
                "Phase {step}: Analyzing thread pool scheduler policies for runtime_{step}.\n\n"
            ),
            2 => format!(
                "Phase {step}: Reviewing serialization schema definitions in src/proto/message_{step}.rs.\n\n"
            ),
            _ => format!(
                "Phase {step}: Verifying cryptographic checksum verification logic in src/crypto/auth_{step}.rs.\n\n"
            ),
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
fn test_adversarial_6_reasoning_recovery_eventually_forces_final() {
    assert_eq!(
        reasoning_loop_recovery_action(0),
        LoopRecoveryAction::Recover
    );
    assert_eq!(
        reasoning_loop_recovery_action(1),
        LoopRecoveryAction::Recover
    );
    assert_eq!(
        reasoning_loop_recovery_action(2),
        LoopRecoveryAction::Recover
    );
    assert_eq!(
        reasoning_loop_recovery_action(3),
        LoopRecoveryAction::Recover
    );
    assert_eq!(
        reasoning_loop_recovery_action(u8::MAX),
        LoopRecoveryAction::ForceFinal
    );
    assert!(REASONING_LOOP_RECOVERY_PROMPT.contains("safe read-only tool"));
    assert!(REASONING_LOOP_RECOVERY_PROMPT.contains("trustworthy target"));
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
        (
            "src/network/stream_request.rs",
            "Examining stream_request for SSE chunk parsing and backpressure.",
        ),
        (
            "src/network/runner.rs",
            "Checking how runner collects chunks and manages timeouts.",
        ),
        (
            "src/network/turn_engine.rs",
            "Synthesizing turn execution lifecycle for final explanation.",
        ),
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

#[test]
fn vision_profile_error_names_bad_value_and_options() {
    let available = vec![
        "deepseek-v4.1-flash".to_string(),
        "gemini-3.6-flash".to_string(),
    ];
    let stale = super::vision_profile_missing_error(Some("qwen-3.8:27b-3bit"), &available);
    assert!(stale.contains("qwen-3.8:27b-3bit"), "{stale}");
    assert!(stale.contains("deepseek-v4.1-flash"), "{stale}");
    assert!(stale.contains("gemini-3.6-flash"), "{stale}");

    let missing = super::vision_profile_missing_error(None, &available);
    assert!(
        missing.contains("no vision_model is configured"),
        "{missing}"
    );

    let empty = super::vision_profile_missing_error(Some(""), &available);
    assert!(empty.contains("no vision_model is configured"), "{empty}");
}

#[tokio::test]
async fn thinking_router_decision_reaches_main_request_and_failure_keeps_defaults() {
    use crate::config::{ModelProfile, ThinkingRouterConfig};
    use crate::network::stream::StreamBuffer;
    use std::time::Duration;
    use tokio_util::sync::CancellationToken;

    for (decision, expected_thinking) in [
        (
            serde_json::json!({"model":"decider-4b","choices":[{"text":"B"}]}),
            false,
        ),
        (serde_json::json!({"error":"inference unavailable"}), true),
    ] {
        let (router_url, router_request, router_release) = gated_json_server(decision).await;
        let release = tokio::spawn(async move {
            router_request
                .await
                .expect("router request reached endpoint");
            router_release.send(()).unwrap();
        });
        let (provider_url, provider_request) = streaming_provider_server().await;
        let endpoint = format!("{provider_url}/v1/chat/completions");
        let mut app = crate::app::AppState::new();
        app.api_base_url = endpoint.clone();
        app.model_name = "router-main-test".into();
        app.config.models = vec![ModelProfile {
            name: app.model_name.clone(),
            url: endpoint.clone(),
            model: app.model_name.clone(),
            context_window: Some(8192),
            enable_thinking: Some(true),
            reasoning_effort: Some("high".into()),
            supports_reasoning_effort: Some(true),
            thinking_budget: Some(1024),
            supports_thinking_budget: Some(true),
            thinking_router: Some(ThinkingRouterConfig {
                url: format!("{router_url}/v1/completions"),
                model: "decider-4b".into(),
                env_key: "PATH".into(),
                timeout_ms: 1000,
            }),
            ..Default::default()
        }];
        app.record_function_calling_support(&endpoint, false);
        app.history.push(ChatMessage::new("user", "What is 2 + 2?"));
        let state = Arc::new(tokio::sync::Mutex::new(app));
        let (sender, _receiver) = ui_adapter::AgentUiEventSender::channel();
        let buffer = Arc::new(tokio::sync::Mutex::new(StreamBuffer::new()));
        let context = tokio::time::timeout(
            Duration::from_secs(5),
            ui_adapter::run_agent_turn_with_events_for_acp(
                &reqwest::Client::new(),
                &state,
                &CancellationToken::new(),
                &Arc::new(AcpPromptTestPolicy),
                &buffer,
                "What is 2 + 2?".into(),
                sender,
            ),
        )
        .await
        .expect("routed turn must finish");
        assert_eq!(
            context.response.final_content,
            "wakeup request reached provider"
        );
        release.await.unwrap();
        let request = provider_request.await.unwrap();
        let offset = request.windows(4).position(|b| b == b"\r\n\r\n").unwrap() + 4;
        let payload: serde_json::Value = serde_json::from_slice(&request[offset..]).unwrap();
        assert_eq!(payload["enable_thinking"], expected_thinking);
        assert_eq!(
            payload["chat_template_kwargs"]["enable_thinking"],
            expected_thinking
        );
        assert_eq!(payload.get("reasoning_effort").is_some(), expected_thinking);
        assert_eq!(payload.get("thinking_budget").is_some(), expected_thinking);
    }
}

/// A provider that answers each request with the next scripted SSE body
/// (repeating the last one) and records every request body.
async fn scripted_provider(
    scripts: Vec<String>,
) -> (String, Arc<std::sync::Mutex<Vec<serde_json::Value>>>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind scripted provider");
    let address = listener.local_addr().expect("provider address");
    let bodies = Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorded = Arc::clone(&bodies);
    tokio::spawn(async move {
        let mut served = 0usize;
        while let Ok((mut socket, _)) = listener.accept().await {
            let mut request = Vec::new();
            let mut buffer = [0_u8; 8_192];
            let body_start = loop {
                let read = socket.read(&mut buffer).await.unwrap_or(0);
                if read == 0 {
                    break None;
                }
                request.extend_from_slice(&buffer[..read]);
                let Some(header_end) = request.windows(4).position(|w| w == b"\r\n\r\n") else {
                    continue;
                };
                let headers = String::from_utf8_lossy(&request[..header_end]).to_string();
                let length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    })
                    .unwrap_or(0);
                if request.len() >= header_end + 4 + length {
                    break Some((header_end + 4, length));
                }
            };
            let Some((start, length)) = body_start else {
                continue;
            };
            if let Ok(body) = serde_json::from_slice(&request[start..start + length]) {
                recorded.lock().unwrap().push(body);
            }
            let script = scripts[served.min(scripts.len() - 1)].clone();
            served += 1;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                script.len(),
                script
            );
            let _ = socket.write_all(response.as_bytes()).await;
            let _ = socket.shutdown().await;
        }
    });
    (format!("http://{address}/v1/chat/completions"), bodies)
}

fn chat_sse(text: &str, finish_reason: &str) -> String {
    format!(
        "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
        serde_json::json!({"choices": [{"delta": {"content": text}}]}),
        serde_json::json!({"choices": [{"delta": {}, "finish_reason": finish_reason}]}),
    )
}

async fn scripted_turn(
    scripts: Vec<String>,
    profile: crate::config::ModelProfile,
) -> (TurnContext, Vec<serde_json::Value>, Vec<ChatMessage>) {
    let (endpoint, bodies) = scripted_provider(scripts).await;
    let mut app = AppState::new();
    app.api_base_url = endpoint.clone();
    app.model_name = "output-limit-test".into();
    app.config.models = vec![crate::config::ModelProfile {
        name: app.model_name.clone(),
        url: endpoint.clone(),
        model: app.model_name.clone(),
        api_protocol: Some(crate::config::ApiProtocol::ChatCompletions),
        context_window: Some(128_000),
        ..profile
    }];
    app.record_function_calling_support(&endpoint, false);
    app.history
        .push(ChatMessage::new("user", "Write the long design summary."));
    let state = Arc::new(Mutex::new(app));
    let policy = Arc::new(super::policy::InteractivePolicy);
    let buffer = Arc::new(Mutex::new(StreamBuffer::new()));
    let ctx = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        run_agent_turn(
            &reqwest::Client::new(),
            &state,
            &tokio_util::sync::CancellationToken::new(),
            &policy,
            &buffer,
        ),
    )
    .await
    .expect("turn finishes");
    let bodies = bodies.lock().unwrap().clone();
    let history = state.lock().await.history.as_slice().to_vec();
    (ctx, bodies, history)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_empty_model_configuration_stops_the_turn_before_any_request() {
    // `models = []` used to resolve to the compiled profiles, which name a
    // private host: the turn then failed with a connection error the user
    // could not act on. It must stop here with a message that says what to do.
    let (endpoint, bodies) = scripted_provider(vec![chat_sse("must not be reached", "stop")]).await;
    let mut app = AppState::new();
    app.api_base_url = endpoint;
    app.model_name = "unconfigured".into();
    app.config.models.clear();
    app.history.push(ChatMessage::new("user", "Hello there."));
    let state = Arc::new(Mutex::new(app));
    let ctx = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        run_agent_turn(
            &reqwest::Client::new(),
            &state,
            &tokio_util::sync::CancellationToken::new(),
            &Arc::new(super::policy::InteractivePolicy),
            &Arc::new(Mutex::new(StreamBuffer::new())),
        ),
    )
    .await
    .expect("turn finishes");

    assert_eq!(
        ctx.lifecycle.stop_reason,
        Some(lifecycle::StopReason::ProviderError(None))
    );
    let notice = ctx
        .response
        .provider_error_notice
        .as_deref()
        .expect("the notice reaches the transcript");
    assert!(notice.contains("No model is configured"), "{notice}");
    let history = state.lock().await.history.as_slice().to_vec();
    assert!(
        history.iter().any(|message| message.role == "system"
            && message.content.contains("No model is configured")),
        "{history:#?}"
    );
    assert!(
        bodies.lock().unwrap().is_empty(),
        "no request reached a provider"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_long_answer_survives_repeated_length_stops_without_the_recovery_clamp() {
    // #2118: the second length stop ended the turn, and the one continuation
    // on Chat Completions was clamped to 1024 tokens.
    let (ctx, bodies, history) = scripted_turn(
        vec![
            chat_sse("Part one. ", "length"),
            chat_sse("Part two. ", "length"),
            chat_sse("Part three. ", "length"),
            chat_sse("Part four. ", "length"),
            chat_sse("Done.", "stop"),
        ],
        crate::config::ModelProfile::default(),
    )
    .await;

    assert_eq!(bodies.len(), 5, "{bodies:#?}");
    let caps: Vec<_> = bodies
        .iter()
        .map(|body| body["max_tokens"].as_u64())
        .collect();
    assert!(
        caps.iter().all(|cap| *cap != Some(1024)),
        "no request is clamped to 1024: {caps:?}"
    );
    // The fake provider is on loopback, so the profile is local: a 4096
    // tool-round cap, doubled for the turn-level continuation.
    assert_eq!(caps[0], Some(4096), "{caps:?}");
    assert_eq!(
        caps[3],
        Some(8192),
        "the continuation lifts the cap: {caps:?}"
    );
    assert!(
        !matches!(
            ctx.lifecycle.stop_reason,
            Some(lifecycle::StopReason::OutputLimit(_) | lifecycle::StopReason::LoopEscalation)
        ),
        "{:?}",
        ctx.lifecycle.stop_reason
    );
    let answer: String = history
        .iter()
        .filter(|message| message.role == "assistant")
        .map(|message| message.content.as_str())
        .collect();
    assert!(answer.contains("Part one."), "{answer}");
    assert!(answer.contains("Done."), "{answer}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reasoning_that_fills_the_cap_is_retried_with_more_room_then_named() {
    // A reasoning-only response that hits the cap gets one retry with the
    // full completion budget and a lower effort; if that also produces
    // nothing, the notice names the cap and `max_output_tokens`.
    let reasoning_only = format!(
        "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
        serde_json::json!({"choices": [{"delta": {"reasoning_content": "thinking hard"}}]}),
        serde_json::json!({"choices": [{"delta": {}, "finish_reason": "length"}]}),
    );
    let (ctx, bodies, _history) = scripted_turn(
        vec![reasoning_only],
        crate::config::ModelProfile {
            reasoning_effort: Some("high".into()),
            supports_reasoning_effort: Some(true),
            ..crate::config::ModelProfile::default()
        },
    )
    .await;

    let retry = bodies
        .iter()
        .find(|body| body["reasoning_effort"] == "low")
        .unwrap_or_else(|| panic!("a retry with lower effort: {bodies:#?}"));
    assert!(
        retry["max_tokens"].as_u64().unwrap_or(0) > bodies[0]["max_tokens"].as_u64().unwrap_or(0),
        "{bodies:#?}"
    );
    assert_eq!(bodies[0]["reasoning_effort"], "high");
    assert!(
        bodies.iter().all(|body| body["max_tokens"] != 1024),
        "{bodies:#?}"
    );
    let reason = ctx.lifecycle.stop_reason.clone().expect("stop reason");
    assert!(
        reason.to_string().starts_with("output_limit:default:"),
        "{reason}"
    );
    assert!(
        ctx.response.final_content.contains("max_output_tokens"),
        "{}",
        ctx.response.final_content
    );
}

/// History whose estimate sits just under compaction's budget (no overhead
/// measured yet, as after a restart) while the rendered request with its
/// system prompt and tail runs over the trim budget. Compaction used to
/// report `below_budget` and the request dropped old turns; it now compacts
/// first and nothing is hard trimmed (#2500).
#[tokio::test]
async fn near_full_history_compacts_before_the_request_trims_it() {
    let mut app = AppState::new();
    let profile = crate::config::ModelProfile {
        name: "near-full".to_owned(),
        url: "http://127.0.0.1:9/v1/chat/completions".to_owned(),
        model: "near-full".to_owned(),
        context_window: Some(32_768),
        soft_context_target: Some(24_000),
        hard_effective_limit: Some(30_000),
        provider_overhead_margin: Some(0),
        ..Default::default()
    };
    app.api_base_url = profile.url.clone();
    app.model_name = profile.name.clone();
    app.config.models = vec![profile];
    app.record_function_calling_support("http://127.0.0.1:9/v1/chat/completions", false);
    let budget = app.active_context_budget();
    let compaction_budget = super::proactive_history_budget(&budget, 0) as usize;
    let filler = |turn: usize, role: &str| {
        (0..30)
            .map(|word| format!("{role}{turn}w{word}"))
            .collect::<Vec<_>>()
            .join(" ")
    };
    let mut turn = 0;
    loop {
        let user = ChatMessage::new("user", filler(turn, "u"));
        let assistant = ChatMessage::new("assistant", filler(turn, "a"));
        let next: usize = app
            .history
            .iter()
            .chain([&user, &assistant])
            .map(compaction::estimate_message_tokens)
            .sum();
        if next >= compaction_budget.saturating_sub(16) {
            break;
        }
        app.history.push(user);
        app.history.push(assistant);
        turn += 1;
    }
    app.history
        .push(ChatMessage::new("user", "Continue the task."));
    let history_tokens: usize = app
        .history
        .iter()
        .map(compaction::estimate_message_tokens)
        .sum();
    assert!(
        history_tokens < compaction_budget,
        "{history_tokens} {compaction_budget}"
    );
    let state = Arc::new(Mutex::new(app));

    let prepared = super::prepare_turn_request_measured(
        &reqwest::Client::new(),
        &state,
        1,
        &tokio_util::sync::CancellationToken::new(),
        ContextCheckpoint::default(),
        None,
    )
    .await
    .expect("request preparation");

    assert_eq!(
        prepared.hard_trimmed, 0,
        "the request dropped history compaction kept"
    );
    let app = state.lock().await;
    assert!(app.last_request_overhead_tokens > 0);
    assert!(
        app.history
            .iter()
            .map(compaction::estimate_message_tokens)
            .sum::<usize>()
            < history_tokens,
        "compaction reduced the stored history"
    );
}

/// After a request has been hard trimmed, the provider's prompt count measures
/// the trimmed request, not the stored history. The compaction gate anchored on
/// that count reported `below_budget`, the `CompactFirst` retry lowered its
/// budget only by the overrun, and the same oldest turn was trimmed again on
/// every request until the local estimate caught up (#2603). Here the stored
/// history renders larger than its own estimate (text-protocol tool results
/// carry a metadata envelope) and the last usage came from a trimmed request:
/// the first preparation that has to act compacts and trims nothing.
#[tokio::test]
async fn a_request_after_a_hard_trim_compacts_instead_of_trimming_again() {
    let profile = crate::config::ModelProfile {
        name: "trimmed".to_owned(),
        url: "http://127.0.0.1:9/v1/chat/completions".to_owned(),
        model: "trimmed".to_owned(),
        context_window: Some(32_768),
        soft_context_target: Some(30_000),
        hard_effective_limit: Some(30_000),
        provider_overhead_margin: Some(0),
        ..Default::default()
    };
    let filler = |turn: usize, role: &str, words: usize| {
        (0..words)
            .map(|word| format!("{role}{turn}w{word}"))
            .collect::<Vec<_>>()
            .join(" ")
    };
    let mut turns: Vec<ChatMessage> = Vec::new();
    // What the previous request measured outside history, as in a session
    // that has been running for a while.
    let mut request_overhead = 0;
    for turn in 0..200 {
        turns.push(ChatMessage::new("user", filler(turn, "u", 20)));
        for call in 0..4 {
            let mut result = ChatMessage::new(
                "tool",
                format!("run_command: {}", filler(turn * 4 + call, "t", 6)),
            );
            result.tool_result = Some(crate::app::ToolResultRecord {
                tool_name: "run_command".to_owned(),
                arguments_hash: format!("{:064x}", turn * 4 + call),
                success: true,
                ..Default::default()
            });
            turns.push(result);
        }
        let history_tokens: usize = turns.iter().map(compaction::estimate_message_tokens).sum();
        // The last response was measured on a request that left the oldest
        // turns out: well under what the stored history holds.
        let usage = crate::app::TokenUsage {
            prompt_tokens: u32::try_from(history_tokens / 2).unwrap_or(u32::MAX),
            completion_tokens: 64,
            total_tokens: u32::try_from(history_tokens / 2 + 64).unwrap_or(u32::MAX),
            ..Default::default()
        };
        let mut reply = ChatMessage::new("assistant", filler(turn, "a", 20));
        reply.token_usage = Some(usage.clone());

        let mut app = AppState::new();
        app.api_base_url = profile.url.clone();
        app.model_name = profile.name.clone();
        app.config.models = vec![profile.clone()];
        app.record_function_calling_support("http://127.0.0.1:9/v1/chat/completions", false);
        app.history.extend(turns.iter().cloned());
        app.history.push(reply.clone());
        app.history
            .push(ChatMessage::new("user", "Continue the task."));
        app.current_token_usage = Some(usage);
        app.last_request_overhead_tokens = request_overhead;
        let state = Arc::new(Mutex::new(app));
        let prepared = super::prepare_turn_request_measured(
            &reqwest::Client::new(),
            &state,
            1,
            &tokio_util::sync::CancellationToken::new(),
            ContextCheckpoint::default(),
            None,
        )
        .await
        .expect("request preparation");
        let (compacted, overhead) = {
            let app = state.lock().await;
            (
                app.last_compaction.is_some(),
                app.last_request_overhead_tokens,
            )
        };
        request_overhead = overhead;
        if !compacted && prepared.hard_trimmed == 0 {
            turns.push(reply);
            continue;
        }
        assert_eq!(
            prepared.hard_trimmed, 0,
            "turn {turn}: the request trimmed history instead of compacting it"
        );
        assert!(compacted, "turn {turn}");
        return;
    }
    panic!("the history never filled the request");
}

/// A caller cancelled while its approval is pending (a subagent stopped by
/// `cancel_agent`) stops waiting at once and withdraws its prompt (#2558).
#[tokio::test]
async fn a_cancelled_caller_withdraws_its_pending_confirmation() {
    let workspace = tempfile::tempdir().expect("temporary workspace");
    let state = Arc::new(Mutex::new(AppState::new_with_workspace_session(
        workspace.path(),
        Some("confirm-cancel-test"),
    )));
    state.lock().await.agent_mode = crate::config::AgentMode::Build;
    let cancel_token = tokio_util::sync::CancellationToken::new();
    let task = {
        let state = Arc::clone(&state);
        let cancel_token = cancel_token.clone();
        tokio::spawn(async move {
            confirm_and_execute(
                &reqwest::Client::new(),
                &state,
                &cancel_token,
                "write_to_file",
                &serde_json::json!({"path": "cancelled.txt", "content": "x"}),
                "agent-1 · write_to_file",
                false,
                None,
                None,
            )
            .await
        })
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while state.lock().await.pending_tool_confirmation.is_none() {
        assert!(std::time::Instant::now() < deadline, "no prompt shown");
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    cancel_token.cancel();
    let (result, _, _) = tokio::time::timeout(std::time::Duration::from_secs(5), task)
        .await
        .expect("the wait ends on cancel")
        .unwrap();
    assert_eq!(
        result.error_kind,
        Some(crate::tools::ToolErrorKind::Cancelled)
    );
    let s = state.lock().await;
    assert!(s.pending_tool_confirmation.is_none());
    assert!(s.tool_confirmation_response.is_none());
    assert!(!workspace.path().join("cancelled.txt").exists());
}

/// A host that cannot show the approval modal (an ACP client) answers the
/// call through its approver; the modal is never left waiting (#2558).
#[tokio::test]
async fn an_external_approver_answers_instead_of_the_modal() {
    let workspace = tempfile::tempdir().expect("temporary workspace");
    let state = Arc::new(Mutex::new(AppState::new_with_workspace_session(
        workspace.path(),
        Some("confirm-external-test"),
    )));
    let asked = Arc::new(std::sync::Mutex::new(Vec::new()));
    {
        let mut s = state.lock().await;
        s.agent_mode = crate::config::AgentMode::Build;
        let asked = Arc::clone(&asked);
        s.external_tool_approver = Some(crate::app::ExternalToolApprover(Arc::new(
            move |request: crate::app::ExternalApprovalRequest| {
                asked.lock().unwrap().push(request.title);
                Box::pin(async { crate::app::ToolConfirmationResponse::Deny })
            },
        )));
    }
    let (result, _, _) = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        confirm_and_execute(
            &reqwest::Client::new(),
            &state,
            &tokio_util::sync::CancellationToken::new(),
            "write_to_file",
            &serde_json::json!({"path": "denied.txt", "content": "x"}),
            "agent-1 · write_to_file",
            false,
            None,
            None,
        ),
    )
    .await
    .expect("the approver answers");
    assert_eq!(
        result.error_kind,
        Some(crate::tools::ToolErrorKind::PermissionDenied)
    );
    assert_eq!(*asked.lock().unwrap(), ["agent-1 · write_to_file"]);
    let s = state.lock().await;
    assert!(s.pending_tool_confirmation.is_none());
    assert!(s.tool_confirmation_response.is_none());
    assert!(!workspace.path().join("denied.txt").exists());
}
