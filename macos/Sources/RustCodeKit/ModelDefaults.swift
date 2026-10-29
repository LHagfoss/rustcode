import Foundation

/// The model profile and reasoning effort new chats start with, as
/// `rustcode defaults` prints them. The engine keeps them in its config, so the
/// terminal UI shares them.
public struct ModelDefaults: Decodable, Equatable, Sendable {
    public struct Profile: Decodable, Equatable, Sendable {
        public var name: String
        public var model: String
        public var efforts: [String]
        /// Whether the profile offers Fast mode (a ChatGPT plan). Nil from an
        /// engine that does not say, which shows no toggle.
        public var fast: Bool?
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
        return efforts.map { ModelChoice(id: $0, name: ModelChoice.effortLabel($0), detail: nil) }
    }

    /// Whether a profile offers Fast mode, before any session runs it.
    public func supportsFast(_ profile: String) -> Bool {
        models.first { $0.name == profile }?.fast == true
    }

    /// The composer options of a new chat that starts from these defaults.
    /// Fast starts off where the default profile offers it (#2595).
    public var options: ModelOptions {
        var options = ModelOptions(current: model, choices: models.map { ModelChoice(id: $0.name, name: $0.name, detail: $0.model) })
        options.efforts = efforts(for: model)
        options.effort = options.efforts.isEmpty ? nil : effort
        options.fast = supportsFast(model) ? false : nil
        return options
    }

    /// A draft's options once `profile` is picked for it: its effort stays
    /// when the profile offers it (else the provider default), and Fast shows
    /// only for a profile that offers it, as `fastPick`, the last Fast choice
    /// made on the draft, so it survives a detour through another model.
    public func draftOptions(_ options: ModelOptions, switchingTo profile: String, fastPick: Bool) -> ModelOptions {
        var options = options
        options.current = profile
        options.efforts = efforts(for: profile)
        if !options.efforts.contains(where: { $0.id == options.effort }) {
            options.effort = options.efforts.isEmpty ? nil : "default"
        }
        options.fast = supportsFast(profile) ? fastPick : nil
        return options
    }
}
