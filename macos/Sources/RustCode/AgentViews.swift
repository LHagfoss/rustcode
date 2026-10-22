import RustCodeKit
import SwiftUI

/// The shown chat's subagents above the composer, one chip each, as the
/// terminal's agent picker lists them. A chip opens that agent's panel.
struct AgentStrip: View {
    @ObservedObject var model: DesktopModel

    var body: some View {
        let agents = model.subagents
        let active = agents.filter(\.isActive).count
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 6) {
                Image(systemName: "person.2")
                Text("Agents").fontWeight(.semibold)
                Text(active > 0 ? "· \(active) working" : "· \(agents.count) finished")
                    .foregroundStyle(.secondary)
            }
            .font(.caption)
            .foregroundStyle(Color.brand)
            .padding(.horizontal, 6)
            ScrollView(.horizontal) {
                HStack(spacing: 6) {
                    ForEach(agents) { agent in
                        Button { model.openAgent(agent) } label: { AgentChip(agent: agent) }
                            .buttonStyle(.plain)
                            .help("\(agent.name): \(agent.task)")
                    }
                }
            }
            .scrollIndicators(.never)
        }
        .animation(.snappy(duration: 0.2), value: agents)
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

/// One subagent's conversation, read as the terminal shows a selected child:
/// what it was asked, what it did, and a field to send it more. Cancel stops
/// it and the agents it started.
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
        .frame(width: 760, height: 620)
        .onAppear { focused = true }
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                AgentStatusMark(agent: info)
                Text(info.name).font(.headline)
                Text(info.agentType).font(.callout).foregroundStyle(.secondary)
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
            Button("Done") { close() }
                .buttonStyle(PillButtonStyle(prominent: false))
        }
    }

    private func send() {
        let text = message
        guard !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
        message = ""
        Task { await model.sendToViewedAgent(text) }
    }
}
