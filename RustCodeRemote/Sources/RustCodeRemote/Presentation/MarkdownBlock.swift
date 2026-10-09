import Foundation

/// A block of assistant text, split just far enough for a native transcript: paragraphs,
/// headings, list items, quotes and fenced code. Inline styling inside a block is left to
/// `AttributedString(markdown:)`.
public enum MarkdownBlock: Sendable, Equatable {
    case paragraph(String)
    case heading(level: Int, text: String)
    case listItem(marker: String, text: String)
    case quote(String)
    case code(language: String?, text: String)

    /// Tolerates text that is still streaming: an unterminated fence is a code block so far.
    public static func parse(_ text: String) -> [MarkdownBlock] {
        var blocks: [MarkdownBlock] = []
        var paragraph: [String] = []
        var code: (language: String?, lines: [String])?

        func flushParagraph() {
            guard !paragraph.isEmpty else { return }
            blocks.append(.paragraph(paragraph.joined(separator: "\n")))
            paragraph.removeAll()
        }

        for line in text.split(separator: "\n", omittingEmptySubsequences: false).map(String.init) {
            let trimmed = line.trimmingCharacters(in: .whitespaces)

            if let open = code {
                if trimmed.hasPrefix("```") {
                    blocks.append(.code(language: open.language, text: open.lines.joined(separator: "\n")))
                    code = nil
                } else {
                    code?.lines.append(line)
                }
                continue
            }

            if trimmed.hasPrefix("```") {
                flushParagraph()
                let language = trimmed.dropFirst(3).trimmingCharacters(in: .whitespaces)
                code = (language.isEmpty ? nil : language, [])
            } else if trimmed.isEmpty {
                flushParagraph()
            } else if let heading = heading(in: trimmed) {
                flushParagraph()
                blocks.append(heading)
            } else if let item = listItem(in: trimmed) {
                flushParagraph()
                blocks.append(item)
            } else if trimmed.hasPrefix(">") {
                flushParagraph()
                blocks.append(.quote(trimmed.dropFirst().trimmingCharacters(in: .whitespaces)))
            } else {
                paragraph.append(trimmed)
            }
        }

        flushParagraph()
        if let open = code {
            blocks.append(.code(language: open.language, text: open.lines.joined(separator: "\n")))
        }
        return blocks
    }

    private static func heading(in line: String) -> MarkdownBlock? {
        let hashes = line.prefix { $0 == "#" }.count
        guard (1...6).contains(hashes) else { return nil }
        let rest = line.dropFirst(hashes)
        guard rest.hasPrefix(" ") else { return nil }
        return .heading(level: hashes, text: rest.trimmingCharacters(in: .whitespaces))
    }

    private static func listItem(in line: String) -> MarkdownBlock? {
        for bullet in ["- ", "* ", "+ "] where line.hasPrefix(bullet) {
            return .listItem(marker: "•", text: String(line.dropFirst(2)))
        }
        let digits = line.prefix { $0.isNumber }
        guard !digits.isEmpty, digits.count <= 3 else { return nil }
        let rest = line.dropFirst(digits.count)
        guard rest.hasPrefix(". ") || rest.hasPrefix(") ") else { return nil }
        return .listItem(marker: "\(digits).", text: String(rest.dropFirst(2)))
    }
}
