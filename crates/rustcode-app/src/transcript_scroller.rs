use std::{ops::Range, time::Duration};

use gpui_kit::{
    AnyElement, App, Axis, Context, ElementId, Entity, FollowMode, InteractiveElement, IntoElement,
    ListAlignment, ListOffset, ListState, ParentElement as _, RenderOnce, Role, SharedString,
    StatefulInteractiveElement as _, StyleRefinement, Styled, Window, container_query, div, hsla,
    list, prelude::FluentBuilder as _, px, rems,
};
use gpui_kit::{
    base::motion::{Transition, transition},
    component::{
        ActiveTheme as _, Disableable as _, IconName, StyledExt,
        button::{Button, ButtonVariants as _},
        scroll::{ScrollableElement, ScrollableMask},
    },
};

use crate::position_rail::{
    MARKER_HIT_TARGET_HEIGHT, MARKER_HIT_TARGET_WIDTH, MIN_MARKER_HIT_TARGET_HEIGHT,
    RAIL_CONTENT_INSET, RAIL_SCROLLBAR_INSET, active_marker_with_count, marker_count_for_height,
    marker_to_row_with_count,
};

const LIST_OVERDRAW: gpui_kit::Pixels = px(400.);
const JUMP_BUTTON_TRANSITION: Duration = Duration::from_millis(200);
const MARKER_TRANSITION: Duration = Duration::from_millis(120);

/// App-owned equivalent of gpui-component's message scroller state.
///
/// This exists because gpui-component 0.6.6 keeps its virtual `ListState`
/// private; the transcript position rail must read the list's real logical top.
pub struct TranscriptScrollerState {
    list_state: ListState,
}

impl TranscriptScrollerState {
    pub fn new(item_count: usize, cx: &mut Context<Self>) -> Self {
        let list_state = ListState::new(item_count, ListAlignment::Top, LIST_OVERDRAW);
        list_state.set_follow_mode(FollowMode::Tail);

        let weak_state = cx.weak_entity();
        list_state.set_scroll_handler(move |_, _, cx| {
            let weak_state = weak_state.clone();
            cx.defer(move |cx| {
                let _ = weak_state.update(cx, |_, cx| cx.notify());
            });
        });

        Self { list_state }
    }

    pub fn item_count(&self) -> usize {
        self.list_state.item_count()
    }

    pub fn logical_top_row(&self) -> usize {
        self.list_state.logical_scroll_top().item_ix
    }

    pub fn is_scrolled_up(&self) -> bool {
        self.list_state.max_offset_for_scrollbar().y > px(0.)
            && !self.list_state.is_following_tail()
            && !self.list_state.is_scrolled_to_end().unwrap_or(false)
    }

    pub fn reset(&mut self, item_count: usize, cx: &mut Context<Self>) {
        self.list_state.reset(item_count);
        self.list_state.set_follow_mode(FollowMode::Tail);
        cx.notify();
    }

    pub fn splice(
        &mut self,
        old_range: Range<usize>,
        count: usize,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.valid_range(&old_range) {
            return false;
        }
        let neighbor = old_range.start.checked_sub(1);
        self.list_state.splice(old_range, count);
        if let Some(last) = self.list_state.item_count().checked_sub(1) {
            self.list_state.remeasure_items(last..last + 1);
            if let Some(neighbor) = neighbor.filter(|neighbor| *neighbor != last) {
                self.list_state.remeasure_items(neighbor..neighbor + 1);
            }
        }
        cx.notify();
        true
    }

    pub fn append(&mut self, count: usize, cx: &mut Context<Self>) -> bool {
        let item_count = self.list_state.item_count();
        self.splice(item_count..item_count, count, cx)
    }

    pub fn prepend(&mut self, count: usize, cx: &mut Context<Self>) -> bool {
        self.splice(0..0, count, cx)
    }

    pub fn remeasure(&mut self, cx: &mut Context<Self>) {
        self.list_state.remeasure();
        cx.notify();
    }

    pub fn remeasure_items(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> bool {
        if !self.valid_range(&range) {
            return false;
        }
        self.list_state.remeasure_items(range);
        cx.notify();
        true
    }

    pub fn scroll_to_item(&mut self, index: usize, cx: &mut Context<Self>) -> bool {
        if index >= self.list_state.item_count() {
            return false;
        }
        self.list_state.scroll_to(ListOffset {
            item_ix: index,
            offset_in_item: px(0.),
        });
        cx.notify();
        true
    }

    pub fn scroll_to_end(&mut self, cx: &mut Context<Self>) {
        self.list_state.set_follow_mode(FollowMode::Tail);
        self.list_state.scroll_to_end();
        cx.notify();
    }

    fn valid_range(&self, range: &Range<usize>) -> bool {
        range.start <= range.end && range.end <= self.list_state.item_count()
    }
}

#[derive(IntoElement)]
pub struct TranscriptScroller {
    id: ElementId,
    state: Entity<TranscriptScrollerState>,
    renderer: Box<dyn FnMut(usize, &mut Window, &mut App) -> AnyElement + 'static>,
    style: StyleRefinement,
    content_style: StyleRefinement,
    list_style: StyleRefinement,
    row_style: StyleRefinement,
    scrollbar: bool,
    position_rail: bool,
    position_rail_rows: usize,
    jump_button: bool,
    jump_button_label: SharedString,
}

impl TranscriptScroller {
    pub fn new<E>(
        id: impl Into<ElementId>,
        state: Entity<TranscriptScrollerState>,
        renderer: impl FnMut(usize, &mut Window, &mut App) -> E + 'static,
    ) -> Self
    where
        E: IntoElement,
    {
        let mut renderer = renderer;
        Self {
            id: id.into(),
            state,
            renderer: Box::new(move |index, window, cx| {
                renderer(index, window, cx).into_any_element()
            }),
            style: StyleRefinement::default(),
            content_style: StyleRefinement::default(),
            list_style: StyleRefinement::default(),
            row_style: StyleRefinement::default(),
            scrollbar: true,
            position_rail: false,
            position_rail_rows: 0,
            jump_button: true,
            jump_button_label: "Jump to latest".into(),
        }
    }

    pub fn with_content_style(mut self, style: StyleRefinement) -> Self {
        self.content_style = style;
        self
    }

    pub fn with_list_style(mut self, style: StyleRefinement) -> Self {
        self.list_style = style;
        self
    }

    pub fn with_row_style(mut self, style: StyleRefinement) -> Self {
        self.row_style = style;
        self
    }

    pub fn with_position_rail(mut self, rows: usize) -> Self {
        self.position_rail = true;
        self.position_rail_rows = rows;
        self
    }
}

impl Styled for TranscriptScroller {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for TranscriptScroller {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let root_id = self.id.clone();
        let (list_state, scrolled_up, logical_top_row) = {
            let state = self.state.read(cx);
            (
                state.list_state.clone(),
                state.is_scrolled_up(),
                state.logical_top_row(),
            )
        };
        let jump_visibility = if self.jump_button {
            transition(
                (root_id.clone(), "jump-button-visibility"),
                if scrolled_up { 1. } else { 0. },
                Transition::new(JUMP_BUTTON_TRANSITION),
                window,
                cx,
            )
        } else {
            0.
        };
        let tokens = cx.theme().semantic_tokens();
        let item_count = list_state.item_count();
        let rail_visible = self.position_rail && self.position_rail_rows > 1;
        let row_style = self.row_style;
        let mut renderer = self.renderer;
        let mut list_style = self.list_style;
        let row_inset_left = list_style.padding.left.take();
        // Keep the wider marker hit boxes in their own strip instead of over
        // message text; explicit caller insets still take precedence.
        let row_inset_right = list_style
            .padding
            .right
            .take()
            .or_else(|| rail_visible.then_some(px(RAIL_CONTENT_INSET).into()));
        let list = list(list_state.clone(), move |index, window, cx| {
            div()
                .w_full()
                .min_w_0()
                .px_3()
                .when(index + 1 < item_count, |this| this.pb_8())
                .when_some(row_inset_left, |this, left| this.pl(left))
                .when_some(row_inset_right, |this, right| this.pr(right))
                .refine_style(&row_style)
                .child(renderer(index, window, cx))
                .into_any_element()
        })
        .size_full()
        .min_h_0()
        .py_2()
        .refine_style(&list_style);

        let viewport = div()
            .id((root_id.clone(), "viewport"))
            .role(Role::Log)
            .size_full()
            .min_h_0()
            .min_w_0()
            .child(list)
            .when(self.scrollbar, |this| this.vertical_scrollbar(&list_state))
            .refine_style(&self.content_style);

        let mut root = div()
            .id(root_id.clone())
            .relative()
            .size_full()
            .min_h_0()
            .overflow_hidden()
            .child(viewport)
            .child(ScrollableMask::new(Axis::Vertical, &list_state).id(root_id.clone()));

        if rail_visible {
            let state = self.state.clone();
            let row_count = self.position_rail_rows;
            let rail_id = root_id.clone();
            root = root.child(
                container_query(move |size, window, cx| {
                    let viewport_height = size.height / px(1.);
                    let markers = marker_count_for_height(row_count, viewport_height);
                    let current_marker =
                        active_marker_with_count(logical_top_row, row_count, markers);
                    let mut rail = div()
                        .id((rail_id.clone(), "position-rail"))
                        // Keep the marker hit areas fully clear of the scrollbar strip.
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .right(px(RAIL_SCROLLBAR_INSET))
                        .w(px(MARKER_HIT_TARGET_WIDTH))
                        .min_h_0()
                        .py_2()
                        .flex()
                        .flex_col()
                        .items_end();
                    for marker in 0..markers {
                        let active = current_marker == Some(marker);
                        let emphasis = transition(
                            (rail_id.clone(), format!("position-marker-{marker}")),
                            if active { 1. } else { 0. },
                            Transition::new(MARKER_TRANSITION),
                            window,
                            cx,
                        );
                        let row = marker_to_row_with_count(marker, row_count, markers).unwrap_or(0);
                        let state = state.clone();
                        let dash = Button::new((
                            rail_id.clone(),
                            format!("position-marker-button-{marker}"),
                        ))
                        .ghost()
                        .accessibility_label(format!("Scroll to transcript position {}", row + 1))
                        .on_click(move |_, _, cx| {
                            state.update(cx, |state, cx| {
                                state.scroll_to_item(row, cx);
                            });
                        })
                        .w(px(MARKER_HIT_TARGET_WIDTH))
                        .h_full()
                        .max_h(px(MARKER_HIT_TARGET_HEIGHT))
                        .min_h(px(MIN_MARKER_HIT_TARGET_HEIGHT))
                        .flex_shrink_1()
                        .px_1()
                        .flex()
                        .justify_end()
                        .items_center()
                        .child(
                            div()
                                .w(px(3. + 3. * emphasis))
                                .h(px(2.))
                                .rounded_full()
                                .bg(hsla(0., 0., 0.38 + 0.46 * emphasis, 0.5 + 0.5 * emphasis)),
                        );
                        rail = rail.child(
                            div()
                                .w_full()
                                .flex_1()
                                .min_h_0()
                                .flex()
                                .justify_end()
                                .items_center()
                                .child(dash),
                        );
                    }
                    rail
                })
                .absolute()
                .top_0()
                .bottom_0()
                .left_0()
                .right_0(),
            );
        }

        if self.jump_button && jump_visibility > 0. {
            let state = self.state.clone();
            root = root.child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom(rems(0.5 + jump_visibility * 0.5))
                    .flex()
                    .justify_center()
                    .opacity(jump_visibility)
                    .child(
                        Button::new((root_id, "jump-to-latest"))
                            .secondary()
                            .icon(IconName::ArrowDown)
                            .tooltip(self.jump_button_label)
                            .rounded(cx.theme().radius_full())
                            .border_1()
                            .border_color(tokens.colors.border)
                            .bg(tokens.colors.background)
                            .text_color(tokens.colors.foreground)
                            .on_click(move |_, _, cx| {
                                state.update(cx, |state, cx| state.scroll_to_end(cx));
                            })
                            .when(!scrolled_up, |button| button.disabled(true)),
                    ),
            );
        }

        root.refine_style(&self.style)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::position_rail::{MAX_MARKERS, RAIL_VERTICAL_INSET};
    use gpui_kit::{AppContext as _, Render};

    struct RailHarness {
        state: Entity<TranscriptScrollerState>,
    }

    impl Render for RailHarness {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            TranscriptScroller::new("conversation", self.state.clone(), |index, _, _| {
                div().h(px(32.)).child(format!("Row {index}"))
            })
            .with_position_rail(100)
            .size_full()
        }
    }

    #[gpui_kit::test]
    fn state_preserves_virtual_list_mutation_and_scroll_behavior(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let state = cx.new(|cx| TranscriptScrollerState::new(3, cx));
        cx.update(|cx| {
            assert_eq!(state.read(cx).item_count(), 3);
            assert!(state.read(cx).logical_top_row() <= 3);
            state.update(cx, |state, cx| {
                assert!(state.append(2, cx));
                assert_eq!(state.item_count(), 5);
                assert!(state.prepend(1, cx));
                assert_eq!(state.item_count(), 6);
                assert!(!state.splice(5..7, 0, cx));
                assert!(state.remeasure_items(0..6, cx));
                assert!(!state.remeasure_items(6..7, cx));
                state.remeasure(cx);
                assert!(state.scroll_to_item(2, cx));
                assert_eq!(state.logical_top_row(), 2);
                assert!(!state.scroll_to_item(6, cx));
                state.scroll_to_end(cx);
                state.reset(2, cx);
            });
            assert_eq!(state.read(cx).item_count(), 2);
        });
    }

    #[test]
    fn rail_is_bounded_and_has_one_marker_for_each_short_display_row() {
        assert_eq!(crate::position_rail::marker_count(2), 2);
        assert_eq!(crate::position_rail::marker_count(8), 8);
        assert_eq!(crate::position_rail::marker_count(500), MAX_MARKERS);
    }

    #[gpui_kit::test]
    fn marker_hit_targets_fit_without_overlapping_scrollbar_at_constrained_heights(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use gpui_kit::test::TestWindowExt as _;

        cx.update(gpui_kit::init);
        let (_, window) = cx.add_window_view(|_, cx| RailHarness {
            state: cx.new(|cx| TranscriptScrollerState::new(100, cx)),
        });

        for height in [31., 32., 47., 48., 120., 180., 300.] {
            window.simulate_resize(gpui_kit::size(px(600.), px(height)));
            let (marker_count, marker_bounds) = window.update(|window, cx| {
                window.render_frame(cx);
                let marker_count = crate::position_rail::marker_count_for_height(100, height);
                let bounds = (0..marker_count)
                    .map(|marker| {
                        window
                            .find((
                                ElementId::from("conversation"),
                                format!("position-marker-button-{marker}"),
                            ))
                            .bounds()
                    })
                    .collect::<Vec<_>>();
                (marker_count, bounds)
            });

            assert_eq!(marker_bounds.len(), marker_count);
            assert!(marker_count <= MAX_MARKERS);
            for bounds in &marker_bounds {
                assert_eq!(bounds.size.width, px(MARKER_HIT_TARGET_WIDTH));
                assert!(bounds.size.height >= px(MIN_MARKER_HIT_TARGET_HEIGHT));
                assert!(bounds.size.height <= px(MARKER_HIT_TARGET_HEIGHT));
                assert!(bounds.origin.y >= px(RAIL_VERTICAL_INSET));
                assert!(bounds.origin.y + bounds.size.height <= px(height - RAIL_VERTICAL_INSET));
                assert!(bounds.origin.x + bounds.size.width <= px(600. - RAIL_SCROLLBAR_INSET));
            }
            for pair in marker_bounds.windows(2) {
                assert!(pair[0].origin.y + pair[0].size.height <= pair[1].origin.y);
            }
        }
    }
}
