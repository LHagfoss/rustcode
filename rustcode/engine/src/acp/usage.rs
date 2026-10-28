//! Context and token usage over ACP. The standard `usage_update` carries the
//! context window (`used` / `size`); RustCode's session token totals ride in
//! `_meta.rustcode`, since the update has no fields for them. The engine
//! does not price requests, so `cost` stays unset.

use agent_client_protocol::schema::v1::{SessionUpdate, UsageUpdate};

/// Bumped when the `_meta.rustcode` usage fields change incompatibly.
pub(crate) const USAGE_META_VERSION: u32 = 1;

/// The session's context and token usage as the TUI footer reports it.
///
/// `used` is the prompt the provider counted for the latest request; with no
/// current request (a freshly loaded session) it is estimated from the active
/// history instead of reusing a saved request's usage, which a compaction may
/// have made stale. Input and output totals sum the usage recorded on each
/// assistant message; input re-counts the conversation on every request, so
/// it is what a provider bills and far larger than the context in use.
pub(crate) fn usage_update(state: &crate::app::AppState) -> SessionUpdate {
    let (used, estimated) = match state.current_token_usage.as_ref() {
        Some(usage) => (u64::from(usage.prompt_tokens), false),
        None => {
            let chars: usize = state
                .history
                .iter()
                .map(|message| message.content.len())
                .sum();
            ((chars / 4) as u64, true)
        }
    };
    let size = u64::from(state.active_context_window());
    let (mut input, mut output, mut cached, mut responses) = (0u64, 0u64, 0u64, 0u64);
    for usage in state
        .history
        .iter()
        .filter(|message| message.role == "assistant")
        .filter_map(|message| message.token_usage.as_ref())
    {
        input += u64::from(usage.prompt_tokens);
        output += u64::from(usage.completion_tokens);
        cached += u64::from(usage.cached_tokens.unwrap_or(0));
        responses += 1;
    }
    let mut meta = serde_json::Map::new();
    meta.insert(
        "rustcode".to_owned(),
        serde_json::json!({
            "kind": "usage",
            "version": USAGE_META_VERSION,
            "estimated": estimated,
            "inputTokens": input,
            "outputTokens": output,
            "cachedTokens": cached,
            "responses": responses,
            "context": context_breakdown(state),
        }),
    );
    SessionUpdate::UsageUpdate(UsageUpdate::new(used, size).meta(meta))
}

/// Bumped when the `_meta.rustcode` context fields change incompatibly.
/// v2 adds the `compaction` object and the compacted-history category.
pub(crate) const CONTEXT_META_VERSION: u32 = 2;

/// The `/context` breakdown as data, for clients that draw it themselves.
/// Category names and order match the TUI legend. `reserves` is null when the
/// active profile's reserves do not describe the window in view.
pub(crate) fn context_breakdown(state: &crate::app::AppState) -> serde_json::Value {
    let breakdown = crate::controller::ContextBreakdown::from_state(state);
    let categories = crate::controller::CONTEXT_CATEGORY_LABELS
        .iter()
        .zip(breakdown.category_tokens())
        .map(|(name, tokens)| serde_json::json!({"name": name, "tokens": tokens}))
        .collect::<Vec<_>>();
    serde_json::json!({
        "version": CONTEXT_META_VERSION,
        "model": state.active_model_profile().map(|profile| profile.name),
        "window": breakdown.context_window,
        "used": breakdown.current_usage.used_tokens,
        "estimated": breakdown.current_usage.source
            == crate::controller::ContextUsageSource::HistoryEstimate,
        "free": breakdown.remaining_tokens,
        "categories": categories,
        "compaction": breakdown.compaction.as_ref().map(|stats| serde_json::json!({
            "trigger": stats.trigger,
            "mode": stats.mode,
            "beforeTokens": stats.before_tokens,
            "afterTokens": stats.after_tokens,
            "reclaimedTokens": stats.reclaimed_tokens,
            "messagesConsidered": stats.messages_considered,
            "count": stats.count,
        })),
        "reserves": breakdown.configured_guards.map(|guards| serde_json::json!({
            "output": guards.output_tokens,
            "thinking": guards.thinking_tokens,
            "tools": guards.tool_tokens,
            "safety": guards.safety_tokens,
            "providerOverhead": guards.provider_framing_tokens,
        })),
    })
}

/// Bumped when the `_meta.rustcode` compaction card fields change
/// incompatibly.
pub(crate) const COMPACTION_META_VERSION: u32 = 1;

/// The chat's compaction card: a reply chunk whose text says what happened,
/// for a client that does not draw the card, and whose
/// `_meta.rustcode.kind == "compaction"` carries the numbers and the summary
/// the model now works from. `stats` is `None` for a replayed compaction the
/// session has no record of. `text` replaces the default one-line notice.
pub(crate) fn compaction_card(
    stats: Option<&crate::app::CompactionStats>,
    summary: Option<&str>,
    text: Option<String>,
) -> SessionUpdate {
    use agent_client_protocol::schema::v1::{ContentBlock, ContentChunk, TextContent};
    let text = text.unwrap_or_else(|| match stats {
        Some(stats) => format!(
            "History compacted: reduced context from {} to {} tokens.",
            stats.before_tokens, stats.after_tokens
        ),
        None => "History compacted.".to_owned(),
    });
    let mut meta = serde_json::Map::new();
    meta.insert(
        "rustcode".to_owned(),
        serde_json::json!({
            "kind": "compaction",
            "version": COMPACTION_META_VERSION,
            "trigger": stats.map(|stats| stats.trigger),
            "mode": stats.map(|stats| stats.mode),
            "beforeTokens": stats.map(|stats| stats.before_tokens),
            "afterTokens": stats.map(|stats| stats.after_tokens),
            "reclaimedTokens": stats.map(|stats| stats.reclaimed_tokens),
            "count": stats.map(|stats| stats.count),
            "summary": summary,
        }),
    );
    SessionUpdate::AgentMessageChunk(
        ContentChunk::new(ContentBlock::Text(TextContent::new(text))).meta(meta),
    )
}

/// `_meta` for the `/context` reply chunk: the breakdown beside the Markdown.
pub(crate) fn context_reply_meta(
    state: &crate::app::AppState,
) -> serde_json::Map<String, serde_json::Value> {
    let mut meta = serde_json::Map::new();
    meta.insert(
        "rustcode".to_owned(),
        serde_json::json!({
            "kind": "context",
            "version": CONTEXT_META_VERSION,
            "context": context_breakdown(state),
        }),
    );
    meta
}

/// `_rustcode/usage/limits`: the plan's short and long usage windows for the
/// session's account, or null when none are known. The app asks when a turn
/// ends and when its meter is hovered, so a slow source never holds a turn.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/usage/limits", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UsageLimitsRequest {
    pub session_id: String,
}

/// Answer `_rustcode/usage/limits`. A ChatGPT sign-in reports no windows
/// itself; the Codex CLI's are read then (cached a minute) and kept.
pub(crate) async fn limits(
    state: &std::sync::Arc<tokio::sync::Mutex<crate::app::AppState>>,
) -> serde_json::Value {
    let (known, chatgpt) = {
        let state = state.lock().await;
        let chatgpt = state.active_model_profile().is_some_and(|profile| {
            profile
                .credential
                .as_ref()
                .is_some_and(|binding| binding.is_chatgpt())
        });
        (state.provider_rate_limits.clone(), chatgpt)
    };
    let limits = if chatgpt {
        match crate::provider_auth::codex_usage::rate_limits().await {
            Some(limits) => {
                state.lock().await.provider_rate_limits = Some(limits.clone());
                Some(limits)
            }
            None => known,
        }
    } else {
        known
    };
    limits_json(limits.as_ref(), if chatgpt { "codex" } else { "provider" })
}

fn limits_json(
    limits: Option<&crate::provider_auth::ProviderRateLimits>,
    source: &str,
) -> serde_json::Value {
    let Some(limits) = limits else {
        return serde_json::Value::Null;
    };
    let window = |window: Option<&crate::provider_auth::RateLimitWindow>| {
        window.map(|window| {
            serde_json::json!({
                "usedPercent": window.used_percent,
                "windowMinutes": window.window_minutes,
                "resetsAt": window.resets_at,
            })
        })
    };
    serde_json::json!({
        "source": source,
        "primary": window(limits.primary.as_ref()),
        "secondary": window(limits.secondary.as_ref()),
    })
}

#[cfg(test)]
mod tests {
    use super::{
        COMPACTION_META_VERSION, USAGE_META_VERSION, compaction_card, context_reply_meta,
        usage_update,
    };
    use crate::app::{ChatMessage, TokenUsage};

    fn assistant(prompt: u32, completion: u32, cached: Option<u32>) -> ChatMessage {
        let mut message = ChatMessage::new("assistant", "reply");
        message.token_usage = Some(TokenUsage {
            prompt_tokens: prompt,
            completion_tokens: completion,
            total_tokens: prompt + completion,
            cached_tokens: cached,
            ..Default::default()
        });
        message
    }

    #[test]
    fn limits_answer_names_both_windows() {
        let limits = crate::provider_auth::ProviderRateLimits {
            primary: Some(crate::provider_auth::RateLimitWindow {
                used_percent: 42.0,
                window_minutes: Some(300),
                resets_at: Some(1_800_000_000),
            }),
            secondary: None,
        };
        let value = super::limits_json(Some(&limits), "codex");
        assert_eq!(value["source"], "codex");
        assert_eq!(value["primary"]["usedPercent"], 42.0);
        assert_eq!(value["primary"]["windowMinutes"], 300);
        assert!(value["secondary"].is_null());
        assert!(super::limits_json(None, "codex").is_null());
    }

    #[test]
    fn usage_update_reports_latest_prompt_and_session_totals() {
        let mut state = crate::app::AppState::new();
        state.history.replace(Vec::new());
        state.history.push(ChatMessage::new("user", "hello"));
        state.history.push(assistant(1_000, 200, Some(400)));
        state.history.push(ChatMessage::new("user", "again"));
        state.history.push(assistant(1_500, 300, None));
        state.current_token_usage = Some(TokenUsage {
            prompt_tokens: 1_500,
            completion_tokens: 300,
            total_tokens: 1_800,
            ..Default::default()
        });

        let value = serde_json::to_value(usage_update(&state)).unwrap();
        assert_eq!(value["sessionUpdate"], "usage_update");
        assert_eq!(value["used"], 1_500);
        assert_eq!(value["size"], u64::from(state.active_context_window()));
        assert!(value.get("cost").is_none());
        let meta = &value["_meta"]["rustcode"];
        assert_eq!(meta["kind"], "usage");
        assert_eq!(meta["version"], USAGE_META_VERSION);
        assert_eq!(meta["estimated"], false);
        assert_eq!(meta["inputTokens"], 2_500);
        assert_eq!(meta["outputTokens"], 500);
        assert_eq!(meta["cachedTokens"], 400);
        assert_eq!(meta["responses"], 2);
    }

    #[test]
    fn compaction_card_carries_the_numbers_and_the_summary() {
        let stats = crate::app::CompactionStats::new(
            crate::app::CompactionTrigger::Manual,
            crate::app::CompactionMode::AiSummary,
            161_495,
            4_200,
            116,
            1,
        );
        let value = serde_json::to_value(compaction_card(Some(&stats), Some("- Goal: ship"), None))
            .unwrap();
        assert_eq!(value["sessionUpdate"], "agent_message_chunk");
        assert_eq!(
            value["content"]["text"],
            "History compacted: reduced context from 161495 to 4200 tokens."
        );
        let meta = &value["_meta"]["rustcode"];
        assert_eq!(meta["kind"], "compaction");
        assert_eq!(meta["version"], COMPACTION_META_VERSION);
        assert_eq!(meta["trigger"], "manual");
        assert_eq!(meta["mode"], "ai_summary");
        assert_eq!(meta["beforeTokens"], 161_495);
        assert_eq!(meta["afterTokens"], 4_200);
        assert_eq!(meta["count"], 2);
        assert_eq!(meta["summary"], "- Goal: ship");

        let unknown = serde_json::to_value(compaction_card(None, None, None)).unwrap();
        assert_eq!(unknown["content"]["text"], "History compacted.");
        assert!(unknown["_meta"]["rustcode"]["count"].is_null());
    }

    /// The provider's last usage measured the prompt before a compaction;
    /// once the history is compacted the meter estimates the new history
    /// instead of showing the old size.
    #[test]
    fn compaction_drops_the_stale_provider_usage() {
        let mut state = crate::app::AppState::new();
        state.history.replace(Vec::new());
        state
            .history
            .push(ChatMessage::new("system", "a".repeat(4_000)));
        state.current_token_usage = Some(TokenUsage {
            prompt_tokens: 161_495,
            completion_tokens: 300,
            total_tokens: 161_795,
            ..Default::default()
        });

        state.record_compaction(
            crate::app::CompactionTrigger::Manual,
            crate::app::CompactionMode::AiSummary,
            161_495,
            1_000,
            116,
        );

        assert!(state.current_token_usage.is_none());
        let value = serde_json::to_value(usage_update(&state)).unwrap();
        assert_eq!(value["used"], 1_000);
        assert_eq!(value["_meta"]["rustcode"]["estimated"], true);
        assert_eq!(
            value["_meta"]["rustcode"]["context"]["compaction"]["count"],
            1
        );
        assert!(state.pending_compaction_card.is_some());
    }

    /// The count goes on across a reload, so a fourth compaction is not
    /// shown as the first.
    #[test]
    fn compaction_count_survives_a_reload() {
        let session_id = crate::config::create_detached_session();
        let mut state = crate::app::AppState::new();
        state.active_session_id = session_id.clone();
        for _ in 0..2 {
            state.record_compaction(
                crate::app::CompactionTrigger::Auto,
                crate::app::CompactionMode::AiSummary,
                100_000,
                5_000,
                40,
            );
        }

        let mut reopened = crate::app::AppState::new();
        reopened.active_session_id = session_id;
        reopened.restore_session_compaction();

        let restored = reopened.last_compaction.expect("restored compaction");
        assert_eq!(restored.count, 2);
        assert_eq!(restored.before_tokens, 100_000);
        assert_eq!(restored.trigger, crate::app::CompactionTrigger::Auto);
        assert!(
            reopened.pending_compaction_card.is_none(),
            "a reload replays the card from history, not as a new one"
        );
    }

    #[test]
    fn loaded_session_without_current_request_estimates_from_history() {
        let mut state = crate::app::AppState::new();
        state.history.replace(Vec::new());
        state.current_token_usage = None;
        state
            .history
            .push(ChatMessage::new("user", "a".repeat(400)));
        state.history.push(assistant(9_000, 10, None));

        let value = serde_json::to_value(usage_update(&state)).unwrap();
        // 400 + 5 characters at four per token, not the saved 9 000 prompt.
        assert_eq!(value["used"], 101);
        assert_eq!(value["_meta"]["rustcode"]["estimated"], true);
        assert_eq!(value["_meta"]["rustcode"]["inputTokens"], 9_000);
    }

    #[test]
    fn context_breakdown_rides_on_usage_and_the_command_reply() {
        let mut state = crate::app::AppState::new();
        state.history.replace(Vec::new());
        state
            .history
            .push(ChatMessage::new("user", "a".repeat(400)));
        let value = serde_json::to_value(usage_update(&state)).unwrap();
        let context = &value["_meta"]["rustcode"]["context"];
        assert_eq!(context["version"], 2);
        assert_eq!(context["window"], u64::from(state.active_context_window()));
        assert_eq!(context["used"], 100);
        assert_eq!(context["estimated"], true);
        // No compaction has run in a fresh session.
        assert!(context["compaction"].is_null());
        let categories = context["categories"].as_array().unwrap();
        assert_eq!(categories.len(), 8);
        assert_eq!(categories[3]["name"], "Compacted history");
        assert_eq!(categories[0]["name"], "User messages");
        assert!(categories[0]["tokens"].as_u64().unwrap() > 0);
        assert_eq!(
            context["free"].as_u64().unwrap(),
            u64::from(state.active_context_window()) - 100
        );

        let meta = context_reply_meta(&state);
        assert_eq!(meta["rustcode"]["kind"], "context");
        assert_eq!(meta["rustcode"]["context"], *context);
    }
}
