import CoreGraphics
import XCTest
@testable import RustCodeKit

final class AsciiBannerTests: XCTestCase {
    /// A `width` by `height` picture filled by `color(x, y)` as RGBA.
    private func pixels(_ width: Int, _ height: Int, _ color: (Int, Int) -> [UInt8]) -> [UInt8] {
        var out: [UInt8] = []
        for y in 0..<height { for x in 0..<width { out += color(x, y) } }
        return out
    }

    func testAPictureNarrowerThanThreeToOneIsCroppedToACenteredBand() {
        XCTAssertEqual(AsciiBanner.crop(width: 300, height: 300), CGRect(x: 0, y: 100, width: 300, height: 100))
        XCTAssertEqual(AsciiBanner.crop(width: 800, height: 100), CGRect(x: 0, y: 0, width: 800, height: 100))
        XCTAssertEqual(AsciiBanner.crop(width: 0, height: 10), .zero)
    }

    func testRowsFollowTheBandsAspectWithTallCells() {
        // 3:1 at 60 columns: 60 / 3 / 2 rows.
        XCTAssertEqual(AsciiBanner.rowCount(columns: 60, width: 300, height: 100), 10)
        // An 8:1 strip is a few rows.
        XCTAssertEqual(AsciiBanner.rowCount(columns: 80, width: 800, height: 100), 5)
    }

    func testLightPartsGetHeavyInkAndKeepTheirColor() {
        // Left half black, right half red.
        let rgba = pixels(60, 20) { x, _ in x < 30 ? [0, 0, 0, 255] : [255, 0, 0, 255] }
        let banner = AsciiBanner.render(rgba: rgba, width: 60, height: 20, columns: 6)
        XCTAssertEqual(banner.columns, 6)
        XCTAssertEqual(banner.rows.count, 1)
        let row = banner.rows[0]
        XCTAssertEqual(row[0].character, " ")
        XCTAssertNotEqual(row[5].character, " ")
        XCTAssertEqual(row[5].red, 1, accuracy: 0.01)
        XCTAssertEqual(row[5].green, 0, accuracy: 0.01)
        // On a light background, light is blank and dark is ink.
        let inverted = AsciiBanner.render(rgba: rgba, width: 60, height: 20, columns: 6, inverted: true)
        XCTAssertEqual(inverted.rows[0][0].character, "@")
    }

    func testSeeThroughPartsAreBlank() {
        let rgba = pixels(30, 10) { _, _ in [255, 255, 255, 0] }
        let banner = AsciiBanner.render(rgba: rgba, width: 30, height: 10, columns: 3)
        XCTAssertEqual(banner.text, "   ")
        XCTAssertEqual(AsciiBanner.render(rgba: rgba, width: 30, height: 10, columns: 3, inverted: true).text, "   ")
    }

    func testACGImageRendersUpright() throws {
        // White on top, black below, so the top row has ink and the bottom none.
        let width = 120, height = 40
        let rgba = pixels(width, height) { _, y in y < height / 2 ? [255, 255, 255, 255] : [0, 0, 0, 255] }
        let data = Data(rgba) as CFData
        let image = try XCTUnwrap(CGImage(width: width, height: height, bitsPerComponent: 8, bitsPerPixel: 32,
                                          bytesPerRow: width * 4, space: CGColorSpaceCreateDeviceRGB(),
                                          bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.premultipliedLast.rawValue),
                                          provider: try XCTUnwrap(CGDataProvider(data: data)), decode: nil,
                                          shouldInterpolate: false, intent: .defaultIntent))
        let banner = AsciiBanner.render(image, columns: 24)
        XCTAssertEqual(banner.rows.count, 4)
        XCTAssertEqual(banner.rows.first?.first?.character, "@")
        XCTAssertEqual(banner.rows.last?.first?.character, " ")
    }
}
