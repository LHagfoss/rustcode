/// A screen rectangle recorded on shared state.
///
/// Shared state must not name a rendering crate's types: everything that is
/// not the terminal UI (headless runs, ACP, the daemon, native frontends)
/// links this state too. Rendering code builds this from its own rect at the
/// edge (see `UiRect::new`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UiRect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl UiRect {
    pub const fn new(x: u16, y: u16, width: u16, height: u16) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// True when a screen cell falls inside this rectangle.
    ///
    /// Matches `ratatui::layout::Rect::contains`: left/top inclusive,
    /// right/bottom exclusive.
    pub const fn contains(&self, column: u16, row: u16) -> bool {
        column >= self.x
            && column < self.x.saturating_add(self.width)
            && row >= self.y
            && row < self.y.saturating_add(self.height)
    }
}
