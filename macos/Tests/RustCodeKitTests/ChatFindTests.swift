import XCTest
@testable import RustCodeKit

@MainActor
final class ChatFindTests: XCTestCase {
    private func transcript(_ texts: [String]) -> Transcript {
        var transcript = Transcript()
        texts.forEach { transcript.appendUser($0) }
        return transcript
    }

    /// A long session: `count` messages of about 1.5 KB, every tenth naming the needle.
    private func longTranscript(count: Int) -> Transcript {
        let filler = String(repeating: "lorem ipsum dolor sit amet consectetur ", count: 40)
        return transcript((0..<count).map { $0 % 10 == 0 ? "\(filler) Needle \($0)" : "\(filler) \($0)" })
    }

    private func settle(_ find: ChatFind, timeout: TimeInterval = 5) async {
        let deadline = Date().addingTimeInterval(timeout)
        while find.isSearching, Date() < deadline { try? await Task.sleep(for: .milliseconds(2)) }
        XCTAssertFalse(find.isSearching, "search did not finish")
    }

    func testMatchesIgnoreCaseAndDiacriticsAndBlankQueries() {
        let items = transcript(["Café latte", "plain", "CAFE"]).items
        XCTAssertEqual(ChatFind.matches(in: items, query: " cafe "), [0, 2])
        XCTAssertEqual(ChatFind.matches(in: items, query: "  "), [])
        XCTAssertEqual(transcript(["Café latte", "plain", "CAFE"]).matches("cafe"), [0, 2])
    }

    func testTypingWaitsForThePauseAndOnlyTheLastQueryApplies() async {
        let find = ChatFind(debounce: .milliseconds(30))
        find.transcriptChanged(transcript(["alpha", "alphabet", "beta"]))
        for query in ["a", "al", "alp", "alph", "alpha", "alphab"] { find.setQuery(query) }
        // Nothing published while typing: the field keeps its own text.
        XCTAssertEqual(find.appliedQuery, "")
        XCTAssertEqual(find.matches, [])
        await settle(find)
        XCTAssertEqual(find.appliedQuery, "alphab")
        XCTAssertEqual(find.matches, [1])
        XCTAssertEqual(find.currentMatch, 1)
    }

    func testClearingTheQueryClearsAtOnceAndCancelsTheSearch() async {
        let find = ChatFind(debounce: .milliseconds(30))
        find.transcriptChanged(transcript(["alpha", "beta"]))
        find.setQuery("alpha")
        await settle(find)
        XCTAssertEqual(find.matches, [0])
        find.setQuery("beta")
        find.setQuery("")
        XCTAssertFalse(find.isSearching)
        XCTAssertEqual(find.appliedQuery, "")
        XCTAssertEqual(find.matches, [])
        XCTAssertNil(find.currentMatch)
        try? await Task.sleep(for: .milliseconds(60))
        XCTAssertEqual(find.matches, [], "a cancelled search must not land later")
    }

    func testSwitchingChatsDropsTheOldChatsMatches() async {
        let find = ChatFind(debounce: .milliseconds(30))
        find.transcriptChanged(transcript(["needle", "x", "needle"]))
        find.setQuery("needle")
        await settle(find)
        XCTAssertEqual(find.matches, [0, 2])
        find.step()
        XCTAssertEqual(find.currentMatch, 2)

        // Another chat opens while a new query is still waiting.
        find.setQuery("needles")
        find.transcriptChanged(transcript(["y", "needles"]), sessionChanged: true)
        XCTAssertEqual(find.matches, [], "the old chat's ids mean nothing in the new one")
        await settle(find)
        XCTAssertEqual(find.appliedQuery, "needles")
        XCTAssertEqual(find.matches, [1])
        XCTAssertEqual(find.index, 0)
    }

    func testStreamingOutputKeepsTheCurrentMatch() async {
        let find = ChatFind(debounce: .milliseconds(10))
        var chat = transcript(["hit", "miss", "hit"])
        find.transcriptChanged(chat)
        find.setQuery("hit")
        await settle(find)
        find.step()
        XCTAssertEqual(find.currentMatch, 2)
        for _ in 0..<5 {
            chat.appendUser("hit again")
            find.transcriptChanged(chat)
        }
        await settle(find)
        XCTAssertEqual(find.matches, [0, 2, 3, 4, 5, 6, 7])
        XCTAssertEqual(find.currentMatch, 2, "new output must not move the reader off their match")
    }

    func testSteppingWraps() async {
        let find = ChatFind(debounce: .milliseconds(1))
        find.transcriptChanged(transcript(["a", "a", "a"]))
        find.setQuery("a")
        await settle(find)
        find.step(-1)
        XCTAssertEqual(find.index, 2)
        find.step()
        XCTAssertEqual(find.index, 0)
    }

    func testCancelledMatchingStopsEarly() async {
        let items = longTranscript(count: 20_000).items
        let task = Task.detached { ChatFind.matches(in: items, query: "needle") }
        task.cancel()
        let found = await task.value
        XCTAssertTrue(found.isEmpty || found.count == 2_000)
    }

    /// #2166: in a long session a keystroke costs the main actor next to
    /// nothing; the scan runs elsewhere and only the last query is searched.
    func testRapidTypingInALongSessionLeavesTheMainActorFree() async {
        let chat = longTranscript(count: 3_000)

        let syncStart = Date()
        XCTAssertEqual(chat.matches("needle").count, 300)
        let scan = Date().timeIntervalSince(syncStart)

        let find = ChatFind(debounce: .milliseconds(20))
        find.transcriptChanged(chat)
        let typed = ["n", "ne", "nee", "need", "needl", "needle", "needle 1", "needle 12"]
        var worstKeystroke: TimeInterval = 0
        for query in typed {
            let start = Date()
            find.setQuery(query)
            worstKeystroke = max(worstKeystroke, Date().timeIntervalSince(start))
            try? await Task.sleep(for: .milliseconds(5))
        }
        await settle(find)
        XCTAssertEqual(find.appliedQuery, "needle 12")
        // Every tenth message has a needle: 120 and 1200, 1210, …, 1290.
        XCTAssertEqual(find.matches.count, 11)
        print("ChatFind: full scan of 3000 items \(Int(scan * 1000)) ms; worst keystroke on main \(String(format: "%.3f", worstKeystroke * 1000)) ms")
        XCTAssertLessThan(worstKeystroke, 0.005)
    }
}
