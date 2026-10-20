/// Numbers overlapping refreshes so only the newest may publish its result.
/// A slow refresh started before a faster, later one would otherwise land last
/// and overwrite it with stale state (#2163).
public struct LatestRequest: Sendable {
    public private(set) var generation = 0

    public init() {}

    /// Starts a request and returns its number.
    public mutating func begin() -> Int {
        generation += 1
        return generation
    }

    /// Whether `request` is still the newest one started.
    public func isCurrent(_ request: Int) -> Bool {
        request == generation
    }
}
