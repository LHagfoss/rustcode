//! Presentation-only cells for the mutable end of the conversation.
//!
//! Codex keeps one active history cell and mutates it as tool events arrive.
//! RustCode's canonical history remains `ChatMessage`; this small projection
//! gives the TUI the same lifecycle shape without serializing terminal state or
//! making provider code depend on ratatui.

use ratatui::{
    style::Modifier,
    text::{Line, Span},
};
use rustcode::controller::{History, LiveToolCall, Verbosity};
use std::cell::RefCell;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::{
    COLOR_BG, COLOR_MUTED, COLOR_PRIMARY, COLOR_TEXT, COLOR_TIP, get_themed_style,
    highlight_shell_command,
};

const MAX_LIVE_CHILDREN: usize = 8;

/// Presentation cells keep semantic source separate from terminal rows.
/// Replaying a cell at a new width therefore re-renders the same Markdown
/// instead of trying to resize already-wrapped ANSI-like output.
pub(super) trait HistoryCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>>;
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
    assistant: Option<AssistantMarkdownCell>,
    tools: Option<LiveToolCell>,
    revision: u64,
    history_revision: Option<u64>,
    model: super::TranscriptModel,
    scroll_rows: usize,
    pub(super) reading_anchor: Option<ReadingAnchor>,
    pub(crate) selection: super::selection::TranscriptSelection,
    committed_cache:
        Option<super::lru::LruCache<(u64, u64, usize, u16, u64), Arc<Vec<Line<'static>>>>>,
}

/// The committed tail at the last painted reading viewport. A fixed offset
/// from the bottom would move the reader when new rows arrive below them.
#[derive(Clone, Copy)]
pub(super) struct ReadingAnchor {
    pub(super) width: u16,
    pub(super) height: u16,
    pub(super) display_start: usize,
    pub(super) history_revision: u64,
    pub(super) history_len: usize,
    pub(super) tail_start: usize,
    pub(super) tail_rows: usize,
}

/// Rows the mouse wheel moves per tick.
///
/// A discrete wheel notch arrives as a single scroll event, and the scrollback
/// a user is used to moves three lines per notch (Ghostty's
/// `mouse-scroll-multiplier` defaults to 3 for discrete devices), so one line
/// per tick is three times slower than the terminal's own scrolling. It is
/// also the expensive choice: a frame's cost is flat in the rows it moves —
/// `bench_wheel_step_cost` measures the same ~0.8 ms at 1, 3 and 6 rows in
/// release — so three rows per tick buys three times the travel for the price
/// of one frame.
///
/// Keyboard scrolling is untouched. `PageUp`/`PageDown` still step a page, the
/// selection caret still walks one row at a time, and `step_selection_scroll`
/// must keep advancing a single row per frame so that every row a scroll
/// crosses is captured before it leaves the screen.
pub(crate) const WHEEL_SCROLL_LINES: usize = 3;

impl TranscriptState {
    pub(crate) fn scroll_up(&mut self, rows: usize) {
        self.scroll_rows = self.scroll_rows.saturating_add(rows).min(10_000);
    }

    pub(crate) fn scroll_down(&mut self, rows: usize) {
        self.scroll_rows = self.scroll_rows.saturating_sub(rows);
        if self.scroll_rows == 0 {
            self.reading_anchor = None;
        }
    }

    /// Advance at most one selected row before painting so every crossed row is cached.
    pub(crate) fn step_selection_scroll(&mut self) -> bool {
        let before = self.scroll_rows;
        let Some(direction) = self.selection.take_scroll_step(before) else {
            return false;
        };
        if direction < 0 {
            self.scroll_up(1);
        } else {
            self.scroll_down(1);
        }
        if self.scroll_rows == before {
            self.selection.cancel_pending_scroll();
            return false;
        }
        true
    }

    pub(crate) fn scroll_rows(&self) -> usize {
        self.scroll_rows
    }

    pub(crate) fn clamp_scroll_rows(&mut self, maximum: usize) -> usize {
        self.scroll_rows = self.scroll_rows.min(maximum);
        if self.scroll_rows == 0 {
            self.reading_anchor = None;
        }
        self.scroll_rows
    }

    pub(super) fn shift_reading_offset(&mut self, delta: isize) {
        self.scroll_rows = if delta >= 0 {
            self.scroll_rows.saturating_add(delta as usize)
        } else {
            self.scroll_rows.saturating_sub(delta.unsigned_abs())
        };
    }

    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }

    pub(crate) fn sync_model(&mut self, history: &History, live_text: &str) {
        if self.history_revision != Some(history.revision()) {
            // Recovery notices remain in canonical/provider history, but are
            // implementation details and must not become transcript cells.
            let visible_history = history
                .iter()
                .filter(|message| {
                    !(message.role == "system" || message.role == "assistant")
                        || !crate::ui::is_hidden_system_notice(&message.content)
                })
                .cloned()
                .collect::<Vec<_>>();
            self.model.sync_history(&visible_history);
            self.history_revision = Some(history.revision());
        }
        self.model.replace_live_text(live_text);
    }

    pub(crate) fn apply_agent_event(&mut self, event: &rustcode::controller::AgentUiEvent) {
        self.model.apply_agent_event(event);
    }

    pub(crate) fn model(&self) -> &super::TranscriptModel {
        &self.model
    }

    pub(crate) fn committed_block(
        &mut self,
        state: &super::RenderSnapshot,
        index: usize,
        width: u16,
    ) -> Arc<Vec<Line<'static>>> {
        let mut theme_hash = std::collections::hash_map::DefaultHasher::new();
        super::theme::active_palette().name.hash(&mut theme_hash);
        let mut session_hash = std::collections::hash_map::DefaultHasher::new();
        state.active_session_id().hash(&mut session_hash);
        let key = (
            session_hash.finish(),
            state.history().revision(),
            index,
            width,
            theme_hash.finish(),
        );
        let cache = self
            .committed_cache
            .get_or_insert_with(|| super::lru::LruCache::new(4));
        if let Some(lines) = cache.get(&key) {
            return Arc::clone(lines);
        }
        let lines = Arc::new(super::render_committed_history_block_snapshot(
            state, index, width,
        ));
        cache.insert(key, Arc::clone(&lines));
        lines
    }

    #[cfg(test)]
    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    #[cfg(test)]
    fn history_revision(&self) -> Option<u64> {
        self.history_revision
    }

    pub(crate) fn set_assistant(
        &mut self,
        source: &str,
        continuation: bool,
        response_time_ms: Option<u64>,
        thought_time_ms: Option<u64>,
        thought_tokens: Option<u32>,
    ) {
        if let Some(cell) = self.assistant.as_mut()
            && cell.source == source
            && cell.continuation == continuation
        {
            let has_thought_preview = source.contains("<think>") || source.contains("</think>");
            let metadata_changed = cell.response_time_ms != response_time_ms
                || cell.thought_time_ms != thought_time_ms
                || cell.thought_tokens != thought_tokens;
            cell.response_time_ms = response_time_ms;
            cell.thought_time_ms = thought_time_ms;
            cell.thought_tokens = thought_tokens;
            if has_thought_preview && metadata_changed {
                cell.cached_display.replace(None);
                self.revision = self.revision.saturating_add(1);
            }
            return;
        }
        if let Some(cell) = self.assistant.as_mut()
            && cell.generating
            && cell.continuation == continuation
            && source.len() > cell.source.len()
            && source.starts_with(&cell.source)
        {
            cell.source.clear();
            cell.source.push_str(source);
            cell.response_time_ms = response_time_ms;
            cell.thought_time_ms = thought_time_ms;
            cell.thought_tokens = thought_tokens;
            cell.cached_display.replace(None);
            self.revision = self.revision.saturating_add(1);
            return;
        }
        let changed = self.assistant.as_ref().is_none_or(|cell| {
            cell.source != source
                || cell.continuation != continuation
                || cell.response_time_ms != response_time_ms
                || cell.thought_time_ms != thought_time_ms
                || cell.thought_tokens != thought_tokens
        });
        if changed {
            self.revision = self.revision.saturating_add(1);
            self.assistant = Some(AssistantMarkdownCell::streaming(
                source,
                continuation,
                response_time_ms,
                thought_time_ms,
                thought_tokens,
            ));
        }
    }

    #[cfg(test)]
    pub(crate) fn set_tools(&mut self, calls: &[LiveToolCall]) {
        let changed = self.tools.as_ref().is_none_or(|cell| cell.calls != calls);
        if changed {
            self.revision = self.revision.saturating_add(1);
            self.tools = Some(LiveToolCell {
                calls: calls.to_vec(),
                verbosity: Verbosity::Low,
            });
        }
    }

    pub(crate) fn set_tools_with_verbosity(
        &mut self,
        calls: &[LiveToolCall],
        verbosity: &Verbosity,
    ) {
        let changed = self
            .tools
            .as_ref()
            .is_none_or(|cell| cell.calls != calls || cell.verbosity != *verbosity);
        if changed {
            self.revision = self.revision.saturating_add(1);
            self.tools = Some(LiveToolCell {
                calls: calls.to_vec(),
                verbosity: verbosity.clone(),
            });
        }
    }

    pub(crate) fn clear_assistant(&mut self) {
        if self.assistant.take().is_some() {
            self.revision = self.revision.saturating_add(1);
        }
    }

    pub(crate) fn clear_tools(&mut self) {
        if self.tools.take().is_some() {
            self.revision = self.revision.saturating_add(1);
        }
    }

    #[cfg(test)]
    pub(crate) fn clear(&mut self) {
        if self.assistant.is_some() || self.tools.is_some() {
            self.revision = self.revision.saturating_add(1);
        }
        self.assistant = None;
        self.tools = None;
    }

    pub(crate) fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines = self
            .tools
            .as_ref()
            .map(|cell| cell.display_lines(width))
            .unwrap_or_default();
        let assistant_lines = self
            .assistant
            .as_ref()
            .map(|cell| cell.display_lines(width))
            .unwrap_or_default();
        if !lines.is_empty() && !assistant_lines.is_empty() {
            lines.push(Line::from(""));
        }
        lines.extend(assistant_lines);
        lines
    }
}

pub(super) struct AssistantMarkdownCell {
    pub(super) source: String,
    token_usage: Option<rustcode::controller::TokenUsage>,
    pub(super) response_time_ms: Option<u64>,
    thought_time_ms: Option<u64>,
    thought_tokens: Option<u32>,
    generating: bool,
    pub(super) continuation: bool,
    cached_display: RefCell<Option<(u16, String, Vec<Line<'static>>)>>,
    streaming_markdown: RefCell<Vec<super::markdown::StreamingMarkdownCache>>,
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
        token_usage: Option<rustcode::controller::TokenUsage>,
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
            streaming_markdown: RefCell::new(Vec::new()),
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
            streaming_markdown: RefCell::new(Vec::new()),
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
        let mut streaming_markdown = self.streaming_markdown.borrow_mut();
        super::render_assistant_message_with_cache(
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
            self.generating.then_some(&mut *streaming_markdown),
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
    rustcode_core::activity::is_exploration_tool(name)
}

fn is_editing_tool(name: &str) -> bool {
    rustcode_core::activity::is_editing_tool(name)
}

pub(super) fn is_live_tool_call_visible(call: &LiveToolCall) -> bool {
    call.execution_started || (!call.target.is_empty() && call.target != "?")
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
#[cfg(test)]
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
    let calls = calls
        .iter()
        .filter(|call| is_live_tool_call_visible(call))
        .collect::<Vec<_>>();
    if calls.is_empty() || width == 0 {
        return Vec::new();
    }

    // Speculative calls are only projections of the model's streamed output;
    // their lifecycle state does not change the category heading.
    let has_speculative = calls.iter().any(|call| !call.execution_started);

    if !has_speculative && calls.len() == 1 && calls[0].tool_name == "run_command" {
        let call = &calls[0];
        let title_style =
            get_themed_style(COLOR_PRIMARY(), COLOR_BG(), Modifier::BOLD, show_picker);
        let command = truncate_to_width(&call.target, (width as usize).saturating_sub(11).max(1));
        let command_spans = highlight_shell_command(&command, COLOR_BG(), show_picker)
            .into_iter()
            .next()
            .map(|line| line.spans)
            .unwrap_or_default();
        let header = vec![
            Span::styled("• ", title_style),
            Span::styled("Running", title_style),
        ];
        let mut invocation = vec![Span::styled(
            "  └ ",
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
        )];
        invocation.push(Span::styled(
            call.action.clone(),
            get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::BOLD, show_picker),
        ));
        invocation.push(Span::styled(
            " $ ",
            get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::empty(), show_picker),
        ));
        invocation.extend(command_spans);
        let mut lines = vec![Line::from(header), Line::from(invocation)];

        if matches!(verbosity, Verbosity::High) {
            return lines;
        }

        let mut output = Vec::<(String, bool)>::new();
        for chunk in &call.output {
            let clean = rustcode_tool_protocol::text::strip_ansi_escapes(&chunk.text);
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
                        Modifier::ITALIC | Modifier::DIM,
                        show_picker,
                    ),
                )));
            }
            lines.push(Line::from(vec![
                Span::styled(
                    "    ",
                    get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::DIM, show_picker),
                ),
                Span::styled(
                    truncate_to_width(&text, (width as usize).saturating_sub(4).max(1)),
                    get_themed_style(
                        if stderr { COLOR_TIP() } else { COLOR_MUTED() },
                        COLOR_BG(),
                        if stderr {
                            Modifier::empty()
                        } else {
                            Modifier::DIM
                        },
                        show_picker,
                    ),
                ),
            ]));
        }
        if call.omitted_output_bytes > 0 {
            lines.push(Line::from(Span::styled(
                format!("    … {} earlier bytes omitted", call.omitted_output_bytes),
                get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::ITALIC, show_picker),
            )));
        }
        return lines;
    }

    if !has_speculative && calls.len() == 1 && calls[0].tool_name == "render_video" {
        let call = &calls[0];
        let title_style =
            get_themed_style(COLOR_PRIMARY(), COLOR_BG(), Modifier::BOLD, show_picker);
        let child_width = (width as usize).saturating_sub(6).max(1);
        let child = if call.target.is_empty() || call.target == "?" {
            call.action.clone()
        } else {
            truncate_to_width(&call.target, child_width)
        };
        let title = if call.execution_started {
            "Running"
        } else {
            "Queued"
        };
        let mut lines = vec![
            Line::from(vec![
                Span::styled("• ", title_style),
                Span::styled(title, title_style),
            ]),
            Line::from(vec![
                Span::styled(
                    "  └ ",
                    get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
                ),
                Span::styled(
                    child,
                    get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::empty(), show_picker),
                ),
            ]),
        ];
        if !matches!(verbosity, Verbosity::High)
            && let Some(progress) = call
                .output
                .iter()
                .flat_map(|chunk| chunk.text.lines())
                .filter(|line| !line.trim().is_empty())
                .next_back()
        {
            lines.push(Line::from(vec![
                Span::styled(
                    "    ",
                    get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
                ),
                Span::styled(
                    truncate_to_width(progress.trim(), (width as usize).saturating_sub(4)),
                    get_themed_style(COLOR_TIP(), COLOR_BG(), Modifier::empty(), show_picker),
                ),
            ]));
        }
        return lines;
    }

    let all_exploration = calls
        .iter()
        .all(|call| is_exploration_tool(&call.tool_name));
    let all_editing = calls.iter().all(|call| is_editing_tool(&call.tool_name));
    // Queued projections have visible targets but no execution yet; show
    // Running/Exploring only after at least one call starts (#1495).
    let all_speculative = calls.iter().all(|call| !call.execution_started);
    let label = if all_speculative {
        "Queued"
    } else if all_exploration {
        "Exploring"
    } else {
        "Running"
    };
    let title_style = get_themed_style(COLOR_PRIMARY(), COLOR_BG(), Modifier::BOLD, show_picker);
    let mut lines = vec![Line::from(vec![
        Span::styled("• ", title_style),
        Span::styled(label, title_style),
    ])];

    let child_width = (width as usize).saturating_sub(6).max(1);
    for (index, call) in calls.iter().take(MAX_LIVE_CHILDREN).enumerate() {
        let prefix = if index == 0 { "  └ " } else { "    " };
        let mut spans = vec![Span::styled(
            prefix,
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
        )];
        if all_editing {
            spans.push(Span::styled(
                call.action.clone(),
                get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::BOLD, show_picker),
            ));
            if !call.target.is_empty() && call.target != "?" {
                spans.push(Span::styled(
                    " ",
                    get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::empty(), show_picker),
                ));
                spans.push(Span::styled(
                    truncate_to_width(
                        &call.target,
                        child_width.saturating_sub(call.action.chars().count() + 1),
                    ),
                    get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::empty(), show_picker),
                ));
            }
        } else {
            spans.push(Span::styled(
                call.action.clone(),
                get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::BOLD, show_picker),
            ));
            if !call.target.is_empty() && call.target != "?" {
                if call.action == "Bash" {
                    spans.push(Span::styled(
                        " $ ",
                        get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::empty(), show_picker),
                    ));
                    let command = truncate_to_width(&call.target, child_width.saturating_sub(2));
                    if let Some(command_line) =
                        highlight_shell_command(&command, COLOR_BG(), show_picker)
                            .into_iter()
                            .next()
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

#[cfg(test)]
mod tests {
    use super::{AssistantMarkdownCell, HistoryCell, TranscriptState};
    use rustcode::controller::{ChatMessage, History, RenderState};

    #[test]
    fn committed_history_cache_shares_large_block_and_projects_only_viewport() {
        // The cache key mixes in the process-global active theme, so a
        // concurrent theme test would change it between the two lookups.
        let _theme_guard = super::super::tests::THEME_TEST_LOCK
            .lock()
            .expect("theme test lock");
        let mut state = RenderState::new();
        state.history.push(ChatMessage::new(
            "assistant",
            (0..1_000)
                .map(|row| format!("history row {row:04}"))
                .collect::<Vec<_>>()
                .join("\n"),
        ));
        let snapshot = crate::ui::render_snapshot::render_snapshot(&state);
        let mut transcript = TranscriptState::default();

        let first = transcript.committed_block(&snapshot, 0, 80);
        let second = transcript.committed_block(&snapshot, 0, 80);
        assert!(std::sync::Arc::ptr_eq(&first, &second));
        assert!(first.len() > 200, "rendered lines: {}", first.len());

        let visible = crate::ui::render_visible_conversation_with_transcript(
            &snapshot,
            80,
            30,
            &mut transcript,
        );
        assert_eq!(visible.len(), 30);
        assert!(visible.iter().any(|line| line.to_string().contains("0999")));
    }

    #[test]
    fn transcript_sync_tracks_history_revision_and_reuses_unchanged_projection() {
        let mut history = History::default();
        history.push(ChatMessage::new("user", "hello"));
        let mut transcript = TranscriptState::default();

        transcript.sync_model(&history, "");
        let first_revision = transcript.history_revision();
        transcript.sync_model(&history, "");

        assert_eq!(transcript.history_revision(), first_revision);
        assert_eq!(transcript.model().committed().len(), 1);

        history.push(ChatMessage::new("assistant", "answer"));
        transcript.sync_model(&history, "");
        assert_ne!(transcript.history_revision(), first_revision);
        assert_eq!(transcript.model().committed().len(), 2);
    }

    #[test]
    fn transcript_hides_deferred_call_notice_but_keeps_other_history() {
        let mut history = History::default();
        history.push(ChatMessage::new("user", "inspect the project"));
        history.push(ChatMessage::new(
            "system",
            "[The model emitted 3 tool calls. Only one was executed this round; the remaining calls (grep, write_to_file) were not executed or scheduled.]",
        ));
        history.push(ChatMessage::new(
            "assistant",
            "Continuing with the real result.",
        ));
        let mut transcript = TranscriptState::default();

        transcript.sync_model(&history, "");

        assert_eq!(transcript.model().committed().len(), 2);
        assert!(
            transcript
                .model()
                .committed()
                .iter()
                .all(|cell| !matches!(cell, crate::ui::transcript::HistoryCell::System(_)))
        );
    }

    #[test]
    fn plain_stream_timer_update_reuses_rendered_markdown() {
        let mut transcript = TranscriptState::default();
        transcript.set_assistant(
            "A **long** response\n\nwith more text",
            false,
            Some(100),
            None,
            None,
        );
        let rendered = transcript.assistant.as_ref().unwrap().display_lines(80);
        let revision = transcript.revision();

        transcript.set_assistant(
            "A **long** response\n\nwith more text",
            false,
            Some(200),
            None,
            None,
        );

        assert_eq!(transcript.revision(), revision);
        assert!(
            transcript
                .assistant
                .as_ref()
                .unwrap()
                .cached_display
                .borrow()
                .is_some()
        );
        assert_eq!(
            transcript.assistant.as_ref().unwrap().display_lines(80),
            rendered
        );
    }

    #[test]
    fn thought_stream_timer_update_refreshes_visible_elapsed_time() {
        let mut transcript = TranscriptState::default();
        let source = "<think>Inspecting the file</think>Answer";
        transcript.set_assistant(source, false, Some(100), Some(100), None);
        let first = transcript.assistant.as_ref().unwrap().display_lines(80);

        transcript.set_assistant(source, false, Some(200), Some(200), None);
        let second = transcript.assistant.as_ref().unwrap().display_lines(80);

        assert_ne!(first, second);
        assert!(second.iter().any(|line| line.to_string().contains("200ms")));
    }

    #[test]
    fn completed_stream_blocks_are_not_reparsed_for_every_append() {
        let _theme_guard = super::super::tests::THEME_TEST_LOCK
            .lock()
            .expect("theme test lock");
        super::super::markdown::take_parsed_bytes();
        let mut transcript = TranscriptState::default();
        let mut source = String::new();
        for index in 0..24 {
            if !source.is_empty() {
                source.push_str("\n\n");
            }
            source.push_str(&format!(
                "Paragraph {index}: **stable Markdown** with enough text to cross a terminal row and a `code` span."
            ));
            transcript.set_assistant(&source, false, None, None, None);
            transcript.assistant.as_ref().unwrap().display_lines(72);
        }
        let parsed_bytes = super::super::markdown::take_parsed_bytes();
        assert!(
            parsed_bytes < source.len() * 8,
            "completed prefix was repeatedly parsed: {parsed_bytes} bytes for {} source bytes",
            source.len()
        );
    }

    #[test]
    fn streamed_markdown_matches_full_render_after_each_append() {
        let _theme_guard = super::super::tests::THEME_TEST_LOCK
            .lock()
            .expect("theme test lock");
        let cases: &[&[&str]] = &[
            &[
                "# Heading",
                "# Heading\n\nFirst paragraph",
                "# Heading\n\nFirst paragraph\n\nSecond **bold** paragraph",
            ],
            &[
                "- first item",
                "- first item\n\n",
                "- first item\n\n- second item",
                "- first item\n\n- second item\n\nAfter the list",
            ],
            &[
                "| Name | Value |",
                "| Name | Value |\n|---|---|",
                "| Name | Value |\n|---|---|\n|",
                "| Name | Value |\n|---|---|\n| key",
                "| Name | Value |\n|---|---|\n| key | **bold** value |",
                "| Name | Value |\n|---|---|\n| key | **bold** value |\n\nAfter table",
            ],
            &[
                "Before code",
                "Before code\n\n```rust\nfn first() {}",
                "Before code\n\n```rust\nfn first() {}\n```\n\nAfter code",
            ],
            &[
                "[site][docs]",
                "[site][docs]\n\nAnother paragraph",
                "[site][docs]\n\nAnother paragraph\n\n[docs]: https://example.com",
                "[site][docs]\n\nAnother paragraph\n\n[docs]: https://example.com\n\nLater text",
            ],
        ];
        for steps in cases {
            let mut transcript = TranscriptState::default();
            for source in *steps {
                transcript.set_assistant(source, false, None, None, None);
                let incremental = transcript.assistant.as_ref().unwrap().display_lines(48);
                let full = AssistantMarkdownCell::streaming(source, false, None, None, None)
                    .display_lines(48);
                assert_eq!(incremental, full, "source: {source:?}");
            }
            let source = steps.last().unwrap();
            let narrow = transcript.assistant.as_ref().unwrap().display_lines(28);
            let full_narrow =
                AssistantMarkdownCell::streaming(source, false, None, None, None).display_lines(28);
            assert_eq!(narrow, full_narrow, "narrow source: {source:?}");
        }
    }

    #[test]
    fn streamed_markdown_matches_full_render_for_partial_tokens() {
        let _theme_guard = super::super::tests::THEME_TEST_LOCK
            .lock()
            .expect("theme test lock");
        let source = "# Heading\n\nA **bold** paragraph with [a link](https://example.com).\n\n- first item\n\n- second item\n\n| Key | Value |\n|---|---|\n| one | two |";
        let mut transcript = TranscriptState::default();
        for end in source
            .char_indices()
            .map(|(index, ch)| index + ch.len_utf8())
        {
            let prefix = &source[..end];
            transcript.set_assistant(prefix, false, None, None, None);
            let incremental = transcript.assistant.as_ref().unwrap().display_lines(48);
            let full =
                AssistantMarkdownCell::streaming(prefix, false, None, None, None).display_lines(48);
            assert_eq!(incremental, full, "prefix ending at byte {end}: {prefix:?}");
        }
    }

    #[test]
    fn streaming_markdown_invalidates_after_source_rewrite_and_theme_change() {
        let _theme_guard = super::super::tests::THEME_TEST_LOCK
            .lock()
            .expect("theme test lock");
        super::super::theme::set_active_theme("default");
        let mut transcript = TranscriptState::default();
        for source in [
            "First **bold** paragraph\n\nSecond paragraph",
            "First **bold** paragraph\n\nSecond paragraph\n\nThird paragraph",
            "Rewritten [link](https://example.com)\n\nSecond paragraph",
        ] {
            transcript.set_assistant(source, false, None, None, None);
            let incremental = transcript.assistant.as_ref().unwrap().display_lines(48);
            let full =
                AssistantMarkdownCell::streaming(source, false, None, None, None).display_lines(48);
            assert_eq!(incremental, full, "source: {source:?}");
        }

        super::super::theme::set_active_theme("nord");
        let source = "Rewritten [link](https://example.com)\n\nSecond paragraph";
        let incremental = transcript.assistant.as_ref().unwrap().display_lines(48);
        let full =
            AssistantMarkdownCell::streaming(source, false, None, None, None).display_lines(48);
        super::super::theme::set_active_theme("default");
        assert_eq!(incremental, full);
    }

    #[test]
    fn thought_timer_redraw_keeps_streaming_cell_and_markdown_cache() {
        let _theme_guard = super::super::tests::THEME_TEST_LOCK
            .lock()
            .expect("theme test lock");
        let mut transcript = TranscriptState::default();
        let source = "<think>Working through this.\n\nAnother thought.</think>\n\nFinal paragraph";
        transcript.set_assistant(source, false, Some(100), Some(100), Some(4));
        let first = transcript.assistant.as_ref().unwrap().display_lines(48);
        let cell = transcript.assistant.as_ref().unwrap() as *const AssistantMarkdownCell;
        let cached_runs = transcript
            .assistant
            .as_ref()
            .unwrap()
            .streaming_markdown
            .borrow()
            .len();

        transcript.set_assistant(source, false, Some(200), Some(200), Some(8));
        let updated = transcript.assistant.as_ref().unwrap();
        assert!(std::ptr::eq(cell, updated));
        assert_eq!(updated.streaming_markdown.borrow().len(), cached_runs);
        let second = updated.display_lines(48);
        assert_ne!(first, second);
        assert!(second.iter().any(|line| line.to_string().contains("200ms")));
    }

    #[test]
    #[ignore = "manual before/after streaming Markdown benchmark"]
    fn benchmark_long_multi_block_stream() {
        let _theme_guard = super::super::tests::THEME_TEST_LOCK
            .lock()
            .expect("theme test lock");
        for run in 0..3 {
            let mut transcript = TranscriptState::default();
            let mut source = String::new();
            let started = std::time::Instant::now();
            for index in 0..48 {
                if !source.is_empty() {
                    source.push_str("\n\n");
                }
                source.push_str(&format!(
                    "Paragraph {index}: **stable text** with `inline code`, [a link](https://example.com), and enough words to wrap across several terminal rows. This paragraph also has _emphasis_ and a second sentence about the rendering path."
                ));
                transcript.set_assistant(&source, false, None, None, None);
                let lines = transcript.assistant.as_ref().unwrap().display_lines(72);
                std::hint::black_box(lines);
            }
            println!("multi-block stream run {run}: {:?}", started.elapsed());
        }
    }
}
