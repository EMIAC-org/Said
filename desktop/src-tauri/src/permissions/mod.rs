//! Cross-platform microphone permission helpers.
//!
//! Tauri commands call `microphone_granted()` and `request_microphone()`
//! through this façade; each platform module implements the same surface:
//!
//! | Platform | Mechanism                                                       |
//! |----------|-----------------------------------------------------------------|
//! | macOS    | `AVCaptureDevice.authorizationStatus(forMediaType: "soun")`     |
//! |          | + `requestAccessForMediaType:completionHandler:` (async grant)  |
//! | Windows  | `recorder::probe_mic_permission()` (cpal/WASAPI error mapping)  |
//! |          | + `start ms-settings:privacy-microphone` flyout on denial       |
//!
//! There is no Accessibility / Input-Monitoring equivalent on Windows;
//! those permission rows are hidden in the React Settings panel.

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::*;

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::*;
