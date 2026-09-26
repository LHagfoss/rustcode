mod projection;
mod search;
mod settings;

mod backend;
mod highlight;
mod image_attachment;
mod position_rail;
mod slash;
pub mod theme;
mod transcript_scroller;
mod view;

use std::path::PathBuf;

use gpui_kit::{
    AppContext, KeyBinding, WindowBounds, WindowOptions,
    component::{Root, Theme, ThemeMode, TitleBar},
    px, rgb, size,
};

use backend::NativeBackend;
use view::{
    AppView, CloseChatSearch, FocusSessionSearch, OpenSettings, ToggleChatSearch, ToggleSidebar,
};

gpui_kit::actions!([Quit, CloseWindow, MinimizeWindow]);

fn main() {
    let launch_dir = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    let backend = NativeBackend::new(launch_dir.clone()).unwrap_or_else(|error| {
        eprintln!("{error}");
        std::process::exit(1);
    });
    let mut backend = Some(backend);

    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            cx.bind_keys([
                KeyBinding::new("cmd-b", ToggleSidebar, None),
                KeyBinding::new("cmd-,", OpenSettings, None),
                KeyBinding::new("cmd-f", ToggleChatSearch, None),
                KeyBinding::new("cmd-k", FocusSessionSearch, None),
                KeyBinding::new("escape", CloseChatSearch, None),
                KeyBinding::new("cmd-q", Quit, None),
                KeyBinding::new("cmd-w", CloseWindow, None),
                KeyBinding::new("cmd-m", MinimizeWindow, None),
            ]);
            cx.on_action(|_: &Quit, cx| cx.quit());
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            Theme::change(ThemeMode::Dark, None, cx);
            {
                use theme::NativePalette as Palette;

                let colors = &mut Theme::global_mut(cx).colors;
                colors.background = rgb(Palette::APP_BACKGROUND).into();
                colors.foreground = rgb(Palette::TEXT_PRIMARY).into();
                colors.border = rgb(Palette::BORDER_SUBTLE).into();
                colors.input = rgb(Palette::BORDER_STRONG).into();
                colors.ring = rgb(Palette::FOCUS_RING).into();
                colors.selection = rgb(Palette::TEXT_SELECTION_BACKGROUND).into();
                colors.sidebar = rgb(Palette::SIDEBAR).into();
                colors.sidebar_border = rgb(Palette::BORDER_SUBTLE).into();
                colors.sidebar_foreground = rgb(Palette::TEXT_SECONDARY).into();
                colors.sidebar_accent = rgb(Palette::SIDEBAR_SELECTED).into();
                colors.sidebar_accent_foreground = rgb(Palette::TEXT_PRIMARY).into();
                colors.list = rgb(Palette::SURFACE_ELEVATED).into();
                colors.list_active = rgb(Palette::SIDEBAR_SELECTED).into();
                colors.list_active_border = rgb(Palette::BORDER_STRONG).into();
                colors.list_hover = rgb(Palette::SIDEBAR_HOVER).into();
                colors.muted = rgb(Palette::SURFACE_ELEVATED).into();
                colors.muted_foreground = rgb(Palette::TEXT_MUTED).into();
                colors.secondary_foreground = rgb(Palette::TEXT_SECONDARY).into();
                colors.accent = rgb(Palette::INLINE_CODE_BACKGROUND).into();
                colors.accent_foreground = rgb(Palette::TEXT_PRIMARY).into();
                colors.primary = rgb(Palette::BUTTON_PRIMARY_BACKGROUND).into();
                colors.primary_hover = rgb(Palette::BUTTON_PRIMARY_HOVER_BACKGROUND).into();
                colors.primary_foreground = rgb(Palette::BUTTON_PRIMARY_FOREGROUND).into();
                colors.button_primary = rgb(Palette::BUTTON_PRIMARY_BACKGROUND).into();
                colors.button_primary_hover = rgb(Palette::BUTTON_PRIMARY_HOVER_BACKGROUND).into();
                colors.button_primary_foreground = rgb(Palette::BUTTON_PRIMARY_FOREGROUND).into();
                colors.danger = rgb(Palette::DESTRUCTIVE_SURFACE).into();
                colors.danger_hover = rgb(Palette::DESTRUCTIVE_SURFACE).into();
                colors.danger_foreground = rgb(Palette::DESTRUCTIVE).into();
                colors.button_danger = rgb(Palette::DESTRUCTIVE_SURFACE).into();
                colors.button_danger_hover = rgb(Palette::DESTRUCTIVE_SURFACE).into();
                colors.button_danger_foreground = rgb(Palette::DESTRUCTIVE).into();
                colors.warning = rgb(Palette::APPROVAL_SURFACE).into();
                colors.warning_foreground = rgb(Palette::APPROVAL).into();
                colors.button_warning = rgb(Palette::APPROVAL_SURFACE).into();
                colors.button_warning_foreground = rgb(Palette::APPROVAL).into();
            }
            let theme = Theme::global_mut(cx);
            theme.tokens = theme.colors.into();
            Theme::sync_base(cx);
            highlight::install(cx);
            let window_bounds = WindowBounds::centered(size(px(1200.), px(800.)), cx);
            let backend = backend.take().expect("native window is opened once");
            cx.spawn(async move |cx| {
                cx.open_window(
                    WindowOptions {
                        window_bounds: Some(window_bounds),
                        window_min_size: Some(size(px(900.), px(620.))),
                        ..TitleBar::window_options()
                    },
                    move |window, cx| {
                        let view = cx.new(|cx| AppView::new(backend, launch_dir, window, cx));
                        let toggle_view = view.downgrade();
                        cx.on_action(move |_: &ToggleSidebar, cx| {
                            let _ = toggle_view.update(cx, |view, cx| view.toggle_sidebar(cx));
                        });
                        let settings_window = window.window_handle();
                        let settings_view = view.downgrade();
                        cx.on_action(move |_: &OpenSettings, cx| {
                            let settings_view = settings_view.clone();
                            cx.defer(move |cx| {
                                // Global actions run while the active window is being dispatched.
                                // Defer until GPUI has returned it to the app before updating it.
                                let _ = cx.update_window(settings_window, |_, window, cx| {
                                    let _ = settings_view.update(cx, |view, cx| {
                                        view.open_settings(window, cx);
                                    });
                                });
                            });
                        });
                        let search_view = view.downgrade();
                        cx.on_action(move |_: &ToggleChatSearch, cx| {
                            let _ = search_view.update(cx, |view, cx| view.toggle_chat_search(cx));
                        });
                        let close_view = view.downgrade();
                        cx.on_action(move |_: &CloseChatSearch, cx| {
                            let _ = close_view.update(cx, |view, cx| view.close_chat_search(cx));
                        });
                        let search_view = view.downgrade();
                        cx.on_action(move |_: &FocusSessionSearch, cx| {
                            let search_view = search_view.clone();
                            cx.defer(move |cx| {
                                let _ = cx.update_window(settings_window, |_, window, cx| {
                                    let _ = search_view.update(cx, |view, cx| {
                                        view.focus_session_search(window, cx);
                                    });
                                });
                            });
                        });
                        let updates = view.update(cx, |view, _| view.take_updates());
                        let update_view = view.downgrade();
                        cx.spawn(async move |cx| {
                            let mut updates = updates;
                            while let Some(event) = updates.recv().await {
                                let mut batch = vec![event];
                                while let Ok(event) = updates.try_recv() {
                                    batch.push(event);
                                }
                                for event in coalesce_text_deltas(batch) {
                                    let _ = update_view
                                        .update(cx, |view, cx| view.apply_event(event, cx));
                                }
                            }
                            let _ = update_view.update(cx, |view, cx| view.controller_stopped(cx));
                        })
                        .detach();

                        cx.new(|cx| Root::new(view, window, cx))
                    },
                )
                .expect("failed to open native window");
            })
            .detach();
        });
}

fn coalesce_text_deltas(
    events: Vec<rustcode::controller::ControllerEvent>,
) -> Vec<rustcode::controller::ControllerEvent> {
    use rustcode::controller::{ControllerUpdate, TurnUpdate};

    let mut coalesced: Vec<rustcode::controller::ControllerEvent> =
        Vec::with_capacity(events.len());
    for event in events {
        if let Some(previous) = coalesced.last_mut()
            && previous.generation == event.generation
            && let (
                ControllerUpdate::Turn(TurnUpdate::TextDelta(previous_text)),
                ControllerUpdate::Turn(TurnUpdate::TextDelta(text)),
            ) = (&mut previous.update, &event.update)
        {
            previous_text.push_str(text);
        } else {
            coalesced.push(event);
        }
    }
    coalesced
}
