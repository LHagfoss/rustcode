import Foundation

/// Settings and ref provenance for opt-in desktop automation.
public enum DesktopAutomation {
    public static let enabledKey = "rustcode.desktopAutomation.enabled"
    public static var isEnabled: Bool { UserDefaults.standard.bool(forKey: enabledKey) }
    public static let method = "_rustcode/desktop"
}

/// Refs are valid only for the chat and app whose latest snapshot issued them.
public struct DesktopRefLedger {
    private var refs: [String: [String: [String: String]]] = [:]
    public init() {}
    public mutating func clear() { refs.removeAll() }
    public mutating func clear(session: String) { refs.removeValue(forKey: session) }
    public mutating func clear(session: String, app: String) { refs[session]?.removeValue(forKey: app) }
    public mutating func record(session: String, app: String, tree: JSONValue, window: String = "") {
        var found: [String: String] = [:]
        func visit(_ node: JSONValue) {
            if let ref = node["ref_id"]?.stringValue {
                let role = node["role"]?.stringValue ?? "element"
                let name = node["name"]?.stringValue ?? node["description"]?.stringValue ?? "unnamed"
                found[ref] = "\(window.isEmpty ? "Window unknown" : window) · \(role) · \(name)"
            }
            for child in node["children"]?.arrayValue ?? [] { visit(child) }
        }
        visit(tree)
        refs[session, default: [:]][app] = found
    }
    public func describe(session: String, app: String, ref: String) -> String? {
        refs[session]?[app]?[ref]
    }
    public func contains(session: String, app: String, ref: String) -> Bool {
        describe(session: session, app: app, ref: ref) != nil
    }
}
