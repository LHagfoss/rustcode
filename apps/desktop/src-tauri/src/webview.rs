//! The native web view's frame rate (docs/desktop.md › Native frame rate).
//!
//! WebKit's "Prefer Page Rendering Updates near 60fps" feature is on by
//! default, so on a 120 Hz (ProMotion) display a WKWebView runs
//! requestAnimationFrame, CSS animations and its rendering updates at 60 fps.
//! Safari turns it off on high-refresh displays; this does the same for the
//! app's web view. The setting is private WebKit API, reached through the
//! `_WKFeature` list, and every step checks the selector exists first, so a
//! WebKit without it leaves the web view as it was instead of crashing.
//!
//! WebKit reads it when the page is created: changed on a live web view it
//! does nothing until the page is recreated. So the main window is built here
//! (`"create": false` in tauri.conf.json) on a configuration that already has it.
//!
//! Low Power Mode is a separate cap (30 fps) WebKit applies whatever this
//! says; `display_info` reports it so the dev overlay can tell the two apart.

use serde::{Deserialize, Serialize};
use tauri::Manager;

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DisplayInfo {
    /// The refresh rate of the screen the window is on (0 when unknown).
    pub max_fps: i64,
    /// macOS Low Power Mode, under which WebKit renders at 30 fps.
    pub low_power_mode: bool,
    /// Whether the web view still prefers rendering near 60 fps (None when unknown).
    pub near_60fps: Option<bool>,
}

/// The window size as the app last closed with (docs/desktop.md), so a restart opens the same
/// shape instead of the config's default. Only the size: position, screens and fullscreen do not
/// belong to the window's own state here.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
struct WindowSize {
    width: f64,
    height: f64,
}

/// The smallest window the config allows, so a bad file cannot open something unusable.
const MIN_SIZE: (f64, f64) = (640.0, 480.0);
/// A window larger than any display is as wrong as one too small to use.
const MAX_SIZE: (f64, f64) = (8192.0, 8192.0);

fn size_file(app: &tauri::AppHandle) -> Option<std::path::PathBuf> {
    app.path()
        .app_config_dir()
        .ok()
        .map(|dir| dir.join("window.json"))
}

fn clamped(width: f64, height: f64) -> (f64, f64) {
    let fit = |value: f64, (min, max): (f64, f64)| {
        if value.is_finite() {
            value.clamp(min, max)
        } else {
            min
        }
    };
    (
        fit(width, (MIN_SIZE.0, MAX_SIZE.0)),
        fit(height, (MIN_SIZE.1, MAX_SIZE.1)),
    )
}

/// The size the window last had, when one was saved and it still makes sense.
fn read_size(path: &std::path::Path) -> Option<(f64, f64)> {
    let text = std::fs::read_to_string(path).ok()?;
    let saved: WindowSize = serde_json::from_str(&text).ok()?;
    Some(clamped(saved.width, saved.height))
}

fn saved_size(app: &tauri::AppHandle) -> Option<(f64, f64)> {
    read_size(&size_file(app)?)
}

/// Remember the window's size for the next launch.
pub fn remember_size(app: &tauri::AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let Ok(size) = window.inner_size() else {
        return;
    };
    // Logical points, so a window moved between displays with different scales keeps its shape.
    let scale = window.scale_factor().unwrap_or(1.0);
    let (width, height) = clamped(
        f64::from(size.width) / scale,
        f64::from(size.height) / scale,
    );
    let (Some(path), Ok(text)) = (
        size_file(app),
        serde_json::to_string(&WindowSize { width, height }),
    ) else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // A failed write only loses the remembered size.
    let _ = std::fs::write(path, text);
}

/// Builds the main window from its tauri.conf.json entry, on a web view
/// configuration that renders at the display's full rate, at the size it last had.
pub fn build_main_window(app: &tauri::App) -> tauri::Result<tauri::WebviewWindow> {
    let config = app
        .config()
        .app
        .windows
        .iter()
        .find(|w| w.label == "main")
        .cloned()
        .unwrap_or_default();
    let mut builder = tauri::WebviewWindowBuilder::from_config(app.handle(), &config)?;
    if let Some((width, height)) = saved_size(app.handle()) {
        builder = builder.inner_size(width, height);
    }
    #[cfg(target_os = "macos")]
    let builder = match macos::full_frame_rate_configuration() {
        Some(configuration) => builder.with_webview_configuration(configuration),
        None => builder,
    };
    let window = builder.build()?;
    apply_material(&window);
    // Closing the window (macOS keeps the app running) is the last chance to remember its size.
    let handle = app.handle().clone();
    window.on_window_event(move |event| {
        if matches!(event, tauri::WindowEvent::CloseRequested { .. }) {
            remember_size(&handle);
        }
    });
    Ok(window)
}

/// Whether the window's backdrop material (Mica) is on, for the page to let it show through.
static MATERIAL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Windows 11's Mica behind the sidebar (docs/design.md › Platforms). The window is transparent in
/// `tauri.windows.conf.json`, so a failure (Windows 10 has no Mica) must leave the page opaque:
/// the page asks `window_material` and only lets the material through when it is on. Acrylic is
/// not the fallback: it drags and resizes badly on both Windows versions. Other systems have no
/// window material; macOS's comes from `windowEffects` in `tauri.conf.json`.
#[cfg(target_os = "windows")]
fn apply_material(window: &tauri::WebviewWindow) {
    // Not `WebviewWindow::set_effects`: tauri drops the platform's result (`let _ =` around
    // `vibrancy::set_window_effects`, whose Windows side ignores `window_vibrancy`'s), so it returns
    // Ok on Windows 10, where nothing was applied. The page would then paint a see-through sidebar
    // over a window whose only backdrop is the desktop. This call's result is the real answer.
    let applied = window_vibrancy::apply_mica(window, None).is_ok();
    MATERIAL.store(applied, std::sync::atomic::Ordering::Relaxed);
}

/// Only Windows has a window material here; macOS's comes from `tauri.conf.json`.
#[cfg(not(target_os = "windows"))]
fn apply_material(_window: &tauri::WebviewWindow) {}

/// What `window_material` answers: whether the window has its material.
fn material_name(applied: bool) -> &'static str {
    if applied {
        "mica"
    } else {
        "none"
    }
}

/// `"mica"` when the window has its material, else `"none"`.
#[tauri::command]
pub fn window_material() -> &'static str {
    material_name(MATERIAL.load(std::sync::atomic::Ordering::Relaxed))
}

/// Save the window's size on quit too, for a quit that does not come from closing the window.
pub fn remember_size_on_exit(handle: &tauri::AppHandle) {
    remember_size(handle);
}

/// The display's refresh rate, Low Power Mode and the web view's frame-rate preference.
#[tauri::command(async)]
pub fn display_info(window: tauri::WebviewWindow) -> DisplayInfo {
    #[cfg(target_os = "macos")]
    {
        let (tx, rx) = std::sync::mpsc::channel();
        let ns_window = window.ns_window().map(|w| w as usize).unwrap_or(0);
        let _ = window.with_webview(move |webview| {
            // SAFETY: both pointers are live AppKit/WebKit objects, read on the main thread.
            let info = unsafe {
                DisplayInfo {
                    max_fps: macos::screen_max_fps(ns_window as *mut _),
                    low_power_mode: macos::low_power_mode(),
                    near_60fps: macos::near_60fps(webview.inner().cast()),
                }
            };
            let _ = tx.send(info);
        });
        rx.recv_timeout(std::time::Duration::from_secs(2))
            .unwrap_or_default()
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = window;
        DisplayInfo::default()
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject, Sel};
    use objc2::{msg_send, sel, MainThreadMarker};
    use objc2_web_kit::WKWebViewConfiguration;
    use std::ffi::CStr;

    const FEATURE: &CStr = c"PreferPageRenderingUpdatesNear60FPSEnabled";

    unsafe fn responds(object: *mut AnyObject, selector: Sel) -> bool {
        !object.is_null() && msg_send![object, respondsToSelector: selector]
    }

    /// A fresh configuration with the 60 fps preference off (None off the main
    /// thread or when this WebKit has no such feature).
    pub fn full_frame_rate_configuration() -> Option<Retained<WKWebViewConfiguration>> {
        let mtm = MainThreadMarker::new()?;
        // SAFETY: a new configuration, on the main thread; `feature` checks every selector.
        unsafe {
            let configuration = WKWebViewConfiguration::new(mtm);
            let prefs = Retained::as_ptr(&configuration.preferences()) as *mut AnyObject;
            let (prefs, feature) = feature(prefs)?;
            let _: () = msg_send![prefs, _setEnabled: false, forFeature: feature];
            Some(configuration)
        }
    }

    /// The WKPreferences of a web view's configuration.
    unsafe fn preferences(web_view: *mut AnyObject) -> *mut AnyObject {
        if !responds(web_view, sel!(configuration)) {
            return std::ptr::null_mut();
        }
        let config: *mut AnyObject = msg_send![web_view, configuration];
        if !responds(config, sel!(preferences)) {
            return std::ptr::null_mut();
        }
        msg_send![config, preferences]
    }

    /// The preferences and the `_WKFeature` for the 60 fps preference.
    unsafe fn feature(prefs: *mut AnyObject) -> Option<(*mut AnyObject, *mut AnyObject)> {
        let class = AnyClass::get(c"WKPreferences")?;
        let class_object = class as *const AnyClass as *mut AnyObject;
        if !responds(class_object, sel!(_features))
            || !responds(prefs, sel!(_setEnabled:forFeature:))
            || !responds(prefs, sel!(_isEnabledForFeature:))
        {
            return None;
        }
        let features: *mut AnyObject = msg_send![class, _features];
        if features.is_null() {
            return None;
        }
        let count: usize = msg_send![features, count];
        for i in 0..count {
            let item: *mut AnyObject = msg_send![features, objectAtIndex: i];
            if !responds(item, sel!(key)) {
                continue;
            }
            let key: *mut AnyObject = msg_send![item, key];
            if !responds(key, sel!(UTF8String)) {
                continue;
            }
            let chars: *const std::ffi::c_char = msg_send![key, UTF8String];
            if !chars.is_null() && CStr::from_ptr(chars) == FEATURE {
                return Some((prefs, item));
            }
        }
        None
    }

    pub unsafe fn near_60fps(web_view: *mut AnyObject) -> Option<bool> {
        let (prefs, feature) = feature(preferences(web_view))?;
        Some(msg_send![prefs, _isEnabledForFeature: feature])
    }

    pub unsafe fn low_power_mode() -> bool {
        let Some(class) = AnyClass::get(c"NSProcessInfo") else {
            return false;
        };
        let info: *mut AnyObject = msg_send![class, processInfo];
        responds(info, sel!(isLowPowerModeEnabled)) && msg_send![info, isLowPowerModeEnabled]
    }

    pub unsafe fn screen_max_fps(ns_window: *mut AnyObject) -> i64 {
        if !responds(ns_window, sel!(screen)) {
            return 0;
        }
        let screen: *mut AnyObject = msg_send![ns_window, screen];
        if !responds(screen, sel!(maximumFramesPerSecond)) {
            return 0;
        }
        let fps: isize = msg_send![screen, maximumFramesPerSecond];
        fps as i64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("rustcode-window-{}-{name}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        dir.join("window.json")
    }

    #[test]
    fn a_saved_size_is_read_back() {
        let path = temp("round-trip");
        std::fs::write(&path, r#"{"width":1512.0,"height":945.0}"#).unwrap();
        assert_eq!(read_size(&path), Some((1512.0, 945.0)));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_size_too_small_or_nonsense_is_clamped_or_ignored() {
        let path = temp("small");
        std::fs::write(&path, r#"{"width":100.0,"height":50.0}"#).unwrap();
        assert_eq!(read_size(&path), Some((640.0, 480.0)));
        std::fs::write(&path, r#"{"width":999999.0,"height":999999.0}"#).unwrap();
        assert_eq!(read_size(&path), Some((8192.0, 8192.0)));
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(read_size(&path), None);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_missing_file_is_no_size() {
        assert_eq!(read_size(&temp("missing")), None);
    }

    // The page lets the window's backdrop show (transparent body, see-through sidebar) only for
    // `mica`; every other answer has to leave it opaque (lib/material.ts).
    #[test]
    fn the_material_is_only_announced_when_it_took() {
        assert_eq!(material_name(true), "mica");
        assert_eq!(material_name(false), "none");
    }
}
