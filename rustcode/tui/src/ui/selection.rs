//! Mouse selection over the currently painted transcript rows.

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
};
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct CellPosition {
    row: usize,
    column: usize,
}

#[derive(Default)]
pub(crate) struct TranscriptSelection {
    area: Rect,
    rows: Vec<Vec<String>>,
    soft_wrap_before: Vec<bool>,
    anchor: Option<CellPosition>,
    focus: Option<CellPosition>,
    dragging: bool,
}

impl TranscriptSelection {
    pub(crate) fn clear(&mut self) {
        self.anchor = None;
        self.focus = None;
        self.dragging = false;
    }

    pub(crate) fn has_selection(&self) -> bool {
        self.range().is_some()
    }

    pub(crate) fn refresh(&mut self, area: Rect, buffer: &Buffer, soft_wrap_before: &[bool]) {
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
        if self.area != area || self.rows != rows || self.soft_wrap_before != soft_wrap_before {
            self.clear();
        }
        self.area = area;
        self.rows = rows;
        self.soft_wrap_before = soft_wrap_before.to_vec();
    }

    fn position(&self, column: u16, row: u16) -> Option<CellPosition> {
        if self.area.is_empty() {
            return None;
        }
        Some(CellPosition {
            row: usize::from(row.clamp(self.area.y, self.area.bottom() - 1) - self.area.y),
            column: usize::from(column.clamp(self.area.x, self.area.right() - 1) - self.area.x),
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
            let cells = self.rows.get(row)?;
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
            if index > 0
                && !self
                    .soft_wrap_before
                    .get(start.row + index)
                    .copied()
                    .unwrap_or(false)
            {
                text.push('\n');
            }
            text.push_str(line);
        }
        (!text.trim().is_empty()).then_some(text)
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
                }
            }
            MouseEventKind::Drag(MouseButton::Left) if self.dragging => {
                self.focus = self.position(event.column, event.row);
            }
            MouseEventKind::Up(MouseButton::Left) if self.dragging => {
                self.focus = self.position(event.column, event.row);
                self.dragging = false;
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
        for row in start.row..=end.row {
            let from = if row == start.row { start.column } else { 0 };
            let through = if row == end.row {
                end.column.saturating_add(1)
            } else {
                usize::from(self.area.width)
            };
            for column in from..through {
                let x = self.area.x.saturating_add(column as u16);
                let y = self.area.y.saturating_add(row as u16);
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
}
