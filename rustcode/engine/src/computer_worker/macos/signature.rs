//! Whether another process is signed the way this worker is: the check that
//! still recognises RustCode's own engine after an update moved the app the
//! engine was started from.

use super::ffi::{self, CFTypeRef, Cf};
use std::ffi::c_void;

/// Whether the process `pid` satisfies this worker's own designated
/// requirement. An ad hoc signature names one exact build, so only a stable
/// signature (Developer ID or the local certificate) matches across versions.
pub fn same_signer(pid: i32) -> bool {
    designated_requirement().is_some_and(|requirement| {
        guest(pid).is_some_and(|code| {
            // SAFETY: both are live Security objects.
            unsafe { ffi::SecCodeCheckValidity(code.get(), 0, requirement.get()) == 0 }
        })
    })
}

fn designated_requirement() -> Option<Cf> {
    let mut own: CFTypeRef = std::ptr::null();
    // SAFETY: `own` outlives the call, which stores one owned reference.
    if unsafe { ffi::SecCodeCopySelf(0, &raw mut own) } != 0 {
        return None;
    }
    let own = Cf::created(own)?;
    let mut requirement: CFTypeRef = std::ptr::null();
    // SAFETY: `own` is a live SecCode; `requirement` outlives the call.
    if unsafe { ffi::SecCodeCopyDesignatedRequirement(own.get(), 0, &raw mut requirement) } != 0 {
        return None;
    }
    Cf::created(requirement)
}

fn guest(pid: i32) -> Option<Cf> {
    // SAFETY: `pid` outlives the call, which copies it.
    let number = Cf::created(unsafe {
        ffi::CFNumberCreate(
            std::ptr::null(),
            ffi::kCFNumberSInt32Type,
            (&raw const pid).cast::<c_void>(),
        )
    })?;
    // SAFETY: the key is Security's constant; the dictionary retains both.
    let attributes = Cf::created(unsafe {
        let keys = [ffi::kSecGuestAttributePid];
        let values = [number.get()];
        ffi::CFDictionaryCreate(
            std::ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            1,
            &raw const ffi::kCFTypeDictionaryKeyCallBacks,
            &raw const ffi::kCFTypeDictionaryValueCallBacks,
        )
    })?;
    let mut code: CFTypeRef = std::ptr::null();
    // SAFETY: `attributes` is a live dictionary; `code` outlives the call.
    let status = unsafe {
        ffi::SecCodeCopyGuestWithAttributes(std::ptr::null(), attributes.get(), 0, &raw mut code)
    };
    if status != 0 {
        return None;
    }
    Cf::created(code)
}
