import RustCodeKit
import SwiftUI

/// The open chat's project changes, read with `git` and never changed from
/// here (#2642): no staging, discarding or checkout. Two scopes: the
/// working tree against its last commit, and the branch against where it
/// left a base branch. Each load says what it compared and what it left out.
@MainActor
final class ChangesStore: ObservableObject {
    enum Phase: Equatable {
        case idle
        case loading
        case loaded(ChangesSnapshot)
        case failed(String)
    }

    @Published private(set) var phase: Phase = .idle
    @Published var scope: ChangesScope = .workingTree
    @Published private(set) var branches: [String] = []
    private var folder: String?
    private var load: Task<Void, Never>?
    private let loader = GitChangesLoader()

    /// Read again for `folder`; a newer call replaces one still running, so
    /// an older answer never lands over a newer one.
    func refresh(folder: String) {
        if folder != self.folder {
            self.folder = folder
            scope = .workingTree
            branches = []
        }
        load?.cancel()
        let scope = scope
        let loader = loader
        if case .loaded = phase {} else { phase = .loading }
        load = Task { [weak self] in
            let result: Phase
            do {
                result = .loaded(try await loader.load(folder: folder, scope: scope))
            } catch let error as ChangesError {
                result = .failed(error.message)
            } catch {
                result = .failed(error.localizedDescription)
            }
            let branches = await Task.detached { loader.branches(folder: folder) }.value
            guard !Task.isCancelled, let self, self.folder == folder, self.scope == scope else { return }
            self.phase = result
            self.branches = branches
        }
    }

    /// Compare with a branch: `base`, else the repository's default branch.
    func compare(with base: String?, folder: String) {
        if let base {
            scope = .branch(base: base)
        } else if let found = loader.defaultBase(folder: folder) {
            scope = .branch(base: found)
        } else {
            phase = .failed("No main or master branch here to compare with. Pick a base branch.")
            return
        }
        refresh(folder: folder)
    }
}

struct ChangesPanel: View {
    @ObservedObject var model: DesktopModel
    @StateObject private var store = ChangesStore()
    @State private var expanded: Set<String> = []
    /// The file whose lines are being picked for a message (#2643).
    @State private var picking: String?
    let close: () -> Void

    private var folder: String { model.chatProject.path }

    var body: some View {
        VStack(spacing: 0) {
            toolbar
            Divider().opacity(0.5)
            content
        }
        .background(Color.chatBackground)
        .task(id: folder) { store.refresh(folder: folder) }
        #if DEBUG
        .onChange(of: model.debugPickFirstFile) { _, pick in
            guard pick, case .loaded(let snapshot) = store.phase, let file = snapshot.files.first else { return }
            expanded.insert(file.id)
            picking = file.id
            model.debugPickFirstFile = false
        }
        #endif
        // A turn that ends may have changed files: read again.
        .onChange(of: model.runningSessions.contains(model.selectedSession ?? "")) { _, running in
            if !running { store.refresh(folder: folder) }
        }
    }

    private var toolbar: some View {
        HStack(spacing: 8) {
            Picker("Compare", selection: Binding(
                get: { store.scope == .workingTree ? 0 : 1 },
                set: { choice in
                    if choice == 0 {
                        store.scope = .workingTree
                        store.refresh(folder: folder)
                    } else {
                        store.compare(with: nil, folder: folder)
                    }
                })) {
                Text("Uncommitted").tag(0)
                Text("Branch").tag(1)
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .fixedSize()
            .help("Uncommitted: the files on disk against the last commit. Branch: everything since the branch left its base.")
            if case .branch(let base) = store.scope {
                Menu {
                    ForEach(store.branches, id: \.self) { branch in
                        Button(branch) { store.compare(with: branch, folder: folder) }
                    }
                } label: {
                    Text("vs \(base)").lineLimit(1).truncationMode(.middle)
                }
                .menuStyle(.button)
                .buttonStyle(PillButtonStyle())
                .fixedSize()
                .help("The base branch")
            }
            Spacer(minLength: 0)
            ToolbarCluster {
                ClusterButton(systemImage: "arrow.clockwise", help: "Read the changes again") { store.refresh(folder: folder) }
                ClusterButton(systemImage: "xmark", help: "Hide changes", action: close)
            }
        }
        .padding(.horizontal, 12)
        .frame(height: 28)
        .padding(.top, 6)
        .padding(.bottom, 8)
    }

    @ViewBuilder
    private var content: some View {
        switch store.phase {
        case .idle, .loading:
            ProgressView().controlSize(.small).frame(maxWidth: .infinity, maxHeight: .infinity)
        case .failed(let message):
            notice(message, systemImage: "exclamationmark.triangle")
        case .loaded(let snapshot):
            if snapshot.files.isEmpty {
                VStack(spacing: 0) {
                    summary(snapshot)
                    notice("No changes", systemImage: "checkmark.circle")
                }
            } else {
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 4) {
                        summary(snapshot)
                        ForEach(snapshot.files) { file in fileRow(file, root: snapshot.root) }
                        omissions(snapshot)
                    }
                    .padding(.horizontal, 12)
                    .padding(.bottom, 16)
                }
                .environment(\.projectRoot, snapshot.root)
            }
        }
    }

    private func notice(_ text: String, systemImage: String) -> some View {
        Label(text, systemImage: systemImage)
            .font(.callout)
            .foregroundStyle(.secondary)
            .multilineTextAlignment(.center)
            .padding(24)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    static func scopeLabel(_ scope: ChangesScope) -> String {
        switch scope {
        case .workingTree: "uncommitted changes"
        case .branch(let base): "the branch since \(base)"
        }
    }

    /// What was compared: the scope, the commit, and that uncommitted and
    /// untracked files are part of it.
    private func summary(_ snapshot: ChangesSnapshot) -> some View {
        let scope: String = switch snapshot.scope {
        case .workingTree: "Uncommitted and untracked changes"
        case .branch(let base): "Since the branch left \(base), including uncommitted"
        }
        let base = snapshot.baseCommit.map { " · against \($0)" } ?? " · no commits yet"
        let branch = snapshot.branch.map { "On \($0)" } ?? "Detached HEAD"
        return VStack(alignment: .leading, spacing: 2) {
            HStack(spacing: 6) {
                Text("\(snapshot.files.count) file\(snapshot.files.count == 1 ? "" : "s")").fontWeight(.semibold)
                Text("+\(snapshot.added)").foregroundStyle(.green)
                Text("\u{2212}\(snapshot.removed)").foregroundStyle(.red)
                Spacer(minLength: 0)
                Text(snapshot.loadedAt, style: .time).foregroundStyle(.tertiary)
                    .help("When the changes were read")
            }
            .font(.callout.monospacedDigit())
            Text("\(scope). \(branch)\(base)")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
        .padding(.vertical, 6)
        .accessibilityElement(children: .combine)
    }

    private func fileRow(_ file: GitFileChange, root: String) -> some View {
        let isOpen = expanded.contains(file.id)
        return VStack(alignment: .leading, spacing: 4) {
            Button {
                withAnimation(.snappy(duration: 0.15)) {
                    if isOpen { expanded.remove(file.id) } else { expanded.insert(file.id) }
                }
            } label: {
                HStack(spacing: 8) {
                    Image(systemName: "chevron.right")
                        .font(.caption2.weight(.semibold))
                        .foregroundStyle(.secondary)
                        .rotationEffect(.degrees(isOpen ? 90 : 0))
                    StatusBadge(status: file.status)
                    VStack(alignment: .leading, spacing: 1) {
                        Text(file.path).font(.callout.monospaced()).lineLimit(1).truncationMode(.middle)
                        if let old = file.oldPath {
                            Text("from \(old)").font(.caption).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle)
                        }
                    }
                    Spacer(minLength: 4)
                    if file.isBinary {
                        Text("binary").font(.caption).foregroundStyle(.secondary)
                    } else {
                        HStack(spacing: 4) {
                            if file.added > 0 { Text("+\(file.added)").foregroundStyle(.green) }
                            if file.removed > 0 { Text("\u{2212}\(file.removed)").foregroundStyle(.red) }
                        }
                        .font(.caption.monospacedDigit())
                    }
                }
                .padding(.vertical, 5)
                .contentShape(.rect)
            }
            .buttonStyle(.plain)
            .accessibilityLabel("\(file.status.rawValue) \(file.path), \(file.added) lines added, \(file.removed) removed")
            .accessibilityValue(isOpen ? "Expanded" : "Collapsed")
            if isOpen {
                if file.isBinary {
                    Text("Binary file; its contents are not shown.").font(.caption).foregroundStyle(.secondary)
                } else if file.rows.isEmpty {
                    Text(file.status == .renamed ? "Renamed without changes." : "No line changes.")
                        .font(.caption).foregroundStyle(.secondary)
                } else if picking == file.id, case .loaded(let snapshot) = store.phase {
                    LinePicker(file: file, root: snapshot.root, scope: Self.scopeLabel(snapshot.scope), baseCommit: snapshot.baseCommit) { reference in
                        model.attachCodeReference(reference)
                        picking = nil
                    } cancel: {
                        picking = nil
                    }
                } else {
                    Button {
                        picking = file.id
                    } label: {
                        Label("Pick lines for a message", systemImage: "text.line.first.and.arrowtriangle.forward")
                    }
                    .buttonStyle(.link)
                    .font(.caption)
                    .help("Select diff lines and attach them to your message as a reference")
                    FileChangeView(change: FileChange(path: URL(fileURLWithPath: root).appendingPathComponent(file.path).path,
                                                      rows: file.rows, added: file.added, removed: file.removed,
                                                      isNewFile: file.status == .added || file.status == .untracked),
                                   showsHeader: false)
                    if file.omittedLines > 0 {
                        Text("\(file.omittedLines) more lines not shown").font(.caption).foregroundStyle(.secondary)
                    }
                }
            }
        }
    }

    @ViewBuilder
    private func omissions(_ snapshot: ChangesSnapshot) -> some View {
        if snapshot.truncated || snapshot.omittedFiles > 0 {
            VStack(alignment: .leading, spacing: 2) {
                if snapshot.truncated {
                    Text("The diff is larger than \(GitChangesLoader.maxDiffBytes >> 20) MB; files after the cut are not listed.")
                }
                if snapshot.omittedFiles > 0 {
                    Text("\(snapshot.omittedFiles) more untracked files are not listed.")
                }
            }
            .font(.caption)
            .foregroundStyle(Color.orange)
            .padding(.top, 8)
        }
    }
}

private struct StatusBadge: View {
    let status: GitFileChange.Status

    var body: some View {
        let (letter, color): (String, Color) = switch status {
        case .added: ("A", .green)
        case .untracked: ("U", .green)
        case .deleted: ("D", .red)
        case .renamed: ("R", .blue)
        case .copied: ("C", .blue)
        case .modified: ("M", .orange)
        }
        Text(letter)
            .font(.caption2.monospaced().weight(.bold))
            .foregroundStyle(color)
            .frame(width: 16, height: 16)
            .background(color.opacity(0.15), in: .rect(cornerRadius: 4))
            .help(status.rawValue.capitalized)
            .accessibilityHidden(true)
    }
}

/// Pick diff lines for a message (#2643): click a line, shift-click (or
/// shift-arrow) to extend, arrows to move, Return to ask, Esc to cancel. A
/// pick stays on one side of the diff: removed lines, or lines as they are now.
private struct LinePicker: View {
    let file: GitFileChange
    let root: String
    let scope: String
    let baseCommit: String?
    let attach: (CodeReference) -> Void
    let cancel: () -> Void
    @State private var anchor: Int?
    @State private var cursor: Int?
    @FocusState private var focused: Bool

    private var lines: [DiffLine] {
        file.rows.compactMap { if case .line(let line) = $0 { line } else { nil } }
    }

    /// The diff's rows in order: each line by its index in `lines`, or a gap
    /// between hunks.
    private var entries: [(line: Int?, gap: Int?)] {
        var next = 0
        return file.rows.map { row in
            switch row {
            case .line:
                defer { next += 1 }
                return (next, nil)
            case .gap(let count):
                return (nil, count)
            }
        }
    }

    private var selection: ClosedRange<Int>? {
        guard let anchor, let cursor else { return nil }
        return min(anchor, cursor)...max(anchor, cursor)
    }

    private func reference(_ intent: CodeReference.Intent) -> CodeReference? {
        guard let selection else { return nil }
        return CodeReference.from(rows: Array(lines[selection]), intent: intent, root: root, path: file.path, oldPath: file.oldPath,
                                  scope: scope, baseCommit: baseCommit)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 8) {
                if let selection, let ref = reference(.ask) {
                    Text(ref.label).font(.caption.monospaced()).lineLimit(1).truncationMode(.middle)
                    Text("\(selection.count) line\(selection.count == 1 ? "" : "s")").font(.caption).foregroundStyle(.secondary)
                } else if selection != nil {
                    Text("Pick removed lines or current lines, not both.").font(.caption).foregroundStyle(.orange)
                } else {
                    Text("Click a line; shift-click to extend.").font(.caption).foregroundStyle(.secondary)
                }
                Spacer(minLength: 4)
                Button("Ask agent") { reference(.ask).map(attach) }
                    .disabled(reference(.ask) == nil)
                    .help("Attach the lines to your message as a question (Return)")
                Button("Request change") { reference(.change).map(attach) }
                    .disabled(reference(.change) == nil)
                    .help("Attach the lines to your message as a change to make")
                Button("Cancel", action: cancel)
            }
            .controlSize(.small)
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    ForEach(Array(entries.enumerated()), id: \.offset) { _, entry in
                        if let index = entry.line {
                            row(lines[index], index: index)
                        } else {
                            Text(entry.gap.map { "\u{22EF} \($0) unchanged line\($0 == 1 ? "" : "s")" } ?? "\u{22EF}")
                                .font(.system(.caption, design: .monospaced))
                                .foregroundStyle(.secondary.opacity(0.6))
                                .padding(.vertical, 2)
                                .padding(.leading, 48)
                                .accessibilityHidden(true)
                        }
                    }
                }
            }
            .frame(maxHeight: 360)
            .background(.background.opacity(0.5), in: .rect(cornerRadius: 8))
            .focusable()
            .focusEffectDisabled()
            .focused($focused)
            .onAppear { focused = true }
            .onKeyPress(.upArrow, phases: .down) { press in move(-1, extend: press.modifiers.contains(.shift)) }
            .onKeyPress(.downArrow, phases: .down) { press in move(1, extend: press.modifiers.contains(.shift)) }
            .onKeyPress(.return) {
                reference(.ask).map(attach)
                return .handled
            }
            .onKeyPress(.escape) {
                cancel()
                return .handled
            }
            .accessibilityElement(children: .contain)
            .accessibilityLabel("Diff lines of \(file.path)")
        }
    }

    private func move(_ step: Int, extend: Bool) -> KeyPress.Result {
        guard !lines.isEmpty else { return .ignored }
        let next = min(max((cursor ?? -1) + step, 0), lines.count - 1)
        cursor = next
        if !extend || anchor == nil { anchor = next }
        return .handled
    }

    private func row(_ line: DiffLine, index: Int) -> some View {
        let selected = selection?.contains(index) ?? false
        let sign = switch line.kind { case .added: "+"; case .removed: "\u{2212}"; case .context: " " }
        let tint: Color = switch line.kind { case .added: .green; case .removed: .red; case .context: .clear }
        return HStack(spacing: 6) {
            Text(line.number.map(String.init) ?? "").foregroundStyle(.secondary.opacity(0.6)).frame(width: 36, alignment: .trailing)
            Text(sign).foregroundStyle(tint == .clear ? Color.secondary : tint)
            Text(line.text.isEmpty ? " " : line.text).lineLimit(1)
            Spacer(minLength: 0)
        }
        .font(.system(.caption, design: .monospaced))
        .padding(.vertical, 1)
        .padding(.horizontal, 6)
        .background(selected ? AnyShapeStyle(Color.accentColor.opacity(0.25)) : AnyShapeStyle(tint.opacity(0.13)))
        .contentShape(.rect)
        .onTapGesture {
            if NSEvent.modifierFlags.contains(.shift), anchor != nil {
                cursor = index
            } else {
                anchor = index
                cursor = index
            }
            focused = true
        }
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(selected ? [.isButton, .isSelected] : .isButton)
        .accessibilityLabel("\(line.kind == .removed ? "removed" : line.kind == .added ? "added" : "") line \(line.number.map(String.init) ?? ""): \(line.text)")
    }
}
