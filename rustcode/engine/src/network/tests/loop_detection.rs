//! Loop detection, compaction, compiler checks and request alignment.

use super::*;

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
    ctx.metrics.failure_replans = 1;
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
    assert_eq!(ctx.compiler.consecutive_diagnostics, 1);
    update_compiler_diagnostic_streak(&mut ctx, compiler_diagnostic_fingerprint(first));
    assert_eq!(ctx.compiler.consecutive_diagnostics, 2);
    update_compiler_diagnostic_streak(&mut ctx, compiler_diagnostic_fingerprint(changed));
    assert_eq!(ctx.compiler.consecutive_diagnostics, 1);
    update_compiler_diagnostic_streak(&mut ctx, None);
    assert_eq!(ctx.compiler.consecutive_diagnostics, 0);
    assert!(ctx.compiler.last_diagnostic_fingerprint.is_none());
}

#[test]
fn compiler_outcomes_only_count_verified_source_diagnostics() {
    use crate::network::compiler::{
        CompilerCheckOutcome, append_compiler_outcome, update_compiler_outcome_streak,
    };

    let mut ctx = TurnContext::new();
    let diagnostic = CompilerCheckOutcome::SourceDiagnostics {
        output: "error[E0425]: missing_symbol".to_string(),
        fingerprint: "error[E0425]: missing_symbol".to_string(),
    };
    let mut source_result = ToolResult {
        tool_name: "replace_file_content".to_string(),
        content: "edit applied".to_string(),
        diff: None,
        file_preview: None,
        metadata: ToolResultMetadata::default(),
    };
    append_compiler_outcome(&mut source_result, &diagnostic);
    update_compiler_outcome_streak(&mut ctx, &diagnostic);
    let marker = "LSP/Compiler errors detected in workspace, please fix:";
    assert!(source_result.content.contains(marker));
    assert_eq!(
        compiler_diagnostic_fingerprint(&source_result.content),
        Some("error[E0425]: missing_symbol".to_string())
    );
    assert_eq!(ctx.compiler.consecutive_diagnostics, 1);
    update_compiler_outcome_streak(&mut ctx, &diagnostic);
    assert_eq!(ctx.compiler.consecutive_diagnostics, 2);

    let mut infrastructure_result = ToolResult {
        tool_name: "replace_file_content".to_string(),
        content: "edit applied".to_string(),
        diff: None,
        file_preview: None,
        metadata: ToolResultMetadata::default(),
    };
    let infrastructure = CompilerCheckOutcome::UnverifiedInfrastructure {
        reason: "PermissionDenied while creating checker tempdir".to_string(),
    };
    append_compiler_outcome(&mut infrastructure_result, &infrastructure);
    update_compiler_outcome_streak(&mut ctx, &infrastructure);
    assert!(!infrastructure_result.content.contains(marker));
    assert!(compiler_diagnostic_fingerprint(&infrastructure_result.content).is_none());
    assert!(
        infrastructure_result
            .content
            .contains("could not be verified")
    );
    assert_eq!(ctx.compiler.consecutive_diagnostics, 2);

    let passed = CompilerCheckOutcome::Passed;
    let mut passed_result = ToolResult {
        tool_name: "replace_file_content".to_string(),
        content: "edit applied".to_string(),
        diff: None,
        file_preview: None,
        metadata: ToolResultMetadata::default(),
    };
    append_compiler_outcome(&mut passed_result, &passed);
    assert_eq!(passed_result.content, "edit applied");
    update_compiler_outcome_streak(&mut ctx, &passed);
    assert_eq!(ctx.compiler.consecutive_diagnostics, 0);
    assert!(ctx.compiler.last_diagnostic_fingerprint.is_none());
}

#[cfg(unix)]
#[tokio::test]
async fn repeated_compiler_diagnostics_trigger_the_budget() {
    use crate::network::compiler::{
        CompilerCheckOutcome, append_compiler_outcome, run_compiler_command_outcome,
        update_compiler_outcome_streak,
    };
    use std::time::Duration;

    if !crate::tools::exec::sandbox::runtime_tests_available() {
        return;
    }

    let workspace = tempfile::tempdir().expect("compiler diagnostic workspace");
    let command = "printf '%s\\n' 'src/main.ts(3,1): error TS2322: wrong type' >&2; exit 1";
    let cancel_token = tokio_util::sync::CancellationToken::new();
    let mut ctx = TurnContext::new();

    for expected_streak in 1..=MAX_CONSECUTIVE_COMPILER_DIAGNOSTICS {
        let outcome = run_compiler_command_outcome(
            workspace.path(),
            command,
            false,
            Duration::from_secs(5),
            &cancel_token,
            crate::config::SandboxMode::default(),
        )
        .await;
        assert!(
            matches!(
                &outcome,
                CompilerCheckOutcome::SourceDiagnostics { output, .. }
                    if output.contains("error TS2322")
            ),
            "checker output should classify as a source diagnostic: {outcome:?}"
        );
        let mut result = ToolResult {
            tool_name: "delete_file".to_owned(),
            content: format!("Applied mutation {}", expected_streak - 1),
            diff: None,
            file_preview: None,
            metadata: ToolResultMetadata::default(),
        };
        append_compiler_outcome(&mut result, &outcome);
        update_compiler_outcome_streak(&mut ctx, &outcome);

        assert!(
            compiler_diagnostic_fingerprint(&result.content).is_some(),
            "source diagnostic should be annotated for the compiler budget"
        );
        assert_eq!(ctx.compiler.consecutive_diagnostics, expected_streak);
        if expected_streak < MAX_CONSECUTIVE_COMPILER_DIAGNOSTICS {
            assert!(turn_budget_exceeded(&ctx).is_none());
        }
    }

    match turn_budget_exceeded(&ctx) {
        Some(TurnBudgetLimit::CompilerDiagnostics(n)) => {
            assert_eq!(n, MAX_CONSECUTIVE_COMPILER_DIAGNOSTICS)
        }
        other => panic!("expected CompilerDiagnostics limit, got {other:?}"),
    }
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn originating_checker_failure_replay_preserves_budget_and_workspace() {
    use crate::network::compiler::{
        CompilerCheckOutcome, append_compiler_outcome, run_compiler_command_outcome,
        update_compiler_outcome_streak,
    };
    use std::time::Duration;

    if !crate::tools::exec::sandbox::runtime_tests_available() {
        return;
    }

    let workspace = tempfile::tempdir().expect("replay workspace");
    let mut ctx = TurnContext::new();
    let cancel_token = tokio_util::sync::CancellationToken::new();
    let checker_command = concat!(
        "printf 'checker temp artifact\\n' > \"$TMPDIR/.replay-check.hm\"; ",
        "mkdir -p \"$BUN_INSTALL_CACHE_DIR/bunx-501-biome@latest\"; ",
        "printf '{}\\n' > \"$BUN_INSTALL_CACHE_DIR/bunx-501-biome@latest/package.json\"; ",
        "printf '%s\\n' 'error: bun is unable to write files to tempdir: PermissionDenied' >&2; ",
        "exit 1"
    );
    let mut unverified_events = 0;

    fn find_checker_artifacts(directory: &std::path::Path, found: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(directory).expect("read checker workspace entry") {
            let entry = entry.expect("checker workspace entry");
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.ends_with(".hm") || name.starts_with("bunx-") {
                found.push(path);
            } else if entry
                .file_type()
                .expect("inspect checker workspace entry")
                .is_dir()
            {
                find_checker_artifacts(&path, found);
            }
        }
    }

    for mutation in 0..4 {
        std::fs::write(
            workspace.path().join(format!("mutation-{mutation}.txt")),
            format!("applied mutation {mutation}\n"),
        )
        .expect("apply replay mutation");

        let outcome = run_compiler_command_outcome(
            workspace.path(),
            checker_command,
            false,
            Duration::from_secs(5),
            &cancel_token,
            crate::config::SandboxMode::default(),
        )
        .await;
        assert!(
            matches!(
                &outcome,
                CompilerCheckOutcome::UnverifiedInfrastructure { reason }
                    if reason.contains("PermissionDenied")
            ),
            "mutation {mutation} should emit an unverified infrastructure event: {outcome:?}"
        );
        unverified_events += 1;

        let mut result = ToolResult {
            tool_name: "delete_file".to_owned(),
            content: format!("Applied mutation {mutation}"),
            diff: None,
            file_preview: None,
            metadata: ToolResultMetadata::default(),
        };
        append_compiler_outcome(&mut result, &outcome);
        update_compiler_outcome_streak(&mut ctx, &outcome);

        assert!(result.content.contains("could not be verified"));
        assert!(compiler_diagnostic_fingerprint(&result.content).is_none());
        assert_eq!(ctx.compiler.consecutive_diagnostics, 0);
        assert!(turn_budget_exceeded(&ctx).is_none());
    }

    assert_eq!(unverified_events, 4);
    let mut artifacts = Vec::new();
    find_checker_artifacts(workspace.path(), &mut artifacts);
    assert!(
        artifacts.is_empty(),
        "checker scratch artifacts leaked into the workspace: {artifacts:?}"
    );

    let shell = crate::tools::exec::run_command_output_with_workspace(
        &serde_json::json!({ "command": "pwd" }),
        Some(workspace.path().to_path_buf()),
    )
    .expect("active workspace should permit shell execution");
    assert!(shell.success, "{}", shell.content);
    assert!(
        shell
            .content
            .contains(&workspace.path().display().to_string()),
        "run_command did not execute in the active workspace: {}",
        shell.content
    );
}

#[test]
fn benchmark_summary_contains_metrics_and_stop_reason() {
    let mut ctx = TurnContext::new();
    ctx.budget.tool_rounds = 7;
    ctx.budget.tokens_used = 1234;
    ctx.metrics.tool_calls = 9;
    ctx.metrics.malformed_calls = 2;
    ctx.metrics.no_progress_results = 3;
    ctx.metrics.cancelled_tool_calls = 2;
    ctx.metrics.grounded_recoveries = 1;
    ctx.metrics.provider_errors = 1;
    ctx.metrics.provider_429s = 1;
    ctx.progress
        .changed_paths
        .insert("src/GameScene.ts".to_string());
    ctx.progress.phase_checkpoint = Some("Phase 3: verify placement".to_string());
    ctx.response.last_stream_termination = Some(lifecycle::StreamTermination::ClientBudget);
    ctx.lifecycle.stop_reason = Some(lifecycle::StopReason::ProviderError(Some(429)));

    let summary = ctx.benchmark_summary();
    assert_eq!(summary["tool_rounds"], 7);
    assert_eq!(summary["tokens_used"], 1234);
    assert_eq!(summary["tool_calls"], 9);
    assert_eq!(summary["provider_429s"], 1);
    assert_eq!(summary["grounded_recoveries"], 1);
    // Cancelled results are reported apart from judged no-progress results.
    assert_eq!(summary["cancelled_tool_calls"], 2);
    assert_eq!(summary["changed_paths"][0], "src/GameScene.ts");
    assert_eq!(summary["phase_checkpoint"], "Phase 3: verify placement");
    assert_eq!(summary["last_stream_termination"], "client_budget");
    assert_eq!(summary["stop_reason"], "provider_error:429");
    assert_eq!(summary["segment_count"], 1);
    assert_eq!(summary["segment_rounds"], 7);
    assert!(summary["effective_segment_limit"].is_null());
    assert!(summary["effective_total_round_limit"].is_null());
    // Provider cache telemetry rides along so long-task prefix stability
    // (cold vs reused) is visible per turn without extra logging.
    assert_eq!(summary["prefix_cache"], "cold");
    assert_eq!(summary["prefix_context_updates"], 0);
}

#[test]
fn provider_error_metrics_distinguish_quota_exhaustion() {
    let mut ctx = TurnContext::new();
    record_provider_error(&mut ctx, "429 Too Many Requests");
    record_provider_error(&mut ctx, "502 Bad Gateway");
    assert_eq!(ctx.metrics.provider_errors, 2);
    assert_eq!(ctx.metrics.provider_429s, 1);
    assert_eq!(
        ctx.lifecycle
            .stop_reason
            .as_ref()
            .map(ToString::to_string)
            .as_deref(),
        Some("provider_error:429")
    );
}

#[test]
fn active_todo_is_the_current_phase_checkpoint() {
    let todos = vec![
        crate::app::TodoItem {
            id: "t1".to_string(),
            blocked_reason: None,
            content: "Scaffold".to_string(),
            status: "completed".to_string(),
            priority: "high".to_string(),
        },
        crate::app::TodoItem {
            id: "t2".to_string(),
            blocked_reason: None,
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
    // #985 immutable-history contract: stored messages are append-only.
    // Context pressure is absorbed by FIFO head compaction (the head becomes
    // a deterministic record) — never by rewriting retained tool outputs in
    // place. The retained tail must stay byte-identical.
    let big_cmd = format!(
        "run_command: {}",
        (0..60)
            .map(|i| format!("output line number {i}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let file = "view_file: [File: src/main.rs, Lines 1 to 5 of 5]\n1: a\n2: b\n3: c\n4: d\n5: e";
    let mut history = vec![
        ChatMessage::new("user", "original goal: keep working on src/main.rs"),
        ChatMessage::new("assistant", "plan: inspect the build output first"),
        ChatMessage::new("tool", big_cmd),
        ChatMessage::new("user", "noted; now check the current file contents"),
        ChatMessage::new("assistant", "reading the file"),
        ChatMessage::new("tool", file.to_string()),
        ChatMessage::new("assistant", "working from the file contents"),
        ChatMessage::new("user", "current follow-up: continue"),
    ];
    let original = history.clone();
    // Budget forces compaction; the head absorbs the cut so the retained
    // tail survives byte-identical.
    assert!(compact_history_to_budget(&mut history, 80).await);
    assert!(
        history[0]
            .content
            .starts_with("[Deterministic context record]"),
        "head must become a deterministic record, got: {}",
        history[0].content
    );
    assert_eq!(
        history[1..],
        original[original.len() - (history.len() - 1)..],
        "retained tail must stay byte-identical"
    );
    assert!(
        !history
            .iter()
            .any(|m| m.content.contains("pruned to maintain context window")),
        "no retained message may be rewritten in place"
    );
}

#[tokio::test]
async fn test_compact_prunes_oldest_result_before_newer_result_in_same_class() {
    // #985 immutable-history contract: same-class outputs are never excerpted
    // in place. Pressure is absorbed by the head record; whatever tail is
    // retained keeps its exact original bytes.
    let old = format!(
        "run_command: old output\n{}",
        (0..80)
            .map(|i| format!("old diagnostic line {i}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let new = format!(
        "grep: new output\n{}",
        (0..80)
            .map(|i| format!("new diagnostic line {i}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let mut history = vec![
        ChatMessage::new("user", "original goal: diagnose the failure"),
        ChatMessage::new("assistant", "plan: collect diagnostics"),
        ChatMessage::new("tool", old),
        ChatMessage::new("user", "noted; now check the newer result"),
        ChatMessage::new("assistant", "reading the latest output"),
        ChatMessage::new("tool", new),
        ChatMessage::new("assistant", "analyzing the latest diagnostics"),
        ChatMessage::new("user", "current follow-up: report findings"),
    ];
    let original = history.clone();

    assert!(compact_history_to_budget(&mut history, 70).await);

    assert!(
        history[0]
            .content
            .starts_with("[Deterministic context record]"),
        "head must become a deterministic record, got: {}",
        history[0].content
    );
    assert_eq!(
        history[1..],
        original[original.len() - (history.len() - 1)..],
        "retained tail must stay byte-identical"
    );
    assert!(
        !history
            .iter()
            .any(|m| m.content.contains("pruned to maintain context window")),
        "no retained message may be rewritten in place"
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
                workspace_generation: None,
                workspace_epoch: None,
                evidence_hash: None,
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
                    workspace_generation: None,
                    workspace_epoch: None,
                    evidence_hash: None,
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

    // #1592: history keeps the cancellation verbatim — it is the record of
    // what the user stopped — but the provider request answers the call with
    // the neutral stub instead of the cancellation record, and the pairing
    // stays valid.
    let before = serde_json::to_string(&history).expect("serialize history");
    let messages = history::to_messages(&history, "system");
    assert_eq!(messages[1]["tool_calls"][0]["id"], "call_done");
    assert_eq!(messages[2]["tool_call_id"], "call_done");
    assert_eq!(messages[3]["tool_call_id"], "call_cancelled");
    let rendered = serde_json::to_string(&messages).expect("render request");
    assert!(!rendered.contains("interrupted by the user"));
    assert!(!rendered.contains("Cancelled"));
    assert!(rendered.contains("this call did not run"));
    assert_eq!(serde_json::to_string(&history).unwrap(), before);
    history::validate_native_tool_messages(&messages)
        .expect("a cancelled batch still pairs every announced call");
}

#[tokio::test]
async fn test_run_compiler_check_success() {
    if !crate::tools::exec::sandbox::runtime_tests_available() {
        return;
    }
    let cwd = std::env::current_dir().unwrap();
    let check = run_compiler_check(
        &cwd,
        &tokio_util::sync::CancellationToken::new(),
        crate::config::SandboxMode::default(),
    )
    .await;
    assert!(check.is_none());
}

#[cfg(unix)]
#[tokio::test]
async fn compiler_check_uses_disposable_temp_outside_workspace() {
    if !crate::tools::exec::sandbox::runtime_tests_available() {
        return;
    }
    use super::compiler::{CompilerCheckOutcome, run_compiler_command_outcome};
    use std::time::Duration;

    const CHILD_WORKSPACE: &str = "RUSTCODE_COMPILER_SCRATCH_TEST_WORKSPACE";
    if let Some(workspace) = std::env::var_os(CHILD_WORKSPACE) {
        let workspace = std::path::PathBuf::from(workspace);
        let command = "test -f Cargo.toml || exit 20; for path in \"$TMPDIR\" \"$TMP\" \"$TEMP\" \"$XDG_CACHE_HOME\" \"$NPM_CONFIG_CACHE\" \"$BUN_INSTALL_CACHE_DIR\"; do case \"$path\" in \"$PWD\"|\"$PWD\"/*) exit 21;; esac; case \"$path\" in \"$TMPDIR\"|\"$TMPDIR\"/*) ;; *) exit 22;; esac; done; touch \"$TMPDIR/checker-cache\"";
        let outcome = run_compiler_command_outcome(
            &workspace,
            command,
            false,
            Duration::from_secs(5),
            &tokio_util::sync::CancellationToken::new(),
            crate::config::SandboxMode::WorkspaceWrite,
        )
        .await;
        assert_eq!(outcome, CompilerCheckOutcome::Passed);
        assert!(!workspace.join("checker-cache").exists());
        return;
    }

    let project = tempfile::tempdir().unwrap();
    std::fs::write(project.path().join("Cargo.toml"), "[workspace]\n").unwrap();
    let output = crate::platform::command(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "network::tests::compiler_check_uses_disposable_temp_outside_workspace",
            "--nocapture",
        ])
        .env(CHILD_WORKSPACE, project.path())
        .env("TMPDIR", project.path())
        .env("TMP", project.path())
        .env("TEMP", project.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "child checker test failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn proactive_history_budget_leaves_soft_target_headroom() {
    let profile = crate::config::ModelProfile {
        name: "local".to_string(),
        url: "http://example.test/v1".to_string(),
        model: "model".to_string(),
        context_window: Some(32_768),
        soft_context_target: Some(24_000),
        hard_effective_limit: Some(30_000),
        provider_overhead_margin: Some(1_024),
        ..Default::default()
    };
    let budget = profile.context_budget();
    let proactive = super::proactive_history_budget(&budget, 0);

    assert!(proactive < budget.history_tokens);
    assert_eq!(
        proactive,
        (24_000 - budget.tool_reserve - 1_024 - 2_048)
            .min(budget.history_tokens.saturating_sub(1).max(1))
    );
}

/// gpt-6-luna on a ChatGPT plan: history at 198k passed the 200k trigger,
/// then the request, with 13k of schemas, prompt and tail, could fit only
/// 196k of messages and dropped 65 of them (session 01a16b37).
#[test]
fn proactive_history_budget_fits_the_last_requests_overhead() {
    let budget = crate::config::ContextBudget {
        context_window: 272_000,
        soft_context_target: 239_232,
        hard_effective_limit: 239_232,
        completion_reserve: 32_768,
        tool_reserve: 4_096,
        provider_overhead_margin: 32_768,
        history_tokens: 239_232,
        max_output_tokens: 32_768,
        thinking_reserve: 0,
        thinking_budget: 16_384,
        minimum_answer_tokens: 0,
        safety_reserve: 0,
    };
    assert_eq!(super::proactive_history_budget(&budget, 0), 200_320);
    let overhead = 2_235 + 10_060 + 786;
    let proactive = super::proactive_history_budget(&budget, overhead);
    let message_budget = 239_232 - 32_768 - 10_060;
    assert!(proactive + 2_235 + 786 < message_budget, "{proactive}");
    assert!(proactive < 198_423, "history at 198k now compacts");
}

#[test]
fn project_root_from_relative_file_is_a_real_directory() {
    let root = get_tool_project_root("delete_file", &serde_json::json!({"path": "src/temp.rs"}))
        .expect("should find project root for workspace file");
    assert!(root.is_absolute());
    assert!(root.is_dir());
    assert!(root.join("Cargo.toml").exists());

    let tmp_root = get_tool_project_root(
        "write_to_file",
        &serde_json::json!({"path": "/tmp/scratch.py"}),
    );
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
fn validation_rejection_keeps_detailed_history_and_model_diagnostics() {
    let detail = "[Tool call rejected before execution: invalid arguments for 'reply_to_chat_message'. Schema path: $.message_id is required. Expected arguments for 'reply_to_chat_message' use these keys: [\"chat_name\", \"confirm\", \"message\", \"message_id\"]. Example: {...}] Emit one corrected tool call.";
    let history = vec![ChatMessage::new("system", detail)];

    assert_eq!(history[0].content, detail);
    assert!(is_model_directed_note(&history[0]));
    let provider_messages = history::to_messages(&history, "system prompt");
    assert!(provider_messages.iter().any(|message| {
        message
            .get("content")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|content| content.contains("$.message_id is required"))
    }));
}

#[test]
fn loop_abort_allows_bounded_recoveries_before_forced_final() {
    // #984: the harness offers several guided nudges with tools enabled
    // before the terminal lockout, instead of disabling tools after one strike.
    assert_eq!(loop_recovery_action(0), LoopRecoveryAction::Recover);
    assert_eq!(loop_recovery_action(1), LoopRecoveryAction::Recover);
    assert_eq!(loop_recovery_action(2), LoopRecoveryAction::Recover);
    assert_eq!(loop_recovery_action(3), LoopRecoveryAction::ForceFinal);
    assert_eq!(
        loop_recovery_action(u8::MAX),
        LoopRecoveryAction::ForceFinal
    );
    assert!(LOOP_RECOVERY_PROMPT.contains("Tools remain enabled"));
    assert!(!LOOP_RECOVERY_PROMPT.contains("```tool"));
    assert_eq!(
        loop_recovery_action_for(3, false),
        LoopRecoveryAction::ForceFinal,
        "non-read-only loop recovery keeps its existing escalation"
    );
}

#[test]
fn repeated_read_only_inspection_has_a_larger_but_finite_recovery_budget() {
    let call = crate::tools::ToolCall {
        name: "view_file".to_string(),
        arguments: serde_json::json!({
            "path": "src/network.rs",
            "start_line": 1,
            "end_line": 20
        }),
        call_id: None,
    };
    let (exact, category) = loop_detect::signatures(&call.name, &call.arguments);
    let read_only_batch = loop_detect::is_read_only_call(&call.name, &call.arguments);
    assert!(read_only_batch);
    let mut detector = loop_detect::LoopDetector::new(6);
    for _ in 0..6 {
        let _ = detector.check_tool(&call.name, &exact, &category);
    }

    assert!(MAX_READ_ONLY_LOOP_RECOVERY_ROUNDS > MAX_LOOP_RECOVERY_ROUNDS);
    assert_eq!(
        loop_recovery_action_for(MAX_READ_ONLY_LOOP_RECOVERY_ROUNDS - 1, read_only_batch),
        LoopRecoveryAction::Recover,
        "a detected read loop gets one more chance than a mutating loop"
    );
    assert_eq!(
        loop_recovery_action_for(MAX_READ_ONLY_LOOP_RECOVERY_ROUNDS, read_only_batch),
        LoopRecoveryAction::ForceFinal,
        "detector resets must not make read-only recovery unbounded"
    );
    assert_eq!(
        loop_recovery_action_for(u8::MAX, read_only_batch),
        LoopRecoveryAction::ForceFinal
    );
}

#[test]
fn canonical_structured_read_only_classification_beats_name_only_fallback() {
    let call = crate::tools::ToolCall {
        name: "run_command".to_string(),
        arguments: serde_json::json!({"command": "git status --short"}),
        call_id: Some("alias-regression".to_string()),
    };
    assert!(crate::tools::is_read_only_call(&call));
    assert!(!loop_detect::is_read_only(&call.name));
}

#[test]
fn loop_warnings_are_coalesced_within_a_user_turn() {
    let mut history = vec![
        ChatMessage::new("user", "do it"),
        ChatMessage::new("system", "[Loop warning: first]"),
        ChatMessage::new("assistant", "trying again"),
        ChatMessage::new("tool", "same output"),
    ];

    push_or_replace_loop_warning(&mut history, "[Loop warning: updated]".to_string());

    let warnings = history
        .iter()
        .filter(|message| message.content.starts_with("[Loop warning:"))
        .collect::<Vec<_>>();
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].content, "[Loop warning: updated]");
}

#[test]
fn recovery_notices_are_coalesced_without_dropping_tool_history() {
    let mut history = vec![ChatMessage::new("user", "inspect and fix")];
    for round in 0..100 {
        history.push(ChatMessage::new(
            "assistant",
            format!("tool call round {round}"),
        ));
        history.push(ChatMessage::new("tool", format!("result {round}")));
        push_or_replace_recovery_notice(
            &mut history,
            format!("[Evidence-based recovery: round {round}]\nUse a different step."),
        );
    }

    let recovery_count = history
        .iter()
        .filter(|message| message.content.starts_with("[Evidence-based recovery:"))
        .count();
    assert_eq!(recovery_count, 1);
    assert_eq!(history.len(), 202, "assistant/tool evidence remains intact");
    assert!(history.last().unwrap().content.contains("result 99"));
    assert!(
        history.iter().any(|message| message.content
            == "[Evidence-based recovery: round 99]\nUse a different step.")
    );
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

// Regression: the runtime-context message follows every prompt, and merging
// the two read the prompt's content as a string. A prompt with an attached
// image has parts, so the whole prompt, image included, became nothing.
#[test]
fn aligning_keeps_the_text_and_image_of_a_prompt_followed_by_runtime_context() {
    let aligned = align_alternating_messages(vec![
        serde_json::json!({"role": "user", "content": [
            {"type": "text", "text": "who is this "},
            {"type": "image_url", "image_url": {"url": "data:image/png;base64,AAA"}},
        ]}),
        serde_json::json!({"role": "user", "content": "<rustcode_context>\nnow\n</rustcode_context>"}),
    ]);
    assert_eq!(
        aligned,
        [serde_json::json!({"role": "user", "content": [
            {"type": "text", "text": "who is this "},
            {"type": "image_url", "image_url": {"url": "data:image/png;base64,AAA"}},
            {"type": "text", "text": "\n\n<rustcode_context>\nnow\n</rustcode_context>"},
        ]})]
    );

    // Plain text still merges into one string.
    let aligned = align_alternating_messages(vec![
        serde_json::json!({"role": "user", "content": "first"}),
        serde_json::json!({"role": "user", "content": "second"}),
    ]);
    assert_eq!(
        aligned,
        [serde_json::json!({"role": "user", "content": "first\n\nsecond"})]
    );
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
fn typed_instruction_prefix_survives_provider_alignment() {
    let raw = vec![
        serde_json::json!({"role": "system", "content": "base"}),
        serde_json::json!({"role": "developer", "content": "project"}),
        serde_json::json!({"role": "user", "content": "task"}),
    ];

    let aligned = align_alternating_messages(raw);

    assert_eq!(
        aligned[0],
        serde_json::json!({"role": "system", "content": "base"})
    );
    assert_eq!(
        aligned[1],
        serde_json::json!({"role": "developer", "content": "project"})
    );
    assert_eq!(
        aligned[2],
        serde_json::json!({"role": "user", "content": "task"})
    );
}

#[test]
fn test_build_dynamic_context_tail() {
    let todo = |content: &str, status: &str| crate::app::TodoItem {
        id: String::new(),
        blocked_reason: None,
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
    let location = "rustcode/engine/src/network.rs";
    let diagnostics = format!("error: mismatched arguments\n --> {location}:1:1");
    let enriched = compiler_diagnostics_with_snippets(&diagnostics);
    assert!(enriched.contains(&diagnostics));
    assert!(enriched.contains(&format!("[compiler context: {location}:1:1]")));
    assert!(enriched.contains("use crate::app::{AppState"));
}

#[test]
fn compiler_diagnostics_preserve_missing_file_output() {
    let diagnostics = "src/does-not-exist.ts(4,2): error TS2339: Missing property";
    assert_eq!(compiler_diagnostics_with_snippets(diagnostics), diagnostics);
}
