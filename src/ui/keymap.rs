use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalKind {
    Generic,
    Tmux,
    VsCode,
    Warp,
    AppleTerminal,
}

impl Default for TerminalKind {
    fn default() -> Self {
        Self::Generic
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyAction {
    Insert(char),
    Submit,
    InsertNewline,
    Cancel,
    ClearScreen,
    Paste,
    MoveLeft,
    MoveRight,
    MoveWordLeft,
    MoveWordRight,
    MoveStart,
    MoveEnd,
    DeleteBackward,
    DeleteForward,
    DeleteWordBackward,
    DeleteWordForward,
    KillLineStart,
    HistoryPrevious,
    HistoryNext,
    Complete,
    CommandPaletteOrPreviousSuggestion,
    NextSuggestion,
    ToggleAutoConfirm,
    Escape,
    Unhandled,
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct KeyMap {
    terminal: TerminalKind,
}

impl KeyMap {
    pub(crate) fn for_terminal(terminal: TerminalKind) -> Self {
        Self { terminal }
    }

    pub(crate) fn from_environment() -> Self {
        let program = std::env::var("TERM_PROGRAM").unwrap_or_default();
        let term = std::env::var("TERM").unwrap_or_default();
        let terminal = if term == "screen" || term.starts_with("tmux") {
            TerminalKind::Tmux
        } else if program.eq_ignore_ascii_case("vscode") {
            TerminalKind::VsCode
        } else if program.eq_ignore_ascii_case("WarpTerminal") {
            TerminalKind::Warp
        } else if program.eq_ignore_ascii_case("Apple_Terminal") {
            TerminalKind::AppleTerminal
        } else {
            TerminalKind::Generic
        };
        Self { terminal }
    }

    pub(crate) fn resolve(&self, key: KeyEvent) -> KeyAction {
        let modifiers = key.modifiers;
        let ctrl = modifiers.contains(KeyModifiers::CONTROL);
        let meta = modifiers.contains(KeyModifiers::META);
        let alt = modifiers.contains(KeyModifiers::ALT) || meta;
        let super_key = modifiers.contains(KeyModifiers::SUPER);

        match key.code {
            KeyCode::Enter => {
                if modifiers
                    .intersects(KeyModifiers::SHIFT | KeyModifiers::CONTROL | KeyModifiers::ALT)
                {
                    KeyAction::InsertNewline
                } else {
                    KeyAction::Submit
                }
            }
            KeyCode::Esc => KeyAction::Escape,
            KeyCode::BackTab => KeyAction::ToggleAutoConfirm,
            KeyCode::Tab => KeyAction::Complete,
            KeyCode::Up => KeyAction::HistoryPrevious,
            KeyCode::Down => KeyAction::HistoryNext,
            KeyCode::Left if alt || ctrl => KeyAction::MoveWordLeft,
            KeyCode::Right if alt || ctrl => KeyAction::MoveWordRight,
            KeyCode::Left => KeyAction::MoveLeft,
            KeyCode::Right => KeyAction::MoveRight,
            KeyCode::Home => KeyAction::MoveStart,
            KeyCode::End => KeyAction::MoveEnd,
            KeyCode::Backspace if super_key => KeyAction::KillLineStart,
            KeyCode::Backspace if alt || ctrl => KeyAction::DeleteWordBackward,
            KeyCode::Backspace => KeyAction::DeleteBackward,
            KeyCode::Delete if super_key => KeyAction::KillLineStart,
            KeyCode::Delete if alt => KeyAction::DeleteWordForward,
            KeyCode::Delete => KeyAction::DeleteForward,
            KeyCode::Char('c') | KeyCode::Char('C') if ctrl => KeyAction::Cancel,
            KeyCode::Char('l') | KeyCode::Char('L') if ctrl => KeyAction::ClearScreen,
            KeyCode::Char('v') | KeyCode::Char('V') if ctrl || super_key || meta => {
                KeyAction::Paste
            }
            KeyCode::Char('p') | KeyCode::Char('P') if ctrl => {
                KeyAction::CommandPaletteOrPreviousSuggestion
            }
            KeyCode::Char('n') | KeyCode::Char('N') if ctrl => KeyAction::NextSuggestion,
            KeyCode::Char('o') | KeyCode::Char('O') if ctrl => KeyAction::InsertNewline,
            KeyCode::Char('a') | KeyCode::Char('A') if ctrl => KeyAction::MoveStart,
            KeyCode::Char('e') | KeyCode::Char('E') if ctrl => KeyAction::MoveEnd,
            KeyCode::Char('u') | KeyCode::Char('U') if ctrl || super_key => {
                KeyAction::KillLineStart
            }
            KeyCode::Char('w') | KeyCode::Char('W') if ctrl => KeyAction::DeleteWordBackward,
            KeyCode::Char('b') | KeyCode::Char('B') if alt => KeyAction::MoveWordLeft,
            KeyCode::Char('f') | KeyCode::Char('F') if alt => KeyAction::MoveWordRight,
            KeyCode::Char('d') | KeyCode::Char('D') if alt => KeyAction::DeleteWordForward,
            KeyCode::Char('∫') if matches!(self.terminal, TerminalKind::AppleTerminal) => {
                KeyAction::MoveWordLeft
            }
            KeyCode::Char('∫') => KeyAction::MoveWordLeft,
            KeyCode::Char('ƒ') if matches!(self.terminal, TerminalKind::AppleTerminal) => {
                KeyAction::MoveWordRight
            }
            KeyCode::Char('ƒ') => KeyAction::MoveWordRight,
            KeyCode::Char('∂') if matches!(self.terminal, TerminalKind::AppleTerminal) => {
                KeyAction::DeleteWordForward
            }
            KeyCode::Char('∂') => KeyAction::DeleteWordForward,
            KeyCode::Char(c) if !ctrl && !alt && !super_key && !c.is_control() => {
                KeyAction::Insert(c)
            }
            _ => KeyAction::Unhandled,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{KeyAction, KeyMap, TerminalKind};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn explicit_bindings_cover_navigation_submit_cancel_and_picker() {
        let map = KeyMap::default();

        assert_eq!(
            map.resolve(key(KeyCode::Left, KeyModifiers::ALT)),
            KeyAction::MoveWordLeft
        );
        assert_eq!(
            map.resolve(key(KeyCode::Enter, KeyModifiers::NONE)),
            KeyAction::Submit
        );
        assert_eq!(
            map.resolve(key(KeyCode::Enter, KeyModifiers::SHIFT)),
            KeyAction::InsertNewline
        );
        assert_eq!(
            map.resolve(key(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            KeyAction::Cancel
        );
        assert_eq!(
            map.resolve(key(KeyCode::Char('p'), KeyModifiers::CONTROL)),
            KeyAction::CommandPaletteOrPreviousSuggestion
        );
    }

    #[test]
    fn terminal_fallbacks_keep_backtab_and_mac_word_keys_usable() {
        let map = KeyMap::for_terminal(TerminalKind::AppleTerminal);

        assert_eq!(
            map.resolve(key(KeyCode::BackTab, KeyModifiers::NONE)),
            KeyAction::ToggleAutoConfirm
        );
        assert_eq!(
            map.resolve(key(KeyCode::Char('∫'), KeyModifiers::NONE)),
            KeyAction::MoveWordLeft
        );
        assert_eq!(
            map.resolve(key(KeyCode::Char('ƒ'), KeyModifiers::NONE)),
            KeyAction::MoveWordRight
        );
    }
}
