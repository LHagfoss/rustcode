import XCTest

@MainActor
final class InputAndTranscriptTests: XCTestCase {
    override func setUpWithError() throws { continueAfterFailure = false }

    func testPairingKeyboardCanAdvanceAndDismissWithoutLosingInput() {
        let app = XCUIApplication()
        app.launch()
        let address = app.textFields["Host address and port"]
        XCTAssertTrue(address.waitForExistence(timeout: 5))
        // Tap near the edge of the field, where the old padding did not focus it.
        address.coordinate(withNormalizedOffset: CGVector(dx: 0.05, dy: 0.5)).tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: 3))
        address.typeText("192.168.1.20:17879")
        app.buttons["Next"].tap()
        let code = app.textFields["Pairing code"]
        code.typeText("12345678")
        app.buttons["Done"].tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForNonExistence(timeout: 3))
        XCTAssertEqual(address.value as? String, "192.168.1.20:17879")
        XCTAssertEqual(code.value as? String, "12345678")
    }

    func testComposerCanDismissAndRefocusWithoutLosingDraft() {
        let app = launchChat(draft: "Keep this draft")
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: 5))
        XCTAssertFalse(app.buttons["Done"].exists)
        app.buttons["Dismiss keyboard"].tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForNonExistence(timeout: 3))
        let field = composer(in: app)
        XCTAssertEqual(field.value as? String, "Keep this draft")
        field.tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: 3))
        XCTAssertEqual(field.value as? String, "Keep this draft")
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

    func testScrollingDismissesKeyboardAndPreservesMultilineDraft() {
        let app = launchChat(draft: "")
        let field = composer(in: app)
        field.typeText("First line\nSecond line")
        XCTAssertEqual(field.value as? String, "First line\nSecond line")
        app.scrollViews.matching(identifier: "transcript").firstMatch.swipeDown()
        XCTAssertTrue(app.keyboards.firstMatch.waitForNonExistence(timeout: 3))
        XCTAssertEqual(composer(in: app).value as? String, "First line\nSecond line")
    }

    func testThinkingExpandsAndMarkdownTableIsRendered() {
        let app = XCUIApplication()
        app.launchArguments = ["-demo", "-demoSession", "idle"]
        app.launch()
        let thinking = app.buttons["Thinking"]
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

    private func launchChat(draft: String) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments = ["-demo", "-demoSession", "idle", "-demoDraft", draft]
        app.launch()
        let field = composer(in: app)
        XCTAssertTrue(field.waitForExistence(timeout: 5))
        field.tap()
        return app
    }

    private func composer(in app: XCUIApplication) -> XCUIElement {
        let textField = app.textFields["Message"]
        return textField.exists ? textField : app.textViews["Message"]
    }
}
