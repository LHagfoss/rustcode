import RustCodeRemote
import SwiftUI

struct QuestionSheet: View {
    @Environment(RemoteStore.self) private var store
    let question: PendingQuestion

    @State private var selection: Set<String> = []
    @State private var freeText = ""
    @State private var usesCustomAnswer = false
    @State private var submissionFailure: String?
    @State private var awaitingNextQuestion = false
    @State private var isCancelling = false
    @FocusState private var isAnswerFocused: Bool
    @State private var isSubmitting = false

    private var answer: QuestionAnswer {
        QuestionAnswer(
            selectedOptionIDs: usesCustomAnswer ? [] : question.options.map(\.id).filter(selection.contains),
            text: usesCustomAnswer ? freeText.trimmingCharacters(in: .whitespacesAndNewlines) : nil
        )
    }

    var body: some View {
        NavigationStack {
            ScrollViewReader { proxy in
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
                                .textSelection(.enabled)
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
                                    Image(systemName: !usesCustomAnswer && selection.contains(option.id) ? "checkmark.circle.fill" : "circle")
                                        .font(.title3)
                                        .foregroundStyle(!usesCustomAnswer && selection.contains(option.id) ? AnyShapeStyle(.tint) : AnyShapeStyle(.tertiary))
                                }
                                .contentShape(.rect)
                            }
                            .buttonStyle(.plain)
                            .accessibilityAddTraits(!usesCustomAnswer && selection.contains(option.id) ? .isSelected : [])
                        }
                    } footer: {
                        if question.allowsMultipleSelection {
                            Text("You can choose more than one.")
                        }
                    }
                    if question.allowsFreeText {
                        Section {
                            Button {
                                if usesCustomAnswer { isAnswerFocused = true }
                                usesCustomAnswer = true
                            } label: {
                                HStack {
                                    Text("Write your own answer").foregroundStyle(.primary)
                                    Spacer()
                                    Image(systemName: usesCustomAnswer ? "checkmark.circle.fill" : "circle")
                                        .foregroundStyle(usesCustomAnswer ? AnyShapeStyle(.tint) : AnyShapeStyle(.tertiary))
                                }
                                .frame(minHeight: 44)
                                .contentShape(.rect)
                            }
                            .buttonStyle(.plain)
                            .accessibilityAddTraits(usesCustomAnswer ? .isSelected : [])
                            if usesCustomAnswer {
                                TextField("Your answer", text: $freeText, axis: .vertical)
                                    .lineLimit(3...8)
                                    .focused($isAnswerFocused)
                                    .accessibilityLabel("Your answer")
                                    .id("custom-answer")
                                    .task {
                                        // Focus after the conditional field has joined the view tree.
                                        await Task.yield()
                                        isAnswerFocused = true
                                    }
                            }
                        } footer: {
                            if usesCustomAnswer {
                                HStack {
                                    Text("Your written answer will be sent.")
                                    Spacer()
                                    if isAnswerFocused {
                                        Button { isAnswerFocused = false } label: {
                                            Image(systemName: "keyboard.chevron.compact.down")
                                                .frame(width: 44, height: 44)
                                        }
                                        .accessibilityLabel("Dismiss answer keyboard")
                                    }
                                }
                            }
                        }
                    }
                    if let submissionFailure {
                        Section {
                            Text(submissionFailure)
                                .foregroundStyle(.red)
                        }
                    }
                }
                .scrollDismissesKeyboard(.interactively)
                .onReceive(NotificationCenter.default.publisher(for: UIResponder.keyboardDidShowNotification)) { _ in
                    guard isAnswerFocused else { return }
                    withAnimation(.easeOut(duration: 0.2)) { proxy.scrollTo("custom-answer", anchor: .center) }
                }
                .onChange(of: isAnswerFocused) { _, focused in
                    if focused { proxy.scrollTo("custom-answer", anchor: .center) }
                }
                .onAppear { usesCustomAnswer = question.options.isEmpty && question.allowsFreeText }
                #if DEBUG
                .onAppear {
                    if LaunchOptions.current.selectsFirstOption, let first = question.options.first {
                        selection = [first.id]
                    }
                }
                #endif
                .sensoryFeedback(.selection, trigger: selection)
                .sensoryFeedback(.success, trigger: isSubmitting) { _, isSubmitting in isSubmitting }
                .navigationTitle("Question")
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) {
                        Button("Cancel", role: .destructive) { cancelQuestions() }
                            .accessibilityLabel("Cancel questions")
                            .disabled(isCancelling || store.connection != .connected || store.isQuestionCancellationPending(question.id))
                    }
                    ToolbarItem(placement: .confirmationAction) {
                        Button(question.position < question.chainLength ? "Next" : "Send") { submit() }
                            .disabled(answer.isEmpty || isSubmitting || awaitingNextQuestion || store.connection != .connected || hasPendingAnswer)
                    }
                }
            }
        }
        .presentationDetents([.large])
    }

    private var caption: String {
        let position = question.chainLength > 1 ? "Question \(question.position) of \(question.chainLength)" : ""
        return [question.header, position].filter { !$0.isEmpty }.joined(separator: " · ")
    }

    private var hasPendingAnswer: Bool {
        store.isQuestionPending(question.id)
    }

    private func toggle(_ id: String) {
        isAnswerFocused = false
        submissionFailure = nil
        if usesCustomAnswer {
            usesCustomAnswer = false
            if !question.allowsMultipleSelection { selection.removeAll() }
            selection.insert(id)
            return
        }
        if selection.contains(id) {
            selection.remove(id)
        } else {
            if !question.allowsMultipleSelection { selection.removeAll() }
            selection.insert(id)
        }
    }

    private func submit() {
        guard !isSubmitting, !answer.isEmpty, !hasPendingAnswer else { return }
        let submittedAnswer = answer
        isAnswerFocused = false
        isSubmitting = true
        submissionFailure = nil
        Task {
            let accepted = await store.answer(question, with: submittedAnswer)
            isSubmitting = false
            if accepted {
                // The host advances the sheet to the next question, or closes it when
                // the chain ends. A receipt alone must not hide an unanswered question.
                awaitingNextQuestion = !store.isQuestionPending(question.id)
            } else {
                submissionFailure = "Your answer wasn’t sent. Your draft is still here; check the connection and try again."
            }
        }
    }

    private func cancelQuestions() {
        guard !isCancelling, !store.isQuestionCancellationPending(question.id) else { return }
        isAnswerFocused = false
        isCancelling = true
        Task {
            let accepted = await store.cancelQuestion(question)
            isCancelling = false
            if !accepted { submissionFailure = "Couldn’t cancel these questions. Check the connection; an older remote gateway may need restarting on your Mac." }
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
    @State private var decision: ApprovalDecision?
    @State private var submissionFailed = false

    private var approval: PendingApproval {
        if let current = store.attached?.pendingApproval, current.batchID == batchID { return current }
        return initial
    }

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 20) {
                    if submissionFailed {
                        Text("The action wasn’t confirmed. Review the connection and try again.")
                            .foregroundStyle(.red)
                    }
                    if store.isApprovalPending(batchID) {
                        Label("Waiting for your Mac…", systemImage: "clock").foregroundStyle(.secondary)
                    }
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
                    Text("Approving lets the thread run exactly what is shown above, under its existing policy.")
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                }
                .padding(20)
            }
            .floatingBottomBar {
                HStack(spacing: 12) {
                    Button(role: .destructive) {
                        resolve(.deny)
                    } label: {
                        Text("Deny").frame(maxWidth: .infinity)
                    }
                    .secondaryButtonStyle()
                    .tint(.red)

                    Button {
                        resolve(.approve)
                    } label: {
                        Text("Approve").frame(maxWidth: .infinity)
                    }
                    .brandProminentButtonStyle()
                    .disabled(!approval.canBeResolved)
                }
                .controlSize(.large)
                .disabled(isSubmitting || store.isApprovalPending(batchID) || store.connection != .connected)
                .padding(.horizontal, 20)
                .padding(.vertical, 12)
            }
            .sensoryFeedback(trigger: decision) { _, decision in
                switch decision {
                case .approve: .success
                case .deny: .warning
                default: nil
                }
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
                .foregroundStyle(.attention)
        } else {
            VStack(alignment: .leading, spacing: 6) {
                Label("Only the beginning has loaded. Approving needs the full action.", systemImage: "exclamationmark.triangle")
                    .font(.footnote)
                    .foregroundStyle(.attention)
                Button("Load Again") {
                    Task { await store.loadApprovalDetails() }
                }
                .font(.footnote.weight(.semibold))
            }
        }
    }

    private func resolve(_ decision: ApprovalDecision) {
        guard !isSubmitting, !store.isApprovalPending(batchID) else { return }
        self.decision = decision
        isSubmitting = true
        submissionFailed = false
        Task {
            let accepted = await store.resolve(approval, decision: decision)
            isSubmitting = false
            if !accepted { submissionFailed = true }
        }
    }
}
