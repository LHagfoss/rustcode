import Foundation

/// Smart UI (#2484): a `rustcode-ui v1` fence in an assistant reply holding a
/// self-contained HTML fragment the app runs in an isolated web view. The
/// format, limits and security model are in docs/smart-ui.md.
public enum SmartUI {
    /// The fence's info string names the format and its version together, so
    /// an unversioned or future block never runs here.
    public static let infoName = "rustcode-ui"
    public static let version = 1
    public static var infoString: String { "\(infoName) v\(version)" }

    /// The setting that turns Smart UI on. Off by default; the engine is told
    /// at `initialize`, so a change applies once the engine restarts.
    public static let enabledKey = "smartUIEnabled"
    public static var isEnabled: Bool { UserDefaults.standard.bool(forKey: enabledKey) }
    /// The `_meta.rustcode.smartUi` version advertised while it is on.
    public static let capabilityVersion = 1

    /// Largest fragment that runs; a bigger one shows as source.
    public static let maxSourceBytes = 64 * 1024
    /// Elements the page may build before it is stopped.
    public static let maxElements = 5_000
    /// Height range of the web view in the transcript.
    public static let minHeight = 48.0
    public static let maxHeight = 720.0

    /// Everything a page may load or contact. Scripts and styles must be
    /// inline, images and fonts `data:` URLs; nothing reaches the network,
    /// the filesystem or another frame.
    public static let contentSecurityPolicy = [
        "default-src 'none'",
        "script-src 'unsafe-inline'",
        "style-src 'unsafe-inline'",
        "img-src data:",
        "font-src data:",
        "media-src 'none'",
        "connect-src 'none'",
        "frame-src 'none'",
        "worker-src 'none'",
        "object-src 'none'",
        "form-action 'none'",
        "base-uri 'none'",
    ].joined(separator: "; ")

    /// Whether a fence opens a Smart UI block of any version. Only `v1` runs;
    /// other versions show their source with a reason.
    public static func isInfoString(_ info: String?) -> Bool {
        guard let info else { return false }
        return info.split(whereSeparator: \.isWhitespace).first.map(String.init) == infoName
    }

    /// The page loaded into the web view: the policy and base styles first,
    /// then the fragment as written. A fragment's own `<meta>` policy can only
    /// narrow this one; browsers enforce every policy present.
    public static func document(for fragment: String) -> String {
        """
        <!doctype html><html><head><meta charset="utf-8">
        <meta http-equiv="Content-Security-Policy" content="\(contentSecurityPolicy)">
        <meta name="viewport" content="width=device-width, initial-scale=1">
        <style>
        :root { color-scheme: light dark; }
        html, body { margin: 0; padding: 0; background: transparent; }
        body { font: 13px -apple-system, system-ui, sans-serif; color: CanvasText; overflow-wrap: anywhere; }
        </style>
        </head><body>
        \(fragment)
        </body></html>
        """
    }
}

/// A `rustcode-ui` fence found in assistant Markdown.
public struct SmartUIBlock: Sendable, Equatable {
    public enum State: Sendable, Equatable {
        /// Ready to run, with what to show around it.
        case ready(title: String, summary: String?)
        /// Shown as source with this reason.
        case invalid(String)
        /// The closing fence has not arrived.
        case incomplete
    }

    /// The whole fence as written, which copying reproduces.
    public let source: String
    /// The HTML between the fences.
    public let body: String
    public let state: State

    /// Identifies this block's page for the shared web view.
    public var id: String { source }

    init(opening: String, info: String, lines: [String], closing: String?) {
        body = lines.joined(separator: "\n")
        source = ([opening] + lines + (closing.map { [$0] } ?? [])).joined(separator: "\n")
        let words = info.split(whereSeparator: \.isWhitespace).map(String.init)
        if closing == nil {
            state = .incomplete
        } else if words.count < 2 {
            state = .invalid("no version; expected \(SmartUI.infoString)")
        } else if words[1] != "v\(SmartUI.version)" {
            state = .invalid("unsupported version \"\(words[1])\"")
        } else if body.utf8.count > SmartUI.maxSourceBytes {
            state = .invalid("larger than \(SmartUI.maxSourceBytes / 1024) KB")
        } else if body.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            state = .invalid("empty block")
        } else {
            let title = Self.element("title", in: body) ?? "Interactive view"
            state = .ready(title: title, summary: Self.description(in: body))
        }
    }

    /// Text of the first `<name>…</name>`, entities left as written.
    static func element(_ name: String, in html: String) -> String? {
        guard let open = html.range(of: "<\(name)", options: .caseInsensitive),
              let start = html[open.upperBound...].firstIndex(of: ">"),
              let close = html.range(of: "</\(name)>", options: .caseInsensitive,
                                     range: html.index(after: start)..<html.endIndex) else { return nil }
        let text = html[html.index(after: start)..<close.lowerBound]
            .trimmingCharacters(in: .whitespacesAndNewlines)
        return text.isEmpty ? nil : String(text.prefix(120))
    }

    /// `content` of `<meta name="description">`.
    static func description(in html: String) -> String? {
        var search = html.startIndex..<html.endIndex
        while let open = html.range(of: "<meta", options: .caseInsensitive, range: search) {
            guard let end = html[open.upperBound...].firstIndex(of: ">") else { return nil }
            let tag = String(html[open.lowerBound..<end])
            search = end..<html.endIndex
            guard tag.range(of: #"name\s*=\s*["']description["']"#, options: [.regularExpression, .caseInsensitive]) != nil,
                  let content = tag.range(of: #"content\s*=\s*("[^"]*"|'[^']*')"#, options: [.regularExpression, .caseInsensitive])
            else { continue }
            let value = tag[content].drop { $0 != "\"" && $0 != "'" }.dropFirst().dropLast()
                .trimmingCharacters(in: .whitespacesAndNewlines)
            return value.isEmpty ? nil : String(value.prefix(200))
        }
        return nil
    }
}
