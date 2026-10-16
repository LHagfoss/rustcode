import Foundation

/// The `version` of `_meta.rustcode.context` this client understands.
public let rustcodeContextMetaVersion = 1

/// How the model's context window is spent, as the engine's `/context`
/// computes it. It rides in `_meta.rustcode.context` on `usage_update` and on
/// the `/context` reply.
public struct ContextBreakdown: Equatable, Sendable, Identifiable {
    public struct Category: Equatable, Sendable {
        public var name: String
        public var tokens: Int

        public init(name: String, tokens: Int) {
            self.name = name
            self.tokens = tokens
        }
    }

    /// Reserves the profile keeps out of the window: limits, not usage.
    public struct Reserves: Equatable, Sendable {
        public var output: Int
        /// Part of `output`.
        public var thinking: Int
        public var tools: Int
        public var safety: Int
        public var providerOverhead: Int

        public init(output: Int, thinking: Int, tools: Int, safety: Int, providerOverhead: Int) {
            self.output = output
            self.thinking = thinking
            self.tools = tools
            self.safety = safety
            self.providerOverhead = providerOverhead
        }
    }

    public var model: String?
    public var window: Int
    public var used: Int
    /// `used` is estimated from the saved history, not counted by the provider.
    public var estimated: Bool
    public var free: Int
    /// In the terminal's legend order.
    public var categories: [Category]
    public var reserves: Reserves?

    public var id: String { "\(window)-\(used)-\(categories.map(\.tokens))" }

    public init(model: String? = nil, window: Int, used: Int, estimated: Bool = false, free: Int,
                categories: [Category] = [], reserves: Reserves? = nil) {
        self.model = model
        self.window = window
        self.used = used
        self.estimated = estimated
        self.free = free
        self.categories = categories
        self.reserves = reserves
    }

    /// Reads `_meta.rustcode.context`; nil for another version or no window.
    public init?(json: JSONValue?) {
        guard let json, json["version"]?.intValue == rustcodeContextMetaVersion,
              let window = json["window"]?.intValue, window > 0,
              let used = json["used"]?.intValue else { return nil }
        self.init(
            model: json["model"]?.stringValue,
            window: window,
            used: max(used, 0),
            estimated: json["estimated"]?.boolValue ?? false,
            free: max(json["free"]?.intValue ?? window - used, 0),
            categories: (json["categories"]?.arrayValue ?? []).compactMap { category in
                guard let name = category["name"]?.stringValue else { return nil }
                return Category(name: name, tokens: max(category["tokens"]?.intValue ?? 0, 0))
            })
        if let reserves = json["reserves"], reserves["output"]?.intValue != nil {
            self.reserves = Reserves(
                output: reserves["output"]?.intValue ?? 0,
                thinking: reserves["thinking"]?.intValue ?? 0,
                tools: reserves["tools"]?.intValue ?? 0,
                safety: reserves["safety"]?.intValue ?? 0,
                providerOverhead: reserves["providerOverhead"]?.intValue ?? 0)
        }
    }

    /// A breakdown with only the headline, for an engine that sends no categories.
    public init(usage: SessionUsage) {
        self.init(window: usage.size, used: usage.used, estimated: usage.estimated,
                  free: max(usage.size - usage.used, 0))
    }

    /// `tokens` as a share of the window, 0...1.
    public func share(_ tokens: Int) -> Double {
        window > 0 ? min(max(Double(tokens) / Double(window), 0), 1) : 0
    }

    /// `12.3%` of the window.
    public func percent(_ tokens: Int) -> String {
        String(format: "%.1f%%", share(tokens) * 100)
    }

    /// Bar segments in legend order. Each category keeps a sliver when it holds
    /// any tokens, and the segments never exceed the whole bar.
    public var segments: [(category: Category, share: Double)] {
        var remaining = 1.0
        return categories.compactMap { category in
            guard category.tokens > 0, remaining > 0 else { return nil }
            let share = min(max(self.share(category.tokens), 0.004), remaining)
            remaining -= share
            return (category, share)
        }
    }
}
