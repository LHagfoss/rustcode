import Foundation
import Testing
@testable import RustCodeRemote

@Suite struct SessionChangesTests {
    private func golden(_ name: String) throws -> RemoteMessage {
        let url = Contract.root.appendingPathComponent("golden/frames/\(name).json")
        return try WireCodec().decode(String(contentsOf: url, encoding: .utf8))
    }

    @Test func theHostsChangesMapWithTheirFreshness() throws {
        guard case .reply(_, .changes(let changes)) = try golden("response_changes") else {
            Issue.record("expected changes")
            return
        }
        #expect(changes.isAvailable)
        #expect(changes.head == "3cf8743")
        #expect(changes.branch == "feat/watch")
        #expect(changes.capturedAt != nil)
        #expect(changes.files.map(\.status) == [.modified, .untracked])
        #expect(changes.additions == 2 && changes.deletions == 1)
        let logo = changes.files[1]
        #expect(logo.isBinary && logo.lines.isEmpty)
        let captured = try #require(changes.capturedAt)
        #expect(changes.freshness(now: captured.addingTimeInterval(30)) == "Read just now")
        #expect(changes.freshness(now: captured.addingTimeInterval(600)) == "Read 10 min ago")
        #expect(changes.isStale(now: captured.addingTimeInterval(600)))
        #expect(!changes.isStale(now: captured.addingTimeInterval(60)))
    }

    @Test func diffLinesAreNumberedAsTheFileIs() throws {
        guard case .reply(_, .changes(let changes)) = try golden("response_changes") else {
            Issue.record("expected changes")
            return
        }
        let lines = changes.files[0].lines
        #expect(lines.first?.kind == .hunk)
        #expect(lines.map(\.kind) == [.hunk, .context, .removed, .added, .added, .context])
        #expect(lines.compactMap(\.number) == [10, 11, 11, 12, 13])
    }

    @Test func aFollowUpQuotesTheSelectedLinesWithTheirNumbers() {
        let file = SessionChanges.File(path: "src/watch.rs", status: .modified, additions: 1, deletions: 0,
                                       diff: "@@ -1,1 +1,2 @@\n keep\n+added\n")
        let draft = SessionChanges.followUp(file: file, lines: file.lines.filter { $0.kind == .added })
        #expect(draft.hasPrefix("About src/watch.rs, line 2:"))
        #expect(draft.contains("+ added"))
        let range = SessionChanges.followUp(file: file, lines: file.lines)
        #expect(range.contains("lines 1–2"))
    }

    @Test func aFolderThatIsNotARepositorySaysWhy() throws {
        let frame = """
        {"kind":"response","protocol_version":1,"request_id":"r","result":{"type":"changes","captured_at":"2026-07-14T09:41:03Z","workspace":"/tmp/x","available":false,"reason":"The workspace is not a Git repository","files":[],"omitted_files":0}}
        """
        guard case .reply(_, .changes(let changes)) = try WireCodec().decode(frame) else {
            Issue.record("expected changes")
            return
        }
        #expect(!changes.isAvailable)
        #expect(changes.reason == "The workspace is not a Git repository")
    }

    @Test func theRequestIsScopedToTheSessionAndReadOnly() throws {
        let ref = SessionRef(sessionID: "s", registrationEpoch: 3)
        let text = try WireCodec().encode(.getChanges(ref), requestID: RequestID("r1"))
        let object = try #require(JSONSerialization.jsonObject(with: Data(text.utf8)) as? [String: Any])
        #expect(object["session_id"] as? String == "s")
        #expect(object["registration_epoch"] as? Int == 3)
        #expect((object["operation"] as? [String: Any])?["type"] as? String == "get_changes")
    }
}
