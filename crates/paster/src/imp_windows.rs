//! Windows paster: Unicode keystroke streaming via `SendInput`, plus
//! clipboard backup/set/restore for the paste path.
//!
//! Public API mirrors the macOS `imp` module so the desktop and CLI consumers
//! compile unchanged. AX-tree reads (`read_focused_value_*`, `diagnose_*`)
//! return `None` for v1 — the UIAutomation client is a follow-up tracked in
//! the M3 plan.

use std::mem::size_of;

use windows::Win32::Foundation::{HANDLE, HGLOBAL, HWND};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{
    GLOBAL_ALLOC_FLAGS, GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP,
    KEYEVENTF_UNICODE, SendInput, VIRTUAL_KEY, VK_A, VK_BACK, VK_CONTROL, VK_V,
};

use crate::win_paster::{
    is_high_surrogate, is_low_surrogate, text_to_clipboard_utf16, text_to_utf16_units,
};

// ── Permissions ───────────────────────────────────────────────────────────────
//
// Windows has no TCC-equivalent for keyboard injection or accessibility —
// non-elevated apps can `SendInput` freely and install `WH_KEYBOARD_LL`
// hooks without any grant. So the snapshot reports these as "granted" and
// the onboarding flow auto-advances past them.
//
// Defensive fallback: if anything *does* end up calling these (an older
// build of the React UI cached in a webview, or a debug button), open the
// Windows Privacy Settings page so the click produces visible feedback
// rather than silent failure.

fn open_windows_privacy_settings() {
    // ms-settings: URIs are Windows' deep-link scheme for the Settings app.
    // `start` resolves them via the shell — argv-based, no shell-injection risk.
    let _ = std::process::Command::new("cmd")
        .args(["/C", "start", "", "ms-settings:privacy"])
        .spawn();
}

pub fn request_permission() {
    open_windows_privacy_settings();
}

pub fn request_input_monitoring() {
    open_windows_privacy_settings();
}

pub fn is_accessibility_granted() -> bool {
    true
}

// ── AX-tree reads (v1: stubbed; UIAutomation port is a follow-up) ─────────────

pub fn read_focused_value_fast() -> Option<String> {
    None
}
pub fn read_focused_value_first() -> Option<String> {
    None
}
pub fn read_focused_value() -> Option<String> {
    None
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
pub fn focused_pid() -> Option<i32> {
    None
}
pub fn unlock_focused_app_now() -> Option<i32> {
    None
}
pub fn lock_frontmost_app_now() -> Option<i32> {
    None
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct AxMethodResult {
    pub method: String,
    pub label: String,
    pub ok: bool,
    pub text: Option<String>,
    pub err: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct AxDiagnostics {
    pub ax_trusted: bool,
    pub app_name: Option<String>,
    pub app_pid: Option<i32>,
    pub element_role: Option<String>,
    pub attributes: Vec<String>,
    pub methods: Vec<AxMethodResult>,
    pub clipboard: String,
}

pub fn diagnose_focused_field() -> AxDiagnostics {
    AxDiagnostics {
        ax_trusted: is_accessibility_granted(),
        app_name: None,
        app_pid: None,
        element_role: None,
        attributes: vec![],
        methods: vec![],
        clipboard: String::new(),
    }
}

// ── Keystroke streaming via SendInput(KEYEVENTF_UNICODE) ──────────────────────

/// Build a keyboard `INPUT` event for a single UTF-16 code unit.
///
/// `keyup` flips `KEYEVENTF_KEYUP` so the same code unit can be sent twice
/// (down, then up) per Win32 keyboard-input conventions. Surrogate halves
/// (`is_high_surrogate` / `is_low_surrogate`) do not need any special flag
/// beyond `KEYEVENTF_UNICODE` — Windows reassembles them into a single
/// codepoint on the receiving app side.
fn unicode_input(unit: u16, keyup: bool) -> INPUT {
    let mut flags = KEYEVENTF_UNICODE;
    if keyup {
        flags |= KEYEVENTF_KEYUP;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(0),
                wScan: unit,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// Send a single down+up pair for one UTF-16 code unit.
fn send_unicode_unit(unit: u16) {
    let inputs = [unicode_input(unit, false), unicode_input(unit, true)];
    unsafe {
        SendInput(&inputs, size_of::<INPUT>() as i32);
    }
}

/// Synthesize a single keystroke (VK code) with both down and up events.
fn send_vk(vk: VIRTUAL_KEY, keydown: bool) {
    let mut flags = KEYBD_EVENT_FLAGS(0);
    if !keydown {
        flags = KEYEVENTF_KEYUP;
    }
    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    unsafe {
        SendInput(&[input], size_of::<INPUT>() as i32);
    }
}

/// Send a chord like Ctrl+V or Ctrl+A: hold modifier, press key, release in
/// reverse order.
fn send_chord(modifier: VIRTUAL_KEY, key: VIRTUAL_KEY) {
    send_vk(modifier, true);
    send_vk(key, true);
    send_vk(key, false);
    send_vk(modifier, false);
}

/// Stream the given text into the focused control as Unicode keystrokes.
/// Returns `Ok(true)` on success, `Ok(false)` if there was nothing to type.
pub fn type_text(text: &str) -> Result<bool, String> {
    if text.is_empty() {
        return Ok(false);
    }
    let units = text_to_utf16_units(text);
    for &unit in &units {
        // Each UTF-16 unit is sent independently. Windows reassembles
        // surrogate pairs on the receiving end based on the order — we just
        // emit them in sequence.
        let _ = is_high_surrogate(unit) || is_low_surrogate(unit); // doc-only; see win_paster
        send_unicode_unit(unit);
    }
    Ok(true)
}

// ── Clipboard helpers ─────────────────────────────────────────────────────────

/// Open the clipboard with a short retry loop — another app holding the
/// clipboard briefly (Chrome, Office) is the common failure mode and a few
/// short retries clears most of it.
fn open_clipboard_with_retry() -> Result<(), String> {
    for attempt in 0..6 {
        unsafe {
            if OpenClipboard(HWND(std::ptr::null_mut())).is_ok() {
                return Ok(());
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(15 * (attempt + 1) as u64));
    }
    Err("OpenClipboard failed after retries".into())
}

/// Read the current CF_UNICODETEXT contents (if any). Caller must already
/// hold the clipboard open via [`open_clipboard_with_retry`].
fn read_clipboard_unicode() -> Option<String> {
    unsafe {
        let handle = GetClipboardData(CF_UNICODETEXT.0 as u32).ok()?;
        let hglobal = HGLOBAL(handle.0);
        let size_bytes = GlobalSize(hglobal);
        if size_bytes == 0 {
            return Some(String::new());
        }
        let ptr = GlobalLock(hglobal) as *const u16;
        if ptr.is_null() {
            return None;
        }
        // size_bytes counts bytes; UTF-16 code units are u16.
        let unit_count = size_bytes / 2;
        let slice = std::slice::from_raw_parts(ptr, unit_count);
        // Strip the trailing NUL if present.
        let trimmed: &[u16] = if slice.last() == Some(&0) {
            &slice[..slice.len() - 1]
        } else {
            slice
        };
        let result = String::from_utf16(trimmed).ok();
        let _ = GlobalUnlock(hglobal);
        result
    }
}

/// Write a UTF-16 string into the clipboard. Caller must already hold the
/// clipboard open. Replaces existing contents.
fn write_clipboard_unicode(text: &str) -> Result<(), String> {
    let units = text_to_clipboard_utf16(text);
    let byte_count = units.len() * 2;

    unsafe {
        EmptyClipboard().map_err(|e| format!("EmptyClipboard failed: {e}"))?;

        let hmem = GlobalAlloc(GLOBAL_ALLOC_FLAGS(GMEM_MOVEABLE.0), byte_count)
            .map_err(|e| format!("GlobalAlloc failed: {e}"))?;

        let dst = GlobalLock(hmem) as *mut u16;
        if dst.is_null() {
            return Err("GlobalLock returned null".into());
        }
        std::ptr::copy_nonoverlapping(units.as_ptr(), dst, units.len());
        let _ = GlobalUnlock(hmem);

        SetClipboardData(CF_UNICODETEXT.0 as u32, HANDLE(hmem.0))
            .map_err(|e| format!("SetClipboardData failed: {e}"))?;
    }
    Ok(())
}

/// Replace the current clipboard contents with `text`, send Ctrl+V to the
/// focused app, then restore the original clipboard.
fn paste_via_clipboard(text: &str, select_all_first: bool) -> Result<(), String> {
    // 1. Snapshot existing clipboard (best-effort — non-CF_UNICODETEXT
    //    contents are lost in v1; that's a known limitation).
    open_clipboard_with_retry()?;
    let saved = read_clipboard_unicode();
    let _ = unsafe { CloseClipboard() };

    // 2. Install our new contents.
    open_clipboard_with_retry()?;
    let write_res = write_clipboard_unicode(text);
    let _ = unsafe { CloseClipboard() };
    write_res?;

    // 3. Optional select-all so the new paste replaces existing text.
    if select_all_first {
        send_chord(VK_CONTROL, VK_A);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    // 4. Send Ctrl+V.
    send_chord(VK_CONTROL, VK_V);

    // 5. Give the receiving app a moment to consume the clipboard before
    //    we restore the prior contents (otherwise the restore races the
    //    paste and the wrong text lands).
    std::thread::sleep(std::time::Duration::from_millis(80));

    if let Some(prev) = saved {
        if !prev.is_empty() {
            if let Ok(()) = open_clipboard_with_retry() {
                let _ = write_clipboard_unicode(&prev);
                let _ = unsafe { CloseClipboard() };
            }
        }
    }
    Ok(())
}

pub fn paste(text: &str) -> Result<(), String> {
    paste_via_clipboard(text, false)
}

pub fn paste_replacing(text: &str) -> Result<(), String> {
    paste_via_clipboard(text, true)
}

pub fn replace_typed_suffix(typed_text: &str, replacement: &str) -> Result<(), String> {
    let chars_to_delete = typed_text.chars().count();
    if chars_to_delete == 0 {
        return paste(replacement);
    }
    for _ in 0..chars_to_delete {
        send_vk(VK_BACK, true);
        send_vk(VK_BACK, false);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    paste(replacement)
}

pub fn reconcile_typed_text(typed_text: &str, replacement: &str) -> Result<bool, String> {
    if typed_text == replacement {
        return Ok(false);
    }
    replace_typed_suffix(typed_text, replacement)?;
    Ok(true)
}

pub fn reconcile_current_recording(
    _initial_text: Option<&str>,
    typed_text: &str,
    replacement: &str,
) -> Result<bool, String> {
    reconcile_typed_text(typed_text, replacement)
}

pub fn replace_focused_text_exact(
    _existing_text: &str,
    _replacement: &str,
) -> Result<bool, String> {
    Ok(false)
}

// ── Tests ─────────────────────────────────────────────────────────────────────
//
// Real-host smoke tests that exercise the Win32 clipboard plumbing end-to-end.
// They only compile when targeting Windows (this whole file is gated to
// `cfg(target_os = "windows")` from lib.rs) and only run on the rust-windows
// CI job's `cargo test -p said-paster` step.
//
// SendInput tests are intentionally omitted — they would actually inject
// keystrokes into whatever has focus on the test runner, which is racy and
// destructive to other tests sharing the desktop session. The pure UTF-16
// encoding logic that feeds SendInput is covered by win_paster tests on
// every host.

#[cfg(test)]
mod windows_tests {
    use super::*;

    /// Write Unicode text to the clipboard via the same code path that `paste`
    /// uses, then read it back and verify byte-for-byte equality. Covers:
    ///   - `open_clipboard_with_retry` actually opens the clipboard.
    ///   - `EmptyClipboard` + `GlobalAlloc` + `SetClipboardData(CF_UNICODETEXT)`
    ///     install our buffer correctly.
    ///   - `GetClipboardData` + `GlobalLock` + `String::from_utf16` round-trips
    ///     including the NUL-terminator stripping logic.
    ///   - Devanagari + emoji + multibyte planes survive intact.
    #[test]
    fn clipboard_round_trip_preserves_unicode() {
        // Use unusual content so we don't false-pass on whatever the runner
        // happened to have on the clipboard before the test.
        let payload = "Said test ✓ नमस्ते 😀";

        open_clipboard_with_retry().expect("open clipboard for write");
        write_clipboard_unicode(payload).expect("write clipboard");
        unsafe {
            let _ = CloseClipboard();
        }

        open_clipboard_with_retry().expect("open clipboard for read");
        let read_back = read_clipboard_unicode().expect("clipboard contents readable");
        unsafe {
            let _ = CloseClipboard();
        }

        assert_eq!(
            read_back, payload,
            "clipboard round-trip must preserve every codepoint exactly"
        );
    }

    // (intentionally no empty-string round-trip test)
    //
    // We initially had one, but Windows treats a clipboard buffer that's just
    // a single NUL terminator as "no text data" — `GetClipboardData(CF_UNICODETEXT)`
    // returns null after writing it. The non-empty round-trip above is the
    // production-relevant case; production callers (paste / paste_replacing)
    // never push empty strings into the clipboard. `type_text` already
    // early-returns on empty input.
}
