import Foundation

/// Transcript notices worth announcing, with the host's duplicate cancellation
/// notices collapsed: one cancelled turn can append both "Request cancelled by user"
/// and "[harness: turn stopped — cancelled]", which would otherwise show two toasts.
public enum NoticePresentation {
    public struct Notice: Identifiable, Hashable, Sendable {
        public let id: String
        public let text: String
        public init(id: String, text: String) { self.id = id; self.text = text }
    }

    public static let cancellationText = "Stopped by you"

    public static func isCancellation(_ text: String) -> Bool {
        switch text.trimmingCharacters(in: .whitespacesAndNewlines) {
        case "Request cancelled by user", "[harness: turn stopped — cancelled]", "Turn interrupted by user", "Turn cancelled":
            true
        default:
            false
        }
    }

    /// Notices in transcript order. At most one cancellation notice is kept per user turn.
    public static func notices(in items: [TranscriptItem]) -> [Notice] {
        var cancellationShown = false
        var notices: [Notice] = []
        for item in items {
            switch item.content {
            case .user:
                cancellationShown = false
            case .notice(let text) where isCancellation(text):
                guard !cancellationShown else { continue }
                cancellationShown = true
                notices.append(Notice(id: item.id, text: cancellationText))
            case .notice(let text):
                notices.append(Notice(id: item.id, text: text))
            default:
                break
            }
        }
        return notices
    }
}
