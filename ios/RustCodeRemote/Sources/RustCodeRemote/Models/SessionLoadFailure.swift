import Foundation
import os

/// Why the selected session could not be opened. The selection stays, so the unsent draft
/// does too; the view shows the reason with Try Again and Back.
public struct SessionLoadFailure: Sendable, Equatable {
    public enum Category: String, Sendable, Equatable, CaseIterable {
        /// The host no longer shares a session with this ID.
        case notFound = "not_found"
        /// The registration changed under the request, or kept changing while it loaded.
        case stale
        /// The process running the session did not answer.
        case ownerUnavailable = "owner_unavailable"
        /// Another process on the Mac (the app, a terminal) has the session open.
        case openElsewhere = "open_elsewhere"
        /// The host did not answer in time.
        case timeout
        /// The host sent something this app could not read. Not retried on its own.
        case invalidData = "invalid_data"
        /// The host refused for another reason, given in `message`.
        case refused
    }

    public var sessionID: String
    public var category: Category
    /// Ready to show. It may carry the host's own words, so it is never logged.
    public var message: String

    public init(sessionID: String, category: Category, message: String) {
        self.sessionID = sessionID
        self.category = category
        self.message = message
    }

    /// `nil` for failures of the connection itself, which the connection loop reports and
    /// recovers from by attaching again.
    public init?(sessionID: String, error: RemoteError) {
        let category: Category
        switch error {
        case .rejected(.staleSession): category = .stale
        case .rejected(.openElsewhere): category = .openElsewhere
        case .rejected(.other(code: "not_found", _)): category = .notFound
        case .rejected: category = .refused
        case .outcomeUnknown: category = .ownerUnavailable
        case .timeout: category = .timeout
        case .invalidFrame: category = .invalidData
        case .rateLimited: category = .refused
        case .notConnected, .connectionClosed, .unauthorized, .incompatibleProtocol, .codecUnavailable,
             .resyncRequired, .pairingFailed, .sessionClosed, .hostIdentityChanged, .linkExpired, .transport:
            return nil
        }
        self.init(sessionID: sessionID, category: category, message: Self.message(for: category, error: error))
    }

    public var title: String {
        switch category {
        case .notFound: "Thread Not Found"
        case .stale: "Thread Changed"
        case .ownerUnavailable: "Mac Not Responding"
        case .openElsewhere: "Open on the Mac"
        case .timeout: "Mac Not Responding"
        case .invalidData: "Couldn’t Read Thread"
        case .refused: "Couldn’t Open Thread"
        }
    }

    public var systemImage: String {
        switch category {
        case .notFound: "questionmark.folder"
        case .stale: "arrow.triangle.2.circlepath"
        case .ownerUnavailable, .timeout: "clock.badge.exclamationmark"
        case .openElsewhere: "macbook"
        case .invalidData: "exclamationmark.triangle"
        case .refused: "xmark.octagon"
        }
    }

    private static func message(for category: Category, error: RemoteError) -> String {
        switch category {
        case .notFound: "The Mac no longer shares this thread. It may have been closed there."
        case .stale: "The thread was shared again on the Mac while it loaded."
        case .ownerUnavailable: "The Mac did not answer for this thread. It may still be starting."
        case .timeout: "The Mac did not answer in time."
        case .invalidData: "The Mac sent data this app could not read. Updating RustCode on the Mac or this app may help."
        case .openElsewhere, .refused: error.localizedDescription
        }
    }
}

/// Privacy-safe diagnostics for opening sessions: the operation, a session ID prefix, the
/// failure category, the attempt and the time taken. Never prompts, frames, tokens, paths
/// or the host's messages.
enum SessionDiagnostics {
    private static let logger = Logger(subsystem: "com.lhagfoss.rustcode", category: "session")

    static func line(operation: String, sessionID: String, category: SessionLoadFailure.Category?,
                     attempt: Int, elapsed: Duration) -> String {
        let milliseconds = elapsed.components.seconds * 1000 + elapsed.components.attoseconds / 1_000_000_000_000_000
        return "op=\(operation) session=\(sessionID.prefix(8)) result=\(category?.rawValue ?? "ok") attempt=\(attempt) elapsed_ms=\(milliseconds)"
    }

    static func record(operation: String, sessionID: String, category: SessionLoadFailure.Category?,
                       attempt: Int, elapsed: Duration) {
        let line = line(operation: operation, sessionID: sessionID, category: category, attempt: attempt, elapsed: elapsed)
        if category == nil {
            logger.info("\(line, privacy: .public)")
        } else {
            logger.error("\(line, privacy: .public)")
        }
    }
}
