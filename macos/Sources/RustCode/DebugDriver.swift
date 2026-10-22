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
            // `synth <items>`: a long chat of mixed messages, shown at once.
            let count = Int(parts[1]) ?? 3000
            var transcript = Transcript()
            for index in 0..<count {
                switch index % 4 {
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
        case "timescroll":
            // `timescroll <steps> <points>`: scroll the chat up step by step,
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
            // `stream <chunks>`: a reply streamed into the shown chat at 30 ms
            // a chunk; the time beyond that is what drawing it cost.
            let chunks = Int(parts[1]) ?? 100
            let start = Date()
            for index in 0..<chunks {
                model.transcript.apply(.agentText(index % 12 == 0 ? "\n\nNext paragraph with `code` and **bold** " : "word "))
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
        case "send":
            // `send <text>`: a follow-up as the composer sends it, starting a
            // turn in the shown chat without an engine.
            let session = model.selectedSession ?? "drive"
            if model.selectedSession == nil { model.selectedSession = session }
            model.draft = ""
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
            worstGap = 0
            for _ in 0..<steps {
                if !parts.contains("nowheel") { wheel(points) }
                try? await Task.sleep(for: .milliseconds(16))
            }
            let wheeledY = scroll.contentView.bounds.minY
            let wheelGap = worstGap
            try? await Task.sleep(for: .milliseconds(1500))
            let restY = scroll.contentView.bounds.minY
            streaming.cancel()
            ticker.invalidate()
            report("wheelstream \(steps)x\(points): moved \(Int(startY - wheeledY)) of \(Int(steps) * Int(points)), drift at rest \(Int(restY - wheeledY)), worst stall wheeling \(Int(wheelGap)) ms, overall \(Int(worstGap)) ms")
        case "scrollby":
            // `scrollby <points> [steps]`: move the chat, negative up, a step a frame.
            let steps = Int(parts.count > 2 ? parts[2] : "1") ?? 1
            for _ in 0..<steps {
                ChatScrollState.current?.debugLetGo()
                ChatScrollState.current?.scroll(by: Double(parts[1]) ?? -800, animated: false)
                try? await Task.sleep(for: .milliseconds(16))
            }
        case "big":
            // `big <prose|code|line|tool> <size> [chunks]`: a large reply
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
        case "agents":
            // `agents [open]`: two subagents above the composer; `open` shows
            // the first one's panel.
            let now = Date().timeIntervalSince1970 * 1000
            let agents = SubagentInfo.list(from: ["agents": [
                ["id": 1, "name": "agent-1", "task": "Find where the ACP prompt arms delegation", "model": "gpt-6-luna",
                 "agentType": "explorer", "status": "running", "activeTurn": true, "messageCount": 3,
                 "startedAtMs": .number(now - 42_000)],
                ["id": 2, "name": "agent-2", "task": "Run the Swift tests", "agentType": "worker", "status": "completed",
                 "writeAccess": true, "messageCount": 2, "startedAtMs": .number(now - 125_000),
                 "finishedAtMs": .number(now - 20_000), "completion": "All 214 tests pass."],
            ]])
            let transcript = SubagentInfo.transcript(from: ["updates": [
                ["sessionUpdate": "user_message_chunk", "content": ["type": "text", "text": "Find where the ACP prompt arms delegation"]],
                ["sessionUpdate": "agent_message_chunk", "content": ["type": "text", "text": "Looking at `acp/prompt.rs` and `acp/commands.rs`."]],
                ["sessionUpdate": "tool_call", "toolCallId": "t1", "title": "search_files", "status": "completed",
                 "rawInput": ["pattern": "begin_task_delegation"]],
            ]])
            model.debugAgents(agents, viewing: parts.count > 1 && parts[1] == "open" ? transcript : nil)
        case "panel":
            // `panel model|effort|add|none`: open a composer panel as its chip does.
            model.composerPanel = ["model": .model, "effort": .effort, "add": .add][parts[1]]
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
            // A key press as the keyboard sends it: `key <code> [shift|cmd|opt]`.
            guard let window = NSApp.windows.first(where: { $0.isVisible && $0.contentView != nil }),
                  let code = UInt16(parts[1]) else { break }
            // Page, Home, End and the arrows come with the Function flag.
            var flags: NSEvent.ModifierFlags = (115...126).contains(code) ? [.function] : []
            if parts.contains("shift") { flags.insert(.shift) }
            if parts.contains("cmd") { flags.insert(.command) }
            if parts.contains("opt") { flags.insert(.option) }
            // `return` stands for the carriage return ⏎ types.
            let named: [UInt16: String] = [36: "\r", 48: "\t", 51: "\u{7f}", 53: "\u{1b}"]
            let characters = parts.count > 2 && !["shift", "cmd", "opt"].contains(parts[2]) ? (parts[2] == "return" ? "\r" : parts[2]) : named[code] ?? ""
            if let event = NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: flags, timestamp: ProcessInfo.processInfo.systemUptime,
                                            windowNumber: window.windowNumber, context: nil, characters: characters,
                                            charactersIgnoringModifiers: characters, isARepeat: false, keyCode: code) {
                NSApp.postEvent(event, atStart: false)
            }
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
        case "cleardraft":
            // What sending does to the composer, without sending anything.
            model.draft = ""
        case "state":
            // The draft, the field and the selection, to `<dir>/state`.
            let window = NSApp.windows.first { $0.isVisible && $0.contentView != nil }
            let editor = window?.firstResponder as? NSTextView
            let lines = ["draft [\(model.draft)]", "field [\(editor?.string ?? "-")] marked \(editor?.hasMarkedText() ?? false)",
                         "selected \(model.selectedSession ?? "-") pinned \(model.isSelectedPinned)",
                         "toasts \(model.toasts.map(\.title))", "key \(window?.isKeyWindow ?? false) responder \(String(describing: window?.firstResponder.map { type(of: $0) }))"]
            try? lines.joined(separator: "\n").write(toFile: (ProcessInfo.processInfo.environment["RUSTCODE_DRIVE"] ?? "/tmp") + "/state",
                                                     atomically: true, encoding: .utf8)
        case "scope": model.showsAllProjects = parts[1] == "all"
        case "settings":
            if parts.count > 1, let section = SettingsSection(rawValue: parts.dropFirst().joined(separator: " ")) {
                model.settingsSection = section
            }
            model.showsSettings = true
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
        case "size":
            NSApp.windows.first?.setFrame(NSRect(x: 80, y: 80, width: Double(parts[1])!, height: Double(parts[2])!), display: true)
        default: break
        }
    }
}
#endif
