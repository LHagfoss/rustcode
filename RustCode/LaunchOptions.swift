import Foundation
import RustCodeRemote

/// Launch arguments for demos and screenshots.
///
/// - `-demo`: start in demo mode instead of the pairing screen.
/// - `-demoSession approval|question|idle`: open that demo session straight away.
/// - `-demoSheet`: present the session's pending question or approval once it appears.
struct LaunchOptions {
    var startsDemo = false
    var demoSessionID: String?
    var presentsAttention = false

    init(arguments: [String]) {
        startsDemo = arguments.contains("-demo")
        presentsAttention = arguments.contains("-demoSheet")
        if let index = arguments.firstIndex(of: "-demoSession"), arguments.indices.contains(index + 1) {
            demoSessionID = switch arguments[index + 1] {
            case "approval": DemoRemoteClient.approvalSessionID
            case "question": DemoRemoteClient.questionSessionID
            case "idle": DemoRemoteClient.idleSessionID
            default: nil
            }
        }
    }
}
