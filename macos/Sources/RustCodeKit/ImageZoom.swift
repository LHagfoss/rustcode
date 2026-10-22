import CoreGraphics

/// How large the image viewer shows an image: fitted to the window until the
/// reader zooms, then at a scale of their own, within limits.
public struct ImageZoom: Equatable, Sendable {
    public static let range: ClosedRange<Double> = 0.05...16
    /// Each ⌘+ or ⌘− step.
    public static let step = 1.25

    /// Nil while the image fits the window.
    public private(set) var scale: Double?

    public init(scale: Double? = nil) {
        self.scale = scale.map(Self.clamped)
    }

    /// The scale that fits `image` inside `bounds`, never enlarging it.
    public static func fit(_ image: CGSize, in bounds: CGSize) -> Double {
        guard image.width > 0, image.height > 0, bounds.width > 0, bounds.height > 0 else { return 1 }
        return min(1, bounds.width / image.width, bounds.height / image.height)
    }

    /// The scale shown now.
    public func current(fitting image: CGSize, in bounds: CGSize) -> Double {
        scale ?? Self.fit(image, in: bounds)
    }

    public var isFitted: Bool { scale == nil }

    public mutating func zoomIn(fitting image: CGSize, in bounds: CGSize) {
        scale = Self.clamped(current(fitting: image, in: bounds) * Self.step)
    }

    public mutating func zoomOut(fitting image: CGSize, in bounds: CGSize) {
        scale = Self.clamped(current(fitting: image, in: bounds) / Self.step)
    }

    /// A pinch: `magnification` times the scale it started from.
    public mutating func magnify(from start: Double, by magnification: Double) {
        scale = Self.clamped(start * magnification)
    }

    public mutating func fit() { scale = nil }

    /// Double-click: actual size when fitted, fitted otherwise.
    public mutating func toggleActualSize() {
        scale = scale == nil ? 1 : nil
    }

    private static func clamped(_ value: Double) -> Double {
        min(max(value, range.lowerBound), range.upperBound)
    }
}
