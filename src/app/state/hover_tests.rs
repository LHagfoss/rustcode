use super::{AppState, HoverTarget};
use ratatui::layout::Rect;

#[test]
fn hover_target_prefers_the_pill_then_clickable_rows() {
    let mut s = AppState::new();
    s.scroll_to_bottom_btn = Some(Rect::new(60, 20, 20, 1));
    s.code_copy_rows = vec![(9, "code".to_string())];

    assert_eq!(s.hover_target_at(65, 20), HoverTarget::ScrollPill);
    assert_eq!(s.hover_target_at(0, 5), HoverTarget::None);
    assert_eq!(s.hover_target_at(40, 9), HoverTarget::CopyBadge(9));
    // Nothing clickable on this row, and outside the pill rect.
    assert_eq!(s.hover_target_at(40, 7), HoverTarget::None);
}
