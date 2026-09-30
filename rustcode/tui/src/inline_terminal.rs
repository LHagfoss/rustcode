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
    /// First screen row this session's projection occupied, recorded before
    /// any scrolling so later viewport growth, history commits, and
    /// `scroll_screen_up` can only move the projection *up*, never below this
    /// anchor.
    ///
    /// `None` until the session first writes (a draw or a history commit), and
    /// again after an erase or a full-screen clear resets the projection.
    /// Unlike `clear_from_y` (pending resize state consumed by the next draw)
    /// this survives draws and commits: the exit erase anchors here, so it
    /// covers the whole range the session painted rather than just the live
    /// viewport.
    session_top: Option<u16>,
    /// Rows this session has pushed the screen up with `append_lines`.
    ///
    /// Every one of them moves the session's first painted row up by one, so
    /// the exit erase anchors at `session_top - scrolled_rows` rather than at
    /// a recorded absolute row. Deriving the anchor from scroll distance
    /// instead of a saved row is what makes it survive viewport growth,
    /// `scroll_screen_up`, and a mid-session resize: the terminal window
    /// always shows the tail of the same content stream, so the session's
    /// first row moves up with the screen instead of going stale.
    scrolled_rows: u32,
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
            session_top: None,
            scrolled_rows: 0,
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
        // The session anchor deliberately survives: Ctrl-L blanks the
        // projection rows but leaves this session's committed transcript on
        // screen, and the exit erase must still cover those rows. Only the
        // pending resize state is consumed here.
        Ok(())
    }

    /// Erase everything this session painted, from its first row to the bottom
    /// of the screen, keeping native scrollback.
    ///
    /// Decision (#1544, revising #1520): an inline exit leaves only the exit
    /// handoff and the shell prompt on screen. #1520 anchored the erase at the
    /// live mutable viewport and preserved committed scrollback, but a long
    /// session commits nearly its whole conversation through `insert_before`
    /// *while it runs*, so that anchor had nothing left to remove by the time
    /// the user quit — the chat stayed on screen above the handoff. The erase
    /// now covers the full range the UI painted for this session: the first
    /// row it ever wrote, shifted up by every row the session scrolled the
    /// screen. That is the whole screen once the session has scrolled at all,
    /// and exactly the rows the session wrote when it has not, so a short
    /// session still leaves the user's earlier terminal output alone.
    ///
    /// Scrollback above the visible screen is never touched: the erase is
    /// `ClearType::AfterCursor` from the anchor, with no `ESC[3J`. The
    /// conversation stays scrollable, output from before the session survives,
    /// and nothing depends on scrollback-purge support that varies by terminal.
    ///
    /// The erase is exact and idempotent: it collapses the viewport and clears
    /// all tracking, so repeats (second restore, `Drop`, editor handoff) clear
    /// nothing. When the session has painted nothing, a stale `fallback_y` is
    /// ignored so output printed after a restore (for example the `--update`
    /// result) is never wiped.
    pub fn erase_session_projection(&mut self, fallback_y: Option<u16>) -> Result<(), B::Error> {
        let screen_height = self.backend.size()?.height;
        if screen_height == 0 {
            return Ok(());
        }
        let mut top = self.session_projection_top();
        let mut dirty = top.is_some();
        if let Some(y) = self.clear_from_y {
            top = Some(top.map_or(y, |top: u16| top.min(y)));
            dirty = true;
        }
        if !dirty {
            return Ok(());
        }
        // The caller's composer row is a hint about a projection the model no
        // longer tracks, and it sits *inside* the live viewport. Narrowing the
        // erase to it is what left the conversation on screen in #1520, so it
        // is only consulted when the session extent is unknown.
        let top = top.or_else(|| fallback_y.filter(|&y| y < screen_height));
        let Some(top) = top else {
            return Ok(());
        };
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
        self.session_top = None;
        self.scrolled_rows = 0;
        self.viewport_area.height = 0;
        self.viewport_area.y = top;
        self.backend.flush()
    }

    /// The topmost row this session can still own, or `None` when it has
    /// painted nothing.
    fn session_projection_top(&self) -> Option<u16> {
        let session_top = self.session_top?;
        let scrolled = u16::try_from(self.scrolled_rows).unwrap_or(u16::MAX);
        Some(session_top.saturating_sub(scrolled))
    }

    /// Record the session anchor at the first row the session writes, before
    /// any scrolling moves it. Commits can run before the first draw (a
    /// resumed session replays its transcript), so both writers arm it.
    fn arm_session_top(&mut self) {
        if self.session_top.is_none() {
            self.session_top = Some(self.viewport_area.y);
        }
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
        // The screen is blank, so the session projection restarts at the
        // origin and the scroll distance measured so far is irrelevant. The
        // next write re-arms the anchor.
        self.session_top = None;
        self.scrolled_rows = 0;
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
            // A resize moves the viewport inside the same content stream, so
            // the session anchor stays put: `scrolled_rows` already accounts
            // for every row the screen has been pushed up.
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

        if area.height > 0 {
            // Anchor the session before the growth scroll below moves the
            // viewport, so the exit erase can follow the projection up instead
            // of pointing at a row that has already scrolled into scrollback.
            self.arm_session_top();
        }

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
        // A resumed session commits its transcript before the first draw, so
        // anchor here too: without it the commit's own scroll would be
        // measured against no anchor and the exit erase would reach above the
        // session into the user's own terminal output.
        self.arm_session_top();
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
        // Committed lines now own every row above the viewport, but they are
        // still this session's output and still on screen, so the session
        // anchor and its scroll distance stay: the exit erase covers them
        // (that is the #1544 decision) instead of stopping at the live
        // viewport. A resize pending-clear is clamped below the committed rows
        // because the next draw repaints from there.
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
            // Every scrolled row moves the session's first painted row up one,
            // which is what keeps the exit erase anchored on the session
            // instead of on a stale absolute row.
            self.scrolled_rows = self.scrolled_rows.saturating_add(u32::from(rows));
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

    /// Trimmed text of every row of `buffer`, top to bottom; blank rows come
    /// back as empty strings.
    fn rows_of(buffer: &Buffer, width: u16) -> Vec<String> {
        (0..buffer.area.height)
            .map(|row| {
                (0..width)
                    .map(|column| buffer[(column, row)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect()
    }

    /// A screen already holding `above` rows of pre-session shell output, with
    /// the cursor parked on row `cursor_y` where an inline session starts.
    fn inline_session(
        width: u16,
        height: u16,
        cursor_y: u16,
        above: u16,
    ) -> InlineTerminal<TestBackend> {
        let filled = format!("shell {}", "o".repeat(usize::from(width) - 6));
        let blank = " ".repeat(usize::from(width));
        let backend = TestBackend::with_lines((0..height).map(|row| {
            ratatui::text::Line::from(if row < above {
                filled.as_str()
            } else {
                blank.as_str()
            })
        }));
        InlineTerminal::with_size_and_cursor(
            backend,
            Size::new(width, height),
            Position::new(0, cursor_y),
        )
    }

    fn commit(terminal: &mut InlineTerminal<TestBackend>, rows: u16, text: &str) {
        terminal
            .insert_before(rows, |buffer| {
                for row in 0..rows {
                    buffer.set_string(0, row, text, ratatui::style::Style::default());
                }
            })
            .unwrap();
    }

    fn composer(terminal: &mut InlineTerminal<TestBackend>, height: u16, text: &str) {
        terminal
            .draw_height(height, |frame| {
                frame.render_widget(
                    ratatui::widgets::Paragraph::new(text),
                    Rect::new(0, 0, frame.area().width, 1),
                );
            })
            .unwrap();
    }

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

    /// #1544 decision, regression: a long session commits nearly its whole
    /// conversation while it runs, so the exit erase must cover every row the
    /// session painted, not just the live mutable viewport. The rows the
    /// session already pushed into scrollback stay there — the erase must not
    /// purge scrollback, which would destroy output from before the session
    /// too and would depend on terminal-specific `ESC[3J` support.
    #[test]
    fn erase_clears_the_whole_session_projection_and_keeps_scrollback() {
        let mut terminal = inline_session(20, 8, 7, 7);
        composer(&mut terminal, 6, "composer");
        // A long session: far more committed rows than the screen can hold.
        commit(&mut terminal, 4, "chat");
        composer(&mut terminal, 6, "composer again");

        terminal.erase_session_projection(None).unwrap();

        assert!(
            rows_of(terminal.backend().buffer(), 20)
                .iter()
                .all(|row| row.is_empty()),
            "conversation survived the exit erase: {:?}",
            rows_of(terminal.backend().buffer(), 20)
        );
        assert_eq!(terminal.area(), Rect::new(0, 0, 20, 0));
        let scrollback = rows_of(terminal.backend().scrollback(), 20);
        assert_eq!(scrollback.len(), 9, "{scrollback:?}");
        assert!(
            scrollback
                .iter()
                .filter(|row| row.contains("shell"))
                .count()
                >= 5,
            "pre-session output was destroyed: {scrollback:?}"
        );
        assert!(
            scrollback.iter().any(|row| row.contains("chat")),
            "committed conversation is no longer scrollable: {scrollback:?}"
        );
    }

    /// A session that never scrolls the screen owns only the rows it wrote, so
    /// the terminal output above it (the user's own commands) survives — even
    /// when the caller passes a stale row that would narrow the erase.
    #[test]
    fn erase_stops_at_the_first_row_the_session_painted() {
        let mut terminal = inline_session(20, 12, 7, 7);
        composer(&mut terminal, 3, "composer");
        commit(&mut terminal, 2, "chat");
        assert_eq!(terminal.scrolled_rows, 0);

        terminal.erase_session_projection(Some(0)).unwrap();

        let rows = rows_of(terminal.backend().buffer(), 20);
        assert!(
            rows[..7].iter().all(|row| row.starts_with("shell")),
            "rows the session never painted were erased: {rows:?}"
        );
        assert!(
            rows[7..].iter().all(|row| row.is_empty()),
            "session rows survived: {rows:?}"
        );
        assert_eq!(terminal.area(), Rect::new(0, 7, 20, 0));
        assert!(terminal.backend().scrollback().area.height == 0);
    }

    /// The session-start row goes stale the moment the screen scrolls: the
    /// anchor is the start row shifted up by the rows the session scrolled, not
    /// the row recorded at startup.
    #[test]
    fn erase_follows_the_session_up_when_the_screen_scrolls() {
        let mut terminal = inline_session(20, 12, 11, 11);
        composer(&mut terminal, 5, "composer");
        commit(&mut terminal, 3, "chat");
        assert_eq!(terminal.scrolled_rows, 7);

        terminal.erase_session_projection(None).unwrap();

        let rows = rows_of(terminal.backend().buffer(), 20);
        assert!(
            rows[..4].iter().all(|row| row.starts_with("shell")),
            "rows above the session extent were erased: {rows:?}"
        );
        assert!(
            rows[4..].iter().all(|row| row.is_empty()),
            "session rows survived the scroll: {rows:?}"
        );
        assert_eq!(terminal.area(), Rect::new(0, 4, 20, 0));
        let scrollback = rows_of(terminal.backend().scrollback(), 20);
        assert_eq!(scrollback.len(), 7, "{scrollback:?}");
    }

    /// Replaces #1520's `erase_covers_tracked_minimum_after_resize_without_redraw`.
    /// The reason it asserted the opposite is #1544: the old test pinned
    /// "committed rows above the live viewport stay on screen", which is
    /// exactly the reported bug. What it was really protecting against is
    /// still checked here: a resize without a redraw must not strand rows.
    #[test]
    fn erase_after_a_resize_without_a_redraw_still_covers_the_session() {
        let mut terminal = inline_session(20, 12, 7, 7);
        composer(&mut terminal, 3, "composer");
        commit(&mut terminal, 2, "chat");
        terminal.backend_mut().resize(20, 20);
        terminal.autoresize().unwrap();

        terminal.erase_session_projection(None).unwrap();

        let rows = rows_of(terminal.backend().buffer(), 20);
        assert!(
            rows[..7].iter().all(|row| row.starts_with("shell")),
            "pre-session rows were erased: {rows:?}"
        );
        assert!(
            rows[7..].iter().all(|row| row.is_empty()),
            "the resize stranded session rows: {rows:?}"
        );
        assert_eq!(terminal.area(), Rect::new(0, 7, 20, 0));
    }

    /// Replaces #1520's `erase_after_late_commits_preserves_all_scrollback`.
    /// That test asserted the committed rows above the live viewport survive
    /// the erase; #1544 is the report that they must not. The preserved half
    /// of the old decision is still covered: scrollback is not purged.
    #[test]
    fn erase_after_late_commits_still_covers_committed_rows() {
        let mut terminal = inline_session(20, 12, 7, 7);
        composer(&mut terminal, 3, "stale");
        commit(&mut terminal, 2, "first");
        commit(&mut terminal, 2, "second");

        terminal.erase_session_projection(None).unwrap();

        let rows = rows_of(terminal.backend().buffer(), 20);
        assert!(
            rows.iter()
                .all(|row| !row.contains("first") && !row.contains("second")),
            "committed rows survived: {rows:?}"
        );
        assert!(
            rows[5..].iter().all(|row| row.is_empty()),
            "session rows survived: {rows:?}"
        );
        assert!(
            rows[..5].iter().all(|row| row.starts_with("shell")),
            "pre-session rows were erased: {rows:?}"
        );
        assert_eq!(terminal.area(), Rect::new(0, 5, 20, 0));
        assert_eq!(rows_of(terminal.backend().scrollback(), 20).len(), 2);
    }

    /// Replaces #1520's `erase_with_live_projection_keeps_scrollback_above`.
    /// The composer row is a hint for a projection the model no longer tracks;
    /// it must never narrow the erase, or the conversation stays on screen.
    #[test]
    fn a_composer_fallback_never_narrows_the_erase_to_the_live_viewport() {
        let mut terminal = inline_session(20, 12, 11, 11);
        composer(&mut terminal, 5, "composer");
        commit(&mut terminal, 3, "chat");

        terminal.erase_session_projection(Some(10)).unwrap();

        let rows = rows_of(terminal.backend().buffer(), 20);
        assert!(
            rows[..4].iter().all(|row| row.starts_with("shell")),
            "pre-session rows were erased: {rows:?}"
        );
        assert!(
            rows[4..].iter().all(|row| row.is_empty()),
            "the fallback narrowed the erase: {rows:?}"
        );
        assert_eq!(terminal.area(), Rect::new(0, 4, 20, 0));
    }

    /// Ctrl-L blanks the projection rows but leaves the session's committed
    /// transcript on screen, so the session anchor must survive it: the exit
    /// erase still owns those rows.
    #[test]
    fn erase_after_clear_still_covers_the_committed_transcript() {
        let mut terminal = inline_session(20, 12, 7, 7);
        composer(&mut terminal, 3, "composer");
        commit(&mut terminal, 2, "chat");
        terminal.clear().unwrap();
        assert!(
            rows_of(terminal.backend().buffer(), 20)[7].contains("chat"),
            "Ctrl-L must keep the committed transcript"
        );

        terminal.erase_session_projection(None).unwrap();

        let rows = rows_of(terminal.backend().buffer(), 20);
        assert!(
            rows[7..].iter().all(|row| row.is_empty()),
            "the committed transcript survived: {rows:?}"
        );
        assert_eq!(terminal.area(), Rect::new(0, 7, 20, 0));
    }

    /// A mid-session resize replaces the projection: the old rows are cleared
    /// and the transcript is replayed from the origin, so the exit erase owns
    /// the whole screen from then on.
    #[test]
    fn erase_after_a_transcript_replacement_covers_the_whole_screen() {
        let mut terminal = inline_session(20, 12, 7, 7);
        composer(&mut terminal, 3, "composer");
        commit(&mut terminal, 2, "old");
        terminal.clear_screen().unwrap();
        composer(&mut terminal, 3, "replayed");
        commit(&mut terminal, 2, "chat");

        terminal.erase_session_projection(None).unwrap();

        assert!(
            rows_of(terminal.backend().buffer(), 20)
                .iter()
                .all(|row| row.is_empty()),
            "the replayed transcript survived: {:?}",
            rows_of(terminal.backend().buffer(), 20)
        );
        assert_eq!(terminal.area(), Rect::new(0, 0, 20, 0));
    }

    /// A resumed session replays its transcript before the first draw, so the
    /// commit has to anchor the session too.
    #[test]
    fn erase_anchors_a_commit_that_precedes_the_first_draw() {
        let mut terminal = inline_session(20, 12, 11, 11);
        commit(&mut terminal, 4, "replay");
        composer(&mut terminal, 2, "composer");

        terminal.erase_session_projection(None).unwrap();

        let rows = rows_of(terminal.backend().buffer(), 20);
        assert!(
            rows.iter().all(|row| !row.contains("replay")),
            "the replayed transcript survived: {rows:?}"
        );
        assert!(
            rows[..6].iter().all(|row| row.starts_with("shell")),
            "pre-session rows were erased: {rows:?}"
        );
        assert_eq!(terminal.area(), Rect::new(0, 6, 20, 0));
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
        terminal.erase_session_projection(Some(3)).unwrap();
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
        terminal.erase_session_projection(Some(3)).unwrap();
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
