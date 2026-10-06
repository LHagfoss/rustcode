//! Passive terminal capability estimates for optional fullscreen startup.
//!
//! This probe never sends terminal queries or reads stdin. Active replies would
//! race crossterm's event reader and could consume a user's first keystrokes.

use std::io::IsTerminal;

/// The color depth inferred from terminal environment variables.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ColorDepth {
    None,
    Ansi16,
    Ansi256,
    TrueColor,
}

/// Conservative, best-effort capabilities. These are hints, not a terminal
/// protocol negotiation: no user input or terminal output is touched.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TerminalCapabilities {
    pub(crate) cursor_position_report: bool,
    pub(crate) color_depth: ColorDepth,
    pub(crate) keyboard_enhancement: bool,
    alternate_screen: bool,
}

impl TerminalCapabilities {
    const fn unsupported() -> Self {
        Self {
            cursor_position_report: false,
            color_depth: ColorDepth::None,
            keyboard_enhancement: false,
            alternate_screen: false,
        }
    }

    pub(crate) const fn supports_alternate_screen(self) -> bool {
        self.alternate_screen
    }
}

#[derive(Clone, Copy)]
struct Environment<'a> {
    term: &'a str,
    color_term: &'a str,
    term_program: &'a str,
    kitty_window_id: bool,
    tmux: bool,
    ssh: bool,
    /// A Windows console host (Windows Terminal, ConPTY, or a console window).
    /// `TERM` is usually unset there, so the Unix `TERM`-prefix heuristics below
    /// never match and Windows was classified as an unsupported terminal. That
    /// silently disabled mouse capture (no wheel scroll) and `--fullscreen`.
    windows_console: bool,
    stdin_tty: bool,
    stdout_tty: bool,
}

/// Return immediately from local process attributes. The 250 ms startup
/// budget is an upper bound for active terminal probes; this passive probe
/// has no wait or input read and therefore needs no timer.
pub(crate) fn probe() -> TerminalCapabilities {
    let term = std::env::var("TERM").unwrap_or_default();
    let color_term = std::env::var("COLORTERM").unwrap_or_default();
    let term_program = std::env::var("TERM_PROGRAM").unwrap_or_default();
    infer(Environment {
        term: &term,
        color_term: &color_term,
        term_program: &term_program,
        kitty_window_id: std::env::var_os("KITTY_WINDOW_ID").is_some(),
        tmux: std::env::var_os("TMUX").is_some() || std::env::var_os("TMUX_PANE").is_some(),
        ssh: std::env::var_os("SSH_TTY").is_some()
            || std::env::var_os("SSH_CONNECTION").is_some()
            || std::env::var_os("SSH_CLIENT").is_some(),
        windows_console: windows_console(),
        stdin_tty: std::io::stdin().is_terminal(),
        stdout_tty: std::io::stdout().is_terminal(),
    })
}

/// Whether we are on a Windows console. There is no reliable `TERM` value to
/// key on (Windows Terminal and ConPTY leave it unset by default), so the host
/// OS plus the Windows Terminal / console-host markers are the signal.
fn windows_console() -> bool {
    windows_console_from_signals(
        cfg!(target_os = "windows"),
        std::env::var_os("WT_SESSION").is_some(),
        std::env::var_os("WT_PROFILE_ID").is_some(),
    )
}

fn windows_console_from_signals(windows_host: bool, wt_session: bool, wt_profile_id: bool) -> bool {
    windows_host || wt_session || wt_profile_id
}

fn infer(env: Environment<'_>) -> TerminalCapabilities {
    let term = env.term.to_ascii_lowercase();
    let term_program = env.term_program.to_ascii_lowercase();
    // A Windows console has no `TERM` by default, so emptiness only disqualifies
    // a terminal when it is not a Windows console host.
    let usable_tty = env.stdin_tty
        && env.stdout_tty
        && (env.windows_console || !term.is_empty())
        && !matches!(term.as_str(), "dumb" | "unknown" | "emacs");
    if !usable_tty {
        return TerminalCapabilities::unsupported();
    }

    let color_depth = if env.color_term.eq_ignore_ascii_case("truecolor")
        || env.color_term.eq_ignore_ascii_case("24bit")
        || term.contains("direct")
        || term.contains("truecolor")
    {
        ColorDepth::TrueColor
    } else if term.contains("256color") || (env.windows_console && term.is_empty()) {
        // A modern Windows console host (Windows Terminal / ConPTY) is at
        // least 256-color even with `TERM` unset or the legacy `ansi` value.
        ColorDepth::Ansi256
    } else {
        ColorDepth::Ansi16
    };

    // A passive hint cannot establish how tmux or a remote peer forwards
    // queries. Leave fullscreen and keyboard protocol off in these cases.
    let direct_terminal = !env.tmux && !env.ssh && !term.starts_with("screen");
    let known_alternate_screen = env.windows_console
        || [
            "xterm",
            "rxvt",
            "alacritty",
            "foot",
            "kitty",
            "wezterm",
            "ghostty",
            "st-",
        ]
        .iter()
        .any(|prefix| term.starts_with(prefix));
    let keyboard_enhancement = direct_terminal
        && (env.kitty_window_id
            || term.contains("kitty")
            || matches!(term_program.as_str(), "wezterm" | "ghostty" | "kitty"));

    TerminalCapabilities {
        cursor_position_report: direct_terminal,
        color_depth,
        keyboard_enhancement,
        alternate_screen: direct_terminal && known_alternate_screen,
    }
}

#[cfg(test)]
mod tests {
    use super::{ColorDepth, Environment, TerminalCapabilities, infer};
    use crate::inline_terminal::InlineTerminal;
    use ratatui::{backend::TestBackend, layout::Rect, widgets::Paragraph};

    fn environment<'a>(term: &'a str) -> Environment<'a> {
        Environment {
            term,
            color_term: "",
            term_program: "",
            kitty_window_id: false,
            tmux: false,
            ssh: false,
            windows_console: false,
            stdin_tty: true,
            stdout_tty: true,
        }
    }

    #[test]
    fn local_xterm_can_enter_fullscreen_with_cursor_reports() {
        let capabilities = infer(environment("xterm-256color"));
        assert!(capabilities.supports_alternate_screen());
        assert!(capabilities.cursor_position_report);
        assert_eq!(capabilities.color_depth, ColorDepth::Ansi256);
    }

    #[test]
    fn truecolor_and_known_keyboard_protocol_are_inferred() {
        let mut env = environment("xterm-256color");
        env.color_term = "truecolor";
        env.term_program = "WezTerm";
        let capabilities = infer(env);
        assert_eq!(capabilities.color_depth, ColorDepth::TrueColor);
        assert!(capabilities.keyboard_enhancement);
    }

    #[test]
    fn tmux_and_ssh_do_not_attempt_fullscreen_takeover() {
        let mut env = environment("xterm-256color");
        env.tmux = true;
        let tmux = infer(env);
        assert!(!tmux.supports_alternate_screen());
        assert!(!tmux.cursor_position_report);
        assert!(!tmux.keyboard_enhancement);

        env.tmux = false;
        env.ssh = true;
        let ssh = infer(env);
        assert!(!ssh.supports_alternate_screen());
        assert!(!ssh.cursor_position_report);
    }

    #[test]
    fn non_tty_or_dumb_terminal_falls_back_to_inline() {
        for mut env in [environment("dumb"), environment("xterm")].into_iter() {
            if env.term == "xterm" {
                env.stdin_tty = false;
            }
            let capabilities = infer(env);
            assert_eq!(capabilities, TerminalCapabilities::unsupported());
        }
    }

    #[test]
    fn legacy_vt100_does_not_claim_alternate_screen() {
        let capabilities = infer(environment("vt100"));
        assert!(!capabilities.supports_alternate_screen());
    }

    /// Windows Terminal / ConPTY leave `TERM` unset. Previously that alone made
    /// the terminal look unsupported, which disabled mouse capture (so the wheel
    /// never scrolled the transcript) and refused `--fullscreen`. A Windows
    /// console host with an empty `TERM` is now a usable direct terminal.
    #[test]
    fn windows_console_without_term_is_a_supported_direct_terminal() {
        let mut env = environment("");
        env.windows_console = true;
        let capabilities = infer(env);
        assert!(
            capabilities.supports_alternate_screen(),
            "a Windows console host must be able to enter fullscreen"
        );
        assert!(
            capabilities.cursor_position_report,
            "a Windows console host is a direct terminal"
        );
        assert_eq!(capabilities.color_depth, ColorDepth::Ansi256);
    }

    /// The Windows console host is recognized via `WT_SESSION`/`WT_PROFILE_ID`
    /// as well as the host OS. Exercise each signal directly so this test does
    /// not depend on the machine running it.
    #[test]
    fn windows_terminal_markers_are_recognized_across_hosts() {
        assert!(super::windows_console_from_signals(true, false, false));
        assert!(super::windows_console_from_signals(false, true, false));
        assert!(super::windows_console_from_signals(false, false, true));
        assert!(!super::windows_console_from_signals(false, false, false));
    }

    /// A genuine unsupported terminal — no tty, or `dumb`/`unknown`/`emacs` —
    /// still falls back to inline even on a Windows console.
    #[test]
    fn unsupported_windows_console_still_falls_back_to_inline() {
        let mut env = environment("dumb");
        env.windows_console = true;
        assert_eq!(infer(env), TerminalCapabilities::unsupported());

        let mut detached = environment("");
        detached.windows_console = true;
        detached.stdout_tty = false;
        assert_eq!(infer(detached), TerminalCapabilities::unsupported());
    }

    #[test]
    fn unsupported_fullscreen_keeps_inline_render_golden() {
        let mut env = environment("xterm-256color");
        env.tmux = true;
        assert!(!infer(env).supports_alternate_screen());
        let mut terminal = InlineTerminal::new_at_origin(TestBackend::new(24, 4)).unwrap();
        terminal
            .draw_height(1, |frame| {
                frame.render_widget(Paragraph::new("inline fallback"), Rect::new(0, 0, 24, 1));
            })
            .unwrap();
        let row: String = (0..24)
            .map(|x| terminal.backend().buffer()[(x, 0)].symbol())
            .collect();
        assert_eq!(
            format!("{}\n", row.trim_end()),
            include_str!("ui/fixtures/fullscreen_inline_fallback.txt")
        );
    }
}
