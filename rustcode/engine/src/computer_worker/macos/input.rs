//! Mouse, keyboard and scroll events. In the background they are posted to
//! one app's process through WindowServer (`skylight.rs`), so they reach it
//! behind the user's windows without moving the user's pointer. Clicks follow
//! the sequence Chromium and Electron accept as a real gesture. Apps that
//! still ignore them get them in the foreground, through the system's input
//! stream, after the app is brought to the front.

use super::ffi::{self, CGPoint, Cf};
use super::skylight;
use crate::computer_worker::keys::Combo;
use std::time::Duration;

const LEFT_DOWN: u32 = 1;
const LEFT_UP: u32 = 2;
const RIGHT_DOWN: u32 = 3;
const RIGHT_UP: u32 = 4;
const MOUSE_MOVED: u32 = 5;
const LEFT_BUTTON: u32 = 0;
const RIGHT_BUTTON: u32 = 1;
const GESTURE_PHASE: u32 = 0;
const CLICK_STATE: u32 = 1;
const BUTTON_NUMBER: u32 = 3;
const SUBTYPE: u32 = 7;
const TARGET_PID: u32 = 40;
const WINDOW_NUMBER: u32 = 51;
const CLICK_GROUP: u32 = 58;
const WINDOW_UNDER_POINTER: u32 = 91;
const WINDOW_THAT_CAN_HANDLE: u32 = 92;
const TOUCH_SUBTYPE: i64 = 3;
const PHASE_MOVE: i64 = 2;
const PHASE_PRIMER_DOWN: i64 = 1;
const PHASE_PRIMER_UP: i64 = 2;
const PHASE_TARGET: i64 = 3;
const OFF_SCREEN: CGPoint = CGPoint { x: -1.0, y: -1.0 };
const MOVE_SETTLE: Duration = Duration::from_millis(15);
const PRIMER_GAP: Duration = Duration::from_millis(100);
const PRESS_GAP: Duration = Duration::from_millis(30);
const REPEAT_GAP: Duration = Duration::from_millis(80);
const KEY_GAP: Duration = Duration::from_millis(8);
const SCROLL_UNIT_LINE: u32 = 1;
const UNICODE_CHUNK: usize = 20;
const HID_EVENT_TAP: u32 = 0;
const HID_SYSTEM_STATE: i32 = 1;

/// Events carry the keyboard's own source, as typed keys do: some apps
/// ignore events created without one.
struct Source(Option<Cf>);

impl Source {
    fn new() -> Self {
        // SAFETY: creating a source has no preconditions; `Cf` releases it.
        Self(Cf::created(
            unsafe { ffi::CGEventSourceCreate(HID_SYSTEM_STATE) }.cast_const(),
        ))
    }

    fn get(&self) -> *const std::ffi::c_void {
        self.0.as_ref().map_or(std::ptr::null(), Cf::get)
    }
}

/// Where input goes: one app's process in the background, or the system's
/// input stream, which reaches whichever app is in front.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Background(i32),
    Foreground,
}

fn post(target: Target, event: ffi::CGEventRef, keyboard: bool) -> Result<(), String> {
    let owned = Cf::created(event.cast_const()).ok_or("Could not create the input event")?;
    let event = owned.get().cast_mut();
    match target {
        Target::Background(pid) => skylight::post(pid, event, keyboard),
        // SAFETY: `event` is a live CGEvent.
        Target::Foreground => unsafe { ffi::CGEventPost(HID_EVENT_TAP, event) },
    }
    Ok(())
}

/// The window a click lands in: its id and its top-left corner on screen.
#[derive(Clone, Copy, Debug)]
pub struct Window {
    pub id: u32,
    pub origin: CGPoint,
}

/// One event of a click.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Step {
    kind: u32,
    at: CGPoint,
    phase: i64,
    click_state: i64,
    pause: Duration,
}

/// The events of a click at `point`, `count` times. In the background a left
/// click is preceded by a move to the point and a press off screen, which
/// Chromium's user-activation check needs before it takes the real press.
fn click_steps(point: CGPoint, right: bool, count: u8, background: bool) -> Vec<Step> {
    let (down, up) = if right {
        (RIGHT_DOWN, RIGHT_UP)
    } else {
        (LEFT_DOWN, LEFT_UP)
    };
    let mut steps = Vec::new();
    if background {
        steps.push(Step {
            kind: MOUSE_MOVED,
            at: point,
            phase: PHASE_MOVE,
            click_state: 0,
            pause: MOVE_SETTLE,
        });
        if !right {
            steps.push(Step {
                kind: down,
                at: OFF_SCREEN,
                phase: PHASE_PRIMER_DOWN,
                click_state: 1,
                pause: Duration::from_millis(1),
            });
            steps.push(Step {
                kind: up,
                at: OFF_SCREEN,
                phase: PHASE_PRIMER_UP,
                click_state: 1,
                pause: PRIMER_GAP,
            });
        }
    }
    let count = count.clamp(1, 3);
    for click in 1..=count {
        steps.push(Step {
            kind: down,
            at: point,
            phase: PHASE_TARGET,
            click_state: i64::from(click),
            pause: PRESS_GAP,
        });
        steps.push(Step {
            kind: up,
            at: point,
            phase: PHASE_TARGET,
            click_state: i64::from(click),
            pause: if click < count {
                REPEAT_GAP
            } else {
                Duration::ZERO
            },
        });
    }
    steps
}

/// Click at a global point, `count` times, with the left or right button.
pub fn click(
    target: Target,
    point: CGPoint,
    window: Option<Window>,
    right: bool,
    count: u8,
) -> Result<(), String> {
    let button = if right { RIGHT_BUTTON } else { LEFT_BUTTON };
    let background = matches!(target, Target::Background(_));
    let group = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| i64::from(since.subsec_nanos()));
    let source = Source::new();
    for step in click_steps(point, right, count, background) {
        // SAFETY: creating an event has no preconditions; a null source is allowed.
        let event =
            unsafe { ffi::CGEventCreateMouseEvent(source.get(), step.kind, step.at, button) };
        if event.is_null() {
            return Err("Could not create the click".into());
        }
        stamp(event, &step, target, window, group, button);
        post(target, event, false)?;
        std::thread::sleep(step.pause);
    }
    Ok(())
}

fn stamp(
    event: ffi::CGEventRef,
    step: &Step,
    target: Target,
    window: Option<Window>,
    group: i64,
    button: u32,
) {
    skylight::set_field(event, CLICK_STATE, step.click_state);
    if let Some(window) = window {
        for field in [WINDOW_UNDER_POINTER, WINDOW_THAT_CAN_HANDLE] {
            skylight::set_field(event, field, i64::from(window.id));
        }
    }
    let Target::Background(pid) = target else {
        return;
    };
    skylight::set_field(event, GESTURE_PHASE, step.phase);
    skylight::set_field(event, BUTTON_NUMBER, i64::from(button));
    skylight::set_field(event, SUBTYPE, TOUCH_SUBTYPE);
    skylight::set_field(event, TARGET_PID, i64::from(pid));
    skylight::set_field(event, CLICK_GROUP, group);
    if let Some(window) = window {
        skylight::set_field(event, WINDOW_NUMBER, i64::from(window.id));
        skylight::set_window_location(event, window_location(step.at, window.origin));
    } else {
        skylight::set_window_location(event, step.at);
    }
}

/// Where in the window an event lands, as WindowServer reads the stamp:
/// window-local from macOS 27, the screen point before.
fn window_location(point: CGPoint, origin: CGPoint) -> CGPoint {
    if point == OFF_SCREEN || !stamps_are_window_local() {
        return point;
    }
    CGPoint {
        x: point.x - origin.x,
        y: point.y - origin.y,
    }
}

fn stamps_are_window_local() -> bool {
    static LOCAL: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LOCAL.get_or_init(|| macos_major().is_some_and(|major| major >= 27))
}

fn macos_major() -> Option<u32> {
    let mut buffer = [0u8; 32];
    let mut size = buffer.len();
    // SAFETY: the buffer and its size outlive the call, which writes at most `size` bytes.
    let read = unsafe {
        libc::sysctlbyname(
            c"kern.osproductversion".as_ptr(),
            buffer.as_mut_ptr().cast(),
            &raw mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if read != 0 {
        return None;
    }
    let text = std::str::from_utf8(&buffer[..size]).ok()?;
    text.trim_end_matches('\0').split('.').next()?.parse().ok()
}

/// Press and release one key combo.
pub fn key(target: Target, combo: Combo) -> Result<(), String> {
    let source = Source::new();
    for down in [true, false] {
        // SAFETY: as above.
        let event = unsafe { ffi::CGEventCreateKeyboardEvent(source.get(), combo.code, down) };
        if event.is_null() {
            return Err("Could not create the key press".into());
        }
        // SAFETY: `event` is a live CGEvent.
        unsafe { ffi::CGEventSetFlags(event, combo.flags) };
        post(target, event, true)?;
        std::thread::sleep(KEY_GAP);
    }
    Ok(())
}

/// Type `text` as key events carrying the characters themselves.
pub fn type_text(target: Target, text: &str) -> Result<(), String> {
    let units: Vec<u16> = text.encode_utf16().collect();
    let source = Source::new();
    for chunk in units.chunks(UNICODE_CHUNK) {
        for down in [true, false] {
            // SAFETY: as above.
            let event = unsafe { ffi::CGEventCreateKeyboardEvent(source.get(), 0, down) };
            if event.is_null() {
                return Err("Could not create the typing event".into());
            }
            // SAFETY: `chunk` outlives the call, which copies it.
            unsafe { ffi::CGEventKeyboardSetUnicodeString(event, chunk.len(), chunk.as_ptr()) };
            post(target, event, true)?;
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
    post(target, event, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    const POINT: CGPoint = CGPoint { x: 400.0, y: 300.0 };

    #[test]
    fn a_background_left_click_moves_primes_off_screen_then_presses() {
        let steps = click_steps(POINT, false, 1, true);
        let kinds: Vec<u32> = steps.iter().map(|step| step.kind).collect();
        assert_eq!(kinds, [MOUSE_MOVED, LEFT_DOWN, LEFT_UP, LEFT_DOWN, LEFT_UP]);
        assert_eq!(steps[1].at, OFF_SCREEN);
        assert_eq!(steps[3].at, POINT);
        assert_eq!(steps[3].phase, PHASE_TARGET);
    }

    #[test]
    fn a_foreground_double_click_is_two_counted_presses() {
        let steps = click_steps(POINT, false, 2, false);
        let states: Vec<i64> = steps.iter().map(|step| step.click_state).collect();
        assert_eq!(states, [1, 1, 2, 2]);
        assert!(steps.iter().all(|step| step.at == POINT));
    }

    #[test]
    fn a_background_right_click_skips_the_primer() {
        let kinds: Vec<u32> = click_steps(POINT, true, 1, true)
            .iter()
            .map(|step| step.kind)
            .collect();
        assert_eq!(kinds, [MOUSE_MOVED, RIGHT_DOWN, RIGHT_UP]);
    }
}
