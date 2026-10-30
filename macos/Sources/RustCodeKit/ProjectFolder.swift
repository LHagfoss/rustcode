import Foundation

/// Which folder a chat works in. New chats start in the default project
/// (Settings); one chat can start in another folder without changing the
/// default. The engine records a chat's folder when it creates the session
/// and loads it there again, so the choice outlives the app.
public enum ProjectFolder {
    /// The folder of the chat on screen: an open chat's recorded folder,
    /// else the folder picked for the draft, else the default.
    public static func forChat(sessionCwd: String?, draft: URL?, default fallback: URL) -> URL {
        if let sessionCwd, !sessionCwd.isEmpty { return URL(fileURLWithPath: sessionCwd, isDirectory: true) }
        return draft ?? fallback
    }

    /// A folder picked for one chat, or nil when it is the default anyway.
    public static func override(_ picked: URL?, default fallback: URL) -> URL? {
        guard let picked else { return nil }
        return same(picked, fallback) ? nil : picked
    }

    /// A folder that exists and can be read, so a chat or shell can work in it.
    public static func isUsable(_ folder: URL) -> Bool {
        var isDirectory: ObjCBool = false
        let path = folder.standardizedFileURL.path
        return FileManager.default.fileExists(atPath: path, isDirectory: &isDirectory) && isDirectory.boolValue
            && FileManager.default.isReadableFile(atPath: path)
    }

    /// The tooltip of the chat's folder: where this chat works and, when it
    /// differs, where new chats start.
    public static func help(chat: URL, default fallback: URL) -> String {
        let here = (chat.path as NSString).abbreviatingWithTildeInPath
        guard !same(chat, fallback) else { return "This thread's folder: \(here), the default project" }
        let start = (fallback.path as NSString).abbreviatingWithTildeInPath
        return "This thread's folder: \(here)\nDefault project for new threads: \(start)"
    }

    /// Two paths name the same folder, ignoring a trailing slash, `.` and `..`.
    public static func same(_ a: URL, _ b: URL) -> Bool {
        a.standardizedFileURL.path == b.standardizedFileURL.path
    }
}
