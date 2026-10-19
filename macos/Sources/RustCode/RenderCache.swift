import Foundation

/// Parsed and styled text kept between redraws, keyed by its source. A long
/// chat redraws a message whenever it changes or its environment does, and
/// parsing its Markdown and highlighting its code again every time made a
/// chat of a few hundred messages lag; a streaming reply now re-parses only
/// the block still growing.
final class RenderCache<Value>: @unchecked Sendable {
    private final class Box {
        let value: Value
        init(_ value: Value) { self.value = value }
    }

    private let storage = NSCache<NSString, Box>()

    init(countLimit: Int) {
        storage.countLimit = countLimit
    }

    func value(for key: String, make: () -> Value) -> Value {
        let key = key as NSString
        if let hit = storage.object(forKey: key) { return hit.value }
        let value = make()
        storage.setObject(Box(value), forKey: key)
        return value
    }
}
