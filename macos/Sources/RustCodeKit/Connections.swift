import Foundation

/// GitHub (through the GitHub CLI) and the saved model accounts, as the engine
/// reports them over `_rustcode/connections`.
public struct ConnectionsSnapshot: Decodable, Equatable, Sendable {
    public struct GitHub: Decodable, Equatable, Sendable {
        public var cliInstalled: Bool
        public var accounts: [GitHubAccount]
        /// A sign-in waiting for the browser step.
        public var pending: DeviceCode?
        public var error: String?

        public init(cliInstalled: Bool = false, accounts: [GitHubAccount] = [], pending: DeviceCode? = nil, error: String? = nil) {
            self.cliInstalled = cliInstalled
            self.accounts = accounts
            self.pending = pending
            self.error = error
        }

        /// The account `gh` uses for github.com, if it is signed in.
        public var active: GitHubAccount? {
            accounts.first { $0.host == "github.com" && $0.active }
        }
    }

    public struct GitHubAccount: Decodable, Equatable, Identifiable, Sendable {
        public var host: String
        public var login: String
        public var active: Bool
        /// False when the stored token no longer works.
        public var valid: Bool
        public var scopes: [String]

        public var id: String { "\(host)/\(login)" }

        /// Scopes the agent's GitHub work needs and the token lacks.
        public var missingScopes: [String] {
            ["repo", "read:org"].filter { !scopes.contains($0) }
        }
    }

    public struct DeviceCode: Codable, Equatable, Sendable {
        public var code: String
        public var url: String

        public init(code: String, url: String) {
            self.code = code
            self.url = url
        }
    }

    public struct ModelAccount: Decodable, Equatable, Identifiable, Sendable {
        public var provider: String
        public var account: String
        public var display: String
        /// `api_key`, `chat_gpt`, `github_copilot`, or legacy `claude_cli`.
        public var method: String
        public var active: Bool
        public var expiresAt: UInt64?
        /// The provider refused the saved sign-in; it needs `/login` again
        /// (#2765). Absent from older engines.
        public var needsSignIn: Bool?

        public var id: String { "\(provider)/\(account)" }

        public var methodLabel: String {
            switch method {
            case "api_key": "API key"
            case "chat_gpt": "ChatGPT sign-in"
            case "github_copilot": "GitHub Copilot"
            case "claude_cli": "Unsupported Claude CLI (legacy)"
            default: method
            }
        }
    }

    public var github: GitHub
    public var accounts: [ModelAccount]

    public init(github: GitHub = GitHub(), accounts: [ModelAccount] = []) {
        self.github = github
        self.accounts = accounts
    }

    public init(json value: JSONValue) throws {
        let data = try JSONEncoder().encode(value)
        self = try JSONDecoder().decode(Self.self, from: data)
    }
}

extension ACPMethod {
    public static let connections = "_rustcode/connections"
    public static let githubLogin = "_rustcode/connections/github/login"
    public static let githubWait = "_rustcode/connections/github/wait"
    public static let githubCancel = "_rustcode/connections/github/cancel"
    public static let githubLogout = "_rustcode/connections/github/logout"
    public static let accountLogout = "_rustcode/connections/account/logout"
    public static let accountRefresh = "_rustcode/connections/account/refresh"
}

extension ACPRequests {
    public static func githubLogout(host: String, login: String) -> JSONValue {
        .object(["host": .string(host), "login": .string(login)])
    }

    public static func accountLogout(provider: String, account: String) -> JSONValue {
        .object(["provider": .string(provider), "account": .string(account)])
    }

    public static func accountRefresh(provider: String, account: String) -> JSONValue {
        .object(["provider": .string(provider), "account": .string(account)])
    }
}
