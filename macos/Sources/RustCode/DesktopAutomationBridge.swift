import AppKit
import Darwin
import Foundation
import RustCodeKit

/// Owns desktop access in the UI process. Every action requires a fresh user
/// approval; refs are scoped to the chat and app that produced them.
@MainActor
final class DesktopAutomationBridge {
    private var ledger = DesktopRefLedger()
    private var namespaces: [String: String] = [:]
    private var generations: [String: Int] = [:]
    private var busy = false
    private var pendingSheet: (session: String, window: NSWindow, sheet: NSWindow)?

    func revoke(session: String) {
        generations[session, default: 0] += 1
        ledger.clear(session: session)
        namespaces.removeValue(forKey: session)
        if let pending = pendingSheet, pending.session == session {
            pending.window.endSheet(pending.sheet, returnCode: .cancel)
            pendingSheet = nil
        }
    }

    func reset() {
        if let pending = pendingSheet {
            pending.window.endSheet(pending.sheet, returnCode: .cancel)
            pendingSheet = nil
        }
        ledger.clear()
        namespaces.removeAll()
        for session in generations.keys { generations[session, default: 0] += 1 }
    }

    func ticket(for session: String) -> Int {
        let generation = generations[session, default: 0]
        generations[session] = generation
        return generation
    }

    func run(_ params: JSONValue, chatTitle: String, ticket: Int, active: @escaping @MainActor () -> Bool) async -> JSONValue {
        func error(_ message: String) -> JSONValue { ["ok": false, "error": ["message": .string(message)]] }
        guard DesktopAutomation.isEnabled else { return error("Desktop control is off in Settings.") }
        guard !busy else { return error("Another desktop operation is awaiting approval or running.") }
        guard let session = params["sessionId"]?.stringValue, !session.isEmpty,
              let operation = params["operation"], let action = operation["action"]?.stringValue,
              let app = operation["app"]?.stringValue, !app.isEmpty,
              ["snapshot", "click", "type"].contains(action) else { return error("Invalid desktop request.") }
        let ref = operation["ref"]?.stringValue
        if action != "snapshot" {
            guard let ref, ledger.contains(session: session, app: app, ref: ref) else {
                return error("That ref was not issued by the latest snapshot for this chat and app.")
            }
        }
        let environment = EngineLocator(userSetting: nil).engineEnvironment()
        let candidates = (environment["PATH"] ?? "").split(separator: ":").map { String($0) + "/agent-desktop" }
        guard let executable = candidates.first(where: EngineLocator.isExecutableFile) else {
            return error("agent-desktop was not found. Install it as described in Settings → General → Desktop automation.")
        }
        guard active(), generations[session, default: 0] == ticket else { return error("The requesting turn is no longer running.") }
        busy = true
        defer { busy = false }
        let generation = ticket
        let started = Date()
        let alert = NSAlert()
        alert.messageText = "Allow desktop access?"
        let target = ref.flatMap { ledger.describe(session: session, app: app, ref: $0) }
        let detail = (target.map { "\nTarget: \($0) (as last observed; UI may have changed)" } ?? "")
            + (action == "type" ? "\nText: \(operation["value"]?.stringValue ?? "")" : "")
        alert.informativeText = "\(chatTitle) wants to \(action) in \(app).\(detail)\nThis may read or change private app content. This approval applies only to this action."
        alert.addButton(withTitle: "Allow Once")
        alert.addButton(withTitle: "Deny")
        guard let window = NSApp.windows.first(where: { $0.isVisible && $0.canBecomeKey }) else { return error("No window is available to request permission.") }
        let response = await withCheckedContinuation { continuation in
            alert.beginSheetModal(for: window) { [weak self] response in
                self?.pendingSheet = nil
                continuation.resume(returning: response)
            }
            pendingSheet = (session, window, alert.window)
            DispatchQueue.main.asyncAfter(deadline: .now() + 120) { [weak alert] in
                guard let sheet = alert?.window, sheet.sheetParent != nil else { return }
                window.endSheet(sheet, returnCode: .cancel)
            }
        }
        guard response == .alertFirstButtonReturn else { return error("The user denied desktop access.") }
        guard Date().timeIntervalSince(started) < 120, DesktopAutomation.isEnabled,
              generations[session, default: 0] == generation, active() else { return error("The desktop request expired or was cancelled; nothing ran.") }
        let namespace = namespaces[session] ?? UUID().uuidString
        namespaces[session] = namespace
        if action == "snapshot" { ledger.clear(session: session, app: app) }
        var arguments = ["--session", namespace, action]
        switch action {
        case "snapshot":
            arguments += ["--app", app, "-i", "--compact"]
            if let windowID = operation["window_id"]?.stringValue { arguments += ["--window-id", windowID] }
        case "click": arguments += [ref!]
        case "type":
            guard let value = operation["value"]?.stringValue else { return error("Missing text.") }
            arguments += [ref!, value]
        default: return error("Unsupported action.")
        }
        let result = await Task.detached { Self.invoke(executable, arguments, environment: environment) }.value
        guard generations[session, default: 0] == generation, active() else { return error("The turn ended while the desktop operation was running; its outcome is unknown. An already dispatched action may still complete.") }
        guard let data = result, data.count <= 2_000_000,
              let envelope = try? JSONDecoder().decode(JSONValue.self, from: data) else {
            ledger.clear(session: session, app: app)
            return error("agent-desktop failed or timed out; the outcome is unknown and an already dispatched action may still complete. Inspect the app before attempting another action.")
        }
        if action == "snapshot", envelope["ok"]?.boolValue == true {
            guard envelope["data"]?["app"]?.stringValue == app, let tree = envelope["data"]?["tree"] else {
                return error("The snapshot did not identify the requested app; no refs were authorized.")
            }
            ledger.record(session: session, app: app, tree: tree, window: envelope["data"]?["window"]?["title"]?.stringValue ?? "")
        }
        if envelope["ok"]?.boolValue != true,
           envelope["error"]?["disposition"]?["retry"]?.stringValue != "safe" {
            ledger.clear(session: session, app: app)
        }
        return envelope
    }

    nonisolated static func invoke(_ executable: String, _ arguments: [String], environment: [String: String], timeout: TimeInterval = 20) -> Data? {
        let process = Process()
        let output = Pipe()
        process.executableURL = URL(fileURLWithPath: executable)
        process.arguments = arguments
        process.environment = environment
        process.standardOutput = output
        process.standardError = FileHandle.nullDevice
        do { try process.run() } catch { return nil }
        let pid = process.processIdentifier
        let fd = output.fileHandleForReading.fileDescriptor
        let deadline = ProcessInfo.processInfo.systemUptime + timeout
        var data = Data()
        var complete = false
        var buffer = [UInt8](repeating: 0, count: 65536)
        while ProcessInfo.processInfo.systemUptime < deadline {
            var descriptor = pollfd(fd: fd, events: Int16(POLLIN | POLLHUP), revents: 0)
            let ready = poll(&descriptor, 1, 100)
            if ready < 0 { break }
            if ready == 0 { continue }
            let count = Darwin.read(fd, &buffer, buffer.count)
            if count == 0 { complete = true; break }
            if count < 0 { break }
            data.append(contentsOf: buffer.prefix(count))
            if data.count > 2_000_000 { break }
        }
        if !complete && process.isRunning { _ = kill(pid, SIGKILL) }
        guard complete else { return nil }
        // EOF can precede exit; never block the caller indefinitely on wait.
        while process.isRunning && ProcessInfo.processInfo.systemUptime < deadline {
            Thread.sleep(forTimeInterval: 0.01)
        }
        guard !process.isRunning, process.terminationReason == .exit,
              data.count <= 2_000_000 else {
            if process.isRunning { _ = kill(pid, SIGKILL) }
            return nil
        }
        return data
    }
}
