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
        if !self.viewport_area.is_empty() {
            self.backend
                .set_cursor_position(self.viewport_area.as_position())?;
            self.backend.clear_region(ClearType::AfterCursor)?;
        }
        self.buffers[1 - self.current].reset();
        Ok(())
    }

    pub fn autoresize(&mut self) -> Result<(), B::Error> {
        let size = self.backend.size()?;
        self.screen_size = size;
        self.viewport_area.width = size.width;
        self.viewport_area.y = self
            .viewport_area
            .y
            .min(size.height.saturating_sub(self.viewport_area.height));
        self.resize_buffers();
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

        if area != self.viewport_area {
            let clear_at = if self.viewport_area.is_empty() {
                area.as_position()
            } else {
                Position::new(0, self.viewport_area.y.min(area.y))
            };
            self.backend.set_cursor_position(clear_at)?;
            self.backend.clear_region(ClearType::AfterCursor)?;
            self.set_viewport_area(area);
            self.buffers[0].reset();
            self.buffers[1].reset();
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
        self.buffers[1 - self.current].reset();
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
        self.backend.draw(
            rows_to_draw
                .iter()
                .enumerate()
                .map(|(index, cell)| ((index % width) as u16, y + (index / width) as u16, cell)),
        )?;
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
}
