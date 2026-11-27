//! What the user sees while an agent drives this Mac: the agent's own cursor,
//! which glides to each element before it acts and names the app it is
//! controlling. Desktop control stops from Settings, or when the app quits.

use super::ffi::{self, CGPoint, CGRect, CGSize};
use crate::computer_worker::cursor_image;
use crate::computer_worker::cursor_motion::{FRAMES_PER_SECOND, Glide, Point};
use dispatch2::DispatchQueue;
use objc2::msg_send;
use objc2::rc::{Allocated, Retained, autoreleasepool};
use objc2::runtime::{AnyClass, AnyObject};
use std::cell::RefCell;
use std::ffi::{CString, c_void};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const CURSOR_POINTS: f64 = 24.0;
const LABEL_WIDTH: f64 = 150.0;
const LABEL_HEIGHT: f64 = 18.0;
const HIDE_AFTER: Duration = Duration::from_secs(4);
const FADE_IN_FRAMES: u32 = 8;
const FADE_OUT_FRAMES: u32 = 18;
const SETTLE: Duration = Duration::from_millis(90);
const PRESS: Duration = Duration::from_millis(110);
const PRESSED_SCALE: f64 = 0.8;
const ACCESSORY_POLICY: isize = 1;
const BORDERLESS: usize = 0;
const BUFFERED: usize = 2;
const SCREEN_SAVER_LEVEL: isize = 1000;
const ALL_SPACES_STATIONARY_IGNORES_CYCLE: usize = 1 | 16 | 64 | 256;
const SHARING_NONE: usize = 0;
const DEFAULT_LABEL: &str = "RustCode";
const SCALE_PROPORTIONALLY_UP_OR_DOWN: usize = 3;

struct Ui {
    window: Retained<AnyObject>,
    label: Retained<AnyObject>,
    image: Retained<AnyObject>,
    image_frame: CGRect,
    size: CGSize,
    tip: CGPoint,
    shown: bool,
}

thread_local! {
    static UI: RefCell<Option<Ui>> = const { RefCell::new(None) };
}

static LAST_MOVE: Mutex<Option<Instant>> = Mutex::new(None);
static POSITION: Mutex<Option<CGPoint>> = Mutex::new(None);
static BEND_LEFT: AtomicBool = AtomicBool::new(true);

fn class(name: &str) -> &'static AnyClass {
    let name = CString::new(name).expect("class names have no NUL");
    AnyClass::get(&name).unwrap_or_else(|| panic!("AppKit class {name:?} is missing"))
}

fn ns_string(text: &str) -> Retained<AnyObject> {
    let text = CString::new(text.replace('\0', "")).expect("NULs were removed");
    // SAFETY: the C string is valid UTF-8 for the call.
    unsafe { msg_send![class("NSString"), stringWithUTF8String: text.as_ptr()] }
}

/// Run AppKit on the main thread until the user stops the worker. Never returns.
pub fn run_main_thread() -> ! {
    autoreleasepool(|_| {
        // SAFETY: called once, on the main thread, before the run loop starts.
        unsafe {
            let app: Retained<AnyObject> = msg_send![class("NSApplication"), sharedApplication];
            let _: bool = msg_send![&app, setActivationPolicy: ACCESSORY_POLICY];
            let ui = build();
            UI.with(|slot| *slot.borrow_mut() = Some(ui));
            std::thread::spawn(hide_when_idle);
            let _: () = msg_send![&app, run];
        }
    });
    std::process::exit(0)
}

unsafe fn build() -> Ui {
    let scale = 2.0;
    let cursor = cursor_image::draw(CURSOR_POINTS, scale);
    let cursor_size = CGSize {
        width: f64::from(cursor.width) / scale,
        height: f64::from(cursor.height) / scale,
    };
    let tip = CGPoint {
        x: cursor.tip.0 / scale,
        y: cursor.tip.1 / scale,
    };
    let size = CGSize {
        width: cursor_size.width + 4.0 + LABEL_WIDTH,
        height: cursor_size.height + LABEL_HEIGHT,
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
        let _: () = msg_send![&image, setSize: cursor_size];
        let view: Retained<AnyObject> =
            msg_send![class("NSImageView"), imageViewWithImage: &*image];
        let _: () = msg_send![&view, setImageScaling: SCALE_PROPORTIONALLY_UP_OR_DOWN];
        let image_frame = CGRect {
            origin: CGPoint {
                x: 0.0,
                y: LABEL_HEIGHT,
            },
            size: cursor_size,
        };
        let _: () = msg_send![&view, setFrame: image_frame];
        let _: () = msg_send![&content, addSubview: &*view];

        let label: Retained<AnyObject> =
            msg_send![class("NSTextField"), labelWithString: &*ns_string(DEFAULT_LABEL)];
        let white: Retained<AnyObject> = msg_send![class("NSColor"), whiteColor];
        let fill: Retained<AnyObject> = msg_send![
            class("NSColor"),
            colorWithSRGBRed: 232.0f64 / 255.0,
            green: 116.0f64 / 255.0,
            blue: 59.0f64 / 255.0,
            alpha: 1.0f64
        ];
        let font: Retained<AnyObject> = msg_send![class("NSFont"), boldSystemFontOfSize: 11.0f64];
        let _: () = msg_send![&label, setFont: &*font];
        let _: () = msg_send![&label, setTextColor: &*white];
        let _: () = msg_send![&label, setBackgroundColor: &*fill];
        let _: () = msg_send![&label, setDrawsBackground: true];
        let _: () = msg_send![
            &label,
            setFrame: CGRect::new(cursor_size.width * 0.6, 0.0, LABEL_WIDTH, LABEL_HEIGHT)
        ];
        let _: () = msg_send![&content, addSubview: &*label];

        Ui {
            window,
            label,
            image: view,
            image_frame,
            size,
            tip: CGPoint { x: tip.x, y: tip.y },
            shown: false,
        }
    }
}

fn primary_height() -> f64 {
    // SAFETY: no preconditions.
    unsafe { ffi::CGDisplayBounds(ffi::CGMainDisplayID()).size.height }
}

/// Glide the agent's cursor to a global point (top-left origin, points)
/// along an eased arc, from where it last was (or from the user's pointer
/// the first time), and give the user a moment to see where the agent is
/// about to act.
pub fn glide(point: CGPoint) {
    touch();
    let from = POSITION
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .or_else(pointer_location)
        .unwrap_or(point);
    let bend_left = BEND_LEFT.fetch_xor(true, Ordering::Relaxed);
    let plan = Glide::plan(motion_point(from), motion_point(point), bend_left);
    appear(from);
    let frame = Duration::from_secs_f64(1.0 / FRAMES_PER_SECOND);
    for step in plan.frames() {
        place(CGPoint {
            x: step.x,
            y: step.y,
        });
        touch();
        std::thread::sleep(frame);
    }
    *POSITION.lock().unwrap_or_else(|e| e.into_inner()) = Some(point);
    std::thread::sleep(SETTLE);
}

/// Show a press at the cursor's tip just before a click lands.
pub fn press() {
    touch();
    set_pressed(true);
    std::thread::sleep(PRESS);
    set_pressed(false);
}

fn touch() {
    *LAST_MOVE.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
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

/// Name the app the agent is controlling in the cursor's label.
pub fn set_activity(label: Option<String>) {
    DispatchQueue::main().exec_async(move || {
        UI.with(|slot| {
            let slot = slot.borrow();
            let Some(ui) = slot.as_ref() else { return };
            let short = label.as_deref().unwrap_or(DEFAULT_LABEL);
            let short: String = short.chars().take(28).collect();
            // SAFETY: this runs on the main thread, which owns the label.
            unsafe {
                let _: () =
                    msg_send![&ui.label, setStringValue: &*ns_string(&format!(" {short} "))];
            }
        });
    });
}

fn hide_when_idle() {
    let frame = Duration::from_secs_f64(1.0 / FRAMES_PER_SECOND);
    loop {
        std::thread::sleep(Duration::from_millis(250));
        let idle = LAST_MOVE
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some_and(|at| at.elapsed() > HIDE_AFTER);
        if !idle {
            continue;
        }
        let mut interrupted = false;
        for step in (0..FADE_OUT_FRAMES).rev() {
            if !still_idle() {
                interrupted = true;
                break;
            }
            set_alpha(f64::from(step) / f64::from(FADE_OUT_FRAMES));
            std::thread::sleep(frame);
        }
        if interrupted || !still_idle() {
            set_alpha(1.0);
            continue;
        }
        *LAST_MOVE.lock().unwrap_or_else(|e| e.into_inner()) = None;
        DispatchQueue::main().exec_async(|| {
            UI.with(|slot| {
                let mut slot = slot.borrow_mut();
                let Some(ui) = slot.as_mut() else { return };
                // SAFETY: this runs on the main thread, which owns the window.
                unsafe {
                    let _: () = msg_send![&ui.window, orderOut: Option::<&AnyObject>::None];
                }
                ui.shown = false;
            });
        });
    }
}

fn still_idle() -> bool {
    LAST_MOVE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_some_and(|at| at.elapsed() > HIDE_AFTER)
}
