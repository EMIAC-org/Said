//! Windows microphone permission.
//!
//! Windows 10/11 has no programmatic request — the user grants mic access
//! through Settings → Privacy → Microphone. We can only:
//!
//!   1. Probe via cpal: if WASAPI returns `HRESULT 0x80070005` ("access
//!      denied"), Settings → Privacy → Microphone has disabled desktop
//!      apps. Otherwise enumeration succeeds and we treat it as Granted.
//!   2. On denial, open the Microphone privacy flyout via the deep-link
//!      URI `ms-settings:privacy-microphone`.
//!
//! There is no Accessibility-permission or Input-Monitoring-permission
//! analogue on Windows — UIA and `WH_KEYBOARD_LL` require no consent.

use std::os::windows::process::CommandExt;
use std::process::Command;

use said_recorder::MicPermission;

/// Suppresses the black console flash on `cmd /C start ...`.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn microphone_granted() -> bool {
    matches!(
        said_recorder::probe_mic_permission(),
        MicPermission::Granted
    )
}

pub fn request_microphone() -> bool {
    match said_recorder::probe_mic_permission() {
        MicPermission::Granted => true,
        MicPermission::DeniedByPrivacySettings => {
            open_privacy_microphone_pane();
            false
        }
        // For Unknown / NoDevice we still bounce the user to the settings
        // pane: the most common cause we've seen on test machines is the
        // privacy toggle, and giving the user one explicit place to look
        // beats a silent failure.
        _ => {
            open_privacy_microphone_pane();
            false
        }
    }
}

fn open_privacy_microphone_pane() {
    let _ = Command::new("cmd")
        .args(["/C", "start", "ms-settings:privacy-microphone"])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
}
