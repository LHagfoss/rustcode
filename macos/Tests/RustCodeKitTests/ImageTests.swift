import Foundation
import XCTest
@testable import RustCodeKit

final class ImageTests: XCTestCase {
    func testMarkersSplitFromTextInOrder() {
        let text = "compare these\n![image](file:///a/one.png)\n![image](file:///a/two%20x.jpg)\n"
        let message = ImageMarkers.split(text)
        XCTAssertEqual(message.text, "compare these")
        XCTAssertEqual(message.images.map(\.path), ["/a/one.png", "/a/two%20x.jpg"])
    }

    func testImageOnlyMessageAndPlainTextAreKept() {
        XCTAssertEqual(ImageMarkers.split("![image](file:///a.png)").text, "")
        XCTAssertEqual(ImageMarkers.split("plain ![image](text").text, "plain ![image](text")
        XCTAssertEqual(ImageMarkers.split("![image](file://relative)").images, [])
        let marker = ImageMarkers.marker(for: URL(fileURLWithPath: "/tmp/x.png"))
        XCTAssertEqual(ImageMarkers.split(marker).images, [URL(fileURLWithPath: "/tmp/x.png")])
    }

    func testTitlesHideMarkersEvenWhenCutOff() {
        XCTAssertEqual(ImageMarkers.title("![image](file:///Users/lagos/.config"), "Image")
        XCTAssertEqual(ImageMarkers.title("what is this\n![image](file:///a.png)"), "what is this")
        XCTAssertEqual(ImageMarkers.title("fix the build"), "fix the build")
    }

    func testPromptSendsTextThenImageBlocks() {
        let image = ImageAttachment(data: Data([1, 2, 3]), mimeType: "image/png")
        let params = ACPRequests.prompt(sessionId: "s", text: "look", images: [image])
        XCTAssertEqual(params["prompt"], [
            ["type": "text", "text": "look"],
            ["type": "image", "mimeType": "image/png", "data": "AQID"],
        ])
        let imageOnly = ACPRequests.prompt(sessionId: "s", text: "", images: [image])
        XCTAssertEqual(imageOnly["prompt"], [["type": "image", "mimeType": "image/png", "data": "AQID"]])
        XCTAssertEqual(ACPRequests.prompt(sessionId: "s", text: "hi")["prompt"], [["type": "text", "text": "hi"]])
    }

    func testMimeTypesAndCapability() throws {
        XCTAssertEqual(ImageAttachment.mimeType(forExtension: "JPG"), "image/jpeg")
        XCTAssertEqual(ImageAttachment.mimeType(forExtension: "webp"), "image/webp")
        XCTAssertNil(ImageAttachment.mimeType(forExtension: "heic"))
        XCTAssertEqual(ImageAttachment(data: Data(), mimeType: "image/gif").fileExtension, "gif")
        XCTAssertFalse(try AgentInfo(initializeResult: ["protocolVersion": 1]).promptImages)
        XCTAssertTrue(try AgentInfo(initializeResult: [
            "protocolVersion": 1, "agentCapabilities": ["promptCapabilities": ["image": true]],
        ]).promptImages)
    }

    func testImageOnlyLinesBecomeImageBlocks() {
        XCTAssertEqual(MarkdownBlock.parse("Here:\n\n![chart](https://x.dev/c.png)\n![](file:///tmp/a.png)"), [
            .paragraph("Here:"),
            .image(alt: "chart", url: URL(string: "https://x.dev/c.png")!),
            .image(alt: "", url: URL(string: "file:///tmp/a.png")!),
        ])
        XCTAssertEqual(MarkdownBlock.parse("see ![x](https://x.dev/c.png)"), [.paragraph("see ![x](https://x.dev/c.png)")])
        XCTAssertEqual(MarkdownBlock.parse("![x](javascript:alert)"), [.paragraph("![x](javascript:alert)")])
    }
}

final class ModelDefaultsTests: XCTestCase {
    func testDefaultsBecomeNewChatOptions() throws {
        let json = Data("""
        {"model":"deep","effort":"high","models":[
          {"name":"fast","model":"fast-1","efforts":["default"]},
          {"name":"deep","model":"deep-1","efforts":["default","low","high"]}]}
        """.utf8)
        let defaults = try ModelDefaults(json: json)
        let options = defaults.options
        XCTAssertEqual(options.current, "deep")
        XCTAssertEqual(options.effort, "high")
        XCTAssertEqual(options.efforts.map(\.name), ["Off", "Low", "High"])
        XCTAssertEqual(options.choices.map(\.id), ["fast", "deep"])
        XCTAssertTrue(defaults.efforts(for: "fast").isEmpty)
    }
}

final class NotificationTextTests: XCTestCase {
    func testSnippetTakesTheFirstProseLine() {
        XCTAssertEqual(notificationSnippet("\n```swift\nlet a = 1\n```"), "let a = 1")
        XCTAssertEqual(notificationSnippet("**Done.** Tests pass.\nMore"), "Done. Tests pass.")
        XCTAssertEqual(notificationSnippet(String(repeating: "a", count: 200), limit: 10), "aaaaaaaaa…")
    }
}
