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

use serde::Serialize;

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

/// Builds the main window from its tauri.conf.json entry, on a web view
/// configuration that renders at the display's full rate.
pub fn build_main_window(app: &tauri::App) -> tauri::Result<tauri::WebviewWindow> {
    let config = app
        .config()
        .app
        .windows
        .iter()
        .find(|w| w.label == "main")
        .cloned()
        .unwrap_or_default();
    let builder = tauri::WebviewWindowBuilder::from_config(app.handle(), &config)?;
    #[cfg(target_os = "macos")]
    let builder = match macos::full_frame_rate_configuration() {
        Some(configuration) => builder.with_webview_configuration(configuration),
        None => builder,
    };
    builder.build()
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
