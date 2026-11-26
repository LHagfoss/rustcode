//! RustCode desktop shell. The webview talks ACP to the engine sidecar through
//! the `engine_*` commands; the other commands are the few native helpers the
//! UI needs (links, Finder, a harness login in Terminal, git changes, the
//! Diagnostics page). Every argument from the webview is validated here.

mod banner;
mod crash_report;
mod diagnostics;
mod engine;
mod engine_mux;
mod git_review;
mod hub;
mod issue_report;
mod links;
mod machines;
mod microphone;
mod native;
mod notify;
mod process;
mod service;
mod smartui;
mod system_setup;
mod threads;
mod update_log;
mod updater;
mod url_scheme;
mod webview;
mod webview_watch;

use engine::{Engine, EngineEvent};
use std::sync::Arc;
use tauri::ipc::Channel;
use tauri::{Emitter, Manager, RunEvent, State};
use tauri_plugin_deep_link::DeepLinkExt;

type EngineState = Arc<Engine>;

/// Event emitted when any window answers an in-flight permission prompt.
const PERMISSION_RESOLVED_EVENT: &str = "rustcode:permission-resolved";

#[tauri::command(async)]
fn engine_start(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    engine: State<'_, EngineState>,
    on_event: Channel<EngineEvent>,
) {
    if let Some(primary) = engine.start(window.label(), on_event) {
        let _ = app.emit(webview::PRIMARY_WINDOW_EVENT, primary);
    }
}

#[tauri::command(async)]
fn engine_send(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    engine: State<'_, EngineState>,
    line: String,
) -> Result<(), String> {
    if let Some(rpc_id) = engine.send(window.label(), &line)? {
        let _ = app.emit(PERMISSION_RESOLVED_EVENT, rpc_id);
    }
    Ok(())
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
    crash_report::install_panic_hook();
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            webview::show_main_window(app);
        }))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_dialog::init())
        // Notifications for a thread that needs the user (src/notify.rs).
        .plugin(tauri_plugin_notification::init())
        .manage(Arc::new(Engine::default()) as EngineState)
        .manage(Arc::new(machines::Machines::default()))
        .manage(links::LinkCache::default())
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
            machines::machines_pair_back,
            machines::machines_ssh_progress,
            machines::machines_cancel,
            machines::machines_remove,
            machines::machines_probe,
            machines::machines_retry,
            machines::machines_set_paused,
            machines::machines_route_add,
            machines::machines_forward_take_over,
            machines::machines_route_remove,
            machines::machines_route_set_off,
            machines::machines_set_display_name,
            machines::machines_apply_fleet,
            machines::machines_set_icon,
            machines::machine_connect,
            machines::machine_send,
            machines::machine_disconnect,
            issue_report::issue_report_options,
            issue_report::issue_report_submit,
            issue_report::issue_report_save_image,
            issue_report::issue_report_save_images,
            native::open_url,
            native::reveal_path,
            native::open_login_terminal,
            native::git_changes,
            git_review::git_commits,
            git_review::git_commit_changes,
            native::project_files,
            native::read_image,
            native::clipboard_image,
            native::thread_image,
            native::attached_files,
            native::path_kinds,
            native::open_terminal_at,
            native::reduce_transparency,
            native::open_path,
            native::skill_create,
            native::open_in_editor,
            native::open_folder_in_editor,
            native::haptic,
            microphone::microphone_access,
            microphone::open_microphone_settings,
            url_scheme::url_scheme_status,
            url_scheme::claim_url_scheme,
            notify::notify_show,
            notify::notify_permission,
            notify::notify_badge,
            banner::banner_save,
            banner::banner_read,
            banner::banner_remove,
            links::link_meta,
            threads::thread_rename,
            threads::thread_user_titled,
            threads::thread_trash,
            threads::thread_log_open,
            threads::thread_log_tail,
            diagnostics::server_status,
            diagnostics::system_info,
            service::server_service_status,
            service::server_service_install,
            service::server_link_path,
            hub::hub_status,
            hub::hub_set,
            system_setup::system_setup_status,
            system_setup::system_setup_run,
            diagnostics::resolve_binaries,
            diagnostics::logs_folder_open,
            diagnostics::diagnostics_save,
            smartui::smartui_page,
            updater::update_check,
            updater::update_channels,
            updater::update_install,
            updater::release_notes,
            webview::display_info,
            webview::webview_blur,
            webview::window_theme,
            webview::open_new_window,
            webview::primary_window_label,
            crash_report::crash_report_status,
            crash_report::crash_report_set_enabled,
            crash_report::crash_report_send,
        ])
        .setup(|app| {
            // Release Cmd+W to Settle and Cmd+M to the model picker.
            // Tauri puts Close in both File and Window. Only macOS has an app menu by
            // default; setting one on Linux and Windows adds a File/Edit bar to the window,
            // and there Ctrl+W reaches the webview without a menu.
            #[cfg(target_os = "macos")]
            {
                use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};

                const MINIMIZE_ID: &str = "minimize-window";
                const NEW_WINDOW_ID: &str = "new-window";
                let menu = Menu::default(app.handle())?;
                let close_text = PredefinedMenuItem::close_window(app, None)?.text()?;
                let minimize_text = PredefinedMenuItem::minimize(app, None)?.text()?;
                let minimize =
                    MenuItem::with_id(app, MINIMIZE_ID, &minimize_text, true, None::<&str>)?;
                let new_window =
                    MenuItem::with_id(app, NEW_WINDOW_ID, "New Window", true, None::<&str>)?;
                let mut added_new_window = false;
                for item in menu.items()? {
                    if let Some(submenu) = item.as_submenu() {
                        if submenu.text().ok().as_deref() == Some("File") {
                            submenu.insert(&new_window, 0)?;
                            added_new_window = true;
                        }
                        for (position, child) in submenu.items()?.into_iter().enumerate().rev() {
                            if let Some(predefined) = child.as_predefined_menuitem() {
                                let text = predefined.text()?;
                                if text == close_text {
                                    submenu.remove(&child)?;
                                } else if text == minimize_text {
                                    submenu.remove(&child)?;
                                    submenu.insert(&minimize, position)?;
                                }
                            }
                        }
                    }
                }
                if !added_new_window {
                    for item in menu.items()? {
                        let Some(submenu) = item.as_submenu() else {
                            continue;
                        };
                        if submenu.text().ok().as_deref() == Some("Window") {
                            submenu.insert(&new_window, 0)?;
                            break;
                        }
                    }
                }
                app.set_menu(menu)?;
                app.on_menu_event(|app, event| {
                    if event.id() == NEW_WINDOW_ID {
                        let _ = webview::create_window(app);
                        return;
                    }
                    if event.id() != MINIMIZE_ID {
                        return;
                    }
                    if let Some(window) = webview::primary_window(app) {
                        let _ = window.minimize();
                    }
                });
            }
            // The engine starts with the app, in parallel with the webview loading the UI.
            app.state::<EngineState>().prestart();
            webview_watch::spawn(app.handle().clone());
            // An AppImage's bundled binary lives in a mount that goes away with the app, so the
            // copy a service runs is refreshed here; the copies outside the app catch up with the
            // engine this build ships, which is what makes an update reach them (#3487).
            std::thread::spawn(service::catch_up_after_update);
            // The per-OS setup steps (docs/system-setup.md), once for each version of the app.
            let version = app.package_info().version.to_string();
            crash_report::CrashReporter::global().set_version(version.clone());
            std::thread::spawn(move || system_setup::run_after_update(&version));
            // Link previews cached by earlier runs: the expired and the oldest past the cap go.
            if let Ok(dir) = app.path().app_cache_dir() {
                std::thread::spawn(move || links::sweep(&dir.join("links")));
            }
            // Built here, not from the config, so it renders at 120 Hz where the display does.
            webview::build_main_window(app)?;
            let handle = app.handle().clone();
            app.deep_link()
                .on_open_url(move |_| webview::show_main_window(&handle));
            #[cfg(any(target_os = "linux", target_os = "windows"))]
            app.deep_link().register_all()?;
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
        }
    });
}
