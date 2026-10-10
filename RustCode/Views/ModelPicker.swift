import RustCodeRemote
import SwiftUI

/// A bounded popover remains usable with large profile catalogs on both iPhone and iPad.
struct ModelPicker: View {
    @Environment(RemoteStore.self) private var store
    @Environment(\.dismiss) private var dismiss
    @State private var query = ""
    @ScaledMetric(relativeTo: .body) private var estimatedRowHeight: CGFloat = 56

    private var contentHeight: CGFloat {
        // Use the full catalog count so typing a search does not move the popover.
        let count = max(store.attached?.settings?.models.count ?? 0, 1)
        let busyCaption: CGFloat = store.attached?.settings?.canChange == false ? 32 : 0
        return min(360, 104 + estimatedRowHeight * CGFloat(count) + busyCaption)
    }

    private var choices: [SessionModelChoice] {
        let models = store.attached?.settings?.models ?? []
        guard !query.isEmpty else { return models }
        return models.filter { $0.id.localizedStandardContains(query) || $0.model.localizedStandardContains(query) }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text("Model").font(.headline)
                Spacer()
                Button("Done") { dismiss() }
                    .font(.subheadline.weight(.semibold))
                    .frame(minWidth: 44, minHeight: 44)
            }
            .padding(.horizontal, 16).frame(height: 44)
            HStack(spacing: 8) {
                Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
                TextField("Search models", text: $query)
                    .font(.body)
                    .frame(maxWidth: .infinity, minHeight: 44)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .accessibilityLabel("Search models")
            }
            .padding(.horizontal, 12).frame(height: 44)
            .background(.fill.quaternary, in: .rect(cornerRadius: 10))
            .padding(.horizontal, 12).padding(.bottom, 8)
            if store.attached?.settings?.canChange == false {
                Text("Available after the current turn stops")
                    .font(.caption).foregroundStyle(.secondary)
                    .padding(.horizontal, 16).padding(.bottom, 8)
            }
            Divider()
            ScrollView {
                LazyVStack(spacing: 0) {
                    ForEach(choices, id: \.id) { choice in
                        Button {
                            guard let settings = store.attached?.settings else { return }
                            let effort = choice.reasoningEfforts.contains(settings.reasoningEffort) ? settings.reasoningEffort : "default"
                            Task {
                                if await store.setSessionSettings(model: choice.id, reasoningEffort: effort) { dismiss() }
                            }
                        } label: {
                            HStack(spacing: 8) {
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(choice.model).font(.body).lineLimit(2)
                                    if choice.id != choice.model {
                                        Text(choice.id).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                                    }
                                }
                                Spacer(minLength: 8)
                                if choice.id == store.attached?.settings?.selectedModel {
                                    Image(systemName: "checkmark").foregroundStyle(.tint)
                                }
                            }
                            .padding(.horizontal, 16).padding(.vertical, 8)
                            .frame(minHeight: 44)
                            .contentShape(.rect)
                        }
                        .buttonStyle(.plain)
                        .disabled(store.attached?.settings?.canChange != true || store.connection != .connected || store.hasPendingSettings)
                        .accessibilityLabel(choice.id == choice.model ? choice.model : "\(choice.model) · \(choice.id)")
                    }
                    if choices.isEmpty {
                        Text("No matching models").foregroundStyle(.secondary).padding()
                    }
                }
            }
            .scrollDismissesKeyboard(.interactively)
            .accessibilityIdentifier("modelChoices")
        }
        .frame(width: 320, height: contentHeight)
    }
}
