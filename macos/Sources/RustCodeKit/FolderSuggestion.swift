import Foundation

/// A thread that works in another folder than its own: most of its tool
/// calls pass a `cwd` outside the thread's folder (thread 01a15d3d worked
/// in ~/code/rustcode from ~/code/discord_bot). Subagents and calls without
/// a `cwd` still start in the thread's folder, so the app offers to move the
/// thread to where the work is.
public enum FolderSuggestion {
    /// Fewer outside calls than this are a quick look elsewhere.
    public static let minimumCalls = 3

    /// The folder `items` mostly work in, when it is not `folder` or inside
    /// it: the project root (`root(of:)`) of the outside `cwd` used most.
    /// Nil when most calls ran in the thread's folder or gave no `cwd`.
    public static func suggest(items: [TranscriptItem], folder: URL,
                               root: (URL) -> URL = { $0 }) -> URL? {
        let base = folder.standardizedFileURL.path
        var calls = 0
        var outside: [String: Int] = [:]
        for item in items {
            guard case .tool(let tool) = item.content else { continue }
            calls += 1
            guard let cwd = cwd(of: tool), !inside(cwd, base) else { continue }
            outside[root(URL(fileURLWithPath: cwd)).standardizedFileURL.path, default: 0] += 1
        }
        let total = outside.values.reduce(0, +)
        guard total >= minimumCalls, total * 2 > calls,
              let (path, _) = outside.max(by: { $0.value < $1.value }),
              !inside(path, base) else { return nil }
        return URL(fileURLWithPath: path, isDirectory: true)
    }

    /// The nearest folder holding `.git` at or above `url`, else `url`.
    public static func projectRoot(of url: URL, fileManager: FileManager = .default) -> URL {
        var candidate = url.standardizedFileURL
        while candidate.path != "/" {
            if fileManager.fileExists(atPath: candidate.appendingPathComponent(".git").path) { return candidate }
            candidate = candidate.deletingLastPathComponent()
        }
        return url.standardizedFileURL
    }

    /// The absolute `cwd` a call's arguments name.
    static func cwd(of tool: ToolItem) -> String? {
        guard let data = tool.input?.data(using: .utf8),
              let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let cwd = (object["cwd"] as? String)?.trimmingCharacters(in: .whitespaces),
              cwd.hasPrefix("/") || cwd.hasPrefix("~") else { return nil }
        return (cwd as NSString).expandingTildeInPath
    }

    private static func inside(_ path: String, _ base: String) -> Bool {
        let path = URL(fileURLWithPath: path).standardizedFileURL.path
        return path == base || path.hasPrefix(base.hasSuffix("/") ? base : base + "/")
    }
}
