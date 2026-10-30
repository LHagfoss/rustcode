import AppKit
import RustCodeTerminal
import SwiftUI

extension View {
    /// The workspace terminal as a resizable drawer under this view.
    func terminalDrawer(_ terminal: TerminalController, keybindings: KeybindingStore) -> some View {
        modifier(TerminalDrawer(terminal: terminal, keybindings: keybindings))
    }
}

private struct TerminalDrawer: ViewModifier {
    @ObservedObject var terminal: TerminalController
    @ObservedObject var keybindings: KeybindingStore
    @AppStorage("terminalHeight") private var height: Double = 260

    func body(content: Content) -> some View {
        GeometryReader { proxy in
            VStack(spacing: 0) {
                content
                if terminal.isShown {
                    VStack(spacing: 0) {
                        TerminalResizeHandle(height: $height, maximum: max(160, proxy.size.height - 220))
                        TerminalPanel(terminal: terminal, manager: terminal.manager, keybindings: keybindings)
                            .frame(height: min(CGFloat(height), max(160, proxy.size.height - 220)))
                    }
                    .transition(.move(edge: .bottom).combined(with: .opacity))
                }
            }
        }
    }
}

/// The terminal's tabs, the project they belong to, and the selected shell.
struct TerminalPanel: View {
    let terminal: TerminalController
    @ObservedObject var manager: TerminalSessionManager
    @ObservedObject var keybindings: KeybindingStore

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider().opacity(0.6)
            if let session = manager.selectedSession {
                TerminalSessionBody(terminal: terminal, session: session)
                    .id(session.id)
            } else {
                emptyState
            }
        }
        .background(Color.chatBackground)
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Terminal")
    }

    private var projectName: String {
        manager.project.map { ($0 as NSString).lastPathComponent } ?? "No project"
    }

    private var header: some View {
        HStack(spacing: 6) {
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 4) {
                    ForEach(manager.projectSessions) { session in
                        TerminalTab(session: session, isSelected: session.id == manager.selectedSession?.id,
                                    select: { terminal.select(session) },
                                    relaunch: { terminal.relaunch(session) },
                                    close: { terminal.requestClose(session) })
                    }
                }
            }
            .fixedSize(horizontal: false, vertical: true)
            ChromeButton(systemImage: "plus", help: keybindings.help("New terminal tab", .newTerminalTab)) { terminal.newSession() }
                .disabled(!manager.canOpenSession)
                .accessibilityLabel("New terminal tab")
            Spacer(minLength: 8)
            if !manager.runningElsewhere.isEmpty {
                let others = manager.runningElsewhere
                Text("\(others.count) running in other projects")
                    .font(.caption)
                    .foregroundStyle(.tertiary)
                    .help(Set(others.map(\.project)).sorted().joined(separator: "\n"))
            }
            Label(abbreviated(manager.selectedSession?.reportedDirectory ?? manager.project ?? ""), systemImage: "folder")
                .labelStyle(.titleAndIcon)
                .font(.caption)
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .truncationMode(.head)
                .help("Terminals of \(projectName). \(Self.boundary)")
                .accessibilityLabel("Project \(projectName), \(manager.project ?? "")")
            ChromeButton(systemImage: "chevron.down", help: keybindings.help("Hide terminal. Shells keep running", .toggleTerminal)) {
                terminal.hide()
            }
            .accessibilityLabel("Hide terminal")
        }
        .padding(.horizontal, 10)
        .frame(height: 34)
    }

    private var emptyState: some View {
        VStack(spacing: 10) {
            Image(systemName: "terminal")
                .font(.system(size: 28, weight: .light))
                .foregroundStyle(.secondary)
            Text("No terminal in \(projectName)")
                .font(.headline)
            Text(Self.boundary)
                .font(.callout)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
                .frame(maxWidth: 420)
            Button("New Terminal") { terminal.newSession() }
                .buttonStyle(PillButtonStyle(prominent: true))
                .disabled(!manager.canOpenSession)
        }
        .padding(20)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    static let boundary = "Commands here run as you, directly in your shell, not through the agent: "
        + "no tool approvals or sandbox, and the agent does not see them."

    private func abbreviated(_ path: String) -> String {
        let home = FileManager.default.homeDirectoryForCurrentUser.path
        return path == home || path.hasPrefix(home + "/") ? "~" + path.dropFirst(home.count) : path
    }
}

private struct TerminalTab: View {
    @ObservedObject var session: TerminalSession
    let isSelected: Bool
    let select: () -> Void
    let relaunch: () -> Void
    let close: () -> Void
    @State private var isHovered = false

    var body: some View {
        HStack(spacing: 6) {
            Circle()
                .fill(session.isRunning ? Color.green : Color.secondary.opacity(0.5))
                .frame(width: 6, height: 6)
                .accessibilityHidden(true)
            Text(session.displayTitle)
                .font(.callout)
                .lineLimit(1)
                .frame(maxWidth: 160)
            Button(action: close) {
                Image(systemName: "xmark")
                    .font(.system(size: 9, weight: .bold))
                    .frame(width: 14, height: 14)
                    .contentShape(.rect)
            }
            .buttonStyle(.plain)
            .foregroundStyle(.secondary)
            .opacity(isHovered || isSelected ? 1 : 0)
            .help("Close terminal")
            .accessibilityLabel("Close \(session.displayTitle)")
        }
        .padding(.leading, 10)
        .padding(.trailing, 6)
        .frame(height: 24)
        .background(isSelected ? AnyShapeStyle(.fill.secondary) : isHovered ? AnyShapeStyle(.fill.quaternary) : AnyShapeStyle(.clear),
                    in: .capsule)
        .contentShape(.capsule)
        .onTapGesture(perform: select)
        .onHover { isHovered = $0 }
        .help(session.project)
        .contextMenu {
            Button("Relaunch Shell", action: relaunch).disabled(!session.canRelaunch)
            Button("Close Terminal", action: close)
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel("\(session.displayTitle), \(statusText)")
        .accessibilityAddTraits(isSelected ? [.isButton, .isSelected] : .isButton)
        .accessibilityAction(named: "Close", close)
        .accessibilityAction(.default, select)
    }

    private var statusText: String {
        switch session.status {
        case .running: return "running"
        case .exited(let code): return "exited with status \(code)"
        case .failed: return "failed to start"
        case .notStarted, .terminated: return "not running"
        }
    }
}

/// The selected session: its emulator and, once the shell ended, a way to
/// start it again.
private struct TerminalSessionBody: View {
    let terminal: TerminalController
    @ObservedObject var session: TerminalSession

    var body: some View {
        VStack(spacing: 0) {
            TerminalHost(view: terminal.view(for: session), focusRequest: terminal.focusRequest)
                .padding(.leading, 8)
                .padding(.top, 4)
                .background(Color(nsColor: .textBackgroundColor))
            switch session.status {
            case .exited(let code):
                endedBar(code == 0 ? "The shell exited." : "The shell exited with status \(code).")
            case .failed(let message):
                endedBar(message)
            case .notStarted, .running, .terminated:
                EmptyView()
            }
        }
    }

    private func endedBar(_ message: String) -> some View {
        HStack(spacing: 8) {
            Image(systemName: "stop.circle").foregroundStyle(.secondary)
            Text(message).font(.callout)
            Spacer(minLength: 8)
            Button("Relaunch") { terminal.relaunch(session) }
                .buttonStyle(PillButtonStyle(prominent: true))
                .keyboardShortcut(.defaultAction)
            Button("Close") { terminal.close(session) }
                .buttonStyle(PillButtonStyle())
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 8)
        .background(.fill.quaternary)
    }
}

/// Hosts a session's emulator, which outlives this view so hiding the panel
/// keeps the screen and scrollback.
private struct TerminalHost: NSViewRepresentable {
    let view: SessionTerminalView
    let focusRequest: Int

    final class Coordinator {
        var focusRequest = -1
    }

    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeNSView(context: Context) -> NSView {
        let container = NSView()
        embed(in: container)
        return container
    }

    func updateNSView(_ container: NSView, context: Context) {
        if view.superview !== container { embed(in: container) }
        guard context.coordinator.focusRequest != focusRequest else { return }
        context.coordinator.focusRequest = focusRequest
        let view = self.view
        DispatchQueue.main.async { view.window?.makeFirstResponder(view) }
    }

    private func embed(in container: NSView) {
        view.removeFromSuperview()
        view.frame = container.bounds
        view.autoresizingMask = [.width, .height]
        container.addSubview(view)
    }
}

/// The line between the chat and the terminal, dragged to resize it.
private struct TerminalResizeHandle: View {
    @Binding var height: Double
    let maximum: CGFloat
    @State private var startHeight: Double?

    var body: some View {
        Rectangle()
            .fill(Color.primary.opacity(0.1))
            .frame(height: 1)
            .padding(.vertical, 3)
            .contentShape(.rect)
            .onHover { inside in
                if inside { NSCursor.resizeUpDown.push() } else { NSCursor.pop() }
            }
            .gesture(
                DragGesture(minimumDistance: 1, coordinateSpace: .global)
                    .onChanged { drag in
                        let start = startHeight ?? height
                        startHeight = start
                        height = min(max(start - drag.translation.height, 140), Double(maximum))
                    }
                    .onEnded { _ in startHeight = nil }
            )
            .accessibilityHidden(true)
    }
}

/// The title bar button that shows or hides the terminal.
struct TerminalToolbarButton: View {
    @ObservedObject var terminal: TerminalController
    @ObservedObject var keybindings: KeybindingStore

    var body: some View {
        ClusterButton(systemImage: "terminal",
                      help: keybindings.help(terminal.isShown ? "Hide terminal" : "Show terminal", .toggleTerminal),
                      isOn: terminal.isShown) { terminal.toggle() }
    }
}
