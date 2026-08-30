use super::*;
use serde_json::json;

#[test]
fn search_variants_share_category() {
    let (_, a) = signatures(
        "run_command",
        &json!({"command": "rg -n 'TODO|FIXME' src/"}),
    );
    let (_, b) = signatures(
        "run_command",
        &json!({"command": "grep -rnE \"TODO|FIXME\" src/ || echo none"}),
    );
    assert_eq!(a, b);
    assert_eq!(a, "search:TODO|FIXME src");
}

#[test]
fn view_file_range_shifting_shares_category() {
    // Same file, different line ranges = one intent. Range-shifting must not
    // dodge the loop detector.
    let (e1, c1) = signatures(
        "view_file",
        &json!({"path": "src/network.rs", "start_line": 1, "end_line": 100}),
    );
    let (e2, c2) = signatures(
        "view_file",
        &json!({"path": "src/network.rs", "start_line": 50, "end_line": 150}),
    );
    assert_ne!(e1, e2, "exact signatures should differ by range");
    assert_eq!(c1, c2, "same region should collapse to one category");
    assert_eq!(c1, "read:src/network.rs#0");
}

#[test]
fn view_file_distinct_regions_stay_distinct() {
    // Reading far-apart parts of a big file is legit paging, not a loop.
    let (_, c1) = signatures(
        "view_file",
        &json!({"path": "src/big.rs", "start_line": 40, "end_line": 240}),
    );
    let (_, c2) = signatures(
        "view_file",
        &json!({"path": "src/big.rs", "start_line": 1400, "end_line": 1600}),
    );
    assert_ne!(c1, c2, "distinct regions must not share a category");
}

#[test]
fn view_file_same_region_churn_aborts() {
    let mut d = LoopDetector::new(4); // warn at 2, abort at 4
    let mut last = LoopStatus::Ok;
    // Cosmetic shifts over the same ~250-region: all bucket 1.
    for start in [250, 260, 250, 255] {
        let (e, c) = signatures(
            "view_file",
            &json!({"path": "src/big.rs", "start_line": start, "end_line": start + 50}),
        );
        last = d.check(&e, &c);
    }
    assert_eq!(last, LoopStatus::Abort(4));
}

#[test]
fn edit_tool_normalizes_category_to_target_path() {
    let (exact, cat) = signatures(
        "replace_file_content",
        &json!({"path": "src/ui/mod.rs", "old_string": "a", "new_string": "b"}),
    );
    assert_ne!(exact, cat);
    assert_eq!(cat, "edit:src/ui/mod.rs");
}

#[test]
fn edit_tool_buckets_category_by_start_line() {
    let (_, cat1) = signatures(
        "replace_file_content",
        &json!({"path": "src/ui/mod.rs", "old_string": "a", "new_string": "b", "start_line": 50}),
    );
    let (_, cat2) = signatures(
        "replace_file_content",
        &json!({"path": "src/ui/mod.rs", "old_string": "x", "new_string": "y", "start_line": 500}),
    );
    assert_eq!(cat1, "edit:src/ui/mod.rs#0");
    assert_eq!(cat2, "edit:src/ui/mod.rs#2");
}

#[test]
fn edit_tool_buckets_string_start_line_like_the_handler() {
    // The edit handler parses string line numbers via parse_json_number;
    // the category signature must bucket them identically.
    let (_, cat) = signatures(
        "replace_file_content",
        &json!({"path": "src/ui/mod.rs", "old_string": "a", "new_string": "b", "start_line": "500"}),
    );
    assert_eq!(cat, "edit:src/ui/mod.rs#2");
}

#[test]
fn alternating_edit_ping_pong_caught_by_category() {
    let mut d = LoopDetector::new(4);
    let edit1 = json!({"path": "src/ui/mod.rs", "old_string": "% 6", "new_string": "% 10"});
    let edit2 = json!({"path": "src/ui/mod.rs", "old_string": "% 10", "new_string": "% 6"});
    let (e1, c1) = signatures("replace_file_content", &edit1);
    let (e2, c2) = signatures("replace_file_content", &edit2);

    assert_eq!(d.check(&e1, &c1), LoopStatus::Ok);
    assert_eq!(d.check(&e2, &c2), LoopStatus::Warning(2));
    assert_eq!(d.check(&e1, &c1), LoopStatus::Warning(3));
    assert_eq!(d.check(&e2, &c2), LoopStatus::Abort(4));
}

#[test]
fn grep_different_patterns_distinct_categories() {
    let (_e1, cat1) = signatures(
        "grep",
        &json!({ "pattern": "command", "path": "src/app/actions.rs" }),
    );
    let (_e2, cat2) = signatures(
        "grep",
        &json!({ "pattern": "/clear", "path": "src/app/actions.rs" }),
    );
    assert_ne!(cat1, cat2);
    assert_eq!(cat1, "grep:command@src/app/actions.rs");
    assert_eq!(cat2, "grep:/clear@src/app/actions.rs");
}

#[test]
fn exact_repeat_warns_then_aborts() {
    let mut d = LoopDetector::new(6);
    assert_eq!(d.check("x", "x"), LoopStatus::Ok);
    assert_eq!(d.check("x", "x"), LoopStatus::Ok);
    assert_eq!(d.check("x", "x"), LoopStatus::Abort(3));
}

#[test]
fn semantic_loop_caught_across_syntax() {
    let mut d = LoopDetector::new(4); // warn at 2, abort at 4
    let cmds = [
        "rg -n 'TODO' src/",
        "rg 'TODO' src/",
        "rg -i 'TODO' src/",
        "grep -rn 'TODO' src/",
    ];
    let results: Vec<LoopStatus> = cmds
        .iter()
        .map(|c| {
            let (e, cat) = signatures("run_command", &json!({ "command": c }));
            d.check(&e, &cat)
        })
        .collect();
    assert_eq!(results[0], LoopStatus::Ok);
    assert_eq!(results[3], LoopStatus::Abort(4));
}

#[test]
fn alternating_churn_caught_by_frequency() {
    let mut d = LoopDetector::new(4); // window = 8
    let mut last = LoopStatus::Ok;
    for i in 0..8 {
        let cmd = if i % 2 == 0 { "cat a.rs" } else { "pwd" };
        let (e, cat) = signatures("run_command", &json!({ "command": cmd }));
        last = d.check(&e, &cat);
    }
    assert_eq!(last, LoopStatus::Abort(4));
}

#[test]
fn read_only_repeats_warn_not_abort() {
    // A model paging around the same region it's editing must be nudged,
    // not hard-stopped: view_file repeats cap at Warning below 3× abort.
    let mut d = LoopDetector::new(4); // warn at 2, abort at 4
    let mut last = LoopStatus::Ok;
    for start in [250, 260, 250, 255, 252, 258] {
        let (e, c) = signatures(
            "view_file",
            &json!({"path": "src/big.rs", "start_line": start, "end_line": start + 50}),
        );
        last = d.check_tool("view_file", &e, &c);
    }
    assert!(matches!(last, LoopStatus::Warning(_)), "got {last:?}");
}

#[test]
fn equivalent_native_and_shell_reads_abort_after_three() {
    let mut detector = LoopDetector::new(8);
    let calls = [
        (
            "view_file",
            json!({"path": "/tmp/project/src/engine.ts", "start_line": 1, "end_line": 120}),
        ),
        (
            "run_command",
            json!({"command": "sed -n '1,120p' /tmp/project/src/engine.ts"}),
        ),
        (
            "run_command",
            json!({"command": "awk 'NR>=1 && NR<=120' /tmp/project/src/engine.ts"}),
        ),
    ];

    let mut statuses = Vec::new();
    for (name, args) in calls {
        let (exact, category) = signatures(name, &args);
        assert_eq!(category, "read:/tmp/project/src/engine.ts#0");
        statuses.push(detector.check_tool(name, &exact, &category));
    }
    assert_eq!(
        statuses,
        [LoopStatus::Ok, LoopStatus::Ok, LoopStatus::Abort(3)]
    );
}

#[test]
fn distinct_read_regions_and_workspace_progress_reset_cross_tool_guard() {
    let mut detector = LoopDetector::new(8);
    for command in [
        "sed -n '1,120p' src/engine.ts",
        "sed -n '250,350p' src/engine.ts",
        "cat src/other.ts",
    ] {
        let (exact, category) = signatures("run_command", &json!({"command": command}));
        assert_eq!(
            detector.check_tool("run_command", &exact, &category),
            LoopStatus::Ok
        );
    }

    let args = json!({"path": "src/engine.ts", "start_line": 1, "end_line": 120});
    let (exact, category) = signatures("view_file", &args);
    assert_eq!(
        detector.check_tool("view_file", &exact, &category),
        LoopStatus::Ok
    );
    detector.reset();
    assert_eq!(
        detector.check_tool("view_file", &exact, &category),
        LoopStatus::Ok
    );
}

#[test]
fn mutating_tool_still_aborts_via_check_tool() {
    // check_tool must not soften non-read-only tools.
    let mut d = LoopDetector::new(4);
    let mut last = LoopStatus::Ok;
    for _ in 0..4 {
        last = d.check_tool("write_to_file", "write_to_file:x", "write_to_file:x");
    }
    assert_eq!(last, LoopStatus::Abort(4));
}

#[test]
fn equivalent_failed_mutations_escalate_and_progress_resets_them() {
    let mut detector = LoopDetector::new(4);
    let first = detector.record_failed_tool("edit:a:1", "edit:src/state.ts");
    assert_eq!(first, LoopStatus::Ok);
    assert_eq!(
        detector.record_failed_tool("edit:a:2", "edit:src/state.ts"),
        LoopStatus::Abort(2)
    );

    detector.reset();
    assert_eq!(
        detector.record_failed_tool("edit:a:3", "edit:src/state.ts"),
        LoopStatus::Ok,
        "a successful mutation reset must clear the failed streak"
    );
}

#[test]
fn safe_git_inspection_repeats_warn_not_abort() {
    let mut d = LoopDetector::new(4);
    let mut last = LoopStatus::Ok;
    for _ in 0..4 {
        let (exact, category) = signatures(
            "run_command",
            &json!({"command": "git log v0.6.0..HEAD --oneline --no-merges"}),
        );
        last = d.check_tool("run_command", &exact, &category);
    }
    assert!(matches!(last, LoopStatus::Warning(_)), "got {last:?}");
}

#[test]
fn stable_git_inspection_is_progress_safe() {
    assert!(is_stable_inspection_command("git status --short"));
    assert!(is_stable_inspection_command("git diff --stat"));
    assert!(!is_stable_inspection_command("git restore -- src/lib.rs"));
}

#[test]
fn leading_cd_does_not_collapse_distinct_shell_actions() {
    let (_, curl) = signatures(
        "run_command",
        &json!({"command": "cd /tmp/project && curl -s http://localhost:5199"}),
    );
    let (_, browser) = signatures(
        "run_command",
        &json!({"command": "cd /tmp/project && terminal-browser open http://localhost:5199"}),
    );

    assert_eq!(curl, "cmd:curl:http://localhost:5199");
    assert_eq!(browser, "cmd:terminal-browser:open http://localhost:5199");
    assert_ne!(curl, browser);
}

#[test]
fn reset_clears_loop_state() {
    // After progress (reset), a previously-churning read starts fresh.
    let mut d = LoopDetector::new(4);
    for start in [250, 260, 250] {
        let (e, c) = signatures(
            "view_file",
            &json!({"path": "src/big.rs", "start_line": start, "end_line": start + 50}),
        );
        d.check(&e, &c);
    }
    d.reset();
    let (e, c) = signatures(
        "view_file",
        &json!({"path": "src/big.rs", "start_line": 255, "end_line": 305}),
    );
    assert_eq!(d.check(&e, &c), LoopStatus::Ok, "reset should clear counts");
}

#[test]
fn output_stagnation() {
    let mut d = LoopDetector::new(4);
    assert_eq!(d.record_output("no matches"), LoopStatus::Ok);
    assert_eq!(d.record_output("no matches"), LoopStatus::Warning(2));
    assert_eq!(d.record_output("no matches"), LoopStatus::Warning(3));
    assert_eq!(d.record_output("no matches"), LoopStatus::Abort(4));
}

#[test]
fn varied_no_match_searches_stagnate_as_one() {
    // Session 1785836601539: the model burned the whole tool-round budget
    // grepping for one hallucinated function name after another. Distinct
    // patterns produce distinct output strings, so exact hashing never
    // fired — the stagnation key must collapse them.
    let mut d = LoopDetector::new(4);
    let outputs = [
        "no matches for 'fn handle_input' under '.' (include filter: 'src/app/**/*.rs')",
        "no matches for 'fn handle_event' under '.' (include filter: 'src/**/*.rs')",
        "no matches for 'handle_key_event' under '.' (include filter: 'src/**/*.rs')",
        "no matches for 'on_key' under '.'",
    ];
    let mut last = LoopStatus::Ok;
    for out in outputs {
        last = d.record_output(stagnation_key(out));
    }
    assert_eq!(last, LoopStatus::Abort(4));
}

#[test]
fn stagnation_key_leaves_real_output_untouched() {
    let out = "matches for 'foo' under '.' (1 file(s)):\n\n./a.rs:\n  1: foo";
    assert_eq!(stagnation_key(out), out);
}

fn observation(output: &str, state: Option<&str>, failure: Option<&str>) -> ProgressObservation {
    ProgressObservation {
        action: "test".to_string(),
        output_fingerprint: stable_hash(output),
        state_fingerprint: state.map(stable_hash),
        failure_fingerprint: failure.map(stable_hash),
        changed_workspace: state.is_some(),
        fresh_read: false,
        search_result: false,
        no_result: false,
        verification: false,
        read_only: false,
        replayed: false,
        success: true,
    }
}

#[test]
fn progress_ledger_distinguishes_fresh_reads_from_cached_replays() {
    let mut ledger = ProgressLedger::default();
    let mut first = observation("file contents", None, None);
    first.fresh_read = true;
    assert_eq!(ledger.observe(&first).reason, ProgressReason::FreshRead);

    let mut replay = first.clone();
    replay.fresh_read = false;
    let assessment = ledger.observe(&replay);
    assert_eq!(assessment.reason, ProgressReason::NoNewInformation);
    assert!(!assessment.meaningful);
}

#[test]
fn progress_ledger_treats_varied_no_result_searches_as_stagnation() {
    let mut ledger = ProgressLedger::default();
    for index in 0..ProgressLedger::RECOVERY_STREAK {
        let mut search = observation(&format!("no-match-{index}"), None, None);
        search.search_result = true;
        search.no_result = true;
        let assessment = ledger.observe(&search);
        assert_eq!(assessment.reason, ProgressReason::NoNewInformation);
    }
    assert_eq!(ledger.no_progress_streak(), ProgressLedger::RECOVERY_STREAK);
}

#[test]
fn stable_successful_verification_suppresses_output_only_stagnation() {
    let mut ledger = ProgressLedger::default();
    let mut check = observation("cargo test: clean", None, None);
    check.verification = true;
    assert!(ledger.observe(&check).suppress_stagnation);
    assert!(ledger.observe(&check).suppress_stagnation);
    assert_eq!(ledger.no_progress_streak(), 0);
}

#[test]
fn failed_verification_is_not_exempt_from_stagnation() {
    let mut ledger = ProgressLedger::default();
    let mut check = observation("cargo test: failed", None, Some("cargo test: failed"));
    check.verification = true;
    check.success = false;
    assert!(!ledger.observe(&check).suppress_stagnation);
    let repeated = ledger.observe(&check);
    assert_eq!(repeated.reason, ProgressReason::RepeatedFailure);
    assert!(ledger.no_progress_streak() > 0);
}

#[test]
fn different_successful_actions_with_identical_output_are_progress() {
    let mut ledger = ProgressLedger::default();
    let first = observation("", None, None);
    assert!(ledger.observe(&first).meaningful);

    let mut second = first.clone();
    second.action = "different-action".to_string();
    assert!(ledger.observe(&second).meaningful);
}

#[test]
fn replayed_reads_do_not_count_as_new_information() {
    let mut ledger = ProgressLedger::default();
    let mut first = observation("file", None, None);
    first.read_only = true;
    first.fresh_read = true;
    assert!(ledger.observe(&first).meaningful);

    let mut replay = first.clone();
    replay.replayed = true;
    replay.fresh_read = false;
    assert!(!ledger.observe(&replay).meaningful);
}

#[test]
fn returning_to_a_previous_workspace_state_is_churn() {
    let mut ledger = ProgressLedger::default();
    assert_eq!(
        ledger.observe(&observation("a", Some("a"), None)).reason,
        ProgressReason::WorkspaceChanged
    );
    assert_eq!(
        ledger.observe(&observation("b", Some("b"), None)).reason,
        ProgressReason::WorkspaceChanged
    );
    let assessment = ledger.observe(&observation("a", Some("a"), None));
    assert_eq!(assessment.reason, ProgressReason::Churn);
    assert!(!assessment.meaningful);
}

#[test]
fn reasoning_loop_detector_catches_consecutive_repeated_sentences() {
    let mut detector = ReasoningLoopDetector::default();
    let sentence =
        "We need to inspect the network module to check the turn engine implementation.\n";
    assert_eq!(detector.feed_chunk(sentence), ReasoningLoopStatus::Ok);
    assert_eq!(detector.feed_chunk(sentence), ReasoningLoopStatus::Ok);
    assert!(matches!(
        detector.feed_chunk(sentence),
        ReasoningLoopStatus::LoopDetected(_)
    ));
}

#[test]
fn reasoning_loop_detector_catches_alternating_2_cycle() {
    let mut detector = ReasoningLoopDetector::default();
    let a = "First let us inspect the network engine to understand turn execution.\n";
    let b = "Now we should review the loop detection rules in loop_detect module.\n";
    assert_eq!(detector.feed_chunk(a), ReasoningLoopStatus::Ok);
    assert_eq!(detector.feed_chunk(b), ReasoningLoopStatus::Ok);
    assert_eq!(detector.feed_chunk(a), ReasoningLoopStatus::Ok);
    assert_eq!(detector.feed_chunk(b), ReasoningLoopStatus::Ok);
    assert!(matches!(
        detector.feed_chunk(a),
        ReasoningLoopStatus::LoopDetected(_)
    ));
}

#[test]
fn reasoning_loop_detector_catches_paragraph_repetition() {
    let mut detector = ReasoningLoopDetector::default();
    let para = "In this step we are carefully inspecting the entire test suite to ensure that all tests pass without errors and no regressions are introduced.\n\n";
    assert_eq!(detector.feed_chunk(para), ReasoningLoopStatus::Ok);
    assert_eq!(detector.feed_chunk(para), ReasoningLoopStatus::Ok);
    assert!(matches!(
        detector.feed_chunk(para),
        ReasoningLoopStatus::LoopDetected(_)
    ));
}

#[test]
fn reasoning_loop_detector_allows_legitimate_long_reasoning() {
    let mut detector = ReasoningLoopDetector::default();
    for i in 0..60 {
        let unique_thought = format!(
            "Step {i}: Considering function handler_{i} in module_{i} for comprehensive architectural refactoring.\n"
        );
        assert_eq!(
            detector.feed_chunk(&unique_thought),
            ReasoningLoopStatus::Ok,
            "legitimate unique reasoning step {i} should not trigger loop detector"
        );
    }
}

#[test]
fn reasoning_loop_detector_ignores_short_common_phrases() {
    let mut detector = ReasoningLoopDetector::default();
    for _ in 0..10 {
        assert_eq!(detector.feed_chunk("Let's see.\n"), ReasoningLoopStatus::Ok);
        assert_eq!(detector.feed_chunk("Wait.\n"), ReasoningLoopStatus::Ok);
        assert_eq!(detector.feed_chunk("Okay.\n"), ReasoningLoopStatus::Ok);
    }
}

#[test]
fn reasoning_loop_detector_catches_cross_turn_stagnant_plan() {
    let mut detector = ReasoningLoopDetector::default();
    let plan =
        "Plan: We need to inspect src/network/turn_engine.rs to check how single turns execute.";
    assert_eq!(
        detector.record_turn_reasoning(plan, false),
        ReasoningLoopStatus::Ok
    );
    assert_eq!(
        detector.record_turn_reasoning(plan, false),
        ReasoningLoopStatus::LoopDetected(DIAG_CROSS_TURN_SAME_PLAN)
    );

    // Workspace progress resets cross-turn plan tracking
    detector.record_turn_reasoning(plan, true);
    assert_eq!(
        detector.record_turn_reasoning(plan, false),
        ReasoningLoopStatus::Ok
    );
}

#[test]
fn reasoning_loop_detector_catches_paraphrased_same_plan() {
    let mut detector = ReasoningLoopDetector::default();
    let turn1 = "I will modify src/network/turn_engine.rs to implement the loop recovery logic for reasoning loops.";
    let turn2 = "We are ready to alter src/network/turn_engine.rs to add the loop recovery behavior for reasoning streams.";

    assert_eq!(
        detector.record_turn_evidence(&TurnEvidence {
            reasoning: turn1,
            target_files: &["src/network/turn_engine.rs"],
            made_progress: false,
            had_edits: false,
            tool_count: 1,
            no_progress_streak: 1,
        }),
        ReasoningLoopStatus::Ok
    );

    assert_eq!(
        detector.record_turn_evidence(&TurnEvidence {
            reasoning: turn2,
            target_files: &["src/network/turn_engine.rs"],
            made_progress: false,
            had_edits: false,
            tool_count: 1,
            no_progress_streak: 2,
        }),
        ReasoningLoopStatus::LoopDetected(DIAG_CROSS_TURN_SAME_PLAN)
    );
}

#[test]
fn reasoning_loop_detector_catches_ready_to_implement_hesitation_loop() {
    let mut detector = ReasoningLoopDetector::default();
    let turn1 = "The architecture is clear. I am ready to implement the changes in src/network.rs. Let's do one more check on the helper functions.";
    let turn2 = "We have confirmed the helper functions. Now proceed with implementation in src/network.rs. Let me do a quick check on the return types first.";

    assert_eq!(
        detector.record_turn_evidence(&TurnEvidence {
            reasoning: turn1,
            target_files: &["src/network.rs"],
            made_progress: false,
            had_edits: false,
            tool_count: 1,
            no_progress_streak: 1,
        }),
        ReasoningLoopStatus::Ok
    );

    assert_eq!(
        detector.record_turn_evidence(&TurnEvidence {
            reasoning: turn2,
            target_files: &["src/network.rs"],
            made_progress: false,
            had_edits: false,
            tool_count: 1,
            no_progress_streak: 2,
        }),
        ReasoningLoopStatus::LoopDetected(DIAG_SEMANTIC_NO_PROGRESS)
    );
}

#[test]
fn reasoning_loop_detector_catches_local_model_write_announcements_followed_by_reads() {
    let mut detector = ReasoningLoopDetector::default();
    let first = "The types are understood. Let me write the implementation now, after viewing the config once more.";
    let second = "The config confirms the values. Let me write the code now, but first inspect the existing test style.";

    assert_eq!(
        detector.record_turn_evidence(&TurnEvidence {
            reasoning: first,
            target_files: &["src/config.ts"],
            made_progress: true,
            had_edits: false,
            tool_count: 1,
            no_progress_streak: 0,
        }),
        ReasoningLoopStatus::Ok
    );
    assert_eq!(
        detector.record_turn_evidence(&TurnEvidence {
            reasoning: second,
            target_files: &["src/example.test.ts"],
            made_progress: true,
            had_edits: false,
            tool_count: 1,
            no_progress_streak: 0,
        }),
        ReasoningLoopStatus::LoopDetected(DIAG_SEMANTIC_NO_PROGRESS)
    );
}

#[test]
fn reasoning_loop_detector_catches_same_files_no_progress() {
    let mut detector = ReasoningLoopDetector::default();
    let file = "src/app/state.rs";

    let turn0 = "Turn 0: Inspecting the AppState struct definition in src/app/state.rs.";
    assert_eq!(
        detector.record_turn_evidence(&TurnEvidence {
            reasoning: turn0,
            target_files: &[file],
            made_progress: false,
            had_edits: false,
            tool_count: 1,
            no_progress_streak: 1,
        }),
        ReasoningLoopStatus::Ok
    );

    let turn1 = "Turn 1: Checking TokenUsage calculations and prompt metrics in src/app/state.rs.";
    assert_eq!(
        detector.record_turn_evidence(&TurnEvidence {
            reasoning: turn1,
            target_files: &[file],
            made_progress: false,
            had_edits: false,
            tool_count: 1,
            no_progress_streak: 2,
        }),
        ReasoningLoopStatus::Ok
    );

    let turn2 = "Turn 2: Viewing session history storage vector in src/app/state.rs.";
    assert_eq!(
        detector.record_turn_evidence(&TurnEvidence {
            reasoning: turn2,
            target_files: &[file],
            made_progress: false,
            had_edits: false,
            tool_count: 1,
            no_progress_streak: 3,
        }),
        ReasoningLoopStatus::LoopDetected(DIAG_SAME_FILES_NO_PROGRESS)
    );
}

#[test]
fn wide_repository_investigation_does_not_trigger_loop() {
    let mut detector = ReasoningLoopDetector::default();
    let files = [
        "src/app/state.rs",
        "src/network/turn_engine.rs",
        "src/tools/exec.rs",
        "src/ui/mod.rs",
        "src/config.rs",
        "src/main.rs",
    ];

    for (idx, file) in files.iter().enumerate() {
        let reasoning = format!(
            "Step {idx}: Exploring module {file} to map codebase architecture and relationships."
        );
        assert_eq!(
            detector.record_turn_evidence(&TurnEvidence {
                reasoning: &reasoning,
                target_files: &[file],
                made_progress: false,
                had_edits: false,
                tool_count: 1,
                no_progress_streak: idx + 1,
            }),
            ReasoningLoopStatus::Ok,
            "legitimate broad exploration of {file} should not trigger loop detector"
        );
    }
}

#[test]
fn semantic_paragraph_similarity_in_stream() {
    let mut detector = ReasoningLoopDetector::default();
    let p1 = "We must carefully inspect the turn execution loop in src/network/turn_engine.rs to verify how recovery actions are triggered.\n\n";
    let p2 = "We should carefully inspect the turn execution loop in src/network/turn_engine.rs to verify how recovery actions are triggered.\n\n";

    assert_eq!(detector.feed_chunk(p1), ReasoningLoopStatus::Ok);
    assert_eq!(
        detector.feed_chunk(p2),
        ReasoningLoopStatus::LoopDetected(DIAG_REPEATED_BLOCK)
    );
}
