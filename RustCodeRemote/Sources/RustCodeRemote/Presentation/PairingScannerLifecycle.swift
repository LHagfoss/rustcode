/// Camera presentation state, independent of capture hardware. A session starting
/// successfully is not evidence that its preview is rendering frames.
public struct PairingScannerLifecycle: Sendable {
    public private(set) var isVisible = false
    public private(set) var isActive = false
    public private(set) var hasPreview = false
    private var hasDelivered = false

    public init() {}

    public var shouldRun: Bool { isVisible && isActive && !hasDelivered }
    public var needsPreviewRecovery: Bool { shouldRun && !hasPreview }

    public mutating func update(visible: Bool, active: Bool) {
        isVisible = visible
        isActive = active
        if !shouldRun { hasPreview = false }
    }

    public mutating func previewChanged(_ ready: Bool) {
        hasPreview = shouldRun && ready
    }

    public mutating func accept(_ payload: String) -> Bool {
        guard shouldRun, !payload.isEmpty else { return false }
        hasDelivered = true
        return true
    }
}
