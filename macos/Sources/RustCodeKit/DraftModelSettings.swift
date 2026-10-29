import Foundation

/// The model, reasoning effort and Fast mode picked on a new chat before its
/// session exists. Once the engine creates the session, these are applied to
/// it before the first prompt is sent (#2595).
public struct DraftModelSettings: Equatable, Sendable {
    public var model: String?
    public var effort: String?
    public var fast: Bool?

    public init(model: String?, effort: String?, fast: Bool?) {
        self.model = model
        self.effort = effort
        self.fast = fast
    }

    /// What the composer shows for the draft.
    public init(options: ModelOptions, selectedModel: String?) {
        self.init(model: selectedModel, effort: options.effort, fast: options.fast)
    }

    /// One `session/set_config_option` call.
    public enum Change: Equatable, Sendable {
        case model(String)
        case effort(String)
        case fast(Bool)
        /// Saved on the model's profile, so it never waits on a draft.
        case toolProtocol(String)

        /// The `session/set_config_option` params for `sessionId`.
        public func params(sessionId: String) -> JSONValue {
            switch self {
            case .model(let model): ACPRequests.setModel(sessionId: sessionId, value: model)
            case .effort(let effort): ACPRequests.setConfig(sessionId: sessionId, configId: "effort", value: effort)
            case .fast(let fast): ACPRequests.setConfig(sessionId: sessionId, configId: "fast", value: fast)
            case .toolProtocol(let id): ACPRequests.setConfig(sessionId: sessionId, configId: "tool_protocol", value: id)
            }
        }

        /// `options` with this change made, for an engine whose reply leaves
        /// out the new options.
        public func applied(to options: ModelOptions) -> ModelOptions {
            var options = options
            switch self {
            case .model(let model): options.current = model
            case .effort(let effort): options.effort = effort
            case .fast(let fast): options.fast = fast
            case .toolProtocol(let id): options.toolProtocol = id
            }
            return options
        }
    }

    /// The next change that brings a session showing `options` to this
    /// draft, or nil when it matches as far as the session allows. The model
    /// goes first, since the efforts and Fast it offers depend on it; a
    /// choice the session does not offer is dropped, not forced.
    public func nextChange(for options: ModelOptions) -> Change? {
        if let model, model != options.current, options.choices.contains(where: { $0.id == model }) {
            return .model(model)
        }
        if let effort, effort != options.effort, options.efforts.contains(where: { $0.id == effort }) {
            return .effort(effort)
        }
        if let fast, options.offersFast, fast != options.fast {
            return .fast(fast)
        }
        return nil
    }
}

/// How the composer's Model & Effort pill describes itself (#2595).
public enum ModelEffortSummary {
    /// What Fast mode does and costs, shown under its toggle.
    public static let fastExplanation = "Faster responses on ChatGPT-plan models, but it uses your plan's usage faster."

    /// The pill's tooltip and accessibility value: the model, then effort
    /// and Fast when the model offers them (nil when it does not).
    public static func help(model: String, effort: String?, fast: Bool?) -> String {
        var parts = ["Model: \(model)"]
        if let effort { parts.append("Reasoning: \(effort)") }
        if let fast { parts.append("Fast mode: \(fast ? "On" : "Off")") }
        return parts.joined(separator: " · ")
    }
}
