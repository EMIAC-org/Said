//! Windows implementation — stubbed scaffolding for P0.
//!
//! Real implementation lands in P3:
//!
//!   * Dedicated `said-paster-uia` thread with `CoInitializeEx(MTA)` for
//!     UI Automation calls
//!   * Six-strategy focused-text read stack mirroring `macos.rs`:
//!     1. `IUIAutomationTextPattern::DocumentRange.GetText(-1)`
//!     2. `IUIAutomationValuePattern::CurrentValue`
//!     3. `IUIAutomationTextPattern::GetSelection`
//!     4. MSAA `accValue` (legacy Win32 controls)
//!     5. `WM_GETTEXT` via `AttachThreadInput`
//!     6. Ctrl+A + Ctrl+C clipboard fallback
//!   * `SendInput` with `KEYEVENTF_UNICODE` for typing, mirroring the macOS
//!     6 ms keydown→keyup cadence (see "HID delays are sacred" in AGENTS.md)
//!   * `arboard` for cross-platform Unicode-safe clipboard save/restore
//!
//! For P0 the only real-on-Windows function is `paste(text)` — which copies
//! to the clipboard via `cmd /C clip` (pre-existing behavior). Everything
//! else returns sensible no-op defaults so the workspace compiles and the
//! Tauri shell can boot on `windows-latest` for CI.

use std::io::Write;
use std::process::Command;

use crate::shared::{AxDiagnostics, AxMethodResult};

fn copy_to_clipboard(text: &str) -> Result<(), String> {
    let mut child = Command::new("cmd")
        .args(["/C", "clip"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to launch clipboard helper: {e}"))?;
    if let Some(ref mut stdin) = child.stdin {
        stdin
            .write_all(text.as_bytes())
            .map_err(|e| format!("failed to write clipboard contents: {e}"))?;
    }
    child
        .wait()
        .map_err(|e| format!("clipboard helper failed: {e}"))?;
    Ok(())
}

pub fn request_permission() {}
pub fn request_input_monitoring() {}

/// Windows UIA needs no permission gate. Real impl (P3) will probe a UIA
/// element fetch once and report failure if COM is broken, but the conceptual
/// "accessibility granted" is unconditionally true.
pub fn is_accessibility_granted() -> bool {
    true
}

pub fn read_focused_value_fast() -> Option<String> {
    None
}
pub fn read_focused_value_first() -> Option<String> {
    None
}
pub fn read_focused_value() -> Option<String> {
    read_focused_value_first()
}
pub fn read_focused_value_fast_for_pid(_pid: i32) -> Option<String> {
    None
}
pub fn read_focused_value_first_for_pid(_pid: i32) -> Option<String> {
    None
}
pub fn capture_focused_text_via_selection() -> Option<String> {
    None
}
pub fn read_selected_text() -> Option<String> {
    None
}

pub fn diagnose_focused_field() -> AxDiagnostics {
    AxDiagnostics {
        ax_trusted: true, // UIA needs no permission on Windows
        app_name: None,
        app_pid: None,
        element_role: None,
        attributes: vec![],
        methods: vec![AxMethodResult {
            method: "0_stub".into(),
            label: "Windows UIA implementation pending (P3)".into(),
            ok: false,
            text: None,
            err: Some("not implemented".into()),
        }],
        clipboard: String::new(),
    }
}

pub fn focused_pid() -> Option<i32> {
    None
}
pub fn unlock_focused_app_now() -> Option<i32> {
    None
}
pub fn lock_frontmost_app_now() -> Option<i32> {
    None
}

pub fn type_text(_text: &str) -> Result<bool, String> {
    Ok(false)
}

pub fn paste(text: &str) -> Result<(), String> {
    copy_to_clipboard(text)
}

pub fn paste_replacing(text: &str) -> Result<(), String> {
    copy_to_clipboard(text)
}
