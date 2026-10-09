import RustCodeRemote
import SwiftUI

struct QuestionSheet: View {
    @Environment(RemoteStore.self) private var store
    @Environment(\.dismiss) private var dismiss
    let question: PendingQuestion

    @State private var selection: Set<String> = []
    @State private var freeText = ""
    @State private var isSubmitting = false

    private var answer: QuestionAnswer {
        QuestionAnswer(
            selectedOptionIDs: question.options.map(\.id).filter(selection.contains),
            text: freeText.isEmpty ? nil : freeText
        )
    }

    var body: some View {
        NavigationStack {
            List {
                Section {
                    Text(question.prompt)
                        .font(.headline)
                        .listRowBackground(Color.clear)
                        .listRowInsets(EdgeInsets(top: 8, leading: 4, bottom: 8, trailing: 4))
                }
                Section {
                    ForEach(question.options) { option in
                        Button {
                            toggle(option.id)
                        } label: {
                            HStack(spacing: 12) {
                                VStack(alignment: .leading, spacing: 3) {
                                    Text(option.label)
                                        .foregroundStyle(.primary)
                                    if let detail = option.detail {
                                        Text(detail)
                                            .font(.footnote)
                                            .foregroundStyle(.secondary)
                                    }
                                }
                                Spacer(minLength: 8)
                                Image(systemName: selection.contains(option.id) ? "checkmark.circle.fill" : "circle")
                                    .font(.title3)
                                    .foregroundStyle(selection.contains(option.id) ? AnyShapeStyle(.tint) : AnyShapeStyle(.tertiary))
                            }
                            .contentShape(.rect)
                        }
                        .buttonStyle(.plain)
                        .accessibilityAddTraits(selection.contains(option.id) ? .isSelected : [])
                    }
                } footer: {
                    if question.allowsMultipleSelection {
                        Text("You can choose more than one.")
                    }
                }
                if question.allowsFreeText {
                    Section("Or answer in your own words") {
                        TextField("Your answer", text: $freeText, axis: .vertical)
                            .lineLimit(1...5)
                    }
                }
            }
            .navigationTitle("Question")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Later") { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Send") { submit() }
                        .disabled(answer.isEmpty || isSubmitting)
                }
            }
        }
        .presentationDetents([.large])
    }

    private func toggle(_ id: String) {
        if selection.contains(id) {
            selection.remove(id)
        } else {
            if !question.allowsMultipleSelection { selection.removeAll() }
            selection.insert(id)
        }
    }

    private func submit() {
        isSubmitting = true
        Task {
            await store.answer(question, with: answer)
            isSubmitting = false
            dismiss()
        }
    }
}

/// Shows everything that is about to be approved, in full. A batch whose detail is still
/// only a preview can be denied but not approved.
struct ApprovalSheet: View {
    @Environment(RemoteStore.self) private var store
    @Environment(\.dismiss) private var dismiss
    let approval: PendingApproval

    @State private var isSubmitting = false

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 20) {
                    ForEach(approval.items) { item in
                        VStack(alignment: .leading, spacing: 8) {
                            Label(item.title, systemImage: "terminal")
                                .font(.headline)
                            Text(item.toolName)
                                .font(.footnote.monospaced())
                                .foregroundStyle(.secondary)
                            Text(item.detail)
                                .font(.callout.monospaced())
                                .textSelection(.enabled)
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .padding(12)
                                .background(.fill.quaternary, in: .rect(cornerRadius: 12))
                            if !item.isDetailComplete {
                                Label("Only a preview has loaded. The full action is needed before it can be approved.", systemImage: "exclamationmark.triangle")
                                    .font(.footnote)
                                    .foregroundStyle(.orange)
                            }
                        }
                    }
                    Text("Approving lets the session run exactly what is shown above, under its existing policy.")
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                }
                .padding(20)
            }
            .safeAreaInset(edge: .bottom, spacing: 0) {
                HStack(spacing: 12) {
                    Button(role: .destructive) {
                        resolve(.deny)
                    } label: {
                        Text("Deny").frame(maxWidth: .infinity)
                    }
                    .buttonStyle(.bordered)

                    Button {
                        resolve(.approve)
                    } label: {
                        Text("Approve").frame(maxWidth: .infinity)
                    }
                    .buttonStyle(.borderedProminent)
                    .disabled(!approval.canBeResolved)
                }
                .controlSize(.large)
                .disabled(isSubmitting)
                .padding(.horizontal, 20)
                .padding(.vertical, 12)
                .background(.bar)
            }
            .navigationTitle(approval.items.count == 1 ? "Approval" : "\(approval.items.count) Approvals")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Later") { dismiss() }
                }
            }
        }
        .presentationDetents([.large])
    }

    private func resolve(_ decision: ApprovalDecision) {
        isSubmitting = true
        Task {
            await store.resolve(approval, decision: decision)
            isSubmitting = false
            dismiss()
        }
    }
}
