//! macOS desktop control: Accessibility for reading and acting by ref,
//! events posted to the target process for keys, scrolls and pixel clicks,
//! ScreenCaptureKit for window screenshots, and AppKit for the agent's cursor.

mod ax;
mod capture;
mod ffi;
mod input;
mod ui;

use super::{MAX_ELEMENTS, Op, keys};
use ffi::{CGPoint, CGRect};
use objc2::msg_send;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{AnyClass, AnyObject};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::HashMap;

pub use ui::run_main_thread;

/// The geometry of the latest screenshot of one window: its bounds in points
/// and its size in pixels, to turn screenshot pixels back into points.
#[derive(Clone, Copy)]
struct ShotGeometry {
    bounds: CGRect,
    width: f64,
    height: f64,
}

#[derive(Default)]
struct State {
    refs: HashMap<i32, Vec<ax::Element>>,
    shots: HashMap<(i32, u32), ShotGeometry>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

/// Answer one request. Runs on the worker's request thread.
pub fn handle(op: Op) -> Result<Value, String> {
    autoreleasepool(|_| dispatch(op))
}

fn dispatch(op: Op) -> Result<Value, String> {
    match op {
        Op::Status => Ok(status(false)),
        Op::RequestPermissions => Ok(status(true)),
        Op::Windows => Ok(json!({
            "windows": capture::windows().iter().map(capture::Window::json).collect::<Vec<_>>(),
        })),
        Op::App { pid } => app(pid),
        Op::Activity { label } => {
            ui::set_activity(label);
            Ok(json!({}))
        }
        Op::Screenshot { pid, window_id } => screenshot(pid, window_id),
        Op::Snapshot {
            pid,
            window_id,
            max_elements,
        } => {
            needs_accessibility()?;
            let limit = max_elements.unwrap_or(300).clamp(1, MAX_ELEMENTS);
            let (answer, refs) = ax::snapshot(pid, window_id, limit)?;
            STATE.with(|state| state.borrow_mut().refs.insert(pid, refs));
            Ok(answer)
        }
        Op::Click {
            pid,
            element,
            window_id,
            x,
            y,
            button,
            count,
        } => {
            needs_accessibility()?;
            let right = match button.as_deref() {
                None | Some("left") => false,
                Some("right") => true,
                Some(other) => return Err(format!("unknown button '{other}'")),
            };
            click(pid, element, window_id, x, y, right, count.unwrap_or(1))
        }
        Op::Type { pid, element, text } => {
            needs_accessibility()?;
            type_text(pid, element, &text)
        }
        Op::Key { pid, key } => {
            needs_accessibility()?;
            let combo = keys::parse(&key)?;
            input::key(pid, combo)?;
            Ok(json!({"pressed": key}))
        }
        Op::Scroll {
            pid,
            element,
            window_id,
            x,
            y,
            dx,
            dy,
        } => {
            needs_accessibility()?;
            let (point, _) = target_point(pid, element.as_deref(), window_id, x, y)?;
            ui::glide(point);
            input::scroll(pid, point, dx, dy)?;
            Ok(json!({"scrolled": {"dx": dx, "dy": dy}}))
        }
    }
}

pub(super) fn status(prompt: bool) -> Value {
    let accessibility = ax::trusted(prompt);
    // SAFETY: neither call has preconditions.
    let screen_recording = unsafe {
        if prompt && !ffi::CGPreflightScreenCaptureAccess() {
            ffi::CGRequestScreenCaptureAccess()
        } else {
            ffi::CGPreflightScreenCaptureAccess()
        }
    };
    json!({
        "platform": "macos",
        "supported": true,
        "reason": Value::Null,
        "accessibility": accessibility,
        "screen_recording": screen_recording,
        "executable": std::env::current_exe().ok().map(|path| path.display().to_string()),
    })
}

fn needs_accessibility() -> Result<(), String> {
    if ax::trusted(false) {
        Ok(())
    } else {
        Err(ax::describe(-25211))
    }
}

fn app(pid: i32) -> Result<Value, String> {
    let class = AnyClass::get(c"NSRunningApplication").ok_or("AppKit is unavailable")?;
    // SAFETY: the selectors are NSRunningApplication's; nil answers are handled.
    unsafe {
        let running: Option<Retained<AnyObject>> =
            msg_send![class, runningApplicationWithProcessIdentifier: pid];
        let running = running.ok_or_else(|| format!("No app runs with pid {pid}"))?;
        let name: Option<Retained<AnyObject>> = msg_send![&running, localizedName];
        let bundle: Option<Retained<AnyObject>> = msg_send![&running, bundleIdentifier];
        Ok(json!({
            "pid": pid,
            "name": name.as_deref().map(capture::ns_text),
            "bundle_id": bundle.as_deref().map(capture::ns_text),
        }))
    }
}

fn screenshot(pid: i32, window_id: u32) -> Result<Value, String> {
    let listed = capture::windows()
        .into_iter()
        .find(|window| window.window_id == window_id)
        .ok_or_else(|| format!("Window {window_id} is not on screen; list windows again"))?;
    if listed.pid != pid {
        return Err(format!("Window {window_id} belongs to another app"));
    }
    let shot = capture::screenshot(window_id)?;
    STATE.with(|state| {
        state.borrow_mut().shots.insert(
            (pid, window_id),
            ShotGeometry {
                bounds: listed.bounds,
                width: f64::from(shot.width),
                height: f64::from(shot.height),
            },
        )
    });
    Ok(shot.json(pid, window_id))
}

fn with_ref<T>(
    pid: i32,
    element: &str,
    act: impl FnOnce(&ax::Element) -> Result<T, String>,
) -> Result<T, String> {
    let index = element
        .strip_prefix('e')
        .and_then(|n| n.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .ok_or_else(|| format!("'{element}' is not a ref such as e12"))?;
    STATE.with(|state| {
        let state = state.borrow();
        let refs = state
            .refs
            .get(&pid)
            .ok_or("Take a computer_snapshot of that app first")?;
        let found = refs
            .get(index - 1)
            .ok_or_else(|| format!("{element} is not in the latest snapshot"))?;
        found.role()?;
        act(found)
    })
}

/// The global point an action targets, and the window it lands in when known.
fn target_point(
    pid: i32,
    element: Option<&str>,
    window_id: Option<u32>,
    x: Option<f64>,
    y: Option<f64>,
) -> Result<(CGPoint, Option<u32>), String> {
    if let Some(element) = element {
        return with_ref(pid, element, |found| {
            let frame = found
                .frame()
                .ok_or("That element has no position on screen")?;
            let window = found.window().and_then(|window| window.window_id());
            Ok((frame.center(), window))
        });
    }
    let (Some(x), Some(y)) = (x, y) else {
        return Err("Pass a ref from computer_snapshot, or x and y with window_id".into());
    };
    let window_id =
        window_id.ok_or("x and y need the window_id of the screenshot they come from")?;
    let shot = STATE
        .with(|state| state.borrow().shots.get(&(pid, window_id)).copied())
        .ok_or("Take a computer_screenshot of that window first; x and y are its pixels")?;
    if !(0.0..=shot.width).contains(&x) || !(0.0..=shot.height).contains(&y) {
        return Err(format!(
            "x and y must be inside the {}x{} screenshot",
            shot.width, shot.height
        ));
    }
    Ok((
        CGPoint {
            x: shot.bounds.origin.x + x / shot.width * shot.bounds.size.width,
            y: shot.bounds.origin.y + y / shot.height * shot.bounds.size.height,
        },
        Some(window_id),
    ))
}

fn click(
    pid: i32,
    element: Option<String>,
    window_id: Option<u32>,
    x: Option<f64>,
    y: Option<f64>,
    right: bool,
    count: u8,
) -> Result<Value, String> {
    let (point, window) = target_point(pid, element.as_deref(), window_id, x, y)?;
    ui::glide(point);
    if let Some(element) = &element
        && !right
        && count == 1
    {
        let pressed = with_ref(pid, element, |found| {
            if found.actions().iter().any(|action| action == "AXPress") {
                found.perform("AXPress").map(|()| true)
            } else {
                Ok(false)
            }
        })?;
        if pressed {
            return Ok(json!({"clicked": element, "how": "accessibility"}));
        }
    }
    input::click(pid, point, window, right, count)?;
    Ok(json!({
        "clicked": element.unwrap_or_else(|| format!("{:.0},{:.0}", point.x, point.y)),
        "how": "event",
    }))
}

fn type_text(pid: i32, element: Option<String>, text: &str) -> Result<Value, String> {
    if let Some(element) = &element {
        let point = with_ref(pid, element, |found| {
            if found.is_secure() {
                return Err("That is a password field; hand it to the user instead".to_owned());
            }
            found.focus()?;
            Ok(found.frame().map(|frame| frame.center()))
        })?;
        if let Some(point) = point {
            ui::glide(point);
        }
    }
    let app = ax::Element::app(pid)?;
    let focused = app.focused();
    if focused.as_ref().is_some_and(ax::Element::is_secure) {
        return Err("The focused field is a password field; hand it to the user instead".into());
    }
    if let Some(focused) = &focused
        && focused.insert_text(text)?
    {
        return Ok(json!({"typed": text.chars().count(), "how": "accessibility"}));
    }
    input::type_text(pid, text)?;
    Ok(json!({"typed": text.chars().count(), "how": "event"}))
}
