import RustCodeKit
import SwiftUI

/// The task inspector (#2641): the steps the agent planned for a larger
/// task, from the header of a thread that has a plan. A step's mark is the
/// agent's claim; the tool calls and agents in the thread are the work. A
/// click on a step scrolls the thread to where the agent last updated it.

/// The header button: a checklist with the steps done of all, shown only
/// while the thread has a plan of two steps or more.
struct TaskPlanButton: View {
    @ObservedObject var model: DesktopModel
    @ObservedObject var chat: LiveTranscript

    var body: some View {
        let plan = model.taskPlan
        if plan.isShown {
            Button { model.showsTaskPlan.toggle() } label: {
                HStack(spacing: 5) {
                    Image(systemName: plan.steps.contains { $0.status == .blocked } ? "exclamationmark.circle" : "checklist")
                    Text(plan.progressLabel).monospacedDigit()
                }
                .font(.callout.weight(.medium))
                .foregroundStyle(model.showsTaskPlan ? AnyShapeStyle(.primary) : AnyShapeStyle(.secondary))
                .padding(.horizontal, 10)
                .frame(height: 28)
                .background(model.showsTaskPlan ? AnyShapeStyle(.fill.secondary) : AnyShapeStyle(.fill.tertiary), in: .capsule)
                .contentShape(.capsule)
            }
            .buttonStyle(.plain)
            .help("Task plan: \(plan.done) of \(plan.steps.count) steps marked done by the agent")
            .accessibilityLabel("Task plan, \(plan.done) of \(plan.steps.count) steps marked done")
            .popover(isPresented: $model.showsTaskPlan, arrowEdge: .bottom) {
                TaskPlanPanel(model: model, items: chat.transcript.items)
            }
        }
    }
}

struct TaskPlanPanel: View {
    @ObservedObject var model: DesktopModel
    let items: [TranscriptItem]
    @FocusState private var focused: Bool
    @State private var highlighted = 0

    var body: some View {
        let plan = model.taskPlan
        let updates = plan.lastUpdates(in: items)
        let latest = TaskPlan.latestUpdate(in: items)
        let running = model.isTurnRunning
        VStack(alignment: .leading, spacing: 0) {
            HStack(alignment: .firstTextBaseline) {
                Text("Plan").font(.headline)
                Spacer()
                Text("\(plan.done) of \(plan.steps.count) done").font(.caption).foregroundStyle(.secondary).monospacedDigit()
            }
            .padding(.horizontal, 14)
            .padding(.top, 12)
            .padding(.bottom, 4)
            Text("As the agent marked each step. The thread's tool calls are the work itself.")
                .font(.caption)
                .foregroundStyle(.tertiary)
                .padding(.horizontal, 14)
                .padding(.bottom, 8)
                .fixedSize(horizontal: false, vertical: true)
            if model.taskPlanStale {
                Label("Could not read the latest plan; this is the last one read.", systemImage: "wifi.exclamationmark")
                    .font(.caption)
                    .foregroundStyle(.orange)
                    .padding(.horizontal, 14)
                    .padding(.bottom, 8)
            }
            ScrollView {
                VStack(alignment: .leading, spacing: 2) {
                    ForEach(Array(plan.steps.enumerated()), id: \.element.id) { index, step in
                        let target = updates[step.id] ?? latest
                        TaskPlanRow(step: step, interrupted: TaskPlan.isInterrupted(step, turnRunning: running),
                                    isHighlighted: index == highlighted, canOpen: target != nil) {
                            open(target)
                        }
                    }
                }
                .padding(.horizontal, 6)
                .padding(.bottom, 8)
            }
            .frame(maxHeight: 420)
        }
        .frame(width: 380)
        .focusable()
        .focused($focused)
        .focusEffectDisabled()
        .onAppear { focused = true }
        .onKeyPress(.downArrow) { highlighted = min(highlighted + 1, plan.steps.count - 1); return .handled }
        .onKeyPress(.upArrow) { highlighted = max(highlighted - 1, 0); return .handled }
        .onKeyPress(.return) {
            guard plan.steps.indices.contains(highlighted) else { return .ignored }
            open(updates[plan.steps[highlighted].id] ?? latest)
            return .handled
        }
    }

    private func open(_ target: Int?) {
        guard let target else { return }
        model.showsTaskPlan = false
        model.revealRequest = target
    }
}

private struct TaskPlanRow: View {
    let step: TaskPlan.Step
    let interrupted: Bool
    let isHighlighted: Bool
    let canOpen: Bool
    let open: () -> Void
    @State private var isHovered = false

    private var mark: (String, Color, String) {
        if interrupted { return ("pause.circle", .orange, "Interrupted: marked in progress when the turn ended") }
        return switch step.status {
        case .completed: ("checkmark.circle.fill", .green, "Marked done")
        case .inProgress: ("circle.dotted.circle", .accentColor, "In progress")
        case .blocked: ("exclamationmark.circle.fill", .orange, "Blocked")
        case .pending: ("circle", .secondary, "To do")
        case .unknown: ("questionmark.circle", .secondary, "Unknown status")
        }
    }

    var body: some View {
        Button(action: open) {
            HStack(alignment: .firstTextBaseline, spacing: 10) {
                Image(systemName: mark.0).foregroundStyle(mark.1).frame(width: 16).accessibilityHidden(true)
                VStack(alignment: .leading, spacing: 2) {
                    Text(step.content)
                        .font(.callout)
                        .foregroundStyle(step.status == .completed ? .secondary : .primary)
                        .lineLimit(3)
                    if step.status == .blocked, let reason = step.blockedReason {
                        Text(reason).font(.caption).foregroundStyle(.orange).lineLimit(2)
                    } else if interrupted {
                        Text("The turn ended while this was in progress").font(.caption).foregroundStyle(.secondary)
                    }
                }
                Spacer(minLength: 0)
                if canOpen {
                    Image(systemName: "arrow.turn.down.right").font(.caption).foregroundStyle(.tertiary)
                        .opacity(isHovered || isHighlighted ? 1 : 0)
                }
            }
            .padding(.horizontal, 8)
            .padding(.vertical, 7)
            .background(isHighlighted ? AnyShapeStyle(Color.brand.opacity(0.18))
                        : isHovered ? AnyShapeStyle(.fill.tertiary) : AnyShapeStyle(.clear), in: .rect(cornerRadius: 8))
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .disabled(!canOpen)
        .onHover { isHovered = $0 }
        .help(canOpen ? "Show where the agent last updated this step" : step.content)
        .accessibilityElement(children: .combine)
        .accessibilityLabel("\(mark.2): \(step.content)")
        .accessibilityHint(canOpen ? "Shows where the agent last updated this step" : "")
    }
}
