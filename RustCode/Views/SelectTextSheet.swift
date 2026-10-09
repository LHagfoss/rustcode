import SwiftUI
import UIKit

struct SelectableText: Identifiable {
    let id = UUID()
    let text: String
}

/// Lets a transcript row ask for its text to be shown for selection.
struct SelectTextAction: Sendable {
    var handler: @MainActor @Sendable (String) -> Void = { _ in }

    init(_ handler: @escaping @MainActor @Sendable (String) -> Void = { _ in }) {
        self.handler = handler
    }

    @MainActor
    func callAsFunction(_ text: String) {
        handler(text)
    }
}

extension EnvironmentValues {
    @Entry var selectText = SelectTextAction()
}

/// A message's text in a text view, where any part of it can be selected. A SwiftUI
/// `Text` only offers the whole text.
struct SelectTextSheet: View {
    @Environment(\.dismiss) private var dismiss
    let text: String

    var body: some View {
        NavigationStack {
            SelectableTextView(text: text)
                .ignoresSafeArea(edges: .bottom)
                .navigationTitle("Select Text")
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .topBarLeading) {
                        Button("Copy All", systemImage: "doc.on.doc") {
                            Clipboard.copy(text)
                            dismiss()
                        }
                    }
                    ToolbarItem(placement: .confirmationAction) {
                        Button("Done") { dismiss() }
                    }
                }
        }
        .presentationDetents([.medium, .large])
        // Opaque, so the transcript behind does not show through the text being selected.
        .presentationBackground(.background)
    }
}

private struct SelectableTextView: UIViewRepresentable {
    let text: String

    func makeUIView(context: Context) -> UITextView {
        let view = UITextView()
        view.isEditable = false
        view.isSelectable = true
        view.backgroundColor = .clear
        view.font = .preferredFont(forTextStyle: .body)
        view.adjustsFontForContentSizeCategory = true
        view.textContainerInset = UIEdgeInsets(top: 16, left: 16, bottom: 24, right: 16)
        view.alwaysBounceVertical = true
        return view
    }

    func updateUIView(_ view: UITextView, context: Context) {
        if view.text != text { view.text = text }
    }
}

/// Copy, Select Text and Share for a piece of transcript text.
struct TextActions: View {
    @Environment(\.selectText) private var selectText
    let text: String
    /// What Select Text shows, when it differs from what is copied.
    var readableText: String?

    var body: some View {
        Button("Copy", systemImage: "doc.on.doc") {
            Clipboard.copy(text)
        }
        Button("Select Text", systemImage: "character.cursor.ibeam") {
            selectText(readableText ?? text)
        }
        ShareLink(item: text) {
            Label("Share…", systemImage: "square.and.arrow.up")
        }
    }
}
