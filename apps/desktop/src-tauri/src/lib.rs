//! RustCode desktop shell. The webview talks ACP to the engine sidecar through
//! the `engine_*` commands; the other commands are the few native helpers the
//! UI needs (links, Finder, a harness login in Terminal, git changes). Every
//! argument from the webview is validated here.

mod engine;
mod links;
mod native;
mod pty;
mod smartui;
mod threads;

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
        .manage(Arc::new(Engine::default()) as EngineState)
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
            native::open_url,
            native::reveal_path,
            native::open_login_terminal,
            native::git_changes,
            native::read_image,
            native::open_terminal_at,
            native::reduce_transparency,
            links::link_meta,
            threads::thread_rename,
            threads::thread_trash,
            pty::pty_open,
            pty::pty_write,
            pty::pty_resize,
            pty::pty_close,
            smartui::smartui_page,
        ])
        .build(tauri::generate_context!())
        .expect("error while building the RustCode desktop app");
    app.run(|handle, event| {
        if let RunEvent::Exit = event {
            // The engine closes its sessions when its stdin closes.
            handle.state::<EngineState>().stop();
            handle.state::<pty::Ptys>().close_all();
        }
    });
}
