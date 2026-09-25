mod projection;

mod backend;
mod highlight;
mod view;

use std::path::PathBuf;

use gpui_kit::{
    AppContext, KeyBinding, WindowBounds, WindowOptions,
    component::{Root, Theme, ThemeMode, TitleBar},
    px, size,
};

use backend::NativeBackend;
use view::{AppView, ToggleSidebar};

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
            cx.bind_keys([KeyBinding::new("cmd-b", ToggleSidebar, None)]);
            Theme::change(ThemeMode::Dark, None, cx);
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
                        let updates = view.update(cx, |view, _| view.take_updates());
                        let update_view = view.clone();
                        cx.spawn(async move |cx| {
                            let mut updates = updates;
                            while let Some(event) = updates.recv().await {
                                let mut batch = vec![event];
                                while let Ok(event) = updates.try_recv() {
                                    batch.push(event);
                                }
                                for event in coalesce_text_deltas(batch) {
                                    update_view.update(cx, |view, cx| view.apply_event(event, cx));
                                }
                            }
                            update_view.update(cx, |view, cx| view.controller_stopped(cx));
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
