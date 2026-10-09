import RustCodeRemote
import SwiftUI

/// The message field and its send or stop button, as one floating glass control.
struct ComposerBar: View {
    @Environment(RemoteStore.self) private var store
    let isRunning: Bool
    /// Whether the running turn accepts steering.
    var canSteer = true
    let isEnabled: Bool

    @State private var text = ""
    @State private var sendCount = 0
    @State private var stopCount = 0
    @FocusState private var isFocused: Bool
    @ScaledMetric(relativeTo: .body) private var buttonSize: CGFloat = 34

    private let inset: CGFloat = 7

    private var trimmed: String {
        text.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private var shape: RoundedRectangle {
        // A capsule while the field is one line; the corners stay put as it grows.
        .rect(cornerRadius: (buttonSize + inset * 2) / 2)
    }

    var body: some View {
        HStack(alignment: .bottom, spacing: 8) {
            TextField(isRunning ? "Queue a message" : "Message", text: $text, axis: .vertical)
                .lineLimit(1...6)
                .focused($isFocused)
                .frame(minHeight: buttonSize)
                .accessibilityLabel(isRunning ? "Message to queue" : "Message")

            if isRunning {
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
        .contentShape(shape)
        .onTapGesture { isFocused = true }
        .glassSurface(in: shape)
        .animation(.snappy(duration: 0.2), value: isRunning)
        .animation(.snappy(duration: 0.2), value: trimmed.isEmpty)
        .disabled(!isEnabled)
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

    private var sendButton: some View {
        Button {
            send(.automatic)
        } label: {
            circle("arrow.up", isProminent: !trimmed.isEmpty)
        }
        .buttonStyle(.plain)
        .disabled(trimmed.isEmpty)
        .accessibilityLabel("Send message")
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
        .accessibilityLabel("Queue message")
        .accessibilityHint("Touch and hold for steering")
    }

    private var stopButton: some View {
        Button {
            stopCount += 1
            Task { await store.cancelTurn() }
        } label: {
            circle("stop.fill", fill: AnyShapeStyle(.red), foreground: .white)
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Stop the running turn")
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
            // A 44-point target without growing the control.
            .padding(5)
            .contentShape(.rect)
            .padding(-5)
    }

    private func send(_ mode: SendMode) {
        let message = trimmed
        guard !message.isEmpty else { return }
        text = ""
        sendCount += 1
        Task {
            // Give the text back if it certainly did not reach the session.
            if await !store.send(message, mode: mode), text.isEmpty {
                text = message
            }
        }
    }
}

/// A pending approval or question, attached to the top of the composer.
struct AttentionAccessory: View {
    let symbol: String
    let title: String
    let detail: String
    let action: String
    let perform: () -> Void

    var body: some View {
        Button(action: perform) {
            HStack(spacing: 8) {
                Image(systemName: symbol)
                    .font(.subheadline.weight(.semibold))
                    .foregroundStyle(.attention)
                    .symbolEffect(.pulse, options: .repeating)
                titleText
                Text(detail)
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                Text(action)
                    .font(.subheadline.weight(.semibold))
                    .foregroundStyle(.white)
                    .padding(.horizontal, 14)
                    .padding(.vertical, 7)
                    .background(.brandFill, in: .capsule)
                    .fixedSize()
            }
            .padding(.leading, 14)
            .padding(.trailing, 6)
            .padding(.vertical, 6)
            .contentShape(.capsule)
        }
        .buttonStyle(.plain)
        .glassSurface(in: .capsule, interactive: true)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(title). \(detail)")
        .accessibilityHint(action)
        .accessibilityAddTraits(.isButton)
    }

    private var titleText: some View {
        Text(title)
            .font(.subheadline.weight(.semibold))
            .lineLimit(1)
            .layoutPriority(1)
    }
}

/// A request whose outcome is not known to be applied. An uncertain one is never sent
/// again automatically: the user checks its status or dismisses it.
struct RequestStatusRow: View {
    @Environment(RemoteStore.self) private var store
    let request: TrackedRequest

    var body: some View {
        HStack(spacing: 10) {
            if request.phase == .sending {
                ProgressView().controlSize(.small)
            } else {
                Image(systemName: "questionmark.circle")
                    .foregroundStyle(.attention)
                    .accessibilityHidden(true)
            }
            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .font(.footnote.weight(.semibold))
                if let text = request.text {
                    Text(text)
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }
            }
            Spacer(minLength: 8)
            if request.isUncertain {
                Button("Check") {
                    Task { await store.checkStatus(of: request.id) }
                }
                .accessibilityLabel("Check whether the request was applied")
                Button("Dismiss", systemImage: "xmark") {
                    store.dismissRequest(request.id)
                }
                .labelStyle(.iconOnly)
            }
        }
        .font(.footnote)
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
        .glassSurface(in: .rect(cornerRadius: 20))
    }

    private var title: String {
        switch request.phase {
        case .sending: "Sending…"
        case .settled(.received): "Delivered, waiting for the session"
        case .settled(.unknown): "Not confirmed. Check the transcript before sending again."
        case .notSent(let reason): reason
        case .settled: "Sent"
        }
    }
}
