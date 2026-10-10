import RustCodeRemote
import SwiftUI

/// The message field and its send or stop button, as one floating glass control.
struct ComposerBar<Accessory: View, Controls: View>: View {
    @Environment(RemoteStore.self) private var store
    let isRunning: Bool
    /// Whether the running turn accepts steering.
    var canSteer = true
    let isEnabled: Bool
    @ViewBuilder var accessory: () -> Accessory
    @ViewBuilder var controls: () -> Controls

    @State private var text = ""
    @State private var sendCount = 0
    @State private var stopCount = 0
    @FocusState.Binding var isFocused: Bool
    @ScaledMetric(relativeTo: .body) private var buttonSize: CGFloat = 44

    private let inset: CGFloat = 7

    private var trimmed: String {
        text.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private var shape: RoundedRectangle {
        .rect(cornerRadius: 26)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            accessory()
            if isFocused && trimmed.hasPrefix("/") { commandSuggestions }
            inputRow
            controls().padding(.horizontal, 8).padding(.bottom, 4)
        }
        .contentShape(shape)
        .glassSurface(in: shape)
        .sensoryFeedback(.impact(weight: .light), trigger: sendCount)
        .sensoryFeedback(.stop, trigger: stopCount)
        #if DEBUG
        .task {
            let options = LaunchOptions.current
            if let draft = options.composerDraft { text = draft }
            if options.focusesComposer {
                try? await Task.sleep(for: .milliseconds(600))
                isFocused = true
            }
        }
        #endif
    }

    private var inputRow: some View {
        HStack(alignment: .bottom, spacing: 8) {
            TextField(isRunning ? "Queue a message" : "Message", text: $text, axis: .vertical)
                .lineLimit(1...6)
                .focused($isFocused)
                .frame(maxWidth: .infinity, minHeight: buttonSize)
                .background {
                    Color.clear.contentShape(.rect).onTapGesture { isFocused = true }
                }
                .accessibilityLabel(isRunning ? "Message to queue" : "Message")

            if isFocused {
                Button {
                    isFocused = false
                } label: {
                    Image(systemName: "keyboard.chevron.compact.down")
                        .foregroundStyle(.secondary)
                        .frame(width: buttonSize, height: buttonSize)
                        .contentShape(.rect)
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Dismiss keyboard")
            }

            if trimmed.hasPrefix("/") {
                sendButton
            } else if isRunning {
                if !trimmed.isEmpty {
                    queueButton
                        .transition(.scale.combined(with: .opacity))
                }
                stopButton
                    .transition(.scale.combined(with: .opacity))
            } else {
                sendButton
                    .transition(.scale.combined(with: .opacity))
            }
        }
        .padding(.leading, 18)
        .padding(.trailing, inset)
        .padding(.vertical, inset)
        .animation(.snappy(duration: 0.2), value: isRunning)
    }

    private var commandSuggestions: some View {
        VStack(alignment: .leading, spacing: 0) {
            if store.attached?.capabilities.contains("execute_command") != true {
                Text("Slash commands need an updated RustCode session and remote gateway.")
                    .font(.footnote).foregroundStyle(.secondary).padding(12)
            } else if !trimmed.contains(where: \.isWhitespace) {
                ForEach(suggestions.filter { $0.0.hasPrefix(trimmed.lowercased()) }.prefix(4), id: \.0) { command, detail in
                    Button {
                        text = command + (command == "/model" || command == "/effort" || command == "/title" ? " " : "")
                    } label: {
                        HStack {
                            Text(command).font(.body.monospaced())
                            Text(detail).font(.footnote).foregroundStyle(.secondary)
                            Spacer()
                            Image(systemName: "arrow.up.left").foregroundStyle(.secondary)
                        }
                        .padding(.horizontal, 16).frame(minHeight: 44)
                        .contentShape(.rect)
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel(command)
                    .accessibilityHint(detail)
                }
                Divider().padding(.horizontal, 14)
            }
        }
    }

    private let suggestions = [
        ("/help", "All commands"), ("/model", "View or change model"),
        ("/effort", "Thinking effort"), ("/cancel", "Stop the current task"),
        ("/status", "Session status"), ("/context", "Context window"),
        ("/usage", "Token usage"), ("/tasks", "Background tasks"),
        ("/mcp", "Configured MCP servers"), ("/title", "Rename session"),
        ("/info", "About RustCode"), ("/perf", "Performance")
    ]

    private var sendButton: some View {
        Button {
            send(.automatic)
        } label: {
            circle("arrow.up", isProminent: !trimmed.isEmpty)
        }
        .buttonStyle(.plain)
        .disabled(trimmed.isEmpty || !isEnabled)
        .accessibilityLabel(trimmed.hasPrefix("/") ? "Run command" : "Send message")
    }

    /// While a turn runs, a tap queues; the menu offers steering instead.
    private var queueButton: some View {
        Menu {
            Button("Queue for Next Turn", systemImage: "text.line.last.and.arrowtriangle.forward") {
                send(.queue)
            }
            if canSteer {
                Button("Steer Running Turn", systemImage: "arrow.triangle.turn.up.right.diamond") {
                    send(.steer)
                }
            }
        } label: {
            circle("arrow.up", isProminent: true)
        } primaryAction: {
            send(.queue)
        }
        .buttonStyle(.plain)
        .disabled(!isEnabled)
        .accessibilityLabel("Queue message")
        .accessibilityHint("Touch and hold for steering")
    }

    private var stopButton: some View {
        Button {
            stopCount += 1
            Task { await store.cancelTurn() }
        } label: {
            if store.hasPendingCancellation {
                ProgressView().frame(width: buttonSize, height: buttonSize)
            } else {
                circle("stop.fill", fill: AnyShapeStyle(.red), foreground: .white)
            }
        }
        .buttonStyle(.plain)
        .disabled(!isEnabled || store.hasPendingCancellation)
        .accessibilityLabel(store.hasPendingCancellation ? "Stopping the running turn" : "Stop the running turn")
    }

    private func circle(_ symbol: String, isProminent: Bool) -> some View {
        circle(
            symbol,
            fill: isProminent ? AnyShapeStyle(.brandFill) : AnyShapeStyle(.fill.secondary),
            foreground: isProminent ? .white : .secondary
        )
    }

    private func circle(_ symbol: String, fill: AnyShapeStyle, foreground: Color) -> some View {
        Image(systemName: symbol)
            .font(.subheadline.weight(.bold))
            .foregroundStyle(foreground)
            .frame(width: buttonSize, height: buttonSize)
            .background(fill, in: .circle)
            .contentShape(.rect)
    }

    private func send(_ mode: SendMode) {
        let message = trimmed
        guard !message.isEmpty else { return }
        isFocused = false
        text = ""
        sendCount += 1
        Task {
            // Give the text back if it certainly did not reach the session.
            let accepted = message.hasPrefix("/")
                ? await store.executeCommand(message)
                : await store.send(message, mode: mode)
            if !accepted, text.isEmpty {
                text = message
            }
        }
    }
}
