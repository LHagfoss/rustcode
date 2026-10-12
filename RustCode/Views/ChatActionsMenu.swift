import SwiftUI

enum ChatAction { case newSession, commands, folder, camera, photos, files, dismissKeyboard, stop }

/// A compact glass menu anchored to the composer's attachment button.
struct ChatActionsMenu: View {
    let hadKeyboard: Bool
    let isRunning: Bool
    let select: (ChatAction) -> Void
    @ScaledMetric(relativeTo: .footnote) private var rowHeight = 40.0
    @State private var showingMore = false

    private var rowCount: Int {
        showingMore ? 4 + (isRunning ? 1 : 0) + (hadKeyboard ? 1 : 0) : 4
    }

    var body: some View {
        ScrollView {
            VStack(spacing: 0) {
                if showingMore {
                    Button {
                        withAnimation(.snappy(duration: 0.18)) { showingMore = false }
                    } label: {
                        row("Back", symbol: "chevron.left")
                    }
                    .buttonStyle(.borderless)
                    .accessibilityLabel("Back to attachments")
                    action("New Session", symbol: "square.and.pencil", .newSession)
                    action("Slash Commands", symbol: "command", .commands)
                    action("New Session in Folder", symbol: "folder.badge.plus", .folder)
                    if isRunning { action("Stop the running turn", symbol: "stop.circle", .stop) }
                    if hadKeyboard { action("Dismiss Keyboard", symbol: "keyboard.chevron.compact.down", .dismissKeyboard) }
                } else {
                    action("Camera", symbol: "camera", .camera)
                    action("Photos", symbol: "photo", .photos)
                    action("Files", symbol: "paperclip", .files)
                    Button {
                        withAnimation(.snappy(duration: 0.18)) { showingMore = true }
                    } label: {
                        row("More", symbol: "ellipsis")
                    }
                    .buttonStyle(.borderless)
                    .accessibilityLabel("More")
                }
            }
            .padding(.horizontal, 8)
            .padding(.vertical, 6)
        }
        .frame(width: 180, height: min(rowHeight * Double(rowCount) + 18, 300))
        .scrollIndicators(.hidden)
        .foregroundStyle(.primary)
        // The native popover supplies Liquid Glass on iOS 26 and material earlier.
        .accessibilityIdentifier("chatActionsMenu")
    }

    private func action(_ title: String, symbol: String, _ action: ChatAction) -> some View {
        Button { select(action) } label: {
            row(title, symbol: symbol)
        }
        .buttonStyle(.borderless)
        .accessibilityLabel(title)
    }

    private func row(_ title: String, symbol: String) -> some View {
        HStack(spacing: 10) {
            Image(systemName: symbol)
                .font(.subheadline.weight(.medium))
                .frame(width: 28, height: 28)
                .background(.fill.tertiary, in: .circle)
                .accessibilityHidden(true)
            Text(title)
                .font(.subheadline)
                .lineLimit(1)
                .minimumScaleFactor(0.85)
            Spacer(minLength: 0)
        }
        .frame(minHeight: rowHeight)
        .padding(.horizontal, 4)
        .contentShape(.rect)
    }
}
