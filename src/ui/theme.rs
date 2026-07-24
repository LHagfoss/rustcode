use ratatui::style::{Color, Modifier, Style};

pub const COLOR_BG: Color = Color::Rgb(21, 23, 26);
pub const COLOR_PANEL: Color = Color::Rgb(26, 29, 32);
pub const COLOR_ELEMENT: Color = Color::Rgb(34, 38, 42);
pub const COLOR_TEXT: Color = Color::Rgb(240, 229, 222);
pub const COLOR_MUTED: Color = Color::Rgb(136, 146, 154);
pub const COLOR_PRIMARY: Color = Color::Rgb(236, 110, 93);
pub const COLOR_SECONDARY: Color = Color::Rgb(60, 88, 101);
pub const COLOR_GREEN: Color = Color::Rgb(127, 216, 143);
pub const COLOR_SELECTION: Color = Color::Rgb(60, 95, 150);
pub const COLOR_TIP: Color = Color::Rgb(224, 169, 109);

pub fn get_themed_style(fg: Color, bg: Color, modifier: Modifier, show_picker: bool) -> Style {
    if show_picker {
        Style::default().fg(Color::Rgb(60, 68, 72)).bg(COLOR_BG)
    } else {
        Style::default().fg(fg).bg(bg).add_modifier(modifier)
    }
}
