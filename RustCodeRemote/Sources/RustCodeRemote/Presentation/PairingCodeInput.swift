/// Presentation of the gateway's eight ASCII digits, retaining leading zeros.
public enum PairingCodeInput {
    public static func formatted(_ input: String) -> String {
        let digits = input.filter { $0.isASCII && $0.isNumber }.prefix(8)
        guard digits.count >= 5 else { return String(digits) }
        return String(digits.prefix(4)) + "-" + String(digits.dropFirst(4))
    }

    /// Use the actual selection and replacement to distinguish deleting the
    /// separator from pasting the same eight digits. Return the caret's position
    /// in the formatted ASCII text so editing never jumps to the end.
    public static func replacing(_ current: String, range: Range<Int>, with replacement: String) -> (text: String, caret: Int) {
        let characters = Array(current)
        guard range.lowerBound >= 0, range.upperBound <= characters.count else {
            return (current, characters.count)
        }
        var start = range.lowerBound
        if replacement.isEmpty, range.count == 1, start > 0, characters[start] == "-" {
            start -= 1
        }
        let prefix = String(characters[..<start]) + replacement
        let candidate = prefix + String(characters[range.upperBound...])
        let text = formatted(candidate)
        let digitsBeforeCaret = prefix.filter { $0.isASCII && $0.isNumber }.count
        let caret = digitsBeforeCaret + (digitsBeforeCaret >= 4 ? 1 : 0)
        return (text, min(caret, text.count))
    }

    public static func isComplete(_ input: String) -> Bool {
        let bare = input.filter { $0 != "-" }
        return bare.count == 8 && bare.allSatisfy { $0.isASCII && $0.isNumber }
    }
}
