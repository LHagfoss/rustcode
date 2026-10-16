import Foundation

/// The `_meta.rustcode.version` of usage updates this client understands.
public let rustcodeUsageMetaVersion = 1

/// A session's context and token usage, from the engine's `usage_update`.
/// `used` / `size` are the context in use and the model's window; the token
/// totals ride in `_meta.rustcode` and are absent from other ACP agents.
public struct SessionUsage: Equatable, Sendable {
    /// Tokens in the context of the latest request.
    public var used: Int
    /// The model's context window in tokens.
    public var size: Int
    /// True when `used` is estimated from the transcript, not counted by the provider.
    public var estimated: Bool
    /// Input tokens summed over every request of the session (what a provider bills).
    public var inputTokens: Int?
    public var outputTokens: Int?
    public var cachedTokens: Int?
    /// Cumulative cost, when the agent reports one.
    public var cost: Double?
    public var currency: String?
    /// The engine's `/context` breakdown, when it sends one.
    public var context: ContextBreakdown?

    public init(
        used: Int, size: Int, estimated: Bool = false, inputTokens: Int? = nil, outputTokens: Int? = nil,
        cachedTokens: Int? = nil, cost: Double? = nil, currency: String? = nil
    ) {
        self.used = used
        self.size = size
        self.estimated = estimated
        self.inputTokens = inputTokens
        self.outputTokens = outputTokens
        self.cachedTokens = cachedTokens
        self.cost = cost
        self.currency = currency
    }

    /// Reads a `usage_update` payload; nil without a usable `used` and `size`.
    public init?(json update: JSONValue) {
        guard let used = update["used"]?.intValue, let size = update["size"]?.intValue, size > 0 else { return nil }
        self.init(used: max(used, 0), size: size)
        let meta = update["_meta"]?["rustcode"]
        if meta?["kind"]?.stringValue == "usage", meta?["version"]?.intValue == rustcodeUsageMetaVersion {
            estimated = meta?["estimated"]?.boolValue ?? false
            inputTokens = meta?["inputTokens"]?.intValue
            outputTokens = meta?["outputTokens"]?.intValue
            cachedTokens = meta?["cachedTokens"]?.intValue
            context = ContextBreakdown(json: meta?["context"])
        }
        if let amount = update["cost"]?["amount"]?.doubleValue {
            cost = amount
            currency = update["cost"]?["currency"]?.stringValue
        }
    }

    /// Share of the window in use, 0...1.
    public var fraction: Double { min(max(Double(used) / Double(size), 0), 1) }

    /// Whole percent of the window still free, as the terminal footer reports it.
    public var percentLeft: Int { Int((100 * (1 - fraction)).rounded(.down)) }

    /// `34k / 128k`, with `~` in front of an estimate.
    public var label: String {
        (estimated ? "~" : "") + Self.compact(used) + " / " + Self.compact(size)
    }

    /// The tooltip: context, then the session's token totals and cost when known.
    public var detail: String {
        var lines = [
            "Context: \(Self.grouped(used)) of \(Self.grouped(size)) tokens (\(percentLeft)% left)"
                + (estimated ? ", estimated" : ""),
        ]
        if let inputTokens {
            var line = "Input: \(Self.grouped(inputTokens)) tokens"
            if let cachedTokens, cachedTokens > 0 { line += " (\(Self.grouped(cachedTokens)) cached)" }
            lines.append(line)
        }
        if let outputTokens { lines.append("Output: \(Self.grouped(outputTokens)) tokens") }
        if let cost {
            lines.append("Cost: " + cost.formatted(.currency(code: currency ?? "USD").precision(.fractionLength(2...4)).locale(Locale(identifier: "en_US"))))
        }
        return lines.joined(separator: "\n")
    }

    /// `950`, `34k`, `1.5M`, matching the terminal's context figures.
    public static func compact(_ tokens: Int) -> String {
        if tokens < 1_000 { return String(tokens) }
        if tokens < 999_500 { return "\((tokens + 500) / 1_000)k" }
        let tenths = (tokens + 50_000) / 100_000
        return tenths % 10 == 0 ? "\(tenths / 10)M" : "\(tenths / 10).\(tenths % 10)M"
    }

    static func grouped(_ tokens: Int) -> String {
        tokens.formatted(.number.grouping(.automatic).locale(Locale(identifier: "en_US")))
    }
}
