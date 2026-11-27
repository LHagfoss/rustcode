//! Reading and acting on other apps through the Accessibility API: the
//! snapshot with refs, and pressing, focusing and inserting text by ref.

use super::ffi::{self, CGPoint, CGRect, CGSize, Cf, cf_string};
use serde_json::{Value, json};
use std::ffi::c_void;

const AX_ERROR_API_DISABLED: i32 = -25211;
const AX_ERROR_INVALID_ELEMENT: i32 = -25202;
const AX_ERROR_CANNOT_COMPLETE: i32 = -25204;
const AX_ERROR_ATTRIBUTE_UNSUPPORTED: i32 = -25205;
const AX_ERROR_ACTION_UNSUPPORTED: i32 = -25206;
const MAX_VISITED: usize = 6000;
const MAX_DEPTH: usize = 48;
const MAX_TEXT: usize = 160;

/// One element of another app.
pub struct Element(Cf);

/// Say what an Accessibility error means for the agent.
pub fn describe(code: i32) -> String {
    match code {
        AX_ERROR_API_DISABLED => "Accessibility permission is missing: turn on RustCode's desktop control in System Settings › Privacy & Security › Accessibility".into(),
        AX_ERROR_INVALID_ELEMENT => "That element is gone; take a new computer_snapshot".into(),
        AX_ERROR_CANNOT_COMPLETE => "The app did not answer (busy or not responding); try again".into(),
        AX_ERROR_ATTRIBUTE_UNSUPPORTED | AX_ERROR_ACTION_UNSUPPORTED => "That element does not support this action".into(),
        other => format!("Accessibility error {other}"),
    }
}

impl Element {
    /// The app with process id `pid`.
    pub fn app(pid: i32) -> Result<Self, String> {
        // SAFETY: creating an application element has no preconditions.
        let element = Cf::created(unsafe { ffi::AXUIElementCreateApplication(pid) })
            .ok_or("Could not reach that app")?;
        // SAFETY: `element` is a live AXUIElement.
        unsafe { ffi::AXUIElementSetMessagingTimeout(element.get(), 3.0) };
        Ok(Self(element))
    }

    fn copy(&self, name: &str) -> Result<Cf, i32> {
        let attribute = cf_string(name);
        let mut value: ffi::CFTypeRef = std::ptr::null();
        // SAFETY: the element and attribute are live; `value` receives a +1 reference.
        let code = unsafe {
            ffi::AXUIElementCopyAttributeValue(self.0.get(), attribute.get(), &mut value)
        };
        if code != 0 {
            return Err(code);
        }
        Cf::created(value).ok_or(AX_ERROR_ATTRIBUTE_UNSUPPORTED)
    }

    fn text(&self, name: &str) -> Option<String> {
        let value = self.copy(name).ok()?;
        let text = ffi::string(value.get())
            .or_else(|| ffi::integer(value.get()).map(|n| n.to_string()))
            .or_else(|| ffi::float(value.get()).map(|n| format!("{n}")))?;
        let text = text.trim().replace('\n', " ");
        (!text.is_empty()).then(|| truncate(&text, MAX_TEXT))
    }

    fn flag(&self, name: &str) -> Option<bool> {
        let value = self.copy(name).ok()?;
        ffi::boolean(value.get())
    }

    fn element(&self, name: &str) -> Option<Element> {
        self.copy(name).ok().map(Element)
    }

    fn elements(&self, name: &str) -> Vec<Element> {
        let Ok(array) = self.copy(name) else {
            return Vec::new();
        };
        ffi::array_items(array.get())
            .into_iter()
            .filter_map(Cf::retained)
            .map(Element)
            .collect()
    }

    fn ax_value<T: Default>(&self, name: &str, kind: u32) -> Option<T> {
        let value = self.copy(name).ok()?;
        let mut out = T::default();
        // SAFETY: the type id is checked and `out` matches `kind`'s layout.
        let read = unsafe {
            ffi::CFGetTypeID(value.get()) == ffi::AXValueGetTypeID()
                && ffi::AXValueGetValue(value.get(), kind, (&raw mut out).cast::<c_void>()) != 0
        };
        read.then_some(out)
    }

    /// Where the element is on screen, in global points.
    pub fn frame(&self) -> Option<CGRect> {
        let origin: CGPoint = self.ax_value("AXPosition", ffi::kAXValueCGPointType)?;
        let size: CGSize = self.ax_value("AXSize", ffi::kAXValueCGSizeType)?;
        Some(CGRect { origin, size })
    }

    pub fn role(&self) -> Result<String, String> {
        let value = self.copy("AXRole").map_err(describe)?;
        Ok(ffi::string(value.get()).unwrap_or_default())
    }

    pub fn is_secure(&self) -> bool {
        self.text("AXSubrole").as_deref() == Some("AXSecureTextField")
            || self.text("AXRole").as_deref() == Some("AXSecureTextField")
    }

    pub fn actions(&self) -> Vec<String> {
        let mut names: ffi::CFArrayRef = std::ptr::null();
        // SAFETY: `names` receives a +1 CFArray.
        let code = unsafe { ffi::AXUIElementCopyActionNames(self.0.get(), &mut names) };
        let Some(names) = (code == 0).then(|| Cf::created(names)).flatten() else {
            return Vec::new();
        };
        ffi::array_items(names.get())
            .into_iter()
            .filter_map(ffi::string)
            .collect()
    }

    pub fn perform(&self, action: &str) -> Result<(), String> {
        let action = cf_string(action);
        // SAFETY: both are live objects.
        let code = unsafe { ffi::AXUIElementPerformAction(self.0.get(), action.get()) };
        if code == 0 {
            Ok(())
        } else {
            Err(describe(code))
        }
    }

    fn set(&self, name: &str, value: ffi::CFTypeRef) -> Result<(), String> {
        let attribute = cf_string(name);
        // SAFETY: all three are live objects.
        let code =
            unsafe { ffi::AXUIElementSetAttributeValue(self.0.get(), attribute.get(), value) };
        if code == 0 {
            Ok(())
        } else {
            Err(describe(code))
        }
    }

    fn settable(&self, name: &str) -> bool {
        let attribute = cf_string(name);
        let mut settable = 0u8;
        // SAFETY: both are live objects; `settable` is written on success.
        let code = unsafe {
            ffi::AXUIElementIsAttributeSettable(self.0.get(), attribute.get(), &mut settable)
        };
        code == 0 && settable != 0
    }

    pub fn focus(&self) -> Result<(), String> {
        // SAFETY: kCFBooleanTrue is a constant.
        self.set("AXFocused", unsafe { ffi::kCFBooleanTrue })
    }

    /// Make this app element the frontmost app, as clicking its Dock icon
    /// would.
    pub fn bring_to_front(&self) -> Result<(), String> {
        // SAFETY: kCFBooleanTrue is a constant.
        self.set("AXFrontmost", unsafe { ffi::kCFBooleanTrue })
    }

    /// Insert `text` at the element's caret, when the app lets
    /// Accessibility edit it directly.
    pub fn insert_text(&self, text: &str) -> Result<bool, String> {
        if !self.settable("AXSelectedText") {
            return Ok(false);
        }
        let value = cf_string(text);
        self.set("AXSelectedText", value.get()).map(|()| true)
    }

    /// The CoreGraphics window id of a window element.
    pub fn window_id(&self) -> Option<u32> {
        let mut id = 0u32;
        // SAFETY: `id` is written on success.
        let code = unsafe { ffi::_AXUIElementGetWindow(self.0.get(), &mut id) };
        (code == 0 && id != 0).then_some(id)
    }

    /// The element of the app that has keyboard focus.
    pub fn focused(&self) -> Option<Element> {
        self.element("AXFocusedUIElement")
    }

    /// The window that contains this element.
    pub fn window(&self) -> Option<Element> {
        self.element("AXWindow")
    }
}

/// The app's window with `window_id`, else its focused or first window.
fn root(app: &Element, window_id: Option<u32>) -> Result<Element, String> {
    let windows = app.elements("AXWindows");
    if windows.is_empty() {
        return match app.copy("AXRole") {
            Err(code) => Err(describe(code)),
            Ok(_) => Err("That app has no windows Accessibility can see".into()),
        };
    }
    if let Some(wanted) = window_id {
        return windows
            .into_iter()
            .find(|window| window.window_id() == Some(wanted))
            .ok_or_else(|| format!("Window {wanted} does not belong to that app"));
    }
    if let Some(focused) = app.element("AXFocusedWindow") {
        return Ok(focused);
    }
    windows
        .into_iter()
        .next()
        .ok_or_else(|| "That app has no windows".into())
}

const QUIET_ROLES: [&str; 8] = [
    "AXGroup",
    "AXScrollArea",
    "AXSplitGroup",
    "AXLayoutArea",
    "AXLayoutItem",
    "AXUnknown",
    "AXSplitter",
    "AXGrowArea",
];

/// Walk one window and list its elements, each with a ref for later actions.
pub fn snapshot(
    pid: i32,
    window_id: Option<u32>,
    max_elements: usize,
) -> Result<(Value, Vec<Element>), String> {
    let app = Element::app(pid)?;
    let window = root(&app, window_id)?;
    let listed_window = window.window_id();
    let mut lines = Vec::new();
    let mut refs = Vec::new();
    let mut visited = 0usize;
    let mut truncated = false;
    let mut stack = vec![(window, 0usize)];
    while let Some((element, depth)) = stack.pop() {
        visited += 1;
        if visited > MAX_VISITED || refs.len() >= max_elements {
            truncated = true;
            break;
        }
        let role = element.text("AXRole").unwrap_or_else(|| "AXUnknown".into());
        let secure = element.is_secure();
        let label = element
            .text("AXTitle")
            .or_else(|| element.text("AXDescription"))
            .or_else(|| element.text("AXPlaceholderValue"));
        let value = if secure {
            None
        } else {
            element.text("AXValue")
        };
        let quiet = QUIET_ROLES.contains(&role.as_str()) && label.is_none() && value.is_none();
        let children = if depth < MAX_DEPTH {
            element.elements("AXChildren")
        } else {
            Vec::new()
        };
        let child_depth = if quiet { depth } else { depth + 1 };
        if !quiet {
            let mut line = format!("{}[e{}] {}", "  ".repeat(depth), refs.len() + 1, role);
            if let Some(label) = &label {
                line.push_str(&format!(" \"{label}\""));
            }
            if let Some(value) = &value
                && Some(value) != label.as_ref()
            {
                line.push_str(&format!(" value=\"{value}\""));
            }
            if secure {
                line.push_str(" (password)");
            }
            if element.flag("AXEnabled") == Some(false) {
                line.push_str(" (disabled)");
            }
            if element.flag("AXFocused") == Some(true) {
                line.push_str(" (focused)");
            }
            lines.push(line);
            refs.push(element);
        }
        for child in children.into_iter().rev() {
            stack.push((child, child_depth));
        }
    }
    let answer = json!({
        "pid": pid,
        "window_id": listed_window,
        "elements": lines.join("\n"),
        "count": refs.len(),
        "truncated": truncated,
    });
    Ok((answer, refs))
}

fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(limit).collect();
    cut.push('…');
    cut
}

/// Whether this process may use Accessibility, optionally asking macOS to
/// show its prompt.
pub fn trusted(prompt: bool) -> bool {
    if !prompt {
        // SAFETY: no arguments.
        return unsafe { ffi::AXIsProcessTrusted() } != 0;
    }
    // SAFETY: the key and value are CF constants.
    let options = unsafe { ffi::dictionary(ffi::kAXTrustedCheckOptionPrompt, ffi::kCFBooleanTrue) };
    match options {
        // SAFETY: `options` is a live CFDictionary.
        Some(options) => unsafe { ffi::AXIsProcessTrustedWithOptions(options.get()) != 0 },
        // SAFETY: no arguments.
        None => unsafe { ffi::AXIsProcessTrusted() != 0 },
    }
}
