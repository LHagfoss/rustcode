import SwiftUI

enum ChatAction: Equatable { case newSession, commands, folder, copySessionID, changes, camera, photos, files, dismissKeyboard, stop }

/// Locate the attachment button in the transcript overlay without changing layout.
struct ChatActionsAnchor: PreferenceKey {
    static var defaultValue: Anchor<CGRect>? { nil }
    static func reduce(value: inout Anchor<CGRect>?, nextValue: () -> Anchor<CGRect>?) {
        value = nextValue() ?? value
    }
}

/// A glass menu anchored to the composer's attachment button.
struct ChatActionsMenu: View {
    let hadKeyboard: Bool
    let isRunning: Bool
    /// The host can show the thread's changes.
    var canReviewChanges = false
    let select: (ChatAction) -> Void
    @ScaledMetric(relativeTo: .body) private var rowHeight = 60.0
    @ScaledMetric(relativeTo: .body) private var iconSize = 38.0
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var showingMore = false

    private var rowCount: Int {
        showingMore ? 5 + (canReviewChanges ? 1 : 0) + (isRunning ? 1 : 0) + (hadKeyboard ? 1 : 0) : 4
    }

    var body: some View {
        ScrollView {
            VStack(spacing: 0) {
                if showingMore {
                    Button {
                        withAnimation(reduceMotion ? nil : .snappy(duration: 0.18)) { showingMore = false }
                    } label: {
                        row("Back", symbol: "chevron.left")
                    }
                    .buttonStyle(ComposerPressStyle())
                    .accessibilityLabel("Back to attachments")
                    action("New Thread", symbol: "square.and.pencil", .newSession)
                    action("Slash Commands", symbol: "command", .commands)
                    action("New Thread in Folder", symbol: "folder.badge.plus", .folder)
                    if canReviewChanges { action("Review Changes", symbol: "plus.forwardslash.minus", .changes) }
                    action("Copy Thread ID", symbol: "number", .copySessionID)
                    if isRunning { action("Stop the running turn", symbol: "stop.circle", .stop) }
                    if hadKeyboard { action("Dismiss Keyboard", symbol: "keyboard.chevron.compact.down", .dismissKeyboard) }
                } else {
                    action("Camera", symbol: "camera", .camera)
                    action("Photos", symbol: "photo", .photos)
                    action("Files", symbol: "paperclip", .files)
                    Button {
                        withAnimation(reduceMotion ? nil : .snappy(duration: 0.18)) { showingMore = true }
                    } label: {
                        row("More", symbol: "ellipsis")
                    }
                    .buttonStyle(ComposerPressStyle())
                    .accessibilityLabel("More")
                }
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 12)
        }
        .frame(width: 248, height: min(rowHeight * Double(rowCount) + 24, 420))
        .scrollIndicators(.hidden)
        .foregroundStyle(.primary)
        .glassSurface(in: .rect(cornerRadius: 36))
        .accessibilityIdentifier("chatActionsMenu")
    }

    private func action(_ title: String, symbol: String, _ action: ChatAction) -> some View {
        Button { select(action) } label: {
            row(title, symbol: symbol)
        }
        .buttonStyle(ComposerPressStyle())
        .accessibilityLabel(title)
    }

    private func row(_ title: String, symbol: String) -> some View {
        HStack(spacing: 14) {
            Image(systemName: symbol)
                .font(.body.weight(.medium))
                .frame(width: iconSize, height: iconSize)
                .background(.fill.tertiary, in: .circle)
                .accessibilityHidden(true)
            Text(title)
                .font(.body)
                .lineLimit(2)
            Spacer(minLength: 0)
        }
        .frame(minHeight: rowHeight)
        .contentShape(.rect)
    }
}
