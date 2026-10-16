/// Camera presentation state, independent of capture hardware. A session starting
/// successfully is not evidence that its preview is rendering frames.
///
/// Progress is compared, never reset: each capture request has a `run` number, a
/// start callback counts only for the run that asked for it, and preview readiness
/// mirrors what the layer last reported. A pause therefore cannot be undone or
/// corrupted by a callback that was queued before it.
public struct PairingScannerLifecycle: Sendable {
    /// Bound on configuring and starting capture. `startRunning()` blocks while the
    /// camera powers up, which is slowest on the first use after a reboot.
    public static let startTimeout: Duration = .seconds(20)
    /// Bound on the first frame once capture runs. It is short because the first
    /// expiry only restarts capture; the second one reports the failure.
    /// Neither bound has been measured on a device yet.
    public static let previewTimeout: Duration = .seconds(5)

    public private(set) var isVisible = false
    /// The app is in the foreground. A merely inactive scene (a permission prompt,
    /// Control Center) keeps the camera running.
    public private(set) var isActive = false
    /// Identifies the current capture request. Start callbacks carry it back.
    public private(set) var run = 0
    private var startedRun: Int?
    private var layerIsPreviewing = false
    private var hasDelivered = false
    private var hasRestarted = false

    public init() {}

    public enum TimeoutAction: Sendable { case ignore, restart, fail }

    public var shouldRun: Bool { isVisible && isActive && !hasDelivered }
    public var hasStarted: Bool { shouldRun && startedRun == run }
    public var isStarting: Bool { shouldRun && !hasStarted }
    public var hasPreview: Bool { shouldRun && layerIsPreviewing }
    public var needsPreviewRecovery: Bool { hasStarted && !hasPreview }
    public var timeout: Duration { isStarting ? Self.startTimeout : Self.previewTimeout }

    public mutating func update(visible: Bool, active: Bool) {
        let wasRunning = shouldRun
        isVisible = visible
        isActive = active
        guard shouldRun, !wasRunning else { return }
        run += 1
        hasRestarted = false
    }

    /// A preview timeout must not count time spent configuring/starting capture.
    public mutating func sessionStarted(run: Int) {
        guard shouldRun, run == self.run else { return }
        startedRun = run
    }

    /// Record the layer's current state, whatever order its changes arrive in.
    public mutating func previewChanged(_ ready: Bool) {
        layerIsPreviewing = ready
    }

    /// The watchdog expired without video. A running session gets one restart.
    public mutating func previewTimedOut() -> TimeoutAction {
        guard shouldRun, !hasPreview else { return .ignore }
        guard hasStarted, !hasRestarted else { return .fail }
        hasRestarted = true
        run += 1
        return .restart
    }

    public mutating func accept(_ payload: String) -> Bool {
        guard shouldRun, !payload.isEmpty else { return false }
        hasDelivered = true
        return true
    }
}

/// Why the pairing camera stopped, worded so the person knows what to do next.
public enum PairingScannerFailure: Equatable, Sendable {
    case accessRevoked
    case inUseByAnotherApp
    case unavailableWhileMultitasking
    case systemPressure
    case interrupted
    case couldNotConfigure
    case couldNotStart
    case noVideo

    /// Explain a watchdog expiry from what is known about the camera at that moment.
    public static func timedOut(authorized: Bool, sessionRunning: Bool) -> Self {
        if !authorized { return .accessRevoked }
        return sessionRunning ? .noVideo : .couldNotStart
    }

    public var offersSettings: Bool { self == .accessRevoked || self == .couldNotConfigure }

    public var message: String {
        switch self {
        case .accessRevoked:
            "Camera access is turned off for RustCode. Allow it in Settings, or enter the code manually."
        case .inUseByAnotherApp:
            "Another app or a call is using the camera. Close it, then tap Try Again."
        case .unavailableWhileMultitasking:
            "The camera is unavailable in Split View and Slide Over. Use RustCode full screen, then tap Try Again."
        case .systemPressure:
            "The camera is paused because this device is too warm or busy. Wait a moment, then tap Try Again."
        case .interrupted:
            "The camera was interrupted. Tap Try Again when it is available."
        case .couldNotConfigure:
            "The camera could not be set up. Check camera access in Settings, then tap Try Again."
        case .couldNotStart:
            "The camera did not start. Close other apps using the camera, then tap Try Again."
        case .noVideo:
            "The camera started but showed no video. Tap Try Again, or enter the code manually."
        }
    }
}
