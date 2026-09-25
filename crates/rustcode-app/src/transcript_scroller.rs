use std::{ops::Range, time::Duration};

use gpui_kit::{
    AnyElement, App, Axis, Context, ElementId, Entity, FollowMode, InteractiveElement, IntoElement,
    ListAlignment, ListOffset, ListState, ParentElement as _, RenderOnce, Role, SharedString,
    StatefulInteractiveElement as _, StyleRefinement, Styled, Window, div, hsla, list,
    prelude::FluentBuilder as _, px, rems,
};
use gpui_kit::{
    base::motion::{Transition, transition},
    component::{
        ActiveTheme as _, Disableable as _, IconName, StyledExt,
        button::{Button, ButtonVariants as _},
        scroll::{ScrollableElement, ScrollableMask},
    },
};

use crate::position_rail::{active_marker, marker_count, marker_to_row};

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
        let row_style = self.row_style;
        let mut renderer = self.renderer;
        let mut list_style = self.list_style;
        let row_inset_left = list_style.padding.left.take();
        let row_inset_right = list_style.padding.right.take();
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

        if self.position_rail && marker_count(self.position_rail_rows) > 0 {
            let state = self.state.clone();
            let current_marker = active_marker(logical_top_row, self.position_rail_rows);
            let markers = marker_count(self.position_rail_rows);
            let mut rail = div()
                .id((root_id.clone(), "position-rail"))
                .absolute()
                .top_0()
                .bottom_0()
                // Keep the marker hit areas just inside the scrollbar overlay.
                .right(px(9.))
                .w(px(22.))
                .py_2()
                .flex()
                .flex_col()
                .justify_between()
                .items_end();
            for marker in 0..markers {
                let active = current_marker == Some(marker);
                let emphasis = transition(
                    (root_id.clone(), format!("position-marker-{marker}")),
                    if active { 1. } else { 0. },
                    Transition::new(MARKER_TRANSITION),
                    window,
                    cx,
                );
                let row = marker_to_row(marker, self.position_rail_rows).unwrap_or(0);
                let state = state.clone();
                let dash =
                    Button::new((root_id.clone(), format!("position-marker-button-{marker}")))
                        .accessibility_label(format!("Scroll to transcript position {}", row + 1))
                        .on_click(move |_, _, cx| {
                            state.update(cx, |state, cx| {
                                state.scroll_to_item(row, cx);
                            });
                        })
                        .w(px(22.))
                        .h(px(12.))
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
                rail = rail.child(dash);
            }
            root = root.child(rail);
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
    use gpui_kit::AppContext as _;

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
        assert_eq!(marker_count(2), 2);
        assert_eq!(marker_count(8), 8);
        assert_eq!(marker_count(500), crate::position_rail::MAX_MARKERS);
    }
}
