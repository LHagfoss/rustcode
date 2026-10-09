import Testing
@testable import RustCodeRemote

@Suite struct BackoffPolicyTests {
    let policy = BackoffPolicy(initialDelay: .seconds(1), multiplier: 2, maximumDelay: .seconds(30), jitter: 0.2, maximumAttempts: 4)

    @Test func doublesWithoutJitterAtTheMidpoint() {
        let delays = (1...5).map { policy.delay(forAttempt: $0, randomUnit: 0.5) }
        #expect(delays == [.seconds(1), .seconds(2), .seconds(4), .seconds(8), .seconds(16)])
    }

    @Test func jitterStaysWithinItsFraction() {
        #expect(policy.delay(forAttempt: 3, randomUnit: 0) == .seconds(3.2))
        #expect(policy.delay(forAttempt: 3, randomUnit: 1) == .seconds(4.8))
        // Out-of-range random input is clamped rather than trusted.
        #expect(policy.delay(forAttempt: 3, randomUnit: 7) == .seconds(4.8))
    }

    @Test func neverExceedsTheCeilingEvenWithJitter() {
        for attempt in [6, 7, 20, 500] {
            for unit in [0.0, 0.5, 1.0] {
                #expect(policy.delay(forAttempt: attempt, randomUnit: unit) <= .seconds(30))
            }
        }
        #expect(policy.delay(forAttempt: 500, randomUnit: 1) == .seconds(30))
    }

    @Test func retriesUpToTheLimitThenGivesUp() {
        var state = ReconnectState(policy: policy)
        let decisions = (1...5).map { _ in state.registerFailure(unauthorized: false, randomUnit: 0.5) }
        #expect(decisions == [
            .retry(attempt: 1, after: .seconds(1)),
            .retry(attempt: 2, after: .seconds(2)),
            .retry(attempt: 3, after: .seconds(4)),
            .retry(attempt: 4, after: .seconds(8)),
            .giveUp,
        ])
    }

    @Test func successAndManualRetryRestartFromTheShortestDelay() {
        var state = ReconnectState(policy: policy)
        _ = state.registerFailure(unauthorized: false, randomUnit: 0.5)
        _ = state.registerFailure(unauthorized: false, randomUnit: 0.5)
        state.registerSuccess()
        #expect(state.registerFailure(unauthorized: false, randomUnit: 0.5) == .retry(attempt: 1, after: .seconds(1)))

        for _ in 0..<10 { _ = state.registerFailure(unauthorized: false, randomUnit: 0.5) }
        state.manualRetry()
        #expect(state.registerFailure(unauthorized: false, randomUnit: 0.5) == .retry(attempt: 1, after: .seconds(1)))
    }

    @Test func unauthorizedStopsForGood() {
        var state = ReconnectState(policy: policy)
        #expect(state.registerFailure(unauthorized: true, randomUnit: 0.5) == .stop)
        state.manualRetry()
        state.registerSuccess()
        #expect(state.registerFailure(unauthorized: false, randomUnit: 0.5) == .stop)
        #expect(state.isStopped)
    }
}

@Suite struct HostAddressTests {
    @Test func parsesHostAndPort() throws {
        #expect(try HostAddress.parse(" 192.168.1.20:7878 ") == HostAddress(host: "192.168.1.20", port: 7878))
        #expect(try HostAddress.parse("ws://mac.local:9000/remote") == HostAddress(host: "mac.local", port: 9000))
        #expect(try HostAddress.parse("[fd00::1]:7878") == HostAddress(host: "fd00::1", port: 7878))
        #expect(HostAddress(host: "fd00::1", port: 7878).description == "[fd00::1]:7878")
    }

    @Test func rejectsIncompleteAddresses() {
        #expect(throws: HostAddress.ParseError.empty) { try HostAddress.parse("  ") }
        #expect(throws: HostAddress.ParseError.missingPort) { try HostAddress.parse("192.168.1.20") }
        #expect(throws: HostAddress.ParseError.missingPort) { try HostAddress.parse("192.168.1.20:") }
        #expect(throws: HostAddress.ParseError.invalidPort) { try HostAddress.parse("192.168.1.20:99999") }
        #expect(throws: HostAddress.ParseError.invalidHost) { try HostAddress.parse("fd00::1:7878") }
    }

    @Test func classifiesNetworks() {
        func network(_ host: String) -> HostAddress.Network { HostAddress(host: host, port: 1).network }
        #expect(network("127.0.0.1") == .loopback)
        #expect(network("localhost") == .loopback)
        #expect(network("10.0.0.4") == .privateLAN)
        #expect(network("172.20.1.1") == .privateLAN)
        #expect(network("172.32.1.1") == .other)
        #expect(network("192.168.1.20") == .privateLAN)
        #expect(network("169.254.3.4") == .linkLocal)
        #expect(network("100.64.0.1") == .overlay)
        #expect(network("100.127.255.254") == .overlay)
        #expect(network("100.128.0.1") == .other)
        #expect(network("100.63.0.1") == .other)
        #expect(network("studio.local") == .localName)
        #expect(network("studio") == .localName)
        #expect(network("example.com") == .other)
        #expect(network("fe80::1") == .linkLocal)
    }

    @Test func onlyTheOverlayAndLoopbackCountAsProtected() {
        #expect(HostAddress(host: "100.90.1.2", port: 1).protection == .tunnelEncrypted)
        #expect(HostAddress(host: "192.168.1.2", port: 1).protection == .unencryptedLAN)
        #expect(!HostAddress(host: "192.168.1.2", port: 1).protection.isEncrypted)
        #expect(!HostAddress(host: "example.com", port: 1).protection.isEncrypted)
    }
}

@Suite struct MarkdownBlockTests {
    @Test func tablesStopAtFollowingMarkdownBlocksWithPipes() {
        #expect(MarkdownBlock.parse("a | b\n--- | ---\n1 | 2\n# A | B | C\n- x | y\n> quote | text") == [
            .table(headers: ["a", "b"], rows: [["1", "2"]]),
            .heading(level: 1, text: "A | B | C"),
            .listItem(marker: "•", text: "x | y"), .quote("quote | text"),
        ])
    }

    @Test func windowsLineEndingsDoNotAddEmptyLinesOrBreakTables() {
        #expect(MarkdownBlock.parse("```swift\r\nlet a = 1\r\nlet b = 2\r\n```") == [
            .code(language: "swift", text: "let a = 1\nlet b = 2"),
        ])
        #expect(MarkdownBlock.parse("a | b\r\n--- | ---\r\n1 | 2") == [
            .table(headers: ["a", "b"], rows: [["1", "2"]]),
        ])
    }

    @Test func supportsTildeAndLongFencesWithoutClosingOnCodeContent() {
        #expect(MarkdownBlock.parse("~~~~swift\nlet x = 1\n~~~\n~~~~") == [
            .code(language: "swift", text: "let x = 1\n~~~"),
        ])
        #expect(MarkdownBlock.parse("````md\n```swift\nlet x = 1\n```\n````") == [
            .code(language: "md", text: "```swift\nlet x = 1\n```"),
        ])
        #expect(MarkdownBlock.parse("```\n```not a closing fence\n```") == [
            .code(language: nil, text: "```not a closing fence"),
        ])
    }

    @Test func parsesTablesWithEscapedAndInlineCodePipes() {
        #expect(MarkdownBlock.parse("| Name | Value |\n| --- | :---: |\n| a\\|b | `x|y` |\nAfter") == [
            .table(headers: ["Name", "Value"], rows: [["a|b", "`x|y`"]]), .paragraph("After"),
        ])
        #expect(MarkdownBlock.parse("a | b\n--- | ---\n1 | 2") == [
            .table(headers: ["a", "b"], rows: [["1", "2"]]),
        ])
        #expect(MarkdownBlock.parse("a | b\n--- | nope") == [.paragraph("a | b\n--- | nope")])
    }

    @Test func rendersTaskListsAndRules() {
        #expect(MarkdownBlock.parse("- [ ] pending\n- [x] done\n---\n***\n___") == [
            .listItem(marker: "☐", text: "pending"), .listItem(marker: "☑", text: "done"),
            .rule, .rule, .rule,
        ])
    }

    @Test func splitsBlocks() {
        let text = """
            # Title

            First line
            second line

            - one
            2. two
            > quoted

            ```swift
            let x = 1

            print(x)
            ```
            after
            """
        #expect(MarkdownBlock.parse(text) == [
            .heading(level: 1, text: "Title"),
            .paragraph("First line\nsecond line"),
            .listItem(marker: "•", text: "one"),
            .listItem(marker: "2.", text: "two"),
            .quote("quoted"),
            .code(language: "swift", text: "let x = 1\n\nprint(x)"),
            .paragraph("after"),
        ])
    }

    @Test func anUnterminatedFenceIsCodeSoFar() {
        #expect(MarkdownBlock.parse("Look:\n```\nlet x") == [.paragraph("Look:"), .code(language: nil, text: "let x")])
    }

    @Test func hashWithoutSpaceIsNotAHeading() {
        #expect(MarkdownBlock.parse("#hashtag") == [.paragraph("#hashtag")])
    }

    @Test func demoChunksReassembleToTheOriginalText() {
        let text = "One two  three\nfour five."
        #expect(DemoRemoteClient.chunks(of: text).joined() == text)
    }
}
