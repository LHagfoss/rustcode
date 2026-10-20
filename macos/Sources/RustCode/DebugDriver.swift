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
        case "pin": if let s = session(parts[1], model) { model.togglePin(s) }
        case "new": await model.newSession()
        case "dump":
            var lines = ["items \(model.transcript.items.count) selected \(model.selectedSession ?? "-")"]
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
        case "size":
            NSApp.windows.first?.setFrame(NSRect(x: 80, y: 80, width: Double(parts[1])!, height: Double(parts[2])!), display: true)
        default: break
        }
    }
}
#endif
