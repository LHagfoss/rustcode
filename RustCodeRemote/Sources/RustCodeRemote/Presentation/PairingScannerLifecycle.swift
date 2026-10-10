/// Camera presentation state, independent of capture hardware. A session starting
/// successfully is not evidence that its preview is rendering frames.
public struct PairingScannerLifecycle: Sendable {
    public private(set) var isVisible = false
    public private(set) var isActive = false
    public private(set) var hasPreview = false
    private var hasStarted = false
    private var hasDelivered = false

    public init() {}

    public var shouldRun: Bool { isVisible && isActive && !hasDelivered }
    public var isStarting: Bool { shouldRun && !hasStarted }
    public var needsPreviewRecovery: Bool { shouldRun && hasStarted && !hasPreview }

    public mutating func update(visible: Bool, active: Bool) {
        isVisible = visible
        isActive = active
        if !shouldRun {
            hasPreview = false
            hasStarted = false
        }
    }

    /// A preview timeout must not count time spent configuring/starting capture.
    public mutating func sessionStarted() { hasStarted = shouldRun }

    public mutating func previewChanged(_ ready: Bool) {
        hasPreview = shouldRun && ready
    }

    public mutating func accept(_ payload: String) -> Bool {
        guard shouldRun, !payload.isEmpty else { return false }
        hasDelivered = true
        return true
    }
}
