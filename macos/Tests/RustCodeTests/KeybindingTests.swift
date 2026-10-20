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
