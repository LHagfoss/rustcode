import Foundation
import RustCodeRemote

/// Launch arguments for demos and screenshots.
///
/// - `-demo`: start in demo mode instead of the pairing screen.
/// - `-demoSession approval|question|questions|idle`: open that demo session straight away.
/// - `-demoSheet`: present the session's pending question or approval once it appears.
///
/// Debug builds only, for screenshots of states the Simulator cannot be driven into:
/// - `-demoFocusComposer`: focus the message field, which brings up the keyboard.
/// - `-demoDraft <text>`: start with that text in the message field.
/// - `-demoSelectText`: present the Select Text sheet for the first message.
/// - `-demoExpandTools`: show tool rows expanded.
/// - `-demoSettings`: present Settings.
/// - `-demoSelectOption`: select the first option of a presented question.
/// - `-demoSummary`: show the structured assistant summary presentation.
struct LaunchOptions {
    static let current = LaunchOptions(arguments: ProcessInfo.processInfo.arguments)

    var startsDemo = false
    var demoSessionID: String?
    var presentsAttention = false
    var focusesComposer = false
    var composerDraft: String?
    var presentsSelectText = false
    var expandsTools = false
    var presentsSettings = false
    var selectsFirstOption = false
    var leavesFirstCreateUnconfirmed = false
    var showsSummaryDemo = false

    init(arguments: [String]) {
        startsDemo = arguments.contains("-demo")
        presentsAttention = arguments.contains("-demoSheet")
        if let index = arguments.firstIndex(of: "-demoSession"), arguments.indices.contains(index + 1) {
            demoSessionID = switch arguments[index + 1] {
            case "approval": DemoRemoteClient.approvalSessionID
            case "question": DemoRemoteClient.questionSessionID
            case "questions": DemoRemoteClient.questionsSessionID
            case "idle": DemoRemoteClient.idleSessionID
            default: nil
            }
        }
        #if DEBUG
        focusesComposer = arguments.contains("-demoFocusComposer")
        presentsSelectText = arguments.contains("-demoSelectText")
        expandsTools = arguments.contains("-demoExpandTools")
        presentsSettings = arguments.contains("-demoSettings")
        selectsFirstOption = arguments.contains("-demoSelectOption")
        leavesFirstCreateUnconfirmed = arguments.contains("-demoUnconfirmedCreate")
        showsSummaryDemo = arguments.contains("-demoSummary")
        if let index = arguments.firstIndex(of: "-demoDraft"), arguments.indices.contains(index + 1) {
            composerDraft = arguments[index + 1]
        }
        #endif
    }
}
