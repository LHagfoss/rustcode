use crate::inline_terminal::InlineTerminal;
use crate::ui::TuiEventStream;
use crossterm::{
    cursor::{MoveTo, SetCursorStyle},
    event::{
        self, DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
        EnableFocusChange, EnableMouseCapture, PopKeyboardEnhancementFlags,
        PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{self, Clear, ClearType},
};
use ratatui::backend::CrosstermBackend;
use std::future::Future;
use std::io::{self, Write};
use std::sync::{
    Once,
    atomic::{AtomicBool, Ordering},
};

static FULLSCREEN_ACTIVE: AtomicBool = AtomicBool::new(false);
static PANIC_HOOK: Once = Once::new();

#[derive(Debug, Default)]
struct AlternateScreen {
    active: bool,
}

impl AlternateScreen {
    fn is_active(&self) -> bool {
        self.active
    }

    fn enter(&mut self, out: &mut impl Write) -> io::Result<()> {
        if self.active {
            return Ok(());
        }
        // A partial write can have entered the alternate screen. Keep cleanup
        // armed until a leave sequence has been written successfully.
        self.active = true;
        out.write_all(b"\x1b[?1049h")?;
        out.flush()
    }

    fn leave(&mut self, out: &mut impl Write) -> io::Result<()> {
        if !self.active {
            return Ok(());
        }
        out.write_all(b"\x1b[?1049l")?;
        out.flush()?;
        self.active = false;
        Ok(())
    }
}

fn install_panic_restore() {
    PANIC_HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if FULLSCREEN_ACTIVE.load(Ordering::SeqCst) {
                let mut out = io::stdout();
                let _ = execute!(out, PopKeyboardEnhancementFlags);
                let _ = execute!(
                    out,
                    DisableBracketedPaste,
                    DisableFocusChange,
                    DisableMouseCapture,
                    SetCursorStyle::DefaultUserShape,
                    crossterm::style::Print("\x1b]9;4;0;0\x07")
                );
                if out.write_all(b"\x1b[?1049l\x1b[?25h").is_ok() && out.flush().is_ok() {
                    FULLSCREEN_ACTIVE.store(false, Ordering::SeqCst);
                }
                let _ = terminal::disable_raw_mode();
            }
            previous(info);
        }));
    });
}

fn restore_partial_start(out: &mut impl Write, screen: &mut AlternateScreen) {
    let _ = execute!(out, PopKeyboardEnhancementFlags);
    let _ = execute!(
        out,
        DisableBracketedPaste,
        DisableFocusChange,
        DisableMouseCapture,
        SetCursorStyle::DefaultUserShape
    );
    if screen.leave(out).is_ok() {
        FULLSCREEN_ACTIVE.store(false, Ordering::SeqCst);
    }
    let _ = terminal::disable_raw_mode();
}

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

    #[allow(dead_code)]
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
    alternate_screen: AlternateScreen,
    fullscreen: bool,
    mouse_capture: bool,
}

impl TerminalRuntime {
    pub(crate) fn start(
        fullscreen_requested: bool,
        mouse_capture: bool,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        terminal::enable_raw_mode()?;

        let mut stdout = io::stdout();
        let mut alternate_screen = AlternateScreen::default();
        if let Err(error) = execute!(
            stdout,
            EnableBracketedPaste,
            EnableFocusChange,
            SetCursorStyle::BlinkingBar,
            crossterm::style::Print("\x1b]0;rustcode · new session\x07"),
            crossterm::style::Print("\x1b]9;4;0;0\x07")
        ) {
            restore_partial_start(&mut stdout, &mut alternate_screen);
            return Err(Box::new(error));
        }

        let _ = execute!(
            stdout,
            event::PushKeyboardEnhancementFlags(
                event::KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
            )
        );

        if fullscreen_requested {
            install_panic_restore();
            FULLSCREEN_ACTIVE.store(true, Ordering::SeqCst);
            if alternate_screen.enter(&mut stdout).is_err() {
                if let Err(error) = alternate_screen.leave(&mut stdout) {
                    restore_partial_start(&mut stdout, &mut alternate_screen);
                    return Err(Box::new(error));
                }
                FULLSCREEN_ACTIVE.store(false, Ordering::SeqCst);
            }
        }
        let fullscreen = alternate_screen.is_active();
        // Both modes now paint a full-height viewport. Enable wheel input only
        // on local terminals whose capabilities we recognize, so tmux and SSH
        // sessions never receive an unverified mouse protocol sequence.
        if mouse_capture {
            let _ = execute!(stdout, EnableMouseCapture);
        }
        let backend = CrosstermBackend::new(stdout);
        let terminal = match if fullscreen {
            InlineTerminal::new_at_origin(backend)
        } else {
            InlineTerminal::new(backend)
        } {
            Ok(terminal) => terminal,
            Err(error) => {
                if fullscreen {
                    let mut out = io::stdout();
                    restore_partial_start(&mut out, &mut alternate_screen);
                } else {
                    restore_partial_start(&mut io::stdout(), &mut alternate_screen);
                }
                return Err(Box::new(error));
            }
        };

        Ok(Self {
            terminal,
            lifecycle: Lifecycle::active(),
            alternate_screen,
            fullscreen,
            mouse_capture,
        })
    }

    pub(crate) fn terminal(&mut self) -> &mut InlineTerminal<CrosstermBackend<io::Stdout>> {
        &mut self.terminal
    }

    pub(crate) fn restore(&mut self) -> io::Result<()> {
        self.restore_at(None)
    }

    pub(crate) fn restore_at(&mut self, _cursor_y: Option<u16>) -> io::Result<()> {
        if self.lifecycle.is_restored() && !self.alternate_screen.is_active() {
            return Ok(());
        }

        let raw_result = terminal::disable_raw_mode();
        let area = self.terminal.area();
        // Keyboard enhancement flags are unsupported by Crossterm's legacy
        // Windows console API. Startup already treats enabling them as
        // best-effort; cleanup must do the same or a normal quit reports a
        // spurious `Unsupported` error after the app has otherwise exited.
        let _ = execute!(self.terminal.backend_mut(), PopKeyboardEnhancementFlags);
        let mode_result = execute!(
            self.terminal.backend_mut(),
            DisableBracketedPaste,
            DisableFocusChange,
            DisableMouseCapture,
            SetCursorStyle::DefaultUserShape,
            crossterm::style::Print("\x1b]9;4;0;0\x07")
        );
        let screen_result = if self.alternate_screen.is_active() {
            let result = self.alternate_screen.leave(&mut io::stdout());
            if result.is_ok() {
                FULLSCREEN_ACTIVE.store(false, Ordering::SeqCst);
            }
            result
        } else if self.fullscreen {
            Ok(())
        } else {
            execute!(
                self.terminal.backend_mut(),
                // The full-height inline projection is transient. Clearing
                // from its first row keeps committed native scrollback above
                // it and avoids duplicating chat beside the exit handoff.
                MoveTo(0, area.y),
                Clear(ClearType::FromCursorDown)
            )
        };
        let cursor_result = self.terminal.show_cursor();
        let result = raw_result
            .and(mode_result)
            .and(screen_result)
            .and(cursor_result);
        if result.is_ok() {
            self.lifecycle.mark_restored();
        }
        result
    }

    #[cfg(unix)]
    pub(crate) async fn suspend(&mut self) -> io::Result<()> {
        self.restore()?;
        // Ctrl-Z is read as a key in raw mode, so the shell cannot suspend us
        // until raw mode and the alternate screen have been released.
        unsafe { libc::raise(libc::SIGTSTP) };
        self.activate().await
    }

    pub(crate) fn is_fullscreen(&self) -> bool {
        self.fullscreen
    }

    #[allow(dead_code)]
    async fn activate(&mut self) -> io::Result<()> {
        if self.lifecycle.is_active() {
            return Ok(());
        }

        terminal::enable_raw_mode()?;
        self.lifecycle.mark_active();
        execute!(
            self.terminal.backend_mut(),
            EnableBracketedPaste,
            EnableFocusChange,
            SetCursorStyle::BlinkingBar
        )?;
        if self.mouse_capture {
            let _ = execute!(self.terminal.backend_mut(), EnableMouseCapture);
        }
        let _ = execute!(
            self.terminal.backend_mut(),
            PushKeyboardEnhancementFlags(
                event::KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
            )
        );
        if self.alternate_screen.is_active() {
            FULLSCREEN_ACTIVE.store(true, Ordering::SeqCst);
        } else if self.fullscreen_requested() {
            FULLSCREEN_ACTIVE.store(true, Ordering::SeqCst);
            self.alternate_screen.enter(&mut io::stdout())?;
            self.terminal.clear_screen()?;
        }
        Ok(())
    }

    fn fullscreen_requested(&self) -> bool {
        // A restored fullscreen session remains opted in for editor handoff
        // and job control. Inline sessions never switch screens on resume.
        self.fullscreen
    }

    #[allow(dead_code)]
    pub(crate) async fn with_restored<F, Fut, T>(
        &mut self,
        events: &mut TuiEventStream,
        f: F,
    ) -> io::Result<T>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = T>,
    {
        struct ResumeEvents<'a>(&'a mut TuiEventStream);
        impl Drop for ResumeEvents<'_> {
            fn drop(&mut self) {
                self.0.resume();
            }
        }

        events.pause();
        let _resume_events = ResumeEvents(events);
        let was_active = self.lifecycle.is_active();
        if was_active {
            self.restore()?;
        }
        let result = f().await;
        if was_active {
            self.activate().await?;
        }
        Ok(result)
    }
}

impl Drop for TerminalRuntime {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

#[cfg(test)]
mod tests {
    use super::{AlternateScreen, Lifecycle};

    #[test]
    fn alternate_screen_enter_leave_writes_only_screen_switches() {
        let mut screen = AlternateScreen::default();
        let mut output = Vec::new();
        screen.enter(&mut output).unwrap();
        screen.enter(&mut output).unwrap();
        screen.leave(&mut output).unwrap();
        screen.leave(&mut output).unwrap();
        assert_eq!(output, b"\x1b[?1049h\x1b[?1049l");
    }

    #[test]
    fn alternate_screen_restore_is_retryable_after_write_failure() {
        struct FailingWriter;
        impl std::io::Write for FailingWriter {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("disconnected"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut screen = AlternateScreen::default();
        assert!(screen.enter(&mut FailingWriter).is_err());
        assert!(screen.is_active());
        assert!(screen.leave(&mut FailingWriter).is_err());
        assert!(screen.is_active());
        let mut output = Vec::new();
        screen.leave(&mut output).unwrap();
        assert_eq!(output, b"\x1b[?1049l");
        assert!(!screen.is_active());
    }

    #[test]
    fn restoring_lifecycle_is_idempotent() {
        let mut lifecycle = Lifecycle::active();

        assert!(lifecycle.is_active());

        lifecycle.mark_restored();
        lifecycle.mark_restored();

        assert!(lifecycle.is_restored());
    }
}
