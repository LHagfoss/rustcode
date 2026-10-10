import RustCodeRemote
import SwiftUI
import UIKit

/// Keep the system field, number pad and selection behavior while inserting
/// the code separator. Editing intent comes from UITextField's actual range.
struct PairingCodeField: UIViewRepresentable {
    @Binding var text: String
    @Binding var isFocused: Bool

    func makeCoordinator() -> Coordinator { Coordinator(self) }

    func makeUIView(context: Context) -> CodeTextField {
        let field = CodeTextField()
        field.delegate = context.coordinator
        field.placeholder = "1234-5678"
        field.accessibilityLabel = "Pairing code"
        field.keyboardType = .numberPad
        field.textContentType = .oneTimeCode
        field.autocorrectionType = .no
        field.font = .preferredFont(forTextStyle: .body)
        field.adjustsFontForContentSizeCategory = true
        field.setContentHuggingPriority(.defaultLow, for: .horizontal)
        field.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        let toolbar = UIToolbar()
        let done = UIBarButtonItem(title: "Done", style: .done, target: context.coordinator, action: #selector(Coordinator.done))
        done.accessibilityLabel = "Dismiss pairing keyboard"
        toolbar.items = [.flexibleSpace(), done]
        toolbar.sizeToFit()
        field.inputAccessoryView = toolbar
        return field
    }

    func updateUIView(_ field: CodeTextField, context: Context) {
        context.coordinator.parent = self
        if field.text != text { field.text = text }
        field.wantsFocus = isFocused
        if isFocused, field.window != nil, !field.isFirstResponder {
            field.becomeFirstResponder()
        } else if !isFocused, field.isFirstResponder {
            field.resignFirstResponder()
        }
    }

    static func dismantleUIView(_ field: CodeTextField, coordinator: Coordinator) {
        // Removing an old row must not clear the replacement row's focus request.
        field.delegate = nil
        field.wantsFocus = false
        field.resignFirstResponder()
    }

    final class CodeTextField: UITextField {
        var wantsFocus = false

        override func didMoveToWindow() {
            super.didMoveToWindow()
            // Selecting a nearby Mac changes the form's section header. Its
            // replacement field must wait until it is attached before focusing.
            if wantsFocus, window != nil, !isFirstResponder { becomeFirstResponder() }
        }
    }

    final class Coordinator: NSObject, UITextFieldDelegate {
        var parent: PairingCodeField
        init(_ parent: PairingCodeField) { self.parent = parent }

        func textFieldDidBeginEditing(_ textField: UITextField) { parent.isFocused = true }
        func textFieldDidEndEditing(_ textField: UITextField) { parent.isFocused = false }
        @objc func done() { parent.isFocused = false }

        func textField(_ field: UITextField, shouldChangeCharactersIn range: NSRange, replacementString string: String) -> Bool {
            let edit = PairingCodeInput.replacing(field.text ?? "", range: range.location..<(range.location + range.length), with: string)
            field.text = edit.text
            if let caret = field.position(from: field.beginningOfDocument, offset: edit.caret) {
                field.selectedTextRange = field.textRange(from: caret, to: caret)
            }
            parent.text = edit.text
            return false
        }
    }
}
