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

    @Test func onlyCompleteCodesCanConnect() {
        #expect(!PairingCodeInput.isComplete("1234-"))
        #expect(PairingCodeInput.isComplete("0123-4567"))
        #expect(PairingCodeInput.isComplete("01234567"))
        #expect(!PairingCodeInput.isComplete("1234-56789"))
        #expect(!PairingCodeInput.isComplete("abcd-5678"))
    }
}
