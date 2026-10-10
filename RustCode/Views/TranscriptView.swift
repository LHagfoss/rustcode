import RustCodeRemote
import SwiftUI

struct TranscriptView: View {
    @Environment(RemoteStore.self) private var store
    @State private var sheet: AttentionSheet?
    @State private var selectable: SelectableText?
    @State private var showsRequests = false
    @State private var showsHostUpdate = false
    @State private var showsModels = false
    @State private var hasAutoPresented = false
    @State private var followsLatest = true
    @State private var isDraggingTranscript = false
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
        .navigationTitle(store.selectedSession?.displayTitle ?? "Session")
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
                QuestionSheet(question: question).id(question.id)
            case .approval(let approval):
                ApprovalSheet(batchID: approval.batchID, initial: approval)
            }
        }
        .sheet(isPresented: $showsRequests) { RequestStatusSheet() }
        .alert("Update RustCode on your Mac", isPresented: $showsHostUpdate) {
            Button("OK", role: .cancel) {}
        } message: {
            Text("This host isn’t providing model or effort controls. Update RustCode on your Mac and reopen its session. Current versions update the background service automatically.")
        }
        .sheet(item: $selectable) { selectable in
            SelectTextSheet(text: selectable.text)
        }
        .onChange(of: store.attached?.pendingQuestion?.id) { reconcileSheet() }
        .onChange(of: store.attached?.pendingApproval?.batchID) { reconcileSheet() }
        .onAppear { reconcileSheet() }
        .onChange(of: store.selectedSession?.ref) { _, _ in
            followsLatest = true
            isDraggingTranscript = false
            hasAutoPresented = false
        }
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
                    ShareLink(item: snapshot.plainText(title: session.displayTitle)) {
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
        let footers = TimingPresentation.footerOwners(items: snapshot.items, lastTurn: snapshot.lastTurn)
        return ScrollViewReader { proxy in
            ScrollView {
                VStack(alignment: .leading, spacing: 18) {
                    if snapshot.historyCursor != nil {
                        Button {
                            followsLatest = false
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
                        TranscriptRow(item: item, footer: footers[item.id])
                    }
                    if isThinking(snapshot) {
                        StreamingIndicator()
                    }
                    SessionActivityView(snapshot: snapshot)
                    if let result = store.commandResult {
                        VStack(alignment: .leading, spacing: 12) {
                            HStack {
                                Text(result.title).font(.headline).accessibilityIdentifier("commandOutputTitle")
                                Spacer()
                                Button("Dismiss command output", systemImage: "xmark") { store.dismissCommandResult() }
                                    .labelStyle(.iconOnly).frame(width: 44, height: 44)
                            }
                            MarkdownText(text: result.output)
                                .contextMenu { TextActions(text: result.output, readableText: MarkdownText.plainText(result.output)) }
                        }
                        .id("command-output")
                    }
                    Color.clear.frame(height: 1).id("transcript-end")
                }
                .padding(.horizontal, 16)
                .padding(.vertical, 16)
                .frame(maxWidth: 760, alignment: .leading)
                .frame(maxWidth: .infinity)
            }
            // Opening the keyboard must not jump a reader to the end. Follow output
            // only while already at the tail and not editing a message.
            .defaultScrollAnchor(.bottom, for: .initialOffset)
            .defaultScrollAnchor(.top, for: .sizeChanges)
            .defaultScrollAnchor(.top, for: .alignment)
            .scrollDismissesKeyboard(.interactively)
            .onScrollPhaseChange { _, phase in
                isDraggingTranscript = phase == .interacting || phase == .decelerating
            }
            .onScrollGeometryChange(for: Bool.self) { geometry in
                geometry.contentOffset.y + geometry.containerSize.height - geometry.contentInsets.bottom >= geometry.contentSize.height - 60
            } action: { _, atBottom in
                if isDraggingTranscript { followsLatest = atBottom }
            }
            .simultaneousGesture(TapGesture().onEnded { isComposerFocused = false })
            .onChange(of: store.commandResult) { _, result in
                if result != nil { proxy.scrollTo("command-output", anchor: .top) }
            }
            .onChange(of: snapshot.items) { _, _ in
                if followsLatest && !isComposerFocused && !isDraggingTranscript {
                    proxy.scrollTo("transcript-end", anchor: .bottom)
                }
            }
            .accessibilityIdentifier("transcript")
            // Keep the editable field in a stable safe-area inset. safeAreaBar relocates
            // its content as the keyboard changes, which breaks the field's focus binding.
            .safeAreaInset(edge: .bottom, spacing: 0) {
                composerDock
            }
        }
    }

    /// The composer and whatever is waiting on the user, floating over the transcript as
    /// one group of glass.
    @ViewBuilder
    private var composerDock: some View {
        if let snapshot = store.attached {
            ComposerBar(
                isRunning: snapshot.isRunning,
                canSteer: snapshot.canSteer,
                isEnabled: store.connection == .connected,
                runCommand: runCommand,
                accessory: {
                    if let approval = snapshot.pendingApproval {
                        AttentionAccessory(
                            symbol: "hand.raised.fill", title: "Approval needed",
                            detail: approval.items.first?.title ?? "Review the action", action: "Review",
                            cancelLabel: "Deny action", isPending: store.isApprovalPending(approval.batchID),
                            cancel: { Task { await store.resolve(approval, decision: .deny) } }
                        ) { isComposerFocused = false; sheet = .approval(approval) }
                    } else if let question = snapshot.pendingQuestion {
                        AttentionAccessory(
                            symbol: "questionmark.bubble.fill",
                            title: question.chainLength > 1 ? "Question \(question.position) of \(question.chainLength)" : "Question",
                            detail: question.prompt, action: "Answer",
                            isPending: store.isQuestionCancellationPending(question.id),
                            cancel: { Task { await store.cancelQuestion(question) } }
                        ) { isComposerFocused = false; sheet = .question(question) }
                    }
                    if !store.visibleRequests.isEmpty {
                        RequestStatusSummary(requests: store.visibleRequests) { showsRequests = true }
                    }
                },
                controls: { sessionControls(snapshot) },
                isFocused: $isComposerFocused
            )
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
            .frame(maxWidth: 760)
            .frame(maxWidth: .infinity)
        }
    }

    @ViewBuilder
    private func sessionControls(_ snapshot: SessionSnapshot) -> some View {
        ViewThatFits(in: .horizontal) {
            controlsRow(snapshot)
            ScrollView(.horizontal) { controlsRow(snapshot) }.scrollIndicators(.hidden)
        }
    }

    private func controlsRow(_ snapshot: SessionSnapshot) -> some View {
        HStack(spacing: 8) {
            if let settings = snapshot.settings {
                let current = settings.models.first { $0.id == settings.selectedModel }
                Button { isComposerFocused = false; showsModels = true } label: {
                    Label(current?.model ?? (store.selectedSession?.model ?? "Model"), systemImage: "chevron.down")
                        .lineLimit(1)
                        .truncationMode(.middle)
                        .frame(maxWidth: 180, minHeight: 44)
                        .contentShape(.rect)
                }
                .popover(isPresented: $showsModels, arrowEdge: .bottom) {
                    ModelPicker().presentationCompactAdaptation(.popover)
                }
                .accessibilityLabel("Model")
                .accessibilityValue(current?.model ?? (store.selectedSession?.model ?? "Model"))

                Menu {
                    if !settings.canChange { Text("Available after the current turn stops") }
                    ForEach(current?.reasoningEfforts ?? [], id: \.self) { effort in
                        Button {
                            Task { await store.setSessionSettings(model: settings.selectedModel, reasoningEffort: effort) }
                        } label: {
                            if effort == settings.reasoningEffort {
                                Label(effort.capitalized, systemImage: "checkmark")
                            } else { Text(effort.capitalized) }
                        }
                        .disabled(!settings.canChange || store.connection != .connected || hasPendingSettings)
                    }
                } label: {
                    Label(settings.reasoningEffort.capitalized, systemImage: "brain").frame(minHeight: 44).contentShape(.rect)
                }
                .accessibilityLabel("Thinking effort")
                .accessibilityValue(settings.reasoningEffort)

            } else {
                Button { showsHostUpdate = true } label: {
                    Label(store.selectedSession?.model ?? "Model", systemImage: "chevron.down")
                        .lineLimit(1).truncationMode(.middle).frame(maxWidth: 180, minHeight: 44)
                        .contentShape(.rect)
                }
                .accessibilityLabel("Model")
                Button { showsHostUpdate = true } label: { Label("Effort", systemImage: "brain").frame(minHeight: 44).contentShape(.rect) }
                    .accessibilityLabel("Thinking effort")
            }
        }
        .font(.caption.weight(.medium))
        .controlSize(.regular)
        .buttonStyle(.plain)
        .fixedSize(horizontal: true, vertical: false)
        .frame(minHeight: 44)
        .padding(.horizontal, 6)
    }

    private func runCommand(_ command: String) async -> Bool {
        let parts = command.split(maxSplits: 1, whereSeparator: \.isWhitespace)
        let name = String(parts.first ?? "").lowercased()
        let argument = parts.count > 1 ? String(parts[1]).trimmingCharacters(in: .whitespacesAndNewlines) : ""
        let catalog = store.attached?.commands.first { choice in
            (choice.name.hasPrefix("/") ? choice.name : "/" + choice.name) == name
        }
        let action = catalog?.action ?? {
            switch name {
            case "/new": "new_session"
            case "/model", "/models": "model_picker"
            case "/history": "session_picker"
            case "/copy": "copy_response"
            default: "execute"
            }
        }()
        switch action {
        case "new_session":
            return await store.createSession(directory: argument.isEmpty ? store.selectedSession?.workspace : argument)
        case "model_picker" where argument.isEmpty:
            guard store.attached?.settings != nil else { showsHostUpdate = true; return false }
            showsModels = true
            return true
        case "session_picker":
            store.select(nil)
            return true
        case "copy_response":
            guard let response = store.attached?.items.reversed().compactMap({ item -> String? in
                if case .assistant(let text) = item.content { return text.text }
                return nil
            }).first else {
                store.notice = "There is no response to copy yet."
                return false
            }
            Clipboard.copy(response)
            return true
        case "terminal":
            store.notice = "\(name) needs the terminal on your Mac. \(catalog?.description ?? "")"
            return false
        case "execute", "model_picker":
            return await store.executeCommand(command)
        default:
            store.notice = "This command needs a newer version of the app. Update the app or use \(name) on your Mac."
            return false
        }
    }

    private var hasPendingSettings: Bool {
        store.hasPendingSettings
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
        case .question:
            if let next = snapshot?.pendingQuestion { sheet = .question(next) }
            else { sheet = nil }
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
            && snapshot.omitted.total == 0 && snapshot.activeTurnTiming == nil && unrepresentedLastTurn == nil
    }

    private var unrepresentedLastTurn: TurnTiming? {
        TimingPresentation.standaloneFooter(items: snapshot.items, lastTurn: snapshot.lastTurn)
    }

    var body: some View {
        if !isEmpty {
            VStack(alignment: .leading, spacing: 10) {
                if let turn = snapshot.activeTurnTiming {
                    Text(TimingPresentation.turnLabel(turn,
                        isWaiting: snapshot.pendingApproval != nil || snapshot.pendingQuestion != nil))
                        .font(.caption).foregroundStyle(.secondary)
                        .accessibilityIdentifier("turnTiming")
                }
                if let turn = unrepresentedLastTurn {
                    TurnFooter(timing: turn)
                }
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
        case .question: "question"
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
