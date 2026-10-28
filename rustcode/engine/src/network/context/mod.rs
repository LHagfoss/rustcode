mod budget;
mod compact;
mod memory;
mod prune;
mod tokens;

#[cfg(test)]
pub use budget::calculate_preflight_budget;
#[allow(unused_imports)]
pub use budget::{PreflightBudget, calculate_preflight_budget_for_projection};
pub use compact::SUMMARY_MARKER;
pub(crate) use compact::durable_compaction_record_message;
pub(crate) use compact::valid_compaction_boundary;
pub(crate) use compact::with_session_summarizer;
#[cfg(test)]
pub use compact::{force_compact, maybe_compact, maybe_compact_with_local_policy};
pub use compact::{
    force_compact_with_budget, force_compact_with_budget_deterministic,
    maybe_compact_with_local_policy_and_usage, maybe_compact_with_local_policy_and_usage_mode,
};
#[allow(unused_imports)]
pub use memory::{
    STRUCTURED_MEMORY_MARKER, StructuredSessionMemory, compact_with_structured_memory,
};
#[allow(unused_imports)]
pub(crate) use memory::{compact_context_block, compact_context_line};
#[allow(unused_imports)]
pub use prune::{DEFAULT_PRUNE_TOKEN_THRESHOLD, KEEP_RECENT_TURNS};
pub use tokens::estimate_message_tokens;
pub use tokens::{estimate_tokens, estimate_tool_schema_tokens};

#[cfg(test)]
mod tests {
    use super::*;
    use super::{compact::*, prune::*, tokens::*};
    use crate::app::ChatMessage;
    use tiktoken_rs::cl100k_base;

    fn tool_msg(content: &str) -> ChatMessage {
        ChatMessage::new("tool", content)
    }

    async fn one_shot_json_server(body: serde_json::Value) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test server");
        let address = listener.local_addr().expect("test server address");
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
        format!("http://{address}")
    }

    async fn pending_response_server() -> (String, tokio::sync::oneshot::Receiver<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test server");
        let address = listener.local_addr().expect("test server address");
        let (accepted_tx, accepted_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (socket, _) = listener.accept().await.expect("accept request");
            accepted_tx.send(()).ok();
            std::future::pending::<()>().await;
            drop(socket);
        });
        (format!("http://{address}"), accepted_rx)
    }

    /// The memo must be a pure cache: whatever it returns, on a cold miss or a
    /// warm hit, has to equal what a direct encode of the same string produces.
    #[test]
    fn memoized_counts_match_a_direct_encode() {
        let bpe = cl100k_base().unwrap();
        let long_repeat = "x ".repeat(5000);
        let code_blob = format!(
            "view_file: {}",
            "fn main() { println!(\"hi\"); }\n".repeat(200)
        );
        let samples: [&str; 6] = [
            "",
            "hello world",
            "run_command: ls -la /usr/local/bin\nexit code 0",
            "emoji ✅ and accents éàü and CJK 日本語テキスト",
            &long_repeat,
            &code_blob,
        ];
        for (i, s) in samples.iter().enumerate() {
            let expected = bpe.encode_ordinary(s).len();
            // Cold (or already-warm) first call, then a guaranteed cache hit.
            assert_eq!(estimate_tokens(s), expected, "first count of sample {i}");
            assert_eq!(estimate_tokens(s), expected, "cached count of sample {i}");
        }
    }

    /// Distinct strings must not share a memo entry, and rewriting a message
    /// (as pruning does) must be counted afresh rather than hitting the old
    /// content's entry.
    #[test]
    fn memo_distinguishes_different_content() {
        let bpe = cl100k_base().unwrap();
        let original = format!("run_command: {}", "data ".repeat(400));
        let rewritten = "run_command: [Tool Output Truncated: 400 tokens reduced to summary.]";

        assert_eq!(
            estimate_tokens(&original),
            bpe.encode_ordinary(&original).len()
        );
        assert_eq!(
            estimate_tokens(rewritten),
            bpe.encode_ordinary(rewritten).len()
        );
        assert_ne!(estimate_tokens(&original), estimate_tokens(rewritten));
    }

    /// Filling the memo past its capacity must not grow it without bound, and
    /// must not corrupt the counts that survive.
    #[test]
    fn memo_stays_bounded_and_correct_under_churn() {
        let bpe = cl100k_base().unwrap();
        let hot = "the hot message that keeps being counted every pass";
        let hot_expected = bpe.encode_ordinary(hot).len();

        for i in 0..(TOKEN_MEMO_CAPACITY * 2 + 100) {
            estimate_tokens(&format!("unique filler message number {i}"));
            if i % 64 == 0 {
                assert_eq!(estimate_tokens(hot), hot_expected);
            }
        }

        let memo = TOKEN_MEMO
            .get()
            .expect("memo initialized by the calls above");
        let guard = memo.lock().unwrap_or_else(|e| e.into_inner());
        assert!(guard.live.len() <= TOKEN_MEMO_CAPACITY);
        assert!(guard.prev.len() <= TOKEN_MEMO_CAPACITY);
        drop(guard);

        assert_eq!(estimate_tokens(hot), hot_expected);
    }

    // Regression: session 1785593632937. A 6677-token read of src/main.rs was
    // collapsed to a one-line summary three round-trips later, while the window
    // was barely used, so the model could no longer answer from what it had just
    // read.
    #[tokio::test]
    async fn a_roomy_window_keeps_every_tool_output_intact() {
        let big_read = format!("view_file: {}", "line of source\n".repeat(4000));
        let mut history = vec![
            ChatMessage::new("user", "what is the binary name"),
            ChatMessage::new("assistant", "reading"),
            ChatMessage::new("tool", big_read.clone()),
        ];
        for _ in 0..8 {
            history.push(ChatMessage::new("assistant", "thinking"));
            history.push(ChatMessage::new("tool", "grep: one match"));
        }

        let client = reqwest::Client::new();
        let cancel_token = tokio_util::sync::CancellationToken::new();
        // Budget far larger than the history: nothing should be touched.
        let compacted = maybe_compact(
            &client,
            "http://unused",
            "model",
            &mut history,
            1_000_000,
            &cancel_token,
        )
        .await;

        assert!(!compacted);
        assert_eq!(history[2].content, big_read, "the read must survive");
        assert!(!history[2].content.contains("Truncated"));
    }

    #[test]
    fn prune_floor_scales_with_the_budget() {
        assert_eq!(prune_floor(100_000), 80_000);
        assert_eq!(prune_floor(8_000), 6_400);
    }

    // #985 immutable-history contract: stored messages are never rewritten.
    // Deduplication is pure selection at request-render time, so storage keeps
    // every read verbatim while the rendered request omits redundant ones.
    #[test]
    fn duplicate_old_file_reads_remain_in_storage_until_request_projection() {
        use crate::network::history::{
            RequestHistoryScope, RequestInstructions, redundant_tool_result_indices,
            to_messages_for_request_with_scope,
        };
        let same = "view_file: [File: src/lib.rs]\n1: old";
        let changed = "view_file: [File: src/lib.rs]\n1: new";
        let history = vec![
            tool_msg(same),
            ChatMessage::new("assistant", "edit").with_diff(Some("real diff".to_string())),
            tool_msg(changed),
        ];
        assert!(redundant_tool_result_indices(&history, 1).is_empty());

        let mut history = history;
        history.extend([
            ChatMessage::new("assistant", "more work"),
            ChatMessage::new("user", "verify"),
        ]);
        // Different file content is not a duplicate: everything renders.
        assert!(redundant_tool_result_indices(&history, 2).is_empty());

        history.insert(0, tool_msg(same));
        // Age the older copy out of the protected recent window so the
        // request-time rule applies.
        for i in 0..11 {
            history.push(ChatMessage::new("user", format!("follow-up {i}")));
        }
        // Storage keeps the older copy verbatim. Under budget pressure, the
        // recent-turn projection may drop stale history as a whole.
        assert!(history[0].content.contains("1: old"));
        assert_eq!(redundant_tool_result_indices(&history, 2), [0].into());
        let rendered = serde_json::to_string(&to_messages_for_request_with_scope(
            &history,
            RequestInstructions::new("system", None),
            RequestHistoryScope::RecentTurns,
        ))
        .unwrap();
        assert_eq!(rendered.matches("1: old").count(), 0);
    }

    #[test]
    fn duplicate_read_selection_requires_file_identity_and_complete_content() {
        use crate::network::history::redundant_tool_result_indices;
        let history = vec![
            tool_msg("view_file: [File: src/a.rs, Lines 1 to 1 of 1]\n1: same"),
            tool_msg("view_file: [File: src/a.rs, Lines 1 to 1 of 1]\n1: same"),
            tool_msg("view_file: [File: src/b.rs, Lines 1 to 1 of 1]\n1: same"),
            tool_msg("view_file: error: cannot read 'src/c.rs'"),
            tool_msg(
                "view_file: [File: src/d.rs, Lines 1 to 1 of 2]\n1: same\n[Truncated: lines 2-2 of 2]",
            ),
            ChatMessage::new("user", "keep recent"),
        ];
        // Only the older of the two identical a.rs reads is redundant; other
        // files, errors, and truncated reads always survive.
        assert_eq!(redundant_tool_result_indices(&history, 1), [0].into());
        assert!(history[1].content.contains("src/a.rs"));
        assert!(history[2].content.contains("src/b.rs"));
        assert!(history[3].content.contains("cannot read"));
        assert!(history[4].content.contains("Truncated"));
    }

    /// #985: a large failing output keeps its full evidence in storage. The
    /// old in-place prune pass that used to be called here is gone; only the
    /// stored message matters now.
    #[test]
    fn old_failures_keep_full_evidence_verbatim() {
        let content = format!("run_command: error: {}", "diagnostic ".repeat(3_000));
        let history = vec![tool_msg(&content)];
        assert_eq!(history[0].content, content);
    }

    #[test]
    fn summary_input_is_globally_bounded_and_keeps_task_and_recent_facts() {
        let mut history = vec![ChatMessage::new(
            "user",
            "ORIGINAL-TASK: preserve this exact objective",
        )];
        for i in 0..200 {
            history.push(ChatMessage::new(
                "tool",
                format!("OLD-FACT-{i}: {}", "x".repeat(2_000)),
            ));
        }
        history.push(ChatMessage::new(
            "assistant",
            "NEWEST-FACT: src/network/compaction.rs is the active file",
        ));
        let refs: Vec<&ChatMessage> = history.iter().collect();

        let input = build_summary_input(&refs, None);

        assert!(
            input.len() <= SUMMARY_INPUT_MAX_BYTES,
            "{} bytes",
            input.len()
        );
        assert!(input.contains("ORIGINAL-TASK: preserve this exact objective"));
        assert!(input.contains("NEWEST-FACT: src/network/compaction.rs is the active file"));
        assert!(
            !input.contains("OLD-FACT-0:"),
            "oldest bulk should be dropped first"
        );
    }

    #[test]
    fn summary_response_rejects_empty_or_invalid_provider_content() {
        let empty = serde_json::json!({
            "choices": [{"message": {"content": "  \n\t "}}]
        });
        let control_only = serde_json::json!({
            "choices": [{"message": {"content": "\u{0000}\u{0007}"}}]
        });
        let missing_content = serde_json::json!({
            "choices": [{"message": {}}]
        });

        assert!(parse_summary_response(&empty).is_none());
        assert!(parse_summary_response(&control_only).is_none());
        assert!(parse_summary_response(&missing_content).is_none());
    }

    #[test]
    fn summary_response_caps_multibyte_utf8_safely() {
        let body = serde_json::json!({
            "choices": [{
                "message": {"content": format!("useful facts {}", "é".repeat(SUMMARY_OUTPUT_MAX_BYTES))}
            }]
        });

        let summary = parse_summary_response(&body).expect("valid summary");

        assert!(
            summary.len() <= SUMMARY_OUTPUT_MAX_BYTES,
            "{} bytes",
            summary.len()
        );
        assert!(summary.starts_with("useful facts"));
        assert!(std::str::from_utf8(summary.as_bytes()).is_ok());
    }

    #[test]
    fn summary_response_rejects_output_invalidated_by_truncation() {
        let body = serde_json::json!({
            "choices": [{
                "message": {
                    "content": format!("{}visible", "\u{0007}".repeat(SUMMARY_OUTPUT_MAX_BYTES))
                }
            }]
        });

        assert!(parse_summary_response(&body).is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn summary_request_total_timeout_bounds_body_decoding() {
        let task =
            tokio::spawn(async { await_summary_request(std::future::pending::<()>(), None).await });
        tokio::task::yield_now().await;

        tokio::time::advance(SUMMARY_REQUEST_TIMEOUT + std::time::Duration::from_millis(1)).await;

        assert_eq!(
            task.await.expect("timeout task must not panic"),
            Err(SummaryRequestError::TimedOut)
        );
    }

    #[tokio::test]
    async fn summary_request_cancellation_interrupts_pending_io() {
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let request_cancel = cancel_token.clone();
        let task = tokio::spawn(async move {
            await_summary_request(std::future::pending::<()>(), Some(&request_cancel)).await
        });
        tokio::task::yield_now().await;

        cancel_token.cancel();

        assert_eq!(
            task.await.expect("cancellation task must not panic"),
            Err(SummaryRequestError::Cancelled)
        );
    }

    #[tokio::test]
    async fn manual_compaction_cancellation_interrupts_pending_summary_request() {
        let (url, request_accepted) = pending_response_server().await;
        let mut history = vec![ChatMessage::new("user", "original task")];
        // Several user turns, so the cut has a complete turn to land on.
        for index in 0..(KEEP_RECENT_TURNS + 4) {
            let role = if index % 2 == 0 { "user" } else { "assistant" };
            history.push(ChatMessage::new(role, format!("fact {index}")));
        }
        let expected: Vec<(String, String)> = history
            .iter()
            .map(|message| (message.role.clone(), message.content.clone()))
            .collect();
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let task_token = cancel_token.clone();
        let mut task = tokio::spawn(async move {
            let result = force_compact(
                &reqwest::Client::new(),
                &url,
                "model",
                &mut history,
                Some(&task_token),
            )
            .await;
            (result, history)
        });
        tokio::select! {
            accepted = request_accepted => {
                accepted.expect("manual compaction server must signal acceptance");
            }
            result = &mut task => {
                let (result, _) = result.expect("manual compaction task must not panic");
                panic!("manual compaction ended before making its request: {result:?}");
            }
            _ = tokio::time::sleep(std::time::Duration::from_secs(10)) => {
                panic!("manual compaction request must start");
            }
        }

        cancel_token.cancel();

        let (result, history) = tokio::time::timeout(std::time::Duration::from_secs(1), task)
            .await
            .expect("cancellation must interrupt manual compaction")
            .expect("manual compaction task must not panic");
        let actual: Vec<(String, String)> = history
            .iter()
            .map(|message| (message.role.clone(), message.content.clone()))
            .collect();
        assert_eq!(result, Err("Failed to generate summary.".to_string()));
        assert_eq!(actual, expected);
    }

    #[tokio::test]
    async fn cancelled_automatic_compaction_keeps_history_byte_identical() {
        let mut history = vec![ChatMessage::new("user", "keep the original task")];
        history.push(tool_msg(&format!("view_file: {}", "source ".repeat(3_000))));
        for i in 0..12 {
            history.push(ChatMessage::new(
                "assistant",
                format!("FACT-{i}: {}", "progress ".repeat(30)),
            ));
        }
        // #985: cancellation permits no storage rewrite at all — not even the
        // deterministic local excerpting the old pipeline performed.
        let expected = serde_json::to_string(&history).unwrap();
        let cancel_token = tokio_util::sync::CancellationToken::new();
        cancel_token.cancel();

        let compacted = maybe_compact(
            &reqwest::Client::new(),
            "http://unused",
            "model",
            &mut history,
            200,
            &cancel_token,
        )
        .await;

        assert!(!compacted);
        assert_eq!(serde_json::to_string(&history).unwrap(), expected);
        assert!(
            !history
                .iter()
                .any(|message| message.content.starts_with(SUMMARY_MARKER))
        );
    }

    #[tokio::test]
    async fn explicit_local_model_hint_skips_summary_request() {
        let mut history = vec![ChatMessage::new("user", "keep this task")];
        for index in 0..10 {
            history.push(ChatMessage::new("user", format!("follow-up {index}")));
            history.push(ChatMessage::new(
                "assistant",
                format!("fact {index}: {}", "detail ".repeat(120)),
            ));
        }

        let compacted = maybe_compact_with_local_policy(
            &reqwest::Client::new(),
            "http://127.0.0.1:9/v1",
            "local-qwen",
            &mut history,
            200,
            &tokio_util::sync::CancellationToken::new(),
            true,
        )
        .await;

        assert!(compacted);
        let boundary = history
            .iter()
            .find_map(|message| message.compaction_boundary.as_ref())
            .expect("local compaction must persist a typed boundary");
        assert_eq!(boundary.version, 1);
        assert!(boundary.summary.contains(STRUCTURED_MEMORY_MARKER));
    }

    /// One long turn has no complete earlier turn to cut at. `/compact`
    /// cuts inside it instead, keeping the recent messages and pinning the
    /// request the turn answers.
    #[tokio::test]
    async fn manual_compaction_of_a_single_turn_cuts_inside_it() {
        let url = one_shot_json_server(serde_json::json!({
            "choices": [{"message": {"content": "- Goal: original task"}}]
        }))
        .await;
        let mut history = vec![ChatMessage::new("user", "original task")];
        for i in 0..(KEEP_RECENT_TURNS + 8) {
            history.push(ChatMessage::new("assistant", format!("fact {i}")));
        }
        let expected_tail = history[history.len() - KEEP_RECENT_TURNS..].to_vec();

        force_compact(&reqwest::Client::new(), &url, "model", &mut history, None)
            .await
            .expect("a single turn compacts");

        assert!(history[0].content.starts_with(SUMMARY_MARKER));
        assert!(history.ends_with(&expected_tail));
        assert!(
            history
                .iter()
                .any(|message| message.content.ends_with("\noriginal task")),
            "the turn's request stays pinned"
        );
    }

    #[tokio::test]
    async fn manual_compaction_failure_keeps_history_and_returns_error() {
        let url = one_shot_json_server(serde_json::json!({
            "choices": [{"message": {"content": "  "}}]
        }))
        .await;
        let mut history = vec![ChatMessage::new("user", "original task")];
        for i in 0..(KEEP_RECENT_TURNS + 1) {
            let role = if i % 2 == 0 { "user" } else { "assistant" };
            history.push(ChatMessage::new(role, format!("fact {i}")));
        }
        let expected: Vec<(String, String)> = history
            .iter()
            .map(|message| (message.role.clone(), message.content.clone()))
            .collect();

        let result =
            force_compact(&reqwest::Client::new(), &url, "model", &mut history, None).await;

        let actual: Vec<(String, String)> = history
            .iter()
            .map(|message| (message.role.clone(), message.content.clone()))
            .collect();
        assert_eq!(result, Err("Failed to generate summary.".to_string()));
        assert_eq!(actual, expected);
        assert!(
            !history
                .iter()
                .any(|message| message.content.starts_with(SUMMARY_MARKER))
        );
    }

    /// The title comes from the first prompt. Compaction moves that prompt
    /// into the pinned-task marker, and the session must keep its name and its
    /// place in the session list (#2302).
    #[tokio::test]
    async fn compaction_keeps_the_session_title_and_content() {
        let url = one_shot_json_server(serde_json::json!({
            "choices": [{"message": {"content": "Goal: fix the redirect"}}]
        }))
        .await;
        let mut history = vec![ChatMessage::new(
            "user",
            "Fix the login redirect\nwith the details",
        )];
        for turn in 0..(KEEP_RECENT_TURNS + 3) {
            history.push(ChatMessage::new("assistant", format!("answer {turn}")));
            history.push(ChatMessage::new("user", format!("follow-up {turn}")));
        }
        history.push(ChatMessage::new("assistant", "last answer"));
        let title = crate::config::session_title(&history);
        assert_eq!(title, "Fix the login redirect");

        force_compact(&reqwest::Client::new(), &url, "model", &mut history, None)
            .await
            .expect("compaction should succeed");

        assert!(
            !history
                .iter()
                .any(|message| message.role == "user" && message.content.starts_with("Fix")),
            "the first prompt should have been summarized away"
        );
        assert_eq!(crate::config::session_title(&history), title);
        assert!(crate::config::session_has_content(&history));
    }

    #[tokio::test]
    async fn incremental_compaction_keeps_the_original_task_marker() {
        let url = one_shot_json_server(serde_json::json!({
            "choices": [{"message": {"content": "Goal: retain the original objective"}}]
        }))
        .await;
        let mut history = vec![
            ChatMessage::new("system", format!("{SUMMARY_MARKER}\nPrior facts")),
            ChatMessage::new(
                "system",
                format!("{ORIGINAL_TASK_MARKER}\noriginal objective"),
            ),
            ChatMessage::new("user", "do not change the public API"),
            ChatMessage::new("user", "current follow-up"),
        ];
        history.extend(
            (0..13).map(|index| ChatMessage::new("assistant", format!("new fact {index}"))),
        );

        force_compact(&reqwest::Client::new(), &url, "model", &mut history, None)
            .await
            .expect("incremental compaction should succeed");

        let boundary = history[0]
            .compaction_boundary
            .as_ref()
            .expect("AI summaries must persist a typed boundary");
        assert_eq!(boundary.version, 1);
        // The earlier summary is carried forward verbatim, never re-summarized.
        assert_eq!(
            boundary.summary,
            "Prior facts\n\n[Work since the previous compaction]\nGoal: retain the original objective"
        );
        assert_eq!(
            boundary
                .first_retained_entry
                .as_ref()
                .map(|entry| entry.role.as_str()),
            Some("system")
        );
        assert_eq!(
            boundary.first_retained_entry,
            history
                .get(1)
                .map(crate::app::CompactionEntry::from_message)
        );
        assert!(history.iter().any(|message| {
            message.content == format!("{ORIGINAL_TASK_MARKER}\noriginal objective")
        }));
        assert!(history.iter().any(|message| {
            message.content
                == format!("{PRESERVED_USER_REQUEST_MARKER}\ndo not change the public API")
        }));
    }

    #[test]
    fn carried_handoff_drops_old_evidence_lists() {
        let summary = "Goal: ship it\n- decided X\n\nCommand evidence (observed commands/results; classification does not assert testing or runtime validation):\nCommand evidence: {}\nRead evidence (document/source inspection only):\n";
        assert_eq!(carried_narrative(summary), "Goal: ship it\n- decided X");
        assert_eq!(carried_narrative("Goal: only"), "Goal: only");
    }

    #[test]
    fn summarizer_sees_the_earlier_handoff_as_context_only() {
        let user = ChatMessage::new("user", "add the flag");
        let input = build_summary_input(&[&user], Some("Goal: keep the API stable"));
        assert!(input.contains("Earlier handoff (context only):\nGoal: keep the API stable"));
        assert!(input.contains("do not repeat it"));
        assert!(!build_summary_input(&[&user], None).contains("Earlier handoff"));
    }

    #[test]
    fn trimming_keeps_the_first_section_and_the_newest_work() {
        let separator = "\n\n[Work since the previous compaction]\n";
        let mut carried = "Goal: original objective\nConstraints: no new deps".to_string();
        for index in 0..40 {
            carried.push_str(separator);
            carried.push_str(&format!("pass {index}: {}", "detail ".repeat(60)));
        }
        let trimmed = trim_carried_sections(&carried, 4_096);
        assert!(trimmed.len() <= 4_096, "{}", trimmed.len());
        assert!(trimmed.starts_with("Goal: original objective\nConstraints: no new deps"));
        assert!(trimmed.contains("pass 39:"));
        assert!(!trimmed.contains("pass 0:"));
        assert!(trimmed.contains("earlier work sections omitted"));
    }

    /// One task carried through several compactions keeps its goal, the
    /// user's constraint and the newest work, and the carried handoff stays
    /// bounded instead of growing a section per pass.
    #[tokio::test]
    async fn repeated_compaction_keeps_task_state_within_a_bound() {
        let mut history = vec![
            ChatMessage::new("user", "Migrate the config loader to TOML"),
            ChatMessage::new("user", "do not change the public API"),
        ];
        for pass in 0..6 {
            history.push(ChatMessage::new(
                "user",
                format!("continue with pass {pass}"),
            ));
            history.extend((0..14).map(|index| {
                ChatMessage::new(
                    "assistant",
                    format!("pass {pass} step {index} {}", "work ".repeat(2_000)),
                )
            }));
            let url = one_shot_json_server(serde_json::json!({
                "choices": [{"message": {"content": format!(
                    "Current task: pass {pass}\n{}",
                    "- changed loader.rs and verified with cargo test\n".repeat(60)
                )}}]
            }))
            .await;
            force_compact(&reqwest::Client::new(), &url, "model", &mut history, None)
                .await
                .expect("each pass should compact");
        }

        let summary = &history[0]
            .compaction_boundary
            .as_ref()
            .expect("typed boundary")
            .summary;
        assert!(
            summary.len() <= 16 * 1024,
            "handoff grew to {} bytes",
            summary.len()
        );
        assert!(summary.contains("Current task: pass 5"));
        assert!(history.iter().any(|message| {
            message.content == format!("{ORIGINAL_TASK_MARKER}\nMigrate the config loader to TOML")
        }));
        assert!(history.iter().any(|message| {
            message.content
                == format!("{PRESERVED_USER_REQUEST_MARKER}\ndo not change the public API")
        }));
    }
}
