import RustCodeKit
import SwiftUI

/// ⌘K: a Liquid Glass panel at the top of the window for jumping to a chat
/// (`SessionSwitcher`). Type to search, ↑↓ to move, ⏎ to open, Esc to close.
/// The open chat stays as it is until a row is picked.
struct SessionSwitcherOverlay: View {
    @ObservedObject var model: DesktopModel

    var body: some View {
        ZStack(alignment: .top) {
            if model.showsSwitcher {
                Color.black.opacity(0.12)
                    .ignoresSafeArea()
                    .contentShape(.rect)
                    .onTapGesture { model.showsSwitcher = false }
                    .transition(.opacity)
                SessionSwitcherPanel(model: model)
                    .padding(.top, 72)
                    .padding(.horizontal, 28)
                    .transition(.scale(scale: 0.96, anchor: .top).combined(with: .opacity))
            }
        }
        .animation(.snappy(duration: 0.18), value: model.showsSwitcher)
    }
}

private struct SessionSwitcherPanel: View {
    @ObservedObject var model: DesktopModel
    @State private var query = ""
    /// The highlighted chat, kept by id so rows arriving under it do not
    /// move the highlight to another chat.
    @State private var selectedID: String?
    @State private var contentMatches: [SessionContentMatch] = []
    @State private var contentQuery: String?
    @State private var searching = false
    @FocusState private var focused: Bool

    private var trimmed: String { query.trimmingCharacters(in: .whitespaces) }

    private var results: [SessionSwitcher.Result] {
        let shown = model.sessions.filter { !model.archivedSessions.contains($0.id) }
        let matches = contentQuery == SessionContentSearch.query(for: query) ? contentMatches : []
        return SessionSwitcher.results(sessions: shown, query: query, pinned: model.pinnedSessions, contentMatches: matches)
    }

    var body: some View {
        let results = results
        let selection = SessionSwitcher.keep(selectedID, in: results)
        VStack(spacing: 0) {
            field(results, selection: selection)
            Divider().opacity(0.5)
            list(results, selection: selection)
            footer
        }
        .frame(width: 600)
        .glassSurface(in: .rect(cornerRadius: 24))
        .shadow(color: .black.opacity(0.18), radius: 30, y: 14)
        .background {
            // Esc closes from wherever focus is.
            Button("Close") { model.showsSwitcher = false }.keyboardShortcut(.cancelAction).hidden()
        }
        .onAppear {
            let initial = SessionSwitcher.initialSelection(results, current: model.selectedSession)
            selectedID = results.indices.contains(initial) ? results[initial].id : nil
            focused = true
        }
        .onDisappear { model.switcherSearches = false }
        .task(id: trimmed) { await searchContents() }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Switch chat")
    }

    private func field(_ results: [SessionSwitcher.Result], selection: Int) -> some View {
        HStack(spacing: 10) {
            Image(systemName: "magnifyingglass")
                .font(.system(size: 17, weight: .medium))
                .foregroundStyle(.secondary)
            TextField("Switch to chat…", text: $query)
                .textFieldStyle(.plain)
                .font(.title3)
                .focused($focused)
                .onKeyPress(.upArrow) { move(-1, results, selection); return .handled }
                .onKeyPress(.downArrow) { move(1, results, selection); return .handled }
                .onKeyPress(.return, phases: .down) { _ in
                    if results.indices.contains(selection) { model.switchTo(results[selection].session) }
                    return .handled
                }
                .onKeyPress(.escape) {
                    model.showsSwitcher = false
                    return .handled
                }
                .onChange(of: query) { _, _ in
                    selectedID = SessionSwitcher.results(sessions: model.sessions.filter { !model.archivedSessions.contains($0.id) },
                                                         query: query, pinned: model.pinnedSessions).first?.id
                }
                .accessibilityLabel("Search chats")
            if searching {
                ProgressView().controlSize(.small)
            }
            ScopeToggle(showsAll: $model.showsAllProjects)
                .help(model.showsAllProjects ? "Searching chats in every project" : "Searching chats in \(model.workspace.lastPathComponent)")
        }
        .padding(.horizontal, 16)
        .frame(height: 52)
    }

    @ViewBuilder
    private func list(_ results: [SessionSwitcher.Result], selection: Int) -> some View {
        if !model.sessionsLoaded {
            status { ProgressView().controlSize(.small); Text("Loading chats…") }
        } else if results.isEmpty {
            status {
                if !trimmed.isEmpty && (searching || model.hasMoreSessions) {
                    ProgressView().controlSize(.small)
                    Text("Searching…")
                } else {
                    Text(trimmed.isEmpty ? "No chats yet." : "No chats match “\(trimmed)”.")
                }
            }
        } else {
            ScrollViewReader { proxy in
                ScrollView {
                    LazyVStack(spacing: 2) {
                        ForEach(Array(results.enumerated()), id: \.element.id) { index, result in
                            SwitcherRow(result: result, query: trimmed, showsProject: model.showsAllProjects,
                                        isSelected: index == selection,
                                        isOpen: result.id == model.selectedSession,
                                        isPinned: model.pinnedSessions.contains(result.id)) {
                                model.switchTo(result.session)
                            }
                            .id(result.id)
                        }
                    }
                    .padding(6)
                }
                .frame(maxHeight: 380)
                .fixedSize(horizontal: false, vertical: true)
                .onChange(of: selectedID) { _, id in
                    if let id { proxy.scrollTo(id) }
                }
            }
        }
    }

    private func status<Content: View>(@ViewBuilder _ content: () -> Content) -> some View {
        HStack(spacing: 8) { content() }
            .font(.callout)
            .foregroundStyle(.secondary)
            .frame(maxWidth: .infinity)
            .frame(height: 64)
    }

    private var footer: some View {
        HStack(spacing: 14) {
            hint("↑↓", "move")
            hint("⏎", "open")
            hint("esc", "close")
            Spacer()
            if model.showsAllProjects {
                Text("All projects")
            } else {
                Label(model.workspace.lastPathComponent, systemImage: "folder")
            }
        }
        .font(.caption)
        .foregroundStyle(.secondary)
        .padding(.horizontal, 16)
        .frame(height: 30)
        .background(Color.primary.opacity(0.03))
        .accessibilityHidden(true)
    }

    private func hint(_ key: String, _ label: String) -> some View {
        HStack(spacing: 4) {
            Text(key)
                .font(.caption.weight(.semibold).monospaced())
                .padding(.horizontal, 5)
                .padding(.vertical, 1)
                .background(Color.primary.opacity(0.08), in: .rect(cornerRadius: 4))
            Text(label)
        }
    }

    private func move(_ step: Int, _ results: [SessionSwitcher.Result], _ selection: Int) {
        guard !results.isEmpty else { return }
        selectedID = results[SessionSwitcher.move(selection, by: step, count: results.count)].id
    }

    /// The engine's content search for what is typed, after a short pause;
    /// typing more cancels it. Text typed also reads the rest of the
    /// sidebar's pages, so titles from the whole history match.
    private func searchContents() async {
        model.switcherSearches = !trimmed.isEmpty
        guard let query = SessionContentSearch.query(for: query) else {
            contentMatches = []
            contentQuery = nil
            return
        }
        try? await Task.sleep(for: .milliseconds(250))
        guard !Task.isCancelled else { return }
        searching = true
        let matches = await model.searchContents(query)
        guard !Task.isCancelled else { return }
        searching = false
        contentMatches = matches
        contentQuery = query
    }
}

private struct SwitcherRow: View {
    let result: SessionSwitcher.Result
    let query: String
    let showsProject: Bool
    let isSelected: Bool
    let isOpen: Bool
    let isPinned: Bool
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 10) {
                Image(systemName: isPinned ? "pin.fill" : "bubble.left")
                    .font(.system(size: 13))
                    .foregroundStyle(isSelected ? Color.brand : .secondary)
                    .frame(width: 18)
                VStack(alignment: .leading, spacing: 2) {
                    Text(highlighted(result.session.title ?? "New chat"))
                        .lineLimit(1)
                    Text(detail)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }
                Spacer(minLength: 8)
                if isOpen {
                    Text("Open")
                        .font(.caption.weight(.medium))
                        .foregroundStyle(.secondary)
                        .padding(.horizontal, 7)
                        .padding(.vertical, 2)
                        .background(Color.primary.opacity(0.07), in: .capsule)
                }
                if isSelected {
                    Image(systemName: "return")
                        .font(.caption.weight(.semibold))
                        .foregroundStyle(.secondary)
                }
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 7)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(isSelected ? Color.brand.opacity(0.16) : .clear, in: .rect(cornerRadius: 12))
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .accessibilityLabel("\(result.session.title ?? "New chat"), \(detail)")
        .accessibilityAddTraits(isSelected ? .isSelected : [])
    }

    /// The text around a content match, else where and when.
    private var detail: AttributedString {
        if let snippet = result.snippet { return highlighted(snippet) }
        let session = result.session
        var parts: [String] = []
        if showsProject, !session.projectName.isEmpty { parts.append(session.projectName) }
        if let model = session.modelName { parts.append(model) }
        if let updated = session.updatedAt { parts.append(SessionRow.age(of: updated, now: Date())) }
        return AttributedString(parts.joined(separator: " · "))
    }

    private func highlighted(_ text: String) -> AttributedString {
        var styled = AttributedString(text)
        guard !query.isEmpty else { return styled }
        var rest = styled.startIndex..<styled.endIndex
        while let match = styled[rest].range(of: query, options: [.caseInsensitive, .diacriticInsensitive]) {
            styled[match].inlinePresentationIntent = .stronglyEmphasized
            rest = match.upperBound..<styled.endIndex
        }
        return styled
    }
}
