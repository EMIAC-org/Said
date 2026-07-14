//! Windows paster: Unicode keystroke streaming via `SendInput`, plus
//! clipboard backup/set/restore for the paste path.
//!
//! Public API mirrors the macOS `imp` module so the desktop and CLI consumers
//! compile unchanged. UIAutomation provides focused-field reads and exact-range
//! selection for edit watching and repair/refine replacement.

use std::mem::size_of;

use windows::Win32::Foundation::{HANDLE, HGLOBAL, HWND};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData, OpenClipboard,
    SetClipboardData,
};
use windows::Win32::System::Memory::{
    GLOBAL_ALLOC_FLAGS, GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP,
    KEYEVENTF_UNICODE, SendInput, VIRTUAL_KEY, VK_A, VK_BACK, VK_C, VK_CONTROL, VK_V,
};

use crate::win_paster::{exact_match_needles, text_to_clipboard_utf16, text_to_utf16_units};

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

// ── Focused-field reads (UIAutomation, via the dedicated worker in `uia`) ──────
//
// Timeouts mirror the macOS contract: the 30ms poll loop uses the `fast` path
// (~80ms budget, value/text patterns only); the one-shot/full reads use a larger
// budget and add the bounded subtree walk for Chromium/Electron. All sit inside
// the watcher's 500ms `blocking_ax_option`, so a wedged provider just drops a tick.

pub fn read_focused_value_fast() -> Option<String> {
    crate::uia::value(true, None, 80)
}
pub fn read_focused_value_first() -> Option<String> {
    crate::uia::value(false, None, 450)
}
pub fn read_focused_value() -> Option<String> {
    crate::uia::value(false, None, 450)
}
pub fn read_focused_value_fast_for_pid(pid: i32) -> Option<String> {
    crate::uia::value(true, Some(pid), 80)
}
pub fn read_focused_value_first_for_pid(pid: i32) -> Option<String> {
    crate::uia::value(false, Some(pid), 450)
}

/// Last-resort full-field read for a11y-blind controls: select-all + copy, read
/// the clipboard, then restore it. Destructive to selection + clipboard, so only
/// used when UIA yields nothing. Not run on password fields (UIA reads already
/// return None there, and the OS blocks copying password text).
pub fn capture_focused_text_via_selection() -> Option<String> {
    open_clipboard_with_retry().ok()?;
    let saved = read_clipboard_snapshot();
    let _ = unsafe { CloseClipboard() };

    send_chord(VK_CONTROL, VK_A);
    std::thread::sleep(std::time::Duration::from_millis(50));
    send_chord(VK_CONTROL, VK_C);
    std::thread::sleep(std::time::Duration::from_millis(150));

    let captured = if open_clipboard_with_retry().is_ok() {
        let captured = read_clipboard_unicode();
        let _ = unsafe { CloseClipboard() };
        captured
    } else {
        None
    };

    restore_clipboard(saved);
    captured.filter(|s| !s.trim().is_empty())
}

/// Read only the selected text. UIA `TextPattern::GetSelection` first; if that's
/// unavailable, fall back to a copy-only clipboard read (guarded so an empty copy
/// doesn't return the prior clipboard contents).
pub fn read_selected_text() -> Option<String> {
    if let Some(sel) = crate::uia::selection(300) {
        return Some(sel);
    }
    copy_selection_read()
}

pub fn focused_pid() -> Option<i32> {
    crate::uia::focused_pid()
}
pub fn unlock_focused_app_now() -> Option<i32> {
    crate::uia::activate_foreground()
}
pub fn lock_frontmost_app_now() -> Option<i32> {
    crate::uia::activate_foreground()
}

/// Copy the current selection (no select-all) and read it back. Returns None if
/// the copy produced nothing new (i.e. there was no selection), so we never
/// mistake leftover clipboard contents for selected text.
fn copy_selection_read() -> Option<String> {
    open_clipboard_with_retry().ok()?;
    let saved = read_clipboard_snapshot();
    let saved_text = saved.unicode_text();
    let _ = unsafe { CloseClipboard() };

    send_chord(VK_CONTROL, VK_C);
    std::thread::sleep(std::time::Duration::from_millis(120));

    let captured = if open_clipboard_with_retry().is_ok() {
        let captured = read_clipboard_unicode();
        let _ = unsafe { CloseClipboard() };
        captured
    } else {
        None
    };

    let result = match (&captured, &saved_text) {
        (Some(c), Some(s)) if c == s => None, // copy changed nothing → no selection
        (Some(c), _) if !c.trim().is_empty() => Some(c.clone()),
        _ => None,
    };
    restore_clipboard(saved);
    result
}

/// Restore previously-saved clipboard contents (best-effort).
fn restore_clipboard(saved: ClipboardSnapshot) {
    if saved.is_empty() {
        return;
    }
    if open_clipboard_with_retry().is_ok() {
        let _ = restore_open_clipboard_snapshot(&saved);
        let _ = unsafe { CloseClipboard() };
    }
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
    let app_pid = crate::uia::focused_pid();
    let (app_name, element_role) = match crate::uia::info(500) {
        Some((name, role)) => (
            Some(name).filter(|s| !s.is_empty()),
            Some(role).filter(|s| !s.is_empty()),
        ),
        None => (None, None),
    };

    let mut methods = Vec::new();
    let value = crate::uia::value(false, None, 500);
    methods.push(AxMethodResult {
        method: "value".into(),
        label: "Value / Text pattern".into(),
        ok: value.is_some(),
        text: value,
        err: None,
    });
    let selection = crate::uia::selection(500);
    methods.push(AxMethodResult {
        method: "selection".into(),
        label: "Selected text".into(),
        ok: selection.is_some(),
        text: selection,
        err: None,
    });

    let clipboard = if open_clipboard_with_retry().is_ok() {
        let c = read_clipboard_unicode().unwrap_or_default();
        let _ = unsafe { CloseClipboard() };
        c
    } else {
        String::new()
    };

    AxDiagnostics {
        ax_trusted: is_accessibility_granted(),
        app_name,
        app_pid,
        element_role,
        attributes: vec![],
        methods,
        clipboard,
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

    // Build every down+up event up front and inject them as atomic batches via a
    // few SendInput calls. The previous one-SendInput-per-character loop
    // interleaved with real user input and dropped/reordered characters in fast
    // or busy apps — which is precisely why long inserts used to fall back to a
    // clipboard paste (and hit the clipboard-restore race). Batched injection is
    // reliable, so direct typing is the default and the clipboard stays out of
    // the common path. Surrogate pairs need no special flag — Windows reassembles
    // them from the ordered KEYEVENTF_UNICODE events.
    let units = text_to_utf16_units(text);
    let mut inputs: Vec<INPUT> = Vec::with_capacity(units.len() * 2);
    for &unit in &units {
        inputs.push(unicode_input(unit, false));
        inputs.push(unicode_input(unit, true));
    }

    // SendInput is atomic per call relative to other input; cap the per-call size
    // so a very long dictation still injects in a bounded number of syscalls.
    const MAX_EVENTS_PER_CALL: usize = 512;
    for batch in inputs.chunks(MAX_EVENTS_PER_CALL) {
        let injected = unsafe { SendInput(batch, size_of::<INPUT>() as i32) };
        if injected as usize != batch.len() {
            // SendInput refuses injection into a higher-integrity (elevated)
            // target window. Report failure so the caller falls back to a
            // clipboard paste rather than silently dropping the text.
            return Err(format!(
                "SendInput injected {injected}/{} events (blocked by target?)",
                batch.len()
            ));
        }
    }
    Ok(true)
}

// ── Clipboard helpers ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
struct ClipboardSnapshot {
    formats: Vec<ClipboardFormatSnapshot>,
}

impl ClipboardSnapshot {
    fn is_empty(&self) -> bool {
        self.formats.is_empty()
    }

    fn unicode_text(&self) -> Option<String> {
        self.formats
            .iter()
            .find(|entry| entry.format == CF_UNICODETEXT.0 as u32)
            .and_then(|entry| decode_clipboard_unicode_bytes(&entry.bytes))
    }
}

#[derive(Debug, Clone)]
struct ClipboardFormatSnapshot {
    format: u32,
    bytes: Vec<u8>,
}

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

/// Snapshot all clipboard formats whose payloads are backed by movable global
/// memory. This preserves rich text, HTML, file drops and DIB images in common
/// apps. Handle-backed formats such as CF_BITMAP cannot be cloned safely here
/// and are skipped instead of poisoning the restore.
fn read_clipboard_snapshot() -> ClipboardSnapshot {
    let mut formats = Vec::new();
    let mut current = 0u32;

    loop {
        let next = unsafe { EnumClipboardFormats(current) };
        if next == 0 {
            break;
        }
        current = next;

        if let Some(bytes) = read_clipboard_format_bytes(current) {
            formats.push(ClipboardFormatSnapshot {
                format: current,
                bytes,
            });
        }
    }

    ClipboardSnapshot { formats }
}

fn read_clipboard_format_bytes(format: u32) -> Option<Vec<u8>> {
    unsafe {
        let handle = GetClipboardData(format).ok()?;
        if handle.0.is_null() {
            return None;
        }
        let hglobal = HGLOBAL(handle.0);
        let size_bytes = GlobalSize(hglobal);
        if size_bytes == 0 {
            return Some(Vec::new());
        }
        let ptr = GlobalLock(hglobal) as *const u8;
        if ptr.is_null() {
            return None;
        }
        let bytes = std::slice::from_raw_parts(ptr, size_bytes).to_vec();
        let _ = GlobalUnlock(hglobal);
        Some(bytes)
    }
}

fn restore_open_clipboard_snapshot(snapshot: &ClipboardSnapshot) -> Result<(), String> {
    unsafe {
        EmptyClipboard().map_err(|e| format!("EmptyClipboard failed: {e}"))?;
    }

    for entry in &snapshot.formats {
        if entry.bytes.is_empty() {
            continue;
        }
        if let Err(err) = write_clipboard_format_bytes(entry.format, &entry.bytes) {
            tracing::debug!(
                format = entry.format,
                error = %err,
                "skipping clipboard format restore"
            );
        }
    }

    Ok(())
}

fn clipboard_unicode_bytes(text: &str) -> Vec<u8> {
    let units = text_to_clipboard_utf16(text);
    let mut bytes = Vec::with_capacity(units.len() * 2);
    for unit in units {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    bytes
}

fn decode_clipboard_unicode_bytes(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() {
        return Some(String::new());
    }
    if bytes.len() % 2 != 0 {
        return None;
    }

    let mut units = Vec::with_capacity(bytes.len() / 2);
    for chunk in bytes.chunks_exact(2) {
        units.push(u16::from_le_bytes([chunk[0], chunk[1]]));
    }

    if units.last() == Some(&0) {
        units.pop();
    }
    String::from_utf16(&units).ok()
}

fn write_clipboard_format_bytes(format: u32, bytes: &[u8]) -> Result<(), String> {
    if bytes.is_empty() {
        return Ok(());
    }

    unsafe {
        let hmem = GlobalAlloc(GLOBAL_ALLOC_FLAGS(GMEM_MOVEABLE.0), bytes.len())
            .map_err(|e| format!("GlobalAlloc failed: {e}"))?;

        let dst = GlobalLock(hmem) as *mut u8;
        if dst.is_null() {
            return Err("GlobalLock returned null".into());
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), dst, bytes.len());
        let _ = GlobalUnlock(hmem);

        if let Err(err) = SetClipboardData(format, HANDLE(hmem.0)) {
            return Err(format!("SetClipboardData failed: {err}"));
        }
    }

    Ok(())
}

/// Read the current CF_UNICODETEXT contents (if any). Caller must already
/// hold the clipboard open via [`open_clipboard_with_retry`].
fn read_clipboard_unicode() -> Option<String> {
    let bytes = read_clipboard_format_bytes(CF_UNICODETEXT.0 as u32)?;
    decode_clipboard_unicode_bytes(&bytes)
}

/// Write a UTF-16 string into the clipboard. Caller must already hold the
/// clipboard open. Replaces existing contents.
fn write_clipboard_unicode(text: &str) -> Result<(), String> {
    unsafe {
        EmptyClipboard().map_err(|e| format!("EmptyClipboard failed: {e}"))?;
    }
    let bytes = clipboard_unicode_bytes(text);
    write_clipboard_format_bytes(CF_UNICODETEXT.0 as u32, &bytes)
}

/// Insert `text` at the caret preferring direct keystroke injection, using a
/// clipboard paste only when injection is unavailable/blocked. Mirrors the macOS
/// `type_or_paste_at_cursor` so the reconcile/replace paths never reach for the
/// clipboard on the common path.
fn type_or_paste_at_cursor(text: &str) -> Result<(), String> {
    if text.is_empty() {
        return Ok(());
    }
    match type_text(text) {
        Ok(true) => Ok(()),
        Ok(false) => paste_via_clipboard(text, false),
        Err(e) => {
            tracing::warn!("[paster] direct typing failed ({e}); clipboard fallback");
            paste_via_clipboard(text, false)
        }
    }
}

/// Block until a clipboard paste has been consumed by the focused app, so the
/// caller can safely restore the user's previous clipboard.
///
/// Instead of a fixed sleep — which races a slow/busy target and lets the restore
/// win, so the app pastes the *previous* clipboard contents — poll the focused
/// field via UIAutomation until it reflects `pasted`, bounded by a hard timeout.
/// When the field can't be read (password fields, a11y-blind controls) fall back
/// to a short conservative delay.
fn wait_until_paste_consumed(pasted: &str) {
    use std::time::{Duration, Instant};
    const FLOOR_MS: u64 = 40; // let the synthetic Ctrl+V reach the queue first
    const POLL_MS: u64 = 25;
    const MAX_WAIT_MS: u64 = 800; // hard ceiling so we never hang the caller
    const BLIND_MS: u64 = 300; // used when the field isn't UIA-readable

    std::thread::sleep(Duration::from_millis(FLOOR_MS));
    let start = Instant::now();
    loop {
        match crate::uia::value(true, None, 60) {
            Some(v) if v.contains(pasted) => return, // confirmed landed
            Some(_) => {}                            // readable, not there yet
            None => {
                if start.elapsed() >= Duration::from_millis(BLIND_MS) {
                    return;
                }
            }
        }
        if start.elapsed() >= Duration::from_millis(MAX_WAIT_MS) {
            return;
        }
        std::thread::sleep(Duration::from_millis(POLL_MS));
    }
}

/// Replace the current clipboard contents with `text`, send Ctrl+V to the
/// focused app, then restore the original clipboard.
fn paste_via_clipboard(text: &str, select_all_first: bool) -> Result<(), String> {
    // 1. Snapshot existing clipboard. Restoring all lockable formats protects
    //    rich text/images/file drops while still allowing AirNote to paste text.
    open_clipboard_with_retry()?;
    let saved = read_clipboard_snapshot();
    let _ = unsafe { CloseClipboard() };

    // 2. Install our new contents.
    open_clipboard_with_retry()?;
    let write_res = write_clipboard_unicode(text);
    let _ = unsafe { CloseClipboard() };
    if let Err(err) = write_res {
        restore_clipboard(saved);
        return Err(err);
    }

    // 3. Optional select-all so the new paste replaces existing text.
    if select_all_first {
        send_chord(VK_CONTROL, VK_A);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    // 4. Send Ctrl+V.
    send_chord(VK_CONTROL, VK_V);

    // 5. Wait until the paste has actually landed in the focused field before
    //    restoring the prior clipboard — confirmed via UIAutomation rather than a
    //    fixed sleep. The old 80ms guess is exactly what let a slow/busy target
    //    read the *restored* (old) clipboard and paste the wrong text.
    wait_until_paste_consumed(text);

    restore_clipboard(saved);
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
        return type_or_paste_at_cursor(replacement);
    }
    for _ in 0..chars_to_delete {
        send_vk(VK_BACK, true);
        send_vk(VK_BACK, false);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    type_or_paste_at_cursor(replacement)
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

fn delete_selection() {
    send_vk(VK_BACK, true);
    send_vk(VK_BACK, false);
    std::thread::sleep(std::time::Duration::from_millis(20));
}

fn replace_selected_text(replacement: &str) -> Result<(), String> {
    if replacement.is_empty() {
        delete_selection();
        Ok(())
    } else {
        type_or_paste_at_cursor(replacement)
    }
}

pub fn replace_focused_text_exact(existing_text: &str, replacement: &str) -> Result<bool, String> {
    if existing_text == replacement {
        return Ok(false);
    }
    if existing_text.is_empty() {
        return Ok(false);
    }

    for needle in exact_match_needles(existing_text) {
        if crate::uia::select_exact_text(&needle, 700) {
            replace_selected_text(replacement)?;
            return Ok(true);
        }
    }

    // ValuePattern-only controls may not support TextPattern ranges. In that
    // case, select-all is safe only when the whole focused field is exactly the
    // previous output we intend to replace.
    let current = read_focused_value();
    if let Some(current_text) = current.as_deref() {
        if exact_match_needles(existing_text)
            .iter()
            .any(|needle| needle == current_text)
        {
            if replacement.is_empty() {
                send_chord(VK_CONTROL, VK_A);
                std::thread::sleep(std::time::Duration::from_millis(20));
                delete_selection();
            } else {
                paste_via_clipboard(replacement, true)?;
            }
            return Ok(true);
        }
    }

    Ok(false)
}

// ── Tests ─────────────────────────────────────────────────────────────────────
//
// Real-host smoke tests that exercise the Win32 clipboard plumbing end-to-end.
// They only compile when targeting Windows (this whole file is gated to
// `cfg(target_os = "windows")` from lib.rs) and are opt-in because headless
// CI/window-station clipboard behavior is not stable enough for safe default
// execution.
//
// SendInput tests are intentionally omitted — they would actually inject
// keystrokes into whatever has focus on the test runner, which is racy and
// destructive to other tests sharing the desktop session. The pure UTF-16
// encoding logic that feeds SendInput is covered by win_paster tests on
// every host.

#[cfg(test)]
mod windows_tests {
    use super::*;
    use windows::Win32::System::DataExchange::RegisterClipboardFormatW;
    use windows::core::PCWSTR;

    fn clipboard_smoke_tests_enabled() -> bool {
        std::env::var_os("AIRNOTE_RUN_WINDOWS_CLIPBOARD_TESTS").is_some()
    }

    /// Headless CI runners (no interactive window-station) frequently have a
    /// clipboard that accepts opens/writes but returns nothing on read, so a
    /// real round-trip can't be exercised there. Probe a tiny round-trip; if it
    /// doesn't survive, the runner can't run these tests meaningfully, so they
    /// skip gracefully instead of failing the build. A real clipboard passes the
    /// probe and the full assertions still run.
    fn clipboard_round_trips() -> bool {
        if !clipboard_smoke_tests_enabled() {
            return false;
        }
        let probe = "airnote-clipboard-probe";
        if open_clipboard_with_retry().is_err() {
            return false;
        }
        let wrote = write_clipboard_unicode(probe);
        unsafe {
            let _ = CloseClipboard();
        }
        if wrote.is_err() {
            return false;
        }
        if open_clipboard_with_retry().is_err() {
            return false;
        }
        let read = read_clipboard_unicode();
        unsafe {
            let _ = CloseClipboard();
        }
        matches!(read, Some(ref s) if s.as_str() == probe)
    }

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
        if !clipboard_round_trips() {
            eprintln!(
                "skipping clipboard_round_trip_preserves_unicode: set AIRNOTE_RUN_WINDOWS_CLIPBOARD_TESTS=1 on an interactive Windows host"
            );
            return;
        }
        // Use unusual content so we don't false-pass on whatever the runner
        // happened to have on the clipboard before the test.
        let payload = "AirNote test ✓ नमस्ते 😀";

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

    #[test]
    fn clipboard_snapshot_restores_registered_non_text_format() {
        if !clipboard_round_trips() {
            eprintln!(
                "skipping clipboard_snapshot_restores_registered_non_text_format: set AIRNOTE_RUN_WINDOWS_CLIPBOARD_TESTS=1 on an interactive Windows host"
            );
            return;
        }
        let format_name: Vec<u16> = "AirNoteSnapshotTestFormat\0".encode_utf16().collect();
        let custom_format = unsafe { RegisterClipboardFormatW(PCWSTR(format_name.as_ptr())) };
        assert_ne!(custom_format, 0, "custom clipboard format must register");

        let custom_payload = b"airnote-rich-payload\x00\x01\x02".to_vec();

        open_clipboard_with_retry().expect("open clipboard for rich setup");
        unsafe {
            EmptyClipboard().expect("empty clipboard");
        }
        let unicode_bytes = clipboard_unicode_bytes("original text");
        write_clipboard_format_bytes(CF_UNICODETEXT.0 as u32, &unicode_bytes)
            .expect("write unicode format");
        write_clipboard_format_bytes(custom_format, &custom_payload).expect("write custom format");
        unsafe {
            let _ = CloseClipboard();
        }

        open_clipboard_with_retry().expect("open clipboard for snapshot");
        let snapshot = read_clipboard_snapshot();
        unsafe {
            let _ = CloseClipboard();
        }
        assert_eq!(snapshot.unicode_text().as_deref(), Some("original text"));
        assert!(
            snapshot
                .formats
                .iter()
                .any(|entry| entry.format == custom_format),
            "snapshot must include registered rich format"
        );

        open_clipboard_with_retry().expect("open clipboard for overwrite");
        write_clipboard_unicode("temporary text").expect("overwrite unicode");
        unsafe {
            let _ = CloseClipboard();
        }

        restore_clipboard(snapshot);

        open_clipboard_with_retry().expect("open clipboard for restored read");
        let restored_text = read_clipboard_unicode().expect("unicode restored");
        let restored_custom =
            read_clipboard_format_bytes(custom_format).expect("custom rich format restored");
        unsafe {
            let _ = CloseClipboard();
        }

        assert_eq!(restored_text, "original text");
        assert_eq!(restored_custom, custom_payload);
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
