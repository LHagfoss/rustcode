import Testing
@testable import RustCodeRemote

@Suite struct CommandSuggestionsTests {
    let catalog = [
        SessionCommandChoice(name: "/help", description: "All commands", insertion: "/help"),
        SessionCommandChoice(name: "/session", description: "Show current session ID, token budget", insertion: "/session"),
        SessionCommandChoice(name: "/sessions", description: "Sessions", insertion: "/sessions"),
        SessionCommandChoice(name: "/history", description: "Browse sessions", insertion: "/history"),
        SessionCommandChoice(name: "/status", description: "Session status", insertion: "/status"),
        SessionCommandChoice(name: "resume", description: "Resume a session", insertion: "/resume"),
    ]

    func names(_ query: String, limit: Int = 8) -> [String] {
        CommandSuggestions.matches(query, in: catalog, limit: limit).map(\.name)
    }

    @Test func listsEveryCommandForABareSlash() {
        #expect(names("/") == catalog.map(\.name))
        #expect(names("/", limit: 2) == ["/help", "/session"])
    }

    @Test func ranksExactThenPrefixThenContainsThenDescription() {
        #expect(names("/session") == ["/session", "/sessions", "/history", "/status", "resume"])
        #expect(names("/sessi") == ["/session", "/sessions", "/history", "/status", "resume"])
        #expect(names("/tat") == ["/status"])
        #expect(names("/SES").prefix(2) == ["/session", "/sessions"])
    }

    @Test func stopsMatchingOnceArgumentsAreTyped() {
        #expect(names("/session now").isEmpty)
        #expect(names("hello").isEmpty)
    }

    @Test func completesOnlyTheUntypedSuffixOfAPrefixMatch() {
        #expect(CommandSuggestions.completionSuffix(for: "/sessi", in: catalog) == "on")
        #expect(CommandSuggestions.completionSuffix(for: "/SESSI", in: catalog) == "on")
        #expect(CommandSuggestions.completionSuffix(for: "/res", in: catalog) == "ume")
        #expect(CommandSuggestions.completionSuffix(for: "/session", in: catalog) == nil)
        #expect(CommandSuggestions.completionSuffix(for: "/tat", in: catalog) == nil)
        #expect(CommandSuggestions.completionSuffix(for: "/", in: catalog) == nil)
        #expect(CommandSuggestions.completionSuffix(for: "/sessi ", in: catalog) == nil)
    }
}

@Suite struct NoticePresentationTests {
    @Test func collapsesTheHostsCancellationNoticesIntoOne() {
        let items = [
            TranscriptItem(id: "u1", .user("first")),
            TranscriptItem(id: "n1", .notice("Request cancelled by user")),
            TranscriptItem(id: "n2", .notice("[harness: turn stopped — cancelled]")),
            TranscriptItem(id: "u2", .user("second")),
            TranscriptItem(id: "n3", .notice("[harness: turn stopped — cancelled]")),
            TranscriptItem(id: "n4", .notice("Request cancelled by user")),
            TranscriptItem(id: "n5", .notice("Queued: more")),
        ]
        #expect(NoticePresentation.notices(in: items) == [
            .init(id: "n1", text: NoticePresentation.cancellationText),
            .init(id: "n3", text: NoticePresentation.cancellationText),
            .init(id: "n5", text: "Queued: more"),
        ])
    }

    @Test func keepsOtherNoticesVerbatim() {
        let items = [TranscriptItem(id: "a", .notice("Steering the running turn")),
                     TranscriptItem(id: "b", .notice("Steering the running turn"))]
        #expect(NoticePresentation.notices(in: items).map(\.id) == ["a", "b"])
    }
}
