use crate::inline_terminal::InlineTerminal;
use crossterm::{
    cursor::SetCursorStyle,
    event::{
        self, DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
        EnableFocusChange, EnableMouseCapture, PopKeyboardEnhancementFlags,
        PushKeyboardEnhancementFlags,
    },
    execute, terminal,
};
use ratatui::backend::CrosstermBackend;
use std::io::{self, Write};
use std::sync::{
    Once, OnceLock,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, ThreadId};

static FULLSCREEN_ACTIVE: AtomicBool = AtomicBool::new(false);
/// Latched when the terminal-owner thread panics. This alone never suppresses
/// cleanup: a recovered panic (background tool/turn/child, or even a caught
/// owner panic) must not disable later `/exit`, Ctrl-C/Ctrl-D, suspend or
/// editor cleanup. Only the conjunction with a live unwind on the restoring
/// thread — see `panic_suppresses_erase` — keeps the projection. (#1564)
static PANIC_UNWINDING: AtomicBool = AtomicBool::new(false);
/// Thread that owns the terminal. The panic hook records every thread panic,
/// but only the owner's panic arms `PANIC_UNWINDING`; a recovered background
/// panic returns the app to idle and must leave later cleanup intact.
static PANIC_OWNER: OnceLock<ThreadId> = OnceLock::new();
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

/// Whether a panic on `panicking_thread` should arm the owner-scoped panic
/// latch. Panics on any other thread are background panics: the controller
/// recovers and returns the app to idle, so they must not affect later
/// cleanup. When no owner is recorded yet (panic before startup), arm the
/// latch to preserve the previous emergency behavior.
fn panic_from_owner(owner: Option<ThreadId>, panicking_thread: ThreadId) -> bool {
    owner.map(|id| id == panicking_thread).unwrap_or(true)
}

/// Whether the panic latch suppresses the exit erase right now. The latch
/// records that the owner panicked; `thread_unwinding` reports whether the
/// restoring thread is actively unwinding. A recovered panic — background or
/// caught owner — has `thread_unwinding == false`, so normal cleanup resumes.
/// An actual terminal-owner unwind keeps the projection and the panic message.
fn panic_suppresses_erase(owner_latched: bool, thread_unwinding: bool) -> bool {
    owner_latched && thread_unwinding
}

fn install_panic_restore() {
    PANIC_HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let from_owner = panic_from_owner(PANIC_OWNER.get().copied(), thread::current().id());
            if from_owner {
                PANIC_UNWINDING.store(true, Ordering::SeqCst);
            }
            // Only the owner tears down the screen: a recovered background
            // panic must leave raw mode and the alternate screen alone, or
            // the still-running session loses its terminal mid-turn. (#1564)
            if from_owner && FULLSCREEN_ACTIVE.load(Ordering::SeqCst) {
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

/// Whether a restore should erase the inline session projection before the
/// shell takes the screen back.
///
/// Fullscreen sessions paint on the alternate screen: leaving it below
/// restores the shell view, so there are no main-screen rows to erase. A
/// panic is not an exit, so the projection and the panic message stay.
fn should_erase_session_projection(
    alternate_screen: bool,
    fullscreen: bool,
    panicking: bool,
) -> bool {
    !panicking && !alternate_screen && !fullscreen
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

        // Both modes need the hook: fullscreen to release the alternate screen
        // before the message prints, inline to arm `PANIC_UNWINDING` so the
        // restore on `Drop` keeps the projection and the panic message during
        // an actual owner unwind. The latch is owner-scoped (a recovered
        // background panic never arms it) and only suppresses the erase while
        // the restoring thread is actively unwinding, so later normal exits,
        // suspend/resume and editor handoffs clean up as usual. (#1564)
        let _ = PANIC_OWNER.get_or_init(|| thread::current().id());
        install_panic_restore();
        if fullscreen_requested {
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

    /// Paint one frame inside a synchronized update (DEC private mode 2026),
    /// so the terminal presents it whole. A scroll rewrites nearly every row,
    /// and without the bracket a terminal can show the top of the new frame
    /// above the bottom of the old one. Terminals without the mode ignore it.
    pub(crate) fn draw_height_synchronized<F>(&mut self, height: u16, render: F) -> io::Result<bool>
    where
        F: FnOnce(&mut crate::inline_terminal::Frame<'_>),
    {
        struct EndSynchronizedUpdate;
        impl Drop for EndSynchronizedUpdate {
            // Also runs when `render` panics: a terminal left mid-update
            // stops repainting until its own timeout expires.
            fn drop(&mut self) {
                let mut out = io::stdout();
                let _ = out.write_all(b"\x1b[?2026l");
                let _ = out.flush();
            }
        }
        io::stdout().write_all(b"\x1b[?2026h")?;
        let _end = EndSynchronizedUpdate;
        self.terminal.draw_height(height, render)
    }

    pub(crate) fn restore(&mut self) -> io::Result<()> {
        self.restore_at(None)
    }

    pub(crate) fn restore_at(&mut self, cursor_y: Option<u16>) -> io::Result<()> {
        // Erase the whole inline projection on every restore — exit, Ctrl-Z
        // suspend, editor handoff — not just the first. Returning early for
        // an already-restored lifecycle left stale rows above the exit handoff
        // whenever the viewport had grown or scrolled since. The erase covers
        // every row this session painted, from the row it started at (kept
        // correct by scroll distance, so growth, `scroll_screen_up` and a
        // mid-session resize cannot strand it) down to the bottom of the
        // screen. It is exact and idempotent, so repeats are safe. A panic
        // suppresses the erase only while the restoring thread is actively
        // unwinding from an owner panic; a recovered background panic (or a
        // caught owner panic) has already returned the app to idle, so later
        // restores erase normally.
        // (#1564; #1545 deliberately keeps the projection on a live unwind.)
        //
        // The erase is enough now, because the transcript is not in native
        // scrollback: the readable conversation lives in this mutable viewport,
        // which is re-projected from the render snapshot every frame, and
        // `preserve_transcript_scrollback` is the opt-in that would put rows
        // where no erase can reach them. #1587 is the report that they used to
        // be there; with the opt-in, that trade is the user's to make.
        let erase_result = if should_erase_session_projection(
            self.alternate_screen.is_active(),
            self.fullscreen,
            panic_suppresses_erase(PANIC_UNWINDING.load(Ordering::SeqCst), thread::panicking()),
        ) {
            self.terminal.erase_session_projection(cursor_y)
        } else {
            Ok(())
        };
        if self.lifecycle.is_restored() && !self.alternate_screen.is_active() {
            return erase_result;
        }

        let raw_result = terminal::disable_raw_mode();
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
        } else {
            // The inline session projection was already erased above.
            Ok(())
        };
        let cursor_result = self.terminal.show_cursor();
        let result = erase_result
            .and(raw_result)
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
}

impl Drop for TerminalRuntime {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AlternateScreen, Lifecycle, panic_from_owner, panic_suppresses_erase,
        should_erase_session_projection,
    };

    #[test]
    fn inline_restore_erases_the_projection_but_fullscreen_and_panics_do_not() {
        assert!(should_erase_session_projection(false, false, false));
        assert!(!should_erase_session_projection(true, false, false));
        assert!(!should_erase_session_projection(false, true, false));
        // A panic is not an exit: the conversation and the panic message are
        // the only context the user has, so nothing is erased.
        assert!(!should_erase_session_projection(false, false, true));
        assert!(!should_erase_session_projection(true, true, true));
    }

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

    #[test]
    fn only_the_owner_thread_arms_the_panic_latch() {
        let owner = std::thread::current().id();
        assert!(panic_from_owner(Some(owner), owner));
        let background = std::thread::spawn(|| std::thread::current().id())
            .join()
            .unwrap();
        assert_ne!(owner, background);
        // A recovered background tool/turn/child panic must not arm the latch,
        // or every later /exit, Ctrl-C/Ctrl-D, suspend and editor cleanup
        // would skip the erase even though the app is back at idle. (#1564)
        assert!(!panic_from_owner(Some(owner), background));
        // Before startup no owner is recorded; keep the emergency behavior.
        assert!(panic_from_owner(None, background));
    }

    #[test]
    fn only_an_active_owner_unwind_suppresses_the_erase() {
        // No panic at all: normal exits erase.
        assert!(!panic_suppresses_erase(false, false));
        // Recovered background panic never arms the latch, so even while some
        // other thread unwinds the owner's restore still erases. This is the
        // #1564 regression: the old code latched globally forever, so any
        // thread panic suppressed every later restore.
        // Owner panicked and the restoring thread is unwinding: keep the
        // projection and the panic message. (#1545 behavior, retained.)
        assert!(panic_suppresses_erase(true, true));
        // Owner panic recovered (caught) and the app is back at idle: later
        // restores erase normally again.
        assert!(!panic_suppresses_erase(true, false));
    }

    #[test]
    fn recovered_background_panic_leaves_later_restores_erasing() {
        // End-to-end through the decision function: simulate a background
        // panic that never arms the latch, followed by a normal restore on
        // the idle owner thread. The erase must still happen.
        let owner = std::thread::current().id();
        let background = std::thread::spawn(|| std::thread::current().id())
            .join()
            .unwrap();
        let latched = panic_from_owner(Some(owner), background);
        assert!(!latched);
        assert!(should_erase_session_projection(
            false,
            false,
            panic_suppresses_erase(latched, false),
        ));
        // And while the owner itself unwinds, the erase stays suppressed.
        assert!(!should_erase_session_projection(
            false,
            false,
            panic_suppresses_erase(true, true),
        ));
    }
}
