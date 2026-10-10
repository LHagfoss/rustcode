import Testing
@testable import RustCodeRemote

@Suite struct PairingScannerLifecycleTests {
    @Test func startsOnlyWhilePresentedAndActive() {
        var scanner = PairingScannerLifecycle()
        scanner.update(visible: false, active: true)
        #expect(!scanner.shouldRun)
        scanner.update(visible: true, active: true)
        #expect(scanner.shouldRun)
        #expect(scanner.needsPreviewRecovery)
    }

    @Test func backgroundingClearsPreviewAndDefersRecoveryUntilReturn() {
        var scanner = PairingScannerLifecycle()
        scanner.update(visible: true, active: true)
        scanner.previewChanged(true)
        #expect(!scanner.needsPreviewRecovery)
        scanner.update(visible: true, active: false)
        #expect(!scanner.shouldRun)
        #expect(!scanner.needsPreviewRecovery)
        scanner.previewChanged(true) // An old queued callback must not mark the paused camera ready.
        scanner.update(visible: true, active: true)
        #expect(scanner.needsPreviewRecovery)
        scanner.previewChanged(true)
        #expect(!scanner.needsPreviewRecovery)
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
        scanner.update(visible: false, active: true)
        #expect(!scanner.shouldRun)
        var reopened = PairingScannerLifecycle()
        reopened.update(visible: true, active: true)
        let nextPayload = reopened.accept("next code")
        #expect(nextPayload)
    }
}
