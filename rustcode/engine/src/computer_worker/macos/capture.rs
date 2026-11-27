//! The window list (CoreGraphics) and one window's screenshot
//! (ScreenCaptureKit, macOS 14 and later).

use super::ffi::{self, CGRect, Cf};
use base64::Engine as _;
use block2::RcBlock;
use objc2::msg_send;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject};
use serde_json::{Value, json};
use std::ffi::{CStr, c_char, c_void};
use std::sync::mpsc;
use std::time::Duration;

/// The longest side of a screenshot, in pixels, so it fits a model's image input.
const MAX_SIDE: f64 = 1600.0;
const WAIT: Duration = Duration::from_secs(10);

/// One on-screen window of a normal app.
#[derive(Debug, Clone)]
pub struct Window {
    pub window_id: u32,
    pub pid: i32,
    pub app: String,
    pub title: Option<String>,
    pub bounds: CGRect,
}

impl Window {
    pub fn json(&self) -> Value {
        json!({
            "window_id": self.window_id,
            "pid": self.pid,
            "app": self.app,
            "title": self.title,
            "bounds": rect_json(self.bounds),
        })
    }
}

pub fn rect_json(rect: CGRect) -> Value {
    json!({
        "x": rect.origin.x.round(),
        "y": rect.origin.y.round(),
        "width": rect.size.width.round(),
        "height": rect.size.height.round(),
    })
}

/// The windows on screen, front to back, without menus, the Dock or this worker's own.
pub fn windows() -> Vec<Window> {
    let own = std::process::id() as i64;
    // SAFETY: the call has no preconditions; the answer is a +1 CFArray.
    let Some(list) = Cf::created(unsafe {
        ffi::CGWindowListCopyWindowInfo(
            ffi::kCGWindowListOptionOnScreenOnly | ffi::kCGWindowListExcludeDesktopElements,
            0,
        )
    }) else {
        return Vec::new();
    };
    let mut windows = Vec::new();
    for entry in ffi::array_items(list.get()) {
        // SAFETY: the keys are CoreGraphics constants and `entry` a CFDictionary.
        let field = |key: ffi::CFStringRef| ffi::dictionary_value(entry, key);
        let (number, pid, layer, owner, name, bounds) = unsafe {
            (
                field(ffi::kCGWindowNumber),
                field(ffi::kCGWindowOwnerPID),
                field(ffi::kCGWindowLayer),
                field(ffi::kCGWindowOwnerName),
                field(ffi::kCGWindowName),
                field(ffi::kCGWindowBounds),
            )
        };
        let (Some(window_id), Some(pid)) = (ffi::integer(number), ffi::integer(pid)) else {
            continue;
        };
        if ffi::integer(layer) != Some(0) || pid == own {
            continue;
        }
        let mut rect = CGRect::default();
        // SAFETY: `bounds` is the window's bounds dictionary, or null.
        if bounds.is_null()
            || !unsafe { ffi::CGRectMakeWithDictionaryRepresentation(bounds, &mut rect) }
        {
            continue;
        }
        windows.push(Window {
            window_id: window_id as u32,
            pid: pid as i32,
            app: ffi::string(owner).unwrap_or_default(),
            title: ffi::string(name).filter(|title| !title.is_empty()),
            bounds: rect,
        });
    }
    windows
}

/// A captured window: PNG bytes, its pixel size, and the window's bounds in points.
pub struct Shot {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub bounds: CGRect,
}

impl Shot {
    pub fn json(&self, pid: i32, window_id: u32) -> Value {
        json!({
            "pid": pid,
            "window_id": window_id,
            "width": self.width,
            "height": self.height,
            "bounds": rect_json(self.bounds),
            "data": base64::engine::general_purpose::STANDARD.encode(&self.png),
        })
    }
}

fn class(name: &CStr) -> Result<&'static AnyClass, String> {
    AnyClass::get(name).ok_or_else(|| "ScreenCaptureKit needs macOS 14 or later".to_owned())
}

/// The text of an NSString or an NSError's description.
pub fn ns_text(object: &AnyObject) -> String {
    // SAFETY: every NSObject answers `description`, and UTF8String is valid while it lives.
    unsafe {
        let description: Retained<AnyObject> = msg_send![object, description];
        let text: *const c_char = msg_send![&description, UTF8String];
        if text.is_null() {
            return String::new();
        }
        CStr::from_ptr(text).to_string_lossy().into_owned()
    }
}

fn error_text(error: *mut AnyObject, fallback: &str) -> String {
    // SAFETY: a completion handler's error is an NSError or nil.
    match unsafe { error.as_ref() } {
        Some(error) => {
            // SAFETY: NSError answers localizedDescription with an NSString.
            let description: Retained<AnyObject> =
                unsafe { msg_send![error, localizedDescription] };
            ns_text(&description)
        }
        None => fallback.to_owned(),
    }
}

fn shareable_window(window_id: u32) -> Result<Retained<AnyObject>, String> {
    let (sender, receiver) = mpsc::channel::<Result<usize, String>>();
    let handler = RcBlock::new(move |content: *mut AnyObject, error: *mut AnyObject| {
        // SAFETY: `content` is an SCShareableContent or nil; retaining keeps it past the block.
        let answer = match unsafe { Retained::retain(content) } {
            Some(content) => Ok(Retained::into_raw(content) as usize),
            None => Err(error_text(error, "Screen Recording permission is missing")),
        };
        let _ = sender.send(answer);
    });
    let content_class = class(c"SCShareableContent")?;
    // SAFETY: the selector and argument types match ScreenCaptureKit's declaration.
    unsafe {
        let _: () = msg_send![
            content_class,
            getShareableContentExcludingDesktopWindows: true,
            onScreenWindowsOnly: true,
            completionHandler: &*handler
        ];
    }
    let raw = receiver
        .recv_timeout(WAIT)
        .map_err(|_| "ScreenCaptureKit did not answer".to_owned())??;
    // SAFETY: `raw` is the +1 reference retained in the handler.
    let content = unsafe { Retained::from_raw(raw as *mut AnyObject) }
        .ok_or("ScreenCaptureKit returned nothing")?;
    // SAFETY: SCShareableContent.windows is an NSArray of SCWindow.
    unsafe {
        let windows: Retained<AnyObject> = msg_send![&content, windows];
        let count: usize = msg_send![&windows, count];
        for index in 0..count {
            let window: Retained<AnyObject> = msg_send![&windows, objectAtIndex: index];
            let id: u32 = msg_send![&window, windowID];
            if id == window_id {
                return Ok(window);
            }
        }
    }
    Err(format!(
        "Window {window_id} is not on screen, or Screen Recording permission is missing"
    ))
}

/// Capture one window, without the pointer, scaled to fit `MAX_SIDE`.
pub fn screenshot(window_id: u32) -> Result<Shot, String> {
    let window = shareable_window(window_id)?;
    let filter_class = class(c"SCContentFilter")?;
    let config_class = class(c"SCStreamConfiguration")?;
    let manager_class = class(c"SCScreenshotManager")?;
    // SAFETY: the selectors and argument types match ScreenCaptureKit's declarations.
    let (filter, config, bounds, width, height) = unsafe {
        let bounds: CGRect = msg_send![&window, frame];
        let allocated: Allocated<AnyObject> = msg_send![filter_class, alloc];
        let filter: Retained<AnyObject> =
            msg_send![allocated, initWithDesktopIndependentWindow: &*window];
        let scale: f32 = msg_send![&filter, pointPixelScale];
        let native_width = bounds.size.width * f64::from(scale);
        let native_height = bounds.size.height * f64::from(scale);
        let shrink = (MAX_SIDE / native_width.max(native_height)).min(1.0);
        let width = (native_width * shrink).round().max(1.0) as usize;
        let height = (native_height * shrink).round().max(1.0) as usize;
        let config: Retained<AnyObject> = msg_send![config_class, new];
        let _: () = msg_send![&config, setWidth: width];
        let _: () = msg_send![&config, setHeight: height];
        let _: () = msg_send![&config, setShowsCursor: false];
        let _: () = msg_send![&config, setScalesToFit: true];
        (filter, config, bounds, width, height)
    };
    let (sender, receiver) = mpsc::channel::<Result<usize, String>>();
    let handler = RcBlock::new(move |image: *mut c_void, error: *mut AnyObject| {
        let answer = if image.is_null() {
            Err(error_text(error, "The window could not be captured"))
        } else {
            // SAFETY: `image` is a live CGImage; this retain keeps it past the block.
            Ok(unsafe { ffi::CFRetain(image.cast_const()) } as usize)
        };
        let _ = sender.send(answer);
    });
    // SAFETY: as above.
    unsafe {
        let _: () = msg_send![
            manager_class,
            captureImageWithFilter: &*filter,
            configuration: &*config,
            completionHandler: &*handler
        ];
    }
    let image = receiver
        .recv_timeout(WAIT)
        .map_err(|_| "ScreenCaptureKit did not answer".to_owned())??;
    let image = Cf::created(image as ffi::CGImageRef).ok_or("No image")?;
    let (png, width, height) = encode_png(image.get(), width, height)?;
    Ok(Shot {
        png,
        width,
        height,
        bounds,
    })
}

fn encode_png(
    image: ffi::CGImageRef,
    width: usize,
    height: usize,
) -> Result<(Vec<u8>, u32, u32), String> {
    let mut pixels = vec![0u8; width * height * 4];
    // SAFETY: the buffer holds `height` rows of `width * 4` bytes for the context's life.
    unsafe {
        let space = ffi::CGColorSpaceCreateDeviceRGB();
        let context = ffi::CGBitmapContextCreate(
            pixels.as_mut_ptr().cast(),
            width,
            height,
            8,
            width * 4,
            space,
            1,
        );
        ffi::CGColorSpaceRelease(space);
        if context.is_null() {
            return Err("Could not draw the screenshot".into());
        }
        ffi::CGContextSetInterpolationQuality(context, 3);
        ffi::CGContextDrawImage(
            context,
            CGRect::new(0.0, 0.0, width as f64, height as f64),
            image,
        );
        ffi::CGContextRelease(context);
    }
    for pixel in pixels.chunks_exact_mut(4) {
        let alpha = u32::from(pixel[3]);
        if alpha > 0 && alpha < 255 {
            for channel in &mut pixel[..3] {
                *channel = ((u32::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8;
            }
        }
    }
    let (width, height) = (width as u32, height as u32);
    let image = image::RgbaImage::from_raw(width, height, pixels).ok_or("Bad screenshot size")?;
    let mut png = Vec::new();
    image
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok((png, width, height))
}
