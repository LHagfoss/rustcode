import XCTest
@testable import RustCodeKit

final class StreamingRevealTests: XCTestCase {
    private let start = Date(timeIntervalSinceReferenceDate: 0)
    private func at(_ seconds: TimeInterval) -> Date { start.addingTimeInterval(seconds) }

    func testEachChunkKeepsItsOwnArrival() {
        var arrivals = StreamingArrivals()
        arrivals.textChanged(to: 10, at: at(0))
        arrivals.textChanged(to: 25, at: at(0.1))
        XCTAssertEqual(arrivals.pending.map(\.end), [10, 25])
        XCTAssertEqual(arrivals.pending.map(\.time), [at(0), at(0.1)])
        XCTAssertEqual(arrivals.settled, 0)
    }

    func testFadedChunksSettleAndNeverFadeAgain() {
        var arrivals = StreamingArrivals()
        arrivals.textChanged(to: 10, at: at(0))
        arrivals.textChanged(to: 20, at: at(0.35))
        XCTAssertEqual(arrivals.settled, 10, "the first chunk finished fading")
        XCTAssertEqual(arrivals.pending.map(\.end), [20])
        arrivals.settleAll()
        XCTAssertEqual(arrivals.settled, 20)
        XCTAssertFalse(arrivals.isAnimating)
    }

    func testShorterTextDropsChunksPastIt() {
        var arrivals = StreamingArrivals()
        arrivals.textChanged(to: 10, at: at(0))
        arrivals.textChanged(to: 18, at: at(0.05))
        // `**bold**` styled loses its four stars.
        arrivals.textChanged(to: 14, at: at(0.1))
        XCTAssertEqual(arrivals.pending.map(\.end), [10, 14])
    }

    func testUnchangedTextAddsNoChunk() {
        var arrivals = StreamingArrivals(appearingWith: 5, at: at(0))
        arrivals.textChanged(to: 5, at: at(0.1))
        XCTAssertEqual(arrivals.pending.count, 1)
        XCTAssertTrue(StreamingArrivals(appearingWith: 0, at: at(0)).pending.isEmpty)
    }

    func testRevealEasesInAndBlurIsLightAndOptional() {
        let first = StreamingArrivals.reveal(elapsed: 0, blurs: true)
        XCTAssertEqual(first.opacity, 0)
        XCTAssertEqual(first.blur, StreamingArrivals.maxBlur)
        let done = StreamingArrivals.reveal(elapsed: StreamingArrivals.fade, blurs: true)
        XCTAssertEqual(done.opacity, 1)
        XCTAssertEqual(done.blur, 0)
        XCTAssertGreaterThan(StreamingArrivals.reveal(elapsed: 0.15, blurs: false).opacity, 0.5)
        XCTAssertEqual(StreamingArrivals.reveal(elapsed: 0.1, blurs: false).blur, 0)
    }
}
