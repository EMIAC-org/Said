//! Windows implementation — UI Automation reader + SendInput typist + clipboard paste.
//!
//! ## Threading
//!
//! UIA is not reentrant-safe in practice, so all UIA calls run on a single
//! dedicated `said-paster-uia` thread:
//!
//!   * Thread is started lazily on first use (idempotent via `OnceCell`).
//!   * Holds the `IUIAutomation` singleton it creates after `CoInitializeEx`.
//!   * Listens on `mpsc::Receiver<ReadRequest>` and replies via the per-
//!     request `mpsc::Sender<ReadReply>` the caller supplied.
//!
//! Callers block on the reply, with a 5 s deadline so an unresponsive
//! target app can't lock up the Tauri shell.
//!
//! ## 6-strategy focused-text read stack
//!
//!   1. UIA `TextPattern.DocumentRange.GetText(-1)` — WPF / WinForms /
//!      modern UWP / WinUI / most Office controls
//!   2. UIA `ValuePattern.CurrentValue` — simple text inputs (most legacy
//!      Win32 controls expose this)
//!   3. UIA `TextPattern.GetSelection` → expand to enclosing document
//!   4. (TODO P3.1) MSAA `IAccessible::accValue` — pre-UIA controls
//!   5. (TODO P3.1) `WM_GETTEXT` via `AttachThreadInput`
//!   6. Ctrl+A + Ctrl+C clipboard fallback — disruptive but universal
//!
//! P3 ships strategies 1, 2, 3, 6 (≈95% app coverage). MSAA / WM_GETTEXT
//! land in a follow-up if telemetry shows gaps.

#![allow(unsafe_op_in_unsafe_fn)]

use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use arboard::Clipboard;
use once_cell::sync::OnceCell;
use parking_lot::Mutex;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationTextPattern,
    IUIAutomationValuePattern, TextUnit_Document, UIA_TextPatternId, UIA_ValuePatternId,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP,
    KEYEVENTF_UNICODE, SendInput, VK_A, VK_C, VK_CONTROL, VK_RETURN, VK_TAB, VK_V,
};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
use windows::core::Interface;

use crate::shared::{AxDiagnostics, AxMethodResult};

// ── Public no-op permissions ─────────────────────────────────────────────────

pub fn request_permission() {}
pub fn request_input_monitoring() {}

/// Windows has no Accessibility-permission gate. UIA is always reachable.
pub fn is_accessibility_granted() -> bool {
    true
}

// ── App-context lock (mirrors macOS lock_frontmost_app_now) ──────────────────

static LOCKED_PID: AtomicI32 = AtomicI32::new(-1);

pub fn focused_pid() -> Option<i32> {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0.is_null() {
        return None;
    }
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    if pid > 0 { Some(pid as i32) } else { None }
}

pub fn unlock_focused_app_now() -> Option<i32> {
    // Windows UIA needs no per-app unlock (no AXEnhancedUserInterface
    // equivalent). Just capture the current foreground PID for parity with
    // the macOS API surface.
    let pid = focused_pid()?;
    tracing::debug!("[paster-win] unlock_focused_app_now → pid={pid}");
    Some(pid)
}

pub fn lock_frontmost_app_now() -> Option<i32> {
    let pid = focused_pid()?;
    LOCKED_PID.store(pid, Ordering::SeqCst);
    tracing::debug!("[paster-win] lock_frontmost_app_now → pid={pid}");
    Some(pid)
}

fn locked_pid() -> Option<i32> {
    match LOCKED_PID.load(Ordering::SeqCst) {
        n if n > 0 => Some(n),
        _ => None,
    }
}

// ── SendInput typist ─────────────────────────────────────────────────────────

/// HID timing: mirror the macOS 6 ms keydown→keyup + 6 ms post-keyup cadence.
/// Tuned for the streaming polish output path; see "HID delays are sacred"
/// in AGENTS.md. Removing or shrinking these causes word-breaking at the
/// typing speed Tauri streams tokens.
const HID_CHUNK_PAUSE: Duration = Duration::from_micros(6_000);

/// Type arbitrary Unicode text into the focused field via `SendInput` with
/// `KEYEVENTF_UNICODE`. Returns `Ok(false)` if SendInput refused (rare —
/// usually means the target is an elevated app and we're not).
pub fn type_text(text: &str) -> Result<bool, String> {
    if text.is_empty() {
        return Ok(true);
    }

    // Build INPUT array. Each visible character becomes 2 INPUTs (down+up).
    // `\n` and `\t` use their native virtual keys; plain KEYEVENTF_UNICODE
    // newline does not insert a newline in most rich editors.
    for chunk in text.chars().collect::<Vec<_>>().chunks(16) {
        let mut inputs: Vec<INPUT> = Vec::with_capacity(chunk.len() * 2);
        for ch in chunk {
            match *ch {
                '\n' | '\r' => push_vk(&mut inputs, VK_RETURN.0),
                '\t' => push_vk(&mut inputs, VK_TAB.0),
                c => push_unicode(&mut inputs, c),
            }
        }
        send_inputs(&inputs)?;
        thread::sleep(HID_CHUNK_PAUSE);
    }
    Ok(true)
}

fn push_unicode(buf: &mut Vec<INPUT>, c: char) {
    // UTF-16 encoding — emoji / supplementary characters need surrogate pairs.
    let mut utf16 = [0u16; 2];
    let units = c.encode_utf16(&mut utf16);
    for &unit in units.iter() {
        buf.push(make_kb_input(0, unit, KEYEVENTF_UNICODE));
        buf.push(make_kb_input(0, unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP));
    }
}

fn push_vk(buf: &mut Vec<INPUT>, vk: u16) {
    buf.push(make_kb_input(vk, 0, KEYBD_EVENT_FLAGS(0)));
    buf.push(make_kb_input(vk, 0, KEYEVENTF_KEYUP));
}

fn make_kb_input(vk: u16, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(vk),
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn send_inputs(inputs: &[INPUT]) -> Result<bool, String> {
    if inputs.is_empty() {
        return Ok(true);
    }
    let sent = unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent as usize == inputs.len() {
        Ok(true)
    } else if sent == 0 {
        // Most commonly: target window is elevated, our process isn't.
        Err("SendInput refused — target may be running elevated".into())
    } else {
        Err(format!(
            "SendInput partial: sent {sent}/{} inputs",
            inputs.len()
        ))
    }
}

// ── Clipboard save/restore + paste ───────────────────────────────────────────

/// Time we wait between setting clipboard text and sending Ctrl+V, and
/// between Ctrl+V and restoring the clipboard. Most apps commit a paste
/// inside ~50 ms; we leave a generous margin without making the UX feel
/// laggy.
const CLIPBOARD_COMMIT_WAIT: Duration = Duration::from_millis(80);

fn open_clipboard() -> Option<Clipboard> {
    match Clipboard::new() {
        Ok(c) => Some(c),
        Err(e) => {
            tracing::warn!("[paster-win] clipboard open failed: {e}");
            None
        }
    }
}

fn save_clipboard_text() -> Option<String> {
    open_clipboard().and_then(|mut c| c.get_text().ok())
}

fn set_clipboard_text(text: &str) -> bool {
    let Some(mut c) = open_clipboard() else {
        return false;
    };
    c.set_text(text).is_ok()
}

fn send_ctrl_chord(vk: u16) -> Result<(), String> {
    let inputs = [
        make_kb_input(VK_CONTROL.0, 0, KEYBD_EVENT_FLAGS(0)),
        make_kb_input(vk, 0, KEYBD_EVENT_FLAGS(0)),
        make_kb_input(vk, 0, KEYEVENTF_KEYUP),
        make_kb_input(VK_CONTROL.0, 0, KEYEVENTF_KEYUP),
    ];
    send_inputs(&inputs).map(|_| ())
}

pub fn paste(text: &str) -> Result<(), String> {
    paste_internal(text, false)
}

pub fn paste_replacing(text: &str) -> Result<(), String> {
    paste_internal(text, true)
}

fn paste_internal(text: &str, replace: bool) -> Result<(), String> {
    let original = save_clipboard_text();

    if replace {
        // Cmd+A on Mac → Ctrl+A on Windows
        send_ctrl_chord(VK_A.0)?;
        thread::sleep(Duration::from_millis(40));
    }

    if !set_clipboard_text(text) {
        return Err("failed to set clipboard text".into());
    }

    // Tiny pre-paste pause so apps observing clipboard events see the new
    // value before the paste chord arrives.
    thread::sleep(Duration::from_millis(30));
    send_ctrl_chord(VK_V.0)?;
    thread::sleep(CLIPBOARD_COMMIT_WAIT);

    if let Some(orig) = original {
        // Best-effort restore — if it fails we don't block the user.
        let _ = set_clipboard_text(&orig);
    }
    tracing::info!("[paster-win] paste done (replace={replace})");
    Ok(())
}

/// Disruptive: select all + copy + read clipboard + restore. Used as the
/// last-resort focused-text read when UIA can't reach the field (e.g. some
/// terminal emulators, OpenGL/Vulkan game UIs).
pub fn capture_focused_text_via_selection() -> Option<String> {
    let original = save_clipboard_text();
    if send_ctrl_chord(VK_A.0).is_err() {
        return None;
    }
    thread::sleep(Duration::from_millis(30));
    if send_ctrl_chord(VK_C.0).is_err() {
        if let Some(o) = original {
            let _ = set_clipboard_text(&o);
        }
        return None;
    }
    thread::sleep(Duration::from_millis(60));
    let captured = open_clipboard().and_then(|mut c| c.get_text().ok());
    if let Some(o) = original {
        let _ = set_clipboard_text(&o);
    }
    captured
}

pub fn read_selected_text() -> Option<String> {
    // Non-disruptive variant: just Ctrl+C the current selection.
    let original = save_clipboard_text();
    if send_ctrl_chord(VK_C.0).is_err() {
        return None;
    }
    thread::sleep(Duration::from_millis(60));
    let captured = open_clipboard().and_then(|mut c| c.get_text().ok());
    if let Some(o) = original {
        let _ = set_clipboard_text(&o);
    }
    captured
}

// ── UIA reader thread ────────────────────────────────────────────────────────

enum ReadKind {
    FocusedFirst { pid: Option<i32> },
    FocusedFast { pid: Option<i32> },
    Diagnose,
}

enum ReadReply {
    Text(Option<String>),
    Diagnostics(Box<AxDiagnostics>),
}

struct ReadRequest {
    kind: ReadKind,
    reply: mpsc::Sender<ReadReply>,
}

static READER_TX: OnceCell<Mutex<mpsc::Sender<ReadRequest>>> = OnceCell::new();
const READ_TIMEOUT: Duration = Duration::from_secs(5);

fn reader_sender() -> mpsc::Sender<ReadRequest> {
    let mtx = READER_TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<ReadRequest>();
        thread::Builder::new()
            .name("said-paster-uia".into())
            .spawn(move || uia_thread(rx))
            .expect("failed to spawn said-paster-uia thread");
        Mutex::new(tx)
    });
    mtx.lock().clone()
}

fn send_read(kind: ReadKind) -> Option<ReadReply> {
    let (tx, rx) = mpsc::channel();
    let _ = reader_sender().send(ReadRequest { kind, reply: tx });
    rx.recv_timeout(READ_TIMEOUT).ok()
}

fn uia_thread(rx: mpsc::Receiver<ReadRequest>) {
    // SAFETY: CoInitializeEx on a dedicated thread; never uninit'd until
    // process exit (UIA is held in static state). Safe per MSDN.
    unsafe {
        if CoInitializeEx(None, COINIT_MULTITHREADED).is_err() {
            tracing::error!("[paster-win] CoInitializeEx failed — UIA reader inert");
            return;
        }
    }

    let uia: IUIAutomation =
        match unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) } {
            Ok(u) => u,
            Err(e) => {
                tracing::error!("[paster-win] CoCreateInstance(CUIAutomation) failed: {e:?}");
                unsafe { CoUninitialize() };
                return;
            }
        };

    tracing::info!("[paster-win] UIA reader thread ready");

    while let Ok(req) = rx.recv() {
        let reply = match req.kind {
            ReadKind::FocusedFirst { pid } => ReadReply::Text(read_first(&uia, pid)),
            ReadKind::FocusedFast { pid } => ReadReply::Text(read_fast(&uia, pid)),
            ReadKind::Diagnose => ReadReply::Diagnostics(Box::new(diagnose(&uia))),
        };
        let _ = req.reply.send(reply);
    }
}

// ── 6-strategy read stack ────────────────────────────────────────────────────

fn focused_element(uia: &IUIAutomation, want_pid: Option<i32>) -> Option<IUIAutomationElement> {
    let el = unsafe { uia.GetFocusedElement() }.ok()?;
    if let Some(want) = want_pid {
        let got = unsafe { el.CurrentProcessId() }.unwrap_or(0);
        if got != want {
            tracing::debug!("[paster-win] focused element pid mismatch: want={want} got={got}");
            return None;
        }
    }
    Some(el)
}

/// Strategy 1: UIA TextPattern.DocumentRange.GetText(-1)
fn read_text_pattern(el: &IUIAutomationElement) -> Option<String> {
    let pat = unsafe { el.GetCurrentPattern(UIA_TextPatternId) }.ok()?;
    let text: IUIAutomationTextPattern = pat.cast().ok()?;
    let range = unsafe { text.DocumentRange() }.ok()?;
    let bstr = unsafe { range.GetText(-1) }.ok()?;
    Some(bstr.to_string())
}

/// Strategy 2: UIA ValuePattern.CurrentValue
fn read_value_pattern(el: &IUIAutomationElement) -> Option<String> {
    let pat = unsafe { el.GetCurrentPattern(UIA_ValuePatternId) }.ok()?;
    let value: IUIAutomationValuePattern = pat.cast().ok()?;
    let bstr = unsafe { value.CurrentValue() }.ok()?;
    let s = bstr.to_string();
    if s.is_empty() { None } else { Some(s) }
}

/// Strategy 3: UIA TextPattern.GetSelection → expand to document → GetText
fn read_text_pattern_selection(el: &IUIAutomationElement) -> Option<String> {
    let pat = unsafe { el.GetCurrentPattern(UIA_TextPatternId) }.ok()?;
    let text: IUIAutomationTextPattern = pat.cast().ok()?;
    let sel = unsafe { text.GetSelection() }.ok()?;
    let count = unsafe { sel.Length() }.unwrap_or(0);
    if count <= 0 {
        return None;
    }
    let range = unsafe { sel.GetElement(0) }.ok()?;
    unsafe { range.ExpandToEnclosingUnit(TextUnit_Document) }.ok()?;
    let bstr = unsafe { range.GetText(-1) }.ok()?;
    Some(bstr.to_string())
}

/// Fast read: try only the cheapest strategies (UIA TextPattern, then
/// ValuePattern). No clipboard fallback (would disrupt typing).
fn read_fast(uia: &IUIAutomation, want_pid: Option<i32>) -> Option<String> {
    let el = focused_element(uia, want_pid)?;
    read_text_pattern(&el).or_else(|| read_value_pattern(&el))
}

/// Full read: TextPattern → ValuePattern → selection-expand → clipboard.
/// Used by edit-watch + diagnostics.
fn read_first(uia: &IUIAutomation, want_pid: Option<i32>) -> Option<String> {
    let el = focused_element(uia, want_pid);
    if let Some(ref e) = el {
        if let Some(t) = read_text_pattern(e) {
            return Some(t);
        }
        if let Some(t) = read_value_pattern(e) {
            return Some(t);
        }
        if let Some(t) = read_text_pattern_selection(e) {
            return Some(t);
        }
    }
    // Strategy 6: clipboard fallback (disruptive).
    capture_focused_text_via_selection()
}

// ── Diagnostics (mirrors macOS diagnose_focused_field) ───────────────────────

fn diagnose(uia: &IUIAutomation) -> AxDiagnostics {
    let mut report = AxDiagnostics {
        ax_trusted: true, // UIA needs no permission on Windows
        app_name: None,
        app_pid: focused_pid(),
        element_role: None,
        attributes: vec![],
        methods: vec![],
        clipboard: save_clipboard_text().unwrap_or_default(),
    };

    let el = focused_element(uia, None);
    if let Some(ref e) = el {
        report.app_name = unsafe { e.CurrentName() }.ok().map(|b| b.to_string());
        report.element_role = unsafe { e.CurrentLocalizedControlType() }
            .ok()
            .map(|b| b.to_string());
    }

    let push = |out: &mut Vec<AxMethodResult>, n: u8, label: &str, res: Option<String>| {
        out.push(AxMethodResult {
            method: format!(
                "{n}_uia_{}",
                label.split_whitespace().next().unwrap_or("strategy")
            ),
            label: label.into(),
            ok: res.is_some(),
            text: res,
            err: None,
        });
    };

    if let Some(ref e) = el {
        push(
            &mut report.methods,
            1,
            "UIA TextPattern.DocumentRange",
            read_text_pattern(e),
        );
        push(
            &mut report.methods,
            2,
            "UIA ValuePattern.CurrentValue",
            read_value_pattern(e),
        );
        push(
            &mut report.methods,
            3,
            "UIA TextPattern.GetSelection (expanded)",
            read_text_pattern_selection(e),
        );
    } else {
        report.methods.push(AxMethodResult {
            method: "0_no_focused_element".into(),
            label: "UIA GetFocusedElement returned no element".into(),
            ok: false,
            text: None,
            err: Some("no focused element".into()),
        });
    }

    // Strategies 4 (MSAA) + 5 (WM_GETTEXT) are noted as TODO; clipboard
    // fallback (6) is omitted from the diagnostic to avoid disrupting the
    // user's clipboard when they hit /diagnose-ax.
    report.methods.push(AxMethodResult {
        method: "4_msaa_skipped".into(),
        label: "MSAA accValue (P3.1)".into(),
        ok: false,
        text: None,
        err: Some("not yet implemented".into()),
    });
    report.methods.push(AxMethodResult {
        method: "5_wm_gettext_skipped".into(),
        label: "WM_GETTEXT (P3.1)".into(),
        ok: false,
        text: None,
        err: Some("not yet implemented".into()),
    });

    report
}

// ── Public read API ──────────────────────────────────────────────────────────

pub fn read_focused_value_fast() -> Option<String> {
    match send_read(ReadKind::FocusedFast { pid: None })? {
        ReadReply::Text(t) => t,
        _ => None,
    }
}

pub fn read_focused_value_first() -> Option<String> {
    match send_read(ReadKind::FocusedFirst { pid: None })? {
        ReadReply::Text(t) => t,
        _ => None,
    }
}

pub fn read_focused_value() -> Option<String> {
    read_focused_value_first()
}

pub fn read_focused_value_fast_for_pid(pid: i32) -> Option<String> {
    match send_read(ReadKind::FocusedFast { pid: Some(pid) })? {
        ReadReply::Text(t) => t,
        _ => None,
    }
}

pub fn read_focused_value_first_for_pid(pid: i32) -> Option<String> {
    match send_read(ReadKind::FocusedFirst { pid: Some(pid) })? {
        ReadReply::Text(t) => t,
        _ => None,
    }
}

pub fn diagnose_focused_field() -> AxDiagnostics {
    match send_read(ReadKind::Diagnose) {
        Some(ReadReply::Diagnostics(d)) => *d,
        _ => AxDiagnostics {
            ax_trusted: true,
            app_name: None,
            app_pid: focused_pid(),
            element_role: None,
            attributes: vec![],
            methods: vec![AxMethodResult {
                method: "0_uia_timeout".into(),
                label: "UIA reader did not reply within 5 s".into(),
                ok: false,
                text: None,
                err: Some("timeout".into()),
            }],
            clipboard: String::new(),
        },
    }
}

// `locked_pid` is reserved for the P3.1 follow-up where edit-watch can opt
// into "read against locked pid even if focus drifted" — until then it's
// referenced only by the unit-test surface.
#[allow(dead_code)]
fn _reserved_for_p3_1() {
    let _ = locked_pid();
}
