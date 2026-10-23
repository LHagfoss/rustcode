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

    /// The share of the window's width a click zooms the image to.
    public static let zoomedWidthShare = 0.5

    /// The scale a click zooms to: the image half as wide as the window, its
    /// aspect kept and the rest reached by scrolling. That is always larger
    /// than the fitted scale; when the fitted image is already that wide, the
    /// click goes to twice the fitted size or the image's actual pixels,
    /// whichever is larger.
    public static func zoomed(_ image: CGSize, in bounds: CGSize, window: CGSize) -> Double {
        let fitted = fit(image, in: bounds)
        guard image.width > 0, window.width > 0 else { return clamped(fitted * 2) }
        let half = window.width * zoomedWidthShare / image.width
        // A few percent larger reads as no zoom at all.
        let target = half > fitted * 1.1 ? half : max(1, fitted * 2)
        return clamped(target)
    }

    /// A click on the image: zoomed when fitted, fitted otherwise (also after
    /// a pinch or ⌘+, so a click always comes back to the whole image).
    public mutating func toggleZoom(fitting image: CGSize, in bounds: CGSize, window: CGSize) {
        scale = scale == nil ? Self.zoomed(image, in: bounds, window: window) : nil
    }

    private static func clamped(_ value: Double) -> Double {
        min(max(value, range.lowerBound), range.upperBound)
    }
}
