//! Presentation-only cells for the mutable end of the conversation.
//!
//! Codex keeps one active history cell and mutates it as tool events arrive.
//! RustCode's canonical history remains `ChatMessage`; this small projection
//! gives the TUI the same lifecycle shape without serializing terminal state or
//! making provider code depend on ratatui.

use crate::app::{ChatMessage, LiveToolCall, Verbosity};
use ratatui::{
    style::Modifier,
    text::{Line, Span},
};
use std::cell::RefCell;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::{
    COLOR_BG, COLOR_MUTED, COLOR_PRIMARY, COLOR_TEXT, COLOR_TIP,
    get_themed_style, highlight_shell_command,
};

const MAX_LIVE_CHILDREN: usize = 8;

/// Presentation cells keep semantic source separate from terminal rows.
/// Replaying a cell at a new width therefore re-renders the same Markdown
/// instead of trying to resize already-wrapped ANSI-like output.
pub(super) trait HistoryCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>>;
}

enum ActiveHistoryCell {
    Assistant(AssistantMarkdownCell),
    Tools(LiveToolCell),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ActiveHistoryCellKind {
    Assistant,
    Tools,
}

impl ActiveHistoryCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        match self {
            Self::Assistant(cell) => cell.display_lines(width),
            Self::Tools(cell) => cell.display_lines(width),
        }
    }
}

/// Presentation-only transcript state for the one mutable item at the end of
/// the TUI transcript.
///
/// Codex keeps this active cell separate from finalized history and replaces
/// its contents on deltas instead of appending duplicate terminal rows. The
/// source and tool summaries here are intentionally not serialized or passed
/// to providers; [`AppState`] remains the canonical conversation boundary.
#[derive(Default)]
pub(crate) struct TranscriptState {
    active: Option<ActiveHistoryCell>,
    active_key: Option<ActiveHistoryCellKind>,
    revision: u64,
    model: super::TranscriptModel,
}

impl TranscriptState {
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }

    pub(crate) fn sync_model(&mut self, history: &[ChatMessage], live_text: &str) {
        self.model.sync_history(history);
        self.model.replace_live_text(live_text);
    }

    pub(crate) fn apply_agent_event(
        &mut self,
        event: &crate::network::ui_adapter::AgentUiEvent,
    ) {
        self.model.apply_agent_event(event);
    }

    pub(crate) fn model(&self) -> &super::TranscriptModel {
        &self.model
    }

    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    pub(crate) fn set_assistant(
        &mut self,
        source: &str,
        continuation: bool,
        response_time_ms: Option<u64>,
        thought_time_ms: Option<u64>,
        thought_tokens: Option<u32>,
    ) {
        let changed = self.active_key != Some(ActiveHistoryCellKind::Assistant)
            || match self.active.as_ref() {
                Some(ActiveHistoryCell::Assistant(cell)) => {
                    cell.source != source
                        || cell.continuation != continuation
                        || cell.response_time_ms != response_time_ms
                        || cell.thought_time_ms != thought_time_ms
                        || cell.thought_tokens != thought_tokens
                }
                _ => true,
            };
        if changed {
            self.revision = self.revision.saturating_add(1);
            self.active = Some(ActiveHistoryCell::Assistant(
                AssistantMarkdownCell::streaming(
                    source,
                    continuation,
                    response_time_ms,
                    thought_time_ms,
                    thought_tokens,
                ),
            ));
        }
        self.active_key = Some(ActiveHistoryCellKind::Assistant);
    }

    pub(crate) fn set_tools(&mut self, calls: &[LiveToolCall]) {
        let changed = self.active_key != Some(ActiveHistoryCellKind::Tools)
            || match self.active.as_ref() {
                Some(ActiveHistoryCell::Tools(cell)) => cell.calls != calls,
                _ => true,
            };
        if changed {
            self.revision = self.revision.saturating_add(1);
            self.active = Some(ActiveHistoryCell::Tools(LiveToolCell {
                calls: calls.to_vec(),
                verbosity: Verbosity::Low,
            }));
        }
        self.active_key = Some(ActiveHistoryCellKind::Tools);
    }

    pub(crate) fn set_tools_with_verbosity(
        &mut self,
        calls: &[LiveToolCall],
        verbosity: &Verbosity,
    ) {
        let changed = self.active_key != Some(ActiveHistoryCellKind::Tools)
            || match self.active.as_ref() {
                Some(ActiveHistoryCell::Tools(cell)) => {
                    cell.calls != calls || cell.verbosity != *verbosity
                }
                _ => true,
            };
        if changed {
            self.revision = self.revision.saturating_add(1);
            self.active = Some(ActiveHistoryCell::Tools(LiveToolCell {
                calls: calls.to_vec(),
                verbosity: verbosity.clone(),
            }));
        }
        self.active_key = Some(ActiveHistoryCellKind::Tools);
    }

    pub(crate) fn clear(&mut self) {
        if self.active.is_some() {
            self.revision = self.revision.saturating_add(1);
        }
        self.active = None;
        self.active_key = None;
    }

    pub(crate) fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        self.active
            .as_ref()
            .map(|cell| cell.display_lines(width))
            .unwrap_or_default()
    }
}

pub(super) struct AssistantMarkdownCell {
    pub(super) source: String,
    token_usage: Option<crate::app::TokenUsage>,
    pub(super) response_time_ms: Option<u64>,
    thought_time_ms: Option<u64>,
    thought_tokens: Option<u32>,
    generating: bool,
    pub(super) continuation: bool,
    cached_display: RefCell<Option<(u16, String, Vec<Line<'static>>)>>,
}

struct LiveToolCell {
    calls: Vec<LiveToolCall>,
    verbosity: Verbosity,
}

impl HistoryCell for LiveToolCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        render_live_tool_cell_with_verbosity(&self.calls, width, &self.verbosity, false)
    }
}

impl AssistantMarkdownCell {
    pub(super) fn committed(
        source: &str,
        token_usage: Option<crate::app::TokenUsage>,
        response_time_ms: Option<u64>,
        thought_time_ms: Option<u64>,
        thought_tokens: Option<u32>,
    ) -> Self {
        Self {
            source: source.to_owned(),
            token_usage,
            response_time_ms,
            thought_time_ms,
            thought_tokens,
            generating: false,
            continuation: false,
            cached_display: RefCell::new(None),
        }
    }

    pub(super) fn streaming(
        source: &str,
        continuation: bool,
        response_time_ms: Option<u64>,
        thought_time_ms: Option<u64>,
        thought_tokens: Option<u32>,
    ) -> Self {
        Self {
            source: source.to_owned(),
            token_usage: None,
            response_time_ms,
            thought_time_ms,
            thought_tokens,
            generating: true,
            continuation,
            cached_display: RefCell::new(None),
        }
    }
}

impl HistoryCell for AssistantMarkdownCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let theme_name = super::theme::active_palette().name.to_owned();
        if let Some((cached_width, cached_theme, lines)) = self.cached_display.borrow().as_ref()
            && *cached_width == width
            && cached_theme == &theme_name
        {
            return lines.clone();
        }

        let mut lines = Vec::new();
        let mut copy_clicks = Vec::new();
        super::render_assistant_message(
            &self.source,
            &mut lines,
            &mut copy_clicks,
            super::AssistantRenderOptions {
                token_usage: self.token_usage.clone(),
                response_time_ms: self.response_time_ms,
                thought_time_ms: self.thought_time_ms,
                thought_tokens: self.thought_tokens,
                is_generating: self.generating,
                viewport_width: width,
                show_picker: false,
                last_copy_text: None,
            },
        );
        if self.continuation {
            super::demote_assistant_bullet(&mut lines);
        }
        if self.generating {
            while lines.last().is_some_and(|line| line.spans.is_empty()) {
                lines.pop();
            }
        }
        let lines = lines
            .into_iter()
            .map(|line| super::own_line(&line))
            .collect::<Vec<_>>();
        self.cached_display
            .replace(Some((width, theme_name, lines.clone())));
        lines
    }
}

fn is_exploration_tool(name: &str) -> bool {
    crate::app::activity::is_exploration_tool(name)
}

fn truncate_to_width(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_owned();
    }
    let suffix = '…';
    let budget = width.saturating_sub(1);
    let mut output = String::new();
    let mut used = 0;
    for character in text.chars() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if used + character_width > budget {
            break;
        }
        used += character_width;
        output.push(character);
    }
    if width > 0 {
        output.push(suffix);
    }
    output
}

/// Render the single mutable live tool cell shown at the end of the transcript.
///
/// The cell deliberately contains only a bounded invocation summary. Tool
/// output belongs to the finalized semantic result and is rendered by the
/// existing verbosity-aware result cells once execution completes.
pub(super) fn render_live_tool_cell(
    calls: &[LiveToolCall],
    width: u16,
    show_picker: bool,
) -> Vec<Line<'static>> {
    render_live_tool_cell_with_verbosity(calls, width, &Verbosity::Low, show_picker)
}

pub(super) fn render_live_tool_cell_with_verbosity(
    calls: &[LiveToolCall],
    width: u16,
    verbosity: &Verbosity,
    show_picker: bool,
) -> Vec<Line<'static>> {
    if calls.is_empty() || width == 0 {
        return Vec::new();
    }

    if calls.len() == 1 && calls[0].tool_name == "run_command" {
        let call = &calls[0];
        let title_style = get_themed_style(
            COLOR_TEXT(),
            COLOR_BG(),
            Modifier::BOLD,
            show_picker,
        );
        let command = truncate_to_width(&call.target, (width as usize).saturating_sub(10).max(1));
        let command_spans = highlight_shell_command(&command, COLOR_BG(), show_picker)
            .into_iter()
            .next()
            .map(|line| line.spans)
            .unwrap_or_default();
        let mut header = vec![
            Span::styled(
                "• ",
                get_themed_style(
                    COLOR_MUTED(),
                    COLOR_BG(),
                    Modifier::empty(),
                    show_picker,
                ),
            ),
            Span::styled("Running ", title_style),
        ];
        header.extend(command_spans);
        let mut lines = vec![Line::from(header)];

        if matches!(verbosity, Verbosity::High) {
            return lines;
        }

        let mut output = Vec::<(String, bool)>::new();
        for chunk in &call.output {
            let clean = crate::network::text::strip_ansi_escapes(&chunk.text);
            output.extend(
                clean
                    .split('\n')
                    .filter(|line| !line.is_empty())
                    .map(|line| (line.to_owned(), chunk.stderr)),
            );
        }
        const MAX_PREVIEW_LINES: usize = 5;
        let omitted_lines = output.len().saturating_sub(MAX_PREVIEW_LINES);
        let visible = if omitted_lines == 0 {
            output
        } else {
            output[..2]
                .iter()
                .chain(output[output.len() - 2..].iter())
                .cloned()
                .collect()
        };
        for (index, (text, stderr)) in visible.into_iter().enumerate() {
            if omitted_lines > 0 && index == 2 {
                lines.push(Line::from(Span::styled(
                    format!("    … +{omitted_lines} lines"),
                    get_themed_style(
                        COLOR_MUTED(),
                        COLOR_BG(),
                        Modifier::ITALIC,
                        show_picker,
                    ),
                )));
            }
            lines.push(Line::from(vec![
                Span::styled(
                    if lines.len() == 1 { "  └ " } else { "    " },
                    get_themed_style(
                        COLOR_MUTED(),
                        COLOR_BG(),
                        Modifier::empty(),
                        show_picker,
                    ),
                ),
                Span::styled(
                    truncate_to_width(&text, (width as usize).saturating_sub(4).max(1)),
                    get_themed_style(
                        if stderr { COLOR_TIP() } else { COLOR_MUTED() },
                        COLOR_BG(),
                        Modifier::empty(),
                        show_picker,
                    ),
                ),
            ]));
        }
        if call.omitted_output_bytes > 0 {
            lines.push(Line::from(Span::styled(
                format!("  └ … {} earlier bytes omitted", call.omitted_output_bytes),
                get_themed_style(
                    COLOR_MUTED(),
                    COLOR_BG(),
                    Modifier::ITALIC,
                    show_picker,
                ),
            )));
        }
        return lines;
    }

    let all_exploration = calls.iter().all(|call| is_exploration_tool(&call.tool_name));
    let has_command = calls.iter().any(|call| call.tool_name == "run_command");
    if !all_exploration && !has_command && calls.len() == 1 {
        let call = &calls[0];
        let target = if call.target.is_empty() || call.target == "?" {
            String::new()
        } else {
            format!(" {}", truncate_to_width(&call.target, (width as usize).saturating_sub(4)))
        };
        let title_style = get_themed_style(
            COLOR_PRIMARY(),
            COLOR_BG(),
            Modifier::BOLD,
            show_picker,
        );
        return vec![Line::from(vec![
            Span::styled("• ", title_style),
            Span::styled(call.action.clone(), title_style),
            Span::styled(
                target,
                get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::empty(), show_picker),
            ),
        ])];
    }
    let label = if all_exploration {
        "Exploring"
    } else if has_command {
        "Running"
    } else {
        "Calling"
    };
    let title_style = get_themed_style(
        COLOR_PRIMARY(),
        COLOR_BG(),
        Modifier::BOLD,
        show_picker,
    );
    let mut lines = vec![Line::from(vec![
        Span::styled("• ", title_style),
        Span::styled(label, title_style),
    ])];

    let child_width = (width as usize).saturating_sub(6).max(1);
    for (index, call) in calls.iter().take(MAX_LIVE_CHILDREN).enumerate() {
        let prefix = if index == 0 { "  └ " } else { "    " };
        let mut spans = vec![
            Span::styled(
                prefix,
                get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
            ),
            Span::styled(
                call.action.clone(),
                get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::BOLD, show_picker),
            ),
        ];
        if !call.target.is_empty() && call.target != "?" {
            if call.action == "Bash" {
                spans.push(Span::styled(
                    " $ ",
                    get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::empty(), show_picker),
                ));
                let command =
                    truncate_to_width(&call.target, child_width.saturating_sub(2));
                if let Some(command_line) =
                    highlight_shell_command(&command, COLOR_BG(), show_picker).into_iter().next()
                {
                    spans.extend(command_line.spans);
                }
            } else {
                spans.push(Span::styled(
                    format!(" {}", truncate_to_width(&call.target, child_width)),
                    get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::empty(), show_picker),
                ));
            }
        }
        lines.push(Line::from(spans));
    }
    if calls.len() > MAX_LIVE_CHILDREN {
        lines.push(Line::from(Span::styled(
            format!("    … +{} more", calls.len() - MAX_LIVE_CHILDREN),
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::ITALIC, show_picker),
        )));
    }

    lines
}
