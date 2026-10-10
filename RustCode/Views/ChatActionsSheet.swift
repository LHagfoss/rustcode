import SwiftUI

enum ChatAction { case newSession, commands, folder, image, dismissKeyboard, stop }

struct ChatActionsSheet: View {
    let hadKeyboard: Bool
    let isRunning: Bool
    let select: (ChatAction) -> Void
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            List {
                action("New Session", symbol: "square.and.pencil", .newSession)
                action("Slash Commands", symbol: "command", .commands)
                action("Choose Folder", symbol: "folder", .folder)
                action("Add Image", symbol: "photo", .image)
                if isRunning { action("Stop the running turn", symbol: "stop.circle", .stop) }
                if hadKeyboard { action("Dismiss Keyboard", symbol: "keyboard.chevron.compact.down", .dismissKeyboard) }
            }
            .navigationTitle("Chat Actions")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } } }
        }
        .presentationDetents([.medium])
        .presentationDragIndicator(.visible)
    }

    private func action(_ title: String, symbol: String, _ action: ChatAction) -> some View {
        Button { select(action) } label: {
            Label(title, systemImage: symbol).frame(minHeight: 44)
        }
    }
}
