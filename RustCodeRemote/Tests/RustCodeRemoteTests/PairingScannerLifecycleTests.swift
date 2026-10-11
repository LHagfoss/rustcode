import Testing
@testable import RustCodeRemote

@Suite struct PairingScannerLifecycleTests {
    @Test func startsOnlyWhilePresentedAndActive() {
        var scanner = PairingScannerLifecycle()
        scanner.update(visible: false, active: true)
        #expect(!scanner.shouldRun)
        scanner.update(visible: true, active: true)
        #expect(scanner.shouldRun)
        #expect(scanner.isStarting)
        #expect(scanner.timeout == PairingScannerLifecycle.startTimeout)
        #expect(!scanner.needsPreviewRecovery, "Configuration is not a stalled preview")
        scanner.sessionStarted(run: scanner.run)
        #expect(!scanner.isStarting)
        #expect(scanner.timeout == PairingScannerLifecycle.previewTimeout)
        #expect(scanner.needsPreviewRecovery)
    }

    @Test(arguments: [true, false])
    func visibleAndActiveMayArriveInEitherOrder(visibleFirst: Bool) {
        var scanner = PairingScannerLifecycle()
        scanner.update(visible: visibleFirst, active: !visibleFirst)
        #expect(!scanner.shouldRun)
        #expect(scanner.run == 0, "Nothing was asked to start yet")
        scanner.update(visible: true, active: true)
        #expect(scanner.shouldRun)
        #expect(scanner.run == 1)
        // Repeating the same state is not a new capture request.
        scanner.update(visible: true, active: true)
        #expect(scanner.run == 1)
        scanner.sessionStarted(run: 1)
        scanner.previewChanged(true)
        #expect(scanner.hasPreview)
    }

    @Test func pausingKeepsProgressAndIgnoresCallbacksFromTheOldRun() {
        var scanner = PairingScannerLifecycle()
        scanner.update(visible: true, active: true)
        let firstRun = scanner.run
        scanner.sessionStarted(run: firstRun)
        scanner.previewChanged(true)
        #expect(scanner.hasPreview)

        scanner.update(visible: true, active: false)
        #expect(!scanner.shouldRun)
        #expect(!scanner.hasPreview, "A paused camera shows no spinner and no video")
        #expect(!scanner.needsPreviewRecovery)

        scanner.update(visible: true, active: true)
        #expect(scanner.run == firstRun + 1)
        #expect(scanner.hasPreview, "The layer never reported that video stopped")
        scanner.sessionStarted(run: firstRun) // A start callback queued before the pause.
        #expect(scanner.isStarting, "Only the current run's start counts")
        scanner.sessionStarted(run: scanner.run)
        #expect(!scanner.isStarting)
        #expect(!scanner.needsPreviewRecovery)
    }

    @Test func previewFollowsTheLayerWhateverOrderItsChangesArriveIn() {
        var scanner = PairingScannerLifecycle()
        // The layer can report before the scene is active or capture has started.
        scanner.previewChanged(true)
        #expect(!scanner.hasPreview)
        scanner.update(visible: true, active: true)
        #expect(scanner.hasPreview)
        scanner.sessionStarted(run: scanner.run)
        #expect(!scanner.needsPreviewRecovery)

        // Stop then start, observed late and after the resume.
        scanner.update(visible: true, active: false)
        scanner.update(visible: true, active: true)
        scanner.previewChanged(false)
        #expect(!scanner.hasPreview)
        scanner.sessionStarted(run: scanner.run)
        #expect(scanner.needsPreviewRecovery)
        scanner.previewChanged(true)
        scanner.previewChanged(true)
        #expect(scanner.hasPreview)
        #expect(!scanner.needsPreviewRecovery)
    }

    @Test func aStalledPreviewRestartsOnceThenFails() {
        var scanner = PairingScannerLifecycle()
        #expect(scanner.previewTimedOut() == .ignore)
        scanner.update(visible: true, active: true)
        #expect(scanner.previewTimedOut() == .fail, "Capture that never started cannot be restarted")

        scanner = PairingScannerLifecycle()
        scanner.update(visible: true, active: true)
        let firstRun = scanner.run
        scanner.sessionStarted(run: firstRun)
        #expect(scanner.previewTimedOut() == .restart)
        #expect(scanner.run == firstRun + 1)
        #expect(scanner.isStarting, "The restart is a new capture request")
        scanner.sessionStarted(run: scanner.run)
        #expect(scanner.previewTimedOut() == .fail)

        // Video arriving after the restart needs no further action.
        scanner.previewChanged(true)
        #expect(scanner.previewTimedOut() == .ignore)

        // Returning from the background earns a fresh restart.
        scanner.previewChanged(false)
        scanner.update(visible: true, active: false)
        scanner.update(visible: true, active: true)
        scanner.sessionStarted(run: scanner.run)
        #expect(scanner.previewTimedOut() == .restart)
    }

    @Test func dismissingOrDeliveringStopsCaptureAndRejectsDuplicateCodes() {
        var scanner = PairingScannerLifecycle()
        scanner.update(visible: false, active: true)
        let hiddenPayload = scanner.accept("payload")
        #expect(!hiddenPayload)
        scanner.update(visible: true, active: true)
        let emptyPayload = scanner.accept("")
        #expect(!emptyPayload)
        let firstPayload = scanner.accept("payload")
        #expect(firstPayload)
        #expect(!scanner.shouldRun)
        let duplicatePayload = scanner.accept("duplicate")
        #expect(!duplicatePayload)
        #expect(!scanner.needsPreviewRecovery)
        #expect(scanner.previewTimedOut() == .ignore)
        scanner.update(visible: false, active: true)
        #expect(!scanner.shouldRun)
        var reopened = PairingScannerLifecycle()
        reopened.update(visible: true, active: true)
        let nextPayload = reopened.accept("next code")
        #expect(nextPayload)
    }

    @Test func timeoutsNameWhatThePersonCanDo() {
        #expect(PairingScannerFailure.timedOut(authorized: false, sessionRunning: true) == .accessRevoked)
        #expect(PairingScannerFailure.timedOut(authorized: true, sessionRunning: false) == .couldNotStart)
        #expect(PairingScannerFailure.timedOut(authorized: true, sessionRunning: true) == .noVideo)
        #expect(PairingScannerFailure.accessRevoked.offersSettings)
        #expect(!PairingScannerFailure.inUseByAnotherApp.offersSettings)
        #expect(PairingScannerFailure.accessRevoked.message.contains("Settings"))
        #expect(PairingScannerFailure.inUseByAnotherApp.message.contains("Another app"))
        #expect(PairingScannerFailure.noVideo.message.contains("Try Again"))
    }
}
