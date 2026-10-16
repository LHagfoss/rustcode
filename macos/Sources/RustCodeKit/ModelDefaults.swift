import Foundation

/// The model profile and reasoning effort new chats start with, as
/// `rustcode defaults` prints them. The engine keeps them in its config, so the
/// terminal UI shares them.
public struct ModelDefaults: Decodable, Equatable, Sendable {
    public struct Profile: Decodable, Equatable, Sendable {
        public var name: String
        public var model: String
        public var efforts: [String]
    }

    public var model: String
    public var effort: String
    public var models: [Profile]

    public init(json: Data) throws {
        self = try JSONDecoder().decode(Self.self, from: json)
    }

    /// Effort choices for a profile, labelled like the engine's effort select.
    /// Empty when the profile offers no choice.
    public func efforts(for profile: String) -> [ModelChoice] {
        let efforts = models.first { $0.name == profile }?.efforts ?? []
        guard efforts.count > 1 else { return [] }
        return efforts.map { ModelChoice(id: $0, name: $0 == "default" ? "Off" : $0.prefix(1).uppercased() + $0.dropFirst(), detail: nil) }
    }

    /// The composer options of a new chat that starts from these defaults.
    public var options: ModelOptions {
        var options = ModelOptions(current: model, choices: models.map { ModelChoice(id: $0.name, name: $0.name, detail: $0.model) })
        options.efforts = efforts(for: model)
        options.effort = options.efforts.isEmpty ? nil : effort
        return options
    }
}
