import RustCodeRemote
import SwiftUI

/// Model and effort share one native sheet and the host remains the source of available choices.
struct ModelPicker: View {
    @Environment(RemoteStore.self) private var store
    @Environment(\.dismiss) private var dismiss
    @State private var query = ""
    @State private var isSearching = false

    private var choices: [SessionModelChoice] {
        let models = store.attached?.settings?.models ?? []
        guard !query.isEmpty else { return models }
        return models.filter { $0.id.localizedStandardContains(query) || $0.model.localizedStandardContains(query) }
    }

    private var canChange: Bool {
        store.attached?.settings?.canChange == true && store.connection == .connected && !store.hasPendingSettings
    }

    var body: some View {
        NavigationStack {
            List {
                if let settings = store.attached?.settings {
                    let current = settings.models.first { $0.id == settings.selectedModel }
                    Section {
                        Picker("Thinking effort", selection: Binding(get: { settings.reasoningEffort }, set: { effort in
                            Task { await store.setSessionSettings(model: settings.selectedModel, reasoningEffort: effort) }
                        })) {
                            ForEach(current?.reasoningEfforts ?? [], id: \.self) { effort in
                                Text(SessionModelChoice.effortLabel(effort)).tag(effort)
                            }
                        }
                        .disabled(!canChange)
                        .accessibilityIdentifier("thinkingEffortPicker")
                        .accessibilityValue(settings.reasoningEffort)
                    } footer: {
                        if !settings.canChange { Text("Model and effort can change after the current turn stops.") }
                    }
                    Section("Models") {
                        ForEach(choices, id: \.id) { choice in
                            Button {
                                let effort = choice.reasoningEfforts.contains(settings.reasoningEffort) ? settings.reasoningEffort : "default"
                                Task {
                                    if await store.setSessionSettings(model: choice.id, reasoningEffort: effort) {
                                        isSearching = false
                                        query = ""
                                    }
                                }
                            } label: {
                                HStack(spacing: 12) {
                                    VStack(alignment: .leading, spacing: 3) {
                                        Text(choice.model).font(.body).foregroundStyle(.primary)
                                        if choice.id != choice.model { Text(choice.id).font(.caption).foregroundStyle(.secondary) }
                                    }
                                    Spacer(minLength: 8)
                                    if choice.id == settings.selectedModel { Image(systemName: "checkmark").foregroundStyle(.tint) }
                                }
                                .frame(minHeight: 44).contentShape(.rect)
                            }
                            .disabled(!canChange)
                            .accessibilityLabel(choice.id == choice.model ? choice.model : "\(choice.model) · \(choice.id)")
                        }
                        if choices.isEmpty { Text("No matching models").foregroundStyle(.secondary) }
                    }
                }
            }
            .searchable(text: $query, isPresented: $isSearching, placement: .navigationBarDrawer(displayMode: .always), prompt: "Search models")
            .scrollDismissesKeyboard(.interactively)
            .sensoryFeedback(.selection, trigger: store.attached?.settings?.selectedModel)
            .sensoryFeedback(.selection, trigger: store.attached?.settings?.reasoningEffort)
            .navigationTitle("Model & Effort")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } } }
        }
        .presentationDetents([.medium, .large])
        .presentationDragIndicator(.visible)
    }
}
