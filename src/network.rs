use crate::app::{AppState, AppStatus, ChatMessage, StreamTracker, ToolConfirmation};
use std::sync::Arc;
use tokio::sync::Mutex;

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
    cap_diff_lines, continuation_nudge, has_intended_tool_call, is_cut_off,
    strip_think_blocks, strip_tool_call_syntax,
};

#[path = "network/stream.rs"]
pub(crate) mod stream;
pub(crate) use stream::StreamBuffer;

#[path = "network/stream_request.rs"]
pub(crate) mod stream_request;
pub use stream_request::stream_request;
pub(crate) use stream_request::{
    estimate_token_usage, parse_native_tool_arguments, request_debug_log_line,
    request_log_summary,
};

#[path = "network/output.rs"]
pub(crate) mod output;
pub(crate) use output::truncate_tool_output_for_message;

#[path = "network/events.rs"]
pub(crate) mod events;
pub(crate) use events::{ToolResult, ToolResultMetadata};

#[path = "network/lifecycle.rs"]
pub(crate) mod lifecycle;

#[path = "network/history.rs"]
pub(crate) mod history;

#[path = "network/runner.rs"]
pub(crate) mod runner;

#[path = "network/policy.rs"]
pub(crate) mod policy;

#[path = "network/verification.rs"]
pub(crate) mod verification;

#[path = "network/image_fallback.rs"]
pub(crate) mod image_fallback;

#[path = "network/payload.rs"]
pub(crate) mod payload;
pub use payload::{fetch_model_quota, parse_multimodal_content};

#[path = "network/compiler.rs"]
pub(crate) mod compiler;
pub(crate) use compiler::{
    append_compiler_diagnostics, cached_compiler_check, compiler_diagnostic_fingerprint,
    compiler_diagnostics_with_snippets, run_compiler_check, update_compiler_diagnostic_streak,
};

#[path = "network/subagents.rs"]
pub(crate) mod subagents;
pub(crate) use subagents::handle_agent_tool;
#[allow(unused_imports)]
pub(crate) use subagents::{run_subagent, set_subagent_status};

#[path = "network/title.rs"]
pub(crate) mod title;
pub use title::generate_title;
pub(crate) use title::{record_prompt_to_history, spawn_title_generation};

#[path = "network/context_tail.rs"]
pub(crate) mod context_tail;
pub(crate) use context_tail::{
    build_dynamic_context_tail, build_volatile_context_block,
    format_read_file_context_entry,
};

/// Injected as a system directive for the final wrap-up turn after a loop is
/// detected. Disables tools and forces a prose answer so the user gets a
/// summary instead of a silently aborted session. Ported from opencode's
/// `MAX_STEPS_PROMPT`.
const FORCE_ANSWER_PROMPT: &str = "CRITICAL — you are stuck in a loop. Tools are now DISABLED for this turn. \
Do NOT emit any tool calls (no reads, writes, edits, searches). Respond with TEXT ONLY, and include: \
a short statement that you stopped to avoid looping, a summary of what you found or accomplished so far, \
any remaining tasks, and a recommendation for what to do next. This overrides all other instructions.";

const LOOP_RECOVERY_PROMPT: &str = "The previous tool action repeated without making progress. Tools remain enabled for one recovery attempt. \
Do not repeat the same tool call or the same exact edit. Re-read a broader file region or use grep to verify exact target content, \
then use a grounded approach. If emitting a tool call in this recovery attempt, output the ```tool block cleanly. \
If the requested change is already present or cannot be applied safely, explain that instead of retrying. This is the final recovery attempt.";

const MAX_LOOP_RECOVERY_ROUNDS: u8 = 1;

/// Safety budgets for a single agent turn. These are deliberately generous —
/// the goal is to catch a runaway session (the benchmark that motivated this
/// hit 106 rounds with no hard stop), not to cut off healthy long-running
/// work. Any one signal firing is enough: a session that is genuinely
/// healthy on every other axis but has spent 500k tokens or 40 rounds has
/// stopped being worth running unattended.
const MAX_TURN_TOKEN_BUDGET: u64 = 5_000_000;
/// A tool that reports success without changing anything (already-applied
/// edits, no-op runs) does not count as progress, so this escalates much
/// faster than the round budget when the agent is just spinning.
const MAX_CONSECUTIVE_NO_PROGRESS: usize = 8;
const MAX_CONSECUTIVE_FAILED_MUTATIONS: usize = 5;
const MAX_CONSECUTIVE_COMPILER_ERROR_GATES: usize = 5;
const MAX_CONSECUTIVE_COMPILER_DIAGNOSTICS: usize = 4;
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
    Tokens(u64),
    NoProgress(usize),
    FailedMutations(usize),
    CompilerErrorGates(usize),
    CompilerDiagnostics(usize),
    MalformedCalls(usize),
}

impl std::fmt::Display for TurnBudgetLimit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TurnBudgetLimit::ToolRounds(n) => write!(f, "maximum tool rounds reached ({n})"),
            TurnBudgetLimit::Tokens(n) => write!(f, "maximum token budget reached (~{n} tokens)"),
            TurnBudgetLimit::NoProgress(n) => write!(
                f,
                "{n} consecutive tool results with no meaningful progress (no-op or unchanged edits)"
            ),
            TurnBudgetLimit::FailedMutations(n) => {
                write!(f, "{n} consecutive failed edits")
            }
            TurnBudgetLimit::CompilerErrorGates(n) => {
                write!(
                    f,
                    "{n} consecutive completion attempts with the build still broken"
                )
            }
            TurnBudgetLimit::CompilerDiagnostics(n) => {
                write!(
                    f,
                    "{n} consecutive edits left the same compiler diagnostics unchanged"
                )
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
    if ctx.tool_rounds >= ctx.max_tool_rounds {
        return Some(TurnBudgetLimit::ToolRounds(ctx.tool_rounds));
    }
    if ctx.tokens_used >= MAX_TURN_TOKEN_BUDGET {
        return Some(TurnBudgetLimit::Tokens(ctx.tokens_used));
    }
    if ctx.consecutive_no_progress >= MAX_CONSECUTIVE_NO_PROGRESS {
        return Some(TurnBudgetLimit::NoProgress(ctx.consecutive_no_progress));
    }
    if ctx.consecutive_failed_mutations >= MAX_CONSECUTIVE_FAILED_MUTATIONS {
        return Some(TurnBudgetLimit::FailedMutations(
            ctx.consecutive_failed_mutations,
        ));
    }
    if ctx.consecutive_compiler_error_gates >= MAX_CONSECUTIVE_COMPILER_ERROR_GATES {
        return Some(TurnBudgetLimit::CompilerErrorGates(
            ctx.consecutive_compiler_error_gates,
        ));
    }
    if ctx.consecutive_compiler_diagnostics >= MAX_CONSECUTIVE_COMPILER_DIAGNOSTICS {
        return Some(TurnBudgetLimit::CompilerDiagnostics(
            ctx.consecutive_compiler_diagnostics,
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
    _state: &Arc<Mutex<AppState>>,
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
    ctx.stop_reason = Some(lifecycle::StopReason::BudgetExceeded(limit.to_string()));
    let mut s = _state.lock().await;
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

/// Push a loop warning, replacing the previous one if it's still the last
/// history entry — a model stuck in a loop would otherwise collect one
/// near-identical warning per round, crowding out the transcript.
fn push_or_replace_loop_warning(history: &mut Vec<ChatMessage>, text: String) {
    if let Some(last) = history.last_mut()
        && last.role == "system"
        && last.content.starts_with("[Loop warning:")
    {
        last.content = text;
    } else {
        history.push(ChatMessage::new("system", text));
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

fn failure_replan_message(tool: &str, category: &str, repeats: usize) -> String {
    format!(
        "[Replan required: {repeats} equivalent mutation attempts for '{tool}' ({category}) failed. These failed attempts changed no files. Do not retry the same edit. Inspect the current workspace, give a concise status, and ask the user for a decision if the requested change still cannot be applied safely.]"
    )
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
pub(crate) fn is_read_only_tool(name: &str) -> bool {
    matches!(
        crate::tools::tool_safety(name),
        crate::tools::ToolSafety::ReadOnly
    )
}

pub(crate) fn is_mutating_tool(name: &str) -> bool {
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
pub(crate) fn view_file_unchanged_since_last_read(
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
    if model.to_lowercase().contains("gemini") {
        dbg_log!(
            "probe_function_calling: model {} contains 'gemini', defaulting to Json tool protocol for thought_signature compatibility",
            model
        );
        return false;
    }

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



/// Handle an interactive `ask_question` tool call: show the option-picker modal
/// and block until the user chooses (or cancels / the turn is cancelled). Returns
/// the chosen option text — that becomes the tool result fed back to the model,
/// so it can continue with the user's answer.
async fn ask_user_question(
    state: &Arc<Mutex<AppState>>,
    cancel_token: &tokio_util::sync::CancellationToken,
    args: &serde_json::Value,
) -> (crate::tools::ToolExecutionOutput, std::time::Duration) {
    let (mut question, mut options, is_multi_select) =
        if let Some(q_arr) = args.get("questions").and_then(|v| v.as_array()) {
            if let Some(first_q) = q_arr.first().and_then(|v| v.as_object()) {
                let q_str = first_q
                    .get("question")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let opts: Vec<String> = first_q
                    .get("options")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|o| o.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default();
                let multi = first_q
                    .get("is_multi_select")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                (q_str, opts, multi)
            } else {
                (String::new(), Vec::new(), false)
            }
        } else {
            let q_str = args
                .get("question")
                .or_else(|| args.get("prompt"))
                .or_else(|| args.get("message"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let opts: Vec<String> = args
                .get("options")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|o| o.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            let multi = args
                .get("is_multi_select")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            (q_str, opts, multi)
        };

    if question.trim().is_empty() {
        question = "Please confirm how to proceed:".to_string();
    }
    if options.is_empty() {
        options = vec!["Proceed".to_string(), "Cancel".to_string()];
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

    let start_wait = std::time::Instant::now();
    let answer = tokio::select! {
        _ = cancel_token.cancelled() => None,
        res = rx => res.ok(),
    };
    let user_wait = start_wait.elapsed();

    {
        let mut s = state.lock().await;
        s.pending_question = None;
        s.question_response = None;
        if s.status == AppStatus::AwaitingQuestion {
            s.status = AppStatus::Streaming;
        }
    }

    let out = match answer {
        Some(a) if !a.is_empty() => {
            crate::tools::ToolExecutionOutput::success(format!("User selected: {a}"))
        }
        _ => crate::tools::ToolExecutionOutput::failure(
            "User cancelled or provided no selection.".to_string(),
        ),
    };
    (out, user_wait)
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
) -> (
    crate::tools::ToolExecutionOutput,
    Option<String>,
    std::time::Duration,
) {
    let (agent_mode, auto_confirm) = {
        let s = state.lock().await;
        (s.agent_mode, s.auto_confirm)
    };
    if let crate::tools::AuthorizationDecision::Deny(reason) =
        crate::tools::authorize_tool_with_args(name, args, agent_mode, auto_confirm, bypass_confirm)
    {
        return (
            crate::tools::ToolExecutionOutput::failure(format!("error: {reason}")),
            None,
            std::time::Duration::ZERO,
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
            });
        }
    }

    let diff_opt = get_diff_preview(name, args);

    let needs_confirm = matches!(
        crate::tools::authorize_tool_with_args(
            name,
            args,
            agent_mode,
            auto_confirm,
            bypass_confirm,
        ),
        crate::tools::AuthorizationDecision::RequireConfirmation
    );
    let mut user_wait_dur = std::time::Duration::ZERO;
    let mut result = if !needs_confirm {
        dbg_log!("Executing tool '{}' immediately...", name);
        let tool_name = name.to_string();
        {
            let mut s = state.lock().await;
            s.running_tools.push(tool_name.clone());
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
            if name == "run_command" {
                let command = args
                    .get("command")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                (
                    crate::tools::command_confirmation_preview(command),
                    command.len(),
                )
            } else {
                let content = args.get("content").and_then(|c| c.as_str()).unwrap_or("");
                let preview = content.lines().take(6).collect::<Vec<_>>().join("\n");
                (preview, content.len())
            }
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
        let start_wait = std::time::Instant::now();
        let rx_res = rx.await;
        user_wait_dur = start_wait.elapsed();

        let res = match rx_res {
            Ok(true) => {
                dbg_log!("User approved tool call '{}', executing...", name);
                let tool_name = name.to_string();
                {
                    let mut s = state.lock().await;
                    s.pending_tool_confirmation = None;
                    s.status = AppStatus::Streaming;
                    s.stream_tracker = Some(StreamTracker::new());
                    s.running_tools.push(tool_name.clone());
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
            s.status = AppStatus::Streaming;
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

    (result, diff_opt, user_wait_dur)
}

const MAX_ACTIVE_SUBAGENTS: usize = 4;

fn push_status_line(s: &mut AppState, text: String) {
    s.history.push(ChatMessage::new("system", text));
    crate::config::save_history(&s.history);
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
    cancel_token: &tokio_util::sync::CancellationToken,
) -> Result<Vec<serde_json::Value>, String> {
    // Try AI-driven compaction if history is long enough.
    //
    // The summarizer is a network round-trip, so the AppState mutex must NOT be
    // held while it runs: the TUI draw loop locks the same mutex every frame and
    // would freeze for the whole call. Instead we take a snapshot of the history
    // under a short lock, compact the owned copy with the lock released, then
    // re-acquire and merge the result back in.
    {
        let (api_url, model_name, budget, active_session_id, captured_history) = {
            let s = state.lock().await;
            (
                s.api_base_url.clone(),
                s.model_name.clone(),
                s.get_history_token_budget() as usize,
                s.active_session_id.clone(),
                s.history.clone(),
            )
        };
        let pre_len = captured_history.len();
        let mut working_history = captured_history.clone();

        // Lock released here: this await performs I/O.
        let compacted = compaction::maybe_compact(
            client,
            &api_url,
            &model_name,
            &mut working_history,
            budget,
            cancel_token,
        )
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
        let live_session_id = s.active_session_id.clone();
        let prefix_intact =
            live_session_id == active_session_id && s.history.starts_with(&captured_history);
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
                "Skipping automatic compaction write-back: active session or history changed underneath the summarizer (captured session '{}', live session '{}', {} messages before, {} now). Live history kept as-is.",
                active_session_id,
                live_session_id,
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
        active_profile,
        vision_profile,
        mut image_cache,
    ) = {
        let mut s = state.lock().await;
        let history_snapshot = s.history.clone();
        let budget_token_limit = s.get_history_token_budget();
        let mut read_files: Vec<String> = s
            .read_file_mtimes
            .iter()
            .map(|(path, snapshot_mtime)| {
                format_read_file_context_entry(path, Some(*snapshot_mtime), path_mtime(path))
            })
            .collect();
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
            s.active_model_profile(),
            s.vision_model_profile(),
            s.image_analysis_cache.clone(),
        )
    };

    compact_history_to_budget(&mut history_snapshot, budget_token_limit).await;

    if history_snapshot
        .iter()
        .any(|m| m.role == "user" && m.content.contains("![image](file://"))
    {
        let active_profile = active_profile.ok_or_else(|| {
            "image analysis failed: active model profile is not configured".to_string()
        })?;
        if active_profile.image_input_supported() != Some(true) {
            let vision_profile = vision_profile.ok_or_else(|| {
                "image analysis failed: configure a dedicated vision_model profile".to_string()
            })?;
            let request_client = client.clone();
            let request_cancel = cancel_token.clone();
            history_snapshot = image_fallback::prepare_history_for_model(
                &history_snapshot,
                &active_profile,
                &vision_profile,
                &mut image_cache,
                |profile, bytes| {
                    let profile = profile.clone();
                    let request_client = request_client.clone();
                    let request_cancel = request_cancel.clone();
                    async move {
                        image_fallback::request_vision_analysis(
                            &request_client,
                            &profile,
                            bytes,
                            &request_cancel,
                        )
                        .await
                    }
                },
            )
            .await?;
            state
                .lock()
                .await
                .image_analysis_cache
                .extend(image_cache);
        }
    }

    history_snapshot.retain(|m| {
        (matches!(m.role.as_str(), "user" | "assistant" | "tool")
            && !m.content.starts_with('/'))
            || is_model_directed_note(m)
    });

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

    Ok(msgs)
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

fn tool_result_history_message(result: ToolResult, answered_call: Option<String>) -> ChatMessage {
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
    user_wait_duration: &mut std::time::Duration,
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
                        let mut user_wait = std::time::Duration::ZERO;
                        execute_tool_batch(
                            client,
                            state,
                            cancel_token,
                            std::slice::from_ref(call),
                            approved,
                            &None,
                            &mut read_dirty,
                            &mut read_cache,
                            &mut user_wait,
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
                    user_wait_duration,
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
        let (executed_name, execution, diff_opt, replay_artifact, user_wait) = async move {
            // File-cache-diff: re-reads of unchanged files replay cached output
            // with a short prefix instead of re-sending full content.
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

            let (execution, diff_opt, user_wait) = if is_repeat {
                // Repeating an unmutated read call: return a cached copy if we have
                // one, or a explicit guidance notice so the model stops
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
                let tuple = match cached {
                    Some(previous) => {
                        let content = if let Some(mut content) = previous.replayable_content {
                            content.insert_str(
                                0,
                                "[Unchanged since the last read of this exact range — repeating that output. \
Re-reading will not produce anything new; if an edit failed to match, expand start_line/end_line range or use grep to verify exact target content.]\n",
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
                };
                (tuple.0, tuple.1, std::time::Duration::ZERO)
            } else if name_clone == "ask_question" {
                let (output, wait) = ask_user_question(&state_clone, &cancel_token_clone, &args_clone).await;
                (output, None, wait)
            } else if plan_mode_denied {
                (
                    crate::tools::ToolExecutionOutput::failure(
                        "error: Plan mode is active; this tool is not permitted.".to_string(),
                    ),
                    None,
                    std::time::Duration::ZERO,
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
                    std::time::Duration::ZERO,
                )
            } else {
                let workspace_root = { state_clone.lock().await.workspace_root.clone() };
                confirm_and_execute(
                    &state_clone,
                    &cancel_token_clone,
                    &name_clone,
                    &args_clone,
                    &name_clone,
                    true, // bypass confirmation
                    workspace_root,
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

            (name_clone, execution, diff_opt, replay_artifact, user_wait)
        }
        .await;
        *user_wait_duration += user_wait;
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
        let mut result = tool_result_from_execution(&executed_name, args, execution, final_diff);
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

fn record_provider_error(ctx: &mut TurnContext, error: &str) {
    ctx.provider_errors += 1;
    let is_quota =
        error.contains("429") || error.to_ascii_lowercase().contains("too many requests");
    if is_quota {
        ctx.provider_429s += 1;
        ctx.stop_reason = Some(lifecycle::StopReason::ProviderError(Some(429)));
    } else if ctx.stop_reason.is_none() {
        ctx.stop_reason = Some(lifecycle::StopReason::ProviderError(None));
    }
}

fn active_todo_checkpoint(todos: &[crate::app::TodoItem]) -> Option<String> {
    todos
        .iter()
        .find(|todo| todo.status == "in_progress")
        .map(|todo| todo.content.clone())
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
    /// Configured hard backstop for a single agent turn. Semantic progress
    /// and failure budgets should stop unhealthy work before this limit.
    pub max_tool_rounds: usize,
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
    /// Consecutive mutation batches that leave the same compiler diagnostics
    /// unchanged. A clean check or a changed diagnostic set resets this.
    pub consecutive_compiler_diagnostics: usize,
    pub last_compiler_diagnostic_fingerprint: Option<String>,
    /// Consecutive tool-call blocks the harness could not parse at all
    /// (distinct from a parsed call that executed and failed). Reset the
    /// moment a well-formed batch reaches execution.
    pub consecutive_malformed_calls: usize,
    /// Set once a safety budget stops the turn, so the caller can tell a
    /// budget stop apart from a normal finish or a detected loop.
    pub budget_stopped: Option<String>,
    /// Accumulated duration spent waiting for interactive user response (ask_question / confirmation).
    pub user_wait_duration: std::time::Duration,
    /// Benchmark counters and terminal facts retained for the final run summary.
    pub tool_calls: usize,
    pub malformed_calls: usize,
    pub no_progress_results: usize,
    pub failure_replans: usize,
    pub provider_errors: usize,
    pub provider_429s: usize,
    pub changed_paths: std::collections::BTreeSet<String>,
    pub phase_checkpoint: Option<String>,
    pub stop_reason: Option<lifecycle::StopReason>,
}

impl TurnContext {
    pub fn new() -> Self {
        Self::with_max_tool_rounds(crate::config::DEFAULT_MAX_TOOL_ROUNDS)
    }

    pub fn with_max_tool_rounds(max_tool_rounds: usize) -> Self {
        Self {
            tool_rounds: 0,
            max_tool_rounds: max_tool_rounds.max(1),
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
            consecutive_compiler_diagnostics: 0,
            last_compiler_diagnostic_fingerprint: None,
            consecutive_malformed_calls: 0,
            budget_stopped: None,
            user_wait_duration: std::time::Duration::ZERO,
            tool_calls: 0,
            malformed_calls: 0,
            no_progress_results: 0,
            failure_replans: 0,
            provider_errors: 0,
            provider_429s: 0,
            changed_paths: std::collections::BTreeSet::new(),
            phase_checkpoint: None,
            stop_reason: None,
        }
    }

    pub fn benchmark_summary(&self) -> serde_json::Value {
        serde_json::json!({
            "tool_rounds": self.tool_rounds,
            "tool_calls": self.tool_calls,
            "tokens_used": self.tokens_used,
            "malformed_calls": self.malformed_calls,
            "no_progress_results": self.no_progress_results,
            "failure_replans": self.failure_replans,
            "compiler_diagnostic_streak": self.consecutive_compiler_diagnostics,
            "provider_errors": self.provider_errors,
            "provider_429s": self.provider_429s,
            "changed_paths": self.changed_paths.iter().collect::<Vec<_>>(),
            "phase_checkpoint": self.phase_checkpoint,
            "stop_reason": self.stop_reason.as_ref().map(ToString::to_string),
        })
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

    if !ctx.force_final {
        ctx.stop_reason = None;
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

    let msgs = match prepare_turn_request(client, state, ctx.tool_rounds, cancel_token).await {
        Ok(msgs) => msgs,
        Err(error) => {
            dbg_log!("Image fallback failed: {error}");
            let mut s = state.lock().await;
            ctx.stop_reason = Some(if error == "cancelled" {
                lifecycle::StopReason::Cancelled
            } else {
                lifecycle::StopReason::RecoveryFailed
            });
            let notice = if error == "cancelled" {
                "Request cancelled by user".to_string()
            } else {
                format!("Image analysis unavailable: {error}")
            };
            s.history.push(ChatMessage::new("system", notice));
            s.current_token_usage = None;
            return false;
        }
    };

    state.lock().await.current_response.clear();
    stream_buffer.lock().await.reset();

    let (api_base_url, model_name) = {
        let s = state.lock().await;
        (s.api_base_url.clone(), s.model_name.clone())
    };

    let turn_start_time = std::time::Instant::now();

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
                let nudge = continuation_nudge(&previous);
                current_msgs.push(serde_json::json!({
                    "role": "assistant",
                    "content": previous
                }));
                current_msgs.push(serde_json::json!({
                    "role": "user",
                    "content": nudge
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
                if e == "cancelled" {
                    ctx.stop_reason = Some(lifecycle::StopReason::Cancelled);
                } else {
                    record_provider_error(ctx, &e);
                }
                let mut s = state.lock().await;
                let notice = if e == "cancelled" {
                    "Request cancelled by user".to_string()
                } else {
                    format!("Error from LLM Provider: {e}")
                };
                s.history.push(ChatMessage::new("system", notice));
                s.current_token_usage = None;
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
    let turn_response_time_ms = turn_start_time.elapsed().as_millis() as u64;

    let turn_token_usage = {
        let s = state.lock().await;
        if s.current_token_usage.is_some() {
            s.current_token_usage.clone()
        } else {
            drop(s);
            let est = estimate_token_usage(&ctx.last_sent_messages, &accumulated_content).await;
            let mut s = state.lock().await;
            s.current_token_usage = est.clone();
            est
        }
    };

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
        ctx.stop_reason = Some(lifecycle::StopReason::Cancelled);
        ctx.turn_machine.cancel();
        return false;
    }

    ctx.final_content = accumulated_content;
    let (native_tool_calls, streamed_call_ids) = {
        let buffer = stream_buffer.lock().await;
        (
            buffer.native_tool_calls.clone(),
            buffer.tool_call_ids.clone(),
        )
    };
    ctx.streamed_call_ids = if native_tool_calls.is_empty() {
        streamed_call_ids
    } else {
        native_tool_calls
            .iter()
            .map(|call| call.call_id.clone())
            .collect()
    };
    dbg_log!(
        "Stream completed successfully. Content length: {} chars",
        ctx.final_content.len()
    );

    if ctx.final_content.is_empty() && native_tool_calls.is_empty() {
        dbg_log!("Stream returned empty content, finishing");
        let mut s = state.lock().await;
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
        ctx.final_content = answer;
        return false;
    }

    let protocol = { state.lock().await.active_tool_protocol() };
    let model_response = if matches!(protocol, crate::config::ToolProtocol::ApiNative) {
        let typed_calls = native_tool_calls
            .into_iter()
            .map(|call| crate::tools::ToolCall {
                name: call.tool_name,
                arguments: call.arguments,
            })
            .collect();
        events::native_response(
            &ctx.final_content,
            response_finish_reason.as_deref(),
            typed_calls,
        )
    } else {
        events::normalize_response(
            &ctx.final_content,
            response_finish_reason.as_deref(),
            protocol,
        )
    };
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
        if lifecycle::is_unavailable_tool_error(&reason) {
            ctx.stop_reason = Some(lifecycle::StopReason::UnavailableTool);
        }
        dbg_log!("Tool-call validation rejected response: {}", reason);
        let mut s = state.lock().await;
        // `ctx.final_content` was already replaced with a shape-only
        // summary when the batch was truncated, so a rejection here can
        // never replay the fabricated prose that came with it.
        let rejected_refs = call_refs_for(&parsed_tool_calls, &ctx.streamed_call_ids);
        let mut msg = ChatMessage::new("assistant", ctx.final_content.clone())
            .with_tool_calls(rejected_refs.clone());
        msg.response_time_ms = Some(turn_response_time_ms);
        msg.token_usage = turn_token_usage.clone();
        s.history.push(msg);
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
        let mut loop_offender: Option<String> = None;
        for call in &tool_calls {
            let (exact, category) = loop_detect::signatures(&call.name, &call.arguments);
            let s = ctx.loop_detector.check_tool(&call.name, &exact, &category);
            if s.rank() > loop_status.rank() {
                loop_status = s;
                loop_offender = Some(format!("{} ({category})", call.name));
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
                        let mut msg = ChatMessage::new("assistant", &ctx.final_content);
                        msg.response_time_ms = Some(turn_response_time_ms);
                        msg.token_usage = turn_token_usage.clone();
                        s.history.push(msg);
                        s.history
                            .push(ChatMessage::new("system", LOOP_RECOVERY_PROMPT));
                        crate::config::save_history(&s.history);
                        s.current_response.clear();
                        s.status = AppStatus::Streaming;
                        s.stream_tracker = Some(StreamTracker::new());
                        drop(s);
                        ctx.turn_machine.abandon_tool_phase();
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
                        let mut msg = ChatMessage::new("assistant", &ctx.final_content);
                        msg.response_time_ms = Some(turn_response_time_ms);
                        msg.token_usage = turn_token_usage.clone();
                        s.history.push(msg);
                        s.history
                            .push(ChatMessage::new("system", FORCE_ANSWER_PROMPT));
                        crate::config::save_history(&s.history);
                        s.current_response.clear();
                        drop(s);
                        ctx.turn_machine.abandon_tool_phase();
                        ctx.stop_reason = Some(lifecycle::StopReason::LoopEscalation);
                        ctx.force_final = true;
                        return true;
                    }
                }
            }
            loop_detect::LoopStatus::Warning(n) => {
                dbg_log!("Loop detector: warning at {} repeats", n);
                let mut s = state.lock().await;
                // Name the offending action: "this action has repeated" left
                // the model (and anyone resuming the transcript) guessing
                // which of the round's calls the warning was about.
                let action = loop_offender.as_deref().unwrap_or("the last tool action");
                let warning_text = format!(
                    "[Loop warning: '{action}' has repeated {n} times. If a tool edit or view is failing, stop retrying the same inputs — if an edit failed to match, view a wider line range or use grep to verify exact target content.]"
                );
                push_or_replace_loop_warning(&mut s.history, warning_text);
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
                let mut msg = ChatMessage::new("assistant", &ctx.final_content)
                    .with_tool_calls(call_refs.clone());
                msg.response_time_ms = Some(turn_response_time_ms);
                msg.token_usage = turn_token_usage.clone();
                s.history.push(msg);
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
                &mut ctx.user_wait_duration,
                deferred_notice,
            )
            .await;

            ctx.tool_calls += results.len();
            let mutation_batch = results
                .iter()
                .any(|result| is_mutating_tool(&result.tool_name));
            if mutation_batch {
                let diagnostics = results
                    .iter()
                    .find_map(|result| compiler_diagnostic_fingerprint(&result.content));
                if diagnostics.is_some() || !ctx.compile_dirty {
                    update_compiler_diagnostic_streak(ctx, diagnostics);
                }
            }

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
                ctx.stop_reason = Some(lifecycle::StopReason::Cancelled);
                let mut s = state.lock().await;
                // The assistant message announcing these calls is already
                // in history; leaving them unanswered would break the next
                // request and strand the model without knowing they were
                // interrupted.
                for message in unanswered_call_results(&call_refs, "interrupted by the user") {
                    s.history.push(message);
                }
                // Text protocols produce no call refs, so the loop above
                // records nothing there. Leave an explicit marker so both
                // the transcript and the model's next context show the turn
                // was cancelled before any result arrived.
                if call_refs.is_empty() {
                    s.history
                        .push(ChatMessage::new("system", "Request cancelled by user"));
                }
                ctx.turn_machine.finish_tools_if_executing();
                return false;
            }

            let mut s = state.lock().await;
            s.status = AppStatus::Streaming;
            let mut completed = false;
            let mut stagnation = loop_detect::LoopStatus::Ok;
            let mut failure_replan = None;
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
                    ctx.verification.record_command(command, metadata.exit_code);
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
                ctx.changed_paths
                    .extend(metadata.changed_paths.iter().cloned());
                if name == "todo_write" && metadata.success {
                    ctx.phase_checkpoint = active_todo_checkpoint(&s.todos);
                    if let Some(phase) = ctx.phase_checkpoint.as_deref() {
                        s.history.push(ChatMessage::new(
                            "system",
                            format!("[phase checkpoint: {phase}]"),
                        ));
                    }
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
                        if let Some(call) = call {
                            let (exact, category) =
                                loop_detect::signatures(&call.name, &call.arguments);
                            if let loop_detect::LoopStatus::Abort(repeats) =
                                ctx.loop_detector.record_failed_tool(&exact, &category)
                            {
                                failure_replan =
                                    Some(failure_replan_message(&call.name, &category, repeats));
                            }
                        }
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
                        ctx.no_progress_results += 1;
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
                match ctx
                    .loop_detector
                    .record_output(loop_detect::stagnation_key(&content))
                {
                    status @ (loop_detect::LoopStatus::Warning(n)
                    | loop_detect::LoopStatus::Abort(n)) => {
                        dbg_log!("Loop detector: output stagnation x{} for '{}'", n, name);
                        if status.rank() > stagnation.rank() {
                            stagnation = status;
                        }
                    }
                    loop_detect::LoopStatus::Ok => {}
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

            // Surface output stagnation to the model. Until now this signal
            // was only written to the debug log, so a model re-rolling search
            // terms against an empty result set got no feedback until the
            // tool-round budget killed the turn.
            if let loop_detect::LoopStatus::Warning(n) | loop_detect::LoopStatus::Abort(n) =
                stagnation
            {
                push_or_replace_loop_warning(
                    &mut s.history,
                    format!(
                        "[Loop warning: the last {n} tool results were identical in kind (e.g. repeated \"no matches\"). Re-phrasing the same search is not progress — the answer is not where you are looking. View the relevant file directly or change approach.]"
                    ),
                );
            }

            if let Some(replan) = failure_replan {
                ctx.failure_replans += 1;
                ctx.stop_reason = Some(lifecycle::StopReason::RecoveryFailed);
                s.history.push(ChatMessage::new("system", replan));
                crate::config::save_history(&s.history);
                s.current_response.clear();
                drop(s);
                ctx.force_final = true;
                ctx.turn_machine.finish_tools_if_executing();
                return true;
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
                ctx.stop_reason = Some(lifecycle::StopReason::Completed);
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
                    {
                        let mut s = state.lock().await;
                        s.status = AppStatus::Streaming;
                    }
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

                dbg_log!("complete_task accepted; finalizing the turn");
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
        ctx.malformed_calls += 1;
        let mut s = state.lock().await;
        let mut msg = ChatMessage::new("assistant", &ctx.final_content);
        msg.response_time_ms = Some(turn_response_time_ms);
        msg.token_usage = turn_token_usage.clone();
        s.history.push(msg);

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
        // The parse failure may have already transitioned the turn machine
        // to Completed (has_tool_calls=false). Reset it so the next model
        // response can be classified and executed normally.
        let _ = ctx.turn_machine.retry_for_finish_gate();
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
        {
            let mut s = state.lock().await;
            s.status = AppStatus::Streaming;
        }
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
                let mut msg = ChatMessage::new("assistant", ctx.final_content.clone());
                msg.response_time_ms = Some(turn_response_time_ms);
                msg.token_usage = turn_token_usage.clone();
                s.history.push(msg);
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
    let mut turn_lifecycle = lifecycle::TurnLifecycle::new();

    let max_tool_rounds = { state.lock().await.config.max_tool_rounds };
    let mut ctx = TurnContext::with_max_tool_rounds(max_tool_rounds);
    while run_single_turn(client, state, cancel_token, policy, stream_buffer, &mut ctx).await {}

    if ctx.stop_reason.is_none() {
        ctx.stop_reason = Some(if ctx.task_completed {
            lifecycle::StopReason::Completed
        } else {
            lifecycle::StopReason::RecoveryFailed
        });
    }
    let stop_reason = ctx
        .stop_reason
        .clone()
        .expect("turn finalization always assigns a stop reason");
    if !turn_lifecycle.mark_finalized() {
        return ctx;
    }
    let had_final_content = !ctx.final_content.trim().is_empty();
    let final_transcript =
        lifecycle::final_transcript_content(ctx.task_completed, &ctx.final_content, &stop_reason);
    if let Some(content) = final_transcript.as_ref()
        && !had_final_content
    {
        ctx.final_content = content.clone();
    }
    crate::logger::operational_event(
        "turn.summary",
        serde_json::json!({
            "completed_task": ctx.task_completed,
            "metrics": ctx.benchmark_summary(),
        }),
    );

    {
        dbg_log!("Finishing agent loop, writing final transcript");
        crate::logger::operational_event(
            "turn.finish",
            serde_json::json!({
                "completed_task": ctx.task_completed,
                "tool_rounds": ctx.tool_rounds,
                "content_bytes": ctx.final_content.len(),
                "cancelled": cancel_token.is_cancelled(),
                "metrics": ctx.benchmark_summary(),
            }),
        );

        let mut s = state.lock().await;
        s.continuous_mode = false;
        s.response_time = Some(prompt_start_time.elapsed());
        // On the complete_task path the summary was already appended; only
        // record token usage / notify below, don't duplicate the reply.
        if let Some(content) = final_transcript {
            let role = if had_final_content {
                "assistant"
            } else {
                "system"
            };
            let mut msg = ChatMessage::new(role, content);
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
            if msg.token_usage.is_none() {
                msg.token_usage = usage.clone();
            }
        }

        let active_id = s.active_session_id.clone();
        crate::config::save_session_history(&active_id, &s.history);
        // Turn end: force the queued snapshot to disk, on a blocking thread so
        // the runtime keeps serving the UI.
        crate::config::flush_history_async();

        s.current_response.clear();
        s.status = AppStatus::Idle;
        s.request_redraw();

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

        let notification = if matches!(stop_reason, lifecycle::StopReason::Cancelled) {
            crate::notifications::FinishedStatus::Cancelled
        } else {
            crate::notifications::FinishedStatus::Success
        };
        let _ = crate::notifications::notify_finished(notification);
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
                break;
            }
            s.status = AppStatus::Streaming;
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
            break;
        }
    }
    // Safety net: every loop exit that isn't the queue-empty branch (stream
    // error, cancel, empty content) lands here — always release the guard so a
    // future turn can start.
    state.lock().await.orchestrator_running = false;
    dbg_log!("Orchestrator finished");
}


#[cfg(test)]
#[path = "network/tests.rs"]
mod tests;
