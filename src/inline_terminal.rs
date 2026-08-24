//! A dynamically-sized inline terminal.
//!
//! Ratatui's stock inline viewport has an immutable height.  Chat UIs need the
//! opposite: finalized rows belong to terminal scrollback while the mutable
//! composer/streaming tail grows and shrinks each frame.  This small wrapper is
//! derived from Ratatui's terminal implementation and follows Codex's viewport
//! model.

use ratatui::backend::{Backend, ClearType};
use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::{Position, Rect, Size};
use ratatui::widgets::{StatefulWidget, Widget};

pub struct Frame<'a> {
    cursor_position: Option<Position>,
    viewport_area: Rect,
    buffer: &'a mut Buffer,
}

impl Frame<'_> {
    pub const fn area(&self) -> Rect {
        self.viewport_area
    }

    pub fn render_widget<W: Widget>(&mut self, widget: W, area: Rect) {
        widget.render(area, self.buffer);
    }

    #[allow(dead_code)]
    pub fn render_stateful_widget<W>(&mut self, widget: W, area: Rect, state: &mut W::State)
    where
        W: StatefulWidget,
    {
        widget.render(area, self.buffer, state);
    }

    pub fn set_cursor_position<P: Into<Position>>(&mut self, position: P) {
        self.cursor_position = Some(position.into());
    }
}

pub struct InlineTerminal<B: Backend> {
    backend: B,
    buffers: [Buffer; 2],
    current: usize,
    hidden_cursor: bool,
    viewport_area: Rect,
    screen_size: Size,
    last_cursor_position: Position,
    needs_clear: bool,
    clear_from_y: Option<u16>,
}

impl<B> InlineTerminal<B>
where
    B: Backend,
{
    pub fn new(mut backend: B) -> Result<Self, B::Error> {
        let screen_size = backend.size()?;
        // Some PTYs do not answer the cursor-position report. Codex treats
        // that as a recoverable startup condition and anchors at the origin.
        let cursor = backend
            .get_cursor_position()
            .unwrap_or_else(|_| Position::new(0, 0));
        Ok(Self {
            backend,
            buffers: [Buffer::empty(Rect::ZERO), Buffer::empty(Rect::ZERO)],
            current: 0,
            hidden_cursor: false,
            viewport_area: Rect::new(0, cursor.y, screen_size.width, 0),
            screen_size,
            last_cursor_position: cursor,
            needs_clear: false,
            clear_from_y: None,
        })
    }

    pub const fn area(&self) -> Rect {
        self.viewport_area
    }

    pub fn size(&self) -> Result<Size, B::Error> {
        self.backend.size()
    }

    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    pub const fn backend(&self) -> &B {
        &self.backend
    }

    pub fn show_cursor(&mut self) -> Result<(), B::Error> {
        self.backend.show_cursor()?;
        self.hidden_cursor = false;
        Ok(())
    }

    pub fn clear(&mut self) -> Result<(), B::Error> {
        let clear_y = self.clear_from_y.take().unwrap_or(self.viewport_area.y);
        if !self.viewport_area.is_empty() || clear_y < self.screen_size.height {
            self.backend
                .set_cursor_position(Position::new(0, clear_y))?;
            self.backend.clear_region(ClearType::AfterCursor)?;
        }
        self.buffers[0].reset();
        self.buffers[1].reset();
        self.needs_clear = false;
        Ok(())
    }

    /// Clear the entire terminal screen and reset the viewport to the origin.
    pub fn clear_screen(&mut self) -> Result<(), B::Error> {
        self.autoresize()?;
        self.backend.clear_region(ClearType::All)?;
        self.backend.set_cursor_position(Position::new(0, 0))?;
        self.viewport_area = Rect::new(0, 0, self.screen_size.width, 0);
        self.last_cursor_position = Position::new(0, 0);
        self.needs_clear = false;
        self.clear_from_y = None;
        self.buffers[0].reset();
        self.buffers[1].reset();
        self.backend.flush()
    }

    pub fn autoresize(&mut self) -> Result<(), B::Error> {
        let size = self.backend.size()?;
        if size != self.screen_size {
            let was_at_bottom = self.screen_size.height > 0
                && self.viewport_area.bottom() >= self.screen_size.height;
            let old_y = self.viewport_area.y;
            self.screen_size = size;
            self.viewport_area.width = size.width;
            if was_at_bottom {
                self.viewport_area.y = size.height.saturating_sub(self.viewport_area.height);
            } else {
                self.viewport_area.y = self
                    .viewport_area
                    .y
                    .min(size.height.saturating_sub(self.viewport_area.height));
            }
            let clear_y = self
                .clear_from_y
                .map_or(old_y.min(self.viewport_area.y), |prev| {
                    prev.min(old_y).min(self.viewport_area.y)
                });
            self.clear_from_y = Some(clear_y);
            self.needs_clear = true;
            self.resize_buffers();
            self.buffers[0].reset();
            self.buffers[1].reset();
        }
        Ok(())
    }

    fn resize_buffers(&mut self) {
        self.buffers[0].resize(self.viewport_area);
        self.buffers[1].resize(self.viewport_area);
    }

    fn set_viewport_area(&mut self, area: Rect) {
        self.viewport_area = area;
        self.resize_buffers();
    }

    /// Resize the mutable viewport to `height` and paint one frame.
    pub fn draw_height<F>(&mut self, height: u16, render: F) -> Result<(), B::Error>
    where
        F: FnOnce(&mut Frame<'_>),
    {
        self.autoresize()?;
        let mut area = self.viewport_area;
        area.width = self.screen_size.width;
        area.height = height.min(self.screen_size.height);

        if area.bottom() > self.screen_size.height {
            let amount = area.bottom() - self.screen_size.height;
            self.scroll_screen_up(amount)?;
            area.y = self.screen_size.height.saturating_sub(area.height);
        }

        if area != self.viewport_area || self.needs_clear {
            let clear_at = if self.viewport_area.is_empty() {
                area.as_position()
            } else {
                let clear_y = self
                    .clear_from_y
                    .take()
                    .unwrap_or(self.viewport_area.y)
                    .min(area.y);
                Position::new(0, clear_y)
            };
            self.backend.set_cursor_position(clear_at)?;
            self.backend.clear_region(ClearType::AfterCursor)?;
            self.set_viewport_area(area);
            self.buffers[0].reset();
            self.buffers[1].reset();
            self.needs_clear = false;
            self.clear_from_y = None;
        }

        let mut frame = Frame {
            cursor_position: None,
            viewport_area: self.viewport_area,
            buffer: &mut self.buffers[self.current],
        };
        render(&mut frame);
        let cursor_position = frame.cursor_position;

        let (previous, current) = if self.current == 0 {
            let (first, second) = self.buffers.split_at_mut(1);
            (&second[0], &first[0])
        } else {
            let (first, second) = self.buffers.split_at_mut(1);
            (&first[0], &second[0])
        };
        self.backend.draw(previous.diff_iter(current))?;

        match cursor_position {
            Some(position) => {
                self.backend.show_cursor()?;
                self.hidden_cursor = false;
                self.backend.set_cursor_position(position)?;
                self.last_cursor_position = position;
            }
            None => {
                self.backend.hide_cursor()?;
                self.hidden_cursor = true;
            }
        }
        self.buffers[1 - self.current].reset();
        self.current = 1 - self.current;
        self.backend.flush()
    }

    /// Render finalized lines immediately above the mutable viewport.
    pub fn insert_before<F>(&mut self, height: u16, draw: F) -> Result<(), B::Error>
    where
        F: FnOnce(&mut Buffer),
    {
        if height == 0 {
            return Ok(());
        }
        self.autoresize()?;
        let width = self.screen_size.width;
        let mut rendered = Buffer::empty(Rect::new(0, 0, width, height));
        draw(&mut rendered);
        let mut cells = rendered.content.as_slice();
        let remaining = height;

        let mut drawn_height = i32::from(self.viewport_area.top());
        let mut buffer_height = i32::from(remaining);
        let viewport_height = i32::from(self.viewport_area.height);
        let screen_height = i32::from(self.screen_size.height);
        while buffer_height + viewport_height > screen_height {
            let to_draw = buffer_height.min(screen_height);
            let scroll_up = 0.max(drawn_height + to_draw - screen_height);
            self.scroll_screen_up(scroll_up as u16)?;
            cells = self.draw_rows((drawn_height - scroll_up) as u16, to_draw as u16, cells)?;
            drawn_height += to_draw - scroll_up;
            buffer_height -= to_draw;
        }
        let scroll_up = 0.max(drawn_height + buffer_height + viewport_height - screen_height);
        self.scroll_screen_up(scroll_up as u16)?;
        self.draw_rows(
            (drawn_height - scroll_up) as u16,
            buffer_height as u16,
            cells,
        )?;
        drawn_height += buffer_height - scroll_up;
        self.set_viewport_area(Rect {
            y: drawn_height as u16,
            ..self.viewport_area
        });

        self.backend
            .set_cursor_position(self.last_cursor_position)?;
        self.buffers[0].reset();
        self.buffers[1].reset();
        self.needs_clear = true;
        Ok(())
    }

    fn draw_rows<'a>(
        &mut self,
        y: u16,
        rows: u16,
        cells: &'a [Cell],
    ) -> Result<&'a [Cell], B::Error> {
        let count = usize::from(self.screen_size.width) * usize::from(rows);
        let (rows_to_draw, rest) = cells.split_at(count);
        let width = usize::from(self.screen_size.width);
        let mut significant_cells = Vec::new();
        for row in 0..rows {
            let row_start = usize::from(row) * width;
            let row_cells = &rows_to_draw[row_start..row_start + width];
            let last_non_empty = row_cells.iter().rposition(|cell| {
                cell.symbol() != " "
                    || cell.bg != ratatui::style::Color::Reset
                    || !cell.modifier.is_empty()
            });
            if let Some(last_idx) = last_non_empty {
                let row_y = y + row;
                for (x, cell) in row_cells[..=last_idx].iter().enumerate() {
                    significant_cells.push((x as u16, row_y, cell));
                }
            }
        }
        self.backend.draw(significant_cells.into_iter())?;
        self.backend.flush()?;
        Ok(rest)
    }

    fn scroll_screen_up(&mut self, rows: u16) -> Result<(), B::Error> {
        if rows > 0 {
            let bottom = self.screen_size.height.saturating_sub(1);
            self.backend.set_cursor_position(Position::new(0, bottom))?;
            self.backend.append_lines(rows)?;
            self.viewport_area.y = self.viewport_area.y.saturating_sub(rows);
        }
        Ok(())
    }

    #[cfg(test)]
    pub fn draw<F>(&mut self, render: F) -> Result<(), B::Error>
    where
        F: FnOnce(&mut Frame<'_>),
    {
        let height = self.size()?.height;
        self.draw_height(height, render)
    }
}

impl<B> Drop for InlineTerminal<B>
where
    B: Backend,
{
    fn drop(&mut self) {
        if self.hidden_cursor {
            let _ = self.backend.show_cursor();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    #[test]
    fn viewport_starts_empty_and_uses_each_requested_height() {
        let backend = TestBackend::new(80, 30);
        let mut terminal = InlineTerminal::new(backend).unwrap();
        assert_eq!(terminal.area().height, 0);

        terminal.draw_height(6, |_| {}).unwrap();
        assert_eq!(terminal.area().height, 6);

        terminal.draw_height(14, |_| {}).unwrap();
        assert_eq!(terminal.area().height, 14);

        terminal.draw_height(4, |_| {}).unwrap();
        assert_eq!(terminal.area().height, 4);
    }

    #[test]
    fn requested_height_is_clamped_to_the_screen() {
        let backend = TestBackend::new(80, 12);
        let mut terminal = InlineTerminal::new(backend).unwrap();
        terminal.draw_height(u16::MAX, |_| {}).unwrap();
        assert_eq!(terminal.area().height, 12);
    }

    #[test]
    fn history_insertion_moves_buffers_with_the_viewport() {
        let backend = TestBackend::new(40, 12);
        let mut terminal = InlineTerminal::new(backend).unwrap();
        terminal.draw_height(4, |_| {}).unwrap();
        terminal.insert_before(3, |_| {}).unwrap();

        assert_eq!(terminal.buffers[0].area, terminal.area());
        assert_eq!(terminal.buffers[1].area, terminal.area());
        terminal.draw_height(4, |_| {}).unwrap();
    }

    #[test]
    fn clear_screen_resets_viewport_and_buffers() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = InlineTerminal::new(backend).unwrap();
        terminal.draw_height(6, |_| {}).unwrap();
        assert_eq!(terminal.area().height, 6);

        terminal.clear_screen().unwrap();
        assert_eq!(terminal.area(), Rect::new(0, 0, 80, 0));
    }

    #[test]
    fn resize_replay_clears_old_width_rows_and_resets_viewport() {
        let backend = TestBackend::new(40, 12);
        let mut terminal = InlineTerminal::new(backend).unwrap();
        terminal
            .insert_before(2, |buffer| {
                buffer.set_string(
                    0,
                    0,
                    "old width transcript",
                    ratatui::style::Style::default(),
                );
            })
            .unwrap();
        terminal.draw_height(4, |_| {}).unwrap();

        terminal.backend_mut().resize(80, 20);
        terminal.clear_screen().unwrap();

        assert_eq!(terminal.area(), Rect::new(0, 0, 80, 0));
        assert!(
            terminal
                .backend()
                .buffer()
                .content
                .iter()
                .all(|cell| cell.symbol().trim().is_empty())
        );
    }

    #[test]
    fn autoresize_updates_screen_size_and_viewport_width() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = InlineTerminal::new(backend).unwrap();
        terminal.draw_height(6, |_| {}).unwrap();

        terminal.backend_mut().resize(100, 30);
        terminal.autoresize().unwrap();
        assert_eq!(terminal.size().unwrap(), Size::new(100, 30));
        assert_eq!(terminal.area().width, 100);
    }

    #[test]
    fn autoresize_maintains_bottom_anchoring_when_terminal_grows_and_shrinks() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = InlineTerminal::new(backend).unwrap();
        // Insert history so viewport moves down and scrolls to bottom
        terminal.insert_before(20, |_| {}).unwrap();
        terminal.draw_height(10, |_| {}).unwrap();
        assert_eq!(terminal.area(), Rect::new(0, 14, 80, 10));

        // Terminal height grows from 24 to 34 -> bottom anchor moves y to 34 - 10 = 24
        terminal.backend_mut().resize(80, 34);
        terminal.autoresize().unwrap();
        assert_eq!(terminal.area(), Rect::new(0, 24, 80, 10));

        // Terminal height shrinks back from 34 to 20 -> bottom anchor moves y to 20 - 10 = 10
        terminal.backend_mut().resize(80, 20);
        terminal.autoresize().unwrap();
        assert_eq!(terminal.area(), Rect::new(0, 10, 80, 10));
    }

    #[test]
    fn autoresize_clears_and_redraws_even_when_requested_height_is_unchanged() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = InlineTerminal::new(backend).unwrap();
        terminal
            .draw_height(6, |f| {
                f.render_widget(ratatui::widgets::Paragraph::new("Hello"), f.area());
            })
            .unwrap();

        // Resize width
        terminal.backend_mut().resize(100, 24);
        // Pre-run autoresize (as happens in event loop before draw)
        terminal.autoresize().unwrap();
        assert!(terminal.needs_clear);

        // draw_height with same height 6 should clear and redraw without diff artifacts
        terminal
            .draw_height(6, |f| {
                f.render_widget(ratatui::widgets::Paragraph::new("World"), f.area());
            })
            .unwrap();
        assert!(!terminal.needs_clear);
    }

    #[test]
    fn autoresize_tracks_lowest_clear_y_across_rapid_resizes() {
        let backend = TestBackend::new(80, 40);
        let mut terminal = InlineTerminal::new(backend).unwrap();
        terminal.insert_before(30, |_| {}).unwrap();
        terminal.draw_height(10, |_| {}).unwrap();
        // y was at 30
        assert_eq!(terminal.area().y, 30);

        // Rapid resize 1: grow height to 60 (anchored y becomes 50)
        terminal.backend_mut().resize(80, 60);
        terminal.autoresize().unwrap();
        assert_eq!(terminal.clear_from_y, Some(30));

        // Rapid resize 2: super wide (300 cols, height 50) -> y becomes 40
        terminal.backend_mut().resize(300, 50);
        terminal.autoresize().unwrap();
        // clear_from_y must keep min(30, 50, 40) = 30
        assert_eq!(terminal.clear_from_y, Some(30));

        // Rapid resize 3: half screen (120 cols, height 70) -> y becomes 60
        terminal.backend_mut().resize(120, 70);
        terminal.autoresize().unwrap();
        assert_eq!(terminal.clear_from_y, Some(30));

        // After draw_height, clear_from_y is consumed and cleared
        terminal.draw_height(10, |_| {}).unwrap();
        assert_eq!(terminal.clear_from_y, None);
        assert!(!terminal.needs_clear);
    }

    #[test]
    fn insert_before_omits_trailing_empty_spaces() {
        let backend = TestBackend::new(200, 30);
        let mut terminal = InlineTerminal::new(backend).unwrap();
        terminal
            .insert_before(1, |buf| {
                buf.set_string(0, 0, "Hello", ratatui::style::Style::default());
            })
            .unwrap();
        // Check that the backend only received the 5 characters, not 200 spaces
        let rendered_line = terminal.backend().buffer();
        // Row 0 should start with "Hello" and the rest should be empty/unwritten in TestBackend
        assert_eq!(
            &rendered_line.content[0..5]
                .iter()
                .map(|c| c.symbol())
                .collect::<String>(),
            "Hello"
        );
    }
}
