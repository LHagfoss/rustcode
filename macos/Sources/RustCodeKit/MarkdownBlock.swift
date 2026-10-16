import Foundation

/// A block of assistant text, split just far enough for a native transcript: paragraphs,
/// headings, list items, quotes, tables and fenced code. Inline styling is left to
/// `AttributedString(markdown:)`.
public enum MarkdownBlock: Sendable, Equatable {
    case paragraph(String)
    case heading(level: Int, text: String)
    case listItem(marker: String, text: String)
    case quote(String)
    case code(language: String?, text: String)
    case table(headers: [String], rows: [[String]])
    case rule
    /// A line holding only `![alt](url)` with a `file://` or `http(s)://` URL.
    case image(alt: String, url: URL)

    /// Tolerates text that is still streaming: an unterminated fence is a code block so far.
    public static func parse(_ text: String) -> [MarkdownBlock] {
        let normalizedText = text.replacingOccurrences(of: "\r\n", with: "\n")
            .replacingOccurrences(of: "\r", with: "\n")
        if let json = formattedJSON(in: normalizedText) {
            return [.code(language: "json", text: json)]
        }

        var blocks: [MarkdownBlock] = []
        var paragraph: [String] = []
        var code: (fence: MarkdownFence, lines: [String])?

        func flushParagraph() {
            guard !paragraph.isEmpty else { return }
            blocks.append(.paragraph(paragraph.joined(separator: "\n")))
            paragraph.removeAll()
        }

        let lines = normalizedText.components(separatedBy: "\n")
        var index = 0
        while index < lines.count {
            let line = lines[index]
            index += 1
            let trimmed = line.trimmingCharacters(in: .whitespaces)

            if let open = code {
                if open.fence.closes(line) {
                    blocks.append(.code(language: open.fence.language, text: open.lines.joined(separator: "\n")))
                    code = nil
                } else {
                    code?.lines.append(line)
                }
                continue
            }

            if let fence = MarkdownFence.opening(line) {
                flushParagraph()
                code = (fence, [])
            } else if index < lines.count,
                      let headers = tableCells(line),
                      let separators = tableCells(lines[index]),
                      headers.count == separators.count,
                      separators.allSatisfy(isTableSeparator) {
                flushParagraph()
                index += 1
                var rows: [[String]] = []
                while index < lines.count, !startsBlock(lines[index]), let cells = tableCells(lines[index]) {
                    rows.append(Array((cells + Array(repeating: "", count: headers.count)).prefix(headers.count)))
                    index += 1
                }
                blocks.append(.table(headers: headers, rows: rows))
            } else if isRule(trimmed) {
                flushParagraph()
                blocks.append(.rule)
            } else if trimmed.isEmpty {
                flushParagraph()
            } else if let image = image(in: trimmed) {
                flushParagraph()
                blocks.append(image)
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
            blocks.append(.code(language: open.fence.language, text: open.lines.joined(separator: "\n")))
        }
        return blocks
    }

    private static func formattedJSON(in text: String) -> String? {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard trimmed.first == "{" || trimmed.first == "[",
              let data = trimmed.data(using: .utf8),
              let value = try? JSONSerialization.jsonObject(with: data),
              value is [String: Any] || value is [Any],
              let formatted = try? JSONSerialization.data(
                withJSONObject: value,
                options: [.prettyPrinted, .sortedKeys]
              ) else { return nil }
        return String(data: formatted, encoding: .utf8)
    }

    private static func image(in line: String) -> MarkdownBlock? {
        guard line.hasPrefix("!["), line.hasSuffix(")"),
              let altEnd = line.range(of: "]("),
              !line[line.index(line.startIndex, offsetBy: 2)..<altEnd.lowerBound].contains("]") else { return nil }
        let target = String(line[altEnd.upperBound..<line.index(before: line.endIndex)])
        guard !target.contains(where: \.isWhitespace), !target.contains(")"),
              let url = URL(string: target),
              ["file", "http", "https"].contains(url.scheme?.lowercased() ?? "") else { return nil }
        let alt = String(line[line.index(line.startIndex, offsetBy: 2)..<altEnd.lowerBound])
        return .image(alt: alt, url: url)
    }

    private static func isRule(_ line: String) -> Bool {
        let marks = line.filter { !$0.isWhitespace }
        guard marks.count >= 3, let first = marks.first, "-*_".contains(first) else { return false }
        return marks.allSatisfy { $0 == first }
    }

    private static func startsBlock(_ line: String) -> Bool {
        let trimmed = line.trimmingCharacters(in: .whitespaces)
        return heading(in: trimmed) != nil || listItem(in: trimmed) != nil
            || trimmed.hasPrefix(">") || isRule(trimmed) || MarkdownFence.opening(line) != nil
    }

    private static func isTableSeparator(_ cell: String) -> Bool {
        let dashes = cell.trimmingCharacters(in: CharacterSet(charactersIn: ":"))
        return dashes.count >= 3 && dashes.allSatisfy { $0 == "-" }
    }

    /// Pipes escaped with a backslash or inside code spans belong to the cell.
    private static func tableCells(_ line: String) -> [String]? {
        let trimmed = line.trimmingCharacters(in: .whitespaces)
        guard !trimmed.isEmpty else { return nil }
        var cells: [String] = []
        var cell = ""
        var ticks = 0
        var hasPipe = false
        var index = trimmed.startIndex
        while index < trimmed.endIndex {
            let character = trimmed[index]
            let next = trimmed.index(after: index)
            if character == "\\", next < trimmed.endIndex, trimmed[next] == "|" {
                cell += "|"
                index = trimmed.index(after: next)
                continue
            }
            if character == "`" {
                let run = trimmed[index...].prefix { $0 == "`" }
                if ticks == 0 { ticks = run.count }
                else if ticks == run.count { ticks = 0 }
                cell += run
                index = trimmed.index(index, offsetBy: run.count)
                continue
            }
            if character == "|", ticks == 0 {
                hasPipe = true
                cells.append(cell.trimmingCharacters(in: .whitespaces))
                cell = ""
            } else {
                cell.append(character)
            }
            index = next
        }
        cells.append(cell.trimmingCharacters(in: .whitespaces))
        if trimmed.hasPrefix("|") { cells.removeFirst() }
        if trimmed.hasSuffix("|"), cells.last == "" { cells.removeLast() }
        return hasPipe && !cells.isEmpty ? cells : nil
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
            let body = String(line.dropFirst(2))
            if body.hasPrefix("[ ] ") { return .listItem(marker: "☐", text: String(body.dropFirst(4))) }
            if body.hasPrefix("[x] ") || body.hasPrefix("[X] ") {
                return .listItem(marker: "☑", text: String(body.dropFirst(4)))
            }
            return .listItem(marker: "•", text: body)
        }
        let digits = line.prefix { $0.isNumber }
        guard !digits.isEmpty, digits.count <= 3 else { return nil }
        let rest = line.dropFirst(digits.count)
        guard rest.hasPrefix(". ") || rest.hasPrefix(") ") else { return nil }
        return .listItem(marker: "\(digits).", text: String(rest.dropFirst(2)))
    }
}

/// Shared by Markdown and thinking parsing so literal tags inside fences stay literal.
struct MarkdownFence {
    let marker: Character
    let count: Int
    let language: String?

    static func opening(_ line: String) -> MarkdownFence? {
        let trimmed = line.trimmingCharacters(in: .whitespaces)
        guard let marker = trimmed.first, marker == "`" || marker == "~" else { return nil }
        let count = trimmed.prefix { $0 == marker }.count
        guard count >= 3 else { return nil }
        let info = trimmed.dropFirst(count).trimmingCharacters(in: .whitespaces)
        guard marker != "`" || !info.contains("`") else { return nil }
        return MarkdownFence(marker: marker, count: count, language: info.isEmpty ? nil : info)
    }

    func closes(_ line: String) -> Bool {
        let trimmed = line.trimmingCharacters(in: .whitespaces)
        let run = trimmed.prefix { $0 == marker }.count
        return run >= count && trimmed.dropFirst(run).allSatisfy(\.isWhitespace)
    }
}
