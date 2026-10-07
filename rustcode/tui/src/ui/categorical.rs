//! Theme-derived categorical ramp for the `/context` usage grid (#1537).
//!
//! The `/context` panel draws eight roles: seven usage categories plus the
//! free/headroom block. Mapping them straight onto theme tokens looked fine on
//! paper but shipped palettes only expose four or five distinct hues, so two
//! or three roles resolved to the same value in most themes — `sky` mapped both
//! `secondary` and `green` to `#88c438`, and `dracula`/`tokyo-night` mapped
//! `muted` and `turn_separator` to the same colour, which made filled and empty
//! blocks indistinguishable by colour and left the `●`/`□` glyph as the only
//! cue.
//!
//! This module builds the ramp from the active palette instead of reading the
//! tokens straight through:
//!
//! 1. **Seeds.** Each category starts from the theme token it already used
//!    (`primary`, `green`, `tip`, `secondary`, `muted`, `text`,
//!    `color_diff_add_fg`). A token that is effectively neutral (Oklab chroma
//!    below [`NEUTRAL_CHROMA`]) has no hue of its own, so it borrows the
//!    palette's chroma-weighted hue centre — a theme cannot paint a categorical
//!    grid out of greys alone.
//! 2. **Hue fan.** Hues are laid out clockwise from the palette's `primary`
//!    hue, which stays pinned so "User messages" keeps the theme's signature
//!    colour. Every circular gap is opened to at least [`MIN_HUE_GAP`] and the
//!    cost is paid out of the widest gaps, so a palette that is already spread
//!    barely moves at all and a degenerate one (two tokens at the same hue)
//!    fans out instead of colliding.
//! 3. **Chroma.** Each category keeps its own seed chroma, floored at
//!    [`CHROMA_FLOOR`] so borrowed-hue and low-chroma themes still read as
//!    colours rather than tints.
//! 4. **Lightness band.** The band runs between the lightness that clears
//!    [`MIN_PANEL_CONTRAST`] against `color_panel()` and the lightness that
//!    clears a far higher ratio, and is divided into three tier positions plus
//!    the recessive position used by the free block. Lightness therefore comes
//!    from the theme's own panel surface, in light and dark palettes alike.
//! 5. **Repair.** Gamut mapping, a contrast floor, and a pairwise separation
//!    pass run afterwards, so the invariants below hold for any palette —
//!    including user themes this module has never seen.
//!
//! The invariants the ramp guarantees, and the tests assert for every shipped
//! theme: every pair of the eight roles is at least [`MIN_SEPARATION`] apart in
//! Oklab (≈4x a just-noticeable difference), and every role clears
//! [`MIN_PANEL_CONTRAST`] against the panel surface. Filled and empty blocks
//! are therefore separated by colour as well as by glyph.

use ratatui::style::Color;

/// Number of filled usage categories (legend order: user, agent, tool calls,
/// system prompt, system tools, skills, subagents).
pub const CATEGORY_COUNT: usize = 7;

/// Oklab chroma below which a seed token counts as neutral and borrows the
/// palette hue centre.
pub const NEUTRAL_CHROMA: f64 = 0.025;

/// Minimum chroma requested for every category, so a low-chroma palette still
/// yields colours rather than tints.
pub const CHROMA_FLOOR: f64 = 0.085;

/// Minimum circular hue gap between two categories, in radians.
pub const MIN_HUE_GAP: f64 = 44f64.to_radians();

/// Smallest Oklab distance allowed between any two of the eight roles.
pub const MIN_SEPARATION: f64 = 0.08;

/// Smallest WCAG contrast ratio allowed between any role and `color_panel()`.
pub const MIN_PANEL_CONTRAST: f64 = 3.0;

/// Contrast the free/headroom block is placed at. Slightly above
/// [`MIN_PANEL_CONTRAST`] so 8-bit rounding cannot drop it below the floor.
const FREE_CONTRAST: f64 = 3.35;

/// Contrast cap for the far end of the lightness band.
const FAR_CONTRAST: f64 = 11.0;

/// Upper bound on the ramp's lightness, where sRGB runs out of chroma headroom.
const MAX_LIGHTNESS: f64 = 0.86;
/// Lower bound on the ramp's lightness in light palettes.
const MIN_LIGHTNESS: f64 = 0.30;
/// Narrowest band the three category tiers are allowed to occupy.
const MIN_BAND: f64 = 0.30;
/// Chroma multiplier for the recessive free block, relative to its own seed.
const FREE_CHROMA_SCALE: f64 = 0.30;

/// Repair passes aim a little past the published invariants so that 8-bit
/// quantisation cannot drop a role back below its floor.
const SEPARATION_TARGET: f64 = MIN_SEPARATION + 0.005;
const CONTRAST_TARGET: f64 = MIN_PANEL_CONTRAST + 0.05;

/// The seven filled categories plus the free block, in legend order.
pub fn ramp() -> [Color; CATEGORY_COUNT + 1] {
    let palette = crate::ui::theme::active_palette();
    let panel = rgb_of(palette.panel);
    let seeds = seed_colors(&palette);

    let mut labs = [[0.0; 3]; CATEGORY_COUNT];
    let mut chroma = [0.0; CATEGORY_COUNT];
    for (slot, seed) in seeds.iter().enumerate() {
        labs[slot] = to_oklab(rgb_of(*seed));
        chroma[slot] = (labs[slot][1].powi(2) + labs[slot][2].powi(2)).sqrt();
    }

    let hues = hue_fan(&hues_of(&labs, &chroma));
    let mut order: Vec<usize> = (0..CATEGORY_COUNT).collect();
    order.sort_by(|a, b| {
        hues[*a]
            .partial_cmp(&hues[*b])
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let (near, far) = lightness_band(panel);
    let span = (far - near) / 4.0;
    let free_hue = circular_mean(&[hues[order[3]], hues[order[4]]]);

    let mut out = [[0.0; 3]; CATEGORY_COUNT + 1];
    for (rank, category) in order.iter().enumerate() {
        let hue = hues[*category];
        // Tier by hue rank, so hue-neighbours always land on different tiers.
        let lightness = near + span * (1.0 + (rank % 3) as f64);
        out[*category] = from_oklab(lightness, chroma[*category].max(CHROMA_FLOOR), hue);
    }
    // The free block is a recessive track: the palette's own `muted` hue at a
    // third of the category chroma, sitting on the tier nearest the panel.
    let muted = to_oklab(rgb_of(palette.muted));
    let free_chroma = ((muted[1].powi(2) + muted[2].powi(2)).sqrt() * FREE_CHROMA_SCALE).max(0.02);
    out[CATEGORY_COUNT] = from_oklab(near, free_chroma, free_hue);

    repair(&mut out, panel);

    let mut colors = [Color::Rgb(0, 0, 0); CATEGORY_COUNT + 1];
    for (slot, lab) in colors.iter_mut().zip(out.iter()) {
        *slot = oklab_color(*lab);
    }
    colors
}

/// The seed token each category is derived from, in legend order. These are the
/// same tokens the pre-#1537 mapping read straight through; the ramp only
/// re-derives hue, chroma and lightness from them.
fn seed_colors(palette: &crate::ui::theme::ThemePalette) -> [Color; CATEGORY_COUNT] {
    [
        palette.primary,
        palette.green,
        palette.tip,
        palette.secondary,
        palette.muted,
        palette.text,
        crate::ui::COLOR_DIFF_ADD_FG(),
    ]
}

/// Oklab distance between two colors. Exposed so tests and callers can assert
/// the ramp's separation invariant without duplicating the conversion.
#[cfg(test)]
pub fn separation(a: Color, b: Color) -> f64 {
    let (x, y) = (to_oklab(rgb_of(a)), to_oklab(rgb_of(b)));
    ((x[0] - y[0]).powi(2) + (x[1] - y[1]).powi(2) + (x[2] - y[2]).powi(2)).sqrt()
}

/// WCAG 2.x contrast ratio between two colors.
pub fn contrast(a: Color, b: Color) -> f64 {
    let (hi, lo) = {
        let (x, y) = (relative_luminance(rgb_of(a)), relative_luminance(rgb_of(b)));
        if x >= y { (x, y) } else { (y, x) }
    };
    (hi + 0.05) / (lo + 0.05)
}

/// Hue of every category, borrowing the palette hue centre where a seed token
/// is too desaturated to carry one.
fn hues_of(labs: &[[f64; 3]], chroma: &[f64]) -> [f64; CATEGORY_COUNT] {
    let weighted = |component: usize| -> f64 {
        (0..CATEGORY_COUNT)
            .filter(|i| chroma[*i] >= NEUTRAL_CHROMA)
            .map(|i| chroma[i] * chroma[i] * labs[i][component])
            .sum::<f64>()
            / (0..CATEGORY_COUNT)
                .filter(|i| chroma[*i] >= NEUTRAL_CHROMA)
                .map(|i| chroma[i] * chroma[i])
                .sum::<f64>()
                .max(f64::MIN_POSITIVE)
    };
    let centre = (weighted(2)).atan2(weighted(1));
    let mut hues = [0.0; CATEGORY_COUNT];
    for i in 0..CATEGORY_COUNT {
        hues[i] = if chroma[i] >= NEUTRAL_CHROMA {
            labs[i][2].atan2(labs[i][1])
        } else {
            centre
        };
    }
    hues
}

/// Lay the category hues out clockwise with [`MIN_HUE_GAP`] between every
/// neighbouring pair. The first category keeps its own hue (the palette's
/// `primary`) so the ramp still reads as the theme's own palette; the remaining
/// hues move only as far as the minimum gap forces them to.
fn hue_fan(hues: &[f64; CATEGORY_COUNT]) -> [f64; CATEGORY_COUNT] {
    let tau = std::f64::consts::TAU;
    let anchor = hues[0].rem_euclid(tau);
    let mut rest: Vec<(f64, usize)> = (1..CATEGORY_COUNT)
        .map(|i| ((hues[i] - anchor).rem_euclid(tau), i))
        .collect();
    rest.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    // Desired gap before each remaining hue, opened to the minimum where the
    // palette crowds two roles together.
    let mut previous = 0.0;
    let mut gaps: Vec<f64> = rest
        .iter()
        .map(|(offset, _)| {
            let gap = MIN_HUE_GAP.max(offset - previous);
            previous = *offset;
            gap
        })
        .collect();

    // The wrap-around gap back to the anchor is whatever the placed hues leave
    // over, so if it is too small the widest placed gaps have to give ground.
    for _ in 0..CATEGORY_COUNT {
        let wrap = tau - gaps.iter().sum::<f64>();
        if wrap >= MIN_HUE_GAP - 1e-12 {
            break;
        }
        let widest = gaps
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(i, _)| i)
            .unwrap_or(0);
        gaps[widest] = MIN_HUE_GAP.max(gaps[widest] - (MIN_HUE_GAP - wrap));
    }

    let mut out = [0.0; CATEGORY_COUNT];
    out[0] = anchor;
    let mut placed = anchor;
    for (slot, gap) in rest.iter().map(|(_, i)| *i).zip(gaps.iter()) {
        placed += gap;
        out[slot] = placed.rem_euclid(tau);
    }
    out
}

/// The lightness band the ramp occupies: `near` is the end closest to the
/// panel surface and clears [`FREE_CONTRAST`], `far` the end furthest from it.
fn lightness_band(panel: [f64; 3]) -> (f64, f64) {
    let panel_luminance = relative_luminance(panel);
    let dark_panel = panel_luminance < 0.5;
    let (near, mut far) = if dark_panel {
        (
            lightness_for_luminance(FREE_CONTRAST * (panel_luminance + 0.05) - 0.05),
            MAX_LIGHTNESS.min(lightness_for_luminance(
                FAR_CONTRAST * (panel_luminance + 0.05) - 0.05,
            )),
        )
    } else {
        (
            lightness_for_luminance((panel_luminance + 0.05) / FREE_CONTRAST - 0.05),
            MIN_LIGHTNESS.max(lightness_for_luminance(
                (panel_luminance + 0.05) / FAR_CONTRAST - 0.05,
            )),
        )
    };
    if dark_panel {
        far = far.max(near + MIN_BAND);
    } else {
        far = far.min(near - MIN_BAND);
    }
    (near.clamp(0.0, 1.0), far.clamp(0.0, 1.0))
}

/// Gamut-map, contrast-floor, and separation-floor the constructed ramp. The
/// passes alternate because each can move a color the other just placed.
fn repair(colors: &mut [[f64; 3]], panel: [f64; 3]) {
    let panel_lightness = to_oklab(panel)[0];
    // Separation first, contrast last: separating a pair can walk a color back
    // towards the panel surface, so the contrast floor has to be reapplied
    // afterwards or the ramp trades one invariant for the other.
    for _ in 0..4 {
        for _ in 0..300 {
            let mut worst: Option<(f64, usize, usize)> = None;
            for i in 0..colors.len() {
                for j in (i + 1)..colors.len() {
                    let gap = distance(colors[i], colors[j]);
                    if worst.is_none_or(|(w, _, _)| gap < w) {
                        worst = Some((gap, i, j));
                    }
                }
            }
            let Some((gap, i, j)) = worst else { break };
            if gap >= SEPARATION_TARGET {
                break;
            }
            let mut push: [f64; 3] = std::array::from_fn(|k| colors[i][k] - colors[j][k]);
            let mut length = (push[0].powi(2) + push[1].powi(2) + push[2].powi(2)).sqrt();
            if length < 1e-9 {
                // Exact duplicate: fan out on a deterministic hue instead.
                let angle = std::f64::consts::TAU * (i as f64 + 0.5) / colors.len() as f64;
                push = [0.0, angle.cos(), angle.sin()];
                length = 1.0;
            }
            let step = (SEPARATION_TARGET - gap) / (2.0 * length);
            let left = gamut_map([
                colors[i][0] + push[0] * step,
                colors[i][1] + push[1] * step,
                colors[i][2] + push[2] * step,
            ]);
            let right = gamut_map([
                colors[j][0] - push[0] * step,
                colors[j][1] - push[1] * step,
                colors[j][2] - push[2] * step,
            ]);
            if distance(left, right) <= distance(colors[i], colors[j]) + 1e-9 {
                // Gamut-blocked at this lightness: trade a little lightness for
                // chroma headroom and try the push again.
                for slot in [i, j] {
                    let towards_headroom = if colors[slot][0] <= 0.65 { 0.02 } else { -0.02 };
                    colors[slot][0] = (colors[slot][0] + towards_headroom).clamp(0.0, 1.0);
                    let head = colors[slot];
                    colors[slot] = gamut_map(head);
                }
                continue;
            }
            colors[i] = left;
            colors[j] = right;
        }
        for color in colors.iter_mut() {
            for _ in 0..200 {
                if contrast(oklab_color(*color), panel_color(panel)) >= CONTRAST_TARGET {
                    break;
                }
                let away = if color[0] >= panel_lightness {
                    1.0
                } else {
                    -1.0
                };
                *color = gamut_map([
                    (color[0] + away * 0.008).clamp(0.0, 1.0),
                    color[1],
                    color[2],
                ]);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Colour math
// ---------------------------------------------------------------------------

/// sRGB triple in `0.0..=1.0`.
type Rgb = [f64; 3];

fn srgb_to_linear(channel: f64) -> f64 {
    if channel <= 0.04045 {
        channel / 12.92
    } else {
        ((channel + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(channel: f64) -> f64 {
    if channel <= 0.0031308 {
        12.92 * channel
    } else {
        1.055 * channel.powf(1.0 / 2.4) - 0.055
    }
}

fn relative_luminance(rgb: Rgb) -> f64 {
    0.2126 * srgb_to_linear(rgb[0])
        + 0.7152 * srgb_to_linear(rgb[1])
        + 0.0722 * srgb_to_linear(rgb[2])
}

fn from_oklab(lightness: f64, chroma: f64, hue: f64) -> [f64; 3] {
    gamut_map([lightness, chroma * hue.cos(), chroma * hue.sin()])
}

fn to_oklab(rgb: Rgb) -> [f64; 3] {
    const LMS_FROM_LINEAR: [[f64; 3]; 3] = [
        [0.412_221_470_8, 0.536_332_536_3, 0.051_445_992_9],
        [0.211_903_498_2, 0.680_699_545_1, 0.107_396_956_6],
        [0.088_302_461_9, 0.281_718_837_6, 0.629_978_700_5],
    ];
    const LAB_FROM_LMS: [[f64; 3]; 3] = [
        [0.210_454_255_3, 0.793_617_785, -0.004_072_046_8],
        [1.977_998_495_1, -2.428_592_205, 0.450_593_709_9],
        [0.025_904_037_1, 0.782_771_766_2, -0.808_675_766],
    ];
    let linear = [
        srgb_to_linear(rgb[0]),
        srgb_to_linear(rgb[1]),
        srgb_to_linear(rgb[2]),
    ];
    // The cube root is per-cone, so the two matrices cannot be pre-folded.
    let mut lms = [0.0; 3];
    for (row, out) in LMS_FROM_LINEAR.iter().zip(lms.iter_mut()) {
        *out = (0..3).map(|c| row[c] * linear[c]).sum::<f64>().cbrt();
    }
    let mut lab = [0.0; 3];
    for (row, out) in LAB_FROM_LMS.iter().zip(lab.iter_mut()) {
        *out = (0..3).map(|c| row[c] * lms[c]).sum::<f64>();
    }
    lab
}

/// Pull an Oklab colour back into sRGB by scaling chroma down uniformly, which
/// preserves hue and lightness.
fn gamut_map(lab: [f64; 3]) -> [f64; 3] {
    if oklab_to_rgb(lab)
        .iter()
        .all(|c| (-1e-4..=1.0001).contains(c))
    {
        return lab;
    }
    let mut low = 0.0;
    let mut high = 1.0;
    for _ in 0..20 {
        let mid = (low + high) / 2.0;
        let scaled = [lab[0], lab[1] * mid, lab[2] * mid];
        if oklab_to_rgb(scaled)
            .iter()
            .all(|c| (-1e-4..=1.0001).contains(c))
        {
            low = mid;
        } else {
            high = mid;
        }
    }
    [lab[0], lab[1] * low, lab[2] * low]
}

fn oklab_to_rgb(lab: [f64; 3]) -> Rgb {
    const LMS_FROM_LAB: [[f64; 3]; 3] = [
        [1.0, 0.396_337_777_4, 0.215_803_757_3],
        [1.0, -0.105_561_345_8, -0.063_854_172_8],
        [1.0, -0.089_484_177_5, -1.291_485_548],
    ];
    const LINEAR_FROM_LMS: [[f64; 3]; 3] = [
        [4.076_741_662_1, -3.307_711_591_3, 0.230_969_929_2],
        [-1.268_438_004_6, 2.609_757_401_1, -0.341_319_396_5],
        [-0.004_196_086_3, -0.703_418_614_7, 1.707_614_700_9],
    ];
    let mut lms = [0.0; 3];
    for (row, out) in LMS_FROM_LAB.iter().zip(lms.iter_mut()) {
        *out = ((0..3).map(|c| row[c] * lab[c]).sum::<f64>()).powi(3);
    }
    let mut linear = [0.0; 3];
    for (row, out) in LINEAR_FROM_LMS.iter().zip(linear.iter_mut()) {
        *out = (0..3).map(|c| row[c] * lms[c]).sum::<f64>();
    }
    [
        linear_to_srgb(linear[0]),
        linear_to_srgb(linear[1]),
        linear_to_srgb(linear[2]),
    ]
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|k| (a[k] - b[k]).powi(2)).sum::<f64>().sqrt()
}

fn circular_mean(hues: &[f64]) -> f64 {
    let sin = hues.iter().map(|h| h.sin()).sum::<f64>();
    let cos = hues.iter().map(|h| h.cos()).sum::<f64>();
    sin.atan2(cos)
}

/// Oklab lightness whose sRGB rendering has the requested relative luminance,
/// used to place the lightness band against a panel surface.
fn lightness_for_luminance(target: f64) -> f64 {
    let mut low = 0.0;
    let mut high = 1.0;
    for _ in 0..40 {
        let mid = (low + high) / 2.0;
        if relative_luminance(oklab_to_rgb([mid, 0.0, 0.0]).map(|c| c.clamp(0.0, 1.0))) < target {
            low = mid;
        } else {
            high = mid;
        }
    }
    (low + high) / 2.0
}

/// `Color` to an sRGB triple. Named colours use their usual xterm values and
/// `Reset` uses the neutral the ramps borrow for desaturated tokens, so a custom
/// theme written with `reset` still produces a usable ramp.
fn rgb_of(color: Color) -> Rgb {
    let rgb = match color {
        Color::Rgb(r, g, b) => (r as f64, g as f64, b as f64),
        Color::Reset | Color::Gray => (136.0, 146.0, 154.0),
        Color::Black => (0.0, 0.0, 0.0),
        Color::Red => (205.0, 49.0, 49.0),
        Color::Green => (13.0, 188.0, 121.0),
        Color::Yellow => (229.0, 229.0, 16.0),
        Color::Blue => (36.0, 114.0, 200.0),
        Color::Magenta => (188.0, 63.0, 188.0),
        Color::Cyan => (17.0, 168.0, 205.0),
        Color::DarkGray => (102.0, 102.0, 102.0),
        Color::LightRed => (241.0, 76.0, 76.0),
        Color::LightGreen => (35.0, 209.0, 139.0),
        Color::LightYellow => (245.0, 245.0, 67.0),
        Color::LightBlue => (59.0, 142.0, 234.0),
        Color::LightMagenta => (214.0, 112.0, 214.0),
        Color::LightCyan => (41.0, 184.0, 219.0),
        Color::White => (229.0, 229.0, 229.0),
        Color::Indexed(i) => indexed_rgb(i),
    };
    [rgb.0 / 255.0, rgb.1 / 255.0, rgb.2 / 255.0]
}

/// xterm 256-colour cube, the only indexed range a theme can name.
fn indexed_rgb(index: u8) -> (f64, f64, f64) {
    const CUBE: [f64; 6] = [0.0, 95.0, 135.0, 175.0, 215.0, 255.0];
    if index < 16 {
        return match index {
            0 => (0.0, 0.0, 0.0),
            1 => (128.0, 0.0, 0.0),
            2 => (0.0, 128.0, 0.0),
            3 => (128.0, 128.0, 0.0),
            4 => (0.0, 0.0, 128.0),
            5 => (128.0, 0.0, 128.0),
            6 => (0.0, 128.0, 128.0),
            7 => (192.0, 192.0, 192.0),
            8 => (128.0, 128.0, 128.0),
            9 => (255.0, 0.0, 0.0),
            10 => (0.0, 255.0, 0.0),
            11 => (255.0, 255.0, 0.0),
            12 => (0.0, 0.0, 255.0),
            13 => (255.0, 0.0, 255.0),
            14 => (0.0, 255.0, 255.0),
            _ => (255.0, 255.0, 255.0),
        };
    }
    if index < 232 {
        let offset = (index - 16) as usize;
        return (CUBE[offset / 36], CUBE[(offset % 36) / 6], CUBE[offset % 6]);
    }
    let grey = 8.0 + (index as f64 - 232.0) * 10.0;
    (grey, grey, grey)
}

fn panel_color(panel: Rgb) -> Color {
    Color::Rgb(
        (panel[0] * 255.0).round() as u8,
        (panel[1] * 255.0).round() as u8,
        (panel[2] * 255.0).round() as u8,
    )
}

fn oklab_color(lab: [f64; 3]) -> Color {
    panel_color(oklab_to_rgb(lab).map(|c| c.clamp(0.0, 1.0)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::tests::THEME_TEST_LOCK;

    const THEMES: [&str; 8] = [
        "default",
        "rain",
        "cozy-rain",
        "light",
        "nord",
        "dracula",
        "tokyo-night",
        "sky",
    ];

    #[test]
    fn every_shipped_theme_yields_eight_distinguishable_roles() {
        let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
        for theme in THEMES {
            crate::ui::theme::set_active_theme(theme);
            let panel = crate::ui::COLOR_PANEL();
            let colors = ramp();
            for (i, a) in colors.iter().enumerate() {
                let ratio = contrast(*a, panel);
                assert!(
                    ratio >= MIN_PANEL_CONTRAST,
                    "theme {theme}: role {i} ({a:?}) only {ratio:.2}:1 against panel {panel:?}"
                );
                for (j, b) in colors.iter().enumerate().skip(i + 1) {
                    let gap = separation(*a, *b);
                    assert!(
                        gap >= MIN_SEPARATION,
                        "theme {theme}: roles {i} ({a:?}) and {j} ({b:?}) only {gap:.4} apart"
                    );
                }
            }
        }
        crate::ui::theme::set_active_theme("default");
    }

    #[test]
    fn filled_and_empty_blocks_differ_by_colour_not_only_by_glyph() {
        let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
        for theme in THEMES {
            crate::ui::theme::set_active_theme(theme);
            let colors = ramp();
            let free = colors[CATEGORY_COUNT];
            for (i, filled) in colors[..CATEGORY_COUNT].iter().enumerate() {
                assert_ne!(
                    *filled, free,
                    "theme {theme}: category {i} matches the free block exactly"
                );
            }
        }
        crate::ui::theme::set_active_theme("default");
    }

    #[test]
    fn free_block_clears_the_panel_contrast_floor_in_light_themes() {
        let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
        // #1515 reported the empty block as effectively invisible on light
        // palettes; `turn_separator` against the light panel is 1.2:1.
        crate::ui::theme::set_active_theme("light");
        let panel = crate::ui::COLOR_PANEL();
        let free = ramp()[CATEGORY_COUNT];
        let ratio = contrast(free, panel);
        assert!(
            ratio >= MIN_PANEL_CONTRAST,
            "light theme free block {free:?} is only {ratio:.2}:1 against panel {panel:?}"
        );
        crate::ui::theme::set_active_theme("default");
    }

    #[test]
    fn sky_separates_the_tokens_that_used_to_collide() {
        let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
        crate::ui::theme::set_active_theme("sky");
        // `sky` maps `secondary` and `green` to the same #88c438.
        assert_eq!(crate::ui::COLOR_SECONDARY(), crate::ui::COLOR_GREEN());
        let colors = ramp();
        let agent = colors[1];
        let system_prompt = colors[3];
        assert_ne!(agent, system_prompt);
        assert!(separation(agent, system_prompt) >= MIN_SEPARATION);
        crate::ui::theme::set_active_theme("default");
    }

    #[test]
    fn dracula_separates_the_empty_block_from_system_tools() {
        let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
        // `dracula` maps `muted` and `turn_separator` to the same #6272a4, so
        // filled and empty used to be colour-identical.
        crate::ui::theme::set_active_theme("dracula");
        assert_eq!(crate::ui::COLOR_MUTED(), crate::ui::COLOR_TURN_SEPARATOR());
        let colors = ramp();
        let system_tools = colors[4];
        let free = colors[CATEGORY_COUNT];
        assert_ne!(system_tools, free);
        assert!(separation(system_tools, free) >= MIN_SEPARATION);
        crate::ui::theme::set_active_theme("default");
    }

    #[test]
    fn hue_fan_keeps_the_primary_hue_pinned_and_opens_tight_gaps() {
        let primary = 30f64.to_radians();
        let hues = hue_fan(&[
            primary,
            primary + 0.01,
            primary + 0.9,
            primary + 2.0,
            3.0,
            4.0,
            5.0,
        ]);
        assert!(
            (hues[0] - primary).abs() < 1e-9,
            "primary hue moved: {:?}",
            hues[0]
        );
        let tau = std::f64::consts::TAU;
        let mut sorted = hues.to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        for window in sorted.windows(2) {
            assert!(window[1] - window[0] >= MIN_HUE_GAP - 1e-9);
        }
        let wrap = tau + sorted[0] - sorted[sorted.len() - 1];
        assert!(wrap >= MIN_HUE_GAP - 1e-9, "wrap gap {wrap} is too small");
    }

    #[test]
    fn ramp_is_deterministic_for_one_theme() {
        let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
        crate::ui::theme::set_active_theme("tokyo-night");
        let first = ramp();
        let second = ramp();
        assert_eq!(first, second);
        crate::ui::theme::set_active_theme("default");
    }

    #[test]
    fn oklab_round_trips_srgb_colours() {
        for rgb in [
            [236.0, 110.0, 93.0],
            [56.0, 148.0, 240.0],
            [21.0, 23.0, 26.0],
        ] {
            let normalized = [rgb[0] / 255.0, rgb[1] / 255.0, rgb[2] / 255.0];
            let back = oklab_to_rgb(to_oklab(normalized));
            for (want, got) in normalized.iter().zip(back.iter()) {
                assert!(
                    (want - got).abs() < 1e-6,
                    "round trip drifted: {want} vs {got}"
                );
            }
        }
    }
}
