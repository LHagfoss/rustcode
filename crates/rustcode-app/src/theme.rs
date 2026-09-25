/// Semantic colors shared by the native app surfaces and GPUI components.
pub(crate) struct NativePalette;

impl NativePalette {
    pub const APP_BACKGROUND: u32 = 0x1b1d1f;
    pub const SIDEBAR: u32 = 0x222426;
    pub const SURFACE_ELEVATED: u32 = 0x282a2d;
    pub const SURFACE_COMPOSER: u32 = 0x303236;
    pub const SURFACE_SELECTED: u32 = 0x393b40;
    pub const SURFACE_HOVER: u32 = 0x45474d;
    pub const SIDEBAR_HOVER: u32 = 0x363b43;
    pub const SIDEBAR_SELECTED: u32 = 0x3b414a;

    pub const BORDER_SUBTLE: u32 = 0x34383d;
    pub const BORDER_STRONG: u32 = 0x484c54;

    pub const TEXT_PRIMARY: u32 = 0xe8e9ed;
    pub const TEXT_SECONDARY: u32 = 0xb5b7bd;
    pub const TEXT_MUTED: u32 = 0x92969e;

    pub const COMMAND_ACCENT: u32 = 0xb69af5;
    pub const COMMAND_ACCENT_HOVER: u32 = 0xc6b1fa;
    pub const DESTRUCTIVE: u32 = 0xf0a0a0;
    pub const DESTRUCTIVE_SURFACE: u32 = 0x482d32;
    pub const APPROVAL: u32 = 0xe2c07a;
    pub const APPROVAL_SURFACE: u32 = 0x403923;
    pub const FOCUS_RING: u32 = 0xb69af5;
}

#[cfg(test)]
mod tests {
    use super::NativePalette;

    fn luminance(color: u32) -> f32 {
        let red = ((color >> 16) & 0xff) as f32;
        let green = ((color >> 8) & 0xff) as f32;
        let blue = (color & 0xff) as f32;
        red * 0.2126 + green * 0.7152 + blue * 0.0722
    }

    #[test]
    fn selected_sidebar_surface_and_primary_text_are_brighter_than_their_neighbors() {
        assert!(luminance(NativePalette::SIDEBAR_SELECTED) > luminance(NativePalette::SIDEBAR));
        assert!(
            luminance(NativePalette::SIDEBAR_SELECTED) > luminance(NativePalette::SIDEBAR_HOVER)
        );
        assert!(luminance(NativePalette::TEXT_PRIMARY) > luminance(NativePalette::TEXT_SECONDARY));
        assert!(luminance(NativePalette::TEXT_SECONDARY) > luminance(NativePalette::SIDEBAR));
    }
}
