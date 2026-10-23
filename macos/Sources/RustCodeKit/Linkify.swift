import Foundation

/// Web addresses written out in plain text, which the chat turns into links.
/// Only an address with a scheme or `www.` counts: a file name such as
/// `README.md` reads as a domain to a data detector but is not a link.
public enum Linkify {
    public struct Link: Equatable, Sendable {
        /// Character offsets into the text.
        public var range: Range<Int>
        public var url: URL
    }

    private static let detector = try? NSDataDetector(types: NSTextCheckingResult.CheckingType.link.rawValue)

    /// The links in `text`, in order. With `skipCodeSpans`, nothing inside a
    /// backtick code span counts, as in a user's message, where code is
    /// still written in Markdown.
    public static func links(in text: String, skipCodeSpans: Bool = false) -> [Link] {
        // Most prose has no address at all, and the detector is slow next to this.
        guard let detector, text.contains("://") || text.localizedCaseInsensitiveContains("www.")
                || text.localizedCaseInsensitiveContains("mailto:") else { return [] }
        let code = skipCodeSpans ? codeSpans(in: text) : []
        // A backtick in the text stops the detector finding any link in it,
        // so it reads a copy with spaces in their place, the same length.
        let scanned = text.replacingOccurrences(of: "`", with: " ")
        let length = (scanned as NSString).length
        return detector.matches(in: scanned, range: NSRange(location: 0, length: length)).compactMap { match in
            guard let url = match.url, let range = Range(match.range, in: text) else { return nil }
            let written = text[range].lowercased()
            guard written.contains("://") || written.hasPrefix("www.") || written.hasPrefix("mailto:") else { return nil }
            let start = text.distance(from: text.startIndex, to: range.lowerBound)
            let offsets = start..<(start + text[range].count)
            guard !code.contains(where: { $0.overlaps(offsets) }) else { return nil }
            return Link(range: offsets, url: url)
        }
    }

    /// Character ranges of code spans and fenced blocks, delimiters
    /// included: a run of backticks opens one, and the next run of the same
    /// length closes it, as in Markdown.
    static func codeSpans(in text: String) -> [Range<Int>] {
        var runs: [(start: Int, length: Int)] = []
        var offset = 0
        var characters = text.makeIterator()
        var pending: Character? = characters.next()
        while let character = pending {
            if character == "`" {
                let start = offset
                while pending == "`" {
                    offset += 1
                    pending = characters.next()
                }
                runs.append((start, offset - start))
            } else {
                offset += 1
                pending = characters.next()
            }
        }
        var spans: [Range<Int>] = []
        var index = 0
        while index < runs.count {
            let open = runs[index]
            guard let close = runs[(index + 1)...].firstIndex(where: { $0.length == open.length }) else {
                index += 1
                continue
            }
            spans.append(open.start..<(runs[close].start + runs[close].length))
            index = close + 1
        }
        return spans
    }
}
