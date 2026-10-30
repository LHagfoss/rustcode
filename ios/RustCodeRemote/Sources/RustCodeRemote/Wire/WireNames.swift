import Foundation

/// Thread names on the wire.
///
/// Remote protocol v1 names a conversation a "session" (`session_id`, `list_sessions`,
/// `kind: "sessions"`, …). The app asks for "thread" in those names instead
/// (`wire_names: "thread"` in its handshake). A gateway that supports it confirms with the
/// same field in its `paired` or `authenticated` frame; an older one leaves it out and the
/// connection keeps the v1 names. Only the names change, so the `Wire…` types stay in v1
/// names and `LiveRemoteClient` translates each frame at the socket.
///
/// Mirrors `rustcode/engine/src/remote_gateway/wire_names.rs`: only the object keys and the
/// tag values (under `type`, `kind`, `code`, `reason`) listed here are translated; text is
/// never touched.
public enum WireNames {
    public static let thread = "thread"

    /// `(v1, thread)` object keys.
    static let keys: [(String, String)] = [
        ("session_id", "thread_id"),
        ("session", "thread"),
        ("sessions", "threads"),
    ]

    static let tagFields: Set<String> = ["type", "kind", "code", "reason"]

    /// `(v1, thread)` tag values.
    static let tags: [(String, String)] = [
        ("attach_session", "attach_thread"),
        ("create_session", "create_thread"),
        ("detach_session", "detach_thread"),
        ("list_saved_sessions", "list_saved_threads"),
        ("list_sessions", "list_threads"),
        ("resume_session", "resume_thread"),
        ("saved_sessions", "saved_threads"),
        ("session_changed", "thread_changed"),
        ("session_closed", "thread_closed"),
        ("session_created", "thread_created"),
        ("session_resumed", "thread_resumed"),
        ("session_settings_updated", "thread_settings_updated"),
        ("sessions", "threads"),
        ("set_session_settings", "set_thread_settings"),
        ("stale_session", "stale_thread"),
        ("subscribe_sessions", "subscribe_threads"),
    ]

    /// Whether a handshake answer confirms the thread names.
    public static func confirmed(in frame: String) -> Bool {
        guard let object = try? JSONSerialization.jsonObject(with: Data(frame.utf8)) as? [String: Any],
              let type = object["type"] as? String, type == "paired" || type == "authenticated"
        else { return false }
        return object["wire_names"] as? String == thread
    }

    /// A frame the app built in v1 names, in thread names.
    public static func toThreadNames(_ frame: String) -> String { rewrite(frame, toThread: true) }

    /// A frame the gateway sent in thread names, in v1 names.
    public static func toSessionNames(_ frame: String) -> String { rewrite(frame, toThread: false) }

    private static func rename(_ name: String, in table: [(String, String)], toThread: Bool) -> String? {
        table.first { (toThread ? $0.0 : $0.1) == name }.map { toThread ? $0.1 : $0.0 }
    }

    private static func translate(_ value: Any, toThread: Bool) -> Any {
        if let array = value as? [Any] {
            return array.map { translate($0, toThread: toThread) }
        }
        guard let object = value as? [String: Any] else { return value }
        var result: [String: Any] = [:]
        for (key, field) in object {
            let name = rename(key, in: keys, toThread: toThread) ?? key
            if tagFields.contains(key), let tag = field as? String {
                result[name] = rename(tag, in: tags, toThread: toThread) ?? tag
            } else {
                result[name] = translate(field, toThread: toThread)
            }
        }
        return result
    }

    private static func rewrite(_ frame: String, toThread: Bool) -> String {
        guard let object = try? JSONSerialization.jsonObject(with: Data(frame.utf8), options: [.fragmentsAllowed]),
              object is [String: Any] || object is [Any],
              let data = try? JSONSerialization.data(
                withJSONObject: translate(object, toThread: toThread),
                options: [.withoutEscapingSlashes]
              )
        else { return frame }
        return String(decoding: data, as: UTF8.self)
    }
}
