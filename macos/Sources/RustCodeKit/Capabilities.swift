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
    public static let addMCPServer = "_rustcode/capabilities/mcp/add"
    public static let removeMCPServer = "_rustcode/capabilities/mcp/remove"
}

extension ACPRequests {
    public static func setCapability(_ kind: CapabilityKind, name: String, enabled: Bool) -> JSONValue {
        .object(["kind": .string(kind.rawValue), "name": .string(name), "enabled": .bool(enabled)])
    }

    public static func removeMCPServer(name: String) -> JSONValue {
        .object(["name": .string(name)])
    }
}

/// What the Add Server sheet collects, before the engine validates it the
/// way `rustcode mcp add` does.
public struct MCPServerDraft: Equatable, Sendable {
    public enum Transport: String, CaseIterable, Sendable {
        case stdio, http
    }

    public var name = ""
    public var transport: Transport = .stdio
    /// The command line for a stdio server, split like a shell would.
    public var command = ""
    public var url = ""
    /// One `KEY=value` per line, stdio only.
    public var environment = ""
    /// One `Name: value` per line, http only.
    public var headers = ""

    public init() {}

    public var trimmedName: String { name.trimmingCharacters(in: .whitespacesAndNewlines) }

    /// The first problem the user can fix before sending, or nil.
    public var problem: String? {
        if trimmedName.isEmpty { return "Enter a name." }
        if trimmedName.contains(where: \.isWhitespace) { return "A name cannot contain spaces." }
        switch transport {
        case .stdio:
            switch Self.splitCommandLine(command) {
            case .failure(let error): return error.message
            case .success(let words) where words.isEmpty: return "Enter the command that starts the server."
            case .success: return nil
            }
        case .http:
            let url = url.trimmingCharacters(in: .whitespaces)
            if url.isEmpty { return "Enter the server URL." }
            if !(url.hasPrefix("http://") || url.hasPrefix("https://")) { return "The URL must start with http:// or https://." }
            return nil
        }
    }

    /// The `_rustcode/capabilities/mcp/add` params, or the problem.
    public func request() -> Result<JSONValue, MCPServerDraftError> {
        if let problem { return .failure(MCPServerDraftError(message: problem)) }
        let target: [String]
        var extra: [String: JSONValue] = [:]
        switch transport {
        case .stdio:
            guard case .success(let words) = Self.splitCommandLine(command) else {
                return .failure(MCPServerDraftError(message: "The command has an unclosed quote."))
            }
            target = words
            extra["env"] = .array(Self.lines(environment).map(JSONValue.string))
        case .http:
            target = [url.trimmingCharacters(in: .whitespaces)]
            extra["headers"] = .array(Self.lines(headers).map(JSONValue.string))
        }
        var params: [String: JSONValue] = [
            "name": .string(trimmedName),
            "transport": .string(transport.rawValue),
            "target": .array(target.map(JSONValue.string)),
        ]
        params.merge(extra) { _, new in new }
        return .success(.object(params))
    }

    static func lines(_ text: String) -> [String] {
        text.split(whereSeparator: \.isNewline)
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .filter { !$0.isEmpty }
    }

    /// Split a command line into words: whitespace separates, single and
    /// double quotes group, a backslash escapes the next character outside
    /// single quotes.
    public static func splitCommandLine(_ line: String) -> Result<[String], MCPServerDraftError> {
        var words: [String] = []
        var word = ""
        var inWord = false
        var quote: Character?
        var escaped = false
        for character in line {
            if escaped {
                word.append(character)
                escaped = false
                continue
            }
            switch (quote, character) {
            case ("'", "'"), ("\"", "\""):
                quote = nil
            case ("'", _):
                word.append(character)
            case (_, "\\"):
                escaped = true
                inWord = true
            case (nil, "'"), (nil, "\""):
                quote = character
                inWord = true
            case (nil, _) where character.isWhitespace:
                if inWord { words.append(word) }
                word = ""
                inWord = false
            default:
                word.append(character)
                inWord = true
            }
        }
        if quote != nil || escaped {
            return .failure(MCPServerDraftError(message: "The command has an unclosed quote."))
        }
        if inWord { words.append(word) }
        return .success(words)
    }
}

public struct MCPServerDraftError: Error, Equatable, Sendable {
    public var message: String
}
