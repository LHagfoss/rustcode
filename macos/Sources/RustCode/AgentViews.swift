import RustCodeKit
import SwiftUI

/// While the shown chat's agents work, a small pill in the composer's button
/// row beside +: `● 2 agents working ▾`. A click lists them in a composer
/// panel like + and the model's (#2626), and a row opens that agent's panel. Over the chat it covered the end of the
/// transcript (#2555). It goes once every agent finished; their rows in the
/// chat still open them (#2327).
struct AgentPill: View {
    @ObservedObject var model: DesktopModel
    @Environment(\.chatLayout) private var layout

    private var showsList: Bool { model.composerPanel == .agents }

    var body: some View {
        if let label = SubagentInfo.workingLabel(model.subagents) {
            Button { model.composerPanel = showsList ? nil : .agents } label: {
                HStack(spacing: 6) {
                    Circle().fill(Color.brand).frame(width: 6, height: 6)
                        .modifier(Pulse())
                    // A narrow chat shows just the count (#2552).
                    Text(layout.compactComposer ? "\(model.subagents.filter(\.isActive).count)" : label)
                        .fontWeight(.medium).monospacedDigit().lineLimit(1)
                    Image(systemName: "chevron.up")
                        .font(.caption2.weight(.semibold))
                        .foregroundStyle(.secondary)
                        .rotationEffect(.degrees(showsList ? 180 : 0))
                }
                .font(.callout)
                .padding(.horizontal, 11)
                .frame(height: 30)
                .background(showsList ? AnyShapeStyle(.fill.secondary) : AnyShapeStyle(.fill.tertiary), in: .capsule)
                .contentShape(.capsule)
            }
            .buttonStyle(.plain)
            .pointingHandCursor()
            .help("Show the agents at work")
            .accessibilityLabel(label)
            .accessibilityHint("Lists the agents. Choose one to open its conversation.")
            .transition(.opacity)
        }
    }
}

/// The dot of the pill breathing while agents work.
private struct Pulse: ViewModifier {
    @State private var dim = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    func body(content: Content) -> some View {
        content
            .opacity(dim ? 0.5 : 1)
            .onAppear {
                guard !reduceMotion else { return }
                withAnimation(.easeInOut(duration: 0.9).repeatForever(autoreverses: true)) { dim = true }
            }
    }
}

/// The pill's panel: every agent of the chat, working ones first, in the
/// rows and glass the composer's other panels use. Arrows, ⏎ and Esc work
/// as there.
struct AgentsPanel: View {
    @ObservedObject var model: DesktopModel
    @State private var highlighted = 0
    @FocusState private var focused: Bool

    private var agents: [SubagentInfo] {
        model.subagents.filter(\.isActive) + model.subagents.filter { !$0.isActive }
    }

    var body: some View {
        let agents = agents
        VStack(alignment: .leading, spacing: 0) {
            Text("Agents")
                .font(.callout.weight(.semibold))
                .padding(.horizontal, 10)
                .padding(.top, 6)
                .padding(.bottom, 4)
            ForEach(Array(agents.enumerated()), id: \.element.id) { index, agent in
                AgentsPanelRow(agent: agent, isHighlighted: index == highlighted) { open(agent) }
            }
        }
        .panelKeys(focused: $focused, count: agents.count, highlighted: $highlighted,
                   choose: { if agents.indices.contains(highlighted) { open(agents[highlighted]) } },
                   close: { model.composerPanel = nil })
        .animation(.snappy(duration: 0.2), value: agents)
    }

    private func open(_ agent: SubagentInfo) {
        model.composerPanel = nil
        model.openAgent(agent)
    }
}

/// An agent in the pill's panel, laid out as `PopoverRow`: how it stands,
/// its name and the first line of its task, and how long it ran.
private struct AgentsPanelRow: View {
    let agent: SubagentInfo
    let isHighlighted: Bool
    let action: () -> Void
    @State private var isHovered = false

    var body: some View {
        Button(action: action) {
            HStack(spacing: 10) {
                AgentStatusMark(agent: agent).frame(width: 18)
                VStack(alignment: .leading, spacing: 1) {
                    Text(agent.name).lineLimit(1).truncationMode(.middle)
                    if !agent.taskLine.isEmpty {
                        Text(agent.taskLine).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                    }
                }
                Spacer(minLength: 12)
                TimelineView(.periodic(from: .now, by: 1)) { context in
                    Text(agent.isActive ? agent.elapsed(now: context.date) ?? "" : agent.status.label)
                        .font(.caption)
                        .foregroundStyle(.tertiary)
                        .monospacedDigit()
                }
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 8)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(isHighlighted ? AnyShapeStyle(Color.brand.opacity(0.2))
                        : isHovered ? AnyShapeStyle(.fill.tertiary) : AnyShapeStyle(.clear), in: .rect(cornerRadius: 14))
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .onHover { isHovered = $0 }
        .pointingHandCursor()
        .help("\(agent.name): \(agent.task)")
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(agent.name), \(agent.isActive ? "working" : agent.status.label)")
        .accessibilityHint("Opens the agent's conversation")
    }
}

/// The agents a turn started, where it started them: a row each, with how
/// it stands, its name, role and task, and how long it ran. A click opens
/// the agent's panel. Several started together share one header (#2327).
struct AgentSpawnGroup: View {
    let spawns: [AgentSpawn]

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            if spawns.count > 1 {
                Label("Started \(spawns.count) agents", systemImage: "person.2")
                    .font(.caption.weight(.medium))
                    .foregroundStyle(.secondary)
                    .padding(.horizontal, 6)
                    .padding(.bottom, 2)
            }
            ForEach(spawns) { spawn in AgentSpawnRow(spawn: spawn) }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

private struct AgentSpawnRow: View {
    let spawn: AgentSpawn
    @Environment(\.openAgent) private var openAgent

    var body: some View {
        let id = spawn.info?.id
        Button { if let id { openAgent?(id) } } label: {
            HStack(spacing: 8) {
                mark.frame(width: 16)
                Text(spawn.name).fontWeight(.medium).lineLimit(1)
                if let type = spawn.info?.agentType, type != "default" {
                    Text(type).foregroundStyle(.secondary)
                }
                if let task = spawn.task {
                    Text(task).foregroundStyle(.secondary).lineLimit(1).truncationMode(.tail)
                }
                Spacer(minLength: 8)
                if let info = spawn.info {
                    if info.isActive {
                        TimelineView(.periodic(from: .now, by: 1)) { context in
                            Text(info.elapsed(now: context.date) ?? "").foregroundStyle(.tertiary).monospacedDigit()
                        }
                    } else {
                        Text([info.status.label, info.elapsed(),
                              info.totalTokens > 0 ? "\(SubagentInfo.compact(info.totalTokens)) tokens" : nil]
                            .compactMap { $0 }.joined(separator: " · "))
                            .foregroundStyle(info.status == .failed ? AnyShapeStyle(.red) : AnyShapeStyle(.tertiary))
                            .monospacedDigit()
                    }
                }
                if id != nil {
                    Image(systemName: "chevron.right").font(.caption2.weight(.semibold)).foregroundStyle(.tertiary)
                }
            }
            .font(.callout)
            .frame(height: 22)
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .disabled(id == nil || openAgent == nil)
        .hoverHighlight(cornerRadius: 8, padding: 5)
        .help(spawn.info.map { "Open \($0.name)'s conversation" } ?? spawn.task ?? "")
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(spawn.accessibilityLabel())
        .accessibilityHint(id != nil ? "Opens the agent's conversation" : "")
        .accessibilityAddTraits(.isButton)
    }

    @ViewBuilder
    private var mark: some View {
        if let info = spawn.info {
            AgentStatusMark(agent: info)
        } else if spawn.status == .failed {
            Image(systemName: "xmark.circle.fill").foregroundStyle(.red)
        } else {
            ProgressView().controlSize(.mini)
        }
    }
}

/// A spinner while the agent works, else a mark for how it ended.
struct AgentStatusMark: View {
    let agent: SubagentInfo

    var body: some View {
        if agent.isActive {
            ProgressView().controlSize(.mini)
        } else {
            let (symbol, color): (String, Color) = switch agent.status {
            case .completed: ("checkmark.circle.fill", .green)
            case .failed: ("xmark.circle.fill", .red)
            case .cancelled, .interrupted: ("stop.circle.fill", .secondary)
            case .unknown: ("questionmark.circle", .secondary)
            case .queued, .running, .cancelling: ("circle", .secondary)
            }
            Image(systemName: symbol).foregroundStyle(color)
        }
    }
}

/// A chat's agents, nested under its row in the sidebar (#2492). For the
/// open chat, its working agents: a row opens the agent's conversation in
/// the chat's place, and a finished agent's row leaves; its result stays on
/// its row in the chat. For a chat off screen, the agents that were working
/// when it was left, until it is opened again, so they can be found and
/// show how they ended (#2556); a row opens the chat and then the agent.
struct SubagentChildRows: View {
    @ObservedObject var model: DesktopModel
    let session: SessionSummary
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        let shown = session.id == model.selectedSession
        let viewing = shown ? model.viewedAgent?.id : nil
        let children = shown
            ? model.sidebarChildren(of: session.id)
            : model.backgroundAgents.agents(of: session.id)
        let stale = !shown && model.backgroundAgents.isStale(session.id)
        VStack(alignment: .leading, spacing: 1) {
            ForEach(children) { agent in
                SubagentChildRow(agent: agent, isSelected: agent.id == viewing, isStale: stale) {
                    if !shown {
                        Task { await model.openAgent(agent, in: session) }
                    } else if agent.id == viewing {
                        model.viewedAgent = nil
                    } else {
                        model.openAgent(agent)
                    }
                }
                .padding(.leading, CGFloat(max(agent.depth - 1, 0)) * 12)
                .transition(reduceMotion ? .opacity : .opacity.combined(with: .move(edge: .top)))
            }
        }
        .padding(.leading, 18)
        .overlay(alignment: .leading) {
            if !children.isEmpty {
                Rectangle().fill(.quaternary).frame(width: 1).padding(.leading, 11).padding(.vertical, 4)
            }
        }
        .animation(reduceMotion ? nil : .snappy(duration: 0.25), value: children.map(\.id))
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Agents of \(session.title ?? "New chat")")
    }
}

private struct SubagentChildRow: View {
    let agent: SubagentInfo
    let isSelected: Bool
    /// The last state read; the latest read got no answer (#2653).
    var isStale = false
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 6) {
                AgentStatusMark(agent: agent).frame(width: 14)
                Text(agent.name).fontWeight(.medium).lineLimit(1)
                if agent.agentType != "default" {
                    Text(agent.agentType).foregroundStyle(.secondary).lineLimit(1)
                }
                Spacer(minLength: 4)
                if isStale {
                    Image(systemName: "wifi.exclamationmark").foregroundStyle(.tertiary)
                } else {
                    TimelineView(.periodic(from: .now, by: 1)) { context in
                        Text(agent.isActive ? agent.elapsed(now: context.date) ?? "" : agent.status.label)
                            .foregroundStyle(.tertiary)
                            .monospacedDigit()
                    }
                }
            }
            .opacity(isStale ? 0.6 : 1)
            .font(.caption)
            .padding(.horizontal, 8)
            .frame(height: 24)
            .background(isSelected ? AnyShapeStyle(.fill.secondary) : AnyShapeStyle(.clear), in: .rect(cornerRadius: 8))
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .hoverHighlight(cornerRadius: 8, padding: 0)
        .pointingHandCursor()
        .help(isStale ? "\(agent.name): last known state; RustCode could not reach the engine to refresh it"
                      : "\(agent.name): \(agent.task)")
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Agent \(agent.name), \(agent.agentType), \(isStale ? "last known " : "")\(agent.isActive ? "working" : agent.status.label)")
        .accessibilityHint(isSelected ? "Goes back to the thread" : "Opens the agent's conversation")
        .accessibilityAddTraits(isSelected ? [.isButton, .isSelected] : .isButton)
    }
}

/// One subagent's conversation in the chat's place, read as the terminal
/// shows a selected child: which chat started it, what it was asked, what it
/// did, and the chat's composer to send it more (#2557). Back (Esc) or the
/// chat's row in the sidebar returns to the chat.
///
/// The composer keeps the main one's field, keys, send and stop, and leaves
/// out what does not reach a child: files and images (the engine passes a
/// child text only), the model and effort pickers (a child keeps the model it
/// was started with), slash commands and the message queue (both act on the
/// chat; a child reads a message sent mid-step at its next step).
struct AgentPanel: View {
    @ObservedObject var model: DesktopModel
    let agent: DesktopModel.ViewedAgent
    @Environment(\.closePanel) private var close
    @FocusState private var focused: Bool

    @State private var width: CGFloat = 0
    @State private var composerHeight: CGFloat = 0

    private var info: SubagentInfo { agent.info }
    private var layout: ChatColumnLayout { ChatColumnLayout(width: Double(width)) }

    /// Laid out as the chat is: a slim bar on top, the messages in the
    /// chat's column, and the composer floating over their foot with the
    /// messages fading out under it.
    var body: some View {
        VStack(spacing: 0) {
            bar
            ZStack(alignment: .bottom) {
                conversation
                AgentComposer(model: model, info: info, focused: $focused)
                    .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { composerHeight = $0 }
                    .frame(maxWidth: CGFloat(ChatColumnLayout.maxColumn))
                    .padding(.horizontal, CGFloat(layout.padding))
                    .padding(.bottom, 16)
                    .frame(maxWidth: .infinity)
                    .background(alignment: .bottom) {
                        LinearGradient(stops: [.init(color: Color.chatBackground.opacity(0), location: 0),
                                               .init(color: Color.chatBackground, location: 0.4)],
                                       startPoint: .top, endPoint: .bottom)
                            .frame(height: composerHeight + 16 + 36)
                            .allowsHitTesting(false)
                    }
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .onGeometryChange(for: CGFloat.self) { $0.size.width } action: { width = $0 }
        .environment(\.chatLayout, layout)
        .onAppear { focused = true }
        // Another agent opened in place: its own draft, ready to type.
        .onChange(of: info.id) { _, _ in focused = true }
    }

    private var parentTitle: String {
        model.sessions.first { $0.id == model.selectedSession }?.title ?? "Thread"
    }

    /// The chat header's bar: back to the thread on the left, as the chat's
    /// title sits, and the agent's state on the right.
    private var bar: some View {
        HStack(spacing: 10) {
            Button { close() } label: {
                HStack(spacing: 6) {
                    Image(systemName: "chevron.left").font(.system(size: 12, weight: .semibold))
                    Text(parentTitle).lineLimit(1)
                }
                .font(.callout.weight(.medium))
                .foregroundStyle(.secondary)
                .padding(.horizontal, 10)
                .frame(height: 28)
                .background(.fill.tertiary, in: .capsule)
                .contentShape(.capsule)
            }
            .buttonStyle(.plain)
            .pointingHandCursor()
            .keyboardShortcut(.cancelAction)
            .help("Back to the thread that started \(info.name) (Esc)")
            .accessibilityLabel("Back to \(parentTitle)")
            HStack(spacing: 6) {
                AgentStatusMark(agent: info)
                Text(info.name).font(.headline).lineLimit(1)
            }
            Spacer(minLength: 0)
            TimelineView(.periodic(from: .now, by: 1)) { context in
                let quiet = info.quietLabel(now: context.date)
                Text([info.status.label, info.model, info.elapsed(now: context.date), quiet]
                    .compactMap { $0 }.joined(separator: " · "))
                    .font(.callout)
                    .foregroundStyle(quiet == nil ? AnyShapeStyle(.secondary) : AnyShapeStyle(.orange))
                    .monospacedDigit()
                    .lineLimit(1)
                    .padding(.horizontal, 10)
                    .frame(height: 28)
                    .background(.fill.tertiary, in: .capsule)
            }
        }
        .padding(.horizontal, 10)
        // Clear of the traffic lights while the sidebar is hidden.
        .padding(.leading, model.showsSidebar ? 0 : SidebarMetrics.barLeading - 10)
        .frame(height: SidebarMetrics.barHeight)
        .padding(.top, SidebarMetrics.barTopInset)
        .padding(.bottom, 8)
        .background(WindowDragArea())
    }

    /// What the agent was asked and may do, at the top of its messages and
    /// scrolling with them, as a new thread's first message does.
    private var details: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(info.agentType).font(.callout.weight(.medium))
                Text("subagent").font(.caption).foregroundStyle(.secondary)
                    .padding(.horizontal, 6).padding(.vertical, 1)
                    .background(.fill.tertiary, in: .capsule)
                if info.writeAccess {
                    Text("writes").font(.caption).padding(.horizontal, 6).padding(.vertical, 1)
                        .background(Color.orange.opacity(0.18), in: .capsule)
                }
            }
            Text(info.task)
                .font(.callout)
                .foregroundStyle(.secondary)
                .lineLimit(3)
                .textSelection(.enabled)
            AgentContract(info: info, parentName: info.parentId.flatMap { id in model.subagents.first { $0.id == id }?.name },
                          verification: info.verification(in: agent.transcript))
            if let totals = info.totalsLabel {
                Label(totals, systemImage: "chart.bar.doc.horizontal")
                    .font(.caption)
                    .foregroundStyle(.tertiary)
                    .monospacedDigit()
                    .lineLimit(1)
                    .help("Tokens over every request \(info.name) made, follow-ups included; tool calls and requests of its latest run")
            }
        }
    }

    private var conversation: some View {
        AgentConversation(transcript: agent.transcript, isLoading: agent.loadedCount < 0,
                          completion: info.isActive ? nil : info.completion,
                          info: info, layout: layout, bottomInset: composerHeight + 16 + 16) { details.card() }
            .equatable()
            .id(info.id)
    }
}

/// What the child was started with (#2639): who started it, where it
/// works, what it may write and the check it was asked to pass, with that
/// check's outcome as the child's own runs show it. A row shows only when
/// the parent set it; nothing here grants or changes access.
private struct AgentContract: View {
    let info: SubagentInfo
    let parentName: String?
    let verification: SubagentInfo.Verification

    private var writes: String {
        guard info.writeAccess else { return "Read-only" }
        return info.allowedPaths.isEmpty ? "May write; no paths listed" : info.allowedPaths.joined(separator: ", ")
    }

    private var workspace: String? {
        guard let mode = info.workspaceMode else { return nil }
        let place = info.workspacePath.map { " · \($0.abbreviatingHome)" } ?? ""
        return (mode == "isolated" ? "Own workspace" : "The thread's folder") + place
    }

    /// `Ran tests · cargo` for `cargo test --workspace`.
    static func checkLabel(_ command: String) -> String {
        let summary = CommandSummary.of(command)
        return [summary.label, summary.detail].compactMap { $0 }.joined(separator: " ")
            + (summary.tag.map { " · \($0)" } ?? "")
    }

    private var check: (String, String, Color)? {
        switch verification {
        case .none: nil
        case .notRun: (info.isActive ? "Not run yet" : "Not run", "circle.dashed", .secondary)
        case .running: ("Running", "clock", .secondary)
        case .passed: ("Passed", "checkmark.circle.fill", .green)
        case .failed: ("Failed", "xmark.circle.fill", .red)
        case .unknown: ("No result", "questionmark.circle", .orange)
        }
    }

    var body: some View {
        Grid(alignment: .leadingFirstTextBaseline, horizontalSpacing: 10, verticalSpacing: 3) {
            row("Started by", parentName ?? "This thread")
            if info.workspaceMode != nil { row("Writes", writes) }
            if let workspace { row("Works in", workspace) }
            if let context = info.contextInheritance { row("Context", Self.contextLabel(context)) }
            if let command = info.verificationCommand, let check {
                GridRow {
                    Text("Check").foregroundStyle(.tertiary)
                    HStack(spacing: 6) {
                        // The command in words; the command itself on hover and to copy.
                        Text(Self.checkLabel(command)).lineLimit(1).truncationMode(.tail)
                            .help(command)
                            .contextMenu { Button("Copy Command", systemImage: "doc.on.doc") { Clipboard.copy(command) } }
                        Label(check.0, systemImage: check.1)
                            .foregroundStyle(check.2)
                            .help("From \(info.name)'s own run of the command, not from what it wrote")
                    }
                }
                .accessibilityElement(children: .combine)
                .accessibilityLabel("Check \(command): \(check.0)")
            }
        }
        .font(.caption)
        .foregroundStyle(.secondary)
    }

    private func row(_ title: String, _ value: String) -> some View {
        GridRow {
            Text(title).foregroundStyle(.tertiary)
            Text(value).lineLimit(2).truncationMode(.middle).textSelection(.enabled).help(value)
        }
        .accessibilityElement(children: .combine)
    }

    static func contextLabel(_ value: String) -> String {
        switch value {
        case "minimal": "The task only"
        case "evidence": "The task and evidence passed to it"
        case "recent": "The thread's recent messages"
        case "fork": "The whole thread so far"
        default: value
        }
    }
}

/// The child's messages. Its own view, compared by what it shows, so the
/// header's status and timers ticking do not lay out every message again;
/// and only the latest `window` rows are drawn until asked for more, so a
/// long-running child opens at once, as the main chat draws a window too.
private struct AgentConversation<Lead: View>: View, Equatable {
    let transcript: Transcript
    let isLoading: Bool
    let completion: String?
    /// What `lead` shows, so it is drawn again only when that changes.
    let info: SubagentInfo
    let layout: ChatColumnLayout
    /// Room under the last message for the composer floating over it.
    let bottomInset: CGFloat
    @ViewBuilder let lead: () -> Lead
    @State private var shown = Self.window

    /// Rows drawn when the panel opens, and added per "Show earlier".
    static var window: Int { 40 }

    static func == (lhs: Self, rhs: Self) -> Bool {
        lhs.transcript == rhs.transcript && lhs.isLoading == rhs.isLoading && lhs.completion == rhs.completion
            && lhs.info == rhs.info && lhs.layout == rhs.layout && lhs.bottomInset == rhs.bottomInset
    }

    var body: some View {
        let items = transcript.items
        let hidden = max(0, items.count - shown)
        ScrollViewReader { proxy in
            ScrollView {
                VStack(alignment: .leading, spacing: 18) {
                    lead()
                    if items.isEmpty {
                        Text(isLoading ? "Loading…" : "Nothing yet.")
                            .foregroundStyle(.secondary)
                            .frame(maxWidth: .infinity)
                            .padding(.top, 40)
                    }
                    if hidden > 0 {
                        Button("Show \(min(hidden, Self.window)) earlier of \(hidden)") { shown += Self.window }
                            .buttonStyle(.link)
                            .font(.callout)
                            .frame(maxWidth: .infinity)
                    }
                    ForEach(items.suffix(items.count - hidden)) { item in
                        TranscriptRow(item: item).equatable()
                    }
                    if let completion {
                        Label(completion, systemImage: "flag.checkered")
                            .font(.callout)
                            .foregroundStyle(.secondary)
                            .textSelection(.enabled)
                    }
                    Color.clear.frame(height: 1).id("end")
                }
                .padding(.top, 8)
                .padding(.bottom, bottomInset)
                .frame(maxWidth: CGFloat(layout.column))
                .padding(.horizontal, CGFloat(layout.padding))
                .frame(maxWidth: .infinity)
            }
            .onAppear { proxy.scrollTo("end", anchor: .bottom) }
            .onChange(of: items.count) { _, _ in
                withAnimation(.snappy(duration: 0.2)) { proxy.scrollTo("end", anchor: .bottom) }
            }
        }
    }
}

/// The subagent panel's composer: the main composer's field and keys (⏎
/// sends, ⇧⏎ or ⌥⏎ start a new line), a draft kept per agent, send, and
/// stop while the agent works.
struct AgentComposer: View {
    @ObservedObject var model: DesktopModel
    let info: SubagentInfo
    var focused: FocusState<Bool>.Binding

    private var draft: Binding<String> {
        let key = model.agentDraftKey(info.id)
        return Binding(get: { model.agentDrafts[key] ?? "" },
                       set: { model.agentDrafts[key] = $0.isEmpty ? nil : $0 })
    }

    private var canSend: Bool { !draft.wrappedValue.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            TextField("Message \(info.name)…", text: draft, axis: .vertical)
                .textFieldStyle(.plain)
                .font(.body)
                .lineLimit(1...10)
                .focused(focused)
                .padding(.horizontal, 6)
                .padding(.top, 4)
                .onKeyPress(.return, phases: .down) { press in
                    MarkedText.commitInWindows()
                    if press.modifiers.contains(.shift) || press.modifiers.contains(.option) {
                        NSApp.sendAction(#selector(NSText.insertNewlineIgnoringFieldEditor(_:)), to: nil, from: nil)
                        return .handled
                    }
                    send()
                    return .handled
                }
                .accessibilityLabel("Message to \(info.name)")
            HStack(spacing: 8) {
                Label(info.isActive ? "\(info.name) reads it at its next step" : "Starts \(info.name) on a follow-up",
                      systemImage: "arrow.turn.down.right")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .padding(.leading, 6)
                Spacer(minLength: 0)
                stopButton
                Button(action: send) {
                    Image(systemName: "arrow.up")
                        .font(.system(size: 13, weight: .bold))
                        .frame(width: 30, height: 30)
                        .background(canSend ? Color.brandFill : Color.secondary.opacity(0.25), in: .circle)
                        .foregroundStyle(.white)
                }
                .buttonStyle(.plain)
                .disabled(!canSend)
                .help("Send to \(info.name) (⏎)")
                .accessibilityLabel("Send to \(info.name)")
            }
            .animation(.spring(duration: 0.3, bounce: 0.2), value: info.isActive)
        }
        .padding(10)
        .floatingSurface(in: .rect(cornerRadius: 22))
        // A click anywhere on the composer but its buttons puts the cursor in it.
        .contentShape(.rect(cornerRadius: 22))
        .onTapGesture { focused.wrappedValue = true }
    }

    @ViewBuilder
    private var stopButton: some View {
        if info.status == .cancelling {
            ProgressView().controlSize(.small)
                .frame(width: 30, height: 30)
                .help("\(info.name) was asked to stop and has not ended yet")
                .accessibilityLabel("Cancelling \(info.name)")
        } else if info.isActive {
            Button { Task { await model.cancelViewedAgent() } } label: {
                Image(systemName: "stop.fill")
                    .font(.system(size: 11, weight: .bold))
                    .frame(width: 30, height: 30)
                    .background(Color.red.gradient, in: .circle)
                    .foregroundStyle(.white)
            }
            .buttonStyle(.plain)
            .help("Stop \(info.name) and the agents it started")
            .accessibilityLabel("Stop \(info.name)")
            .transition(.scale(scale: 0.5).combined(with: .opacity))
        }
    }

    private func send() {
        let text = draft.wrappedValue
        guard canSend else { return }
        draft.wrappedValue = ""
        Task { await model.sendToViewedAgent(text) }
    }
}
