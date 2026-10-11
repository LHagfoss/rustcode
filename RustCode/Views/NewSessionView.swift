import RustCodeRemote
import SwiftUI

/// Browses the authenticated Mac's folders, never the iPhone's filesystem.
struct NewSessionView: View {
    @Environment(RemoteStore.self) private var store
    @Environment(\.dismiss) private var dismiss
    @State private var listing: DirectoryListing?
    @State private var isLoading = false
    @State private var error: String?
    @State private var browseTask: Task<Void, Never>?
    @State private var query = ""

    private var isBusy: Bool { isLoading || store.isCreatingSession }

    /// Folders of the sessions this Mac already lists, most recent first.
    private var recentFolders: [String] {
        var seen = Set<String>()
        return Array(store.sessions.map(\.workspace).filter { !$0.isEmpty && seen.insert($0).inserted }.prefix(4))
    }

    private var isPathQuery: Bool { query.hasPrefix("/") || query.hasPrefix("~") }

    private var visibleFolders: [RemoteDirectory] {
        guard let listing else { return [] }
        guard !query.isEmpty, !isPathQuery else { return listing.directories }
        return listing.directories.filter { $0.name.localizedStandardContains(query) }
    }

    var body: some View {
        NavigationStack {
            List {
                if let error {
                    Section {
                        Label(error, systemImage: "exclamationmark.circle")
                            .foregroundStyle(.red)
                            .fixedSize(horizontal: false, vertical: true)
                            .textSelection(.enabled)
                        if listing == nil { Button("Try Again") { load(nil) } }
                    }
                }
                if query.isEmpty, !recentFolders.isEmpty {
                    Section {
                        ForEach(recentFolders, id: \.self) { folder in
                            Button { start(in: folder) } label: {
                                FolderRow(name: Self.name(of: folder), detail: folder, symbol: "clock.arrow.circlepath", trailing: "plus.circle")
                            }
                            .accessibilityLabel("Start session in \(Self.name(of: folder))")
                            .accessibilityHint(folder)
                        }
                    } header: {
                        Text("Recent Folders")
                    } footer: {
                        Text("Tap a recent folder to start a session there.")
                    }
                    .disabled(isBusy)
                }
                if let listing {
                    Section {
                        if isPathQuery {
                            Button { open(query) } label: {
                                FolderRow(name: "Open \(query)", detail: nil, symbol: "arrow.right.circle", trailing: nil)
                            }
                        } else if let parent = listing.parent, query.isEmpty {
                            Button { load(parent) } label: {
                                FolderRow(name: Self.name(of: parent), detail: nil, symbol: "arrow.turn.up.left", trailing: nil)
                            }
                            .accessibilityLabel("Back to \(Self.name(of: parent))")
                        }
                        ForEach(visibleFolders) { directory in
                            Button { load(directory.path) } label: {
                                FolderRow(name: directory.name, detail: nil, symbol: "folder", trailing: "chevron.right")
                            }
                        }
                        if visibleFolders.isEmpty, !isPathQuery {
                            Text(query.isEmpty ? "No folders inside" : "No folders match “\(query)”")
                                .foregroundStyle(.secondary)
                        }
                    } header: {
                        HStack(spacing: 8) {
                            Text(listing.path).textCase(nil).lineLimit(1).truncationMode(.head)
                            if isLoading { ProgressView().controlSize(.small) }
                        }
                    } footer: {
                        if listing.truncated {
                            Text("Some folders are not shown. Type a path in the search field to open one directly.")
                        }
                    }
                    .disabled(isBusy)
                } else if isLoading {
                    ProgressView("Loading folders…")
                }
            }
            .scrollDismissesKeyboard(.interactively)
            .searchable(text: $query, placement: .navigationBarDrawer(displayMode: .always), prompt: "Filter folders or type a path")
            .textInputAutocapitalization(.never)
            .autocorrectionDisabled()
            .onSubmit(of: .search) { if isPathQuery { open(query) } }
            .navigationTitle("New Session")
            .navigationBarTitleDisplayMode(.inline)
            .softNavigationChrome()
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                        .disabled(store.isCreatingSession)
                }
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Home Folder", systemImage: "house") { load(nil) }
                        .disabled(isBusy)
                }
            }
            .safeAreaInset(edge: .bottom) { startBar }
            .interactiveDismissDisabled(store.isCreatingSession)
            .onAppear { if listing == nil { load(nil) } }
            .onDisappear { browseTask?.cancel() }
        }
    }

    /// The main action stays in reach and always names the folder it will use.
    private var startBar: some View {
        Button {
            if let listing { start(in: listing.path) }
        } label: {
            HStack(spacing: 8) {
                if store.isCreatingSession { ProgressView() }
                Text(store.isCreatingSession ? "Starting…" : "Start in \(listing.map { Self.name(of: $0.path) } ?? "Folder")")
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            .frame(maxWidth: .infinity)
        }
        .brandProminentButtonStyle()
        .controlSize(.large)
        .disabled(listing == nil || isBusy || store.connection != .connected)
        .accessibilityLabel("Start session in selected folder")
        .accessibilityValue(listing?.path ?? "")
        .padding(.horizontal, 16)
        .padding(.vertical, 8)
        .background(.bar)
    }

    private static func name(of path: String) -> String {
        let name = (path as NSString).lastPathComponent
        return name.isEmpty ? path : name
    }

    private func open(_ path: String) {
        query = ""
        load(path.trimmingCharacters(in: .whitespaces))
    }

    private func start(in folder: String) {
        error = nil
        Task {
            if await store.createSession(directory: folder) {
                dismiss()
            } else {
                error = store.notice ?? "The session could not be started. Check your connection and try again."
                store.notice = nil
            }
        }
    }

    /// The current listing stays on screen while the next one loads, and when it fails.
    private func load(_ requestedPath: String?) {
        browseTask?.cancel()
        isLoading = true
        error = nil
        browseTask = Task {
            do {
                let result = try await store.browseDirectories(path: requestedPath)
                guard !Task.isCancelled else { return }
                listing = result
                query = ""
                isLoading = false
            } catch {
                guard !Task.isCancelled else { return }
                self.error = error.localizedDescription
                isLoading = false
            }
        }
    }
}

private struct FolderRow: View {
    let name: String
    let detail: String?
    let symbol: String
    let trailing: String?

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: symbol).foregroundStyle(.tint).frame(width: 28).accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 2) {
                Text(name).foregroundStyle(.primary).lineLimit(1).truncationMode(.middle)
                if let detail {
                    Text(detail).font(.caption).foregroundStyle(.secondary).lineLimit(1).truncationMode(.head)
                }
            }
            Spacer(minLength: 4)
            if let trailing {
                Image(systemName: trailing).font(.footnote.weight(.semibold)).foregroundStyle(.tertiary).accessibilityHidden(true)
            }
        }
        .frame(minHeight: 36)
        .contentShape(.rect)
    }
}
