use crate::inline_terminal::InlineTerminal;
use crossterm::{
    cursor::{MoveTo, SetCursorStyle},
    event::{
        self, DisableBracketedPaste, DisableFocusChange, EnableBracketedPaste, EnableFocusChange,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{self, Clear, ClearType},
};
use ratatui::backend::CrosstermBackend;
use std::future::Future;
use std::io;

#[derive(Debug, Default)]
struct Lifecycle {
    restored: bool,
}

impl Lifecycle {
    fn active() -> Self {
        Self { restored: false }
    }

    fn is_active(&self) -> bool {
        !self.restored
    }

    fn mark_active(&mut self) {
        self.restored = false;
    }

    fn mark_restored(&mut self) {
        self.restored = true;
    }

    fn is_restored(&self) -> bool {
        self.restored
    }
}

pub(crate) struct TerminalRuntime {
    terminal: InlineTerminal<CrosstermBackend<io::Stdout>>,
    lifecycle: Lifecycle,
}

impl TerminalRuntime {
    pub(crate) fn start() -> Result<Self, Box<dyn std::error::Error>> {
        terminal::enable_raw_mode()?;

        let mut stdout = io::stdout();
        if let Err(error) = execute!(
            stdout,
            EnableBracketedPaste,
            EnableFocusChange,
            SetCursorStyle::BlinkingBar,
            crossterm::style::Print("\x1b]0;rustcode · new session\x07")
        ) {
            let _ = terminal::disable_raw_mode();
            return Err(Box::new(error));
        }

        let _ = execute!(
            stdout,
            event::PushKeyboardEnhancementFlags(
                event::KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
            )
        );

        let backend = CrosstermBackend::new(stdout);
        let terminal = match InlineTerminal::new(backend) {
            Ok(terminal) => terminal,
            Err(error) => {
                let _ = terminal::disable_raw_mode();
                return Err(Box::new(error));
            }
        };

        Ok(Self {
            terminal,
            lifecycle: Lifecycle::active(),
        })
    }

    pub(crate) fn terminal(&mut self) -> &mut InlineTerminal<CrosstermBackend<io::Stdout>> {
        &mut self.terminal
    }

    pub(crate) fn restore(&mut self) -> io::Result<()> {
        self.restore_at(None)
    }

    pub(crate) fn restore_at(&mut self, cursor_y: Option<u16>) -> io::Result<()> {
        if self.lifecycle.is_restored() {
            return Ok(());
        }

        terminal::disable_raw_mode()?;
        let area = self.terminal.area();
        let transcript_end =
            cursor_y.unwrap_or_else(|| area.y.saturating_add(area.height.saturating_sub(1)));
        execute!(
            self.terminal.backend_mut(),
            DisableBracketedPaste,
            DisableFocusChange,
            PopKeyboardEnhancementFlags,
            SetCursorStyle::DefaultUserShape,
            MoveTo(0, transcript_end),
            Clear(ClearType::FromCursorDown)
        )?;
        self.terminal.show_cursor()?;
        self.lifecycle.mark_restored();
        Ok(())
    }

    async fn activate(&mut self) -> io::Result<()> {
        if self.lifecycle.is_active() {
            return Ok(());
        }

        terminal::enable_raw_mode()?;
        execute!(
            self.terminal.backend_mut(),
            EnableBracketedPaste,
            EnableFocusChange,
            PushKeyboardEnhancementFlags(
                event::KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
            ),
            SetCursorStyle::BlinkingBar
        )?;
        self.lifecycle.mark_active();
        Ok(())
    }

    pub(crate) async fn with_restored<F, Fut, T>(&mut self, f: F) -> T
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = T>,
    {
        let was_active = self.lifecycle.is_active();
        if was_active {
            let _ = self.restore();
        }
        let result = f().await;
        if was_active {
            let _ = self.activate().await;
        }
        result
    }
}

impl Drop for TerminalRuntime {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

#[cfg(test)]
mod tests {
    use super::Lifecycle;

    #[test]
    fn restoring_lifecycle_is_idempotent() {
        let mut lifecycle = Lifecycle::active();

        assert!(lifecycle.is_active());

        lifecycle.mark_restored();
        lifecycle.mark_restored();

        assert!(lifecycle.is_restored());
    }
}
