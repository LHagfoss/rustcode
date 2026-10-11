/// Presentation of the gateway's eight ASCII digits, retaining leading zeros.
public enum PairingCodeInput {
    public static func formatted(_ input: String) -> String {
        let digits = input.filter { $0.isASCII && $0.isNumber }.prefix(8)
        guard digits.count >= 5 else { return String(digits) }
        return String(digits.prefix(4)) + "-" + String(digits.dropFirst(4))
    }

    /// Use the actual selection and replacement to distinguish deleting the
    /// separator from pasting the same eight digits. `range` and the returned
    /// caret are UTF-16 offsets, the units UIKit selections are measured in, so
    /// text that is not yet formatted cannot shift the edit. A range reaching
    /// past the text is clamped to it.
    public static func replacing(_ current: String, range: Range<Int>, with replacement: String) -> (text: String, caret: Int) {
        let units = Array(current.utf16)
        let lower = min(max(range.lowerBound, 0), units.count)
        let upper = min(max(range.upperBound, lower), units.count)
        var start = lower
        // Backspacing the separator removes the digit before it instead.
        if replacement.isEmpty, upper - lower == 1, start > 0, units[start] == separator {
            start -= 1
        }
        let prefix = String(decoding: units[..<start], as: UTF16.self) + replacement
        let candidate = prefix + String(decoding: units[upper...], as: UTF16.self)
        let text = formatted(candidate)
        // The caret follows the digits typed so far. The result is ASCII, so
        // its character and UTF-16 offsets are the same.
        let digitsBeforeCaret = prefix.filter { $0.isASCII && $0.isNumber }.count
        let caret = digitsBeforeCaret + (digitsBeforeCaret >= 4 ? 1 : 0)
        return (text, min(caret, text.utf16.count))
    }

    private static let separator = UInt16(UInt8(ascii: "-"))

    public static func isComplete(_ input: String) -> Bool {
        let bare = input.filter { $0 != "-" }
        return bare.count == 8 && bare.allSatisfy { $0.isASCII && $0.isNumber }
    }
}
