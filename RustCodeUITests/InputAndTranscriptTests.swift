import XCTest

@MainActor
final class InputAndTranscriptTests: XCTestCase {
    override func setUpWithError() throws { continueAfterFailure = false }

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
        app.buttons.matching(NSPredicate(format: "label BEGINSWITH %@", "demo-fast-model")).firstMatch.tap()
        XCTAssertEqual(model.value as? String, "demo-fast-model")
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
        let model = app.buttons["Model"]
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
        let complete = app.buttons["Complete command"]
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

    func testSlashModelOpensBoundedSearchablePicker() {
        let app = launchChat(draft: "/model")
        app.buttons["Run command"].tap()
        let search = app.textFields["Search models"]
        XCTAssertTrue(search.waitForExistence(timeout: 5))
        let choices = app.scrollViews["modelChoices"]
        XCTAssertLessThanOrEqual(choices.frame.height, 300)
        let screenshot = XCTAttachment(screenshot: app.screenshot())
        screenshot.name = "Bounded model picker"
        screenshot.lifetime = .keepAlways
        add(screenshot)
        search.tap()
        search.typeText("fast")
        app.buttons.matching(NSPredicate(format: "label BEGINSWITH %@", "demo-fast-model")).firstMatch.tap()
        XCTAssertTrue(search.waitForNonExistence(timeout: 5))
        XCTAssertEqual(app.buttons["Model"].value as? String, "demo-fast-model")
        XCTAssertFalse(app.staticTexts["You: /model"].exists)
    }

    func testHistoryCommandShowsSessionsAndNewSessionBrowsesFolders() {
        let app = launchChat(draft: "/history")
        app.buttons["Run command"].tap()
        let create = app.buttons["New Session"]
        XCTAssertTrue(create.waitForExistence(timeout: 5))
        create.tap()
        let path = app.textFields["Session folder path"]
        XCTAssertTrue(path.waitForExistence(timeout: 5))
        XCTAssertTrue(app.buttons["Home Folder"].exists)
        let start = app.buttons["Start session in selected folder"]
        XCTAssertTrue(start.waitForExistence(timeout: 5))
        let screenshot = XCTAttachment(screenshot: app.screenshot())
        screenshot.name = "New session folder picker"
        screenshot.lifetime = .keepAlways
        add(screenshot)
        start.tap()
        XCTAssertTrue(app.navigationBars["New Session"].waitForNonExistence(timeout: 5))
        XCTAssertTrue(app.buttons["Model"].waitForExistence(timeout: 5))
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
