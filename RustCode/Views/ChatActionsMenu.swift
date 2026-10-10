import SwiftUI

enum ChatAction { case newSession, commands, folder, camera, photos, files, dismissKeyboard, stop }

/// A compact glass menu anchored to the composer's attachment button.
struct ChatActionsMenu: View {
    let hadKeyboard: Bool
    let isRunning: Bool
    let select: (ChatAction) -> Void
    @ScaledMetric(relativeTo: .body) private var rowHeight = 54.0

    var body: some View {
        ScrollView {
            VStack(spacing: 2) {
                action("Camera", symbol: "camera", .camera)
                action("Photos", symbol: "photo", .photos)
                action("Files", symbol: "paperclip", .files)
                Divider().padding(.vertical, 6).padding(.horizontal, 8)
                action("New Session", symbol: "square.and.pencil", .newSession)
                action("Slash Commands", symbol: "command", .commands)
                action("Choose Folder", symbol: "folder", .folder)
                if isRunning { action("Stop the running turn", symbol: "stop.circle", .stop) }
                if hadKeyboard { action("Dismiss Keyboard", symbol: "keyboard.chevron.compact.down", .dismissKeyboard) }
            }
            .padding(12)
        }
        .frame(width: 280, height: min(rowHeight * Double(6 + (hadKeyboard ? 1 : 0) + (isRunning ? 1 : 0)) + 52, 480))
        .scrollIndicators(.hidden)
        .foregroundStyle(.primary)
        // The native popover supplies Liquid Glass on iOS 26 and material earlier.
        .accessibilityIdentifier("chatActionsMenu")
    }

    private func action(_ title: String, symbol: String, _ action: ChatAction) -> some View {
        Button { select(action) } label: {
            HStack(spacing: 14) {
                Image(systemName: symbol)
                    .font(.title3)
                    .frame(width: 40, height: 40)
                    .background(.fill.tertiary, in: .circle)
                    .accessibilityHidden(true)
                Text(title).font(.body)
                Spacer(minLength: 0)
            }
            .frame(minHeight: rowHeight)
            .padding(.horizontal, 8)
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .accessibilityLabel(title)
    }
}
