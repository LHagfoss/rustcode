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
