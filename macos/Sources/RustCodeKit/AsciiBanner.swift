import CoreGraphics
import Foundation

/// A wide picture drawn in text for the new thread screen: each cell is a
/// character whose ink matches the brightness of that part of the picture,
/// in the picture's color there. Set in Settings; the picture is cropped to
/// at least `minAspect` wide, so the banner stays a band and not a block.
public struct AsciiBanner: Equatable, Sendable {
    public struct Cell: Equatable, Sendable {
        public var character: Character
        /// 0...1 each.
        public var red: Double
        public var green: Double
        public var blue: Double
    }

    public var rows: [[Cell]]

    public var columns: Int { rows.first?.count ?? 0 }

    /// Lightest ink first. A blank cell is a dark part of the picture on a
    /// dark background, or a light part on a light one (`inverted`).
    public static let ramp: [Character] = Array(" .:-=+*#%@")

    /// Width over height the banner is cropped to at least.
    public static let minAspect = 3.0

    /// A monospaced cell is about twice as tall as wide.
    public static let cellAspect = 2.0

    /// The rows of text the banner takes at `columns` for a picture
    /// `width` by `height`, after cropping.
    public static func rowCount(columns: Int, width: Int, height: Int) -> Int {
        let crop = crop(width: width, height: height)
        let aspect = Double(crop.width) / Double(max(crop.height, 1))
        return max(1, Int((Double(columns) / aspect / cellAspect).rounded()))
    }

    /// The centered band of a `width` by `height` picture at least
    /// `minAspect` wide; a picture already wide enough is kept whole.
    public static func crop(width: Int, height: Int) -> CGRect {
        guard width > 0, height > 0 else { return .zero }
        let tallest = Int((Double(width) / minAspect).rounded(.down))
        guard height > tallest else { return CGRect(x: 0, y: 0, width: width, height: height) }
        let top = (height - tallest) / 2
        return CGRect(x: 0, y: top, width: width, height: max(tallest, 1))
    }

    /// The banner of RGBA pixels (`width` by `height`, 4 bytes each, rows
    /// top to bottom), `columns` characters wide. `inverted` maps light to
    /// blank, for a light background.
    public static func render(rgba: [UInt8], width: Int, height: Int, columns: Int,
                              inverted: Bool = false) -> AsciiBanner {
        guard width > 0, height > 0, columns > 0, rgba.count >= width * height * 4 else { return AsciiBanner(rows: []) }
        let crop = crop(width: width, height: height)
        let rowCount = rowCount(columns: columns, width: width, height: height)
        let cellWidth = crop.width / Double(columns)
        let cellHeight = crop.height / Double(rowCount)
        var rows: [[Cell]] = []
        rows.reserveCapacity(rowCount)
        for row in 0..<rowCount {
            var cells: [Cell] = []
            cells.reserveCapacity(columns)
            let y0 = Int(crop.minY + Double(row) * cellHeight)
            let y1 = max(y0 + 1, min(height, Int(crop.minY + Double(row + 1) * cellHeight)))
            for column in 0..<columns {
                let x0 = Int(Double(column) * cellWidth)
                let x1 = max(x0 + 1, min(width, Int(Double(column + 1) * cellWidth)))
                var red = 0.0, green = 0.0, blue = 0.0, alpha = 0.0, count = 0.0
                for y in y0..<y1 {
                    var index = (y * width + x0) * 4
                    for _ in x0..<x1 {
                        let a = Double(rgba[index + 3]) / 255
                        red += Double(rgba[index]) / 255 * a
                        green += Double(rgba[index + 1]) / 255 * a
                        blue += Double(rgba[index + 2]) / 255 * a
                        alpha += a
                        count += 1
                        index += 4
                    }
                }
                // Premultiplied sums: a see-through part reads as background.
                let coverage = alpha / count
                let r = alpha > 0 ? red / alpha : 0, g = alpha > 0 ? green / alpha : 0, b = alpha > 0 ? blue / alpha : 0
                let luminance = (0.2126 * r + 0.7152 * g + 0.0722 * b) * coverage
                let ink = inverted ? (1 - luminance) * coverage : luminance
                let step = min(ramp.count - 1, Int(ink * Double(ramp.count)))
                cells.append(Cell(character: ramp[step], red: r, green: g, blue: b))
            }
            rows.append(cells)
        }
        return AsciiBanner(rows: rows)
    }

    /// The banner of `image`, `columns` characters wide.
    public static func render(_ image: CGImage, columns: Int, inverted: Bool = false) -> AsciiBanner {
        // Drawn small first: a cell needs only a few pixels, and a photo
        // straight from a camera is tens of millions.
        let scale = min(1, Double(columns * 8) / Double(max(image.width, 1)))
        let width = max(1, Int(Double(image.width) * scale))
        let height = max(1, Int(Double(image.height) * scale))
        var pixels = [UInt8](repeating: 0, count: width * height * 4)
        let drawn = pixels.withUnsafeMutableBytes { buffer -> Bool in
            guard let context = CGContext(data: buffer.baseAddress, width: width, height: height, bitsPerComponent: 8,
                                          bytesPerRow: width * 4, space: CGColorSpaceCreateDeviceRGB(),
                                          bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return false }
            context.interpolationQuality = .medium
            context.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))
            return true
        }
        guard drawn else { return AsciiBanner(rows: []) }
        // The context holds premultiplied color; `render` wants it straight.
        for index in stride(from: 0, to: pixels.count, by: 4) {
            let alpha = Double(pixels[index + 3])
            guard alpha > 0, alpha < 255 else { continue }
            for channel in 0..<3 {
                pixels[index + channel] = UInt8(min(255, Double(pixels[index + channel]) * 255 / alpha))
            }
        }
        return render(rgba: pixels, width: width, height: height, columns: columns, inverted: inverted)
    }

    /// The banner as plain text, a line per row.
    public var text: String { rows.map { String($0.map(\.character)) }.joined(separator: "\n") }
}
