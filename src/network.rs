use crate::app::{AppState, AppStatus, ChatMessage, StreamTracker, TokenUsage, ToolConfirmation};
use futures_util::StreamExt;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::Mutex;
use tokio_util::io::StreamReader;

#[path = "network/compaction.rs"]
pub(crate) mod compaction;

#[path = "network/retry.rs"]
pub(crate) mod retry;

#[path = "network/loop_detect.rs"]
pub(crate) mod loop_detect;

#[path = "network/helpers.rs"]
pub(crate) mod helpers;
pub(crate) use helpers::{classify_tool_msg, count_tokens, parse_sse_line};

#[path = "network/messages.rs"]
pub(crate) mod messages;
pub(crate) use messages::{
    RESPONSE_RESERVE_TOKENS, append_to_last_message, inject_system_reminder, trim_msgs_to_budget,
};

#[path = "network/text.rs"]
pub(crate) mod text;
use text::{
    cap_diff_lines, has_intended_tool_call, is_cut_off, strip_ansi_escapes, strip_leading_think,
    strip_think_blocks, strip_tool_call_syntax,
};

#[path = "network/stream.rs"]
pub(crate) mod stream;
pub(crate) use stream::StreamBuffer;

#[path = "network/output.rs"]
pub(crate) mod output;
pub(crate) use output::truncate_tool_output_for_message;

#[path = "network/events.rs"]
pub(crate) mod events;
pub(crate) use events::{ToolResult, ToolResultMetadata};

#[path = "network/history.rs"]
pub(crate) mod history;

#[path = "network/runner.rs"]
pub(crate) mod runner;

#[path = "network/policy.rs"]
pub(crate) mod policy;

#[path = "network/verification.rs"]
pub(crate) mod verification;

/// Injected as a system directive for the final wrap-up turn after a loop is
/// detected. Disables tools and forces a prose answer so the user gets a
/// summary instead of a silently aborted session. Ported from opencode's
/// `MAX_STEPS_PROMPT`.
const FORCE_ANSWER_PROMPT: &str = "CRITICAL — you are stuck in a loop. Tools are now DISABLED for this turn. \
Do NOT emit any tool calls (no reads, writes, edits, searches). Respond with TEXT ONLY, and include: \
a short statement that you stopped to avoid looping, a summary of what you found or accomplished so far, \
any remaining tasks, and a recommendation for what to do next. This overrides all other instructions.";

const LOOP_RECOVERY_PROMPT: &str = "The previous tool action repeated without making progress. Tools remain enabled for one recovery attempt. \
Do not repeat the same tool call or the same exact edit. Re-read the smallest relevant file region, \
compare it with the current file on disk, then use a different, grounded approach. If the requested change \
is already present or cannot be applied safely, explain that instead of retrying. This is the final recovery attempt.";

const MAX_LOOP_RECOVERY_ROUNDS: u8 = 1;

/// Safety budgets for a single agent turn. These are deliberately generous —
/// the goal is to catch a runaway session (the benchmark that motivated this
/// hit 106 rounds with no hard stop), not to cut off healthy long-running
/// work. Any one signal firing is enough: a session that is genuinely
/// healthy on every other axis but has spent 500k tokens, or ten minutes, or
/// 40 rounds, has stopped being worth running unattended.
const MAX_TOOL_ROUNDS: usize = 40;
const MAX_TURN_WALL_CLOCK: std::time::Duration = std::time::Duration::from_secs(600);
const MAX_TURN_TOKEN_BUDGET: u64 = 500_000;
/// A tool that reports success without changing anything (already-applied
/// edits, no-op runs) does not count as progress, so this escalates much
/// faster than the round budget when the agent is just spinning.
const MAX_CONSECUTIVE_NO_PROGRESS: usize = 8;
const MAX_CONSECUTIVE_FAILED_MUTATIONS: usize = 5;
const MAX_CONSECUTIVE_COMPILER_ERROR_GATES: usize = 5;
/// A malformed tool-call block is a protocol error, not a failed mutation —
/// the model tried to call a tool and produced text the harness couldn't
/// parse at all. Retrying blindly forever wastes rounds and tokens on a
/// model that isn't going to self-correct, so this budget trips much faster
/// than the general round cap.
const MAX_CONSECUTIVE_MALFORMED_CALLS: usize = 4;

/// Which safety budget stopped the turn, with enough detail for the final
/// summary to name the exact limit that was hit.
#[derive(Debug, Clone, PartialEq, Eq)]
enum TurnBudgetLimit {
    ToolRounds(usize),
    WallClock(u64),
    Tokens(u64),
    NoProgress(usize),
    FailedMutations(usize),
    CompilerErrorGates(usize),
    MalformedCalls(usize),
}

impl std::fmt::Display for TurnBudgetLimit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TurnBudgetLimit::ToolRounds(n) => write!(f, "maximum tool rounds reached ({n})"),
            TurnBudgetLimit::WallClock(secs) => {
                write!(f, "maximum turn wall-clock time reached ({secs}s)")
            }
            TurnBudgetLimit::Tokens(n) => write!(f, "maximum token budget reached (~{n} tokens)"),
            TurnBudgetLimit::NoProgress(n) => write!(
                f,
                "{n} consecutive tool results with no meaningful progress (no-op or unchanged edits)"
            ),
            TurnBudgetLimit::FailedMutations(n) => {
                write!(f, "{n} consecutive failed edits")
            }
            TurnBudgetLimit::CompilerErrorGates(n) => {
                write!(f, "{n} consecutive completion attempts with the build still broken")
            }
            TurnBudgetLimit::MalformedCalls(n) => {
                write!(
                    f,
                    "{n} consecutive malformed tool-call blocks the harness could not parse"
                )
            }
        }
    }
}

/// Adds this round's token usage onto the turn's running total. The
/// provider's `usage` field is per-response (this round's full prompt +
/// completion), not a cumulative conversation total, so it is correct to sum
/// it round over round rather than overwrite — overwriting would let a
/// 30-round turn look like it spent only what the last round used, silently
/// defeating the token safety budget. Falls back to a character-based
/// estimate for providers that don't report usage.
fn accumulate_tokens_used(current: u64, reported_this_round: Option<u64>, content: &str) -> u64 {
    current.saturating_add(reported_this_round.unwrap_or_else(|| count_tokens(content) as u64))
}

/// Checks every budget signal and returns the first one that has been
/// exceeded, if any. Order matters only for which reason is reported when
/// several trip on the same round — all are equally terminal.
fn turn_budget_exceeded(ctx: &TurnContext) -> Option<TurnBudgetLimit> {
    if ctx.tool_rounds >= MAX_TOOL_ROUNDS {
        return Some(TurnBudgetLimit::ToolRounds(ctx.tool_rounds));
    }
    let elapsed = ctx.turn_started_at.elapsed();
    if elapsed >= MAX_TURN_WALL_CLOCK {
        return Some(TurnBudgetLimit::WallClock(elapsed.as_secs()));
    }
    if ctx.tokens_used >= MAX_TURN_TOKEN_BUDGET {
        return Some(TurnBudgetLimit::Tokens(ctx.tokens_used));
    }
    if ctx.consecutive_no_progress >= MAX_CONSECUTIVE_NO_PROGRESS {
        return Some(TurnBudgetLimit::NoProgress(ctx.consecutive_no_progress));
    }
    if ctx.consecutive_failed_mutations >= MAX_CONSECUTIVE_FAILED_MUTATIONS {
        return Some(TurnBudgetLimit::FailedMutations(ctx.consecutive_failed_mutations));
    }
    if ctx.consecutive_compiler_error_gates >= MAX_CONSECUTIVE_COMPILER_ERROR_GATES {
        return Some(TurnBudgetLimit::CompilerErrorGates(
            ctx.consecutive_compiler_error_gates,
        ));
    }
    if ctx.consecutive_malformed_calls >= MAX_CONSECUTIVE_MALFORMED_CALLS {
        return Some(TurnBudgetLimit::MalformedCalls(
            ctx.consecutive_malformed_calls,
        ));
    }
    None
}

/// Stop the turn safely when a budget has been exceeded: never claim
/// completion, leave the transcript intact, and explain exactly which limit
/// was hit so the user can decide whether to resume.
async fn stop_turn_for_budget(
    state: &Arc<Mutex<AppState>>,
    ctx: &mut TurnContext,
    limit: TurnBudgetLimit,
) -> bool {
    dbg_log!("Turn budget exceeded: {}", limit);
    crate::logger::operational_event(
        "turn.budget_exceeded",
        serde_json::json!({
            "limit": limit.to_string(),
            "tool_rounds": ctx.tool_rounds,
            "elapsed_secs": ctx.turn_started_at.elapsed().as_secs(),
            "tokens_used": ctx.tokens_used,
            "failed_mutations": ctx.failed_mutations,
        }),
    );
    let summary = format!(
        "[harness: stopped after {} tool round(s) — {limit}. The task is NOT complete. \
         Review the transcript above; if the remaining work is still valid, resume it in a new turn.]",
        ctx.tool_rounds
    );
    ctx.final_content = summary;
    ctx.task_completed = false;
    ctx.budget_stopped = Some(limit.to_string());
    let mut s = state.lock().await;
    s.continuous_mode = false;
    s.status = AppStatus::Idle;
    drop(s);
    false
}

#[derive(Debug, PartialEq, Eq)]
enum LoopRecoveryAction {
    Recover,
    ForceFinal,
}

fn loop_recovery_action(attempts: u8) -> LoopRecoveryAction {
    if attempts < MAX_LOOP_RECOVERY_ROUNDS {
        LoopRecoveryAction::Recover
    } else {
        LoopRecoveryAction::ForceFinal
    }
}

/// True when a mutating tool's result reflects real forward progress —
/// not merely a reported success. A failed edit changed nothing, and
/// neither did an idempotent no-op (PR #306's "already applied" signal
/// for an edit that was already applied). Both cases must be treated the
/// same by every consumer that gates on "did this round move the task
/// forward": the no-progress safety budget, and the loop detector's
/// reset-on-progress rule. Otherwise a model that keeps re-submitting an
/// already-applied edit gets a "success" every round that resets the loop
/// detector, so it never trips — defeating the detector entirely.
fn mutation_made_progress(success: bool, content: &str) -> bool {
    if !success {
        return false;
    }
    let lower = content.trim_start().to_ascii_lowercase();
    !lower.starts_with("error") && !lower.contains("already applied")
}

/// True when a tool result has already been reduced to a stub (nothing left to prune).
fn is_fully_stubbed(m: &ChatMessage) -> bool {
    let rest = m
        .content
        .split_once(':')
        .map(|x| x.1)
        .unwrap_or("")
        .trim_start();
    rest.starts_with("[Tool output truncated") || rest.starts_with("[superseded")
}

/// Reduce one tool message a single notch toward a stub (full → 2 lines → fully
/// stubbed). Returns the new token count. Idempotent on already-stubbed messages.
async fn reduce_tool_msg(m: &mut ChatMessage, current_tokens: u32) -> u32 {
    let tool_name = m
        .content
        .split(':')
        .next()
        .unwrap_or("tool")
        .trim()
        .to_string();
    let rest = m
        .content
        .split_once(':')
        .map(|x| x.1)
        .unwrap_or("")
        .to_string();

    if is_fully_stubbed(m) {
        return current_tokens;
    }

    let lines: Vec<&str> = rest.lines().collect();
    if lines.len() > 2 {
        let truncated = format!("{}: {}\n{}", tool_name, lines[0], lines[1]);
        let t = count_tokens(&truncated);
        m.content = truncated;
        t
    } else {
        let stubbed = format!(
            "{}: [Tool output truncated: {} tokens pruned to maintain context window]",
            tool_name, current_tokens
        );
        count_tokens(&stubbed)
    }
}

/// Repeatedly reduce the oldest non-stubbed tool result of `class` until under budget
/// or the class is exhausted. Mutates `history`, `tokens`, and `total` in place.
async fn prune_class(
    history: &mut [ChatMessage],
    tokens: &mut [u32],
    total: &mut u32,
    budget: u32,
    class: &'static str,
) {
    while *total > budget {
        let target = history
            .iter()
            .enumerate()
            .find(|(_, m)| classify_tool_msg(m) == Some(class) && !is_fully_stubbed(m))
            .map(|(i, _)| i);
        let Some(idx) = target else {
            return;
        };
        let before = tokens[idx];
        let new_t = reduce_tool_msg(&mut history[idx], before).await;
        if new_t >= before {
            // Defensive: nothing more we can do here.
            return;
        }
        *total = total.saturating_sub(before).saturating_add(new_t);
        tokens[idx] = new_t;
    }
}

pub(crate) async fn compact_history_to_budget(history: &mut [ChatMessage], budget: u32) {
    if history.is_empty() {
        return;
    }

    // Strip <think> blocks from all assistant messages first to free up budget.
    for m in history.iter_mut() {
        if m.role == "assistant" {
            m.content = strip_think_blocks(&m.content);
        }
    }

    // Drop superseded reads of the same file before measuring tokens.
    // dedup disabled

    let mut tokens = Vec::with_capacity(history.len());
    for m in history.iter() {
        tokens.push(count_tokens(&m.content));
    }
    let mut total: u32 = tokens.iter().sum();
    if total <= budget {
        return;
    }

    dbg_log!(
        "History tokens ({}) exceed budget ({}). Compacting tool outputs by priority.",
        total,
        budget
    );

    // Prune lowest-value outputs first: throwaway snapshots, then file contents,
    // then anything else still taking space. Each class is flattened oldest-first.
    prune_class(history, &mut tokens, &mut total, budget, "throwaway").await;
    prune_class(history, &mut tokens, &mut total, budget, "file").await;
    prune_class(history, &mut tokens, &mut total, budget, "other").await;

    dbg_log!("Compact finished. New history tokens: {}", total);
    crate::logger::operational_event(
        "context.compaction",
        serde_json::json!({"history_tokens": total, "budget": budget}),
    );
}

async fn estimate_token_usage(messages: &[serde_json::Value], reply: &str) -> Option<TokenUsage> {
    let mut prompt_text = String::new();
    for msg in messages {
        if let Some(content) = msg.get("content") {
            if let Some(s) = content.as_str() {
                prompt_text.push_str(s);
                prompt_text.push('\n');
            } else if content.is_array() {
                if let Some(arr) = content.as_array() {
                    for item in arr {
                        if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                            prompt_text.push_str(text);
                            prompt_text.push('\n');
                        }
                    }
                }
            } else {
                prompt_text.push_str(&content.to_string());
                prompt_text.push('\n');
            }
        }
    }
    let prompt = count_tokens(&prompt_text);
    let full = prompt_text + reply + "\n";
    let total = count_tokens(&full);
    Some(TokenUsage {
        prompt_tokens: prompt,
        completion_tokens: total.saturating_sub(prompt),
        total_tokens: total,
        cached_tokens: None,
    })
}

/// Extract a context length from ollama's /api/show `model_info` blob;
/// the key is architecture-prefixed, e.g. "llama.context_length".
fn context_length_from_model_info(info: &serde_json::Value) -> Option<u32> {
    info.as_object()?
        .iter()
        .find(|(k, _)| k.ends_with(".context_length"))
        .and_then(|(_, v)| v.as_u64())
        .map(|n| n as u32)
}

/// Ask an ollama server for a model's context window. Returns None for
/// non-ollama endpoints or on any error.
pub async fn fetch_context_window(
    client: &reqwest::Client,
    chat_url: &str,
    model: &str,
    engine: Option<&str>,
) -> Option<u32> {
    let base = chat_url.strip_suffix("/v1/chat/completions")?;

    if let Some(eng) = engine {
        match eng.to_lowercase().as_str() {
            "ollama" => {
                let show_url = format!("{base}/api/show");
                let resp = client
                    .post(&show_url)
                    .json(&serde_json::json!({"model": model}))
                    .send()
                    .await
                    .ok()?;
                if resp.status().is_success() {
                    let body: serde_json::Value = resp.json().await.ok()?;
                    if let Some(ctx) = context_length_from_model_info(body.get("model_info")?) {
                        return Some(ctx);
                    }
                }
            }
            "llamacpp" | "llama.cpp" | "llama" => {
                let props_url = format!("{base}/props");
                let resp = client.get(&props_url).send().await.ok()?;
                if resp.status().is_success() {
                    let body: serde_json::Value = resp.json().await.ok()?;
                    if let Some(n) = body
                        .get("default_generation_settings")
                        .and_then(|v| v.get("n_ctx"))
                        .and_then(|v| v.as_u64())
                    {
                        return Some(n as u32);
                    }
                    if let Some(n) = body.get("n_ctx").and_then(|v| v.as_u64()) {
                        return Some(n as u32);
                    }
                }
            }
            _ => {}
        }
    }

    // Fallback: try llama.cpp first, then Ollama
    let props_url = format!("{base}/props");
    if let Ok(resp) = client.get(&props_url).send().await
        && resp.status().is_success()
        && let Ok(body) = resp.json::<serde_json::Value>().await
    {
        if let Some(n) = body
            .get("default_generation_settings")
            .and_then(|v| v.get("n_ctx"))
            .and_then(|v| v.as_u64())
        {
            return Some(n as u32);
        }
        if let Some(n) = body.get("n_ctx").and_then(|v| v.as_u64()) {
            return Some(n as u32);
        }
    }

    let show_url = format!("{base}/api/show");
    let resp = client
        .post(&show_url)
        .json(&serde_json::json!({"model": model}))
        .send()
        .await
        .ok()?;
    if resp.status().is_success() {
        let body: serde_json::Value = resp.json().await.ok()?;
        if let Some(ctx) = context_length_from_model_info(body.get("model_info")?) {
            return Some(ctx);
        }
    }

    None
}

/// Read-only tools whose results can be safely short-circuited by the repeat guard.
fn is_read_only_tool(name: &str) -> bool {
    matches!(
        crate::tools::tool_safety(name),
        crate::tools::ToolSafety::ReadOnly
    )
}

/// Tools that write to the filesystem — the ones whose result runs a compiler
/// check and that the finish gate cares about.
fn is_mutating_tool(name: &str) -> bool {
    matches!(
        name,
        "replace_file_content"
            | "multi_replace_file_content"
            | "write_to_file"
            | "delete_file"
            | "move_file"
            | "copy_file"
            | "spawn_agent"
            | "send_agent"
    )
}

/// True only if we have read this file before AND its mtime is unchanged since.
/// A re-read is allowed whenever the file is new, missing, or modified on disk —
/// so the agent can always refresh after a (possibly partial) edit.
fn view_file_unchanged_since_last_read(
    stored: Option<std::time::SystemTime>,
    current: Option<std::time::SystemTime>,
) -> bool {
    matches!((stored, current), (Some(a), Some(b)) if a == b)
}

/// Best-effort mtime of the resolved tool path (None if it can't be stat'd).
fn path_mtime(raw_path: &str) -> Option<std::time::SystemTime> {
    std::fs::metadata(crate::tools::resolve_tool_path(raw_path))
        .and_then(|m| m.modified())
        .ok()
}

/// A canonical key identifying "the same call" for the repeat guard.
fn tool_signature(name: &str, args: &serde_json::Value) -> String {
    let key = match name {
        // Bucket full/default reads together so paging can't bypass the guard.
        "view_file" => {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let start = args.get("start_line").and_then(|v| v.as_u64()).unwrap_or(1);
            let end_str = args
                .get("end_line")
                .and_then(|v| v.as_u64())
                .map(|e| e.to_string())
                .unwrap_or_else(|| "end".to_string());
            format!("{path}|{start}-{end_str}")
        }
        _ => serde_json::to_string(args).unwrap_or_default(),
    };
    format!("{name}:{key}")
}

fn align_alternating_messages(raw_msgs: Vec<serde_json::Value>) -> Vec<serde_json::Value> {
    if raw_msgs.is_empty() {
        return raw_msgs;
    }

    let mut msgs = Vec::new();
    let mut system_content = String::new();

    // 1. Merge the leading system messages into the prompt, and keep any later
    //    one where it happened, as a user turn.
    //
    //    A harness note earns its meaning from its position: "this action has
    //    repeated 5 times" answers the call above it. Hoisting it into the
    //    system prompt files it 12k characters away from the thing it is about,
    //    behind the skill catalogue, where it reads as a standing instruction
    //    rather than a response. Providers that demand strict alternation reject
    //    a mid-conversation system role, so it is carried as user text instead.
    let mut still_leading = true;
    for msg in raw_msgs {
        if let Some(role) = msg.get("role").and_then(|r| r.as_str()) {
            if role == "system" && still_leading {
                if let Some(content) = msg.get("content").and_then(|c| c.as_str()) {
                    if !system_content.is_empty() {
                        system_content.push_str("\n\n");
                    }
                    system_content.push_str(content);
                }
            } else if role == "system" {
                let content = msg
                    .get("content")
                    .and_then(|c| c.as_str())
                    .unwrap_or_default()
                    .to_string();
                msgs.push(serde_json::json!({ "role": "user", "content": content }));
            } else {
                still_leading = false;
                msgs.push(msg);
            }
        }
    }

    let mut final_msgs = Vec::new();
    if !system_content.is_empty() {
        final_msgs.push(serde_json::json!({
            "role": "system",
            "content": system_content,
        }));
    }

    if msgs.is_empty() {
        return final_msgs;
    }

    // 2. Ensure the first message is a "user" message
    let first_role = msgs[0]
        .get("role")
        .and_then(|r| r.as_str())
        .unwrap_or("user");
    if first_role != "user" {
        final_msgs.push(serde_json::json!({
            "role": "user",
            "content": "[Context initialization]",
        }));
    }

    // 3. Alternate roles, merging consecutive same-role non-tool messages
    for msg in msgs {
        let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("user");
        let content = msg
            .get("content")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();

        // A message carrying structured tool calls can never be merged: the
        // merge keeps only text, so folding it into a neighbour would silently
        // drop the calls and leave the following tool results answering nothing.
        let carries_calls = msg.get("tool_calls").is_some();
        if let Some(last) = final_msgs.last_mut() {
            let last_role = last.get("role").and_then(|r| r.as_str()).unwrap_or("user");
            let last_carries_calls = last.get("tool_calls").is_some();
            if last_role == role && role != "tool" && !carries_calls && !last_carries_calls {
                if let Some(last_content) = last.get_mut("content") {
                    let mut new_content = last_content.as_str().unwrap_or("").to_string();
                    new_content.push_str("\n\n");
                    new_content.push_str(&content);
                    *last_content = serde_json::Value::String(new_content);
                }
                continue;
            }
        }
        final_msgs.push(msg);
    }

    final_msgs
}

/// Ask an endpoint whether it implements OpenAI-style function calling.
///
/// Hostnames cannot answer this: a gateway on `localhost:3000` may front a
/// model with full tool support, and an endpoint at a well-known provider's
/// address may be a proxy that strips the field. So the endpoint is asked
/// directly, once, with the smallest request that still carries a tool schema.
/// Anything other than a clean acceptance counts as unsupported — staying on
/// the text protocol costs quality, while wrongly assuming tool support breaks
/// every turn.
pub async fn probe_function_calling(
    client: &reqwest::Client,
    state: &Arc<Mutex<AppState>>,
    url: &str,
    model: &str,
) -> bool {
    let resolved_url = {
        let trimmed = url.trim_end_matches('/');
        if trimmed.ends_with("/chat/completions") || trimmed.ends_with("/chats/completion") {
            trimmed.to_string()
        } else {
            format!("{trimmed}/chat/completions")
        }
    };
    let api_key = {
        let s = state.lock().await;
        s.config
            .models
            .iter()
            .find(|m| m.url == url || m.endpoint_url() == resolved_url)
            .and_then(|m| m.resolved_api_key())
    };

    let payload = serde_json::json!({
        "model": model,
        "messages": [{"role": "user", "content": "ping"}],
        "max_tokens": 1,
        "stream": false,
        "tools": [{
            "type": "function",
            "function": {
                "name": "probe",
                "description": "capability probe",
                "parameters": {"type": "object", "properties": {}},
            },
        }],
        "tool_choice": "none",
    });

    let mut req = client
        .post(&resolved_url)
        .json(&payload)
        .timeout(std::time::Duration::from_secs(20));
    if let Some(ref key) = api_key {
        req = req
            .header("Authorization", format!("Bearer {key}"))
            .header("X-Api-Key", key);
    }

    match req.send().await {
        Ok(response) if response.status().is_success() => {
            dbg_log!(
                "probe_function_calling: {} accepts tool schemas",
                resolved_url
            );
            true
        }
        Ok(response) => {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            dbg_log!(
                "probe_function_calling: {} rejected tool schema ({}): {}",
                resolved_url,
                status,
                body
            );
            false
        }
        Err(error) => {
            dbg_log!(
                "probe_function_calling: {} probe failed: {}",
                resolved_url,
                error
            );
            false
        }
    }
}

/// Metadata-only summary of an outbound chat-completion request: round shape
/// and size, not content. This is what gets written to debug.log by default
/// in place of the full serialized payload (see `request_debug_log_line`).
fn request_log_summary(
    model: &str,
    message_count: usize,
    tool_count: usize,
    payload_bytes: usize,
) -> String {
    format!(
        "stream_request: sending model={model} messages={message_count} tools={tool_count} payload_bytes={payload_bytes}"
    )
}

/// Choose what to write to the debug log for an outbound request: the cheap
/// structured `summary` by default, or the full serialized `payload`
/// (pretty-printed, exactly as it goes over the wire) only when
/// `verbose` (`config.debug_verbose_network_logging`) is explicitly set.
/// Kept pure/separate from the call site so both paths are unit-testable
/// without an app state, a request, or a file write.
fn request_debug_log_line(verbose: bool, summary: &str, payload: &serde_json::Value) -> String {
    if verbose {
        format!(
            "stream_request: Request payload: {}",
            serde_json::to_string_pretty(payload).unwrap_or_default()
        )
    } else {
        summary.to_string()
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn stream_request(
    client: &reqwest::Client,
    state: Arc<Mutex<AppState>>,
    cancel_token: tokio_util::sync::CancellationToken,
    url: &str,
    model: &str,
    messages: &[serde_json::Value],
    buffer: Arc<Mutex<StreamBuffer>>,
    quiet: bool,
) -> Result<Option<String>, String> {
    let aligned_messages = align_alternating_messages(messages.to_vec());
    let message_count = aligned_messages.len();
    let mut payload = serde_json::json!({
        "model": model,
        "messages": aligned_messages,
        "stream": true,
        "stream_options": {
            "include_usage": true
        },
        // Low temperature for the main agent loop: this drives structured
        // tool-calling and code edits, where 0.7 makes small models incoherent
        // and prone to token-level repetition collapse (e.g. a regex degenerating
        // into `.*?\n` repeated hundreds of times). This is sent explicitly
        // because a request value overrides the model's Modelfile PARAMETER, so
        // the server-side temperature can't be relied on. Keep it low.
        "temperature": 0.2,
        "max_tokens": 4096,
    });

    // Guard against runaway repetition even at low temperature. Google's
    // OpenAI-compat endpoint (generativelanguage.googleapis.com) rejects
    // `frequency_penalty` with a 400, so only send it to providers that accept
    // it — which is also where small open models need the repetition guard most.
    if !url.contains("generativelanguage.googleapis.com") {
        payload["frequency_penalty"] = serde_json::json!(0.3);
    }

    // ApiNative protocol: attach the tool schema so the provider returns
    // structured `tool_calls` (handled by the SSE accumulator below) instead of
    // the model writing tool calls as text. Only sent for this opt-in protocol;
    // text protocols leave the payload untouched.
    let tool_protocol = { state.lock().await.active_tool_protocol() };
    if matches!(tool_protocol, crate::config::ToolProtocol::ApiNative) {
        // Served from the same PromptCache as the system prompt (built together
        // under one key), so this is a hit after prepare_turn_request ran.
        let delegation_active = { state.lock().await.delegation_active };
        let schema = {
            let mut s = state.lock().await;
            let agent_mode = s.agent_mode;
            s.prompt_cache
                .native_schema(delegation_active, tool_protocol, agent_mode)
                .to_vec()
        };
        if !schema.is_empty() {
            payload["tools"] = serde_json::Value::Array(schema);
            payload["tool_choice"] = serde_json::json!("auto");
        }
    }

    // Full-payload logging is opt-in (`debug_verbose_network_logging`): the
    // entire message array — including every file's contents still in
    // context and the full tool schema list — used to get pretty-printed
    // into debug.log on *every* round, which is what grew the file to
    // hundreds of MB over a long session. By default we log a cheap,
    // metadata-only summary instead; still enough to diagnose a session
    // (round shape, model, sizes) without the payload itself.
    let tool_count = payload
        .get("tools")
        .and_then(|t| t.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    let payload_bytes = serde_json::to_vec(&payload).map(|v| v.len()).unwrap_or(0);
    let verbose_network_logging = { state.lock().await.config.debug_verbose_network_logging };
    dbg_log!(
        "{}",
        request_debug_log_line(
            verbose_network_logging,
            &request_log_summary(model, message_count, tool_count, payload_bytes),
            &payload,
        )
    );

    let resolved_url = {
        let trimmed = url.trim_end_matches('/');
        if trimmed.ends_with("/chat/completions") || trimmed.ends_with("/chats/completion") {
            trimmed.to_string()
        } else {
            format!("{trimmed}/chat/completions")
        }
    };

    let api_key = {
        let s = state.lock().await;
        s.config
            .models
            .iter()
            .find(|m| m.url == url || m.name == s.model_name || m.endpoint_url() == resolved_url)
            .and_then(|m| m.resolved_api_key())
    };

    // Establish the connection with retry/backoff on transient failures
    // (429, 5xx, network blips). We only retry here, before any SSE bytes are
    // read — retrying mid-stream would duplicate partial output.
    let mut attempt = 0usize;
    let response = loop {
        if cancel_token.is_cancelled() {
            return Err("cancelled".to_string());
        }
        let mut req = client.post(&resolved_url).json(&payload);
        if let Some(ref key) = api_key {
            req = req
                .header("Authorization", format!("Bearer {key}"))
                .header("X-Api-Key", key);
        }
        // Race the header wait against cancellation (so a cancel while
        // headers are in flight interrupts immediately, not once the
        // request finishes or the provider times out) and against a
        // bounded first-byte timeout (so a connection that succeeds but
        // never sends headers can't hang forever). This deliberately does
        // NOT bound the rest of the response — long legitimate SSE streams
        // must keep working.
        let send_result = retry::race_cancellable(
            tokio::time::timeout(retry::HEADER_TIMEOUT, req.send()),
            &cancel_token,
        )
        .await;
        let send_result = match send_result {
            None => return Err("cancelled".to_string()),
            Some(Err(_elapsed)) => {
                if attempt < retry::MAX_RETRIES {
                    let delay = retry::delay_for_attempt(attempt, 0);
                    dbg_log!(
                        "stream_request: timed out waiting for response headers (attempt {}/{}), backing off {}ms",
                        attempt + 1,
                        retry::MAX_RETRIES,
                        delay.as_millis()
                    );
                    if retry::race_cancellable(tokio::time::sleep(delay), &cancel_token)
                        .await
                        .is_none()
                    {
                        return Err("cancelled".to_string());
                    }
                    attempt += 1;
                    continue;
                }
                return Err(format!(
                    "timed out waiting for response headers after {}s",
                    retry::HEADER_TIMEOUT.as_secs()
                ));
            }
            Some(Ok(r)) => r,
        };
        match send_result {
            Ok(resp) if resp.status().is_success() => {
                dbg_log!(
                    "stream_request: Received response status: {}",
                    resp.status()
                );
                break resp;
            }
            Ok(resp) => {
                let status = resp.status();
                let code = status.as_u16();
                // Reading the error body is itself an in-flight I/O wait
                // (the response was accepted but the body may still be
                // streaming in), so it must race cancellation just like the
                // header wait and the retry backoff above — otherwise a
                // cancel that lands while we're pulling a slow error body
                // has to wait for that read to finish before it takes
                // effect.
                let err_body = match retry::race_cancellable(resp.text(), &cancel_token).await {
                    None => return Err("cancelled".to_string()),
                    Some(body) => body.unwrap_or_default(),
                };
                if retry::is_retryable_status(code) && attempt < retry::MAX_RETRIES {
                    let delay = retry::delay_for_attempt(attempt, code);
                    dbg_log!(
                        "stream_request: retryable status {} (attempt {}/{}), backing off {}ms",
                        status,
                        attempt + 1,
                        retry::MAX_RETRIES,
                        delay.as_millis()
                    );
                    if retry::race_cancellable(tokio::time::sleep(delay), &cancel_token)
                        .await
                        .is_none()
                    {
                        return Err("cancelled".to_string());
                    }
                    attempt += 1;
                    continue;
                }
                dbg_log!(
                    "stream_request: Request failed with status {}. Body: {}",
                    status,
                    err_body
                );
                return Err(format!("{status} - {err_body}"));
            }
            Err(e) => {
                if retry::is_retryable_transport(&e) && attempt < retry::MAX_RETRIES {
                    let delay = retry::delay_for_attempt(attempt, 0);
                    dbg_log!(
                        "stream_request: transient network error (attempt {}/{}), backing off {}ms: {}",
                        attempt + 1,
                        retry::MAX_RETRIES,
                        delay.as_millis(),
                        e
                    );
                    if retry::race_cancellable(tokio::time::sleep(delay), &cancel_token)
                        .await
                        .is_none()
                    {
                        return Err("cancelled".to_string());
                    }
                    attempt += 1;
                    continue;
                }
                let mut msg = format!("Request failed: {e}");
                let mut src = std::error::Error::source(&e);
                while let Some(cause) = src {
                    msg.push_str(&format!(": {cause}"));
                    src = cause.source();
                }
                return Err(msg);
            }
        }
    };

    let stream = response
        .bytes_stream()
        .map(|r| r.map_err(std::io::Error::other));
    let wrapped = StreamReader::new(stream);
    let mut reader = BufReader::with_capacity(4096, wrapped);
    let mut line_buf = String::with_capacity(4096);
    let mut in_reasoning = false;
    let mut finish_reason: Option<String> = None;

    #[derive(Debug)]
    struct ToolAccumulator {
        /// Provider-assigned call id. Results must be sent back naming this id,
        /// so it is carried out of the stream rather than dropped.
        id: String,
        name: String,
        arguments: String,
    }
    let mut accumulators: Vec<ToolAccumulator> = Vec::new();
    let mut fences = ToolFenceCounter::default();
    // A model that plans a whole session ahead keeps emitting tool calls that
    // can never run. Stop reading once it is past the batch limit instead of
    // paying for the rest of the response.
    let runaway_limit = crate::tools::MAX_TOOL_CALLS_PER_RESPONSE;

    dbg_log!("stream_request: Starting SSE stream read loop");
    loop {
        if cancel_token.is_cancelled() {
            dbg_log!("stream_request: Stream reading cancelled via token");
            return Ok(None);
        }

        tokio::select! {
            r = reader.read_line(&mut line_buf) => {
                match r {
                    Ok(0) => {
                        dbg_log!("stream_request: SSE stream read EOF (0 bytes)");
                        break;
                    }
                    Ok(_) => {
                        let trimmed = line_buf.trim();
                        if let Some(json_str) = parse_sse_line(trimmed) {
                            if let Ok(val) = serde_json::from_str::<serde_json::Value>(json_str) {
                                if let Some(choices) = val.get("choices").and_then(|c| c.as_array())
                                    && !choices.is_empty() {
                                        if let Some(fr) = choices[0].get("finish_reason").and_then(|f| f.as_str()) {
                                            finish_reason = Some(fr.to_string());
                                        }
                                         let delta = choices[0].get("delta");
                                         let reasoning = delta
                                             .and_then(|d| d.get("reasoning").or_else(|| d.get("reasoning_content")))
                                             .and_then(|r| r.as_str());
                                         let content = delta
                                             .and_then(|d| d.get("content").or_else(|| d.get("text")))
                                             .and_then(|c| c.as_str());

                                         if let Some(tool_calls) = delta.and_then(|d| d.get("tool_calls")).and_then(|t| t.as_array()) {
                                             const MAX_TOOL_CALL_INDEX: usize = 127;
                                             for tc in tool_calls {
                                                 let mut idx = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                                                 if idx > MAX_TOOL_CALL_INDEX {
                                                     eprintln!("Warning: tool call index {} exceeds max allowed ({}), clamping.", idx, MAX_TOOL_CALL_INDEX);
                                                     idx = idx.min(MAX_TOOL_CALL_INDEX);
                                                 }
                                                 while accumulators.len() <= idx {
                                                     accumulators.push(ToolAccumulator {
                                                         id: String::new(),
                                                         name: String::new(),
                                                         arguments: String::new(),
                                                     });
                                                 }
                                                 let acc = &mut accumulators[idx];
                                                 if let Some(id) = tc.get("id").and_then(|i| i.as_str()) {
                                                     acc.id.push_str(id);
                                                 }
                                                 if let Some(name) = tc.get("function").and_then(|f| f.get("name")).and_then(|n| n.as_str()) {
                                                     acc.name.push_str(name);
                                                 }
                                                 if let Some(args) = tc.get("function").and_then(|f| f.get("arguments")).and_then(|a| a.as_str()) {
                                                     acc.arguments.push_str(args);
                                                 }
                                             }
                                         }

                                         let mut chunk = String::new();
                                        if let Some(r_token) = reasoning {
                                            if !in_reasoning {
                                                in_reasoning = true;
                                                chunk.push_str("<think>\n");
                                            }
                                            chunk.push_str(r_token);
                                        } else if let Some(c_token) = content {
                                            if in_reasoning {
                                                in_reasoning = false;
                                                chunk.push_str("\n</think>\n\n");
                                            }
                                            chunk.push_str(c_token);
                                        }
                                        let runaway = fences.push(&chunk) > runaway_limit
                                            || accumulators
                                                .iter()
                                                .filter(|acc| !acc.name.is_empty())
                                                .count()
                                                > runaway_limit;
                                        if !chunk.is_empty() {
                                            let tokens = (chunk.len() as f64 * crate::app::TOKENS_PER_CHAR_APPROX) as u32;
                                            if let Some(ref mut tracker) = state.lock().await.stream_tracker {
                                                tracker.tokens_so_far += tokens;
                                                tracker.record_chunk();
                                            }

                                            buffer.lock().await.content.push_str(&chunk);
                                            if !quiet {
                                                let mut s = state.lock().await;
                                                s.current_response.push_str(&chunk);
                                                if s.raw_cli_mode {
                                                    use std::io::Write;
                                                    print!("{chunk}");
                                                    let _ = std::io::stdout().flush();
                                                }
                                            }
                                        }
                                        if runaway {
                                            dbg_log!(
                                                "stream_request: past {} tool calls in one response — cutting the stream",
                                                runaway_limit
                                            );
                                            crate::logger::operational_event(
                                                "stream.runaway_cut",
                                                serde_json::json!({ "limit": runaway_limit }),
                                            );
                                            // The call that tripped the limit is
                                            // partial — its arguments would fail
                                            // to parse and cost a whole turn.
                                            accumulators.truncate(runaway_limit);
                                            line_buf.clear();
                                            break;
                                        }
                                    }
                                if let Some(usage) = val.get("usage").filter(|_| !quiet)
                                    && let (Some(p), Some(c), Some(t)) = (
                                        usage.get("prompt_tokens").and_then(|v| v.as_u64()),
                                        usage.get("completion_tokens").and_then(|v| v.as_u64()),
                                        usage.get("total_tokens").and_then(|v| v.as_u64()),
                                    ) {
                                        let cached = usage.get("prompt_tokens_details")
                                            .and_then(|details| details.get("cached_tokens"))
                                            .and_then(|v| v.as_u64())
                                            .or_else(|| usage.get("cached_tokens").and_then(|v| v.as_u64()))
                                            .map(|n| n as u32);

                                        state.lock().await.current_token_usage = Some(TokenUsage {
                                            prompt_tokens: p as u32,
                                            completion_tokens: c as u32,
                                            total_tokens: t as u32,
                                            cached_tokens: cached,
                                        });
                                    }
                            } else {
                                dbg_log!("stream_request: Failed to parse JSON from data payload: '{}'", json_str);
                            }
                        }
                        line_buf.clear();
                    }
                    Err(e) => {
                        dbg_log!("stream_request: SSE read error: {}", e);
                        break;
                    }
                }
            }
            _ = cancel_token.cancelled() => {
                dbg_log!("stream_request: Cancelled via select branch");
                return Ok(None);
            }
        }
    }

    if in_reasoning {
        buffer.lock().await.content.push_str("\n</think>\n\n");
        if !quiet {
            let mut s = state.lock().await;
            s.current_response.push_str("\n</think>\n\n");
            if s.raw_cli_mode {
                use std::io::Write;
                print!("\n</think>\n\n");
                let _ = std::io::stdout().flush();
            }
        }
    }

    let mut translation = String::new();
    let mut streamed_call_ids: Vec<String> = Vec::new();
    for (position, acc) in accumulators.iter().enumerate() {
        if acc.name.is_empty() {
            continue;
        }

        let args_json = parse_native_tool_arguments(&acc.arguments);

        let tool_call_obj = serde_json::json!({
            "name": acc.name,
            "arguments": args_json
        });

        // Providers that omit the id still need one to pair results with calls;
        // position within the response is stable enough to stand in.
        streamed_call_ids.push(if acc.id.is_empty() {
            format!("call_{position}")
        } else {
            acc.id.clone()
        });

        translation.push_str("\n\n```tool\n");
        translation.push_str(&serde_json::to_string(&tool_call_obj).unwrap_or_default());
        translation.push_str("\n```\n");
    }

    if !translation.is_empty() {
        dbg_log!(
            "stream_request: Translating and appending native tool call: {}",
            translation
        );
        {
            let mut buf = buffer.lock().await;
            buf.content.push_str(&translation);
            buf.tool_call_ids = streamed_call_ids;
        }
        if !quiet {
            let mut s = state.lock().await;
            s.current_response.push_str(&translation);
            if s.raw_cli_mode {
                use std::io::Write;
                print!("{translation}");
                let _ = std::io::stdout().flush();
            }
        }
    }

    let mut buf = buffer.lock().await;
    buf.content = buf
        .content
        .trim_end_matches(char::is_whitespace)
        .to_string();
    dbg_log!(
        "stream_request: Stream request loop ended. Total content: {} chars",
        buf.content.len()
    );
    Ok(finish_reason)
}

/// Preserve malformed native arguments for validation and model feedback
/// instead of silently turning them into an empty object. This keeps the raw
/// provider failure visible while ensuring the call cannot execute.
fn parse_native_tool_arguments(raw: &str) -> serde_json::Value {
    match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(value) if value.is_object() => value,
        Ok(value) => serde_json::json!({ "_invalid_arguments": value }),
        Err(error) => serde_json::json!({
            "_invalid_arguments": raw,
            "_parse_error": error.to_string(),
        }),
    }
}

pub(crate) fn get_diff_preview(name: &str, args: &serde_json::Value) -> Option<String> {
    if name == "replace_file_content" {
        // Recognize every alias the edit tools themselves accept
        // (target_content/target/old_string/old_text/oldString/oldText and
        // their replacement counterparts) — a call shaped with any of them
        // must not fall through to an empty, misleading preview.
        let (target, replacement) = crate::tools::edit_target_and_replacement(args);
        let search_block = target.as_deref().unwrap_or("");
        let replace_block = replacement.as_deref().unwrap_or("");

        let diff = similar::TextDiff::from_lines(search_block, replace_block);
        let old_slices: Vec<&str> = diff.iter_old_slices().collect();
        let new_slices: Vec<&str> = diff.iter_new_slices().collect();

        let mut prev = String::new();
        for op in diff.ops() {
            let old_slice = &old_slices[op.old_range()];
            let new_slice = &new_slices[op.new_range()];
            match op.tag() {
                similar::DiffTag::Equal => {
                    for (o, n) in old_slice.iter().zip(new_slice.iter()) {
                        prev.push_str(&format!(
                            " {}\x00 {}\n",
                            o.trim_end_matches('\n').trim_end_matches('\r'),
                            n.trim_end_matches('\n').trim_end_matches('\r')
                        ));
                    }
                }
                similar::DiffTag::Delete => {
                    for o in old_slice {
                        prev.push_str(&format!(
                            "-{}\x00~\n",
                            o.trim_end_matches('\n').trim_end_matches('\r')
                        ));
                    }
                }
                similar::DiffTag::Insert => {
                    for n in new_slice {
                        prev.push_str(&format!(
                            "~\x00+{}\n",
                            n.trim_end_matches('\n').trim_end_matches('\r')
                        ));
                    }
                }
                similar::DiffTag::Replace => {
                    let max_len = old_slice.len().max(new_slice.len());
                    for i in 0..max_len {
                        let o_val = old_slice.get(i);
                        let n_val = new_slice.get(i);
                        match (o_val, n_val) {
                            (Some(o), Some(n)) => {
                                prev.push_str(&format!(
                                    "-{}\x00+{}\n",
                                    o.trim_end_matches('\n').trim_end_matches('\r'),
                                    n.trim_end_matches('\n').trim_end_matches('\r')
                                ));
                            }
                            (Some(o), None) => {
                                prev.push_str(&format!(
                                    "-{}\x00~\n",
                                    o.trim_end_matches('\n').trim_end_matches('\r')
                                ));
                            }
                            (None, Some(n)) => {
                                prev.push_str(&format!(
                                    "~\x00+{}\n",
                                    n.trim_end_matches('\n').trim_end_matches('\r')
                                ));
                            }
                            (None, None) => {}
                        }
                    }
                }
            }
        }
        Some(cap_diff_lines(prev))
    } else if name == "write_to_file" && args.get("__rustcode_legacy_write_diff").is_some() {
        let path = args.get("path").and_then(|p| p.as_str()).unwrap_or("");
        let old_content = std::fs::read_to_string(path).unwrap_or_default();
        let new_content = args.get("content").and_then(|c| c.as_str()).unwrap_or("");

        let diff = similar::TextDiff::from_lines(&old_content, new_content);
        let old_slices: Vec<&str> = diff.iter_old_slices().collect();
        let new_slices: Vec<&str> = diff.iter_new_slices().collect();

        let mut prev = String::new();
        for group in diff.grouped_ops(3) {
            for op in group {
                let old_slice = &old_slices[op.old_range()];
                let new_slice = &new_slices[op.new_range()];
                match op.tag() {
                    similar::DiffTag::Equal => {
                        for (o, n) in old_slice.iter().zip(new_slice.iter()) {
                            prev.push_str(&format!(
                                " {}\x00 {}\n",
                                o.trim_end_matches('\n').trim_end_matches('\r'),
                                n.trim_end_matches('\n').trim_end_matches('\r')
                            ));
                        }
                    }
                    similar::DiffTag::Delete => {
                        for o in old_slice {
                            prev.push_str(&format!(
                                "-{}\x00~\n",
                                o.trim_end_matches('\n').trim_end_matches('\r')
                            ));
                        }
                    }
                    similar::DiffTag::Insert => {
                        for n in new_slice {
                            prev.push_str(&format!(
                                "~\x00+{}\n",
                                n.trim_end_matches('\n').trim_end_matches('\r')
                            ));
                        }
                    }
                    similar::DiffTag::Replace => {
                        let max_len = old_slice.len().max(new_slice.len());
                        for i in 0..max_len {
                            let o_val = old_slice.get(i);
                            let n_val = new_slice.get(i);
                            match (o_val, n_val) {
                                (Some(o), Some(n)) => {
                                    prev.push_str(&format!(
                                        "-{}\x00+{}\n",
                                        o.trim_end_matches('\n').trim_end_matches('\r'),
                                        n.trim_end_matches('\n').trim_end_matches('\r')
                                    ));
                                }
                                (Some(o), None) => {
                                    prev.push_str(&format!(
                                        "-{}\x00~\n",
                                        o.trim_end_matches('\n').trim_end_matches('\r')
                                    ));
                                }
                                (None, Some(n)) => {
                                    prev.push_str(&format!(
                                        "~\x00+{}\n",
                                        n.trim_end_matches('\n').trim_end_matches('\r')
                                    ));
                                }
                                (None, None) => {}
                            }
                        }
                    }
                }
            }
        }
        Some(cap_diff_lines(prev))
    } else {
        None
    }
}

/// Pull the real ```diff fence out of a tool's own result content. Since
/// PR #309, `replace_file_content_tool`/`multi_replace_file_content_tool`
/// embed a unified diff generated from the actual before/after file content
/// directly in the string they return — this is the one true diff for what
/// actually landed on disk, as opposed to `get_diff_preview`'s best-effort,
/// argument-only preview computed *before* the edit runs (used only for the
/// confirmation modal). Returns `None` when the result has no such fence —
/// a no-op ("already applied") edit, a failed edit, or a tool that doesn't
/// embed a diff at all.
fn extract_diff_block(content: &str) -> Option<String> {
    let after_fence = content.split_once("```diff\n")?.1;
    let (body, _) = after_fence.split_once("\n```")?;
    if body.trim().is_empty() {
        None
    } else {
        Some(body.to_string())
    }
}

/// The real diff for a tool result, or `None` if there's nothing real to
/// show. Falls back to the pre-execution argument-only preview only when it
/// carries actual content — an empty fallback (e.g. `get_diff_preview` was
/// never given `target_content`/`replacement_content` under those exact
/// keys) is exactly the kind of misleading, non-representative diff this
/// must never let through.
fn final_tool_diff(result: &str, preview_fallback: Option<String>) -> Option<String> {
    extract_diff_block(result).or_else(|| preview_fallback.filter(|d| !d.trim().is_empty()))
}

/// True when a tool result means "nothing changed" — a no-op ("already
/// applied", PR #306) or a failed edit. `get_diff_preview` computes its
/// preview purely from the call's arguments, before execution, so a
/// non-empty preview says nothing about whether the edit actually landed.
/// The preview must never be handed to `final_tool_diff` as a fallback for
/// either of these outcomes, or a no-op/failed call could show a diff for a
/// change that never happened — final filesystem diffs must stay
/// authoritative. Call sites use this to decide what to pass as
/// `final_tool_diff`'s `preview_fallback`, so `final_tool_diff` itself never
/// has to change.
fn tool_result_precludes_preview_fallback(content: &str) -> bool {
    let lower = content.trim_start().to_ascii_lowercase();
    lower.starts_with("error") || lower.contains("already applied")
}

fn get_file_preview(name: &str, args: &serde_json::Value) -> Option<(String, String)> {
    if name != "write_to_file" {
        return None;
    }
    Some((
        args.get("path")?.as_str()?.to_string(),
        args.get("content")?.as_str()?.to_string(),
    ))
}

fn get_tool_project_root(_name: &str, args: &serde_json::Value) -> std::path::PathBuf {
    let raw_path = if let Some(p) = args.get("path").and_then(|p| p.as_str()) {
        Some(p)
    } else if let Some(s) = args.get("src").and_then(|s| s.as_str()) {
        Some(s)
    } else {
        args.get("dest").and_then(|d| d.as_str())
    };

    let resolved = if let Some(rp) = raw_path {
        crate::tools::resolve_tool_path(rp)
    } else {
        std::env::current_dir().unwrap_or_default()
    };

    // Find project root from resolved path
    let mut current = if resolved.is_dir() {
        resolved.clone()
    } else {
        resolved
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or(resolved)
    };

    loop {
        if current.join("Cargo.toml").exists() || current.join("tsconfig.json").exists() {
            // `Path::parent()` turns a relative `src/...` path into `""` at
            // the workspace root. That path works for joins but is invalid as
            // a child-process cwd, causing cargo to fail with ENOENT. Always
            // hand verification an existing absolute directory.
            return current
                .canonicalize()
                .or_else(|_| std::env::current_dir())
                .unwrap_or(current);
        }
        if let Some(parent) = current.parent() {
            current = parent.to_path_buf();
        } else {
            break;
        }
    }

    std::env::current_dir().unwrap_or_default()
}

/// Resolve a build tool to an absolute path. GUI-launched apps (and some
/// spawned environments) don't inherit the shell PATH, so a bare
/// `Command::new("cargo")` fails with ENOENT even though the tool is installed.
/// Check the canonical install locations first, then fall back to the bare
/// name (which relies on PATH) so terminal launches still work.
fn resolve_bin(name: &str) -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    let candidates = [
        format!("{home}/.cargo/bin/{name}"),
        format!("/opt/homebrew/bin/{name}"),
        format!("/usr/local/bin/{name}"),
        format!("/usr/bin/{name}"),
    ];
    for c in candidates {
        if std::path::Path::new(&c).exists() {
            return std::path::PathBuf::from(c);
        }
    }
    std::path::PathBuf::from(name)
}

/// PATH for spawned build tools. GUI/Dock launches don't inherit the shell
/// PATH, so `cargo` can't find `rustc` (and bare-name spawns fail with ENOENT)
/// even though the toolchain is installed. Prepend the canonical toolchain dirs
/// to whatever PATH we did inherit so the checker — and its own subprocesses —
/// resolve regardless of how rustcode was launched.
pub(crate) fn augmented_path() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut dirs = vec![
        format!("{home}/.cargo/bin"),
        "/opt/homebrew/bin".to_string(),
        "/usr/local/bin".to_string(),
        "/usr/bin".to_string(),
        "/bin".to_string(),
    ];
    if let Ok(existing) = std::env::var("PATH") {
        dirs.extend(existing.split(':').map(|s| s.to_string()));
    }
    dirs.join(":")
}

async fn run_compiler_check(cwd: &std::path::Path) -> Option<String> {
    if cwd.join("Cargo.toml").exists() {
        // Run through `sh -c` (like run_command) so the SHELL resolves `cargo`
        // using the augmented PATH. A bare-name direct spawn
        // (`Command::new("cargo")`) does not use the command's env PATH for
        // program lookup, so on GUI/Dock launches — where `resolve_bin`'s
        // exists() checks can't see /opt/homebrew — it fell back to "cargo" and
        // failed with ENOENT even though `cargo check` via run_command worked.
        let mut cmd = tokio::process::Command::new("/bin/sh");
        cmd.args(["-c", "cargo check --message-format=json"])
            .current_dir(cwd)
            .env("PATH", augmented_path())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                dbg_log!("Could not spawn cargo check ({e}), skipping compiler check");
                return Some(format!(
                    "__BUILD_UNVERIFIED__: could not run `cargo check` ({e}). \
                     The build was NOT verified — do not claim the task compiles."
                ));
            }
        };

        // `cargo check` on a non-trivial crate routinely exceeds a few seconds,
        // especially the first run after edits. Too short a timeout leaves the
        // agent blind to compile errors — the whole point of this check.
        let timeout_duration = std::time::Duration::from_secs(120);
        let output_res = tokio::time::timeout(timeout_duration, child.wait_with_output()).await;

        let output = match output_res {
            Ok(Ok(out)) => out,
            Ok(Err(e)) => {
                dbg_log!("cargo check failed to run ({e}), skipping compiler check");
                return Some(format!(
                    "__BUILD_UNVERIFIED__: `cargo check` failed to run ({e}). \
                     The build was NOT verified."
                ));
            }
            Err(_) => {
                dbg_log!("cargo check timed out, skipping compiler check");
                return Some(
                    "__BUILD_UNVERIFIED__: `cargo check` timed out. \
                     The build was NOT verified."
                        .to_string(),
                );
            }
        };

        let stdout_str = String::from_utf8_lossy(&output.stdout);
        let mut errors = Vec::new();

        for line in stdout_str.lines() {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(line)
                && val.get("reason").and_then(|r| r.as_str()) == Some("compiler-message")
                && let Some(msg) = val.get("message")
                && let Some(level) = msg.get("level").and_then(|l| l.as_str())
                && level == "error"
                && let Some(rendered) = msg.get("rendered").and_then(|r| r.as_str())
            {
                errors.push(strip_ansi_escapes(rendered));
            }
        }

        if !errors.is_empty() {
            return Some(errors.join("\n"));
        }
    } else if cwd.join("biome.json").exists() || cwd.join("biome.jsonc").exists() {
        let (runner, bin_arg) = if resolve_bin("bunx").exists() {
            (resolve_bin("bunx"), "biome")
        } else {
            (resolve_bin("npx"), "@biomejs/biome")
        };

        let mut cmd = tokio::process::Command::new(runner);
        cmd.args([bin_arg, "check", "."])
            .current_dir(cwd)
            .env("PATH", augmented_path())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                dbg_log!("Could not spawn biome check ({e}), skipping compiler check");
                return None;
            }
        };

        let timeout_duration = std::time::Duration::from_secs(60);
        let output_res = tokio::time::timeout(timeout_duration, child.wait_with_output()).await;

        let output = match output_res {
            Ok(Ok(out)) => out,
            Ok(Err(_)) | Err(_) => return None,
        };

        if !output.status.success() {
            let stdout_str = String::from_utf8_lossy(&output.stdout);
            let stderr_str = String::from_utf8_lossy(&output.stderr);
            let combined = format!("{stdout_str}\n{stderr_str}");
            let trimmed = combined.trim();
            if !trimmed.is_empty() {
                return Some(strip_ansi_escapes(trimmed));
            }
        }
    } else if cwd.join("tsconfig.json").exists() {
        let (runner, bin_arg) = if resolve_bin("bunx").exists() {
            (resolve_bin("bunx"), "tsc")
        } else {
            (resolve_bin("npx"), "tsc")
        };

        let mut cmd = tokio::process::Command::new(runner);
        cmd.args([bin_arg, "--noEmit"])
            .current_dir(cwd)
            .env("PATH", augmented_path())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                dbg_log!("Could not spawn {bin_arg} ({e}), skipping compiler check");
                return None;
            }
        };

        let timeout_duration = std::time::Duration::from_secs(60);
        let output_res = tokio::time::timeout(timeout_duration, child.wait_with_output()).await;

        let output = match output_res {
            Ok(Ok(out)) => out,
            Ok(Err(_)) | Err(_) => return None,
        };

        if !output.status.success() {
            let stdout_str = String::from_utf8_lossy(&output.stdout);
            let stderr_str = String::from_utf8_lossy(&output.stderr);
            let combined = format!("{stdout_str}\n{stderr_str}");
            let trimmed = combined.trim();
            if !trimmed.is_empty() {
                return Some(strip_ansi_escapes(trimmed));
            }
        }
    }

    None
}

/// Run a compiler check, reusing the previous result when the tree hasn't been
/// dirtied since. `cargo check` is slow, and one task can hit the check at
/// several points in a single round (inline after a tool batch, then again at
/// the finish gate). Without this, an edit-and-complete round runs `cargo check`
/// two or three times over an identical tree. `dirty` is set by the caller
/// whenever a mutating tool runs; this clears it after a fresh check.
async fn cached_compiler_check(
    root: &std::path::Path,
    dirty: &mut bool,
    cache: &mut Option<(std::path::PathBuf, Option<String>)>,
) -> Option<String> {
    if !*dirty
        && let Some((cached_root, cached_result)) = cache.as_ref()
        && cached_root == root
    {
        dbg_log!("Compiler check: reusing cached result (tree unchanged since last check)");
        return cached_result.clone();
    }
    let result = run_compiler_check(root).await;
    *cache = Some((root.to_path_buf(), result.clone()));
    *dirty = false;
    result
}

/// Handle an interactive `ask_question` tool call: show the option-picker modal
/// and block until the user chooses (or cancels / the turn is cancelled). Returns
/// the chosen option text — that becomes the tool result fed back to the model,
/// so it can continue with the user's answer.
async fn ask_user_question(
    state: &Arc<Mutex<AppState>>,
    cancel_token: &tokio_util::sync::CancellationToken,
    args: &serde_json::Value,
) -> crate::tools::ToolExecutionOutput {
    let question = args
        .get("question")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let options: Vec<String> = args
        .get("options")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|o| o.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let is_multi_select = args
        .get("is_multi_select")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if question.is_empty() || options.is_empty() {
        return crate::tools::ToolExecutionOutput::failure(
            "error: ask_question requires a non-empty 'question' and 'options'".to_string(),
        );
    }

    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    {
        let mut s = state.lock().await;
        s.pending_question = Some(crate::app::PendingQuestion {
            question,
            chosen: vec![false; options.len()],
            options,
            is_multi_select,
            selected: 0,
            custom_input: None,
        });
        s.question_response = Some(tx);
        s.status = AppStatus::AwaitingQuestion;
    }
    let _ = crate::notifications::notify_pending_confirmation("ask_question");

    let answer = tokio::select! {
        _ = cancel_token.cancelled() => None,
        res = rx => res.ok(),
    };

    {
        let mut s = state.lock().await;
        s.pending_question = None;
        s.question_response = None;
        if s.status == AppStatus::AwaitingQuestion {
            let model_name = s.model_name.clone();
            s.status = AppStatus::Streaming;
            if s.config.discord_rpc_enabled {
                s.discord_rpc
                    .set_activity("Thinking", &format!("Using model: {}", model_name));
            }
        }
    }

    match answer {
        Some(a) if !a.is_empty() => {
            crate::tools::ToolExecutionOutput::success(format!("User selected: {a}"))
        }
        _ => crate::tools::ToolExecutionOutput::failure(
            "error: the user dismissed the question without answering".to_string(),
        ),
    }
}

/// Show the Y/N confirmation modal (when the tool requires it) and run the
/// tool. `display_name` is what the modal shows — subagent calls prefix it
/// with the agent id so the user knows who is asking.
async fn confirm_and_execute(
    state: &Arc<Mutex<AppState>>,
    cancel_token: &tokio_util::sync::CancellationToken,
    name: &str,
    args: &serde_json::Value,
    display_name: &str,
    bypass_confirm: bool,
    workspace_root: Option<std::path::PathBuf>,
) -> (crate::tools::ToolExecutionOutput, Option<String>) {
    let (agent_mode, auto_confirm) = {
        let s = state.lock().await;
        (s.agent_mode, s.auto_confirm)
    };
    if let crate::tools::AuthorizationDecision::Deny(reason) =
        crate::tools::authorize_tool(name, agent_mode, auto_confirm, bypass_confirm)
    {
        return (
            crate::tools::ToolExecutionOutput::failure(format!("error: {reason}")),
            None,
        );
    }

    struct ToolCleanup {
        state: Arc<Mutex<AppState>>,
        tool_name: String,
    }
    impl Drop for ToolCleanup {
        fn drop(&mut self) {
            let state = self.state.clone();
            let tool_name = self.tool_name.clone();
            tokio::spawn(async move {
                let mut s = state.lock().await;
                if let Some(pos) = s.running_tools.iter().position(|t| t == &tool_name) {
                    s.running_tools.remove(pos);
                }
                if s.config.discord_rpc_enabled {
                    let model_name = s.model_name.clone();
                    let activity = crate::discord_rpc::activity_for_tools(s.running_tools.len());
                    s.discord_rpc
                        .set_activity(activity, &format!("Using model: {}", model_name));
                }
            });
        }
    }

    let diff_opt = get_diff_preview(name, args);

    let needs_confirm = matches!(
        crate::tools::authorize_tool(name, agent_mode, auto_confirm, bypass_confirm),
        crate::tools::AuthorizationDecision::RequireConfirmation
    );
    let mut result = if !needs_confirm {
        dbg_log!("Executing tool '{}' immediately...", name);
        let tool_name = name.to_string();
        {
            let mut s = state.lock().await;
            s.running_tools.push(tool_name.clone());
            if s.config.discord_rpc_enabled {
                let model_name = s.model_name.clone();
                let running_tools = s.running_tools.len();
                s.discord_rpc.set_activity(
                    crate::discord_rpc::activity_for_tools(running_tools),
                    &format!("Using model: {}", model_name),
                );
            }
        }
        let _cleanup = ToolCleanup {
            state: Arc::clone(state),
            tool_name,
        };

        let name_owned = name.to_string();
        let args_owned = args.clone();
        let session_id = { state.lock().await.active_session_id.clone() };
        let workspace_root_for_task = workspace_root.clone();
        let run_fut = tokio::task::spawn_blocking(move || {
            crate::tools::set_active_session_id(Some(session_id));
            crate::tools::set_active_workspace_root(workspace_root_for_task);
            let result = crate::tools::execute_with_metadata(&name_owned, &args_owned);
            crate::tools::set_active_workspace_root(None);
            crate::tools::set_active_session_id(None);
            result
        });

        tokio::select! {
            res = run_fut => {
                res.unwrap_or_else(|e| {
                    crate::tools::ToolExecutionOutput::failure(format!("tool panicked: {e}"))
                })
            }
            _ = cancel_token.cancelled() => {
                dbg_log!("Tool execution cancelled during spawn_blocking await (immediate execution)");
                crate::tools::ToolExecutionOutput::failure(
                    "error: tool execution cancelled by user".to_string(),
                )
            }
        }
    } else {
        dbg_log!("Tool '{}' requires confirmation", name);
        let path = if let Some(p) = args.get("path").and_then(|p| p.as_str()) {
            p.to_string()
        } else if let Some(cmd) = args.get("command").and_then(|c| c.as_str()) {
            cmd.to_string()
        } else if let (Some(src), Some(dest)) = (
            args.get("src").and_then(|s| s.as_str()),
            args.get("dest").and_then(|d| d.as_str()),
        ) {
            format!("{src} -> {dest}")
        } else {
            "?".to_string()
        };
        let (preview, content_bytes) = if let Some(ref d) = diff_opt {
            (d.clone(), d.len())
        } else {
            let content = args.get("content").and_then(|c| c.as_str()).unwrap_or("");
            let preview = content.lines().take(6).collect::<Vec<_>>().join("\n");
            (preview, content.len())
        };
        let (tx, rx) = tokio::sync::oneshot::channel::<bool>();
        {
            let mut s = state.lock().await;
            s.modal_scroll_row = 0;
            s.pending_tool_confirmation = Some(vec![ToolConfirmation {
                tool_name: display_name.to_string(),
                path,
                content_preview: preview,
                content_bytes,
            }]);
            s.tool_confirmation_response = Some(tx);
            s.status = AppStatus::AwaitingToolConfirmation;
        }
        // Notify the user via Ghostty / iTerm2 OSC sequence that a tool needs
        // their approval. Harmless on other terminals.
        let _ = crate::notifications::notify_pending_confirmation(name);
        dbg_log!("Awaiting user confirmation for '{}'", name);
        let res = match rx.await {
            Ok(true) => {
                dbg_log!("User approved tool call '{}', executing...", name);
                let tool_name = name.to_string();
                {
                    let mut s = state.lock().await;
                    s.pending_tool_confirmation = None;
                    let model_name = s.model_name.clone();
                    s.status = AppStatus::Streaming;
                    if s.config.discord_rpc_enabled {
                        s.discord_rpc
                            .set_activity("Thinking", &format!("Using model: {}", model_name));
                    }
                    s.stream_tracker = Some(StreamTracker::new());
                    s.running_tools.push(tool_name.clone());
                    if s.config.discord_rpc_enabled {
                        let model_name = s.model_name.clone();
                        let running_tools = s.running_tools.len();
                        s.discord_rpc.set_activity(
                            crate::discord_rpc::activity_for_tools(running_tools),
                            &format!("Using model: {}", model_name),
                        );
                    }
                }
                let _cleanup = ToolCleanup {
                    state: Arc::clone(state),
                    tool_name,
                };

                let name_owned = name.to_string();
                let args_owned = args.clone();
                let session_id = { state.lock().await.active_session_id.clone() };
                let workspace_root_for_task = workspace_root.clone();
                let run_fut = tokio::task::spawn_blocking(move || {
                    crate::tools::set_active_session_id(Some(session_id));
                    crate::tools::set_active_workspace_root(workspace_root_for_task);
                    let result = crate::tools::execute_with_metadata(&name_owned, &args_owned);
                    crate::tools::set_active_workspace_root(None);
                    crate::tools::set_active_session_id(None);
                    result
                });

                tokio::select! {
                    res = run_fut => {
                        res.unwrap_or_else(|e| {
                            crate::tools::ToolExecutionOutput::failure(format!("tool panicked: {e}"))
                        })
                    }
                    _ = cancel_token.cancelled() => {
                        dbg_log!("Tool execution cancelled during spawn_blocking await");
                        crate::tools::ToolExecutionOutput::failure(
                            "error: tool execution cancelled by user".to_string(),
                        )
                    }
                }
            }
            Ok(false) => {
                dbg_log!("User denied tool call '{}'", name);
                let _ = crate::notifications::notify_finished(
                    crate::notifications::FinishedStatus::Denied,
                );
                crate::tools::ToolExecutionOutput::failure(
                    "error: user denied this tool call".to_string(),
                )
            }
            Err(_) => {
                dbg_log!("Confirmation channel closed for '{}'", name);
                crate::tools::ToolExecutionOutput::failure(
                    "error: confirmation channel closed".to_string(),
                )
            }
        };
        {
            let mut s = state.lock().await;
            s.pending_tool_confirmation = None;
            let model_name = s.model_name.clone();
            s.status = AppStatus::Streaming;
            if s.config.discord_rpc_enabled {
                s.discord_rpc
                    .set_activity("Thinking", &format!("Using model: {}", model_name));
            }
            s.stream_tracker = Some(StreamTracker::new());
        }
        res
    };

    if matches!(
        name,
        "replace_file_content"
            | "multi_replace_file_content"
            | "write_to_file"
            | "delete_file"
            | "move_file"
            | "copy_file"
    ) && result.success
    {
        let cwd = get_tool_project_root(name, args);
        if let Some(errors) = run_compiler_check(&cwd).await {
            result.content.push_str("\n\nCompiler errors/warnings:\n");
            result.content.push_str(&errors);
        }
    }

    (result, diff_opt)
}

const MAX_ACTIVE_SUBAGENTS: usize = 4;

fn push_status_line(s: &mut AppState, text: String) {
    s.history.push(ChatMessage::new("system", text));
    crate::config::save_history(&s.history);
}

/// Drop a leading <think>...</think> block so the main agent only gets the
/// subagent's actual reply, not its reasoning.
/// Run one subagent conversation until it produces a plain reply (no tool
/// call). Tokens stream quietly (not into the main chat view); tool calls
/// surface as status lines and go through the same confirmation modal as
/// the main agent.
async fn run_subagent(
    client: &reqwest::Client,
    state: &Arc<Mutex<AppState>>,
    cancel_token: &tokio_util::sync::CancellationToken,
    agent_id: u32,
) -> Result<String, String> {
    crate::logger::operational_event("subagent.start", serde_json::json!({"agent_id": agent_id}));
    let stream_buffer = Arc::new(Mutex::new(StreamBuffer::new()));
    let mut rounds = 0usize;
    let mut loop_detector = loop_detect::LoopDetector::new(6);
    loop {
        if cancel_token.is_cancelled() {
            crate::logger::operational_event(
                "subagent.finish",
                serde_json::json!({"agent_id": agent_id, "status": "cancelled"}),
            );
            return Err("error: cancelled".to_string());
        }
        let mut history_snapshot: Vec<ChatMessage> = {
            let s = state.lock().await;
            s.subagents
                .iter()
                .find(|a| a.id == agent_id)
                .map(|a| a.history.clone())
                .unwrap_or_default()
        };
        if history_snapshot.is_empty() {
            return Err(format!("error: no subagent with id {agent_id}"));
        }

        let budget_token_limit = { state.lock().await.get_history_token_budget() };
        compact_history_to_budget(&mut history_snapshot, budget_token_limit).await;

        let protocol = { state.lock().await.active_tool_protocol() };
        let agent_mode = { state.lock().await.agent_mode };
        let delegation_contract = {
            let s = state.lock().await;
            s.subagents
                .iter()
                .find(|agent| agent.id == agent_id)
                .map(|agent| {
                    format!(
                        "Delegation contract: write_access={}, allowed_paths={:?}, verification_command={:?}.",
                        agent.write_access, agent.allowed_paths, agent.verification_command
                    )
                })
                .unwrap_or_else(|| "Delegation contract unavailable; remain read-only.".to_string())
        };
        let system_prompt = format!(
            "{}\n\nYou are subagent {agent_id}, working for a main agent in the same \
rustcode session. Complete the task you were given, then reply in plain text \
with NO tool call — that reply is returned to the main agent. Keep the final \
reply compact and information-dense. {delegation_contract}\n\n{}",
            crate::tools::tool_system_prompt(false, protocol, agent_mode),
            crate::context::environment_context()
        );
        let mut msgs: Vec<serde_json::Value> = vec![serde_json::json!({
            "role": "system",
            "content": system_prompt,
        })];
        msgs.extend(history_snapshot.iter().map(|m| {
            if m.role == "tool" {
                serde_json::json!({
                    "role": "user",
                    "content": format!("<tool_result>\n{}\n</tool_result>", m.content),
                })
            } else {
                serde_json::json!({"role": m.role, "content": m.content})
            }
        }));
        let window = { state.lock().await.active_context_window() };
        let budget = window.saturating_sub(RESPONSE_RESERVE_TOKENS).max(512);
        trim_msgs_to_budget(&mut msgs, budget);
        inject_system_reminder(&mut msgs);

        stream_buffer.lock().await.reset();
        let (api_base_url, model_name) = {
            let s = state.lock().await;
            let subagent = s
                .subagents
                .iter()
                .find(|a| a.id == agent_id)
                .expect("Subagent not found");
            let target_model_name = subagent.model.as_deref().unwrap_or(&s.model_name);
            if let Some(profile) = s.config.models.iter().find(|p| p.name == target_model_name) {
                (profile.url.clone(), profile.model.clone())
            } else {
                (s.api_base_url.clone(), s.model_name.clone())
            }
        };
        dbg_log!(
            "subagent {} round {}: requesting {}",
            agent_id,
            rounds,
            model_name
        );
        let request_client = client.clone();
        let request_state = Arc::clone(state);
        let request_cancel = cancel_token.clone();
        let request_buffer = Arc::clone(&stream_buffer);
        let request_api_url = api_base_url.clone();
        let request_model = model_name.clone();
        let request_msgs = msgs.clone();
        let (content, _finish_reason) = match runner::collect_response(move |previous| {
            let mut current_msgs = request_msgs.clone();
            if !previous.is_empty() {
                current_msgs.push(serde_json::json!({
                    "role": "assistant",
                    "content": previous
                }));
                current_msgs.push(serde_json::json!({
                    "role": "user",
                    "content": "continue"
                }));
            }
            let request_client = request_client.clone();
            let request_state = Arc::clone(&request_state);
            let request_cancel = request_cancel.clone();
            let request_buffer = Arc::clone(&request_buffer);
            let request_api_url = request_api_url.clone();
            let request_model = request_model.clone();
            async move {
                request_buffer.lock().await.reset();
                let finish_reason = stream_request(
                    &request_client,
                    request_state,
                    request_cancel,
                    &request_api_url,
                    &request_model,
                    &current_msgs,
                    Arc::clone(&request_buffer),
                    true,
                )
                .await
                .map_err(|e| e.to_string())?;
                let chunk_content = request_buffer.lock().await.content.clone();
                Ok((chunk_content, finish_reason))
            }
        })
        .await
        {
            Ok(result) => result,
            Err(e) => return Err(format!("error: subagent request failed: {e}")),
        };

        if content.is_empty() {
            return Err("error: subagent returned an empty reply".to_string());
        }

        let protocol = { state.lock().await.active_tool_protocol() };
        if let Some(tool_call) = crate::tools::parse_tool_call(&content, protocol) {
            let name = &tool_call.name;
            let args = &tool_call.arguments;
            if let Err(reason) = crate::tools::validate_tool_calls(std::slice::from_ref(&tool_call))
            {
                let mut s = state.lock().await;
                if let Some(a) = s.subagents.iter_mut().find(|a| a.id == agent_id) {
                    a.history.push(ChatMessage::new("assistant", &content));
                    a.history.push(ChatMessage::new(
                        "tool",
                        format!("{name}: error: tool call rejected before execution: {reason}"),
                    ));
                }
                continue;
            }
            let (exact, category) = loop_detect::signatures(name, args);
            if let loop_detect::LoopStatus::Abort(repeats) =
                loop_detector.check_tool(name, &exact, &category)
            {
                return Err(format!(
                    "error: subagent {agent_id} stopped after {repeats} repeated '{name}' actions"
                ));
            }
            rounds += 1;
            let (write_access, allowed_paths) = {
                let s = state.lock().await;
                s.subagents
                    .iter()
                    .find(|agent| agent.id == agent_id)
                    .map(|agent| (agent.write_access, agent.allowed_paths.clone()))
                    .unwrap_or((false, Vec::new()))
            };
            let needs_write_access = is_mutating_tool(name) || name == "run_command";
            let path_outside_contract = args
                .get("path")
                .and_then(|value| value.as_str())
                .is_some_and(|path| {
                    !allowed_paths.iter().any(|allowed| {
                        path == allowed
                            || path.starts_with(&format!("{}/", allowed.trim_end_matches('/')))
                    })
                });
            let (execution, diff_opt) = if needs_write_access && !write_access {
                (
                    crate::tools::ToolExecutionOutput::failure(
                        "error: subagents are read-only by default; request write_access with allowed_paths explicitly".to_string(),
                    ),
                    None,
                )
            } else if write_access && path_outside_contract {
                (
                    crate::tools::ToolExecutionOutput::failure(
                        "error: requested path is outside the subagent allowed_paths contract"
                            .to_string(),
                    ),
                    None,
                )
            } else if crate::tools::is_agent_tool(name) {
                (
                    crate::tools::ToolExecutionOutput::failure(
                        "error: subagents cannot spawn or message other agents".to_string(),
                    ),
                    None,
                )
            } else {
                {
                    let mut s = state.lock().await;
                    let target = args
                        .get("path")
                        .or_else(|| args.get("command"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    push_status_line(&mut s, format!("agent-{agent_id} → {name} {target}"));
                }
                confirm_and_execute(
                    state,
                    cancel_token,
                    name,
                    args,
                    &format!("agent-{agent_id} · {name}"),
                    false,
                    {
                        let s = state.lock().await;
                        s.subagents
                            .iter()
                            .find(|agent| agent.id == agent_id)
                            .and_then(|agent| agent.workspace_root.clone())
                    },
                )
                .await
            };
            // Same rule as the main tool-execution path: the real diff
            // embedded in the tool's own result always wins over the
            // pre-execution, argument-only preview — and a no-op or failed
            // edit gets no fallback preview at all.
            let preview_fallback = if tool_result_precludes_preview_fallback(&execution.content) {
                None
            } else {
                diff_opt
            };
            let final_diff = final_tool_diff(&execution.content, preview_fallback);
            let message = subagent_tool_history_message(name, args, execution, final_diff);
            let mut s = state.lock().await;
            if let Some(a) = s.subagents.iter_mut().find(|a| a.id == agent_id) {
                a.history.push(ChatMessage::new("assistant", &content));
                a.history.push(message);
            }
            continue;
        }

        let mut s = state.lock().await;
        if let Some(a) = s.subagents.iter_mut().find(|a| a.id == agent_id) {
            a.history.push(ChatMessage::new("assistant", &content));
        }
        crate::logger::operational_event(
            "subagent.finish",
            serde_json::json!({"agent_id": agent_id, "status": "completed", "rounds": rounds}),
        );
        return Ok(strip_leading_think(&content).to_string());
    }
}

async fn set_subagent_status(
    state: &Arc<Mutex<AppState>>,
    agent_id: u32,
    status: crate::app::SubAgentStatus,
) {
    let mut s = state.lock().await;
    if let Some(agent) = s.subagents.iter_mut().find(|agent| agent.id == agent_id) {
        agent.status = status;
    }
}

/// Handle spawn_agent / send_agent from the main agent: run a nested
/// subagent conversation (the main agent waits) and return the subagent's
/// reply as the tool result.
async fn handle_agent_tool(
    client: &reqwest::Client,
    state: &Arc<Mutex<AppState>>,
    cancel_token: &tokio_util::sync::CancellationToken,
    name: &str,
    args: &serde_json::Value,
) -> crate::tools::ToolExecutionOutput {
    match name {
        "spawn_agent" => {
            if !state.lock().await.delegation_active {
                return crate::tools::ToolExecutionOutput::failure(
                    "error: subagents are disabled for this task. Run /delegate before starting the task.".to_string(),
                );
            }
            let Some(task) = args
                .get("task")
                .and_then(|t| t.as_str())
                .filter(|t| !t.trim().is_empty())
            else {
                return crate::tools::ToolExecutionOutput::failure(
                    "error: missing 'task' argument".to_string(),
                );
            };
            let model = args
                .get("model")
                .and_then(|m| m.as_str())
                .map(|s| s.to_string());
            let write_access = args
                .get("write_access")
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            let allowed_paths = args
                .get("allowed_paths")
                .and_then(|value| value.as_array())
                .map(|paths| {
                    paths
                        .iter()
                        .filter_map(|path| path.as_str().map(str::to_string))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if write_access && allowed_paths.is_empty() {
                return crate::tools::ToolExecutionOutput::failure(
                    "error: write-enabled subagents require at least one allowed_paths entry"
                        .to_string(),
                );
            }
            let verification_command = args
                .get("verification_command")
                .and_then(|value| value.as_str())
                .map(str::to_string);
            let verification_label = verification_command
                .as_deref()
                .unwrap_or("none")
                .to_string();
            let agent_id = {
                let mut s = state.lock().await;
                let active_count = s
                    .subagents
                    .iter()
                    .filter(|agent| agent.status == crate::app::SubAgentStatus::Running)
                    .count();
                if active_count >= MAX_ACTIVE_SUBAGENTS {
                    return crate::tools::ToolExecutionOutput::failure(format!(
                        "error: maximum active subagents reached ({MAX_ACTIVE_SUBAGENTS}); wait for an existing agent to finish"
                    ));
                }
                let id = s.next_subagent_id;
                s.next_subagent_id += 1;
                let workspace_root = if write_access {
                    match crate::config::create_subagent_workspace(&s.active_session_id, id) {
                        Ok(path) => Some(path),
                        Err(error) => {
                            return crate::tools::ToolExecutionOutput::failure(format!(
                                "error: unable to create isolated subagent workspace: {error}"
                            ));
                        }
                    }
                } else {
                    None
                };
                s.subagents.push(crate::app::SubAgent {
                    id,
                    task: task.to_string(),
                    model,
                    history: vec![ChatMessage::new("user", task)],
                    status: crate::app::SubAgentStatus::Running,
                    write_access,
                    allowed_paths,
                    verification_command,
                    workspace_root,
                    review_manifest: None,
                });
                let brief: String = task.chars().take(60).collect();
                push_status_line(
                    &mut s,
                    format!(
                        "agent-{id} spawned: {brief} (write_access={write_access}, verify={})",
                        verification_label
                    ),
                );
                id
            };
            let reply = run_subagent(client, state, cancel_token, agent_id).await;
            let failed = reply.is_err();
            let reply = reply.unwrap_or_else(|error| error);
            let review_manifest = {
                let s = state.lock().await;
                s.subagents
                    .iter()
                    .find(|agent| agent.id == agent_id)
                    .and_then(|agent| agent.workspace_root.as_ref())
                    .and_then(|workspace| {
                        crate::config::write_subagent_review_manifest(workspace, agent_id)
                    })
            };
            if let Some(manifest) = review_manifest
                && let Some(agent) = state
                    .lock()
                    .await
                    .subagents
                    .iter_mut()
                    .find(|agent| agent.id == agent_id)
            {
                agent.review_manifest = Some(manifest);
            }
            set_subagent_status(
                state,
                agent_id,
                if failed {
                    crate::app::SubAgentStatus::Failed
                } else if cancel_token.is_cancelled() {
                    crate::app::SubAgentStatus::Cancelled
                } else {
                    crate::app::SubAgentStatus::Completed
                },
            )
            .await;
            push_status_line(&mut *state.lock().await, format!("agent-{agent_id} done"));
            let content = format!("(subagent id {agent_id} — follow up with send_agent)\n{reply}");
            if failed {
                crate::tools::ToolExecutionOutput::failure(content)
            } else {
                crate::tools::ToolExecutionOutput::success(content)
            }
        }
        "send_agent" => {
            if !state.lock().await.delegation_active {
                return crate::tools::ToolExecutionOutput::failure(
                    "error: subagents are disabled for this task. Run /delegate before starting the task.".to_string(),
                );
            }
            let id = args.get("id").and_then(|v| {
                v.as_u64()
                    .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
            });
            let Some(id) = id else {
                return crate::tools::ToolExecutionOutput::failure(
                    "error: missing or invalid 'id' argument".to_string(),
                );
            };
            let id = id as u32;
            let Some(message) = args
                .get("message")
                .and_then(|m| m.as_str())
                .filter(|m| !m.trim().is_empty())
            else {
                return crate::tools::ToolExecutionOutput::failure(
                    "error: missing 'message' argument".to_string(),
                );
            };
            {
                let mut s = state.lock().await;
                let Some(task) = s
                    .subagents
                    .iter()
                    .find(|a| a.id == id)
                    .map(|a| a.task.chars().take(40).collect::<String>())
                else {
                    let known: Vec<String> = s.subagents.iter().map(|a| a.id.to_string()).collect();
                    return crate::tools::ToolExecutionOutput::failure(if known.is_empty() {
                        "error: no subagents exist — use spawn_agent first".to_string()
                    } else {
                        format!(
                            "error: no subagent with id {id}. Known ids: {}",
                            known.join(", ")
                        )
                    });
                };
                push_status_line(&mut s, format!("agent-{id} ← follow-up ({task})"));
                if let Some(a) = s.subagents.iter_mut().find(|a| a.id == id) {
                    if a.status == crate::app::SubAgentStatus::Failed
                        || a.status == crate::app::SubAgentStatus::Cancelled
                    {
                        return crate::tools::ToolExecutionOutput::failure(format!(
                            "error: subagent {id} is not available for follow-up"
                        ));
                    }
                    a.status = crate::app::SubAgentStatus::Running;
                    a.history.push(ChatMessage::new("user", message));
                }
            }
            let reply = run_subagent(client, state, cancel_token, id).await;
            let failed = reply.is_err();
            let reply = reply.unwrap_or_else(|error| error);
            set_subagent_status(
                state,
                id,
                if failed {
                    crate::app::SubAgentStatus::Failed
                } else if cancel_token.is_cancelled() {
                    crate::app::SubAgentStatus::Cancelled
                } else {
                    crate::app::SubAgentStatus::Completed
                },
            )
            .await;
            push_status_line(&mut *state.lock().await, format!("agent-{id} done"));
            let content = format!("(subagent id {id})\n{reply}");
            if failed {
                crate::tools::ToolExecutionOutput::failure(content)
            } else {
                crate::tools::ToolExecutionOutput::success(content)
            }
        }
        "set_goal" => {
            let goal = args.get("goal").and_then(|g| g.as_str()).unwrap_or("");
            if goal.is_empty() {
                return crate::tools::ToolExecutionOutput::failure(
                    "error: missing 'goal' argument".to_string(),
                );
            }
            let mut s = state.lock().await;
            s.continuous_mode = true;
            s.input_buffer.clear();
            s.cursor_position = 0;
            crate::tools::ToolExecutionOutput::success(format!(
                "Success: Goal set to '{}'. You are now in continuous autoloop mode. Continue executing tools to complete this goal, and call the 'complete_task' tool when fully done.",
                goal
            ))
        }
        "todo_write" => {
            let Some(arr) = args.get("todos").and_then(|t| t.as_array()) else {
                return crate::tools::ToolExecutionOutput::failure(
                    "error: missing 'todos' array argument".to_string(),
                );
            };
            let mut todos = Vec::with_capacity(arr.len());
            for item in arr {
                let Some(content) = item
                    .get("content")
                    .and_then(|c| c.as_str())
                    .filter(|c| !c.trim().is_empty())
                else {
                    return crate::tools::ToolExecutionOutput::failure(
                        "error: each todo needs a non-empty 'content'".to_string(),
                    );
                };
                let status = item
                    .get("status")
                    .and_then(|s| s.as_str())
                    .unwrap_or("pending")
                    .to_string();
                let priority = item
                    .get("priority")
                    .and_then(|s| s.as_str())
                    .unwrap_or("medium")
                    .to_string();
                todos.push(crate::app::TodoItem {
                    content: content.to_string(),
                    status,
                    priority,
                });
            }
            let summary = format!(
                "Plan updated ({} item(s)): {}",
                todos.len(),
                todos
                    .iter()
                    .map(|t| format!("[{}] {}", t.status, t.content))
                    .collect::<Vec<_>>()
                    .join(" | ")
            );
            let mut s = state.lock().await;
            s.todos = todos;
            drop(s);
            crate::tools::ToolExecutionOutput::success(summary)
        }
        _ => crate::tools::ToolExecutionOutput::failure(format!(
            "error: unknown agent tool '{name}'"
        )),
    }
}

/// Generate a title from the first user message using the small model.
/// Returns None if the message starts with '/' (slash command).
pub async fn generate_title(
    client: &reqwest::Client,
    config: &crate::config::AppConfig,
    first_message: &str,
) -> Option<String> {
    if first_message.trim().starts_with('/') {
        return None;
    }

    let small_model_name = config.default.small();
    let (url, model) = crate::config::resolve_model_endpoint(config, small_model_name);

    let first_line = first_message.lines().next()?;
    let prompt = format!(
        "Generate a short, concise title (max 5 words) summarizing this user's coding request/intent. Do not use quotes, punctuation, or any introductory text. Return only the title itself.\n\nIntent: {}",
        first_line.trim()
    );

    let messages = vec![serde_json::json!({
        "role": "user",
        "content": prompt
    })];

    let payload = serde_json::json!({
        "model": model,
        "messages": messages,
        "max_tokens": 30,
        "temperature": 0.3,
    });

    let res = client.post(&url).json(&payload).send().await.ok()?;

    if !res.status().is_success() {
        return None;
    }

    let json: serde_json::Value = res.json().await.ok()?;
    let title = json
        .get("choices")?
        .get(0)?
        .get("message")?
        .get("content")?
        .as_str()?;

    let cleaned_title = title.trim().trim_matches('"').trim().to_string();
    if cleaned_title.is_empty() {
        None
    } else {
        Some(cleaned_title)
    }
}

/// Push the incoming prompt (user message, or a background-task wakeup system
/// note) onto history, persist it, and reset the per-response scratch fields.
async fn record_prompt_to_history(
    state: &Arc<Mutex<AppState>>,
    is_wakeup: bool,
    next_prompt: &str,
) {
    let mut s = state.lock().await;
    if is_wakeup {
        let task_id = next_prompt.strip_prefix("__task_wakeup__:").unwrap_or("");
        s.history.push(ChatMessage::new(
            "system",
            format!("Task {task_id} has finished running in the background."),
        ));
    } else {
        s.history
            .push(ChatMessage::new("user", next_prompt.to_string()));
    }
    let active_id = s.active_session_id.clone();
    crate::config::save_session_history(&active_id, &s.history);
    s.current_response.clear();
    s.current_token_usage = None;
    s.response_time = None;
}

/// Fire-and-forget: generate a session title from the first user message.
async fn spawn_title_generation(
    client: &reqwest::Client,
    state: &Arc<Mutex<AppState>>,
    first_msg: String,
) {
    let client_clone = client.clone();
    // Captured before spawn so title reflects the session as of this prompt.
    let (config_clone, session_id) = {
        let s = state.lock().await;
        (s.config.clone(), s.active_session_id.clone())
    };
    let state_clone = Arc::clone(state);
    tokio::spawn(async move {
        if let Some(title) = generate_title(&client_clone, &config_clone, &first_msg).await {
            crate::config::save_session_title(&session_id, &title);
            let mut s = state_clone.lock().await;
            s.invalidate_session_title_cache();
            s.request_redraw();
        }
    });
}

#[allow(unused_assignments)]
/// Assemble the turn-varying context tail appended to the last message. Kept
/// separate from the static system prefix so the provider prompt cache stays
/// warm: this lists the files already in context (so the agent doesn't re-read
/// them) and re-injects the persistent task plan so work continues across turns
/// instead of re-planning from scratch.
/// Render the volatile runtime block — the "cache divider" that must sit at the
/// very end of the request payload, after the static (cacheable) prefix and the
/// conversation. Everything here changes turn-to-turn (clock, cwd, quota), so
/// keeping it strictly at the tail lets the provider's implicit prefix cache
/// cover the entire static prefix plus the stable conversation history.
fn build_volatile_context_block(
    token_usage: Option<&crate::app::TokenUsage>,
    quota_remaining: Option<f32>,
    context_window: u32,
) -> String {
    let now = chrono::Local::now();
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "(unknown)".to_string());
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "(unknown)".to_string());

    let mut b = String::from("# Runtime Context (volatile — do not rely on this being cached)\n");
    b.push_str(&format!(
        "- Current date/time: {}\n",
        now.format("%A %Y-%m-%d %H:%M:%S %Z")
    ));
    b.push_str(&format!("- Working directory: {cwd}\n"));
    b.push_str(&format!(
        "- Platform: {} {}\n",
        std::env::consts::OS,
        std::env::consts::ARCH
    ));
    b.push_str(&format!("- Shell: {shell}\n"));
    b.push_str(&format!("- Context window: {context_window} tokens\n"));
    if let Some(u) = token_usage {
        b.push_str(&format!(
            "- Last-turn token usage: prompt {} / completion {} / total {}",
            u.prompt_tokens, u.completion_tokens, u.total_tokens
        ));
        if let Some(cached) = u.cached_tokens {
            b.push_str(&format!(" (cached {cached})"));
        }
        b.push('\n');
    }
    if let Some(q) = quota_remaining {
        b.push_str(&format!("- Model quota remaining: {q:.1}%\n"));
    }
    b
}

fn build_dynamic_context_tail(
    context_section: String,
    read_files: &[String],
    todos: &[crate::app::TodoItem],
) -> String {
    let mut fragments = vec![history::ContextFragment::new(
        "environment",
        context_section,
    )];

    if !read_files.is_empty() {
        fragments.push(history::ContextFragment::new(
            "files",
            format!(
                "# Files already in context (do NOT re-read these unless they changed on disk)\n{}",
                read_files
                    .iter()
                    .map(|f| format!("- {f}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
        ));
    }

    if !todos.is_empty() {
        let mut plan =
            String::from("# Your current task plan (execute in order; update via todo_write)\n");
        for (i, t) in todos.iter().enumerate() {
            let mark = match t.status.as_str() {
                "completed" => "[x]",
                "in_progress" => "[~]",
                _ => "[ ]",
            };
            plan.push_str(&format!(
                "{}. {} {} ({})\n",
                i + 1,
                mark,
                t.content,
                t.priority
            ));
        }
        fragments.push(history::ContextFragment::new("task plan", plan));
    }

    history::render_context_fragments(&fragments)
}

/// Cheap identity fingerprint for a history message, used to tell whether the
/// prefix we snapshotted is still the same prefix after a lock has been released
/// and re-acquired. `ChatMessage` has no `PartialEq`, and hashing role +
/// timestamp + content is enough to catch a rewritten or replaced entry without
/// cloning the (potentially large) content.
fn message_identity(m: &ChatMessage) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    m.role.hash(&mut hasher);
    m.timestamp.hash(&mut hasher);
    m.content.hash(&mut hasher);
    hasher.finish()
}

/// Assemble the full provider request for one agent turn.
///
/// Runs AI compaction if the history is long enough, snapshots the eligible
/// history, then builds the message array: a STATIC system prefix (tool
/// protocol + agent mode only, so the provider prompt cache stays warm) plus
/// the conversation, with all turn-varying context (environment delta,
/// files-in-context, task plan) appended to the last message. Finally trims to
/// the context-window budget and injects the system reminder. `tool_rounds` is
/// only used to decide whether a one-time "context window full" notice is shown.
async fn prepare_turn_request(
    client: &reqwest::Client,
    state: &Arc<Mutex<AppState>>,
    tool_rounds: usize,
) -> Vec<serde_json::Value> {
    // Try AI-driven compaction if history is long enough.
    //
    // The summarizer is a network round-trip, so the AppState mutex must NOT be
    // held while it runs: the TUI draw loop locks the same mutex every frame and
    // would freeze for the whole call. Instead we take a snapshot of the history
    // under a short lock, compact the owned copy with the lock released, then
    // re-acquire and merge the result back in.
    {
        let (api_url, model_name, budget, mut working_history) = {
            let s = state.lock().await;
            (
                s.api_base_url.clone(),
                s.model_name.clone(),
                s.get_history_token_budget() as usize,
                s.history.clone(),
            )
        };
        let pre_len = working_history.len();
        let pre_identity: Vec<u64> = working_history.iter().map(message_identity).collect();

        // Lock released here: this await performs I/O.
        let compacted =
            compaction::maybe_compact(client, &api_url, &model_name, &mut working_history, budget)
                .await;

        // Merge policy for history appended while the lock was down (a tool
        // result or a user message can land mid-compaction):
        //
        //  - unchanged history  -> write the compacted copy back wholesale;
        //  - history grew, and the prefix we compacted is still intact -> keep
        //    the compacted prefix and re-append the new tail verbatim. Nothing
        //    that arrived during the call is lost;
        //  - anything else (history shrank or was rewritten underneath us, e.g.
        //    /new, /compact, a rollback) -> discard our copy entirely and log
        //    the miss. Compaction is best-effort and will retry on the next
        //    turn; clobbering the live history is not an acceptable trade.
        //
        // Note that `maybe_compact` also performs local tool-output pruning even
        // when it returns false, so the write-back is attempted regardless of
        // the return value; the flag only gates the cache invalidation below.
        let mut s = state.lock().await;
        let prefix_intact = s.history.len() >= pre_len
            && s.history
                .iter()
                .take(pre_len)
                .map(message_identity)
                .eq(pre_identity.iter().copied());
        if prefix_intact {
            if s.history.len() > pre_len {
                working_history.extend(s.history.drain(pre_len..));
            }
            s.history = working_history;
            if compacted {
                dbg_log!("History compacted via AI summarization. Clearing read/dedup cache.");
                s.recent_read_calls.clear();
                s.recent_read_outputs.clear();
                s.read_file_mtimes.clear();
                crate::config::save_history(&s.history);
            }
        } else {
            dbg_log!(
                "Skipping compaction write-back: history changed underneath the summarizer ({} messages before, {} now). Live history kept as-is.",
                pre_len,
                s.history.len()
            );
        }
        drop(s);
    }

    // Everything the request needs from AppState is read in one guarded block so
    // the lock is taken a couple of times instead of once per field. The
    // environment snapshot is captured first because it touches the filesystem.
    let current_snapshot = crate::context::ContextSnapshot::capture();
    let (
        mut history_snapshot,
        budget_token_limit,
        read_files,
        todos,
        volatile_usage,
        volatile_quota,
        volatile_window,
        context_section,
        system_prompt,
    ) = {
        let mut s = state.lock().await;
        let history_snapshot: Vec<ChatMessage> = s
            .history
            .iter()
            .filter(|m| {
                matches!(m.role.as_str(), "user" | "assistant" | "tool")
                    && !m.content.starts_with('/')
                    || is_model_directed_note(m)
            })
            .cloned()
            .collect();
        let budget_token_limit = s.get_history_token_budget();
        let mut read_files: Vec<String> = s.read_file_mtimes.keys().cloned().collect();
        read_files.sort();
        let todos = s.todos.clone();
        let volatile_usage = s.current_token_usage.clone();
        let volatile_quota = s.model_quota_remaining;
        let volatile_window = s.active_context_window();
        let context_section = match &s.context_snapshot {
            Some(prev) => prev
                .diff(&current_snapshot)
                .unwrap_or_else(|| "# Environment\n(unchanged since session start)".to_string()),
            None => crate::context::environment_context(),
        };
        let protocol = s.active_tool_protocol();
        let agent_mode = s.agent_mode;
        let delegation_active = s.delegation_active;
        let system_prompt = s
            .prompt_cache
            .system_prompt(delegation_active, protocol, agent_mode)
            .to_string();
        // Store the snapshot if this is the first turn.
        if s.context_snapshot.is_none() {
            s.context_snapshot = Some(current_snapshot);
        }
        (
            history_snapshot,
            budget_token_limit,
            read_files,
            todos,
            volatile_usage,
            volatile_quota,
            volatile_window,
            context_section,
            system_prompt,
        )
    };

    compact_history_to_budget(&mut history_snapshot, budget_token_limit).await;

    // The system prompt is kept STATIC across turns (it only depends on the
    // tool protocol and agent mode, which don't change mid-task). A stable
    // prefix lets the provider's automatic prompt cache stay warm — every
    // round after the first re-bills only the dynamic tail below instead of
    // the whole tool-definition block. Turn-varying context (environment
    // delta, files-in-context, task plan) is appended to the LAST message
    // instead, so it never invalidates the cached prefix.
    //
    // The static system prompt is served from AppState's PromptCache: it's only
    // rebuilt (skill scan + MCP schema serialization) when the protocol, agent
    // mode, or MCP tool set changes, not on every turn. It is read in the
    // grouped state block above along with the rest of the turn inputs.
    //
    // Build the turn-varying context tail (appended to the last message
    // after the history is assembled, to preserve the cached prefix). The
    // volatile runtime block (clock/cwd/quota) goes last, as the explicit cache
    // divider at the very end of the payload.
    let mut dynamic_context = build_dynamic_context_tail(context_section, &read_files, &todos);
    let volatile_block =
        build_volatile_context_block(volatile_usage.as_ref(), volatile_quota, volatile_window);
    if !dynamic_context.is_empty() {
        dynamic_context.push_str("\n\n");
    }
    dynamic_context.push_str(&volatile_block);
    let mut msgs = history::to_messages(&history_snapshot, system_prompt.clone());

    // Attach turn-varying context to the tail so the static system prefix
    // stays cache-stable. Done before budget trimming so its size counts
    // toward the budget.
    append_to_last_message(&mut msgs, &dynamic_context);

    let budget = volatile_window
        .saturating_sub(RESPONSE_RESERVE_TOKENS)
        .max(512);
    let dropped = trim_msgs_to_budget(&mut msgs, budget);
    inject_system_reminder(&mut msgs);
    if dropped > 0 {
        dbg_log!(
            "context budget {} tokens exceeded: dropped {} oldest message(s)",
            budget,
            dropped
        );
        if tool_rounds == 0 {
            let mut s = state.lock().await;
            s.history.push(ChatMessage::new(
                "system",
                format!(
                    "context window full: dropped {} oldest message(s) from the request. Use /new to start fresh.",
                    dropped
                ),
            ));
        }
    }

    msgs
}

/// Execute a batch of tool calls and return `(name, result, diff)` per call.
///
/// When `approved` is false every call resolves to a denial message. Otherwise
/// calls execute in model order. Read-only calls could be parallelized safely,
/// but preserving one ordering rule for every batch prevents edits, commands,
/// and reads from racing each other or hiding dependencies. If any mutating
/// tool ran, a single cached compiler check is appended to the first mutating
/// tool's result so build errors surface inline.
fn stable_arguments_hash(arguments: &serde_json::Value) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    arguments.to_string().hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn append_compiler_diagnostics(result: &mut ToolResult, diagnostics: &str) {
    result
        .content
        .push_str("\n\nLSP/Compiler errors detected in workspace, please fix:\n");
    result.content.push_str(diagnostics);
}

fn tool_result_from_execution(
    tool_name: &str,
    args: &serde_json::Value,
    execution: crate::tools::ToolExecutionOutput,
    diff: Option<String>,
) -> ToolResult {
    let changed_paths = if is_mutating_tool(tool_name) {
        args.get("path")
            .and_then(|value| value.as_str())
            .map(|path| vec![path.to_string()])
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    ToolResult {
        tool_name: tool_name.to_string(),
        content: execution.content,
        diff,
        file_preview: get_file_preview(tool_name, args),
        metadata: ToolResultMetadata {
            call_id: None,
            arguments_hash: stable_arguments_hash(args),
            success: execution.success,
            exit_code: execution.exit_code,
            changed_paths,
            truncated: execution.truncated,
            full_output_artifact: None,
        },
    }
}

fn finalize_tool_result_for_prefix(
    mut result: ToolResult,
    deferred_notice: Option<&str>,
    prefix: &str,
) -> ToolResult {
    if let Some(notice) = deferred_notice {
        result.content.push_str("\n\n");
        result.content.push_str(notice);
    }
    let bounded = truncate_tool_output_for_message(&result.tool_name, result.content, prefix);
    result.content = bounded.content;
    if bounded.truncated {
        result.metadata.truncated = true;
        result.metadata.full_output_artifact = bounded.full_output_artifact;
    }
    result
}

fn finalize_tool_result(result: ToolResult, deferred_notice: Option<&str>) -> ToolResult {
    let prefix = format!("{}: ", result.tool_name);
    finalize_tool_result_for_prefix(result, deferred_notice, &prefix)
}

fn tool_result_history_message(
    result: ToolResult,
    answered_call: Option<String>,
) -> ChatMessage {
    let prefix = format!("{}: ", result.tool_name);
    tool_result_history_message_with_prefix(result, &prefix, answered_call)
}

fn tool_result_history_message_with_prefix(
    result: ToolResult,
    prefix: &str,
    answered_call: Option<String>,
) -> ChatMessage {
    let ToolResult {
        tool_name,
        content,
        diff,
        file_preview,
        metadata,
    } = result;
    ChatMessage::new("tool", format!("{prefix}{content}"))
        .answering(answered_call)
        .with_diff(diff)
        .with_file_preview(file_preview)
        .with_tool_result(crate::app::ToolResultRecord {
            tool_name,
            arguments_hash: metadata.arguments_hash,
            success: metadata.success,
            exit_code: metadata.exit_code,
            changed_paths: metadata.changed_paths,
            truncated: metadata.truncated,
            full_output_artifact: metadata.full_output_artifact,
        })
}

pub(crate) fn bounded_tool_result_history_message(
    result: ToolResult,
    prefix: &str,
    answered_call: Option<String>,
) -> ChatMessage {
    let result = finalize_tool_result_for_prefix(result, None, prefix);
    tool_result_history_message_with_prefix(result, prefix, answered_call)
}

fn subagent_tool_history_message(
    tool_name: &str,
    args: &serde_json::Value,
    execution: crate::tools::ToolExecutionOutput,
    diff: Option<String>,
) -> ChatMessage {
    let prefix = format!("{tool_name}: ");
    bounded_tool_result_history_message(
        tool_result_from_execution(tool_name, args, execution, diff),
        &prefix,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
async fn execute_tool_batch(
    client: &reqwest::Client,
    state: &Arc<Mutex<AppState>>,
    cancel_token: &tokio_util::sync::CancellationToken,
    tool_calls: &[crate::tools::ToolCall],
    approved: bool,
    edit_root: &Option<std::path::PathBuf>,
    compile_dirty: &mut bool,
    compile_cache: &mut Option<(std::path::PathBuf, Option<String>)>,
    deferred_notice: Option<String>,
) -> Vec<ToolResult> {
    if !approved {
        return tool_calls
            .iter()
            .map(|call| ToolResult {
                tool_name: call.name.clone(),
                content: "error: user denied this tool call".to_string(),
                diff: None,
                file_preview: None,
                metadata: ToolResultMetadata {
                    success: false,
                    ..Default::default()
                },
            })
            .collect::<Vec<_>>();
    }

    // Gate concurrency per call rather than per batch. Consecutive
    // parallel-capable calls (reads) run together; anything that can change the
    // workspace runs alone, after every earlier call has finished and before any
    // later one starts. A batch that mixes the two therefore still parallelises
    // its reads instead of falling back to fully sequential execution, and a
    // read written after an edit in the same batch still observes that edit.
    // The recursive single-call path keeps repeat detection, cancellation, and
    // result shaping in one place; `join_all` preserves input order.
    if tool_calls.len() > 1 {
        let mut results = Vec::with_capacity(tool_calls.len());
        let mut index = 0;
        while index < tool_calls.len() {
            let parallel_run_end = tool_calls[index..]
                .iter()
                .position(|call| !crate::tools::supports_parallel_execution(&call.name))
                .map(|offset| index + offset)
                .unwrap_or(tool_calls.len());

            if parallel_run_end > index + 1 {
                let futures = tool_calls[index..parallel_run_end]
                    .iter()
                    .map(|call| async {
                        let mut read_dirty = false;
                        let mut read_cache = None;
                        execute_tool_batch(
                            client,
                            state,
                            cancel_token,
                            std::slice::from_ref(call),
                            approved,
                            &None,
                            &mut read_dirty,
                            &mut read_cache,
                            deferred_notice.clone(),
                        )
                        .await
                    });
                results.extend(
                    futures_util::future::join_all(futures)
                        .await
                        .into_iter()
                        .flatten(),
                );
                index = parallel_run_end;
                continue;
            }

            results.extend(
                Box::pin(execute_tool_batch(
                    client,
                    state,
                    cancel_token,
                    std::slice::from_ref(&tool_calls[index]),
                    approved,
                    edit_root,
                    compile_dirty,
                    compile_cache,
                    deferred_notice.clone(),
                ))
                .await,
            );
            index += 1;
        }
        return results;
    }

    dbg_log!("Executing {} tool calls sequentially", tool_calls.len());
    let mut results = Vec::with_capacity(tool_calls.len());
    for call in tool_calls {
        let name = &call.name;
        let args = &call.arguments;
        let client_clone = client.clone();
        let state_clone = Arc::clone(state);
        let cancel_token_clone = cancel_token.clone();
        let name_clone = name.clone();
        let args_clone = args.clone();
        let plan_mode_denied = {
            let plan_mode = state.lock().await.agent_mode == crate::config::AgentMode::Plan;
            plan_mode && !crate::tools::allowed_in_plan_mode(name)
        };
        let (executed_name, execution, diff_opt, replay_artifact) = async move {
            let is_read_only = is_read_only_tool(&name_clone);
            let mut replay_artifact = None;

            // Repeat-loop guard for read-only tools. For view_file we go
            // further than a signature match: a re-read is only blocked when
            // the file is UNCHANGED on disk since the last read, so the agent
            // can always refresh after a (possibly partial) edit. Other
            // read-only tools use a signature window.
            let mut is_repeat = false;
            let mut view_path: Option<String> = None;
            let mut view_mtime: Option<std::time::SystemTime> = None;

            if is_read_only {
                if name_clone == "view_file" {
                    if let Some(p) = args_clone.get("path").and_then(|p| p.as_str()) {
                        let sig = tool_signature(&name_clone, &args_clone);
                        let already_seen = {
                            let s = state_clone.lock().await;
                            s.recent_read_calls.iter().any(|c| c == &sig)
                        };
                        if already_seen {
                            let current = path_mtime(p);
                            let stored = {
                                let s = state_clone.lock().await;
                                s.read_file_mtimes.get(p).copied()
                            };
                            is_repeat = view_file_unchanged_since_last_read(stored, current);
                        }
                        view_path = Some(p.to_string());
                        view_mtime = path_mtime(p);
                    }
                } else {
                    let sig = tool_signature(&name_clone, &args_clone);
                    is_repeat = {
                        let s = state_clone.lock().await;
                        s.recent_read_calls.iter().any(|c| c == &sig)
                    };
                }
            }

            let (execution, diff_opt) = if is_repeat {
                // Serve the content again when it is small enough to be worth
                // repeating. A notice that points at earlier context is not an
                // answer: a model that wanted those lines simply asks a third
                // time, which is how an identical read repeats four times in a
                // row while the guard keeps declining it.
                let cached = {
                    let s = state_clone.lock().await;
                    s.recent_read_outputs
                        .get(&tool_signature(&name_clone, &args_clone))
                        .cloned()
                };
                match cached {
                    Some(previous) => {
                        let content = if let Some(mut content) = previous.replayable_content {
                            content.insert_str(
                                0,
                                "[Unchanged since the last read of this exact range — repeating that output. \
Re-reading will not produce anything new; act on this content.]\n",
                            );
                            content
                        } else {
                            let mut notice = "[Notice: This exact read was already executed, but its output exceeded the repeat cache limit and is not repeated. Use the original result or request a narrower range.".to_string();
                            if let Some(path) = previous.full_output_artifact.as_deref() {
                                notice.push_str(&format!(
                                    " The bounded output remains available at: {path}."
                                ));
                            }
                            notice.push(']');
                            notice
                        };
                        replay_artifact = previous.full_output_artifact;
                        (
                            crate::tools::ToolExecutionOutput {
                                content,
                                success: previous.success,
                                exit_code: previous.exit_code,
                                truncated: previous.truncated,
                            },
                            None,
                        )
                    }
                    None => (
                        crate::tools::ToolExecutionOutput::success("[Notice: This exact read tool call was previously executed with identical arguments, \
and the file has not changed since. Its output is above in the context — use it. To see something \
different, read another range or make an edit first; repeating this call returns this same notice.]"
                            .to_string()),
                        None,
                    ),
                }
            } else if name_clone == "ask_question" {
                (
                    ask_user_question(&state_clone, &cancel_token_clone, &args_clone).await,
                    None,
                )
            } else if plan_mode_denied {
                (
                    crate::tools::ToolExecutionOutput::failure(
                        "error: Plan mode is active; this tool is not permitted.".to_string(),
                    ),
                    None,
                )
            } else if crate::tools::is_agent_tool(&name_clone) {
                (
                    handle_agent_tool(
                        &client_clone,
                        &state_clone,
                        &cancel_token_clone,
                        &name_clone,
                        &args_clone,
                    )
                    .await,
                    None,
                )
            } else {
                confirm_and_execute(
                    &state_clone,
                    &cancel_token_clone,
                    &name_clone,
                    &args_clone,
                    &name_clone,
                    true, // bypass confirmation
                    None,
                )
                .await
            };

            // Record this call so future identical read-only calls are caught.
            {
                let mut s = state_clone.lock().await;
                if let Some(p) = view_path
                    && !is_repeat
                {
                    if let Some(mt) = view_mtime {
                        s.read_file_mtimes.insert(p, mt);
                    } else {
                        // File couldn't be stat'd (e.g. already gone);
                        // drop any stale entry so a later read is allowed.
                        s.read_file_mtimes.remove(&p);
                    }
                }
                if is_read_only && !is_repeat {
                    let sig = tool_signature(&name_clone, &args_clone);
                    s.recent_read_outputs.insert(
                        sig.clone(),
                        crate::app::CachedReadOutput {
                            replayable_content: (execution.content.len()
                                <= REPLAYABLE_READ_LIMIT)
                                .then(|| execution.content.clone()),
                            success: execution.success,
                            exit_code: execution.exit_code,
                            truncated: execution.truncated,
                            full_output_artifact: None,
                        },
                    );
                    if !s.recent_read_calls.contains(&sig) {
                        s.recent_read_calls.push_back(sig);
                        while s.recent_read_calls.len() > 8 {
                            s.recent_read_calls.pop_front();
                        }
                        while s.recent_read_outputs.len() > 8
                            && let Some(oldest) = s
                                .recent_read_outputs
                                .keys()
                                .find(|key| !s.recent_read_calls.contains(key))
                                .cloned()
                        {
                            s.recent_read_outputs.remove(&oldest);
                        }
                    }
                }
            }

            (name_clone, execution, diff_opt, replay_artifact)
        }
        .await;
        // The real diff (from actual before/after file content, embedded by
        // the edit tools themselves) always wins over the pre-execution,
        // argument-only preview — that preview is provisional and must
        // never stand in for what the transcript/UI shows as the final
        // result. It's kept only as a fallback for the rare case where a
        // tool has no embedded diff of its own (e.g. a legacy write_to_file
        // preview) but a preview was still computed — never for a no-op or
        // failed edit, which must show no diff at all.
        let preview_fallback = if tool_result_precludes_preview_fallback(&execution.content) {
            None
        } else {
            diff_opt
        };
        let final_diff = final_tool_diff(&execution.content, preview_fallback);
        let mut result = tool_result_from_execution(
            &executed_name,
            args,
            execution,
            final_diff,
        );
        result.metadata.full_output_artifact = replay_artifact;
        results.push(result);
        if cancel_token.is_cancelled() {
            break;
        }
    }
    // Whether THIS batch changed anything, which is not the same as the task
    // having changed something earlier. Clearing the read cache on the sticky
    // task flag disabled repeat detection for every batch after the first edit,
    // so identical full-file reads sailed through back to back.
    let batch_changed_files = results.iter().any(|result| {
        is_mutating_tool(&result.tool_name)
            && result.metadata.success
            && !result
                .content
                .trim_start()
                .to_ascii_lowercase()
                .starts_with("error")
    });
    if batch_changed_files {
        {
            let mut s = state.lock().await;
            s.recent_read_calls.clear();
            s.recent_read_outputs.clear();
            s.read_file_mtimes.clear();
        }
        let root = edit_root
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        if let Some(compiler_errors) = cached_compiler_check(&root, compile_dirty, compile_cache)
            .await
            .filter(|e| !e.starts_with("__BUILD_UNVERIFIED__"))
        {
            dbg_log!("Inline compiler check detected errors after edit");
            if let Some(result) = results
                .iter_mut()
                .find(|result| is_mutating_tool(&result.tool_name))
            {
                append_compiler_diagnostics(result, &compiler_errors);
            }
        }
    }
    for (result, call) in results.iter_mut().zip(tool_calls) {
        let notice = (result.tool_name == "use_skill")
            .then_some(deferred_notice.as_deref())
            .flatten();
        let finalized = finalize_tool_result(result.clone(), notice);
        *result = finalized;
        if is_read_only_tool(&call.name) {
            let sig = tool_signature(&call.name, &call.arguments);
            if let Some(cached) = state.lock().await.recent_read_outputs.get_mut(&sig) {
                cached.success = result.metadata.success;
                cached.exit_code = result.metadata.exit_code;
                cached.truncated = result.metadata.truncated;
                if result.metadata.full_output_artifact.is_some() {
                    cached.full_output_artifact = result.metadata.full_output_artifact.clone();
                }
            }
        }
    }
    results
}

/// One result message per call that will never run, so no call is left
/// unanswered.
///
/// A structured transcript replays an assistant message together with the calls
/// it made; a call with no matching result is a protocol violation the provider
/// rejects, and — worse — leaves the model free to assume whatever it likes
/// about what happened. Answering with the failure keeps the record honest.
/// What to tell a model whose completion claim wrote nothing.
///
/// The branch that matters is the second one. A model whose edits all failed is
/// often looking at a workspace that already holds the requested state — left
/// from an earlier run — and if the only sanctioned ways out are "make the
/// change" or "it cannot be made", neither fits, so it manufactures a mutation
/// to satisfy the check. In one session the cheapest mutation available was
/// deleting the very line it had been asked to add, which it then reported as
/// having added and removed.
fn completion_block_message(failed: usize) -> String {
    format!(
        "[Finish blocked — {failed} edit(s) were attempted in this task and every one failed, so this task \
wrote nothing. Exactly one of these is true; establish which from a fresh read, then act.\n\
 1. The change still needs making — make it, verify it, then finish.\n\
 2. The workspace is already in the requested state, possibly from before this task began — say exactly \
that and finish. This is a valid outcome and requires no edit.\n\
 3. The change cannot be made — finish by stating why.\n\
Do NOT edit something else, delete existing content, or reverse the request in order to clear this check. \
An edit that moves the workspace further from what was asked is worse than writing nothing.]"
    )
}

/// Whether a `system` history entry is something the model must actually read.
///
/// Everything the harness tells the model — loop warnings, rejected tool calls,
/// blocked completions, compaction summaries — is written into history as a
/// system message. Those were filtered out of the request, so the harness spent
/// entire sessions correcting a model that never received a word of it: one
/// session issued 25 loop warnings while the model repeated the same read 25
/// times, having been told nothing.
///
/// Session chatter that only makes sense in the TUI (command output, model
/// switches) stays out: it is noise in the prompt and was never meant for the
/// model. Harness notes are bracketed; compaction summaries carry their marker.
fn is_model_directed_note(message: &ChatMessage) -> bool {
    message.role == "system"
        && (message.content.starts_with('[')
            || message
                .content
                .starts_with(crate::network::compaction::SUMMARY_MARKER))
}

/// Largest read output kept for replay to an identical repeat call. Small reads
/// are cheaper to repeat than to argue about; large ones stay behind a notice so
/// a loop cannot re-send a whole file every turn.
const REPLAYABLE_READ_LIMIT: usize = 4096;

/// How many times the completion gate argues before letting a claim through.
const MAX_COMPLETION_BLOCKS: u8 = 2;
const MAX_VERIFICATION_BLOCKS: u8 = 2;

/// Whether a `complete_task` claim describes work that never reached disk.
///
/// True when every mutating call in the task failed: the workspace is untouched,
/// yet the model is reporting the job done — usually because it read the file,
/// found the state it wanted already there for some unrelated reason, and took
/// that as proof of its own edit. Capped so the gate cannot argue forever with a
/// model that insists.
fn completion_claims_unapplied_work(made_edits: bool, failed: usize, blocks: u8) -> bool {
    !made_edits && failed > 0 && blocks < MAX_COMPLETION_BLOCKS
}

/// Pair calls with the ids the provider assigned them, by position. Yields
/// nothing under the text protocols, where calls are prose without identity.
fn call_refs_for(calls: &[crate::tools::ToolCall], ids: &[String]) -> Vec<crate::app::ToolCallRef> {
    calls
        .iter()
        .zip(ids.iter())
        .map(|(call, id)| crate::app::ToolCallRef {
            id: id.clone(),
            name: call.name.clone(),
            arguments: call.arguments.to_string(),
        })
        .collect()
}

fn unanswered_call_results(calls: &[crate::app::ToolCallRef], reason: &str) -> Vec<ChatMessage> {
    calls
        .iter()
        .map(|call| {
            ChatMessage::new("tool", format!("{}: error: {reason}", call.name))
                .answering(Some(call.id.clone()))
        })
        .collect()
}

/// Replacement transcript text for a response whose tool batch was truncated.
/// Records only which tools survived, never the model's prose: a response that
/// plans a whole session ahead also narrates results for calls that never ran,
/// and replaying that text lets the next turn treat its own fiction as
/// observed fact.
fn truncated_batch_summary(kept: &[crate::tools::ToolCall], dropped: usize) -> String {
    let names = kept
        .iter()
        .map(|call| call.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "[Oversized response: only the first {} tool calls were kept ({names}); {dropped} more were dropped. Anything the response claimed about their results was imagined — continue from the real results below.]",
        kept.len()
    )
}

/// Counts ```` ```tool ```` fences across streamed chunks so a runaway response
/// can be cut mid-flight. Chunk boundaries split markers, so the tail of each
/// chunk is carried into the next comparison.
#[derive(Default)]
struct ToolFenceCounter {
    seen: usize,
    tail: String,
}

impl ToolFenceCounter {
    const MARKER: &'static str = "```tool";

    /// Feeds one streamed chunk and returns the running fence count.
    fn push(&mut self, chunk: &str) -> usize {
        if chunk.is_empty() {
            return self.seen;
        }
        let mut window = std::mem::take(&mut self.tail);
        window.push_str(chunk);
        self.seen += window.matches(Self::MARKER).count();
        // Keep just enough of the tail that a marker straddling the next chunk
        // boundary still matches exactly once.
        let carry = Self::MARKER.len() - 1;
        let mut kept: Vec<char> = window.chars().rev().take(carry).collect();
        kept.reverse();
        self.tail = kept.into_iter().collect();
        self.seen
    }
}

pub struct TurnContext {
    pub tool_rounds: usize,
    pub oversized_batch_rejections: u8,
    pub loop_detector: loop_detect::LoopDetector,
    pub loop_recovery_attempts: u8,
    pub force_final: bool,
    /// A mutating tool actually changed something. Distinct from having *tried*:
    /// a failed edit leaves the workspace untouched, and treating an attempt as
    /// a change lets a task finish on work that never landed.
    pub made_edits: bool,
    /// Mutating calls that failed since the task began.
    pub failed_mutations: usize,
    /// How many times a completion claim has been sent back for having applied
    /// nothing, so the gate cannot argue with the model forever.
    pub completion_blocks: u8,
    pub verification_blocks: u8,
    pub verification: verification::VerificationLedger,
    pub edit_root: Option<std::path::PathBuf>,
    pub compile_dirty: bool,
    pub compile_cache: Option<(std::path::PathBuf, Option<String>)>,
    pub finish_gate_retries: u32,
    pub turn_machine: events::TurnMachine,
    pub last_sent_messages: Vec<serde_json::Value>,
    pub final_content: String,
    /// Provider-assigned ids for the tool calls in the response just streamed,
    /// in parse order. Empty under the text protocols.
    pub streamed_call_ids: Vec<String>,
    pub task_completed: bool,
    /// When this turn started, for the wall-clock safety budget.
    pub turn_started_at: std::time::Instant,
    /// Best-effort running total of prompt+completion tokens spent this turn.
    pub tokens_used: u64,
    /// Consecutive mutating-tool results (across rounds) that reported
    /// success but changed nothing — an already-applied edit, a no-op run.
    /// Reset by any mutating tool that actually changes something.
    pub consecutive_no_progress: usize,
    /// Consecutive mutating-tool failures. Reset by any mutating success.
    pub consecutive_failed_mutations: usize,
    /// Consecutive complete_task attempts blocked by the build still not
    /// compiling. Reset whenever the build passes.
    pub consecutive_compiler_error_gates: usize,
    /// Consecutive tool-call blocks the harness could not parse at all
    /// (distinct from a parsed call that executed and failed). Reset the
    /// moment a well-formed batch reaches execution.
    pub consecutive_malformed_calls: usize,
    /// Set once a safety budget stops the turn, so the caller can tell a
    /// budget stop apart from a normal finish or a detected loop.
    pub budget_stopped: Option<String>,
}

impl TurnContext {
    pub fn new() -> Self {
        Self {
            tool_rounds: 0,
            oversized_batch_rejections: 0,
            loop_detector: loop_detect::LoopDetector::new(6),
            loop_recovery_attempts: 0,
            force_final: false,
            made_edits: false,
            failed_mutations: 0,
            completion_blocks: 0,
            verification_blocks: 0,
            verification: verification::VerificationLedger::default(),
            edit_root: None,
            compile_dirty: true,
            compile_cache: None,
            finish_gate_retries: 0,
            turn_machine: events::TurnMachine::new(),
            last_sent_messages: Vec::new(),
            final_content: String::new(),
            streamed_call_ids: Vec::new(),
            task_completed: false,
            turn_started_at: std::time::Instant::now(),
            tokens_used: 0,
            consecutive_no_progress: 0,
            consecutive_failed_mutations: 0,
            consecutive_compiler_error_gates: 0,
            consecutive_malformed_calls: 0,
            budget_stopped: None,
        }
    }
}

pub async fn run_single_turn<P: policy::TurnPolicy + 'static>(
    client: &reqwest::Client,
    state: &Arc<Mutex<AppState>>,
    cancel_token: &tokio_util::sync::CancellationToken,
    policy: &Arc<P>,
    stream_buffer: &Arc<Mutex<StreamBuffer>>,
    ctx: &mut TurnContext,
) -> bool {
    const MAX_FINISH_GATE_RETRIES: u32 = 2;
    dbg_log!("Starting agent loop round {}", ctx.tool_rounds);

    // Cancellation takes priority over the safety budgets: if the user
    // already asked to stop, don't spend a budget-exceeded round explaining
    // why we're also stopping — the existing cancellation handling further
    // down (and in the request layer) owns that path.
    if !cancel_token.is_cancelled()
        && let Some(limit) = turn_budget_exceeded(ctx)
    {
        return stop_turn_for_budget(state, ctx, limit).await;
    }

    // Resolve tool-call support before the prompt is built: the two
    // protocols need different system prompts, so this cannot be
    // discovered from a failure partway through the turn.
    let unprobed = {
        let s = state.lock().await;
        let url = s.api_base_url.clone();
        s.function_calling_unknown(&url)
            .then_some((url, s.model_name.clone()))
    };
    if let Some((url, model)) = unprobed {
        let supported = probe_function_calling(client, state, &url, &model).await;
        let mut s = state.lock().await;
        s.record_function_calling_support(&url, supported);
        dbg_log!(
            "Tool protocol for {}: {:?} (probe said supported={})",
            url,
            s.tool_protocol_for(&url),
            supported
        );
    }

    let msgs = prepare_turn_request(client, state, ctx.tool_rounds).await;

    state.lock().await.current_response.clear();
    stream_buffer.lock().await.reset();

    let (api_base_url, model_name) = {
        let s = state.lock().await;
        (s.api_base_url.clone(), s.model_name.clone())
    };

    dbg_log!(
        "Sending request to {} for model {}",
        api_base_url,
        model_name
    );
    ctx.last_sent_messages = msgs.clone();
    let request_client = client.clone();
    let request_state = Arc::clone(state);
    let request_cancel = cancel_token.clone();
    let request_buffer = Arc::clone(stream_buffer);
    let request_api_url = api_base_url.clone();
    let request_model = model_name.clone();
    let request_msgs = msgs.clone();
    let (accumulated_content, response_finish_reason) =
        match runner::collect_response(move |previous| {
            let mut current_msgs = request_msgs.clone();
            if !previous.is_empty() {
                current_msgs.push(serde_json::json!({
                    "role": "assistant",
                    "content": previous
                }));
                current_msgs.push(serde_json::json!({
                    "role": "user",
                    "content": "continue"
                }));
            }
            let request_client = request_client.clone();
            let request_state = Arc::clone(&request_state);
            let request_cancel = request_cancel.clone();
            let request_buffer = Arc::clone(&request_buffer);
            let request_api_url = request_api_url.clone();
            let request_model = request_model.clone();
            async move {
                request_buffer.lock().await.reset();
                let finish_reason = stream_request(
                    &request_client,
                    request_state.clone(),
                    request_cancel,
                    &request_api_url,
                    &request_model,
                    &current_msgs,
                    Arc::clone(&request_buffer),
                    false,
                )
                .await
                .map_err(|e| e.to_string())?;
                let chunk_content = request_buffer.lock().await.content.clone();
                Ok((chunk_content, finish_reason))
            }
        })
        .await
        {
            Ok(result) => result,
            Err(e) => {
                ctx.turn_machine.recover_error();
                dbg_log!("Stream request failed: {}", e);
                let mut s = state.lock().await;
                let notice = if e == "cancelled" {
                    "Request cancelled by user".to_string()
                } else {
                    format!("Error from LLM Provider: {e}")
                };
                s.history.push(ChatMessage::new("system", notice));
                crate::config::save_history(&s.history);
                s.current_response.clear();
                s.current_token_usage = None;
                s.status = AppStatus::Idle;
                return false;
            }
        };

    crate::logger::operational_event(
        "model.response",
        serde_json::json!({
            "round": ctx.tool_rounds,
            "finish_reason": response_finish_reason,
            "content_bytes": accumulated_content.len(),
        }),
    );
    {
        let mut s = state.lock().await;
        s.current_response = accumulated_content.clone();
        let reported = s
            .current_token_usage
            .as_ref()
            .map(|u| u.total_tokens as u64);
        ctx.tokens_used = accumulate_tokens_used(ctx.tokens_used, reported, &accumulated_content);
    }

    if cancel_token.is_cancelled() {
        ctx.turn_machine.cancel();
        return false;
    }

    ctx.final_content = accumulated_content;
    ctx.streamed_call_ids = stream_buffer.lock().await.tool_call_ids.clone();
    dbg_log!(
        "Stream completed successfully. Content length: {} chars",
        ctx.final_content.len()
    );

    if ctx.final_content.is_empty() {
        dbg_log!("Stream returned empty content, finishing");
        let mut s = state.lock().await;
        s.status = AppStatus::Idle;
        s.current_token_usage = None;
        return false;
    }

    // This is the forced wrap-up turn after a detected loop: tools were
    // disabled via the injected directive. Push whatever prose the model
    // produced and stop — never parse or execute tool calls here, or we'd
    // risk re-entering the loop we just broke out of.
    if ctx.force_final {
        dbg_log!("Loop wrap-up: recording forced text answer and finishing");
        // Promote bare `thought` markers first, then drop the reasoning
        // outright: this is the answer the user reads, and a wrap-up
        // that opens with paragraphs of planning is not an answer.
        let promoted = text::promote_bare_thought_markers(&ctx.final_content);
        let prose = strip_tool_call_syntax(&text::strip_think_blocks(&promoted));
        // Filter out any system prompt leak or empty content
        let clean_prose = prose
            .lines()
            .filter(|line| {
                !line.trim().starts_with("- ")
                    && !line.contains("system directive")
                    && !line.contains("CRITICAL — you are stuck")
            })
            .collect::<Vec<_>>()
            .join("\n");
        let answer = if clean_prose.trim().is_empty() {
            "I encountered a repeating loop while running tool actions and have stopped to prevent unnecessary repetition. I was unable to complete the task automatically. Please check the current changes or re-run with a more specific prompt."
                        .to_string()
        } else {
            clean_prose.trim().to_string()
        };
        let mut s = state.lock().await;
        s.history.push(ChatMessage::new("assistant", &answer));
        crate::config::save_history(&s.history);
        s.current_response.clear();
        s.continuous_mode = false;
        s.status = AppStatus::Idle;
        return false;
    }

    let protocol = { state.lock().await.active_tool_protocol() };
    let model_response = events::normalize_response(
        &ctx.final_content,
        response_finish_reason.as_deref(),
        protocol,
    );
    dbg_log!(
        "Model response normalized from {:?}; raw length={} chars",
        model_response.source,
        model_response.raw_content.len()
    );
    let response_events = model_response.events;
    let parsed_tool_calls = response_events
        .iter()
        .filter_map(|event| match event {
            events::AgentEvent::ToolCall(call) => Some(call.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    // Keep the leading calls and drop the rest rather than rejecting the
    // whole response: real tool output is what pulls a model back to the
    // actual repo state, and a rejection gives it none.
    let requested_calls = parsed_tool_calls.len();
    let (parsed_tool_calls, dropped_calls) = crate::tools::truncate_tool_batch(parsed_tool_calls);
    if dropped_calls > 0 {
        dbg_log!(
            "Oversized batch: running {} of {} requested tool calls",
            parsed_tool_calls.len(),
            requested_calls
        );
        crate::logger::operational_event(
            "tools.batch_truncated",
            serde_json::json!({
                "requested": requested_calls,
                "executed": parsed_tool_calls.len(),
                "dropped": dropped_calls,
            }),
        );
        // The dropped calls came with prose describing results they never
        // produced. Replace the response text so none of that reaches the
        // next turn as if it had been observed.
        ctx.final_content = truncated_batch_summary(&parsed_tool_calls, dropped_calls);
    }
    let oversized_batch = dropped_calls > 0;
    if let Err(reason) = crate::tools::validate_tool_calls(&parsed_tool_calls) {
        dbg_log!("Tool-call validation rejected response: {}", reason);
        let mut s = state.lock().await;
        // `ctx.final_content` was already replaced with a shape-only
        // summary when the batch was truncated, so a rejection here can
        // never replay the fabricated prose that came with it.
        let rejected_refs = call_refs_for(&parsed_tool_calls, &ctx.streamed_call_ids);
        s.history.push(
            ChatMessage::new("assistant", ctx.final_content.clone())
                .with_tool_calls(rejected_refs.clone()),
        );
        // The model learns which call was rejected from the call's own
        // result, not just from a system note it may skim past.
        for message in unanswered_call_results(&rejected_refs, &reason) {
            s.history.push(message);
        }
        let guidance = if oversized_batch {
            ctx.oversized_batch_rejections = ctx.oversized_batch_rejections.saturating_add(1);
            if ctx.oversized_batch_rejections >= 2 {
                ctx.force_final = true;
            }
            format!(
                " This response contained {requested_calls} separate tool calls; the leading {} were kept and the rest dropped, then the remainder failed validation, so nothing ran and nothing it claimed about their results happened. Start again from the last real tool result. Reads may be issued together; keep calls that change the workspace to at most {} per response so each one is grounded in the previous result.",
                parsed_tool_calls.len(),
                crate::tools::MAX_MUTATING_CALLS_PER_RESPONSE
            )
        } else {
            ctx.oversized_batch_rejections = 0;
            String::new()
        };
        s.history.push(ChatMessage::new(
                    "system",
                    format!(
                        "[Tool call rejected before execution: {reason}] Emit one corrected tool call.{guidance}"
                    ),
                ));
        crate::config::save_history(&s.history);
        s.current_response.clear();
        s.status = AppStatus::Streaming;
        drop(s);
        ctx.tool_rounds += 1;
        return true;
    }
    ctx.oversized_batch_rejections = 0;
    let (tool_calls, deferred_tool_calls) =
        crate::tools::isolate_control_plane_call(parsed_tool_calls);
    // Pair each executable call with the id the provider gave it. Both
    // truncation and control-plane isolation keep a prefix of the parsed
    // order, so ids line up by position; the text protocols supply no
    // ids and produce no refs.
    let call_refs = call_refs_for(&tool_calls, &ctx.streamed_call_ids);
    let turn_action = match ctx.turn_machine.model_finished(
        cancel_token.is_cancelled(),
        ctx.force_final,
        !tool_calls.is_empty(),
        ctx.task_completed,
    ) {
        Ok(action) => action,
        Err(invalid) => {
            // An illegal internal transition is a bug, not a user error.
            // Debug builds already asserted inside the machine; in
            // release, log it and finish the turn defensively rather
            // than executing tools from an unexpected state.
            dbg_log!("Turn machine rejected model_finished: {invalid}");
            crate::logger::operational_event(
                "turn.invalid_transition",
                serde_json::json!({
                    "stage": "model_finished",
                    "detail": invalid.to_string(),
                }),
            );
            events::TurnAction::FinishResponse
        }
    };
    if turn_action == events::TurnAction::Cancel {
        return false;
    }
    if matches!(turn_action, events::TurnAction::ExecuteTools) {
        // A well-formed batch reached execution — the parse-failure streak
        // (if any) is over regardless of whether the calls themselves
        // succeed or fail once run.
        ctx.consecutive_malformed_calls = 0;
        dbg_log!("Parsed {} tool call requests", tool_calls.len());

        // Loop detection: feed each requested call to the detector and
        // keep the worst status. Abort stops auto-execution; Warning
        // injects a nudge so the model changes approach.
        let mut loop_status = loop_detect::LoopStatus::Ok;
        for call in &tool_calls {
            let (exact, category) = loop_detect::signatures(&call.name, &call.arguments);
            let s = ctx.loop_detector.check_tool(&call.name, &exact, &category);
            if s.rank() > loop_status.rank() {
                loop_status = s;
            }
            // Remember where code is being touched, so the finish gate can
            // compile-check before accepting a "done". Whether anything
            // actually changed is decided from the results, not from the
            // attempt.
            if is_mutating_tool(&call.name) {
                ctx.edit_root = Some(get_tool_project_root(&call.name, &call.arguments));
                // A mutating tool will run this round — invalidate the
                // cached compiler result so the next check recompiles.
                ctx.compile_dirty = true;
            }
        }
        match loop_status {
            loop_detect::LoopStatus::Abort(n) => {
                match loop_recovery_action(ctx.loop_recovery_attempts) {
                    LoopRecoveryAction::Recover => {
                        ctx.loop_recovery_attempts += 1;
                        ctx.loop_detector.reset();
                        dbg_log!(
                            "Loop detector: abort after {} repeats — allowing bounded recovery turn",
                            n
                        );
                        let mut s = state.lock().await;
                        s.history
                            .push(ChatMessage::new("assistant", &ctx.final_content));
                        s.history
                            .push(ChatMessage::new("system", LOOP_RECOVERY_PROMPT));
                        crate::config::save_history(&s.history);
                        s.current_response.clear();
                        s.status = AppStatus::Streaming;
                        s.stream_tracker = Some(StreamTracker::new());
                        drop(s);
                        ctx.turn_machine.finish_tools_if_executing();
                        ctx.tool_rounds += 1;
                        return true;
                    }
                    LoopRecoveryAction::ForceFinal => {
                        dbg_log!(
                            "Loop detector: abort after {} repeats — forcing wrap-up turn",
                            n
                        );
                        // Don't stop silently. Record the looping turn, then inject
                        // a directive that disables tools and demands a prose
                        // summary, and run exactly one more turn (`ctx.force_final`).
                        let mut s = state.lock().await;
                        s.history
                            .push(ChatMessage::new("assistant", &ctx.final_content));
                        s.history
                            .push(ChatMessage::new("system", FORCE_ANSWER_PROMPT));
                        crate::config::save_history(&s.history);
                        s.current_response.clear();
                        drop(s);
                        ctx.force_final = true;
                        return true;
                    }
                }
            }
            loop_detect::LoopStatus::Warning(n) => {
                dbg_log!("Loop detector: warning at {} repeats", n);
                let mut s = state.lock().await;
                s.history.push(ChatMessage::new(
                            "system",
                            format!(
                                "[Loop warning: this action has repeated {n} times. If a tool edit or view is failing, stop retrying the same inputs — call view_file to check exact line numbers or change your approach.]"
                            ),
                        ));
                drop(s);
            }
            loop_detect::LoopStatus::Ok => {}
        }

        if !cancel_token.is_cancelled() {
            ctx.tool_rounds += 1;

            let approved = policy.should_approve(state, &tool_calls).await;

            // Update UI state immediately after confirmation is resolved
            {
                let mut s = state.lock().await;
                s.pending_tool_confirmation = None;
                s.status = AppStatus::Streaming;
                s.stream_tracker = Some(StreamTracker::new());
                s.history.push(
                    ChatMessage::new("assistant", &ctx.final_content)
                        .with_tool_calls(call_refs.clone()),
                );
                if dropped_calls > 0 {
                    s.history.push(ChatMessage::new(
                                "system",
                                format!(
                                    "[{dropped_calls} of the {requested_calls} tool calls in that response were dropped; only the first {} ran. Their results follow — plan the next step from those, not from what the response predicted. Reads may be issued together; keep calls that change the workspace to at most {} per response.]",
                                    tool_calls.len(),
                                    crate::tools::MAX_MUTATING_CALLS_PER_RESPONSE
                                ),
                            ));
                }
                crate::config::save_history(&s.history);
            }

            // Approval must be resolved on the machine BEFORE any tool
            // runs: execution is gated on the machine reaching
            // ExecutingTools, so a denial leaves it in AwaitingModel and
            // nothing executes.
            let transition = if approved {
                ctx.turn_machine.approval_granted()
            } else {
                ctx.turn_machine.approval_denied()
            };
            if let Err(invalid) = transition {
                dbg_log!("Turn machine rejected approval transition: {invalid}");
                crate::logger::operational_event(
                    "turn.invalid_transition",
                    serde_json::json!({
                        "stage": "approval",
                        "approved": approved,
                        "detail": invalid.to_string(),
                    }),
                );
            }

            let deferred_notice = (deferred_tool_calls > 0).then(|| {
                format!(
                    "[harness: deferred {deferred_tool_calls} additional tool call(s) until the next model turn after skill loading]"
                )
            });
            let results = execute_tool_batch(
                client,
                state,
                cancel_token,
                &tool_calls,
                ctx.turn_machine.state() == events::TurnState::ExecutingTools,
                &ctx.edit_root,
                &mut ctx.compile_dirty,
                &mut ctx.compile_cache,
                deferred_notice,
            )
            .await;

            crate::logger::operational_event(
                "tools.batch.finish",
                serde_json::json!({
                    "count": results.len(),
                    "successes": results.iter().filter(|result| result.metadata.success).count(),
                    "failed": results.iter().filter(|result| !result.metadata.success).count(),
                    "changed_paths": results.iter().map(|result| result.metadata.changed_paths.len()).sum::<usize>(),
                }),
            );

            if cancel_token.is_cancelled() {
                dbg_log!("Orchestrator: Cancelled during tool execution");
                let mut s = state.lock().await;
                // The assistant message announcing these calls is already
                // in history; leaving them unanswered would break the next
                // request and strand the model without knowing they were
                // interrupted.
                for message in unanswered_call_results(&call_refs, "interrupted by the user") {
                    s.history.push(message);
                }
                crate::config::save_history(&s.history);
                s.status = AppStatus::Idle;
                ctx.turn_machine.finish_tools_if_executing();
                return false;
            }

            let mut s = state.lock().await;
            s.status = AppStatus::Streaming;
            let mut completed = false;
            let executed = results.len();
            for (position, result) in results.into_iter().enumerate() {
                let call = tool_calls.get(position);
                let answered_call = call_refs.get(position).map(|call| call.id.clone());
                let name = result.tool_name;
                let metadata = result.metadata.clone();
                let content = result.content;
                if call.is_some_and(|call| call.name == "run_command")
                    && let Some(command) = call
                        .and_then(|call| call.arguments.get("command"))
                        .and_then(|command| command.as_str())
                {
                    ctx.verification
                        .record_command(command, metadata.exit_code);
                }
                let diff_opt = result.diff;
                dbg_log!(
                    "Tool '{}' finished with result length: {} chars",
                    name,
                    content.len()
                );
                if name == "complete_task" {
                    completed = true;
                }
                // An edit counts once the tool reports it applied. A tool
                // that returned an error changed nothing, however much the
                // model's prose says otherwise.
                if is_mutating_tool(&name) {
                    let failed = !metadata.success
                        || content
                            .trim_start()
                            .to_ascii_lowercase()
                            .starts_with("error");
                    let made_progress = mutation_made_progress(metadata.success, &content);
                    if failed {
                        ctx.failed_mutations += 1;
                        ctx.consecutive_failed_mutations += 1;
                    } else {
                        ctx.made_edits = true;
                        ctx.consecutive_failed_mutations = 0;
                        if !metadata.changed_paths.is_empty() || name != "run_command" {
                            ctx.verification.record_edit();
                        }
                    }
                    // A tool that reports success without changing anything
                    // (an edit that was already applied, a no-op run) is not
                    // progress — it must keep the no-progress budget
                    // climbing exactly like a failed edit would, or a
                    // duplicate-stacking edit that always reports success
                    // could spin forever without ever tripping a budget.
                    if made_progress {
                        ctx.consecutive_no_progress = 0;
                    } else {
                        ctx.consecutive_no_progress += 1;
                    }
                    // Progress resets the loop detector: a successful,
                    // change-making mutating tool means the agent moved the
                    // work forward, so any re-reads that follow (to verify
                    // or find the next anchor) shouldn't inherit the
                    // pre-edit read history and trip the frequency signal.
                    // Failed edits and no-op successes (e.g. an
                    // already-applied edit) are not progress and must keep
                    // accumulating toward the detector's own abort
                    // threshold instead of getting a free reset every round.
                    if made_progress {
                        ctx.loop_detector.reset();
                    }
                }
                // Output-stagnation signal: repeated identical results
                // (e.g. "No matches found") despite varied commands.
                if let loop_detect::LoopStatus::Warning(n) | loop_detect::LoopStatus::Abort(n) =
                    ctx.loop_detector.record_output(&content)
                {
                    dbg_log!("Loop detector: output stagnation x{} for '{}'", n, name);
                }
                s.history.push(tool_result_history_message(
                    ToolResult {
                        tool_name: name,
                        content,
                        diff: diff_opt,
                        file_preview: result.file_preview,
                        metadata,
                    },
                    answered_call,
                ));
            }
            // Safety net: an executor that returns fewer results than
            // calls would leave ids unanswered in the replayed transcript.
            if executed < call_refs.len() {
                for message in
                    unanswered_call_results(&call_refs[executed..], "no result was produced")
                {
                    s.history.push(message);
                }
            }

            // Completion gate: every edit this task attempted failed, so
            // nothing reached disk. A model in that position tends to read
            // the file, find the state it wanted already there for some
            // other reason, and report the work as done — which is how a
            // task finishes with the workspace untouched.
            if completed
                && completion_claims_unapplied_work(
                    ctx.made_edits,
                    ctx.failed_mutations,
                    ctx.completion_blocks,
                )
            {
                ctx.completion_blocks += 1;
                dbg_log!(
                    "Completion blocked: {} failed edits, none applied",
                    ctx.failed_mutations
                );
                crate::logger::operational_event(
                    "turn.completion_blocked",
                    serde_json::json!({ "failed_mutations": ctx.failed_mutations }),
                );
                s.history.push(ChatMessage::new(
                    "system",
                    completion_block_message(ctx.failed_mutations),
                ));
                crate::config::save_history(&s.history);
                s.current_response.clear();
                drop(s);
                ctx.turn_machine.finish_tools_if_executing();
                return true;
            }

            if completed {
                if ctx.made_edits
                    && !ctx.verification.has_fresh_successful_verification()
                    && ctx.verification_blocks < MAX_VERIFICATION_BLOCKS
                {
                    ctx.verification_blocks += 1;
                    let reason = ctx
                        .verification
                        .last_failure()
                        .map(|evidence| {
                            format!(
                                "The latest verification failed: {} (exit code {:?}).",
                                evidence.command, evidence.exit_code
                            )
                        })
                        .unwrap_or_else(|| {
                            "No verification command was run after the latest edit.".to_string()
                        });
                    s.history.push(ChatMessage::new(
                        "system",
                        format!(
                            "[Finish blocked — {reason} Run the relevant project verification command after the latest edit, inspect its result, then report completion.]"
                        ),
                    ));
                    crate::config::save_history(&s.history);
                    s.current_response.clear();
                    drop(s);
                    ctx.turn_machine.finish_tools_if_executing();
                    return true;
                }
                let mut build_status = if ctx.made_edits {
                    "pending"
                } else {
                    "not run (no workspace edits detected)"
                };
                // Finish gate check: verify the project builds cleanly before accepting completion
                if ctx.made_edits {
                    let root = ctx
                        .edit_root
                        .clone()
                        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
                    if let Some(errors) =
                        cached_compiler_check(&root, &mut ctx.compile_dirty, &mut ctx.compile_cache)
                            .await
                    {
                        if errors.starts_with("__BUILD_UNVERIFIED__") {
                            // The checker itself couldn't run (missing toolchain,
                            // timeout, …). We can't prove the build is red, so we
                            // don't loop forever — but we must NOT let the agent
                            // report a clean completion, so surface it loudly.
                            dbg_log!("complete_task finish gate: build unverified — {errors}");
                            build_status = "unverified";
                            s.history.push(ChatMessage::new(
                                "system",
                                format!("[⚠ Build could not be verified — {errors}]"),
                            ));
                        } else {
                            dbg_log!("complete_task finish gate failed with compiler errors");
                            ctx.consecutive_compiler_error_gates += 1;
                            s.history.push(ChatMessage::new(
                                        "system",
                                        format!(
                                            "[Finish blocked — the build does not compile. You cannot report this \
                                             task as done while there are compiler errors. Fix them, then finish. \
                                             Compiler errors:\n{errors}]"
                                        ),
                                    ));
                            crate::config::save_history(&s.history);
                            s.current_response.clear();
                            drop(s);
                            ctx.turn_machine.finish_tools_if_executing();
                            return true;
                        }
                    } else {
                        build_status = "passed";
                        ctx.consecutive_compiler_error_gates = 0;
                    }
                }

                dbg_log!(
                    "complete_task called, turning off continuous mode and breaking loop immediately"
                );
                s.continuous_mode = false;
                s.status = AppStatus::Idle;
                // Extract task result text from the complete_task call
                let task_result_summary = tool_calls
                    .iter()
                    .find(|call| call.name == "complete_task")
                    .and_then(|call| call.arguments.get("result").and_then(|r| r.as_str()))
                    .map(|s| s.to_string());

                if let Some(mut summary_text) = task_result_summary
                    && !summary_text.is_empty()
                {
                    let mut changed_paths = std::collections::BTreeSet::new();
                    for message in &s.history {
                        if let Some(metadata) = &message.tool_result {
                            changed_paths.extend(metadata.changed_paths.iter().cloned());
                        }
                    }
                    let paths = if changed_paths.is_empty() {
                        "none recorded".to_string()
                    } else {
                        changed_paths.into_iter().collect::<Vec<_>>().join(", ")
                    };
                    summary_text.push_str(&format!(
                        "\n\n[harness verification: build={build_status}; tool_verification={}; changed_paths={paths}]",
                        ctx.verification.summary()
                    ));
                    // The gate gives up arguing after two rounds. If it
                    // does, the reader still needs to know the summary
                    // describes work that never landed.
                    if !ctx.made_edits && ctx.failed_mutations > 0 {
                        summary_text.push_str(&format!(
                            "\n[harness warning: {} edit(s) failed and none were applied — \
nothing in this summary was written to disk by this task]",
                            ctx.failed_mutations
                        ));
                    }
                    s.history.push(ChatMessage::new("assistant", summary_text));
                }
                crate::config::save_history(&s.history);
                s.current_response.clear();
                drop(s);
                ctx.task_completed = true;
                ctx.turn_machine.finish_tools_if_executing();
                return false;
            }
            crate::config::save_history(&s.history);
            s.current_response.clear();
            drop(s);
            ctx.turn_machine.finish_tools_if_executing();
            dbg_log!("Tool round finished, looping back");
            return true;
        } else {
            dbg_log!("Tool execution cancelled");
            ctx.turn_machine.finish_tools_if_executing();
            return false;
        }
    } else if has_intended_tool_call(&ctx.final_content) {
        dbg_log!("Orchestrator: Detected malformed tool call, auto-correcting and retrying...");
        ctx.tool_rounds += 1;
        ctx.consecutive_malformed_calls += 1;
        let mut s = state.lock().await;
        s.history
            .push(ChatMessage::new("assistant", &ctx.final_content));

        let reason = crate::tools::diagnose_failed_tool_call(&ctx.final_content)
            .map(|r| format!("{r}\n\n"))
            .unwrap_or_default();
        let feedback = format!(
            "tool_error: The tool call block was malformed or could not be parsed. {reason}\
Please output a single, complete, valid tool call block inside a ```tool fenced block using JSON format:\n\n\
```tool\n\
{{\"name\": \"tool_name\", \"arguments\": {{...}}}}\n\
```\n\n\
Make sure keys are exactly \"name\" and \"arguments\", and do not wrap numbers/booleans in quotes if they are expected as numbers/booleans."
        );

        s.history.push(ChatMessage::new("tool", feedback));
        crate::config::save_history(&s.history);
        s.current_response.clear();
        s.status = AppStatus::Streaming;
        s.stream_tracker = Some(StreamTracker::new());
        drop(s);
        dbg_log!("Retrying agent loop round due to malformed tool call");
        return true;
    }

    let is_continuous = { state.lock().await.continuous_mode };
    if is_continuous && ctx.tool_rounds > 0 {
        dbg_log!(
            "Continuous mode active, assistant responded with text prose. Ending continuous mode turn."
        );
        let mut s = state.lock().await;
        s.continuous_mode = false;
    } else if is_continuous && ctx.tool_rounds == 0 {
        dbg_log!(
            "Continuous mode active, but assistant gave a plain conversational reply (no tools used). Ending turn."
        );
        let mut s = state.lock().await;
        s.continuous_mode = false;
    }

    // Finish gate: the model wants to stop with a prose answer. If it
    // edited code this task, don't accept "done" on a red build — run a
    // compile check and, if it fails, hand the errors back and force
    // another round. Skip on the forced wrap-up turn (tools already
    // disabled) and once the retry budget is spent, so we can't spin.
    if policy.should_verify_completion()
        && ctx.made_edits
        && !ctx.force_final
        && ctx.finish_gate_retries < MAX_FINISH_GATE_RETRIES
    {
        let root = ctx
            .edit_root
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        dbg_log!(
            "Finish gate: compile-checking {} before accepting done",
            root.display()
        );
        if let Some(errors) =
            cached_compiler_check(&root, &mut ctx.compile_dirty, &mut ctx.compile_cache).await
        {
            if errors.starts_with("__BUILD_UNVERIFIED__") {
                // Checker couldn't run — surface it but accept done rather
                // than spinning against an environment we can't fix.
                dbg_log!("Finish gate: build unverified — {errors}");
                let mut s = state.lock().await;
                s.history.push(ChatMessage::new(
                    "system",
                    format!("[⚠ Build could not be verified — {errors}]"),
                ));
                crate::config::save_history(&s.history);
                drop(s);
            } else {
                ctx.finish_gate_retries += 1;
                ctx.tool_rounds += 1;
                dbg_log!(
                    "Finish gate: build is RED, forcing a fix round ({}/{})",
                    ctx.finish_gate_retries,
                    MAX_FINISH_GATE_RETRIES
                );
                let mut s = state.lock().await;
                s.history
                    .push(ChatMessage::new("assistant", ctx.final_content.clone()));
                s.history.push(ChatMessage::new(
                            "system",
                            format!(
                                "[Finish blocked — the build does not compile. You cannot report this \
                                 task as done while there are compiler errors. Fix them, then finish. \
                                 Compiler errors:\n{errors}]"
                            ),
                        ));
                crate::config::save_history(&s.history);
                s.current_response.clear();
                s.status = AppStatus::Streaming;
                s.stream_tracker = Some(StreamTracker::new());
                drop(s);
                if let Err(invalid) = ctx.turn_machine.retry_for_finish_gate() {
                    dbg_log!("Turn machine rejected finish-gate retry: {invalid}");
                    return false;
                }
                return true;
            }
        }
        dbg_log!("Finish gate: build is green, accepting done");
    }

    false
}

/// Drive one already-recorded prompt through the shared agent loop and finalize
/// it. This is the single turn-lifecycle implementation: it runs
/// [`run_single_turn`] until the turn stops (so it shares the `TurnMachine`,
/// approval/safety policy, retry behavior, compiler/build verification, and the
/// `complete_task` finish gate), then records the assistant reply, resolves and
/// tracks token usage, persists history/session, and fires completion
/// side-effects.
///
/// Both the interactive queue orchestrator and the headless raw CLI call this,
/// so `--prompt` execution and interactive execution can no longer diverge. It
/// is non-interactive by construction: interactivity lives entirely in the
/// injected [`policy::TurnPolicy`], which the raw CLI supplies as a headless,
/// non-blocking implementation. Returns the finished [`TurnContext`] for the
/// caller to inspect (final content, whether the task completed, tool rounds).
pub async fn run_agent_turn<P: policy::TurnPolicy + 'static>(
    client: &reqwest::Client,
    state: &Arc<Mutex<AppState>>,
    cancel_token: &tokio_util::sync::CancellationToken,
    policy: &Arc<P>,
    stream_buffer: &Arc<Mutex<StreamBuffer>>,
) -> TurnContext {
    let prompt_start_time = std::time::Instant::now();

    let mut ctx = TurnContext::new();
    while run_single_turn(client, state, cancel_token, policy, stream_buffer, &mut ctx).await {}

    if !ctx.final_content.is_empty() {
        dbg_log!("Finishing agent loop, writing final assistant reply");
        crate::logger::operational_event(
            "turn.finish",
            serde_json::json!({
                "completed_task": ctx.task_completed,
                "tool_rounds": ctx.tool_rounds,
                "content_bytes": ctx.final_content.len(),
                "cancelled": cancel_token.is_cancelled(),
            }),
        );

        let mut s = state.lock().await;
        s.continuous_mode = false;
        s.response_time = Some(prompt_start_time.elapsed());
        // On the complete_task path the summary was already appended; only
        // record token usage / notify below, don't duplicate the reply.
        if !ctx.task_completed {
            let mut msg = ChatMessage::new("assistant", ctx.final_content.clone());
            msg.response_time_ms = s.response_time.map(|d| d.as_millis() as u64);
            s.history.push(msg);
        }

        drop(s);

        let usage = {
            let s = state.lock().await;
            if s.current_token_usage.is_some() {
                s.current_token_usage.clone()
            } else {
                drop(s);
                dbg_log!("Estimating token usage...");
                let est = estimate_token_usage(&ctx.last_sent_messages, &ctx.final_content).await;
                dbg_log!("Token usage estimation result: {:?}", est);
                est
            }
        };

        let mut s = state.lock().await;
        if let Some(msg) = s.history.iter_mut().rev().find(|m| m.role == "assistant") {
            msg.token_usage = usage.clone();
        }

        let active_id = s.active_session_id.clone();
        crate::config::save_session_history(&active_id, &s.history);
        // Turn end: force the queued snapshot to disk, on a blocking thread so
        // the runtime keeps serving the UI.
        crate::config::flush_history_async();

        s.current_response.clear();
        s.status = AppStatus::Idle;

        if let Some(u) = &usage {
            crate::config::track_usage(u.prompt_tokens as u64, u.completion_tokens as u64);
        }
        s.current_token_usage = usage;
        drop(s);

        // Fetch live Gemini model quota from proxy endpoint
        let state_quota = Arc::clone(state);
        let client_quota = client.clone();
        tokio::spawn(async move {
            fetch_model_quota(&client_quota, &state_quota).await;
        });

        // Notify the user that the agent loop completed successfully.
        let _ =
            crate::notifications::notify_finished(crate::notifications::FinishedStatus::Success);
    }

    ctx
}

pub async fn process_queue_orchestrator<P: policy::TurnPolicy + 'static>(
    client: reqwest::Client,
    state: Arc<Mutex<AppState>>,
    cancel_token: tokio_util::sync::CancellationToken,
    policy: Arc<P>,
) {
    dbg_log!("Orchestrator started");
    loop {
        let next_prompt = {
            let mut s = state.lock().await;
            if s.pending_queue.is_empty() {
                dbg_log!("Pending queue empty, setting status to Idle");
                s.status = AppStatus::Idle;
                s.delegation_active = false;
                // Clear the single-flight guard under the same lock that saw the
                // queue empty, so an enqueue racing this exit either lands before
                // (queue non-empty here → we keep going) or after (guard clear →
                // the enqueuer spawns a fresh orchestrator). No lost wakeups, no
                // second concurrent orchestrator.
                s.orchestrator_running = false;
                if s.config.discord_rpc_enabled {
                    let model_name = s.model_name.clone();
                    s.discord_rpc
                        .set_activity("Idle", &format!("Using model: {}", model_name));
                }
                break;
            }
            let model_name = s.model_name.clone();
            s.status = AppStatus::Streaming;
            if s.config.discord_rpc_enabled {
                s.discord_rpc
                    .set_activity("Thinking", &format!("Using model: {}", model_name));
            }
            s.generation_start_time = Some(std::time::Instant::now());
            s.stream_tracker = Some(StreamTracker::new());
            s.recent_read_calls.clear();
            s.recent_read_outputs.clear();
            s.read_file_mtimes.clear();
            let prompt = s.pending_queue.remove(0);
            dbg_log!("Popped prompt from queue: '{}'", prompt);
            prompt
        };

        let stream_buffer = Arc::new(Mutex::new(StreamBuffer::new()));
        let is_wakeup = next_prompt.starts_with("__task_wakeup__:");

        let mut is_first_prompt = false;
        if !is_wakeup {
            let s = state.lock().await;
            is_first_prompt = s.history.is_empty();
        }

        record_prompt_to_history(&state, is_wakeup, &next_prompt).await;
        crate::logger::operational_event("turn.start", serde_json::json!({"wakeup": is_wakeup}));

        if is_first_prompt {
            spawn_title_generation(&client, &state, next_prompt.clone()).await;
        }

        run_agent_turn(&client, &state, &cancel_token, &policy, &stream_buffer).await;

        if cancel_token.is_cancelled() {
            dbg_log!("Cancel token is cancelled, exiting orchestrator loop");
            // Best-effort: notify the user that a cancellation happened.
            let _ = crate::notifications::notify_finished(
                crate::notifications::FinishedStatus::Cancelled,
            );
            break;
        }
    }
    // Safety net: every loop exit that isn't the queue-empty branch (stream
    // error, cancel, empty content) lands here — always release the guard so a
    // future turn can start.
    state.lock().await.orchestrator_running = false;
    dbg_log!("Orchestrator finished");
}

pub async fn fetch_model_quota(client: &reqwest::Client, state: &Arc<Mutex<AppState>>) {
    let (url, model_name, api_key_opt) = {
        let s = state.lock().await;
        let active_url = s.api_base_url.clone();
        let key = s
            .config
            .models
            .iter()
            .find(|m| m.url == active_url || m.model == s.model_name)
            .and_then(|m| m.api_key.clone());
        (active_url, s.model_name.clone(), key)
    };

    if !url.contains("localhost:3000")
        && !url.contains("127.0.0.1:3000")
        && !url.contains("127.0.0.1:10531")
        && !url.contains("localhost:10531")
    {
        return;
    }

    // Construct proxy base URL (remove /v1/chat/completions or trailing slashes)
    let base_url = if let Some(idx) = url.find("/v1") {
        &url[..idx]
    } else {
        url.trim_end_matches('/')
    };
    let status_url = format!("{}/auth/status", base_url);

    let mut req = client.get(&status_url);
    if let Some(key) = api_key_opt {
        req = req.header("Authorization", format!("Bearer {key}"));
    }

    let Ok(res) = req.send().await else {
        return;
    };
    let Ok(json) = res.json::<serde_json::Value>().await else {
        return;
    };

    let quota_obj = json.get("quota");
    let buckets_arr = quota_obj
        .and_then(|q| q.get("buckets").or_else(|| q.get("quotaBuckets")))
        .and_then(|b| b.as_array());

    if let Some(quota_buckets) = buckets_arr {
        let mut matched_pct = None;
        for bucket in quota_buckets {
            if let Some(model_id) = bucket.get("modelId").and_then(|m| m.as_str())
                && let Some(fraction) = bucket.get("remainingFraction").and_then(|f| f.as_f64())
            {
                let pct = (fraction * 100.0) as f32;
                if matched_pct.is_none() {
                    matched_pct = Some(pct);
                }
                if model_id == model_name
                    || model_name.contains(model_id)
                    || model_id.contains(&model_name)
                {
                    matched_pct = Some(pct);
                    break;
                }
            }
        }
        if let Some(pct) = matched_pct {
            let mut s = state.lock().await;
            s.model_quota_remaining = Some(pct);
            s.request_redraw();
        }
        return;
    }

    // The ChatGPT/Codex usage response reports account-wide rate limits rather
    // than per-model Gemini-style buckets. Use the primary window for the
    // footer quota indicator; /status and /quota display both windows.
    let primary_window = json
        .get("rate_limits")
        .and_then(|r| r.get("primary"))
        .or_else(|| json.get("rate_limit").and_then(|r| r.get("primary_window")));
    if let Some(used_percent) = primary_window
        .and_then(|p| p.get("used_percent"))
        .and_then(|v| v.as_f64())
    {
        let mut s = state.lock().await;
        s.model_quota_remaining = Some((100.0 - used_percent).clamp(0.0, 100.0) as f32);
        s.request_redraw();
    }
}

pub fn parse_multimodal_content(text: &str) -> serde_json::Value {
    if !text.contains("![image](file://") {
        return serde_json::Value::String(text.to_string());
    }

    let mut parts: Vec<serde_json::Value> = Vec::new();
    let mut remaining = text;

    while let Some(start_idx) = remaining.find("![image](file://") {
        let text_part = &remaining[..start_idx];
        if !text_part.is_empty() {
            parts.push(serde_json::json!({
                "type": "text",
                "text": text_part.to_string(),
            }));
        }

        let path_start = start_idx + "![image](file://".len();
        let rest = &remaining[path_start..];
        if let Some(end_idx) = rest.find(')') {
            let path_str = &rest[..end_idx];
            if let Ok(bytes) = std::fs::read(path_str) {
                use base64::{Engine as _, engine::general_purpose};
                let base64_str = general_purpose::STANDARD.encode(bytes);
                let mime = if path_str.ends_with(".jpg") || path_str.ends_with(".jpeg") {
                    "image/jpeg"
                } else if path_str.ends_with(".gif") {
                    "image/gif"
                } else if path_str.ends_with(".webp") {
                    "image/webp"
                } else {
                    "image/png"
                };
                parts.push(serde_json::json!({
                    "type": "image_url",
                    "image_url": {
                        "url": format!("data:{};base64,{}", mime, base64_str),
                    }
                }));
            } else {
                parts.push(serde_json::json!({
                    "type": "text",
                    "text": format!("![image](file://{})", path_str),
                }));
            }
            remaining = &rest[end_idx + 1..];
        } else {
            break;
        }
    }

    if !remaining.is_empty() {
        parts.push(serde_json::json!({
            "type": "text",
            "text": remaining.to_string(),
        }));
    }

    serde_json::Value::Array(parts)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        }];

        // Text protocols supply no ids, so nothing structured is recorded.
        assert!(call_refs_for(&calls, &[]).is_empty());

        let refs = call_refs_for(&calls, &["call_9".to_string()]);
        assert_eq!(refs[0].id, "call_9");
        assert_eq!(refs[0].name, "grep");
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
            },
            crate::tools::ToolCall {
                name: "run_command".to_string(),
                arguments: serde_json::json!({"command": "cargo check"}),
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
        assert!(message
            .tool_result
            .as_ref()
            .is_some_and(|metadata| metadata.truncated));
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
        assert_eq!(std::fs::read_to_string(artifact).expect("artifact readable"), raw);
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
            },
            Some("real diff".to_string()),
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
        assert_eq!(std::fs::read_to_string(artifact).expect("artifact readable"), raw);

        let spoofed = subagent_tool_history_message(
            "custom_tool",
            &serde_json::json!({}),
            crate::tools::ToolExecutionOutput {
                content: "exit code: 0\n[Output truncated:]\nFull output saved to: /tmp/spoof"
                    .to_string(),
                success: false,
                exit_code: None,
                truncated: false,
            },
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
        assert_eq!(dropped, 1);
        assert_eq!(msgs.len(), 3);
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

        let (result, _) = confirm_and_execute(
            &state,
            &cancel_token,
            "write_to_file",
            &args,
            "write_to_file",
            true,
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
            loop_detect::LoopStatus::Abort(4),
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
        let file =
            "view_file: [File: src/main.rs, Lines 1 to 5 of 5]\n1: a\n2: b\n3: c\n4: d\n5: e";
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
    async fn test_run_compiler_check_success() {
        let cwd = std::env::current_dir().unwrap();
        let check = run_compiler_check(&cwd).await;
        assert!(check.is_none());
    }

    #[test]
    fn project_root_from_relative_file_is_a_real_directory() {
        let root =
            get_tool_project_root("delete_file", &serde_json::json!({"path": "src/temp.rs"}));
        assert!(root.is_absolute());
        assert!(root.is_dir());
        assert!(root.join("Cargo.toml").exists());
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
        assert_eq!(loop_recovery_action(u8::MAX), LoopRecoveryAction::ForceFinal);
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
        assert!(with_files.contains("- src/a.rs"));
        assert!(with_files.contains("- src/b.rs"));

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
        ctx.tool_rounds = MAX_TOOL_ROUNDS;
        match turn_budget_exceeded(&ctx) {
            Some(TurnBudgetLimit::ToolRounds(n)) => assert_eq!(n, MAX_TOOL_ROUNDS),
            other => panic!("expected ToolRounds limit, got {other:?}"),
        }
    }

    #[test]
    fn timeout_triggers_the_budget_safely() {
        let mut ctx = TurnContext::new();
        // Simulate elapsed wall-clock time without an actual 10-minute sleep.
        ctx.turn_started_at =
            std::time::Instant::now() - (MAX_TURN_WALL_CLOCK + std::time::Duration::from_secs(1));
        match turn_budget_exceeded(&ctx) {
            Some(TurnBudgetLimit::WallClock(secs)) => {
                assert!(secs >= MAX_TURN_WALL_CLOCK.as_secs())
            }
            other => panic!("expected WallClock limit, got {other:?}"),
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
        for _ in 0..20 {
            ctx.tokens_used = accumulate_tokens_used(ctx.tokens_used, Some(30_000), "");
        }
        assert!(
            ctx.tokens_used >= MAX_TURN_TOKEN_BUDGET,
            "20 rounds of 30k tokens each must exceed the {MAX_TURN_TOKEN_BUDGET} budget"
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
        ctx.tool_rounds = MAX_TOOL_ROUNDS;
        ctx.task_completed = false;

        let limit = turn_budget_exceeded(&ctx).expect("budget should be exceeded");
        let should_continue = stop_turn_for_budget(&state, &mut ctx, limit).await;

        assert!(!should_continue, "a budget stop must end the loop");
        assert!(!ctx.task_completed, "a budget stop must never claim completion");
        assert!(ctx.budget_stopped.is_some(), "the exact limit reached must be recorded");
        assert!(
            ctx.final_content.contains("stopped"),
            "the summary must explain the stop: {}",
            ctx.final_content
        );
        assert!(
            ctx.final_content.to_ascii_lowercase().contains("not complete"),
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
        ctx.tool_rounds = MAX_TOOL_ROUNDS;

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
        let mut results = execute_tool_batch(
            &client,
            state,
            &cancel_token,
            &[call],
            true,
            &None,
            &mut compile_dirty,
            &mut compile_cache,
            None,
        )
        .await;
        results.remove(0)
    }

    async fn run_one_tool(call: crate::tools::ToolCall) -> ToolResult {
        let state = Arc::new(Mutex::new(AppState::new()));
        run_one_tool_with_state(&state, call).await
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
    async fn repeated_failed_read_preserves_structured_failure() {
        let state = Arc::new(Mutex::new(AppState::new()));
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("missing").to_string_lossy().to_string();
        let call = test_tool_call(
            "list_directory",
            serde_json::json!({"path": missing}),
        );

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
        let result = "successfully replaced target_content in 'x.rs'\n\n```diff\n@@ -1,1 +1,1 @@\n-a\n+b\n```\n";
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
}
