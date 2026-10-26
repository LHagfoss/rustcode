import Foundation

public struct SessionModelChoice: Hashable, Sendable {
    public var id: String
    public var model: String
    public var reasoningEfforts: [String]
    public init(id: String, model: String, reasoningEfforts: [String]) {
        self.id = id
        self.model = model
        self.reasoningEfforts = reasoningEfforts
    }

    /// How an effort level reads in the picker: `Off` for the provider default,
    /// `xHigh` (not the awkward `Xhigh`), else the first letter capitalised.
    /// The wire value is unchanged.
    public static func effortLabel(_ effort: String) -> String {
        switch effort {
        case "default": return "Off"
        case "xhigh": return "xHigh"
        default: return effort.prefix(1).uppercased() + effort.dropFirst()
        }
    }
}

/// Choices supplied by the host, scoped to this session. Absence means unsupported.
public struct SessionSettings: Hashable, Sendable {
    public var models: [SessionModelChoice]
    public var selectedModel: String
    public var reasoningEffort: String
    public var canChange: Bool
    public init(models: [SessionModelChoice], selectedModel: String, reasoningEffort: String, canChange: Bool) {
        self.models = models
        self.selectedModel = selectedModel
        self.reasoningEffort = reasoningEffort
        self.canChange = canChange
    }
}

/// The terminal's command catalog, including actions presented natively by the app.
public struct SessionCommandChoice: Codable, Hashable, Sendable, Identifiable {
    public var name: String
    public var description: String
    public var insertion: String
    public var action: String
    public var id: String { name }
    public init(name: String, description: String, insertion: String, action: String = "execute") {
        self.name = name; self.description = description; self.insertion = insertion; self.action = action
    }
}
