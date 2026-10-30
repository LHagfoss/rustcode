import RustCodeRemote
import SwiftUI

/// The attached thread's uncommitted changes, for review on the phone (#2650): a summary, the
/// changed files, and each file's diff. Read-only. Lines can be selected to write a follow-up
/// into the thread's composer; nothing is sent until the user sends it.
struct ChangesView: View {
    let sessionID: String
    @Environment(RemoteStore.self) private var store
    @Environment(ChatDrafts.self) private var drafts
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            Group {
                switch store.changes {
                case .idle, .loading:
                    ProgressView("Reading changes…")
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                case .failed(let message):
                    ContentUnavailableView {
                        Label("No Changes to Show", systemImage: "exclamationmark.triangle")
                    } description: {
                        Text(message)
                    } actions: {
                        Button("Try Again") { Task { await store.loadChanges() } }
                    }
                case .loaded(let changes, _):
                    if !changes.isAvailable {
                        ContentUnavailableView("No Changes to Show", systemImage: "folder.badge.questionmark",
                                               description: Text(changes.reason ?? "The workspace can't be read."))
                    } else if changes.files.isEmpty {
                        ContentUnavailableView("No Uncommitted Changes", systemImage: "checkmark.circle",
                                               description: Text(summary(changes)))
                    } else {
                        list(changes)
                    }
                }
            }
            .navigationTitle("Changes")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Done") { dismiss() } }
                ToolbarItem(placement: .primaryAction) {
                    Button { Task { await store.loadChanges() } } label: { Image(systemName: "arrow.clockwise") }
                        .accessibilityLabel("Read Again")
                        .disabled(store.changes == .loading)
                }
            }
        }
        .task { if case .loaded = store.changes {} else { await store.loadChanges() } }
    }

    private func summary(_ changes: SessionChanges) -> String {
        var parts = [changes.freshness()]
        if let branch = changes.branch { parts.append(branch) }
        if let head = changes.head { parts.append("at \(head)") }
        return parts.joined(separator: " · ")
    }

    private func list(_ changes: SessionChanges) -> some View {
        List {
            Section {
                HStack {
                    Text("\(changes.files.count + changes.omittedFiles) files")
                    Spacer()
                    Text("+\(changes.additions)").foregroundStyle(.green)
                    Text("−\(changes.deletions)").foregroundStyle(.red)
                }
                .font(.subheadline.monospacedDigit())
                Label(summary(changes), systemImage: changes.isStale() ? "clock.badge.exclamationmark" : "clock")
                    .font(.footnote)
                    .foregroundStyle(changes.isStale() ? .orange : .secondary)
                if changes.omittedFiles > 0 {
                    Text("\(changes.omittedFiles) more files aren't listed.")
                        .font(.footnote).foregroundStyle(.secondary)
                }
            }
            Section("Files") {
                ForEach(changes.files) { file in
                    NavigationLink {
                        FileDiffView(file: file) { text in
                            let current = drafts.draft(for: sessionID).text
                            drafts.setText(current.isEmpty ? text : current + "\n" + text, for: sessionID)
                            dismiss()
                        }
                    } label: {
                        FileRow(file: file)
                    }
                }
            }
        }
        .refreshable { await store.loadChanges() }
    }
}

private struct FileRow: View {
    let file: SessionChanges.File

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            Text((file.path as NSString).lastPathComponent).font(.body.monospaced()).lineLimit(1)
            HStack(spacing: 6) {
                Text(file.status.rawValue.capitalized)
                Text((file.path as NSString).deletingLastPathComponent).lineLimit(1).truncationMode(.head)
                Spacer()
                if file.isBinary {
                    Text("Binary")
                } else {
                    Text("+\(file.additions)").foregroundStyle(.green)
                    Text("−\(file.deletions)").foregroundStyle(.red)
                }
            }
            .font(.caption.monospacedDigit())
            .foregroundStyle(.secondary)
        }
        .padding(.vertical, 2)
        .accessibilityElement(children: .combine)
    }
}

/// One file's diff. Tap lines to select them, then ask about them in the thread.
private struct FileDiffView: View {
    let file: SessionChanges.File
    let ask: (String) -> Void
    @State private var selected: Set<Int> = []

    var body: some View {
        let lines = file.lines
        // Vertical outside, sideways inside, so a short diff starts at the top.
        ScrollView(.vertical) {
            ScrollView(.horizontal) {
            LazyVStack(alignment: .leading, spacing: 0) {
                if file.isBinary {
                    note("A binary file; there are no lines to show.")
                } else if lines.isEmpty {
                    note("The host didn't send this file's diff.")
                }
                ForEach(lines) { line in
                    row(line)
                }
                if file.diffTruncated {
                    note("The diff was cut here to keep it small. See the rest on the computer.")
                }
            }
            .padding(.vertical, 8)
            }
        }
        .navigationTitle((file.path as NSString).lastPathComponent)
        .navigationBarTitleDisplayMode(.inline)
        .safeAreaInset(edge: .bottom) {
            if !selected.isEmpty {
                Button {
                    ask(SessionChanges.followUp(file: file, lines: lines.filter { selected.contains($0.id) }))
                } label: {
                    Label("Ask About \(selected.count) Line\(selected.count == 1 ? "" : "s")", systemImage: "text.bubble")
                        .frame(maxWidth: .infinity)
                }
                .brandProminentButtonStyle()
                .controlSize(.large)
                .padding()
                .accessibilityHint("Writes a message about these lines in the thread; it isn't sent")
            }
        }
    }

    private func row(_ line: SessionChanges.DiffLine) -> some View {
        let isSelected = selected.contains(line.id)
        return HStack(alignment: .firstTextBaseline, spacing: 8) {
            Text(line.number.map(String.init) ?? "")
                .frame(width: 40, alignment: .trailing)
                .foregroundStyle(.secondary)
            Text(line.kind == .hunk ? line.text : (line.kind == .added ? "+ " : line.kind == .removed ? "− " : "  ") + line.text)
                .foregroundStyle(line.kind == .hunk ? .secondary : .primary)
                .fixedSize()
        }
        .font(.caption.monospaced())
        .padding(.horizontal, 8)
        .padding(.vertical, 2)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(background(line, selected: isSelected))
        .contentShape(.rect)
        .onTapGesture {
            guard line.kind != .hunk else { return }
            if isSelected { selected.remove(line.id) } else { selected.insert(line.id) }
        }
        .accessibilityAddTraits(isSelected ? .isSelected : [])
    }

    private func background(_ line: SessionChanges.DiffLine, selected: Bool) -> Color {
        if selected { return Color.accentColor.opacity(0.25) }
        switch line.kind {
        case .added: return .green.opacity(0.12)
        case .removed: return .red.opacity(0.12)
        default: return .clear
        }
    }

    private func note(_ text: String) -> some View {
        Text(text).font(.footnote).foregroundStyle(.secondary).padding(.horizontal, 12).padding(.vertical, 8)
    }
}
