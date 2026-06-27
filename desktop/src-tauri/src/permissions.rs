#[cfg(target_os = "macos")]
mod imp {
    use std::time::Duration;

    use block::ConcreteBlock;
    use objc::runtime::Class;
    use objc::{msg_send, sel, sel_impl};

    const AV_AUTHORIZED: i64 = 3;

    // CoreGraphics screen-capture permission (gates ScreenCaptureKit, which is how
    // meetings capture system audio). Preflight checks without prompting; Request
    // raises the TCC dialog the first time and is a no-op afterward.
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
        fn CGRequestScreenCaptureAccess() -> bool;
    }

    fn open_privacy_pane(anchor: &str) {
        let _ = std::process::Command::new("open")
            .arg(format!(
                "x-apple.systempreferences:com.apple.preference.security?{anchor}"
            ))
            .spawn();
    }

    unsafe fn audio_media_type() -> *mut objc::runtime::Object {
        // AVMediaTypeAudio is the NSString "soun". Build it via objc msg_send
        // directly so we don't depend on the deprecated cocoa::foundation helpers.
        // Owned (+1) via alloc/init; callers release it like the previous code did.
        unsafe {
            let cls = Class::get("NSString").expect("NSString class is always available");
            let obj: *mut objc::runtime::Object = msg_send![cls, alloc];
            let s: *mut objc::runtime::Object =
                msg_send![obj, initWithUTF8String: c"soun".as_ptr()];
            s
        }
    }

    pub fn microphone_granted() -> bool {
        unsafe {
            let Some(cls) = Class::get("AVCaptureDevice") else {
                return false;
            };
            let media_type = audio_media_type();
            let status: i64 = msg_send![cls, authorizationStatusForMediaType: media_type];
            let _: () = msg_send![media_type, release];
            status == AV_AUTHORIZED
        }
    }

    pub fn request_microphone() -> bool {
        unsafe {
            let Some(cls) = Class::get("AVCaptureDevice") else {
                open_privacy_pane("Privacy_Microphone");
                return false;
            };

            let media_type = audio_media_type();
            let status: i64 = msg_send![cls, authorizationStatusForMediaType: media_type];
            if status == AV_AUTHORIZED {
                let _: () = msg_send![media_type, release];
                return true;
            }

            let (tx, rx) = std::sync::mpsc::channel::<bool>();
            let block = ConcreteBlock::new(move |granted: bool| {
                let _ = tx.send(granted);
            })
            .copy();
            let _: () = msg_send![
                cls,
                requestAccessForMediaType: media_type
                completionHandler: &*block
            ];

            let granted = rx.recv_timeout(Duration::from_secs(120)).unwrap_or(false);
            let _: () = msg_send![media_type, release];
            if !granted {
                open_privacy_pane("Privacy_Microphone");
            }
            granted
        }
    }

    /// Whether Screen Recording is already granted (no prompt). Meetings need this
    /// because system-audio capture goes through ScreenCaptureKit.
    pub fn screen_recording_granted() -> bool {
        unsafe { CGPreflightScreenCaptureAccess() }
    }

    /// Ensure Screen Recording access: returns true if already granted, otherwise
    /// raises the macOS prompt (first time) and opens the Screen Recording pane,
    /// returning the resulting grant state. NOTE: macOS often only honors a fresh
    /// grant after the app is relaunched, so callers should treat `false` as
    /// "not ready yet — guide the user, don't start capture".
    pub fn request_screen_recording() -> bool {
        unsafe {
            if CGPreflightScreenCaptureAccess() {
                return true;
            }
            let granted = CGRequestScreenCaptureAccess();
            if !granted {
                open_privacy_pane("Privacy_ScreenCapture");
            }
            granted
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    // Windows desktop apps (unpackaged Win32) have NO reliable pre-flight mic-
    // permission query — the WinRT AppCapability model is for packaged/UWP apps.
    // Denial is enforced at capture time (silence / access-denied), so we can't
    // truly detect grant state here. We report "granted" and rely on the
    // capture-time error (now platform-correctly worded in the recorder crate) to
    // tell the user what to do, and `request_microphone` opens the right settings.
    pub fn microphone_granted() -> bool {
        true
    }
    pub fn request_microphone() -> bool {
        // Deep-link straight to the Windows microphone privacy page so the user
        // can flip access on. CREATE_NO_WINDOW avoids a console flash.
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            let _ = std::process::Command::new("cmd")
                .args(["/C", "start", "", "ms-settings:privacy-microphone"])
                .creation_flags(CREATE_NO_WINDOW)
                .spawn();
        }
        true
    }

    // Screen Recording is a macOS-only TCC permission; other platforms don't gate
    // system-audio capture this way, so report it as always available.
    pub fn screen_recording_granted() -> bool {
        true
    }
    pub fn request_screen_recording() -> bool {
        true
    }
}

pub use imp::*;
