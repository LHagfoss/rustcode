import AppKit
import RustCodeKit
import SwiftUI

/// The thread's project's pull requests (#2645), read-only: what is open,
/// its checks and reviews, its description, files and activity, and a way
/// to start a thread about one. Nothing here checks out, reviews, comments,
/// approves or merges.
@MainActor
final class PullRequestsStore: ObservableObject {
    enum Phase: Equatable {
        case idle, loading, loaded, failed(PullRequestError)
    }

    @Published var state = PullRequest.State.open.rawValue
    @Published var search = ""
    @Published private(set) var pulls: [PullRequest] = []
    @Published private(set) var hasMore = false
    @Published private(set) var phase = Phase.idle
    /// When the list shown was read: how fresh it is.
    @Published private(set) var fetchedAt: Date?
    @Published private(set) var detail: PullRequestDetail?
    @Published private(set) var detailError: PullRequestError?
    @Published private(set) var loadingDetail: Int?
    private var limit = 30
    private var folder: URL?

    func load(_ connection: ACPConnection?, folder: URL, more: Bool = false) async {
        guard let connection else { return }
        if folder != self.folder {
            self.folder = folder
            pulls = []
            detail = nil
        }
        limit = more ? limit + 30 : 30
        phase = .loading
        let result: JSONValue
        do {
            result = try await connection.request(
                ACPMethod.pulls, params: ACPRequests.pulls(cwd: folder, state: state, search: search, limit: limit),
                timeout: .seconds(40))
        } catch {
            phase = .failed(PullRequestError(code: "failed", message: "\(error)"))
            return
        }
        switch PullRequestList(result: result) {
        case .loaded(let pulls, let hasMore):
            self.pulls = pulls
            self.hasMore = hasMore
            fetchedAt = Date()
            phase = .loaded
        case .failed(let error):
            phase = .failed(error)
        }
    }

    func open(_ pull: PullRequest, _ connection: ACPConnection?) async {
        guard let connection, let folder else { return }
        loadingDetail = pull.number
        detailError = nil
        defer { loadingDetail = nil }
        do {
            let result = try await connection.request(
                ACPMethod.pullView, params: ACPRequests.pullView(cwd: folder, number: pull.number), timeout: .seconds(40))
            switch PullRequestDetail.from(result: result) {
            case .success(let detail): self.detail = detail
            case .failure(let error): detailError = error
            }
        } catch {
            detailError = PullRequestError(code: "failed", message: "\(error)")
        }
    }

    func closeDetail() {
        detail = nil
        detailError = nil
    }
}

/// The toolbar's Pull Requests button and its panel.
struct PullRequestsButton: View {
    @ObservedObject var model: DesktopModel
    @StateObject private var store = PullRequestsStore()

    var body: some View {
        Button { model.showsPullRequests.toggle() } label: {
            Label("Pull Requests", systemImage: "arrow.triangle.pull")
        }
        .help("Pull requests of \(model.chatProject.lastPathComponent)")
        .popover(isPresented: $model.showsPullRequests, arrowEdge: .bottom) {
            PullRequestsPanel(model: model, store: store) { model.showsPullRequests = false }
                .frame(width: 560, height: 620)
        }
    }
}

private struct PullRequestsPanel: View {
    @ObservedObject var model: DesktopModel
    @ObservedObject var store: PullRequestsStore
    let close: () -> Void

    var body: some View {
        Group {
            if let detail = store.detail {
                PullRequestDetailView(detail: detail, back: store.closeDetail) {
                    askInNewThread(detail.pull)
                }
            } else {
                list
            }
        }
        .task(id: model.chatProject) {
            if store.pulls.isEmpty || store.phase == .idle {
                await store.load(model.engineConnection, folder: model.chatProject)
            }
        }
        .onChange(of: model.pullToOpen) { _, number in
            guard let number else { return }
            model.pullToOpen = nil
            if let pull = store.pulls.first(where: { $0.number == number }) {
                Task { await store.open(pull, model.engineConnection) }
            }
        }
    }

    private var list: some View {
        VStack(alignment: .leading, spacing: 0) {
            VStack(alignment: .leading, spacing: 10) {
                HStack {
                    Text("Pull Requests").font(.headline)
                    Text(model.chatProject.lastPathComponent).foregroundStyle(.secondary)
                    Spacer()
                    if let fetched = store.fetchedAt {
                        Text(fetched, format: .relative(presentation: .named))
                            .font(.caption)
                            .foregroundStyle(.tertiary)
                            .help("When this list was read")
                    }
                    Button { reload() } label: { Image(systemName: "arrow.clockwise") }
                        .buttonStyle(.borderless)
                        .disabled(store.phase == .loading)
                        .help("Read again")
                }
                HStack(spacing: 8) {
                    Picker("State", selection: $store.state) {
                        Text("Open").tag("open")
                        Text("Closed").tag("closed")
                        Text("Merged").tag("merged")
                        Text("All").tag("all")
                    }
                    .pickerStyle(.segmented)
                    .labelsHidden()
                    .frame(width: 240)
                    .onChange(of: store.state) { _, _ in reload() }
                    TextField("Search, e.g. author:me or a title", text: $store.search)
                        .textFieldStyle(.roundedBorder)
                        .onSubmit { reload() }
                }
            }
            .padding(14)
            Divider()
            content
        }
    }

    @ViewBuilder
    private var content: some View {
        switch store.phase {
        case .failed(let error):
            VStack(spacing: 10) {
                Image(systemName: "exclamationmark.triangle").font(.title2).foregroundStyle(Color.attention)
                Text(error.title).font(.headline)
                Text(error.message).font(.callout).foregroundStyle(.secondary)
                    .multilineTextAlignment(.center).textSelection(.enabled).lineLimit(6)
                HStack {
                    if error.opensConnections {
                        Button("Open Connections") { close(); model.openSettings(.connections) }
                    }
                    Button("Try Again") { reload() }
                }
            }
            .padding(24)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        case .loading where store.pulls.isEmpty:
            ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
        default:
            if store.pulls.isEmpty {
                Text(store.search.isEmpty ? "No \(store.state == "all" ? "" : store.state + " ")pull requests" : "No pull requests match")
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 2) {
                        ForEach(store.pulls) { pull in
                            PullRequestRow(pull: pull, loading: store.loadingDetail == pull.number) {
                                Task { await store.open(pull, model.engineConnection) }
                            }
                        }
                        if store.hasMore {
                            Button(store.phase == .loading ? "Loading…" : "Show More") {
                                Task { await store.load(model.engineConnection, folder: model.chatProject, more: true) }
                            }
                            .disabled(store.phase == .loading)
                            .frame(maxWidth: .infinity)
                            .padding(10)
                        }
                        if let error = store.detailError {
                            Text("\(error.title): \(error.message)")
                                .font(.caption).foregroundStyle(Color.attention).padding(10)
                        }
                    }
                    .padding(8)
                }
            }
        }
    }

    private func reload() {
        Task { await store.load(model.engineConnection, folder: model.chatProject) }
    }

    /// A new thread in the project, its message written but not sent.
    private func askInNewThread(_ pull: PullRequest) {
        close()
        Task {
            await model.newSession(in: model.chatProject)
            model.draft = pull.threadDraft
        }
    }
}

private struct PullRequestRow: View {
    let pull: PullRequest
    let loading: Bool
    let open: () -> Void

    var body: some View {
        Button(action: open) {
            HStack(alignment: .top, spacing: 10) {
                PullStateIcon(pull: pull)
                VStack(alignment: .leading, spacing: 3) {
                    Text(pull.title).font(.callout.weight(.medium)).lineLimit(2)
                    HStack(spacing: 6) {
                        Text("#\(String(pull.number)) · \(pull.author)")
                        if pull.draft { Badge(text: "Draft", color: .secondary) }
                        if let review = pull.reviewLabel {
                            Badge(text: review, color: pull.review == "approved" ? .green : (pull.review == "changes_requested" ? .red : .secondary))
                        }
                        if let checks = pull.checks.label {
                            Badge(text: checks, color: pull.checks.failed > 0 ? .red : (pull.checks.pending > 0 ? .orange : .green))
                        }
                    }
                    .font(.caption)
                    .foregroundStyle(.secondary)
                }
                Spacer(minLength: 8)
                VStack(alignment: .trailing, spacing: 3) {
                    if loading { ProgressView().controlSize(.small) }
                    if let updated = pull.updatedAt {
                        Text(updated, format: .relative(presentation: .named)).font(.caption).foregroundStyle(.tertiary)
                    }
                    Text("+\(pull.additions) −\(pull.deletions)").font(.caption.monospacedDigit()).foregroundStyle(.tertiary)
                }
            }
            .padding(.horizontal, 8)
            .padding(.vertical, 7)
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .hoverHighlight(cornerRadius: 8)
    }
}

private struct PullStateIcon: View {
    let pull: PullRequest

    var body: some View {
        Image(systemName: pull.state == .merged ? "arrow.triangle.merge" : "arrow.triangle.pull")
            .foregroundStyle(pull.state == .merged ? Color.purple : (pull.state == .closed ? Color.red : (pull.draft ? Color.secondary : Color.green)))
            .frame(width: 18)
            .accessibilityLabel(pull.draft ? "Draft" : pull.state.rawValue.capitalized)
    }
}

private struct Badge: View {
    let text: String
    let color: Color

    var body: some View {
        Text(text)
            .font(.caption2.weight(.medium))
            .foregroundStyle(color)
            .padding(.horizontal, 6)
            .padding(.vertical, 1)
            .background(color.opacity(0.12), in: .capsule)
    }
}

private struct PullRequestDetailView: View {
    let detail: PullRequestDetail
    let back: () -> Void
    let askInNewThread: () -> Void

    private var pull: PullRequest { detail.pull }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Button(action: back) { Label("Pull Requests", systemImage: "chevron.left") }
                    .buttonStyle(.borderless)
                Spacer()
                if let url = pull.url {
                    Button("Open on GitHub") { NSWorkspace.shared.open(url) }
                }
                Button("Ask in New Thread", action: askInNewThread)
                    .buttonStyle(.borderedProminent)
                    .help("Start a thread about this pull request; the message is written, not sent")
            }
            .padding(12)
            Divider()
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    VStack(alignment: .leading, spacing: 6) {
                        HStack(alignment: .firstTextBaseline, spacing: 8) {
                            PullStateIcon(pull: pull)
                            Text(pull.title).font(.title3.weight(.semibold)).textSelection(.enabled)
                        }
                        Text("#\(String(pull.number)) by \(pull.author) · \(pull.headRef) → \(pull.baseRef)")
                            .font(.callout).foregroundStyle(.secondary)
                        HStack(spacing: 6) {
                            Text("Commit \(pull.shortSHA)").font(.caption.monospaced())
                            Button { Clipboard.copy(pull.headSHA) } label: { Image(systemName: "doc.on.doc") }
                                .buttonStyle(.borderless).help("Copy the full commit")
                            if let review = pull.reviewLabel { Badge(text: review, color: .secondary) }
                        }
                        .foregroundStyle(.secondary)
                    }
                    section("Description") {
                        if detail.body.isEmpty {
                            Text("No description.").foregroundStyle(.secondary)
                        } else {
                            MarkdownText(text: detail.body)
                        }
                    }
                    if !detail.checks.isEmpty {
                        section("Checks") {
                            ForEach(detail.checks) { check in
                                HStack(spacing: 8) {
                                    Image(systemName: Self.icon(check.status)).foregroundStyle(Self.color(check.status))
                                    Text(check.name)
                                    Spacer()
                                    if let url = check.url {
                                        Button("Details") { NSWorkspace.shared.open(url) }.buttonStyle(.link)
                                    }
                                }
                                .font(.callout)
                            }
                        }
                    }
                    section("Files (\(detail.files.count))") {
                        ForEach(detail.files) { file in
                            HStack {
                                Text(file.path).font(.callout.monospaced()).lineLimit(1).truncationMode(.middle)
                                Spacer()
                                Text("+\(file.additions)").foregroundStyle(.green)
                                Text("−\(file.deletions)").foregroundStyle(.red)
                            }
                            .font(.caption.monospacedDigit())
                        }
                    }
                    if !detail.activity.isEmpty {
                        section("Activity") {
                            ForEach(detail.activity) { item in
                                VStack(alignment: .leading, spacing: 3) {
                                    HStack(spacing: 6) {
                                        Text(item.author).font(.callout.weight(.medium))
                                        Text(item.kind.replacingOccurrences(of: "_", with: " ")).foregroundStyle(.secondary)
                                        Spacer()
                                        if let at = item.at {
                                            Text(at, format: .relative(presentation: .named)).foregroundStyle(.tertiary)
                                        }
                                    }
                                    .font(.caption)
                                    if !item.body.isEmpty {
                                        MarkdownText(text: item.body)
                                    }
                                }
                            }
                        }
                    }
                }
                .padding(16)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
    }

    private func section<Content: View>(_ title: String, @ViewBuilder content: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(title).font(.caption.weight(.semibold)).foregroundStyle(.secondary)
            content()
        }
    }

    private static func icon(_ status: String) -> String {
        switch status {
        case "passed": "checkmark.circle.fill"
        case "failed": "xmark.circle.fill"
        case "skipped": "minus.circle"
        default: "clock"
        }
    }

    private static func color(_ status: String) -> Color {
        switch status {
        case "passed": .green
        case "failed": .red
        case "skipped": .secondary
        default: .orange
        }
    }
}
