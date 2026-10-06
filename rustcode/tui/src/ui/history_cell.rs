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
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::{
    COLOR_BG, COLOR_MUTED, COLOR_PRIMARY, COLOR_TEXT, COLOR_TIP, get_themed_style,
    highlight_shell_command, push_wrapped_with_continuation, tool_transcript::tool_preview_window,
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
pub(crate) struct TranscriptState {
    assistant: Option<AssistantMarkdownCell>,
    tools: Option<LiveToolCell>,
    revision: u64,
    history_revision: Option<u64>,
    model: super::TranscriptModel,
    scroll_rows: usize,
    /// Whether the last painted frame showed the newest transcript row.
    ///
    /// Recomputed from the clamped offset on every projection rather than
    /// tracked incrementally, following Codex's `tail_visible` (#1595): a
    /// derived fact about the current offset cannot drift out of sync with it.
    tail_visible: bool,
    /// Content arrived below the reading position while the user was away.
    ///
    /// Cleared as soon as the tail is visible again, so the affordance cannot
    /// claim unseen activity the user has already seen.
    unseen_activity: bool,
    /// `(history revision, live content fingerprint)` the last projection consumed,
    /// so the next one can tell that new output arrived below the reader.
    last_content: Option<(u64, usize)>,
    /// The row the affordance is painted into, owned by the render layer.
    pub(super) follow_control: super::follow_control::FollowControl,
    pub(super) reading_anchor: Option<ReadingAnchor>,
    pub(crate) selection: super::selection::TranscriptSelection,
    /// Independent selection for the currently visible informational panel.
    /// Panel rows use a different painted surface from the conversation, so
    /// sharing the transcript selection would pin and copy the wrong content.
    pub(crate) panel_selection: super::selection::TranscriptSelection,
    /// Content rectangle from the last painted informational panel frame.
    pub(crate) panel_selection_area: Option<ratatui::layout::Rect>,
    /// Whether the visible panel body responds to vertical scrolling.
    pub(crate) panel_selection_scrollable: bool,
    committed_cache:
        Option<super::lru::LruCache<(u64, u64, usize, u16, u64), Arc<Vec<Line<'static>>>>>,
}

impl Default for TranscriptState {
    /// A transcript that has painted nothing yet is following: the newest row
    /// is by definition the one on screen, and the first projection confirms
    /// it either way. Deriving `Default` would start it *not* following and
    /// paint a "return to bottom" affordance over the welcome banner.
    fn default() -> Self {
        Self {
            assistant: None,
            tools: None,
            revision: 0,
            history_revision: None,
            model: super::TranscriptModel::default(),
            scroll_rows: 0,
            tail_visible: true,
            unseen_activity: false,
            last_content: None,
            follow_control: super::follow_control::FollowControl::default(),
            reading_anchor: None,
            selection: super::selection::TranscriptSelection::default(),
            panel_selection: super::selection::TranscriptSelection::default(),
            panel_selection_area: None,
            panel_selection_scrollable: false,
            committed_cache: None,
        }
    }
}

/// The committed and live tail at the last painted reading viewport. A fixed offset
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
    pub(super) live_rows: usize,
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
/// Selected wheel scrolling applies the same delta in the next frame.
/// Keyboard and edge-drag scrolling still advance one row at a time.
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

    /// Apply queued wheel movement, or one edge-drag row, before painting.
    pub(crate) fn step_selection_scroll(&mut self) -> bool {
        let before = self.scroll_rows;
        let Some(direction) = self.selection.take_scroll_step(before) else {
            return false;
        };
        if direction < 0 {
            self.scroll_up(direction.unsigned_abs());
        } else {
            self.scroll_down(direction.unsigned_abs());
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

    /// Whether the transcript is showing the newest row and nothing is being
    /// selected, following Codex's `is_following`.
    ///
    /// Two explicit inputs, not one flag: a reading offset *or* a live text
    /// selection releases follow, so pointing at a row is enough to keep the
    /// viewport still while the model streams (#1595).
    pub(crate) fn is_following(&self) -> bool {
        self.tail_visible && !self.selection.is_active()
    }

    /// Whether the last painted frame showed the final transcript row.
    pub(crate) fn tail_visible(&self) -> bool {
        self.tail_visible
    }

    /// Whether content arrived below the reading position since it was last
    /// visible.
    pub(crate) fn unseen_activity(&self) -> bool {
        self.unseen_activity
    }

    pub(crate) fn follow_control(&self) -> &super::follow_control::FollowControl {
        &self.follow_control
    }

    /// Re-enter follow: the newest row is on screen and the "new activity"
    /// affordance has nothing left to announce.
    ///
    /// Optimistic, like Codex's `jump_to_latest`: the position is `Latest`
    /// before the next projection confirms it, so the control disappears on the
    /// same press that asked for it.
    pub(crate) fn jump_to_latest(&mut self) {
        self.scroll_down(usize::MAX);
        self.tail_visible = true;
        self.unseen_activity = false;
        self.follow_control.clear();
    }

    /// Record what the projection actually showed this frame.
    ///
    /// `content_changed` is the signal that something below the reading
    /// position moved since the last projection. The follow test uses the
    /// *previous* frame's tail visibility, exactly like Codex, so a stream
    /// that started while the user was reading raises the flag before the
    /// next frame's recomputation can clear it (#1595).
    pub(super) fn note_projection(
        &mut self,
        tail_visible: bool,
        content_changed: bool,
        content_mark: (u64, usize),
    ) {
        let following = self.is_following();
        self.tail_visible = tail_visible;
        self.last_content = Some(content_mark);
        if tail_visible {
            self.unseen_activity = false;
        } else if content_changed && !following {
            self.unseen_activity = true;
        }
    }

    /// `(history revision, live content fingerprint)` the last projection consumed.
    pub(super) fn last_content(&self) -> Option<(u64, usize)> {
        self.last_content
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
            // Viewports showing more than ~4 messages thrashed a cap-4 cache:
            // the suffix walk and the viewport loop read the same blocks, so
            // keep enough entries for a tall viewport plus its suffix (#1582).
            .get_or_insert_with(|| super::lru::LruCache::new(32));
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
                home_path: None,
            });
        }
    }

    pub(crate) fn set_tools_with_verbosity(
        &mut self,
        calls: &[LiveToolCall],
        verbosity: &Verbosity,
        home_path: Option<&str>,
    ) {
        // Compare before allocating so an unchanged frame allocates nothing.
        let changed = self.tools.as_ref().is_none_or(|cell| {
            cell.calls != calls
                || cell.verbosity != *verbosity
                || cell.home_path.as_deref() != home_path
        });
        if changed {
            self.revision = self.revision.saturating_add(1);
            self.tools = Some(LiveToolCell {
                calls: calls.to_vec(),
                verbosity: verbosity.clone(),
                home_path: home_path.map(str::to_owned),
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
    /// Home directory used to contract absolute paths, matching the committed
    /// transcript projection.
    home_path: Option<String>,
}

impl HistoryCell for LiveToolCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        render_live_tool_cell_with_verbosity(
            &self.calls,
            width,
            &self.verbosity,
            false,
            self.home_path.as_deref(),
        )
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

/// Keep the tail of an over-long target so the informative part (the file name
/// and its parents) survives on a single row instead of the head, which says
/// little about *which* item is queued or running.
pub(super) fn tail_to_width(text: &str, max_width: usize) -> String {
    if text.width() <= max_width {
        return text.to_owned();
    }
    if max_width == 0 {
        return String::new();
    }
    let suffix = '…';
    let budget = max_width.saturating_sub(1);
    // Walk backwards so the kept graphemes stay one contiguous tail; skipping
    // over-wide graphemes instead would splice unrelated characters together.
    let mut kept: Vec<&str> = Vec::new();
    let mut used = 0;
    for (index, grapheme) in text.grapheme_indices(true).rev() {
        let grapheme_width = grapheme.width();
        if used + grapheme_width > budget {
            break;
        }
        used += grapheme_width;
        kept.push(&text[index..index + grapheme.len()]);
    }
    let mut tail = String::from(suffix);
    for grapheme in kept.into_iter().rev() {
        tail.push_str(grapheme);
    }
    tail
}

/// Cancel affordance for a running live cell, sized to the space the heading
/// has already used. The full hint needs 16 display columns and the short form
/// 6, so narrow terminals keep a compact `esc` instead of losing the
/// affordance entirely (#1725).
fn cancel_hint_suffix(width: u16, used: usize) -> &'static str {
    let available = usize::from(width).saturating_sub(used);
    if available >= " · esc interrupt".width() {
        " · esc interrupt"
    } else if available >= " · esc".width() {
        " · esc"
    } else {
        ""
    }
}

pub(super) fn is_live_tool_call_visible(call: &LiveToolCall) -> bool {
    call.execution_started || (!call.target.is_empty() && call.target != "?")
}

/// Width-aware truncation shared with the indicator row so wide glyphs never
/// overflow a narrow terminal row.
fn truncate_to_width(text: &str, width: usize) -> String {
    super::composer_render::truncate_to_display_width(text, width)
}

/// Keep each live status summary on one terminal row while preserving its
/// colored prefix and marking clipped detail with an ellipsis.
fn fit_live_row(line: Line<'static>, width: usize) -> Line<'static> {
    if line.width() <= width {
        return line;
    }
    if width == 0 {
        return Line::default();
    }
    let mut fitted = Vec::new();
    let mut remaining = width.saturating_sub(1);
    let mut clipped_style = None;
    'spans: for span in line.spans {
        let mut kept = String::new();
        for grapheme in span.content.graphemes(true) {
            let grapheme_width = grapheme.width();
            if grapheme_width > remaining {
                clipped_style = Some(span.style);
                if !kept.is_empty() {
                    fitted.push(Span::styled(kept, span.style));
                }
                break 'spans;
            }
            remaining -= grapheme_width;
            kept.push_str(grapheme);
        }
        if !kept.is_empty() {
            fitted.push(Span::styled(kept, span.style));
        }
    }
    if let Some(style) = clipped_style {
        fitted.push(Span::styled("…", style));
    }
    Line::from(fitted)
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
    render_live_tool_cell_with_verbosity(calls, width, &Verbosity::Low, show_picker, None)
}

pub(super) fn render_live_tool_cell_with_verbosity(
    calls: &[LiveToolCall],
    width: u16,
    verbosity: &Verbosity,
    show_picker: bool,
    home_path: Option<&str>,
) -> Vec<Line<'static>> {
    render_live_tool_cell_at(
        calls,
        width,
        verbosity,
        show_picker,
        std::time::Instant::now(),
        home_path,
    )
}

pub(super) fn render_live_tool_cell_at(
    calls: &[LiveToolCall],
    width: u16,
    verbosity: &Verbosity,
    show_picker: bool,
    now: std::time::Instant,
    home_path: Option<&str>,
) -> Vec<Line<'static>> {
    let calls = calls
        .iter()
        .filter(|call| is_live_tool_call_visible(call))
        .collect::<Vec<_>>();
    if calls.is_empty() || width == 0 {
        return Vec::new();
    }

    // Speculative calls are only projections of streamed model output; the
    // compact generic rows avoid presenting those as an executing command.
    let has_speculative = calls.iter().any(|call| !call.execution_started);

    if !has_speculative && calls.len() == 1 && calls[0].tool_name == "run_command" {
        let call = &calls[0];
        let title_style =
            get_themed_style(COLOR_PRIMARY(), COLOR_BG(), Modifier::BOLD, show_picker);
        let command = call.target.clone();
        // One indicator row: the heading carries the state word, the command
        // and the elapsed clock together, mirroring the committed
        // `• Ran $ <cmd> · <status>` summary (#1725). A separate `●` child row
        // repeating the same running state was the duplicate indicator.
        let mut header = vec![
            Span::styled("• ", title_style),
            Span::styled("Running", title_style),
        ];
        let show_command = command.is_empty() || command == "?";
        if show_command {
            header.push(Span::styled(
                " Bash",
                get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::BOLD, show_picker),
            ));
        } else {
            header.push(Span::styled(
                " $ ",
                get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::empty(), show_picker),
            ));
        }
        let used: usize = header.iter().map(|span| span.content.width()).sum();
        let elapsed =
            super::fmt_elapsed_compact(now.saturating_duration_since(call.started_at).as_secs());
        // The elapsed clock shares the row, so reserve its columns too.
        let elapsed_suffix = format!(" · {elapsed}");
        let elapsed_suffix = if usize::from(width) > used + elapsed_suffix.width() {
            elapsed_suffix
        } else {
            String::new()
        };
        let room_after_elapsed = usize::from(width).saturating_sub(used + elapsed_suffix.width());
        let cancel_hint = if room_after_elapsed >= " · esc interrupt".width() + 4 {
            " · esc interrupt"
        } else if room_after_elapsed >= " · esc".width() + 4 {
            " · esc"
        } else {
            ""
        };
        if !show_command {
            let command_width = usize::from(width)
                .saturating_sub(used + elapsed_suffix.width() + cancel_hint.width());
            let command = super::modals::truncate_middle_to_width(&command, command_width);
            header.extend(
                highlight_shell_command(&command, COLOR_BG(), show_picker)
                    .into_iter()
                    .next()
                    .map(|line| line.spans)
                    .unwrap_or_default(),
            );
        }
        header.push(Span::styled(
            format!("{elapsed_suffix}{cancel_hint}"),
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
        ));
        let mut lines = vec![fit_live_row(Line::from(header), usize::from(width))];
        if let Some(cwd) = &call.cwd {
            push_wrapped_with_continuation(
                &mut lines,
                vec![
                    super::tool_transcript::tool_body_spine(show_picker),
                    Span::styled(
                        format!("cwd: {cwd}"),
                        get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::DIM, show_picker),
                    ),
                ],
                usize::from(width).max(1),
                Some(super::tool_transcript::tool_body_spine(show_picker)),
            );
        }
        // High verbosity never shows live output in this cell. Quiet calls
        // also stay one row tall until output actually arrives.
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
        let mut body = Vec::new();
        for (text, stderr) in output {
            let mut spans = vec![super::tool_transcript::tool_body_spine(show_picker)];
            spans.push(Span::styled(
                text,
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
            ));
            let continuation = super::tool_transcript::tool_body_spine(show_picker);
            push_wrapped_with_continuation(
                &mut body,
                spans,
                (width as usize).max(10),
                Some(continuation),
            );
        }

        let byte_note = (call.omitted_output_bytes > 0).then(|| {
            if call.omitted_output_bytes >= 1024 {
                format!("{}K", call.omitted_output_bytes.div_ceil(1024))
            } else {
                format!("{}B", call.omitted_output_bytes)
            }
        });
        if let Some(window) = tool_preview_window(body.len(), byte_note.is_some()) {
            let mut marker = match (window.omitted_rows > 0, byte_note) {
                (true, Some(note)) => format!("… +{} lines · {note}", window.omitted_rows),
                (true, None) => format!("… +{} lines", window.omitted_rows),
                (false, Some(note)) => format!("… {note} omitted"),
                (false, None) => String::new(),
            };
            marker = truncate_to_width(&marker, (width as usize).saturating_sub(2).max(1));
            let marker = Line::from(vec![
                super::tool_transcript::tool_body_spine(show_picker),
                Span::styled(
                    marker,
                    get_themed_style(
                        COLOR_MUTED(),
                        COLOR_BG(),
                        Modifier::ITALIC | Modifier::DIM,
                        show_picker,
                    ),
                ),
            ]);
            let tail_start = body.len().saturating_sub(window.tail_rows);
            let tail_rows = body.split_off(tail_start);
            body.truncate(window.head_rows);
            body.push(marker);
            body.extend(tail_rows);
        }
        lines.extend(body);
        return lines;
    }

    let title_style = get_themed_style(COLOR_PRIMARY(), COLOR_BG(), Modifier::BOLD, show_picker);
    let detail_style = get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker);
    let action_style = get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::BOLD, show_picker);
    let target_style = get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::empty(), show_picker);
    let mut lines = Vec::new();
    // Several calls share one heading and hang beneath it as a tree, the shape
    // the committed `• Ran` group takes once they finish. A lone call stays
    // folded into its heading, like the committed single-command summary.
    let grouped = calls.len() > 1;
    let any_started = calls.iter().any(|call| call.execution_started);
    let shown = calls.len().min(MAX_LIVE_CHILDREN);
    let has_overflow = calls.len() > MAX_LIVE_CHILDREN;
    if grouped {
        let heading = if any_started { "Running" } else { "Queued" };
        let hint = if any_started {
            cancel_hint_suffix(width, 2 + heading.width())
        } else {
            ""
        };
        lines.push(fit_live_row(
            Line::from(vec![
                Span::styled("• ", title_style),
                Span::styled(heading, title_style),
                Span::styled(hint, detail_style),
            ]),
            usize::from(width),
        ));
    }
    for (call_index, call) in calls.iter().take(MAX_LIVE_CHILDREN).enumerate() {
        let status = if call.execution_started {
            "Running"
        } else {
            "Queued"
        };
        let target = super::tool_transcript::contract_home_path(&call.target, home_path);
        let target = if target.is_empty() || target == "?" {
            String::new()
        } else if call.action == "Bash" && !grouped {
            format!("$ {target}")
        } else {
            target
        };
        let matching = calls
            .iter()
            .filter(|other| other.action == call.action && other.target == call.target)
            .count();
        let ordinal = calls[..call_index]
            .iter()
            .filter(|other| other.action == call.action && other.target == call.target)
            .count()
            + 1;
        let duplicate_suffix = if matching > 1 {
            format!(" · {ordinal}/{matching}")
        } else {
            String::new()
        };
        let elapsed_suffix = if call.execution_started {
            let elapsed = now.saturating_duration_since(call.started_at).as_secs();
            (elapsed >= 5).then(|| format!(" · {}", super::fmt_elapsed_compact(elapsed)))
        } else {
            None
        }
        .unwrap_or_default();
        // The group heading names the running state once; a child only says
        // so when it differs, i.e. it is still waiting its turn.
        let state_suffix = if grouped && any_started && !call.execution_started {
            " · queued"
        } else {
            ""
        };
        let is_last = call_index + 1 == shown && !has_overflow;
        let prefix_width = if grouped {
            4 + call.action.width()
        } else {
            2 + status.width() + 1 + call.action.width()
        };
        let target_space = usize::from(!target.is_empty());
        let min_target = if target.is_empty() { 0 } else { 6 };
        let suffix_width = duplicate_suffix.width() + elapsed_suffix.width() + state_suffix.width();
        let interrupt_hint = if !grouped && call.execution_started {
            cancel_hint_suffix(
                width,
                prefix_width + target_space + min_target + suffix_width,
            )
        } else {
            ""
        };
        let fixed = prefix_width + target_space + suffix_width + interrupt_hint.width();
        let mut spans = if grouped {
            vec![
                super::tool_transcript::tool_tree_prefix(is_last, show_picker),
                Span::styled(
                    if call.execution_started {
                        "• "
                    } else {
                        "◦ "
                    },
                    detail_style,
                ),
                Span::styled(call.action.clone(), action_style),
            ]
        } else {
            vec![
                Span::styled("• ", title_style),
                Span::styled(format!("{status} "), title_style),
                Span::styled(call.action.clone(), action_style),
            ]
        };
        if !target.is_empty() {
            let target_width = usize::from(width).saturating_sub(fixed);
            if target_width > 0 {
                spans.push(Span::raw(" "));
                spans.push(Span::styled(
                    tail_to_width(&target, target_width),
                    target_style,
                ));
            }
        }
        if !duplicate_suffix.is_empty() {
            spans.push(Span::styled(duplicate_suffix, detail_style));
        }
        if !elapsed_suffix.is_empty() {
            spans.push(Span::styled(elapsed_suffix, detail_style));
        }
        if !state_suffix.is_empty() {
            spans.push(Span::styled(state_suffix, detail_style));
        }
        if !interrupt_hint.is_empty() {
            spans.push(Span::styled(interrupt_hint, detail_style));
        }
        lines.push(fit_live_row(Line::from(spans), usize::from(width)));
        if call.execution_started && !matches!(verbosity, Verbosity::High) {
            let latest = call
                .output
                .iter()
                .flat_map(|chunk| chunk.text.lines())
                .filter(|line| !line.trim().is_empty())
                .next_back();
            if let Some(latest) = latest {
                // Output hangs under its own call: beside the spine while
                // siblings follow, under the action once it is the last child.
                let indent = match (grouped, is_last) {
                    (false, _) => "  ",
                    (true, true) => "    ",
                    (true, false) => "│   ",
                };
                lines.push(Line::from(vec![
                    Span::styled(indent, detail_style),
                    Span::styled(
                        truncate_to_width(
                            &rustcode_tool_protocol::text::strip_ansi_escapes(latest),
                            usize::from(width).saturating_sub(indent.width()).max(1),
                        ),
                        detail_style,
                    ),
                ]));
            }
        }
    }
    if has_overflow {
        lines.push(Line::from(vec![
            super::tool_transcript::tool_tree_prefix(true, show_picker),
            Span::styled(
                truncate_to_width(
                    &format!("… +{} more", calls.len() - MAX_LIVE_CHILDREN),
                    usize::from(width).saturating_sub(2).max(1),
                ),
                detail_style,
            ),
        ]));
    }

    lines
}

#[cfg(test)]
mod tests {
    use super::{AssistantMarkdownCell, HistoryCell, TranscriptState};
    use rustcode::controller::{ChatMessage, History, RenderState, Verbosity};

    #[test]
    fn live_tool_rows_keep_state_and_action_without_nested_decoration() {
        let mut call = rustcode::controller::LiveToolCall::new(
            "call-1",
            None,
            "view_file",
            "Read",
            "src/main.rs",
        );
        call.execution_started = true;
        call.output
            .push_back(rustcode::controller::LiveToolOutputChunk {
                stderr: false,
                text: "12 lines read".to_owned(),
            });

        let rendered = super::render_live_tool_cell(&[call], 80, false)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>();

        assert_eq!(
            rendered,
            [
                "• Running Read src/main.rs · esc interrupt",
                "  12 lines read"
            ]
        );
    }

    #[test]
    fn generic_running_rows_add_elapsed_only_after_five_seconds() {
        let mut call = rustcode::controller::LiveToolCall::new(
            "call-1",
            None,
            "view_file",
            "Read",
            "src/main.rs",
        );
        call.execution_started = true;
        let now = call.started_at + std::time::Duration::from_secs(4);
        let before_threshold =
            super::render_live_tool_cell_at(&[call.clone()], 80, &Verbosity::Low, false, now, None)
                .into_iter()
                .map(|line| line.to_string())
                .collect::<Vec<_>>()
                .join("\n");
        assert!(!before_threshold.contains("4s"), "{before_threshold}");

        let now = call.started_at + std::time::Duration::from_secs(5);
        let after_threshold =
            super::render_live_tool_cell_at(&[call], 80, &Verbosity::Low, false, now, None)
                .into_iter()
                .map(|line| line.to_string())
                .collect::<Vec<_>>()
                .join("\n");
        assert!(after_threshold.contains("· 5s"), "{after_threshold}");
    }

    #[test]
    fn identical_live_rows_get_ordinals_and_one_interrupt_hint() {
        let mut first = rustcode::controller::LiveToolCall::new(
            "internal-1",
            None,
            "view_file",
            "Read",
            "src/main.rs",
        );
        first.execution_started = true;
        let second = rustcode::controller::LiveToolCall::new(
            "internal-2",
            None,
            "view_file",
            "Read",
            "src/main.rs",
        );
        let rendered = super::render_live_tool_cell(&[first, second], 80, false)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>();

        assert_eq!(
            rendered,
            [
                "• Running · esc interrupt",
                "├ • Read src/main.rs · 1/2",
                "└ • Read src/main.rs · 2/2"
            ]
        );
        assert_eq!(
            rendered.join("\n").matches("esc").count(),
            1,
            "{rendered:?}"
        );
        assert!(!rendered.join("\n").contains("internal-"));

        let bash = rustcode::controller::LiveToolCall::new(
            "internal-bash",
            None,
            "run_command",
            "Bash",
            "cargo test",
        );
        let rendered_bash = super::render_live_tool_cell(&[bash], 80, false)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(rendered_bash.matches("esc").count(), 1, "{rendered_bash}");
    }

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
    fn live_tool_preview_keeps_all_short_output_before_forced_byte_marker() {
        let mut call = rustcode::controller::LiveToolCall::new(
            "local:1",
            None,
            "run_command",
            "Bash",
            "echo ok",
        );
        call.output
            .push_back(rustcode::controller::LiveToolOutputChunk {
                stderr: false,
                text: "visible output\n".to_owned(),
            });
        call.omitted_output_bytes = 1;

        let rendered = super::render_live_tool_cell(&[call], 80, false)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>();
        assert_eq!(rendered.len(), 3, "{rendered:?}");
        assert!(rendered[1].contains("visible output"), "{rendered:?}");
        assert!(rendered[2].contains("1B omitted"), "{rendered:?}");
    }

    /// #1595: follow is a two-input state, not one flag. A reading offset or a
    /// live selection releases it, output arriving while the user is away only
    /// raises a flag, and re-entering follow clears it.
    #[test]
    fn follow_state_releases_on_scroll_and_re_enters_on_jump_to_latest() {
        let mut transcript = TranscriptState::default();
        assert!(
            transcript.is_following(),
            "a transcript that has painted nothing yet is following"
        );

        transcript.note_projection(/* tail_visible */ true, false, (1, 0));
        assert!(transcript.is_following());
        assert!(transcript.tail_visible());
        assert!(!transcript.unseen_activity());

        // The user scrolls up: follow is released, and the affordance appears.
        transcript.scroll_up(3);
        transcript.note_projection(/* tail_visible */ false, false, (1, 0));
        assert!(!transcript.is_following());
        assert!(!transcript.tail_visible());
        assert!(!transcript.unseen_activity());

        // Output arrives below the reader. The flag is raised; the offset is
        // untouched, because the projection never moves a reading position.
        let offset = transcript.scroll_rows();
        transcript.note_projection(/* tail_visible */ false, true, (2, 40));
        assert!(transcript.unseen_activity());
        assert_eq!(transcript.scroll_rows(), offset);

        // Showing the tail again clears it, so the control cannot claim
        // activity the user has already seen.
        transcript.note_projection(/* tail_visible */ true, true, (2, 40));
        assert!(!transcript.unseen_activity());
        assert!(transcript.is_following());

        // And returning to the newest row re-follows and clears the flag.
        // The first frame after the user's own scroll carries no new content,
        // so it must not be reported back to them as unseen activity.
        transcript.scroll_up(3);
        transcript.note_projection(/* tail_visible */ false, false, (2, 40));
        assert!(!transcript.unseen_activity());
        transcript.note_projection(/* tail_visible */ false, true, (3, 90));
        assert!(transcript.unseen_activity());
        transcript.jump_to_latest();
        assert_eq!(transcript.scroll_rows(), 0);
        assert!(!transcript.unseen_activity());
        assert!(transcript.is_following());
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
