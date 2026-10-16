import Testing
@testable import RustCodeRemote

@Suite struct ChatFeedbackTests {
    private let ref = SessionRef(sessionID: "chat", registrationEpoch: 1)

    @Test func givesFeedbackForLiveTurnStartAndCompletion() {
        let idle = SessionSnapshot(session: ref, sequence: 3)
        let start = SessionUpdate(session: ref, sequence: 4, event: .turnStarted(turnID: "t"))
        #expect(ChatFeedback.kind(for: start, previous: idle) == .started)
        let running = SessionSnapshot(session: ref, activeTurnID: "t", sequence: 4)
        let finish = SessionUpdate(session: ref, sequence: 5, event: .turnFinished(turnID: "t", outcome: .completed))
        #expect(ChatFeedback.kind(for: finish, previous: running) == .completed)
    }

    @Test func distinguishesCancellationFailureAndUnknownOutcome() {
        let running = SessionSnapshot(session: ref, activeTurnID: "t", sequence: 4)
        for (outcome, expected) in [(TurnOutcome.cancelled, ChatFeedback.Kind.cancelled), (.failed, .failed)] {
            let update = SessionUpdate(session: ref, sequence: 5, event: .turnFinished(turnID: "t", outcome: outcome))
            #expect(ChatFeedback.kind(for: update, previous: running) == expected)
        }
        #expect(ChatFeedback.kind(for: SessionUpdate(session: ref, sequence: 5,
            event: .turnFinished(turnID: "t", outcome: .unknown("future"))), previous: running) == nil)
    }

    @Test func ignoresStreamingDuplicatesGapsForeignSessionsAndReplacementSnapshots() {
        let running = SessionSnapshot(session: ref, activeTurnID: "t", sequence: 4)
        let event = SessionEvent.turnFinished(turnID: "t", outcome: .completed)
        #expect(ChatFeedback.kind(for: SessionUpdate(session: ref, sequence: 5,
            event: .responseDelta("more words")), previous: running) == nil)
        #expect(ChatFeedback.kind(for: SessionUpdate(session: ref, sequence: 4, event: event), previous: running) == nil)
        #expect(ChatFeedback.kind(for: SessionUpdate(session: ref, sequence: 6, event: event), previous: running) == nil)
        #expect(ChatFeedback.kind(for: SessionUpdate(session: SessionRef(sessionID: "other", registrationEpoch: 1), sequence: 5,
            event: event), previous: running) == nil)
        #expect(ChatFeedback.kind(for: SessionUpdate(session: ref, sequence: 5,
            event: .replaced(SessionSnapshot(session: ref))), previous: running) == nil)
        #expect(ChatFeedback.kind(for: SessionUpdate(session: ref, sequence: 5,
            event: .turnFinished(turnID: "old", outcome: .completed)), previous: running) == nil)
    }

    @Test func doesNotCelebrateAnUnobservedTurn() {
        let idle = SessionSnapshot(session: ref, sequence: 4)
        let update = SessionUpdate(session: ref, sequence: 5, event: .turnFinished(turnID: "t", outcome: .completed))
        #expect(ChatFeedback.kind(for: update, previous: idle) == nil)
    }

    @Test func alertsForNewAttentionButNotRepeatedOrResolvedRequests() {
        let question = PendingQuestion(id: "q", prompt: "Choose", options: [])
        let approval = PendingApproval(batchID: "b", items: [])
        let idle = SessionSnapshot(session: ref, sequence: 4)
        let waiting = SessionSnapshot(session: ref, pendingQuestion: question, pendingApproval: approval, sequence: 4)
        for event in [SessionEvent.questionChanged(question), .approvalChanged(approval)] {
            let update = SessionUpdate(session: ref, sequence: 5, event: event)
            #expect(ChatFeedback.kind(for: update, previous: idle) == .attention)
            #expect(ChatFeedback.kind(for: update, previous: waiting) == nil)
        }
        for event in [SessionEvent.questionChanged(nil), .approvalChanged(nil), .questionResolved(questionID: "q"), .approvalResolved(batchID: "b")] {
            #expect(ChatFeedback.kind(for: SessionUpdate(session: ref, sequence: 5, event: event), previous: waiting) == nil)
        }
    }

    @Test func batchesProduceOneFinalMeaningfulFeedback() {
        let running = SessionSnapshot(session: ref, activeTurnID: "t", sequence: 4)
        let update = SessionUpdate(session: ref, sequence: 5, changes: [
            .responseDelta("Done"), .assistantCompleted(itemID: "a"), .turnFinished(turnID: nil, outcome: .completed)
        ])
        #expect(ChatFeedback.kind(for: update, previous: running) == .completed)
    }
}
