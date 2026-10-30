import RustCodeKit
import SwiftUI

/// The attention inbox (#2636): every thread's waiting approvals and
/// questions, stopped turns and unread replies in one list. Selecting an
/// entry opens its thread, where the approval and question cards stay the
/// place to answer; the inbox never answers or resolves anything itself.
@MainActor
extension DesktopModel {
    /// Rebuild the entries from what the app has observed.
    func refreshAttention(now: Date = Date()) {
        let requests = permissionQueue.map { request in
            AttentionInbox.Request(id: request.id, sessionId: request.sessionId,
                                   question: request.question?.text, title: request.title)
        }
        let entries = attentionInbox.entries(requests: requests, stopped: stoppedTurns, unseen: unseenReplies,
                                             known: Set(sessions.map(\.id)), now: now)
        if entries != attentionEntries { attentionEntries = entries }
    }

    func dismissAttention(_ entry: AttentionEntry) {
        attentionInbox.dismiss(entry)
        refreshAttention()
    }

    func openAttention(_ entry: AttentionEntry) async {
        guard let session = sessions.first(where: { $0.id == entry.sessionId }) else { return }
        await select(session)
    }

    /// Entries that wait for an answer: the count on the inbox button.
    var unresolvedAttention: Int { attentionEntries.filter(\.isUnresolved).count }
}

/// The sidebar's inbox button: a tray with the number of unanswered requests.
struct AttentionInboxButton: View {
    @ObservedObject var model: DesktopModel

    var body: some View {
        let unresolved = model.unresolvedAttention
        let total = model.attentionEntries.count
        ClusterButton(systemImage: total > 0 ? "tray.full" : "tray", help: help(unresolved: unresolved, total: total),
                      isOn: model.showsInbox) {
            model.showsInbox.toggle()
        }
        .overlay(alignment: .topTrailing) {
            if unresolved > 0 {
                Text("\(unresolved)")
                    .font(.caption2.weight(.bold).monospacedDigit())
                    .foregroundStyle(.white)
                    .padding(.horizontal, 4)
                    .frame(minWidth: 14, minHeight: 14)
                    .background(Color.orange, in: .capsule)
                    .offset(x: 2, y: -3)
                    .allowsHitTesting(false)
                    .accessibilityHidden(true)
            }
        }
        .popover(isPresented: $model.showsInbox, arrowEdge: .bottom) {
            AttentionInboxPanel(model: model) { model.showsInbox = false }
        }
    }

    private func help(unresolved: Int, total: Int) -> String {
        if unresolved > 0 { return "Inbox: \(unresolved) waiting for you" }
        if total > 0 { return "Inbox: \(total) update\(total == 1 ? "" : "s")" }
        return "Inbox"
    }
}

struct AttentionInboxPanel: View {
    @ObservedObject var model: DesktopModel
    let close: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text("Inbox")
                .font(.headline)
                .padding(.horizontal, 14)
                .padding(.top, 12)
                .padding(.bottom, 8)
            if !model.engineConnected {
                Label("The engine is not connected, so waiting approvals and questions cannot be listed.",
                      systemImage: "bolt.horizontal.circle")
                    .font(.callout)
                    .foregroundStyle(.orange)
                    .padding(.horizontal, 14)
                    .padding(.bottom, 8)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if model.attentionEntries.isEmpty {
                Label("Nothing needs you.", systemImage: "checkmark.circle")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .padding(.horizontal, 14)
                    .padding(.vertical, 18)
            } else {
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 2) {
                        ForEach(model.attentionEntries) { entry in
                            AttentionRow(entry: entry, session: model.sessions.first { $0.id == entry.sessionId }) {
                                close()
                                Task { await model.openAttention(entry) }
                            } dismiss: {
                                model.dismissAttention(entry)
                            }
                        }
                    }
                    .padding(.horizontal, 6)
                    .padding(.bottom, 8)
                }
                .frame(maxHeight: 420)
            }
        }
        .frame(width: 360)
    }
}

private struct AttentionRow: View {
    let entry: AttentionEntry
    let session: SessionSummary?
    let open: () -> Void
    let dismiss: () -> Void
    @State private var isHovered = false

    private var symbol: (String, Color) {
        switch entry.kind {
        case .approval: ("hand.raised.fill", .orange)
        case .question: ("questionmark.bubble.fill", .orange)
        case .stopped: ("exclamationmark.octagon.fill", .red)
        case .finished: ("checkmark.circle.fill", .green)
        }
    }

    private var status: String {
        switch entry.kind {
        case .approval: "Waiting for approval"
        case .question: "Waiting for an answer"
        case .stopped: "Stopped"
        case .finished: "Finished"
        }
    }

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Button(action: open) {
                HStack(alignment: .top, spacing: 10) {
                    Image(systemName: symbol.0)
                        .foregroundStyle(symbol.1)
                        .frame(width: 18)
                        .accessibilityHidden(true)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(session?.title ?? "Untitled thread")
                            .font(.callout.weight(.semibold))
                            .lineLimit(1)
                        Text(entry.reason)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .lineLimit(2)
                        HStack(spacing: 4) {
                            if let project = session?.projectName { Text(project) }
                            Text("·")
                            Text(entry.since, style: .relative)
                        }
                        .font(.caption2)
                        .foregroundStyle(.tertiary)
                    }
                    Spacer(minLength: 0)
                }
                .contentShape(.rect)
            }
            .buttonStyle(.plain)
            .accessibilityElement(children: .combine)
            .accessibilityLabel("\(status): \(session?.title ?? "thread"). \(entry.reason)")
            .accessibilityHint("Opens the thread")
            if entry.isDismissible {
                Button(action: dismiss) {
                    Image(systemName: "xmark")
                        .font(.caption.weight(.semibold))
                        .foregroundStyle(.secondary)
                        .frame(width: 20, height: 20)
                }
                .buttonStyle(.plain)
                .opacity(isHovered ? 1 : 0.4)
                .help("Dismiss")
                .accessibilityLabel("Dismiss")
            }
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 7)
        .background(isHovered ? AnyShapeStyle(.fill.tertiary) : AnyShapeStyle(.clear), in: .rect(cornerRadius: 8))
        .onHover { isHovered = $0 }
    }
}
