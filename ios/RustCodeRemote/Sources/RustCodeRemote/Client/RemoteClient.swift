import Foundation

/// The result of attaching: where the transcript starts, and everything after it.
public struct SessionAttachment: Sendable {
    public enum Start: Sendable, Equatable {
        /// An authoritative snapshot; replaces whatever the client had.
        case snapshot(SessionSnapshot)
        /// The host can replay everything after the cursor the client supplied; keep the
        /// current state and apply `updates`.
        case resumed
    }

    public var start: Start
    /// Updates after the starting point, in sequence. Throws `RemoteError.resyncRequired`
    /// when the host can no longer deliver a contiguous range.
    public var updates: AsyncThrowingStream<SessionUpdate, any Error>

    public init(start: Start, updates: AsyncThrowingStream<SessionUpdate, any Error>) {
        self.start = start
        self.updates = updates
    }
}

/// Everything the app can ask of a RustCode host. Implemented by `LiveRemoteClient`
/// (WebSocket + codec) and `DemoRemoteClient` (in-memory simulation).
///
/// Mutations take a caller-chosen `RequestID` and return the owner's receipt. A thrown error
/// from a mutation means the outcome is not known; callers must not resend automatically
/// and can ask again with `getRequestStatus`.
public protocol RemoteClient: Sendable {
    /// Exchanges a pairing credential for a device token. Does not leave a connection open.
    func pair(_ request: PairingRequest, deviceName: String) async throws -> StoredCredential

    /// Opens an authenticated connection and reports who answered. Throws
    /// `RemoteError.unauthorized` when the token was revoked.
    @discardableResult
    func connect(using credential: StoredCredential) async throws -> ConnectionIdentity
    func receiptUpdates() async -> AsyncStream<RequestReceipt>
    func disconnect() async

    /// The live shared sessions of the host: the current list first, then every change.
    /// The stream ends (or throws) when the connection does.
    func subscribeSessions() async throws -> AsyncThrowingStream<[SessionSummary], any Error>

    /// Asks for the list again. The answer arrives through the stream `subscribeSessions`
    /// returned.
    func browseDirectories(path: String?) async throws -> DirectoryListing
    func createSession(directory: String?, requestID: RequestID) async throws -> SessionSummary

    /// Optional host-wide operations the host listed with its sessions, such as
    /// `HostCapability.savedSessions`. Empty until the session list has been received.
    func hostCapabilities() async -> Set<String>
    /// One page of the sessions saved on the host, shared or not, newest first.
    func listSavedSessions(cursor: String?, limit: Int, query: String?) async throws -> SavedSessionPage
    /// Reopens a saved session that nothing holds and returns it once it is shared.
    func resumeSession(_ sessionID: String, requestID: RequestID) async throws -> SessionSummary

    func refreshSessions() async throws

    /// - Parameter resumeAfter: the last sequence already applied for this exact registration,
    ///   to ask for a replay instead of a snapshot.
    func attach(_ session: SessionRef, resumeAfter: UInt64?) async throws -> SessionAttachment
    func detach(_ session: SessionRef) async

    func getHistory(_ session: SessionRef, before cursor: HistoryCursor, limit: Int) async throws -> HistoryPage

    /// The session's uncommitted changes, read-only. Needs `HostCapability.sessionChanges`.
    func getChanges(_ session: SessionRef) async throws -> SessionChanges

    /// One chunk of content that another frame carried only in part, for example the full
    /// detail of an approval.
    func getContent(
        _ contentID: String,
        in session: SessionRef,
        offset: UInt64,
        maxBytes: Int
    ) async throws -> ContentChunk

    func uploadImage(_ chunk: ImageUploadChunk, in session: SessionRef, requestID: RequestID) async throws -> ImageUploadProgress

    func executeCommand(_ command: String, in session: SessionRef, requestID: RequestID) async throws -> RequestReceipt
    func cancelQuestion(_ questionID: String, in session: SessionRef, requestID: RequestID) async throws -> RequestReceipt
    func setSessionSettings(model: String, reasoningEffort: String, in session: SessionRef, requestID: RequestID) async throws -> RequestReceipt

    /// Idle-only. Rejected with `.busy` while a turn is running.
    func submit(_ text: String, to session: SessionRef, requestID: RequestID) async throws -> RequestReceipt
    func steer(_ text: String, in session: SessionRef, requestID: RequestID) async throws -> RequestReceipt
    func queue(_ text: String, in session: SessionRef, requestID: RequestID) async throws -> RequestReceipt
    /// - Parameter turnID: the turn the user was looking at, so a stale cancel cannot stop a newer one.
    func cancel(turnID: String, in session: SessionRef, requestID: RequestID) async throws -> RequestReceipt
    func answerQuestion(
        _ questionID: String,
        with answer: QuestionAnswer,
        in session: SessionRef,
        requestID: RequestID
    ) async throws -> RequestReceipt
    func resolveApproval(
        batchID: String,
        decision: ApprovalDecision,
        in session: SessionRef,
        requestID: RequestID
    ) async throws -> RequestReceipt

    /// Looks up the outcome of an earlier mutation. Never creates or repeats a command.
    func getRequestStatus(_ requestID: RequestID, in session: SessionRef) async throws -> RequestReceipt
}

public extension RemoteClient {
    func uploadImage(_ chunk: ImageUploadChunk, in session: SessionRef, requestID: RequestID) async throws -> ImageUploadProgress { throw RemoteError.rejected(.unsupportedOperation) }
    func browseDirectories(path: String?) async throws -> DirectoryListing { throw RemoteError.rejected(.unsupportedOperation) }
    func createSession(directory: String?, requestID: RequestID) async throws -> SessionSummary { throw RemoteError.rejected(.unsupportedOperation) }
    func hostCapabilities() async -> Set<String> { [] }
    func listSavedSessions(cursor: String?, limit: Int, query: String?) async throws -> SavedSessionPage { throw RemoteError.rejected(.unsupportedOperation) }
    func resumeSession(_ sessionID: String, requestID: RequestID) async throws -> SessionSummary { throw RemoteError.rejected(.unsupportedOperation) }
    func getChanges(_ session: SessionRef) async throws -> SessionChanges { throw RemoteError.rejected(.unsupportedOperation) }
    func receiptUpdates() async -> AsyncStream<RequestReceipt> { AsyncStream { $0.finish() } }
    func executeCommand(_ command: String, in session: SessionRef, requestID: RequestID) async throws -> RequestReceipt { RequestReceipt(requestID: requestID, state: .rejected(.unsupportedOperation)) }
    func cancelQuestion(_ questionID: String, in session: SessionRef, requestID: RequestID) async throws -> RequestReceipt { RequestReceipt(requestID: requestID, state: .rejected(.unsupportedOperation)) }
    func setSessionSettings(model: String, reasoningEffort: String, in session: SessionRef, requestID: RequestID) async throws -> RequestReceipt {
        RequestReceipt(requestID: requestID, state: .rejected(.unsupportedOperation))
    }
}
