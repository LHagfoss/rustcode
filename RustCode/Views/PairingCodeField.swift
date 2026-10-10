import RustCodeRemote
import SwiftUI
import UIKit

/// Keep the system field, number pad and selection behavior while inserting
/// the code separator. Editing intent comes from UITextField's actual range.
struct PairingCodeField: View {
    @Binding var text: String
    @Binding var isFocused: Bool
    var body: some View { PairingTextField(text: $text, isFocused: $isFocused, isCode: true) }
}

struct PairingAddressField: View {
    @Binding var text: String
    @Binding var isFocused: Bool
    var onNext: () -> Void
    var body: some View { PairingTextField(text: $text, isFocused: $isFocused, isCode: false, onNext: onNext) }
}

private struct PairingTextField: UIViewRepresentable {
    @Binding var text: String
    @Binding var isFocused: Bool

    var isCode: Bool
    var onNext: () -> Void = {}

    func makeCoordinator() -> Coordinator { Coordinator(self) }

    func makeUIView(context: Context) -> FocusTextField {
        let field = FocusTextField()
        field.delegate = context.coordinator
        field.placeholder = isCode ? "1234-5678" : "192.168.1.20:17879"
        field.accessibilityLabel = isCode ? "Pairing code" : "Host address and port"
        field.keyboardType = isCode ? .numberPad : .URL
        field.returnKeyType = .next
        field.autocapitalizationType = .none
        field.addTarget(context.coordinator, action: #selector(Coordinator.changed(_:)), for: .editingChanged)
        field.textContentType = isCode ? .oneTimeCode : .URL
        field.autocorrectionType = .no
        field.font = .preferredFont(forTextStyle: .body)
        field.adjustsFontForContentSizeCategory = true
        field.setContentHuggingPriority(.defaultLow, for: .horizontal)
        field.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        let toolbar = UIToolbar()
        toolbar.accessibilityIdentifier = "Pairing keyboard toolbar"
        let appearance = UIToolbarAppearance()
        appearance.configureWithDefaultBackground()
        toolbar.standardAppearance = appearance
        toolbar.scrollEdgeAppearance = appearance
        let done = UIBarButtonItem(title: "Done", style: .plain, target: context.coordinator, action: #selector(Coordinator.done))
        done.accessibilityLabel = "Dismiss pairing keyboard"
        toolbar.items = [.flexibleSpace(), done]
        toolbar.sizeToFit()
        field.inputAccessoryView = toolbar
        return field
    }

    func updateUIView(_ field: FocusTextField, context: Context) {
        context.coordinator.parent = self
        if field.text != text { field.text = text }
        field.wantsFocus = isFocused
        if isFocused, field.window != nil, !field.isFirstResponder {
            field.becomeFirstResponder()
        } else if !isFocused, field.isFirstResponder {
            field.resignFirstResponder()
        }
    }

    static func dismantleUIView(_ field: FocusTextField, coordinator: Coordinator) {
        // Removing an old row must not clear the replacement row's focus request.
        field.delegate = nil
        field.wantsFocus = false
        field.resignFirstResponder()
    }

    final class FocusTextField: UITextField {
        var wantsFocus = false

        override func didMoveToWindow() {
            super.didMoveToWindow()
            // Selecting a nearby Mac changes the form's section header. Its
            // replacement field must wait until it is attached before focusing.
            if wantsFocus, window != nil, !isFirstResponder { becomeFirstResponder() }
        }
    }

    final class Coordinator: NSObject, UITextFieldDelegate {
        var parent: PairingTextField
        init(_ parent: PairingTextField) { self.parent = parent }

        func textFieldDidBeginEditing(_ textField: UITextField) { parent.isFocused = true }
        func textFieldDidEndEditing(_ textField: UITextField) { parent.isFocused = false }
        @objc func done() { parent.isFocused = false }
        @objc func changed(_ field: UITextField) { parent.text = field.text ?? "" }
        func textFieldShouldReturn(_ textField: UITextField) -> Bool { parent.onNext(); return false }

        func textField(_ field: UITextField, shouldChangeCharactersIn range: NSRange, replacementString string: String) -> Bool {
            guard parent.isCode else { return true }
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
