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

    /// The toast text for a notice: a host turn-stop marker such as
    /// "[harness: turn stopped — loop_escalation]" reads as a short sentence.
    public static func displayText(_ text: String) -> String {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard trimmed.hasPrefix("[harness: turn stopped — "), trimmed.hasSuffix("]") else { return text }
        let reason = String(trimmed.dropFirst("[harness: turn stopped — ".count).dropLast())
        let parts = reason.split(separator: ":", maxSplits: 1).map(String.init)
        let kind = parts.first ?? reason
        let detail = parts.count > 1 ? parts[1] : ""
        switch kind {
        case "cancelled": return cancellationText
        case "loop_escalation": return "Stopped: the agent was repeating itself"
        case "recovery_failed": return "Stopped: the turn could not recover"
        case "unverified": return "Stopped: completion could not be verified"
        case "unavailable_tool": return "Stopped: a required tool is unavailable"
        case "provider_error": return detail.isEmpty ? "Stopped: provider error" : "Stopped: provider error \(detail)"
        case "budget": return "Stopped: \(detail.replacingOccurrences(of: "_", with: " ")) limit reached"
        case "infrastructure_failure": return "Stopped: infrastructure failure"
        case "connect_timeout", "header_timeout", "first_event_timeout", "stream_idle_timeout":
            return "Stopped: the provider timed out"
        case "premature_eof", "response_body_decode", "malformed_sse":
            return "Stopped: the provider connection dropped"
        default: return "Stopped: \(reason.replacingOccurrences(of: "_", with: " "))"
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
                notices.append(Notice(id: item.id, text: displayText(text)))
            default:
                break
            }
        }
        return notices
    }
}
