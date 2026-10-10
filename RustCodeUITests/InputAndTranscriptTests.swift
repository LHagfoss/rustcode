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
        app.keyboards.buttons["Next:"].tap()
        let code = app.textFields["Pairing code"]
        code.typeText("12345678")
        app.buttons["Dismiss pairing keyboard"].tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForNonExistence(timeout: 3))
        XCTAssertEqual(address.value as? String, "192.168.1.20:17879")
        XCTAssertEqual(code.value as? String, "12345678")
    }

    func testPairingUsesInlinePasteAndNoFloatingToolbar() {
        let app = XCUIApplication()
        app.launch()
        let address = app.textFields["Host address and port"]
        XCTAssertTrue(address.waitForExistence(timeout: 5))
        address.tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: 3))
        XCTAssertTrue(app.buttons["Paste pairing code"].exists)
        XCTAssertTrue(app.buttons["Dismiss pairing keyboard"].exists)
        let screenshot = XCTAttachment(screenshot: app.screenshot())
        screenshot.name = "Pairing inline controls with keyboard"
        screenshot.lifetime = .keepAlways
        add(screenshot)
        XCTAssertFalse(app.toolbars.buttons["Next"].exists)
        XCTAssertFalse(app.toolbars.buttons["Done"].exists)
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
        app.buttons["Dismiss keyboard"].tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForNonExistence(timeout: 3))
        let field = composer(in: app)
        XCTAssertEqual(field.value as? String, "Keep this draft")
        field.tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: 3))
        XCTAssertEqual(field.value as? String, "Keep this draft")
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

    func testScrollingDismissesKeyboardAndPreservesMultilineDraft() {
        let app = launchChat(draft: "")
        let field = composer(in: app)
        field.typeText("First line\nSecond line")
        XCTAssertEqual(field.value as? String, "First line\nSecond line")
        let transcript = app.scrollViews.matching(identifier: "transcript").firstMatch
        transcript.coordinate(withNormalizedOffset: CGVector(dx: 0.8, dy: 0.4))
            .press(forDuration: 0.05, thenDragTo: app.coordinate(withNormalizedOffset: CGVector(dx: 0.8, dy: 0.95)))
        XCTAssertTrue(app.keyboards.firstMatch.waitForNonExistence(timeout: 3))
        XCTAssertEqual(composer(in: app).value as? String, "First line\nSecond line")
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
        XCTAssertTrue(app.staticTexts["Completed · Worked for 32s"].exists)
        XCTAssertEqual(app.staticTexts.matching(identifier: "Completed · Worked for 32s").count, 1)
        XCTAssertTrue(app.staticTexts.matching(NSPredicate(format: "label BEGINSWITH %@", "Completed at ")).firstMatch.exists)
        XCTAssertTrue(app.staticTexts["Failed · Worked for 8s"].exists)
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

    func testModelAndEffortMenusApplyHostSettings() {
        let app = XCUIApplication()
        app.launchArguments = ["-demo", "-demoSession", "idle"]
        app.launch()
        let model = app.buttons["Model"]
        XCTAssertTrue(model.waitForExistence(timeout: 5))
        app.buttons["Thinking effort"].tap()
        app.buttons["High"].tap()
        XCTAssertEqual(app.buttons["Thinking effort"].value as? String, "high")
        model.tap()
        app.buttons["demo-fast-model"].tap()
        XCTAssertEqual(model.value as? String, "demo-fast-model")
        XCTAssertTrue(app.staticTexts["sessionTurnCount"].exists)
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
