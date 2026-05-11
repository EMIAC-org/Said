#[cfg(target_os = "macos")]
mod imp {
    use std::time::Duration;

    use block::ConcreteBlock;
    use cocoa::base::nil;
    use cocoa::foundation::NSString;
    use objc::runtime::Class;
    use objc::{msg_send, sel, sel_impl};

    const AV_AUTHORIZED: i64 = 3;

    fn open_privacy_pane(anchor: &str) {
        let _ = std::process::Command::new("open")
            .arg(format!(
                "x-apple.systempreferences:com.apple.preference.security?{anchor}"
            ))
            .spawn();
    }

    unsafe fn audio_media_type() -> *mut objc::runtime::Object {
        unsafe { NSString::alloc(nil).init_str("soun") }
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
}

#[cfg(not(target_os = "macos"))]
mod imp {
    pub fn microphone_granted() -> bool {
        true
    }
    pub fn request_microphone() -> bool {
        true
    }
}

pub use imp::*;
