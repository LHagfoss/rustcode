import Foundation

/// Where the app found the `rustcode` engine binary.
public struct EngineLocation: Equatable, Sendable {
    public enum Source: String, Equatable, Sendable {
        /// Path set in Settings.
        case userSetting = "Settings"
        /// `RUSTCODE_ENGINE` in the app's environment (development).
        case environment = "RUSTCODE_ENGINE"
        /// `RustCode.app/Contents/Helpers/rustcode`, shipped with the app.
        case bundled = "Bundled with the app"
        /// A CLI install found on a well-known path.
        case installed = "Installed CLI"
    }

    public var url: URL
    public var source: Source
}

public enum EngineLocatorError: Error, Equatable, CustomStringConvertible {
    case notExecutable(path: String, source: EngineLocation.Source)
    case notFound(searched: [String])

    public var description: String {
        switch self {
        case .notExecutable(let path, let source):
            return "The engine path from \(source.rawValue) is not an executable file: \(path)"
        case .notFound(let searched):
            return "No RustCode engine was found. Install the RustCode CLI or set its path in "
                + "Settings. Searched: \(searched.joined(separator: ", "))"
        }
    }
}

/// Resolves the engine binary. Order: an explicit setting, then the
/// `RUSTCODE_ENGINE` variable, then the copy bundled in the app, then a CLI
/// install. The bundled copy comes from the same release as the app, so the
/// app and its protocol peer are versioned together; an explicit choice
/// always wins so a developer can point the app at a local build.
public struct EngineLocator {
    public var userSetting: String?
    public var environment: [String: String]
    public var bundleURL: URL
    public var homeDirectory: URL
    public var isExecutable: (String) -> Bool

    public init(
        userSetting: String?,
        environment: [String: String] = ProcessInfo.processInfo.environment,
        bundleURL: URL = Bundle.main.bundleURL,
        homeDirectory: URL = FileManager.default.homeDirectoryForCurrentUser,
        isExecutable: @escaping (String) -> Bool = EngineLocator.isExecutableFile
    ) {
        self.userSetting = userSetting
        self.environment = environment
        self.bundleURL = bundleURL
        self.homeDirectory = homeDirectory
        self.isExecutable = isExecutable
    }

    public static func isExecutableFile(_ path: String) -> Bool {
        var isDirectory: ObjCBool = false
        return FileManager.default.fileExists(atPath: path, isDirectory: &isDirectory)
            && !isDirectory.boolValue
            && FileManager.default.isExecutableFile(atPath: path)
    }

    public var bundledPath: String {
        bundleURL.appendingPathComponent("Contents/Helpers/rustcode").path
    }

    /// Well-known CLI install locations: `install.sh`, Homebrew, Cargo.
    public var installedCandidates: [String] {
        [
            homeDirectory.appendingPathComponent(".local/bin/rustcode").path,
            "/opt/homebrew/bin/rustcode",
            "/usr/local/bin/rustcode",
            homeDirectory.appendingPathComponent(".cargo/bin/rustcode").path,
        ]
    }

    public func locate() throws -> EngineLocation {
        if let path = Self.expand(userSetting, home: homeDirectory) {
            guard isExecutable(path) else {
                throw EngineLocatorError.notExecutable(path: path, source: .userSetting)
            }
            return EngineLocation(url: URL(fileURLWithPath: path), source: .userSetting)
        }
        if let path = Self.expand(environment["RUSTCODE_ENGINE"], home: homeDirectory) {
            guard isExecutable(path) else {
                throw EngineLocatorError.notExecutable(path: path, source: .environment)
            }
            return EngineLocation(url: URL(fileURLWithPath: path), source: .environment)
        }
        if isExecutable(bundledPath) {
            return EngineLocation(url: URL(fileURLWithPath: bundledPath), source: .bundled)
        }
        for path in installedCandidates where isExecutable(path) {
            return EngineLocation(url: URL(fileURLWithPath: path), source: .installed)
        }
        throw EngineLocatorError.notFound(searched: [bundledPath] + installedCandidates)
    }

    /// The engine's environment. Apps launched from Finder get a minimal
    /// `PATH`, so the usual tool locations are appended (never prepended) so
    /// shell tools find the same `git`, `cargo` and `brew` as a terminal. The
    /// engine itself hydrates provider API keys from the login shell.
    public func engineEnvironment() -> [String: String] {
        var environment = self.environment
        var path = (environment["PATH"] ?? "/usr/bin:/bin:/usr/sbin:/sbin")
            .split(separator: ":").map(String.init)
        let extras = [
            "/opt/homebrew/bin", "/opt/homebrew/sbin", "/usr/local/bin",
            homeDirectory.appendingPathComponent(".cargo/bin").path,
            homeDirectory.appendingPathComponent(".local/bin").path,
            homeDirectory.appendingPathComponent(".bun/bin").path,
        ]
        for extra in extras where !path.contains(extra) {
            path.append(extra)
        }
        environment["PATH"] = path.joined(separator: ":")
        return environment
    }

    static func expand(_ raw: String?, home: URL) -> String? {
        guard let raw = raw?.trimmingCharacters(in: .whitespacesAndNewlines), !raw.isEmpty else {
            return nil
        }
        if raw == "~" { return home.path }
        if raw.hasPrefix("~/") {
            return home.appendingPathComponent(String(raw.dropFirst(2))).path
        }
        return raw
    }
}
