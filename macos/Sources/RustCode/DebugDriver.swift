#if DEBUG
import AppKit
import RustCodeKit
import SwiftUI

/// Debug builds only: drives the app from a command file so a change can be
/// checked in the real window without a pointer. Set `RUSTCODE_DRIVE` to a
/// directory; each line written to `<dir>/cmd` runs once, and the window's
/// number is written to `<dir>/window` for `screencapture -l`.
@MainActor
enum DebugDriver {
    static func start(_ model: DesktopModel) {
        guard let dir = ProcessInfo.processInfo.environment["RUSTCODE_DRIVE"] else { return }
        let command = URL(fileURLWithPath: dir).appendingPathComponent("cmd")
        Task { @MainActor in
            while true {
                try? await Task.sleep(for: .milliseconds(300))
                if let window = NSApp.windows.first(where: { $0.isVisible && $0.contentView != nil }) {
                    try? String(window.windowNumber).write(toFile: dir + "/window", atomically: true, encoding: .utf8)
                }
                guard let text = try? String(contentsOf: command, encoding: .utf8), !text.isEmpty else { continue }
                try? FileManager.default.removeItem(at: command)
                for line in text.split(separator: "\n") { await run(String(line), model) }
            }
        }
    }

    private static let smartUISample = """
        <title>Slowest tests</title>
        <meta name="description" content="Filter by name, click a heading to sort">
        <style>
        input { width: 100%; box-sizing: border-box; padding: 4px 8px; margin-bottom: 6px; border-radius: 6px; border: 1px solid #8886; background: Canvas; color: CanvasText; }
        table { border-collapse: collapse; width: 100%; } th { text-align: left; cursor: pointer; color: AccentColor; }
        td, th { padding: 3px 6px; border-bottom: 1px solid #8883; } td:last-child, th:last-child { text-align: right; }
        </style>
        <input id="q" placeholder="Filter tests"><table><thead><tr><th data-c="0">Test</th><th data-c="1">Time (s)</th></tr></thead><tbody id="b"></tbody></table>
        <script>
        let rows = [["network::tests::stream_retry", 4.2], ["config::session_open", 0.4], ["tools::exec::timeout", 3.1], ["acp::load_session", 1.7], ["compaction::repeated", 0.9], ["tui::scroll::anchor", 2.6]];
        let desc = true;
        const body = document.getElementById('b'), q = document.getElementById('q');
        const draw = () => { body.innerHTML = rows.filter(r => r[0].includes(q.value)).map(r => `<tr><td>${r[0]}</td><td>${r[1].toFixed(1)}</td></tr>`).join(''); };
        document.querySelectorAll('th').forEach(th => th.onclick = () => { const c = +th.dataset.c; desc = !desc; rows.sort((a, b) => (a[c] > b[c] ? 1 : -1) * (desc ? -1 : 1)); draw(); });
        q.oninput = draw; rows.sort((a, b) => b[1] - a[1]); draw();
        </script>
        """

    private static func session(_ index: String, _ model: DesktopModel) -> SessionSummary? {
        Int(index).flatMap { model.visibleSessions.indices.contains($0) ? model.visibleSessions[$0] : nil }
    }

    private static func ms(_ from: Date, _ to: Date) -> Int { Int(to.timeIntervalSince(from) * 1000) }

    /// Appends a line to `<dir>/time`.
    private static func report(_ line: String) {
        let path = (ProcessInfo.processInfo.environment["RUSTCODE_DRIVE"] ?? "/tmp") + "/time"
        let old = (try? String(contentsOfFile: path, encoding: .utf8)) ?? ""
        try? (old + line + "\n").write(toFile: path, atomically: true, encoding: .utf8)
    }

    private static func run(_ line: String, _ model: DesktopModel) async {
        let parts = line.split(separator: " ").map(String.init)
        switch parts.first {
        case "select": if let s = session(parts[1], model) { await model.select(s) }
        case "open":
            await model.select(model.sessions.first { $0.id == parts[1] } ?? SessionSummary(id: parts[1], cwd: model.workspace.path))
        case "timeopen":
            // How long a chat takes to open and to draw its first frame.
            let start = Date()
            await model.select(model.sessions.first { $0.id == parts[1] } ?? SessionSummary(id: parts[1], cwd: model.workspace.path))
            let loaded = Date()
            let window = NSApp.windows.first { $0.isVisible && $0.contentView != nil }
            window?.contentView?.layoutSubtreeIfNeeded()
            window?.displayIfNeeded()
            let drawn = Date()
            report("open items \(model.transcript.items.count) load \(ms(start, loaded)) ms first frame \(ms(loaded, drawn)) ms")
        case "synth":
            // `synth <items> [long]`: a long chat of mixed messages, shown at
            // once; with `long`, questions and every other answer run long.
            let count = Int(parts[1]) ?? 3000
            let long = parts.contains("long")
            var transcript = Transcript()
            for index in 0..<count {
                switch index % 4 {
                case 0 where long:
                    transcript.apply(.userText((0..<20).map { "Question \(index) line \($0): how does the parser handle nested blocks?" }.joined(separator: "\n")))
                case 2 where long:
                    transcript.apply(.agentText((0..<30).map { "Answer \(index) paragraph \($0), with `code` and **bold** text." }.joined(separator: "\n\n")))
                case 0: transcript.apply(.userText("Question \(index): how does the parser handle nested blocks?"))
                case 1: transcript.apply(.toolCall(id: "t\(index)", title: "Read src/file\(index).rs", kind: "read", status: .completed,
                                                   input: nil, detail: "src/file\(index).rs"))
                default: transcript.apply(.agentText("Answer \(index). " + String(repeating: "The parser walks each block and **keeps** its `state` for the next one. ", count: 1 + index % 7) + "\n\n- one\n- two\n\n```swift\nlet x = \(index)\n```\n"))
                }
            }
            let start = Date()
            model.transcript = transcript
            // The chat publishes on its next frame.
            try? await Task.sleep(for: .milliseconds(40))
            let window = NSApp.windows.first { $0.isVisible && $0.contentView != nil }
            window?.contentView?.layoutSubtreeIfNeeded()
            window?.displayIfNeeded()
            report("synth items \(model.transcript.items.count) first frame \(ms(start, Date())) ms")
        case "turncard":
            // A finished turn that edited, checked and committed, to look at
            // its summary card without a model.
            var transcript = Transcript()
            let start = Date()
            transcript.appendUser("Fill in the changelog hash and commit it.")
            transcript.beginTurn(at: start)
            let edits = [("e1", "/p/CHANGELOG.md", "a\nb\n", "a\nc\nd\ne\n"),
                         ("e2", "/p/levering/kompetansemaal-12.md", "x\ny\nz\n", "x\nw\nv\n")]
            for (id, path, old, new) in edits {
                transcript.apply(.toolCall(id: id, title: "edit_file", kind: "edit", status: .inProgress, input: nil))
                transcript.apply(.toolCallUpdate(id: id, title: nil, status: .completed, output: "ok",
                                                 diffs: [FileDiff(path: path, oldText: old, newText: new)]))
            }
            let commands: [(String, ToolStatus)] = [
                ("git status --short --branch && git diff --check && git diff -- CHANGELOG.md levering/kompetansemaal-12.md && git add -- CHANGELOG.md", .completed),
                ("cargo test --workspace", .completed),
                ("npm run lint", .failed),
                ("git diff --check && git add -- CHANGELOG.md && git commit -m 'docs(changelog): fyll inn hash for kundebehov' && git status --short --branch && git log --oneline -1", .completed),
                ("git push -u origin docs/changelog-hash", .completed),
            ]
            for (index, (command, status)) in commands.enumerated() {
                transcript.apply(.toolCall(id: "c\(index)", title: "run_command", kind: nil, status: .inProgress,
                                           input: ["command": .string(command)]))
                transcript.apply(.toolCallUpdate(id: "c\(index)", title: nil, status: status, output: "ok",
                                                 exitCode: status == .completed ? 0 : 1))
            }
            transcript.apply(.agentText("Done. The changelog has the hash, and the change is committed and pushed."))
            transcript.finishTurn(at: start.addingTimeInterval(89))
            model.transcript = transcript
        case "timescroll":
            // `timescroll <steps> <points> [reader] [trace]`: scroll the chat up step by step,
            // laying out and drawing each, as a reader paging back would.
            let steps = Int(parts[1]) ?? 40
            let delta = Double(parts.count > 2 ? parts[2] : "800") ?? 800
            func find(_ view: NSView) -> NSScrollView? {
                if let scroll = view as? NSScrollView, scroll.frame.width > 400 { return scroll }
                return view.subviews.lazy.compactMap(find).first
            }
            guard let window = NSApp.windows.first(where: { $0.isVisible && $0.contentView != nil }),
                  let scroll = window.contentView.flatMap(find) else { break }
            var worst = 0.0, total = 0.0
            for _ in 0..<steps {
                let start = Date()
                // `reader`: each step is the reader's own wheel, which lets go
                // of the end, as a real scroll does.
                if parts.contains("reader") { ChatScrollInput.shared.debugObserve(deltaY: delta) }
                var bounds = scroll.contentView.bounds
                bounds.origin.y -= delta
                scroll.contentView.setBoundsOrigin(scroll.contentView.constrainBoundsRect(bounds).origin)
                scroll.reflectScrolledClipView(scroll.contentView)
                window.contentView?.layoutSubtreeIfNeeded()
                window.displayIfNeeded()
                let step = Date().timeIntervalSince(start) * 1000
                if parts.contains("trace") { report("  y \(Int(scroll.contentView.bounds.minY)) \(ChatScrollState.current?.debugDescription ?? "-")") }
                worst = max(worst, step)
                total += step
                try? await Task.sleep(for: .milliseconds(20))
            }
            report("timescroll \(steps)x\(Int(delta)) avg \(Int(total / Double(steps))) ms worst \(Int(worst)) ms, at y \(Int(scroll.contentView.bounds.minY)) of \(Int(scroll.documentView?.frame.height ?? 0))")
        case "diff":
            // An edit whose diff ends in added lines, at the end of the chat.
            let old = (0..<12).map { "line \($0)" }.joined(separator: "\n")
            let new = (0..<4).map { "line \($0)" }.joined(separator: "\n") + "\n.added {\n  display: grid;\n  gap: 12px;\n}\n"
            model.transcript.apply(.toolCall(id: "diff", title: "Edit styles.css", kind: "edit", status: .completed, input: nil))
            model.transcript.apply(.toolCallUpdate(id: "diff", title: nil, status: .completed, output: nil,
                                                   diffs: [FileDiff(path: "/tmp/styles.css", oldText: old, newText: new)]))
        case "stream":
            // `stream <chunks> [table]`: a reply streamed into the shown chat
            // at 30 ms a chunk; the time beyond that is what drawing it cost.
            // With `table`, the reply is a table written a cell at a time.
            let chunks = Int(parts[1]) ?? 100
            let table = parts.contains("table")
            let start = Date()
            for index in 0..<chunks {
                let chunk = table
                    ? (index == 0 ? "| Name | Kind | Notes |\n|---|---|---|\n" : index % 3 == 0 ? "row \(index) note |\n" : "| cell \(index) ")
                    : index % 12 == 0 ? "\n\nNext paragraph with `code` and **bold** " : "word "
                model.transcript.apply(.agentText(chunk))
                try? await Task.sleep(for: .milliseconds(30))
            }
            let elapsed = Date().timeIntervalSince(start) * 1000
            report("stream \(chunks) chunks over \(model.transcript.items.count) items: \(Int(elapsed)) ms, overhead \(Int(elapsed) - chunks * 30) ms")
        case "snap":
            // `snap <path.png>`: the window drawn into a PNG by the app
            // itself, for hosts where `screencapture` lacks permission.
            guard parts.count > 1, let view = NSApp.windows.first(where: { $0.isVisible && $0.contentView != nil })?
                .contentView?.superview ?? NSApp.windows.first?.contentView,
                  let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds) else { break }
            view.cacheDisplay(in: view.bounds, to: rep)
            try? rep.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: parts[1]))
        case "hit":
            // `hit <x> <y>`: which AppKit view takes the pointer at a point
            // of the window, from its top left; a link shows `LinkHitView`.
            guard parts.count > 2, let x = Double(parts[1]), let y = Double(parts[2]),
                  let root = NSApp.windows.first(where: { $0.isVisible && $0.contentView != nil })?.contentView?.superview else { break }
            let hit = root.hitTest(NSPoint(x: x, y: root.bounds.height - y))
            report("hit \(Int(x)),\(Int(y)): \(hit.map { String(describing: type(of: $0)) } ?? "nothing")")
        case "whatsnew":
            // `whatsnew <tag> [full]`: that release's What's New card, or its changelog.
            await model.whatsNew.debugShow(tag: parts.count > 1 ? parts[1] : "v0.103.0", full: parts.contains("full"))
        case "vizguide": model.showsVisualizationGuide = true
        case "vizreply":
            // `vizreply [ms] [type]`: a reply holding the guide samples (or
            // one type's), streamed in 40-character chunks in the background,
            // so a `snap` can catch the open-fence placeholder.
            let delay = Int(parts.count > 1 ? parts[1] : "") ?? 20
            let entries = VisualizationGuide.entries.filter { parts.count < 3 || $0.type == parts[2] }
            let text = entries.map { "\($0.name):\n\n\($0.sampleMarkdown)" }.joined(separator: "\n\n")
            Task { @MainActor in
                var rest = Substring(text)
                while !rest.isEmpty {
                    model.transcript.apply(.agentText(String(rest.prefix(40))))
                    rest = rest.dropFirst(40)
                    try? await Task.sleep(for: .milliseconds(delay))
                }
            }
        case "uireply":
            // `uireply`: a reply holding a Smart UI block (#2484), streamed
            // like `vizreply`. Needs Smart UI on (`-smartUIEnabled YES`).
            let text = "Here is a filterable view:\n\n```rustcode-ui v1\n\(smartUISample)\n```\n\nType to filter; click a heading to sort."
            Task { @MainActor in
                var rest = Substring(text)
                while !rest.isEmpty {
                    model.transcript.apply(.agentText(String(rest.prefix(80))))
                    rest = rest.dropFirst(80)
                    try? await Task.sleep(for: .milliseconds(15))
                }
            }
        case "send":
            // `send <text>`: a follow-up as the composer sends it, starting a
            // turn in the shown chat without an engine.
            let session = model.selectedSession ?? "drive"
            if model.selectedSession == nil { model.selectedSession = session }
            model.draft = ""
            model.transcript.beginTurn(at: Date())
            model.transcript.appendUser(parts.dropFirst().joined(separator: " "))
            model.debugTurn(true, in: session)
        case "end":
            model.debugTurn(false, in: model.selectedSession ?? "drive")
        case "vizturn":
            // `vizturn`: a turn whose call never gets its finishing update,
            // then a reply holding a line chart, then the turn's end. After
            // it no card may still spin, and the empty one is not drawn.
            let session = model.selectedSession ?? "drive"
            if model.selectedSession == nil { model.selectedSession = session }
            model.transcript.appendUser("How has the release binary size changed? Show it as a line chart.")
            model.debugTurn(true, in: session)
            model.transcript.apply(.toolCall(id: "vizturn-run", title: "run_command", kind: "execute", status: .inProgress,
                                             input: .object(["command": .string("gh release list")])))
            model.transcript.apply(.toolCallUpdate(id: "vizturn-orphan", title: nil, status: .inProgress, output: nil, diffs: []))
            let sample = VisualizationGuide.entries.first { $0.type == "line_chart" }?.sampleMarkdown ?? ""
            var rest = Substring("Here is the trend:\n\n\(sample)\n\nSize dropped after LTO.")
            while !rest.isEmpty {
                model.transcript.apply(.agentText(String(rest.prefix(40))))
                rest = rest.dropFirst(40)
                try? await Task.sleep(for: .milliseconds(20))
            }
            model.debugTurn(false, in: session)
            let spinning = model.transcript.items.filter {
                if case .tool(let tool) = $0.content { return tool.status == .pending || tool.status == .inProgress }
                return false
            }
            report("vizturn items \(model.transcript.items.count) spinning \(spinning.count)")
        case "vis":
            // `vis <tag>`: how many drawn views lie in the chat's visible area;
            // zero with content in the chat is the blank chat.
            if let scroll = ChatScrollState.current?.scrollView, let doc = scroll.documentView {
                let visible = scroll.contentView.bounds
                var count = 0
                func walk(_ view: NSView) {
                    for sub in view.subviews {
                        let frame = sub.convert(sub.bounds, to: doc)
                        if frame.intersects(visible), frame.height < 4000 { count += 1 }
                        walk(sub)
                    }
                }
                walk(doc)
                report("vis \(parts.count > 1 ? parts[1] : "-") \(count) views, clip y \(Int(visible.minY)) doc \(Int(doc.frame.height)) items \(model.transcript.items.count)")
            }
        case "rail":
            // `rail [tag]`: the session timeline's markers (#2309): how many
            // turns and ticks, the current turn, and how many ticks' messages
            // are in view.
            guard let state = ChatScrollState.current else { break }
            let layout = state.rail.layout
            let inView = layout.markers.filter(\.isInView).count
            let current = layout.markers.first(where: \.isCurrent).map { "turn \($0.number) row \($0.id)" } ?? "-"
            report("rail \(parts.count > 1 ? parts[1] : "-") turns \(state.rail.turnCount) markers \(layout.markers.count) current \(current) in view \(inView)")
        case "railjump":
            // `railjump <turn>`: go to turn `turn` (from 1) as its marker
            // does, then where its message is in the view over the next
            // second.
            guard let state = ChatScrollState.current else { break }
            let turns = TimelineTurn.turns(in: model.transcript.items)
            let number = min(max(Int(parts.count > 1 ? parts[1] : "1") ?? 1, 1), turns.count)
            guard number > 0 else { break }
            let id = turns[number - 1].id
            state.debugJumpToTurn(id)
            var seen: [String] = []
            for _ in 0..<8 {
                try? await Task.sleep(for: .milliseconds(125))
                seen.append(state.debugScreenTop(id).map { String(Int($0)) } ?? "-")
            }
            let current = state.rail.layout.markers.first(where: \.isCurrent).map { String($0.number) } ?? "-"
            report("railjump \(number) row \(id) top over 1 s: \(seen.joined(separator: " ")) current \(current)")
        case "jumpvis":
            // `jumpvis <points>`: one jump, as a scroller drag far up does,
            // then how many rows of output are drawn in view right away and
            // over the next frames.
            guard let state = ChatScrollState.current else { break }
            ChatScrollInput.shared.debugObserve(deltaY: 1)
            state.debugLetGo()
            state.scroll(by: Double(parts.count > 1 ? parts[1] : "-20000") ?? -20000, animated: false)
            var seen: [String] = []
            for _ in 0..<6 {
                let window = NSApp.windows.first { $0.isVisible && $0.contentView != nil }
                window?.contentView?.layoutSubtreeIfNeeded()
                seen.append(String(state.debugVisible.split(separator: "[").last?.split(separator: ",").count ?? 0))
                try? await Task.sleep(for: .milliseconds(16))
            }
            report("jumpvis rows in view per frame: \(seen.joined(separator: " "))")
        case "watch":
            // `watch <seconds> <tag>`: log the chat's visible rows every 50 ms, in the background.
            let seconds = Double(parts.count > 1 ? parts[1] : "3") ?? 3
            let tag = parts.count > 2 ? parts[2] : "w"
            Task { @MainActor in
                let start = Date()
                while Date().timeIntervalSince(start) < seconds {
                    report("\(tag) \(ms(start, Date())) \(ChatScrollState.current?.debugVisible ?? "-") items \(model.transcript.items.count)")
                    try? await Task.sleep(for: .milliseconds(50))
                }
            }
        case "viewimage":
            // `viewimage <path>`: open a file in the image viewer.
            model.viewedImage = .file(URL(fileURLWithPath: parts[1]))
        case "pace":
            // `pace <ms> [stream]`: for `ms`, how the main thread kept up while
            // real input (as an external tool posts) scrolls the chat: the
            // worst gap between 8 ms timer ticks, the gaps over 25 ms, and how
            // far the chat moved (#2442). With `stream`, a reply streams meanwhile.
            let duration = Double(parts.count > 1 ? parts[1] : "3000") ?? 3000
            let streaming = Task { @MainActor in
                guard parts.contains("stream") else { return }
                for index in 0..<Int(duration / 30) {
                    guard !Task.isCancelled else { return }
                    model.transcript.apply(.agentText(index % 12 == 0 ? "\n\nNext paragraph with `code` and **bold** " : "word "))
                    try? await Task.sleep(for: .milliseconds(30))
                }
            }
            let scroll = ChatScrollState.current?.scrollView
            let startY = scroll?.contentView.bounds.minY ?? 0
            var gaps: [Double] = []
            var last = Date()
            // The top row's on-screen moves between ticks: during a scroll in
            // one direction they share a sign, so the other sign is the chat
            // jumping back, and a big one is a jump.
            var moves: [Double] = []
            var lastTop: (row: Int, minY: Double)?
            let ticker = Timer.scheduledTimer(withTimeInterval: 0.008, repeats: true) { _ in
                MainActor.assumeIsolated {
                    gaps.append(Date().timeIntervalSince(last) * 1000)
                    last = Date()
                    let top = ChatScrollState.current?.topRow()
                    if let top, let lastTop, top.row == lastTop.row, top.minY != lastTop.minY {
                        moves.append(top.minY - lastTop.minY)
                    }
                    lastTop = top
                }
            }
            RunLoop.main.add(ticker, forMode: .common)
            try? await Task.sleep(for: .milliseconds(Int(duration)))
            ticker.invalidate()
            streaming.cancel()
            let moved = Int(abs((scroll?.contentView.bounds.minY ?? 0) - startY))
            let long = gaps.filter { $0 > 25 }
            let sign: Double = moves.reduce(0, +) >= 0 ? 1 : -1
            let back = moves.filter { $0 * sign < -1 }
            let jumps = moves.filter { abs($0) > 60 }
            func list(_ values: [Double]) -> String { values.prefix(12).map { String(Int($0)) }.joined(separator: ",") }
            report("pace \(Int(duration)) ms: worst \(Int(gaps.max() ?? 0)) ms, over 25 ms \(long.count) (\(list(long))), moved \(moved), row moves \(moves.count), back \(back.count) (\(list(back))), jumps \(jumps.count) (\(list(jumps)))")
        case "wheelstream":
            // `wheelstream <steps> <points> [nostream|nowheel]`: a reply
            // streams while the reader scrolls up `steps` times by `points`, a
            // frame apart, then rests (#2260). Reports how far the chat moved
            // against how far it was scrolled, how far it drifted at rest as
            // output kept coming, and the main thread's worst stall. Run it
            // under `sample` to see where the main thread goes.
            let steps = Int(parts.count > 1 ? parts[1] : "20") ?? 20
            let points = Int32(parts.count > 2 ? parts[2] : "40") ?? 40
            guard let scroll = ChatScrollState.current?.scrollView else { break }
            let streaming = Task { @MainActor in
                for index in 0..<400 {
                    guard !Task.isCancelled, !parts.contains("nostream") else { return }
                    model.transcript.apply(.agentText(index % 12 == 0 ? "\n\nNext paragraph with `code` and **bold** " : "word "))
                    try? await Task.sleep(for: .milliseconds(30))
                }
            }
            var worstGap = 0.0
            var last = Date()
            let ticker = Timer.scheduledTimer(withTimeInterval: 0.008, repeats: true) { _ in
                MainActor.assumeIsolated {
                    worstGap = max(worstGap, Date().timeIntervalSince(last) * 1000)
                    last = Date()
                }
            }
            try? await Task.sleep(for: .milliseconds(600))
            // Synthesized wheel events do not reach SwiftUI's scroll view, so
            // each step tells the input monitor what a wheel event would, then
            // moves the chat by its delta as the scroll view does.
            func wheel(_ delta: Int32) {
                ChatScrollInput.shared.debugObserve(deltaY: Double(delta))
                ChatScrollState.current?.scroll(by: -Double(delta), animated: false)
            }
            let startY = scroll.contentView.bounds.minY
            // A row of output at the top, measured where it is drawn: the
            // scroll offset also moves when rows above take their height.
            let state = ChatScrollState.current
            let row = state?.topRow().map(\.row).flatMap { $0 == Int.max ? nil : $0 }
            let rowStart = row.flatMap { state?.debugScreenTop($0) }
            worstGap = 0
            for _ in 0..<steps {
                if !parts.contains("nowheel") { wheel(points) }
                try? await Task.sleep(for: .milliseconds(16))
            }
            let wheeledY = scroll.contentView.bounds.minY
            let rowWheeled = row.flatMap { state?.debugScreenTop($0) }
            let wheelGap = worstGap
            try? await Task.sleep(for: .milliseconds(1500))
            let restY = scroll.contentView.bounds.minY
            let rowRest = row.flatMap { state?.debugScreenTop($0) }
            var rowMoved = ""
            if let rowStart, let rowWheeled {
                rowMoved = "row moved \(Int(rowWheeled - rowStart)), at rest \(Int((rowRest ?? rowWheeled) - rowWheeled)); "
            }
            streaming.cancel()
            ticker.invalidate()
            report("wheelstream \(steps)x\(points): \(rowMoved)moved \(Int(startY - wheeledY)) of \(Int(steps) * Int(points)), drift at rest \(Int(restY - wheeledY)), worst stall wheeling \(Int(wheelGap)) ms, overall \(Int(worstGap)) ms")
        case "readback":
            // `readback <steps> <points> [trace]`: the reader wheels up the
            // chat a frame at a time, through pages of older rows joining
            // above (#2442, #2445). Each step follows the row at the top of
            // the view: it should move down by exactly the step. Reports how
            // often and how far it strayed, and the pages that joined.
            let steps = Int(parts.count > 1 ? parts[1] : "300") ?? 300
            let points = Double(parts.count > 2 ? parts[2] : "120") ?? 120
            guard let state = ChatScrollState.current else { break }
            var strays = 0, worst = 0.0, pages = 0, lost = 0
            var rows = state.debugRowCount
            for index in 0..<steps {
                let before = state.topRow().flatMap { top in top.row == Int.max ? nil : state.debugScreenTop(top.row).map { (top.row, $0, top.minY) } }
                ChatScrollInput.shared.debugObserve(deltaY: points)
                state.scroll(by: -points, animated: false)
                try? await Task.sleep(for: .milliseconds(16))
                if state.debugRowCount != rows { pages += 1; rows = state.debugRowCount }
                guard let (row, top, frameTop) = before else { continue }
                guard let now = state.debugScreenTop(row) else { lost += 1; continue }
                let stray = now - top - points
                // Where the row reported itself drawn, from its own frame.
                let frameStray = state.debugFrameTop(row).map { $0 - frameTop - points }
                if abs(stray) > 2 {
                    strays += 1
                    worst = max(worst, abs(stray))
                    if parts.contains("trace") { report("  step \(index) row \(row) stray \(Int(stray)) frame \(frameStray.map { String(Int($0)) } ?? "-") rows \(rows)") }
                }
            }
            report("readback \(steps)x\(Int(points)): strays \(strays), worst \(Int(worst)), lost \(lost), pages \(pages), rows \(rows), \(state.debugDescription)")
        case "toolstream":
            // `toolstream <steps> [ms] [text] [wheel] [noitems]`: a turn keeps
            // working while the reader is away from the end (#2310). Each step
            // adds a tool call that runs and finishes, with `text` a line of
            // reply after it, `ms` apart; with `wheel` the reader also scrolls
            // up 40 points a step, and with `noitems` only scrolls. Reports how far the row that was at the top
            // of the view strayed from where the reader's own scrolling put it.
            let steps = Int(parts.count > 1 ? parts[1] : "30") ?? 30
            let pause = Int(parts.count > 2 ? parts[2] : "150") ?? 150
            // The working row moves with every item; measure a row of output.
            try? await Task.sleep(for: .milliseconds(50))
            guard let state = ChatScrollState.current, let row = state.topRow()?.row, row != Int.max,
                  let start = state.debugScreenTop(row) else {
                report("toolstream: no row at the top")
                break
            }
            var worst = 0.0, lost = 0, last = start, scrolled = 0.0
            func check() {
                guard let now = state.debugScreenTop(row) else { lost += 1; return }
                last = now
                worst = max(worst, abs(now - start - scrolled))
                if parts.contains("trace") {
                    report("  stray \(Int(now - start - scrolled)) top \(state.topRow().map { "\($0.row)@\(Int($0.minY))" } ?? "-") \(state.debugVisible.prefix(60))")
                }
            }
            for index in 0..<steps {
                // `panel`: the reader scrolls an agent's panel over the chat.
                if parts.contains("panel") { ChatScrollInput.shared.debugObserve(deltaY: 40) }
                if parts.contains("wheel") {
                    ChatScrollInput.shared.debugObserve(deltaY: 40)
                    state.scroll(by: -40, animated: false)
                    scrolled += 40
                }
                guard !parts.contains("noitems") else {
                    try? await Task.sleep(for: .milliseconds(pause))
                    check()
                    continue
                }
                let id = "toolstream-\(index)-\(model.transcript.items.count)"
                model.transcript.apply(.toolCall(id: id, title: "run_command", kind: "execute", status: .inProgress,
                                                 input: nil, detail: "cargo test --package step-\(index)",
                                                 summary: "Testing step \(index)"))
                try? await Task.sleep(for: .milliseconds(pause / 2))
                check()
                model.transcript.apply(.toolCallUpdate(id: id, title: nil, status: .completed,
                                                       output: (0..<8).map { "ok line \($0)" }.joined(separator: "\n"), diffs: []))
                if parts.contains("text") { model.transcript.apply(.agentText("Step \(index) passed; moving on.")) }
                try? await Task.sleep(for: .milliseconds(pause / 2))
                check()
            }
            report("toolstream \(steps): row \(row) from \(Int(start)) scrolled \(Int(scrolled)), now \(Int(last)), worst stray \(Int(worst)), lost \(lost) of \(steps * 2) checks")
        case "fold":
            // `fold [user] [up]`: opens or folds the first long user message
            // in view, as its Show more control does (#2303);
            // with `up`, the chat is first scrolled so the message's top is
            // above the view. Reports where its top and end are in the view
            // before and over the next frames: opening and folding keep its
            // top, folding one whose top is above keeps its end.
            guard let state = ChatScrollState.current else { break }
            let window = NSApp.windows.first { $0.isVisible && $0.contentView != nil }
            func frame(_ id: Int) -> (top: Int, end: Int)? {
                guard let top = state.debugScreenTop(id), let height = state.debugFrame(id)?.height else { return nil }
                return (Int(top), Int(top + height))
            }
            let candidates = model.transcript.items.filter { item in
                switch item.content {
                case .user(let text): MessageCollapse.mayBeLong(text)
                default: false
                }
            }
            guard let item = candidates.first(where: { item in
                guard let place = frame(item.id) else { return false }
                return place.top < Int(state.visible) && place.end > 0
            }) else {
                report("fold: no long message in view")
                break
            }
            if parts.contains("up"), let place = frame(item.id) {
                ChatScrollInput.shared.debugObserve(deltaY: -1)
                state.debugLetGo()
                state.scroll(by: Double(place.top) + 60, animated: false)
                // Past the input monitor's window: the click is not a scroll.
                try? await Task.sleep(for: .milliseconds(400))
            }
            let before = frame(item.id)
            let reported = state.debugFrame(item.id).map { "\(Int($0.minY))+\(Int($0.height))" } ?? "-"
            state.debugToggleMessage(item.id)
            var after: [String] = []
            for _ in 0..<8 {
                window?.contentView?.layoutSubtreeIfNeeded()
                try? await Task.sleep(for: .milliseconds(16))
                after.append((frame(item.id).map { "\($0.top)..\($0.end)" } ?? "-") + (parts.contains("clip") ? "@\(Int(state.scrollView?.contentView.bounds.minY ?? -1))" : ""))
            }
            report("fold \(item.id) \(model.expandedInChat.contains(item.id) ? "opened" : "folded"): reported \(reported) before \(before.map { "\($0.top)..\($0.end)" } ?? "-") after \(after.joined(separator: " "))")
        case "scrollby":
            // `scrollby <points> [steps]`: move the chat, negative up, a step a frame.
            let steps = Int(parts.count > 2 ? parts[2] : "1") ?? 1
            let delta = Double(parts[1]) ?? -800
            for _ in 0..<steps {
                // As a wheel event over the chat tells the input monitor.
                ChatScrollInput.shared.debugObserve(deltaY: -delta)
                ChatScrollState.current?.debugLetGo()
                ChatScrollState.current?.scroll(by: delta, animated: false)
                try? await Task.sleep(for: .milliseconds(16))
            }
        case "big":
            // `big <prose|code|line|tool|markdown|links|quote> <size> [chunks]`: a large reply
            // streamed in `chunks` pieces at 30 ms each.
            let kind = parts.count > 1 ? parts[1] : "prose"
            let size = Int(parts.count > 2 ? parts[2] : "2000") ?? 2000
            let chunks = max(Int(parts.count > 3 ? parts[3] : "40") ?? 40, 1)
            if kind == "tool" {
                model.transcript.apply(.toolCall(id: "big", title: "Run tests", kind: "execute", status: .inProgress, input: nil))
                model.transcript.apply(.toolCallUpdate(id: "big", title: nil, status: .completed,
                                                       output: (0..<size).map { "output line \($0)" }.joined(separator: "\n"), diffs: []))
                break
            }
            let text = switch kind {
            case "code": "Here:\n\n```swift\n" + (0..<size).map { "let value\($0) = compute(\($0)) // line" }.joined(separator: "\n") + "\n```\n"
            case "wide": "Here:\n\n```swift\n" + (0..<size).map { "let value\($0) = compute(\($0)) // " + String(repeating: "wide ", count: 40) }.joined(separator: "\n") + "\n```\n"
            case "line": String(repeating: "word ", count: size)
            case "markdown": DebugDriver.markdownSample
            case "links": DebugDriver.linksSample
            case "quote": DebugDriver.quoteSample
            default: (0..<size).map { "Paragraph \($0) of a long reply, with `code` and **bold** text." }.joined(separator: "\n\n")
            }
            let step = max(text.count / chunks, 1)
            var rest = Substring(text)
            while !rest.isEmpty {
                model.transcript.apply(.agentText(String(rest.prefix(step))))
                rest = rest.dropFirst(step)
                try? await Task.sleep(for: .milliseconds(30))
            }
        case "pin": if let s = session(parts[1], model) { model.togglePin(s) }
        case "new": await model.newSession()
        case "sidebar": model.toggleSidebar()
        case "pulls":
            // `pulls [number]`: open the pull requests panel, or one of them.
            model.showsPullRequests = true
            if parts.count > 1 { model.pullToOpen = Int(parts[1]) }
        case "pullguide":
            model.makesPullGuide = true
        case "watchpull":
            // `watchpull <number>`: watch it from the shown thread.
            if parts.count > 1, let number = Int(parts[1]) { await model.watchPull(number) }
        case "orch":
            // `orch <tool> <json args>`: an agent's call from the shown thread,
            // as the engine forwards it; the answer goes to `<drive>/orch.json`.
            let rest = line.split(separator: " ", maxSplits: 2).map(String.init)
            guard rest.count >= 2, let session = model.selectedSession,
                  let dir = ProcessInfo.processInfo.environment["RUSTCODE_DRIVE"] else { break }
            let args = rest.count > 2 ? (try? JSONDecoder().decode(JSONValue.self, from: Data(rest[2].utf8))) ?? [:] : [:]
            Task { @MainActor in
                let answer = await model.answerOrchestration(["sessionId": .string(session), "tool": .string(rest[1]), "args": args])
                let data = (try? JSONEncoder().encode(answer)) ?? Data()
                try? data.write(to: URL(fileURLWithPath: dir).appendingPathComponent("orch.json"))
            }
        case "grant":
            // `grant always|once|deny`: answer a thread's request to message another.
            if let grant = model.threadGrant {
                model.threadGrant = nil
                grant.resolve(parts.count > 1 && parts[1] == "once" ? .once : parts.count > 1 && parts[1] == "deny" ? .deny : .always)
            }
        case "pullfile":
            // `pullfile <path>`: open a file's diff in the open pull request.
            if parts.count > 1 { model.pullFileToOpen = parts[1] }
        case "banner":
            // `banner <path> [picture]` or `banner none`: the new thread
            // banner as Settings sets it, from a picture on disk.
            let defaults = UserDefaults.standard
            NewThreadBanner.remove(defaults.string(forKey: NewThreadBanner.key) ?? "")
            defaults.set(parts.contains("picture") ? "picture" : "ascii", forKey: NewThreadBanner.styleKey)
            if parts.count > 1, parts[1] != "none" {
                let source = URL(fileURLWithPath: parts[1].replacingOccurrences(of: "%20", with: " "))
                let name = "new-thread-banner-drive.\(source.pathExtension)"
                try? FileManager.default.createDirectory(at: NewThreadBanner.directory, withIntermediateDirectories: true)
                try? FileManager.default.copyItem(at: source, to: NewThreadBanner.directory.appendingPathComponent(name))
                defaults.set(name, forKey: NewThreadBanner.key)
            } else {
                defaults.set("", forKey: NewThreadBanner.key)
            }
        case "tools":
            // A turn's tool calls: a lone search, then three commands (one
            // failed), an edit, two more commands, then the answer.
            model.transcript.apply(.toolCall(id: "drive-tool-search", title: "web_search", kind: "fetch",
                                             status: .completed,
                                             input: .object(["query": .string("lhagfoss"),
                                                             "summary": .string("Finding public profiles")])))
            for (index, page) in ["https://github.com/LHagfoss", "https://docs.github.com/en/rest"].enumerated() {
                model.transcript.apply(.toolCall(id: "drive-tool-page-\(index)", title: "read_url", kind: "fetch",
                                                 status: .completed, input: nil, detail: page,
                                                 summary: "Reading the page"))
            }
            model.transcript.apply(.agentText("Found a profile; checking the build next."))
            for (index, kind) in "ttxett".enumerated() {
                let id = "drive-tool-\(index)"
                model.transcript.apply(.toolCall(id: id, title: "run_command", kind: "execute",
                                                 status: kind == "x" ? .failed : .completed, input: nil,
                                                 detail: "cargo test --package step-\(index)",
                                                 summary: "Testing step \(index)"))
                if kind == "e" {
                    model.transcript.apply(.toolCallUpdate(id: id, title: "Edit", status: .completed, output: nil,
                                                           diffs: [FileDiff(path: "/tmp/lib.rs", oldText: "let a = 1;\n", newText: "let a = 2;\n")]))
                }
            }
            model.transcript.apply(.agentText("All steps ran."))
        case "steps":
            // `steps [live]`: a long finished turn as a real session reads,
            // what the model said between runs of calls, an edit, and the
            // answer; with `live`, the turn is still running at its end.
            model.transcript.apply(.userText("Remove the tutorial banner and make unaffordable towers disabled."))
            model.transcript.apply(.thoughtText("Planning the picker and upgrade changes", summary: true))
            model.transcript.apply(.agentText("I'll remove the tutorial banner, make unaffordable tower cards clearly disabled, and adjust the startup defaults."))
            let rounds: [(String, [(String, String)])] = [
                ("I've loaded the interface guidance; I'll check its project context next.",
                 [("skill", "Load frontend UX guidance")]),
                ("The project context confirms this is a refinement of the existing game. I'll inspect the tower, upgrade, sound and hotkey code.",
                 [("view_file", "Read rules.js"), ("view_file", "Read main.jsx"), ("view_file", "Read styles.css"),
                  ("grep", "Find Tab handlers"), ("view_file", "Read audio.js")]),
                ("I've mapped the changes, including the slime tower's slow-first behavior. Editing the rules now.",
                 [("edit", "Make slime slow and add a corrosive tier")]),
                ("The slime slow is in place; I'm now making sound default on and Tab speed-only.",
                 [("view_file", "Inspect TowerCard"), ("run_command", "npm test -- rules"), ("run_command", "npm run build")]),
            ]
            var index = 0
            for (said, calls) in rounds {
                model.transcript.apply(.thoughtText("Thinking about the next step", summary: true))
                model.transcript.apply(.agentText(said))
                for (title, summary) in calls {
                    let id = "drive-step-\(index)"
                    index += 1
                    model.transcript.apply(.toolCall(id: id, title: title == "edit" ? "replace_file_content" : title,
                                                     kind: title == "edit" ? "edit" : "read",
                                                     status: index == 7 ? .failed : .completed,
                                                     input: .object(["summary": .string(summary)]),
                                                     detail: "btd6/src/main.jsx"))
                    if title == "edit" {
                        model.transcript.apply(.toolCallUpdate(id: id, title: "Edit", status: .completed, output: nil,
                                                               diffs: [FileDiff(path: "/tmp/btd6/src/rules.js",
                                                                                oldText: "slow: 0\n", newText: "slow: 0.4\n")]))
                    }
                }
            }
            if parts.contains("live") {
                let session = model.selectedSession ?? "drive"
                if model.selectedSession == nil { model.selectedSession = session }
                model.debugTurn(true, in: session)
            }
            if !parts.contains("live") {
                model.transcript.apply(.agentText("Done. Unaffordable towers are disabled with the shortfall shown, the tutorial is gone, and sound and autoplay start on."))
            }
        case "drawn":
            // Which of the older rows are drawn in full, and which stand in.
            report(ChatScrollState.current?.debugDrawn ?? "no chat")
        case "openturn":
            let folding = ToolFolding(items: model.transcript.items, verbosity: .low,
                                      open: model.openToolRuns, running: false)
            if let last = folding.turns.values.max(by: { $0.anchor < $1.anchor }) {
                // Animated, as a click on the turn's row is.
                withAnimation(.rowReveal) { model.toggleToolRun(last.key) }
            }
        case "previewshot":
            // `previewshot <url> <file.png>`: the agent's navigate then
            // screenshot in the preview panel, approved as the user would;
            // reports the capture's size, layout width and page errors.
            let session = model.selectedSession ?? "drive"
            if !model.showsPreview { model.togglePreview() }
            try? await Task.sleep(for: .milliseconds(500))
            func run(_ operation: JSONValue) async -> JSONValue {
                let task = Task { await model.preview.runAgentRequest(["sessionId": .string(session), "operation": operation]) }
                for _ in 0..<100 {
                    if model.preview.agent.pending != nil { model.preview.answerAgentApproval(allow: true); break }
                    try? await Task.sleep(for: .milliseconds(20))
                }
                return await task.value
            }
            _ = await run(["action": "navigate", "url": .string(parts[1])])
            let shot = await run(["action": "screenshot"])
            if let data = shot["image"]?["data"]?.stringValue.flatMap({ Data(base64Encoded: $0) }), parts.count > 2 {
                try? data.write(to: URL(fileURLWithPath: parts[2]))
            }
            report("previewshot ok \(shot["ok"] ?? .null) image \(shot["image"]?["width"] ?? .null)x\(shot["image"]?["height"] ?? .null) viewport \(shot["viewport"] ?? .null) errors \(shot["errors"] ?? .null) error \(shot["error"] ?? .null)")
        case "verbosity":
            // `verbosity low|high|edits`: the Settings picker's value.
            UserDefaults.standard.set(parts[1], forKey: ToolVerbosity.key)
        case "openrun":
            if let first = model.transcript.items.first(where: { if case .tool = $0.content { true } else { false } }) {
                model.toggleToolRun(first.id)
            }
        case "agents":
            // `agents [chat] [done] [open] [long]`: two subagents, the first
            // still working; `chat` adds the turn that spawned them, `done`
            // lets the first finish too, `open` shows the first one's panel
            // and reports how long it took to draw, `long` gives the first a
            // 600-message conversation.
            let flags = Set(parts.dropFirst())
            let now = Date().timeIntervalSince1970 * 1000
            if flags.contains("chat") {
                model.transcript.appendUser("Find where delegation is armed and run the Swift tests.")
                model.transcript.apply(.agentText("I'll split this between two agents."))
                for (id, task, type) in [(1, "Find where the ACP prompt arms delegation", "explorer"),
                                         (2, "Run the Swift tests", "worker")] {
                    model.transcript.apply(.toolCall(id: "spawn-\(id)", title: "spawn_agent", kind: nil, status: .pending,
                                                     input: ["task": .string(task), "agent_type": .string(type)]))
                    model.transcript.apply(.toolCallUpdate(
                        id: "spawn-\(id)", title: nil, status: .completed,
                        output: #"{"agent_id":\#(id),"nickname":"agent-\#(id)","agent_type":"\#(type)","status":"queued"}"#))
                }
                model.transcript.apply(.agentText("Both agents are on it; I'll report back when they finish."))
            }
            let first: JSONValue = flags.contains("done")
                ? ["id": 1, "name": "agent-1", "task": "Find where the ACP prompt arms delegation", "model": "gpt-6-luna",
                   "agentType": "explorer", "status": "completed", "messageCount": 3,
                   "toolCalls": 37, "requests": 12, "inputTokens": 180_000, "outputTokens": 4_200,
                   "cachedInputTokens": 120_000, "verificationCommand": "cargo test -p rustcode acp::prompt",
                   "workspaceMode": "shared", "workspacePath": "/Users/me/code/rustcode", "contextInheritance": "evidence",
                   "startedAtMs": .number(now - 42_000), "finishedAtMs": .number(now - 3_000),
                   "completion": "`acp/prompt.rs` arms it."]
                : ["id": 1, "name": "agent-1", "task": "Find where the ACP prompt arms delegation", "model": "gpt-6-luna",
                   "agentType": "explorer", "status": "running", "activeTurn": true, "messageCount": 3,
                   "toolCalls": 21, "requests": 8, "inputTokens": 96_000, "outputTokens": 2_100,
                   "cachedInputTokens": 64_000, "startedAtMs": .number(now - 42_000)]
            let agents = SubagentInfo.list(from: ["agents": [
                first,
                ["id": 2, "name": "agent-2", "task": "Run the Swift tests", "agentType": "worker", "status": "completed",
                 "writeAccess": true, "messageCount": 2, "startedAtMs": .number(now - 125_000),
                 "finishedAtMs": .number(now - 20_000), "completion": "All 214 tests pass."],
            ]])
            var updates: [JSONValue] = [
                ["sessionUpdate": "user_message_chunk", "content": ["type": "text", "text": "Find where the ACP prompt arms delegation"]],
                ["sessionUpdate": "agent_message_chunk", "content": ["type": "text", "text": "Looking at `acp/prompt.rs` and `acp/commands.rs`."]],
                ["sessionUpdate": "tool_call", "toolCallId": "t1", "title": "search_files", "status": "completed",
                 "rawInput": ["pattern": "begin_task_delegation"]],
                ["sessionUpdate": "tool_call", "toolCallId": "t2", "title": "run_command", "status": "completed",
                 "rawInput": ["command": "cargo test -p rustcode acp::prompt"]],
            ]
            if flags.contains("long") {
                for index in 0..<300 {
                    updates.append(["sessionUpdate": "tool_call", "toolCallId": .string("l\(index)"), "title": "read_file",
                                    "status": "completed", "rawInput": ["path": .string("src/file\(index).rs")]])
                    updates.append(["sessionUpdate": "agent_message_chunk", "content": ["type": "text", "text": .string(
                        "Step \(index): the parser **keeps** its `state` here.\n\n- one\n- two\n\n```rust\nlet x = \(index);\n```")]])
                }
            }
            let transcript = SubagentInfo.transcript(from: ["updates": .array(updates)])
            let start = Date()
            model.debugAgents(agents, viewing: flags.contains("open") ? transcript : nil)
            if flags.contains("open") {
                let window = NSApp.windows.first { $0.isVisible && $0.contentView != nil }
                window?.contentView?.layoutSubtreeIfNeeded()
                window?.displayIfNeeded()
                report("agent open items \(transcript.items.count) first frame \(ms(start, Date())) ms")
            }
        case "panelwheel":
            // `panelwheel <steps> [points]`: the reader's wheel over an
            // agent's panel, as the chat's input monitor sees it (#2598).
            let steps = Int(parts.count > 1 ? parts[1] : "10") ?? 10
            let points = Double(parts.count > 2 ? parts[2] : "40") ?? 40
            for _ in 0..<steps {
                ChatScrollInput.shared.debugObserve(deltaY: points)
                try? await Task.sleep(for: .milliseconds(16))
            }
        case "closeagent":
            // Back to the chat from an agent's panel, as its close button does.
            model.viewedAgent = nil
        case "usage":
            // `usage [hover]`: plan usage as a provider reports it, with a
            // 5-hour window at 82% used and a weekly one at 40%; `hover`
            // shows the meter's panel as the pointer would.
            let now = Date().timeIntervalSince1970
            model.planUsage = PlanUsage(json: [
                "source": "provider",
                "primary": ["usedPercent": 82, "windowMinutes": 300, "resetsAt": .number(now + 3 * 3600)],
                "secondary": ["usedPercent": 40, "windowMinutes": 10080, "resetsAt": .number(now + 4 * 86400)],
            ])
            withAnimation(.snappy(duration: 0.15)) { model.showsPlanUsage = parts.contains("hover") }
        case "model":
            // `model <profile>`: switch the shown chat's model, as the picker does.
            if parts.count > 1 { await model.setModel(parts.dropFirst().joined(separator: " ")) }
        case "panel":
            // `panel model|effort|add|agents|none`: open a composer panel as its chip does;
            // `effort` opens the Model & Effort picker, which holds it now.
            model.composerPanel = ["model": .model, "effort": .model, "add": .add, "agents": .agents][parts[1]]
        case "snap":
            // `snap <file.png>`: the window drawn to a file, for when
            // `screencapture` cannot see the screen.
            guard parts.count > 1, let main = NSApp.windows.first(where: { $0.isVisible && $0.contentView != nil }),
                  case let window = main.attachedSheet ?? main,
                  let view = window.contentView?.superview ?? window.contentView,
                  let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds) else { break }
            view.cacheDisplay(in: view.bounds, to: rep)
            try? rep.representation(using: .png, properties: [:])?
                .write(to: URL(fileURLWithPath: parts.dropFirst().joined(separator: " ")))
        case "snappop":
            // `snappop <file.png>`: the open popover drawn to a file.
            guard parts.count > 1,
                  let window = NSApp.windows.last(where: { $0.isVisible && String(describing: type(of: $0)).contains("Popover") }),
                  let view = window.contentView?.superview ?? window.contentView,
                  let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds) else { break }
            view.cacheDisplay(in: view.bounds, to: rep)
            try? rep.representation(using: .png, properties: [:])?
                .write(to: URL(fileURLWithPath: parts.dropFirst().joined(separator: " ")))
        case "chatfolder":
            // `chatfolder <path>`: a new chat in that folder, the default kept.
            await model.newSession(in: URL(fileURLWithPath: parts.dropFirst().joined(separator: " "), isDirectory: true))
        case "dump":
            var lines = ["items \(model.transcript.items.count) selected \(model.selectedSession ?? "-")",
                         "sessions \(model.sessions.count) visible \(model.visibleSessions.count) more \(model.hasMoreSessions)",
                         "chat \(ChatScrollState.current?.debugDescription ?? "-")"]
            func walk(_ view: NSView) {
                if let scroll = view as? NSScrollView, scroll.frame.width > 400 {
                    let doc = scroll.documentView
                    lines.append("scroll frame \(scroll.frame) doc \(doc?.frame ?? .zero) clip \(scroll.contentView.bounds) insets \(scroll.contentInsets)")
                    if let doc { lines.append("  doc subviews \(doc.subviews.count) layer sublayers \(doc.layer?.sublayers?.count ?? -1)") }
                }
                view.subviews.forEach(walk)
            }
            if let content = NSApp.windows.first?.contentView { walk(content) }
            try? lines.joined(separator: "\n").write(toFile: (ProcessInfo.processInfo.environment["RUSTCODE_DRIVE"] ?? "/tmp") + "/dump", atomically: true, encoding: .utf8)
        case "key":
            // A key press as the keyboard sends it: `key <code> [shift|cmd|opt|repeat]`;
            // `repeat` marks it as auto-repeat from a held key.
            guard let window = NSApp.windows.first(where: { $0.isVisible && $0.contentView != nil }),
                  let code = UInt16(parts[1]) else { break }
            // Page, Home, End and the arrows come with the Function flag.
            var flags: NSEvent.ModifierFlags = (115...126).contains(code) ? [.function] : []
            if parts.contains("shift") { flags.insert(.shift) }
            if parts.contains("cmd") { flags.insert(.command) }
            if parts.contains("opt") { flags.insert(.option) }
            // `return` stands for the carriage return ⏎ types.
            let named: [UInt16: String] = [36: "\r", 48: "\t", 51: "\u{7f}", 53: "\u{1b}"]
            let characters = parts.count > 2 && !["shift", "cmd", "opt", "repeat"].contains(parts[2]) ? (parts[2] == "return" ? "\r" : parts[2]) : named[code] ?? ""
            if let event = NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: flags, timestamp: ProcessInfo.processInfo.systemUptime,
                                            windowNumber: window.windowNumber, context: nil, characters: characters,
                                            charactersIgnoringModifiers: characters, isARepeat: parts.contains("repeat"), keyCode: code) {
                NSApp.postEvent(event, atStart: false)
            }
        case "mouse":
            // `mouse down|drag|move|up <x> <y>`: a left-button event at a point in
            // the window, from its top-left, as the mouse sends it.
            guard let window = NSApp.windows.first(where: { $0.isVisible && $0.contentView != nil }),
                  parts.count > 3, let x = Double(parts[2]), let y = Double(parts[3]) else { break }
            let type: NSEvent.EventType = switch parts[1] {
            case "down": .leftMouseDown
            case "drag": .leftMouseDragged
            case "move": .mouseMoved
            default: .leftMouseUp
            }
            let point = NSPoint(x: x, y: (window.contentView?.frame.height ?? 0) - y)
            if let event = NSEvent.mouseEvent(with: type, location: point, modifierFlags: [], timestamp: ProcessInfo.processInfo.systemUptime,
                                              windowNumber: window.windowNumber, context: nil, eventNumber: 0, clickCount: 1,
                                              pressure: type == .leftMouseUp ? 0 : 1) {
                NSApp.postEvent(event, atStart: false)
            }
        case "dragselect":
            // `dragselect <x1> <y1> <x2> <y2>`: a press, a drag in steps and a
            // release, from the window's top left. Reports what holds the
            // keyboard and the selection.
            guard let window = NSApp.windows.first(where: { $0.isVisible && $0.contentView != nil }),
                  parts.count > 4, let x1 = Double(parts[1]), let y1 = Double(parts[2]),
                  let x2 = Double(parts[3]), let y2 = Double(parts[4]) else { break }
            // From the window's top left, as `hit` counts.
            guard let root = window.contentView?.superview else { break }
            let height = root.bounds.height
            let steps = 6
            func event(_ type: NSEvent.EventType, _ x: Double, _ y: Double) -> NSEvent? {
                NSEvent.mouseEvent(with: type, location: NSPoint(x: x, y: height - y), modifierFlags: [],
                                   timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: window.windowNumber,
                                   context: nil, eventNumber: 0, clickCount: 1, pressure: type == .leftMouseUp ? 0 : 1)
            }
            var rest: [NSEvent] = []
            for step in 1...steps {
                let t = Double(step) / Double(steps)
                event(.leftMouseDragged, x1 + (x2 - x1) * t, y1 + (y2 - y1) * t).map { rest.append($0) }
            }
            event(.leftMouseUp, x2, y2).map { rest.append($0) }
            guard let down = event(.leftMouseDown, x1, y1) else { break }
            // A press on text goes to it directly, its tracking loop then
            // taking the queued drag: a window that is not key would take a
            // posted press only as the click that activates it. Elsewhere the
            // press is posted, so the chat's mouse monitor sees it.
            let hit = root.hitTest(down.locationInWindow)
            if let text = hit as? NSTextView {
                rest.forEach { NSApp.postEvent($0, atStart: false) }
                window.makeFirstResponder(text)
                text.mouseDown(with: down)
            } else {
                ([down] + rest).forEach { NSApp.postEvent($0, atStart: false) }
            }
            try? await Task.sleep(for: .milliseconds(400))
            report("dragselect " + DebugDriver.selectionReport(window))
        case "copysel":
            // ⌘C to what holds the keyboard, then what the pasteboard holds.
            NSPasteboard.general.clearContents()
            let window = NSApp.windows.first { $0.isVisible && $0.contentView != nil }
            _ = window?.firstResponder?.tryToPerform(#selector(NSText.copy(_:)), with: nil)
            report("copied \(String(reflecting: NSPasteboard.general.string(forType: .string) ?? ""))")
        case "find":
            // `find on|off`: the find bar, focused, for `type` to fill.
            model.showsFind = parts.count < 2 || parts[1] != "off"
        case "selection":
            guard let window = NSApp.windows.first(where: { $0.isVisible && $0.contentView != nil }) else { break }
            report("selection " + DebugDriver.selectionReport(window))
        case "escape":
            // Esc as the chat's key monitor sees it.
            guard let window = NSApp.windows.first(where: { $0.isVisible && $0.contentView != nil }),
                  let event = NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [], timestamp: ProcessInfo.processInfo.systemUptime,
                                               windowNumber: window.windowNumber, context: nil, characters: "\u{1b}",
                                               charactersIgnoringModifiers: "\u{1b}", isARepeat: false,
                                               keyCode: KeyRecorderInput.escapeKeyCode) else { break }
            NSApp.postEvent(event, atStart: false)
        case "responder":
            // What has the keyboard, and what the window hits at a point.
            guard let window = NSApp.windows.first(where: { $0.isVisible && $0.contentView != nil }) else { break }
            let cursors: [(NSCursor, String)] = [(.pointingHand, "hand"), (.iBeam, "ibeam"), (.arrow, "arrow")]
            let cursor = cursors.first { $0.0 == NSCursor.current }?.1 ?? "other"
            var lines = ["first \(window.firstResponder.map { String(describing: type(of: $0)) } ?? "-") cursor \(cursor)"]
            if parts.count > 2, let x = Double(parts[1]), let y = Double(parts[2]), let content = window.contentView {
                let hit = content.hitTest(NSPoint(x: x, y: content.frame.height - y))
                var chain: [String] = []
                var view = hit
                while let current = view, chain.count < 8 { chain.append(String(describing: type(of: current))); view = current.superview }
                lines.append("hit " + chain.joined(separator: " < "))
            }
            try? lines.joined(separator: "\n").write(toFile: (ProcessInfo.processInfo.environment["RUSTCODE_DRIVE"] ?? "/tmp") + "/responder", atomically: true, encoding: .utf8)
        case "sidebarend":
            // Scroll the sidebar's chat list to its end, as a reader would.
            func walk(_ view: NSView) -> NSScrollView? {
                if let scroll = view as? NSScrollView, scroll.frame.width < 300, scroll.frame.height > 200 { return scroll }
                return view.subviews.lazy.compactMap(walk).first
            }
            if let scroll = NSApp.windows.first?.contentView.flatMap(walk), let document = scroll.documentView {
                let y = document.isFlipped ? document.frame.height - scroll.contentView.bounds.height : 0
                scroll.contentView.scroll(to: NSPoint(x: 0, y: y))
                scroll.reflectScrolledClipView(scroll.contentView)
            }
        case "type":
            // Text typed into whatever has focus, a key press per character.
            guard let window = NSApp.windows.first(where: { $0.isVisible && $0.contentView != nil }) else { break }
            for character in line.dropFirst(5) {
                if let event = NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [], timestamp: ProcessInfo.processInfo.systemUptime,
                                                windowNumber: window.windowNumber, context: nil, characters: String(character),
                                                charactersIgnoringModifiers: String(character), isARepeat: false, keyCode: 0) {
                    NSApp.postEvent(event, atStart: false)
                }
            }
        case "mark":
            // `mark <text>`: text left composing in the focused field, as a
            // dead key such as ` on a Norwegian keyboard leaves it.
            guard let editor = NSApp.windows.first(where: { $0.isVisible && $0.contentView != nil })?.firstResponder as? NSTextView else { break }
            let text = String(line.dropFirst(5))
            editor.setMarkedText(text, selectedRange: NSRange(location: text.utf16.count, length: 0),
                                 replacementRange: NSRange(location: NSNotFound, length: 0))
        case "menus":
            // Every menu item with a key equivalent, to `<dir>/menus`.
            var lines: [String] = []
            func walk(_ menu: NSMenu, _ path: String) {
                for item in menu.items {
                    if !item.keyEquivalent.isEmpty || item.title.contains("Pin") {
                        lines.append("\(path)/\(item.title) [\(item.keyEquivalent)] mods \(item.keyEquivalentModifierMask.rawValue) enabled \(item.isEnabled) action \(item.action.map(NSStringFromSelector) ?? "-")")
                    }
                    if let submenu = item.submenu { walk(submenu, path + "/" + item.title) }
                }
            }
            if let menu = NSApp.mainMenu { walk(menu, "") }
            try? lines.joined(separator: "\n").write(toFile: (ProcessInfo.processInfo.environment["RUSTCODE_DRIVE"] ?? "/tmp") + "/menus",
                                                     atomically: true, encoding: .utf8)
        case "draft":
            // `draft <text>`: the composer's text, with `\n` for line breaks.
            model.draft = String(line.dropFirst(6)).replacingOccurrences(of: "\\n", with: "\n")
        case "submit":
            // Send the composer's text as ⏎ does, through the engine.
            await model.submit()
        case "cleardraft":
            // What sending does to the composer, without sending anything.
            model.draft = ""
        case "state":
            // The draft, the field and the selection, to `<dir>/state`.
            let window = NSApp.windows.first { $0.isVisible && $0.contentView != nil }
            let editor = window?.firstResponder as? NSTextView
            let lines = ["draft [\(model.draft)]", "field [\(editor?.string ?? "-")] marked \(editor?.hasMarkedText() ?? false)",
                         "selected \(model.selectedSession ?? "-") pinned \(model.isSelectedPinned)",
                         "toasts \(model.toasts.map(\.title))", "key \(window?.isKeyWindow ?? false) responder \(String(describing: window?.firstResponder.map { type(of: $0) }))",
                         "terminal \(model.terminal.isShown) tabs \(model.terminal.manager.projectSessions.count)",
                         "predictions \(editor.map { $0.inlinePredictionType == .yes ? "yes" : "default" } ?? "-")"]
            try? lines.joined(separator: "\n").write(toFile: (ProcessInfo.processInfo.environment["RUSTCODE_DRIVE"] ?? "/tmp") + "/state",
                                                     atomically: true, encoding: .utf8)
        case "scope": model.showsAllProjects = parts[1] == "all"
        case "keyequiv":
            // `keyequiv <code> <character>`: a ⌘-shortcut handed straight to
            // the window, which `key` cannot reach while the app is in the
            // background and its window not key.
            guard parts.count > 2, let code = UInt16(parts[1]),
                  let window = NSApp.windows.first(where: { $0.isVisible && $0.contentView != nil }),
                  let event = NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [.command], timestamp: ProcessInfo.processInfo.systemUptime,
                                               windowNumber: window.windowNumber, context: nil, characters: parts[2],
                                               charactersIgnoringModifiers: parts[2], isARepeat: false, keyCode: code) else { break }
            report("keyequiv \(parts[2]) handled \(window.performKeyEquivalent(with: event))")
        case "appearance":
            // `appearance light|dark|system`: for this run only, never saved.
            let names = ["light": NSAppearance.Name.aqua, "dark": .darkAqua]
            NSApp.appearance = parts.count > 1 ? names[parts[1]].flatMap(NSAppearance.init(named:)) : nil
        case "settings":
            // `settings [section]|close`
            if parts.count > 1, parts[1] == "close" {
                model.closeSettings()
            } else {
                model.openSettings(parts.count > 1 ? SettingsSection(rawValue: parts.dropFirst().joined(separator: " ")) : nil)
            }
        case "keybind":
            // `keybind <action> <key> [cmd|opt|shift|ctrl]...`
            guard parts.count > 2, let action = KeybindingAction(rawValue: parts[1]) else { break }
            var modifiers: KeyModifiers = []
            for part in parts.dropFirst(3) {
                switch part {
                case "cmd": modifiers.insert(.command)
                case "opt": modifiers.insert(.option)
                case "shift": modifiers.insert(.shift)
                case "ctrl": modifiers.insert(.control)
                default: break
                }
            }
            _ = model.keybindings.assign(KeyCombo(key: parts[2], modifiers: modifiers), to: action)
        case "snap":
            // `snap <name>`: the window drawn into `<dir>/<name>.png`, for
            // when screencapture cannot see the screen.
            guard parts.count > 1, let view = NSApp.windows.first(where: { $0.isVisible && $0.contentView != nil })?.contentView,
                  let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds) else { break }
            view.cacheDisplay(in: view.bounds, to: rep)
            let path = (ProcessInfo.processInfo.environment["RUSTCODE_DRIVE"] ?? "/tmp") + "/\(parts[1]).png"
            try? rep.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: path))
        case "state":
            // Where the keyboard is and what the panels show, to `<dir>/state`.
            let windows = NSApp.windows.filter { $0.isVisible && $0.contentView != nil }
            let responder = windows.first?.firstResponder
            let field = (responder as? NSTextView)?.delegate as? NSTextField
            var menuItems: [String] = []
            func walkMenu(_ menu: NSMenu) {
                for item in menu.items {
                    if !item.keyEquivalent.isEmpty, item.keyEquivalentModifierMask.contains(.command), "wk".contains(item.keyEquivalent) {
                        menuItems.append("\(item.title)=\(item.keyEquivalent) enabled \(item.isEnabled)")
                    }
                    if let submenu = item.submenu { walkMenu(submenu) }
                }
            }
            if let main = NSApp.mainMenu { walkMenu(main) }
            let lines = ["menu \(menuItems.joined(separator: ", "))",
                         "windows \(windows.count)",
                         "responder \(responder.map { String(describing: type(of: $0)) } ?? "-")",
                         "field \(field?.placeholderString ?? "-") value \(field?.stringValue ?? "-")",
                         "sessionFilter \(model.sessionFilter)",
                         "switcher \(model.showsSwitcher) all \(model.showsAllProjects)",
                         "terminal \(model.terminal.isShown)",
                         "settings \(model.showsSettings)",
                         "closeTerminal \(model.keybindings.display(for: .closeTerminal)) newChat \(model.keybindings.display(for: .newChat)) undo \(model.keybindings.canUndo)"]
            try? lines.joined(separator: "\n").write(toFile: (ProcessInfo.processInfo.environment["RUSTCODE_DRIVE"] ?? "/tmp") + "/state",
                                                     atomically: true, encoding: .utf8)
        case "terminal": model.terminal.open()
        case "terminaltab": model.terminal.newSession()
        case "focussearch": model.focusSessionSearch = true
        case "click":
            // `click <x> <y>`: a left click at a point measured from the
            // window's top-left, as a `snap` image shows it.
            guard parts.count > 2, let x = Double(parts[1]), let y = Double(parts[2]),
                  let window = NSApp.windows.first(where: { $0.isVisible && $0.contentView != nil }) else { break }
            let point = NSPoint(x: x, y: window.frame.height - y)
            for type in [NSEvent.EventType.leftMouseDown, .leftMouseUp] {
                if let event = NSEvent.mouseEvent(with: type, location: point, modifierFlags: [], timestamp: ProcessInfo.processInfo.systemUptime,
                                                  windowNumber: window.windowNumber, context: nil, eventNumber: 0, clickCount: 1, pressure: 1) {
                    NSApp.postEvent(event, atStart: false)
                }
            }
        case "filter":
            model.sessionFilter = parts.dropFirst().joined(separator: " ")
        case "completion":
            // `completion [ship]`: a finished turn that edited a file and ran
            // checks (one passed, one failed, one never reported), for its
            // card; `ship` also commits, pushes, opens a pull request and
            // watches its checks, which are still pending.
            let start = Date().addingTimeInterval(-42)
            model.transcript.appendUser("Fix the rules and run the checks.")
            model.transcript.beginTurn(at: start)
            model.transcript.apply(.toolCall(id: "drive-c-edit", title: "edit_file", kind: "edit", status: .inProgress, input: nil))
            model.transcript.apply(.toolCallUpdate(id: "drive-c-edit", title: nil, status: .completed, output: "ok",
                                                   diffs: [FileDiff(path: "/tmp/btd6/src/rules.js", oldText: "a\n", newText: "b\nc\n")]))
            for (index, (command, status)) in [("npm test -- rules", ToolStatus.completed), ("npm run lint", .failed),
                                               ("npm run build", nil)].enumerated() {
                model.transcript.apply(.toolCall(id: "drive-c-\(index)", title: "run_command", kind: "execute",
                                                 status: .inProgress, input: nil, detail: command))
                if let status {
                    model.transcript.apply(.toolCallUpdate(id: "drive-c-\(index)", title: nil, status: status, output: "done"))
                }
            }
            if parts.contains("ship") {
                for (index, (command, status, exit, output)) in [
                    ("git add -A && git commit -m 'fix(rules): allow towers'", ToolStatus.completed, 0, "1 file changed"),
                    ("git push -u origin HEAD", .completed, 0, ""),
                    ("gh pr create --fill", .completed, 0, "https://github.com/LHagfoss/btd6/pull/42"),
                    ("gh pr checks 42 --watch", .failed, 8, "some checks are still pending"),
                ].enumerated() {
                    model.transcript.apply(.toolCall(id: "drive-s-\(index)", title: "run_command", kind: "execute",
                                                     status: .inProgress, input: nil, detail: command))
                    model.transcript.apply(.toolCallUpdate(id: "drive-s-\(index)", title: nil, status: status,
                                                           output: output, exitCode: exit))
                }
            }
            model.transcript.apply(.agentText("Fixed the rules; the lint still fails on an unused import."))
            model.transcript.finishTurn(at: Date())
        case "pick":
            // `pick`: the Changes panel's line picker on its first file.
            model.debugPickFirstFile = true
        case "reference":
            // `reference`: a picked diff line attached to the composer.
            model.attachCodeReference(CodeReference(intent: .change, root: model.chatProject.path, path: "src/parser.rs",
                                                    side: .new, lines: 40...44, scope: "uncommitted changes",
                                                    baseCommit: "a1b2c3d", excerpt: ["fn parse() {", "    todo!()", "}"]))
        case "plan":
            // `plan [open] [go]`: a turn that planned five steps (one done, one in
            // progress, one blocked) and the plan the engine would answer;
            // `open` shows the inspector.
            let steps: [JSONValue] = [
                ["id": "t1", "content": "Read how the parser handles nested blocks", "status": "completed"],
                ["id": "t2", "content": "Fix nesting in fenced code", "status": "in_progress"],
                ["id": "t3", "content": "Add a fixture with deep nesting", "status": "blocked",
                 "blockedReason": "Needs a sample document from the user"],
                ["id": "t4", "content": "Run the parser tests", "status": "pending"],
                ["id": "t5", "content": "Update the changelog", "status": "pending"],
            ]
            if model.transcript.isEmpty {
                model.transcript.appendUser("Fix nested code blocks in the parser.")
                model.transcript.apply(.toolCall(id: "plan-1", title: "todo_write", kind: nil, status: .completed,
                                                 input: ["todos": .array(steps)]))
                model.transcript.apply(.agentText("Reading the parser first."))
                model.transcript.apply(.toolCall(id: "plan-2", title: "todo_write", kind: nil, status: .completed,
                                                 input: ["merge": true, "todos": [["id": "t2", "status": "in_progress"]]]))
            }
            let plan = TaskPlan(result: ["steps": .array(steps)])
            model.debugPlan(plan)
            model.showsTaskPlan = parts.contains("open")
            // `go`: choose the first step, as a click on its row does.
            if parts.contains("go") { model.revealRequest = plan.lastUpdates(in: model.transcript.items)["t1"] }
        case "inbox":
            // `inbox [demo]`: open the attention inbox; `demo` first marks the
            // first thread stopped and the second unread.
            if parts.count > 1, parts[1] == "demo" {
                let ids = model.sessions.map(\.id)
                model.debugAttention(stopped: ids.first, unseen: ids.dropFirst().first)
            }
            model.showsInbox = true
        case "changes":
            // `changes`: show or hide the Changes panel.
            model.toggleChanges()
        case "size":
            NSApp.windows.first?.setFrame(NSRect(x: 80, y: 80, width: Double(parts[1])!, height: Double(parts[2])!), display: true)
        default: break
        }
    }
}
#endif

#if DEBUG
extension DebugDriver {
    /// `big markdown`: the tables and fences replies hold (#2432).
    /// Every way a reply writes a link (#2413).
    static let linksSample = """
    A [named link](https://example.com/docs) in a sentence, and a bare one: https://github.com/LHagfoss/rustcode/issues/2413.

    ## A heading with [a link](https://example.com/heading)

    - A list item with **[a bold link](https://example.com/bold)** and *[an italic one](https://example.com/italic)*.
    - [`code in a link`](https://example.com/code) and `https://not.a.link/in/code`.

    > A quote with [a link](https://example.com/quote).

    | Site | Address |
    |---|---|
    | Docs | [docs](https://example.com/table) |
    | Bare | https://example.com/bare |
    """

    /// Short, long, multi-paragraph and linked `>` quotes (#2525).
    static let quoteSample = """
    A short quote:

    > Ship it.

    A handover prompt:

    > You are taking over the **release** work in RustCode. Read [the release skill](https://github.com/LHagfoss/rustcode) first, then check `scripts/release.sh` for the steps. Every merged PR needs its `## Changelog` line filled in.
    > Keep the checkout clean.
    >
    > When you are done, report the version and the tag.

    And text after it.
    """

    /// What holds the keyboard, the text it has selected, and the chat's
    /// prose selection in transcript terms.
    static func selectionReport(_ window: NSWindow) -> String {
        let responder = window.firstResponder.map { String(describing: type(of: $0)) } ?? "-"
        var selected = ""
        if let text = window.firstResponder as? NSTextView {
            selected = (text.string as NSString).substring(with: text.selectedRange())
        }
        let model = ProseSelection.shared.selection.bounds.map {
            "\($0.start.run.item).\($0.start.run.segment)@\($0.start.offset)-\($0.end.run.item).\($0.end.run.segment)@\($0.end.offset)"
        } ?? "none"
        return "first \(responder) selected \(String(reflecting: selected)) model \(model)"
    }

    static let markdownSample = """
    Here is the comparison:

    | Option | Speed | Notes |
    |:--|:-:|--:|
    | `ripgrep` | fast | Skips `.gitignore`d files; the pipe in `a|b` stays in its cell |
    | grep | slow | — |
    | ast-grep | medium | Structural search over the syntax tree, with rewrites and lint rules for many languages |

    A wide one:

    | Crate | Version | License | Downloads | Last release | Maintainers | MSRV | Features |
    |---|---|---|---|---|---|---|---|
    | serde | 1.0.210 | MIT/Apache-2.0 | 400M | 2026-09-01 | dtolnay | 1.61 | derive, std, alloc |
    | tokio | 1.40.0 | MIT | 250M | 2026-08-20 | tokio-rs | 1.70 | full, rt, macros |

    Steps:

    1. Install it:

       ```sh
       brew install ripgrep
       rg --files | head
       ```

    2. Then run:
       ```rust
       fn main() {
           println!("{}", "a|b"); // a pipe in code
       }
       ```

    ```python
    def total(xs: list[int]) -> int:
        return sum(x for x in xs if x > 0)  # a long line that runs on past the width of the chat so it has to scroll sideways
    ```

    ~~~json
    {"a": 1, "b": [true, null]}
    ~~~
    """
}
#endif
