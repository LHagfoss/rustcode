//! Microphone access for voice dictation (`ui/Dictation.tsx`).
//!
//! On macOS the web view's `getUserMedia` fails without a word when the app
//! itself has no microphone access, so dictation asks here first: the
//! AVFoundation authorization status, and the system prompt (with
//! Info.plist's `NSMicrophoneUsageDescription`) when the user has not yet
//! been asked. Other systems answer `unsupported`, and the page records as
//! it did before.

use serde::Serialize;

/// Where this app stands with the microphone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub enum MicAccess {
    Granted,
    Denied,
    /// A device policy (parental controls, MDM) refuses it; the user may not be able to change it.
    Restricted,
    NotDetermined,
    /// Not a system with an app-level microphone permission to check.
    Unsupported,
}

/// `AVAuthorizationStatus`: notDetermined 0, restricted 1, denied 2, authorized 3.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn from_av_status(status: isize) -> MicAccess {
    match status {
        0 => MicAccess::NotDetermined,
        1 => MicAccess::Restricted,
        2 => MicAccess::Denied,
        3 => MicAccess::Granted,
        // A status a later macOS adds: let the page try, and its own error says the rest.
        _ => MicAccess::Unsupported,
    }
}

/// The pane of System Settings that lists the apps allowed the microphone.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const MICROPHONE_SETTINGS: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone";

/// The status, after asking the user first when `request` is set and they have not been asked.
#[tauri::command(async)]
pub fn microphone_access(request: bool) -> MicAccess {
    #[cfg(target_os = "macos")]
    {
        let status = mac::status();
        if request && status == MicAccess::NotDetermined {
            return mac::request();
        }
        status
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = request;
        MicAccess::Unsupported
    }
}

/// Opens Privacy & Security › Microphone in System Settings.
#[tauri::command(async)]
pub fn open_microphone_settings() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        crate::process::command("open")
            .arg(MICROPHONE_SETTINGS)
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("Only the macOS app has a microphone setting to open.".into())
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use super::{from_av_status, MicAccess};
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, AnyObject, Bool};
    use std::sync::mpsc;
    use std::time::Duration;

    #[link(name = "AVFoundation", kind = "framework")]
    extern "C" {
        /// `AVMediaTypeAudio`, an `NSString *` constant.
        static AVMediaTypeAudio: &'static AnyObject;
    }

    /// The user has this long to answer the system prompt before the page is told the status as it is.
    const PROMPT_WAIT: Duration = Duration::from_secs(120);

    fn device_class() -> Option<&'static AnyClass> {
        AnyClass::get(c"AVCaptureDevice")
    }

    pub fn status() -> MicAccess {
        let Some(class) = device_class() else {
            return MicAccess::Unsupported;
        };
        // SAFETY: a documented class method taking an AVMediaType, returning an NSInteger.
        let status: isize =
            unsafe { msg_send![class, authorizationStatusForMediaType: AVMediaTypeAudio] };
        from_av_status(status)
    }

    /// Shows the system prompt and waits for the answer.
    pub fn request() -> MicAccess {
        let Some(class) = device_class() else {
            return MicAccess::Unsupported;
        };
        let (tx, rx) = mpsc::channel::<bool>();
        let block = block2::RcBlock::new(move |granted: Bool| {
            let _ = tx.send(granted.as_bool());
        });
        // SAFETY: a documented class method; the handler is called once, on any queue, and the
        // block is copied by the callee, so dropping ours after the call is fine.
        unsafe {
            let _: () = msg_send![
                class,
                requestAccessForMediaType: AVMediaTypeAudio,
                completionHandler: &*block
            ];
        }
        match rx.recv_timeout(PROMPT_WAIT) {
            Ok(true) => MicAccess::Granted,
            Ok(false) => status(),
            Err(_) => status(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn av_statuses_map_to_access() {
        assert_eq!(from_av_status(0), MicAccess::NotDetermined);
        assert_eq!(from_av_status(1), MicAccess::Restricted);
        assert_eq!(from_av_status(2), MicAccess::Denied);
        assert_eq!(from_av_status(3), MicAccess::Granted);
        assert_eq!(from_av_status(9), MicAccess::Unsupported);
    }

    #[test]
    fn access_serializes_as_the_page_reads_it() {
        assert_eq!(
            serde_json::to_string(&MicAccess::NotDetermined).unwrap(),
            "\"not-determined\""
        );
        assert_eq!(
            serde_json::to_string(&MicAccess::Granted).unwrap(),
            "\"granted\""
        );
    }

    #[test]
    fn settings_link_opens_the_microphone_pane() {
        assert!(MICROPHONE_SETTINGS.starts_with("x-apple.systempreferences:"));
        assert!(MICROPHONE_SETTINGS.ends_with("Privacy_Microphone"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn status_message_matches_avfoundation() {
        // objc2 checks encodings in debug builds: a wrong argument type panics here.
        let _ = mac::status();
    }
}
