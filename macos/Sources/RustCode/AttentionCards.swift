import RustCodeKit
import SwiftUI

/// A tool approval: what the engine wants to run and one button per option.
struct PermissionCard: View {
    @ObservedObject var model: DesktopModel
    let request: PermissionRequest

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Label("Permission requested", systemImage: "hand.raised.fill")
                .font(.headline)
                .foregroundStyle(Color.attention)
            Text(request.title).textSelection(.enabled)
            if let input = request.input {
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
            HStack {
                Spacer()
                ForEach(request.options) { option in
                    if option.kind?.isAllow == true {
                        Button(option.name) { Task { await model.answer(request, with: option) } }
                            .brandProminent()
                    } else {
                        Button(option.name) { Task { await model.answer(request, with: option) } }
                            .buttonStyle(.bordered)
                    }
                }
            }
            .controlSize(.large)
        }
        .padding(16)
        .floatingSurface(in: .rect(cornerRadius: 20))
        .overlay { RoundedRectangle(cornerRadius: 20).strokeBorder(Color.attention.opacity(0.5), lineWidth: 1) }
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
            Label(question.header.isEmpty ? "RustCode has a question" : question.header, systemImage: "questionmark.bubble.fill")
                .font(.headline)
                .foregroundStyle(Color.attention)
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
                VStack(alignment: .leading, spacing: 6) {
                    ForEach(request.answerOptions) { option in
                        Button { Task { await model.answer(request, with: option) } } label: {
                            Text(option.name).frame(maxWidth: .infinity, alignment: .leading)
                        }
                        .buttonStyle(.bordered)
                        .controlSize(.large)
                    }
                }
            }
            HStack(spacing: 8) {
                TextField("Or write your own answer…", text: $custom)
                    .textFieldStyle(.roundedBorder)
                    .onSubmit { Task { await model.answer(request, text: custom) } }
                Button("Send") { Task { await model.answer(request, text: custom) } }
                    .disabled(custom.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            }
            HStack {
                Spacer()
                if let reject = request.rejectOption {
                    Button(reject.name) { Task { await model.answer(request, with: reject) } }
                        .buttonStyle(.bordered)
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
        .floatingSurface(in: .rect(cornerRadius: 20))
        .overlay { RoundedRectangle(cornerRadius: 20).strokeBorder(Color.attention.opacity(0.5), lineWidth: 1) }
    }
}
