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

    /// The viewer's margin: a 1000 × 700 window fits images into 800 × 600.
    private let window = CGSize(width: 1000, height: 700)

    func testClickTogglesBetweenFittedAndZoomed() {
        let image = CGSize(width: 1600, height: 1200)
        var zoom = ImageZoom()
        zoom.toggleZoom(fitting: image, in: bounds, window: window)
        // Fitted at 0.5 (800 wide) already passes half the window, so the
        // click goes to the image's actual pixels.
        XCTAssertEqual(zoom.scale, 1)
        zoom.toggleZoom(fitting: image, in: bounds, window: window)
        XCTAssertTrue(zoom.isFitted)
        // A pinch or ⌘+ is undone by a click too.
        zoom.magnify(from: 0.5, by: 3)
        zoom.toggleZoom(fitting: image, in: bounds, window: window)
        XCTAssertTrue(zoom.isFitted)
    }

    func testPortraitImageZoomsToHalfTheWindowWidth() {
        // Fitted by its height: 200 wide in a 1000-wide window.
        let image = CGSize(width: 1000, height: 3000)
        XCTAssertEqual(ImageZoom.fit(image, in: bounds), 0.2, accuracy: 0.0001)
        let zoomed = ImageZoom.zoomed(image, in: bounds, window: window)
        XCTAssertEqual(image.width * zoomed, 500, accuracy: 0.0001)
        // Taller than the window: the rest is reached by scrolling, nothing cropped.
        XCTAssertGreaterThan(image.height * zoomed, bounds.height)
    }

    func testSmallImageIsEnlargedToHalfTheWindowWidth() {
        let image = CGSize(width: 100, height: 80)
        XCTAssertEqual(ImageZoom.fit(image, in: bounds), 1)
        XCTAssertEqual(ImageZoom.zoomed(image, in: bounds, window: window), 5, accuracy: 0.0001)
    }

    func testWideImageAlreadyPastHalfTheWindowStillZoomsIn() {
        // Small but wide: fitted at actual size, 700 of 1000 points.
        let small = CGSize(width: 700, height: 100)
        XCTAssertEqual(ImageZoom.zoomed(small, in: bounds, window: window), 2)
        // Large landscape: twice the fitted size beats actual pixels.
        let large = CGSize(width: 1200, height: 400)
        let fitted = ImageZoom.fit(large, in: bounds)
        XCTAssertEqual(ImageZoom.zoomed(large, in: bounds, window: window), fitted * 2, accuracy: 0.0001)
        // Huge: actual pixels beat twice the fitted size.
        let huge = CGSize(width: 8000, height: 6000)
        XCTAssertEqual(ImageZoom.zoomed(huge, in: bounds, window: window), 1)
    }

    func testNarrowWindowZoomsPastTheFittedSize() {
        // A narrow window: the fitted width fills it, so half of it would shrink.
        let narrow = CGSize(width: 400, height: 900)
        let fitBounds = CGSize(width: 352, height: 868)
        let image = CGSize(width: 1200, height: 900)
        let fitted = ImageZoom.fit(image, in: fitBounds)
        XCTAssertGreaterThan(ImageZoom.zoomed(image, in: fitBounds, window: narrow), fitted)
    }

    func testZoomedScaleStaysInRange() {
        XCTAssertEqual(ImageZoom.zoomed(CGSize(width: 1, height: 1), in: bounds, window: window), ImageZoom.range.upperBound)
        XCTAssertEqual(ImageZoom.zoomed(.zero, in: bounds, window: window), 2)
    }
}
