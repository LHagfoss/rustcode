import XCTest
@testable import RustCodeKit

final class StreamPacingTests: XCTestCase {
    /// Bursts with pauses between them play out as an even write: no frame
    /// jumps far ahead of the one before, and the text never stalls.
    func testBurstsPlayOutEvenly() {
        var pacer = StreamPacer(showing: 0)
        let start = Date()
        var steps: [Int] = []
        var last = 0
        var received = 0
        for frame in 1...120 {
            let time = start.addingTimeInterval(Double(frame) / 60)
            // 120 characters every quarter second: 480 a second, in bursts.
            if frame % 15 == 0 { received += 120; pacer.received(received, at: time) }
            let shown = pacer.advance(by: 1.0 / 60, streaming: true)
            if frame > 30 { steps.append(shown - last) }
            last = shown
        }
        XCTAssertLessThan(steps.max() ?? 0, 40, "no burst lands in one frame")
        XCTAssertGreaterThan(steps.filter { $0 > 0 }.count, steps.count * 3 / 4, "the write keeps moving between bursts")
    }

    /// Once the stream ends the rest is shown within about the drain time.
    func testTheRestDrainsQuicklyAtTheEnd() {
        var pacer = StreamPacer(showing: 0)
        let start = Date()
        pacer.received(100, at: start)
        pacer.received(900, at: start.addingTimeInterval(0.1))
        pacer.advance(by: 0.05, streaming: true)
        XCTAssertFalse(pacer.isCaughtUp)
        for _ in 0..<Int(StreamPacer.drain * 60) + 2 { pacer.advance(by: 1.0 / 60, streaming: false) }
        XCTAssertTrue(pacer.isCaughtUp)
    }

    /// Text already there when the reply appears is not typed out again, and
    /// a huge jump is shown mostly at once.
    func testExistingAndHugeTextShowAtOnce() {
        var pacer = StreamPacer(showing: 500)
        XCTAssertTrue(pacer.isCaughtUp)
        pacer.received(500 + 20_000, at: Date())
        XCTAssertGreaterThanOrEqual(Int(pacer.shown), 20_500 - StreamPacer.maximumBacklog)
    }

    func testPrefixEndsOnAWord() {
        XCTAssertEqual("hello world again".pacedPrefix(2), "hello")
        XCTAssertEqual("hello world".pacedPrefix(5), "hello")
        XCTAssertEqual("hi".pacedPrefix(10), "hi")
    }
}
