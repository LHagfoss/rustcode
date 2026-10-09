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
                    VStack(alignment: .leading, spacing: 6) {
                        if !question.header.isEmpty || question.chainLength > 1 {
                            Text(caption)
                                .font(.footnote.weight(.semibold))
                                .foregroundStyle(.secondary)
                        }
                        Text(question.prompt)
                            .font(.headline)
                    }
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
                    Section {
                        TextField("Your answer", text: $freeText, axis: .vertical)
                            .lineLimit(1...5)
                    } header: {
                        Text("Or answer in your own words")
                    } footer: {
                        if !freeText.isEmpty, !selection.isEmpty {
                            Text("Your own answer is sent instead of the selected option.")
                        }
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

    private var caption: String {
        let position = question.chainLength > 1 ? "Question \(question.position) of \(question.chainLength)" : ""
        return [question.header, position].filter { !$0.isEmpty }.joined(separator: " · ")
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
    let batchID: String
    /// What was pending when the sheet opened; the store's copy replaces it as details load.
    let initial: PendingApproval

    @State private var isSubmitting = false

    private var approval: PendingApproval {
        if let current = store.attached?.pendingApproval, current.batchID == batchID { return current }
        return initial
    }

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
                            if !item.risk.isEmpty {
                                Text(item.risk)
                                    .font(.footnote)
                                    .foregroundStyle(.secondary)
                            }
                            Text(item.detail)
                                .font(.callout.monospaced())
                                .textSelection(.enabled)
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .padding(12)
                                .background(.fill.quaternary, in: .rect(cornerRadius: 12))
                            if !item.isDetailComplete {
                                incompleteNote(for: item)
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
            .task { await store.loadApprovalDetails() }
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

    @ViewBuilder
    private func incompleteNote(for item: PendingApproval.Item) -> some View {
        if store.isLoadingApprovalDetails {
            HStack(spacing: 8) {
                ProgressView().controlSize(.small)
                Text("Loading the full action…")
            }
            .font(.footnote)
            .foregroundStyle(.secondary)
        } else if item.detailContentID == nil
                    || (item.detailTotalBytes ?? 0) > RemoteStore.maximumApprovalDetailBytes {
            Label("This is only the beginning of the action and the rest cannot be shown here. Review it in the terminal; from here it can only be denied.", systemImage: "exclamationmark.triangle")
                .font(.footnote)
                .foregroundStyle(.orange)
        } else {
            VStack(alignment: .leading, spacing: 6) {
                Label("Only the beginning has loaded. Approving needs the full action.", systemImage: "exclamationmark.triangle")
                    .font(.footnote)
                    .foregroundStyle(.orange)
                Button("Load Again") {
                    Task { await store.loadApprovalDetails() }
                }
                .font(.footnote.weight(.semibold))
            }
        }
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
