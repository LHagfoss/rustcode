import Testing
@testable import RustCodeRemote

@Suite struct PairingInputTests {
    @Test func formatsTypingAndPastingWithoutLosingLeadingZeros() {
        #expect(PairingCodeInput.formatted("123") == "123")
        #expect(PairingCodeInput.formatted("1234") == "1234")
        #expect(PairingCodeInput.formatted("12345") == "1234-5")
        #expect(PairingCodeInput.formatted("01234567") == "0123-4567")
        #expect(PairingCodeInput.formatted(" 0123-4567\n") == "0123-4567")
        #expect(PairingCodeInput.formatted("123456789") == "1234-5678")
        #expect(PairingCodeInput.formatted("١٢٣٤abcd") == "")
    }

    @Test func backspaceCanPassTheInsertedSeparator() {
        let trailing = PairingCodeInput.replacing("1234-", range: 4..<5, with: "")
        #expect(trailing.text == "123")
        #expect(trailing.caret == 3)
        let middle = PairingCodeInput.replacing("1234-5678", range: 4..<5, with: "")
        #expect(middle.text == "1235-678")
        #expect(middle.caret == 3)
        #expect(PairingCodeInput.replacing("1234-5678", range: 8..<9, with: "").text == "1234-567")
        #expect(PairingCodeInput.replacing("1234-", range: 0..<5, with: "").text == "")
    }

    @Test func pastingAndReplacingSelectionsKeepsEveryDigitAndCaret() {
        let pasted = PairingCodeInput.replacing("1234-5678", range: 0..<9, with: "12345678")
        #expect(pasted.text == "1234-5678")
        #expect(pasted.caret == 9)
        let replaced = PairingCodeInput.replacing("1234-5678", range: 1..<3, with: "09")
        #expect(replaced.text == "1094-5678")
        #expect(replaced.caret == 3)
    }

    @Test func typingKeepsTheCaretBesideTheDigitJustEntered() {
        var text = ""
        var caret = 0
        for (digit, expected) in zip("01234567", ["0", "01", "012", "0123", "0123-4", "0123-45", "0123-456", "0123-4567"]) {
            let edit = PairingCodeInput.replacing(text, range: caret..<caret, with: String(digit))
            (text, caret) = (edit.text, edit.caret)
            #expect(text == expected)
            #expect(caret == expected.count)
        }
        // A ninth digit is dropped without moving the caret off the end.
        let full = PairingCodeInput.replacing(text, range: 9..<9, with: "8")
        #expect(full.text == "0123-4567")
        #expect(full.caret == 9)
        // Inserting in the middle leaves the caret after the new digit.
        let middle = PairingCodeInput.replacing("1234-56", range: 2..<2, with: "9")
        #expect(middle.text == "1293-456")
        #expect(middle.caret == 3)
        // Crossing the separator puts the caret after it.
        let crossing = PairingCodeInput.replacing("1234-56", range: 3..<3, with: "9")
        #expect(crossing.text == "1239-456")
        #expect(crossing.caret == 5)
    }

    @Test func rangesAreUTF16OffsetsAndNeverTrap() {
        // Text UIKit autofilled without formatting: "😀" is two UTF-16 units.
        let emoji = PairingCodeInput.replacing("😀12", range: 4..<4, with: "3")
        #expect(emoji.text == "123")
        #expect(emoji.caret == 3)
        let deleted = PairingCodeInput.replacing("😀12", range: 3..<4, with: "")
        #expect(deleted.text == "1")
        #expect(deleted.caret == 1)
        // A stale selection past the end is clamped, not ignored.
        let clamped = PairingCodeInput.replacing("12", range: 5..<9, with: "3")
        #expect(clamped.text == "123")
        #expect(clamped.caret == 3)
        let reversed = PairingCodeInput.replacing("1234-5678", range: -3..<2, with: "")
        #expect(reversed.text == "3456-78")
        #expect(reversed.caret == 0)
        // Non-digits in a paste never count towards the caret.
        let pasted = PairingCodeInput.replacing("", range: 0..<0, with: " 01-23 ")
        #expect(pasted.text == "0123")
        #expect(pasted.caret == 4)
    }

    @Test func onlyCompleteCodesCanConnect() {
        #expect(!PairingCodeInput.isComplete("1234-"))
        #expect(PairingCodeInput.isComplete("0123-4567"))
        #expect(PairingCodeInput.isComplete("01234567"))
        #expect(!PairingCodeInput.isComplete("1234-56789"))
        #expect(!PairingCodeInput.isComplete("abcd-5678"))
    }
}
