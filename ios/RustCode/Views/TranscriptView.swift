import RustCodeRemote
import SwiftUI

private typealias NoticeToast = NoticePresentation.Notice

private struct LatestTranscriptButton: View {
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Label("Latest", systemImage: "arrow.down")
                .font(.subheadline.weight(.semibold))
                .padding(.horizontal, 16)
                .frame(minHeight: 44)
                .glassSurface(in: .capsule, interactive: true)
                // The whole pill, not just its glyphs, takes the tap.
                .contentShape(.capsule)
        }
        .buttonStyle(.plain)
        .contentShape(.capsule)
        .accessibilityHint("Scroll to the newest message")
    }
}

private struct NoticeToastStack: View {
    let toasts: [NoticeToast]
    let dismiss: (String) -> Void

    var body: some View {
        VStack(spacing: 8) {
            ForEach(toasts) { toast in
                HStack(alignment: .top, spacing: 10) {
                    Image(systemName: "info.circle.fill").foregroundStyle(.secondary)
                    Text(toast.text).font(.footnote).lineLimit(3)
                        .frame(maxWidth: .infinity, alignment: .leading)
                    Button("Dismiss", systemImage: "xmark") { dismiss(toast.id) }
                        .labelStyle(.iconOnly)
                        .accessibilityLabel("Dismiss notice")
                }
                .padding(12)
                .glassSurface(in: .rect(cornerRadius: 16), interactive: false)
                .contextMenu {
                    Button("Copy", systemImage: "doc.on.doc") { Clipboard.copy(toast.text) }
                }
                .task(id: toast.id) {
                    try? await Task.sleep(for: .seconds(5))
                    dismiss(toast.id)
                }
                .transition(.move(edge: .top).combined(with: .opacity))
            }
        }
        .padding(.horizontal, 16)
        .padding(.top, 8)
    }
}

struct TranscriptView: View {
    @Environment(RemoteStore.self) private var store
    @Environment(\.openSessionSidebar) private var openSessions
    @Environment(\.isChatObscured) private var isChatObscured
    @Environment(\.scenePhase) private var scenePhase
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var sheet: AttentionSheet?
    @State private var selectable: SelectableText?
    @State private var showsRequests = false
    @State private var showsHostUpdate = false
    @State private var showsModels = false
    @State private var showsChatActions = false
    @State private var nextChatAction: ChatAction?
    @State private var hasAutoPresented = false
    @State private var followsLatest = true
    @State private var isDraggingTranscript = false
    /// Session notices announced as a transient toast instead of inline rows.
    @State private var noticeToasts: [NoticeToast] = []
    /// Notice ids already seen for the attached session, so history does not
    /// announce itself when a session is opened.
    @State private var seenNoticeIDs: Set<String> = []
    @State private var hasSeededNotices = false
    @FocusState private var isComposerFocused: Bool
    var presentsAttentionOnAppear = false

    /// How close to the tail still counts as following the newest message. Just past
    /// this, a scroll up hands control to the reader and shows the Latest button.
    private static let followThreshold: CGFloat = 24

    var body: some View { stateManagedContent }

    private var navigationContent: some View {
        Group {
            if let failure = store.attachFailure, failure.sessionID == store.selectedSessionID {
                // The selection stays, so the draft does too; Back only shows the sessions.
                ContentUnavailableView {
                    Label(failure.title, systemImage: failure.systemImage)
                } description: {
                    Text(failure.message)
                } actions: {
                    Button("Try Again") { store.retryAttachment() }
                        .buttonStyle(.borderedProminent)
                    Button("Back") { openSessions() }
                }
            } else if let snapshot = store.attached {
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
        .workspaceSubtitle(store.selectedSession?.workspace)
        .navigationBarTitleDisplayMode(.inline)
        .navigationBarBackButtonHidden()
        .toolbar {
            ToolbarItem(placement: .topBarLeading) {
                Button("Show sessions", systemImage: "line.3.horizontal") { isComposerFocused = false; openSessions(toggle: true) }
                    .accessibilityHidden(isChatObscured)
            }
            if let session = store.selectedSession {
                ToolbarItem(placement: .topBarTrailing) {
                    sessionMenu(session).accessibilityHidden(isChatObscured)
                }
            }
        }
        .environment(\.selectText, SelectTextAction { selectable = SelectableText(text: $0) })
    }

    private var presentationContent: some View {
        navigationContent
        .sheet(item: $sheet) { sheet in
            switch sheet {
            case .question(let question):
                QuestionSheet(question: question).id(question.id)
            case .approval(let approval):
                ApprovalSheet(batchID: approval.batchID, initial: approval)
            }
        }
        .sheet(isPresented: $showsRequests) { RequestStatusSheet() }
        .sheet(isPresented: $showsModels) { ModelPicker() }
        .alert("Update RustCode on your Mac", isPresented: $showsHostUpdate) {
            Button("OK", role: .cancel) {}
        } message: {
            Text("This host isn’t providing model or effort controls. Update RustCode on your Mac and reopen its session. Current versions update the background service automatically.")
        }
        .sheet(item: $selectable) { selectable in
            SelectTextSheet(text: selectable.text)
        }
    }

    private var stateManagedContent: some View {
        presentationContent
        .onChange(of: store.attached?.pendingQuestion?.id) { reconcileSheet() }
        .onChange(of: store.attached?.pendingApproval?.batchID) { reconcileSheet() }
        .onAppear { reconcileSheet() }
        .sensoryFeedback(.selection, trigger: showsModels)
        .sensoryFeedback(trigger: store.chatFeedback) { _, feedback in
            chatSensoryFeedback(for: feedback)
        }
        .onChange(of: store.selectedSession?.ref) { _, _ in
            followsLatest = true
            isDraggingTranscript = false
            hasAutoPresented = false
            seenNoticeIDs = []
            hasSeededNotices = false
            noticeToasts = []
        }
        .onChange(of: noticeFingerprint) { _, notices in
            // A notice whose text is already on screen is the same event seen twice
            // (for example a live row replaced by its transcript record).
            let fresh = notices.filter { notice in
                !seenNoticeIDs.contains(notice.id) && !noticeToasts.contains { $0.text == notice.text }
            }
            seenNoticeIDs.formUnion(notices.map(\.id))
            guard hasSeededNotices else {
                // The first fingerprint after attaching is history: seed the
                // seen set silently instead of announcing old notices.
                hasSeededNotices = true
                return
            }
            guard !fresh.isEmpty else { return }
            withAnimation(.snappy(duration: 0.25)) {
                noticeToasts.append(contentsOf: fresh)
                if noticeToasts.count > 3 {
                    noticeToasts.removeFirst(noticeToasts.count - 3)
                }
            }
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

    /// Notices in the attached transcript as `(id, text)`, watched for arrivals.
    private var noticeFingerprint: [NoticeToast] {
        NoticePresentation.notices(in: store.attached?.items ?? [])
    }

    private func dismissToast(_ id: String) {
        withAnimation(.snappy(duration: 0.25)) {
            noticeToasts.removeAll { $0.id == id }
        }
    }

    private func chatSensoryFeedback(for feedback: ChatFeedback?) -> SensoryFeedback? {
        guard scenePhase == .active, !isChatObscured, store.connection == .connected,
              let feedback, feedback.session == store.attached?.session else { return nil }
        return switch feedback.kind {
        case .started: SensoryFeedback.impact(weight: .light, intensity: 0.6)
        case .completed: SensoryFeedback.success
        case .cancelled: SensoryFeedback.impact(weight: .medium, intensity: 0.8)
        case .failed: SensoryFeedback.error
        case .attention: SensoryFeedback.warning
        }
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
                // Not lazy: in the iOS 26.5 Simulator a LazyVStack here kept re-signalling
                // its prefetch after a send and a scroll, holding the main thread at 100%
                // for minutes. History arrives in pages of 50 and rows are equatable, so
                // laying out every row is the cheaper trade.
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
                        // Skip rebuilding a row whose content has not changed: only the
                        // streaming row recomputes, so a growing transcript stays smooth.
                        TranscriptRow(item: item, footer: footers[item.id]).equatable()
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
            .onScrollGeometryChange(for: CGFloat.self) { geometry in
                // How far the content reaches below the visible bottom edge. Positive
                // once the reader has scrolled up away from the newest message.
                geometry.contentSize.height - geometry.contentInsets.bottom
                    - (geometry.contentOffset.y + geometry.containerSize.height)
            } action: { _, distanceFromBottom in
                // Only a deliberate scroll changes whether we follow the tail, so a
                // growing answer never re-engages follow on its own. Any upward scroll
                // past a small threshold disengages at once and reveals the Latest
                // button; returning to the tail follows again.
                guard isDraggingTranscript else { return }
                followsLatest = distanceFromBottom <= Self.followThreshold
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
            // Once the first layout is in, scroll to the actual last row, in case the
            // initial bottom anchor was computed before every row had its size.
            .task(id: snapshot.session) {
                await Task.yield()
                proxy.scrollTo("transcript-end", anchor: .bottom)
            }
            .accessibilityIdentifier("transcript")
            .overlay(alignment: .bottomTrailing) {
                if !followsLatest {
                    LatestTranscriptButton {
                        withAnimation(.snappy(duration: 0.25)) {
                            followsLatest = true
                            proxy.scrollTo("transcript-end", anchor: .bottom)
                        }
                    }
                    .padding(.trailing, 16)
                    .padding(.bottom, 12)
                    .transition(.move(edge: .bottom).combined(with: .opacity))
                }
            }
            .overlay(alignment: .top) {
                NoticeToastStack(toasts: noticeToasts, dismiss: dismissToast)
            }
            // Keep the editable field in a stable safe-area inset. safeAreaBar relocates
            // its content as the keyboard changes, which breaks the field's focus binding.
            .safeAreaInset(edge: .bottom, spacing: 0) {
                composerDock
            }
            .overlayPreferenceValue(ChatActionsAnchor.self) { anchor in
                if let anchor {
                    GeometryReader { geometry in
                        let button = geometry[anchor]
                        // Keep the transition's container mounted while closing;
                        // removing the GeometryReader would skip the exit animation.
                        ZStack(alignment: .bottomLeading) {
                            if showsChatActions {
                                Color.clear
                                    .contentShape(.rect)
                                    .onTapGesture { showsChatActions = false }
                                    .accessibilityLabel("Dismiss chat actions")
                                    .accessibilityAddTraits(.isButton)
                                ChatActionsMenu(hadKeyboard: isComposerFocused, isRunning: snapshot.isRunning) { action in
                                    nextChatAction = action
                                    showsChatActions = false
                                }
                                .transition(reduceMotion ? .opacity : .scale(scale: 0.94, anchor: .bottomLeading).combined(with: .opacity))
                                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .bottomLeading)
                                .padding(.leading, max(12, button.minX - 8))
                                .padding(.bottom, max(8, geometry.size.height - button.minY + 8))
                            }
                        }
                        .animation(reduceMotion ? .easeOut(duration: 0.12) : .snappy(duration: 0.22), value: showsChatActions)
                        .allowsHitTesting(showsChatActions)
                    }
                }
            }
        }
    }

    /// The composer and whatever is waiting on the user, floating over the transcript as
    /// one group of glass.
    @ViewBuilder
    private var composerDock: some View {
        if let snapshot = store.attached {
            ComposerBar(
                sessionID: snapshot.session.sessionID,
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
                keepsExpanded: snapshot.pendingApproval != nil || snapshot.pendingQuestion != nil || !store.visibleRequests.isEmpty,
                showsActions: $showsChatActions,
                nextAction: $nextChatAction,
                isFocused: $isComposerFocused
            )
            .padding(12)
            .frame(maxWidth: 760)
            .frame(maxWidth: .infinity)
        }
    }

    private func sessionControls(_ snapshot: SessionSnapshot) -> some View {
        Button {
            isComposerFocused = false
            if snapshot.settings == nil { showsHostUpdate = true } else { showsModels = true }
        } label: {
            HStack(spacing: 5) {
                let settings = snapshot.settings
                let current = settings?.models.first { $0.id == settings?.selectedModel }
                Text(current?.model ?? store.selectedSession?.model ?? "Model")
                    .lineLimit(1).truncationMode(.middle)
                if let effort = settings?.reasoningEffort, effort != "default" {
                    Text(effort.capitalized).foregroundStyle(.secondary)
                }
            }
            .font(.subheadline.weight(.medium))
            .padding(.horizontal, 12)
            .frame(minHeight: 44)
            .contentShape(.capsule)
            .background(.fill.tertiary, in: .capsule)
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Model and effort")
        .accessibilityValue("\(snapshot.settings?.models.first { $0.id == snapshot.settings?.selectedModel }?.model ?? store.selectedSession?.model ?? "Model"), \(snapshot.settings?.reasoningEffort ?? "default")")
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
            openSessions()
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
