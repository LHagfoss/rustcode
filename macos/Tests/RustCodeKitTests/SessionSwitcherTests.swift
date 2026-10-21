import XCTest
@testable import RustCodeKit

final class SessionSwitcherTests: XCTestCase {
    private let sessions = [
        SessionSummary(id: "a", cwd: "/w/app", title: "Refactor the parser", model: "openai/gpt-6"),
        SessionSummary(id: "b", cwd: "/w/site", title: "Parser bug in lexer"),
        SessionSummary(id: "c", cwd: "/w/app", title: "Fix sparse arrays"),
        SessionSummary(id: "d", cwd: "/w/parsers", title: "Unrelated"),
        SessionSummary(id: "e", cwd: "/w/app", title: "Deploy notes", model: "anthropic/claude-sonnet"),
        SessionSummary(id: "f", cwd: "/w/app", title: nil),
    ]

    private func ids(_ query: String, pinned: Set<String> = [], content: [SessionContentMatch] = []) -> [String] {
        SessionSwitcher.results(sessions: sessions, query: query, pinned: pinned, contentMatches: content).map(\.id)
    }

    func testEmptyQueryListsPinnedFirstThenTheSidebarOrder() {
        XCTAssertEqual(ids(""), ["a", "b", "c", "d", "e", "f"])
        XCTAssertEqual(ids("  ", pinned: ["e", "c"]), ["c", "e", "a", "b", "d", "f"])
    }

    /// Title start, then a word start, then anywhere in the title, then the
    /// project, then the model.
    func testRanksTitleMatchesAboveProjectAndModel() {
        XCTAssertEqual(ids("parser"), ["b", "a", "d"])
        XCTAssertEqual(ids("pars"), ["b", "a", "c", "d"])
        XCTAssertEqual(ids("sonnet"), ["e"])
        XCTAssertEqual(ids("site"), ["b"])
    }

    func testMatchingIgnoresCaseAndDiacritics() {
        XCTAssertEqual(ids("PARSER"), ids("parser"))
        XCTAssertEqual(ids("dëploy"), ["e"])
    }

    func testUntitledChatsMatchAsNewChat() {
        XCTAssertEqual(ids("new chat"), ["f"])
    }

    /// Chats found by their messages follow the title matches, once each,
    /// with the text around the match; ones outside the scope are dropped.
    func testContentMatchesFollowWithTheirSnippet() {
        let content = [
            SessionContentMatch(id: "e", title: nil, snippet: "…parser config…", matches: 2, cwd: nil),
            SessionContentMatch(id: "a", title: nil, snippet: "dup", matches: 1, cwd: nil),
            SessionContentMatch(id: "zz", title: nil, snippet: "other project", matches: 1, cwd: nil),
        ]
        let results = SessionSwitcher.results(sessions: sessions, query: "parser", pinned: [], contentMatches: content)
        XCTAssertEqual(results.map(\.id), ["b", "a", "d", "e"])
        XCTAssertNil(results[1].snippet)
        XCTAssertEqual(results[3].snippet, "…parser config…")
    }

    func testResultsAreCapped() {
        let many = (0..<200).map { SessionSummary(id: "s\($0)", cwd: "/w", title: "Chat \($0)") }
        XCTAssertEqual(SessionSwitcher.results(sessions: many, query: "", pinned: []).count, SessionSwitcher.limit)
        XCTAssertEqual(SessionSwitcher.results(sessions: many, query: "chat", pinned: [], limit: 5).count, 5)
    }

    /// ⌘K ⏎ goes somewhere: the open chat is skipped for the first highlight.
    func testInitialSelectionSkipsTheOpenChat() {
        let results = SessionSwitcher.results(sessions: sessions, query: "", pinned: [])
        XCTAssertEqual(SessionSwitcher.initialSelection(results, current: "a"), 1)
        XCTAssertEqual(SessionSwitcher.initialSelection(results, current: "c"), 0)
        XCTAssertEqual(SessionSwitcher.initialSelection(results, current: nil), 0)
        XCTAssertEqual(SessionSwitcher.initialSelection([], current: "a"), 0)
        let onlyOpen = [SessionSwitcher.Result(session: sessions[0])]
        XCTAssertEqual(SessionSwitcher.initialSelection(onlyOpen, current: "a"), 0)
    }

    func testArrowsWrapAtEitherEnd() {
        XCTAssertEqual(SessionSwitcher.move(0, by: 1, count: 3), 1)
        XCTAssertEqual(SessionSwitcher.move(2, by: 1, count: 3), 0)
        XCTAssertEqual(SessionSwitcher.move(0, by: -1, count: 3), 2)
        XCTAssertEqual(SessionSwitcher.move(0, by: 1, count: 0), 0)
    }

    /// Rows arriving under the highlight, such as content matches or a later
    /// page, keep it on the same chat.
    func testHighlightStaysOnItsChatAsRowsChange() {
        let before = SessionSwitcher.results(sessions: sessions, query: "parser", pinned: [])
        XCTAssertEqual(SessionSwitcher.keep("a", in: before), 1)
        let content = [SessionContentMatch(id: "e", title: nil, snippet: "x", matches: 1, cwd: nil)]
        let after = SessionSwitcher.results(sessions: sessions, query: "parser", pinned: [], contentMatches: content)
        XCTAssertEqual(after[SessionSwitcher.keep("a", in: after)].id, "a")
        XCTAssertEqual(SessionSwitcher.keep("gone", in: after), 0)
        XCTAssertEqual(SessionSwitcher.keep(nil, in: after), 0)
    }

    /// Letters typed right after ⌘K, before the field has focus, are kept
    /// for the search; keys the switcher or a menu handles are not.
    func testTypeaheadKeepsOnlyTypedText() {
        XCTAssertEqual(SessionSwitcher.typeahead("f", commandKey: false), "f")
        XCTAssertEqual(SessionSwitcher.typeahead("Ø", commandKey: false), "Ø")
        XCTAssertEqual(SessionSwitcher.typeahead(" ", commandKey: false), " ")
        XCTAssertNil(SessionSwitcher.typeahead("k", commandKey: true))
        XCTAssertNil(SessionSwitcher.typeahead(nil, commandKey: false))
        XCTAssertNil(SessionSwitcher.typeahead("", commandKey: false))
        XCTAssertNil(SessionSwitcher.typeahead("\r", commandKey: false))
        XCTAssertNil(SessionSwitcher.typeahead("\t", commandKey: false))
        XCTAssertNil(SessionSwitcher.typeahead("\u{1b}", commandKey: false))
        XCTAssertNil(SessionSwitcher.typeahead("\u{7f}", commandKey: false))
        XCTAssertNil(SessionSwitcher.typeahead("\u{F700}", commandKey: false)) // ↑
    }
}
