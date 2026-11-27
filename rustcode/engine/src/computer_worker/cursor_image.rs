//! The agent's cursor: a rounded arrow in RustCode's rust colour with a white
//! rim and a soft shadow, drawn here so it looks the same on every platform
//! and never depends on a system cursor theme.
//!
//! The outline is the default cursor of Cua Driver
//! (<https://github.com/trycua/cua>, MIT License, Copyright (c) 2025 Cua AI,
//! Inc.), stood upright like the system pointer.

/// One cubic Bézier vertex: (vertex, in-tangent, out-tangent), the tangents
/// relative to their vertex.
type Vertex = ((f64, f64), (f64, f64), (f64, f64));

/// The outline, tip first, in design units with the tip at the origin.
const OUTLINE: [Vertex; 8] = [
    ((0.0, 0.0), (0.0, 0.0), (-5.89, -4.27)),
    ((-15.04, 6.23), (1.8, -7.86), (0.0, 0.0)),
    ((-14.8, 66.98), (0.0, 0.0), (0.08, 8.54)),
    ((-2.93, 72.36), (-6.15, 5.21), (0.0, 0.0)),
    ((12.37, 56.65), (0.0, 0.0), (3.25, -3.07)),
    ((23.91, 51.27), (-4.44, 0.51), (0.0, 0.0)),
    ((38.52, 49.14), (0.0, 0.0), (7.95, -1.36)),
    ((42.02, 36.58), (5.55, 5.21), (0.0, 0.0)),
];
const OUTLINE_HEIGHT: f64 = 72.36;
const CURVE_STEPS: usize = 12;
const OUTLINE_SAMPLES: usize = 480;
const ROUNDING: usize = 9;
const FILL: [f64; 3] = [232.0, 116.0, 59.0];
const RIM: [f64; 3] = [255.0, 255.0, 255.0];
const SHADOW: [f64; 3] = [0.0, 0.0, 0.0];
const RIM_POINTS: f64 = 0.9;
const SHADOW_BLUR_POINTS: f64 = 3.0;
const SHADOW_DROP_POINTS: f64 = 1.0;
const SHADOW_ALPHA: f64 = 0.28;
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
    let unit = points * scale / OUTLINE_HEIGHT;
    let rim = RIM_POINTS * scale;
    let blur = SHADOW_BLUR_POINTS * scale;
    let drop = SHADOW_DROP_POINTS * scale;
    let margin = rim + blur + drop + scale;
    let outline = outline();
    let (min_x, min_y, max_x, max_y) = bounds(&outline);
    let polygon: Vec<(f64, f64)> = outline
        .iter()
        .map(|(x, y)| ((x - min_x) * unit + margin, (y - min_y) * unit + margin))
        .collect();
    let width = ((max_x - min_x) * unit + 2.0 * margin).ceil() as u32;
    let height = ((max_y - min_y) * unit + 2.0 * margin).ceil() as u32;
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
                    let (paint, coverage) = paint_at(&polygon, point, rim, blur, drop);
                    for (channel, value) in color.iter_mut().zip(paint) {
                        *channel += value * coverage;
                    }
                    alpha += coverage;
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
        tip: (-min_x * unit + margin, -min_y * unit + margin),
    }
}

/// The paint and its coverage (0..=1) at one sample: the body, then the rim
/// straddling the outline, then the shadow below and around both.
fn paint_at(
    polygon: &[(f64, f64)],
    point: (f64, f64),
    rim: f64,
    blur: f64,
    drop: f64,
) -> ([f64; 3], f64) {
    let inside = contains(polygon, point);
    let distance = edge_distance(polygon, point);
    if inside && distance > rim {
        return (FILL, 1.0);
    }
    if distance <= rim {
        return (RIM, 1.0);
    }
    let lifted = (point.0, point.1 - drop);
    let beyond = if contains(polygon, lifted) {
        0.0
    } else {
        (edge_distance(polygon, lifted) - rim).max(0.0)
    };
    let fade = (1.0 - beyond / blur).max(0.0);
    (SHADOW, SHADOW_ALPHA * fade * fade)
}

/// The outline as a polygon with its corners rounded further: sampled evenly
/// along its length, then each point averaged with its neighbours, and moved
/// so the rounded tip sits on the hotspot.
fn outline() -> Vec<(f64, f64)> {
    let even = resample(&flatten(), OUTLINE_SAMPLES);
    let count = even.len();
    let smooth: Vec<(f64, f64)> = (0..count)
        .map(|index| {
            let (mut x, mut y) = (0.0, 0.0);
            for offset in 0..=2 * ROUNDING {
                let (px, py) = even[(index + count + offset - ROUNDING) % count];
                x += px;
                y += py;
            }
            let span = (2 * ROUNDING + 1) as f64;
            (x / span, y / span)
        })
        .collect();
    let tip = smooth[0];
    smooth
        .into_iter()
        .map(|(x, y)| (x - tip.0, y - tip.1))
        .collect()
}

/// `count` points spaced evenly along the closed polygon, starting at its first.
fn resample(points: &[(f64, f64)], count: usize) -> Vec<(f64, f64)> {
    let segment = |index: usize| {
        let (a, b) = (points[index], points[(index + 1) % points.len()]);
        (a, b, (b.0 - a.0).hypot(b.1 - a.1))
    };
    let perimeter: f64 = (0..points.len()).map(|index| segment(index).2).sum();
    let step = perimeter / count as f64;
    let mut out = Vec::with_capacity(count);
    let (mut index, mut walked) = (0, 0.0);
    for sample in 0..count {
        let wanted = sample as f64 * step;
        while walked + segment(index).2 < wanted && index + 1 < points.len() {
            walked += segment(index).2;
            index += 1;
        }
        let (a, b, length) = segment(index);
        let t = if length == 0.0 {
            0.0
        } else {
            ((wanted - walked) / length).clamp(0.0, 1.0)
        };
        out.push((a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t));
    }
    out
}

fn flatten() -> Vec<(f64, f64)> {
    let mut points = Vec::with_capacity(OUTLINE.len() * CURVE_STEPS);
    for (index, &(from, _, out)) in OUTLINE.iter().enumerate() {
        let (to, into, _) = OUTLINE[(index + 1) % OUTLINE.len()];
        let first = (from.0 + out.0, from.1 + out.1);
        let second = (to.0 + into.0, to.1 + into.1);
        for step in 0..CURVE_STEPS {
            let t = step as f64 / CURVE_STEPS as f64;
            let u = 1.0 - t;
            let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
            points.push((
                a * from.0 + b * first.0 + c * second.0 + d * to.0,
                a * from.1 + b * first.1 + c * second.1 + d * to.1,
            ));
        }
    }
    points
}

fn bounds(points: &[(f64, f64)]) -> (f64, f64, f64, f64) {
    points.iter().fold(
        (f64::MAX, f64::MAX, f64::MIN, f64::MIN),
        |(min_x, min_y, max_x, max_y), &(x, y)| {
            (min_x.min(x), min_y.min(y), max_x.max(x), max_y.max(y))
        },
    )
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
        let cursor = draw(24.0, 2.0);
        assert!(cursor.png.starts_with(&[0x89, b'P', b'N', b'G']));
        let image = image::load_from_memory(&cursor.png).unwrap().to_rgba8();
        assert_eq!(
            (image.width(), image.height()),
            (cursor.width, cursor.height)
        );
        assert_eq!(image.get_pixel(cursor.width - 1, 0)[3], 0);
        let unit = 24.0 * 2.0 / OUTLINE_HEIGHT;
        let body = image.get_pixel(
            (cursor.tip.0 + 5.0 * unit) as u32,
            (cursor.tip.1 + 40.0 * unit) as u32,
        );
        assert_eq!([body[0], body[1], body[2], body[3]], [232, 116, 59, 255]);
        let rim = image.get_pixel(cursor.tip.0 as u32, cursor.tip.1 as u32);
        assert!(rim[0] > 200 && rim[3] > 0);
    }

    #[test]
    fn rounding_keeps_the_tip_on_the_hotspot_and_the_size_close() {
        let rounded = outline();
        assert_eq!(rounded[0], (0.0, 0.0));
        let (min_x, min_y, max_x, max_y) = bounds(&rounded);
        assert!(min_y > -3.0);
        assert!((max_y - min_y - OUTLINE_HEIGHT).abs() < 6.0);
        assert!(max_x - min_x > 50.0);
    }

    #[test]
    fn the_outline_is_closed_and_its_tip_is_its_top_left_point() {
        let outline = flatten();
        assert_eq!(outline[0], (0.0, 0.0));
        let (_, min_y, _, max_y) = bounds(&outline);
        assert!(min_y > -3.0 && min_y <= 0.0);
        assert!(max_y > OUTLINE_HEIGHT - 1.0 && max_y < OUTLINE_HEIGHT + 4.0);
    }
}
