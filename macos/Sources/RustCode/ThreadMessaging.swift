import AppKit
import RustCodeKit
import SwiftUI

/// A thread asking the user whether it may message another one (#2637).
struct ThreadGrantRequest: Identifiable {
    enum Choice { case always, once, deny }

    let id = UUID()
    let from: String
    let to: String
    let message: String
    let resolve: (Choice) -> Void
}

extension Orchestration {
    static let method = "_rustcode/orchestration"
}

/// The app's side of threads messaging threads: it answers the engine's
/// `_rustcode/orchestration` requests by asking the user, delivering the
/// message as a turn of the other thread through the ordinary queue, and
/// reporting that turn's answer. Every rule is in `Orchestration`.
extension DesktopModel {
    static var linkBookURL: URL {
        FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("RustCode/thread-links.json")
    }

    private func saveLinks() {
        linkBook.prune()
        try? linkBook.save(to: Self.linkBookURL)
    }

    private func thread(_ id: String) -> Orchestration.Thread? {
        guard let session = sessions.first(where: { $0.id == id }) else { return nil }
        return Orchestration.Thread(id: session.id, title: session.title ?? "Untitled thread", folder: session.cwd,
                                    updatedAt: session.updatedAt, isRunning: runningSessions.contains(session.id))
    }

    func answerOrchestration(_ params: JSONValue) async -> JSONValue {
        guard let callerId = params["sessionId"]?.stringValue, let caller = thread(callerId) else {
            return Orchestration.Failure("not_found", "the calling thread is not listed").json
        }
        switch Orchestration.Call.parse(params) {
        case .failure(let failure):
            return failure.json
        case .success(.list(let query)):
            let threads = sessions.compactMap { thread($0.id) }
            return Orchestration.list(threads, from: caller, query: query)
        case .success(.message(let target, let text)):
            return await message(from: caller, to: target, text: text)
        case .success(.wait(let links, let timeoutMs)):
            return await wait(for: links, from: callerId, timeoutMs: timeoutMs)
        }
    }

    private func message(from caller: Orchestration.Thread, to targetId: String, text: String) async -> JSONValue {
        if let failure = Orchestration.check(caller: caller, target: thread(targetId), book: linkBook) { return failure.json }
        guard let target = thread(targetId), let summary = sessions.first(where: { $0.id == targetId }) else {
            return Orchestration.Failure("not_found", "no thread has that id").json
        }
        if linkBook.grants[caller.id]?.contains(target.id) != true {
            // One question at a time; the agent can ask again.
            guard threadGrant == nil else {
                return Orchestration.Failure("busy", "another thread's request is waiting for the user").json
            }
            TurnNotifier.shared.notify(session: caller.id, title: "A thread wants to message another",
                                       body: "“\(caller.title)” → “\(target.title)”")
            let choice = await withCheckedContinuation { continuation in
                threadGrant = ThreadGrantRequest(from: caller.title, to: target.title, message: text) { choice in
                    continuation.resume(returning: choice)
                }
            }
            switch choice {
            case .deny:
                return Orchestration.Failure("denied", "the user did not allow messaging “\(target.title)”").json
            case .always:
                linkBook.grants[caller.id, default: []].insert(target.id)
            case .once:
                break
            }
            // The user took their time: look again.
            if let failure = Orchestration.check(caller: caller, target: thread(targetId), book: linkBook) { return failure.json }
        }
        let id = linkBook.nextId(for: caller.id)
        let delivered = Orchestration.delivered(text, from: caller.title)
        let busy = runningSessions.contains(target.id)
        linkBook.links.append(OrchestrationLink(id: id, orchestrator: caller.id, target: target.id,
                                                orchestratorTitle: caller.title, targetTitle: target.title,
                                                message: text, state: busy ? .queued : .working))
        saveLinks()
        guard await load(summary) else {
            linkBook.update(id) { $0.state = .failed; $0.answer = "That thread could not be opened." }
            saveLinks()
            return Orchestration.Failure("target_unavailable", "that thread could not be opened").json
        }
        let prompt = QueuedPrompt(sessionId: target.id, typed: delivered, text: delivered, images: [], link: id)
        if runningSessions.contains(target.id) {
            // After the turn it is running, never into it.
            queuedPrompts.append(prompt)
        } else {
            Task { await run(prompt) }
        }
        return ["link_id": .string(id), "receipt": .string(busy ? "queued" : "started"),
                "session_id": .string(target.id), "title": .string(target.title)]
    }

    /// What a link shows now: a running turn that waits on an approval or a
    /// question needs the user.
    func state(of link: OrchestrationLink) -> OrchestrationLink.State {
        if link.state == .working, permissionQueue.contains(where: { $0.sessionId == link.target }) { return .needsUser }
        return link.state
    }

    private func wait(for ids: [String], from caller: String, timeoutMs: Int) async -> JSONValue {
        let mine = ids.filter { linkBook.link($0)?.orchestrator == caller }
        guard !mine.isEmpty else { return Orchestration.Failure("not_found", "no such link from this thread").json }
        let deadline = Date().addingTimeInterval(Double(timeoutMs) / 1000)
        while true {
            for id in mine {
                guard var link = linkBook.link(id) else { continue }
                if sessions.contains(where: { $0.id == link.target }) == false, link.state.isActive {
                    linkBook.update(id) { $0.state = .failed; $0.answer = "That thread is gone." }
                    saveLinks()
                    link = linkBook.link(id) ?? link
                }
                let now = state(of: link)
                if !now.isActive || now == .needsUser {
                    link.state = now
                    return ["links": [link.result]]
                }
            }
            if Date() >= deadline {
                return ["status": "timeout",
                        "links": .array(mine.compactMap { id in linkBook.link(id).map { link in
                            var shown = link
                            shown.state = state(of: link)
                            return shown.result
                        } })]
            }
            try? await Task.sleep(for: .milliseconds(300))
        }
    }

    func linkStarted(_ id: String) {
        linkBook.update(id) { $0.state = .working }
        saveLinks()
    }

    /// The link's turn ended: its answer is that turn's last reply, after
    /// the attributed message, never an earlier one.
    func linkFinished(_ id: String, in session: String, failure: String?, cancelled: Bool) {
        let items = session == selectedSession ? transcript.items : openChats.snapshot(session)?.transcript.items ?? []
        let message = linkBook.link(id).map { Orchestration.delivered($0.message, from: $0.orchestratorTitle) }
        let start = items.lastIndex { item in
            if case .user(let text) = item.content { text == message } else { false }
        }
        var reply = ""
        if let start, let last = items[start...].last(where: { if case .assistant = $0.content { true } else { false } }),
           case .assistant(let text) = last.content {
            reply = text
        }
        // A turn that stopped on its own (a provider error, a guard) failed.
        let failure = failure ?? stoppedTurns[session]
        let (answer, truncated) = Orchestration.capped(reply)
        linkBook.update(id) { link in
            link.state = cancelled ? .cancelled : failure != nil ? .failed : .done
            link.answer = failure.map { answer.isEmpty ? $0 : "\(answer)\n\n\($0)" } ?? answer
            link.truncated = truncated
        }
        saveLinks()
    }

    /// Stop the work `orchestrator` gave other threads: their link turns
    /// are cancelled and their queued messages dropped. Turns the user
    /// started there keep running.
    func stopLinks(from orchestrator: String) async {
        for link in linkBook.links where link.orchestrator == orchestrator && link.state.isActive {
            await stop(link)
        }
    }

    func stop(_ link: OrchestrationLink) async {
        if link.state == .queued {
            queuedPrompts.removeAll { $0.link == link.id }
            linkBook.update(link.id) { $0.state = .cancelled }
        } else if link.state.isActive, let connection = engineConnection {
            try? await connection.notify(ACPMethod.sessionCancel, params: ACPRequests.session(link.target))
            let waiting = permissionQueue.filter { $0.sessionId == link.target }
            permissionQueue.removeAll { $0.sessionId == link.target }
            for request in waiting {
                try? await connection.respond(id: request.rpcId, result: ACPRequests.permissionCancelled)
            }
        }
        saveLinks()
    }

    /// Forget a finished link's chip.
    func dismiss(_ link: OrchestrationLink) {
        linkBook.links.removeAll { $0.id == link.id }
        saveLinks()
    }
}

/// Above a thread's composer: the threads it is messaging (#2638), and the
/// thread messaging it. A click opens the other thread.
struct LinkedThreads: View {
    @ObservedObject var model: DesktopModel

    var body: some View {
        let session = model.selectedSession
        let driving = model.linkBook.links.filter { $0.orchestrator == session }.suffix(4)
        let drivenBy = model.linkBook.links.last { $0.target == session && $0.state.isActive }
        if session != nil, !driving.isEmpty || drivenBy != nil {
            VStack(alignment: .leading, spacing: 6) {
                if let drivenBy { banner(drivenBy) }
                if !driving.isEmpty {
                    ScrollView(.horizontal, showsIndicators: false) {
                        HStack(spacing: 6) {
                            ForEach(Array(driving)) { link in chip(link) }
                        }
                    }
                }
            }
        }
    }

    private func banner(_ link: OrchestrationLink) -> some View {
        HStack(spacing: 8) {
            Image(systemName: "arrow.turn.down.right").foregroundStyle(Color.brand)
            Text("Working for “\(link.orchestratorTitle)”").lineLimit(1)
            Text("· \(model.state(of: link).label)").foregroundStyle(.secondary)
            Spacer(minLength: 4)
            Button("Open") { open(link.orchestrator) }.buttonStyle(PillButtonStyle())
            Button("Stop") { Task { await model.stop(link) } }
                .buttonStyle(PillButtonStyle())
                .help("Stop the work that thread gave this one; your own messages are not affected")
        }
        .font(.caption)
        .padding(.horizontal, 12)
        .padding(.vertical, 6)
        .glassSurface(in: .capsule)
    }

    private func chip(_ link: OrchestrationLink) -> some View {
        let state = model.state(of: link)
        return HStack(spacing: 6) {
            Image(systemName: icon(state)).foregroundStyle(tint(state))
            Button { open(link.target) } label: {
                Text("\(link.targetTitle) · \(state.label)").lineLimit(1)
            }
            .buttonStyle(.plain)
            .help(link.message)
            Button {
                Task { if state.isActive { await model.stop(link) } else { model.dismiss(link) } }
            } label: {
                Image(systemName: "xmark").font(.caption2.weight(.semibold)).foregroundStyle(.secondary)
            }
            .buttonStyle(.plain)
            .help(state.isActive ? "Stop this message's work in that thread" : "Remove")
        }
        .font(.caption)
        .padding(.horizontal, 10)
        .frame(height: 26)
        .glassSurface(in: .capsule)
    }

    private func open(_ id: String) {
        guard let session = model.sessions.first(where: { $0.id == id }) else { return }
        Task { await model.select(session) }
    }

    private func icon(_ state: OrchestrationLink.State) -> String {
        switch state {
        case .queued: "clock"
        case .working: "arrow.triangle.2.circlepath"
        case .needsUser: "hand.raised.fill"
        case .done: "checkmark.circle.fill"
        case .failed: "xmark.octagon.fill"
        case .cancelled, .interrupted: "pause.circle"
        }
    }

    private func tint(_ state: OrchestrationLink.State) -> Color {
        switch state {
        case .needsUser: .orange
        case .done: .green
        case .failed: .red
        default: .secondary
        }
    }
}
