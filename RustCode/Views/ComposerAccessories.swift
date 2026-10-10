import RustCodeRemote
import SwiftUI

/// Attention stays inside the composer, next to the action that resolves it.
struct AttentionAccessory: View {
    @Environment(RemoteStore.self) private var store
    let symbol: String
    let title: String
    let detail: String
    let action: String
    var cancelLabel = "Cancel questions"
    var isPending = false
    var cancel: (() -> Void)? = nil
    let perform: () -> Void

    var body: some View {
        HStack(spacing: 4) {
            Button(action: perform) {
                HStack(spacing: 10) {
                    Image(systemName: symbol).foregroundStyle(.attention)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(title).font(.caption.weight(.semibold)).foregroundStyle(.secondary)
                        Text(detail).font(.subheadline).lineLimit(2).multilineTextAlignment(.leading)
                    }
                    Spacer(minLength: 4)
                    Text(action).font(.subheadline.weight(.semibold)).foregroundStyle(.tint)
                }
                .frame(minHeight: 44).contentShape(.rect)
            }
            .buttonStyle(.plain)
            .accessibilityElement(children: .ignore)
            .accessibilityLabel("\(title). \(detail)")
            .accessibilityHint(action)
            if let cancel {
                Button(role: .destructive, action: cancel) {
                    Group {
                        if isPending { ProgressView().controlSize(.small) }
                        else { Image(systemName: "xmark") }
                    }
                    .frame(width: 44, height: 44).contentShape(.rect)
                }
                .buttonStyle(.plain)
                .disabled(isPending || store.connection != .connected)
                .accessibilityLabel(cancelLabel)
            }
        }
        .padding(.leading, 16).padding(.trailing, 6).padding(.vertical, 8)
        Divider().padding(.horizontal, 14)
    }
}

/// One row remains in the composer regardless of how many outcomes need review.
struct RequestStatusSummary: View {
    let requests: [TrackedRequest]
    let review: () -> Void

    private var isSending: Bool { requests.allSatisfy { $0.phase == .sending } }

    var body: some View {
        Button(action: review) {
            HStack(spacing: 8) {
                if isSending { ProgressView().controlSize(.small) }
                else { Image(systemName: "exclamationmark.circle").foregroundStyle(.attention) }
                Text(isSending ? "Sending…" : "\(requests.count) \(requests.count == 1 ? "action needs" : "actions need") attention")
                    .lineLimit(1)
                Spacer(minLength: 4)
                Text("Review").foregroundStyle(.tint)
                Image(systemName: "chevron.right").foregroundStyle(.secondary)
            }
            .font(.footnote).padding(.horizontal, 16).frame(minHeight: 44).contentShape(.rect)
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier("requestSummary")
    }
}

struct RequestStatusSheet: View {
    @Environment(RemoteStore.self) private var store
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            List {
                if store.visibleRequests.isEmpty {
                    Label("No notices to review", systemImage: "checkmark.circle")
                } else {
                    Section {
                        ForEach(store.visibleRequests) { RequestStatusRow(request: $0) }
                    } footer: {
                        Text("The app checks these actions with your Mac. It never sends an uncertain action again automatically.")
                    }
                }
            }
            .navigationTitle("Pending Actions")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } } }
        }
        .presentationDetents([.medium, .large])
    }
}

struct RequestStatusRow: View {
    @Environment(RemoteStore.self) private var store
    let request: TrackedRequest

    private var action: String {
        switch request.kind {
        case .cancel: "Stop turn"
        case .cancelQuestion: "Cancel questions"
        case .command: "Run command"
        case .answer: "Answer question"
        case .approval: "Resolve approval"
        case .settings: "Change model or effort"
        case .submit: "Send message"
        case .queue: "Queue message"
        case .steer: "Steer turn"
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(action).font(.headline)
            Text(status).font(.subheadline).foregroundStyle(.secondary)
            if let text = request.text { Text(text).font(.footnote).lineLimit(3) }
            HStack {
                if request.isUncertain {
                    Button("Check status") { Task { await store.checkStatus(of: request.id) } }
                        .disabled(store.connection != .connected)
                }
                Spacer()
                if request.phase != .sending {
                    Button("Dismiss notice") { store.dismissRequest(request.id) }
                }
            }
            .font(.footnote).frame(minHeight: 44)
        }
        .padding(.vertical, 4)
    }

    private var status: String {
        switch request.phase {
        case .sending: "Sending to your Mac…"
        case .settled(.received): "Received by your Mac; waiting for confirmation."
        case .settled(.unknown): "Confirmation hasn’t arrived. Check status with your Mac."
        case .notSent(let reason): "Not sent. \(reason)"
        case .settled(.rejected(let reason)): reason.message
        case .settled(.applied): "Confirmed."
        }
    }
}
