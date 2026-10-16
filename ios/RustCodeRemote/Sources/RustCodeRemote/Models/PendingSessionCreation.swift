import Foundation

/// A `create_session` request the host has not confirmed. Retrying the same folder reuses
/// `requestID`, which is also the identifier the host records the request under.
public struct PendingSessionCreation: Equatable, Sendable {
    /// `nil` asks the host for its default folder.
    public let directory: String?
    public let requestID: RequestID
    public let startedAt: Date
    /// What the host, or the connection, last said about the request. `nil` while the first
    /// answer is still awaited.
    public var hostMessage: String?

    public init(directory: String?, requestID: RequestID, startedAt: Date, hostMessage: String? = nil) {
        self.directory = directory
        self.requestID = requestID
        self.startedAt = startedAt
        self.hostMessage = hostMessage
    }
}
