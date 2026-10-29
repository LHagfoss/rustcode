import XCTest
@testable import RustCodeKit

/// The Model & Effort picker's draft and session rules (#2595).
final class ModelEffortPickerTests: XCTestCase {
    private let defaultsJSON = Data("""
    {"model": "plan", "effort": "default", "models": [
      {"name": "plan", "model": "gpt-6-luna", "efforts": ["default", "low", "high"], "fast": true},
      {"name": "local", "model": "qwen", "efforts": ["default"], "fast": false},
      {"name": "old", "model": "x", "efforts": ["default", "low"]}
    ]}
    """.utf8)

    private func sessionOptions(model: String, effort: String? = "default", fast: Bool? = false) -> ModelOptions {
        var options: [JSONValue] = [[
            "id": "model", "type": "select", "currentValue": .string(model),
            "options": [["value": "plan", "name": "plan"], ["value": "local", "name": "local"]],
        ]]
        if let effort {
            options.append(["id": "effort", "type": "select", "currentValue": .string(effort),
                            "options": [["value": "default", "name": "Off"], ["value": "low", "name": "Low"], ["value": "high", "name": "High"]]])
        }
        if let fast { options.append(["id": "fast", "type": "boolean", "currentValue": .bool(fast)]) }
        return ModelOptions(configOptions: .array(options))
    }

    func testEmptyDraftShowsFastOffForASupportedDefaultModel() throws {
        let defaults = try ModelDefaults(json: defaultsJSON)
        XCTAssertEqual(defaults.options.fast, false, "an empty draft shows Fast before any session exists")
        XCTAssertEqual(defaults.options.current, "plan")
        XCTAssertEqual(defaults.options.effort, "default")
    }

    func testUnsupportedOrUnknownModelsShowNoFastToggle() throws {
        let defaults = try ModelDefaults(json: defaultsJSON)
        XCTAssertTrue(defaults.supportsFast("plan"))
        XCTAssertFalse(defaults.supportsFast("local"))
        XCTAssertFalse(defaults.supportsFast("old"), "an engine that does not say offers no toggle")
        XCTAssertFalse(defaults.supportsFast("missing"))
        let local = defaults.draftOptions(defaults.options, switchingTo: "local", fastPick: true)
        XCTAssertNil(local.fast)
        XCTAssertFalse(local.offersFast)
        XCTAssertTrue(local.efforts.isEmpty)
        XCTAssertNil(local.effort)
        // A session's options say the same from the engine.
        XCTAssertFalse(sessionOptions(model: "local", fast: nil).offersFast)
    }

    func testDraftChoicesSurviveSwitchingModels() throws {
        let defaults = try ModelDefaults(json: defaultsJSON)
        var options = defaults.options
        options.effort = "high"
        options.fast = true
        // Away to a model without Fast and back: the Fast pick comes back.
        let away = defaults.draftOptions(options, switchingTo: "local", fastPick: true)
        let back = defaults.draftOptions(away, switchingTo: "plan", fastPick: true)
        XCTAssertEqual(back.fast, true)
        XCTAssertEqual(back.current, "plan")
        XCTAssertEqual(back.effort, "default", "local had no efforts, so the effort fell back")
        // An effort the next model also offers stays.
        let old = defaults.draftOptions(options, switchingTo: "old", fastPick: true)
        XCTAssertEqual(defaults.draftOptions(options, switchingTo: "plan", fastPick: false).effort, "high")
        XCTAssertEqual(old.effort, "default")
    }

    func testDraftSettingsAreAppliedToTheNewSessionModelFirst() {
        let draft = DraftModelSettings(model: "plan", effort: "high", fast: true)
        // The new session starts on another model, without Fast.
        var options = sessionOptions(model: "local", effort: nil, fast: nil)
        var changes: [DraftModelSettings.Change] = []
        let engine: [String: ModelOptions] = ["plan": sessionOptions(model: "plan")]
        for _ in 0..<5 {
            guard let change = draft.nextChange(for: options) else { break }
            changes.append(change)
            options = { if case .model(let m) = change { return engine[m]! } else { return change.applied(to: options) } }()
        }
        XCTAssertEqual(changes, [.model("plan"), .effort("high"), .fast(true)])
        XCTAssertNil(draft.nextChange(for: options))
    }

    func testDraftDropsChoicesTheSessionDoesNotOffer() {
        let draft = DraftModelSettings(model: "local", effort: "high", fast: true)
        XCTAssertNil(draft.nextChange(for: sessionOptions(model: "local", effort: nil, fast: nil)))
        XCTAssertNil(DraftModelSettings(model: "gone", effort: nil, fast: nil).nextChange(for: sessionOptions(model: "plan")))
        // Fast off on a draft is applied too, not only Fast on.
        XCTAssertEqual(DraftModelSettings(model: "plan", effort: "default", fast: false)
            .nextChange(for: sessionOptions(model: "plan", fast: true)), .fast(false))
    }

    func testChangesAreAddressedToTheirOwnSession() {
        // Every change names the session it was made for, never "the chat on screen".
        XCTAssertEqual(DraftModelSettings.Change.fast(true).params(sessionId: "a"),
                       ["sessionId": "a", "configId": "fast", "type": "boolean", "value": true])
        XCTAssertEqual(DraftModelSettings.Change.effort("low").params(sessionId: "b"),
                       ["sessionId": "b", "configId": "effort", "value": "low"])
        XCTAssertEqual(DraftModelSettings.Change.model("plan").params(sessionId: "c"),
                       ["sessionId": "c", "configId": "model", "value": "plan"])
    }

    func testPillDescribesOnlyWhatTheModelOffers() {
        XCTAssertEqual(ModelEffortSummary.help(model: "gpt-6-luna", effort: "High", fast: true),
                       "Model: gpt-6-luna · Reasoning: High · Fast mode: On")
        XCTAssertEqual(ModelEffortSummary.help(model: "qwen", effort: nil, fast: nil), "Model: qwen")
        XCTAssertTrue(ModelEffortSummary.fastExplanation.contains("usage"))
    }
}
