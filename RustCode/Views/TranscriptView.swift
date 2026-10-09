import RustCodeRemote
import SwiftUI

struct TranscriptView: View {
    @Environment(RemoteStore.self) private var store
    @State private var sheet: AttentionSheet?
    @State private var selectable: SelectableText?
    @State private var hasAutoPresented = false
    @FocusState private var isComposerFocused: Bool
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
        .toolbar {
            if let session = store.selectedSession {
                ToolbarItem(placement: .topBarTrailing) {
                    sessionMenu(session)
                }
            }
        }
        .environment(\.selectText, SelectTextAction { selectable = SelectableText(text: $0) })
        .sheet(item: $sheet) { sheet in
            switch sheet {
            case .question(let question):
                QuestionSheet(question: question)
            case .approval(let approval):
                ApprovalSheet(batchID: approval.batchID, initial: approval)
            }
        }
        .sheet(item: $selectable) { selectable in
            SelectTextSheet(text: selectable.text)
        }
        .onChange(of: store.attached?.pendingQuestion?.id) { reconcileSheet() }
        .onChange(of: store.attached?.pendingApproval?.batchID) { reconcileSheet() }
        .onAppear { reconcileSheet() }
        #if DEBUG
        .task(id: store.attached?.items.isEmpty) {
            guard LaunchOptions.current.presentsSelectText, selectable == nil else { return }
            let text = store.attached?.items.lazy.compactMap { item -> String? in
                if case .assistant(let assistant) = item.content { assistant.text } else { nil }
            }.first
            if let text { selectable = SelectableText(text: MarkdownText.plainText(text)) }
        }
        #endif
    }

    private func sessionMenu(_ session: SessionSummary) -> some View {
        Menu {
            Section("\(session.workspace) · \(session.model)") {
                if let snapshot = store.attached, !snapshot.items.isEmpty {
                    ShareLink(item: snapshot.plainText(title: session.title)) {
                        Label("Share Transcript", systemImage: "square.and.arrow.up")
                    }
                }
                Button("Copy Workspace Path", systemImage: "folder") {
                    Clipboard.copy(session.workspace)
                }
            }
        } label: {
            Label("Session", systemImage: "ellipsis")
        }
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
                    .buttonStyle(.bordered)
                    .buttonBorderShape(.capsule)
                    .controlSize(.small)
                    .frame(maxWidth: .infinity)
                    .disabled(store.isLoadingHistory)
                }
                ForEach(snapshot.items) { item in
                    TranscriptRow(item: item)
                }
                if isThinking(snapshot) {
                    StreamingIndicator()
                }
                SessionActivityView(snapshot: snapshot)
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
        .scrollDismissesKeyboard(.immediately)
        .onScrollPhaseChange { _, phase in
            if phase == .interacting { isComposerFocused = false }
        }
        .accessibilityIdentifier("transcript")
        // Keep the editable field in a stable safe-area inset. safeAreaBar relocates
        // its content as the keyboard changes, which breaks the field's focus binding.
        .safeAreaInset(edge: .bottom, spacing: 0) {
            composerDock
        }
    }

    /// The composer and whatever is waiting on the user, floating over the transcript as
    /// one group of glass.
    @ViewBuilder
    private var composerDock: some View {
        if let snapshot = store.attached {
            GlassGroup(spacing: 8) {
                VStack(alignment: .leading, spacing: 8) {
                    ForEach(store.visibleRequests) { request in
                        RequestStatusRow(request: request)
                    }
                    if let approval = snapshot.pendingApproval {
                        AttentionAccessory(
                            symbol: "hand.raised.fill",
                            title: "Approval needed",
                            detail: approval.items.first?.title ?? "Review the action",
                            action: "Review"
                        ) { sheet = .approval(approval) }
                    }
                    if let question = snapshot.pendingQuestion {
                        AttentionAccessory(
                            symbol: "questionmark.bubble.fill",
                            title: "Question",
                            detail: question.prompt,
                            action: "Answer"
                        ) { sheet = .question(question) }
                    }
                    ComposerBar(
                        isRunning: snapshot.isRunning,
                        canSteer: snapshot.canSteer,
                        isEnabled: store.connection == .connected,
                        isFocused: $isComposerFocused
                    )
                }
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
            .frame(maxWidth: 760)
            .frame(maxWidth: .infinity)
        }
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

/// What the session is doing besides the conversation: sub-agents, background commands and
/// prompts it is holding. Only a snapshot carries the last two, so they are as fresh as the
/// last time the session was loaded.
private struct SessionActivityView: View {
    let snapshot: SessionSnapshot

    private var isEmpty: Bool {
        snapshot.subagents.isEmpty && snapshot.backgroundTasks.isEmpty && snapshot.pendingPrompts.isEmpty
            && snapshot.omitted.total == 0
    }

    var body: some View {
        if !isEmpty {
            VStack(alignment: .leading, spacing: 10) {
                ForEach(snapshot.subagents) { item in
                    if case .subagent(let subagent) = item.content {
                        SubagentRow(subagent: subagent)
                    }
                }
                ForEach(snapshot.backgroundTasks) { task in
                    activityRow(
                        symbol: "gearshape.2",
                        title: task.command,
                        detail: "Background task · \(task.elapsed.formatted(.units(allowed: [.hours, .minutes, .seconds], width: .abbreviated)))",
                        monospaced: true
                    )
                    .contextMenu {
                        Button("Copy Command", systemImage: "doc.on.doc") { Clipboard.copy(task.command) }
                    }
                }
                ForEach(Array(snapshot.pendingPrompts.enumerated()), id: \.offset) { _, prompt in
                    activityRow(
                        symbol: prompt.kind == .steer ? "arrow.triangle.turn.up.right.diamond" : "text.line.last.and.arrowtriangle.forward",
                        title: prompt.text,
                        detail: prompt.kind == .steer ? "Steering, waiting for the turn" : "Queued for the next turn",
                        monospaced: false
                    )
                    .contextMenu {
                        Button("Copy", systemImage: "doc.on.doc") { Clipboard.copy(prompt.text) }
                    }
                }
                if snapshot.omitted.total > 0 {
                    Text("\(snapshot.omitted.total) more not shown here. See the terminal for the full list.")
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                }
            }
        }
    }

    private func activityRow(symbol: String, title: String, detail: String, monospaced: Bool) -> some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: symbol)
                .foregroundStyle(.secondary)
                .frame(width: 22)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .font(monospaced ? .footnote.monospaced() : .footnote)
                    .lineLimit(2)
                Text(detail)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Spacer(minLength: 0)
        }
        .padding(12)
        .background(.fill.quaternary, in: .rect(cornerRadius: 14))
        .contentShape(.contextMenuPreview, .rect(cornerRadius: 14))
        .accessibilityElement(children: .combine)
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

extension SessionSnapshot {
    /// The transcript as plain text, for sharing.
    func plainText(title: String) -> String {
        var parts = [title]
        for item in items {
            switch item.content {
            case .user(let text): parts.append("You:\n\(text)")
            case .assistant(let assistant): parts.append(assistant.text)
            case .tool(let tool): parts.append("[\(tool.name)] \(tool.summary)")
            case .subagent(let subagent): parts.append("[\(subagent.name)] \(subagent.task)")
            case .notice(let text): parts.append(text)
            }
        }
        return parts.joined(separator: "\n\n")
    }
}
