//! Acting on an app behind the user's windows: its window is made key without
//! being raised before the input, and if the app brings itself to the front
//! anyway, the app the user was in is put back in front after.

use super::ffi::CGPoint;
use super::{ax, capture, input, skylight};
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use std::time::Duration;

const FOCUS_SETTLE: Duration = Duration::from_millis(50);

/// One background action: what was in front before it.
pub struct Session {
    pid: i32,
    front_before: Option<i32>,
}

impl Session {
    /// Get `pid`'s window ready for background input. Does nothing for input
    /// sent in the foreground.
    pub fn begin(target: input::Target, pid: i32, window: Option<u32>) -> Option<Session> {
        if target == input::Target::Foreground {
            return None;
        }
        let front_before = frontmost_pid();
        if front_before != Some(pid)
            && let Some(window) = window.or_else(|| key_window(pid))
            && skylight::focus_without_raise(pid, window)
        {
            std::thread::sleep(FOCUS_SETTLE);
        }
        Some(Session { pid, front_before })
    }

    /// Put the user's app back in front if the target took it.
    pub fn end(self) {
        let Some(before) = self.front_before.filter(|before| *before != self.pid) else {
            return;
        };
        std::thread::sleep(FOCUS_SETTLE);
        if frontmost_pid() == Some(self.pid)
            && let Ok(app) = ax::Element::app(before)
        {
            let _ = app.bring_to_front();
        }
    }
}

/// The window an action lands in, with its top-left corner: `window` when
/// given, else the app's focused window.
pub fn window(pid: i32, window: Option<u32>) -> Option<input::Window> {
    let id = window.or_else(|| key_window(pid))?;
    let bounds = capture::windows()
        .into_iter()
        .find(|found| found.window_id == id)?
        .bounds;
    Some(input::Window {
        id,
        origin: CGPoint {
            x: bounds.origin.x,
            y: bounds.origin.y,
        },
    })
}

fn key_window(pid: i32) -> Option<u32> {
    ax::Element::app(pid)
        .ok()
        .and_then(|app| app.focused())
        .and_then(|element| element.window())
        .and_then(|window| window.window_id())
        .or_else(|| {
            capture::windows()
                .into_iter()
                .find(|found| found.pid == pid)
                .map(|found| found.window_id)
        })
}

fn frontmost_pid() -> Option<i32> {
    let class = AnyClass::get(c"NSWorkspace")?;
    // SAFETY: NSWorkspace's selectors; a nil application is handled.
    unsafe {
        let workspace: Retained<AnyObject> = msg_send![class, sharedWorkspace];
        let app: Option<Retained<AnyObject>> = msg_send![&workspace, frontmostApplication];
        let pid: i32 = msg_send![&app?, processIdentifier];
        Some(pid)
    }
}
