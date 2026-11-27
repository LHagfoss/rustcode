//! What the user sees while an agent drives this Mac: the agent's own cursor,
//! which glides to each element before it acts, floats gently for a while
//! after, and then rests where it last acted, even across restarts of the
//! app. It steps aside as soon as the user moves their pointer or types, and
//! comes back with the agent's next action. Desktop control stops from
//! Settings, or when the app quits.

use super::ffi::{self, CGPoint, CGRect, CGSize};
use crate::computer_worker::cursor_image;
use crate::computer_worker::cursor_motion::{FRAMES_PER_SECOND, Glide, Point, ease_in_out};
use dispatch2::DispatchQueue;
use objc2::msg_send;
use objc2::rc::{Allocated, Retained, autoreleasepool};
use objc2::runtime::{AnyClass, AnyObject};
use std::cell::RefCell;
use std::ffi::{CString, c_void};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const CURSOR_POINTS: f64 = 24.0;
const FADE_IN_FRAMES: u32 = 8;
const SETTLE: Duration = Duration::from_millis(90);
const PRESS: Duration = Duration::from_millis(110);
const PRESSED_SCALE: f64 = 0.82;
const FLOAT_FRAMES_PER_SECOND: f64 = 30.0;
const FLOAT_PERIOD_SECS: f64 = 4.0;
const FLOAT_SWAY: f64 = 1.6;
const FLOAT_BOB: f64 = 2.0;
const FLOAT_LIFT: f64 = 1.6;
const FLOAT_HOLD_SECS: f64 = 0.5;
const FLOAT_EASE_IN_SECS: f64 = 0.8;
const FLOAT_FOR_SECS: f64 = 30.0;
const FLOAT_EASE_OUT_SECS: f64 = 1.2;
const RESTING_POLL: Duration = Duration::from_millis(250);
const FADE_OUT_FRAMES: u32 = 10;
const USER_GRACE: Duration = Duration::from_millis(600);
const POINTER_JITTER: f64 = 2.0;
const HID_SYSTEM_STATE: i32 = 1;
const KEY_DOWN: u32 = 10;
const ACCESSORY_POLICY: isize = 1;
const BORDERLESS: usize = 0;
const BUFFERED: usize = 2;
const SCREEN_SAVER_LEVEL: isize = 1000;
const ALL_SPACES_STATIONARY_IGNORES_CYCLE: usize = 1 | 16 | 64 | 256;
const SHARING_NONE: usize = 0;
const SCALE_PROPORTIONALLY_UP_OR_DOWN: usize = 3;

struct Ui {
    window: Retained<AnyObject>,
    image: Retained<AnyObject>,
    image_frame: CGRect,
    size: CGSize,
    tip: CGPoint,
    shown: bool,
}

thread_local! {
    static UI: RefCell<Option<Ui>> = const { RefCell::new(None) };
}

/// Where the cursor rests: the last point it acted on.
static POSITION: Mutex<Option<CGPoint>> = Mutex::new(None);
/// Where its tip is drawn now, float included.
static DRAWN: Mutex<Option<CGPoint>> = Mutex::new(None);
/// When it last came to rest, which starts its float.
static RESTED: Mutex<Option<Instant>> = Mutex::new(None);
static MOVING: AtomicBool = AtomicBool::new(false);
/// When the agent last sent input, so its own events are not taken for the
/// user's.
static ACTED: Mutex<Option<Instant>> = Mutex::new(None);
static BEND_LEFT: AtomicBool = AtomicBool::new(true);
static CLOCK: OnceLock<Instant> = OnceLock::new();

fn class(name: &str) -> &'static AnyClass {
    let name = CString::new(name).expect("class names have no NUL");
    AnyClass::get(&name).unwrap_or_else(|| panic!("AppKit class {name:?} is missing"))
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// Run AppKit on the main thread until the user stops the worker. Never
/// returns. `restore` brings the cursor back where it last rested, for the
/// worker the app keeps running.
pub fn run_main_thread(restore: bool) -> ! {
    autoreleasepool(|_| {
        // SAFETY: called once, on the main thread, before the run loop starts.
        unsafe {
            let app: Retained<AnyObject> = msg_send![class("NSApplication"), sharedApplication];
            let _: bool = msg_send![&app, setActivationPolicy: ACCESSORY_POLICY];
            let ui = build();
            UI.with(|slot| *slot.borrow_mut() = Some(ui));
            if let Some(point) = restore.then(saved_position).flatten() {
                *lock(&POSITION) = Some(point);
                std::thread::spawn(move || {
                    appear(point);
                    *lock(&RESTED) = Some(Instant::now());
                });
            }
            std::thread::spawn(float);
            let _: () = msg_send![&app, run];
        }
    });
    std::process::exit(0)
}

unsafe fn build() -> Ui {
    let scale = 2.0;
    let cursor = cursor_image::draw(CURSOR_POINTS, scale);
    let size = CGSize {
        width: f64::from(cursor.width) / scale,
        height: f64::from(cursor.height) / scale,
    };
    let tip = CGPoint {
        x: cursor.tip.0 / scale,
        y: cursor.tip.1 / scale,
    };
    // SAFETY: the caller is on the main thread; every selector is AppKit's.
    unsafe {
        let allocated: Allocated<AnyObject> = msg_send![class("NSWindow"), alloc];
        let window: Retained<AnyObject> = msg_send![
            allocated,
            initWithContentRect: CGRect { origin: CGPoint::default(), size },
            styleMask: BORDERLESS,
            backing: BUFFERED,
            defer: false
        ];
        let clear: Retained<AnyObject> = msg_send![class("NSColor"), clearColor];
        let _: () = msg_send![&window, setOpaque: false];
        let _: () = msg_send![&window, setBackgroundColor: &*clear];
        let _: () = msg_send![&window, setIgnoresMouseEvents: true];
        let _: () = msg_send![&window, setHasShadow: false];
        let _: () = msg_send![&window, setLevel: SCREEN_SAVER_LEVEL];
        let _: () = msg_send![&window, setCollectionBehavior: ALL_SPACES_STATIONARY_IGNORES_CYCLE];
        let _: () = msg_send![&window, setSharingType: SHARING_NONE];
        let _: () = msg_send![&window, setReleasedWhenClosed: false];
        let content: Retained<AnyObject> = msg_send![&window, contentView];

        let data: Retained<AnyObject> = msg_send![
            class("NSData"),
            dataWithBytes: cursor.png.as_ptr().cast::<c_void>(),
            length: cursor.png.len()
        ];
        let allocated: Allocated<AnyObject> = msg_send![class("NSImage"), alloc];
        let image: Retained<AnyObject> = msg_send![allocated, initWithData: &*data];
        let _: () = msg_send![&image, setSize: size];
        let view: Retained<AnyObject> =
            msg_send![class("NSImageView"), imageViewWithImage: &*image];
        let _: () = msg_send![&view, setImageScaling: SCALE_PROPORTIONALLY_UP_OR_DOWN];
        let image_frame = CGRect {
            origin: CGPoint::default(),
            size,
        };
        let _: () = msg_send![&view, setFrame: image_frame];
        let _: () = msg_send![&content, addSubview: &*view];

        Ui {
            window,
            image: view,
            image_frame,
            size,
            tip,
            shown: false,
        }
    }
}

fn primary_height() -> f64 {
    // SAFETY: no preconditions.
    unsafe { ffi::CGDisplayBounds(ffi::CGMainDisplayID()).size.height }
}

/// Glide the agent's cursor to a global point (top-left origin, points)
/// along an eased arc, from where it is drawn (or from the user's pointer
/// the first time), and give the user a moment to see where the agent is
/// about to act. It rests there afterwards.
pub fn glide(point: CGPoint) {
    MOVING.store(true, Ordering::SeqCst);
    acting();
    let from = lock(&DRAWN).or_else(pointer_location).unwrap_or(point);
    let bend_left = BEND_LEFT.fetch_xor(true, Ordering::Relaxed);
    let plan = Glide::plan(motion_point(from), motion_point(point), bend_left);
    appear(from);
    let frame = Duration::from_secs_f64(1.0 / FRAMES_PER_SECOND);
    for step in plan.frames() {
        place(CGPoint {
            x: step.x,
            y: step.y,
        });
        std::thread::sleep(frame);
    }
    *lock(&POSITION) = Some(point);
    save_position(point);
    std::thread::sleep(SETTLE);
    *lock(&RESTED) = Some(Instant::now());
    acting();
    MOVING.store(false, Ordering::SeqCst);
}

/// Note that the agent is sending input now, so the pointer moves and keys it
/// causes do not count as the user's.
pub fn acting() {
    *lock(&ACTED) = Some(Instant::now());
}

/// Show a press at the cursor's tip just before a click lands.
pub fn press() {
    set_pressed(true);
    std::thread::sleep(PRESS);
    set_pressed(false);
}

fn motion_point(point: CGPoint) -> Point {
    Point {
        x: point.x,
        y: point.y,
    }
}

fn pointer_location() -> Option<CGPoint> {
    // SAFETY: a null source is allowed; the event is released by `Cf`.
    let event = unsafe { ffi::CGEventCreate(std::ptr::null()) };
    let owned = ffi::Cf::created(event.cast_const())?;
    // SAFETY: `owned` is a live CGEvent.
    Some(unsafe { ffi::CGEventGetLocation(owned.get().cast_mut()) })
}

fn place(point: CGPoint) {
    *lock(&DRAWN) = Some(point);
    let height = primary_height();
    DispatchQueue::main().exec_async(move || {
        UI.with(|slot| {
            let slot = slot.borrow();
            let Some(ui) = slot.as_ref() else { return };
            let origin = CGPoint {
                x: point.x - ui.tip.x,
                y: height - point.y - ui.size.height + ui.tip.y,
            };
            // SAFETY: this runs on the main thread, which owns the window.
            unsafe {
                let _: () = msg_send![&ui.window, setFrameOrigin: origin];
            }
        });
    });
}

fn set_alpha(alpha: f64) {
    DispatchQueue::main().exec_async(move || {
        UI.with(|slot| {
            let slot = slot.borrow();
            let Some(ui) = slot.as_ref() else { return };
            // SAFETY: this runs on the main thread, which owns the window.
            unsafe {
                let _: () = msg_send![&ui.window, setAlphaValue: alpha];
            }
        });
    });
}

fn set_pressed(pressed: bool) {
    DispatchQueue::main().exec_async(move || {
        UI.with(|slot| {
            let slot = slot.borrow();
            let Some(ui) = slot.as_ref() else { return };
            let base = ui.image_frame;
            let frame = if pressed {
                let size = CGSize {
                    width: base.size.width * PRESSED_SCALE,
                    height: base.size.height * PRESSED_SCALE,
                };
                CGRect {
                    origin: CGPoint {
                        x: base.origin.x + ui.tip.x * (1.0 - PRESSED_SCALE),
                        y: base.origin.y + (base.size.height - ui.tip.y) * (1.0 - PRESSED_SCALE),
                    },
                    size,
                }
            } else {
                base
            };
            // SAFETY: this runs on the main thread, which owns the view.
            unsafe {
                let _: () = msg_send![&ui.image, setFrame: frame];
            }
        });
    });
}

/// Bring the cursor on screen at `point`, fading in, unless it is already up.
fn appear(point: CGPoint) {
    let (sender, receiver) = std::sync::mpsc::channel();
    DispatchQueue::main().exec_async(move || {
        let fresh = UI.with(|slot| {
            let mut slot = slot.borrow_mut();
            let Some(ui) = slot.as_mut() else {
                return false;
            };
            if ui.shown {
                // SAFETY: this runs on the main thread, which owns the window.
                unsafe {
                    let _: () = msg_send![&ui.window, setAlphaValue: 1.0f64];
                }
                return false;
            }
            // SAFETY: as above.
            unsafe {
                let _: () = msg_send![&ui.window, setAlphaValue: 0.0f64];
                let _: () = msg_send![&ui.window, orderFrontRegardless];
            }
            ui.shown = true;
            true
        });
        let _ = sender.send(fresh);
    });
    place(point);
    if receiver.recv().unwrap_or(false) {
        let frame = Duration::from_secs_f64(1.0 / FRAMES_PER_SECOND);
        for step in 1..=FADE_IN_FRAMES {
            set_alpha(f64::from(step) / f64::from(FADE_IN_FRAMES));
            std::thread::sleep(frame);
        }
    }
}

/// How far the float lifts the cursor off its resting point `secs` into its
/// clock: a slow sway and bob that never snaps back, as its phase runs on
/// across moves.
fn float_offset(secs: f64) -> (f64, f64) {
    let angle = secs.rem_euclid(FLOAT_PERIOD_SECS) / FLOAT_PERIOD_SECS * std::f64::consts::TAU;
    (
        FLOAT_SWAY * angle.sin(),
        FLOAT_BOB * angle.cos() - FLOAT_LIFT,
    )
}

/// How strongly the cursor floats `rested` seconds after it came to rest:
/// still for a moment, easing in, then easing out to lie still on its point.
fn float_amplitude(rested: f64) -> f64 {
    let rise = ease_in_out((rested - FLOAT_HOLD_SECS) / FLOAT_EASE_IN_SECS);
    let fall = ease_in_out((rested - FLOAT_FOR_SECS) / FLOAT_EASE_OUT_SECS);
    rise * (1.0 - fall)
}

fn float() {
    let clock = *CLOCK.get_or_init(Instant::now);
    let frame = Duration::from_secs_f64(1.0 / FLOAT_FRAMES_PER_SECOND);
    let mut still_since: Option<Instant> = None;
    let mut pointer = pointer_location();
    loop {
        let now = pointer_location();
        if user_is_active(pointer, now) {
            step_aside();
        }
        pointer = now;
        let rested = *lock(&RESTED);
        let base = *lock(&POSITION);
        let (Some(rested), Some(base)) = (rested, base) else {
            std::thread::sleep(RESTING_POLL);
            continue;
        };
        if MOVING.load(Ordering::SeqCst) {
            std::thread::sleep(frame);
            continue;
        }
        let since = rested.elapsed().as_secs_f64();
        if since > FLOAT_FOR_SECS + FLOAT_EASE_OUT_SECS {
            if still_since != Some(rested) {
                place(base);
                still_since = Some(rested);
            }
            std::thread::sleep(RESTING_POLL);
            continue;
        }
        let amplitude = float_amplitude(since);
        let (dx, dy) = float_offset(clock.elapsed().as_secs_f64());
        place(CGPoint {
            x: base.x + dx * amplitude,
            y: base.y + dy * amplitude,
        });
        std::thread::sleep(frame);
    }
}

/// Whether the user moved their pointer or typed since the last look, apart
/// from what the agent's own input caused.
fn user_is_active(before: Option<CGPoint>, now: Option<CGPoint>) -> bool {
    if MOVING.load(Ordering::SeqCst) || lock(&ACTED).is_some_and(|at| at.elapsed() < USER_GRACE) {
        return false;
    }
    let moved = before
        .zip(now)
        .is_some_and(|(a, b)| (a.x - b.x).hypot(a.y - b.y) > POINTER_JITTER);
    // SAFETY: no preconditions.
    let typed = unsafe { ffi::CGEventSourceSecondsSinceLastEventType(HID_SYSTEM_STATE, KEY_DOWN) }
        < RESTING_POLL.as_secs_f64();
    moved || typed
}

/// Fade the cursor out and take it off screen; it keeps its resting point and
/// comes back with the next glide.
fn step_aside() {
    let (sender, receiver) = std::sync::mpsc::channel();
    DispatchQueue::main().exec_async(move || {
        let _ = sender.send(UI.with(|slot| slot.borrow().as_ref().is_some_and(|ui| ui.shown)));
    });
    if !receiver.recv().unwrap_or(false) {
        return;
    }
    let frame = Duration::from_secs_f64(1.0 / FRAMES_PER_SECOND);
    for step in (0..FADE_OUT_FRAMES).rev() {
        if MOVING.load(Ordering::SeqCst) {
            return;
        }
        set_alpha(f64::from(step) / f64::from(FADE_OUT_FRAMES));
        std::thread::sleep(frame);
    }
    DispatchQueue::main().exec_async(|| {
        UI.with(|slot| {
            let mut slot = slot.borrow_mut();
            let Some(ui) = slot.as_mut() else { return };
            if MOVING.load(Ordering::SeqCst) {
                return;
            }
            // SAFETY: this runs on the main thread, which owns the window.
            unsafe {
                let _: () = msg_send![&ui.window, orderOut: Option::<&AnyObject>::None];
            }
            ui.shown = false;
        });
    });
}

fn position_file() -> Option<PathBuf> {
    crate::config::get_config_dir().map(|dir| dir.join("run").join("cursor-position"))
}

fn save_position(point: CGPoint) {
    if let Some(path) = position_file() {
        let _ = std::fs::write(path, format!("{} {}", point.x, point.y));
    }
}

/// Where the cursor last rested, if that point is still on a display.
fn saved_position() -> Option<CGPoint> {
    let text = std::fs::read_to_string(position_file()?).ok()?;
    let (x, y) = text.trim().split_once(' ')?;
    let point = CGPoint {
        x: x.parse().ok()?,
        y: y.parse().ok()?,
    };
    let mut display = 0u32;
    let mut count = 0u32;
    // SAFETY: both outputs outlive the call, which writes at most one display.
    let found = unsafe { ffi::CGGetDisplaysWithPoint(point, 1, &raw mut display, &raw mut count) };
    (found == 0 && count > 0).then_some(point)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_float_eases_in_after_a_pause_and_settles_back_on_the_point() {
        assert_eq!(float_amplitude(0.0), 0.0);
        assert_eq!(float_amplitude(FLOAT_HOLD_SECS), 0.0);
        assert_eq!(float_amplitude(FLOAT_HOLD_SECS + FLOAT_EASE_IN_SECS), 1.0);
        assert_eq!(float_amplitude(FLOAT_FOR_SECS + FLOAT_EASE_OUT_SECS), 0.0);
    }

    #[test]
    fn the_float_stays_within_a_few_points() {
        for tenth in 0..40 {
            let (dx, dy) = float_offset(f64::from(tenth) / 10.0);
            assert!(dx.abs() <= FLOAT_SWAY && dy.abs() <= FLOAT_BOB + FLOAT_LIFT);
        }
    }
}
