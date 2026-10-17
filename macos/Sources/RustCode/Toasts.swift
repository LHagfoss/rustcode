import RustCodeKit
import SwiftUI

/// A short message over the chat that goes away on its own: a confirmation,
/// a command's answer, or news from a chat that is not the one shown.
struct Toast: Identifiable, Equatable {
    enum Style: Equatable { case info, success, warning, progress }

    let id = UUID()
    var style: Style = .info
    var systemImage: String
    var title: String
    var message: String?
    /// The chat a click opens, when the toast is about another chat.
    var session: String?
    /// A command's full answer, opened in a sheet from the toast.
    var result: CommandResult?

    /// How long it stays up; a toast in progress stays until replaced.
    var lifetime: Duration? {
        switch style {
        case .progress: nil
        case .warning: .seconds(6)
        default: .seconds(result == nil && session == nil ? 2.5 : 5)
        }
    }

    static func == (lhs: Toast, rhs: Toast) -> Bool { lhs.id == rhs.id }
}

/// The toasts in the top-right corner of the chat, newest last.
struct ToastStack: View {
    @ObservedObject var model: DesktopModel

    var body: some View {
        VStack(alignment: .trailing, spacing: 8) {
            ForEach(model.toasts) { toast in
                ToastView(toast: toast) { model.open(toast) } dismiss: { model.dismissToast(toast.id) }
                    .transition(.move(edge: .top).combined(with: .opacity))
            }
        }
        // Below the chat header.
        .padding(.top, 56)
        .padding(.trailing, 16)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topTrailing)
        .animation(.snappy(duration: 0.25), value: model.toasts)
    }
}

private struct ToastView: View {
    let toast: Toast
    let open: () -> Void
    let dismiss: () -> Void
    @State private var isHovered = false

    private var opens: Bool { toast.session != nil || toast.result != nil }

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            icon.frame(width: 18, height: 18)
            VStack(alignment: .leading, spacing: 2) {
                Text(toast.title).font(.callout.weight(.semibold)).lineLimit(1)
                if let message = toast.message, !message.isEmpty {
                    Text(message).font(.callout).foregroundStyle(.secondary).lineLimit(3)
                }
                if toast.result != nil {
                    Text("Click to see all").font(.caption).foregroundStyle(.tertiary)
                }
            }
            .frame(maxWidth: 280, alignment: .leading)
            if isHovered, toast.style != .progress {
                Button(action: dismiss) {
                    Image(systemName: "xmark").font(.caption.weight(.bold)).foregroundStyle(.secondary)
                }
                .buttonStyle(.plain)
                .help("Dismiss")
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .background(.regularMaterial, in: .rect(cornerRadius: 14))
        .overlay { RoundedRectangle(cornerRadius: 14).strokeBorder(.separator) }
        .shadow(color: .black.opacity(0.18), radius: 10, y: 4)
        .contentShape(.rect)
        .onTapGesture { if opens { open() } else { dismiss() } }
        .onHover { isHovered = $0 }
        .help(opens ? "Open" : "")
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(opens ? .isButton : [])
    }

    @ViewBuilder
    private var icon: some View {
        switch toast.style {
        case .progress: ProgressView().controlSize(.small)
        case .success: Image(systemName: toast.systemImage).foregroundStyle(.green)
        case .warning: Image(systemName: toast.systemImage).foregroundStyle(Color.attention)
        case .info: Image(systemName: toast.systemImage).foregroundStyle(Color.brand)
        }
    }
}

/// A host command's full answer in a sheet, with its Markdown rendered.
struct CommandResultSheet: View {
    let result: CommandResult
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 10) {
                Image(systemName: "terminal").foregroundStyle(Color.brand)
                VStack(alignment: .leading, spacing: 2) {
                    Text(result.title).font(.headline)
                    slashCommandText(result.input).font(.caption).foregroundStyle(.secondary)
                }
                Spacer()
                Button("Copy", systemImage: "doc.on.doc") { Clipboard.copy(result.content) }
                Button("Done") { dismiss() }.keyboardShortcut(.defaultAction)
            }
            .padding(16)
            Divider()
            ScrollView {
                MarkdownText(text: result.content)
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(16)
            }
        }
        .frame(minWidth: 520, idealWidth: 620, minHeight: 280, idealHeight: 480)
    }
}
