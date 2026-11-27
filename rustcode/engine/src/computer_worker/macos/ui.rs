//! What the user sees while an agent drives this Mac: the agent's own cursor,
//! which glides to each element before it acts, and a menu bar item that
//! names the thread and stops desktop control.

use super::ffi::{self, CGPoint, CGRect, CGSize};
use crate::computer_worker::cursor_image;
use dispatch2::DispatchQueue;
use objc2::rc::{Allocated, Retained, autoreleasepool};
use objc2::runtime::{AnyClass, AnyObject, Sel};
use objc2::{msg_send, sel};
use std::cell::RefCell;
use std::ffi::{CString, c_void};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const CURSOR_POINTS: f64 = 24.0;
const LABEL_WIDTH: f64 = 150.0;
const LABEL_HEIGHT: f64 = 18.0;
const HIDE_AFTER: Duration = Duration::from_secs(5);
const ACCESSORY_POLICY: isize = 1;
const BORDERLESS: usize = 0;
const BUFFERED: usize = 2;
const SCREEN_SAVER_LEVEL: isize = 1000;
const ALL_SPACES_STATIONARY_IGNORES_CYCLE: usize = 1 | 16 | 64 | 256;
const SHARING_NONE: usize = 0;
const VARIABLE_LENGTH: f64 = -1.0;
const DEFAULT_LABEL: &str = "RustCode";

struct Ui {
    window: Retained<AnyObject>,
    label: Retained<AnyObject>,
    activity: Retained<AnyObject>,
    size: CGSize,
    tip: CGPoint,
    shown: bool,
}

thread_local! {
    static UI: RefCell<Option<Ui>> = const { RefCell::new(None) };
}

static LAST_MOVE: Mutex<Option<Instant>> = Mutex::new(None);

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
            let ui = build(&app);
            UI.with(|slot| *slot.borrow_mut() = Some(ui));
            std::thread::spawn(hide_when_idle);
            let _: () = msg_send![&app, run];
        }
    });
    std::process::exit(0)
}

unsafe fn build(app: &AnyObject) -> Ui {
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
        let _: () = msg_send![
            &view,
            setFrame: CGRect {
                origin: CGPoint { x: 0.0, y: LABEL_HEIGHT },
                size: cursor_size,
            }
        ];
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

        let activity = status_item(app);
        Ui {
            window,
            label,
            activity,
            size,
            tip: CGPoint { x: tip.x, y: tip.y },
            shown: false,
        }
    }
}

unsafe fn status_item(app: &AnyObject) -> Retained<AnyObject> {
    // SAFETY: the caller is on the main thread; every selector is AppKit's.
    unsafe {
        let bar: Retained<AnyObject> = msg_send![class("NSStatusBar"), systemStatusBar];
        let item: Retained<AnyObject> = msg_send![&bar, statusItemWithLength: VARIABLE_LENGTH];
        let button: Option<Retained<AnyObject>> = msg_send![&item, button];
        if let Some(button) = button {
            let _: () = msg_send![&button, setTitle: &*ns_string("◉ RustCode")];
        }
        let menu: Retained<AnyObject> = msg_send![class("NSMenu"), new];
        let _: () = msg_send![&menu, setAutoenablesItems: false];
        let activity = menu_item("RustCode can control this Mac", None);
        let _: () = msg_send![&activity, setEnabled: false];
        let _: () = msg_send![&menu, addItem: &*activity];
        let separator: Retained<AnyObject> = msg_send![class("NSMenuItem"), separatorItem];
        let _: () = msg_send![&menu, addItem: &*separator];
        let stop = menu_item("Stop desktop control", Some(sel!(terminate:)));
        let _: () = msg_send![&stop, setTarget: app];
        let _: () = msg_send![&menu, addItem: &*stop];
        let _: () = msg_send![&item, setMenu: &*menu];
        std::mem::forget(item);
        activity
    }
}

unsafe fn menu_item(title: &str, action: Option<Sel>) -> Retained<AnyObject> {
    // SAFETY: the caller is on the main thread.
    unsafe {
        let allocated: Allocated<AnyObject> = msg_send![class("NSMenuItem"), alloc];
        msg_send![
            allocated,
            initWithTitle: &*ns_string(title),
            action: action,
            keyEquivalent: &*ns_string("")
        ]
    }
}

fn primary_height() -> f64 {
    // SAFETY: no preconditions.
    unsafe { ffi::CGDisplayBounds(ffi::CGMainDisplayID()).size.height }
}

/// Glide the agent's cursor to a global point (top-left origin, points) and
/// give the user a moment to see where the agent is about to act.
pub fn glide(point: CGPoint) {
    *LAST_MOVE.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
    let height = primary_height();
    DispatchQueue::main().exec_async(move || {
        UI.with(|slot| {
            let mut slot = slot.borrow_mut();
            let Some(ui) = slot.as_mut() else { return };
            let flipped = height - point.y;
            let frame = CGRect {
                origin: CGPoint {
                    x: point.x - ui.tip.x,
                    y: flipped - ui.size.height + ui.tip.y,
                },
                size: ui.size,
            };
            // SAFETY: this runs on the main thread, which owns the window.
            unsafe {
                if ui.shown {
                    let _: () =
                        msg_send![&ui.window, setFrame: frame, display: true, animate: true];
                } else {
                    let _: () = msg_send![&ui.window, setFrame: frame, display: true];
                    let _: () = msg_send![&ui.window, orderFrontRegardless];
                    ui.shown = true;
                }
            }
        });
    });
    std::thread::sleep(Duration::from_millis(260));
}

/// Name what the agent is doing, in the cursor's label and the menu bar item.
pub fn set_activity(label: Option<String>) {
    DispatchQueue::main().exec_async(move || {
        UI.with(|slot| {
            let slot = slot.borrow();
            let Some(ui) = slot.as_ref() else { return };
            let short = label.as_deref().unwrap_or(DEFAULT_LABEL);
            let short: String = short.chars().take(28).collect();
            let menu = match &label {
                Some(label) => format!("Controlling: {label}"),
                None => "RustCode can control this Mac".to_owned(),
            };
            // SAFETY: this runs on the main thread, which owns both views.
            unsafe {
                let _: () =
                    msg_send![&ui.label, setStringValue: &*ns_string(&format!(" {short} "))];
                let _: () = msg_send![&ui.activity, setTitle: &*ns_string(&menu)];
            }
        });
    });
}

fn hide_when_idle() {
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let idle = LAST_MOVE
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some_and(|at| at.elapsed() > HIDE_AFTER);
        if !idle {
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
