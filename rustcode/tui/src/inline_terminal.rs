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

    pub(crate) fn buffer(&self) -> &Buffer {
        self.buffer
    }

    pub(crate) fn buffer_mut(&mut self) -> &mut Buffer {
        self.buffer
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
    /// Lowest screen row that may still hold stale transient viewport rows.
    ///
    /// Unlike `clear_from_y` (pending resize state consumed by the next
    /// draw), this survives draws: viewport growth, shrinks, and resizes can
    /// leave previously painted rows outside the current viewport, and the
    /// exit erase must cover them without touching committed scrollback
    /// above. Reset whenever rows are known clean: history commits (rows
    /// above become committed scrollback), full-screen clears, and erases.
    transient_top: Option<u16>,
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
        Ok(Self::with_size_and_cursor(backend, screen_size, cursor))
    }

    /// The alternate screen begins at the origin; no cursor report is needed.
    pub fn new_at_origin(backend: B) -> Result<Self, B::Error> {
        let screen_size = backend.size()?;
        Ok(Self::with_size_and_cursor(
            backend,
            screen_size,
            Position::new(0, 0),
        ))
    }

    fn with_size_and_cursor(backend: B, screen_size: Size, cursor: Position) -> Self {
        Self {
            backend,
            buffers: [Buffer::empty(Rect::ZERO), Buffer::empty(Rect::ZERO)],
            current: 0,
            hidden_cursor: false,
            viewport_area: Rect::new(0, cursor.y, screen_size.width, 0),
            screen_size,
            last_cursor_position: cursor,
            needs_clear: false,
            clear_from_y: None,
            transient_top: None,
        }
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

    #[cfg(test)]
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
        self.transient_top = None;
        Ok(())
    }

    /// Erase the transient viewport projection, preserving committed scrollback.
    ///
    /// Shutdown calls this before printing the exit handoff: it clears from
    /// the lowest row that may hold stale transient content — the live
    /// viewport top, any pending resize row, the tracked historical minimum,
    /// or the last known composer row — down to the bottom of the screen.
    /// Committed native scrollback above that anchor is left intact, and the
    /// handoff prints where the projection was.
    ///
    /// The erase is exact and idempotent: it collapses the viewport and
    /// clears all tracking, so repeats (second restore, `Drop`, post-update
    /// output) clear nothing. When the model holds no projection at all, a
    /// stale `fallback_y` is ignored so output printed after a restore (for
    /// example the `--update` result) is never wiped.
    pub fn erase_transient_projection(&mut self, fallback_y: Option<u16>) -> Result<(), B::Error> {
        let screen_height = self.backend.size()?.height;
        if screen_height == 0 {
            return Ok(());
        }
        let mut top = self.viewport_area.y;
        let mut dirty = !self.viewport_area.is_empty();
        if let Some(y) = self.clear_from_y {
            top = top.min(y);
            dirty = true;
        }
        if let Some(y) = self.transient_top {
            top = top.min(y);
            dirty = true;
        }
        if !dirty {
            return Ok(());
        }
        if let Some(y) = fallback_y.filter(|&y| y < screen_height) {
            top = top.min(y);
        }
        if top >= screen_height {
            // The tracked rows scrolled entirely off-screen; nothing visible
            // left to erase.
            return Ok(());
        }
        self.backend.set_cursor_position(Position::new(0, top))?;
        self.backend.clear_region(ClearType::AfterCursor)?;
        self.buffers[0].reset();
        self.buffers[1].reset();
        self.needs_clear = false;
        self.clear_from_y = None;
        self.transient_top = None;
        self.viewport_area.height = 0;
        self.viewport_area.y = top;
        self.backend.flush()
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
        self.transient_top = None;
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
            self.transient_top = Some(self.transient_top.map_or(clear_y, |prev| prev.min(clear_y)));
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

        // Track the lowest row the transient projection may occupy so the
        // exit erase covers viewport growth and scrolls, not just the final
        // viewport top. Draws only ever move the viewport up (growth scroll)
        // within transient rows, so extending the minimum here can never
        // reach committed scrollback above.
        self.transient_top = Some(
            self.transient_top
                .map_or(area.y.min(self.viewport_area.y), |prev| {
                    prev.min(area.y).min(self.viewport_area.y)
                }),
        );

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
        // Committed lines now own every row above the viewport; previously
        // tracked minima would point into scrollback, so the exit erase must
        // anchor at the live viewport from here on. The same holds for a
        // resize pending-clear: clamp it below the committed rows.
        self.transient_top = None;
        self.clear_from_y = self.clear_from_y.map(|y| y.max(self.viewport_area.y));

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
            // `insert_before` writes over rows that may still contain the old
            // mutable composer. Clear the complete physical row first so a
            // shorter committed line cannot leave stale footer/composer text
            // or background cells at its right edge.
            self.backend
                .set_cursor_position(Position::new(0, y + row))?;
            self.backend.clear_region(ClearType::CurrentLine)?;
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
    fn clear_removes_mutable_viewport_but_preserves_transcript_rows() {
        let backend = TestBackend::new(40, 12);
        let mut terminal = InlineTerminal::new(backend).unwrap();
        terminal
            .insert_before(2, |buffer| {
                buffer.set_string(0, 0, "transcript", ratatui::style::Style::default());
            })
            .unwrap();
        terminal
            .draw_height(4, |frame| {
                frame.render_widget(ratatui::widgets::Paragraph::new("stale TUI"), frame.area());
            })
            .unwrap();

        terminal.clear().unwrap();

        assert_eq!(terminal.area(), Rect::new(0, 2, 40, 4));
        assert_eq!(
            terminal.backend().buffer().cell((0, 0)).unwrap().symbol(),
            "t"
        );
        assert!(
            terminal
                .backend()
                .buffer()
                .content
                .iter()
                .skip(2 * 40)
                .all(|cell| cell.symbol().trim().is_empty())
        );
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

    #[test]
    fn erase_covers_tracked_minimum_after_resize_without_redraw() {
        let backend = TestBackend::new(40, 20);
        let mut terminal = InlineTerminal::new(backend).unwrap();
        terminal
            .insert_before(2, |buffer| {
                buffer.set_string(0, 0, "committed", ratatui::style::Style::default());
            })
            .unwrap();
        terminal
            .draw_height(4, |frame| {
                frame.render_widget(ratatui::widgets::Paragraph::new("live"), frame.area());
            })
            .unwrap();
        // Grow scrollback, repaint, then resize without a redraw: the old
        // exit anchor (live viewport top) would miss rows 16..23.
        terminal.insert_before(14, |_| {}).unwrap();
        terminal
            .draw_height(4, |frame| {
                frame.render_widget(ratatui::widgets::Paragraph::new("live"), frame.area());
            })
            .unwrap();
        terminal.backend_mut().resize(40, 28);
        terminal.autoresize().unwrap();
        assert_eq!(terminal.transient_top, Some(16));

        terminal.erase_transient_projection(None).unwrap();

        let committed: String = (0..9)
            .map(|column| terminal.backend().buffer()[(column, 0)].symbol())
            .collect();
        assert_eq!(committed, "committed");
        let stale: String = (0..40)
            .map(|column| terminal.backend().buffer()[(column, 16)].symbol())
            .collect();
        assert!(
            stale.trim().is_empty(),
            "stale viewport row survived: {stale:?}"
        );
        assert!(
            terminal
                .backend()
                .buffer()
                .content
                .iter()
                .skip(16 * 40)
                .all(|cell| cell.symbol().trim().is_empty())
        );
        assert_eq!(terminal.area(), Rect::new(0, 16, 40, 0));
    }

    #[test]
    fn erase_after_late_commits_preserves_all_scrollback() {
        let backend = TestBackend::new(40, 12);
        let mut terminal = InlineTerminal::new(backend).unwrap();
        terminal
            .insert_before(2, |buffer| {
                buffer.set_string(0, 0, "first", ratatui::style::Style::default());
            })
            .unwrap();
        terminal
            .draw_height(4, |frame| {
                frame.render_widget(ratatui::widgets::Paragraph::new("stale"), frame.area());
            })
            .unwrap();
        // Late history commits move the viewport down and turn the rows
        // above into scrollback: the tracked minimum must reset instead of
        // pointing the exit erase at committed lines.
        terminal
            .insert_before(3, |buffer| {
                buffer.set_string(0, 0, "second", ratatui::style::Style::default());
            })
            .unwrap();
        assert_eq!(terminal.transient_top, None);
        terminal.draw_height(4, |_| {}).unwrap();

        terminal.erase_transient_projection(None).unwrap();

        let first: String = (0..5)
            .map(|column| terminal.backend().buffer()[(column, 0)].symbol())
            .collect();
        assert_eq!(first, "first");
        let second: String = (0..6)
            .map(|column| terminal.backend().buffer()[(column, 2)].symbol())
            .collect();
        assert_eq!(second, "second");
        assert!(
            terminal
                .backend()
                .buffer()
                .content
                .iter()
                .skip(5 * 40)
                .all(|cell| cell.symbol().trim().is_empty())
        );
        assert_eq!(terminal.area(), Rect::new(0, 5, 40, 0));
    }

    #[test]
    fn erase_with_live_projection_keeps_scrollback_above() {
        let backend = TestBackend::new(40, 12);
        let mut terminal = InlineTerminal::new(backend).unwrap();
        terminal
            .insert_before(2, |buffer| {
                buffer.set_string(0, 0, "kept", ratatui::style::Style::default());
            })
            .unwrap();
        terminal
            .draw_height(4, |frame| {
                frame.render_widget(ratatui::widgets::Paragraph::new("gone"), frame.area());
            })
            .unwrap();

        // A composer row inside the live viewport must not move the anchor
        // above the tracked projection.
        terminal.erase_transient_projection(Some(4)).unwrap();

        let kept: String = (0..4)
            .map(|column| terminal.backend().buffer()[(column, 0)].symbol())
            .collect();
        assert_eq!(kept, "kept");
        assert!(
            terminal
                .backend()
                .buffer()
                .content
                .iter()
                .skip(2 * 40)
                .all(|cell| cell.symbol().trim().is_empty())
        );
        assert_eq!(terminal.area(), Rect::new(0, 2, 40, 0));
    }

    #[test]
    fn erase_is_idempotent_and_ignores_stale_fallback_once_clean() {
        let backend = TestBackend::new(40, 12);
        let mut terminal = InlineTerminal::new(backend).unwrap();
        terminal
            .draw_height(4, |frame| {
                frame.render_widget(ratatui::widgets::Paragraph::new("live"), frame.area());
            })
            .unwrap();
        terminal.erase_transient_projection(Some(3)).unwrap();
        assert_eq!(terminal.area(), Rect::new(0, 0, 40, 0));

        // Simulate `--update` output printed after the restore, bypassing
        // the (now collapsed) viewport model.
        let mut cell = Cell::default();
        cell.set_symbol("!");
        terminal
            .backend_mut()
            .draw(std::iter::once((0u16, 0u16, &cell)))
            .unwrap();

        // A stale composer row from before the restore must not wipe it:
        // with no tracked projection the erase is a no-op.
        terminal.erase_transient_projection(Some(3)).unwrap();
        assert_eq!(terminal.backend().buffer()[(0, 0)].symbol(), "!");
        assert_eq!(terminal.area(), Rect::new(0, 0, 40, 0));
    }

    #[test]
    fn insert_before_clears_old_composer_cells_before_short_history_rows() {
        let backend = TestBackend::new(40, 10);
        let mut terminal = InlineTerminal::new(backend).unwrap();
        terminal
            .draw_height(4, |frame| {
                frame.render_widget(
                    ratatui::widgets::Paragraph::new("old footer 72% context left"),
                    ratatui::layout::Rect::new(0, 0, 40, 1),
                );
            })
            .unwrap();

        terminal
            .insert_before(1, |buffer| {
                buffer.set_string(0, 0, "new history", ratatui::style::Style::default());
            })
            .unwrap();
        terminal.draw_height(4, |_| {}).unwrap();

        let row: String = (0..40)
            .map(|column| terminal.backend().buffer()[(column, 0)].symbol())
            .collect();
        assert_eq!(row.trim_end(), "new history");
        assert!(!row.contains("old footer"));
        assert!(!row.contains("72% context left"));
    }
}
