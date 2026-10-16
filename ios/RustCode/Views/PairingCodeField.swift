import RustCodeRemote
import SwiftUI
import UIKit

/// Keep the system field, number pad and selection behavior while inserting
/// the code separator. Editing intent comes from UITextField's actual range.
struct PairingCodeField: View {
    @Binding var text: String
    @Binding var isFocused: Bool
    var onPaste: (String) -> Void
    var body: some View { PairingTextField(text: $text, isFocused: $isFocused, isCode: true, onPaste: onPaste) }
}

struct PairingAddressField: View {
    @Binding var text: String
    @Binding var isFocused: Bool
    var onNext: () -> Void
    var onPaste: (String) -> Void
    var body: some View { PairingTextField(text: $text, isFocused: $isFocused, isCode: false, onNext: onNext, onPaste: onPaste) }
}

/// Geometry of the bar above the pairing keyboards.
enum PairingKeyboardBar {
    /// Glass bar items float on iOS 26, so they are lifted off the keyboard's top edge.
    static var floatingGap: CGFloat {
        if #available(iOS 26, *) { 10 } else { 0 }
    }
}

/// Pasting has three entry points, each needed for a different reach: the bar's
/// system paste control (the number pad has no paste key), the row's
/// `PasteButton` (works without opening a keyboard) and the edit menu on the
/// code. All three end in `onPaste`, the one place that fills the form and
/// connects once it is complete. Only an edit-menu paste into the address stays
/// a native insertion, because that is how part of an address is corrected.
private struct PairingTextField: UIViewRepresentable {
    @Binding var text: String
    @Binding var isFocused: Bool

    var isCode: Bool
    var onNext: () -> Void = {}
    var onPaste: (String) -> Void

    func makeCoordinator() -> Coordinator { Coordinator(self) }

    func makeUIView(context: Context) -> FocusTextField {
        let field = FocusTextField()
        field.delegate = context.coordinator
        if isCode {
            field.onMenuPaste = { [coordinator = context.coordinator] in coordinator.parent.onPaste($0) }
        }
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
        let done = UIBarButtonItem(systemItem: .done, primaryAction: UIAction { _ in
            context.coordinator.dismiss()
        })
        done.accessibilityIdentifier = "pairingKeyboardDone"
        done.accessibilityLabel = "Dismiss pairing keyboard"
        done.accessibilityHint = "Keeps what you entered"
        // The system paste control reads the clipboard without a permission prompt.
        let configuration = UIPasteControl.Configuration()
        configuration.displayMode = .iconAndLabel
        configuration.cornerStyle = .capsule
        let pasteControl = UIPasteControl(configuration: configuration)
        pasteControl.target = context.coordinator
        pasteControl.accessibilityIdentifier = "pairingKeyboardPaste"
        let paste = UIBarButtonItem(customView: pasteControl)
        if #available(iOS 26, *) { paste.hidesSharedBackground = true }
        toolbar.items = [paste, .flexibleSpace(), done]
        toolbar.sizeToFit()
        let accessory = UIView(frame: CGRect(x: 0, y: 0, width: toolbar.frame.width, height: toolbar.frame.height + PairingKeyboardBar.floatingGap))
        toolbar.autoresizingMask = .flexibleWidth
        accessory.addSubview(toolbar)
        field.inputAccessoryView = accessory
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
        var onMenuPaste: ((String) -> Void)?
        private(set) var isLeavingWindow = false

        override func willMove(toWindow newWindow: UIWindow?) {
            isLeavingWindow = newWindow == nil
            super.willMove(toWindow: newWindow)
        }

        override func paste(_ sender: Any?) {
            guard let onMenuPaste, let value = UIPasteboard.general.string else { return super.paste(sender) }
            onMenuPaste(value)
        }

        override func didMoveToWindow() {
            super.didMoveToWindow()
            isLeavingWindow = false
            // Selecting a nearby Mac changes the form's section header. Its
            // replacement field must wait until it is attached before focusing.
            if wantsFocus, window != nil, !isFirstResponder { becomeFirstResponder() }
        }
    }

    final class Coordinator: NSObject, UITextFieldDelegate, UIPasteConfigurationSupporting {
        var parent: PairingTextField
        var pasteConfiguration: UIPasteConfiguration? = UIPasteConfiguration(forAccepting: NSString.self)
        init(_ parent: PairingTextField) { self.parent = parent }

        func canPaste(_ itemProviders: [NSItemProvider]) -> Bool {
            itemProviders.contains { $0.canLoadObject(ofClass: NSString.self) }
        }

        func paste(itemProviders: [NSItemProvider]) {
            guard let provider = itemProviders.first(where: { $0.canLoadObject(ofClass: NSString.self) }) else { return }
            _ = provider.loadObject(ofClass: NSString.self) { value, _ in
                guard let value = value as? String else { return }
                Task { @MainActor [weak self] in self?.parent.onPaste(value) }
            }
        }

        func textFieldDidBeginEditing(_ textField: UITextField) { parent.isFocused = true }
        func textFieldDidEndEditing(_ textField: UITextField) {
            // A row the Form is replacing resigns as it leaves the window. That is
            // not the person ending editing, and must not cancel the focus its
            // replacement is about to take.
            if (textField as? FocusTextField)?.isLeavingWindow == true { return }
            // Removal can also resign before the window changes; decide once it has settled.
            DispatchQueue.main.async { [weak self, weak textField] in
                guard let self, let textField, textField.window != nil, !textField.isFirstResponder else { return }
                parent.isFocused = false
            }
        }
        func dismiss() { parent.isFocused = false }
        @objc func changed(_ field: UITextField) {
            // Autofill sets a one-time code without asking the delegate; format it too.
            if parent.isCode, let raw = field.text, raw != PairingCodeInput.formatted(raw) {
                field.text = PairingCodeInput.formatted(raw)
            }
            parent.text = field.text ?? ""
        }
        func textFieldShouldReturn(_ textField: UITextField) -> Bool { parent.onNext(); return false }

        func textField(_ field: UITextField, shouldChangeCharactersIn range: NSRange, replacementString string: String) -> Bool {
            guard parent.isCode else { return true }
            // NSRange and UITextPosition offsets are both UTF-16, as `replacing` expects.
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
