import AppKit
import Combine
import Foundation
import SwiftUI

/// The modifiers of a shortcut, kept in the order macOS draws them.
struct KeyModifiers: OptionSet, Codable, Hashable, Sendable {
    let rawValue: Int

    static let control = KeyModifiers(rawValue: 1 << 0)
    static let option = KeyModifiers(rawValue: 1 << 1)
    static let shift = KeyModifiers(rawValue: 1 << 2)
    static let command = KeyModifiers(rawValue: 1 << 3)

    init(rawValue: Int) { self.rawValue = rawValue }

    init(_ flags: NSEvent.ModifierFlags) {
        var modifiers: KeyModifiers = []
        if flags.contains(.control) { modifiers.insert(.control) }
        if flags.contains(.option) { modifiers.insert(.option) }
        if flags.contains(.shift) { modifiers.insert(.shift) }
        if flags.contains(.command) { modifiers.insert(.command) }
        self = modifiers
    }

    var eventModifiers: EventModifiers {
        var modifiers: EventModifiers = []
        if contains(.control) { modifiers.insert(.control) }
        if contains(.option) { modifiers.insert(.option) }
        if contains(.shift) { modifiers.insert(.shift) }
        if contains(.command) { modifiers.insert(.command) }
        return modifiers
    }

    var appKitModifiers: NSEvent.ModifierFlags {
        var modifiers: NSEvent.ModifierFlags = []
        if contains(.control) { modifiers.insert(.control) }
        if contains(.option) { modifiers.insert(.option) }
        if contains(.shift) { modifiers.insert(.shift) }
        if contains(.command) { modifiers.insert(.command) }
        return modifiers
    }

    /// ⌃⌥⇧⌘, the order System Settings uses.
    var symbols: String {
        var text = ""
        if contains(.control) { text += "⌃" }
        if contains(.option) { text += "⌥" }
        if contains(.shift) { text += "⇧" }
        if contains(.command) { text += "⌘" }
        return text
    }

    /// A shortcut with only Shift is a key being typed, not a command.
    var hasCommandModifier: Bool {
        !intersection([.command, .option, .control]).isEmpty
    }
}

/// One app shortcut: the key pressed and the modifiers held with it. The key
/// is a lowercase character for printable keys, or a name such as `space`,
/// `return`, `up` or `pageDown` for the keys that have no character.
struct KeyCombo: Codable, Hashable, Sendable {
    var key: String
    var modifiers: KeyModifiers

    init(key: String, modifiers: KeyModifiers) {
        self.key = key
        self.modifiers = modifiers
    }

    /// Reads a shortcut from a key press. Escape is left to the recorder so
    /// it can cancel, and keys that have no menu equivalent return nil.
    init?(event: NSEvent) {
        let key: String
        switch event.keyCode {
        case 36: key = "return"
        case 48: key = "tab"
        case 49: key = "space"
        case 51: key = "delete"
        case 53: return nil
        case 115: key = "home"
        case 116: key = "pageUp"
        case 117: key = "forwardDelete"
        case 119: key = "end"
        case 121: key = "pageDown"
        case 123: key = "left"
        case 124: key = "right"
        case 125: key = "down"
        case 126: key = "up"
        default:
            // The base character, without Shift: ⇧⌘P is key "p" plus Shift,
            // rather than a different key. Keeping Shift as a modifier means
            // the same binding survives keyboard layouts.
            guard let characters = event.characters(byApplyingModifiers: []) ?? event.charactersIgnoringModifiers,
                  characters.count == 1,
                  let scalar = characters.unicodeScalars.first,
                  !CharacterSet.controlCharacters.contains(scalar),
                  !CharacterSet.illegalCharacters.contains(scalar),
                  !CharacterSet.whitespacesAndNewlines.contains(scalar)
            else { return nil }
            key = characters.lowercased()
        }
        self.init(key: key, modifiers: KeyModifiers(event.modifierFlags))
    }

    /// The key as SwiftUI needs it, or nil for a key it cannot draw in a menu.
    var keyEquivalent: KeyEquivalent? {
        switch key {
        case "return": return .return
        case "tab": return .tab
        case "space": return .space
        case "delete": return .delete
        case "forwardDelete": return .deleteForward
        case "home": return .home
        case "end": return .end
        case "pageUp": return .pageUp
        case "pageDown": return .pageDown
        case "up": return .upArrow
        case "down": return .downArrow
        case "left": return .leftArrow
        case "right": return .rightArrow
        default:
            guard key.count == 1, let character = key.first else { return nil }
            return KeyEquivalent(character)
        }
    }

    var keyboardShortcut: KeyboardShortcut? {
        guard let keyEquivalent else { return nil }
        return KeyboardShortcut(keyEquivalent, modifiers: modifiers.eventModifiers)
    }

    /// The character AppKit stores on a menu item.
    var appKitKeyEquivalent: String {
        guard let keyEquivalent else { return "" }
        return String(keyEquivalent.character).lowercased()
    }

    /// How the shortcut reads in a menu or a tooltip: ⌘N, ⇧⌘P, ⌥⌘←.
    var display: String {
        modifiers.symbols + Self.keyName(key)
    }

    private static func keyName(_ key: String) -> String {
        switch key {
        case "return": return "↩"
        case "tab": return "⇥"
        case "space": return "Space"
        case "delete": return "⌫"
        case "forwardDelete": return "⌦"
        case "home": return "↖"
        case "end": return "↘"
        case "pageUp": return "⇞"
        case "pageDown": return "⇟"
        case "up": return "↑"
        case "down": return "↓"
        case "left": return "←"
        case "right": return "→"
        default: return key.uppercased()
        }
    }

    /// Keys macOS or the app's own text editing already owns. Offering one
    /// here would take it away from typing or from the system.
    var reservedReason: String? {
        Self.reserved[key]?[modifiers]
    }

    private static let reserved: [String: [KeyModifiers: String]] = [
        "q": [.command: "Quit RustCode"],
        "w": [.command: "Close the window"],
        "h": [.command: "Hide RustCode", [.command, .option]: "Hide other apps"],
        "m": [.command: "Minimize the window"],
        "z": [.command: "Undo", [.command, .shift]: "Redo"],
        "x": [.command: "Cut"],
        "c": [.command: "Copy"],
        "v": [.command: "Paste"],
        "a": [.command: "Select All"],
        "tab": [.command: "Switch apps"],
        "space": [.command: "Spotlight"],
    ]
}

/// A command whose shortcut the user can change.
enum KeybindingAction: String, CaseIterable, Identifiable, Codable, Sendable {
    case newChat
    case switchChat
    case stopTurn
    case pinChat
    case findInChat
    case findNext
    case findPrevious
    case toggleSidebar
    case togglePreview
    case reloadPreview
    case toggleTerminal
    case newTerminalTab
    case chooseProject
    case settings

    var id: String { rawValue }

    var title: String {
        switch self {
        case .newChat: return "New Chat"
        case .switchChat: return "Switch Chat"
        case .stopTurn: return "Stop Turn"
        case .pinChat: return "Pin or Unpin Chat"
        case .findInChat: return "Find in Chat"
        case .findNext: return "Find Next"
        case .findPrevious: return "Find Previous"
        case .toggleSidebar: return "Show or Hide Sidebar"
        case .togglePreview: return "Show or Hide Preview"
        case .reloadPreview: return "Reload Preview"
        case .toggleTerminal: return "Show or Hide Terminal"
        case .newTerminalTab: return "New Terminal Tab"
        case .chooseProject: return "Choose Project"
        case .settings: return "Open Settings"
        }
    }

    var detail: String {
        switch self {
        case .newChat: return "Start a fresh chat in the open project"
        case .switchChat: return "Jump to a chat by title"
        case .stopTurn: return "Stop the reply that is streaming"
        case .pinChat: return "Keep the open chat at the top of the sidebar"
        case .findInChat: return "Search the open chat"
        case .findNext: return "Move to the next match"
        case .findPrevious: return "Move to the previous match"
        case .toggleSidebar: return "More room for the chat"
        case .togglePreview: return "Show localhost sites beside the chat"
        case .reloadPreview: return "Reload the page in the preview"
        case .toggleTerminal: return "Your own shell, below the chat"
        case .newTerminalTab: return "Open another shell in this project"
        case .chooseProject: return "Choose the default project folder for new chats"
        case .settings: return "General, keybinds, skills and MCP"
        }
    }

    var category: KeybindingCategory {
        switch self {
        case .newChat, .switchChat, .stopTurn, .pinChat: return .chat
        case .findInChat, .findNext, .findPrevious: return .find
        case .toggleSidebar, .togglePreview, .reloadPreview, .toggleTerminal, .newTerminalTab: return .view
        case .chooseProject, .settings: return .app
        }
    }

    /// The shortcut the app ships with, or nil for a command that has none
    /// until the user gives it one.
    var defaultCombo: KeyCombo? {
        switch self {
        case .newChat: return KeyCombo(key: "n", modifiers: .command)
        case .switchChat: return KeyCombo(key: "k", modifiers: .command)
        case .stopTurn: return KeyCombo(key: ".", modifiers: .command)
        case .pinChat: return KeyCombo(key: "p", modifiers: .command)
        case .findInChat: return KeyCombo(key: "f", modifiers: .command)
        case .findNext: return KeyCombo(key: "g", modifiers: .command)
        case .findPrevious: return KeyCombo(key: "g", modifiers: [.command, .shift])
        case .toggleSidebar: return KeyCombo(key: "b", modifiers: .command)
        case .togglePreview: return KeyCombo(key: "p", modifiers: [.command, .shift])
        case .reloadPreview: return KeyCombo(key: "r", modifiers: .command)
        case .toggleTerminal: return KeyCombo(key: "t", modifiers: [.command, .shift])
        case .newTerminalTab: return nil
        case .chooseProject: return KeyCombo(key: "o", modifiers: .command)
        case .settings: return KeyCombo(key: ",", modifiers: .command)
        }
    }
}

enum KeybindingCategory: String, CaseIterable, Identifiable {
    case chat = "Chat"
    case find = "Find"
    case view = "View"
    case app = "App"

    var id: Self { self }

    var actions: [KeybindingAction] {
        KeybindingAction.allCases.filter { $0.category == self }
    }
}

/// The result of recording a shortcut.
enum KeybindingAssignment: Equatable {
    /// The shortcut was saved; `displaced` had been using it and is now unset.
    case assigned(displaced: KeybindingAction?)
    /// The shortcut belongs to the system or to text editing.
    case rejected(String)
}

/// The user's shortcuts, saved on this Mac. A shortcut that is not overridden
/// uses its default; clearing one remembers that it is off rather than
/// falling back to the default again.
@MainActor
final class KeybindingStore: ObservableObject {
    static let defaultsKey = "keybindings"

    private let defaults: UserDefaults
    private var overrides: [KeybindingAction: KeyCombo] = [:]
    private var disabled: Set<KeybindingAction> = []

    /// Bumped by every save, so views that read a shortcut redraw.
    @Published private(set) var revision = 0

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        load()
    }

    func combo(for action: KeybindingAction) -> KeyCombo? {
        if let override = overrides[action] { return override }
        return disabled.contains(action) ? nil : action.defaultCombo
    }

    func shortcut(for action: KeybindingAction) -> KeyboardShortcut? {
        combo(for: action)?.keyboardShortcut
    }

    func display(for action: KeybindingAction) -> String {
        combo(for: action)?.display ?? "Not set"
    }

    /// A menu item's title followed by its shortcut, for tooltips.
    func help(_ title: String, _ action: KeybindingAction) -> String {
        guard let combo = combo(for: action) else { return title }
        return "\(title) (\(combo.display))"
    }

    func isCustomized(_ action: KeybindingAction) -> Bool {
        overrides[action] != nil || disabled.contains(action)
    }

    var hasCustomizations: Bool { !overrides.isEmpty || !disabled.isEmpty }

    /// Saves a recorded shortcut, unbinding whichever action used it before.
    @discardableResult
    func assign(_ combo: KeyCombo, to action: KeybindingAction) -> KeybindingAssignment {
        guard combo.modifiers.hasCommandModifier else {
            return .rejected("Use ⌘, ⌥ or ⌃ with a key.")
        }
        if let reserved = combo.reservedReason {
            return .rejected("\(combo.display) is used by macOS for \(reserved).")
        }
        let displaced = conflictingAction(for: combo, excluding: action)
        if let displaced {
            overrides.removeValue(forKey: displaced)
            disabled.insert(displaced)
        }
        // A shortcut equal to the default needs no override; storing it as
        // one would keep the action pinned if the default ever changes.
        disabled.remove(action)
        if combo == action.defaultCombo {
            overrides.removeValue(forKey: action)
        } else {
            overrides[action] = combo
        }
        save()
        return .assigned(displaced: displaced)
    }

    /// Removes a shortcut from an action, leaving it in the menu without a key.
    func clear(_ action: KeybindingAction) {
        overrides.removeValue(forKey: action)
        if action.defaultCombo == nil {
            disabled.remove(action)
        } else {
            disabled.insert(action)
        }
        save()
    }

    func reset(_ action: KeybindingAction) {
        overrides.removeValue(forKey: action)
        disabled.remove(action)
        save()
    }

    func resetAll() {
        overrides.removeAll()
        disabled.removeAll()
        save()
    }

    /// The action already using `combo`, if any. The store never keeps two
    /// actions on one shortcut, and this also catches older saved data.
    func conflictingAction(for combo: KeyCombo, excluding action: KeybindingAction) -> KeybindingAction? {
        KeybindingAction.allCases.first { $0 != action && self.combo(for: $0) == combo }
    }

    // MARK: - Saving

    private struct Stored: Codable {
        var overrides: [String: KeyCombo] = [:]
        var disabled: [String] = []
    }

    private func load() {
        guard let data = defaults.data(forKey: Self.defaultsKey),
              let stored = try? JSONDecoder().decode(Stored.self, from: data) else { return }
        for (id, combo) in stored.overrides {
            if let action = KeybindingAction(rawValue: id) { overrides[action] = combo }
        }
        disabled = Set(stored.disabled.compactMap(KeybindingAction.init(rawValue:)))
    }

    private func save() {
        let stored = Stored(overrides: Dictionary(uniqueKeysWithValues: overrides.map { ($0.key.rawValue, $0.value) }),
                            disabled: disabled.map(\.rawValue).sorted())
        if let data = try? JSONEncoder().encode(stored) {
            defaults.set(data, forKey: Self.defaultsKey)
        }
        revision += 1
    }
}

/// AppKit draws a menu item's shortcut from its key equivalent. SwiftUI lays
/// that out when it builds the commands but does not reliably replace it when
/// the user remaps a command. Keeping the real menus in step here means a new
/// shortcut both shows in the menu and is received by it, right away.
@MainActor
final class KeybindingMenuSync {
    private let store: KeybindingStore
    private var changes: AnyCancellable?
    private var observers: [NSObjectProtocol] = []
    private var applying = false

    init(store: KeybindingStore) {
        self.store = store
        changes = store.objectWillChange.sink { [weak self] _ in
            Task { @MainActor in self?.apply() }
        }
        let center = NotificationCenter.default
        // SwiftUI rebuilds menus as state changes; reapply whenever an item
        // appears, changes, or the menu is about to be shown.
        for name in [NSMenu.didAddItemNotification, NSMenu.didChangeItemNotification,
                     NSMenu.didBeginTrackingNotification] {
            observers.append(center.addObserver(forName: name, object: nil, queue: .main) { [weak self] _ in
                Task { @MainActor in self?.apply() }
            })
        }
    }

    func apply() {
        guard !applying, let menu = NSApp.mainMenu else { return }
        applying = true
        defer { applying = false }
        update(menu)
    }

    private func update(_ menu: NSMenu) {
        for item in menu.items {
            if let action = Self.actions[item.title] {
                let combo = store.combo(for: action)
                let key = combo?.appKitKeyEquivalent ?? ""
                let modifiers = combo?.modifiers.appKitModifiers ?? []
                if item.keyEquivalent != key { item.keyEquivalent = key }
                if item.keyEquivalentModifierMask != modifiers { item.keyEquivalentModifierMask = modifiers }
            }
            if let submenu = item.submenu { update(submenu) }
        }
    }

    /// SwiftUI does not expose the menu item behind a command, so they are
    /// found by the titles this app gives them.
    private static let actions: [String: KeybindingAction] = [
        "New Chat": .newChat,
        "Switch Chat…": .switchChat,
        "Stop Turn": .stopTurn,
        "Pin Chat": .pinChat,
        "Unpin Chat": .pinChat,
        "Find in Chat…": .findInChat,
        "Find Next": .findNext,
        "Find Previous": .findPrevious,
        "Show Sidebar": .toggleSidebar,
        "Hide Sidebar": .toggleSidebar,
        "Show Preview": .togglePreview,
        "Hide Preview": .togglePreview,
        "Reload Preview": .reloadPreview,
        "Show Terminal": .toggleTerminal,
        "Hide Terminal": .toggleTerminal,
        "New Terminal Tab": .newTerminalTab,
        "Choose Project…": .chooseProject,
        "Settings…": .settings,
    ]
}
