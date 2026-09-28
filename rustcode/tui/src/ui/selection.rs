//! Mouse selection anchored to a pinned transcript viewport.

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
};
use std::{collections::BTreeMap, sync::Arc};
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

#[derive(Default)]
pub(crate) struct TranscriptSelection {
    area: Rect,
    rows: Vec<Vec<String>>,
    soft_wrap_before: Vec<bool>,
    anchor: Option<CellPosition>,
    focus: Option<CellPosition>,
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
    }

    pub(crate) fn is_dragging(&self) -> bool {
        self.dragging
    }

    pub(crate) fn is_active(&self) -> bool {
        self.snapshot.is_some()
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
        let previous_rows = &self.rows;
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
        if self.snapshot.is_some() && scroll_rows != self.viewport_scroll {
            let old_count = previous_rows.len().min(self.soft_wrap_before.len());
            let new_count = rows.len().min(soft_wrap_before.len());
            let old = &previous_rows[..old_count];
            let new = &rows[..new_count];
            let scroll_up = scroll_rows > self.viewport_scroll;
            let minimum_shift = scroll_rows.abs_diff(self.viewport_scroll);
            let max_overlap = old_count.min(new_count);
            let overlap = (1..=max_overlap).rev().find(|&count| {
                if scroll_up {
                    new_count - count >= minimum_shift && old[..count] == new[new_count - count..]
                } else {
                    old_count - count >= minimum_shift && old[old_count - count..] == new[..count]
                }
            });
            let advanced = if scroll_up {
                new_count - overlap.unwrap_or(0)
            } else {
                old_count - overlap.unwrap_or(0)
            } as i64;
            self.viewport_top_key += if scroll_up { -advanced } else { advanced };
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
        let (Some(anchor), Some(focus)) = (self.anchor, self.focus) else {
            return None;
        };
        (anchor != focus).then_some((anchor.min(focus), anchor.max(focus)))
    }

    pub(crate) fn selected_text(&self) -> Option<String> {
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
            lines.push(text.trim_end().to_owned());
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
                self.clear();
                if self.inside(event.column, event.row) {
                    self.anchor = self.position(event.column, event.row);
                    self.focus = self.anchor;
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
}
