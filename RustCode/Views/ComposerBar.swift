import RustCodeRemote
import SwiftUI

struct ComposerBar: View {
    @Environment(RemoteStore.self) private var store
    let isRunning: Bool
    let isEnabled: Bool

    @State private var text = ""
    @FocusState private var isFocused: Bool

    private var trimmed: String {
        text.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    var body: some View {
        HStack(alignment: .bottom, spacing: 8) {
            TextField(isRunning ? "Queue a message" : "Message", text: $text, axis: .vertical)
                .lineLimit(1...6)
                .focused($isFocused)
                .padding(.horizontal, 16)
                .padding(.vertical, 10)
                .background(.fill.tertiary, in: .rect(cornerRadius: 22))
                .accessibilityLabel(isRunning ? "Message to queue" : "Message")

            if isRunning {
                Button {
                    Task { await store.cancelTurn() }
                } label: {
                    Image(systemName: "stop.circle.fill")
                        .font(.system(size: 34))
                }
                .accessibilityLabel("Stop the running turn")
            }

            if isRunning && !trimmed.isEmpty {
                // While a turn runs, a tap queues; the menu offers steering instead.
                Menu {
                    Button("Queue for Next Turn", systemImage: "text.line.last.and.arrowtriangle.forward") {
                        send(.queue)
                    }
                    Button("Steer Running Turn", systemImage: "arrow.triangle.turn.up.right.diamond") {
                        send(.steer)
                    }
                } label: {
                    sendIcon
                } primaryAction: {
                    send(.queue)
                }
                .accessibilityLabel("Queue message")
                .accessibilityHint("Touch and hold for steering")
            } else if !isRunning {
                Button {
                    send(.automatic)
                } label: {
                    sendIcon
                }
                .disabled(trimmed.isEmpty)
                .accessibilityLabel("Send message")
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .disabled(!isEnabled)
    }

    private var sendIcon: some View {
        Image(systemName: "arrow.up.circle.fill")
            .font(.system(size: 34))
    }

    private func send(_ mode: SendMode) {
        let message = trimmed
        guard !message.isEmpty else { return }
        text = ""
        Task {
            // Give the text back if it certainly did not reach the session.
            if await !store.send(message, mode: mode), text.isEmpty {
                text = message
            }
        }
    }
}
