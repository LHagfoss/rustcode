import Foundation

/// A host slash command's answer, shown natively instead of in the chat. The
/// engine answers `**Title**` and then the output.
public struct CommandResult: Identifiable, Equatable, Sendable {
    public let id = UUID()
    public let input: String
    public let output: String

    public init(input: String, output: String) {
        self.input = input
        self.output = output
    }

    private var titleRange: Range<String.Index>? {
        guard output.hasPrefix("**"), let end = output.dropFirst(2).range(of: "**") else { return nil }
        return output.index(output.startIndex, offsetBy: 2)..<end.lowerBound
    }

    public var title: String { titleRange.map { String(output[$0]) } ?? input }

    public var content: String {
        let body = titleRange.map { output[output.index($0.upperBound, offsetBy: 2)...] } ?? output[...]
        return body.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    /// One short line fits a toast; anything longer gets a sheet.
    public var isBrief: Bool {
        !content.contains("\n") && content.count <= 140
    }
}
