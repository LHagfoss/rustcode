//! Mouse, keyboard and scroll events. In the background they are posted to
//! one app's process, so they reach it without moving the user's pointer or
//! taking focus. Apps that ignore such events (Chromium and Electron apps)
//! get them in the foreground, through the system's input stream, after the
//! app is brought to the front.

use super::ffi::{self, CGPoint, Cf};
use crate::computer_worker::keys::Combo;
use std::time::Duration;

const LEFT_DOWN: u32 = 1;
const LEFT_UP: u32 = 2;
const RIGHT_DOWN: u32 = 3;
const RIGHT_UP: u32 = 4;
const LEFT_BUTTON: u32 = 0;
const RIGHT_BUTTON: u32 = 1;
const CLICK_STATE: u32 = 1;
const WINDOW_UNDER_POINTER: u32 = 91;
const WINDOW_THAT_CAN_HANDLE: u32 = 92;
const SCROLL_UNIT_LINE: u32 = 1;
const UNICODE_CHUNK: usize = 20;
const HID_EVENT_TAP: u32 = 0;

/// Where input goes: one app's process in the background, or the system's
/// input stream, which reaches whichever app is in front.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Background(i32),
    Foreground,
}

fn post(target: Target, event: ffi::CGEventRef) -> Result<(), String> {
    let owned = Cf::created(event.cast_const()).ok_or("Could not create the input event")?;
    // SAFETY: `owned` is a live CGEvent.
    unsafe {
        match target {
            Target::Background(pid) => ffi::CGEventPostToPid(pid, owned.get().cast_mut()),
            Target::Foreground => ffi::CGEventPost(HID_EVENT_TAP, owned.get().cast_mut()),
        }
    };
    Ok(())
}

/// Click at a global point, `count` times, with the left or right button.
pub fn click(
    target: Target,
    point: CGPoint,
    window_id: Option<u32>,
    right: bool,
    count: u8,
) -> Result<(), String> {
    let (down, up, button) = if right {
        (RIGHT_DOWN, RIGHT_UP, RIGHT_BUTTON)
    } else {
        (LEFT_DOWN, LEFT_UP, LEFT_BUTTON)
    };
    for click in 1..=count.clamp(1, 3) {
        for kind in [down, up] {
            // SAFETY: creating an event has no preconditions; a null source is allowed.
            let event =
                unsafe { ffi::CGEventCreateMouseEvent(std::ptr::null(), kind, point, button) };
            if event.is_null() {
                return Err("Could not create the click".into());
            }
            // SAFETY: `event` is a live CGEvent.
            unsafe {
                ffi::CGEventSetIntegerValueField(event, CLICK_STATE, i64::from(click));
                if let Some(window) = window_id {
                    ffi::CGEventSetIntegerValueField(
                        event,
                        WINDOW_UNDER_POINTER,
                        i64::from(window),
                    );
                    ffi::CGEventSetIntegerValueField(
                        event,
                        WINDOW_THAT_CAN_HANDLE,
                        i64::from(window),
                    );
                }
            }
            post(target, event)?;
            std::thread::sleep(Duration::from_millis(30));
        }
    }
    Ok(())
}

/// Press and release one key combo.
pub fn key(target: Target, combo: Combo) -> Result<(), String> {
    for down in [true, false] {
        // SAFETY: as above.
        let event = unsafe { ffi::CGEventCreateKeyboardEvent(std::ptr::null(), combo.code, down) };
        if event.is_null() {
            return Err("Could not create the key press".into());
        }
        // SAFETY: `event` is a live CGEvent.
        unsafe { ffi::CGEventSetFlags(event, combo.flags) };
        post(target, event)?;
        std::thread::sleep(Duration::from_millis(15));
    }
    Ok(())
}

/// Type `text` as key events carrying the characters themselves.
pub fn type_text(target: Target, text: &str) -> Result<(), String> {
    let units: Vec<u16> = text.encode_utf16().collect();
    for chunk in units.chunks(UNICODE_CHUNK) {
        for down in [true, false] {
            // SAFETY: as above.
            let event = unsafe { ffi::CGEventCreateKeyboardEvent(std::ptr::null(), 0, down) };
            if event.is_null() {
                return Err("Could not create the typing event".into());
            }
            // SAFETY: `chunk` outlives the call, which copies it.
            unsafe { ffi::CGEventKeyboardSetUnicodeString(event, chunk.len(), chunk.as_ptr()) };
            post(target, event)?;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}

/// Scroll at a global point; positive `dy` scrolls down, positive `dx` right.
pub fn scroll(target: Target, point: CGPoint, dx: i32, dy: i32) -> Result<(), String> {
    // SAFETY: as above.
    let event = unsafe {
        ffi::CGEventCreateScrollWheelEvent2(
            std::ptr::null(),
            SCROLL_UNIT_LINE,
            2,
            -dy.clamp(-50, 50),
            -dx.clamp(-50, 50),
            0,
        )
    };
    if event.is_null() {
        return Err("Could not create the scroll".into());
    }
    // SAFETY: `event` is a live CGEvent.
    unsafe { ffi::CGEventSetLocation(event, point) };
    post(target, event)
}
