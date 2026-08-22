use super::*;

// Regression: session 1785594233488. A read of exactly lines 1-1 ended with
// "content truncated (use end_line or content_offset to read more)", so the
// model believed it had missed something and re-read the same file four
// times, tripping the loop detector.
#[test]
fn a_read_that_ends_where_asked_is_not_reported_as_truncated() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("symbols.rs");
    std::fs::write(&file, "// scratch\nuse std::fs;\nfn main() {}\n").expect("write");
    let path = file.to_string_lossy().to_string();

    let ranged = view_file_tool(&serde_json::json!({
        "path": path,
        "start_line": 1,
        "end_line": 1,
    }))
    .expect("read");
    assert!(ranged.contains("1: // scratch"), "got: {ranged}");
    assert!(!ranged.contains("truncated"), "got: {ranged}");
    assert!(
        ranged.contains("the file continues to line 3"),
        "got: {ranged}"
    );

    // A read the tool itself cut short still says so.
    let whole = view_file_tool(&serde_json::json!({ "path": path })).expect("read");
    assert!(!whole.contains("truncated"), "got: {whole}");
    assert!(!whole.contains("continues"), "got: {whole}");
}

#[test]
fn replacement_preserves_mixed_line_endings() {
    let content = "first\r\nold\nlast\r\n";
    let edit = SingleEdit {
        target: "old".to_string(),
        replacement: "new\ninserted".to_string(),
        start_line: None,
        end_line: None,
    };

    let outcome = apply_single_edit_to_content(content, "mixed.txt", &edit).expect("edit");

    let EditOutcome::Changed(result) = outcome else {
        panic!("edit unexpectedly unchanged");
    };
    assert_eq!(result, "first\r\nnew\ninserted\nlast\r\n");
}

#[test]
fn replacement_preserves_uniform_crlf_line_endings() {
    let content = "first\r\nold\r\nlast\r\n";
    let edit = SingleEdit {
        target: "old".to_string(),
        replacement: "new\ninserted".to_string(),
        start_line: None,
        end_line: None,
    };

    let outcome = apply_single_edit_to_content(content, "crlf.txt", &edit).expect("edit");

    let EditOutcome::Changed(result) = outcome else {
        panic!("edit unexpectedly unchanged");
    };
    assert_eq!(result, "first\r\nnew\r\ninserted\r\nlast\r\n");
}

// Feature 5 (lossless reads): a read genuinely cut short by view_file's own
// default window must say so unambiguously, name exactly which lines were
// omitted, and tell the model exactly how to fetch them.
#[test]
fn a_read_cut_short_by_the_default_window_names_the_omitted_range() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("big.txt");
    let total_lines = DEFAULT_READ_WINDOW_LINES + 500;
    let content: String = (1..=total_lines).map(|n| format!("line {n}\n")).collect();
    std::fs::write(&file, &content).expect("write");
    let path = file.to_string_lossy().to_string();

    // No end_line given: the tool applies its default window and must
    // clearly flag the read as incomplete. The inclusive window contains
    // exactly DEFAULT_READ_WINDOW_LINES lines.
    let out = view_file_tool(&serde_json::json!({ "path": path, "start_line": 1 })).expect("read");
    assert!(out.contains("[Truncated:"), "got: {out}");
    let window_end = DEFAULT_READ_WINDOW_LINES;
    let expected_next = window_end + 1;
    assert!(
        out.contains(&format!(
            "lines {expected_next}-{total_lines} of {total_lines}"
        )),
        "expected omitted range {expected_next}-{total_lines} named, got: {out}"
    );
    assert!(
        out.contains(&format!("start_line={expected_next}")),
        "expected follow-up start_line hint, got: {out}"
    );
    assert!(
        out.contains(&format!("end_line={total_lines}")),
        "expected follow-up end_line hint, got: {out}"
    );
    assert!(out.contains(&format!("[File: {}, Lines 1 to {window_end} of", path)));
    assert_eq!(
        out.lines().filter(|line| line.contains(": line ")).count(),
        DEFAULT_READ_WINDOW_LINES
    );
    // Last line actually shown is the last line of the default window, not
    // the true end of the file.
    assert!(out.contains(&format!("{window_end}: line {window_end}")));
    assert!(!out.contains(&format!("{}: line {}", window_end + 1, window_end + 1)));
    assert!(!out.contains(&format!("{total_lines}: line {total_lines}")));
}

// An explicit end_line far beyond the hard cap must be bounded, not
// honored — otherwise a single huge requested range (e.g. a model asking
// for the whole file at once) bypasses the safety window entirely and
// returns thousands of lines in one tool result.
#[test]
fn an_oversized_explicit_range_is_bounded_and_reported_as_capped_not_complete() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("huge.txt");
    let total_lines = DEFAULT_READ_WINDOW_LINES * 4;
    let content: String = (1..=total_lines).map(|n| format!("line {n}\n")).collect();
    std::fs::write(&file, &content).expect("write");
    let path = file.to_string_lossy().to_string();

    let out = view_file_tool(&serde_json::json!({
        "path": path,
        "start_line": 1,
        "end_line": total_lines,
    }))
    .expect("read");

    let window_end = DEFAULT_READ_WINDOW_LINES;
    // Bounded to the cap, not the requested end_line.
    assert!(
        out.contains(&format!("{window_end}: line {window_end}")),
        "got: {out}"
    );
    assert!(
        !out.contains(&format!("{total_lines}: line {total_lines}")),
        "got: {out}"
    );
    assert_eq!(
        out.lines().filter(|line| line.contains(": line ")).count(),
        DEFAULT_READ_WINDOW_LINES,
        "expected exactly {DEFAULT_READ_WINDOW_LINES} returned content lines, got: {out}"
    );
    // Reported as a genuine, capped truncation — not as "end of requested
    // range", which would falsely imply the read is complete.
    assert!(out.contains("[Truncated:"), "got: {out}");
    assert!(out.contains("capped at"), "got: {out}");
    assert!(!out.contains("end of requested range"), "got: {out}");
    let next_start = window_end + 1;
    assert!(
        out.contains(&format!("start_line={next_start}")),
        "got: {out}"
    );
    assert!(
        out.contains(&format!("end_line={total_lines}")),
        "got: {out}"
    );
}

// Feature 5: after a truncated read, following the tool's own recovery
// instructions (start_line/end_line for exactly the omitted range) must
// return exactly that content — not a repeat of the first chunk, not a
// different range.
#[test]
fn targeted_follow_up_read_recovers_exactly_the_omitted_range() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("big.txt");
    // Kept within the hard cap for the second, targeted read too — a
    // recovery range larger than the cap would itself be bounded, which
    // is covered separately by
    // `an_oversized_explicit_range_is_bounded_and_reported_as_capped_not_complete`.
    let total_lines = DEFAULT_READ_WINDOW_LINES + 100;
    let content: String = (1..=total_lines).map(|n| format!("line {n}\n")).collect();
    std::fs::write(&file, &content).expect("write");
    let path = file.to_string_lossy().to_string();

    let first =
        view_file_tool(&serde_json::json!({ "path": path, "start_line": 1 })).expect("read");
    // start_line=1, so the default window contains exactly
    // DEFAULT_READ_WINDOW_LINES lines.
    let next_start = DEFAULT_READ_WINDOW_LINES + 1;

    let second = view_file_tool(&serde_json::json!({
        "path": path,
        "start_line": next_start,
        "end_line": total_lines,
    }))
    .expect("read");

    // Second chunk contains exactly the lines the first chunk said were
    // missing, and none of the content the first chunk already showed.
    assert!(
        second.contains(&format!("{next_start}: line {next_start}")),
        "got: {second}"
    );
    assert!(
        second.contains(&format!("{total_lines}: line {total_lines}")),
        "got: {second}"
    );
    assert!(!second.contains("1: line 1\n"), "got: {second}");
    // It's a real different read, not a repeat of the first chunk.
    assert_ne!(first, second);
    // It ended exactly where asked, so it is not itself reported truncated.
    assert!(!second.contains("Truncated"), "got: {second}");
}

// Feature 5: reading a file in two truncated-window chunks must not lose
// content that spans the boundary between them — a target string sitting
// right at the seam must be fully recoverable from a follow-up read, so an
// exact-match edit downstream has a real chance of matching what the
// second chunk actually shows.
#[test]
fn two_chunk_read_does_not_drop_content_at_the_seam() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("seam.txt");
    // Put a distinctive marker just after the default window boundary so
    // it would NOT be visible in a first, window-limited read. start_line=1,
    // so the default window's last shown line is
    // DEFAULT_READ_WINDOW_LINES (inclusive).
    let marker_line = DEFAULT_READ_WINDOW_LINES + 1;
    let total_lines = DEFAULT_READ_WINDOW_LINES + 12;
    let mut content = String::new();
    for n in 1..=total_lines {
        if n == marker_line {
            content.push_str("UNIQUE_TARGET_MARKER\n");
        } else {
            content.push_str(&format!("line {n}\n"));
        }
    }
    std::fs::write(&file, &content).expect("write");
    let path = file.to_string_lossy().to_string();

    let first =
        view_file_tool(&serde_json::json!({ "path": path, "start_line": 1 })).expect("read");
    assert!(
        !first.contains("UNIQUE_TARGET_MARKER"),
        "marker should be past the default window, got: {first}"
    );

    // Follow the tool's own recovery instructions for the omitted range.
    let next_start = DEFAULT_READ_WINDOW_LINES + 1;
    let second = view_file_tool(&serde_json::json!({
        "path": path,
        "start_line": next_start,
        "end_line": total_lines,
    }))
    .expect("read");
    assert!(
        second.contains("UNIQUE_TARGET_MARKER"),
        "the seam-adjacent line must be recoverable via a targeted follow-up read, got: {second}"
    );
}

#[test]
fn content_offset_preserves_global_line_numbers_and_bounded_follow_up_ranges() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("offset.txt");
    let total_lines = DEFAULT_READ_WINDOW_LINES + 150;
    let content: String = (1..=total_lines).map(|n| format!("line {n}\n")).collect();
    let byte_offset = content.find("line 2\n").expect("line 2 offset");
    std::fs::write(&file, &content).expect("write");
    let path = file.to_string_lossy().to_string();

    let first = view_file_tool(&serde_json::json!({
        "path": path,
        "content_offset": byte_offset,
        "start_line": 2,
    }))
    .expect("read");

    assert!(first.contains(&format!(
        "[File: {}, Lines 2 to {} of {total_lines}, Bytes offset: {byte_offset}]",
        path,
        1 + DEFAULT_READ_WINDOW_LINES
    )));
    assert_eq!(
        first
            .lines()
            .filter(|line| line.contains(": line "))
            .count(),
        DEFAULT_READ_WINDOW_LINES
    );
    assert!(first.contains("2: line 2"));
    assert!(first.contains(&format!(
        "{}: line {}",
        1 + DEFAULT_READ_WINDOW_LINES,
        1 + DEFAULT_READ_WINDOW_LINES
    )));
    assert!(!first.contains(&format!(
        "{}: line {}",
        2 + DEFAULT_READ_WINDOW_LINES,
        2 + DEFAULT_READ_WINDOW_LINES
    )));

    let next_start = 2 + DEFAULT_READ_WINDOW_LINES;
    let second = view_file_tool(&serde_json::json!({
        "path": path,
        "content_offset": byte_offset,
        "start_line": next_start,
        "end_line": total_lines,
    }))
    .expect("read");

    assert!(second.contains(&format!("{next_start}: line {next_start}")));
    assert!(!second.lines().any(|line| line == "2: line 2"));
    let previous_line = format!("{}: line {}", next_start - 1, next_start - 1);
    assert!(!second.lines().any(|line| line == previous_line));
}

// Feature 5: two different range requests on the same file must return
// genuinely different output — repeated reads are not silently collapsed
// into the same content regardless of the requested range.
#[test]
fn varying_range_arguments_change_the_returned_content() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("small.txt");
    std::fs::write(&file, "a\nb\nc\nd\ne\n").expect("write");
    let path = file.to_string_lossy().to_string();

    let first = view_file_tool(&serde_json::json!({
        "path": path, "start_line": 1, "end_line": 2,
    }))
    .expect("read");
    let second = view_file_tool(&serde_json::json!({
        "path": path, "start_line": 3, "end_line": 4,
    }))
    .expect("read");
    assert_ne!(first, second);
    assert!(first.contains("1: a") && first.contains("2: b"));
    assert!(second.contains("3: c") && second.contains("4: d"));

    let via_offset = view_file_tool(&serde_json::json!({
        "path": path, "content_offset": 4, // byte offset into "a\nb\nc\nd\ne\n"
    }))
    .expect("read");
    assert!(via_offset.contains("c") || via_offset.contains("d") || via_offset.contains("e"));
    assert_ne!(via_offset, first);
}

#[test]
fn malformed_view_file_ranges_return_clear_errors() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("ranges.txt");
    std::fs::write(&file, "one\ntwo\nthree\n").expect("write");
    let path = file.to_string_lossy().to_string();
    let cases = [
        (
            serde_json::json!({"path": path, "start_line": 2, "end_line": 1}),
            "end_line",
        ),
        (
            serde_json::json!({"path": path, "start_line": 0, "end_line": 1}),
            "start_line",
        ),
        (
            serde_json::json!({"path": path, "start_line": 1, "end_line": 0}),
            "end_line",
        ),
        (
            serde_json::json!({"path": path, "start_line": "18446744073709551616"}),
            "start_line",
        ),
        (
            serde_json::json!({"path": path, "end_line": "18446744073709551616"}),
            "end_line",
        ),
        (
            serde_json::json!({"path": path, "content_offset": "18446744073709551616"}),
            "content_offset",
        ),
    ];

    for (args, expected) in cases {
        let error = view_file_tool(&args).expect_err("malformed range must be rejected");
        assert!(error.contains(expected), "expected {expected} in: {error}");
    }

    let empty = dir.path().join("empty.txt");
    std::fs::write(&empty, "").expect("write empty file");
    let error = view_file_tool(&serde_json::json!({
        "path": empty.to_string_lossy(),
        "start_line": 1,
        "end_line": 1,
    }))
    .expect_err("a range cannot address empty sliced content");
    assert!(error.contains("out of bounds"), "got: {error}");
}

#[test]
fn malformed_offset_ranges_return_errors_without_panicking() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("offset-ranges.txt");
    let content = "one\ntwo\nthree\n";
    std::fs::write(&file, content).expect("write");
    let path = file.to_string_lossy().to_string();
    let line_three_offset = content.find("three").expect("line three offset");
    let cases = [
        (
            serde_json::json!({
                "path": path,
                "content_offset": line_three_offset,
                "end_line": 1,
            }),
            "end_line 1 must be greater than or equal to start_line 3",
        ),
        (
            serde_json::json!({
                "path": path,
                "content_offset": line_three_offset,
                "end_line": 0,
            }),
            "end_line must be at least 1",
        ),
        (
            serde_json::json!({
                "path": path,
                "content_offset": "18446744073709551616",
                "end_line": 1,
            }),
            "content_offset",
        ),
    ];

    for (args, expected) in cases {
        let result = std::panic::catch_unwind(|| view_file_tool(&args));
        let tool_result = result.expect("malformed range must return Err, not panic");
        let error = tool_result.expect_err("malformed range must be rejected");
        assert!(
            error.contains(expected),
            "expected {expected:?} in: {error}"
        );
    }
}

#[test]
fn view_file_rejects_invalid_utf8_offsets_and_ranges_outside_the_slice() {
    let dir = tempfile::tempdir().expect("tempdir");
    let utf8_file = dir.path().join("utf8.txt");
    std::fs::write(&utf8_file, "éclair\nsecond\nthird\n").expect("write");
    let utf8_path = utf8_file.to_string_lossy().to_string();

    let boundary_error = view_file_tool(&serde_json::json!({
        "path": utf8_path,
        "content_offset": 1,
    }))
    .expect_err("offset inside a UTF-8 code point must be rejected");
    assert!(
        boundary_error.contains("UTF-8 boundary"),
        "got: {boundary_error}"
    );

    let slice_offset = "éclair\n".len();
    let range_error = view_file_tool(&serde_json::json!({
        "path": utf8_path,
        "content_offset": slice_offset,
        "start_line": 1,
        "end_line": 1,
    }))
    .expect_err("range before sliced content must be rejected");
    assert!(range_error.contains("out of bounds"), "got: {range_error}");

    let zero_offset = view_file_tool(&serde_json::json!({
        "path": utf8_path,
        "content_offset": 0,
        "start_line": 1,
        "end_line": 1,
    }))
    .expect("zero is a valid byte offset");
    assert!(zero_offset.contains("1: éclair"));
}

// Regression: session 1785594233488. The model tried to prepend a line by
// sending an empty old_string; it matched at every offset and came back as
// "found 13573 matches", which reads like the file is at fault. The model
// then retried the same shape through multi_replace and gave up on editing.
#[test]
fn empty_edit_anchor_is_rejected_with_insert_guidance() {
    let edit = SingleEdit {
        target: String::new(),
        replacement: "// scratch\n".to_string(),
        start_line: None,
        end_line: None,
    };

    let error = apply_single_edit_to_content("line one\nline two\n", "src/symbols.rs", &edit)
        .expect_err("an empty anchor cannot identify an edit site");

    assert!(error.contains("empty"), "got: {error}");
    assert!(error.contains("matches everywhere"), "got: {error}");
    // Points at what to do instead, since the model wanted to insert.
    assert!(error.contains("prepend"), "got: {error}");
    assert!(!error.contains("found 2 matches"), "got: {error}");
}

// Regression: session 1785315367588 looped because the model's target
// block had a single backslash in the char literal (`'\'`) where the file
// has an escaped one (`'\\'`), and the block-anchor fallback bailed on the
// inner `}` before reaching the function's closing `}`.
#[test]
fn resolves_block_with_single_line_escape_drift() {
    let file = "pub fn show_spinner() {\n\
                        let spinner = vec!['|', '/', '-', '\\\\'];\n\
                    let mut i = 0;\n\
                    while running {\n\
                    print!(\"x\");\n\
                    }\n\
                    flush();\n\
                    }\n";
    // Target differs only on the spinner line (one backslash, not two).
    let edit = SingleEdit {
        target: "pub fn show_spinner() {\n\
                     let spinner = vec!['|', '/', '-', '\\'];\n\
                     let mut i = 0;\n\
                     while running {\n\
                     print!(\"x\");\n\
                     }\n\
                     flush();\n\
                     }"
        .to_string(),
        replacement: "REPLACED".to_string(),
        start_line: None,
        end_line: None,
    };
    let out = match apply_single_edit_to_content(file, "src/ui/mod.rs", &edit)
        .expect("should resolve via block-anchor fuzzy match")
    {
        EditOutcome::Changed(content) => content,
        EditOutcome::Unchanged => panic!("expected a change"),
    };
    assert!(out.contains("REPLACED"), "got: {out}");
    assert!(
        !out.contains("show_spinner"),
        "old block should be gone: {out}"
    );
}

#[test]
fn missing_target_error_shows_closest_region() {
    let file = "line one\nlet x = 1;\nline three\n";
    let edit = SingleEdit {
        target: "let x = 2;".to_string(),
        replacement: "let x = 3;".to_string(),
        start_line: None,
        end_line: None,
    };
    let err = apply_single_edit_to_content(file, "f.rs", &edit).unwrap_err();
    assert!(err.contains("Closest region"), "got: {err}");
    assert!(
        err.contains("let x = 1;"),
        "should quote actual line: {err}"
    );
}

#[test]
fn closest_region_is_bounded_for_large_inputs() {
    let content = (0..10_000)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let target = (0..100)
        .map(|i| format!("target {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(closest_region(&content, &target).is_none());
}

// Regression for the benchmark session: a prepend-shaped edit (old_string
// is a suffix of new_string) kept matching after it had already been
// applied, so repeating the exact same tool call stacked the inserted
// line forever.
#[test]
fn repeating_the_same_prepend_edit_twice_does_not_duplicate() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("state.rs");
    std::fs::write(&file, "    s.discord_rpc.set_activity(\"Idle\", ...);\n").expect("write");
    let path = file.to_string_lossy().to_string();

    let args = serde_json::json!({
        "path": path,
        "old_string": "    s.discord_rpc.set_activity(\"Idle\", ...);",
        "new_string": "    let model_name = ...;\n    s.discord_rpc.set_activity(\"Idle\", ...);",
    });

    let first = replace_file_content_tool(&args).expect("first apply should succeed");
    assert!(first.contains("successfully"), "got: {first}");
    let after_first = std::fs::read_to_string(&file).expect("read");
    assert_eq!(
        after_first.matches("let model_name").count(),
        1,
        "got: {after_first}"
    );

    let second = replace_file_content_tool(&args).expect("repeat should not error");
    assert!(second.contains("already applied"), "got: {second}");
    let after_second = std::fs::read_to_string(&file).expect("read");
    assert_eq!(
        after_second, after_first,
        "repeating the edit must not change the file further"
    );
    assert_eq!(
        after_second.matches("let model_name").count(),
        1,
        "the inserted line must not be duplicated: {after_second}"
    );
}

// Repeating a plain (non-insert-shaped) replacement after it already
// landed must be a reported no-op, not an error and not a second write.
#[test]
fn repeating_a_plain_replacement_is_a_noop() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("state.rs");
    std::fs::write(&file, "let status = Idle;\n").expect("write");
    let path = file.to_string_lossy().to_string();

    let args = serde_json::json!({
        "path": path,
        "old_string": "let status = Idle;",
        "new_string": "let status = Active;",
    });

    let first = replace_file_content_tool(&args).expect("first apply should succeed");
    assert!(first.contains("successfully"), "got: {first}");

    let second = replace_file_content_tool(&args).expect("repeat should be a no-op");
    assert!(second.contains("already applied"), "got: {second}");

    let content = std::fs::read_to_string(&file).expect("read");
    assert_eq!(content, "let status = Active;\n");
}

// A different, still-valid edit must keep working after the file has
// already been changed by a prior edit — the idempotency guard must not
// block genuine follow-up edits.
#[test]
fn a_legitimate_second_edit_still_works_after_the_file_changed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("state.rs");
    std::fs::write(&file, "let a = 1;\nlet b = 2;\n").expect("write");
    let path = file.to_string_lossy().to_string();

    let first = replace_file_content_tool(&serde_json::json!({
        "path": path,
        "old_string": "let a = 1;",
        "new_string": "let a = 100;",
    }))
    .expect("first edit should succeed");
    assert!(first.contains("successfully"), "got: {first}");

    let second = replace_file_content_tool(&serde_json::json!({
        "path": path,
        "old_string": "let b = 2;",
        "new_string": "let b = 200;",
    }))
    .expect("second, distinct edit should succeed");
    assert!(second.contains("successfully"), "got: {second}");

    let content = std::fs::read_to_string(&file).expect("read");
    assert_eq!(content, "let a = 100;\nlet b = 200;\n");
}

// Ambiguous edits must still be rejected — the idempotency guard must
// never paper over a genuinely ambiguous target.
#[test]
fn multiple_matches_remain_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("state.rs");
    std::fs::write(&file, "dup();\ndup();\n").expect("write");
    let path = file.to_string_lossy().to_string();

    let err = replace_file_content_tool(&serde_json::json!({
        "path": path,
        "old_string": "dup();",
        "new_string": "single();",
    }))
    .expect_err("ambiguous target must be rejected");
    assert!(err.contains("matches"), "got: {err}");

    let content = std::fs::read_to_string(&file).expect("read");
    assert_eq!(content, "dup();\ndup();\n", "file must be untouched");
}

// A failed edit (target genuinely not present) must return Err, never a
// success message — success-shaped failures are what let a broken edit
// loop pass the harness's "did the tool report success" check.
#[test]
fn failed_edits_do_not_report_success() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("state.rs");
    std::fs::write(&file, "let a = 1;\n").expect("write");
    let path = file.to_string_lossy().to_string();

    let err = replace_file_content_tool(&serde_json::json!({
        "path": path,
        "old_string": "let z = 999;",
        "new_string": "let z = 1000;",
    }))
    .expect_err("target not present must fail");
    assert!(err.contains("not found"), "got: {err}");

    let content = std::fs::read_to_string(&file).expect("read");
    assert_eq!(content, "let a = 1;\n", "file must be untouched on failure");
}

// multi_replace_file_content_tool must share the same idempotency
// protection as the single-edit path.
#[test]
fn multi_replace_repeated_call_is_a_noop() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("state.rs");
    std::fs::write(&file, "let a = 1;\nlet b = 2;\n").expect("write");
    let path = file.to_string_lossy().to_string();

    let args = serde_json::json!({
        "path": path,
        "replacements": [
            { "start_line": 1, "end_line": 1, "target_content": "let a = 1;", "replacement_content": "let a = 100;" },
        ],
    });

    let first = multi_replace_file_content_tool(&args).expect("first apply should succeed");
    assert!(first.contains("successfully"), "got: {first}");

    let second = multi_replace_file_content_tool(&args).expect("repeat should be a no-op");
    assert!(second.contains("already applied"), "got: {second}");

    let content = std::fs::read_to_string(&file).expect("read");
    assert_eq!(content, "let a = 100;\nlet b = 2;\n");
}

fn diff_block_of(result: &str) -> &str {
    result
        .split_once("```diff\n")
        .and_then(|(_, rest)| rest.split_once("\n```"))
        .map(|(diff, _)| diff)
        .unwrap_or_else(|| panic!("expected a ```diff block in: {result}"))
}

// Feature 4 regression: the diff shown to the caller must reflect the real
// file content before/after the edit, not the tool call's raw arguments.
// A one-line replacement must produce a real, correctly numbered diff.
#[test]
fn one_line_replacement_produces_real_diff_with_correct_line_numbers() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("state.rs");
    std::fs::write(&file, "let a = 1;\nlet b = 2;\nlet c = 3;\n").expect("write");
    let path = file.to_string_lossy().to_string();

    let result = replace_file_content_tool(&serde_json::json!({
        "path": path,
        "old_string": "let b = 2;",
        "new_string": "let b = 200;",
    }))
    .expect("edit should succeed");

    let diff = diff_block_of(&result);
    assert!(diff.contains("@@ -1,3 +1,3 @@"), "got: {diff}");
    assert!(diff.contains("-let b = 2;"), "got: {diff}");
    assert!(diff.contains("+let b = 200;"), "got: {diff}");
    // Unrelated lines are untouched context, not fabricated changes.
    assert!(diff.contains(" let a = 1;"), "got: {diff}");
    assert!(diff.contains(" let c = 3;"), "got: {diff}");
}

// An insertion (prepend-shaped edit, PR #306's example) must produce a
// diff showing only the inserted line as `+`, with the untouched anchor
// line shown as context — not as a fabricated replacement of the whole
// target block.
#[test]
fn insertion_produces_real_diff() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("state.rs");
    std::fs::write(&file, "    s.discord_rpc.set_activity(\"Idle\", ...);\n").expect("write");
    let path = file.to_string_lossy().to_string();

    let result = replace_file_content_tool(&serde_json::json!({
        "path": path,
        "old_string": "    s.discord_rpc.set_activity(\"Idle\", ...);",
        "new_string": "    let model_name = ...;\n    s.discord_rpc.set_activity(\"Idle\", ...);",
    }))
    .expect("insertion should succeed");

    let diff = diff_block_of(&result);
    assert!(diff.contains("+    let model_name = ...;"), "got: {diff}");
    assert!(
        diff.contains(" ") && diff.contains("s.discord_rpc.set_activity"),
        "unchanged anchor line should appear as context, not a fabricated change: {diff}"
    );
    assert!(
        !diff.contains("-    s.discord_rpc.set_activity"),
        "the anchor line was never removed, so it must not appear as deleted: {diff}"
    );
}

// A deletion must produce a diff with only `-` lines for the removed
// content, reflecting the real file, not argument text.
#[test]
fn deletion_produces_real_diff() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("state.rs");
    std::fs::write(&file, "let a = 1;\nlet b = 2;\nlet c = 3;\n").expect("write");
    let path = file.to_string_lossy().to_string();

    let result = replace_file_content_tool(&serde_json::json!({
        "path": path,
        "old_string": "let b = 2;\n",
        "new_string": "",
    }))
    .expect("deletion should succeed");

    let diff = diff_block_of(&result);
    assert!(diff.contains("-let b = 2;"), "got: {diff}");
    assert!(!diff.contains("+let b"), "got: {diff}");

    let content = std::fs::read_to_string(&file).expect("read");
    assert_eq!(content, "let a = 1;\nlet c = 3;\n");
}

// Multiple edits in one call must produce ONE diff reflecting the
// combined real before/after content, with correct line numbers for
// each hunk — not a per-edit fabrication stitched from arguments.
#[test]
fn multiple_edits_produce_one_combined_diff_with_correct_hunks() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("state.rs");
    let lines: Vec<String> = (1..=20).map(|i| format!("line {i}")).collect();
    std::fs::write(&file, lines.join("\n") + "\n").expect("write");
    let path = file.to_string_lossy().to_string();

    let result = replace_file_content_tool(&serde_json::json!({
        "path": path,
        "edits": [
            { "target_content": "line 2\n", "replacement_content": "LINE TWO\n" },
            { "target_content": "line 18", "replacement_content": "LINE EIGHTEEN" },
        ],
    }))
    .expect("multi-edit should succeed");

    let diff = diff_block_of(&result);
    // Two separate hunks, far enough apart that they don't share context.
    let hunk_count = diff.matches("@@").count() / 2;
    assert_eq!(hunk_count, 2, "expected two separate hunks, got: {diff}");
    assert!(diff.contains("-line 2"), "got: {diff}");
    assert!(diff.contains("+LINE TWO"), "got: {diff}");
    assert!(diff.contains("-line 18"), "got: {diff}");
    assert!(diff.contains("+LINE EIGHTEEN"), "got: {diff}");
    // The second hunk's line numbers must reflect its real position near
    // line 18, not line 1 or an argument-derived number.
    assert!(
        diff.contains("@@ -16,") || diff.contains("@@ -15,"),
        "second hunk should be anchored near line 18, got: {diff}"
    );
}

// A repeated no-op edit (PR #306's idempotency case) must produce NO
// diff at all in the response, not a fake one derived from arguments.
#[test]
fn repeated_noop_edit_produces_no_diff() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("state.rs");
    std::fs::write(&file, "let status = Idle;\n").expect("write");
    let path = file.to_string_lossy().to_string();

    let args = serde_json::json!({
        "path": path,
        "old_string": "let status = Idle;",
        "new_string": "let status = Active;",
    });

    let first = replace_file_content_tool(&args).expect("first apply should succeed");
    assert!(
        first.contains("```diff"),
        "first real edit should carry a diff: {first}"
    );

    let second = replace_file_content_tool(&args).expect("repeat should be a no-op");
    assert!(second.contains("already applied"), "got: {second}");
    assert!(
        !second.contains("```diff"),
        "a no-op edit must not fabricate a diff: {second}"
    );
}

// Core regression: an edit far from the start of the file must report
// truthful line numbers in the diff hunk header — not numbers derived
// from the argument text (which would always start at line 1).
#[test]
fn diff_line_numbers_reflect_real_position_in_file_not_arguments() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("big.rs");
    let lines: Vec<String> = (1..=100).map(|i| format!("line {i}")).collect();
    std::fs::write(&file, lines.join("\n") + "\n").expect("write");
    let path = file.to_string_lossy().to_string();

    let result = replace_file_content_tool(&serde_json::json!({
        "path": path,
        "old_string": "line 50",
        "new_string": "line fifty",
    }))
    .expect("edit should succeed");

    let diff = diff_block_of(&result);
    // Context radius is 3, so the hunk should start around line 47, and
    // must NOT claim to start at line 1 as argument-derived numbering
    // would (target text "line 50" alone has no line context).
    assert!(
        diff.contains("@@ -47,"),
        "expected a hunk anchored near line 50, got: {diff}"
    );
    assert!(
        !diff.contains("@@ -1,"),
        "must not fabricate line 1 from arguments: {diff}"
    );
}

// multi_replace_file_content_tool must also produce a real diff from the
// true before/after file content, not per-replacement argument text.
#[test]
fn multi_replace_produces_real_diff() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("state.rs");
    let lines: Vec<String> = (1..=20).map(|i| format!("line {i}")).collect();
    std::fs::write(&file, lines.join("\n") + "\n").expect("write");
    let path = file.to_string_lossy().to_string();

    let result = multi_replace_file_content_tool(&serde_json::json!({
            "path": path,
            "replacements": [
                { "start_line": 18, "end_line": 18, "target_content": "line 18", "replacement_content": "LINE EIGHTEEN" },
            ],
        }))
        .expect("replacement should succeed");

    let diff = diff_block_of(&result);
    assert!(diff.contains("-line 18"), "got: {diff}");
    assert!(diff.contains("+LINE EIGHTEEN"), "got: {diff}");
    assert!(
        !diff.contains("@@ -1,"),
        "must reflect real position, not line 1: {diff}"
    );
}

// A no-op multi_replace call (all chunks already applied) must not
// fabricate a diff either.
#[test]
fn multi_replace_repeated_noop_produces_no_diff() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("state.rs");
    std::fs::write(&file, "let a = 1;\nlet b = 2;\n").expect("write");
    let path = file.to_string_lossy().to_string();

    let args = serde_json::json!({
        "path": path,
        "replacements": [
            { "start_line": 1, "end_line": 1, "target_content": "let a = 1;", "replacement_content": "let a = 100;" },
        ],
    });

    let first = multi_replace_file_content_tool(&args).expect("first apply should succeed");
    assert!(first.contains("```diff"), "got: {first}");

    let second = multi_replace_file_content_tool(&args).expect("repeat should be a no-op");
    assert!(second.contains("already applied"), "got: {second}");
    assert!(
        !second.contains("```diff"),
        "a no-op must not fabricate a diff: {second}"
    );
}

// --- edit_target_and_replacement: every supported alias pair ---
//
// This is the shared helper behind both extract_edit_chunks (the actual
// edit tools) and get_diff_preview (the confirmation-modal preview, in
// network.rs) — every alias pair a model or legacy caller might use
// must be recognized here, since both consumers rely on this one
// implementation to agree.

#[test]
fn edit_target_and_replacement_supports_target_content_replacement_content() {
    let (t, r) = edit_target_and_replacement(&serde_json::json!({
        "target_content": "old",
        "replacement_content": "new",
    }));
    assert_eq!(t.as_deref(), Some("old"));
    assert_eq!(r.as_deref(), Some("new"));
}

#[test]
fn edit_target_and_replacement_supports_target_replacement() {
    let (t, r) = edit_target_and_replacement(&serde_json::json!({
        "target": "old",
        "replacement": "new",
    }));
    assert_eq!(t.as_deref(), Some("old"));
    assert_eq!(r.as_deref(), Some("new"));
}

#[test]
fn edit_target_and_replacement_supports_old_string_new_string() {
    let (t, r) = edit_target_and_replacement(&serde_json::json!({
        "old_string": "old",
        "new_string": "new",
    }));
    assert_eq!(t.as_deref(), Some("old"));
    assert_eq!(r.as_deref(), Some("new"));
}

#[test]
fn edit_target_and_replacement_supports_old_text_new_text() {
    let (t, r) = edit_target_and_replacement(&serde_json::json!({
        "old_text": "old",
        "new_text": "new",
    }));
    assert_eq!(t.as_deref(), Some("old"));
    assert_eq!(r.as_deref(), Some("new"));
}

#[test]
fn edit_target_and_replacement_supports_camel_case_old_string() {
    let (t, r) = edit_target_and_replacement(&serde_json::json!({
        "oldString": "old",
        "newString": "new",
    }));
    assert_eq!(t.as_deref(), Some("old"));
    assert_eq!(r.as_deref(), Some("new"));
}

#[test]
fn edit_target_and_replacement_supports_camel_case_old_text() {
    let (t, r) = edit_target_and_replacement(&serde_json::json!({
        "oldText": "old",
        "newText": "new",
    }));
    assert_eq!(t.as_deref(), Some("old"));
    assert_eq!(r.as_deref(), Some("new"));
}

#[test]
fn edit_target_and_replacement_returns_none_when_no_alias_present() {
    let (t, r) = edit_target_and_replacement(&serde_json::json!({ "path": "x.rs" }));
    assert_eq!(t, None);
    assert_eq!(r, None);
}

#[test]
fn edit_target_and_replacement_prefers_canonical_keys_over_aliases() {
    let (t, r) = edit_target_and_replacement(&serde_json::json!({
        "target_content": "canonical old",
        "replacement_content": "canonical new",
        "old_string": "alias old",
        "new_string": "alias new",
    }));
    assert_eq!(t.as_deref(), Some("canonical old"));
    assert_eq!(r.as_deref(), Some("canonical new"));
}

// Regression guard: refactoring extract_edit_chunks to share
// edit_target_and_replacement must not change extract_edit_chunks's own
// behavior for any alias — including inside an `edits` array, which
// uses the same helper per-item.
#[test]
fn replace_file_content_still_works_with_every_alias_after_the_shared_helper_refactor() {
    for (target_key, replacement_key) in [
        ("target_content", "replacement_content"),
        ("target", "replacement"),
        ("old_string", "new_string"),
        ("old_text", "new_text"),
        ("oldString", "newString"),
        ("oldText", "newText"),
    ] {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("state.rs");
        std::fs::write(&file, "let a = 1;\n").expect("write");
        let path = file.to_string_lossy().to_string();

        let mut args = serde_json::json!({ "path": path });
        args[target_key] = serde_json::json!("let a = 1;");
        args[replacement_key] = serde_json::json!("let a = 100;");

        let result = replace_file_content_tool(&args)
            .unwrap_or_else(|e| panic!("alias {target_key}/{replacement_key} failed: {e}"));
        assert!(result.contains("successfully"), "got: {result}");
        let content = std::fs::read_to_string(&file).expect("read");
        assert_eq!(
            content, "let a = 100;\n",
            "alias {target_key}/{replacement_key}"
        );
    }
}

// Regression guard: a lone `start_line` (without `end_line`) must anchor
// the edit to that line. Previously anchoring required BOTH start_line and
// end_line, so start_line-only edits fell through to global matching and
// failed with "found N matches" on non-unique targets — agents then
// retried the identical call in a loop.
#[test]
fn start_line_alone_anchors_nonunique_target() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("tool_result.rs");
    std::fs::write(
        &file,
        "fn a() {\n    false,\n}\nfn b() {\n    false,\n}\nfn c() {\n    false,\n}\n",
    )
    .expect("write");
    let path = file.to_string_lossy().to_string();

    let result = replace_file_content_tool(&serde_json::json!({
        "path": path,
        "start_line": 5,
        "target_content": "    false,",
        "replacement_content": "    false,\n    &crate::app::Verbosity::Low,",
    }))
    .expect("start_line-only anchored edit should succeed");
    assert!(result.contains("successfully"), "got: {result}");

    let content = std::fs::read_to_string(&file).expect("read");
    assert_eq!(
        content,
        "fn a() {\n    false,\n}\nfn b() {\n    false,\n    &crate::app::Verbosity::Low,\n}\nfn c() {\n    false,\n}\n"
    );
}

#[test]
fn replace_file_content_fuzzy_matches_trailing_whitespace_and_indentation() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("indent.rs");
    std::fs::write(&file, "fn test() {\n    let value = 42;   \n    println!(\"{value}\");\n}\n").expect("write");
    let path = file.to_string_lossy().to_string();

    // Model target has different indentation and no trailing spaces
    let result = replace_file_content_tool(&serde_json::json!({
        "path": path,
        "target_content": "  let value = 42;\n  println!(\"{value}\");",
        "replacement_content": "    let value = 100;\n    println!(\"updated: {value}\");",
    }))
    .expect("fuzzy indentation and rstrip match should succeed");
    assert!(result.contains("successfully"), "got: {result}");

    let updated = std::fs::read_to_string(&file).expect("read");
    assert!(updated.contains("let value = 100;"), "got: {updated}");
}

#[test]
fn replace_file_content_normalises_unicode_smart_quotes_and_dashes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("doc.md");
    std::fs::write(&file, "# Title — Overview\n\nUse “smart” quotes and ‘single’ quotes.\n").expect("write");
    let path = file.to_string_lossy().to_string();

    // Model emits ASCII quotes and plain dash
    let result = replace_file_content_tool(&serde_json::json!({
        "path": path,
        "target_content": "# Title - Overview\n\nUse \"smart\" quotes and 'single' quotes.",
        "replacement_content": "# Title - Overview\n\nUse \"standard\" quotes.",
    }))
    .expect("unicode-normalized match should succeed");
    assert!(result.contains("successfully"), "got: {result}");

    let updated = std::fs::read_to_string(&file).expect("read");
    assert!(updated.contains("standard"), "got: {updated}");
}

#[test]
fn write_to_file_defaults_to_overwrite_true() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("existing.txt");
    std::fs::write(&file, "old content\n").expect("write");
    let path = file.to_string_lossy().to_string();

    // overwrite is omitted -> should default to true and succeed
    let result = write_to_file_tool(&serde_json::json!({
        "path": path,
        "content": "new content\n",
    }))
    .expect("write_to_file without explicit overwrite should succeed");
    assert!(result.contains("wrote"), "got: {result}");
    assert_eq!(std::fs::read_to_string(&file).expect("read"), "new content\n");

    // overwrite explicitly false -> should return error
    let err = write_to_file_tool(&serde_json::json!({
        "path": path,
        "content": "another\n",
        "overwrite": false,
    }))
    .expect_err("write_to_file with overwrite: false on existing file must error");
    assert!(err.contains("already exists"), "got: {err}");
}

#[test]
fn replace_file_content_schema_has_root_old_and_new_string() {
    let schema = replace_file_content_schema();
    assert!(schema["properties"].get("old_string").is_some());
    assert!(schema["properties"].get("new_string").is_some());
    assert!(schema["properties"].get("target_content").is_some());
    assert!(schema["properties"].get("replacement_content").is_some());
    assert!(schema["properties"].get("start_line").is_some());
    assert!(schema["properties"].get("end_line").is_some());
}

#[test]
fn replace_file_content_schema_declares_every_handler_alias() {
    // The handler accepts every key in EDIT_TARGET_ALIASES /
    // EDIT_REPLACEMENT_ALIASES at root and inside edits[] items; with
    // additionalProperties: false the schema must declare all of them or
    // strict validators reject calls the handler would accept.
    let schema = replace_file_content_schema();
    let root = &schema["properties"];
    let items = &schema["properties"]["edits"]["items"]["properties"];
    for alias in [
        "target_content",
        "target",
        "old_string",
        "old_text",
        "oldString",
        "oldText",
        "replacement_content",
        "replacement",
        "new_string",
        "new_text",
        "newString",
        "newText",
    ] {
        assert!(root.get(alias).is_some(), "schema missing root alias {alias}");
        assert!(
            items.get(alias).is_some(),
            "schema missing edits[] item alias {alias}"
        );
    }
    // edits[] items must not require old_string/new_string specifically: any
    // alias pair is valid for the handler.
    assert!(schema["properties"]["edits"]["items"].get("required").is_none());
}

#[test]
fn unrelated_missing_target_is_not_falsely_reported_as_already_applied() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("code.rs");
    std::fs::write(&file, "use std::path::Path;\n\nfn run() {}\n").expect("write");
    let path = file.to_string_lossy().to_string();

    // The replacement "use std::path::Path;" exists in the file, but the target
    // "fn complex_unrelated_missing_symbol()" does not exist and has zero similarity.
    // This must error with target not found, NOT report "already applied".
    let err = replace_file_content_tool(&serde_json::json!({
        "path": path,
        "old_string": "fn complex_unrelated_missing_symbol()",
        "new_string": "use std::path::Path;",
    }))
    .expect_err("unrelated missing target must error");

    assert!(err.contains("target_content not found") || err.contains("not found in"), "got: {err}");
}

#[test]
fn missing_target_with_surrounding_context_and_matching_inner_block_errors() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("service.rs");
    std::fs::write(
        &file,
        "fn calculate_total() {\n    let count = 42;\n    save_to_db(count);\n}\n",
    )
    .expect("write");
    let path = file.to_string_lossy().to_string();

    // Target contains surrounding context for a different function that does not exist,
    // while replacement matches an inner block present in the file (`let count = 42;`).
    // This must NOT be classified as "already applied"; it must error with target not found.
    let err = replace_file_content_tool(&serde_json::json!({
        "path": path,
        "old_string": "fn missing_outer_fn() {\n    let count = 42;\n    notify_admin();\n}",
        "new_string": "    let count = 42;",
    }))
    .expect_err("missing target with surrounding context must error");

    assert!(
        err.contains("target_content not found") || err.contains("not found in"),
        "got: {err}"
    );
}

#[test]
fn unrelated_same_prefix_single_line_is_not_reported_as_already_applied() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("state.rs");
    std::fs::write(&file, "let bar = 2;\n").expect("write");
    let path = file.to_string_lossy().to_string();

    // The file already contains the replacement `let bar = 2;`, and the
    // target shares indentation and the first token (`let`) with it — but
    // `let foo = 1;` may never have existed. Same statement prefix alone is
    // not proof of a prior application; this must error with target not found.
    let err = replace_file_content_tool(&serde_json::json!({
        "path": path,
        "old_string": "let foo = 1;",
        "new_string": "let bar = 2;",
    }))
    .expect_err("unrelated same-prefix statement must error");

    assert!(
        err.contains("target_content not found") || err.contains("not found in"),
        "got: {err}"
    );

    let content = std::fs::read_to_string(&file).expect("read");
    assert_eq!(content, "let bar = 2;\n", "file must be untouched");
}
