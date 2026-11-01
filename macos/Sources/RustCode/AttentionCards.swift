import RustCodeKit
import SwiftUI

/// A tool approval: what the engine wants to do in words, the command or
/// the edit as the transcript shows it, and one button per option. Raw
/// arguments stay behind "Details" for the tools it cannot shape.
struct PermissionCard: View {
    @ObservedObject var model: DesktopModel
    let request: PermissionRequest
    @State private var showsDetails = false

    private var preview: PermissionPreview { request.preview }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            header
            if let body = preview.body { content(body) }
            if !preview.notes.isEmpty {
                VStack(alignment: .leading, spacing: 4) {
                    ForEach(preview.notes, id: \.self) { note in
                        Label(note, systemImage: "exclamationmark.shield")
                            .font(.callout)
                            .foregroundStyle(Color.attention)
                    }
                }
            }
            if showsDetails, let input = request.input {
                ScrollView {
                    Text(input.prettyPrinted())
                        .font(.system(.caption, design: .monospaced))
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(10)
                }
                .frame(maxHeight: 160)
                .fixedSize(horizontal: false, vertical: true)
                .background(.fill.quaternary, in: .rect(cornerRadius: 8))
            }
            actions
        }
        .padding(16)
        .attentionCard()
    }

    private var header: some View {
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            Image(systemName: "hand.raised.fill")
                .foregroundStyle(Color.attention)
            VStack(alignment: .leading, spacing: 2) {
                Text(preview.headline)
                    .font(.headline)
                if let summary = preview.summary {
                    Text(summary)
                        .font(.callout)
                        .foregroundStyle(.secondary)
                        .textSelection(.enabled)
                }
            }
            Spacer(minLength: 8)
            Text("Needs approval")
                .font(.caption.weight(.medium))
                .foregroundStyle(Color.attention)
            if request.input != nil {
                Button(showsDetails ? "Hide details" : "Details") {
                    withAnimation(.snappy(duration: 0.2)) { showsDetails.toggle() }
                }
                .buttonStyle(.link)
                .font(.caption)
                .pointingHandCursor()
            }
        }
    }

    @ViewBuilder
    private func content(_ body: PermissionPreview.Body) -> some View {
        switch body {
        case .command(let command, let cwd):
            VStack(alignment: .leading, spacing: 4) {
                ScrollView {
                    (Text("$ ").foregroundStyle(.secondary) + Text(command))
                        .font(.system(.callout, design: .monospaced))
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(10)
                }
                .frame(maxHeight: 140)
                .fixedSize(horizontal: false, vertical: true)
                .background(.fill.quaternary, in: .rect(cornerRadius: 8))
                if let cwd {
                    Label(cwd, systemImage: "folder")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
            }
        case .edit(let change):
            ScrollView {
                FileChangeView(change: change, showsHeader: true)
                    .padding(.trailing, 4)
            }
            .frame(maxHeight: 260)
            .fixedSize(horizontal: false, vertical: true)
        case .fields(let fields):
            Grid(alignment: .leadingFirstTextBaseline, horizontalSpacing: 12, verticalSpacing: 6) {
                ForEach(fields) { field in
                    GridRow {
                        Text(field.label)
                            .foregroundStyle(.secondary)
                            .gridColumnAlignment(.trailing)
                        Text(field.value)
                            .font(.system(.callout, design: .monospaced))
                            .textSelection(.enabled)
                            .lineLimit(6)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                }
            }
            .font(.callout)
        }
    }

    private var actions: some View {
        HStack(spacing: 8) {
            if request.autoApproveOption != nil {
                Button("Auto-approve from now on") { Task { await model.setAutoApprove(true) } }
                    .buttonStyle(.link)
                    .font(.callout)
                    .pointingHandCursor()
                    .help("Approve every tool call without asking. Turn it off in Settings.")
            }
            Spacer()
            // Reject first and the one-time allow last, so the main action
            // sits at the trailing edge like a dialog's default button.
            ForEach(ordered) { option in
                let button = Button(label(option)) { Task { await model.answer(request, with: option) } }
                    .help(option.name)
                if option.kind == .allowOnce {
                    button.brandProminent()
                } else {
                    button.buttonStyle(PillButtonStyle())
                }
            }
        }
        .controlSize(.large)
    }

    private var ordered: [PermissionOption] {
        let rank: (PermissionOption) -> Int = {
            switch $0.kind {
            case .rejectOnce?, .rejectAlways?: 0
            case .allowAlways?: 1
            case .allowOnce?: 2
            case nil: 1
            }
        }
        return request.options.enumerated()
            .sorted { (rank($0.element), $0.offset) < (rank($1.element), $1.offset) }
            .map(\.element)
    }

    /// The engine names an always-allow after the whole command; the button
    /// keeps it short and the tooltip keeps the full name.
    private func label(_ option: PermissionOption) -> String {
        if option.kind == .allowAlways, option.name.contains("`") { return "Always allow this" }
        return option.name
    }
}

/// An `ask_question` from the engine: one button per option, ticks and a
/// Submit for multi-select questions, and a free-text answer.
struct QuestionCard: View {
    @ObservedObject var model: DesktopModel
    let request: PermissionRequest
    @State private var ticked: Set<String> = []
    @State private var custom = ""

    var body: some View {
        let question = request.question!
        VStack(alignment: .leading, spacing: 12) {
            // A question is a conversation, not a warning: the brand, not amber.
            Label(question.header.isEmpty ? "RustCode has a question" : question.header, systemImage: "questionmark.bubble.fill")
                .font(.headline)
                .foregroundStyle(Color.brand)
            Text(question.text).textSelection(.enabled)
            if question.multiple {
                VStack(alignment: .leading, spacing: 6) {
                    ForEach(request.answerOptions) { option in
                        Toggle(option.name, isOn: Binding(
                            get: { ticked.contains(option.id) },
                            set: { if $0 { ticked.insert(option.id) } else { ticked.remove(option.id) } }))
                            .toggleStyle(.checkbox)
                    }
                }
            } else {
                VStack(alignment: .leading, spacing: 4) {
                    ForEach(request.answerOptions) { option in
                        AnswerRow(title: option.name) { Task { await model.answer(request, with: option) } }
                    }
                }
            }
            HStack(spacing: 8) {
                TextField("Or write your own answer…", text: $custom)
                    .textFieldStyle(.roundedBorder)
                    .onSubmit { Task { await model.answer(request, text: custom) } }
                Button("Send") { Task { await model.answer(request, text: custom) } }
                    .buttonStyle(PillButtonStyle(prominent: true))
                    .disabled(custom.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            }
            HStack {
                Spacer()
                if let reject = request.rejectOption {
                    Button(reject.name) { Task { await model.answer(request, with: reject) } }
                        .buttonStyle(PillButtonStyle())
                }
                if question.multiple {
                    Button("Submit") {
                        let chosen = request.answerOptions.filter { ticked.contains($0.id) }
                        Task { await model.answer(request, choosing: chosen) }
                    }
                    .brandProminent()
                    .disabled(ticked.isEmpty)
                }
            }
        }
        .padding(16)
        .attentionCard()
    }
}

/// One choice of a question: a full-width row that lights under the pointer.
private struct AnswerRow: View {
    let title: String
    let action: () -> Void
    @State private var isHovered = false

    var body: some View {
        Button(action: action) {
            HStack {
                Text(title).multilineTextAlignment(.leading)
                Spacer(minLength: 8)
                Image(systemName: "arrow.up.circle.fill")
                    .foregroundStyle(Color.brand)
                    .opacity(isHovered ? 1 : 0)
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 9)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(isHovered ? AnyShapeStyle(Color.brand.opacity(0.14)) : AnyShapeStyle(.fill.quaternary),
                        in: .rect(cornerRadius: 12))
            .contentShape(.rect(cornerRadius: 12))
        }
        .buttonStyle(.plain)
        .onHover { isHovered = $0 }
    }
}

private extension View {
    /// An approval or question above the composer: the blurred material with a
    /// neutral hairline. The header's symbol carries the state, not the edge.
    func attentionCard() -> some View {
        blurSurface(in: .rect(cornerRadius: 20))
            .overlay { RoundedRectangle(cornerRadius: 20).strokeBorder(Color.primary.opacity(0.1), lineWidth: 1) }
    }
}
