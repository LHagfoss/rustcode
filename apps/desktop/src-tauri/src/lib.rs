//! RustCode desktop shell. The webview talks ACP to the engine sidecar through
//! the `engine_*` commands; the other commands are the few native helpers the
//! UI needs (links, Finder, a harness login in Terminal, git changes, the
//! Diagnostics page). Every argument from the webview is validated here.

mod banner;
mod diagnostics;
mod engine;
mod links;
mod machines;
mod native;
mod notify;
mod process;
mod pty;
mod service;
mod smartui;
mod threads;
mod updater;
mod webview;

use engine::{Engine, EngineEvent};
use std::sync::Arc;
use tauri::ipc::Channel;
use tauri::{Manager, RunEvent, State};

type EngineState = Arc<Engine>;

#[tauri::command(async)]
fn engine_start(engine: State<'_, EngineState>, on_event: Channel<EngineEvent>) {
    engine.start(on_event);
}

#[tauri::command(async)]
fn engine_send(engine: State<'_, EngineState>, line: String) -> Result<(), String> {
    engine.send(&line)
}

#[tauri::command(async)]
fn engine_restart(engine: State<'_, EngineState>) {
    engine.restart();
}

#[tauri::command(async)]
fn engine_stop(engine: State<'_, EngineState>) {
    engine.stop();
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        // Notifications for a thread that needs the user (src/notify.rs).
        .plugin(tauri_plugin_notification::init())
        .manage(Arc::new(Engine::default()) as EngineState)
        .manage(Arc::new(machines::Machines::default()))
        .manage(links::LinkCache::default())
        .manage(pty::Ptys::default())
        .manage(smartui::SmartUiPages::default())
        .register_uri_scheme_protocol("rcui", |ctx, request| {
            smartui::respond(&ctx.app_handle().state::<smartui::SmartUiPages>(), &request)
        })
        .invoke_handler(tauri::generate_handler![
            engine_start,
            engine_send,
            engine_restart,
            engine_stop,
            machines::machines_list,
            machines::machines_discover,
            machines::machines_ask,
            machines::machines_pair,
            machines::machines_connect_code,
            machines::machines_connect_ssh,
            machines::machines_ssh_progress,
            machines::machines_cancel,
            machines::machines_remove,
            machines::machines_probe,
            machines::machines_retry,
            machines::machines_set_paused,
            machines::machines_set_display_name,
            machines::machines_set_icon,
            machines::machine_connect,
            machines::machine_send,
            machines::machine_disconnect,
            native::open_url,
            native::reveal_path,
            native::open_login_terminal,
            native::git_changes,
            native::project_files,
            native::read_image,
            native::clipboard_image,
            native::thread_image,
            native::attached_files,
            native::open_terminal_at,
            native::reduce_transparency,
            native::open_path,
            native::skill_create,
            native::open_in_editor,
            native::haptic,
            notify::notify_show,
            notify::notify_permission,
            notify::notify_badge,
            banner::banner_save,
            banner::banner_read,
            banner::banner_remove,
            links::link_meta,
            threads::thread_rename,
            threads::thread_trash,
            threads::thread_log_open,
            diagnostics::server_status,
            diagnostics::system_info,
            service::server_service_status,
            service::server_service_install,
            service::server_link_path,
            diagnostics::resolve_binaries,
            diagnostics::logs_folder_open,
            diagnostics::diagnostics_save,
            pty::pty_open,
            pty::pty_write,
            pty::pty_resize,
            pty::pty_close,
            smartui::smartui_page,
            updater::update_check,
            updater::update_channels,
            updater::update_install,
            updater::release_notes,
            webview::display_info,
            webview::webview_blur,
        ])
        .setup(|app| {
            // The engine starts with the app, in parallel with the webview loading the UI.
            app.state::<EngineState>().prestart();
            // An AppImage's bundled binary lives in a mount that goes away with the app, so the
            // copy a service runs is refreshed here; the copies outside the app catch up with the
            // engine this build ships, which is what makes an update reach them (#3487).
            std::thread::spawn(service::catch_up_after_update);
            // Built here, not from the config, so it renders at 120 Hz where the display does.
            webview::build_main_window(app)?;
            // A click on a notification is only reported while something listens for it.
            notify::register(app.handle());
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building the RustCode desktop app");
    app.run(|handle, event| {
        // A click on the Dock icon with the window hidden (webview.rs hides it on close).
        #[cfg(target_os = "macos")]
        if let RunEvent::Reopen { .. } = event {
            webview::show_main_window(handle);
            return;
        }
        if let RunEvent::Exit = event {
            // The window opens where it was last left (webview.rs).
            webview::remember_size_on_exit(handle);
            // The engine closes its sessions when its stdin closes.
            handle.state::<EngineState>().stop();
            handle.state::<pty::Ptys>().close_all();
        }
    });
}
