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

/// The toasts centered at the top of the chat, newest last. The stack covers
/// the window so it draws above panels, and is inset by the sidebar and the
/// preview beside the chat so it centers on the chat itself.
struct ToastStack: View {
    @ObservedObject var model: DesktopModel
    /// The width taken beside the chat on each side.
    var leadingInset: CGFloat = 0
    var trailingInset: CGFloat = 0

    var body: some View {
        VStack(alignment: .center, spacing: 8) {
            ForEach(model.toasts) { toast in
                ToastView(toast: toast) { model.open(toast) } dismiss: { model.dismissToast(toast.id) }
                    .transition(.asymmetric(
                        insertion: .move(edge: .top).combined(with: .opacity),
                        // A swiped toast is already on its way out; closing
                        // fades it where it is rather than sliding it back up.
                        removal: .opacity.combined(with: .scale(scale: 0.96))
                    ))
            }
        }
        // Below the chat header.
        .padding(.top, 56)
        .padding(.horizontal, 16)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .padding(.leading, leadingInset)
        .padding(.trailing, trailingInset)
        .animation(.snappy(duration: 0.25), value: model.toasts)
    }
}

/// How a dragged toast moves and whether letting go dismisses it. Toasts sit
/// centered at the top of the chat, so they leave upward or to either side; a
/// drag downward follows the pointer with resistance and springs back.
enum ToastSwipe {
    /// How far a toast must travel to go when released.
    static let distance: CGFloat = 80
    /// How far a quick flick must be heading when released.
    static let flick: CGFloat = 180

    /// Where the toast is drawn for a drag of `translation`.
    static func offset(for translation: CGSize) -> CGSize {
        CGSize(
            width: translation.width,
            height: translation.height < 0 ? translation.height : resisted(translation.height)
        )
    }

    /// Whether letting go of a drag dismisses the toast: far enough out, or a
    /// flick heading out, upward or to either side.
    static func dismisses(translation: CGSize, predicted: CGSize) -> Bool {
        abs(translation.width) >= distance || -translation.height >= distance
            || abs(predicted.width) >= flick || -predicted.height >= flick
    }

    /// The direction it leaves in once dismissed, along the drag's main axis.
    static func exit(for translation: CGSize) -> CGSize {
        if -translation.height > abs(translation.width) { return CGSize(width: 0, height: -160) }
        return CGSize(width: translation.width < 0 ? -420 : 420, height: 0)
    }

    /// How faded the toast is for its drawn `offset`, more as it heads out.
    static func fade(for offset: CGSize) -> Double {
        Double(min(max(abs(offset.width), -offset.height, 0) / (distance * 3), 0.6))
    }

    /// A drag against the way out moves a fraction of the pointer, to a limit.
    private static func resisted(_ value: CGFloat) -> CGFloat {
        let limit: CGFloat = 24
        let magnitude = limit * (1 - 1 / (abs(value) / limit + 1))
        return value < 0 ? -magnitude : magnitude
    }
}

private struct ToastView: View {
    let toast: Toast
    let open: () -> Void
    let dismiss: () -> Void
    @State private var isHovered = false
    @State private var offset: CGSize = .zero

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
            if toast.style != .progress {
                // Always laid out, so showing it on hover fades it in place
                // instead of widening the toast.
                Button(action: dismiss) {
                    Image(systemName: "xmark")
                        .font(.caption.weight(.bold))
                        .foregroundStyle(.secondary)
                        .frame(width: 16, height: 16)
                        .contentShape(.rect)
                }
                .buttonStyle(.plain)
                .help("Dismiss")
                .opacity(isHovered ? 1 : 0)
                .scaleEffect(isHovered ? 1 : 0.8)
                .allowsHitTesting(isHovered)
                .accessibilityHidden(!isHovered)
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .glassSurface(in: .rect(cornerRadius: 14))
        .contentShape(.rect)
        .offset(offset)
        .opacity(1 - ToastSwipe.fade(for: offset))
        .onTapGesture { if opens { open() } else { dismiss() } }
        .gesture(swipe)
        .onHover { hovering in
            withAnimation(.easeOut(duration: 0.15)) { isHovered = hovering }
        }
        .help(opens ? "Open" : "")
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(opens ? .isButton : [])
        .accessibilityAction(named: "Dismiss", dismiss)
    }

    /// Drag up or to either side to dismiss; anything short of that springs back.
    private var swipe: some Gesture {
        DragGesture(minimumDistance: 6)
            .onChanged { value in
                offset = ToastSwipe.offset(for: value.translation)
            }
            .onEnded { value in
                if ToastSwipe.dismisses(translation: value.translation, predicted: value.predictedEndTranslation) {
                    withAnimation(.easeOut(duration: 0.18)) { offset = ToastSwipe.exit(for: value.translation) }
                    dismiss()
                } else {
                    withAnimation(.spring(response: 0.3, dampingFraction: 0.75)) { offset = .zero }
                }
            }
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
    @Environment(\.closePanel) private var dismiss

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
        .frame(maxWidth: 700, maxHeight: 560)
    }
}
