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
pub(crate) const CONTEXT_META_VERSION: u32 = 1;

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
        "reserves": breakdown.configured_guards.map(|guards| serde_json::json!({
            "output": guards.output_tokens,
            "thinking": guards.thinking_tokens,
            "tools": guards.tool_tokens,
            "safety": guards.safety_tokens,
            "providerOverhead": guards.provider_framing_tokens,
        })),
    })
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

#[cfg(test)]
mod tests {
    use super::{USAGE_META_VERSION, context_reply_meta, usage_update};
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
        assert_eq!(context["version"], 1);
        assert_eq!(context["window"], u64::from(state.active_context_window()));
        assert_eq!(context["used"], 100);
        assert_eq!(context["estimated"], true);
        let categories = context["categories"].as_array().unwrap();
        assert_eq!(categories.len(), 7);
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
