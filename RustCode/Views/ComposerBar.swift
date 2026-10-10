import RustCodeRemote
import SwiftUI

/// The message field and its send or stop button, as one floating glass control.
struct ComposerBar<Accessory: View, Controls: View>: View {
    @Environment(RemoteStore.self) private var store
    let isRunning: Bool
    /// Whether the running turn accepts steering.
    var canSteer = true
    let isEnabled: Bool
    var runCommand: (String) async -> Bool
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
                .font(trimmed.hasPrefix("/") ? .body.monospaced() : .body)
                .overlay(alignment: .leading) {
                    if let completion {
                        (Text(text).foregroundColor(.clear) + Text(completion.dropFirst(text.count)).foregroundColor(.secondary.opacity(0.55)))
                            .font(.body.monospaced()).lineLimit(1)
                            .allowsHitTesting(false).accessibilityHidden(true)
                    }
                }
                .focused($isFocused)
                .frame(maxWidth: .infinity, minHeight: buttonSize)
                .background {
                    Color.clear.contentShape(.rect).onTapGesture { isFocused = true }
                }
                .accessibilityLabel(isRunning ? "Message to queue" : "Message")

            if let completion, isFocused {
                Button { text = completion } label: {
                    Image(systemName: "arrow.right.to.line").frame(width: buttonSize, height: buttonSize)
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Complete command")
                .accessibilityHint(completion)
            }

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

    private var matchingCommands: [SessionCommandChoice] {
        guard !trimmed.contains(where: \.isWhitespace) else { return [] }
        return commands.filter { commandText($0).hasPrefix(trimmed.lowercased()) }
    }

    private var completion: String? {
        guard isFocused, text == trimmed, trimmed.count > 1,
              let first = matchingCommands.first else { return nil }
        let value = commandText(first)
        return value.count > trimmed.count ? value : nil
    }

    private func commandText(_ choice: SessionCommandChoice) -> String {
        choice.name.hasPrefix("/") ? choice.name : "/" + choice.name
    }

    private var commandSuggestions: some View {
        VStack(alignment: .leading, spacing: 0) {
            if !matchingCommands.isEmpty {
                ScrollView {
                    LazyVStack(spacing: 0) {
                        ForEach(matchingCommands) { choice in
                            Button {
                                text = choice.insertion.isEmpty ? commandText(choice) : choice.insertion
                            } label: {
                                HStack(spacing: 10) {
                                    Text(commandText(choice)).font(.subheadline.monospaced())
                                    Text(choice.description).font(.caption).foregroundStyle(.secondary)
                                        .lineLimit(1)
                                    Spacer(minLength: 0)
                                    Image(systemName: choice.action == "terminal" ? "desktopcomputer" : "arrow.up.left")
                                        .font(.caption).foregroundStyle(.secondary)
                                }
                                .padding(.horizontal, 14).frame(minHeight: 44)
                                .contentShape(.rect)
                            }
                            .buttonStyle(.plain)
                            .accessibilityLabel(commandText(choice))
                            .accessibilityHint(choice.description)
                        }
                    }
                }
                .frame(height: min(CGFloat(matchingCommands.count) * 44, 176))
                .scrollDismissesKeyboard(.never)
                .accessibilityIdentifier("commandSuggestions")
                Divider().padding(.horizontal, 14)
            }
        }
    }

    private var commands: [SessionCommandChoice] {
        if let commands = store.attached?.commands, !commands.isEmpty { return commands }
        return [
            ("/help", "All commands", "execute"), ("/model", "Choose a model", "model_picker"),
            ("/new", "New session", "new_session"), ("/history", "Sessions", "session_picker"),
            ("/copy", "Copy last response", "copy_response"), ("/effort", "Thinking effort", "execute"),
            ("/cancel", "Stop the current task", "execute"), ("/status", "Session status", "execute"),
            ("/context", "Context window", "execute"), ("/usage", "Token usage", "execute"),
            ("/tasks", "Background tasks", "execute"), ("/mcp", "Configured MCP servers", "execute"),
            ("/title", "Rename session", "execute"), ("/info", "About RustCode", "execute"),
            ("/perf", "Performance", "execute")
        ].map { SessionCommandChoice(name: $0.0, description: $0.1, insertion: $0.0, action: $0.2) }
    }

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
                ? await runCommand(message)
                : await store.send(message, mode: mode)
            if !accepted, text.isEmpty {
                text = message
            }
        }
    }
}
