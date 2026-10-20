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

    private static func run(_ line: String, _ model: DesktopModel) async {
        let parts = line.split(separator: " ").map(String.init)
        switch parts.first {
        case "select": if let s = session(parts[1], model) { await model.select(s) }
        case "open":
            if let s = model.sessions.first(where: { $0.id == parts[1] }) { await model.select(s) }
        case "pin": if let s = session(parts[1], model) { model.togglePin(s) }
        case "new": await model.newSession()
        case "dump":
            var lines = ["items \(model.transcript.items.count) selected \(model.selectedSession ?? "-")",
                         "sessions \(model.sessions.count) visible \(model.visibleSessions.count) more \(model.hasMoreSessions)"]
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
            var flags: NSEvent.ModifierFlags = [.function]
            if parts.contains("shift") { flags.insert(.shift) }
            if parts.contains("cmd") { flags.insert(.command) }
            if parts.contains("opt") { flags.insert(.option) }
            let characters = parts.count > 2 && !["shift", "cmd", "opt"].contains(parts[2]) ? parts[2] : ""
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
        case "scope": model.showsAllProjects = parts[1] == "all"
        case "filter":
            model.sessionFilter = parts.dropFirst().joined(separator: " ")
        case "size":
            NSApp.windows.first?.setFrame(NSRect(x: 80, y: 80, width: Double(parts[1])!, height: Double(parts[2])!), display: true)
        default: break
        }
    }
}
#endif
