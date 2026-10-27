import Foundation

/// The small picture at the start of a tool call's row: what kind of work it
/// was, or for a page the agent read, the site it came from.
public enum ToolIcon {
    /// An SF Symbol for the call's kind of work.
    public static func symbol(for call: ToolItem) -> String {
        switch ToolPhrase.kind(of: call) {
        case .read: "doc.text"
        case .search: "magnifyingglass"
        case .web: call.displayTitle == "Web search" ? "globe" : "safari"
        case .edit: "pencil"
        case .run: "terminal"
        case .skill: "book"
        case .agent: "person.2"
        case .other: "wrench.and.screwdriver"
        }
    }

    /// The site icon for a page the call read: its origin's `/favicon.ico`.
    /// Only for a page the agent already fetched, so showing it tells the
    /// site nothing new. Nil for anything else, and for local addresses.
    public static func siteIcon(for call: ToolItem) -> URL? {
        guard call.displayTitle == "Read URL", let detail = call.detail,
              let url = URL(string: detail.trimmingCharacters(in: .whitespacesAndNewlines)),
              let scheme = url.scheme?.lowercased(), scheme == "https" || scheme == "http",
              let host = url.host(), !host.isEmpty, !PreviewURL.isLocal(url) else { return nil }
        var components = URLComponents()
        components.scheme = scheme
        components.host = host
        components.port = url.port
        components.path = "/favicon.ico"
        return components.url
    }
}
