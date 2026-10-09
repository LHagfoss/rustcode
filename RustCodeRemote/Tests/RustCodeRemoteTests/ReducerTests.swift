import Testing
@testable import RustCodeRemote

@Suite struct SnapshotReducerTests {
    let ref = SessionRef(sessionID: "s1", registrationEpoch: 3)

    @Test func appliesDeltasInSequenceAndBuildsOneItem() {
        var snapshot = SessionSnapshot(session: ref, sequence: 4)
        #expect(snapshot.apply(update(ref, 5, .turnStarted(turnID: "t1"))) == .applied)
        #expect(snapshot.apply(update(ref, 6, .assistantDelta(itemID: "a1", text: "Hel"))) == .applied)
        #expect(snapshot.apply(update(ref, 7, .assistantDelta(itemID: "a1", text: "lo"))) == .applied)

        #expect(snapshot.items == [TranscriptItem(id: "a1", .assistant(AssistantText(text: "Hello", isStreaming: true)))])
        #expect(snapshot.isRunning)
        #expect(snapshot.sequence == 7)
    }

    @Test func ignoresADeltaTheSnapshotAlreadyContains() {
        var snapshot = SessionSnapshot(
            session: ref,
            items: [TranscriptItem(id: "a1", .assistant(AssistantText(text: "Hello", isStreaming: true)))],
            sequence: 7
        )
        #expect(snapshot.apply(update(ref, 7, .assistantDelta(itemID: "a1", text: "lo"))) == .duplicate)
        #expect(snapshot.apply(update(ref, 2, .assistantDelta(itemID: "a1", text: "x"))) == .duplicate)
        #expect(snapshot.items.first?.content == .assistant(AssistantText(text: "Hello", isStreaming: true)))
    }

    @Test func reportsAGapWithoutChangingState() {
        var snapshot = SessionSnapshot(session: ref, sequence: 7)
        let before = snapshot
        #expect(snapshot.apply(update(ref, 9, .assistantDelta(itemID: "a1", text: "late"))) == .gap(expected: 8, received: 9))
        #expect(snapshot == before)
    }

    @Test func ignoresUpdatesForAnotherRegistration() {
        var snapshot = SessionSnapshot(session: ref, sequence: 7)
        let other = SessionRef(sessionID: "s1", registrationEpoch: 4)
        #expect(snapshot.apply(update(other, 8, .turnStarted(turnID: "t"))) == .foreign)
        #expect(snapshot.activeTurnID == nil)
    }

    @Test func updatesAToolRowInPlace() {
        var snapshot = SessionSnapshot(session: ref)
        let running = ToolCall(name: "search", summary: "rg foo", status: .running)
        var done = running
        done.status = .succeeded
        done.output = "2 matches"
        snapshot.apply(update(ref, 1, .itemAppended(TranscriptItem(id: "tool", .tool(running)))))
        snapshot.apply(update(ref, 2, .itemAppended(TranscriptItem(id: "u", .user("next")))))
        snapshot.apply(update(ref, 3, .itemUpdated(TranscriptItem(id: "tool", .tool(done)))))

        #expect(snapshot.items.map(\.id) == ["tool", "u"])
        #expect(snapshot.items[0].content == .tool(done))
    }

    @Test func finishingATurnStopsStreamingAndOnlyClearsTheMatchingTurn() {
        var snapshot = SessionSnapshot(session: ref)
        snapshot.apply(update(ref, 1, .turnStarted(turnID: "t2")))
        snapshot.apply(update(ref, 2, .assistantDelta(itemID: "a", text: "partial")))
        snapshot.apply(update(ref, 3, .turnFinished(turnID: "t1", outcome: .cancelled)))
        #expect(snapshot.activeTurnID == "t2")
        #expect(snapshot.items[0].content == .assistant(AssistantText(text: "partial", isStreaming: false)))

        snapshot.apply(update(ref, 4, .turnFinished(turnID: "t2", outcome: .completed)))
        #expect(!snapshot.isRunning)
    }

    @Test func tracksPendingAttention() {
        var snapshot = SessionSnapshot(session: ref)
        let question = PendingQuestion(id: "q", prompt: "Which?", options: [.init(id: "a", label: "A")])
        let approval = PendingApproval(batchID: "b", items: [.init(id: "i", toolName: "run", title: "Run", detail: "ls")])
        snapshot.apply(update(ref, 1, .questionChanged(question)))
        snapshot.apply(update(ref, 2, .approvalChanged(approval)))
        #expect(snapshot.pendingQuestion == question)
        #expect(snapshot.pendingApproval == approval)
        snapshot.apply(update(ref, 3, .questionChanged(nil)))
        #expect(snapshot.pendingQuestion == nil)
    }

    @Test func prependsHistoryWithoutDuplicates() {
        var snapshot = SessionSnapshot(
            session: ref,
            items: [TranscriptItem(id: "3", .user("c"))],
            historyCursor: HistoryCursor("c1")
        )
        snapshot.prepend(HistoryPage(
            items: [TranscriptItem(id: "1", .user("a")), TranscriptItem(id: "2", .user("b")), TranscriptItem(id: "3", .user("c"))],
            nextCursor: nil
        ))
        #expect(snapshot.items.map(\.id) == ["1", "2", "3"])
        #expect(snapshot.historyCursor == nil)
    }

    @Test func approvalWithAPreviewOnlyItemCannotBeResolved() {
        let approval = PendingApproval(batchID: "b", items: [
            .init(id: "1", toolName: "run", title: "Run", detail: "full"),
            .init(id: "2", toolName: "write", title: "Write", detail: "first 4 KiB…", isDetailComplete: false),
        ])
        #expect(!approval.canBeResolved)
    }
}

@Suite struct ReceiptLedgerTests {
    let ref = SessionRef(sessionID: "s1", registrationEpoch: 1)

    private func ledger(with id: RequestID) -> ReceiptLedger {
        var ledger = ReceiptLedger()
        ledger.begin(TrackedRequest(id: id, session: ref, kind: .submit("hello")))
        return ledger
    }

    @Test func appliedRequestsLeaveTheLedger() {
        let id = RequestID("r1")
        var ledger = ledger(with: id)
        ledger.record(RequestReceipt(requestID: id, state: .received))
        #expect(ledger.requests.first?.phase == .settled(.received))
        #expect(ledger.uncertain.map(\.id) == [id])
        ledger.record(RequestReceipt(requestID: id, state: .applied))
        #expect(ledger.requests.isEmpty)
    }

    @Test func aLostConnectionMakesInFlightRequestsUncertain() {
        let id = RequestID("r1")
        var ledger = ledger(with: id)
        ledger.connectionLost()
        #expect(ledger.requests.first?.phase == .settled(.unknown))
        #expect(ledger.requests.first?.isUncertain == true)
        #expect(ledger.requests.first?.text == "hello")
    }

    @Test func aRejectionIsNotOverwrittenByALateNonFinalState() {
        let id = RequestID("r1")
        var ledger = ledger(with: id)
        ledger.record(RequestReceipt(requestID: id, state: .rejected(.busy)))
        ledger.record(RequestReceipt(requestID: id, state: .received))
        ledger.markUncertain(id)
        ledger.connectionLost()
        #expect(ledger.requests.first?.phase == .settled(.rejected(.busy)))
        #expect(ledger.uncertain.isEmpty)
    }

    @Test func unknownResolvesOnceTheHostAnswers() {
        let id = RequestID("r1")
        var ledger = ledger(with: id)
        ledger.markUncertain(id)
        ledger.record(RequestReceipt(requestID: id, state: .applied))
        #expect(ledger.requests.isEmpty)
    }

    @Test func receiptsForUnknownRequestsAreIgnored() {
        var ledger = ledger(with: RequestID("r1"))
        ledger.record(RequestReceipt(requestID: RequestID("other"), state: .applied))
        #expect(ledger.requests.count == 1)
        #expect(ledger.requests(for: SessionRef(sessionID: "s2", registrationEpoch: 1)).isEmpty)
    }
}

@Suite struct AttentionBadgesTests {
    @Test func marksACompletionOnlyForSessionsNotOnScreen() {
        var badges = AttentionBadges()
        let before = [makeSession("a", activity: .running), makeSession("b", activity: .running)]
        let after = [makeSession("a"), makeSession("b")]
        badges.observe(previous: before, current: after, selected: "a")
        #expect(badges.badge(for: after[0]) == nil)
        #expect(badges.badge(for: after[1]) == .completed)
        #expect(badges.count(in: after) == 1)

        badges.markSeen("b")
        #expect(badges.badge(for: after[1]) == nil)
    }

    @Test func approvalOutranksQuestionOutranksCompletion() {
        var badges = AttentionBadges()
        badges.observe(previous: [makeSession("a", activity: .running)], current: [makeSession("a")], selected: nil)
        #expect(badges.badge(for: makeSession("a", attention: .init(pendingQuestions: 1))) == .question)
        #expect(badges.badge(for: makeSession("a", attention: .init(pendingQuestions: 1, pendingApprovals: 1))) == .approval)
        #expect(badges.badge(for: makeSession("a")) == .completed)
    }

    @Test func forgetsSessionsThatAreNoLongerShared() {
        var badges = AttentionBadges()
        badges.observe(previous: [makeSession("a", activity: .running)], current: [makeSession("a")], selected: nil)
        badges.observe(previous: [makeSession("a")], current: [], selected: nil)
        #expect(badges.unseenCompletions.isEmpty)
    }

    @Test func waitingForTheUserIsNotACompletion() {
        var badges = AttentionBadges()
        badges.observe(
            previous: [makeSession("a", activity: .running)],
            current: [makeSession("a", activity: .waitingForUser)],
            selected: nil
        )
        #expect(badges.unseenCompletions.isEmpty)
    }
}
