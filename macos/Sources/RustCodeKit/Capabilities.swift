import Foundation

/// Skills and MCP servers as the engine reports them over
/// `_rustcode/capabilities` (#2165). The terminal UI's `/skills` and `/mcp`
/// panels read the same snapshot, and every toggle is saved to the user
/// config, so both frontends always agree.
public struct CapabilitySnapshot: Decodable, Equatable, Sendable {
    public struct Skill: Decodable, Equatable, Identifiable, Sendable {
        public var name: String
        public var description: String
        public var path: String
        /// `workspace`, `universal`, `rustcode` or `configured`.
        public var source: String
        public var enabled: Bool

        public var id: String { name }

        /// Where the skill was found, for a row's subtitle.
        public var sourceLabel: String {
            switch source {
            case "workspace": "This project"
            case "universal": "Shared (~/.agents/skills)"
            case "rustcode": "RustCode"
            default: "Extra skill folder"
            }
        }
    }

    public struct MCPServer: Decodable, Equatable, Identifiable, Sendable {
        public enum Connection: String, Decodable, Sendable {
            case connected, notConnected = "not_connected", disabled, failed
        }

        public var name: String
        public var transport: String
        /// Command line or URL with credentials masked by the engine.
        public var target: String
        public var enabled: Bool
        /// `user`, `project` or `session`; only `user` servers can be toggled.
        public var scope: String
        public var connection: Connection
        public var tools: [String]
        public var error: String?

        public var id: String { name }
        public var canToggle: Bool { scope == "user" }

        public var statusLabel: String {
            switch connection {
            case .connected: tools.count == 1 ? "Connected · 1 tool" : "Connected · \(tools.count) tools"
            case .notConnected: "Not connected"
            case .disabled: "Off"
            case .failed: "Failed"
            }
        }
    }

    public var skills: [Skill]
    public var mcpServers: [MCPServer]
    /// The user config every toggle is written to.
    public var configPath: String?

    public init(skills: [Skill] = [], mcpServers: [MCPServer] = [], configPath: String? = nil) {
        self.skills = skills
        self.mcpServers = mcpServers
        self.configPath = configPath
    }

    public init(json value: JSONValue) throws {
        let data = try JSONEncoder().encode(value)
        self = try JSONDecoder().decode(Self.self, from: data)
    }

    /// Skills whose name or description contains `query`, ignoring case.
    public func skills(matching query: String) -> [Skill] {
        let query = query.trimmingCharacters(in: .whitespaces)
        guard !query.isEmpty else { return skills }
        return skills.filter { $0.name.localizedCaseInsensitiveContains(query) || $0.description.localizedCaseInsensitiveContains(query) }
    }

    /// Servers whose name, target or tools contain `query`, ignoring case.
    public func mcpServers(matching query: String) -> [MCPServer] {
        let query = query.trimmingCharacters(in: .whitespaces)
        guard !query.isEmpty else { return mcpServers }
        return mcpServers.filter { server in
            server.name.localizedCaseInsensitiveContains(query) || server.target.localizedCaseInsensitiveContains(query)
                || server.tools.contains { $0.localizedCaseInsensitiveContains(query) }
        }
    }
}

public enum CapabilityKind: String, Sendable {
    case skill, mcp
}

extension ACPMethod {
    public static let capabilities = "_rustcode/capabilities"
    public static let setCapability = "_rustcode/capabilities/set"
}

extension ACPRequests {
    public static func setCapability(_ kind: CapabilityKind, name: String, enabled: Bool) -> JSONValue {
        .object(["kind": .string(kind.rawValue), "name": .string(name), "enabled": .bool(enabled)])
    }
}
