import Foundation

/// Human-readable user messages; the stored prompt and copy action retain the original text.
public enum ImageAttachmentPresentation {
    /// Returns upload IDs only for validated image markers outside Markdown code fences.
    public static func attachmentIDs(in raw: String) -> [String] {
        let remote = try! NSRegularExpression(
            pattern: #"^!\[image\]\(file://[^\r\n]+/remote-images/[0-9a-f]{64}/([A-Za-z0-9_-]{1,64})\.(png|jpg)\)$"#
        )
        let demo = try! NSRegularExpression(pattern: #"^!\[image\]\(file:///demo/([A-Za-z0-9_-]{1,64})\.png\)$"#)
        var fence: String?
        return raw.components(separatedBy: "\n").compactMap { line in
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            if trimmed.hasPrefix("```") || trimmed.hasPrefix("~~~") {
                let marker = String(trimmed.prefix(3))
                if fence == marker { fence = nil } else if fence == nil { fence = marker }
                return nil
            }
            guard fence == nil, line == trimmed else { return nil }
            let range = NSRange(line.startIndex..<line.endIndex, in: line)
            let expression = remote.firstMatch(in: line, range: range) ?? demo.firstMatch(in: line, range: range)
            guard let expression, let idRange = Range(expression.range(at: 1), in: line) else { return nil }
            return String(line[idRange])
        }
    }

    /// Removes validated image-only lines while preserving all other transcript text.
    public static func textWithoutAttachments(_ raw: String) -> String {
        let ids = attachmentIDs(in: raw)
        guard !ids.isEmpty else { return raw }
        let remotePattern = #"^!\[image\]\(file://[^\r\n]+/remote-images/[0-9a-f]{64}/[A-Za-z0-9_-]{1,64}\.(png|jpg)\)$"#
        let demoPattern = #"^!\[image\]\(file:///demo/[A-Za-z0-9_-]{1,64}\.png\)$"#
        var fence: String?
        return raw.components(separatedBy: "\n").filter { line in
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            if trimmed.hasPrefix("```") || trimmed.hasPrefix("~~~") {
                let marker = String(trimmed.prefix(3))
                if fence == marker { fence = nil } else if fence == nil { fence = marker }
                return true
            }
            guard fence == nil, line == trimmed else { return true }
            return line.range(of: remotePattern, options: .regularExpression) == nil
                && line.range(of: demoPattern, options: .regularExpression) == nil
        }.joined(separator: "\n").trimmingCharacters(in: .whitespacesAndNewlines)
    }

    public static func displayText(_ raw: String) -> String {
        let pattern = #"^!\[image\]\(file://[^\r\n]+/remote-images/[0-9a-f]{64}/[A-Za-z0-9_-]{1,64}\.(png|jpg)\)$"#
        let demoPattern = #"^!\[image\]\(file:///demo/[A-Za-z0-9_-]{1,64}\.png\)$"#
        var fence: String?
        return raw.components(separatedBy: "\n").map { line in
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            if trimmed.hasPrefix("```") || trimmed.hasPrefix("~~~") {
                let marker = String(trimmed.prefix(3))
                if fence == marker { fence = nil } else if fence == nil { fence = marker }
                return line
            }
            guard fence == nil, line == trimmed,
                  line.range(of: pattern, options: .regularExpression) != nil
                    || line.range(of: demoPattern, options: .regularExpression) != nil else { return line }
            return "Image attachment"
        }.joined(separator: "\n")
    }
}
