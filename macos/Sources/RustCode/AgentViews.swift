import RustCodeKit
import SwiftUI

/// While the shown chat's agents work, a small pill over the composer's top
/// edge: `● 2 agents working ▾`. A click lists them as chips, and a chip
/// opens that agent's panel. It takes no room in the composer and goes once
/// every agent finished; their rows in the chat still open them (#2327).
struct AgentPill: View {
    @ObservedObject var model: DesktopModel
    @State private var showsList = false

    var body: some View {
        if let label = SubagentInfo.workingLabel(model.subagents) {
            Button { showsList.toggle() } label: {
                HStack(spacing: 6) {
                    Circle().fill(Color.brand).frame(width: 6, height: 6)
                        .modifier(Pulse())
                    Text(label).fontWeight(.medium).monospacedDigit()
                    Image(systemName: "chevron.down")
                        .font(.caption2.weight(.semibold))
                        .foregroundStyle(.secondary)
                        .rotationEffect(.degrees(showsList ? 180 : 0))
                }
                .font(.caption)
                .padding(.horizontal, 10)
                .frame(height: 24)
                .contentShape(.capsule)
                .interactiveGlassSurface(in: .capsule)
            }
            .buttonStyle(.plain)
            .pointingHandCursor()
            .help("Show the agents at work")
            .accessibilityLabel(label)
            .accessibilityHint("Lists the agents. Choose one to open its conversation.")
            .popover(isPresented: $showsList, arrowEdge: .top) {
                AgentList(model: model) { showsList = false }
            }
            .transition(.opacity.combined(with: .scale(scale: 0.9, anchor: .bottom)))
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

/// The pill's popover: every agent of the chat, one chip each, the working
/// ones first.
private struct AgentList: View {
    @ObservedObject var model: DesktopModel
    let close: () -> Void

    var body: some View {
        let agents = model.subagents.filter(\.isActive) + model.subagents.filter { !$0.isActive }
        VStack(alignment: .leading, spacing: 6) {
            Text("Agents").font(.caption.weight(.semibold)).foregroundStyle(.secondary)
            ForEach(agents) { agent in
                Button {
                    close()
                    model.openAgent(agent)
                } label: { AgentChip(agent: agent) }
                    .buttonStyle(.plain)
                    .pointingHandCursor()
                    .help("\(agent.name): \(agent.task)")
                    .accessibilityLabel("\(agent.name), \(agent.isActive ? "working" : agent.status.label)")
                    .accessibilityHint("Opens the agent's conversation")
            }
        }
        .padding(12)
        .frame(minWidth: 220, alignment: .leading)
        .animation(.snappy(duration: 0.2), value: agents)
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
                        Text([info.status.label, info.elapsed()].compactMap { $0 }.joined(separator: " · "))
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

private struct AgentChip: View {
    let agent: SubagentInfo

    var body: some View {
        HStack(spacing: 6) {
            AgentStatusMark(agent: agent)
            Text(agent.name).fontWeight(.medium)
            if agent.agentType != "default" {
                Text(agent.agentType).foregroundStyle(.secondary)
            }
            TimelineView(.periodic(from: .now, by: 1)) { context in
                if let elapsed = agent.elapsed(now: context.date) {
                    Text(elapsed).foregroundStyle(.tertiary).monospacedDigit()
                }
            }
        }
        .font(.caption)
        .padding(.horizontal, 10)
        .padding(.vertical, 5)
        .background(.fill.tertiary, in: .capsule)
        .contentShape(.capsule)
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
            case .queued, .running: ("circle", .secondary)
            }
            Image(systemName: symbol).foregroundStyle(color)
        }
    }
}

/// The open chat's working agents, nested under its row in the sidebar
/// (#2492). A row opens the agent's conversation in the chat's place. A
/// finished agent's row leaves; its result stays on its row in the chat.
struct SubagentChildRows: View {
    @ObservedObject var model: DesktopModel
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        let viewing = model.viewedAgent?.id
        let children = SubagentInfo.sidebarChildren(model.subagents, viewing: viewing)
        VStack(alignment: .leading, spacing: 1) {
            ForEach(children) { agent in
                SubagentChildRow(agent: agent, isSelected: agent.id == viewing) {
                    if agent.id == viewing { model.viewedAgent = nil } else { model.openAgent(agent) }
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
        .accessibilityLabel("Agents of this chat")
    }
}

private struct SubagentChildRow: View {
    let agent: SubagentInfo
    let isSelected: Bool
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
                TimelineView(.periodic(from: .now, by: 1)) { context in
                    Text(agent.isActive ? agent.elapsed(now: context.date) ?? "" : agent.status.label)
                        .foregroundStyle(.tertiary)
                        .monospacedDigit()
                }
            }
            .font(.caption)
            .padding(.horizontal, 8)
            .frame(height: 24)
            .background(isSelected ? AnyShapeStyle(.fill.secondary) : AnyShapeStyle(.clear), in: .rect(cornerRadius: 8))
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .hoverHighlight(cornerRadius: 8, padding: 0)
        .pointingHandCursor()
        .help("\(agent.name): \(agent.task)")
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Agent \(agent.name), \(agent.agentType), \(agent.isActive ? "working" : agent.status.label)")
        .accessibilityHint(isSelected ? "Goes back to the chat" : "Opens the agent's conversation")
        .accessibilityAddTraits(isSelected ? [.isButton, .isSelected] : .isButton)
    }
}

/// One subagent's conversation in the chat's place, read as the terminal
/// shows a selected child: which chat started it, what it was asked, what it
/// did, and a field to send it more. Cancel stops it and the agents it
/// started; Back (Esc) returns to the chat.
struct AgentPanel: View {
    @ObservedObject var model: DesktopModel
    let agent: DesktopModel.ViewedAgent
    @Environment(\.closePanel) private var close
    @State private var message = ""
    @FocusState private var focused: Bool

    private var info: SubagentInfo { agent.info }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header.padding(20)
            Divider()
            conversation
            Divider()
            composer.padding(14)
        }
        .frame(maxWidth: 980, maxHeight: .infinity) // The chat column's width.
        .frame(maxWidth: .infinity)
        .onAppear { focused = true }
    }

    private var parentTitle: String {
        model.sessions.first { $0.id == model.selectedSession }?.title ?? "Chat"
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 8) {
            Button { close() } label: {
                Label(parentTitle, systemImage: "chevron.left").lineLimit(1)
            }
            .buttonStyle(.plain)
            .font(.callout)
            .foregroundStyle(.secondary)
            .pointingHandCursor()
            .keyboardShortcut(.cancelAction)
            .help("Back to the chat that started \(info.name) (Esc)")
            .accessibilityLabel("Back to \(parentTitle)")
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                AgentStatusMark(agent: info)
                Text(info.name).font(.headline)
                Text(info.agentType).font(.callout).foregroundStyle(.secondary)
                Text("subagent").font(.caption).foregroundStyle(.secondary)
                    .padding(.horizontal, 6).padding(.vertical, 1)
                    .background(.fill.tertiary, in: .capsule)
                if info.writeAccess {
                    Text("writes").font(.caption).padding(.horizontal, 6).padding(.vertical, 1)
                        .background(Color.orange.opacity(0.18), in: .capsule)
                }
                Spacer()
                TimelineView(.periodic(from: .now, by: 1)) { context in
                    Text([info.status.label, info.model, info.elapsed(now: context.date)]
                        .compactMap { $0 }.joined(separator: " · "))
                        .font(.callout)
                        .foregroundStyle(.secondary)
                        .monospacedDigit()
                        .lineLimit(1)
                }
            }
            Text(info.task)
                .font(.callout)
                .foregroundStyle(.secondary)
                .lineLimit(3)
                .textSelection(.enabled)
        }
    }

    private var conversation: some View {
        let items = agent.transcript.items
        return ScrollViewReader { proxy in
            ScrollView {
                VStack(alignment: .leading, spacing: 18) {
                    if items.isEmpty {
                        Text(agent.loadedCount < 0 ? "Loading…" : "Nothing yet.")
                            .foregroundStyle(.secondary)
                            .frame(maxWidth: .infinity)
                            .padding(.top, 40)
                    }
                    ForEach(items) { item in
                        TranscriptRow(item: item)
                    }
                    if let completion = info.completion, !info.isActive {
                        Label(completion, systemImage: "flag.checkered")
                            .font(.callout)
                            .foregroundStyle(.secondary)
                            .textSelection(.enabled)
                    }
                    Color.clear.frame(height: 1).id("end")
                }
                .padding(20)
            }
            .onAppear { proxy.scrollTo("end", anchor: .bottom) }
            .onChange(of: items.count) { _, _ in
                withAnimation(.snappy(duration: 0.2)) { proxy.scrollTo("end", anchor: .bottom) }
            }
        }
    }

    private var composer: some View {
        HStack(spacing: 8) {
            TextField(info.isActive ? "Tell \(info.name) more; it reads it at its next step"
                      : "Give \(info.name) a follow-up task", text: $message, axis: .vertical)
                .textFieldStyle(.plain)
                .lineLimit(1...5)
                .focused($focused)
                .onSubmit(send)
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
                .background(Color.primary.opacity(0.06), in: .rect(cornerRadius: 14))
            Button(action: send) { Image(systemName: "arrow.up") }
                .buttonStyle(PillButtonStyle(prominent: true))
                .disabled(message.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                .help("Send to \(info.name)")
            if info.isActive {
                Button("Cancel agent") { Task { await model.cancelViewedAgent() } }
                    .buttonStyle(PillButtonStyle(prominent: false))
                    .help("Stop \(info.name) and the agents it started")
            }
        }
    }

    private func send() {
        let text = message
        guard !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
        message = ""
        Task { await model.sendToViewedAgent(text) }
    }
}
