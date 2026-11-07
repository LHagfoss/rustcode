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
    /// Root requests carry the Smart UI hint (#2484).
    pub smart_ui: crate::app::SmartUi,
    /// The last compaction applied to the active history, when the frontend
    /// has one to show. Reporting only; it does not change any number above.
    pub compaction: Option<crate::app::CompactionStats>,
}

/// One thing inside a category: a message, a tool call, a skill or a subagent.
/// `tokens` is an estimate in the same unit as the category, not a provider
/// count; the client shows it the way it shows every other context number.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextItem {
    pub name: String,
    pub tokens: usize,
}

impl ContextItem {
    fn new(name: impl Into<String>, tokens: usize) -> Self {
        Self {
            name: name.into(),
            tokens,
        }
    }
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
    /// The biggest things in each category, in [`CONTEXT_CATEGORY_LABELS`]
    /// order, largest first. A client may show fewer; the counts stay in the
    /// same estimate as the category itself.
    pub items: [Vec<ContextItem>; 8],
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
        let mut breakdown = context_breakdown(ContextBreakdownInput {
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
            smart_ui: state.smart_ui,
            compaction: state.last_compaction.clone(),
        });
        // Subagent transcripts are held by the state, not the input: the root
        // view lists each child beside its own tokens.
        if selected.is_none() {
            breakdown.items[7] = state
                .subagents
                .iter()
                .filter_map(|agent| {
                    let tokens: usize = agent
                        .history
                        .iter()
                        .map(super::estimate_message_tokens)
                        .sum();
                    (tokens > 0).then(|| ContextItem {
                        name: if agent.name.is_empty() {
                            snippet(&agent.task)
                        } else {
                            agent.name.clone()
                        },
                        tokens,
                    })
                })
                .collect();
        }
        breakdown
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
    let mut user_items = Vec::new();
    let mut assistant_items = Vec::new();
    let mut tool_items: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
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
            "user" => {
                let tokens = super::estimate_tokens(&message.content);
                user_tokens += tokens;
                if tokens > 0 {
                    user_items.push(ContextItem::new(snippet(&message.content), tokens));
                }
            }
            "assistant" => {
                let tokens = super::estimate_tokens(&message.content);
                assistant_tokens += tokens;
                if tokens > 0 {
                    assistant_items.push(ContextItem::new(snippet(&message.content), tokens));
                }
                for call in &message.tool_calls {
                    let Ok(arguments) = serde_json::to_string(call) else {
                        continue;
                    };
                    let tokens = super::estimate_tokens(&arguments);
                    tool_tokens += tokens;
                    *tool_items
                        .entry(tool_label(&call.name, &call.arguments))
                        .or_default() += tokens;
                }
            }
            "tool" => {
                let mut tokens = super::estimate_tokens(&message.content);
                if let Some(id) = message.tool_call_id.as_deref() {
                    tokens += super::estimate_tokens(id);
                }
                tool_tokens += tokens;
                *tool_items.entry(tool_result_label(message)).or_default() += tokens;
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
        tools_prompt.push_str(&crate::tools::schema::output_style_hint(
            input.visualization_level,
            input.smart_ui,
        ));
        tools_prompt.push_str(&crate::tools::schema::taught_visualization_hint(
            input.visualization_level,
            input.smart_ui,
        ));
        tools_prompt.push_str(crate::tools::schema::smart_ui_hint(input.smart_ui));
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
    let mut items: [Vec<ContextItem>; 8] = std::array::from_fn(|_| Vec::new());
    items[0] = top_items(user_items);
    items[1] = top_items(assistant_items);
    items[2] = top_items(
        tool_items
            .into_iter()
            .map(|(name, tokens)| ContextItem::new(name, tokens))
            .collect(),
    );
    if compacted_history_tokens > 0 {
        items[3] = vec![ContextItem::new(
            "Compaction summary",
            compacted_history_tokens,
        )];
    }
    if system_prompt_tokens > 0 {
        items[4] = vec![ContextItem::new(
            "Instructions and hints",
            system_prompt_tokens,
        )];
    }
    items[5] = tool_schema_items(system_tools_tokens);
    items[6] = skill_items(&skills, skills_tokens);
    // Every request carries the system prompt, tools and skills, so an
    // estimate made before the provider counts one includes them. From the
    // transcript alone a new chat read as `~95 / 272k` while its first
    // request already held thousands of tokens.
    let mut current_usage = input.current_usage;
    if current_usage.source == ContextUsageSource::HistoryEstimate {
        current_usage.used_tokens = current_usage
            .used_tokens
            .saturating_add(u32::try_from(full_system_prompt_tokens).unwrap_or(u32::MAX));
    }

    ContextBreakdown {
        context_window,
        current_usage,
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
        remaining_tokens: context_window.saturating_sub(current_usage.used_tokens as usize),
        items,
    }
}

/// How many items one category carries: enough to name what dominates the
/// window, small enough for a card and its JSON.
const MAX_CONTEXT_ITEMS: usize = 12;

/// The first meaningful line of a message, bounded for a tile's list.
fn snippet(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    let mut shortened: String = line.chars().take(72).collect();
    if line.chars().count() > 72 {
        shortened.push('…');
    }
    if shortened.is_empty() {
        "(empty message)".to_owned()
    } else {
        shortened
    }
}

/// A tool call as a readable item: its name and the file, command or query it
/// named, so a reader can tell two reads of different files apart.
fn tool_label(name: &str, arguments: &str) -> String {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(arguments) else {
        return name.to_owned();
    };
    const TARGETS: [&str; 10] = [
        "path",
        "file_path",
        "file",
        "filename",
        "target_file",
        "target",
        "command",
        "pattern",
        "query",
        "url",
    ];
    let target = TARGETS
        .iter()
        .find_map(|key| value.get(*key).and_then(serde_json::Value::as_str))
        .map(snippet);
    match target {
        Some(target) => format!("{name} {target}"),
        None => name.to_owned(),
    }
}

/// A tool's result as an item: the tool and the file it changed, or its
/// command, since the result body itself is what the token count covers.
fn tool_result_label(message: &ChatMessage) -> String {
    let Some(record) = message.tool_result.as_ref() else {
        return "Tool result".to_owned();
    };
    if let Some(path) = record.changed_paths.first() {
        return format!("{} {path}", record.tool_name);
    }
    if let Some(command) = record.command.as_deref() {
        return format!("{} {}", record.tool_name, snippet(command));
    }
    if record.tool_name.is_empty() {
        "Tool result".to_owned()
    } else {
        record.tool_name.clone()
    }
}

/// Sort largest first, drop empty entries, and keep the list a card can show.
fn top_items(mut items: Vec<ContextItem>) -> Vec<ContextItem> {
    items.retain(|item| item.tokens > 0);
    items.sort_by(|a, b| b.tokens.cmp(&a.tokens).then(a.name.cmp(&b.name)));
    items.truncate(MAX_CONTEXT_ITEMS);
    items
}

/// The built-in tools' own definitions, as the prompt lists them. The
/// category also holds protocol guidance and examples, so the remainder is
/// named rather than folded into a tool or dropped.
fn tool_schema_items(category_tokens: usize) -> Vec<ContextItem> {
    let mut items = crate::tools::TOOLS
        .iter()
        .map(|tool| {
            let text = format!(
                "- {} | Args: {} | {}",
                tool.name, tool.arguments, tool.description
            );
            ContextItem::new(tool.name, super::estimate_tokens(&text))
        })
        .collect::<Vec<_>>();
    let listed: usize = items.iter().map(|item| item.tokens).sum();
    if category_tokens > listed {
        items.push(ContextItem::new(
            "Tool protocol and examples",
            category_tokens - listed,
        ));
    }
    top_items(items)
}

/// One item per discovered skill, with the remainder of the category named as
/// the headings and separators around them.
fn skill_items(skills: &[super::SkillInfo], category_tokens: usize) -> Vec<ContextItem> {
    let mut items = skills
        .iter()
        .map(|skill| {
            ContextItem::new(
                &skill.name,
                super::estimate_tokens(&format!("{} {}", skill.name, skill.description)),
            )
        })
        .collect::<Vec<_>>();
    let listed: usize = items.iter().map(|item| item.tokens).sum();
    if category_tokens > listed {
        items.push(ContextItem::new("Skill headings", category_tokens - listed));
    }
    top_items(items)
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
        // The transcript's two tokens plus what every request carries.
        let overhead = breakdown.system_prompt_tokens
            + breakdown.system_tools_tokens
            + breakdown.skills_tokens;
        assert!(overhead > 0);
        assert_eq!(
            breakdown.current_usage,
            ContextUsage {
                used_tokens: (2 + overhead) as u32,
                source: ContextUsageSource::HistoryEstimate,
            }
        );
        assert_eq!(
            breakdown.remaining_tokens,
            breakdown.context_window - 2 - overhead
        );
    }

    #[test]
    fn each_category_names_its_biggest_items() {
        let mut state = AppState::new();
        state.history.replace(Vec::new());
        state
            .history
            .push(ChatMessage::new("user", "Fix the composer context card"));
        let mut assistant = ChatMessage::new("assistant", "I will read the composer first.");
        assistant.tool_calls.push(rustcode_core::ToolCallRef {
            id: "call-1".to_owned(),
            name: "view_file".to_owned(),
            arguments: r#"{"path":"apps/desktop/src/ui/ComposerUsage.tsx"}"#.to_owned(),
        });
        state.history.push(assistant);

        let breakdown = ContextBreakdown::from_state(&state);
        let [user, agent, tools, _, _, schemas, skills, _] = &breakdown.items;
        assert_eq!(user[0].name, "Fix the composer context card");
        assert_eq!(agent[0].name, "I will read the composer first.");
        assert!(
            tools.iter().any(|item| item
                .name
                .contains("view_file apps/desktop/src/ui/ComposerUsage.tsx")),
            "{tools:?}"
        );
        assert!(schemas.iter().any(|item| item.name == "view_file"));
        assert!(skills.iter().all(|item| item.tokens > 0));
    }

    #[test]
    fn markdown_report_has_a_bar_a_category_table_and_reserves() {
        let mut state = AppState::new();
        state
            .history
            .push(ChatMessage::new("user", "x".repeat(4_000)));
        let breakdown = ContextBreakdown::from_state(&state);
        let report = context_report_markdown(&breakdown, Some("m"));
        assert!(report.contains("Model: `m`"));
        let used = format_token_count(breakdown.current_usage.used_tokens as usize);
        assert!(report.contains(&format!("**Used {used} of")), "{report}");
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
