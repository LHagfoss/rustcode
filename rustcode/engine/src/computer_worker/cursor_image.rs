//! The agent's cursor: an arrow in RustCode's rust colour with a white rim,
//! drawn here so it looks the same on every platform and never depends on a
//! system cursor theme.

/// The arrow's outline, tip first, in design units.
const ARROW: [(f64, f64); 7] = [
    (0.0, 0.0),
    (0.0, 17.0),
    (4.2, 13.2),
    (7.0, 19.6),
    (9.6, 18.4),
    (6.9, 12.2),
    (12.4, 12.2),
];
const FILL: [f64; 3] = [232.0, 116.0, 59.0];
const RIM: [f64; 3] = [255.0, 255.0, 255.0];
const SAMPLES: usize = 4;

/// A drawn cursor: PNG bytes, its size in pixels, and where its tip is.
pub struct CursorImage {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub tip: (f64, f64),
}

/// Draw the cursor `points` tall at `scale` pixels per point.
pub fn draw(points: f64, scale: f64) -> CursorImage {
    let unit = points * scale / 22.0;
    let rim = 1.6 * scale;
    let margin = rim + scale;
    let polygon: Vec<(f64, f64)> = ARROW
        .iter()
        .map(|(x, y)| (x * unit + margin, y * unit + margin))
        .collect();
    let width = (12.4 * unit + 2.0 * margin).ceil() as u32;
    let height = (19.6 * unit + 2.0 * margin).ceil() as u32;
    let mut pixels = vec![0u8; (width * height * 4) as usize];
    for py in 0..height {
        for px in 0..width {
            let mut color = [0.0f64; 3];
            let mut alpha = 0.0;
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let point = (
                        f64::from(px) + (sx as f64 + 0.5) / SAMPLES as f64,
                        f64::from(py) + (sy as f64 + 0.5) / SAMPLES as f64,
                    );
                    let inside = contains(&polygon, point);
                    let distance = edge_distance(&polygon, point);
                    let paint = if inside && distance > rim {
                        Some(FILL)
                    } else if distance <= rim {
                        Some(RIM)
                    } else {
                        None
                    };
                    if let Some(paint) = paint {
                        for (channel, value) in color.iter_mut().zip(paint) {
                            *channel += value;
                        }
                        alpha += 1.0;
                    }
                }
            }
            let index = ((py * width + px) * 4) as usize;
            if alpha > 0.0 {
                for channel in 0..3 {
                    pixels[index + channel] = (color[channel] / alpha).round() as u8;
                }
                pixels[index + 3] = (alpha / (SAMPLES * SAMPLES) as f64 * 255.0).round() as u8;
            }
        }
    }
    let mut png = Vec::new();
    if let Some(image) = image::RgbaImage::from_raw(width, height, pixels) {
        let _ = image.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png);
    }
    CursorImage {
        png,
        width,
        height,
        tip: (margin, margin),
    }
}

fn contains(polygon: &[(f64, f64)], (x, y): (f64, f64)) -> bool {
    let mut inside = false;
    let mut previous = polygon[polygon.len() - 1];
    for &current in polygon {
        let ((x1, y1), (x2, y2)) = (previous, current);
        if (y1 > y) != (y2 > y) && x < (x2 - x1) * (y - y1) / (y2 - y1) + x1 {
            inside = !inside;
        }
        previous = current;
    }
    inside
}

fn edge_distance(polygon: &[(f64, f64)], (x, y): (f64, f64)) -> f64 {
    let mut best = f64::MAX;
    let mut previous = polygon[polygon.len() - 1];
    for &current in polygon {
        let ((x1, y1), (x2, y2)) = (previous, current);
        let (dx, dy) = (x2 - x1, y2 - y1);
        let length = dx * dx + dy * dy;
        let t = if length == 0.0 {
            0.0
        } else {
            (((x - x1) * dx + (y - y1) * dy) / length).clamp(0.0, 1.0)
        };
        let (cx, cy) = (x1 + t * dx, y1 + t * dy);
        best = best.min(((x - cx).powi(2) + (y - cy).powi(2)).sqrt());
        previous = current;
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draws_a_png_with_a_filled_body_a_rim_and_a_clear_background() {
        let cursor = draw(22.0, 2.0);
        assert!(cursor.png.starts_with(&[0x89, b'P', b'N', b'G']));
        let image = image::load_from_memory(&cursor.png).unwrap().to_rgba8();
        assert_eq!(
            (image.width(), image.height()),
            (cursor.width, cursor.height)
        );
        assert_eq!(image.get_pixel(cursor.width - 1, 0)[3], 0);
        let body = image.get_pixel((cursor.tip.0 + 6.0) as u32, (cursor.tip.1 + 20.0) as u32);
        assert_eq!([body[0], body[1], body[2], body[3]], [232, 116, 59, 255]);
        let rim = image.get_pixel(cursor.tip.0 as u32, cursor.tip.1 as u32);
        assert!(rim[0] > 240 && rim[3] > 0);
    }
}
