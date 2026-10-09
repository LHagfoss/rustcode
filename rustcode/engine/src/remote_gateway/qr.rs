//! Draws the pairing QR code in a terminal.
//!
//! The `qrcode` crate (built without its image features, so it brings no
//! dependencies of its own) produces the module matrix; the drawing is here
//! because both places that show a code need the same guarantees. Two
//! modules share one character cell through the half-block characters, which
//! keeps a pairing code near 30 rows, and the four-module quiet zone a
//! scanner needs is part of the drawing rather than left to whatever
//! surrounds it.
//!
//! A scanner expects dark modules on a light ground. A terminal's own colours
//! cannot promise that, so the colours are explicit: the command line gets
//! black on white as ANSI colours, and the terminal UI gets rows it styles the
//! same way (see [`PANEL_ROW_MARK`]). The code then reads the same in a light
//! and in a dark theme.

use qrcode::{Color, EcLevel, QrCode};

/// Modules of light border on every side.
const QUIET_ZONE: usize = 4;

/// Prefix of every QR row inside command-panel text. It is a zero-width
/// character, so a frontend that does not know it shows the row unchanged;
/// the terminal UI recognises it, keeps the rows unwrapped and paints them
/// black on white.
pub const PANEL_ROW_MARK: char = '\u{2060}';

/// Rows of the code for `payload`, quiet zone included. A block is a dark
/// module: `▀` upper, `▄` lower, `█` both. `None` when the payload is too
/// long for a QR code.
pub fn rows(payload: &str) -> Option<Vec<String>> {
    let code = QrCode::with_error_correction_level(payload.as_bytes(), EcLevel::L).ok()?;
    let width = code.width();
    let colors = code.to_colors();
    let size = width + 2 * QUIET_ZONE;
    let dark = |x: usize, y: usize| {
        let inside = |v: usize| (QUIET_ZONE..QUIET_ZONE + width).contains(&v);
        inside(x) && inside(y) && colors[(y - QUIET_ZONE) * width + (x - QUIET_ZONE)] == Color::Dark
    };
    Some(
        (0..size)
            .step_by(2)
            .map(|y| {
                (0..size)
                    .map(|x| match (dark(x, y), dark(x, y + 1)) {
                        (true, true) => '█',
                        (true, false) => '▀',
                        (false, true) => '▄',
                        (false, false) => ' ',
                    })
                    .collect()
            })
            .collect(),
    )
}

/// The code for a terminal that understands ANSI colours: black on white
/// from the 256-colour cube, which no theme redefines.
pub fn ansi(payload: &str) -> Option<String> {
    Some(
        rows(payload)?
            .iter()
            .map(|row| format!("\x1b[38;5;16;48;5;231m{row}\x1b[0m"))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

/// The code as command-panel rows, each prefixed with [`PANEL_ROW_MARK`].
pub fn panel(payload: &str) -> Option<String> {
    Some(
        rows(payload)?
            .iter()
            .map(|row| format!("{PANEL_ROW_MARK}{row}"))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Read the drawing back into a module matrix.
    fn modules(rows: &[String]) -> Vec<Vec<bool>> {
        let mut matrix = Vec::new();
        for row in rows {
            let (mut upper, mut lower) = (Vec::new(), Vec::new());
            for cell in row.chars() {
                upper.push(matches!(cell, '▀' | '█'));
                lower.push(matches!(cell, '▄' | '█'));
            }
            matrix.push(upper);
            matrix.push(lower);
        }
        matrix
    }

    #[test]
    fn the_drawing_is_the_code_with_a_quiet_zone() {
        let payload = r#"{"protocol_version":1,"address":"100.92.13.44:17879","gateway_id":"0123456789abcdef0123456789abcdef","credential":"q0lYc1o3bXJ3T2d5d0l2Rk5kU2tqeTZqZ0Z2ZUo0V2s","host_name":"studio"}"#;
        let rows = rows(payload).expect("a pairing payload fits a QR code");
        let code = QrCode::with_error_correction_level(payload.as_bytes(), EcLevel::L).unwrap();
        let width = code.width();
        let size = width + 2 * QUIET_ZONE;
        assert_eq!(rows.len(), size.div_ceil(2));
        assert!(rows.iter().all(|row| row.chars().count() == size));
        // Small enough for a terminal panel.
        assert!(size <= 72, "{size} columns");

        let drawn = modules(&rows);
        for y in 0..size {
            for x in 0..size {
                let inside = (QUIET_ZONE..QUIET_ZONE + width).contains(&x)
                    && (QUIET_ZONE..QUIET_ZONE + width).contains(&y);
                let expected = inside && code[(x - QUIET_ZONE, y - QUIET_ZONE)] == Color::Dark;
                assert_eq!(drawn[y][x], expected, "module ({x}, {y})");
            }
        }
    }

    #[test]
    fn both_renderings_carry_explicit_colours_or_the_panel_mark() {
        let ansi = ansi("payload").unwrap();
        assert!(
            ansi.lines()
                .all(|line| line.starts_with("\x1b[38;5;16;48;5;231m") && line.ends_with("\x1b[0m"))
        );
        let panel = panel("payload").unwrap();
        assert!(panel.lines().all(|line| line.starts_with(PANEL_ROW_MARK)));
        assert_eq!(panel.lines().count(), ansi.lines().count());
        assert!(rows(&"x".repeat(8_000)).is_none());
    }
}
