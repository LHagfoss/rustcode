//! The `/context` breakdown: how the active model's window is spent.
//!
//! One computation serves the TUI modal and the remote and ACP `/context`
//! reports, so every frontend shows the same numbers. Frontends build a
//! [`ContextBreakdownInput`] from the state they hold (the TUI from its render
//! snapshot, the engine from [`AppState`]) and render the result.

use crate::app::{AppState, ChatMessage};
use crate::config::{AgentMode, ModelProfile, ToolProtocol};

/// Where the headline usage figure comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextUsageSource {
    /// The provider's prompt count for the current request.
    ProviderPrompt,
    /// A quarter of the active history's byte length.
    HistoryEstimate,
}

impl ContextUsageSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::ProviderPrompt => "measured provider prompt",
            Self::HistoryEstimate => "saved history estimate",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContextUsage {
    pub used_tokens: u32,
    pub source: ContextUsageSource,
}

impl ContextUsage {
    /// Usage from the current request when the provider counted it, else an
    /// estimate from `history`. Completion tokens do not occupy the window.
    pub fn new(provider_prompt_tokens: Option<u32>, history: &[ChatMessage]) -> Self {
        match provider_prompt_tokens {
            Some(used_tokens) => Self {
                used_tokens,
                source: ContextUsageSource::ProviderPrompt,
            },
            None => Self {
                used_tokens: (history
                    .iter()
                    .map(|message| message.content.len())
                    .sum::<usize>()
                    / 4) as u32,
                source: ContextUsageSource::HistoryEstimate,
            },
        }
    }
}

/// Reserves the active profile keeps out of the window. They are configured
/// limits, not usage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfiguredContextGuards {
    pub output_tokens: u32,
    pub thinking_tokens: u32,
    pub tool_tokens: u32,
    pub safety_tokens: u32,
    pub provider_framing_tokens: u32,
}

/// What the breakdown is computed from.
pub struct ContextBreakdownInput<'a> {
    pub context_window: u32,
    pub current_usage: ContextUsage,
    pub active_profile: Option<&'a ModelProfile>,
    /// `Some` while a subagent's context is shown, holding its configured
    /// model when known. Reserves are only shown when it runs the active
    /// profile's model.
    pub selected_subagent_model: Option<Option<&'a str>>,
    /// The history in view: the root conversation or the selected subagent's.
    pub history: &'a [ChatMessage],
    pub tool_protocol: ToolProtocol,
    pub delegation_active: bool,
    pub agent_mode: AgentMode,
    /// Tokens held by subagent transcripts, counted only for the root view.
    pub subagent_tokens: usize,
    /// The `rustcode-viz` level the frontend renders; above 0, root requests
    /// carry the visualization hint for that level (#2088, #2217).
    pub visualization_level: u8,
    /// The last compaction applied to the active history, when the frontend
    /// has one to show. Reporting only; it does not change any number above.
    pub compaction: Option<crate::app::CompactionStats>,
}

#[derive(Debug, Clone)]
pub struct ContextBreakdown {
    pub context_window: usize,
    pub current_usage: ContextUsage,
    pub configured_guards: Option<ConfiguredContextGuards>,
    pub user_tokens: usize,
    pub assistant_tokens: usize,
    pub tool_tokens: usize,
    pub system_prompt_tokens: usize,
    pub system_tools_tokens: usize,
    pub skills_tokens: usize,
    pub subagent_tokens: usize,
    /// Tokens held by the compaction summary or deterministic record. These
    /// messages are `system`-role, so before this category existed they matched
    /// no arm below and silently under-reported the used total.
    pub compacted_history_tokens: usize,
    /// What the last compaction in this session did, if any.
    pub compaction: Option<crate::app::CompactionStats>,
    pub remaining_tokens: usize,
}

/// The category names, in legend order, shared by every frontend.
pub const CONTEXT_CATEGORY_LABELS: [&str; 8] = [
    "User messages",
    "Agent responses",
    "Tool calls",
    "Compacted history",
    "System prompt",
    "System tools",
    "Skills",
    "Subagents",
];

impl ContextBreakdown {
    /// The breakdown for the session `state` holds, as the TUI would show it.
    pub(crate) fn from_state(state: &AppState) -> Self {
        let selected = state
            .selected_subagent_id
            .map(|id| state.subagents.iter().find(|agent| agent.id == id));
        let history: &[ChatMessage] = match selected {
            Some(Some(agent)) => agent.history.as_slice(),
            _ => state.history.as_slice(),
        };
        let provider_prompt = selected
            .is_none()
            .then(|| state.current_token_usage.as_ref())
            .flatten()
            .map(|usage| usage.prompt_tokens);
        let profile = state.active_model_profile();
        let tool_protocol = state
            .config
            .models
            .iter()
            .find(|model| model.url == state.api_base_url)
            .and_then(|model| model.tool_protocol)
            .unwrap_or(state.config.tool_protocol);
        let subagent_tokens = if selected.is_none() {
            state
                .subagents
                .iter()
                .flat_map(|agent| agent.history.iter())
                .map(super::estimate_message_tokens)
                .sum()
        } else {
            0
        };
        context_breakdown(ContextBreakdownInput {
            context_window: state.active_context_window(),
            current_usage: ContextUsage::new(provider_prompt, history),
            active_profile: profile.as_ref(),
            selected_subagent_model: selected
                .map(|agent| agent.and_then(|agent| agent.model.as_deref())),
            history,
            tool_protocol,
            delegation_active: state.delegation_active,
            agent_mode: state.agent_mode,
            subagent_tokens,
            visualization_level: state.visualization_level,
            compaction: state.last_compaction.clone(),
        })
    }

    /// Each category's tokens, in [`CONTEXT_CATEGORY_LABELS`] order.
    pub fn category_tokens(&self) -> [usize; 8] {
        [
            self.user_tokens,
            self.assistant_tokens,
            self.tool_tokens,
            self.compacted_history_tokens,
            self.system_prompt_tokens,
            self.system_tools_tokens,
            self.skills_tokens,
            self.subagent_tokens,
        ]
    }

    /// `tokens` as a percentage of the window.
    pub fn percent(&self, tokens: usize) -> f64 {
        if self.context_window > 0 {
            tokens as f64 / self.context_window as f64 * 100.0
        } else {
            0.0
        }
    }
}

pub fn context_breakdown(input: ContextBreakdownInput<'_>) -> ContextBreakdown {
    let context_window = input.context_window as usize;
    let configured_guards = input.active_profile.and_then(|profile| {
        if let Some(selected_model) = input.selected_subagent_model {
            let selected_model = selected_model?;
            if !selected_model.eq_ignore_ascii_case(&profile.model)
                && !selected_model.eq_ignore_ascii_case(&profile.name)
            {
                return None;
            }
        }
        let budget = profile.context_budget();
        if budget.context_window != input.context_window {
            return None;
        }
        Some(ConfiguredContextGuards {
            output_tokens: budget.max_output_tokens,
            thinking_tokens: budget.thinking_reserve,
            tool_tokens: budget.tool_reserve,
            safety_tokens: budget.safety_reserve,
            provider_framing_tokens: budget.provider_overhead_margin,
        })
    });

    let (mut user_tokens, mut assistant_tokens, mut tool_tokens) = (0, 0, 0);
    let mut compacted_history_tokens = 0usize;
    for message in input.history {
        // The compaction summary and the deterministic record are `system`
        // messages. Count them here so the categories add up to the used total
        // instead of leaving the compacted prefix invisible.
        if message.compaction_boundary.is_some()
            || message
                .content
                .starts_with(crate::network::compaction::SUMMARY_MARKER)
            || message
                .content
                .starts_with(crate::network::compaction::STRUCTURED_MEMORY_MARKER)
        {
            compacted_history_tokens += super::estimate_tokens(&message.content);
            continue;
        }
        match message.role.as_str() {
            "user" => user_tokens += super::estimate_tokens(&message.content),
            "assistant" => {
                assistant_tokens += super::estimate_tokens(&message.content);
                if !message.tool_calls.is_empty()
                    && let Ok(calls) = serde_json::to_string(&message.tool_calls)
                {
                    tool_tokens += super::estimate_tokens(&calls);
                }
            }
            "tool" => {
                tool_tokens += super::estimate_tokens(&message.content);
                if let Some(id) = message.tool_call_id.as_deref() {
                    tool_tokens += super::estimate_tokens(id);
                }
            }
            _ => {}
        }
    }

    let mut tools_prompt = super::tool_system_prompt(
        input.delegation_active,
        input.tool_protocol,
        input.agent_mode,
    );
    // Subagent prompts never carry the hint.
    if input.selected_subagent_model.is_none() {
        tools_prompt.push_str(&super::visualization_prompt_hint(input.visualization_level));
    }
    let full_system_prompt_tokens = super::estimate_tokens(&tools_prompt);
    let skills = super::discover_skills();
    let skills_tokens = if skills.is_empty() {
        0
    } else {
        super::estimate_tokens(
            &skills
                .iter()
                .map(|skill| format!("{} {}", skill.name, skill.description))
                .collect::<Vec<_>>()
                .join(" "),
        )
    };
    let system_tools_tokens = full_system_prompt_tokens.saturating_sub(skills_tokens) / 2;
    let system_prompt_tokens = full_system_prompt_tokens
        .saturating_sub(system_tools_tokens)
        .saturating_sub(skills_tokens);

    ContextBreakdown {
        context_window,
        current_usage: input.current_usage,
        configured_guards,
        user_tokens,
        assistant_tokens,
        tool_tokens,
        system_prompt_tokens,
        system_tools_tokens,
        skills_tokens,
        subagent_tokens: input.subagent_tokens,
        compacted_history_tokens,
        compaction: input.compaction.clone(),
        remaining_tokens: context_window.saturating_sub(input.current_usage.used_tokens as usize),
    }
}

/// `1.2M`, `34.5k` or a plain count.
pub fn format_token_count(tokens: usize) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}k", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
    }
}

/// Cells in the text usage bar of the Markdown report.
const BAR_CELLS: usize = 30;

/// A Markdown `/context` report for clients that render Markdown: headline,
/// a text usage bar, one table row per category, and the reserves.
pub fn context_report_markdown(breakdown: &ContextBreakdown, model: Option<&str>) -> String {
    let used = breakdown.current_usage.used_tokens as usize;
    let used_percent = breakdown.percent(used);
    let filled = if breakdown.context_window == 0 {
        0
    } else {
        ((used as f64 / breakdown.context_window as f64) * BAR_CELLS as f64)
            .round()
            .clamp(0.0, BAR_CELLS as f64) as usize
    };
    let mut out = String::new();
    if let Some(model) = model {
        out.push_str(&format!("Model: `{model}`  \n"));
    }
    out.push_str(&format!(
        "**Used {} of {} tokens · {:.0}%** ({})\n\n",
        format_token_count(used),
        format_token_count(breakdown.context_window),
        used_percent,
        breakdown.current_usage.source.label(),
    ));
    out.push_str(&format!(
        "`{}{}` {:.0}% free\n\n",
        "█".repeat(filled),
        "░".repeat(BAR_CELLS - filled),
        breakdown.percent(breakdown.remaining_tokens),
    ));
    out.push_str("| Category | Tokens | % of window |\n|---|---:|---:|\n");
    for (label, tokens) in CONTEXT_CATEGORY_LABELS
        .iter()
        .zip(breakdown.category_tokens())
    {
        out.push_str(&format!(
            "| {label} | {} | {:.1}% |\n",
            format_token_count(tokens),
            breakdown.percent(tokens)
        ));
    }
    out.push_str(&format!(
        "| Free | {} | {:.1}% |\n\n",
        format_token_count(breakdown.remaining_tokens),
        breakdown.percent(breakdown.remaining_tokens)
    ));
    out.push_str("Categories are estimated from the saved history.\n\n");
    if let Some(compaction) = breakdown.compaction.as_ref() {
        out.push_str(&format!(
            "**Last compaction ({}, {})**: {} → {} tokens, {} reclaimed ({} messages considered)\n\n",
            compaction.trigger.label(),
            compaction.mode.label(),
            format_token_count(compaction.before_tokens),
            format_token_count(compaction.after_tokens),
            format_token_count(compaction.reclaimed_tokens),
            compaction.messages_considered,
        ));
        if compaction.mode.is_reduced() {
            out.push_str(
                "This provider has no plain summarizer endpoint, so `/compact` writes a \
                 reduced deterministic record rather than a written summary. Exact messages \
                 stay recoverable with `zoom_context`.\n\n",
            );
        }
        if compaction.is_repeated() {
            out.push_str(&format!(
                "This session has compacted {} times. Repeated compaction over a long session \
                 makes the model less accurate; start a new chat when the task is finished.\n\n",
                compaction.count
            ));
        }
    }
    match breakdown.configured_guards {
        Some(guards) => out.push_str(&format!(
            "Configured reserves (not usage): output {} incl. thinking {} · tools {} · safety {} · provider overhead {}",
            format_token_count(guards.output_tokens as usize),
            format_token_count(guards.thinking_tokens as usize),
            format_token_count(guards.tool_tokens as usize),
            format_token_count(guards.safety_tokens as usize),
            format_token_count(guards.provider_framing_tokens as usize),
        )),
        None => out.push_str("Configured reserves are unavailable for the active model."),
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breakdown_counts_history_roles_and_uses_the_provider_prompt() {
        let mut state = AppState::new();
        state.history.push(ChatMessage::new("user", "hello there"));
        state
            .history
            .push(ChatMessage::new("assistant", "general kenobi"));
        state.current_token_usage = Some(crate::app::TokenUsage {
            prompt_tokens: 1_000,
            completion_tokens: 10,
            total_tokens: 1_010,
            ..Default::default()
        });
        let breakdown = ContextBreakdown::from_state(&state);
        assert_eq!(breakdown.current_usage.used_tokens, 1_000);
        assert_eq!(
            breakdown.current_usage.source,
            ContextUsageSource::ProviderPrompt
        );
        assert_eq!(
            breakdown.user_tokens,
            super::super::estimate_tokens("hello there")
        );
        assert_eq!(
            breakdown.assistant_tokens,
            super::super::estimate_tokens("general kenobi")
        );
        assert_eq!(breakdown.remaining_tokens, breakdown.context_window - 1_000);
        assert!(breakdown.system_prompt_tokens > 0);
    }

    #[test]
    fn without_provider_usage_the_headline_is_a_history_estimate() {
        let mut state = AppState::new();
        state.history.push(ChatMessage::new("user", "abcdefgh"));
        let breakdown = ContextBreakdown::from_state(&state);
        assert_eq!(
            breakdown.current_usage,
            ContextUsage {
                used_tokens: 2,
                source: ContextUsageSource::HistoryEstimate,
            }
        );
    }

    #[test]
    fn markdown_report_has_a_bar_a_category_table_and_reserves() {
        let mut state = AppState::new();
        state
            .history
            .push(ChatMessage::new("user", "x".repeat(4_000)));
        let report = context_report_markdown(&ContextBreakdown::from_state(&state), Some("m"));
        assert!(report.contains("Model: `m`"));
        assert!(report.contains("**Used 1.0k of"));
        assert!(report.contains('█') || report.contains('░'));
        assert!(report.contains("| Category | Tokens | % of window |"));
        for label in CONTEXT_CATEGORY_LABELS {
            assert!(report.contains(&format!("| {label} |")), "{label}");
        }
        assert!(report.contains("| Free |"));
        assert!(report.contains("reserves"));
    }

    #[test]
    fn token_counts_format_compactly() {
        assert_eq!(format_token_count(999), "999");
        assert_eq!(format_token_count(1_500), "1.5k");
        assert_eq!(format_token_count(2_500_000), "2.5M");
    }
}
