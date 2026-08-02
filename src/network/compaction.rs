use crate::app::ChatMessage;
use std::collections::HashMap;
use std::future::Future;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use tiktoken_rs::{cl100k_base, CoreBPE};


static BPE: OnceLock<CoreBPE> = OnceLock::new();

/// Number of distinct message contents kept in the live half of the token memo.
/// Two halves are retained (see [`TokenMemo`]), so the real ceiling is twice
/// this. Sized to comfortably hold a long session's history so a compaction
/// pass never evicts an entry it is about to look up again.
const TOKEN_MEMO_CAPACITY: usize = 4096;

/// Bounded memo mapping message content to its BPE token count.
///
/// Compaction walks the whole history several times per turn, and the history
/// itself barely changes between turns, so the same strings would otherwise be
/// re-encoded over and over. Keying on the content means a message that *is*
/// rewritten (by pruning) simply misses and gets encoded once under its new
/// value — counts stay exactly what a direct encode would produce.
///
/// Eviction is generational rather than LRU: entries land in `live`, and when
/// `live` fills it becomes `prev` and a fresh `live` is started. A hit in `prev`
/// is promoted back into `live`, so anything still in use survives rotations
/// while genuinely dead entries fall out after two of them. That keeps the
/// memory bounded without the bookkeeping of a true LRU.
#[derive(Default)]
struct TokenMemo {
    live: HashMap<(usize, u64), usize>,
    prev: HashMap<(usize, u64), usize>,
}

static TOKEN_MEMO: OnceLock<Mutex<TokenMemo>> = OnceLock::new();

/// Key on (byte length, hash) so a 64-bit hash collision between two strings of
/// different lengths cannot hand back the wrong count.
fn memo_key(text: &str) -> (usize, u64) {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    (text.len(), hasher.finish())
}

/// Exact token count for `text` under `cl100k_base`.
///
/// Uses `encode_ordinary`: message content is data (file contents, command
/// output, user prose), never a control channel, so literal `<|endoftext|>`-style
/// markers appearing inside it must be counted as the ordinary text they are
/// rather than collapsed into a single special token. It is also the cheaper of
/// the two encoders, since it skips the special-token scan.
///
/// Results are memoized (see [`TokenMemo`]); the first call for a given string
/// pays a full BPE encode, repeats are a hash and a map lookup.
pub fn estimate_tokens(text: &str) -> usize {
    let key = memo_key(text);
    let memo = TOKEN_MEMO.get_or_init(|| Mutex::new(TokenMemo::default()));

    {
        // A poisoned lock only means some other caller panicked mid-count; the
        // map is still a valid cache, so recover rather than propagate.
        let mut guard = memo.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(&count) = guard.live.get(&key) {
            return count;
        }
        if let Some(count) = guard.prev.remove(&key) {
            guard.live.insert(key, count);
            return count;
        }
    }

    // Encode outside the lock: this is the expensive part and there is no need
    // to serialize concurrent counts of different messages behind it.
    let bpe = BPE.get_or_init(|| cl100k_base().unwrap());
    let count = bpe.encode_ordinary(text).len();

    let mut guard = memo.lock().unwrap_or_else(|e| e.into_inner());
    if guard.live.len() >= TOKEN_MEMO_CAPACITY {
        guard.prev = std::mem::take(&mut guard.live);
    }
    guard.live.insert(key, count);
    count
}

pub const DEFAULT_PRUNE_TOKEN_THRESHOLD: usize = 90_000;

/// Number of most-recent messages whose tool outputs are always kept verbatim.
/// Older tool outputs are eligible for message-count-based pruning and, on
/// structured compaction, everything before this suffix is folded into a summary.
pub const KEEP_RECENT_TURNS: usize = 6;

/// Hard byte ceiling for the complete user prompt sent to the summarizer.
/// 64 KiB is deliberately conservative: it leaves ample room for the pinned
/// task, a prior summary, and several recent messages without allowing history
/// length to grow the request without bound.
const SUMMARY_INPUT_MAX_BYTES: usize = 64 * 1024;

/// A prior summary is high-value context, but must leave room for the original
/// task and recent facts inside [`SUMMARY_INPUT_MAX_BYTES`].
const SUMMARY_PRIOR_MAX_BYTES: usize = 24 * 1024;

/// Provider output is requested at 1024 tokens; 16 KiB is a generous defensive
/// byte ceiling for providers that ignore that limit.
const SUMMARY_OUTPUT_MAX_BYTES: usize = 16 * 1024;

/// Total wall-clock budget for a non-streaming summary request, including
/// connection, response headers, body transfer, and JSON decoding. Sixty
/// seconds is conservative for a 1024-token summary while remaining finite.
const SUMMARY_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// Preserve the existing per-message limit while additionally enforcing the
/// whole-input byte ceiling above.
const SUMMARY_MESSAGE_MAX_CHARS: usize = 2_000;

/// Tool outputs above this token size are collapsed once they age out of the
/// recent window.
const PRUNE_TOKEN_THRESHOLD: usize = 1000;

/// Message-count-based pruning of historical tool outputs.
///
/// Keeps the most recent `keep_recent_count` messages fully intact for accuracy.
/// For older messages, any tool result larger than [`PRUNE_TOKEN_THRESHOLD`] is
/// replaced with a one-line summary that preserves the `tool_name:` prefix — so
/// the tool call / result pairing and schema validity stay intact — along with
/// the original token count and, when detectable, the command's exit status.
pub fn prune_historical_tool_outputs(history: &mut [ChatMessage], keep_recent_count: usize) {
    let len = history.len();
    if len <= keep_recent_count {
        return;
    }
    let cutoff = len - keep_recent_count;
    for m in history[..cutoff].iter_mut() {
        if m.role != "tool" {
            continue;
        }
        // Skip anything already collapsed by a prior pass.
        if m.content.contains("[Tool Output Truncated")
            || m.content.contains("content cleared to save context")
        {
            continue;
        }
        let tokens = estimate_tokens(&m.content);
        if tokens <= PRUNE_TOKEN_THRESHOLD {
            continue;
        }
        // Preserve the "tool_name: " prefix so the call/result pairing survives.
        let prefix = match m.content.find(": ") {
            Some(pos) => m.content[..pos + 2].to_string(),
            None => String::new(),
        };
        let status = detect_exit_status(&m.content);
        m.content =
            format!("{prefix}[Tool Output Truncated: {tokens} tokens reduced to summary.{status}]");
    }
}

/// Best-effort extraction of a command exit code from raw tool output, so the
/// pruned summary can still report whether the command succeeded.
fn detect_exit_status(content: &str) -> String {
    if let Some(idx) = content.find("exit code") {
        let code: String = content[idx..]
            .chars()
            .skip_while(|c| !c.is_ascii_digit())
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if !code.is_empty() {
            return format!(" Command exited with code {code}.");
        }
    }
    String::new()
}

pub fn prune_old_tool_outputs(history: &mut [ChatMessage], threshold: usize) {
    let mut total_tool_tokens = 0;
    // Walk backward through history
    for m in history.iter_mut().rev() {
        if m.role == "tool" {
            let tokens = estimate_tokens(&m.content);
            total_tool_tokens += tokens;
            // Protect the last ~90k tokens of tool outputs (approx 360k chars).
            // Prune older ones to save context window space. Sized for the 128k
            // main model's ~108k budget so a whole large source file (e.g. a
            // 32k-token network.rs) stays fully in context instead of being
            // wiped mid-read — the amnesia that made the agent re-read forever.
            // NOTE: still a fixed cap; if you run a small-context model as the
            // main model, lower this to fit its window.
            if total_tool_tokens > threshold && !m.content.contains("content cleared to save context") {
                if let Some(pos) = m.content.find(": ") {
                    let tool_name = &m.content[..pos];
                    m.content = format!("{}: [Old tool result content cleared to save context]", tool_name);
                } else {
                    m.content = "[Old tool result content cleared to save context]".to_string();
                }
            }
        }
    }
}

/// Share of the budget that must be in use before old tool output is collapsed.
///
/// Below this the window has room to spare, and keeping what the model actually
/// read is worth more than the tokens reclaimed.
const PRUNE_PRESSURE_RATIO: f64 = 0.5;

/// Token count at which pruning starts for a given budget.
fn prune_floor(budget: usize) -> usize {
    (budget as f64 * PRUNE_PRESSURE_RATIO) as usize
}

/// Check if history needs compaction and compact if so.
/// Returns true if compaction was performed.
pub async fn maybe_compact(
    client: &reqwest::Client,
    url: &str,
    model: &str,
    history: &mut Vec<ChatMessage>,
    budget: usize,
    cancel_token: &tokio_util::sync::CancellationToken,
) -> bool {
    // 1. Local, zero-cost tool output pruning: collapse large tool outputs that
    //    have aged past the recent window, then apply the hard rolling token cap
    //    as a safety net.
    //
    //    Only under real pressure. Collapsing a file the model read three
    //    round-trips ago while most of the window sits unused buys nothing and
    //    costs the model its memory of what it just looked at — it re-reads,
    //    which is both slower and how repeat-loops start. Below the threshold
    //    the history is left exactly as it happened.
    //
    //    These two passes each rewrite the messages they collapse, so their
    //    counts are deliberately not shared: a collapsed message is a different
    //    string and must be counted as such. The memo in `estimate_tokens` is
    //    what keeps the passes cheap — every message the passes leave alone is
    //    encoded once and looked up thereafter.
    let raw_tokens: usize = history.iter().map(|m| estimate_tokens(&m.content)).sum();
    if raw_tokens < prune_floor(budget) {
        return false;
    }

    prune_historical_tool_outputs(history, KEEP_RECENT_TURNS);
    prune_old_tool_outputs(history, (budget as f64 * 0.6) as usize);

    // 2. Count the post-prune history once. `history` is not touched again
    //    until compaction actually runs, so the same per-message counts serve
    //    both the budget check and the keep-suffix walk below.
    let per_message: Vec<usize> = history.iter().map(|m| estimate_tokens(&m.content)).collect();
    let total_tokens: usize = per_message.iter().sum();
    if total_tokens < budget {
        return false;
    }

    // Determine how many messages to summarize.
    // We want to keep at least the KEEP_RECENT_TURNS most recent messages
    // verbatim, but also retain a recent suffix of up to 30% of the token
    // budget verbatim.
    let mut accumulated_tokens = 0;
    let keep_token_limit = (budget as f64 * 0.3) as usize; // Keep 30% of budget verbatim

    let mut keep_count = 0;
    for &tokens in per_message.iter().rev() {
        if accumulated_tokens + tokens <= keep_token_limit || keep_count < KEEP_RECENT_TURNS {
            accumulated_tokens += tokens;
            keep_count += 1;
        } else {
            break;
        }
    }

    let summarize_count = history.len().saturating_sub(keep_count);
    if summarize_count < 4 {
        return false;
    }
    if cancel_token.is_cancelled() {
        return false;
    }

    force_compact_internal(
        client,
        url,
        model,
        history,
        summarize_count,
        Some(cancel_token),
    )
    .await
    .is_ok()
}

pub async fn force_compact(
    client: &reqwest::Client,
    url: &str,
    model: &str,
    history: &mut Vec<ChatMessage>,
    cancel_token: Option<&tokio_util::sync::CancellationToken>,
) -> Result<(usize, usize), String> {
    let before_tokens: usize = history.iter().map(|m| estimate_tokens(&m.content)).sum();
    prune_historical_tool_outputs(history, KEEP_RECENT_TURNS);
    prune_old_tool_outputs(history, DEFAULT_PRUNE_TOKEN_THRESHOLD);

    // Summarize all but the most recent KEEP_RECENT_TURNS messages.
    let summarize_count = history.len().saturating_sub(KEEP_RECENT_TURNS);
    if summarize_count < 1 {
        return Err("Not enough messages to compact.".to_string());
    }

    let result =
        force_compact_internal(client, url, model, history, summarize_count, cancel_token).await;
    let after_tokens: usize = history.iter().map(|m| estimate_tokens(&m.content)).sum();
    result.map(|_| (before_tokens, after_tokens))
}

async fn force_compact_internal(
    client: &reqwest::Client,
    url: &str,
    model: &str,
    history: &mut Vec<ChatMessage>,
    summarize_count: usize,
    cancel_token: Option<&tokio_util::sync::CancellationToken>,
) -> Result<(), String> {
    // Incremental compaction: if a prior summary already sits at the front of the
    // range, preserve its facts and only summarize the messages that came after.
    // Avoids re-compressing an already-compressed summary (which drifts and loses
    // detail every pass).
    let prior_summary = history
        .iter()
        .take(summarize_count)
        .find(|m| m.role == "system" && m.content.starts_with(SUMMARY_MARKER))
        .map(|m| {
            m.content
                .trim_start_matches(SUMMARY_MARKER)
                .trim_start_matches('\n')
                .to_string()
        });

    // Pin the original task (first user message) so the goal is never blurred away.
    let first_user_task = history
        .iter()
        .find(|m| m.role == "user")
        .map(|m| m.content.clone());

    // Only summarize messages that aren't the prior summary itself.
    let to_summarize: Vec<&ChatMessage> = history[..summarize_count]
        .iter()
        .filter(|m| !(m.role == "system" && m.content.starts_with(SUMMARY_MARKER)))
        .collect();

    let summary = match generate_summary(
        client,
        url,
        model,
        prior_summary.as_deref(),
        &to_summarize,
        cancel_token,
    )
    .await
    {
        Some(s) => s,
        None => return Err("Failed to generate summary.".to_string()),
    };

    let tail: Vec<ChatMessage> = history[summarize_count..].to_vec();
    let task_in_tail = first_user_task
        .as_ref()
        .is_some_and(|t| tail.iter().any(|m| m.role == "user" && &m.content == t));

    // Replace the summarized range with a single summary message.
    history.clear();
    history.push(ChatMessage::new(
        "system",
        format!("{SUMMARY_MARKER}\n{summary}\n[End Summary — the following messages are the most recent conversation]"),
    ));
    // Re-inject the original task verbatim if it fell inside the summarized range.
    if let Some(task) = first_user_task
        && !task_in_tail
    {
        history.push(ChatMessage::new(
            "system",
            format!("[Original task — do not lose sight of this]\n{task}"),
        ));
    }
    history.extend(tail);

    Ok(())
}


/// Prefix that marks a compaction summary message, used to detect and preserve
/// prior summaries during incremental compaction.
pub(crate) const SUMMARY_MARKER: &str = "[Session History Summary]";

fn truncate_utf8(value: &str, max_bytes: usize) -> &str {
    if value.len() <= max_bytes {
        return value;
    }

    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

fn render_summary_message(message: &ChatMessage) -> String {
    let role_label = match message.role.as_str() {
        "user" => "User",
        "assistant" => "Assistant",
        "tool" => "Tool Result",
        "system" => "System",
        _ => "Unknown",
    };
    let mut chars = message.content.chars();
    let mut content: String = chars.by_ref().take(SUMMARY_MESSAGE_MAX_CHARS).collect();
    if chars.next().is_some() {
        content.push_str("... [truncated]");
    }
    format!("{role_label}:\n{content}\n\n")
}

/// Build a bounded prompt that pins the original task and prior summary, then
/// spends the remaining space on the newest messages in chronological order.
fn build_summary_input(prior_summary: Option<&str>, messages: &[&ChatMessage]) -> String {
    let first_user_index = messages.iter().position(|message| message.role == "user");
    let mut input = String::with_capacity(SUMMARY_INPUT_MAX_BYTES);
    input.push_str("Summarize this coding conversation.\n\n");

    if let Some(index) = first_user_index {
        input.push_str("Original user task (preserve this objective):\n");
        let rendered = render_summary_message(messages[index]);
        let remaining = SUMMARY_INPUT_MAX_BYTES.saturating_sub(input.len());
        input.push_str(truncate_utf8(&rendered, remaining));
    }

    if let Some(previous) = prior_summary {
        input.push_str("Existing summary of earlier context (preserve every fact):\n");
        let previous = truncate_utf8(previous, SUMMARY_PRIOR_MAX_BYTES);
        let remaining = SUMMARY_INPUT_MAX_BYTES.saturating_sub(input.len());
        input.push_str(truncate_utf8(previous, remaining));
        input.push_str("\n\n");
    }

    input.push_str("Newest messages to fold into the summary:\n\n");
    let mut recent = Vec::new();
    let mut remaining = SUMMARY_INPUT_MAX_BYTES.saturating_sub(input.len());
    for (index, message) in messages.iter().enumerate().rev() {
        if Some(index) == first_user_index {
            continue;
        }
        let rendered = render_summary_message(message);
        if rendered.len() <= remaining {
            remaining -= rendered.len();
            recent.push(rendered);
            continue;
        }
        if remaining > 0 {
            recent.push(truncate_utf8(&rendered, remaining).to_string());
        }
        break;
    }
    for rendered in recent.into_iter().rev() {
        input.push_str(&rendered);
    }

    debug_assert!(input.len() <= SUMMARY_INPUT_MAX_BYTES);
    input
}

fn parse_summary_response(body: &serde_json::Value) -> Option<String> {
    let content = body
        .get("choices")?
        .as_array()?
        .first()?
        .get("message")?
        .get("content")?
        .as_str()?
        .trim();
    let content = truncate_utf8(content, SUMMARY_OUTPUT_MAX_BYTES).trim_end();
    if content.is_empty()
        || !content
            .chars()
            .any(|character| !character.is_control() && !character.is_whitespace())
    {
        return None;
    }

    Some(content.to_string())
}

#[derive(Debug, PartialEq, Eq)]
enum SummaryRequestError {
    Cancelled,
    TimedOut,
}

/// Apply one deadline to the complete summary exchange. The supplied future
/// includes both `send()` and response decoding, unlike a client-level connect
/// timeout which only covers establishing the connection.
async fn await_summary_request<F>(
    future: F,
    cancel_token: Option<&tokio_util::sync::CancellationToken>,
) -> Result<F::Output, SummaryRequestError>
where
    F: Future,
{
    let timed = tokio::time::timeout(SUMMARY_REQUEST_TIMEOUT, future);
    tokio::pin!(timed);

    if let Some(cancel_token) = cancel_token {
        tokio::select! {
            biased;
            _ = cancel_token.cancelled() => Err(SummaryRequestError::Cancelled),
            result = &mut timed => result.map_err(|_| SummaryRequestError::TimedOut),
        }
    } else {
        timed.await.map_err(|_| SummaryRequestError::TimedOut)
    }
}

async fn generate_summary(
    client: &reqwest::Client,
    url: &str,
    model: &str,
    prior_summary: Option<&str>,
    messages: &[&ChatMessage],
    cancel_token: Option<&tokio_util::sync::CancellationToken>,
) -> Option<String> {
    let user_content = build_summary_input(prior_summary, messages);

    let payload = serde_json::json!({
        "model": model,
        "messages": [
            {
                "role": "system",
                "content": "You are a conversation summarizer for a coding session. Produce a concise bullet-point summary. Always preserve: the original user request/goal; every file read, created, or modified (with exact paths); key tool results, findings, and errors; and the current state of the work plus the next step. Be specific about file paths and code changes. Never invent facts and never drop facts from an existing summary. Do NOT include tool call syntax or JSON."
            },
            {
                "role": "user",
                "content": user_content
            }
        ],
        "stream": false,
        "temperature": 0.3,
        "max_tokens": 1024,
    });

    let request = async {
        let resp = client.post(url).json(&payload).send().await.ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let body: serde_json::Value = resp.json().await.ok()?;
        parse_summary_response(&body)
    };

    await_summary_request(request, cancel_token)
        .await
        .ok()
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let code_blob = format!("view_file: {}", "fn main() { println!(\"hi\"); }\n".repeat(200));
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

        assert_eq!(estimate_tokens(&original), bpe.encode_ordinary(&original).len());
        assert_eq!(estimate_tokens(rewritten), bpe.encode_ordinary(rewritten).len());
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

        let memo = TOKEN_MEMO.get().expect("memo initialized by the calls above");
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
        assert_eq!(prune_floor(100_000), 50_000);
        assert_eq!(prune_floor(8_000), 4_000);
    }

    #[test]
    fn prune_historical_keeps_recent_and_collapses_old() {
        let big = format!("run_command: {}", "x ".repeat(3000)); // > 1000 tokens
        // A large tool output at the front, and a large one near the tail.
        let mut history = vec![tool_msg(&big)]; // index 0: will age out
        for i in 0..7 {
            history.push(ChatMessage::new("user", format!("pad {i}")));
        }
        let recent_idx = history.len();
        history.push(tool_msg(&big)); // within the last KEEP_RECENT_TURNS -> kept

        prune_historical_tool_outputs(&mut history, KEEP_RECENT_TURNS);

        // Old, large tool output collapsed with prefix + token count preserved.
        assert!(history[0].content.starts_with("run_command: [Tool Output Truncated:"));
        assert!(history[0].content.contains("tokens reduced to summary"));
        // Recent large tool output left fully intact.
        assert!(history[recent_idx].content.starts_with("run_command: x x"));
    }

    #[test]
    fn prune_historical_reports_exit_code() {
        let big = format!("run_command: {} exit code 2", "y ".repeat(3000));
        let mut history = vec![tool_msg(&big)];
        for i in 0..8 {
            history.push(ChatMessage::new("user", format!("m{i}")));
        }
        prune_historical_tool_outputs(&mut history, KEEP_RECENT_TURNS);
        assert!(history[0].content.contains("Command exited with code 2."));
    }

    #[test]
    fn prune_historical_leaves_small_outputs_alone() {
        let mut history = vec![tool_msg("grep: match at line 4")];
        for i in 0..8 {
            history.push(ChatMessage::new("user", format!("m{i}")));
        }
        prune_historical_tool_outputs(&mut history, KEEP_RECENT_TURNS);
        assert_eq!(history[0].content, "grep: match at line 4");
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
        let prior = format!("PRIOR-SUMMARY: {}", "é".repeat(SUMMARY_INPUT_MAX_BYTES));

        let input = build_summary_input(Some(&prior), &refs);

        assert!(input.len() <= SUMMARY_INPUT_MAX_BYTES, "{} bytes", input.len());
        assert!(input.contains("ORIGINAL-TASK: preserve this exact objective"));
        assert!(input.contains("NEWEST-FACT: src/network/compaction.rs is the active file"));
        assert!(input.contains("PRIOR-SUMMARY:"));
        assert!(!input.contains("OLD-FACT-0:"), "oldest bulk should be dropped first");
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

        assert!(summary.len() <= SUMMARY_OUTPUT_MAX_BYTES, "{} bytes", summary.len());
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
        let task = tokio::spawn(async {
            await_summary_request(std::future::pending::<()>(), None).await
        });
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
            await_summary_request(
                std::future::pending::<()>(),
                Some(&request_cancel),
            )
            .await
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
        for index in 0..7 {
            history.push(ChatMessage::new("assistant", format!("fact {index}")));
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
    async fn cancelled_automatic_compaction_keeps_history_with_local_pruning_only() {
        let mut history = vec![ChatMessage::new("user", "keep the original task")];
        history.push(tool_msg(&format!("view_file: {}", "source ".repeat(3_000))));
        for i in 0..12 {
            history.push(ChatMessage::new(
                "assistant",
                format!("FACT-{i}: {}", "progress ".repeat(30)),
            ));
        }
        let expected_non_tool: Vec<(String, String)> = history
            .iter()
            .filter(|message| message.role != "tool")
            .map(|message| (message.role.clone(), message.content.clone()))
            .collect();
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

        let actual_non_tool: Vec<(String, String)> = history
            .iter()
            .filter(|message| message.role != "tool")
            .map(|message| (message.role.clone(), message.content.clone()))
            .collect();
        assert!(!compacted);
        assert_eq!(actual_non_tool, expected_non_tool);
        assert!(history[1].content.contains("Tool Output Truncated"));
        assert!(!history.iter().any(|message| message.content.starts_with(SUMMARY_MARKER)));
    }

    #[tokio::test]
    async fn manual_compaction_failure_keeps_history_and_returns_error() {
        let url = one_shot_json_server(serde_json::json!({
            "choices": [{"message": {"content": "  "}}]
        }))
        .await;
        let mut history = vec![ChatMessage::new("user", "original task")];
        for i in 0..7 {
            history.push(ChatMessage::new("assistant", format!("fact {i}")));
        }
        let expected: Vec<(String, String)> = history
            .iter()
            .map(|message| (message.role.clone(), message.content.clone()))
            .collect();

        let result = force_compact(
            &reqwest::Client::new(),
            &url,
            "model",
            &mut history,
            None,
        )
        .await;

        let actual: Vec<(String, String)> = history
            .iter()
            .map(|message| (message.role.clone(), message.content.clone()))
            .collect();
        assert_eq!(result, Err("Failed to generate summary.".to_string()));
        assert_eq!(actual, expected);
        assert!(!history.iter().any(|message| message.content.starts_with(SUMMARY_MARKER)));
    }
}
