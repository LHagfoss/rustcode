import Foundation

/// Human-readable user messages; the stored prompt and copy action retain the original text.
public enum ImageAttachmentPresentation {
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
