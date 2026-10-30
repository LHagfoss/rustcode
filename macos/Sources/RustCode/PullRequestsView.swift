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
        diffFiles = nil
        diffError = nil
        submission = nil
    }

    // MARK: Review drafts (#2646)

    /// The open pull request's files with their diff rows, once read.
    @Published private(set) var diffFiles: [GitFileChange]?
    @Published private(set) var diffTruncated = false
    @Published private(set) var diffError: PullRequestError?
    @Published private(set) var loadingDiff = false
    /// The review being written for the open pull request.
    @Published var drafts: ReviewDrafts? {
        didSet { if let drafts { try? draftStore.save(drafts) } }
    }
    @Published private(set) var submitting = false
    @Published var submission: ReviewSubmission?

    private let draftStore = ReviewDraftStore(directory: FileManager.default
        .urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        .appendingPathComponent("RustCode/review-drafts", isDirectory: true))

    /// The drafts of `detail`'s pull request, from disk.
    func loadDrafts(for detail: PullRequestDetail) {
        guard let repository = ReviewDrafts.repository(of: detail.pull.url) else { return }
        drafts = draftStore.load(repository: repository, number: detail.pull.number)
    }

    func loadDiff(_ connection: ACPConnection?) async {
        guard diffFiles == nil, !loadingDiff, let connection, let folder, let number = detail?.pull.number else { return }
        loadingDiff = true
        defer { loadingDiff = false }
        do {
            let result = try await connection.request(
                ACPMethod.pullDiff, params: ACPRequests.pullView(cwd: folder, number: number), timeout: .seconds(40))
            if let error = result["error"], let code = error["code"]?.stringValue {
                diffError = PullRequestError(code: code, message: error["message"]?.stringValue ?? "")
                return
            }
            diffFiles = UnifiedDiff.parse(result["diff"]?.stringValue ?? "")
            diffTruncated = result["truncated"]?.boolValue ?? false
        } catch {
            diffError = PullRequestError(code: "failed", message: "\(error)")
        }
    }

    /// Post the drafts as one review at the pull request's current commit.
    /// Posted, they are gone; refused, they stay; with no answer, they stay
    /// and say the review may have been posted.
    func submit(_ connection: ACPConnection?) async {
        guard let connection, let folder, var drafts, let head = detail?.pull.headSHA, !submitting else { return }
        submitting = true
        defer { submitting = false }
        let outcome: ReviewSubmission
        do {
            let result = try await connection.request(
                ACPMethod.pullReview, params: drafts.request(cwd: folder, head: head), timeout: .seconds(60))
            outcome = ReviewSubmission(result: result)
        } catch {
            outcome = .uncertain("\(error)")
        }
        switch outcome {
        case .posted:
            drafts = ReviewDrafts(repository: drafts.repository, number: drafts.number)
        case .uncertain:
            drafts.uncertainSince = Date()
        case .failed:
            break
        }
        self.drafts = drafts
        submission = outcome
    }
}

/// The header's Pull Requests button: opens the panel beside the chat.
struct PullRequestsButton: View {
    @ObservedObject var model: DesktopModel

    var body: some View {
        ToolbarCluster {
            ClusterButton(systemImage: "arrow.triangle.pull", help: "Pull requests of \(model.chatProject.lastPathComponent)") {
                withAnimation(.snappy(duration: 0.25)) { model.showsPullRequests = true }
            }
        }
    }
}

/// The pull requests panel beside the chat, in the preview's place, like
/// Changes.
struct PullRequestsSidePanel: View {
    @ObservedObject var model: DesktopModel
    @StateObject private var store = PullRequestsStore()
    let close: () -> Void

    var body: some View {
        PullRequestsPanel(model: model, store: store, close: close)
            .background(Color.chatBackground)
    }
}

private struct PullRequestsPanel: View {
    @ObservedObject var model: DesktopModel
    @ObservedObject var store: PullRequestsStore
    let close: () -> Void

    var body: some View {
        Group {
            if let detail = store.detail {
                PullRequestDetailView(model: model, store: store, detail: detail, back: store.closeDetail) {
                    askInNewThread(detail.pull)
                } askAboutLine: { path, line in
                    askInNewThread(detail.pull, about: (path, line))
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
            HStack(spacing: 8) {
                Text("Pull Requests").font(.headline)
                Text(model.chatProject.lastPathComponent).foregroundStyle(.secondary).lineLimit(1)
                Spacer(minLength: 0)
                if let fetched = store.fetchedAt {
                    Text(fetched, format: .relative(presentation: .named))
                        .font(.caption)
                        .foregroundStyle(.tertiary)
                        .help("When this list was read")
                }
                ToolbarCluster {
                    ClusterButton(systemImage: "arrow.clockwise", help: "Read again") { reload() }
                    ClusterButton(systemImage: "xmark", help: "Hide pull requests", action: close)
                }
            }
            .padding(.horizontal, 12)
            .frame(height: 28)
            .padding(.top, 6)
            .padding(.bottom, 8)
            VStack(alignment: .leading, spacing: 8) {
                SettingsSegments("State", selection: $store.state,
                                 options: [("open", "Open"), ("closed", "Closed"), ("merged", "Merged"), ("all", "All")])
                    .onChange(of: store.state) { _, _ in reload() }
                HStack(spacing: 6) {
                    Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
                    TextField("Search, e.g. author:me or a title", text: $store.search)
                        .textFieldStyle(.plain)
                        .onSubmit { reload() }
                    if !store.search.isEmpty {
                        Button { store.search = ""; reload() } label: {
                            Image(systemName: "xmark.circle.fill").foregroundStyle(.tertiary)
                        }
                        .buttonStyle(.plain)
                        .accessibilityLabel("Clear search")
                    }
                }
                .font(.callout)
                .padding(.horizontal, 12)
                .frame(height: 30)
                .glassSurface(in: .capsule)
            }
            .padding(.horizontal, 12)
            .padding(.bottom, 10)
            Divider().opacity(0.5)
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
                            .buttonStyle(PillButtonStyle())
                    }
                    Button("Try Again") { reload() }.buttonStyle(PillButtonStyle(prominent: true))
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
    private func askInNewThread(_ pull: PullRequest, about line: (path: String, line: DiffLine)? = nil) {
        close()
        var text = pull.threadDraft
        if let line {
            let number = line.line.number.map(String.init) ?? "?"
            text += "\nAbout \(line.path) line \(number):\n```\n\(line.line.text)\n```\n"
        }
        Task {
            await model.newSession(in: model.chatProject)
            model.draft = text
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
    @ObservedObject var model: DesktopModel
    @ObservedObject var store: PullRequestsStore
    let detail: PullRequestDetail
    let back: () -> Void
    let askInNewThread: () -> Void
    let askAboutLine: (String, DiffLine) -> Void
    @State private var openFiles: Set<String> = []
    @State private var confirmsSubmit = false

    private var pull: PullRequest { detail.pull }
    private var canReview: Bool { model.canReviewPullRequests && store.drafts != nil }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 8) {
                ToolbarCluster {
                    ClusterButton(systemImage: "chevron.left", help: "Back to the pull requests", action: back)
                }
                Text("#\(String(pull.number))").font(.headline).monospacedDigit()
                Spacer(minLength: 0)
                if let url = pull.url {
                    Button("Open on GitHub") { NSWorkspace.shared.open(url) }
                        .buttonStyle(PillButtonStyle())
                }
                if model.canWatchPullRequests, model.selectedSession != nil { watchButton }
                Button("Ask in New Thread", action: askInNewThread)
                    .buttonStyle(PillButtonStyle(prominent: true))
                    .help("Start a thread about this pull request; the message is written, not sent")
            }
            .padding(.horizontal, 12)
            .frame(height: 28)
            .padding(.top, 6)
            .padding(.bottom, 8)
            Divider().opacity(0.5)
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    header
                    section("Description") {
                        if detail.body.isEmpty {
                            Text("No description.").foregroundStyle(.secondary)
                        } else {
                            MarkdownText(text: detail.body)
                        }
                    }
                    if !detail.checks.isEmpty { checks }
                    files
                    if canReview { review }
                    if !detail.activity.isEmpty { activity }
                }
                .padding(16)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .onAppear { if model.canReviewPullRequests { store.loadDrafts(for: detail) } }
        .onChange(of: model.pullFileToOpen) { _, path in
            guard let path else { return }
            model.pullFileToOpen = nil
            openFiles.insert(path)
            Task { await store.loadDiff(model.engineConnection) }
        }
    }

    private var watch: PullWatch? {
        model.pullWatches(in: model.selectedSession).first { $0.number == pull.number && $0.isWatching }
    }

    /// Watch this pull request from the shown thread (#2648).
    private var watchButton: some View {
        Button(watch == nil ? "Watch" : "Watching") {
            Task {
                if let watch { await model.stopWatching(watch) } else { await model.watchPull(pull.number) }
            }
        }
        .buttonStyle(PillButtonStyle())
        .help(watch == nil
              ? "Tell this thread when checks fail or pass, reviews or comments arrive, conflicts appear, or it closes. Nothing is done on the pull request"
              : "Stop watching")
    }

    private var header: some View {
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
    }

    // MARK: Review

    /// The review being written: drafts stay on this Mac until Submit.
    @ViewBuilder
    private var review: some View {
        if let drafts = store.drafts {
            section("Your review (not posted)") {
                if let since = drafts.uncertainSince {
                    Callout(color: .orange, systemImage: "questionmark.circle") {
                        Text("A submission \(since.formatted(.relative(presentation: .named))) got no answer and may have been posted. Check the pull request before submitting again.")
                        HStack {
                            if let url = pull.url { Button("Open on GitHub") { NSWorkspace.shared.open(url) } }
                            Button("It Was Not Posted") { store.drafts?.uncertainSince = nil }
                        }
                    }
                }
                switch store.submission {
                case .posted(let url):
                    Callout(color: .green, systemImage: "checkmark.circle") {
                        Text("Review posted.")
                        if let url { Button("Open") { NSWorkspace.shared.open(url) }.buttonStyle(.link) }
                    }
                case .failed(let message):
                    Callout(color: .red, systemImage: "xmark.octagon") {
                        Text("Not posted: \(message). Your drafts are kept.").textSelection(.enabled)
                    }
                default: EmptyView()
                }
                let stale = drafts.stale(head: pull.headSHA)
                if !stale.isEmpty {
                    Callout(color: .orange, systemImage: "exclamationmark.triangle") {
                        Text("\(stale.count) comment(s) were written at an earlier commit. The pull request has changed since, so their lines may have moved.")
                    }
                }
                if drafts.comments.isEmpty {
                    Text("Open a file below and click a line to draft a comment on it.")
                        .font(.callout).foregroundStyle(.secondary)
                } else {
                    ForEach(drafts.comments) { comment in
                        DraftCommentRow(comment: comment, isStale: comment.commit != pull.headSHA) { body in
                            if let index = store.drafts?.comments.firstIndex(where: { $0.id == comment.id }) {
                                store.drafts?.comments[index].body = body
                            }
                        } delete: {
                            store.drafts?.comments.removeAll { $0.id == comment.id }
                        }
                    }
                }
                TextField("Summary (optional for comments)", text: Binding(
                    get: { store.drafts?.summary ?? "" }, set: { store.drafts?.summary = $0 }), axis: .vertical)
                    .lineLimit(2...6)
                    .textFieldStyle(.plain).padding(.horizontal, 10).padding(.vertical, 6).background(.fill.quaternary, in: .rect(cornerRadius: 10))
                HStack {
                    SettingsSegments("Event", selection: Binding(get: { store.drafts?.event ?? .comment },
                                                                 set: { store.drafts?.event = $0 }),
                                     options: ReviewDrafts.Event.allCases.map { ($0, $0.label) })
                    Spacer()
                    if store.submitting { ProgressView().controlSize(.small) }
                    Button("Submit Review…") { confirmsSubmit = true }
                        .buttonStyle(PillButtonStyle(prominent: true))
                        .disabled(drafts.problem != nil || store.submitting || drafts.uncertainSince != nil)
                        .help(drafts.uncertainSince != nil ? "Check whether the last submission was posted first"
                              : drafts.problem ?? "Post this review to the pull request")
                }
                .confirmationDialog(submitTitle(drafts), isPresented: $confirmsSubmit) {
                    Button("Post Review") { Task { await store.submit(model.engineConnection) } }
                    Button("Cancel", role: .cancel) {}
                } message: {
                    Text(submitMessage(drafts))
                }
            }
        }
    }

    private func submitTitle(_ drafts: ReviewDrafts) -> String {
        "Post a review to #\(String(pull.number))?"
    }

    private func submitMessage(_ drafts: ReviewDrafts) -> String {
        var text = "\(drafts.event.label) with \(drafts.comments.count) line comment(s), at commit \(pull.shortSHA). Everyone with access to the pull request will see it."
        let stale = drafts.stale(head: pull.headSHA).count
        if stale > 0 { text += " \(stale) comment(s) were written at an earlier commit." }
        return text
    }

    // MARK: Sections

    private var checks: some View {
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

    private var files: some View {
        section("Files (\(detail.files.count))") {
            if store.diffTruncated {
                Text("The diff is too long to show whole; later files may be missing.")
                    .font(.caption).foregroundStyle(Color.attention)
            }
            if let error = store.diffError {
                Text("\(error.title): \(error.message)").font(.caption).foregroundStyle(Color.attention)
            }
            ForEach(detail.files) { file in
                VStack(alignment: .leading, spacing: 4) {
                    Button {
                        if openFiles.contains(file.path) { openFiles.remove(file.path) } else {
                            openFiles.insert(file.path)
                            Task { await store.loadDiff(model.engineConnection) }
                        }
                    } label: {
                        HStack {
                            Image(systemName: openFiles.contains(file.path) ? "chevron.down" : "chevron.right")
                                .font(.caption2).frame(width: 10)
                            Text(file.path).font(.callout.monospaced()).lineLimit(1).truncationMode(.middle)
                            Spacer()
                            let count = store.drafts?.comments.filter { $0.path == file.path }.count ?? 0
                            if count > 0 { Badge(text: "\(count) draft\(count == 1 ? "" : "s")", color: .orange) }
                            Text("+\(file.additions)").foregroundStyle(.green)
                            Text("−\(file.deletions)").foregroundStyle(.red)
                        }
                        .font(.caption.monospacedDigit())
                        .contentShape(.rect)
                    }
                    .buttonStyle(.plain)
                    if openFiles.contains(file.path) { fileDiff(file.path) }
                }
            }
        }
    }

    @ViewBuilder
    private func fileDiff(_ path: String) -> some View {
        if let files = store.diffFiles {
            if let change = files.first(where: { $0.path == path }) {
                PullFileDiff(change: change, comments: store.drafts?.comments.filter { $0.path == path } ?? [],
                             canComment: canReview) { line, body in
                    if let comment = ReviewDrafts.Comment(path: path, line: line, body: body, commit: pull.headSHA) {
                        store.drafts?.comments.append(comment)
                    }
                } ask: { line in
                    askAboutLine(path, line)
                }
            } else {
                Text("No diff for this file (binary, renamed, or past the shown part).")
                    .font(.caption).foregroundStyle(.secondary)
            }
        } else if store.loadingDiff {
            ProgressView().controlSize(.small)
        }
    }

    private var activity: some View {
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

/// A note in the review section, tinted by what it says.
private struct Callout<Content: View>: View {
    let color: Color
    let systemImage: String
    @ViewBuilder let content: Content

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            Image(systemName: systemImage).foregroundStyle(color)
            VStack(alignment: .leading, spacing: 6) { content }
                .font(.callout)
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(color.opacity(0.1), in: .rect(cornerRadius: 8))
    }
}

/// A drafted line comment: its line, its text to edit, and Delete.
private struct DraftCommentRow: View {
    let comment: ReviewDrafts.Comment
    let isStale: Bool
    let edit: (String) -> Void
    let delete: () -> Void
    @State private var text = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 6) {
                Text("\(comment.path):\(String(comment.line))").font(.caption.monospaced())
                if comment.side == .left { Badge(text: "removed line", color: .red) }
                if isStale { Badge(text: "earlier commit", color: .orange) }
                Spacer()
                Button(role: .destructive, action: delete) { Image(systemName: "trash") }
                    .buttonStyle(.borderless).help("Delete this draft")
            }
            Text(comment.lineText).font(.caption.monospaced()).foregroundStyle(.secondary).lineLimit(1)
            TextField("Comment", text: $text, axis: .vertical)
                .lineLimit(1...6)
                .textFieldStyle(.plain).padding(.horizontal, 10).padding(.vertical, 6).background(.fill.quaternary, in: .rect(cornerRadius: 10))
                .onChange(of: text) { _, new in if new != comment.body { edit(new) } }
        }
        .padding(8)
        .background(.fill.quaternary, in: .rect(cornerRadius: 8))
        .onAppear { text = comment.body }
    }
}

/// One file's diff, a line at a time: a click on a line opens a draft
/// comment for it; drafts show under their lines.
private struct PullFileDiff: View {
    let change: GitFileChange
    let comments: [ReviewDrafts.Comment]
    let canComment: Bool
    let add: (DiffLine, String) -> Void
    let ask: (DiffLine) -> Void
    @State private var commenting: Int?
    @State private var text = ""
    @State private var showsAll = false

    private static let shownRows = 400

    var body: some View {
        let rows = Array(change.rows.enumerated())
        VStack(alignment: .leading, spacing: 0) {
            ForEach(showsAll ? rows : Array(rows.prefix(Self.shownRows)), id: \.offset) { index, row in
                switch row {
                case .gap:
                    Text("⋯").font(.caption.monospaced()).foregroundStyle(.tertiary).padding(.vertical, 2)
                case .line(let line):
                    lineRow(index, line)
                    ForEach(comments.filter { matches($0, line) }) { comment in
                        Text(comment.body).font(.caption).padding(6)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .background(Color.orange.opacity(0.12), in: .rect(cornerRadius: 6))
                            .padding(.leading, 40).padding(.vertical, 2)
                    }
                    if commenting == index { composer(line) }
                }
            }
            if rows.count > Self.shownRows, !showsAll {
                Button("Show all \(rows.count) lines") { showsAll = true }.buttonStyle(.link).font(.caption)
            }
        }
        .padding(6)
        .background(.fill.quinary, in: .rect(cornerRadius: 8))
    }

    private func matches(_ comment: ReviewDrafts.Comment, _ line: DiffLine) -> Bool {
        comment.side == .left ? (line.kind == .removed && comment.line == line.oldNumber)
            : (line.kind != .removed && comment.line == line.newNumber)
    }

    private func lineRow(_ index: Int, _ line: DiffLine) -> some View {
        Button {
            guard canComment else { return }
            text = ""
            commenting = commenting == index ? nil : index
        } label: {
            HStack(alignment: .top, spacing: 6) {
                Text(line.number.map(String.init) ?? "").frame(width: 34, alignment: .trailing).foregroundStyle(.tertiary)
                Text(line.kind == .added ? "+" : line.kind == .removed ? "−" : " ").foregroundStyle(.secondary)
                Text(line.text).frame(maxWidth: .infinity, alignment: .leading).lineLimit(nil)
            }
            .font(.caption.monospaced())
            .padding(.vertical, 1)
            .background(line.kind == .added ? Color.green.opacity(0.1) : line.kind == .removed ? Color.red.opacity(0.1) : .clear)
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .help(canComment ? "Draft a comment on this line" : "")
    }

    private func composer(_ line: DiffLine) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            TextField("Comment on this line (kept as a draft)", text: $text, axis: .vertical)
                .lineLimit(2...6)
                .textFieldStyle(.plain).padding(.horizontal, 10).padding(.vertical, 6).background(.fill.quaternary, in: .rect(cornerRadius: 10))
            HStack {
                Button("Ask in New Thread") { ask(line) }
                    .buttonStyle(PillButtonStyle())
                    .help("Start a thread about this line; nothing is posted")
                Spacer()
                Button("Cancel") { commenting = nil }.buttonStyle(PillButtonStyle())
                Button("Add Draft") {
                    add(line, text)
                    commenting = nil
                }
                .buttonStyle(PillButtonStyle(prominent: true))
                .disabled(text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            }
        }
        .padding(8)
        .padding(.leading, 34)
    }
}

/// A thread's pull request watch in its title bar: what it last saw, a way
/// to hand that to the agent as a message, and Stop (#2648).
struct PullWatchChip: View {
    @ObservedObject var model: DesktopModel
    let watch: PullWatch

    var body: some View {
        Menu {
            if watch.events.isEmpty {
                Text(watch.isWatching ? "Nothing new yet" : "Nothing happened while watching")
            }
            ForEach(Array(watch.events.suffix(8).enumerated().reversed()), id: \.offset) { _, event in
                Label(event.at.map { "\(event.message) · \($0.formatted(.relative(presentation: .named)))" } ?? event.message,
                      systemImage: event.systemImage)
            }
            if let error = watch.lastError, watch.isWatching {
                Label("Last check failed: \(error)", systemImage: "wifi.exclamationmark")
            }
            Divider()
            if !watch.events.isEmpty {
                Button("Ask the Agent…") { model.askAgent(about: watch) }
            }
            if let url = watch.url {
                Button("Open on GitHub") { NSWorkspace.shared.open(url) }
            }
            if watch.isWatching {
                Button("Stop Watching") { Task { await model.stopWatching(watch) } }
            } else {
                Button("Remove") { Task { await model.stopWatching(watch, remove: true) } }
            }
        } label: {
            HStack(spacing: 5) {
                Image(systemName: icon).foregroundStyle(tint)
                Text(watch.label).lineLimit(1)
            }
            .font(.caption.weight(.medium))
            .padding(.horizontal, 10)
            .frame(height: 24)
            .glassSurface(in: .capsule)
        }
        .menuStyle(.button)
        .buttonStyle(.plain)
        .menuIndicator(.hidden)
        .fixedSize()
        .help(watch.isWatching ? "RustCode checks this pull request every few minutes" : (watch.stopReason ?? "Stopped"))
    }

    private var icon: String {
        guard watch.isWatching else { return "eye.slash" }
        return watch.events.last?.systemImage ?? "eye"
    }

    private var tint: Color {
        switch watch.events.last?.kind {
        case "checks_failed", "conflicts": .red
        case "checks_passed": .green
        case "activity": Color.brand
        default: .secondary
        }
    }
}
