import RustCodeRemote
import SwiftUI

struct TranscriptView: View {
    @Environment(RemoteStore.self) private var store
    @State private var sheet: AttentionSheet?
    @State private var hasAutoPresented = false
    var presentsAttentionOnAppear = false

    var body: some View {
        Group {
            if let snapshot = store.attached {
                transcript(snapshot)
            } else if store.connection == .connected {
                ProgressView("Loading session…")
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                ContentUnavailableView(
                    "Not Connected",
                    systemImage: "wifi.slash",
                    description: Text("The session loads again once the host is reachable.")
                )
            }
        }
        .navigationTitle(store.selectedSession?.title ?? "Session")
        .navigationBarTitleDisplayMode(.inline)
        .safeAreaInset(edge: .bottom, spacing: 0) {
            bottomBar
        }
        .sheet(item: $sheet) { sheet in
            switch sheet {
            case .question(let question):
                QuestionSheet(question: question)
            case .approval(let approval):
                ApprovalSheet(approval: approval)
            }
        }
        .onChange(of: store.attached?.pendingQuestion?.id) { reconcileSheet() }
        .onChange(of: store.attached?.pendingApproval?.batchID) { reconcileSheet() }
        .onAppear { reconcileSheet() }
    }

    private func transcript(_ snapshot: SessionSnapshot) -> some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                if snapshot.historyCursor != nil {
                    Button {
                        Task { await store.loadEarlierHistory() }
                    } label: {
                        if store.isLoadingHistory {
                            ProgressView()
                        } else {
                            Text("Load Earlier Messages")
                        }
                    }
                    .font(.footnote.weight(.medium))
                    .frame(maxWidth: .infinity)
                    .disabled(store.isLoadingHistory)
                }
                ForEach(snapshot.items) { item in
                    TranscriptRow(item: item)
                }
                if isThinking(snapshot) {
                    StreamingIndicator()
                }
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 16)
            .frame(maxWidth: 760, alignment: .leading)
            .frame(maxWidth: .infinity)
        }
        // Open at the latest message and follow new output, but keep a short transcript
        // at the top of the screen instead of pinning it to the composer.
        .defaultScrollAnchor(.bottom, for: .initialOffset)
        .defaultScrollAnchor(.bottom, for: .sizeChanges)
        .defaultScrollAnchor(.top, for: .alignment)
        .scrollDismissesKeyboard(.interactively)
    }

    private var bottomBar: some View {
        VStack(spacing: 0) {
            if let snapshot = store.attached {
                VStack(spacing: 8) {
                    ForEach(store.visibleRequests) { request in
                        RequestStatusRow(request: request)
                    }
                    if let approval = snapshot.pendingApproval {
                        AttentionBanner(
                            symbol: "hand.raised.fill",
                            title: "Approval needed",
                            detail: approval.items.first?.title ?? "Review the action",
                            action: "Review"
                        ) { sheet = .approval(approval) }
                    }
                    if let question = snapshot.pendingQuestion {
                        AttentionBanner(
                            symbol: "questionmark.bubble.fill",
                            title: "Question",
                            detail: question.prompt,
                            action: "Answer"
                        ) { sheet = .question(question) }
                    }
                }
                .padding(.horizontal, 12)
                .padding(.top, snapshot.pendingApproval != nil || snapshot.pendingQuestion != nil || !store.visibleRequests.isEmpty ? 10 : 0)

                ComposerBar(isRunning: snapshot.isRunning, isEnabled: store.connection == .connected)
            }
        }
        .frame(maxWidth: 760)
        .frame(maxWidth: .infinity)
        .background(.bar)
    }

    /// A turn is running but nothing on screen shows it: no streaming text, no running tool
    /// and nothing waiting for the user.
    private func isThinking(_ snapshot: SessionSnapshot) -> Bool {
        guard snapshot.isRunning, snapshot.pendingQuestion == nil, snapshot.pendingApproval == nil else { return false }
        switch snapshot.items.last?.content {
        case .assistant(let assistant): return !assistant.isStreaming
        case .tool(let tool): return tool.status != .running
        default: return true
        }
    }

    /// Closes a sheet whose question or approval is gone (answered here, in the terminal or
    /// on another device) and, when asked to by a launch argument, opens the pending one.
    private func reconcileSheet() {
        let snapshot = store.attached
        switch sheet {
        case .question(let question) where snapshot?.pendingQuestion?.id != question.id:
            sheet = nil
        case .approval(let approval) where snapshot?.pendingApproval?.batchID != approval.batchID:
            sheet = nil
        default:
            break
        }
        guard presentsAttentionOnAppear, !hasAutoPresented, sheet == nil else { return }
        if let approval = snapshot?.pendingApproval {
            hasAutoPresented = true
            sheet = .approval(approval)
        } else if let question = snapshot?.pendingQuestion {
            hasAutoPresented = true
            sheet = .question(question)
        }
    }
}

enum AttentionSheet: Identifiable {
    case question(PendingQuestion)
    case approval(PendingApproval)

    var id: String {
        switch self {
        case .question(let question): "question-\(question.id)"
        case .approval(let approval): "approval-\(approval.batchID)"
        }
    }
}

private struct AttentionBanner: View {
    let symbol: String
    let title: String
    let detail: String
    let action: String
    let perform: () -> Void

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: symbol)
                .font(.title3)
                .foregroundStyle(.orange)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .font(.subheadline.weight(.semibold))
                Text(detail)
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                    .lineLimit(2)
            }
            Spacer(minLength: 8)
            Button(action, action: perform)
                .buttonStyle(.borderedProminent)
                .buttonBorderShape(.capsule)
                .accessibilityLabel("\(action): \(title)")
        }
        .padding(12)
        .background(.fill.quaternary, in: .rect(cornerRadius: 16))
    }
}

/// A request whose outcome is not known to be applied. An uncertain one is never sent
/// again automatically: the user checks its status or dismisses it.
private struct RequestStatusRow: View {
    @Environment(RemoteStore.self) private var store
    let request: TrackedRequest

    var body: some View {
        HStack(spacing: 10) {
            if request.phase == .sending {
                ProgressView().controlSize(.small)
            } else {
                Image(systemName: "questionmark.circle")
                    .foregroundStyle(.orange)
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
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .background(.fill.quaternary, in: .rect(cornerRadius: 12))
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
