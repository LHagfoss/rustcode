import XCTest

@MainActor
final class InputAndTranscriptTests: XCTestCase {
    override func setUpWithError() throws { continueAfterFailure = false }

    func testAttachmentMenuDismissalPreservesDraftAndComposerControls() {
        let app = launchChat(draft: "Keep my attachment menu draft")
        app.buttons["Chat actions"].tap()
        XCTAssertTrue(app.buttons["Camera"].waitForExistence(timeout: 3))
        XCTAssertTrue(app.buttons["Photos"].isHittable)
        XCTAssertTrue(app.buttons["Files"].isHittable)
        XCTAssertTrue(app.buttons["More"].isHittable)
        let menu = app.scrollViews["chatActionsMenu"]
        XCTAssertGreaterThanOrEqual(menu.frame.width, 240)
        XCTAssertTrue(app.keyboards.firstMatch.exists)
        XCTAssertGreaterThanOrEqual(menu.frame.height, 240)
        XCTAssertLessThan(menu.frame.maxY, app.buttons["Chat actions"].frame.minY)
        let screenshot = XCTAttachment(screenshot: app.screenshot())
        screenshot.name = "Compact attachment menu above the composer"
        screenshot.lifetime = .keepAlways
        add(screenshot)
        app.buttons["More"].tap()
        XCTAssertTrue(app.buttons["Slash Commands"].waitForExistence(timeout: 3))
        XCTAssertTrue(app.buttons["New Session in Folder"].isHittable)
        app.coordinate(withNormalizedOffset: CGVector(dx: 0.9, dy: 0.2)).tap()
        XCTAssertTrue(app.buttons["Photos"].waitForNonExistence(timeout: 3))
        XCTAssertEqual(composer(in: app).value as? String, "Keep my attachment menu draft")
        XCTAssertTrue(app.buttons["Model and effort"].isHittable)
        XCTAssertTrue(app.buttons["Send message"].isHittable)
    }

    func testImageFilePickerCancellationPreservesDraft() {
        let app = launchChat(draft: "Keep my file draft")
        app.buttons["Chat actions"].tap()
        let files = app.buttons["Files"]
        XCTAssertTrue(files.waitForExistence(timeout: 3))
        files.tap()
        let cancel = app.buttons["Cancel"]
        XCTAssertTrue(cancel.waitForExistence(timeout: 5))
        cancel.tap()
        XCTAssertTrue(cancel.waitForNonExistence(timeout: 5))
        XCTAssertEqual(composer(in: app).value as? String, "Keep my file draft")
        XCTAssertTrue(app.buttons["Model and effort"].isHittable)
    }

    func testUnavailableCameraExplainsAlternativesWithoutLosingDraft() {
        let app = launchChat(draft: "Keep my camera draft")
        app.buttons["Chat actions"].tap()
        let camera = app.buttons["Camera"]
        XCTAssertTrue(camera.waitForExistence(timeout: 3))
        camera.tap()
        let notice = app.alerts.firstMatch
        XCTAssertTrue(notice.waitForExistence(timeout: 5))
        XCTAssertTrue(notice.staticTexts["The camera is unavailable on this device. Choose Photos or Files to attach an image."].exists)
        notice.buttons["OK"].tap()
        XCTAssertEqual(composer(in: app).value as? String, "Keep my camera draft")
    }

    func testLaunchWithoutSelectionKeepsChatAsTheMainScreen() {
        let app = XCUIApplication()
        app.launchArguments = ["-demo"]
        app.launch()
        XCTAssertTrue(app.buttons["Start a new session"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.buttons["Show sessions"].isHittable)
        XCTAssertFalse(app.navigationBars["Sessions"].isHittable)
        let screenshot = XCTAttachment(screenshot: app.screenshot())
        screenshot.name = "Chat landing without an automatic sidebar"
        screenshot.lifetime = .keepAlways
        add(screenshot)
        app.buttons["Show sessions"].tap()
        XCTAssertTrue(app.navigationBars["Sessions"].waitForExistence(timeout: 3))
        app.navigationBars["Sessions"].buttons["Close sessions"].tap()
        XCTAssertTrue(app.buttons["Start a new session"].isHittable)
    }

    func testSidebarSearchStaysAboveResultsAndDismissesWithTheDrawer() {
        let app = launchChat(draft: "Keep my search draft")
        app.buttons["Show sessions"].tap()
        let search = app.textFields["Search sessions"]
        XCTAssertTrue(search.waitForExistence(timeout: 3))
        let row = app.buttons.matching(NSPredicate(format: "label CONTAINS %@", "Draft release notes")).firstMatch
        XCTAssertLessThan(search.frame.maxY, row.frame.minY)
        search.tap()
        search.typeText("Page the session store")
        XCTAssertFalse(row.exists)
        XCTAssertTrue(app.buttons.matching(NSPredicate(format: "label CONTAINS %@", "Page the session store")).firstMatch.exists)
        let screenshot = XCTAttachment(screenshot: app.screenshot())
        screenshot.name = "Sidebar search above filtered sessions with keyboard"
        screenshot.lifetime = .keepAlways
        add(screenshot)
        app.navigationBars["Sessions"].buttons["Close sessions"].tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForNonExistence(timeout: 3))
        XCTAssertEqual(composer(in: app).value as? String, "Keep my search draft")
    }

    func testTappingOutsideTheDrawerReturnsToTheSameChat() {
        let app = launchChat(draft: "Keep my backdrop draft")
        app.buttons["Show sessions"].tap()
        let close = app.navigationBars["Sessions"].buttons["Close sessions"]
        XCTAssertTrue(close.waitForExistence(timeout: 3))
        app.coordinate(withNormalizedOffset: CGVector(dx: 0.95, dy: 0.5)).tap()
        XCTAssertTrue(close.waitForNonExistence(timeout: 3))
        XCTAssertTrue(app.buttons["Model and effort"].isHittable)
        XCTAssertEqual(composer(in: app).value as? String, "Keep my backdrop draft")
    }

    func testSidebarPushesTheChatScreenAndReturnsItOnDismissal() {
        let app = launchChat(draft: "Keep my pushed chat draft")
        let chatBar = app.navigationBars["Draft release notes"]
        let originalX = chatBar.frame.minX
        app.buttons["Show sessions"].tap()
        let close = app.navigationBars["Sessions"].buttons["Close sessions"]
        XCTAssertTrue(close.waitForExistence(timeout: 3))
        XCTAssertGreaterThan(chatBar.frame.minX - originalX, app.frame.width * 0.75)
        let screenshot = XCTAttachment(screenshot: app.screenshot())
        screenshot.name = "Sessions reveal pushes the whole chat screen"
        screenshot.lifetime = .keepAlways
        add(screenshot)
        app.coordinate(withNormalizedOffset: CGVector(dx: 0.95, dy: 0.5)).tap()
        XCTAssertTrue(close.waitForNonExistence(timeout: 3))
        XCTAssertEqual(chatBar.frame.minX, originalX, accuracy: 1)
        XCTAssertEqual(composer(in: app).value as? String, "Keep my pushed chat draft")
    }

    func testLeavingHostDismissesSettingsBeforeShowingPairing() {
        let app = XCUIApplication()
        app.launchArguments = ["-demo", "-demoSettings"]
        app.launch()
        XCTAssertTrue(app.buttons["Exit Demo"].waitForExistence(timeout: 5))
        app.buttons["Exit Demo"].tap()
        XCTAssertTrue(app.textFields["Host address and port"].waitForExistence(timeout: 5))
        XCTAssertFalse(app.navigationBars["Settings"].exists)
        XCTAssertTrue(app.buttons["Try the Demo"].exists)
    }

    func testPairingKeyboardCanAdvanceAndDismissWithoutLosingInput() {
        let app = XCUIApplication()
        app.launch()
        dismissLaunchNotice(in: app)
        let address = app.textFields["Host address and port"]
        XCTAssertTrue(address.waitForExistence(timeout: 5))
        // Tap near the edge of the field, where the old padding did not focus it.
        address.coordinate(withNormalizedOffset: CGVector(dx: 0.05, dy: 0.5)).tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: 3))
        address.typeText("192.168.1.20:17879")
        app.keyboards.buttons["Next:"].tap()
        let code = app.textFields["Pairing code"]
        XCTAssertTrue(app.keyboards.keys["1"].exists)
        XCTAssertFalse(app.keyboards.keys["a"].exists, "The eight-digit code uses a number pad")
        code.typeText("1234")
        XCTAssertEqual(code.value as? String, "1234")
        code.typeText("5")
        XCTAssertEqual(code.value as? String, "1234-5")
        code.typeText("678")
        app.buttons["Dismiss pairing keyboard"].tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForNonExistence(timeout: 3))
        XCTAssertEqual(address.value as? String, "192.168.1.20:17879")
        XCTAssertEqual(code.value as? String, "1234-5678")
    }

    func testPairingOutsideTapDismissesEachKeyboardWithoutLosingInput() {
        let app = XCUIApplication()
        app.launch()
        dismissLaunchNotice(in: app)
        let address = app.textFields["Host address and port"]
        XCTAssertTrue(address.waitForExistence(timeout: 5))
        address.tap()
        address.typeText("192.168.1.20:17879")
        app.staticTexts["Nearby Macs"].tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForNonExistence(timeout: 3))
        let code = app.textFields["Pairing code"]
        code.tap()
        code.typeText("12345678")
        app.staticTexts["Nearby Macs"].tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForNonExistence(timeout: 3))
        XCTAssertEqual(address.value as? String, "192.168.1.20:17879")
        XCTAssertEqual(code.value as? String, "1234-5678")
    }

    func testPairingAddressDoneUsesCompactNativeKeyboardToolbar() {
        let app = XCUIApplication()
        app.launch()
        dismissLaunchNotice(in: app)
        let address = app.textFields["Host address and port"]
        XCTAssertTrue(address.waitForExistence(timeout: 5))
        address.tap()
        address.typeText("192.168.1.20:17879")
        let done = app.buttons["pairingKeyboardDone"]
        XCTAssertTrue(done.waitForExistence(timeout: 3))
        XCTAssertLessThan(done.frame.width, app.frame.width * 0.25)
        done.tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForNonExistence(timeout: 3))
        XCTAssertEqual(address.value as? String, "192.168.1.20:17879")
    }

    func testPairingInputRowWhitespaceDismissesKeyboard() {
        let app = XCUIApplication()
        app.launch()
        dismissLaunchNotice(in: app)
        let address = app.textFields["Host address and port"]
        XCTAssertTrue(address.waitForExistence(timeout: 5))
        address.tap()
        address.typeText("192.168.1.20:17879")
        // Inside the Form row, just above the field's editing bounds.
        address.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0))
            .withOffset(CGVector(dx: 0, dy: -6)).tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForNonExistence(timeout: 3))
        XCTAssertEqual(address.value as? String, "192.168.1.20:17879")
    }

    func testPairingOutsideDismissalDoesNotBlockConnectValidation() {
        let app = XCUIApplication()
        app.launch()
        dismissLaunchNotice(in: app)
        let address = app.textFields["Host address and port"]
        XCTAssertTrue(address.waitForExistence(timeout: 5))
        address.tap()
        address.typeText("localhost")
        app.keyboards.buttons["Next:"].tap()
        app.textFields["Pairing code"].typeText("12345678")
        app.buttons["Dismiss pairing keyboard"].tap()
        app.buttons["Connect"].tap()
        XCTAssertTrue(app.staticTexts["Include the port, for example 192.168.1.20:17879."].waitForExistence(timeout: 3))
    }

    func testPastingTheCodeConnectsAndExplainsAFailure() {
        UIPasteboard.general.string = "12345678"
        let app = XCUIApplication()
        app.launch()
        dismissLaunchNotice(in: app)
        let address = app.textFields["Host address and port"]
        XCTAssertTrue(address.waitForExistence(timeout: 5))
        address.tap()
        // Nothing listens on the discard port, so the connection is refused.
        address.typeText("127.0.0.1:9")
        app.keyboards.buttons["Next:"].tap()
        // Pasting the last missing value connects without a tap on Connect.
        app.descendants(matching: .any)["pairingKeyboardPaste"].tap()
        let failure = app.staticTexts.matching(NSPredicate(format: "label BEGINSWITH %@", "Pairing failed.")).firstMatch
        XCTAssertTrue(failure.waitForExistence(timeout: 20))
        XCTAssertTrue(failure.label.contains("Nothing is listening at 127.0.0.1:9"), "The reason names what failed: \(failure.label)")
        XCTAssertEqual(app.textFields["Pairing code"].value as? String, "1234-5678")
        XCTAssertTrue(XCTWaiter.wait(for: [XCTNSPredicateExpectation(predicate: NSPredicate(format: "hittable == true"), object: failure)], timeout: 3) == .completed, "The failure reason must be on screen")
        let screenshot = XCTAttachment(screenshot: app.screenshot())
        screenshot.name = "Pairing failure with its reason"
        screenshot.lifetime = .keepAlways
        add(screenshot)
    }

    func testPairingScannerCanCloseAndReopenWithoutLosingTypedCode() {
        let app = XCUIApplication()
        app.launch()
        dismissLaunchNotice(in: app)
        let code = app.textFields["Pairing code"]
        XCTAssertTrue(code.waitForExistence(timeout: 5))
        code.tap()
        code.typeText("12345678")
        app.buttons["Dismiss pairing keyboard"].tap()
        for _ in 0..<2 {
            let scan = app.buttons["Scan Pairing Code"]
            if !scan.isHittable { app.swipeDown() }
            scan.tap()
            XCTAssertTrue(app.staticTexts["Scanning Unavailable"].waitForExistence(timeout: 5))
            app.buttons["Enter Code Manually"].tap()
            XCTAssertTrue(app.navigationBars["Scan Pairing Code"].waitForNonExistence(timeout: 3))
        }
        if !code.isHittable { app.swipeUp() }
        XCTAssertEqual(code.value as? String, "1234-5678")
        code.tap()
        code.typeText("9")
        XCTAssertEqual(code.value as? String, "1234-5678", "Entry is limited to eight digits")
        code.typeText(String(repeating: XCUIKeyboardKey.delete.rawValue, count: 5))
        XCTAssertEqual(code.value as? String, "123", "Backspace must pass the automatic dash")
    }

    /// The Simulator has no camera, so capture fails to configure. Reaching that
    /// error on every open, retry and reopen proves the scanner always asks the
    /// camera to start, instead of sitting on a black view with no way forward.
    func testPairingScannerAlwaysStartsCaptureOnEveryOpenAndRetry() {
        let app = XCUIApplication()
        app.launchArguments = ["-scannerWithoutCamera"]
        app.launch()
        dismissLaunchNotice(in: app)
        let scan = app.buttons["Scan Pairing Code"]
        XCTAssertTrue(scan.waitForExistence(timeout: 5))
        let failure = app.staticTexts["Camera Couldn’t Start"]
        for cycle in 0..<8 {
            scan.tap()
            XCTAssertTrue(failure.waitForExistence(timeout: 10), "Open \(cycle) never started capture")
            app.buttons["Try Again"].tap()
            XCTAssertTrue(failure.waitForExistence(timeout: 10), "Retry \(cycle) never started capture")
            if cycle.isMultiple(of: 2) {
                app.buttons["Enter Code Manually"].tap()
            } else {
                app.navigationBars["Scan Pairing Code"].buttons["Cancel"].tap()
            }
            XCTAssertTrue(app.navigationBars["Scan Pairing Code"].waitForNonExistence(timeout: 3))
        }
    }

    func testPairingUsesInlinePasteAndKeyboardDone() {
        let app = XCUIApplication()
        app.launch()
        dismissLaunchNotice(in: app)
        let address = app.textFields["Host address and port"]
        XCTAssertTrue(address.waitForExistence(timeout: 5))
        address.tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: 3))
        XCTAssertTrue(app.buttons["Paste pairing code"].exists)
        XCTAssertTrue(app.buttons["Dismiss pairing keyboard"].exists)
        let paste = app.descendants(matching: .any)["pairingKeyboardPaste"]
        XCTAssertTrue(paste.exists, "The keyboard toolbar offers Paste beside Done")
        let keyboard = app.keyboards.firstMatch
        XCTAssertLessThanOrEqual(app.buttons["pairingKeyboardDone"].frame.maxY, keyboard.frame.minY - 8, "Toolbar buttons keep a gap above the keys")
        let screenshot = XCTAttachment(screenshot: app.screenshot())
        screenshot.name = "Pairing inline controls with keyboard"
        screenshot.lifetime = .keepAlways
        add(screenshot)
        app.buttons["Dismiss pairing keyboard"].tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForNonExistence(timeout: 3))
    }

    func testPairingKeyboardPasteFillsFocusedCode() {
        UIPasteboard.general.string = "87654321"
        let app = XCUIApplication()
        app.launch()
        dismissLaunchNotice(in: app)
        let code = app.textFields["Pairing code"]
        XCTAssertTrue(code.waitForExistence(timeout: 5))
        code.tap()
        let paste = app.descendants(matching: .any)["pairingKeyboardPaste"]
        XCTAssertTrue(paste.waitForExistence(timeout: 3))
        XCTAssertLessThanOrEqual(code.frame.maxY, paste.frame.minY, "The floating toolbar must not cover the field being edited")
        let screenshot = XCTAttachment(screenshot: app.screenshot())
        screenshot.name = "Pairing keyboard toolbar with Paste and Done"
        screenshot.lifetime = .keepAlways
        add(screenshot)
        paste.tap()
        XCTAssertEqual(code.value as? String, "8765-4321")
        XCTAssertTrue(app.keyboards.firstMatch.exists, "Pasting keeps the field focused")
    }

    func testPairingNearbyMacFocusesCodeAndKeepsConnectCompact() {
        let app = XCUIApplication()
        app.launchArguments = ["-pairingNearbyHost"]
        app.launch()
        dismissLaunchNotice(in: app)
        let mac = app.buttons.matching(NSPredicate(format: "label BEGINSWITH %@", "Fixture Mac")).firstMatch
        XCTAssertTrue(mac.waitForExistence(timeout: 5))
        let demo = app.buttons["Try the Demo"]
        XCTAssertTrue(demo.waitForExistence(timeout: 3))
        XCTAssertTrue(XCTWaiter.wait(for: [XCTNSPredicateExpectation(predicate: NSPredicate(format: "hittable == true"), object: demo)], timeout: 3) == .completed, "Demo belongs in the header")
        let connect = app.buttons["Connect"]
        XCTAssertTrue(connect.isHittable, "The complete pairing form should fit without scrolling")
        // A large control plus its row insets; taller would push the form off screen.
        XCTAssertLessThanOrEqual(connect.frame.height, 68)
        mac.tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: 3))
        XCTAssertTrue(app.keyboards.keys["1"].exists)
        XCTAssertFalse(app.keyboards.keys["a"].exists)
        XCTAssertEqual(app.textFields["Host address and port"].value as? String, "fixture.local:17879")
        let code = app.textFields["Pairing code"]
        code.typeText("01234567")
        XCTAssertEqual(code.value as? String, "0123-4567")
        app.buttons["Dismiss pairing keyboard"].tap()
        XCTAssertTrue(connect.isEnabled)
        let screenshot = XCTAttachment(screenshot: app.screenshot())
        screenshot.name = "Compact pairing form with selected Mac"
        screenshot.lifetime = .keepAlways
        add(screenshot)
    }

    func testPairingNearbyMacKeepsCodeFocusWhenChangingAddress() {
        let app = XCUIApplication()
        app.launchArguments = ["-pairingNearbyHost"]
        app.launch()
        dismissLaunchNotice(in: app)
        let code = app.textFields["Pairing code"]
        XCTAssertTrue(code.waitForExistence(timeout: 5))
        code.tap()
        code.typeText("1234")
        let mac = app.buttons.matching(NSPredicate(format: "label BEGINSWITH %@", "Fixture Mac")).firstMatch
        mac.tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: 3))
        code.typeText("01234567")
        XCTAssertEqual(code.value as? String, "0123-4567")
        XCTAssertEqual(app.textFields["Host address and port"].value as? String, "fixture.local:17879")
    }

    func testPairingNearbyPickKeepsFocusAndCaretAtTheEnd() {
        let app = XCUIApplication()
        app.launchArguments = ["-pairingNearbyHost"]
        app.launch()
        dismissLaunchNotice(in: app)
        let mac = app.buttons.matching(NSPredicate(format: "label BEGINSWITH %@", "Fixture Mac")).firstMatch
        XCTAssertTrue(mac.waitForExistence(timeout: 5))
        mac.tap()
        let code = app.textFields["Pairing code"]
        let focused = NSPredicate(format: "hasKeyboardFocus == true")
        XCTAssertTrue(XCTWaiter.wait(for: [XCTNSPredicateExpectation(predicate: focused, object: code)], timeout: 3) == .completed, "Picking a Mac lands in the code")
        code.typeText("0123")
        // Picking the same Mac again keeps the digits, the focus and the caret.
        mac.tap()
        XCTAssertTrue(XCTWaiter.wait(for: [XCTNSPredicateExpectation(predicate: focused, object: code)], timeout: 3) == .completed)
        XCTAssertTrue(app.keyboards.keys["1"].exists, "The number pad never left")
        code.typeText("4567")
        XCTAssertEqual(code.value as? String, "0123-4567")
        // Moving to the address and back by its Next key returns to the end of the code.
        let address = app.textFields["Host address and port"]
        address.tap()
        XCTAssertTrue(XCTWaiter.wait(for: [XCTNSPredicateExpectation(predicate: focused, object: address)], timeout: 3) == .completed)
        XCTAssertEqual(code.value as? String, "0123-4567")
        app.keyboards.buttons["Next:"].tap()
        XCTAssertTrue(XCTWaiter.wait(for: [XCTNSPredicateExpectation(predicate: focused, object: code)], timeout: 3) == .completed)
        code.typeText(XCUIKeyboardKey.delete.rawValue)
        XCTAssertEqual(code.value as? String, "0123-456")
    }

    func testPairingCodeCaretStaysWhereTheDigitWasTyped() {
        let app = XCUIApplication()
        app.launch()
        dismissLaunchNotice(in: app)
        let code = app.textFields["Pairing code"]
        XCTAssertTrue(code.waitForExistence(timeout: 5))
        code.tap()
        code.typeText("1234567")
        // Put the caret before the first digit.
        code.coordinate(withNormalizedOffset: CGVector(dx: 0.02, dy: 0.5)).tap()
        app.keyboards.keys["9"].tap()
        XCTAssertEqual(code.value as? String, "9123-4567")
        // A caret that jumped to the end would drop this digit from the full code.
        app.keyboards.keys["0"].tap()
        XCTAssertEqual(code.value as? String, "9012-3456")
        app.keyboards.keys["Delete"].tap()
        app.keyboards.keys["Delete"].tap()
        XCTAssertEqual(code.value as? String, "1234-56")
    }

    func testPairingInlinePasteFillsCodeAndFocusesIt() {
        UIPasteboard.general.string = " 8765-43 "
        let app = XCUIApplication()
        app.launch()
        dismissLaunchNotice(in: app)
        let code = app.textFields["Pairing code"]
        XCTAssertTrue(code.waitForExistence(timeout: 5))
        app.buttons["Paste pairing code"].tap()
        XCTAssertTrue(XCTWaiter.wait(for: [XCTNSPredicateExpectation(predicate: NSPredicate(format: "value == %@", "8765-43"), object: code)], timeout: 3) == .completed)
        // An incomplete paste leaves the caret at its end, ready for the rest.
        XCTAssertTrue(app.keyboards.keys["1"].waitForExistence(timeout: 3))
        code.typeText("21")
        XCTAssertEqual(code.value as? String, "8765-4321")
    }

    func testPairingKeyboardBarNeverCoversConnect() {
        let app = XCUIApplication()
        app.launchArguments = ["-pairingNearbyHost"]
        app.launch()
        dismissLaunchNotice(in: app)
        let connect = app.buttons["Connect"]
        XCTAssertTrue(connect.waitForExistence(timeout: 5))
        let done = app.buttons["pairingKeyboardDone"]
        for field in [app.textFields["Pairing code"], app.textFields["Host address and port"]] {
            field.tap()
            XCTAssertTrue(done.waitForExistence(timeout: 3))
            let clear = NSPredicate { _, _ in connect.frame.maxY <= done.frame.minY && field.frame.maxY <= done.frame.minY }
            XCTAssertTrue(XCTWaiter.wait(for: [XCTNSPredicateExpectation(predicate: clear, object: nil)], timeout: 3) == .completed, "Connect (\(connect.frame)) and the field stay above the keyboard bar (\(done.frame))")
        }
        let screenshot = XCTAttachment(screenshot: app.screenshot())
        screenshot.name = "Connect above the pairing keyboard bar"
        screenshot.lifetime = .keepAlways
        add(screenshot)
    }

    func testLongResponseUsesCompactPreviewAndSelectionSheet() {
        let app = XCUIApplication()
        app.launchArguments = ["-demo", "-demoSession", "idle"]
        app.launch()
        let response = app.staticTexts["Release notes"]
        XCTAssertTrue(response.waitForExistence(timeout: 5))
        response.press(forDuration: 1)
        XCTAssertTrue(app.buttons["Select Text"].waitForExistence(timeout: 3))
        let preview = XCTAttachment(screenshot: app.screenshot())
        preview.name = "Compact long-message actions"
        preview.lifetime = .keepAlways
        add(preview)
        app.buttons["Select Text"].tap()
        XCTAssertTrue(app.navigationBars["Select Text"].waitForExistence(timeout: 3))
        XCTAssertTrue(app.textViews.firstMatch.exists)
    }

    func testComposerCanDismissAndRefocusWithoutLosingDraft() {
        let app = launchChat(draft: "Keep this draft")
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: 5))
        XCTAssertFalse(app.buttons["Done"].exists)
        app.buttons["Chat actions"].tap()
        app.buttons["More"].tap()
        app.buttons["Dismiss Keyboard"].tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForNonExistence(timeout: 3))
        let field = composer(in: app)
        XCTAssertEqual(field.value as? String, "Keep this draft")
        field.tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: 3))
        XCTAssertEqual(field.value as? String, "Keep this draft")
    }

    func testEmptyComposerCompactsWhenKeyboardDismissesAndExpandsOnFocus() {
        let app = XCUIApplication()
        app.launchArguments = ["-demo", "-demoSession", "idle"]
        app.launch()
        let field = composer(in: app)
        XCTAssertTrue(field.waitForExistence(timeout: 5))
        field.tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: 5))
        let expandedHeight = app.buttons["Send message"].frame.maxY - field.frame.minY
        let screenshot = XCTAttachment(screenshot: app.screenshot())
        screenshot.name = "Expanded composer with keyboard"
        screenshot.lifetime = .keepAlways
        add(screenshot)
        app.buttons["Chat actions"].tap()
        app.buttons["More"].tap()
        app.buttons["Dismiss Keyboard"].tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForNonExistence(timeout: 3))
        XCTAssertTrue(field.waitForExistence(timeout: 3))
        let compactHeight = app.buttons["Send message"].frame.maxY - field.frame.minY
        XCTAssertLessThan(compactHeight, expandedHeight)
        let actions = app.buttons["Chat actions"].frame
        let send = app.buttons["Send message"].frame
        XCTAssertEqual(actions.minY, send.minY, accuracy: 1)
        XCTAssertEqual(actions.minX - 12, app.frame.maxX - 12 - send.maxX, accuracy: 1)
        let compactScreenshot = XCTAttachment(screenshot: app.screenshot())
        compactScreenshot.name = "Compact composer with even control insets"
        compactScreenshot.lifetime = .keepAlways
        add(compactScreenshot)
        field.tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: 3))
        XCTAssertGreaterThan(app.buttons["Send message"].frame.maxY - field.frame.minY, compactHeight)
    }

    func testStructuredAssistantSummaryIsShownAsReadableCard() {
        let app = XCUIApplication()
        app.launchArguments = ["-demo", "-demoSession", "idle", "-demoSummary"]
        app.launch()
        let summary = app.descendants(matching: .any)["assistantSummaryCard"]
        XCTAssertTrue(summary.waitForExistence(timeout: 5))
        XCTAssertEqual(summary.label, "Summary. A short recap from the demo session.")
        XCTAssertFalse(summary.label.contains("{\"summary\""))
        let screenshot = XCTAttachment(screenshot: app.screenshot())
        screenshot.name = "Structured assistant summary card"
        screenshot.lifetime = .keepAlways
        add(screenshot)
    }

    func testOpeningKeyboardPreservesReadingPosition() {
        let app = XCUIApplication()
        app.launchArguments = ["-demo", "-demoSession", "idle"]
        app.launch()
        let transcript = app.scrollViews.matching(identifier: "transcript").firstMatch
        XCTAssertTrue(transcript.waitForExistence(timeout: 5))
        transcript.swipeDown()
        transcript.swipeDown()
        let earlier = app.staticTexts["You: Try the earlier draft"]
        XCTAssertTrue(earlier.isHittable)
        let before = earlier.frame.minY
        composer(in: app).tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: 3))
        XCTAssertTrue(earlier.isHittable)
        XCTAssertEqual(earlier.frame.minY, before, accuracy: 40)
        let screenshot = XCTAttachment(screenshot: app.screenshot())
        screenshot.name = "Keyboard preserves earlier reading position"
        screenshot.lifetime = .keepAlways
        add(screenshot)
        transcript.coordinate(withNormalizedOffset: CGVector(dx: 0.95, dy: 0.2)).tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForNonExistence(timeout: 3))
    }

    func testSendDismissesKeyboardAndDeliversMessageOnce() {
        let app = launchChat(draft: "Touch send regression")
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: 5))
        let send = app.buttons["Send message"]
        XCTAssertGreaterThanOrEqual(send.frame.width, 44)
        XCTAssertGreaterThanOrEqual(send.frame.height, 44)
        send.coordinate(withNormalizedOffset: CGVector(dx: 0.1, dy: 0.5)).tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForNonExistence(timeout: 3))
        let message = app.descendants(matching: .any)["You: Touch send regression"]
        app.swipeDown()
        XCTAssertTrue(message.waitForExistence(timeout: 5))
        XCTAssertEqual(app.descendants(matching: .any).matching(identifier: "You: Touch send regression").count, 1)
    }

    func testUserMessageCanBeRetriedFromItsMenu() {
        let app = launchChat(draft: "Retry regression")
        app.buttons["Send message"].tap()
        let message = app.descendants(matching: .any)["You: Retry regression"]
        XCTAssertTrue(message.waitForExistence(timeout: 5))
        let sent = app.descendants(matching: .any).matching(identifier: "You: Retry regression")
        // Wait for the demo turn to finish so the retry starts a turn rather than queueing.
        XCTAssertTrue(app.buttons["Send message"].waitForExistence(timeout: 20))
        message.press(forDuration: 1)
        XCTAssertTrue(app.buttons["Copy"].waitForExistence(timeout: 3))
        let retry = app.buttons["Retry"]
        XCTAssertTrue(retry.waitForExistence(timeout: 3))
        retry.tap()
        let deadline = Date().addingTimeInterval(10)
        while sent.count < 2, Date() < deadline { RunLoop.current.run(until: Date().addingTimeInterval(0.25)) }
        XCTAssertEqual(sent.count, 2)
    }

    func testScrollingDismissesKeyboardAndPreservesMultilineDraft() {
        let app = launchChat(draft: "")
        let field = composer(in: app)
        field.typeText("First line\nSecond line")
        XCTAssertEqual(field.value as? String, "First line\nSecond line")
        let actions = app.buttons["Chat actions"]
        let send = app.buttons["Send message"]
        XCTAssertGreaterThanOrEqual(actions.frame.minY, field.frame.maxY - 1)
        XCTAssertGreaterThanOrEqual(send.frame.minY, field.frame.maxY - 1)
        XCTAssertEqual(actions.frame.midY, send.frame.midY, accuracy: 2)
        let transcript = app.scrollViews.matching(identifier: "transcript").firstMatch
        transcript.coordinate(withNormalizedOffset: CGVector(dx: 0.8, dy: 0.4))
            .press(forDuration: 0.05, thenDragTo: app.coordinate(withNormalizedOffset: CGVector(dx: 0.8, dy: 0.95)))
        XCTAssertTrue(app.keyboards.firstMatch.waitForNonExistence(timeout: 3))
        XCTAssertEqual(composer(in: app).value as? String, "First line\nSecond line")
    }

    func testSlashCompletionStaysAttachedToTypedPrefix() {
        let app = XCUIApplication()
        app.launchArguments = ["-demo", "-demoSession", "idle", "-demoDraft", "/sess", "-demoFocusComposer"]
        app.launch()
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: 5))
        XCTAssertTrue(app.buttons["/session"].waitForExistence(timeout: 5))
        // The panel shows whole rows only: none is cut off against the field below it.
        let panel = app.scrollViews["commandSuggestions"]
        XCTAssertTrue(panel.exists)
        for row in panel.buttons.allElementsBoundByIndex where row.isHittable {
            XCTAssertTrue(panel.frame.insetBy(dx: -1, dy: -1).contains(row.frame), "\(row.label) is clipped")
        }
        XCTAssertLessThanOrEqual(panel.frame.maxY, composer(in: app).frame.minY)
        let screenshot = XCTAttachment(screenshot: app.screenshot())
        screenshot.name = "Inline slash completion beside typed prefix"
        screenshot.lifetime = .keepAlways
        add(screenshot)
    }

    func testScrollingUpShowsLatestButtonInsteadOfFollowingOutput() {
        let app = XCUIApplication()
        app.launchArguments = ["-demo", "-demoSession", "idle"]
        app.launch()
        let transcript = app.scrollViews.matching(identifier: "transcript").firstMatch
        XCTAssertTrue(transcript.waitForExistence(timeout: 5))
        // At the tail there is nothing to catch up to.
        XCTAssertFalse(app.buttons["Latest"].exists)
        transcript.swipeDown()
        transcript.swipeDown()
        let latest = app.buttons["Latest"]
        XCTAssertTrue(latest.waitForExistence(timeout: 3))
        let screenshot = XCTAttachment(screenshot: app.screenshot())
        screenshot.name = "Latest button after scrolling up"
        screenshot.lifetime = .keepAlways
        add(screenshot)
        latest.tap()
        XCTAssertTrue(latest.waitForNonExistence(timeout: 3))
    }

    func testThinkingExpandsAndMarkdownTableIsRendered() {
        let app = XCUIApplication()
        app.launchArguments = ["-demo", "-demoSession", "idle"]
        app.launch()
        let thinking = app.buttons["Thinking · 420ms · ~96 tokens"]
        XCTAssertTrue(thinking.waitForExistence(timeout: 5))
        app.swipeDown()
        if !thinking.isHittable { app.swipeDown() }
        thinking.tap()
        let thought = app.staticTexts["I’ll group the merged changes by scope, then check the release steps."]
        XCTAssertTrue(thought.waitForExistence(timeout: 3))
        let expanded = XCTAttachment(screenshot: app.screenshot())
        expanded.name = "Expanded thinking padding"
        expanded.lifetime = .keepAlways
        add(expanded)
        thinking.tap()
        XCTAssertFalse(app.staticTexts["I’ll group the merged changes by scope, then check the release steps."].exists)
        app.swipeUp()
        XCTAssertTrue(app.descendants(matching: .any)["markdownTable"].waitForExistence(timeout: 3))
    }

    func testHistoricalTimingIsVisibleAndThoughtAggregateAppearsOnce() {
        let app = XCUIApplication()
        app.launchArguments = ["-demo", "-demoSession", "idle"]
        app.launch()
        let thinking = app.buttons["Thinking · 420ms · ~96 tokens"]
        XCTAssertTrue(thinking.waitForExistence(timeout: 5))
        XCTAssertEqual(app.buttons.matching(identifier: "Thinking · 420ms · ~96 tokens").count, 1)
        XCTAssertTrue(app.staticTexts["Response · 12s"].exists)
        // One line per turn: outcome, work time, then when it ended.
        let completed = app.staticTexts.matching(NSPredicate(format: "label BEGINSWITH %@", "Completed · Worked for 32s · "))
        XCTAssertEqual(completed.count, 1)
        XCTAssertTrue(app.staticTexts.matching(NSPredicate(format: "label BEGINSWITH %@", "Failed · Worked for 8s · ")).firstMatch.exists)
        XCTAssertFalse(app.staticTexts.matching(NSPredicate(format: "label BEGINSWITH %@", "Completed at ")).firstMatch.exists)
    }

    func testQuestionCustomAnswerAndSelectionAreExclusive() {
        let app = XCUIApplication()
        app.launchArguments = ["-demo", "-demoSession", "question", "-demoSheet"]
        app.launch()
        let custom = app.buttons["Write your own answer"]
        XCTAssertTrue(custom.waitForExistence(timeout: 5))
        custom.tap()
        let answer = app.textFields["Your answer"]
        answer.typeText("Keep my draft")
        app.buttons["Dismiss answer keyboard"].tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForNonExistence(timeout: 3))
        app.buttons.matching(NSPredicate(format: "label CONTAINS %@", "Segment files")).firstMatch.tap()
        XCTAssertFalse(app.textFields["Your answer"].exists)
        custom.tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: 3))
        XCTAssertEqual(app.textFields["Your answer"].value as? String, "Keep my draft")
        XCTAssertTrue(app.textFields["Your answer"].isHittable)
        XCTAssertLessThanOrEqual(app.textFields["Your answer"].frame.maxY, app.keyboards.firstMatch.frame.minY)
        let screenshot = XCTAttachment(screenshot: app.screenshot())
        screenshot.name = "Question custom answer and keyboard"
        screenshot.lifetime = .keepAlways
        add(screenshot)
        app.buttons["Send"].tap()
        XCTAssertTrue(app.navigationBars["Question"].waitForNonExistence(timeout: 5))
    }

    func testModelAndEffortApplyInTheSameNativeSheet() {
        let app = XCUIApplication()
        app.launchArguments = ["-demo", "-demoSession", "idle"]
        app.launch()
        let model = modelButton(in: app)
        model.tap()
        XCTAssertTrue(app.navigationBars["Model & Effort"].waitForExistence(timeout: 3))
        app.buttons["thinkingEffortPicker"].tap()
        app.buttons["High"].tap()
        XCTAssertTrue(app.navigationBars["Model & Effort"].exists)
        app.navigationBars["Model & Effort"].buttons["Done"].tap()
        XCTAssertEqual(modelButton(in: app).value as? String, "demo-model, high")
        model.tap()
        let search = app.searchFields["Search models"]
        search.tap()
        search.typeText("fast")
        app.buttons.matching(NSPredicate(format: "label BEGINSWITH %@", "demo-fast-model")).firstMatch.tap()
        XCTAssertTrue(app.navigationBars["Model & Effort"].exists)
        app.navigationBars["Model & Effort"].buttons["Done"].tap()
        XCTAssertEqual(modelButton(in: app).value as? String, "demo-fast-model, default")
        XCTAssertFalse(app.staticTexts["sessionTurnCount"].exists)
    }

    func testQuestionCanCancelWithoutSendingAnAnswer() {
        let app = XCUIApplication()
        app.launchArguments = ["-demo", "-demoSession", "question", "-demoSheet"]
        app.launch()
        let cancel = app.navigationBars["Question"].buttons["Cancel questions"]
        XCTAssertTrue(cancel.waitForExistence(timeout: 5))
        cancel.tap()
        XCTAssertTrue(app.navigationBars["Question"].waitForNonExistence(timeout: 5))
        XCTAssertFalse(app.buttons["Stop the running turn"].exists)
    }

    func testSlashSuggestionsRunHelpWithoutSendingAModelPrompt() {
        let app = launchChat(draft: "/he")
        let help = app.buttons["/help"]
        XCTAssertTrue(help.waitForExistence(timeout: 5))
        help.tap()
        app.buttons["Run command"].tap()
        XCTAssertTrue(app.staticTexts["commandOutputTitle"].waitForExistence(timeout: 5))
        XCTAssertFalse(app.staticTexts["You: /help"].exists)
    }

    func testModelChoicesCanBeInspectedWhileAQuestionIsWaiting() {
        let app = XCUIApplication()
        app.launchArguments = ["-demo", "-demoSession", "question"]
        app.launch()
        let model = app.buttons["Model and effort"]
        XCTAssertTrue(model.waitForExistence(timeout: 5))
        XCTAssertTrue(model.isEnabled)
        let dock = XCTAttachment(screenshot: app.screenshot())
        dock.name = "Question and settings in composer"
        dock.lifetime = .keepAlways
        add(dock)
        model.tap()
        XCTAssertTrue(app.buttons.matching(NSPredicate(format: "label BEGINSWITH %@", "demo-fast-model")).firstMatch.waitForExistence(timeout: 3))
        XCTAssertFalse(app.buttons.matching(NSPredicate(format: "label BEGINSWITH %@", "demo-fast-model")).firstMatch.isEnabled)
    }

    func testQuestionChainAdvancesInTheSameSheetAndCanBeCancelled() {
        let app = XCUIApplication()
        app.launchArguments = ["-demo", "-demoSession", "questions", "-demoSheet"]
        app.launch()
        for number in 1...2 {
            let progress = app.staticTexts.matching(NSPredicate(format: "label CONTAINS %@", "Question \(number) of 3")).firstMatch
            XCTAssertTrue(progress.waitForExistence(timeout: 5))
            app.buttons.matching(NSPredicate(format: "label CONTAINS %@", "Segment files")).firstMatch.tap()
            app.buttons["Next"].tap()
        }
        XCTAssertTrue(app.staticTexts.matching(NSPredicate(format: "label CONTAINS %@", "Question 3 of 3")).firstMatch.waitForExistence(timeout: 5))
        let screenshot = XCTAttachment(screenshot: app.screenshot())
        screenshot.name = "Continuous question chain"
        screenshot.lifetime = .keepAlways
        add(screenshot)
        app.navigationBars["Question"].buttons["Cancel questions"].tap()
        XCTAssertTrue(app.navigationBars["Question"].waitForNonExistence(timeout: 5))
        XCTAssertFalse(app.buttons["Stop the running turn"].exists)
    }

    func testSlashAutocompleteCompletesInputAndListScrollsWithinBounds() {
        let app = launchChat(draft: "/he")
        let complete = app.buttons["/help"]
        XCTAssertTrue(complete.waitForExistence(timeout: 5))
        complete.tap()
        XCTAssertEqual(composer(in: app).value as? String, "/help")
        app.terminate()
        app.launchArguments = ["-demo", "-demoSession", "idle", "-demoDraft", "/", "-demoFocusComposer"]
        app.launch()
        composer(in: app).tap()
        let suggestions = app.scrollViews["commandSuggestions"]
        XCTAssertTrue(suggestions.waitForExistence(timeout: 5))
        XCTAssertLessThanOrEqual(suggestions.frame.height, 180)
        suggestions.swipeUp()
        suggestions.swipeUp()
        let screenshot = XCTAttachment(screenshot: app.screenshot())
        screenshot.name = "Scrollable compact slash commands"
        screenshot.lifetime = .keepAlways
        add(screenshot)
        XCTAssertTrue(app.buttons["Run command"].isHittable)
    }

    func testSlashModelOpensSearchableNativeSheet() {
        let app = launchChat(draft: "/model")
        app.buttons["Run command"].tap()
        let search = app.searchFields["Search models"]
        XCTAssertTrue(search.waitForExistence(timeout: 5))
        XCTAssertTrue(app.navigationBars["Model & Effort"].exists)
        XCTAssertTrue(app.buttons["thinkingEffortPicker"].exists)
        let screenshot = XCTAttachment(screenshot: app.screenshot())
        screenshot.name = "Native combined model and effort sheet"
        screenshot.lifetime = .keepAlways
        add(screenshot)
        search.tap()
        search.typeText("fast")
        app.buttons.matching(NSPredicate(format: "label BEGINSWITH %@", "demo-fast-model")).firstMatch.tap()
        XCTAssertTrue(app.navigationBars["Model & Effort"].exists)
        app.navigationBars["Model & Effort"].buttons["Done"].tap()
        XCTAssertTrue(search.waitForNonExistence(timeout: 5))
        XCTAssertEqual(modelButton(in: app).value as? String, "demo-fast-model, default")
        XCTAssertFalse(app.staticTexts["You: /model"].exists)
    }

    func testHistoryCommandShowsSessionsAndNewSessionBrowsesFolders() {
        let app = launchChat(draft: "/history")
        app.buttons["Run command"].tap()
        let create = app.buttons["New"]
        XCTAssertTrue(create.waitForExistence(timeout: 5))
        create.tap()
        XCTAssertTrue(app.navigationBars["New Session"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.buttons["Home Folder"].exists)
        XCTAssertTrue(app.searchFields.firstMatch.exists, "Folders can be filtered or opened by path")
        let start = app.buttons["Start session in selected folder"]
        XCTAssertTrue(start.waitForExistence(timeout: 5))
        XCTAssertTrue(start.isHittable, "Start stays in reach at the bottom")
        XCTAssertEqual(start.value as? String, "/Users/demo")
        // Browsing keeps the action on the folder being shown.
        app.buttons["Projects"].tap()
        XCTAssertTrue(app.buttons["Back to demo"].waitForExistence(timeout: 5))
        XCTAssertEqual(start.value as? String, "/Users/demo/Projects")
        XCTAssertEqual(start.label, "Start session in selected folder")
        let screenshot = XCTAttachment(screenshot: app.screenshot())
        screenshot.name = "New session folder picker"
        screenshot.lifetime = .keepAlways
        add(screenshot)
        start.tap()
        XCTAssertTrue(app.navigationBars["New Session"].waitForNonExistence(timeout: 5))
        XCTAssertTrue(modelButton(in: app).exists, "The new session opens in the chat")
    }

    func testUnconfirmedNewSessionKeepsBrowsingAndAllowsAnotherFolder() {
        let app = XCUIApplication()
        app.launchArguments = ["-demo", "-demoSession", "idle", "-demoDraft", "/history", "-demoUnconfirmedCreate"]
        app.launch()
        let field = composer(in: app)
        XCTAssertTrue(field.waitForExistence(timeout: 5))
        field.tap()
        app.buttons["Run command"].tap()
        let create = app.buttons["New"]
        XCTAssertTrue(create.waitForExistence(timeout: 5))
        create.tap()
        let start = app.buttons["Start session in selected folder"]
        XCTAssertTrue(start.waitForExistence(timeout: 5))
        start.tap()

        // The unconfirmed request is named, with the host's words, and can be given up.
        XCTAssertTrue(app.staticTexts["Unconfirmed Session"].waitForExistence(timeout: 5) || app.staticTexts["UNCONFIRMED SESSION"].exists)
        XCTAssertTrue(app.staticTexts["session is starting; retry with the same request identifier to check it"].exists)
        XCTAssertTrue(app.buttons["Check Again"].exists)
        XCTAssertTrue(app.buttons["Cancel Request"].exists)
        XCTAssertTrue(app.navigationBars["New Session"].buttons["Cancel"].isEnabled, "The sheet can always be left")
        let screenshot = XCTAttachment(screenshot: app.screenshot())
        screenshot.name = "Unconfirmed new session"
        screenshot.lifetime = .keepAlways
        add(screenshot)

        // Browsing still works, and another folder asks before abandoning the first request.
        let projects = app.buttons["Projects"]
        if !projects.waitForExistence(timeout: 2) { app.swipeUp() }
        XCTAssertTrue(projects.waitForExistence(timeout: 5))
        projects.tap()
        XCTAssertTrue(app.buttons["Back to demo"].waitForExistence(timeout: 5))
        XCTAssertEqual(start.value as? String, "/Users/demo/Projects")
        XCTAssertTrue(start.isEnabled)
        start.tap()
        let replace = app.buttons["Start in Projects"]
        XCTAssertTrue(replace.waitForExistence(timeout: 5))
        replace.tap()
        XCTAssertTrue(app.navigationBars["New Session"].waitForNonExistence(timeout: 5))
        XCTAssertTrue(modelButton(in: app).exists, "The new session opens in the chat")
    }

    func testSessionsDrawerDismissesWithoutLosingDraftAndSelectsAnotherSession() {
        let app = launchChat(draft: "Keep my sidebar draft")
        let show = app.buttons["Show sessions"]
        XCTAssertTrue(show.waitForExistence(timeout: 5))
        show.tap()
        let close = app.navigationBars["Sessions"].buttons["Close sessions"]
        XCTAssertTrue(close.waitForExistence(timeout: 3))
        XCTAssertTrue(app.buttons["New"].isHittable)
        XCTAssertTrue(app.keyboards.firstMatch.waitForNonExistence(timeout: 3))
        let screenshot = XCTAttachment(screenshot: app.screenshot())
        screenshot.name = "Sessions drawer beside the pushed chat"
        screenshot.lifetime = .keepAlways
        add(screenshot)
        close.tap()
        XCTAssertTrue(close.waitForNonExistence(timeout: 3))
        XCTAssertEqual(composer(in: app).value as? String, "Keep my sidebar draft")
        show.tap()
        let other = app.buttons.matching(NSPredicate(format: "label CONTAINS %@", "Page the session store")).firstMatch
        XCTAssertTrue(other.waitForExistence(timeout: 3))
        other.tap()
        XCTAssertTrue(close.waitForNonExistence(timeout: 3))
        XCTAssertTrue(app.buttons.matching(NSPredicate(format: "label ENDSWITH %@", "Answer")).firstMatch.waitForExistence(timeout: 5))
        XCTAssertTrue(show.exists)
        let otherComposer = app.textFields["Message to queue"].exists ? app.textFields["Message to queue"] : app.textViews["Message to queue"]
        XCTAssertNotEqual(otherComposer.value as? String, "Keep my sidebar draft")
        show.tap()
        app.buttons.matching(NSPredicate(format: "label CONTAINS %@", "Draft release notes")).firstMatch.tap()
        XCTAssertTrue(composer(in: app).waitForExistence(timeout: 5))
        XCTAssertEqual(composer(in: app).value as? String, "Keep my sidebar draft")
    }

    func testDetachUsesRightSwipeAndKeepsSidebarOpen() {
        let app = launchChat(draft: "Detach test")
        let show = app.buttons["Show sessions"]
        XCTAssertTrue(show.waitForExistence(timeout: 5))
        show.tap()
        let close = app.navigationBars["Sessions"].buttons["Close sessions"]
        XCTAssertTrue(close.waitForExistence(timeout: 3))
        XCTAssertTrue(app.buttons["New"].isHittable)

        let session = app.buttons.matching(NSPredicate(format: "label CONTAINS %@", "Page the session store")).firstMatch
        XCTAssertTrue(session.waitForExistence(timeout: 3))
        session.tap()
        show.tap()
        let selected = app.buttons.matching(NSPredicate(format: "label CONTAINS %@", "Page the session store")).firstMatch
        selected.swipeRight()
        XCTAssertTrue(app.buttons["Detach"].waitForExistence(timeout: 3))
        XCTAssertTrue(close.isHittable)
    }

    func testSessionListCopiesSessionIDNotTitle() {
        let app = launchChat(draft: "Copy ID test")
        let show = app.buttons["Show sessions"]
        let close = app.navigationBars["Sessions"].buttons["Close sessions"]
        XCTAssertTrue(show.waitForExistence(timeout: 5))
        show.tap()
        XCTAssertTrue(close.waitForExistence(timeout: 3))

        // Dismissing the menu copies nothing.
        UIPasteboard.general.string = "untouched"
        let live = app.buttons.matching(NSPredicate(format: "label CONTAINS %@", "Page the session store")).firstMatch
        XCTAssertTrue(live.waitForExistence(timeout: 3))
        live.press(forDuration: 1)
        let copy = app.buttons["Copy Session ID"]
        XCTAssertTrue(copy.waitForExistence(timeout: 3))
        app.coordinate(withNormalizedOffset: CGVector(dx: 0.9, dy: 0.5)).tap()
        XCTAssertTrue(copy.waitForNonExistence(timeout: 3))
        XCTAssertEqual(UIPasteboard.general.string, "untouched")

        // A live session copies its host identifier, not its title or workspace.
        live.press(forDuration: 1)
        XCTAssertTrue(copy.waitForExistence(timeout: 3))
        copy.tap()
        let copied = XCTAttachment(screenshot: app.screenshot())
        copied.name = "Session ID copied from the session list"
        copied.lifetime = .keepAlways
        add(copied)
        close.tap()
        XCTAssertEqual(pasteIntoEmptyComposer(app), "demo-store")

        // A saved session copies its own identifier too.
        show.tap()
        let saved = app.buttons.matching(NSPredicate(format: "label CONTAINS %@", "Fix the flaky parser test")).firstMatch
        XCTAssertTrue(saved.waitForExistence(timeout: 5))
        saved.press(forDuration: 1)
        XCTAssertTrue(copy.waitForExistence(timeout: 3))
        copy.tap()
        XCTAssertTrue(copy.waitForNonExistence(timeout: 3))
        close.tap()
        XCTAssertEqual(pasteIntoEmptyComposer(app), "demo-saved-parser")
    }

    func testSavedSessionsOpenElsewhereSayWhereTheyAreOpen() {
        let app = launchChat(draft: "Where is it open")
        app.buttons["Show sessions"].tap()
        XCTAssertTrue(app.navigationBars["Sessions"].waitForExistence(timeout: 3))
        func row(_ title: String) -> XCUIElement {
            let row = app.buttons.matching(NSPredicate(format: "label CONTAINS %@", title)).firstMatch
            for _ in 0..<4 where !row.exists || !row.isHittable { app.swipeUp() }
            XCTAssertTrue(row.waitForExistence(timeout: 5), title)
            return row
        }
        let inApp = row("Review the sandbox profile")
        XCTAssertTrue(inApp.label.contains("open in the RustCode app on your Mac"), inApp.label)
        XCTAssertFalse(inApp.isEnabled)
        let inTerminal = row("Profile slow startup")
        XCTAssertTrue(inTerminal.label.contains("open in a terminal on your Mac"), inTerminal.label)
        XCTAssertFalse(inTerminal.isEnabled)
        let screenshot = XCTAttachment(screenshot: app.screenshot())
        screenshot.name = "Saved sessions open in the Mac app and in a terminal"
        screenshot.lifetime = .keepAlways
        add(screenshot)
    }

    func testChatActionsCopySessionID() {
        let app = launchChat(draft: "Copy ID from chat")
        app.buttons["Chat actions"].tap()
        app.buttons["More"].tap()
        let copy = app.buttons["Copy Session ID"]
        XCTAssertTrue(copy.waitForExistence(timeout: 3))
        copy.tap()
        XCTAssertTrue(app.staticTexts["Session ID Copied"].waitForExistence(timeout: 3))
        XCTAssertEqual(composer(in: app).value as? String, "Copy ID from chat")
        XCTAssertEqual(pasteIntoEmptyComposer(app), "demo-notes")
    }

    /// The test runner may not read what the app copied, so paste it over the draft instead.
    private func pasteIntoEmptyComposer(_ app: XCUIApplication) -> String? {
        let field = composer(in: app)
        XCTAssertTrue(field.waitForExistence(timeout: 5))
        field.tap()
        field.press(forDuration: 1)
        if editMenuItem("Select All", in: app).waitForExistence(timeout: 3) {
            editMenuItem("Select All", in: app).tap()
        }
        let paste = editMenuItem("Paste", in: app)
        XCTAssertTrue(paste.waitForExistence(timeout: 3))
        paste.tap()
        return field.value as? String
    }

    private func editMenuItem(_ title: String, in app: XCUIApplication) -> XCUIElement {
        app.menuItems[title].exists ? app.menuItems[title] : app.buttons[title]
    }

    func testMinimalComposerActionsPreserveDraftAndOfferSlashCommands() {
        let app = launchChat(draft: "Keep my actions draft")
        XCTAssertTrue(app.buttons["Model and effort"].exists)
        XCTAssertTrue(app.buttons["Send message"].exists)
        XCTAssertFalse(app.buttons["thinkingEffortPicker"].exists)
        app.buttons["Chat actions"].tap()
        app.buttons["More"].tap()
        app.buttons["Slash Commands"].tap()
        XCTAssertEqual(composer(in: app).value as? String, "Keep my actions draft")
        app.terminate()
        app.launchArguments = ["-demo", "-demoSession", "idle"]
        app.launch()
        XCTAssertTrue(app.buttons["Chat actions"].waitForExistence(timeout: 5))
        app.buttons["Chat actions"].tap()
        app.buttons["More"].tap()
        app.buttons["Slash Commands"].tap()
        XCTAssertEqual(composer(in: app).value as? String, "/")
        XCTAssertTrue(app.scrollViews["commandSuggestions"].waitForExistence(timeout: 3))
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: 3))
    }

    func testSidebarEdgeSwipeOpensAndClosingSwipePreservesDraft() {
        let app = launchChat(draft: "Keep my swipe draft")
        let edge = app.coordinate(withNormalizedOffset: CGVector(dx: 0.015, dy: 0.5))
        let open = app.coordinate(withNormalizedOffset: CGVector(dx: 0.78, dy: 0.5))
        edge.press(forDuration: 0.05, thenDragTo: open)
        let close = app.navigationBars["Sessions"].buttons["Close sessions"]
        XCTAssertTrue(close.waitForExistence(timeout: 4))
        XCTAssertTrue(close.isHittable)
        XCTAssertTrue(app.keyboards.firstMatch.waitForNonExistence(timeout: 3))
        let screenshot = XCTAttachment(screenshot: app.screenshot())
        screenshot.name = "Interactive sidebar and charcoal chat surface"
        screenshot.lifetime = .keepAlways
        add(screenshot)
        let from = app.coordinate(withNormalizedOffset: CGVector(dx: 0.95, dy: 0.65))
        let to = app.coordinate(withNormalizedOffset: CGVector(dx: 0.03, dy: 0.65))
        from.press(forDuration: 0.05, thenDragTo: to)
        let modelAndEffort = app.buttons["Model and effort"]
        XCTAssertTrue(modelAndEffort.waitForExistence(timeout: 4))
        expectation(for: NSPredicate(format: "hittable == true"), evaluatedWith: modelAndEffort)
        waitForExpectations(timeout: 4)
        XCTAssertFalse(close.isHittable)
        XCTAssertEqual(composer(in: app).value as? String, "Keep my swipe draft")
    }

    func testImagePickerCancellationPreservesMessageDraft() {
        let app = launchChat(draft: "Keep my image draft")
        app.buttons["Chat actions"].tap()
        XCTAssertTrue(app.buttons["Photos"].waitForExistence(timeout: 3))
        app.buttons["Photos"].tap()
        let close = app.buttons.matching(NSPredicate(format: "label IN %@", ["Cancel", "Close"])).firstMatch
        XCTAssertTrue(close.waitForExistence(timeout: 5))
        let screenshot = XCTAttachment(screenshot: app.screenshot())
        screenshot.name = "Native image picker"
        screenshot.lifetime = .keepAlways
        add(screenshot)
        close.tap()
        XCTAssertTrue(close.waitForNonExistence(timeout: 5))
        XCTAssertEqual(composer(in: app).value as? String, "Keep my image draft")
        XCTAssertTrue(app.buttons["Model and effort"].exists)
    }

    private func launchChat(draft: String) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments = ["-demo", "-demoSession", "idle", "-demoDraft", draft]
        app.launch()
        let field = composer(in: app)
        XCTAssertTrue(field.waitForExistence(timeout: 5))
        field.tap()
        return app
    }

    /// An unsigned Simulator build (`CODE_SIGNING_ALLOWED=NO`) has no keychain
    /// access, so loading a saved pairing at launch reports error -34018 in an
    /// alert over the pairing screen. XCTest's interruption handler only runs for
    /// interactions, and the tap that triggers it can be lost while the alert
    /// animates away, so close it before the test looks at or touches the form.
    private func dismissLaunchNotice(in app: XCUIApplication) {
        let notice = app.alerts["RustCode"]
        guard notice.waitForExistence(timeout: 2) else { return }
        notice.buttons["OK"].tap()
        XCTAssertTrue(notice.waitForNonExistence(timeout: 3))
    }

    /// The empty, unfocused composer is compact and hides the session controls
    /// (PR #32); focusing it brings the model and effort control back.
    private func modelButton(in app: XCUIApplication) -> XCUIElement {
        let model = app.buttons["Model and effort"]
        if !model.waitForExistence(timeout: 2) {
            let field = composer(in: app)
            XCTAssertTrue(field.waitForExistence(timeout: 5))
            field.tap()
            XCTAssertTrue(model.waitForExistence(timeout: 3))
        }
        return model
    }

    private func composer(in app: XCUIApplication) -> XCUIElement {
        let textField = app.textFields["Message"]
        return textField.exists ? textField : app.textViews["Message"]
    }
}
