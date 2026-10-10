import RustCodeRemote
import SwiftUI

/// Browses the authenticated Mac's folders, never the iPhone's filesystem.
struct NewSessionView: View {
    @Environment(RemoteStore.self) private var store
    @Environment(\.dismiss) private var dismiss
    @State private var listing: DirectoryListing?
    @State private var path = "~"
    @State private var isLoading = false
    @State private var error: String?
    @State private var browseTask: Task<Void, Never>?
    @FocusState private var pathFocused: Bool

    var body: some View {
        NavigationStack {
            List {
                Section {
                    HStack(spacing: 8) {
                        TextField("Folder path", text: $path)
                            .font(.body.monospaced())
                            .textInputAutocapitalization(.never)
                            .autocorrectionDisabled()
                            .submitLabel(.go)
                            .focused($pathFocused)
                            .onSubmit { load(path) }
                            .accessibilityLabel("Session folder path")
                        Button("Open", systemImage: "arrow.right") { load(path) }
                            .labelStyle(.iconOnly)
                            .frame(minWidth: 44, minHeight: 44)
                            .disabled(isLoading || store.isCreatingSession)
                    }
                    Button("Home Folder", systemImage: "house") { load(nil) }
                        .disabled(isLoading || store.isCreatingSession)
                } header: {
                    Text("Folder on \(store.host?.name ?? "your Mac")")
                } footer: {
                    Text("The session runs on your Mac. Choose Home or browse to a project folder.")
                }
                if let error {
                    Section {
                        Label(error, systemImage: "exclamationmark.circle")
                            .foregroundStyle(.secondary)
                        Button("Try Again") { load(path) }
                    }
                }
                if isLoading {
                    ProgressView("Loading folders…")
                } else if let listing {
                    Section {
                        if let parent = listing.parent {
                            Button("Parent Folder", systemImage: "arrow.turn.up.left") { load(parent) }
                        }
                        ForEach(listing.directories, id: \.path) { directory in
                            Button { load(directory.path) } label: {
                                HStack {
                                    Label(directory.name, systemImage: "folder")
                                        .foregroundStyle(.primary)
                                    Spacer()
                                    Image(systemName: "chevron.right")
                                        .font(.caption.weight(.semibold))
                                        .foregroundStyle(.tertiary)
                                }
                                .frame(minHeight: 32)
                            }
                        }
                        if listing.directories.isEmpty {
                            Text("No folders here")
                                .foregroundStyle(.secondary)
                        }
                    } footer: {
                        if listing.truncated {
                            Text("Some folders are not shown. Enter a path above to open one directly.")
                        }
                    }
                    .disabled(store.isCreatingSession)
                }
            }
            .scrollDismissesKeyboard(.interactively)
            .navigationTitle("New Session")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                        .disabled(store.isCreatingSession)
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button {
                        guard let listing else { return }
                        pathFocused = false
                        Task {
                            if await store.createSession(directory: listing.path) {
                                dismiss()
                            } else {
                                error = store.notice ?? "The session could not be started. Check your connection and try again."
                                store.notice = nil
                            }
                        }
                    } label: {
                        if store.isCreatingSession { ProgressView() } else { Text("Start") }
                    }
                    .disabled(listing == nil || path != listing?.path || isLoading || store.isCreatingSession || store.connection != .connected)
                    .accessibilityLabel("Start session in selected folder")
                }
            }
            .interactiveDismissDisabled(store.isCreatingSession)
            .onAppear { if listing == nil { load(nil) } }
            .onDisappear { browseTask?.cancel() }
        }
    }

    private func load(_ requestedPath: String?) {
        browseTask?.cancel()
        pathFocused = false
        isLoading = true
        path = requestedPath ?? "~"
        error = nil
        listing = nil
        browseTask = Task {
            do {
                let result = try await store.browseDirectories(path: requestedPath)
                guard !Task.isCancelled else { return }
                listing = result
                path = result.path
                isLoading = false
            } catch {
                guard !Task.isCancelled else { return }
                self.error = error.localizedDescription
                isLoading = false
            }
        }
    }
}
