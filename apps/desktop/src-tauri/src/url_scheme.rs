//! Who opens `rustcode://` links (`lib/deepLinks.ts`): this copy of the app, or
//! another one. A second or older install (a copy in Downloads, a previous
//! version kept as a backup) can own the scheme, and then a thread or prompt
//! link opens there. Settings › General reads this and offers to take it back.
//!
//! macOS: Launch Services' default handler for the scheme, compared with this
//! bundle; the fix makes this bundle the default. Windows and Linux: the deep
//! link plugin's registration check (registry command / xdg-mime default), and
//! its registration as the fix.

use serde::Serialize;
#[cfg(any(test, target_os = "macos"))]
use std::path::Path;

/// The scheme in tauri.conf.json (`plugins.deep-link.desktop.schemes`).
pub const SCHEME: &str = "rustcode";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SchemeOwner {
    /// Links open in this copy.
    This,
    /// Another app, or another copy of this one, opens them.
    Other,
    /// Nothing is registered for the scheme (told apart on macOS only).
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    None,
    /// Not checkable here: a development build that is not an app bundle, or a check that failed.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemeStatus {
    pub owner: SchemeOwner,
    /// The app that opens the links when it is not this one, where the system says.
    pub handler: Option<String>,
    /// Whether `claim_url_scheme` can make this copy the handler.
    pub can_fix: bool,
}

/// The owner, from this app's bundle path and the handler's (macOS).
#[cfg(any(test, target_os = "macos"))]
fn owner_of(own: Option<&Path>, handler: Option<&Path>) -> SchemeOwner {
    let Some(own) = own.filter(|p| p.extension().is_some_and(|e| e == "app")) else {
        return SchemeOwner::Unknown;
    };
    let Some(handler) = handler else {
        return SchemeOwner::None;
    };
    let canonical = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    if canonical(own) == canonical(handler) {
        SchemeOwner::This
    } else {
        SchemeOwner::Other
    }
}

#[tauri::command(async)]
pub fn url_scheme_status(app: tauri::AppHandle) -> SchemeStatus {
    #[cfg(target_os = "macos")]
    {
        let _ = app;
        let own = mac::own_bundle();
        let handler = mac::handler();
        let owner = owner_of(own.as_deref(), handler.as_deref());
        SchemeStatus {
            owner,
            handler: (owner == SchemeOwner::Other)
                .then(|| handler.map(|p| p.display().to_string()))
                .flatten(),
            can_fix: owner != SchemeOwner::Unknown,
        }
    }
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    {
        use tauri_plugin_deep_link::DeepLinkExt;
        let owner = match app.deep_link().is_registered(SCHEME) {
            Ok(true) => SchemeOwner::This,
            Ok(false) => SchemeOwner::Other,
            Err(_) => SchemeOwner::Unknown,
        };
        SchemeStatus {
            owner,
            handler: None,
            can_fix: owner != SchemeOwner::Unknown,
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        let _ = app;
        SchemeStatus {
            owner: SchemeOwner::Unknown,
            handler: None,
            can_fix: false,
        }
    }
}

/// Makes this copy open `rustcode://` links, then reports where that left things.
#[tauri::command(async)]
pub fn claim_url_scheme(app: tauri::AppHandle) -> Result<SchemeStatus, String> {
    #[cfg(target_os = "macos")]
    mac::claim()?;
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    {
        use tauri_plugin_deep_link::DeepLinkExt;
        app.deep_link()
            .register(SCHEME)
            .map_err(|e| e.to_string())?;
    }
    Ok(url_scheme_status(app))
}

#[cfg(target_os = "macos")]
mod mac {
    use super::SCHEME;
    use objc2::msg_send;
    use objc2::rc::autoreleasepool;
    use objc2::runtime::{AnyClass, AnyObject};
    use std::ffi::{c_char, CStr, CString};
    use std::path::PathBuf;
    use std::sync::mpsc;
    use std::time::Duration;

    // SAFETY (all helpers): documented Foundation/AppKit methods, called inside an autorelease
    // pool; the references they return do not outlive it.

    unsafe fn ns_string<'a>(text: &str) -> Option<&'a AnyObject> {
        let text = CString::new(text).ok()?;
        let class = AnyClass::get(c"NSString")?;
        let string: *mut AnyObject = msg_send![class, stringWithUTF8String: text.as_ptr()];
        string.as_ref()
    }

    unsafe fn rust_string(string: &AnyObject) -> Option<String> {
        let utf8: *const c_char = msg_send![string, UTF8String];
        (!utf8.is_null()).then(|| CStr::from_ptr(utf8).to_string_lossy().into_owned())
    }

    /// A file URL's path.
    unsafe fn url_path(url: &AnyObject) -> Option<PathBuf> {
        let path: *mut AnyObject = msg_send![url, path];
        rust_string(path.as_ref()?).map(PathBuf::from)
    }

    unsafe fn workspace<'a>() -> Option<&'a AnyObject> {
        let class = AnyClass::get(c"NSWorkspace")?;
        let shared: *mut AnyObject = msg_send![class, sharedWorkspace];
        shared.as_ref()
    }

    /// `NSBundle.mainBundle.bundleURL`.
    unsafe fn own_bundle_url<'a>() -> Option<&'a AnyObject> {
        let class = AnyClass::get(c"NSBundle")?;
        let bundle: *mut AnyObject = msg_send![class, mainBundle];
        let url: *mut AnyObject = msg_send![bundle.as_ref()?, bundleURL];
        url.as_ref()
    }

    /// This running app's bundle (`…/RustCode.app`), or the binary's folder in a development build.
    pub fn own_bundle() -> Option<PathBuf> {
        autoreleasepool(|_| unsafe { url_path(own_bundle_url()?) })
    }

    /// The app Launch Services opens `rustcode://` links with.
    pub fn handler() -> Option<PathBuf> {
        autoreleasepool(|_| unsafe {
            let class = AnyClass::get(c"NSURL")?;
            let text = ns_string(&format!("{SCHEME}://"))?;
            let link: *mut AnyObject = msg_send![class, URLWithString: text];
            let app: *mut AnyObject =
                msg_send![workspace()?, URLForApplicationToOpenURL: link.as_ref()?];
            url_path(app.as_ref()?)
        })
    }

    /// Makes this bundle the scheme's default handler (macOS 12+; the app needs 13).
    pub fn claim() -> Result<(), String> {
        let own = own_bundle().ok_or("This copy of the app has no bundle to register.")?;
        if own.extension().is_none_or(|e| e != "app") {
            return Err("A development build cannot open links; use the installed app.".into());
        }
        let (tx, rx) = mpsc::channel::<Option<String>>();
        let block = block2::RcBlock::new(move |error: *mut AnyObject| {
            // SAFETY: the handler's argument is an NSError or nil.
            let message = unsafe { error.as_ref() }.map(|error| unsafe {
                let text: *mut AnyObject = msg_send![error, localizedDescription];
                text.as_ref()
                    .and_then(|t| rust_string(t))
                    .filter(|t| !t.is_empty())
                    .unwrap_or_else(|| "Launch Services refused the change.".into())
            });
            let _ = tx.send(message);
        });
        autoreleasepool(|_| unsafe {
            let own_url = own_bundle_url().ok_or("The app's location could not be read.")?;
            let scheme = ns_string(SCHEME).ok_or("The scheme could not be read.")?;
            let workspace = workspace().ok_or("Launch Services is not available.")?;
            // The completion handler is copied by the callee and called once.
            let _: () = msg_send![
                workspace,
                setDefaultApplicationAtURL: own_url,
                toOpenURLsWithScheme: scheme,
                completionHandler: &*block
            ];
            Ok::<(), String>(())
        })?;
        match rx.recv_timeout(Duration::from_secs(30)) {
            Ok(None) => Ok(()),
            Ok(Some(message)) => Err(message),
            Err(_) => Err("macOS did not answer; try again.".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn scheme_matches_the_config() {
        let config = include_str!("../tauri.conf.json");
        assert!(config.contains(&format!("\"schemes\": [\"{SCHEME}\"]")));
    }

    #[test]
    fn this_bundle_owns_its_own_handler() {
        let own = Path::new("/Applications/RustCode.app");
        assert_eq!(owner_of(Some(own), Some(own)), SchemeOwner::This);
    }

    #[test]
    fn another_copy_is_another_owner() {
        assert_eq!(
            owner_of(
                Some(Path::new("/Applications/RustCode.app")),
                Some(Path::new("/Users/me/Downloads/RustCode.app"))
            ),
            SchemeOwner::Other
        );
    }

    #[test]
    fn no_handler_is_none() {
        assert_eq!(
            owner_of(Some(Path::new("/Applications/RustCode.app")), None),
            SchemeOwner::None
        );
    }

    #[test]
    fn a_dev_build_is_not_checked() {
        assert_eq!(
            owner_of(
                Some(Path::new("/repo/target/debug")),
                Some(Path::new("/Applications/RustCode.app"))
            ),
            SchemeOwner::Unknown
        );
        assert_eq!(owner_of(None, None), SchemeOwner::Unknown);
    }

    #[test]
    fn status_serializes_as_the_page_reads_it() {
        let status = SchemeStatus {
            owner: SchemeOwner::Other,
            handler: Some("/x/RustCode.app".into()),
            can_fix: true,
        };
        assert_eq!(
            serde_json::to_string(&status).unwrap(),
            r#"{"owner":"other","handler":"/x/RustCode.app","canFix":true}"#
        );
    }
}
