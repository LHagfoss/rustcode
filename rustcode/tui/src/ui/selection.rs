//! Mouse selection anchored to a pinned transcript viewport.

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::Line,
    widgets::{Paragraph, Wrap},
};
use std::{collections::BTreeMap, sync::Arc, time::Instant};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::RenderSnapshot;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct CellPosition {
    row: i64,
    column: usize,
}

/// One painted visual row of the transcript.
///
/// `text_end` is derived once when the row is read, so copy and highlight never
/// rescan the row's terminal-width padding.
#[derive(Clone, Eq, PartialEq)]
struct PaintedRow {
    cells: Vec<String>,
    soft_wrap_before: bool,
    text_end: usize,
}

/// Lazily rendered committed blocks for the immutable history behind an
/// active selection. Blocks are stored newest-first with cumulative row
/// counts, so scrolling deeper adds only newly exposed history and painting a
/// viewport does not revisit the selected prefix.
pub(super) struct SelectedHistoryProjection {
    blocks: Vec<Arc<Vec<Line<'static>>>>,
    rows_from_tail: Vec<usize>,
    next_index: usize,
    display_start: usize,
    width: u16,
    height: u16,
    finished: bool,
}

impl SelectedHistoryProjection {
    fn new(history_len: usize, display_start: usize, width: u16, height: u16) -> Self {
        Self {
            blocks: Vec::new(),
            rows_from_tail: vec![0],
            next_index: history_len,
            display_start,
            width,
            height,
            finished: false,
        }
    }

    pub(super) fn total_rows(&self) -> usize {
        self.rows_from_tail.last().copied().unwrap_or_default()
    }

    pub(super) fn next_index(&self) -> usize {
        self.next_index
    }

    pub(super) fn display_start(&self) -> usize {
        self.display_start
    }

    pub(super) fn finished(&self) -> bool {
        self.finished
    }

    pub(super) fn append(&mut self, block: Arc<Vec<Line<'static>>>, next_index: usize) {
        self.next_index = next_index;
        if block.is_empty() {
            return;
        }
        let total_rows = self.total_rows().saturating_add(block.len());
        self.blocks.push(block);
        self.rows_from_tail.push(total_rows);
    }

    pub(super) fn finish(&mut self) {
        self.finished = true;
    }

    /// Return a chronological line slice measured from the newest history row.
    pub(super) fn rows_from_tail_range(&self, start: usize, end: usize) -> Vec<Line<'static>> {
        let end = end.min(self.total_rows());
        if start >= end || self.blocks.is_empty() {
            return Vec::new();
        }
        let start = start.min(end);
        let newest = self
            .rows_from_tail
            .partition_point(|rows| *rows <= start)
            .saturating_sub(1)
            .min(self.blocks.len().saturating_sub(1));
        let oldest = self
            .rows_from_tail
            .partition_point(|rows| *rows < end)
            .saturating_sub(1)
            .min(self.blocks.len().saturating_sub(1));
        let mut rows = Vec::with_capacity(end - start);
        for index in (newest..=oldest).rev() {
            let block_start = self.rows_from_tail[index];
            let block_end = self.rows_from_tail[index + 1];
            let from_tail = start.max(block_start);
            let through_tail = end.min(block_end);
            if from_tail >= through_tail {
                continue;
            }
            let block = &self.blocks[index];
            let start_in_block = block.len() - (through_tail - block_start);
            let end_in_block = block.len() - (from_tail - block_start);
            rows.extend_from_slice(&block[start_in_block..end_in_block]);
        }
        rows
    }
}

impl PaintedRow {
    fn new(cells: Vec<String>, soft_wrap_before: bool) -> Self {
        let text_end = cells
            .iter()
            .rposition(|cell| !cell.trim().is_empty())
            .map_or(0, |column| column + 1);
        Self {
            cells,
            soft_wrap_before,
            text_end,
        }
    }
}

/// Reads one visual row out of a rendered frame.
///
/// Empty cells are the trailing half of a wide grapheme, so a cell covered by
/// an earlier wide symbol yields no character of its own.
fn buffer_row_cells(area: Rect, buffer: &Buffer, y: u16) -> Vec<String> {
    let mut continuation = 0;
    (area.x..area.right())
        .map(|x| {
            let symbol = buffer
                .cell((x, y))
                .map(|cell| cell.symbol().to_owned())
                .unwrap_or_default();
            if continuation > 0 {
                continuation -= 1;
                String::new()
            } else {
                continuation = symbol.width().saturating_sub(1);
                symbol
            }
        })
        .collect()
}

/// Hashes the visible buffer without allocating per-cell Strings.
///
/// `buffer_row_cells` allocates one String per cell; hashing borrows each
/// symbol so an unchanged frame can skip the scan entirely (#1582).
fn buffer_content_hash(area: Rect, buffer: &Buffer) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    area.hash(&mut hasher);
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            buffer
                .cell((x, y))
                .map(|cell| cell.symbol())
                .unwrap_or_default()
                .hash(&mut hasher);
        }
    }
    hasher.finish()
}

#[derive(Clone, Copy, Default, Eq, PartialEq)]
enum SelectionUnit {
    #[default]
    Character,
    Word,
    Line,
}

#[derive(Default)]
pub(crate) struct TranscriptSelection {
    area: Rect,
    rows: Vec<PaintedRow>,
    rows_scanned: bool,
    soft_wrap_before: Vec<bool>,
    anchor: Option<CellPosition>,
    focus: Option<CellPosition>,
    semantic_range: Option<(CellPosition, CellPosition)>,
    origin_range: Option<(CellPosition, CellPosition)>,
    unit: SelectionUnit,
    last_click: Option<(Instant, u16, u16, u8)>,
    keyboard_mode: bool,
    keyboard_anchor: Option<CellPosition>,
    keyboard_focus: Option<CellPosition>,
    pending_keyboard_move: Option<(KeyCode, i64, usize)>,
    dragging: bool,
    snapshot: Option<Arc<RenderSnapshot>>,
    selected_projection: Option<SelectedHistoryProjection>,
    pinned_width: u16,
    pinned_height: u16,
    pinned_scroll: usize,
    captured: BTreeMap<i64, PaintedRow>,
    viewport_scroll: usize,
    viewport_top_key: i64,
    pending_scroll: isize,
    pointer: Option<(u16, u16)>,
    origin_row: u16,
    moved_vertically: bool,
    last_edge_attempt: Option<(isize, usize)>,
    unpinned_hash: Option<(Rect, Vec<bool>, usize, u64)>,
}

impl TranscriptSelection {
    pub(crate) fn clear(&mut self) {
        self.anchor = None;
        self.focus = None;
        self.semantic_range = None;
        self.origin_range = None;
        self.unit = SelectionUnit::Character;
        self.keyboard_mode = false;
        self.keyboard_anchor = None;
        self.keyboard_focus = None;
        self.pending_keyboard_move = None;
        self.dragging = false;
        self.snapshot = None;
        self.selected_projection = None;
        self.captured.clear();
        self.pointer = None;
        self.moved_vertically = false;
        self.last_edge_attempt = None;
        self.pending_scroll = 0;
    }

    pub(crate) fn has_selection(&self) -> bool {
        self.range().is_some()
            || (self.keyboard_mode && self.keyboard_anchor != self.keyboard_focus)
    }

    pub(crate) fn is_dragging(&self) -> bool {
        self.dragging
    }

    pub(crate) fn is_active(&self) -> bool {
        self.snapshot.is_some()
    }

    pub(super) fn ensure_selected_projection(
        &mut self,
        history_len: usize,
        display_start: usize,
        width: u16,
        height: u16,
    ) {
        let stale = self
            .selected_projection
            .as_ref()
            .is_some_and(|projection| projection.width != width || projection.height != height);
        if stale {
            self.selected_projection = None;
        }
        if self.snapshot.is_some() && self.selected_projection.is_none() {
            self.selected_projection = Some(SelectedHistoryProjection::new(
                history_len,
                display_start,
                width,
                height,
            ));
        }
    }

    pub(super) fn selected_projection(&self) -> Option<&SelectedHistoryProjection> {
        self.selected_projection.as_ref()
    }

    pub(super) fn selected_projection_mut(&mut self) -> Option<&mut SelectedHistoryProjection> {
        self.selected_projection.as_mut()
    }

    pub(crate) fn is_keyboard_mode(&self) -> bool {
        self.keyboard_mode
    }

    #[cfg(test)]
    pub(crate) fn area(&self) -> Rect {
        self.area
    }

    pub(crate) fn refresh(&mut self, area: Rect, buffer: &Buffer, soft_wrap_before: &[bool]) {
        self.refresh_view(area, buffer, soft_wrap_before, 0);
    }

    pub(crate) fn refresh_view(
        &mut self,
        area: Rect,
        buffer: &Buffer,
        soft_wrap_before: &[bool],
        scroll_rows: usize,
    ) {
        let geometry_changed = self.area != area;
        if self.snapshot.is_some() {
            // Scroll offsets count pre-wrapped visual Lines. The selected view
            // stays bottom-anchored when its height changes.
            self.viewport_top_key += self.viewport_scroll as i64 - scroll_rows as i64
                + self.area.height as i64
                - area.height as i64;
            // A pinned view reads its text from `captured`, so the frame is
            // only scanned for the rows that scrolled in since the last frame.
            self.area = area;
            self.rows = Vec::new();
            self.rows_scanned = false;
            self.set_soft_wrap_before(soft_wrap_before);
            self.viewport_scroll = scroll_rows;
            self.capture_missing_rows(area, buffer);
        } else {
            // No active selection: reuse the last scan when the frame is
            // unchanged instead of rebuilding a Vec<PaintedRow> with one
            // String per cell plus a deep equality compare every frame.
            // The buffer hash walks cells without allocating Strings, so an
            // unchanged frame does no per-cell allocation and never reaches
            // `buffer_row_cells` (#1582).
            let keyboard_selected =
                self.keyboard_mode && self.keyboard_anchor != self.keyboard_focus;
            if self.anchor.is_none() && self.focus.is_none() && !keyboard_selected && !self.dragging
            {
                // Fast path: geometry/scroll/wrap changed => content definitely
                // changed, rescan without paying for a hash first. Only hash
                // when a static frame is possible.
                let geometry_same = self.rows_scanned
                    && self.area == area
                    && self.soft_wrap_before == soft_wrap_before
                    && self.viewport_scroll == scroll_rows;
                if geometry_same {
                    let hash = buffer_content_hash(area, buffer);
                    if let Some((_, _, _, prev_hash)) = &self.unpinned_hash
                        && *prev_hash == hash
                    {
                        return;
                    }
                    let rows = (area.y..area.bottom())
                        .map(|y| {
                            PaintedRow::new(
                                buffer_row_cells(area, buffer, y),
                                soft_wrap_before
                                    .get(usize::from(y - area.y))
                                    .copied()
                                    .unwrap_or(false),
                            )
                        })
                        .collect::<Vec<_>>();
                    self.rows = rows;
                    self.rows_scanned = true;
                    self.unpinned_hash = Some((area, soft_wrap_before.to_vec(), scroll_rows, hash));
                    return;
                }
                let rows = (area.y..area.bottom())
                    .map(|y| {
                        PaintedRow::new(
                            buffer_row_cells(area, buffer, y),
                            soft_wrap_before
                                .get(usize::from(y - area.y))
                                .copied()
                                .unwrap_or(false),
                        )
                    })
                    .collect::<Vec<_>>();
                // No selection to clear on drift; just refresh the cache.
                // Geometry changed, so skip hashing this frame; the next
                // static frame will hash and populate the cache.
                self.area = area;
                self.rows = rows;
                self.rows_scanned = true;
                self.set_soft_wrap_before(soft_wrap_before);
                self.viewport_scroll = scroll_rows;
                self.unpinned_hash = None;
                return;
            }
            let rows = (area.y..area.bottom())
                .map(|y| {
                    PaintedRow::new(
                        buffer_row_cells(area, buffer, y),
                        soft_wrap_before
                            .get(usize::from(y - area.y))
                            .copied()
                            .unwrap_or(false),
                    )
                })
                .collect::<Vec<_>>();
            if self.rows_scanned
                && (self.area != area
                    || self.rows != rows
                    || self.soft_wrap_before != soft_wrap_before)
            {
                self.clear();
            }
            self.area = area;
            self.rows = rows;
            self.rows_scanned = true;
            self.set_soft_wrap_before(soft_wrap_before);
            self.viewport_scroll = scroll_rows;
        }
        if let Some((key, previous_top, count)) = self.pending_keyboard_move
            && self.viewport_top_key != previous_top
        {
            self.pending_keyboard_move = None;
            self.move_keyboard(key);
            if count > 1 {
                self.move_keyboard(key);
                if let Some((_, _, remaining)) = &mut self.pending_keyboard_move {
                    *remaining = remaining.saturating_add(count - 2);
                }
            }
        }
        if self.snapshot.is_some() {
            if self.dragging && !geometry_changed {
                self.extend_from_pointer();
            }
            self.prune_captured();
        }
    }

    fn set_soft_wrap_before(&mut self, soft_wrap_before: &[bool]) {
        if self.soft_wrap_before != soft_wrap_before {
            self.soft_wrap_before.clear();
            self.soft_wrap_before.extend_from_slice(soft_wrap_before);
        }
    }

    fn row_key(&self, local_row: usize) -> i64 {
        self.viewport_top_key + local_row as i64
    }

    /// Captures the painted rows that are not pinned yet.
    ///
    /// Pinned rows are already in `captured`, so a scrolled frame only pays for
    /// the rows that actually moved into view.
    fn capture_missing_rows(&mut self, area: Rect, buffer: &Buffer) {
        let count = usize::from(area.height).min(self.soft_wrap_before.len());
        for local_row in 0..count {
            let key = self.row_key(local_row);
            if self.captured.contains_key(&key) {
                continue;
            }
            let cells = buffer_row_cells(area, buffer, area.y + local_row as u16);
            let soft_wrap_before = self.soft_wrap_before[local_row];
            self.captured
                .insert(key, PaintedRow::new(cells, soft_wrap_before));
        }
    }

    fn capture_painted_rows(&mut self) {
        for local_row in 0..usize::from(self.area.height).min(self.soft_wrap_before.len()) {
            let key = self.row_key(local_row);
            if self.captured.contains_key(&key) {
                continue;
            }
            let Some(row) = self.rows.get(local_row) else {
                break;
            };
            self.captured.insert(key, row.clone());
        }
    }

    /// Drops captured rows that no live selection or viewport can reach.
    ///
    /// A gesture in progress can move its focus to any row in the viewport, so
    /// everything between the selection and the viewport stays reachable. Once
    /// the gesture is over only the selected rows, the viewport, and the rows a
    /// shift-click would extend from remain, which is what keeps a long scroll
    /// away from a released selection from pinning the whole history. Soft-wrapped
    /// runs are contiguous, so growing each range out to its run boundaries keeps
    /// the rows a word or line selection can still walk into.
    fn prune_captured(&mut self) {
        let top = self.viewport_top_key;
        let mut ranges = vec![(
            top,
            top + self.soft_wrap_before.len().saturating_sub(1) as i64,
        )];
        // A collapsed drag still has to stay reachable, so fall back to the
        // anchor and focus rows when there is no range to read yet.
        let selected = self.range().or_else(|| {
            self.anchor
                .zip(self.focus)
                .map(|(start, end)| (start.min(end), start.max(end)))
        });
        for (start, end) in [selected, self.origin_range].into_iter().flatten() {
            ranges.push((start.row.min(end.row), start.row.max(end.row)));
        }
        if self.dragging || self.keyboard_mode {
            let low = ranges.iter().map(|(low, _)| *low).min().unwrap_or(top);
            let high = ranges.iter().map(|(_, high)| *high).max().unwrap_or(top);
            ranges = vec![(low, high)];
        }
        for (low, high) in &mut ranges {
            while self.soft_wrap_before(*low) && self.captured.contains_key(&(*low - 1)) {
                *low -= 1;
            }
            while self.soft_wrap_before(*high + 1) && self.captured.contains_key(&(*high + 1)) {
                *high += 1;
            }
        }
        let span = ranges.iter().map(|(low, high)| high - low + 1).sum::<i64>();
        if self.captured.len() as i64 <= span {
            return;
        }
        self.captured.retain(|key, _| {
            ranges
                .iter()
                .any(|(low, high)| (*low..=*high).contains(key))
        });
    }

    /// Rebuilds painted rows a released selection pruned before a shift-click.
    ///
    /// Pruning keeps only the selected rows and the viewport once the gesture
    /// is over, so scrolling several viewports away drops everything between
    /// them. A shift-click then extends over rows that are no longer in
    /// `captured`, and `selected_text` would return `None`. The pinned
    /// snapshot is immutable, so the missing visual rows are re-rendered from
    /// it at the pinned width: the same `Paragraph` wrap the live frame uses,
    /// which preserves wide characters, soft wraps and the pinned layout
    /// across resizes. Only the extended range (plus one neighbour each side
    /// for the trailing-space decision in `copied_range`) is inserted, so an
    /// idle selection stays bounded and the capture grows only to the rows the
    /// new selection actually covers (#1562).
    fn regenerate_gap_from_snapshot(&mut self) {
        let Some(range) = self.range() else {
            return;
        };
        let (start, end) = (range.0.row.min(range.1.row), range.0.row.max(range.1.row));
        let mut gaps = Vec::new();
        let mut gap_start = None;
        for row in start..=end {
            if self.captured.contains_key(&row) {
                if let Some(first) = gap_start.take() {
                    gaps.push((first, row - 1));
                }
            } else {
                gap_start.get_or_insert(row);
            }
        }
        if let Some(first) = gap_start {
            gaps.push((first, end));
        }
        for (mut first, last) in gaps {
            while first <= last {
                let through = last.min(first.saturating_add(60_000));
                self.regenerate_snapshot_rows(first, through);
                first = through.saturating_add(1);
            }
        }
    }

    fn regenerate_snapshot_rows(&mut self, start: i64, end: i64) {
        let Some(snapshot) = self.snapshot.as_ref().map(Arc::clone) else {
            return;
        };
        if self.pinned_width == 0 || self.pinned_height == 0 {
            return;
        }
        // Render only the requested span, measured from the pinned tail.
        // A deep transcript must not be rendered in full after a wheel burst.
        let requested_start = start.saturating_sub(1);
        let requested_end = end.saturating_add(1);
        let pinned_tail = self.pinned_height as i64 + self.pinned_scroll as i64;
        let scroll = pinned_tail.saturating_sub(requested_end + 1).max(0) as usize;
        let height = requested_end
            .saturating_sub(requested_start)
            .saturating_add(1);
        let Ok(height) = u16::try_from(height) else {
            return;
        };
        let mut temp = super::TranscriptState::default();
        temp.scroll_up(scroll);
        // Reuse the immutable selected-history projection that already covers
        // the scrolled viewport, instead of revisiting all newer blocks.
        let projection_height = self
            .selected_projection
            .as_ref()
            .filter(|projection| projection.width == self.pinned_width)
            .map(|projection| projection.height);
        if projection_height.is_some() {
            temp.selection.snapshot = Some(Arc::clone(&snapshot));
            temp.selection.selected_projection = self.selected_projection.take();
            if let Some(projection) = temp.selection.selected_projection.as_mut() {
                projection.height = height;
            }
        }
        let lines = super::render_visible_conversation_with_transcript(
            &snapshot,
            self.pinned_width,
            height,
            &mut temp,
        );
        if let Some(original_height) = projection_height {
            self.selected_projection = temp.selection.selected_projection.take();
            if let Some(projection) = self.selected_projection.as_mut() {
                projection.height = original_height;
            }
        }
        if lines.is_empty() {
            return;
        }
        let soft_wrap_flags: Vec<bool> = lines
            .iter()
            .flat_map(|line| {
                let count = Paragraph::new(line.clone())
                    .wrap(Wrap { trim: false })
                    .line_count(self.pinned_width)
                    .max(1);
                std::iter::once(false).chain(std::iter::repeat_n(true, count.saturating_sub(1)))
            })
            .collect();
        let total_visual = soft_wrap_flags.len();
        if total_visual == 0 {
            return;
        }
        let Ok(total_height) = u16::try_from(total_visual) else {
            return;
        };
        let source_area = Rect::new(0, 0, self.pinned_width, total_height);
        let mut source = Buffer::empty(source_area);
        {
            use ratatui::widgets::Widget as _;
            let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
            paragraph.render(source_area, &mut source);
        }
        // Map the returned tail window back to the original pinned keys.
        let base = total_visual as i64 + temp.scroll_rows() as i64 - pinned_tail;
        // One neighbour each side keeps `copied_range`'s trailing-space
        // decision exact when the extended edge ends mid-logical-line.
        for row in start.saturating_sub(1)..=end.saturating_add(1) {
            if self.captured.contains_key(&row) {
                continue;
            }
            let index = base + row;
            if index < 0 || index >= total_visual as i64 {
                continue;
            }
            let visual = index as u16;
            let cells = buffer_row_cells(source_area, &source, visual);
            let soft_wrap_before = soft_wrap_flags
                .get(index as usize)
                .copied()
                .unwrap_or(false);
            self.captured
                .insert(row, PaintedRow::new(cells, soft_wrap_before));
        }
        // Drop the neighbours again if they fall outside the selected and
        // viewport ranges; they were only needed to decide trailing spaces.
        // `prune_captured` on the next frame does this, so no work here.
    }

    fn position(&self, column: u16, row: u16) -> Option<CellPosition> {
        if self.area.is_empty() {
            return None;
        }
        let local_row = usize::from(row.clamp(self.area.y, self.area.bottom() - 1) - self.area.y);
        let local_row = if self.snapshot.is_some() {
            local_row.min(self.soft_wrap_before.len().checked_sub(1)?)
        } else {
            local_row
        };
        Some(CellPosition {
            row: if self.snapshot.is_some() {
                self.row_key(local_row)
            } else {
                local_row as i64
            },
            column: {
                let local =
                    usize::from(column.clamp(self.area.x, self.area.right() - 1) - self.area.x);
                if self.snapshot.is_some() {
                    local.min(self.pinned_width.saturating_sub(1) as usize)
                } else {
                    local
                }
            },
        })
    }

    fn inside(&self, column: u16, row: u16) -> bool {
        column >= self.area.x
            && column < self.area.right()
            && row >= self.area.y
            && row < self.area.bottom()
    }

    fn range(&self) -> Option<(CellPosition, CellPosition)> {
        if let Some(range) = self.semantic_range {
            return Some(range);
        }
        let (Some(anchor), Some(focus)) = (self.anchor, self.focus) else {
            return None;
        };
        (anchor != focus).then_some((anchor.min(focus), anchor.max(focus)))
    }

    /// Columns a visual row contributes to the copied text.
    ///
    /// A visual row is padded to the terminal width, so the tail of a row that
    /// ends a logical line is gutter padding rather than content. Only that
    /// padding is dropped: leading whitespace stays, and a row the next visual
    /// row soft-wraps into keeps its trailing whitespace, because that is the
    /// space that rejoins the wrapped words. Copy and highlight share this so
    /// they never disagree about which whitespace is selected.
    fn copied_range(&self, row: i64, from: usize, through: usize) -> Option<(usize, usize)> {
        let painted = self.painted_row(row)?;
        let from = from.min(painted.cells.len());
        let through = through.min(painted.cells.len());
        let through = if self.soft_wrap_before(row + 1) {
            through
        } else {
            // A drag that starts inside the padding of a row selects nothing
            // from it, so the end never moves behind the start.
            through.min(painted.text_end).max(from)
        };
        Some((from, through))
    }

    pub(crate) fn selected_text(&self) -> Option<String> {
        if self.keyboard_mode {
            return self.keyboard_selected_text();
        }
        let (start, end) = self.range()?;
        let mut text = String::new();
        for row in start.row..=end.row {
            if row > start.row && !self.soft_wrap_before(row) {
                text.push('\n');
            }
            let from = if row == start.row { start.column } else { 0 };
            let cells = self.row_cells(row)?;
            let through = if row == end.row {
                end.column.saturating_add(1)
            } else {
                cells.len()
            };
            let (from, through) = self.copied_range(row, from, through)?;
            text.extend(cells.get(from..through)?.iter().map(String::as_str));
        }
        // A drag that overshoots into the blank rows below the transcript has
        // still selected prose, not the blank lines after it. Keyboard range
        // selection keeps a trailing newline: walking a caret across a row
        // boundary asks for that newline.
        while text.ends_with('\n') {
            text.pop();
        }
        (!text.trim().is_empty()).then_some(text)
    }

    fn keyboard_selected_text(&self) -> Option<String> {
        let (Some(anchor), Some(focus)) = (self.keyboard_anchor, self.keyboard_focus) else {
            return None;
        };
        if anchor == focus {
            return None;
        }
        let (start, end) = (anchor.min(focus), anchor.max(focus));
        let mut text = String::new();
        for row in start.row..=end.row {
            if row > start.row && !self.soft_wrap_before(row) {
                text.push('\n');
            }
            let from = if row == start.row { start.column } else { 0 };
            let cells = self.row_cells(row)?;
            let through = if row == end.row {
                end.column
            } else {
                cells.len()
            };
            let (from, through) = self.copied_range(row, from, through)?;
            text.extend(cells.get(from..through)?.iter().map(String::as_str));
        }
        (!text.is_empty()).then_some(text)
    }

    fn soft_wrap_before(&self, row: i64) -> bool {
        self.painted_row(row)
            .is_some_and(|painted| painted.soft_wrap_before)
    }

    fn painted_row(&self, row: i64) -> Option<&PaintedRow> {
        if self.snapshot.is_some() {
            self.captured.get(&row)
        } else {
            self.rows.get(row as usize)
        }
    }

    fn row_cells(&self, row: i64) -> Option<&[String]> {
        Some(&self.painted_row(row)?.cells)
    }

    fn unit_range(
        &self,
        position: CellPosition,
        unit: SelectionUnit,
    ) -> Option<(CellPosition, CellPosition)> {
        if unit == SelectionUnit::Character {
            return Some((position, position));
        }
        let mut first = position.row;
        while first > self.first_row()
            && self.soft_wrap_before(first)
            && self.row_cells(first - 1).is_some()
        {
            first -= 1;
        }
        let mut last = position.row;
        while self.soft_wrap_before(last + 1) && self.row_cells(last + 1).is_some() {
            last += 1;
        }
        if unit == SelectionUnit::Line {
            let end_column = self
                .row_cells(last)?
                .iter()
                .rposition(|cell| !cell.trim().is_empty())
                .unwrap_or(0);
            return Some((
                CellPosition {
                    row: first,
                    column: 0,
                },
                CellPosition {
                    row: last,
                    column: end_column,
                },
            ));
        }
        // Keep a byte-to-cell map while joining painted rows. Empty cells are
        // continuations of a wide grapheme, so they share the preceding cell.
        let mut text = String::new();
        let mut spans = Vec::new();
        for row in first..=last {
            let cells = self.row_cells(row)?;
            let limit = if row == last {
                self.text_end(row)
            } else {
                cells.len()
            };
            for (column, cell) in cells.iter().take(limit).enumerate() {
                if cell.is_empty() {
                    continue;
                }
                let start = text.len();
                text.push_str(cell);
                spans.push((start, text.len(), CellPosition { row, column }));
            }
        }
        let byte = spans
            .iter()
            .rfind(|(_, _, cell)| cell.row == position.row && cell.column <= position.column)
            .map(|(start, _, _)| *start)?;
        let (start, segment) = text
            .split_word_bound_indices()
            .find(|(start, segment)| byte >= *start && byte < *start + segment.len())?;
        let (start, end) = if segment.chars().any(|ch| ch.is_alphanumeric() || ch == '_') {
            (start, start + segment.len())
        } else {
            let (offset, grapheme) =
                segment.grapheme_indices(true).find(|(offset, grapheme)| {
                    byte >= start + offset && byte < start + offset + grapheme.len()
                })?;
            (start + offset, start + offset + grapheme.len())
        };
        let start_position = spans
            .iter()
            .find(|(begin, through, _)| start >= *begin && start < *through)
            .map(|(_, _, cell)| *cell)?;
        let end_position = spans
            .iter()
            .find(|(begin, through, _)| end > *begin && end <= *through)
            .map(|(_, _, cell)| *cell)?;
        Some((start_position, end_position))
    }

    fn first_row(&self) -> i64 {
        if self.snapshot.is_some() {
            *self.captured.keys().next().unwrap_or(&0)
        } else {
            0
        }
    }

    fn last_row(&self) -> i64 {
        if self.snapshot.is_some() {
            *self.captured.keys().next_back().unwrap_or(&0)
        } else {
            self.rows.len().saturating_sub(1) as i64
        }
    }

    /// Column just past the last cell that carries text, i.e. where the
    /// terminal-width padding of the row starts.
    fn text_end(&self, row: i64) -> usize {
        self.painted_row(row).map_or(0, |painted| painted.text_end)
    }

    pub(crate) fn begin_keyboard_with_snapshot(
        &mut self,
        snapshot: RenderSnapshot,
        scroll_rows: usize,
    ) {
        let old_start = self.range().map(|range| range.0);
        if self.snapshot.is_none() {
            self.snapshot = Some(Arc::new(snapshot));
            self.pinned_width = self.area.width;
            self.pinned_height = self.area.height;
            self.pinned_scroll = scroll_rows;
            self.viewport_scroll = scroll_rows;
            self.viewport_top_key = 0;
            self.capture_painted_rows();
        }
        self.keyboard_mode = true;
        let cursor = old_start.unwrap_or(CellPosition {
            row: self.viewport_top_key,
            column: 0,
        });
        self.keyboard_anchor = Some(cursor);
        self.keyboard_focus = Some(cursor);
        self.pending_keyboard_move = None;
        self.anchor = Some(cursor);
        self.focus = Some(cursor);
        self.semantic_range = None;
        self.origin_range = None;
    }

    fn queue_keyboard_move(&mut self, key: KeyCode, direction: isize, visible_first: i64) {
        if let Some((pending_key, pending_top, count)) = &mut self.pending_keyboard_move
            && *pending_key == key
            && *pending_top == visible_first
        {
            *count = count.saturating_add(1);
            return;
        }
        self.cancel_pending_scroll();
        self.pending_keyboard_move = Some((key, visible_first, 1));
        self.queue_scroll(direction, 1);
    }

    pub(crate) fn move_keyboard(&mut self, key: KeyCode) {
        if !self.keyboard_mode {
            return;
        }
        let Some(mut cursor) = self.keyboard_focus else {
            return;
        };
        let visible_first = self.viewport_top_key;
        let visible_last = visible_first + self.soft_wrap_before.len().saturating_sub(1) as i64;
        match key {
            KeyCode::Right => {
                let cells = self.row_cells(cursor.row);
                if let Some(cells) = cells {
                    let end = self.text_end(cursor.row);
                    if cursor.column < end {
                        cursor.column += 1;
                        while cursor.column < end && cells[cursor.column].is_empty() {
                            cursor.column += 1;
                        }
                    } else if cursor.row >= visible_last {
                        self.queue_keyboard_move(key, 1, visible_first);
                        return;
                    } else if cursor.row < self.last_row() {
                        cursor.row += 1;
                        cursor.column = 0;
                    }
                }
            }
            KeyCode::Left => {
                if cursor.column > 0 {
                    cursor.column -= 1;
                    if let Some(cells) = self.row_cells(cursor.row) {
                        while cursor.column > 0 && cells[cursor.column].is_empty() {
                            cursor.column -= 1;
                        }
                    }
                } else if cursor.row <= visible_first {
                    self.queue_keyboard_move(key, -1, visible_first);
                    return;
                } else if cursor.row > self.first_row() {
                    cursor.row -= 1;
                    cursor.column = self.text_end(cursor.row);
                }
            }
            KeyCode::Up if cursor.row <= visible_first => {
                self.queue_keyboard_move(key, -1, visible_first);
                return;
            }
            KeyCode::Up if cursor.row > self.first_row() => {
                cursor.row -= 1;
                cursor.column = cursor.column.min(self.text_end(cursor.row));
            }
            KeyCode::Down if cursor.row >= visible_last => {
                self.queue_keyboard_move(key, 1, visible_first);
                return;
            }
            KeyCode::Down if cursor.row < self.last_row() => {
                cursor.row += 1;
                cursor.column = cursor.column.min(self.text_end(cursor.row));
            }
            _ => return,
        }
        self.keyboard_focus = Some(cursor);
        let Some(anchor) = self.keyboard_anchor else {
            return;
        };
        let (start, end) = (anchor.min(cursor), anchor.max(cursor));
        if start == end {
            self.semantic_range = None;
            return;
        }
        let last = if end.column > 0 {
            CellPosition {
                row: end.row,
                column: end.column - 1,
            }
        } else if end.row > self.first_row() {
            let row = end.row - 1;
            CellPosition {
                row,
                column: self.text_end(row).saturating_sub(1),
            }
        } else {
            end
        };
        self.semantic_range = Some((start, last.max(start)));
    }

    fn extend_semantic(&mut self, position: CellPosition) {
        let Some(origin) = self.origin_range else {
            return;
        };
        let Some(target) = self.unit_range(position, self.unit) else {
            return;
        };
        self.semantic_range = Some(if target.1 < origin.0 {
            (target.0, origin.1)
        } else if target.0 > origin.1 {
            (origin.0, target.1)
        } else {
            origin
        });
    }

    /// Pinning history is cheap; only visible rows are copied into the selection cache.
    pub(crate) fn begin_with_snapshot(
        &mut self,
        event: MouseEvent,
        snapshot: RenderSnapshot,
        scroll_rows: usize,
    ) {
        self.mouse(event);
        if self.dragging {
            let local_row = usize::from(event.row.saturating_sub(self.area.y));
            if local_row >= self.soft_wrap_before.len() {
                self.clear();
                return;
            }
            self.snapshot = Some(Arc::new(snapshot));
            self.pinned_width = self.area.width;
            self.pinned_height = self.area.height;
            self.pinned_scroll = scroll_rows;
            self.viewport_scroll = scroll_rows;
            self.viewport_top_key = 0;
            self.origin_row = event.row;
            self.anchor = self.position(event.column, event.row);
            self.focus = self.anchor;
            self.capture_painted_rows();
        }
    }

    fn extend_from_pointer(&mut self) {
        if let Some((column, row)) = self.pointer {
            self.focus = self.position(column, row);
            if self.unit != SelectionUnit::Character || self.origin_range.is_some() {
                if let Some(position) = self.focus {
                    self.extend_semantic(position);
                }
            }
        }
    }

    pub(crate) fn pinned_snapshot(&self) -> Option<Arc<RenderSnapshot>> {
        self.snapshot.as_ref().map(Arc::clone)
    }

    pub(crate) fn pinned_width(&self) -> Option<u16> {
        self.snapshot.as_ref().map(|_| self.pinned_width)
    }

    pub(crate) fn pause_edge_scroll(&mut self) {
        self.pointer = None;
        self.last_edge_attempt = None;
    }

    pub(crate) fn queue_scroll(&mut self, direction: isize, count: usize) {
        self.pause_edge_scroll();
        self.pending_scroll = self
            .pending_scroll
            .saturating_add(direction.saturating_mul(count as isize))
            .clamp(-10_000, 10_000);
    }

    pub(crate) fn take_scroll_step(&mut self, scroll_rows: usize) -> Option<isize> {
        if self.pending_scroll != 0 {
            // Wheel events already carry the same row delta as unselected
            // scrolling. Consume their coalesced delta in the next frame;
            // throttling it to one row makes a flick take dozens of frames.
            return Some(std::mem::take(&mut self.pending_scroll));
        }
        let direction = self.edge_scroll_direction(scroll_rows)?;
        self.mark_edge_attempt(direction, scroll_rows);
        Some(direction)
    }

    pub(crate) fn cancel_pending_scroll(&mut self) {
        self.pending_scroll = 0;
        self.pending_keyboard_move = None;
    }

    pub(crate) fn edge_scroll_direction(&self, scroll_rows: usize) -> Option<isize> {
        if !self.dragging || !self.moved_vertically || self.snapshot.is_none() {
            return None;
        }
        let (_, row) = self.pointer?;
        let direction = if row <= self.area.y {
            -1
        } else if row >= self.area.bottom().saturating_sub(1) {
            1
        } else {
            return None;
        };
        (self.last_edge_attempt != Some((direction, scroll_rows))).then_some(direction)
    }

    pub(crate) fn mark_edge_attempt(&mut self, direction: isize, scroll_rows: usize) {
        self.last_edge_attempt = Some((direction, scroll_rows));
    }

    /// Returns text to copy only for the explicit right-click copy action.
    /// Left-button release keeps the highlight visible without touching the
    /// clipboard; copy requires Ctrl+C or right-click on the selection.
    /// (Cmd+C is the terminal's native selection on macOS and never reaches
    /// the app; see #1566.)
    pub(crate) fn mouse(&mut self, event: MouseEvent) -> Option<String> {
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let shift_extend =
                    event.modifiers.contains(KeyModifiers::SHIFT) && self.has_selection();
                let old_range = self.range();
                let old_unit = self.unit;
                let now = Instant::now();
                let count = match self.last_click {
                    Some((when, x, y, count))
                        if x == event.column
                            && y == event.row
                            && now.duration_since(when).as_millis() <= 400 =>
                    {
                        count % 3 + 1
                    }
                    _ => 1,
                };
                if !shift_extend {
                    self.last_click = Some((now, event.column, event.row, count));
                }
                if !shift_extend {
                    self.clear();
                }
                if self.inside(event.column, event.row) {
                    let position = self.position(event.column, event.row);
                    if shift_extend {
                        self.origin_range = old_range;
                        self.unit = old_unit;
                        if let Some(position) = position {
                            self.extend_semantic(position);
                        }
                        // The released selection pruned the rows between it
                        // and this viewport; rebuild them from the pinned
                        // snapshot so the extended range stays copyable.
                        self.regenerate_gap_from_snapshot();
                    } else {
                        self.anchor = position;
                        self.focus = position;
                        self.unit = match count {
                            2 => SelectionUnit::Word,
                            3 => SelectionUnit::Line,
                            _ => SelectionUnit::Character,
                        };
                        if self.unit != SelectionUnit::Character {
                            self.origin_range =
                                position.and_then(|position| self.unit_range(position, self.unit));
                            self.semantic_range = self.origin_range;
                        }
                    }
                    self.dragging = true;
                    self.origin_row = event.row;
                }
            }
            MouseEventKind::Drag(MouseButton::Left) if self.dragging => {
                self.pointer = Some((event.column, event.row));
                self.moved_vertically |= event.row != self.origin_row;
                self.last_edge_attempt = None;
                self.extend_from_pointer();
                // A coalesced wheel burst can skip entire viewports. Recover
                // missing rows only when the gesture extends across that gap,
                // so scrolling itself stays as cheap as unselected scrolling.
                self.regenerate_gap_from_snapshot();
            }
            MouseEventKind::Up(MouseButton::Left) if self.dragging => {
                if self.snapshot.is_some() {
                    self.pointer = Some((event.column, event.row));
                    self.extend_from_pointer();
                    // A shift-drag can jump viewports in one gesture; fill any
                    // rows it crossed so release leaves a copyable range.
                    self.regenerate_gap_from_snapshot();
                } else {
                    self.focus = self.position(event.column, event.row);
                    if let Some(position) = self.focus {
                        if self.origin_range.is_some() {
                            self.extend_semantic(position);
                        }
                    }
                }
                self.dragging = false;
                self.pointer = None;
                if !self.has_selection() {
                    self.clear();
                }
                // Keep the highlight but do not copy: copying requires an
                // explicit Ctrl+C or right-click. See #1492 and #1566.
                return None;
            }
            MouseEventKind::Down(MouseButton::Right) if self.inside(event.column, event.row) => {
                return self.selected_text();
            }
            _ => {}
        }
        None
    }

    pub(crate) fn highlight(&self, buffer: &mut Buffer) {
        let Some((start, end)) = self.range() else {
            return;
        };
        for local_row in 0..self.area.height as usize {
            let row = if self.snapshot.is_some() {
                self.row_key(local_row)
            } else {
                local_row as i64
            };
            if row < start.row || row > end.row {
                continue;
            }
            let from = if row == start.row { start.column } else { 0 };
            let through = if row == end.row {
                end.column.saturating_add(1)
            } else {
                usize::from(self.area.width)
            };
            // Mark exactly the cells the copy reads, so the highlight never
            // claims trailing padding that the clipboard will not carry.
            let through = self
                .copied_range(row, from, through)
                .map_or(through, |(_, through)| through)
                .min(usize::from(self.area.width));
            for column in from..through {
                let x = self.area.x.saturating_add(column as u16);
                let y = self.area.y.saturating_add(local_row as u16);
                if let Some(cell) = buffer.cell_mut((x, y)) {
                    cell.set_style(Style::default().add_modifier(Modifier::REVERSED));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::WHEEL_SCROLL_LINES;
    use crossterm::event::KeyModifiers;
    use rustcode::controller::{ChatMessage, RenderState};
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    struct CountingAllocator;

    static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
    static ALLOCATED_BYTES: AtomicUsize = AtomicUsize::new(0);

    // Keep the ignored end-to-end benchmark honest about renderer allocations.
    #[global_allocator]
    static TEST_ALLOCATOR: CountingAllocator = CountingAllocator;

    // SAFETY: every allocation is delegated unchanged to the system allocator.
    unsafe impl GlobalAlloc for CountingAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            ALLOCATED_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
            // SAFETY: the system allocator accepts the caller's layout.
            unsafe { System.alloc(layout) }
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            // SAFETY: this pointer and layout came from the delegated system allocator.
            unsafe { System.dealloc(ptr, layout) }
        }

        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            ALLOCATED_BYTES.fetch_add(new_size, Ordering::Relaxed);
            // SAFETY: this pointer and layout came from the delegated system allocator.
            unsafe { System.realloc(ptr, layout, new_size) }
        }
    }

    fn rendered_transcript(
        state: &RenderState,
        transcript: &mut super::super::history_cell::TranscriptState,
    ) -> Buffer {
        rendered_transcript_size(state, transcript, 32, 14)
    }

    fn rendered_transcript_size(
        state: &RenderState,
        transcript: &mut super::super::history_cell::TranscriptState,
        width: u16,
        height: u16,
    ) -> Buffer {
        use crate::inline_terminal::InlineTerminal;
        use ratatui::backend::TestBackend;

        let snapshot = super::super::render_snapshot::render_snapshot(state);
        let mut terminal = InlineTerminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| {
                super::super::render_with_transcript_snapshot(frame, &snapshot, transcript);
            })
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn long_conversation() -> RenderState {
        let mut state = RenderState::new();
        let text = (0..40)
            .map(|row| format!("history row {row:02}"))
            .collect::<Vec<_>>()
            .join("\n\n");
        state.history.push(ChatMessage::new("assistant", text));
        state
    }

    #[test]
    fn selected_history_projection_slices_rows_in_chronological_order() {
        let mut projection = SelectedHistoryProjection::new(4, 0, 42, 18);
        projection.append(
            Arc::new(vec![Line::from("newer row"), Line::from("newest row")]),
            2,
        );
        projection.append(
            Arc::new(vec![Line::from("oldest row"), Line::from("older row")]),
            0,
        );

        let text =
            |rows: Vec<Line<'static>>| rows.iter().map(ToString::to_string).collect::<Vec<_>>();
        assert_eq!(
            text(projection.rows_from_tail_range(0, 4)),
            ["oldest row", "older row", "newer row", "newest row"]
        );
        assert_eq!(text(projection.rows_from_tail_range(0, 1)), ["newest row"]);
        assert_eq!(
            text(projection.rows_from_tail_range(1, 3)),
            ["older row", "newer row"]
        );
        assert_eq!(projection.rows_from_tail_range(4, 8).len(), 0);
    }

    #[test]
    fn selected_projection_matches_unselected_view_with_tool_groups_and_welcome() {
        let _theme_guard = crate::ui::tests::THEME_TEST_LOCK
            .lock()
            .expect("theme test lock");
        let mut state = RenderState::new();
        for index in 0..24 {
            state.history.push(ChatMessage::new(
                if index % 2 == 0 { "user" } else { "assistant" },
                format!("history entry {index:02}"),
            ));
            if index == 11 {
                state
                    .history
                    .push(ChatMessage::new("tool", "first grouped tool result"));
                state
                    .history
                    .push(ChatMessage::new("tool", "second grouped tool result"));
            }
        }

        let symbols = |buffer: &Buffer, width: u16, height: u16| {
            (0..height)
                .map(|y| {
                    (0..width)
                        .map(|x| buffer[(x, y)].symbol())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
        };
        for requested_scroll in [0, 8, 24, 10_000] {
            let mut unselected = super::super::history_cell::TranscriptState::default();
            unselected.scroll_up(requested_scroll);
            let expected = rendered_transcript_size(&state, &mut unselected, 42, 18);
            let scroll = unselected.scroll_rows();

            let mut selected = super::super::history_cell::TranscriptState::default();
            selected.scroll_up(requested_scroll);
            let _ = rendered_transcript_size(&state, &mut selected, 42, 18);
            let area = selected.selection.area;
            selected.selection.begin_with_snapshot(
                mouse(
                    MouseEventKind::Down(MouseButton::Left),
                    area.x + 2,
                    area.y + 2,
                ),
                super::super::render_snapshot::render_snapshot(&state),
                selected.scroll_rows(),
            );
            let actual = rendered_transcript_size(&state, &mut selected, 42, 18);
            assert_eq!(
                symbols(&actual, 42, 18),
                symbols(&expected, 42, 18),
                "selection changed visible rows at scroll offset {scroll}"
            );
        }
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn refresh_view_without_selection_reuses_unchanged_frame() {
        // No anchor, no focus, no keyboard range: the first frame scans once
        // so a later pin has fresh rows, and an identical second frame reuses
        // the scan without reaching `buffer_row_cells` again (#1582).
        let area = Rect::new(0, 0, 32, 10);
        let mut buffer = Buffer::empty(area);
        buffer.set_string(0, 0, "hello world", Style::default());
        let mut selection = TranscriptSelection::default();
        let soft_wrap_before = vec![false; 10];
        selection.refresh_view(area, &buffer, &soft_wrap_before, 0);
        assert!(!selection.rows.is_empty());
        assert!(selection.rows_scanned);
        assert!(!selection.has_selection());
        let rows_len = selection.rows.len();
        // Identical frame: reuse, no rescan.
        selection.refresh_view(area, &buffer, &soft_wrap_before, 0);
        assert_eq!(selection.rows.len(), rows_len);
        assert!(selection.rows_scanned);
        // Changed content: rescan.
        buffer.set_string(0, 1, "second row", Style::default());
        selection.refresh_view(area, &buffer, &soft_wrap_before, 0);
        assert_eq!(selection.rows.len(), rows_len);
        assert!(selection.rows_scanned);
    }

    #[test]
    fn double_click_selects_unicode_word_and_single_grapheme() {
        let area = Rect::new(0, 0, 24, 1);
        let mut buffer = Buffer::empty(area);
        buffer.set_string(0, 0, "café 🙂 next", Style::default());
        let mut selection = TranscriptSelection::default();
        selection.refresh(area, &buffer, &[false]);
        for _ in 0..2 {
            selection.mouse(mouse(MouseEventKind::Down(MouseButton::Left), 2, 0));
            selection.mouse(mouse(MouseEventKind::Up(MouseButton::Left), 2, 0));
        }
        assert_eq!(selection.selected_text().as_deref(), Some("café"));
        for _ in 0..2 {
            selection.mouse(mouse(MouseEventKind::Down(MouseButton::Left), 5, 0));
            selection.mouse(mouse(MouseEventKind::Up(MouseButton::Left), 5, 0));
        }
        assert_eq!(selection.selected_text().as_deref(), Some("🙂"));
    }

    #[test]
    fn double_click_word_crosses_soft_wrap() {
        let area = Rect::new(0, 0, 6, 2);
        let mut buffer = Buffer::empty(area);
        buffer.set_string(0, 0, "caféin", Style::default());
        buffer.set_string(0, 1, "side", Style::default());
        let mut selection = TranscriptSelection::default();
        selection.refresh(area, &buffer, &[false, true]);
        for _ in 0..2 {
            selection.mouse(mouse(MouseEventKind::Down(MouseButton::Left), 1, 1));
            selection.mouse(mouse(MouseEventKind::Up(MouseButton::Left), 1, 1));
        }
        assert_eq!(selection.selected_text().as_deref(), Some("caféinside"));
    }

    #[test]
    fn triple_click_selects_logical_line_across_soft_wrap() {
        let area = Rect::new(0, 0, 6, 3);
        let mut buffer = Buffer::empty(area);
        buffer.set_string(0, 0, "hello ", Style::default());
        buffer.set_string(0, 1, "world", Style::default());
        buffer.set_string(0, 2, "other", Style::default());
        let mut selection = TranscriptSelection::default();
        selection.refresh(area, &buffer, &[false, true, false]);
        for _ in 0..3 {
            selection.mouse(mouse(MouseEventKind::Down(MouseButton::Left), 1, 1));
            selection.mouse(mouse(MouseEventKind::Up(MouseButton::Left), 1, 1));
        }
        assert_eq!(selection.selected_text().as_deref(), Some("hello world"));
    }

    #[test]
    fn shift_click_extends_word_selection_in_reverse() {
        let area = Rect::new(0, 0, 20, 1);
        let mut buffer = Buffer::empty(area);
        buffer.set_string(0, 0, "one two three", Style::default());
        let mut selection = TranscriptSelection::default();
        selection.refresh(area, &buffer, &[false]);
        for _ in 0..2 {
            selection.mouse(mouse(MouseEventKind::Down(MouseButton::Left), 5, 0));
            selection.mouse(mouse(MouseEventKind::Up(MouseButton::Left), 5, 0));
        }
        let mut shifted = mouse(MouseEventKind::Down(MouseButton::Left), 1, 0);
        shifted.modifiers = KeyModifiers::SHIFT;
        selection.mouse(shifted);
        shifted.kind = MouseEventKind::Up(MouseButton::Left);
        selection.mouse(shifted);
        assert_eq!(selection.selected_text().as_deref(), Some("one two"));
    }

    #[test]
    fn keyboard_range_selects_one_grapheme_and_reverses() {
        let area = Rect::new(0, 0, 8, 1);
        let mut buffer = Buffer::empty(area);
        buffer.set_string(0, 0, "🙂ab", Style::default());
        let mut selection = TranscriptSelection::default();
        selection.refresh(area, &buffer, &[false]);
        selection.begin_keyboard_with_snapshot(
            super::super::render_snapshot::render_snapshot(&RenderState::new()),
            0,
        );
        selection.keyboard_anchor = Some(CellPosition { row: 0, column: 0 });
        selection.keyboard_focus = selection.keyboard_anchor;
        selection.move_keyboard(KeyCode::Right);
        assert_eq!(selection.selected_text().as_deref(), Some("🙂"));
        selection.move_keyboard(KeyCode::Right);
        assert_eq!(selection.selected_text().as_deref(), Some("🙂a"));
        selection.move_keyboard(KeyCode::Left);
        assert_eq!(selection.selected_text().as_deref(), Some("🙂"));
        selection.move_keyboard(KeyCode::Left);
        assert!(!selection.has_selection());
    }

    #[test]
    fn keyboard_mode_starts_at_top_left_of_visible_transcript() {
        let area = Rect::new(3, 2, 8, 2);
        let mut buffer = Buffer::empty(area);
        buffer.set_string(3, 2, "first", Style::default());
        buffer.set_string(3, 3, "last", Style::default());
        let mut selection = TranscriptSelection::default();
        selection.refresh(area, &buffer, &[false, false]);
        selection.begin_keyboard_with_snapshot(
            super::super::render_snapshot::render_snapshot(&RenderState::new()),
            0,
        );
        assert_eq!(
            selection.keyboard_anchor,
            Some(CellPosition { row: 0, column: 0 })
        );
        selection.move_keyboard(KeyCode::Right);
        assert_eq!(selection.selected_text().as_deref(), Some("f"));
    }

    #[test]
    fn keyboard_range_copies_hard_newline_when_crossing_row_boundary() {
        let area = Rect::new(0, 0, 4, 2);
        let mut buffer = Buffer::empty(area);
        buffer.set_string(0, 0, "abc", Style::default());
        buffer.set_string(0, 1, "def", Style::default());
        let mut selection = TranscriptSelection::default();
        selection.refresh(area, &buffer, &[false, false]);
        selection.begin_keyboard_with_snapshot(
            super::super::render_snapshot::render_snapshot(&RenderState::new()),
            0,
        );
        selection.keyboard_anchor = Some(CellPosition { row: 0, column: 3 });
        selection.keyboard_focus = selection.keyboard_anchor;
        selection.move_keyboard(KeyCode::Right);
        assert_eq!(selection.selected_text().as_deref(), Some("\n"));
        selection.move_keyboard(KeyCode::Right);
        assert_eq!(selection.selected_text().as_deref(), Some("\nd"));
    }

    #[test]
    fn keyboard_range_scrolls_and_captures_offscreen_row() {
        let state = long_conversation();
        let mut transcript = super::super::history_cell::TranscriptState::default();
        let _ = rendered_transcript(&state, &mut transcript);
        let area = transcript.selection.area;
        transcript.selection.begin_keyboard_with_snapshot(
            super::super::render_snapshot::render_snapshot(&state),
            transcript.scroll_rows(),
        );
        transcript.selection.keyboard_anchor = Some(CellPosition { row: 0, column: 0 });
        transcript.selection.keyboard_focus = transcript.selection.keyboard_anchor;
        transcript.selection.move_keyboard(KeyCode::Up);
        assert!(transcript.step_selection_scroll());
        let _ = rendered_transcript(&state, &mut transcript);
        assert_eq!(transcript.selection.keyboard_focus.unwrap().row, -1);
        assert!(transcript.selection.selected_text().is_some());
        assert!(transcript.selection.captured.contains_key(&-1));
        assert!(area.height > 0);
    }

    #[test]
    fn rapid_keyboard_moves_capture_each_offscreen_row() {
        let state = long_conversation();
        let mut transcript = super::super::history_cell::TranscriptState::default();
        let _ = rendered_transcript(&state, &mut transcript);
        transcript.selection.begin_keyboard_with_snapshot(
            super::super::render_snapshot::render_snapshot(&state),
            transcript.scroll_rows(),
        );
        transcript.selection.keyboard_anchor = Some(CellPosition { row: 0, column: 0 });
        transcript.selection.keyboard_focus = transcript.selection.keyboard_anchor;
        transcript.selection.move_keyboard(KeyCode::Up);
        transcript.selection.move_keyboard(KeyCode::Up);
        assert!(transcript.step_selection_scroll());
        let _ = rendered_transcript(&state, &mut transcript);
        assert_eq!(transcript.selection.keyboard_focus.unwrap().row, -1);
        assert!(transcript.step_selection_scroll());
        let _ = rendered_transcript(&state, &mut transcript);
        assert_eq!(transcript.selection.keyboard_focus.unwrap().row, -2);
        assert!(transcript.selection.captured.contains_key(&-2));
    }

    #[test]
    fn pinned_shift_click_drag_keeps_original_word_as_anchor() {
        let area = Rect::new(0, 0, 24, 1);
        let mut buffer = Buffer::empty(area);
        buffer.set_string(0, 0, "one two three four", Style::default());
        let mut selection = TranscriptSelection::default();
        selection.refresh(area, &buffer, &[false]);
        for _ in 0..2 {
            selection.begin_with_snapshot(
                mouse(MouseEventKind::Down(MouseButton::Left), 5, 0),
                super::super::render_snapshot::render_snapshot(&RenderState::new()),
                0,
            );
            selection.mouse(mouse(MouseEventKind::Up(MouseButton::Left), 5, 0));
        }
        assert_eq!(selection.selected_text().as_deref(), Some("two"));
        let mut shifted = mouse(MouseEventKind::Down(MouseButton::Left), 1, 0);
        shifted.modifiers = KeyModifiers::SHIFT;
        selection.mouse(shifted);
        selection.mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 9, 0));
        selection.mouse(mouse(MouseEventKind::Up(MouseButton::Left), 9, 0));
        assert_eq!(selection.selected_text().as_deref(), Some("two three"));
    }

    #[test]
    fn drag_selects_painted_unicode_and_keeps_highlight() {
        let area = Rect::new(2, 3, 8, 2);
        let mut buffer = Buffer::empty(area);
        buffer.set_string(2, 3, "é🙂 hi", Style::default());
        buffer.set_string(2, 4, "next", Style::default());
        let mut selection = TranscriptSelection::default();
        selection.refresh(area, &buffer, &[false, false]);
        selection.mouse(mouse(MouseEventKind::Down(MouseButton::Left), 2, 3));
        selection.mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 4, 3));
        // Left release must keep the highlight without copying (#1492).
        assert_eq!(
            selection.mouse(mouse(MouseEventKind::Up(MouseButton::Left), 4, 3)),
            None
        );
        selection.highlight(&mut buffer);
        assert!(
            buffer
                .cell((2, 3))
                .unwrap()
                .modifier
                .contains(Modifier::REVERSED)
        );
        assert_eq!(selection.selected_text().as_deref(), Some("é🙂"));
        selection.mouse(mouse(MouseEventKind::Down(MouseButton::Left), 2, 3));
        selection.mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 7, 3));
        assert_eq!(selection.selected_text().as_deref(), Some("é🙂 hi"));
    }

    #[test]
    fn selection_survives_same_frame_and_clears_on_content_change() {
        let area = Rect::new(0, 0, 8, 1);
        let mut buffer = Buffer::empty(area);
        buffer.set_string(0, 0, "hello", Style::default());
        let mut selection = TranscriptSelection::default();
        selection.refresh(area, &buffer, &[false]);
        selection.mouse(mouse(MouseEventKind::Down(MouseButton::Left), 0, 0));
        selection.mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 3, 0));
        selection.mouse(mouse(MouseEventKind::Up(MouseButton::Left), 3, 0));
        selection.refresh(area, &buffer, &[false]);
        assert_eq!(selection.selected_text().as_deref(), Some("hell"));
        buffer.set_string(0, 0, "other", Style::default());
        selection.refresh(area, &buffer, &[false]);
        assert!(!selection.has_selection());
    }

    #[test]
    fn copy_joins_soft_wrapped_rows_but_keeps_hard_newlines() {
        let area = Rect::new(0, 0, 5, 3);
        let mut buffer = Buffer::empty(area);
        buffer.set_string(0, 0, "hello", Style::default());
        buffer.set_string(0, 1, "world", Style::default());
        buffer.set_string(0, 2, "again", Style::default());
        let mut selection = TranscriptSelection::default();
        selection.refresh(area, &buffer, &[false, true, false]);
        selection.mouse(mouse(MouseEventKind::Down(MouseButton::Left), 0, 0));
        selection.mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 4, 2));
        // Left release keeps the highlight; explicit copy reads selected_text().
        assert_eq!(
            selection.mouse(mouse(MouseEventKind::Up(MouseButton::Left), 4, 2)),
            None
        );
        assert_eq!(
            selection.selected_text().as_deref(),
            Some("helloworld\nagain")
        );
    }

    #[test]
    fn copy_keeps_the_space_that_joins_a_soft_wrapped_line() {
        // A soft-wrapped row is full width by construction, so the space at
        // its end is content, not terminal padding (#1538).
        let area = Rect::new(0, 0, 6, 2);
        let mut buffer = Buffer::empty(area);
        buffer.set_string(0, 0, "hello ", Style::default());
        buffer.set_string(0, 1, "world", Style::default());
        let mut selection = TranscriptSelection::default();
        selection.refresh(area, &buffer, &[false, true]);
        selection.mouse(mouse(MouseEventKind::Down(MouseButton::Left), 0, 0));
        selection.mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 5, 1));
        selection.mouse(mouse(MouseEventKind::Up(MouseButton::Left), 5, 1));
        assert_eq!(selection.selected_text().as_deref(), Some("hello world"));
    }

    #[test]
    fn keyboard_copy_keeps_the_space_that_joins_a_soft_wrapped_line() {
        let area = Rect::new(0, 0, 6, 2);
        let mut buffer = Buffer::empty(area);
        buffer.set_string(0, 0, "hello ", Style::default());
        buffer.set_string(0, 1, "world", Style::default());
        let mut selection = TranscriptSelection::default();
        selection.refresh(area, &buffer, &[false, true]);
        selection.begin_keyboard_with_snapshot(
            super::super::render_snapshot::render_snapshot(&RenderState::new()),
            0,
        );
        for _ in 0..11 {
            selection.move_keyboard(KeyCode::Right);
        }
        assert_eq!(
            selection.keyboard_focus,
            Some(CellPosition { row: 1, column: 5 })
        );
        assert_eq!(selection.selected_text().as_deref(), Some("hello world"));
    }

    #[test]
    fn copy_drops_terminal_padding_but_keeps_leading_indentation() {
        let area = Rect::new(0, 0, 10, 3);
        let mut buffer = Buffer::empty(area);
        buffer.set_string(0, 0, "  indented", Style::default());
        buffer.set_string(0, 1, "tail", Style::default());
        buffer.set_string(0, 2, "", Style::default());
        let mut selection = TranscriptSelection::default();
        selection.refresh(area, &buffer, &[false, false, false]);
        selection.mouse(mouse(MouseEventKind::Down(MouseButton::Left), 0, 0));
        selection.mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 9, 2));
        selection.mouse(mouse(MouseEventKind::Up(MouseButton::Left), 9, 2));
        // Leading indentation survives, terminal-width padding and the blank
        // row the drag overshot into do not.
        assert_eq!(
            selection.selected_text().as_deref(),
            Some("  indented\ntail")
        );
    }

    #[test]
    fn copy_round_trips_a_code_block_and_an_ascii_table() {
        let mut state = RenderState::new();
        state.history.push(ChatMessage::new(
            "assistant",
            "Prose line that wraps past the edge of the narrow viewport.\n\n\
             ```\nfn main() {\n    let x = 1;\n}\n```\n\n\
             ```\n+------+------+\n| name | value |\n+------+------+\n| a    | 1    |\n+------+------+\n```",
        ));
        let mut transcript = super::super::history_cell::TranscriptState::default();
        let _ = rendered_transcript_size(&state, &mut transcript, 40, 20);
        let area = transcript.selection.area;
        transcript.selection.begin_with_snapshot(
            mouse(MouseEventKind::Down(MouseButton::Left), area.x, area.y + 1),
            super::super::render_snapshot::render_snapshot(&state),
            transcript.scroll_rows(),
        );
        transcript.selection.mouse(mouse(
            MouseEventKind::Drag(MouseButton::Left),
            area.x + 20,
            area.bottom() - 1,
        ));
        let copied = transcript.selection.selected_text().unwrap();
        // The transcript indents every row by two columns; that indentation is
        // content and round-trips, as does the code block's own four spaces.
        assert_eq!(
            copied,
            "• Prose line that wraps past the edge\n  \
             of the narrow viewport.\n\n  \
             fn main() {\n      let x = 1;\n  }\n\n  \
             +------+------+\n  | name | value |\n  +------+------+\n  \
             | a    | 1    |\n  +------+------+"
        );
        for line in copied.lines() {
            assert_eq!(line, line.trim_end(), "trailing padding in {line:?}");
        }
    }

    #[test]
    fn highlight_stops_where_the_copy_stops() {
        let area = Rect::new(0, 0, 8, 3);
        let mut buffer = Buffer::empty(area);
        buffer.set_string(0, 0, "hello", Style::default());
        buffer.set_string(0, 1, "world", Style::default());
        buffer.set_string(0, 2, "again", Style::default());
        let mut selection = TranscriptSelection::default();
        selection.refresh(area, &buffer, &[false, false, false]);
        selection.mouse(mouse(MouseEventKind::Down(MouseButton::Left), 0, 0));
        selection.mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 2, 2));
        selection.mouse(mouse(MouseEventKind::Up(MouseButton::Left), 2, 2));
        assert_eq!(
            selection.selected_text().as_deref(),
            Some("hello\nworld\naga")
        );
        selection.highlight(&mut buffer);
        let highlighted = |y: u16| {
            (0..8u16)
                .filter(|x| {
                    buffer
                        .cell((*x, y))
                        .is_some_and(|cell| cell.modifier.contains(Modifier::REVERSED))
                })
                .count()
        };
        assert_eq!((highlighted(0), highlighted(1), highlighted(2)), (5, 5, 3));
    }

    #[test]
    fn highlight_keeps_covering_a_soft_wrapped_row() {
        let area = Rect::new(0, 0, 6, 2);
        let mut buffer = Buffer::empty(area);
        buffer.set_string(0, 0, "hello ", Style::default());
        buffer.set_string(0, 1, "world", Style::default());
        let mut selection = TranscriptSelection::default();
        selection.refresh(area, &buffer, &[false, true]);
        selection.mouse(mouse(MouseEventKind::Down(MouseButton::Left), 0, 0));
        selection.mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 4, 1));
        selection.mouse(mouse(MouseEventKind::Up(MouseButton::Left), 4, 1));
        assert_eq!(selection.selected_text().as_deref(), Some("hello world"));
        selection.highlight(&mut buffer);
        // The wrapped row stays fully marked, matching the space the copy keeps.
        assert!((0..6u16).all(|x| {
            buffer
                .cell((x, 0))
                .is_some_and(|cell| cell.modifier.contains(Modifier::REVERSED))
        }));
    }

    #[test]
    fn left_release_does_not_copy_while_right_click_does() {
        let area = Rect::new(0, 0, 8, 1);
        let mut buffer = Buffer::empty(area);
        buffer.set_string(0, 0, "hello", Style::default());
        let mut selection = TranscriptSelection::default();
        selection.refresh(area, &buffer, &[false]);
        selection.mouse(mouse(MouseEventKind::Down(MouseButton::Left), 0, 0));
        selection.mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 3, 0));
        assert_eq!(
            selection.mouse(mouse(MouseEventKind::Up(MouseButton::Left), 3, 0)),
            None
        );
        // Highlight remains after release for explicit Ctrl+C.
        assert_eq!(selection.selected_text().as_deref(), Some("hell"));
        // Right-click on the selection is the explicit mouse copy action.
        assert_eq!(
            selection.mouse(mouse(MouseEventKind::Down(MouseButton::Right), 1, 0)),
            Some("hell".to_owned())
        );
        assert_eq!(selection.selected_text().as_deref(), Some("hell"));
    }

    #[test]
    fn double_click_release_keeps_word_without_copying() {
        let area = Rect::new(0, 0, 20, 1);
        let mut buffer = Buffer::empty(area);
        buffer.set_string(0, 0, "one two three", Style::default());
        let mut selection = TranscriptSelection::default();
        selection.refresh(area, &buffer, &[false]);
        for _ in 0..2 {
            selection.mouse(mouse(MouseEventKind::Down(MouseButton::Left), 5, 0));
            assert_eq!(
                selection.mouse(mouse(MouseEventKind::Up(MouseButton::Left), 5, 0)),
                None
            );
        }
        assert_eq!(selection.selected_text().as_deref(), Some("two"));
    }

    #[test]
    fn scrolling_preserves_selected_text() {
        let area = Rect::new(0, 0, 8, 2);
        let mut buffer = Buffer::empty(area);
        buffer.set_string(0, 0, "first", Style::default());
        buffer.set_string(0, 1, "second", Style::default());
        let mut transcript = super::super::history_cell::TranscriptState::default();
        transcript.selection.refresh(area, &buffer, &[false, false]);
        transcript
            .selection
            .mouse(mouse(MouseEventKind::Down(MouseButton::Left), 0, 0));
        transcript
            .selection
            .mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 2, 1));
        transcript
            .selection
            .mouse(mouse(MouseEventKind::Up(MouseButton::Left), 2, 1));
        assert_eq!(
            transcript.selection.selected_text().as_deref(),
            Some("first\nsec")
        );

        transcript.scroll_up(1);
        assert_eq!(
            transcript.selection.selected_text().as_deref(),
            Some("first\nsec")
        );
        transcript.scroll_down(1);
        assert_eq!(
            transcript.selection.selected_text().as_deref(),
            Some("first\nsec")
        );
    }

    #[test]
    fn edge_drag_selects_offscreen_rows_and_wheel_pauses_auto_scroll() {
        let mut state = long_conversation();
        let mut transcript = super::super::history_cell::TranscriptState::default();
        let _ = rendered_transcript(&state, &mut transcript);
        let area = transcript.selection.area;
        assert!(area.height >= 3);
        let down = mouse(
            MouseEventKind::Down(MouseButton::Left),
            area.x + 2,
            area.bottom() - 1,
        );
        transcript.selection.begin_with_snapshot(
            down,
            super::super::render_snapshot::render_snapshot(&state),
            transcript.scroll_rows(),
        );
        assert!(transcript.selection.captured.len() <= area.height as usize);
        transcript.selection.mouse(mouse(
            MouseEventKind::Drag(MouseButton::Left),
            area.x + 2,
            area.y,
        ));
        let before = transcript.selection.selected_text().unwrap();
        assert_eq!(transcript.selection.edge_scroll_direction(0), Some(-1));

        transcript.selection.mark_edge_attempt(-1, 0);
        transcript.scroll_up(1);
        let _ = rendered_transcript(&state, &mut transcript);
        let after = transcript.selection.selected_text().unwrap();
        assert!(
            after.len() > before.len(),
            "before={before:?}, after={after:?}"
        );
        assert!(transcript.selection.captured.len() > area.height as usize);
        assert_eq!(transcript.selection.edge_scroll_direction(1), Some(-1));

        transcript.selection.pause_edge_scroll();
        transcript.scroll_up(1);
        let _ = rendered_transcript(&state, &mut transcript);
        assert_eq!(
            transcript.selection.selected_text().as_deref(),
            Some(after.as_str())
        );
        assert_eq!(transcript.selection.edge_scroll_direction(2), None);

        transcript.selection.mouse(mouse(
            MouseEventKind::Drag(MouseButton::Left),
            area.x + 2,
            area.y,
        ));
        assert!(transcript.selection.selected_text().unwrap().len() > after.len());
        // Left release keeps the highlight without returning copy text (#1492).
        assert_eq!(
            transcript.selection.mouse(mouse(
                MouseEventKind::Up(MouseButton::Left),
                area.x + 2,
                area.y,
            )),
            None
        );
        let kept = transcript.selection.selected_text();

        state
            .history
            .push(ChatMessage::new("assistant", "new streamed response"));
        let _ = rendered_transcript(&state, &mut transcript);
        assert_eq!(kept, transcript.selection.selected_text());
    }

    #[test]
    fn downward_edge_drag_extends_selection_after_scrolling() {
        let state = long_conversation();
        let mut transcript = super::super::history_cell::TranscriptState::default();
        let _ = rendered_transcript(&state, &mut transcript);
        transcript.scroll_up(4);
        let _ = rendered_transcript(&state, &mut transcript);
        let area = transcript.selection.area;
        transcript.selection.begin_with_snapshot(
            mouse(MouseEventKind::Down(MouseButton::Left), area.x + 2, area.y),
            super::super::render_snapshot::render_snapshot(&state),
            transcript.scroll_rows(),
        );
        transcript.selection.mouse(mouse(
            MouseEventKind::Drag(MouseButton::Left),
            area.x + 2,
            area.bottom() - 1,
        ));
        let before = transcript.selection.selected_text().unwrap();
        assert_eq!(transcript.selection.edge_scroll_direction(4), Some(1));
        transcript.scroll_down(1);
        let _ = rendered_transcript(&state, &mut transcript);
        let after = transcript.selection.selected_text().unwrap();
        assert!(
            after.len() > before.len(),
            "before={before:?}, after={after:?}"
        );
    }

    #[test]
    fn resize_clips_pinned_layout_without_changing_selected_copy() {
        let state = long_conversation();
        let mut transcript = super::super::history_cell::TranscriptState::default();
        let _ = rendered_transcript(&state, &mut transcript);
        let area = transcript.selection.area;
        transcript.selection.begin_with_snapshot(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                area.x + 2,
                area.bottom() - 2,
            ),
            super::super::render_snapshot::render_snapshot(&state),
            transcript.scroll_rows(),
        );
        transcript.selection.mouse(mouse(
            MouseEventKind::Drag(MouseButton::Left),
            area.x + 5,
            area.y,
        ));
        let selected = transcript.selection.selected_text().unwrap();
        let before = transcript.selection.anchor.unwrap();

        let narrower = rendered_transcript_size(&state, &mut transcript, 20, 14);

        assert_eq!(transcript.selection.pinned_width(), Some(32));
        assert_eq!(transcript.selection.anchor, Some(before));
        assert_eq!(
            transcript.selection.selected_text().as_deref(),
            Some(selected.as_str())
        );
        assert!(narrower.area.width == 20);
    }

    #[test]
    fn large_history_mouse_down_only_captures_visible_rows() {
        let mut state = RenderState::new();
        let text = (0..12_000)
            .map(|row| format!("history row {row:05}"))
            .collect::<Vec<_>>()
            .join("\n");
        state.history.push(ChatMessage::new("assistant", text));
        let mut transcript = super::super::history_cell::TranscriptState::default();
        let _ = rendered_transcript(&state, &mut transcript);
        let area = transcript.selection.area;

        transcript.selection.begin_with_snapshot(
            mouse(MouseEventKind::Down(MouseButton::Left), area.x + 2, area.y),
            super::super::render_snapshot::render_snapshot(&state),
            transcript.scroll_rows(),
        );

        assert!(transcript.selection.captured.len() <= area.height as usize);
        assert_eq!(transcript.selection.anchor, transcript.selection.focus);
    }

    #[test]
    fn scrolling_away_from_a_released_selection_keeps_the_capture_bounded() {
        let mut state = RenderState::new();
        let text = (0..2_000)
            .map(|row| format!("history row {row:04}"))
            .collect::<Vec<_>>()
            .join("\n");
        state.history.push(ChatMessage::new("assistant", text));
        let mut transcript = super::super::history_cell::TranscriptState::default();
        let _ = rendered_transcript(&state, &mut transcript);
        let area = transcript.selection.area;
        transcript.selection.begin_with_snapshot(
            mouse(MouseEventKind::Down(MouseButton::Left), area.x + 1, area.y),
            super::super::render_snapshot::render_snapshot(&state),
            transcript.scroll_rows(),
        );
        transcript.selection.mouse(mouse(
            MouseEventKind::Drag(MouseButton::Left),
            area.x + 4,
            area.y + 2,
        ));
        transcript.selection.mouse(mouse(
            MouseEventKind::Up(MouseButton::Left),
            area.x + 4,
            area.y + 2,
        ));
        let selected = transcript.selection.selected_text().unwrap();
        let selected_rows = transcript.selection.last_row() - transcript.selection.first_row() + 1;
        assert_eq!(transcript.selection.captured.len() as i64, selected_rows);

        // Scrolling far from the drag origin keeps the copy and stops growing
        // the capture cache instead of pinning every row ever painted.
        for _ in 0..200 {
            transcript.scroll_up(1);
            let _ = rendered_transcript(&state, &mut transcript);
        }
        assert_eq!(
            transcript.selection.selected_text().as_deref(),
            Some(selected.as_str())
        );
        assert!(
            transcript.selection.captured.len() <= area.height as usize + 3,
            "captured {}",
            transcript.selection.captured.len()
        );
    }

    #[test]
    fn shift_click_extends_released_selection_after_distant_scroll() {
        // Select, release, scroll several viewports away, then shift-click:
        // the pruned gap is rebuilt from the pinned snapshot so the whole
        // extended range stays copyable (#1562).
        let state = long_conversation();
        let mut transcript = super::super::history_cell::TranscriptState::default();
        let _ = rendered_transcript(&state, &mut transcript);
        let area = transcript.selection.area;
        transcript.selection.begin_with_snapshot(
            mouse(MouseEventKind::Down(MouseButton::Left), area.x + 1, area.y),
            super::super::render_snapshot::render_snapshot(&state),
            transcript.scroll_rows(),
        );
        transcript.selection.mouse(mouse(
            MouseEventKind::Drag(MouseButton::Left),
            area.x + 4,
            area.y + 2,
        ));
        transcript.selection.mouse(mouse(
            MouseEventKind::Up(MouseButton::Left),
            area.x + 4,
            area.y + 2,
        ));
        let original = transcript.selection.selected_text().unwrap();

        for _ in 0..30 {
            transcript.scroll_up(1);
            let _ = rendered_transcript(&state, &mut transcript);
        }
        assert_eq!(
            transcript.selection.selected_text().as_deref(),
            Some(original.as_str())
        );
        assert!(
            transcript.selection.captured.len() <= area.height as usize + 3,
            "captured {}",
            transcript.selection.captured.len()
        );

        let mut shifted = mouse(MouseEventKind::Down(MouseButton::Left), area.x + 1, area.y);
        shifted.modifiers = KeyModifiers::SHIFT;
        transcript.selection.mouse(shifted);
        transcript.selection.mouse(mouse(
            MouseEventKind::Up(MouseButton::Left),
            area.x + 1,
            area.y,
        ));
        let extended = transcript
            .selection
            .selected_text()
            .expect("shift-click over pruned rows must stay copyable");
        assert!(
            extended.len() > original.len(),
            "original={original:?}, extended={extended:?}"
        );
        assert!(
            extended.contains("history row"),
            "extended copy lost the transcript text: {extended:?}"
        );
        assert!(
            extended.contains(original.trim()),
            "extended copy lost the original selection {original:?}: {extended:?}"
        );
    }

    #[test]
    fn shift_click_extends_in_the_opposite_direction_after_distant_scroll() {
        // Mirror of the above: select in older history, scroll back toward the
        // bottom, then shift-click forward. Both extension directions must
        // rebuild the pruned gap (#1562).
        let state = long_conversation();
        let mut transcript = super::super::history_cell::TranscriptState::default();
        let _ = rendered_transcript(&state, &mut transcript);
        for _ in 0..30 {
            transcript.scroll_up(1);
        }
        let _ = rendered_transcript(&state, &mut transcript);
        let area = transcript.selection.area;
        transcript.selection.begin_with_snapshot(
            mouse(MouseEventKind::Down(MouseButton::Left), area.x + 1, area.y),
            super::super::render_snapshot::render_snapshot(&state),
            transcript.scroll_rows(),
        );
        transcript.selection.mouse(mouse(
            MouseEventKind::Drag(MouseButton::Left),
            area.x + 4,
            area.y,
        ));
        transcript.selection.mouse(mouse(
            MouseEventKind::Up(MouseButton::Left),
            area.x + 4,
            area.y,
        ));
        let original = transcript.selection.selected_text().unwrap();

        for _ in 0..30 {
            transcript.scroll_down(1);
            let _ = rendered_transcript(&state, &mut transcript);
        }
        assert_eq!(
            transcript.selection.selected_text().as_deref(),
            Some(original.as_str())
        );

        let mut shifted = mouse(
            MouseEventKind::Down(MouseButton::Left),
            area.x + 1,
            area.bottom() - 1,
        );
        shifted.modifiers = KeyModifiers::SHIFT;
        transcript.selection.mouse(shifted);
        transcript.selection.mouse(mouse(
            MouseEventKind::Up(MouseButton::Left),
            area.x + 1,
            area.bottom() - 1,
        ));
        let extended = transcript
            .selection
            .selected_text()
            .expect("forward shift-click over pruned rows must stay copyable");
        assert!(
            extended.len() > original.len(),
            "original={original:?}, extended={extended:?}"
        );
        assert!(extended.contains("history row"));
        assert!(extended.contains(original.trim()));
    }

    #[test]
    fn shift_click_after_scroll_preserves_soft_wrap_and_wide_graphemes() {
        // Soft-wrapped rows join without a newline and wide graphemes round-
        // trip even when the gap was pruned and rebuilt (#1562).
        let mut state = RenderState::new();
        let text = (0..40)
            .map(|row| {
                format!("row {row:02} café 🙂 hello world {row:02} abcdefghijklmnopqrstuvwxyz")
            })
            .collect::<Vec<_>>()
            .join("\n");
        state.history.push(ChatMessage::new("assistant", text));
        let mut transcript = super::super::history_cell::TranscriptState::default();
        let _ = rendered_transcript(&state, &mut transcript);
        let area = transcript.selection.area;
        transcript.selection.begin_with_snapshot(
            mouse(MouseEventKind::Down(MouseButton::Left), area.x + 1, area.y),
            super::super::render_snapshot::render_snapshot(&state),
            transcript.scroll_rows(),
        );
        transcript.selection.mouse(mouse(
            MouseEventKind::Drag(MouseButton::Left),
            area.x + 4,
            area.y + 2,
        ));
        transcript.selection.mouse(mouse(
            MouseEventKind::Up(MouseButton::Left),
            area.x + 4,
            area.y + 2,
        ));
        assert!(transcript.selection.selected_text().is_some());

        for _ in 0..30 {
            transcript.scroll_up(1);
            let _ = rendered_transcript(&state, &mut transcript);
        }

        let mut shifted = mouse(MouseEventKind::Down(MouseButton::Left), area.x + 1, area.y);
        shifted.modifiers = KeyModifiers::SHIFT;
        transcript.selection.mouse(shifted);
        transcript.selection.mouse(mouse(
            MouseEventKind::Up(MouseButton::Left),
            area.x + 1,
            area.y,
        ));
        let extended = transcript
            .selection
            .selected_text()
            .expect("extended copy with wraps and wide chars must exist");
        assert!(
            extended.contains("café"),
            "wide/unicode text lost: {extended:?}"
        );
        assert!(extended.contains("🙂"), "emoji lost: {extended:?}");
        // A soft-wrapped logical line rejoins with its space, not a newline:
        // every wrapped "hello world" survives as one phrase.
        assert!(
            extended.contains("hello world"),
            "soft-wrapped phrase split: {extended:?}"
        );
        for line in extended.lines() {
            assert_eq!(line, line.trim_end(), "trailing padding in {line:?}");
        }
    }

    #[test]
    fn shift_click_extension_after_scroll_uses_the_pinned_snapshot() {
        // New history arriving after the pin must not leak into the rebuilt
        // gap: the extension copies what was pinned, not what streamed later.
        let mut state = long_conversation();
        let mut transcript = super::super::history_cell::TranscriptState::default();
        let _ = rendered_transcript(&state, &mut transcript);
        let area = transcript.selection.area;
        transcript.selection.begin_with_snapshot(
            mouse(MouseEventKind::Down(MouseButton::Left), area.x + 1, area.y),
            super::super::render_snapshot::render_snapshot(&state),
            transcript.scroll_rows(),
        );
        transcript.selection.mouse(mouse(
            MouseEventKind::Drag(MouseButton::Left),
            area.x + 4,
            area.y + 2,
        ));
        transcript.selection.mouse(mouse(
            MouseEventKind::Up(MouseButton::Left),
            area.x + 4,
            area.y + 2,
        ));

        state
            .history
            .push(ChatMessage::new("assistant", "new streamed response"));
        for _ in 0..30 {
            transcript.scroll_up(1);
            let _ = rendered_transcript(&state, &mut transcript);
        }

        let mut shifted = mouse(MouseEventKind::Down(MouseButton::Left), area.x + 1, area.y);
        shifted.modifiers = KeyModifiers::SHIFT;
        transcript.selection.mouse(shifted);
        transcript.selection.mouse(mouse(
            MouseEventKind::Up(MouseButton::Left),
            area.x + 1,
            area.y,
        ));
        let extended = transcript
            .selection
            .selected_text()
            .expect("pinned extension must stay copyable");
        assert!(extended.contains("history row"));
        assert!(
            !extended.contains("new streamed response"),
            "pinned copy leaked live history: {extended:?}"
        );
    }

    #[test]
    fn wrapped_rows_keep_visual_anchors_across_reverse_scroll() {
        let mut state = RenderState::new();
        let text = (0..40)
            .map(|row| format!("row {row:02}: abcdefghijklmnopqrstuvwxyz 123456789"))
            .collect::<Vec<_>>()
            .join("\n");
        state.history.push(ChatMessage::new("assistant", text));
        let mut transcript = super::super::history_cell::TranscriptState::default();
        let _ = rendered_transcript(&state, &mut transcript);
        let area = transcript.selection.area;
        transcript.selection.begin_with_snapshot(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                area.x + 2,
                area.bottom() - 1,
            ),
            super::super::render_snapshot::render_snapshot(&state),
            transcript.scroll_rows(),
        );
        transcript.selection.mouse(mouse(
            MouseEventKind::Drag(MouseButton::Left),
            area.x + 2,
            area.y,
        ));
        let before = transcript.selection.selected_text().unwrap();
        let old_top = transcript.selection.viewport_top_key;

        transcript.scroll_up(1);
        let _ = rendered_transcript(&state, &mut transcript);

        assert_eq!(transcript.selection.viewport_top_key, old_top - 1);
        let after = transcript.selection.selected_text().unwrap();
        assert!(after.len() > before.len());
        transcript.selection.pause_edge_scroll();
        transcript.scroll_down(1);
        let _ = rendered_transcript(&state, &mut transcript);
        assert_eq!(transcript.selection.viewport_top_key, old_top);
        assert_eq!(transcript.selection.selected_text().unwrap(), after);
    }

    #[test]
    fn repeated_identical_rows_still_advance_anchor_when_scrolled() {
        let mut state = RenderState::new();
        state.history.push(ChatMessage::new(
            "assistant",
            std::iter::repeat_n("repeat", 40)
                .collect::<Vec<_>>()
                .join("\n"),
        ));
        let mut transcript = super::super::history_cell::TranscriptState::default();
        let _ = rendered_transcript(&state, &mut transcript);
        let area = transcript.selection.area;
        transcript.selection.begin_with_snapshot(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                area.x + 1,
                area.bottom() - 1,
            ),
            super::super::render_snapshot::render_snapshot(&state),
            transcript.scroll_rows(),
        );
        let old_top = transcript.selection.viewport_top_key;

        transcript.scroll_up(1);
        let _ = rendered_transcript(&state, &mut transcript);

        assert_eq!(transcript.selection.viewport_top_key, old_top - 1);
    }

    #[test]
    fn coalesced_wheel_steps_capture_each_row_before_later_drag() {
        let state = long_conversation();
        let mut transcript = super::super::history_cell::TranscriptState::default();
        let _ = rendered_transcript(&state, &mut transcript);
        let area = transcript.selection.area;
        transcript.selection.begin_with_snapshot(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                area.x + 2,
                area.bottom() - 1,
            ),
            super::super::render_snapshot::render_snapshot(&state),
            transcript.scroll_rows(),
        );
        transcript.selection.queue_scroll(-1, 3);
        assert!(transcript.step_selection_scroll());
        let _ = rendered_transcript(&state, &mut transcript);
        assert_eq!(transcript.scroll_rows(), 3);
        assert!(!transcript.step_selection_scroll());
        assert!(transcript.selection.captured.len() >= area.height as usize + 3);

        transcript.selection.mouse(mouse(
            MouseEventKind::Drag(MouseButton::Left),
            area.x + 2,
            area.y,
        ));
        let selected = transcript.selection.selected_text().unwrap();
        assert!(selected.contains("history row"));
        assert_eq!(transcript.selection.viewport_top_key, -3);
    }

    #[test]
    fn a_wheel_flick_coalesces_into_one_drained_run() {
        let state = long_conversation();
        let mut transcript = super::super::history_cell::TranscriptState::default();
        let _ = rendered_transcript(&state, &mut transcript);
        let area = transcript.selection.area;
        transcript.scroll_up(6);
        let _ = rendered_transcript(&state, &mut transcript);
        let start = transcript.scroll_rows();
        transcript.selection.begin_with_snapshot(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                area.x + 2,
                area.bottom() - 1,
            ),
            super::super::render_snapshot::render_snapshot(&state),
            transcript.scroll_rows(),
        );
        transcript.selection.mouse(mouse(
            MouseEventKind::Drag(MouseButton::Left),
            area.x + 2,
            area.y,
        ));

        // A flick: eight ticks arrive before the loop paints a frame.
        for _ in 0..8 {
            transcript.selection.queue_scroll(-1, WHEEL_SCROLL_LINES);
        }
        assert_eq!(
            transcript.selection.pending_scroll,
            -8 * isize::try_from(WHEEL_SCROLL_LINES).unwrap(),
            "ticks that land before the next frame coalesce instead of being dropped"
        );

        let mut frames = 0;
        while transcript.step_selection_scroll() {
            let _ = rendered_transcript(&state, &mut transcript);
            frames += 1;
            assert!(frames <= 8 * WHEEL_SCROLL_LINES, "the queue must drain");
        }
        assert_eq!(
            frames, 1,
            "a coalesced wheel flick moves its entire delta in the next frame"
        );
        assert_eq!(transcript.scroll_rows(), start + 8 * WHEEL_SCROLL_LINES);

        // A flick that changes direction nets out to what is left over, so
        // reversing a scroll does not run away past where it started.
        for _ in 0..4 {
            transcript.selection.queue_scroll(-1, WHEEL_SCROLL_LINES);
        }
        for _ in 0..6 {
            transcript.selection.queue_scroll(1, WHEEL_SCROLL_LINES);
        }
        assert_eq!(transcript.selection.pending_scroll, 6);
        let mut frames = 0;
        while transcript.step_selection_scroll() {
            let _ = rendered_transcript(&state, &mut transcript);
            frames += 1;
        }
        assert_eq!(frames, 1);
        assert_eq!(
            transcript.scroll_rows(),
            start + 8 * WHEEL_SCROLL_LINES - 6,
            "the reversed run should scroll back 6 rows, not run away"
        );
    }

    #[test]
    fn selected_and_plain_wheel_movement_match_at_each_frame() {
        let state = long_conversation();
        for released in [false, true] {
            let mut selected = super::super::history_cell::TranscriptState::default();
            let mut plain = super::super::history_cell::TranscriptState::default();
            selected.scroll_up(60);
            plain.scroll_up(60);
            let _ = rendered_transcript(&state, &mut selected);
            let area = selected.selection.area;
            selected.selection.begin_with_snapshot(
                mouse(
                    MouseEventKind::Down(MouseButton::Left),
                    area.x + 2,
                    area.y + 2,
                ),
                super::super::render_snapshot::render_snapshot(&state),
                selected.scroll_rows(),
            );
            selected.selection.mouse(mouse(
                MouseEventKind::Drag(MouseButton::Left),
                area.x + 2,
                area.y + 1,
            ));
            if released {
                selected.selection.mouse(mouse(
                    MouseEventKind::Up(MouseButton::Left),
                    area.x + 2,
                    area.y + 1,
                ));
            }
            // Identical event cadence: single ticks and bursts coalesced before a
            // frame, including reversal. Compare movement after each frame.
            for directions in [&[-1][..], &[-1, -1, -1, -1], &[1], &[1, 1, -1]] {
                for &direction in directions {
                    selected
                        .selection
                        .queue_scroll(direction, WHEEL_SCROLL_LINES);
                    if direction < 0 {
                        plain.scroll_up(WHEEL_SCROLL_LINES);
                    } else {
                        plain.scroll_down(WHEEL_SCROLL_LINES);
                    }
                }
                assert!(selected.step_selection_scroll());
                assert_eq!(selected.scroll_rows(), plain.scroll_rows());
                let _ = rendered_transcript(&state, &mut selected);
                assert!(!selected.step_selection_scroll(), "no slow frame backlog");
            }
        }
    }

    #[test]
    fn a_wheel_flick_captures_the_rows_it_crosses() {
        let state = long_conversation();
        let mut transcript = super::super::history_cell::TranscriptState::default();
        let painted = rendered_transcript(&state, &mut transcript);
        let area = transcript.selection.area;
        let before = (0..area.height)
            .map(|row| {
                (0..area.width)
                    .map(|column| painted[(area.x + column, area.y + row)].symbol())
                    .collect::<String>()
            })
            .filter(|row| !row.trim().is_empty())
            .map(|row| row.trim_end().to_owned())
            .collect::<Vec<_>>();
        assert!(!before.is_empty(), "the viewport should show text");

        transcript.selection.begin_with_snapshot(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                area.x + 2,
                area.bottom() - 1,
            ),
            super::super::render_snapshot::render_snapshot(&state),
            transcript.scroll_rows(),
        );
        // A hard flick: ten ticks, four times the height of the viewport, all
        // landing before the loop paints a frame.
        for _ in 0..10 {
            transcript.selection.queue_scroll(-1, WHEEL_SCROLL_LINES);
        }
        let mut frames = 0;
        while transcript.step_selection_scroll() {
            let _ = rendered_transcript(&state, &mut transcript);
            frames += 1;
        }
        assert_eq!(frames, 1);
        assert!(
            transcript.scroll_rows() > 3 * usize::from(area.height),
            "the flick should carry the viewport well past where it started"
        );

        // Extend across skipped viewports while still dragging: copying must
        // recover the gap before release, including the original viewport.
        transcript.selection.mouse(mouse(
            MouseEventKind::Drag(MouseButton::Left),
            area.x + 2,
            area.y,
        ));
        let selected = transcript.selection.selected_text().unwrap();
        assert!(
            selected.contains("history row 30"),
            "the skipped interior row must be recovered: {selected:?}"
        );
        for row in &before {
            assert!(
                selected.contains(row.as_str()),
                "the flick scrolled {row:?} off screen but the copy lost it: {selected:?}"
            );
        }
    }

    #[test]
    #[ignore = "manual transcript scroll benchmark"]
    fn bench_long_selection_scroll() {
        let mut state = RenderState::new();
        let text = (0..50_000)
            .map(|row| format!("history row {row:05} with a few words"))
            .collect::<Vec<_>>()
            .join("\n");
        state.history.push(ChatMessage::new("assistant", text));
        let mut transcript = super::super::history_cell::TranscriptState::default();
        let start = std::time::Instant::now();
        let painted = rendered_transcript_size(&state, &mut transcript, 100, 40);
        eprintln!("first paint: {:?}", start.elapsed());

        // Baseline: the same scroll with no selection pinned, so the pinned
        // numbers below can be read as the selection's share of a frame.
        transcript.scroll_up(10);
        let start = std::time::Instant::now();
        for _ in 0..10 {
            transcript.scroll_down(1);
            let _ = rendered_transcript_size(&state, &mut transcript, 100, 40);
        }
        eprintln!("10 unselected wheel frames: {:?}", start.elapsed());
        transcript.scroll_up(10);
        let _ = rendered_transcript_size(&state, &mut transcript, 100, 40);

        let area = transcript.selection.area;
        transcript.selection.begin_with_snapshot(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                area.x + 2,
                area.bottom() - 1,
            ),
            super::super::render_snapshot::render_snapshot(&state),
            transcript.scroll_rows(),
        );
        transcript.selection.mouse(mouse(
            MouseEventKind::Drag(MouseButton::Left),
            area.x + 2,
            area.y,
        ));
        let start = std::time::Instant::now();
        for _ in 0..10 {
            assert!(transcript.step_selection_scroll());
            let _ = rendered_transcript_size(&state, &mut transcript, 100, 40);
        }
        eprintln!("10 edge frames: {:?}", start.elapsed());
        transcript.selection.queue_scroll(1, 10);
        let start = std::time::Instant::now();
        for _ in 0..10 {
            assert!(transcript.step_selection_scroll());
            let _ = rendered_transcript_size(&state, &mut transcript, 100, 40);
        }
        eprintln!("10 wheel frames: {:?}", start.elapsed());

        transcript.selection.clear();
        transcript.scroll_up(10);
        let _ = rendered_transcript_size(&state, &mut transcript, 100, 40);
        transcript.selection.begin_with_snapshot(
            mouse(MouseEventKind::Down(MouseButton::Left), area.x + 2, area.y),
            super::super::render_snapshot::render_snapshot(&state),
            transcript.scroll_rows(),
        );
        transcript.selection.mouse(mouse(
            MouseEventKind::Drag(MouseButton::Left),
            area.x + 2,
            area.bottom() - 1,
        ));
        let start = std::time::Instant::now();
        for _ in 0..10 {
            assert!(transcript.step_selection_scroll());
            let _ = rendered_transcript_size(&state, &mut transcript, 100, 40);
        }
        eprintln!("10 downward edge frames: {:?}", start.elapsed());

        bench_selection_frame_cost(painted, area);
        bench_wheel_step_cost(&state);
    }

    #[test]
    #[ignore = "manual end-to-end deep selection scroll benchmark"]
    fn bench_deep_selection_scroll_many_history_entries() {
        let _theme_guard = crate::ui::tests::THEME_TEST_LOCK
            .lock()
            .expect("theme test lock");
        let mut state = RenderState::new();
        for index in 0..50_000 {
            state.history.push(ChatMessage::new(
                if index % 2 == 0 { "user" } else { "assistant" },
                format!("history entry {index:04}: read this line and keep scrolling"),
            ));
        }

        let mut transcript = super::super::history_cell::TranscriptState::default();
        let area = {
            let _ = rendered_transcript_size(&state, &mut transcript, 132, 48);
            transcript.selection.area
        };
        transcript.scroll_up(5_000);
        let _ = rendered_transcript_size(&state, &mut transcript, 132, 48);
        transcript.selection.begin_with_snapshot(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                area.x + 2,
                area.bottom() - 1,
            ),
            super::super::render_snapshot::render_snapshot(&state),
            transcript.scroll_rows(),
        );
        transcript.selection.mouse(mouse(
            MouseEventKind::Drag(MouseButton::Left),
            area.x + 2,
            area.y,
        ));

        // Include actual wheel deltas and pointer extension in every sample,
        // so gap recovery cost is represented in the interaction benchmark.
        let mut frames = Vec::with_capacity(101);
        let mut allocation_counts = Vec::with_capacity(101);
        let mut allocated_bytes = Vec::with_capacity(101);
        for _ in 0..101 {
            let before_allocations = ALLOCATIONS.load(Ordering::Relaxed);
            let before_bytes = ALLOCATED_BYTES.load(Ordering::Relaxed);
            let started = Instant::now();
            transcript.selection.queue_scroll(-1, 30);
            assert!(transcript.step_selection_scroll());
            let _ = rendered_transcript_size(&state, &mut transcript, 132, 48);
            transcript.selection.mouse(mouse(
                MouseEventKind::Drag(MouseButton::Left),
                area.x + 2,
                area.y,
            ));
            frames.push(started.elapsed());
            allocation_counts.push(ALLOCATIONS.load(Ordering::Relaxed) - before_allocations);
            allocated_bytes.push(ALLOCATED_BYTES.load(Ordering::Relaxed) - before_bytes);
        }

        let first_frame = frames[0];
        let mut warm_frames = frames[1..].to_vec();
        let mut warm_allocations = allocation_counts[1..].to_vec();
        let mut warm_bytes = allocated_bytes[1..].to_vec();
        warm_frames.sort_unstable();
        warm_allocations.sort_unstable();
        warm_bytes.sort_unstable();
        let percentile_index = |len: usize, p: usize| len.saturating_mul(p).div_ceil(100) - 1;
        let percentile = |sorted: &[Duration], p: usize| sorted[percentile_index(sorted.len(), p)];
        eprintln!(
            "deep many-entry selection wheel+drag frames (50,000 history entries, 5,000-row initial offset, 132x48, TestBackend + buffer clone): cold={first_frame:?}; 100 warm frames p50/p95/p99={:?}/{:?}/{:?}; warm allocations/frame p50/p95/p99={}/{}/{}, requested bytes/frame p50/p95/p99={}/{}/{}",
            percentile(&warm_frames, 50),
            percentile(&warm_frames, 95),
            percentile(&warm_frames, 99),
            warm_allocations[percentile_index(warm_allocations.len(), 50)],
            warm_allocations[percentile_index(warm_allocations.len(), 95)],
            warm_allocations[percentile_index(warm_allocations.len(), 99)],
            warm_bytes[percentile_index(warm_bytes.len(), 50)],
            warm_bytes[percentile_index(warm_bytes.len(), 95)],
            warm_bytes[percentile_index(warm_bytes.len(), 99)],
        );
    }

    /// Times the same number of scrolled *lines* at several wheel step sizes.
    ///
    /// A bigger step is only cheaper per line if the frame cost is flat in the row
    /// count, so this walks a fixed line budget at 1, 3 and 6 rows per tick and
    /// prints the per-frame and per-line cost of each. With a selection pinned the
    /// selected scrolling now applies the complete wheel delta per frame too.
    fn bench_wheel_step_cost(state: &RenderState) {
        const LINES: usize = 60;
        for step in [1usize, 3, 6] {
            let ticks = LINES / step;
            let mut transcript = super::super::history_cell::TranscriptState::default();
            let _ = rendered_transcript_size(state, &mut transcript, 100, 40);
            transcript.scroll_up(LINES * 2);
            let _ = rendered_transcript_size(state, &mut transcript, 100, 40);
            let start = std::time::Instant::now();
            for _ in 0..ticks {
                transcript.scroll_down(step);
                let _ = rendered_transcript_size(state, &mut transcript, 100, 40);
            }
            let elapsed = start.elapsed();
            eprintln!(
                "{LINES} lines at {step} row(s)/tick: {elapsed:?} ({} ticks, {:?}/frame, {:?}/line)",
                ticks,
                elapsed / ticks as u32,
                elapsed / LINES as u32,
            );

            let mut pinned = super::super::history_cell::TranscriptState::default();
            let _ = rendered_transcript_size(state, &mut pinned, 100, 40);
            let area = pinned.selection.area;
            pinned.selection.begin_with_snapshot(
                mouse(
                    MouseEventKind::Down(MouseButton::Left),
                    area.x + 2,
                    area.bottom() - 1,
                ),
                super::super::render_snapshot::render_snapshot(state),
                pinned.scroll_rows(),
            );
            pinned.selection.mouse(mouse(
                MouseEventKind::Drag(MouseButton::Left),
                area.x + 2,
                area.y,
            ));
            pinned.scroll_up(LINES);
            let _ = rendered_transcript_size(state, &mut pinned, 100, 40);
            let start = std::time::Instant::now();
            for _ in 0..ticks {
                pinned.selection.queue_scroll(1, step);
                for _ in 0..step {
                    assert!(pinned.step_selection_scroll());
                    let _ = rendered_transcript_size(state, &mut pinned, 100, 40);
                }
            }
            let elapsed = start.elapsed();
            eprintln!(
                "{LINES} lines at {step} row(s)/tick, selection pinned: {elapsed:?} ({} ticks, {:?}/line)",
                ticks,
                elapsed / LINES as u32,
            );
        }
    }

    /// Times the selection-only part of a frame: capturing the rows a scrolled
    /// frame adds and painting the highlight.
    fn bench_selection_frame_cost(painted: Buffer, area: Rect) {
        let mut selection = TranscriptSelection::default();
        let soft_wrap_before = vec![false; usize::from(area.height)];
        selection.refresh(area, &painted, &soft_wrap_before);
        selection.begin_with_snapshot(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                area.x + 2,
                area.bottom() - 1,
            ),
            super::super::render_snapshot::render_snapshot(&RenderState::new()),
            0,
        );
        selection.mouse(mouse(
            MouseEventKind::Drag(MouseButton::Left),
            area.x + 2,
            area.y,
        ));
        let mut highlighted = painted.clone();
        let start = std::time::Instant::now();
        for scroll in 0..200 {
            selection.refresh_view(area, &painted, &soft_wrap_before, scroll);
            selection.highlight(&mut highlighted);
        }
        eprintln!("200 pinned capture+highlight frames: {:?}", start.elapsed());

        let mut unpinned = TranscriptSelection::default();
        unpinned.refresh(area, &painted, &soft_wrap_before);
        let start = std::time::Instant::now();
        for _ in 0..200 {
            unpinned.refresh(area, &painted, &soft_wrap_before);
        }
        eprintln!("200 unpinned capture frames: {:?}", start.elapsed());
        eprintln!("captured rows: {}", selection.captured.len());
    }
}
