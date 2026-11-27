//! Input that reaches an app behind the user's windows, as Chromium and
//! Electron apps accept it: events posted to the app's process through
//! WindowServer's own path (SkyLight), keyboard events with the
//! authentication message macOS 14+ asks of them, and a focus record that
//! makes the target window key without raising it.
//!
//! These are private interfaces, resolved at run time; when one is missing
//! the caller falls back to the public per-process post. The recipe is Cua
//! Driver's (<https://github.com/trycua/cua>, MIT License, Copyright (c)
//! 2025-2026 Cua AI, Inc.).

use super::ffi::{self, CGEventRef, CGPoint};
use std::ffi::{CStr, c_char, c_int, c_uint, c_void};
use std::sync::OnceLock;

type PostToPid = unsafe extern "C" fn(i32, CGEventRef);
type SetAuthentication = unsafe extern "C" fn(CGEventRef, *mut c_void);
type SetWindowLocation = unsafe extern "C" fn(CGEventRef, f64, f64);
type SetIntegerField = unsafe extern "C" fn(CGEventRef, u32, i64);
type AuthenticationFactory =
    unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void, c_int, c_uint) -> *mut c_void;
type GetClass = unsafe extern "C" fn(*const c_char) -> *mut c_void;
type RegisterSelector = unsafe extern "C" fn(*const c_char) -> *mut c_void;
type RespondsTo = unsafe extern "C" fn(*mut c_void, *mut c_void) -> bool;
type PostEventRecord = unsafe extern "C" fn(*const c_void, *const u8) -> i32;
type MainConnection = unsafe extern "C" fn() -> u32;
type WindowOwner = unsafe extern "C" fn(u32, u32, *mut u32) -> i32;
type ConnectionProcess = unsafe extern "C" fn(u32, *mut c_void) -> i32;
type ProcessForPid = unsafe extern "C" fn(i32, *mut c_void) -> i32;

const SKYLIGHT: &CStr = c"/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight";
const FOCUS_RECORD_SIZE: usize = 0xF8;
const FOCUS: u8 = 0x01;
const RECORD_OFFSETS: [usize; 3] = [24, 32, 16];

fn symbol<T: Copy>(name: &CStr) -> Option<T> {
    static LOADED: OnceLock<()> = OnceLock::new();
    LOADED.get_or_init(|| {
        // SAFETY: a valid C path; the handle stays loaded for the process.
        unsafe { libc::dlopen(SKYLIGHT.as_ptr(), libc::RTLD_LAZY | libc::RTLD_GLOBAL) };
    });
    // SAFETY: a valid C name; a null answer is handled.
    let found = unsafe { libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr()) };
    // SAFETY: callers name a function whose C signature is `T`.
    (!found.is_null()).then(|| unsafe { std::mem::transmute_copy::<*mut c_void, T>(&found) })
}

macro_rules! resolved {
    ($name:ident: $kind:ty = $symbol:literal) => {
        fn $name() -> Option<$kind> {
            static CELL: OnceLock<Option<$kind>> = OnceLock::new();
            *CELL.get_or_init(|| symbol::<$kind>($symbol))
        }
    };
}

resolved!(post_to_pid_fn: PostToPid = c"SLEventPostToPid");
resolved!(set_authentication_fn: SetAuthentication = c"SLEventSetAuthenticationMessage");
resolved!(set_window_location_fn: SetWindowLocation = c"CGEventSetWindowLocation");
resolved!(set_integer_field_fn: SetIntegerField = c"SLEventSetIntegerValueField");
resolved!(message_send_fn: AuthenticationFactory = c"objc_msgSend");
resolved!(get_class_fn: GetClass = c"objc_getClass");
resolved!(register_selector_fn: RegisterSelector = c"sel_registerName");
resolved!(responds_to_fn: RespondsTo = c"class_respondsToSelector");
resolved!(post_event_record_fn: PostEventRecord = c"SLPSPostEventRecordTo");
resolved!(main_connection_fn: MainConnection = c"CGSMainConnectionID");
resolved!(window_owner_fn: WindowOwner = c"SLSGetWindowOwner");
resolved!(connection_process_fn: ConnectionProcess = c"SLSGetConnectionPSN");
resolved!(process_for_pid_fn: ProcessForPid = c"GetProcessForPID");

/// Whether events can go through WindowServer's own per-process path.
pub fn available() -> bool {
    post_to_pid_fn().is_some()
}

/// Post `event` to `pid`. Keyboard events carry the authentication message
/// Chromium asks of them (`authenticate`). Falls back to the public post.
pub fn post(pid: i32, event: CGEventRef, authenticate: bool) {
    let Some(post) = post_to_pid_fn() else {
        // SAFETY: `event` is a live CGEvent.
        unsafe { ffi::CGEventPostToPid(pid, event) };
        return;
    };
    if authenticate {
        attach_authentication(pid, event);
    }
    // SAFETY: `event` is a live CGEvent; the symbol has this signature.
    unsafe { post(pid, event) };
}

fn attach_authentication(pid: i32, event: CGEventRef) {
    let (Some(get_class), Some(register), Some(responds), Some(send), Some(set)) = (
        get_class_fn(),
        register_selector_fn(),
        responds_to_fn(),
        message_send_fn(),
        set_authentication_fn(),
    ) else {
        return;
    };
    // SAFETY: valid C names; null answers are checked before use.
    let (class, selector) = unsafe {
        (
            get_class(c"SLSEventAuthenticationMessage".as_ptr()),
            register(c"messageWithEventRecord:pid:version:".as_ptr()),
        )
    };
    // SAFETY: both are non-null runtime objects when checked.
    if class.is_null() || selector.is_null() || !unsafe { responds(class, selector) } {
        return;
    }
    let record = event_record(event);
    if record.is_null() {
        return;
    }
    // SAFETY: the class answers this selector with this signature.
    let message = unsafe { send(class, selector, record, pid, 0) };
    if !message.is_null() {
        // SAFETY: `event` is live and `message` is the message just made.
        unsafe { set(event, message) };
    }
}

/// The SkyLight record a CGEvent wraps: the first non-null pointer at the
/// offsets it has had across releases.
fn event_record(event: CGEventRef) -> *mut c_void {
    for offset in RECORD_OFFSETS {
        // SAFETY: a CGEvent is larger than the largest offset read.
        let slot = unsafe { event.cast::<u8>().add(offset).cast::<*mut c_void>() };
        // SAFETY: as above; the read is unaligned-safe.
        let pointer = unsafe { std::ptr::read_unaligned(slot) };
        if !pointer.is_null() {
            return pointer;
        }
    }
    std::ptr::null_mut()
}

/// Set a raw event field by SkyLight's numbering.
pub fn set_field(event: CGEventRef, field: u32, value: i64) {
    match set_integer_field_fn() {
        // SAFETY: `event` is a live CGEvent.
        Some(set) => unsafe { set(event, field, value) },
        // SAFETY: as above.
        None => unsafe { ffi::CGEventSetIntegerValueField(event, field, value) },
    }
}

/// Stamp where in the window an event lands.
pub fn set_window_location(event: CGEventRef, point: CGPoint) {
    if let Some(set) = set_window_location_fn() {
        // SAFETY: `event` is a live CGEvent.
        unsafe { set(event, point.x, point.y) };
    }
}

/// Make `window_id` of `pid` the key window without raising it or taking
/// key status from the user's window. False when the private calls are
/// missing or refused.
pub fn focus_without_raise(pid: i32, window_id: u32) -> bool {
    let Some(post) = post_event_record_fn() else {
        return false;
    };
    let Some(process) = process_of(pid, window_id) else {
        return false;
    };
    let mut record = [0u8; FOCUS_RECORD_SIZE];
    record[0x04] = 0xF8;
    record[0x08] = 0x0D;
    record[0x3C..0x40].copy_from_slice(&window_id.to_le_bytes());
    record[0x8A] = FOCUS;
    // SAFETY: both buffers outlive the call.
    unsafe { post(process.as_ptr().cast(), record.as_ptr()) == 0 }
}

/// The process serial number that owns `window_id`, else `pid`'s.
fn process_of(pid: i32, window_id: u32) -> Option<[u8; 8]> {
    let mut process = [0u8; 8];
    if let (Some(main), Some(owner), Some(serial)) = (
        main_connection_fn(),
        window_owner_fn(),
        connection_process_fn(),
    ) {
        let mut connection = 0u32;
        // SAFETY: the outputs outlive the calls.
        let found = unsafe {
            owner(main(), window_id, &raw mut connection) == 0
                && connection != 0
                && serial(connection, process.as_mut_ptr().cast()) == 0
        };
        if found {
            return Some(process);
        }
    }
    let serial = process_for_pid_fn()?;
    // SAFETY: the output outlives the call.
    (unsafe { serial(pid, process.as_mut_ptr().cast()) } == 0).then_some(process)
}
