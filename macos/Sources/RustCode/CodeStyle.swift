import AppKit
import RustCodeKit
import SwiftUI

/// Colours code the way Xcode's default themes do, in light and dark.
enum CodeStyle {
    static func color(_ kind: SyntaxHighlighter.Kind) -> Color {
        switch kind {
        case .keyword: adaptive(light: 0x9B2393, dark: 0xFC5FA3)
        case .string: adaptive(light: 0xC41A16, dark: 0xFC6A5D)
        case .comment: adaptive(light: 0x5D6C79, dark: 0x7F8C98)
        case .number: adaptive(light: 0x1C00CF, dark: 0xD0BF69)
        case .type: adaptive(light: 0x0B4F79, dark: 0x5DD8FF)
        }
    }

    /// `lines` joined by newlines, each highlighted; a nil highlighter leaves
    /// them plain. A block comment carries from one line to the next.
    static func attributed(_ lines: [String], highlighter: SyntaxHighlighter?) -> AttributedString {
        var result = AttributedString()
        var inBlockComment = false
        for (index, line) in lines.enumerated() {
            if index > 0 { result += AttributedString("\n") }
            result += attributed(line, highlighter: highlighter, inBlockComment: &inBlockComment)
        }
        return result
    }

    static func attributed(_ line: String, highlighter: SyntaxHighlighter?, inBlockComment: inout Bool) -> AttributedString {
        var text = AttributedString(line)
        guard let highlighter else { return text }
        for token in highlighter.tokens(in: line, inBlockComment: &inBlockComment) {
            let lower = line.distance(from: line.startIndex, to: token.range.lowerBound)
            let length = line.distance(from: token.range.lowerBound, to: token.range.upperBound)
            let start = text.index(text.startIndex, offsetByCharacters: lower)
            let end = text.index(start, offsetByCharacters: length)
            text[start..<end].foregroundColor = color(token.kind)
        }
        return text
    }

    private static func adaptive(light: UInt32, dark: UInt32) -> Color {
        Color(nsColor: NSColor(name: nil) { appearance in
            let hex = appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua ? dark : light
            return NSColor(srgbRed: CGFloat((hex >> 16) & 0xFF) / 255, green: CGFloat((hex >> 8) & 0xFF) / 255,
                           blue: CGFloat(hex & 0xFF) / 255, alpha: 1)
        })
    }
}
