//! Mouse selection anchored to a pinned transcript viewport.

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
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

struct CapturedRow {
    cells: Vec<String>,
    soft_wrap_before: bool,
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
    rows: Vec<Vec<String>>,
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
    pinned_width: u16,
    captured: BTreeMap<i64, CapturedRow>,
    viewport_scroll: usize,
    viewport_top_key: i64,
    pending_scroll: isize,
    pointer: Option<(u16, u16)>,
    origin_row: u16,
    moved_vertically: bool,
    last_edge_attempt: Option<(isize, usize)>,
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
        let rows = (area.y..area.bottom())
            .map(|y| {
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
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        if self.snapshot.is_some() {
            // Scroll offsets count pre-wrapped visual Lines. The selected view
            // stays bottom-anchored when its height changes.
            self.viewport_top_key += self.viewport_scroll as i64 - scroll_rows as i64
                + self.area.height as i64
                - area.height as i64;
        }
        if self.snapshot.is_none()
            && (self.area != area || self.rows != rows || self.soft_wrap_before != soft_wrap_before)
        {
            self.clear();
        }
        self.area = area;
        self.rows = rows;
        self.soft_wrap_before = soft_wrap_before.to_vec();
        self.viewport_scroll = scroll_rows;
        if self.snapshot.is_some() {
            self.capture_visible_rows();
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
        if self.snapshot.is_some() && self.dragging && !geometry_changed {
            self.extend_from_pointer();
        }
    }

    fn row_key(&self, local_row: usize) -> i64 {
        self.viewport_top_key + local_row as i64
    }

    fn capture_visible_rows(&mut self) {
        for (local_row, cells) in self
            .rows
            .iter()
            .take(self.soft_wrap_before.len())
            .enumerate()
        {
            let key = self.row_key(local_row);
            self.captured.entry(key).or_insert_with(|| CapturedRow {
                cells: cells.clone(),
                soft_wrap_before: self
                    .soft_wrap_before
                    .get(local_row)
                    .copied()
                    .unwrap_or(false),
            });
        }
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

    pub(crate) fn selected_text(&self) -> Option<String> {
        if self.keyboard_mode {
            return self.keyboard_selected_text();
        }
        let (start, end) = self.range()?;
        let mut lines = Vec::new();
        for row in start.row..=end.row {
            let cells = if self.snapshot.is_some() {
                &self.captured.get(&row)?.cells
            } else {
                self.rows.get(row as usize)?
            };
            let from = if row == start.row { start.column } else { 0 };
            let through = if row == end.row {
                end.column.saturating_add(1)
            } else {
                cells.len()
            };
            let text = cells
                .get(from..through.min(cells.len()))?
                .iter()
                .map(String::as_str)
                .collect::<String>();
            let next_is_soft_wrap = row < end.row && self.soft_wrap_before(row + 1);
            lines.push(if next_is_soft_wrap {
                text
            } else {
                text.trim_end().to_owned()
            });
        }
        let mut text = String::new();
        for (index, line) in lines.iter().enumerate() {
            if index > 0 && !self.soft_wrap_before(start.row + index as i64) {
                text.push('\n');
            }
            text.push_str(line);
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
            let cells = self.row_cells(row)?;
            let from = if row == start.row { start.column } else { 0 };
            let through = if row == end.row {
                end.column
            } else {
                self.text_end(row)
            };
            text.extend(
                cells
                    .get(from.min(cells.len())..through.min(cells.len()))?
                    .iter()
                    .map(String::as_str),
            );
        }
        (!text.is_empty()).then_some(text)
    }

    fn soft_wrap_before(&self, row: i64) -> bool {
        if self.snapshot.is_some() {
            self.captured
                .get(&row)
                .is_some_and(|row| row.soft_wrap_before)
        } else {
            self.soft_wrap_before
                .get(row as usize)
                .copied()
                .unwrap_or(false)
        }
    }

    fn row_cells(&self, row: i64) -> Option<&[String]> {
        if self.snapshot.is_some() {
            Some(&self.captured.get(&row)?.cells)
        } else {
            Some(self.rows.get(row as usize)?)
        }
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

    fn text_end(&self, row: i64) -> usize {
        self.row_cells(row)
            .and_then(|cells| {
                cells
                    .iter()
                    .rposition(|cell| !cell.trim().is_empty())
                    .map(|column| column + 1)
            })
            .unwrap_or(0)
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
            self.viewport_scroll = scroll_rows;
            self.viewport_top_key = 0;
            self.capture_visible_rows();
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
            self.viewport_scroll = scroll_rows;
            self.viewport_top_key = 0;
            self.origin_row = event.row;
            self.anchor = self.position(event.column, event.row);
            self.focus = self.anchor;
            self.capture_visible_rows();
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
            let direction = self.pending_scroll.signum();
            self.pending_scroll -= direction;
            return Some(direction);
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

    /// Returns text to copy when a drag is released or an existing selection
    /// is right-clicked. The highlight stays in place until another action.
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
            }
            MouseEventKind::Up(MouseButton::Left) if self.dragging => {
                if self.snapshot.is_some() {
                    self.pointer = Some((event.column, event.row));
                    self.extend_from_pointer();
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
                return self.selected_text();
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
    use crossterm::event::KeyModifiers;
    use rustcode::app::{AppState, ChatMessage};

    fn rendered_transcript(
        state: &AppState,
        transcript: &mut super::super::history_cell::TranscriptState,
    ) -> Buffer {
        rendered_transcript_size(state, transcript, 32, 14)
    }

    fn rendered_transcript_size(
        state: &AppState,
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

    fn long_conversation() -> AppState {
        let mut state = AppState::new();
        let text = (0..40)
            .map(|row| format!("history row {row:02}"))
            .collect::<Vec<_>>()
            .join("\n\n");
        state.history.push(ChatMessage::new("assistant", text));
        state
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
            super::super::render_snapshot::render_snapshot(&AppState::new()),
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
            super::super::render_snapshot::render_snapshot(&AppState::new()),
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
            super::super::render_snapshot::render_snapshot(&AppState::new()),
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
                super::super::render_snapshot::render_snapshot(&AppState::new()),
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
        assert_eq!(
            selection.mouse(mouse(MouseEventKind::Up(MouseButton::Left), 4, 3)),
            Some("é🙂".to_owned())
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
        assert_eq!(
            selection.mouse(mouse(MouseEventKind::Up(MouseButton::Left), 4, 2)),
            Some("helloworld\nagain".to_owned())
        );
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
        let copied = transcript.selection.mouse(mouse(
            MouseEventKind::Up(MouseButton::Left),
            area.x + 2,
            area.y,
        ));
        assert_eq!(copied, transcript.selection.selected_text());

        state
            .history
            .push(ChatMessage::new("assistant", "new streamed response"));
        let _ = rendered_transcript(&state, &mut transcript);
        assert_eq!(copied, transcript.selection.selected_text());
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
        let mut state = AppState::new();
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
    fn wrapped_rows_keep_visual_anchors_across_reverse_scroll() {
        let mut state = AppState::new();
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
        let mut state = AppState::new();
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
        for step in 1..=3 {
            assert!(transcript.step_selection_scroll());
            let _ = rendered_transcript(&state, &mut transcript);
            assert_eq!(transcript.scroll_rows(), step);
        }
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
    #[ignore = "manual transcript scroll benchmark"]
    fn bench_long_selection_scroll() {
        let mut state = AppState::new();
        let text = (0..50_000)
            .map(|row| format!("history row {row:05} with a few words"))
            .collect::<Vec<_>>()
            .join("\n");
        state.history.push(ChatMessage::new("assistant", text));
        let mut transcript = super::super::history_cell::TranscriptState::default();
        let start = std::time::Instant::now();
        let _ = rendered_transcript_size(&state, &mut transcript, 100, 40);
        eprintln!("first paint: {:?}", start.elapsed());
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
    }
}
