//! Turn safety budgets: rounds, tokens, malformed calls and request logging.

use super::*;

// Regression: the benchmark session ran 106 tool rounds with no hard
// stop because the only guard was the loop detector, and a mutation that
// reports success while duplicating content resets it every round. These
// tests exercise the safety budgets directly against a constructed
// TurnContext so they run without a mock server.

#[test]
fn healthy_progress_does_not_trigger_the_budget() {
    let mut ctx = TurnContext::new();
    ctx.budget.tool_rounds = 12;
    ctx.budget.tokens_used = 40_000;
    ctx.progress.consecutive_no_progress = 0;
    ctx.progress.consecutive_failed_mutations = 0;
    ctx.compiler.consecutive_error_gates = 0;
    assert!(turn_budget_exceeded(&ctx).is_none());
}

#[test]
fn max_tool_rounds_triggers_the_budget() {
    let mut ctx = TurnContext::with_max_tool_rounds(40);
    ctx.budget.tool_rounds = ctx.budget.max_tool_rounds;
    match turn_budget_exceeded(&ctx) {
        Some(TurnBudgetLimit::ToolRounds(n)) => assert_eq!(n, ctx.budget.max_tool_rounds),
        other => panic!("expected ToolRounds limit, got {other:?}"),
    }
}

#[tokio::test]
async fn legacy_40_round_checkpoint_resumes_without_round_caps_or_replaying_history() {
    let state = Arc::new(Mutex::new(AppState::new()));
    {
        let mut s = state.lock().await;
        s.history
            .push(ChatMessage::new("user", "complete the long task"));
        s.history.push(
            ChatMessage::new("tool", "completed call-1").with_tool_result(
                crate::app::ToolResultRecord {
                    workspace_generation: None,
                    workspace_epoch: None,
                    evidence_hash: None,
                    tool_name: "write_file_chunk".to_string(),
                    success: true,
                    ..Default::default()
                },
            ),
        );
    }

    let mut ctx = TurnContext::with_max_tool_rounds(40);
    ctx.budget.tool_rounds = 40;
    ctx.progress.meaningful_events = 1;
    assert!(matches!(
        turn_budget_exceeded(&ctx),
        Some(TurnBudgetLimit::ProductiveSegment {
            used: 40,
            maximum: 40
        })
    ));

    let limit = turn_budget_exceeded(&ctx).expect("productive segment should yield");
    assert!(!stop_turn_for_budget(&state, &mut ctx, limit).await);
    assert!(ctx.budget.continuation_pending);
    assert!(ctx.budget.budget_stopped.is_none());
    assert!(ctx.response.final_content.contains("without replaying"));

    {
        let mut s = state.lock().await;
        save_turn_context_after_run(&mut s, ctx, true);
    }
    let history_len = state.lock().await.history.len();
    let mut resumed = {
        let mut s = state.lock().await;
        take_turn_context_for_prompt(&mut s, true, 40)
    };
    assert_eq!(resumed.budget.tool_rounds, 40);
    assert_eq!(resumed.budget.segment_count, 2);
    assert_eq!(resumed.segment_rounds(), 0);
    assert_eq!(resumed.budget.max_tool_rounds, usize::MAX);
    assert_eq!(resumed.budget.max_total_tool_rounds, usize::MAX);
    assert_eq!(state.lock().await.history.len(), history_len);

    // The logical turn can keep running past the legacy checkpoint ceiling.
    resumed.budget.tool_rounds = 141;
    assert!(turn_budget_exceeded(&resumed).is_none());
}

#[test]
fn productive_segment_requires_new_progress_after_each_boundary() {
    let mut ctx = TurnContext::with_max_tool_rounds(40);
    ctx.budget.tool_rounds = 40;
    ctx.progress.meaningful_events = 1;
    let first = turn_budget_exceeded(&ctx).expect("first segment should end");
    assert!(matches!(first, TurnBudgetLimit::ProductiveSegment { .. }));
    ctx.budget.continuation_pending = true;
    ctx.begin_next_segment();
    ctx.budget.tool_rounds = 80;
    assert!(matches!(
        turn_budget_exceeded(&ctx),
        Some(TurnBudgetLimit::ToolRounds(40))
    ));
}

#[test]
fn explicit_total_round_limit_stops_productive_continuation() {
    let mut ctx = TurnContext::with_budgets(40, 80);
    ctx.budget.tool_rounds = 80;
    ctx.progress.meaningful_events = 2;
    assert!(matches!(
        turn_budget_exceeded(&ctx),
        Some(TurnBudgetLimit::TotalToolRounds(80))
    ));
}

#[test]
fn default_turn_allows_productive_work_past_previous_round_ceiling() {
    let mut ctx = TurnContext::new();
    for round in 0..64 {
        ctx.budget.tool_rounds = round;
        ctx.budget.tokens_used = round as u64 * 100;
        ctx.progress.consecutive_no_progress = 0;
        ctx.progress.consecutive_failed_mutations = 0;
        ctx.compiler.consecutive_error_gates = 0;
        assert!(turn_budget_exceeded(&ctx).is_none(), "round {round}");
    }
}

#[test]
fn round_budget_notice_warns_once_before_the_hard_stop() {
    let mut ctx = TurnContext::with_max_tool_rounds(40);
    ctx.budget.tool_rounds = 31;
    assert!(turn_engine::take_round_budget_notice(&mut ctx).is_none());
    ctx.budget.tool_rounds = 32;
    let notice = turn_engine::take_round_budget_notice(&mut ctx).unwrap();
    assert!(notice.contains("32/40"));
    assert!(notice.contains("8 rounds remain at this checkpoint"));
    assert!(notice.contains("required validation"));
    assert!(notice.contains("Do not claim success without evidence"));
    for used in 32..40 {
        ctx.budget.tool_rounds = used;
        assert!(turn_engine::take_round_budget_notice(&mut ctx).is_none());
        assert!(turn_budget_exceeded(&ctx).is_none());
    }
    ctx.budget.tool_rounds = 40;
    assert!(matches!(
        turn_budget_exceeded(&ctx),
        Some(TurnBudgetLimit::ToolRounds(40))
    ));
    assert!(!ctx.lifecycle.task_completed);
}

#[test]
fn round_budget_notice_respects_small_custom_and_exhausted_budgets() {
    for maximum in [1, 3, 7, 100] {
        let mut ctx = TurnContext::with_max_tool_rounds(maximum);
        let threshold = maximum - maximum.div_ceil(5).min(8);
        for used in 0..threshold {
            ctx.budget.tool_rounds = used;
            assert!(turn_engine::take_round_budget_notice(&mut ctx).is_none());
        }
        ctx.budget.tool_rounds = threshold;
        let notice = turn_engine::take_round_budget_notice(&mut ctx).unwrap();
        assert!(notice.contains(&format!("{threshold}/{maximum}")));
        assert!(turn_budget_exceeded(&ctx).is_none());

        let mut exhausted = TurnContext::with_max_tool_rounds(maximum);
        exhausted.budget.tool_rounds = maximum;
        assert!(turn_engine::take_round_budget_notice(&mut exhausted).is_none());
        assert!(
            matches!(turn_budget_exceeded(&exhausted), Some(TurnBudgetLimit::ToolRounds(n)) if n == maximum)
        );
    }
}

#[tokio::test]
async fn round_budget_notice_reaches_the_provider_request() {
    let mut ctx = TurnContext::with_max_tool_rounds(40);
    ctx.budget.tool_rounds = 32;
    let notice = turn_engine::take_round_budget_notice(&mut ctx).unwrap();
    let mut app = AppState::new();
    app.history
        .push(ChatMessage::new("user", "Fix the compiler errors"));
    app.history.push(ChatMessage::new("system", notice.clone()));
    let messages = prepare_turn_request(
        &reqwest::Client::new(),
        &Arc::new(Mutex::new(app)),
        ctx.budget.tool_rounds,
        &tokio_util::sync::CancellationToken::new(),
    )
    .await
    .expect("request preparation");
    assert!(messages.iter().any(|message| {
        message["content"]
            .as_str()
            .is_some_and(|content| content.contains(&notice))
    }));
}

#[tokio::test]
async fn request_preparation_under_budget_does_not_mark_history_rewritten() {
    let mut app = AppState::new();
    app.history
        .push(ChatMessage::new("user", "Fix the compiler errors"));
    app.history
        .push(ChatMessage::new("assistant", "Reading the build output."));
    let before = app.history.clone();
    let state = Arc::new(Mutex::new(app));

    prepare_turn_request(
        &reqwest::Client::new(),
        &state,
        1,
        &tokio_util::sync::CancellationToken::new(),
    )
    .await
    .expect("request preparation");

    let app = state.lock().await;
    assert_eq!(app.history.as_slice(), before.as_slice());
    assert_eq!(
        app.history.rewrite_revision(),
        before.rewrite_revision(),
        "a round that compacts nothing must not look like a history rewrite"
    );
}

#[tokio::test]
async fn request_assembly_separates_project_instructions_from_runtime_notices() {
    let root = tempfile::tempdir().expect("workspace");
    std::fs::write(root.path().join("AGENTS.md"), "DEVELOPER-PROVENANCE-RULE")
        .expect("write instructions fixture");
    let mut app = AppState::new();
    app.workspace_root = Some(root.path().to_path_buf());
    app.history
        .push(ChatMessage::new("user", "inspect the workspace"));
    app.history.push(ChatMessage::new(
        "system",
        "[Loop warning: RUNTIME-PROVENANCE-GUIDANCE]",
    ));

    let messages = prepare_turn_request(
        &reqwest::Client::new(),
        &Arc::new(Mutex::new(app)),
        1,
        &tokio_util::sync::CancellationToken::new(),
    )
    .await
    .expect("request preparation");
    let rendered = serde_json::to_string(&messages).unwrap();

    assert_eq!(rendered.matches("DEVELOPER-PROVENANCE-RULE").count(), 1);
    assert_eq!(rendered.matches("RUNTIME-PROVENANCE-GUIDANCE").count(), 1);
    assert!(messages.iter().any(|message| {
        message["role"] == "developer"
            && message["content"]
                .as_str()
                .is_some_and(|content| content.contains("DEVELOPER-PROVENANCE-RULE"))
    }));
    assert!(messages.iter().any(|message| {
        message["role"] == "user"
            && message["content"]
                .as_str()
                .is_some_and(|content| content.contains("provenance=\"lifecycle\""))
    }));
}

#[test]
fn custom_tool_round_limit_triggers_at_the_configured_round() {
    let mut ctx = TurnContext::with_max_tool_rounds(3);
    ctx.budget.tool_rounds = 3;
    match turn_budget_exceeded(&ctx) {
        Some(TurnBudgetLimit::ToolRounds(n)) => assert_eq!(n, 3),
        other => panic!("expected configured ToolRounds limit, got {other:?}"),
    }
}

#[test]
fn loop_recovery_nudges_do_not_cap_the_turn_before_its_round_budget() {
    // #984: autonomous turns are budgeted by max_tool_rounds (and the other
    // safety budgets), not by recovery state. A turn mid-recovery keeps its
    // tools until the bounded recovery budget is exhausted or a real budget
    // trips — there is no artificial progress cap on top.
    let mut ctx = TurnContext::with_max_tool_rounds(40);
    ctx.budget.tool_rounds = 12;
    ctx.recovery.loop_recovery_attempts = 2;
    ctx.recovery.reasoning_recovery_attempts = 2;
    assert_eq!(
        loop_recovery_action(ctx.recovery.loop_recovery_attempts),
        LoopRecoveryAction::Recover
    );
    assert_eq!(
        reasoning_loop_recovery_action(ctx.recovery.reasoning_recovery_attempts),
        LoopRecoveryAction::Recover
    );
    assert!(turn_budget_exceeded(&ctx).is_none());

    // Exhausted recovery still escalates to a final answer, and the round
    // budget remains the hard backstop for autonomous execution.
    ctx.recovery.loop_recovery_attempts = 3;
    assert_eq!(
        loop_recovery_action(ctx.recovery.loop_recovery_attempts),
        LoopRecoveryAction::ForceFinal
    );
    assert!(turn_budget_exceeded(&ctx).is_none());
    ctx.budget.tool_rounds = ctx.budget.max_tool_rounds;
    assert!(matches!(
        turn_budget_exceeded(&ctx),
        Some(TurnBudgetLimit::ToolRounds(_))
    ));
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
fn cached_prompt_reads_are_charged_at_a_tenth_of_the_turn_token_budget() {
    // Session 01a14e34cc01 turn 3 (#2114): 43 rounds, 5,053,432 input tokens
    // of which 3,768,576 were prompt-cache reads, plus 29,390 output. Raw,
    // that is 5,082,822 tokens and tripped the 5M budget. Weighted, it is
    // about a third of it.
    let usage = crate::app::TokenUsage {
        prompt_tokens: 5_053_432,
        completion_tokens: 29_390,
        total_tokens: 5_082_822,
        cached_tokens: Some(3_768_576),
        ..Default::default()
    };
    let charged = budget_tokens_for_usage(&usage);
    assert_eq!(charged, 5_082_822 - 3_768_576 + 376_858);
    assert!(charged < MAX_TURN_TOKEN_BUDGET / 2);

    // Without cache information every token counts at full weight.
    let uncached = crate::app::TokenUsage {
        cached_tokens: None,
        ..usage.clone()
    };
    assert_eq!(budget_tokens_for_usage(&uncached), 5_082_822);

    // A malformed report cannot underflow.
    let bogus = crate::app::TokenUsage {
        total_tokens: 10,
        cached_tokens: Some(50),
        ..Default::default()
    };
    assert_eq!(budget_tokens_for_usage(&bogus), 1);
}

#[test]
fn a_fully_cached_runaway_turn_is_still_bounded_by_the_token_budget() {
    let round = crate::app::TokenUsage {
        prompt_tokens: 200_000,
        completion_tokens: 0,
        total_tokens: 200_000,
        cached_tokens: Some(200_000),
        ..Default::default()
    };
    let mut ctx = TurnContext::new();
    let mut rounds = 0;
    while turn_budget_exceeded(&ctx).is_none() {
        ctx.budget.tokens_used = accumulate_tokens_used(
            ctx.budget.tokens_used,
            Some(budget_tokens_for_usage(&round)),
            "",
        );
        rounds += 1;
        assert!(rounds <= 250, "cached reads must still consume the budget");
    }
    assert_eq!(rounds, 250);
}

#[tokio::test]
async fn token_budget_stop_notice_leads_with_the_budget_not_the_round_count() {
    let mut ctx = TurnContext::new();
    ctx.budget.tool_rounds = 43;
    ctx.budget.tokens_used = 5_082_822;
    let limit = turn_budget_exceeded(&ctx).expect("token budget is exceeded");
    let state = Arc::new(Mutex::new(AppState::new()));
    stop_turn_for_budget(&state, &mut ctx, limit).await;
    let notice = &ctx.response.final_content;
    assert!(
        notice.starts_with(
            "[harness: stopped — token budget reached (~5.1M of 5.0M budget tokens; cached prompt reads count at 1/10), after 43 tool round(s)."
        ),
        "{notice}"
    );
    assert!(notice.contains("not a tool-round limit"), "{notice}");
    assert!(notice.contains("The task is NOT complete"), "{notice}");
}

#[test]
fn a_genuinely_oversized_turn_trips_the_token_budget() {
    let mut ctx = TurnContext::new();
    for _ in 0..200 {
        ctx.budget.tokens_used = accumulate_tokens_used(ctx.budget.tokens_used, Some(30_000), "");
    }
    assert!(
        ctx.budget.tokens_used >= MAX_TURN_TOKEN_BUDGET,
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
        ctx.budget.tokens_used = accumulate_tokens_used(ctx.budget.tokens_used, Some(5_000), "");
        ctx.budget.tool_rounds += 1;
    }
    assert!(
        turn_budget_exceeded(&ctx).is_none(),
        "10 rounds of light, real work must not trip a safety budget"
    );
}

/// Session 01a15ef714ce turn 3 (#2236): two PRs merged inside one turn, then
/// a third stopped mid-edit at round 199 once a ~300k cached prefix had spent
/// the 5M window. A window that changed the workspace is renewed instead.
#[test]
fn a_window_that_changed_the_workspace_is_renewed_instead_of_stopping() {
    let mut ctx = TurnContext::new();
    ctx.budget.tool_rounds = 199;
    ctx.budget.tokens_used = 5_015_842;
    ctx.progress.workspace_change_events = 51;

    assert!(renew_token_window_if_productive(&mut ctx));
    assert_eq!(turn_budget_exceeded(&ctx), None);
    assert_eq!(ctx.budget.token_window_start, 5_015_842);
    assert_eq!(ctx.budget.token_window_change_checkpoint, 51);

    // The renewed window only counts what is spent after it opened.
    ctx.budget.tokens_used += MAX_TURN_TOKEN_BUDGET - 1;
    assert_eq!(turn_budget_exceeded(&ctx), None);
}

#[test]
fn a_window_without_workspace_changes_still_stops_the_turn() {
    let mut ctx = TurnContext::new();
    ctx.budget.tokens_used = 5_015_842;
    ctx.progress.workspace_change_events = 51;
    assert!(renew_token_window_if_productive(&mut ctx));

    // The next window only reads and re-verifies: no renewal.
    ctx.budget.tokens_used += MAX_TURN_TOKEN_BUDGET;
    ctx.progress.meaningful_events += 40;
    assert!(!renew_token_window_if_productive(&mut ctx));
    assert_eq!(
        turn_budget_exceeded(&ctx),
        Some(TurnBudgetLimit::Tokens(MAX_TURN_TOKEN_BUDGET))
    );

    // A turn that never changed anything stops at the first window, as before.
    let mut read_only = TurnContext::new();
    read_only.budget.tokens_used = MAX_TURN_TOKEN_BUDGET;
    assert!(!renew_token_window_if_productive(&mut read_only));
    assert!(matches!(
        turn_budget_exceeded(&read_only),
        Some(TurnBudgetLimit::Tokens(_))
    ));
}

#[test]
fn the_token_ceiling_stops_even_a_productive_turn() {
    let round = crate::app::TokenUsage {
        prompt_tokens: 300_000,
        completion_tokens: 0,
        total_tokens: 300_000,
        cached_tokens: Some(300_000),
        ..Default::default()
    };
    let mut ctx = TurnContext::new();
    let mut rounds = 0;
    loop {
        // Every round edits a file.
        ctx.progress.workspace_change_events += 1;
        renew_token_window_if_productive(&mut ctx);
        if let Some(limit) = turn_budget_exceeded(&ctx) {
            assert!(
                matches!(limit, TurnBudgetLimit::TokenCeiling(n) if n >= MAX_TURN_TOKEN_CEILING),
                "{limit:?}"
            );
            break;
        }
        ctx.budget.tokens_used = accumulate_tokens_used(
            ctx.budget.tokens_used,
            Some(budget_tokens_for_usage(&round)),
            "",
        );
        rounds += 1;
        assert!(rounds <= 1_000, "the ceiling must bound a productive turn");
    }
    assert_eq!(rounds as u64, MAX_TURN_TOKEN_CEILING.div_ceil(30_000));
    assert!(
        TurnBudgetLimit::TokenCeiling(MAX_TURN_TOKEN_CEILING)
            .to_string()
            .starts_with("turn token ceiling reached (~20.0M of 20.0M")
    );
}

#[test]
fn token_budget_triggers_the_budget() {
    let mut ctx = TurnContext::new();
    ctx.budget.tokens_used = MAX_TURN_TOKEN_BUDGET;
    match turn_budget_exceeded(&ctx) {
        Some(TurnBudgetLimit::Tokens(n)) => assert_eq!(n, MAX_TURN_TOKEN_BUDGET),
        other => panic!("expected Tokens limit, got {other:?}"),
    }
}

#[test]
fn repeated_malformed_tool_calls_trigger_the_budget_and_leave_it_idle() {
    let mut ctx = TurnContext::new();
    ctx.recovery.consecutive_malformed_calls = MAX_CONSECUTIVE_MALFORMED_CALLS;
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
    assert_eq!(ctx.recovery.consecutive_malformed_calls, 2);
    assert_eq!(ctx.metrics.malformed_calls, 2);
    assert!(ctx.recovery.last_malformed_call.as_ref().unwrap().len() < 64);
    assert!(!super::turn_engine::record_malformed_call(
        &mut ctx,
        "ignored for parsed calls",
        &[crate::tools::ToolCall {
            name: "replace_file_content".to_string(),
            arguments: serde_json::json!({"path":"src/other.ts","edits":"[]"}),
            call_id: None,
        }]
    ));
    assert_eq!(ctx.recovery.consecutive_malformed_calls, 1);
}

#[test]
fn below_the_malformed_call_budget_does_not_trip() {
    let mut ctx = TurnContext::new();
    ctx.recovery.consecutive_malformed_calls = MAX_CONSECUTIVE_MALFORMED_CALLS - 1;
    assert!(turn_budget_exceeded(&ctx).is_none());
}

#[test]
fn repeated_failed_edits_trigger_the_budget() {
    let mut ctx = TurnContext::new();
    ctx.progress.consecutive_failed_mutations = MAX_CONSECUTIVE_FAILED_MUTATIONS;
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
    ctx.progress.consecutive_no_progress = MAX_CONSECUTIVE_NO_PROGRESS;
    match turn_budget_exceeded(&ctx) {
        Some(TurnBudgetLimit::NoProgress(n)) => assert_eq!(n, MAX_CONSECUTIVE_NO_PROGRESS),
        other => panic!("expected NoProgress limit, got {other:?}"),
    }
}

#[test]
fn repeated_compiler_error_gates_trigger_the_budget() {
    let mut ctx = TurnContext::new();
    ctx.compiler.consecutive_error_gates = MAX_CONSECUTIVE_COMPILER_ERROR_GATES;
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
    let mut ctx = TurnContext::with_max_tool_rounds(40);
    ctx.budget.tool_rounds = ctx.budget.max_tool_rounds;
    ctx.lifecycle.task_completed = false;
    ctx.response.final_content = "Applying the last edit".to_string();
    ctx.response.final_content_persisted = true;

    let limit = turn_budget_exceeded(&ctx).expect("budget should be exceeded");
    let should_continue = stop_turn_for_budget(&state, &mut ctx, limit).await;

    assert!(!should_continue, "a budget stop must end the loop");
    let transcript = lifecycle::final_transcript_content(
        ctx.lifecycle.task_completed,
        &ctx.response.final_content,
        ctx.response.final_content_persisted,
        ctx.lifecycle.stop_reason.as_ref().unwrap(),
    )
    .expect("the new budget explanation must be persisted");
    assert_eq!(transcript, ctx.response.final_content);
    assert!(transcript.contains("resume it in a new turn"));
    assert!(
        !ctx.lifecycle.task_completed,
        "a budget stop must never claim completion"
    );
    assert!(
        ctx.budget.budget_stopped.is_some(),
        "the exact limit reached must be recorded"
    );
    assert!(
        ctx.response.final_content.contains("stopped"),
        "the summary must explain the stop: {}",
        ctx.response.final_content
    );
    assert!(
        ctx.response
            .final_content
            .to_ascii_lowercase()
            .contains("not complete"),
        "the summary must be explicit that the task is unfinished: {}",
        ctx.response.final_content
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
    ctx.recovery.consecutive_malformed_calls = MAX_CONSECUTIVE_MALFORMED_CALLS;

    let limit = turn_budget_exceeded(&ctx).expect("budget should be exceeded");
    assert!(matches!(limit, TurnBudgetLimit::MalformedCalls(_)));
    let should_continue = stop_turn_for_budget(&state, &mut ctx, limit).await;

    assert!(!should_continue, "a budget stop must end the loop");
    assert!(
        !ctx.lifecycle.task_completed,
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
    ctx.budget.tool_rounds = ctx.budget.max_tool_rounds;

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
fn request_log_summary_identifies_textual_tool_contracts() {
    let summary = super::stream_request::request_log_summary_with_protocol(
        "local-model",
        3,
        18,
        4096,
        "textual",
        "json",
        18,
        142,
        142,
    );

    assert!(summary.contains("tool_mode=textual"));
    assert!(summary.contains("tool_protocol=json"));
    assert!(summary.contains("tools=18"));
    assert!(summary.contains("available_builtin_tools=18"));
    assert!(summary.contains("tool_schema_tokens=142"));
    assert!(summary.contains("textual_contract_tokens=142"));
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
