import Foundation

/// The `_meta.rustcode.presence` version this app sends in `initialize`, so
/// the engine publishes Discord Rich Presence for it (#2179).
public let rustcodePresenceCapabilityVersion = 1

/// Discord Rich Presence as the engine reports it: whether the setting is on
/// for the project, and whether Discord's desktop app is reachable.
public struct PresenceStatus: Equatable, Sendable {
    public var enabled: Bool
    public var discordDetected: Bool

    public init(enabled: Bool, discordDetected: Bool) {
        self.enabled = enabled
        self.discordDetected = discordDetected
    }

    public init?(result: JSONValue) {
        guard let enabled = result["enabled"]?.boolValue else { return nil }
        self.init(enabled: enabled, discordDetected: result["discordDetected"]?.boolValue ?? false)
    }
}

public enum PresenceRequests {
    public static let status = "_rustcode/presence"
    public static let set = "_rustcode/presence/set"

    public static func status(cwd: URL) -> JSONValue {
        ["cwd": .string(cwd.path)]
    }

    public static func set(enabled: Bool, cwd: URL) -> JSONValue {
        ["enabled": .bool(enabled), "cwd": .string(cwd.path)]
    }
}
