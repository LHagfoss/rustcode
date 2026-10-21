import AppKit
import XCTest
@testable import RustCode

/// The shortcuts saved in Settings: their defaults, what a recording saves,
/// and how conflicts and reserved keys are handled.
@MainActor
final class KeybindingTests: XCTestCase {
    private var suite: String!
    private var defaults: UserDefaults!

    override func setUp() {
        super.setUp()
        suite = "KeybindingTests.\(UUID().uuidString)"
        defaults = UserDefaults(suiteName: suite)
    }

    override func tearDown() {
        defaults.removePersistentDomain(forName: suite)
        defaults = nil
        suite = nil
        super.tearDown()
    }

    private func makeStore() -> KeybindingStore {
        KeybindingStore(defaults: defaults)
    }

    func testDefaultsAreTheMenusShortcuts() {
        let store = makeStore()
        XCTAssertEqual(store.display(for: .newChat), "⌘N")
        XCTAssertEqual(store.display(for: .switchChat), "⌘K")
        XCTAssertEqual(store.display(for: .findPrevious), "⇧⌘G")
        XCTAssertEqual(store.display(for: .togglePreview), "⇧⌘P")
        XCTAssertEqual(store.display(for: .settings), "⌘,")
        XCTAssertEqual(store.display(for: .newTerminalTab), "Not set")
        XCTAssertNil(store.combo(for: .newTerminalTab))
        XCTAssertFalse(store.hasCustomizations)
    }

    func testAssignmentPersists() {
        let first = makeStore()
        let combo = KeyCombo(key: "j", modifiers: [.command, .option])
        XCTAssertEqual(first.assign(combo, to: .newChat), .assigned(displaced: nil))

        let reopened = makeStore()
        XCTAssertEqual(reopened.combo(for: .newChat), combo)
        XCTAssertEqual(reopened.display(for: .newChat), "⌥⌘J")
        XCTAssertTrue(reopened.isCustomized(.newChat))
    }

    func testAssigningAUsedShortcutUnbindsTheOtherAction() {
        let store = makeStore()
        let result = store.assign(KeyCombo(key: "k", modifiers: .command), to: .newChat)
        XCTAssertEqual(result, .assigned(displaced: .switchChat))
        XCTAssertEqual(store.display(for: .newChat), "⌘K")
        XCTAssertNil(store.combo(for: .switchChat))
        XCTAssertTrue(store.isCustomized(.switchChat))
    }

    func testClearingAndResetting() {
        let store = makeStore()
        store.clear(.newChat)
        XCTAssertNil(store.combo(for: .newChat))
        XCTAssertEqual(store.display(for: .newChat), "Not set")
        XCTAssertTrue(store.isCustomized(.newChat))

        // Clearing survives a restart rather than falling back to ⌘N.
        XCTAssertNil(makeStore().combo(for: .newChat))

        store.reset(.newChat)
        XCTAssertEqual(store.display(for: .newChat), "⌘N")
        XCTAssertFalse(store.isCustomized(.newChat))
    }

    func testResetAllRestoresEveryDefault() {
        let store = makeStore()
        store.assign(KeyCombo(key: "j", modifiers: .command), to: .newChat)
        store.clear(.findInChat)
        XCTAssertTrue(store.hasCustomizations)

        store.resetAll()
        XCTAssertFalse(store.hasCustomizations)
        XCTAssertEqual(store.display(for: .newChat), "⌘N")
        XCTAssertEqual(store.display(for: .findInChat), "⌘F")
        XCTAssertFalse(makeStore().hasCustomizations)
    }

    func testAssigningTheDefaultRemovesTheOverride() {
        let store = makeStore()
        store.assign(KeyCombo(key: "j", modifiers: .command), to: .newChat)
        XCTAssertTrue(store.isCustomized(.newChat))

        store.assign(KeyCombo(key: "n", modifiers: .command), to: .newChat)
        XCTAssertFalse(store.isCustomized(.newChat))
        XCTAssertEqual(store.display(for: .newChat), "⌘N")
    }

    func testSystemAndTypingKeysAreRejected() {
        let store = makeStore()
        XCTAssertEqual(store.assign(KeyCombo(key: "q", modifiers: .command), to: .newChat),
                       .rejected("⌘Q is used by macOS for Quit RustCode."))
        XCTAssertEqual(store.assign(KeyCombo(key: "n", modifiers: []), to: .newChat),
                       .rejected("Use ⌘, ⌥ or ⌃ with a key."))
        XCTAssertEqual(store.display(for: .newChat), "⌘N")
    }

    func testKeyPressParsing() throws {
        let press = try XCTUnwrap(NSEvent.keyEvent(
            with: .keyDown, location: .zero, modifierFlags: [.command, .shift], timestamp: 0,
            windowNumber: 0, context: nil, characters: "P", charactersIgnoringModifiers: "P",
            isARepeat: false, keyCode: 35))
        XCTAssertEqual(KeyCombo(event: press), KeyCombo(key: "p", modifiers: [.command, .shift]))
        XCTAssertEqual(KeyCombo(event: press)?.display, "⇧⌘P")

        let left = try XCTUnwrap(NSEvent.keyEvent(
            with: .keyDown, location: .zero, modifierFlags: .command, timestamp: 0,
            windowNumber: 0, context: nil, characters: "\u{F702}", charactersIgnoringModifiers: "\u{F702}",
            isARepeat: false, keyCode: 123))
        XCTAssertEqual(KeyCombo(event: left), KeyCombo(key: "left", modifiers: .command))
        XCTAssertEqual(KeyCombo(event: left)?.display, "⌘←")
    }

    func testUndoRevertsTheLastChange() {
        let store = makeStore()
        XCTAssertFalse(store.canUndo)
        XCTAssertFalse(store.undo())

        store.assign(KeyCombo(key: "j", modifiers: .command), to: .newChat)
        store.clear(.findInChat)
        XCTAssertTrue(store.canUndo)

        XCTAssertTrue(store.undo())
        XCTAssertEqual(store.display(for: .findInChat), "⌘F")
        XCTAssertEqual(store.display(for: .newChat), "⌘J")
        // The undone state is what is saved.
        XCTAssertEqual(makeStore().display(for: .findInChat), "⌘F")

        XCTAssertTrue(store.undo())
        XCTAssertEqual(store.display(for: .newChat), "⌘N")
        XCTAssertFalse(store.hasCustomizations)
        XCTAssertFalse(store.canUndo)
    }

    func testUndoBringsBackADisplacedShortcut() {
        let store = makeStore()
        store.assign(KeyCombo(key: "k", modifiers: .command), to: .newChat)
        XCTAssertNil(store.combo(for: .switchChat))

        store.undo()
        XCTAssertEqual(store.display(for: .newChat), "⌘N")
        XCTAssertEqual(store.display(for: .switchChat), "⌘K")
    }

    func testUndoAfterResetAllRestoresEveryCustomization() {
        let store = makeStore()
        store.assign(KeyCombo(key: "j", modifiers: .command), to: .newChat)
        store.clear(.settings)
        store.resetAll()
        XCTAssertFalse(store.hasCustomizations)

        store.undo()
        XCTAssertEqual(store.display(for: .newChat), "⌘J")
        XCTAssertNil(store.combo(for: .settings))
    }

    /// A change that leaves every shortcut as it was is not one to undo.
    func testNoOpChangesAreNotRemembered() {
        let store = makeStore()
        store.reset(.newChat)
        store.resetAll()
        store.assign(KeyCombo(key: "n", modifiers: .command), to: .newChat)
        store.assign(KeyCombo(key: "q", modifiers: .command), to: .newChat)
        XCTAssertFalse(store.canUndo)
    }

    func testUndoHistoryIsBounded() {
        let store = makeStore()
        for index in 0..<(KeybindingStore.historyLimit + 10) {
            let key = index.isMultiple(of: 2) ? "j" : "l"
            store.assign(KeyCombo(key: key, modifiers: .command), to: .newChat)
        }
        var undone = 0
        while store.undo() { undone += 1 }
        XCTAssertEqual(undone, KeybindingStore.historyLimit)
    }

    /// ⌘W closes the terminal panel rather than the window, and can be
    /// remapped like any other command.
    func testCommandWClosesTheTerminalAndIsConfigurable() {
        let store = makeStore()
        let commandW = KeyCombo(key: "w", modifiers: .command)
        XCTAssertEqual(store.combo(for: .closeTerminal), commandW)
        XCTAssertNil(commandW.reservedReason)
        XCTAssertEqual(KeybindingAction.closeTerminal.category, .view)

        XCTAssertEqual(store.assign(KeyCombo(key: "w", modifiers: [.command, .shift]), to: .closeTerminal),
                       .assigned(displaced: nil))
        XCTAssertEqual(store.display(for: .closeTerminal), "⇧⌘W")

        // Another command may take ⌘W, leaving the terminal without it.
        store.reset(.closeTerminal)
        XCTAssertEqual(store.assign(commandW, to: .newTerminalTab), .assigned(displaced: .closeTerminal))
        XCTAssertNil(store.combo(for: .closeTerminal))
    }

    func testRecorderKeys() {
        let combo = KeyCombo(key: "j", modifiers: .command)
        // Esc cancels, whatever is held with it.
        XCTAssertEqual(KeyRecorderInput(keyCode: 53, modifiers: [], combo: nil), .cancel)
        XCTAssertEqual(KeyRecorderInput(keyCode: 53, modifiers: .command, combo: nil), .cancel)
        // Delete and Forward Delete clear; with ⌘ they are a shortcut.
        XCTAssertEqual(KeyRecorderInput(keyCode: 51, modifiers: [], combo: nil), .clear)
        XCTAssertEqual(KeyRecorderInput(keyCode: 117, modifiers: .shift, combo: nil), .clear)
        let commandDelete = KeyCombo(key: "delete", modifiers: .command)
        XCTAssertEqual(KeyRecorderInput(keyCode: 51, modifiers: .command, combo: commandDelete), .record(commandDelete))
        XCTAssertEqual(KeyRecorderInput(keyCode: 38, modifiers: .command, combo: combo), .record(combo))
        XCTAssertEqual(KeyRecorderInput(keyCode: 122, modifiers: .command, combo: nil), .unusable)
    }

    func testRecorderReadsEscapeFromAKeyPress() throws {
        let escape = try XCTUnwrap(NSEvent.keyEvent(
            with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0,
            windowNumber: 0, context: nil, characters: "\u{1b}", charactersIgnoringModifiers: "\u{1b}",
            isARepeat: false, keyCode: 53))
        XCTAssertEqual(KeyRecorderInput(event: escape), .cancel)
    }

    func testHelpIncludesTheCurrentShortcut() {
        let store = makeStore()
        XCTAssertEqual(store.help("New chat", .newChat), "New chat (⌘N)")
        store.clear(.newChat)
        XCTAssertEqual(store.help("New chat", .newChat), "New chat")
    }

    /// The AppKit menu is what actually receives a shortcut; a remap has to
    /// reach it, and clearing a shortcut has to take the old one away.
    func testMenuSyncFollowsRemaps() {
        _ = NSApplication.shared
        let previous = NSApp.mainMenu
        defer { NSApp.mainMenu = previous }

        let menu = NSMenu()
        let newChat = NSMenuItem(title: "New Chat", action: nil, keyEquivalent: "n")
        newChat.keyEquivalentModifierMask = .command
        let settings = NSMenuItem(title: "Settings…", action: nil, keyEquivalent: ",")
        settings.keyEquivalentModifierMask = .command
        menu.addItem(newChat)
        menu.addItem(settings)
        NSApp.mainMenu = menu

        let store = makeStore()
        let sync = KeybindingMenuSync(store: store)
        sync.apply()
        XCTAssertEqual(newChat.keyEquivalent, "n")
        XCTAssertEqual(settings.keyEquivalent, ",")

        store.assign(KeyCombo(key: "j", modifiers: .command), to: .newChat)
        store.clear(.settings)
        sync.apply()

        XCTAssertEqual(newChat.keyEquivalent, "j")
        XCTAssertEqual(newChat.keyEquivalentModifierMask, .command)
        XCTAssertEqual(settings.keyEquivalent, "")
        XCTAssertEqual(settings.keyEquivalentModifierMask, [])
    }
}
