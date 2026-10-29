import Foundation

/// What ⌘W (Close Terminal Tab) does to the terminal panel, and whether the
/// panel stays once a terminal has closed.
///
/// One press closes at most one terminal. Holding ⌘W a moment too long sends
/// auto-repeats of the same key, and each used to close another terminal and
/// then the panel, so a press meant for one terminal closed them all (#2594).
public enum TerminalPanelClose {
    public enum Step: Equatable {
        /// Leave everything as it is.
        case nothing
        /// The panel shows no terminal of the open project: hide it.
        case hidePanel
        /// Close this terminal.
        case close(TerminalSession.ID)
    }

    /// The step for a ⌘W with the panel `shown` and `selected` as the open
    /// project's selected terminal. A held key's repeats do nothing.
    public static func step(panelShown: Bool, selected: TerminalSession.ID?, isKeyRepeat: Bool) -> Step {
        guard panelShown, !isKeyRepeat else { return .nothing }
        return selected.map(Step.close) ?? .hidePanel
    }

    /// After a terminal closed, the panel stays while the open project has
    /// terminals left and goes with its last one.
    public static func keepsPanel(remaining: Int) -> Bool {
        remaining > 0
    }
}
