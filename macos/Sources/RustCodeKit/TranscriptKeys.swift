import Foundation

/// Reading a long chat from the keyboard (#2191).
///
/// - Page Up / Page Down (fn-↑ / fn-↓ on a laptop) scroll the chat by its
///   visible height, less a little kept for context.
/// - Home / End (fn-← / fn-→) go to the first message and to the latest one;
///   End follows the chat's output again, as the Latest button does.
///
/// The keys reach the chat while the composer or a search field has focus:
/// none of them edit text there (a text field's Home and End only scroll its
/// own document, and the composer never grows tall enough to page). With
/// Shift, Option, Control or Command they are left alone, so selecting by page
/// still works in the composer. They never reach the chat from the terminal,
/// the preview or an open sheet, which have their own uses for them.
public enum TranscriptKeys {
    public enum Action: Equatable, Sendable {
        case pageUp, pageDown, top, end
    }

    /// Where the keyboard focus is when a key comes in.
    public enum Focus: Equatable, Sendable {
        /// The chat itself, a button or nothing in particular.
        case chat
        /// The composer or a one-line field such as chat search.
        case textField
        /// The workspace terminal, which pages its own scrollback.
        case terminal
        /// The localhost preview's page.
        case web
        /// A sheet or panel over the chat, such as Settings.
        case panel
    }

    /// macOS virtual key codes.
    public enum KeyCode {
        public static let home: UInt16 = 115
        public static let pageUp: UInt16 = 116
        public static let end: UInt16 = 119
        public static let pageDown: UInt16 = 121
    }

    /// The chat action for a key press, or nil to leave the key to whoever
    /// has focus. `modified` is whether Shift, Option, Control or Command is
    /// held; the Function key alone does not count, as laptops need it to
    /// type these keys at all.
    public static func action(keyCode: UInt16, modified: Bool, focus: Focus) -> Action? {
        guard !modified, focus == .chat || focus == .textField else { return nil }
        switch keyCode {
        case KeyCode.pageUp: return .pageUp
        case KeyCode.pageDown: return .pageDown
        case KeyCode.home: return .top
        case KeyCode.end: return .end
        default: return nil
        }
    }

    /// Kept from one page to the next, so a line read last is still in view.
    public static let pageOverlap: Double = 40

    /// How far one page scrolls in a chat this tall between its insets.
    public static func pageDistance(visible: Double) -> Double {
        max(visible - pageOverlap, pageOverlap)
    }

    /// A page down that would reach the end goes all the way there and holds
    /// on, so output streaming in afterwards stays in view. `distanceFromBottom`
    /// is how much further the chat can scroll, when known.
    public static func resolved(_ action: Action, distanceFromBottom: Double?, visible: Double) -> Action {
        guard action == .pageDown, let distanceFromBottom else { return action }
        return distanceFromBottom <= pageDistance(visible: visible) + ChatFollow.threshold ? .end : action
    }
}
