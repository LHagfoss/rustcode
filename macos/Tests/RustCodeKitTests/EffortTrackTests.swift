import XCTest
@testable import RustCodeKit

final class EffortTrackTests: XCTestCase {
    func testStopsAreEvenlySpacedWithOffFirst() {
        XCTAssertEqual(EffortTrack.fraction(of: 0, count: 5), 0)
        XCTAssertEqual(EffortTrack.fraction(of: 2, count: 5), 0.5)
        XCTAssertEqual(EffortTrack.fraction(of: 4, count: 5), 1)
        XCTAssertEqual(EffortTrack.fraction(of: 9, count: 5), 1, "an index past the end stays on the track")
        XCTAssertEqual(EffortTrack.fraction(of: 0, count: 1), 0)
    }

    func testADragSnapsToTheNearestStop() {
        XCTAssertEqual(EffortTrack.index(nearest: 0.1, count: 5), 0)
        XCTAssertEqual(EffortTrack.index(nearest: 0.13, count: 5), 1)
        XCTAssertEqual(EffortTrack.index(nearest: 0.62, count: 5), 2)
        XCTAssertEqual(EffortTrack.index(nearest: -0.4, count: 5), 0, "overshooting the start")
        XCTAssertEqual(EffortTrack.index(nearest: 1.6, count: 5), 4, "overshooting the end")
    }

    func testThePointerMapsToTheKnobsTravel() {
        // A 228 pt track with a 28 pt knob travels 200 pt.
        XCTAssertEqual(EffortTrack.fraction(atX: 14, width: 228, knob: 28), 0)
        XCTAssertEqual(EffortTrack.fraction(atX: 114, width: 228, knob: 28), 0.5)
        XCTAssertEqual(EffortTrack.fraction(atX: 400, width: 228, knob: 28), 1)
        XCTAssertEqual(EffortTrack.fraction(atX: 10, width: 20, knob: 28), 0, "no travel")
    }
}
