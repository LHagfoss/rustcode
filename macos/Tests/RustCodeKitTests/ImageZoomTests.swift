import CoreGraphics
import XCTest
@testable import RustCodeKit

final class ImageZoomTests: XCTestCase {
    private let bounds = CGSize(width: 800, height: 600)

    func testFitShrinksLargeImagesAndNeverEnlargesSmallOnes() {
        XCTAssertEqual(ImageZoom.fit(CGSize(width: 1600, height: 600), in: bounds), 0.5)
        XCTAssertEqual(ImageZoom.fit(CGSize(width: 400, height: 1200), in: bounds), 0.5)
        XCTAssertEqual(ImageZoom.fit(CGSize(width: 100, height: 100), in: bounds), 1)
        XCTAssertEqual(ImageZoom.fit(.zero, in: bounds), 1)
    }

    func testZoomStepsFromTheFittedScaleAndStaysInRange() {
        let image = CGSize(width: 1600, height: 1200)
        var zoom = ImageZoom()
        XCTAssertTrue(zoom.isFitted)
        zoom.zoomIn(fitting: image, in: bounds)
        XCTAssertEqual(zoom.scale!, 0.5 * ImageZoom.step, accuracy: 0.0001)
        zoom.zoomOut(fitting: image, in: bounds)
        XCTAssertEqual(zoom.scale!, 0.5, accuracy: 0.0001)
        zoom.magnify(from: 1, by: 1000)
        XCTAssertEqual(zoom.scale, ImageZoom.range.upperBound)
        zoom.fit()
        XCTAssertEqual(zoom.current(fitting: image, in: bounds), 0.5)
    }

    func testDoubleClickTogglesActualSize() {
        var zoom = ImageZoom()
        zoom.toggleActualSize()
        XCTAssertEqual(zoom.scale, 1)
        zoom.toggleActualSize()
        XCTAssertTrue(zoom.isFitted)
    }
}
