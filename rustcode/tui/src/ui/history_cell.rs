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
    COLOR_BG, COLOR_MUTED, COLOR_TEXT, COLOR_TIP, get_themed_style, push_wrapped_with_continuation,
    tool_transcript::tool_preview_window,
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
/// What a click on a cell of an open panel does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PanelTarget {
    /// The `esc` hint: close the panel.
    Escape,
    /// A list row this many rows from the selected one: choose it.
    ListRow(isize),
}

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
    /// Last full viewport of the wrapped command panel in the painted frame.
    pub(crate) panel_max_scroll_row: u16,
    pub(crate) task_log_max_scroll_row: usize,
    committed_cache: Option<super::lru::LruCache<CommittedKey, Arc<Vec<Line<'static>>>>>,
    /// For each cached tool chain, the call every one of its rows belongs to.
    tool_owner_cache: Option<super::lru::LruCache<CommittedKey, Arc<Vec<Option<ToolBlock>>>>>,
    /// `(inputs fingerprint, (expanded, collapsible))` of the last expand
    /// readout. Finding the collapsible rows renders every tool result in the
    /// session, which is far too much to redo on each frame.
    expand_progress: Option<(u64, (usize, usize))>,
    /// For each transcript line of the projection just built, the tool block
    /// it belongs to. `None` while a selection pins the view: the rows painted
    /// before the press stay the click targets.
    pub(super) tool_line_flags: Option<Vec<Option<ToolBlock>>>,
    /// Painted transcript area and, per row, the tool block shown there.
    tool_rows: (ratatui::layout::Rect, Vec<Option<ToolBlock>>),
    /// A press on a tool block; releasing in place opens or closes its output.
    pub(crate) tool_click: Option<((u16, u16), ToolBlock)>,
    /// The tool block under the pointer, lit so it reads as clickable.
    hovered_tool_block: Option<ToolBlock>,
    /// For each transcript line of the projection just built, the history
    /// index of the user or assistant message it belongs to. `None` while a
    /// selection pins the view, like [`Self::tool_line_flags`].
    pub(super) message_line_flags: Option<Vec<Option<usize>>>,
    /// Painted transcript area and, per row, the message shown there.
    message_rows: (ratatui::layout::Rect, Vec<Option<usize>>),
    /// The message under the pointer, lit so a right-click's target is clear.
    hovered_message: Option<usize>,
    /// Where the footer's task counter was painted, if it was.
    pub(crate) tasks_chip: Option<ratatui::layout::Rect>,
    pub(crate) tasks_chip_hovered: bool,
    /// The permission posture token in the footer.
    pub(crate) posture_chip: Option<ratatui::layout::Rect>,
    pub(crate) posture_chip_hovered: bool,
    /// Clickable cells of the panel painted last frame: its `esc` hint and
    /// the rows of its list.
    panel_targets: Vec<(ratatui::layout::Rect, PanelTarget)>,
    /// The panel target or follow control under the pointer.
    hovered_target: Option<ratatui::layout::Rect>,
    /// Where the pointer last was. Rows move under a still pointer whenever
    /// the layout changes height, so each frame resolves the hover again.
    pointer: Option<(u16, u16)>,
    /// The running indicator last painted and when, see
    /// [`Self::settle_indicator`].
    held_indicator: Option<HeldIndicator>,
    /// Click counting and word/line units for mouse selection in the composer.
    pub(crate) composer_mouse: super::ComposerMouse,
    /// Sort and collapse state of visualization widgets, keyed by
    /// `(history index, viz ordinal)`. Terminal UI state only: never saved
    /// with the session or sent to a provider.
    viz_views: std::collections::BTreeMap<(usize, usize), super::visualization::VizView>,
    /// Clickable widget spans of each cached committed block.
    viz_hit_cache:
        Option<super::lru::LruCache<CommittedKey, Arc<Vec<super::visualization::VizHit>>>>,
    /// For each widget target of the projection just built, its transcript
    /// line and columns. `None` while a selection pins the view, like
    /// [`Self::tool_line_flags`].
    pub(super) viz_line_flags: Option<Vec<(usize, std::ops::Range<u16>, VizTarget)>>,
    /// Painted cells of each widget target in the last frame.
    viz_rows: Vec<(ratatui::layout::Rect, VizTarget)>,
    /// A press on a widget target; releasing in place applies it.
    pub(crate) viz_click: Option<((u16, u16), VizTarget)>,
    /// The widget target under the pointer, lit so it reads as clickable.
    hovered_viz: Option<ratatui::layout::Rect>,
}

/// One clickable part of a visualization in the transcript: a table header
/// or a tree node with children.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct VizTarget {
    /// History index of the assistant message holding the widget.
    pub(crate) message: usize,
    /// Which `rustcode-viz` fence of that message.
    pub(crate) ordinal: usize,
    /// Fingerprint of the fence body the target was drawn from.
    pub(crate) source: u64,
    pub(crate) action: super::visualization::VizAction,
    /// Layout width the target was drawn at.
    pub(crate) width: u16,
}

struct HeldIndicator {
    line: Line<'static>,
    shown_at: std::time::Instant,
    /// The state behind `line` is gone and only the hold keeps it painted.
    stale: bool,
}

/// How long the running indicator outlives the state that produced it.
///
/// Between two tool rounds the turn briefly reports nothing running. The
/// indicator owns two rows, so dropping it for those frames made the whole
/// transcript jump down and back up on every round.
#[cfg(not(test))]
const INDICATOR_HOLD: std::time::Duration = std::time::Duration::from_millis(350);
// Render tests assert the frame right after a state change.
#[cfg(test)]
const INDICATOR_HOLD: std::time::Duration = std::time::Duration::ZERO;

/// `(session, history revision, first index, end index, width, theme)`.
type CommittedKey = (u64, u64, usize, usize, u16, u64);

/// One tool block in the transcript: the history range `[first, end)` of the
/// tool results it shows.
pub(crate) type ToolBlock = (usize, usize);

/// Rendered committed blocks kept for scrolling back through a long session.
const COMMITTED_CACHE_BLOCKS: usize = 4096;

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
            panel_max_scroll_row: 0,
            task_log_max_scroll_row: 0,
            committed_cache: None,
            tool_owner_cache: None,
            expand_progress: None,
            tool_line_flags: None,
            tool_rows: (ratatui::layout::Rect::default(), Vec::new()),
            tool_click: None,
            hovered_tool_block: None,
            message_line_flags: None,
            message_rows: (ratatui::layout::Rect::default(), Vec::new()),
            hovered_message: None,
            tasks_chip: None,
            tasks_chip_hovered: false,
            posture_chip: None,
            posture_chip_hovered: false,
            panel_targets: Vec::new(),
            hovered_target: None,
            pointer: None,
            held_indicator: None,
            composer_mouse: super::ComposerMouse::default(),
            viz_views: std::collections::BTreeMap::new(),
            viz_hit_cache: None,
            viz_line_flags: None,
            viz_rows: Vec::new(),
            viz_click: None,
            hovered_viz: None,
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

/// The finest wheel step: one row per event.
///
/// Selected wheel scrolling applies the same delta in the next frame.
/// Keyboard and edge-drag scrolling also advance one row at a time. The step
/// the wheel actually uses is [`wheel_scroll_lines`].
pub(crate) const WHEEL_SCROLL_LINES: usize = 1;

/// The configured wheel step; 0 until startup sets it.
static CONFIGURED_WHEEL_LINES: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// Rows one wheel event moves unless `scroll_lines` says otherwise.
///
/// One row per event (#1800): the terminal decides how many events a wheel
/// notch or trackpad gesture sends, so its own setting (kitty's
/// `touch_scroll_multiplier` on Wayland, for one) speeds up every TUI at once.
pub(crate) const fn default_wheel_scroll_lines() -> usize {
    WHEEL_SCROLL_LINES
}

/// Apply `scroll_lines` from the config: `None` keeps the default.
pub(crate) fn set_wheel_scroll_lines(configured: Option<u16>) {
    let lines = configured.map_or(default_wheel_scroll_lines(), |lines| {
        usize::from(rustcode::controller::effective_scroll_lines(Some(lines)))
    });
    CONFIGURED_WHEEL_LINES.store(lines, std::sync::atomic::Ordering::Relaxed);
}

/// Rows one mouse wheel event moves the transcript or a scrollable panel.
pub(crate) fn wheel_scroll_lines() -> usize {
    match CONFIGURED_WHEEL_LINES.load(std::sync::atomic::Ordering::Relaxed) {
        0 => default_wheel_scroll_lines(),
        lines => lines,
    }
}

/// Upper bound on the reading offset between two frames. Each projection
/// clamps the offset to the real transcript height, so this only stops a
/// burst of wheel events from overflowing before the next frame.
const MAX_SCROLL_ROWS: usize = 1_000_000;

impl TranscriptState {
    pub(crate) fn scroll_up(&mut self, rows: usize) {
        self.scroll_rows = self.scroll_rows.saturating_add(rows).min(MAX_SCROLL_ROWS);
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

    /// Keep the running indicator steady across gaps shorter than
    /// [`INDICATOR_HOLD`]. `hold_allowed` is false when the indicator is
    /// hidden on purpose, such as behind an approval prompt.
    pub(super) fn settle_indicator(
        &mut self,
        current: Option<Line<'static>>,
        hold_allowed: bool,
    ) -> Option<Line<'static>> {
        self.settle_indicator_at(
            current,
            hold_allowed,
            std::time::Instant::now(),
            INDICATOR_HOLD,
        )
    }

    fn settle_indicator_at(
        &mut self,
        current: Option<Line<'static>>,
        hold_allowed: bool,
        now: std::time::Instant,
        hold: std::time::Duration,
    ) -> Option<Line<'static>> {
        if let Some(line) = current {
            self.held_indicator = Some(HeldIndicator {
                line: line.clone(),
                shown_at: now,
                stale: false,
            });
            return Some(line);
        }
        let mut held = self.held_indicator.take()?;
        if hold_allowed && now.saturating_duration_since(held.shown_at) < hold {
            held.stale = true;
            let line = held.line.clone();
            self.held_indicator = Some(held);
            return Some(line);
        }
        None
    }

    /// Time left before a held indicator must be repainted away, so the
    /// runtime can schedule that frame even when nothing else changes.
    pub(crate) fn indicator_hold_remaining(&self) -> Option<std::time::Duration> {
        let held = self.held_indicator.as_ref().filter(|held| held.stale)?;
        Some(INDICATOR_HOLD.saturating_sub(held.shown_at.elapsed()))
    }

    pub(super) fn set_tool_rows(
        &mut self,
        area: ratatui::layout::Rect,
        rows: Vec<Option<ToolBlock>>,
    ) {
        // A block that scrolled away or was replaced is no longer hovered.
        if self
            .hovered_tool_block
            .is_some_and(|hovered| !rows.contains(&Some(hovered)))
        {
            self.hovered_tool_block = None;
        }
        // A panel over the bottom of the chat cuts the area short; the rows
        // under it are not on screen.
        let mut rows = rows;
        rows.truncate(usize::from(area.height));
        self.tool_rows = (area, rows);
    }

    /// The tool block the last painted frame showed at this cell.
    pub(crate) fn tool_block_at(&self, column: u16, row: u16) -> Option<ToolBlock> {
        let (area, rows) = &self.tool_rows;
        if !area.contains(ratatui::layout::Position::new(column, row)) {
            return None;
        }
        rows.get(usize::from(row - area.y)).copied().flatten()
    }

    pub(super) fn set_message_rows(
        &mut self,
        area: ratatui::layout::Rect,
        rows: Vec<Option<usize>>,
    ) {
        if self
            .hovered_message
            .is_some_and(|hovered| !rows.contains(&Some(hovered)))
        {
            self.hovered_message = None;
        }
        let mut rows = rows;
        rows.truncate(usize::from(area.height));
        self.message_rows = (area, rows);
    }

    /// History index of the user or assistant message the last painted frame
    /// showed at this cell.
    pub(crate) fn message_at(&self, column: u16, row: u16) -> Option<usize> {
        let (area, rows) = &self.message_rows;
        if !area.contains(ratatui::layout::Position::new(column, row)) {
            return None;
        }
        rows.get(usize::from(row - area.y)).copied().flatten()
    }

    /// Whether this cell is on the footer's task counter.
    pub(crate) fn tasks_chip_at(&self, column: u16, row: u16) -> bool {
        self.tasks_chip
            .is_some_and(|area| area.contains(ratatui::layout::Position::new(column, row)))
    }

    /// Whether this cell is on the footer's permission posture token.
    pub(crate) fn posture_chip_at(&self, column: u16, row: u16) -> bool {
        self.posture_chip
            .is_some_and(|area| area.contains(ratatui::layout::Position::new(column, row)))
    }

    /// Find what a click can reach in the panel just painted into `area`: an
    /// `esc` hint closing a row, and, when the panel is a list, the rows
    /// around its `›` selection marker. Panels are read from the painted
    /// cells so every panel gets the same affordance without declaring it.
    pub(super) fn set_panel_targets(
        &mut self,
        buffer: &ratatui::buffer::Buffer,
        area: ratatui::layout::Rect,
        list: bool,
    ) {
        let area = area.intersection(buffer.area);
        // Per row: the column and symbol of its first cell with text, and the
        // column after its last.
        let rows = (area.y..area.bottom())
            .map(|y| {
                let filled = |x: &u16| !buffer[(*x, y)].symbol().trim().is_empty();
                let first = (area.x..area.right()).find(filled)?;
                let last = (area.x..area.right()).rev().find(filled)?;
                Some((first, last + 1))
            })
            .collect::<Vec<_>>();
        let mut targets = Vec::new();
        for (offset, row) in rows.iter().enumerate() {
            let Some((_, end)) = *row else { continue };
            let y = area.y + offset as u16;
            let word_start = (area.x..end)
                .rev()
                .take_while(|x| !buffer[(*x, y)].symbol().trim().is_empty())
                .last()
                .unwrap_or(end);
            let word = (word_start..end)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>();
            if word == "esc" {
                targets.push((
                    ratatui::layout::Rect::new(word_start, y, end - word_start, 1),
                    PanelTarget::Escape,
                ));
            }
        }
        let selected = rows.iter().enumerate().find_map(|(offset, row)| {
            let (first, _) = (*row)?;
            (buffer[(first, area.y + offset as u16)].symbol() == "›").then_some((offset, first))
        });
        if list && let Some((selected, marker)) = selected {
            // An item's text starts two cells in from the marker; a heading
            // or a hint row starts elsewhere and ends the list.
            let is_item = |offset: usize| {
                rows.get(offset)
                    .copied()
                    .flatten()
                    .is_some_and(|(first, _)| first == marker + 2)
            };
            let above = (0..selected).rev().take_while(|&offset| is_item(offset));
            let below = (selected + 1..rows.len()).take_while(|&offset| is_item(offset));
            for offset in above.chain(below) {
                targets.push((
                    ratatui::layout::Rect::new(area.x, area.y + offset as u16, area.width, 1),
                    PanelTarget::ListRow(offset as isize - selected as isize),
                ));
            }
        }
        if self
            .hovered_target
            .is_some_and(|hovered| !targets.iter().any(|(rect, _)| *rect == hovered))
            && self.hovered_target != self.follow_control.area()
        {
            self.hovered_target = None;
        }
        self.panel_targets = targets;
    }

    /// What a click at this cell does in the panel painted last frame. An
    /// `esc` hint inside a list row wins over the row.
    pub(crate) fn panel_target_at(&self, column: u16, row: u16) -> Option<PanelTarget> {
        self.panel_target_rect_at(column, row)
            .map(|(_, target)| target)
    }

    fn panel_target_rect_at(
        &self,
        column: u16,
        row: u16,
    ) -> Option<(ratatui::layout::Rect, PanelTarget)> {
        let position = ratatui::layout::Position::new(column, row);
        self.panel_targets
            .iter()
            .filter(|(rect, _)| rect.contains(position))
            .min_by_key(|(rect, _)| rect.width)
            .copied()
    }

    /// Move the hover to whatever clickable thing is at this cell. Returns
    /// whether it changed, so pointer motion inside one target costs no frame.
    pub(crate) fn hover_at(&mut self, column: u16, row: u16) -> bool {
        self.pointer = Some((column, row));
        self.resolve_hover(column, row)
    }

    /// Point the hover at what the frame being painted shows under the
    /// pointer, which has not moved but may now be over something else.
    pub(super) fn refresh_hover(&mut self) {
        if let Some((column, row)) = self.pointer {
            self.resolve_hover(column, row);
        }
    }

    fn resolve_hover(&mut self, column: u16, row: u16) -> bool {
        let block = self.tool_block_at(column, row);
        let chip = self.tasks_chip_at(column, row);
        let posture = self.posture_chip_at(column, row);
        let target = self
            .panel_target_rect_at(column, row)
            .map(|(rect, _)| rect)
            .or_else(|| {
                self.follow_control
                    .area()
                    .filter(|area| area.contains(ratatui::layout::Position::new(column, row)))
            });
        // A panel painted over the chat owns the pointer.
        let message = self
            .message_at(column, row)
            .filter(|_| target.is_none() && self.panel_selection_area.is_none());
        let viz = self
            .viz_rect_at(column, row)
            .map(|(rect, _)| rect)
            .filter(|_| target.is_none() && self.panel_selection_area.is_none());
        let changed = block != self.hovered_tool_block
            || chip != self.tasks_chip_hovered
            || posture != self.posture_chip_hovered
            || target != self.hovered_target
            || message != self.hovered_message
            || viz != self.hovered_viz;
        self.hovered_viz = viz;
        self.hovered_message = message;
        self.hovered_tool_block = block;
        self.tasks_chip_hovered = chip;
        self.posture_chip_hovered = posture;
        self.hovered_target = target;
        changed
    }

    /// Light the panel target or follow control under the pointer.
    pub(super) fn highlight_hovered_target(&self, buffer: &mut ratatui::buffer::Buffer) {
        let Some(hovered) = self.hovered_target else {
            return;
        };
        let still_painted = self.follow_control.area() == Some(hovered)
            || self.panel_targets.iter().any(|(rect, _)| *rect == hovered);
        if !still_painted {
            return;
        }
        let hovered = hovered.intersection(buffer.area);
        for y in hovered.y..hovered.bottom() {
            for x in hovered.x..hovered.right() {
                buffer[(x, y)].set_bg(super::COLOR_HOVER_BG());
            }
        }
    }

    /// Light the hovered block's rows in the painted frame.
    ///
    /// `lights` decides which blocks get the hover background: file edits
    /// stay unlit so a diff never flashes under the pointer, while every
    /// block stays a click and right-click copy target.
    pub(super) fn highlight_hovered_tool_block(
        &self,
        buffer: &mut ratatui::buffer::Buffer,
        lights: impl Fn(ToolBlock) -> bool,
    ) {
        let Some(hovered) = self.hovered_tool_block.filter(|&block| lights(block)) else {
            return;
        };
        let (area, rows) = &self.tool_rows;
        for (offset, block) in rows.iter().enumerate() {
            if *block != Some(hovered) {
                continue;
            }
            let y = area.y + offset as u16;
            for x in area.x..area.right() {
                buffer[(x, y)].set_bg(super::COLOR_HOVER_BG());
            }
        }
    }

    /// Light the hovered message's rows in the painted frame.
    ///
    /// `lights` decides which messages get the hover background: plain model
    /// prose stays unlit (it reads as text, not a control), while user bubbles
    /// do. Every message stays a right-click copy target either way.
    pub(super) fn highlight_hovered_message(
        &self,
        buffer: &mut ratatui::buffer::Buffer,
        lights: impl Fn(usize) -> bool,
    ) {
        let Some(hovered) = self.hovered_message.filter(|&index| lights(index)) else {
            return;
        };
        let (area, rows) = &self.message_rows;
        for (offset, message) in rows.iter().enumerate() {
            if *message != Some(hovered) {
                continue;
            }
            let y = area.y + offset as u16;
            for x in area.x..area.right() {
                buffer[(x, y)].set_bg(super::COLOR_HOVER_BG());
            }
        }
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
        let key = self.committed_key(state, index, index, width);
        if let Some(lines) = self.committed_cache().get(&key) {
            return Arc::clone(lines);
        }
        self.render_committed_block(state, index, width, key).0
    }

    /// Render the block behind `key` and cache its rows and widget targets.
    fn render_committed_block(
        &mut self,
        state: &super::RenderSnapshot,
        index: usize,
        width: u16,
        key: CommittedKey,
    ) -> (
        Arc<Vec<Line<'static>>>,
        Arc<Vec<super::visualization::VizHit>>,
    ) {
        let (lines, hits) = super::render_committed_history_block_interactive(
            state,
            index,
            width,
            self.viz_views_for(index),
        );
        let (lines, hits) = (Arc::new(lines), Arc::new(hits));
        self.committed_cache().insert(key, Arc::clone(&lines));
        self.viz_hit_cache().insert(key, Arc::clone(&hits));
        (lines, hits)
    }

    /// The clickable widget spans of [`Self::committed_block`].
    pub(crate) fn committed_block_viz_hits(
        &mut self,
        state: &super::RenderSnapshot,
        index: usize,
        width: u16,
    ) -> Arc<Vec<super::visualization::VizHit>> {
        let key = self.committed_key(state, index, index, width);
        if let Some(hits) = self.viz_hit_cache().get(&key) {
            return Arc::clone(hits);
        }
        self.render_committed_block(state, index, width, key).1
    }

    fn viz_views_for(&self, message: usize) -> Vec<(usize, super::visualization::VizView)> {
        self.viz_views
            .range((message, 0)..(message + 1, 0))
            .map(|(&(_, ordinal), view)| (ordinal, view.clone()))
            .collect()
    }

    /// Apply a click on a widget target: sort the table or open or close the
    /// tree node. A reader scrolled up keeps the clicked row where it is.
    pub(crate) fn apply_viz_target(&mut self, state: &super::RenderSnapshot, target: VizTarget) {
        let before = self
            .committed_block(state, target.message, target.width)
            .len();
        let key = (target.message, target.ordinal);
        let view = self.viz_views.entry(key).or_default();
        if view.source != target.source {
            *view = super::visualization::VizView {
                source: target.source,
                ..Default::default()
            };
        }
        view.apply(target.action);
        if view.sort.is_none() && view.toggled.is_empty() {
            self.viz_views.remove(&key);
        }
        let after = self
            .committed_block(state, target.message, target.width)
            .len();
        if self.scroll_rows > 0 && after != before {
            // Rows came or went below the clicked one; the offset counts
            // from the bottom.
            self.shift_reading_offset(after as isize - before as isize);
            self.reading_anchor = None;
        }
    }

    pub(super) fn set_viz_rows(
        &mut self,
        area: ratatui::layout::Rect,
        targets: Vec<(u16, std::ops::Range<u16>, VizTarget)>,
    ) {
        self.viz_rows = targets
            .into_iter()
            .filter(|(row, columns, _)| *row < area.height && columns.start < columns.end)
            .map(|(row, columns, target)| {
                let rect = ratatui::layout::Rect::new(
                    area.x.saturating_add(columns.start),
                    area.y + row,
                    columns.end - columns.start,
                    1,
                )
                .intersection(area);
                (rect, target)
            })
            .filter(|(rect, _)| !rect.is_empty())
            .collect();
        if self
            .hovered_viz
            .is_some_and(|hovered| !self.viz_rows.iter().any(|(rect, _)| *rect == hovered))
        {
            self.hovered_viz = None;
        }
    }

    fn viz_rect_at(&self, column: u16, row: u16) -> Option<(ratatui::layout::Rect, VizTarget)> {
        let position = ratatui::layout::Position::new(column, row);
        self.viz_rows
            .iter()
            .find(|(rect, _)| rect.contains(position))
            .copied()
    }

    /// The widget target the last painted frame showed at this cell.
    pub(crate) fn viz_target_at(&self, column: u16, row: u16) -> Option<VizTarget> {
        self.viz_rect_at(column, row).map(|(_, target)| target)
    }

    /// Light the widget target under the pointer.
    pub(super) fn highlight_hovered_viz(&self, buffer: &mut ratatui::buffer::Buffer) {
        let Some(hovered) = self.hovered_viz else {
            return;
        };
        let hovered = hovered.intersection(buffer.area);
        for y in hovered.y..hovered.bottom() {
            for x in hovered.x..hovered.right() {
                buffer[(x, y)].set_bg(super::COLOR_HOVER_BG());
            }
        }
    }

    /// The committed tool chain `first..end`, including its trailing blank
    /// row, cached like [`Self::committed_block`].
    ///
    /// A reader far up a long session walks every block between the newest row
    /// and the viewport on each frame, so an uncached chain re-rendered its
    /// whole output once per wheel tick.
    pub(crate) fn committed_tool_group(
        &mut self,
        state: &super::RenderSnapshot,
        first: usize,
        end: usize,
        width: u16,
    ) -> Arc<Vec<Line<'static>>> {
        let key = self.committed_key(state, first, end, width);
        if let Some(lines) = self.committed_cache().get(&key) {
            return Arc::clone(lines);
        }
        self.render_tool_group(state, first, end, width, key).0
    }

    /// Render the chain behind `key` and cache its rows and their owners.
    fn render_tool_group(
        &mut self,
        state: &super::RenderSnapshot,
        first: usize,
        end: usize,
        width: u16,
        key: CommittedKey,
    ) -> (Arc<Vec<Line<'static>>>, Arc<Vec<Option<ToolBlock>>>) {
        let indices = (first..end)
            .filter(|&index| state.history()[index].role == "tool")
            .collect::<Vec<_>>();
        let (mut block, mut owners) =
            super::render_committed_tool_result_group_rows(state, &indices, width, false);
        // Calls still in flight are the next rows of this block, so nothing
        // separates them from it.
        if !block.is_empty() && !Self::block_stays_open(state, end) {
            block.push(Line::from(""));
            owners.push(None);
        }
        let (lines, owners) = (Arc::new(block), Arc::new(owners));
        self.committed_cache().insert(key, Arc::clone(&lines));
        self.tool_owner_cache().insert(key, Arc::clone(&owners));
        (lines, owners)
    }

    /// For each row of [`Self::committed_tool_group`], the call it belongs
    /// to, or the folded run a summary row stands for.
    pub(crate) fn committed_tool_group_owners(
        &mut self,
        state: &super::RenderSnapshot,
        first: usize,
        end: usize,
        width: u16,
    ) -> Arc<Vec<Option<ToolBlock>>> {
        let key = self.committed_key(state, first, end, width);
        if let Some(owners) = self.tool_owner_cache().get(&key) {
            return Arc::clone(owners);
        }
        self.render_tool_group(state, first, end, width, key).1
    }

    /// Whether the tool block ending at `end` is the one live calls continue.
    fn block_stays_open(state: &super::RenderSnapshot, end: usize) -> bool {
        end == state.history().len()
            && super::conversation_render::live_tools_join_committed_block(state)
    }

    /// The state of the child a `spawn_agent` result at `message` started, as
    /// its row draws it: a row follows its agent live (#2327). A running
    /// child's clock is not part of it, so its row is not redrawn each second.
    fn spawned_agent_state(
        state: &super::RenderSnapshot,
        message: usize,
    ) -> Option<(u32, u8, Option<u64>)> {
        let message = state.history().get(message)?;
        if message.role != "tool" {
            return None;
        }
        let result = message.content.strip_prefix("spawn_agent: ")?;
        let id = super::tool_transcript::spawn_receipt_agent_id(result)?;
        let agent = state.subagents().iter().find(|agent| agent.id == id)?;
        use rustcode::controller::SubAgentStatus;
        let finished = !matches!(
            agent.status,
            SubAgentStatus::Queued | SubAgentStatus::Running
        );
        Some((
            id,
            agent.status as u8,
            finished.then_some(agent.elapsed_ms / 1000),
        ))
    }

    /// `end == index` names a single block; a tool chain uses its exclusive
    /// end, which is always greater, so the two never share a key.
    fn committed_key(
        &self,
        state: &super::RenderSnapshot,
        index: usize,
        end: usize,
        width: u16,
    ) -> CommittedKey {
        let mut theme_hash = std::collections::hash_map::DefaultHasher::new();
        super::theme::active_palette().name.hash(&mut theme_hash);
        // Tool rows also depend on presentation settings outside history.
        state.verbosity().hash(&mut theme_hash);
        state.home_path().hash(&mut theme_hash);
        Self::block_stays_open(state, end).hash(&mut theme_hash);
        for message in index..end.max(index + 1) {
            state
                .expanded_thoughts()
                .contains(&message)
                .hash(&mut theme_hash);
            state.tool_run_open(message).hash(&mut theme_hash);
            Self::spawned_agent_state(state, message).hash(&mut theme_hash);
        }
        // A sorted table or collapsed tree draws different rows.
        if end == index {
            for view in self.viz_views.range((index, 0)..(index + 1, 0)) {
                view.hash(&mut theme_hash);
            }
        }
        let mut session_hash = std::collections::hash_map::DefaultHasher::new();
        state.active_session_id().hash(&mut session_hash);
        (
            session_hash.finish(),
            state.history().revision(),
            index,
            end,
            width,
            theme_hash.finish(),
        )
    }

    /// How much of the transcript is expanded, as `(expanded, collapsible)`,
    /// recomputed only when something it depends on changed.
    pub(super) fn expand_progress(
        &mut self,
        state: &super::RenderSnapshot,
        width: u16,
    ) -> (usize, usize) {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        state.active_session_id().hash(&mut hasher);
        state.selected_subagent_id().hash(&mut hasher);
        state.history().revision().hash(&mut hasher);
        width.hash(&mut hasher);
        super::theme::active_palette().name.hash(&mut hasher);
        state.verbosity().hash(&mut hasher);
        state.home_path().hash(&mut hasher);
        // A subagent's history has no revision of its own: fingerprint the
        // shape of whichever history is on screen.
        let history = state.active_history();
        history.len().hash(&mut hasher);
        for message in history {
            message.content.len().hash(&mut hasher);
            message.tool_calls.len().hash(&mut hasher);
        }
        // Order-independent: the set iterates in arbitrary order.
        let expanded = state.expanded_thoughts();
        expanded.len().hash(&mut hasher);
        expanded
            .iter()
            .fold(0u64, |acc, index| {
                acc ^ (*index as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)
            })
            .hash(&mut hasher);
        let key = hasher.finish();
        if let Some((cached_key, progress)) = self.expand_progress
            && cached_key == key
        {
            return progress;
        }
        let progress = super::tool_transcript::expand_progress(state, width);
        self.expand_progress = Some((key, progress));
        progress
    }

    fn committed_cache(
        &mut self,
    ) -> &mut super::lru::LruCache<CommittedKey, Arc<Vec<Line<'static>>>> {
        // The walk from the newest row to a deep reading position touches
        // every block in between on each frame. A cache smaller than that span
        // evicts what the next frame needs and re-renders the whole span per
        // wheel tick, which is what froze fast scrolling in long sessions.
        self.committed_cache
            .get_or_insert_with(|| super::lru::LruCache::new(COMMITTED_CACHE_BLOCKS))
    }

    fn viz_hit_cache(
        &mut self,
    ) -> &mut super::lru::LruCache<CommittedKey, Arc<Vec<super::visualization::VizHit>>> {
        self.viz_hit_cache
            .get_or_insert_with(|| super::lru::LruCache::new(COMMITTED_CACHE_BLOCKS))
    }

    fn tool_owner_cache(
        &mut self,
    ) -> &mut super::lru::LruCache<CommittedKey, Arc<Vec<Option<ToolBlock>>>> {
        self.tool_owner_cache
            .get_or_insert_with(|| super::lru::LruCache::new(COMMITTED_CACHE_BLOCKS))
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

    /// Label the live message's reasoning as a provider summary or not.
    pub(crate) fn set_assistant_reasoning_summary(&mut self, reasoning_summary: bool) {
        if let Some(cell) = self.assistant.as_mut()
            && cell.reasoning_summary != reasoning_summary
        {
            cell.reasoning_summary = reasoning_summary;
            cell.cached_display.replace(None);
            self.revision = self.revision.saturating_add(1);
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
                continues: false,
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
    /// Its `<think>` spans are the provider's reasoning summary.
    reasoning_summary: bool,
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
    /// The rows continue the committed tool block above them, so they carry
    /// no heading of their own.
    continues: bool,
}

impl HistoryCell for LiveToolCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines = render_live_tool_cell_with_verbosity(
            &self.calls,
            width,
            &self.verbosity,
            false,
            self.home_path.as_deref(),
        );
        if self.continues && !lines.is_empty() {
            lines.remove(0);
        }
        lines
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
            reasoning_summary: false,
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
            reasoning_summary: false,
            generating: true,
            continuation,
            cached_display: RefCell::new(None),
            streaming_markdown: RefCell::new(Vec::new()),
        }
    }
}

impl AssistantMarkdownCell {
    pub(super) fn with_reasoning_summary(mut self, reasoning_summary: bool) -> Self {
        self.reasoning_summary = reasoning_summary;
        self
    }

    /// Rows of a finalized message drawn under its widgets' view state, and
    /// the clickable spans of those widgets. Not cached here: the committed
    /// block cache keys on the view state instead.
    pub(super) fn interactive_display_lines(
        &self,
        width: u16,
        views: Vec<(usize, super::visualization::VizView)>,
    ) -> (Vec<Line<'static>>, Vec<super::visualization::VizHit>) {
        debug_assert!(!self.generating, "a live stream is never interactive");
        let mut viz = super::visualization::VizContext {
            views,
            hits: Vec::new(),
        };
        let lines = self.render_lines(width, Some(&mut viz));
        (lines, viz.hits)
    }

    fn render_lines(
        &self,
        width: u16,
        viz: Option<&mut super::visualization::VizContext>,
    ) -> Vec<Line<'static>> {
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
                reasoning_summary: self.reasoning_summary,
                is_generating: self.generating,
                viewport_width: width,
                show_picker: false,
                last_copy_text: None,
            },
            self.generating.then_some(&mut *streaming_markdown),
            viz.filter(|_| !self.generating),
        );
        if self.continuation {
            super::demote_assistant_bullet(&mut lines);
        }
        if self.generating {
            while lines.last().is_some_and(|line| line.spans.is_empty()) {
                lines.pop();
            }
        }
        lines
            .into_iter()
            .map(|line| super::own_line(&line))
            .collect::<Vec<_>>()
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
        let lines = self.render_lines(width, None);
        self.cached_display
            .replace(Some((width, theme_name, lines.clone())));
        lines
    }
}

pub(super) fn is_live_tool_call_visible(call: &LiveToolCall) -> bool {
    call.execution_started || (!call.target.is_empty() && call.target != "?")
}

/// How long a call must have been in flight before it is drawn as running.
/// A read or an edit finishes well inside this and goes straight to `Ran`;
/// drawing it first made every fast call flash a `Running` block.
pub(super) const LIVE_TOOL_CALL_GRACE: std::time::Duration = std::time::Duration::from_millis(200);

/// Whether a visible call has been in flight long enough to draw.
pub(super) fn live_tool_call_is_settled(call: &LiveToolCall) -> bool {
    // A call that ended inside the grace period never earned a row of its
    // own; it only rides along once the block is on screen for another call.
    if let Some(finish) = call.finished {
        return finish.elapsed >= LIVE_TOOL_CALL_GRACE;
    }
    // Rendering tests build calls and draw them in the same instant, and the
    // frozen `/test` preview pins its calls to a start time in the future.
    cfg!(test)
        || call.started_at > std::time::Instant::now()
        || call.started_at.elapsed() >= LIVE_TOOL_CALL_GRACE
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
    let calls = calls.iter().collect::<Vec<_>>();
    render_live_tool_cell_at_refs(&calls, width, verbosity, show_picker, now, home_path)
}

pub(super) fn render_live_tool_cell_at_refs(
    calls: &[&LiveToolCall],
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

    // Calls in flight take the shape and the heading of the committed block
    // they become, so finishing changes a row's glyph and nothing else. How to
    // interrupt them is on the status row (#1850).
    let detail_style = get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker);
    let action_style = get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::BOLD, show_picker);
    let target_style = get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::empty(), show_picker);
    let mut lines = vec![fit_live_row(
        super::tool_transcript::tool_group_header(
            super::tool_transcript::TOOL_BLOCK_HEADING,
            show_picker,
        ),
        usize::from(width),
    )];
    let lone_command = calls.len() == 1 && calls[0].tool_name == "run_command";
    for (call_index, call) in calls.iter().take(MAX_LIVE_CHILDREN).enumerate() {
        let target = super::tool_transcript::contract_home_path(&call.target, home_path);
        let target = if target.is_empty() || target == "?" {
            String::new()
        } else {
            target.split_whitespace().collect::<Vec<_>>().join(" ")
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
        let mut suffix = if matching > 1 {
            format!(" · {ordinal}/{matching}")
        } else {
            String::new()
        };
        // How long the call has run sits behind it. A call still waiting its
        // turn looks the same without a time: it is in flight either way.
        if call.execution_started && call.finished.is_none() {
            let elapsed = now.saturating_duration_since(call.started_at).as_secs();
            if elapsed >= 1 {
                suffix.push_str(&format!(" · {}", super::fmt_elapsed_compact(elapsed)));
            }
        }
        // The same one-line layout as a finished row: the state keeps its
        // place and the target takes what is left.
        let state = if suffix.is_empty() {
            Vec::new()
        } else {
            vec![super::tool_transcript::RowState::required(Span::styled(
                suffix,
                detail_style,
            ))]
        };
        let lead = vec![Span::styled(
            // A call that has ended keeps its row, with the glyph it will
            // have once its batch is recorded.
            format!(
                "  {} ",
                match call.finished {
                    None => super::tool_transcript::LIVE_ROW_GLYPH,
                    Some(finish) if finish.success => '✓',
                    Some(_) => '✗',
                }
            ),
            detail_style,
        )];
        let label = vec![Span::styled(call.action.clone(), action_style)];
        let mut target = if target.is_empty() {
            Vec::new()
        } else {
            vec![Span::styled(target, target_style)]
        };
        super::tool_transcript::push_call_summary(
            &lead,
            &label,
            &mut target,
            &state,
            call.summary.as_deref(),
            usize::from(width),
            show_picker,
        );
        lines.push(super::tool_transcript::fit_tool_row(
            lead,
            label,
            target,
            // A command is recognised by how it starts, a path by how it ends.
            if call.tool_name == "run_command" {
                super::tool_transcript::TargetKeep::Head
            } else {
                super::tool_transcript::TargetKeep::Tail
            },
            state,
            usize::from(width),
        ));
        if lone_command && let Some(cwd) = &call.cwd {
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
                Some(Span::styled("    ", detail_style)),
            );
        }
        // High verbosity keeps output closed while a call runs, as it does
        // once the call has finished.
        if !call.execution_started || call.finished.is_some() || verbosity.closed_by_default() {
            continue;
        }
        if lone_command {
            lines.extend(live_command_output(call, width, show_picker));
        } else if let Some(latest) = call
            .output
            .iter()
            .flat_map(|chunk| chunk.text.lines())
            .filter(|line| !line.trim().is_empty())
            .next_back()
        {
            lines.push(Line::from(vec![
                super::tool_transcript::tool_body_spine(show_picker),
                Span::styled(
                    truncate_to_width(
                        &rustcode_tool_protocol::text::strip_ansi_escapes(latest),
                        usize::from(width).saturating_sub(4).max(1),
                    ),
                    detail_style,
                ),
            ]));
        }
    }
    if calls.len() > MAX_LIVE_CHILDREN {
        lines.push(Line::from(vec![
            Span::styled("  ", detail_style),
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

/// The bounded window of a running command's output: its first and last rows
/// around a count of what is left out.
fn live_command_output(call: &LiveToolCall, width: u16, show_picker: bool) -> Vec<Line<'static>> {
    let mut body = Vec::new();
    for chunk in &call.output {
        let clean = rustcode_tool_protocol::text::strip_ansi_escapes(&chunk.text);
        for text in clean.split('\n').filter(|line| !line.is_empty()) {
            let spans = vec![
                super::tool_transcript::tool_body_spine(show_picker),
                Span::styled(
                    text.to_owned(),
                    get_themed_style(
                        if chunk.stderr {
                            COLOR_TIP()
                        } else {
                            COLOR_MUTED()
                        },
                        COLOR_BG(),
                        if chunk.stderr {
                            Modifier::empty()
                        } else {
                            Modifier::DIM
                        },
                        show_picker,
                    ),
                ),
            ];
            push_wrapped_with_continuation(
                &mut body,
                spans,
                (width as usize).max(10),
                Some(Span::styled(
                    "  │ ",
                    get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::DIM, show_picker),
                )),
            );
        }
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
        marker = truncate_to_width(&marker, (width as usize).saturating_sub(4).max(1));
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
    body
}

#[cfg(test)]
mod tests {
    use super::{AssistantMarkdownCell, HistoryCell, PanelTarget, TranscriptState};

    fn painted(rows: &[&str]) -> (ratatui::buffer::Buffer, ratatui::layout::Rect) {
        let area = ratatui::layout::Rect::new(0, 0, 40, rows.len() as u16);
        let mut buffer = ratatui::buffer::Buffer::empty(area);
        for (y, row) in rows.iter().enumerate() {
            buffer.set_string(0, y as u16, row, ratatui::style::Style::default());
        }
        (buffer, area)
    }

    #[test]
    fn a_panel_offers_its_esc_hint_and_list_rows_to_the_pointer() {
        let (mut buffer, area) = painted(&[
            "  Select model                      esc",
            "",
            "  › first/model            First Model",
            "    second/model          Second Model",
            "    third/model            Third Model",
            "  select ↑/↓  confirm enter  cancel esc",
        ]);
        let mut transcript = TranscriptState::default();
        transcript.set_panel_targets(&buffer, area, true);

        // Both `esc` hints close the panel; the title and the blank row do nothing.
        assert_eq!(transcript.panel_target_at(37, 0), Some(PanelTarget::Escape));
        assert_eq!(transcript.panel_target_at(38, 5), Some(PanelTarget::Escape));
        assert_eq!(transcript.panel_target_at(4, 0), None);
        assert_eq!(transcript.panel_target_at(4, 1), None);
        // Rows are counted from the selected one, which is not a target
        // itself, and the hint row under the list is not an item.
        assert_eq!(transcript.panel_target_at(10, 2), None);
        assert_eq!(
            transcript.panel_target_at(10, 3),
            Some(PanelTarget::ListRow(1))
        );
        assert_eq!(
            transcript.panel_target_at(30, 4),
            Some(PanelTarget::ListRow(2))
        );
        assert_eq!(transcript.panel_target_at(4, 5), None);

        // The row under the pointer is lit, and only that row.
        assert!(transcript.hover_at(10, 3));
        assert!(!transcript.hover_at(12, 3));
        transcript.highlight_hovered_target(&mut buffer);
        assert_eq!(buffer[(0, 3)].bg, crate::ui::COLOR_HOVER_BG());
        assert_ne!(buffer[(0, 4)].bg, crate::ui::COLOR_HOVER_BG());

        // A panel that is not a list offers only its `esc` hint, and a
        // target that is no longer painted stops being lit.
        transcript.set_panel_targets(&buffer, area, false);
        assert_eq!(transcript.panel_target_at(10, 3), None);
        assert_eq!(transcript.panel_target_at(37, 0), Some(PanelTarget::Escape));
        assert!(!transcript.hover_at(10, 3));
    }

    #[test]
    fn hover_follows_the_rows_that_move_under_a_still_pointer() {
        let (mut buffer, area) = painted(&["• Ran", "", "• Read", "", ""]);
        let hover = crate::ui::COLOR_HOVER_BG();
        let (ran, read) = ((2, 3), (5, 6));
        let mut transcript = TranscriptState::default();
        transcript.set_tool_rows(area, vec![Some(ran), None, Some(read), None, None]);
        assert!(transcript.hover_at(1, 2));

        // The indicator row went away and every row moved down two.
        transcript.set_tool_rows(area, vec![None, None, Some(ran), None, Some(read)]);
        transcript.refresh_hover();
        transcript.highlight_hovered_tool_block(&mut buffer, |_| true);
        assert_eq!(buffer[(0, 2)].bg, hover);
        assert_ne!(buffer[(0, 4)].bg, hover);
        assert_eq!(transcript.tool_block_at(1, 2), Some(ran));
    }

    #[test]
    fn hover_stays_out_of_the_rows_a_panel_covers() {
        let (mut buffer, area) = painted(&["• Ran", "  Bash ls", "  question", "  › yes"]);
        let hover = crate::ui::COLOR_HOVER_BG();
        let visible = ratatui::layout::Rect::new(area.x, area.y, area.width, 2);
        let mut transcript = TranscriptState::default();
        transcript.set_tool_rows(visible, vec![Some((2, 3)); 4]);
        assert!(transcript.hover_at(1, 0));
        transcript.highlight_hovered_tool_block(&mut buffer, |_| true);
        assert_eq!(buffer[(0, 1)].bg, hover);
        assert_ne!(buffer[(0, 2)].bg, hover);
        assert_ne!(buffer[(0, 3)].bg, hover);
    }

    #[test]
    fn hovering_an_edit_block_keeps_it_clickable_without_lighting_it() {
        let (mut buffer, area) = painted(&["• Edited", "  src/main.rs"]);
        let edit = (2, 3);
        let mut transcript = TranscriptState::default();
        transcript.set_tool_rows(area, vec![Some(edit); 2]);
        assert!(transcript.hover_at(1, 1));
        assert_eq!(transcript.tool_block_at(1, 1), Some(edit));
        transcript.highlight_hovered_tool_block(&mut buffer, |block| block != edit);
        assert_ne!(buffer[(0, 1)].bg, crate::ui::COLOR_HOVER_BG());
    }

    #[test]
    fn hovering_a_message_lights_its_rows_and_names_it_for_copy() {
        let (mut buffer, area) = painted(&["› first", "  second", "", "• answer"]);
        let hover = crate::ui::COLOR_HOVER_BG();
        let mut transcript = TranscriptState::default();
        transcript.set_message_rows(area, vec![Some(0), Some(0), None, Some(1)]);

        assert!(transcript.hover_at(3, 1));
        // Moving within the same message costs no frame.
        assert!(!transcript.hover_at(5, 0));
        assert_eq!(transcript.message_at(5, 0), Some(0));
        assert_eq!(transcript.message_at(5, 2), None);
        transcript.highlight_hovered_message(&mut buffer, |_| true);
        assert_eq!(buffer[(0, 0)].bg, hover);
        assert_eq!(buffer[(area.right() - 1, 1)].bg, hover);
        assert_ne!(buffer[(0, 3)].bg, hover);

        // A message that scrolled out of the frame is no longer lit.
        transcript.set_message_rows(area, vec![None, None, None, Some(1)]);
        let (mut repainted, _) = painted(&["", "", "", "• answer"]);
        transcript.highlight_hovered_message(&mut repainted, |_| true);
        assert_ne!(repainted[(0, 0)].bg, hover);
    }

    #[test]
    fn hovering_model_prose_keeps_it_a_copy_target_without_lighting_it() {
        let (mut buffer, area) = painted(&["• answer"]);
        let mut transcript = TranscriptState::default();
        transcript.set_message_rows(area, vec![Some(1)]);
        assert!(transcript.hover_at(2, 0));
        assert_eq!(transcript.message_at(2, 0), Some(1));
        transcript.highlight_hovered_message(&mut buffer, |index| index != 1);
        assert_ne!(buffer[(0, 0)].bg, crate::ui::COLOR_HOVER_BG());
    }

    #[test]
    fn completion_menu_rows_are_click_targets() {
        let (buffer, area) = painted(&[
            "› /model   Open model picker",
            "  /models  Open the model picker",
        ]);
        let mut transcript = TranscriptState::default();
        transcript.set_panel_targets(&buffer, area, true);
        assert_eq!(
            transcript.panel_target_at(4, 1),
            Some(PanelTarget::ListRow(1))
        );
        assert_eq!(transcript.panel_target_at(4, 0), None);
    }
    use rustcode::controller::{ChatMessage, History, RenderState};

    #[test]
    fn remote_panel_clears_a_hidden_indicator_hold() {
        let _theme_guard = super::super::tests::THEME_TEST_LOCK
            .lock()
            .expect("theme test lock");
        let mut transcript = TranscriptState::default();
        let now = std::time::Instant::now();
        let hold = std::time::Duration::from_secs(1);
        transcript.settle_indicator_at(
            Some(ratatui::text::Line::from("Executing")),
            true,
            now,
            hold,
        );
        transcript.settle_indicator_at(None, true, now, hold);
        assert!(transcript.indicator_hold_remaining().is_some());
        let mut state = RenderState::new();
        state.command_panel = Some(rustcode::controller::CommandPanel {
            title: "Remote",
            content: "Pair a device".into(),
        });
        let mut terminal =
            crate::inline_terminal::InlineTerminal::new(ratatui::backend::TestBackend::new(80, 40))
                .unwrap();
        terminal
            .draw(|frame| {
                crate::ui::render_with_transcript(frame, &state, &mut transcript);
            })
            .unwrap();
        assert!(transcript.indicator_hold_remaining().is_none());
    }

    #[test]
    fn indicator_survives_a_short_gap_but_not_a_long_or_deliberate_one() {
        let hold = std::time::Duration::from_millis(350);
        let start = std::time::Instant::now();
        let after = |ms| start + std::time::Duration::from_millis(ms);
        let mut transcript = TranscriptState::default();
        let line = ratatui::text::Line::from("Executing");

        assert!(
            transcript
                .settle_indicator_at(None, true, start, hold)
                .is_none()
        );
        transcript.settle_indicator_at(Some(line.clone()), true, start, hold);
        assert!(transcript.indicator_hold_remaining().is_none());

        // A gap between two tool rounds keeps the row, so nothing jumps.
        assert_eq!(
            transcript.settle_indicator_at(None, true, after(100), hold),
            Some(line.clone())
        );
        assert!(transcript.indicator_hold_remaining().is_some());

        // The turn really ended: the row leaves once the hold runs out.
        assert!(
            transcript
                .settle_indicator_at(None, true, after(400), hold)
                .is_none()
        );
        assert!(transcript.indicator_hold_remaining().is_none());

        // An approval prompt hides the indicator at once.
        transcript.settle_indicator_at(Some(line), true, after(500), hold);
        assert!(
            transcript
                .settle_indicator_at(None, false, after(510), hold)
                .is_none()
        );
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
    fn deep_scroll_reuses_every_block_between_the_tail_and_the_reader() {
        let _theme_guard = super::super::tests::THEME_TEST_LOCK
            .lock()
            .expect("theme test lock");
        // More blocks than the old 32-entry cache: walking back to the first
        // one evicted the newest, so each frame re-rendered the whole span.
        let mut state = RenderState::new();
        for index in 0..200 {
            state.history.push(ChatMessage::new(
                "assistant",
                format!("history block {index:03}"),
            ));
        }
        let snapshot = crate::ui::render_snapshot::render_snapshot(&state);
        let mut transcript = TranscriptState::default();

        let first_pass = (0..200)
            .rev()
            .map(|index| transcript.committed_block(&snapshot, index, 80))
            .collect::<Vec<_>>();
        let second_pass = (0..200)
            .rev()
            .map(|index| transcript.committed_block(&snapshot, index, 80))
            .collect::<Vec<_>>();
        assert!(
            first_pass
                .iter()
                .zip(&second_pass)
                .all(|(first, second)| std::sync::Arc::ptr_eq(first, second))
        );

        // A burst of wheel events is clamped by the next frame, not by a fixed
        // row cap that stops short of the first message.
        transcript.scroll_up(50_000);
        let top = crate::ui::render_visible_conversation_with_transcript(
            &snapshot,
            80,
            30,
            &mut transcript,
        );
        assert!(transcript.scroll_rows() < 50_000);
        assert!(
            top.iter()
                .any(|line| line.to_string().contains("history block 000")),
            "{top:?}"
        );
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
