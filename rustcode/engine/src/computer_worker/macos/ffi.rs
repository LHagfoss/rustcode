//! The C halves of CoreFoundation, CoreGraphics and the Accessibility API the
//! worker calls, declared by hand, and an owned CoreFoundation reference.
#![allow(non_upper_case_globals, non_snake_case, dead_code)]

use objc2::encode::{Encode, Encoding};
use std::ffi::{CStr, c_char, c_void};

pub type CFTypeRef = *const c_void;
pub type CFStringRef = *const c_void;
pub type CFArrayRef = *const c_void;
pub type CFDictionaryRef = *const c_void;
pub type CFIndex = isize;
pub type CFTypeID = usize;
pub type AXUIElementRef = *const c_void;
pub type AXError = i32;
pub type CGEventRef = *mut c_void;
pub type CGImageRef = *const c_void;
pub type CGContextRef = *mut c_void;
pub type CGColorSpaceRef = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CGPoint {
    pub x: f64,
    pub y: f64,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CGSize {
    pub width: f64,
    pub height: f64,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CGRect {
    pub origin: CGPoint,
    pub size: CGSize,
}
impl CGRect {
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            origin: CGPoint { x, y },
            size: CGSize { width, height },
        }
    }
    pub fn center(&self) -> CGPoint {
        CGPoint {
            x: self.origin.x + self.size.width / 2.0,
            y: self.origin.y + self.size.height / 2.0,
        }
    }
}
// SAFETY: the layouts match CoreGraphics' CGPoint, CGSize and CGRect on 64-bit.
unsafe impl Encode for CGPoint {
    const ENCODING: Encoding = Encoding::Struct("CGPoint", &[f64::ENCODING, f64::ENCODING]);
}
// SAFETY: as above.
unsafe impl Encode for CGSize {
    const ENCODING: Encoding = Encoding::Struct("CGSize", &[f64::ENCODING, f64::ENCODING]);
}
// SAFETY: as above.
unsafe impl Encode for CGRect {
    const ENCODING: Encoding = Encoding::Struct("CGRect", &[CGPoint::ENCODING, CGSize::ENCODING]);
}

#[repr(C)]
pub struct Opaque {
    _private: [u8; 0],
}

pub const kCFStringEncodingUTF8: u32 = 0x0800_0100;
pub const kCFNumberSInt32Type: i32 = 3;
pub const kCFNumberSInt64Type: i32 = 4;
pub const kCFNumberFloat64Type: i32 = 6;
pub const kAXValueCGPointType: u32 = 1;
pub const kAXValueCGSizeType: u32 = 2;
pub const kCGWindowListOptionOnScreenOnly: u32 = 1;
pub const kCGWindowListExcludeDesktopElements: u32 = 16;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    pub static kCFTypeDictionaryKeyCallBacks: Opaque;
    pub static kCFTypeDictionaryValueCallBacks: Opaque;
    pub static kCFBooleanTrue: CFTypeRef;
    pub static kCFBooleanFalse: CFTypeRef;
    pub fn CFRetain(value: CFTypeRef) -> CFTypeRef;
    pub fn CFRelease(value: CFTypeRef);
    pub fn CFGetTypeID(value: CFTypeRef) -> CFTypeID;
    pub fn CFStringGetTypeID() -> CFTypeID;
    pub fn CFNumberGetTypeID() -> CFTypeID;
    pub fn CFBooleanGetTypeID() -> CFTypeID;
    pub fn CFArrayGetTypeID() -> CFTypeID;
    pub fn CFStringCreateWithBytes(
        allocator: CFTypeRef,
        bytes: *const u8,
        length: CFIndex,
        encoding: u32,
        external: u8,
    ) -> CFStringRef;
    pub fn CFStringGetLength(string: CFStringRef) -> CFIndex;
    pub fn CFStringGetMaximumSizeForEncoding(length: CFIndex, encoding: u32) -> CFIndex;
    pub fn CFStringGetCString(
        string: CFStringRef,
        buffer: *mut c_char,
        size: CFIndex,
        encoding: u32,
    ) -> u8;
    pub fn CFArrayGetCount(array: CFArrayRef) -> CFIndex;
    pub fn CFArrayGetValueAtIndex(array: CFArrayRef, index: CFIndex) -> *const c_void;
    pub fn CFDictionaryGetValue(dictionary: CFDictionaryRef, key: *const c_void) -> *const c_void;
    pub fn CFDictionaryCreate(
        allocator: CFTypeRef,
        keys: *const *const c_void,
        values: *const *const c_void,
        count: CFIndex,
        key_callbacks: *const Opaque,
        value_callbacks: *const Opaque,
    ) -> CFDictionaryRef;
    pub fn CFNumberGetValue(number: CFTypeRef, kind: i32, out: *mut c_void) -> u8;
    pub fn CFNumberCreate(allocator: CFTypeRef, kind: i32, value: *const c_void) -> CFTypeRef;
    pub fn CFBooleanGetValue(boolean: CFTypeRef) -> u8;
}

#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    pub static kSecGuestAttributePid: CFStringRef;
    pub fn SecCodeCopySelf(flags: u32, code: *mut CFTypeRef) -> i32;
    pub fn SecCodeCopyGuestWithAttributes(
        host: CFTypeRef,
        attributes: CFDictionaryRef,
        flags: u32,
        guest: *mut CFTypeRef,
    ) -> i32;
    pub fn SecCodeCopyDesignatedRequirement(
        code: CFTypeRef,
        flags: u32,
        requirement: *mut CFTypeRef,
    ) -> i32;
    pub fn SecCodeCheckValidity(code: CFTypeRef, flags: u32, requirement: CFTypeRef) -> i32;
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    pub static kAXTrustedCheckOptionPrompt: CFStringRef;
    pub fn AXIsProcessTrusted() -> u8;
    pub fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> u8;
    pub fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
    pub fn AXUIElementCopyAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> AXError;
    pub fn AXUIElementSetAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: CFTypeRef,
    ) -> AXError;
    pub fn AXUIElementIsAttributeSettable(
        element: AXUIElementRef,
        attribute: CFStringRef,
        settable: *mut u8,
    ) -> AXError;
    pub fn AXUIElementPerformAction(element: AXUIElementRef, action: CFStringRef) -> AXError;
    pub fn AXUIElementCopyActionNames(element: AXUIElementRef, names: *mut CFArrayRef) -> AXError;
    pub fn AXUIElementSetMessagingTimeout(element: AXUIElementRef, seconds: f32) -> AXError;
    pub fn AXValueGetTypeID() -> CFTypeID;
    pub fn AXValueGetValue(value: CFTypeRef, kind: u32, out: *mut c_void) -> u8;
    pub fn _AXUIElementGetWindow(element: AXUIElementRef, window: *mut u32) -> AXError;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    pub static kCGWindowNumber: CFStringRef;
    pub static kCGWindowOwnerPID: CFStringRef;
    pub static kCGWindowOwnerName: CFStringRef;
    pub static kCGWindowName: CFStringRef;
    pub static kCGWindowLayer: CFStringRef;
    pub static kCGWindowBounds: CFStringRef;
    pub fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> CFArrayRef;
    pub fn CGRectMakeWithDictionaryRepresentation(
        dictionary: CFDictionaryRef,
        rect: *mut CGRect,
    ) -> bool;
    pub fn CGPreflightScreenCaptureAccess() -> bool;
    pub fn CGRequestScreenCaptureAccess() -> bool;
    pub fn CGMainDisplayID() -> u32;
    pub fn CGDisplayBounds(display: u32) -> CGRect;
    pub fn CGGetDisplaysWithPoint(
        point: CGPoint,
        max_displays: u32,
        displays: *mut u32,
        count: *mut u32,
    ) -> i32;
    pub fn CGEventCreateMouseEvent(
        source: *const c_void,
        kind: u32,
        location: CGPoint,
        button: u32,
    ) -> CGEventRef;
    pub fn CGEventCreateKeyboardEvent(source: *const c_void, key: u16, down: bool) -> CGEventRef;
    pub fn CGEventCreateScrollWheelEvent2(
        source: *const c_void,
        units: u32,
        count: u32,
        wheel1: i32,
        wheel2: i32,
        wheel3: i32,
    ) -> CGEventRef;
    pub fn CGEventKeyboardSetUnicodeString(event: CGEventRef, length: usize, text: *const u16);
    pub fn CGEventSetFlags(event: CGEventRef, flags: u64);
    pub fn CGEventSetIntegerValueField(event: CGEventRef, field: u32, value: i64);
    pub fn CGEventSetLocation(event: CGEventRef, location: CGPoint);
    pub fn CGEventPostToPid(pid: i32, event: CGEventRef);
    pub fn CGEventPost(tap: u32, event: CGEventRef);
    pub fn CGEventCreate(source: *const c_void) -> CGEventRef;
    pub fn CGEventSourceCreate(state: i32) -> *mut c_void;
    pub fn CGEventGetLocation(event: CGEventRef) -> CGPoint;
    pub fn CGEventSourceSecondsSinceLastEventType(state: i32, kind: u32) -> f64;
    pub fn CGImageGetWidth(image: CGImageRef) -> usize;
    pub fn CGImageGetHeight(image: CGImageRef) -> usize;
    pub fn CGImageRelease(image: CGImageRef);
    pub fn CGColorSpaceCreateDeviceRGB() -> CGColorSpaceRef;
    pub fn CGColorSpaceRelease(space: CGColorSpaceRef);
    pub fn CGBitmapContextCreate(
        data: *mut c_void,
        width: usize,
        height: usize,
        bits_per_component: usize,
        bytes_per_row: usize,
        space: CGColorSpaceRef,
        info: u32,
    ) -> CGContextRef;
    pub fn CGContextSetInterpolationQuality(context: CGContextRef, quality: i32);
    pub fn CGContextDrawImage(context: CGContextRef, rect: CGRect, image: CGImageRef);
    pub fn CGContextRelease(context: CGContextRef);
}

#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {}
#[link(name = "ScreenCaptureKit", kind = "framework")]
unsafe extern "C" {}

/// An owned CoreFoundation reference, released on drop.
pub struct Cf(CFTypeRef);

impl Cf {
    /// Take ownership of a reference from a Create or Copy function.
    pub fn created(value: CFTypeRef) -> Option<Self> {
        (!value.is_null()).then_some(Self(value))
    }
    /// Retain a reference borrowed from a Get function.
    pub fn retained(value: CFTypeRef) -> Option<Self> {
        if value.is_null() {
            return None;
        }
        // SAFETY: `value` is a live CoreFoundation object.
        Some(Self(unsafe { CFRetain(value) }))
    }
    pub fn get(&self) -> CFTypeRef {
        self.0
    }
}

impl Drop for Cf {
    fn drop(&mut self) {
        // SAFETY: this holds one retain of a live object.
        unsafe { CFRelease(self.0) }
    }
}

/// A new CoreFoundation string.
pub fn cf_string(text: &str) -> Cf {
    // SAFETY: the bytes are valid UTF-8 for the call's duration.
    let value = unsafe {
        CFStringCreateWithBytes(
            std::ptr::null(),
            text.as_ptr(),
            text.len() as CFIndex,
            kCFStringEncodingUTF8,
            0,
        )
    };
    Cf::created(value).expect("CFStringCreateWithBytes returned null for UTF-8")
}

fn is(value: CFTypeRef, type_id: CFTypeID) -> bool {
    // SAFETY: `value` is non-null and a CoreFoundation object.
    !value.is_null() && unsafe { CFGetTypeID(value) } == type_id
}

/// The text of a CFString, or None for anything else.
pub fn string(value: CFTypeRef) -> Option<String> {
    // SAFETY: the type is checked before the string is read.
    unsafe {
        if !is(value, CFStringGetTypeID()) {
            return None;
        }
        let length = CFStringGetLength(value);
        let size = CFStringGetMaximumSizeForEncoding(length, kCFStringEncodingUTF8) + 1;
        let mut buffer = vec![0 as c_char; size.max(1) as usize];
        if CFStringGetCString(value, buffer.as_mut_ptr(), size, kCFStringEncodingUTF8) == 0 {
            return None;
        }
        Some(
            CStr::from_ptr(buffer.as_ptr())
                .to_string_lossy()
                .into_owned(),
        )
    }
}

/// The value of a CFNumber as an integer.
pub fn integer(value: CFTypeRef) -> Option<i64> {
    let mut out = 0i64;
    // SAFETY: the type is checked and `out` fits the requested kind.
    unsafe {
        (is(value, CFNumberGetTypeID())
            && CFNumberGetValue(value, kCFNumberSInt64Type, (&raw mut out).cast()) != 0)
            .then_some(out)
    }
}

/// The value of a CFNumber as a float.
pub fn float(value: CFTypeRef) -> Option<f64> {
    let mut out = 0f64;
    // SAFETY: as above.
    unsafe {
        (is(value, CFNumberGetTypeID())
            && CFNumberGetValue(value, kCFNumberFloat64Type, (&raw mut out).cast()) != 0)
            .then_some(out)
    }
}

/// The value of a CFBoolean.
pub fn boolean(value: CFTypeRef) -> Option<bool> {
    // SAFETY: the type is checked first.
    unsafe { is(value, CFBooleanGetTypeID()).then(|| CFBooleanGetValue(value) != 0) }
}

/// Whether `value` is a CFArray.
pub fn is_array(value: CFTypeRef) -> bool {
    // SAFETY: only the type id is read.
    is(value, unsafe { CFArrayGetTypeID() })
}

/// The elements of a CFArray, borrowed from it.
pub fn array_items(array: CFArrayRef) -> Vec<CFTypeRef> {
    if !is_array(array) {
        return Vec::new();
    }
    // SAFETY: `array` is a CFArray; each index is below its count.
    unsafe {
        (0..CFArrayGetCount(array))
            .map(|index| CFArrayGetValueAtIndex(array, index))
            .collect()
    }
}

/// A value from a CFDictionary, borrowed from it.
pub fn dictionary_value(dictionary: CFDictionaryRef, key: CFStringRef) -> CFTypeRef {
    // SAFETY: both are live CoreFoundation objects.
    unsafe { CFDictionaryGetValue(dictionary, key) }
}

/// A one-entry CFDictionary.
pub fn dictionary(key: CFTypeRef, value: CFTypeRef) -> Option<Cf> {
    let keys = [key];
    let values = [value];
    // SAFETY: one key and one value, with the standard CF callbacks.
    let created = unsafe {
        CFDictionaryCreate(
            std::ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            1,
            &raw const kCFTypeDictionaryKeyCallBacks,
            &raw const kCFTypeDictionaryValueCallBacks,
        )
    };
    Cf::created(created)
}
