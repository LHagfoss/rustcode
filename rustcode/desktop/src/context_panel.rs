//! The `/context` display: how full the model's context window is and what
//! fills it.

use gpui_kit::{IntoElement, ParentElement as _, Styled as _, div, px, relative, rgb};
use rustcode::controller::ContextSummary;

use crate::theme::NativePalette as Palette;

/// One slice of the context window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ContextSegment {
    pub label: &'static str,
    pub tokens: u32,
    pub color: u32,
}

/// What occupies the window, in the order the bar paints it. The free part is
/// not a segment: it is whatever the bar leaves unpainted.
pub(crate) fn context_segments(context: &ContextSummary) -> Vec<ContextSegment> {
    let mut segments = vec![
        ContextSegment {
            label: "System, tools & skills",
            tokens: context.overhead_tokens(),
            color: Palette::CONTEXT_SYSTEM,
        },
        ContextSegment {
            label: "You",
            tokens: context.user_tokens,
            color: Palette::CONTEXT_USER,
        },
        ContextSegment {
            label: "Assistant",
            tokens: context.assistant_tokens,
            color: Palette::CONTEXT_ASSISTANT,
        },
        ContextSegment {
            label: "Tool calls",
            tokens: context.tool_tokens,
            color: Palette::CONTEXT_TOOLS,
        },
    ];
    // The per-role figures are estimates and can add up past the provider's
    // own count; scale them down so the bar never claims more than is used.
    let total: u64 = segments
        .iter()
        .map(|segment| u64::from(segment.tokens))
        .sum();
    let used = u64::from(context.used_tokens);
    if total > used && total > 0 {
        for segment in &mut segments {
            segment.tokens = (u64::from(segment.tokens) * used / total) as u32;
        }
    }
    segments.retain(|segment| segment.tokens > 0);
    segments
}

/// `1.2k`, `340`, `1.5M`: token counts at the precision a glance needs.
pub(crate) fn format_tokens(tokens: u32) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", f64::from(tokens) / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}k", f64::from(tokens) / 1_000.0)
    } else {
        tokens.to_string()
    }
}

/// Share of the window, one decimal below ten percent so small slices do not
/// all read as `0%`.
pub(crate) fn format_share(tokens: u32, window: u32) -> String {
    if window == 0 {
        return "0%".to_owned();
    }
    let share = f64::from(tokens) / f64::from(window) * 100.0;
    if share < 10.0 {
        format!("{share:.1}%")
    } else {
        format!("{share:.0}%")
    }
}

/// The short label for the composer's context chip.
pub(crate) fn chip_label(context: &ContextSummary) -> String {
    format!("{}% context left", context.remaining_percent())
}

fn fraction(tokens: u32, window: u32) -> f32 {
    if window == 0 {
        return 0.;
    }
    (tokens as f32 / window as f32).clamp(0., 1.)
}

/// The segmented usage bar, shared by the panel and the settings page.
pub(crate) fn usage_bar(context: &ContextSummary) -> impl IntoElement {
    let window = context.context_window;
    div()
        .w_full()
        .h(px(10.))
        .rounded_full()
        .overflow_hidden()
        .flex()
        .bg(rgb(Palette::BORDER_SUBTLE))
        .children(context_segments(context).into_iter().map(|segment| {
            div()
                .h_full()
                .flex_none()
                // A sliver stays visible however small its share is.
                .min_w(px(2.))
                .w(relative(fraction(segment.tokens, window)))
                .bg(rgb(segment.color))
        }))
}

fn legend_row(color: u32, label: &str, tokens: u32, window: u32) -> impl IntoElement {
    div()
        .w_full()
        .flex()
        .items_center()
        .gap_2()
        .text_sm()
        .child(div().flex_none().size(px(8.)).rounded_full().bg(rgb(color)))
        .child(div().flex_1().min_w_0().child(label.to_owned()))
        .child(
            div()
                .flex_none()
                .text_color(rgb(Palette::TEXT_SECONDARY))
                .child(format_tokens(tokens)),
        )
        .child(
            div()
                .flex_none()
                .w(px(48.))
                .flex()
                .justify_end()
                .text_xs()
                .text_color(rgb(Palette::TEXT_MUTED))
                .child(format_share(tokens, window)),
        )
}

/// Headline, bar and per-category legend for `context`.
pub(crate) fn usage_breakdown(context: &ContextSummary, model: Option<&str>) -> impl IntoElement {
    let window = context.context_window;
    let source = if context.provider_counted {
        "counted by the provider"
    } else {
        "estimated from the transcript"
    };
    let mut legend = div().w_full().flex().flex_col().gap_1();
    for segment in context_segments(context) {
        legend = legend.child(legend_row(
            segment.color,
            segment.label,
            segment.tokens,
            window,
        ));
    }
    legend = legend.child(legend_row(
        Palette::BORDER_STRONG,
        "Free",
        context.remaining_tokens(),
        window,
    ));

    div()
        .w_full()
        .flex()
        .flex_col()
        .gap_3()
        .child(
            div()
                .w_full()
                .flex()
                .items_end()
                .justify_between()
                .gap_3()
                .child(
                    div()
                        .flex()
                        .items_end()
                        .gap_2()
                        .child(
                            div()
                                .text_2xl()
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .child(format!("{}%", context.remaining_percent())),
                        )
                        .child(
                            div()
                                .pb(px(3.))
                                .text_sm()
                                .text_color(rgb(Palette::TEXT_SECONDARY))
                                .child("left"),
                        ),
                )
                .child(
                    div()
                        .pb(px(3.))
                        .text_sm()
                        .text_color(rgb(Palette::TEXT_SECONDARY))
                        .child(format!(
                            "{} of {} tokens",
                            format_tokens(context.used_tokens),
                            format_tokens(window)
                        )),
                ),
        )
        .child(usage_bar(context))
        .child(legend)
        .child(
            div()
                .text_xs()
                .text_color(rgb(Palette::TEXT_MUTED))
                .child(match model {
                    Some(model) => format!("{model} · {source}"),
                    None => format!("Usage {source}"),
                }),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(provider_counted: bool, used: u32) -> ContextSummary {
        ContextSummary {
            context_window: 200_000,
            used_tokens: used,
            provider_counted,
            user_tokens: 2_000,
            assistant_tokens: 6_000,
            tool_tokens: 12_000,
        }
    }

    #[test]
    fn provider_count_attributes_the_remainder_to_system_overhead() {
        let segments = context_segments(&context(true, 50_000));
        assert_eq!(
            segments
                .iter()
                .map(|segment| (segment.label, segment.tokens))
                .collect::<Vec<_>>(),
            [
                ("System, tools & skills", 30_000),
                ("You", 2_000),
                ("Assistant", 6_000),
                ("Tool calls", 12_000),
            ]
        );
    }

    #[test]
    fn estimates_show_no_overhead_and_never_exceed_what_is_used() {
        let estimated = context_segments(&context(false, 20_000));
        assert!(
            estimated
                .iter()
                .all(|segment| segment.label != "System, tools & skills")
        );

        // The transcript estimate ran past the provider's count.
        let scaled = context_segments(&context(true, 10_000));
        let total: u32 = scaled.iter().map(|segment| segment.tokens).sum();
        assert!(total <= 10_000, "{scaled:?}");
        assert_eq!(scaled[0].label, "You");
    }

    #[test]
    fn token_and_share_labels_stay_short() {
        assert_eq!(format_tokens(340), "340");
        assert_eq!(format_tokens(71_234), "71.2k");
        assert_eq!(format_tokens(1_500_000), "1.5M");
        assert_eq!(format_share(600, 200_000), "0.3%");
        assert_eq!(format_share(50_000, 200_000), "25%");
        assert_eq!(format_share(1, 0), "0%");
        assert_eq!(chip_label(&context(true, 50_000)), "75% context left");
    }
}
