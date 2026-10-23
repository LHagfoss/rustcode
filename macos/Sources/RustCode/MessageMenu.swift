import AppKit
import SwiftUI

/// One action in a message's context menu.
struct MessageMenuItem {
    var title: String
    var systemImage: String
    var action: () -> Void
    /// Starts a new group: a separator goes above it.
    var startsGroup = false
}

extension View {
    /// A message's context menu that a right-click over its selectable text
    /// opens too. On such text the click went to the text, which selected a
    /// word instead of opening the menu (#2307). The SwiftUI menu stays for
    /// VoiceOver and the keyboard.
    func messageMenu(_ items: @escaping () -> [MessageMenuItem]) -> some View {
        contextMenu {
            ForEach(Array(items().enumerated()), id: \.offset) { _, item in
                if item.startsGroup { Divider() }
                Button(item.title, systemImage: item.systemImage, action: item.action)
            }
        }
        .overlay(RightClickCatcher(items: items))
    }
}

/// A transparent view over a message that takes only right- and
/// control-clicks, so every other click, drag and hover reaches the text
/// below it and selection works as before.
private struct RightClickCatcher: NSViewRepresentable {
    let items: () -> [MessageMenuItem]

    func makeNSView(context: Context) -> CatcherView { CatcherView() }

    func updateNSView(_ view: CatcherView, context: Context) {
        view.items = items
    }

    final class CatcherView: NSView {
        var items: () -> [MessageMenuItem] = { [] }

        static func opensMenu(_ event: NSEvent?) -> Bool {
            guard let event else { return false }
            switch event.type {
            case .rightMouseDown, .rightMouseUp: return true
            case .leftMouseDown, .leftMouseUp: return event.modifierFlags.contains(.control)
            default: return false
            }
        }

        override func hitTest(_ point: NSPoint) -> NSView? {
            Self.opensMenu(NSApp.currentEvent) ? super.hitTest(point) : nil
        }

        override func menu(for event: NSEvent) -> NSMenu? {
            let menu = NSMenu()
            for item in items() {
                if item.startsGroup { menu.addItem(.separator()) }
                let entry = ClosureMenuItem(title: item.title, action: item.action)
                entry.image = NSImage(systemSymbolName: item.systemImage, accessibilityDescription: nil)
                menu.addItem(entry)
            }
            return menu.items.isEmpty ? nil : menu
        }

        override func rightMouseDown(with event: NSEvent) {
            guard let menu = menu(for: event) else { return super.rightMouseDown(with: event) }
            NSMenu.popUpContextMenu(menu, with: event, for: self)
        }

        override func mouseDown(with event: NSEvent) {
            guard event.modifierFlags.contains(.control), let menu = menu(for: event) else {
                return super.mouseDown(with: event)
            }
            NSMenu.popUpContextMenu(menu, with: event, for: self)
        }
    }
}

/// A menu item that runs a closure, so the menu needs no target object.
private final class ClosureMenuItem: NSMenuItem {
    private let run: () -> Void

    init(title: String, action: @escaping () -> Void) {
        run = action
        super.init(title: title, action: #selector(runAction), keyEquivalent: "")
        target = self
    }

    @available(*, unavailable)
    required init(coder: NSCoder) { fatalError("init(coder:) is not used") }

    @objc private func runAction() { run() }
}
