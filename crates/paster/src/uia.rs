//! Windows UI Automation reader for the learning pipeline.
//!
//! Mirrors the macOS Accessibility reads in `lib.rs` so the 30s edit-watcher can
//! see the focused text field on Windows and learning turns on. UIA is COM and
//! must run on a dedicated multithreaded-apartment (MTA) thread that owns no
//! windows — otherwise reading *other* apps' UI can deadlock. So all UIA work
//! happens on one long-lived worker thread; callers talk to it over channels and
//! only ever receive plain values (`String`/`i32`), never COM pointers (which
//! have apartment affinity and must not cross threads).
//!
//! The public `value`/`selection`/`info` helpers take a per-call timeout: a
//! wedged accessibility provider costs one dropped read, never a frozen daemon
//! (UIA's own transaction timeouts are unreliable, so this is the real guard).
//! `focused_pid` and `activate_foreground` are plain Win32 (no COM), so they run
//! inline without the worker.

use std::sync::OnceLock;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::time::Duration;

use uiautomation::UIAutomation;
use uiautomation::UIElement;
use uiautomation::UITreeWalker;
use uiautomation::patterns::{UITextPattern, UIValuePattern};

use crate::win_paster::find_unique_text_span_chars;

use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowThreadProcessId, SMTO_ABORTIFHUNG, SendMessageTimeoutW,
    WM_GETOBJECT,
};

// Mirror the macOS bounded subtree walk for Chromium/Electron contenteditable.
const MAX_WALK_DEPTH: u32 = 4;
const MAX_WALK_NODES: u32 = 64;

// WM_GETOBJECT lParam that asks for the client OBJID — the "knock" that makes
// Chromium/Electron build their accessibility tree for us. Defined locally to
// avoid pulling the Win32_UI_Accessibility feature just for one constant.
const OBJID_CLIENT: i32 = -4;

enum Req {
    /// Read the focused field's full text. `fast` skips the subtree walk (hot
    /// path). `pid` (if set) requires the focused element to belong to that
    /// process, preserving the macOS target-app lock.
    Value { fast: bool, pid: Option<i32> },
    /// Read only the selected text of the focused field.
    Selection,
    /// (app_name, control_type) of the focused element, for diagnostics.
    Info,
    /// Select one exact text range in the focused element. Refuses absent or
    /// duplicate matches so callers can safely paste over the selection.
    SelectExactText { text: String },
}

enum Rep {
    Text(Option<String>),
    Info(Option<(String, String)>),
    Bool(bool),
}

struct Reader {
    tx: SyncSender<(Req, SyncSender<Rep>)>,
}

static READER: OnceLock<Reader> = OnceLock::new();

fn reader() -> &'static Reader {
    READER.get_or_init(|| {
        let (tx, rx) = sync_channel::<(Req, SyncSender<Rep>)>(8);
        std::thread::Builder::new()
            .name("uia-worker".into())
            .spawn(move || worker_main(rx))
            .expect("spawn uia-worker thread");
        Reader { tx }
    })
}

fn call(req: Req, timeout_ms: u64) -> Option<Rep> {
    let (rtx, rrx) = sync_channel(1);
    reader().tx.try_send((req, rtx)).ok()?;
    rrx.recv_timeout(Duration::from_millis(timeout_ms)).ok()
}

/// Full focused-field text. `fast=true` is the 30ms-poll hot path (value/text
/// patterns only); `fast=false` adds the bounded subtree walk for browsers.
pub fn value(fast: bool, pid: Option<i32>, timeout_ms: u64) -> Option<String> {
    match call(Req::Value { fast, pid }, timeout_ms)? {
        Rep::Text(t) => t,
        _ => None,
    }
}

/// Selected text of the focused field (UIA `TextPattern::GetSelection`).
pub fn selection(timeout_ms: u64) -> Option<String> {
    match call(Req::Selection, timeout_ms)? {
        Rep::Text(t) => t,
        _ => None,
    }
}

/// (app_name, control_type) of the focused element — diagnostics only.
pub fn info(timeout_ms: u64) -> Option<(String, String)> {
    match call(Req::Info, timeout_ms)? {
        Rep::Info(i) => i,
        _ => None,
    }
}

/// Select an exact, unique text span in the focused element.
pub fn select_exact_text(text: &str, timeout_ms: u64) -> bool {
    if text.is_empty() {
        return false;
    }
    match call(
        Req::SelectExactText {
            text: text.to_string(),
        },
        timeout_ms,
    ) {
        Some(Rep::Bool(selected)) => selected,
        _ => false,
    }
}

/// PID of the foreground window's process. Plain Win32 — no COM, any thread.
pub fn focused_pid() -> Option<i32> {
    unsafe {
        let hwnd = GetForegroundWindow();
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        (pid != 0).then_some(pid as i32)
    }
}

/// "Knock" the foreground window with WM_GETOBJECT to activate Chromium/Electron
/// accessibility (mostly automatic on Chrome 126+, harmless on native apps), and
/// return its PID. The macOS analog sets AXEnhancedUserInterface/AXManualAccessibility.
pub fn activate_foreground() -> Option<i32> {
    unsafe {
        let hwnd = GetForegroundWindow();
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 {
            return None;
        }
        let mut result: usize = 0;
        // Bounded so a hung target can't wedge the worker/caller.
        let _ = SendMessageTimeoutW(
            hwnd,
            WM_GETOBJECT,
            WPARAM(0),
            LPARAM(OBJID_CLIENT as isize),
            SMTO_ABORTIFHUNG,
            200,
            Some(&mut result),
        );
        Some(pid as i32)
    }
}

// ── Worker thread (owns COM + the UIAutomation client for its whole life) ──────

fn worker_main(rx: Receiver<(Req, SyncSender<Rep>)>) {
    // `UIAutomation::new()` does CoInitializeEx(COINIT_MULTITHREADED) on this
    // thread and is !Send, so it must be created and used only here.
    let automation = match UIAutomation::new() {
        Ok(a) => a,
        Err(_) => {
            for (req, reply) in rx {
                let _ = reply.try_send(match req {
                    Req::Info => Rep::Info(None),
                    _ => Rep::Text(None),
                });
            }
            return;
        }
    };

    for (req, reply) in rx {
        let rep = match req {
            Req::Value { fast, pid } => Rep::Text(read_value(&automation, fast, pid)),
            Req::Selection => Rep::Text(read_selection(&automation)),
            Req::Info => Rep::Info(read_info(&automation)),
            Req::SelectExactText { text } => Rep::Bool(select_exact_text_range(&automation, &text)),
        };
        let _ = reply.try_send(rep);
    }
}

fn read_value(automation: &UIAutomation, fast: bool, pid: Option<i32>) -> Option<String> {
    let el = automation.get_focused_element().ok()?;
    if let Some(want) = pid {
        if el.get_process_id().ok()? != want as u32 {
            return None;
        }
    }
    if el.is_password().unwrap_or(false) {
        return None;
    }
    if let Some(v) = value_pattern_text(&el) {
        return Some(v);
    }
    if let Some(t) = text_pattern_text(&el) {
        return Some(t);
    }
    if fast {
        return None;
    }
    // Browser/Electron contenteditable: the focused container often exposes no
    // direct value; walk a bounded subtree for the first text-bearing node.
    let walker = automation.create_tree_walker().ok()?;
    let mut visited: u32 = 0;
    walk_subtree(&walker, &el, 0, &mut visited)
}

fn read_selection(automation: &UIAutomation) -> Option<String> {
    let el = automation.get_focused_element().ok()?;
    if el.is_password().unwrap_or(false) {
        return None;
    }
    let tp = el.get_pattern::<UITextPattern>().ok()?;
    let ranges = tp.get_selection().ok()?;
    let first = ranges.first()?;
    let text = first.get_text(-1).ok()?;
    (!text.trim().is_empty()).then_some(text)
}

fn read_info(automation: &UIAutomation) -> Option<(String, String)> {
    let el = automation.get_focused_element().ok()?;
    let name = el.get_name().unwrap_or_default();
    let control = el
        .get_control_type()
        .map(|c| format!("{c:?}"))
        .unwrap_or_default();
    Some((name, control))
}

fn select_exact_text_range(automation: &UIAutomation, text: &str) -> bool {
    if text.is_empty() {
        return false;
    }
    let current = match read_value(automation, false, None) {
        Some(value) => value,
        None => return false,
    };
    if find_unique_text_span_chars(&current, text).is_none() {
        return false;
    }

    let el = match automation.get_focused_element() {
        Ok(el) => el,
        Err(_) => return false,
    };
    if el.is_password().unwrap_or(false) {
        return false;
    }
    let tp = match el.get_pattern::<UITextPattern>() {
        Ok(tp) => tp,
        Err(_) => return false,
    };
    let document_range = match tp.get_document_range() {
        Ok(range) => range,
        Err(_) => return false,
    };
    let range = match document_range.find_text(text, false, false) {
        Ok(range) => range,
        Err(_) => return false,
    };
    if range.get_text(-1).ok().as_deref() != Some(text) {
        return false;
    }
    range.select().is_ok()
}

fn value_pattern_text(el: &UIElement) -> Option<String> {
    let vp = el.get_pattern::<UIValuePattern>().ok()?;
    let v = vp.get_value().ok()?;
    (!v.is_empty()).then_some(v)
}

fn text_pattern_text(el: &UIElement) -> Option<String> {
    let tp = el.get_pattern::<UITextPattern>().ok()?;
    let range = tp.get_document_range().ok()?;
    let text = range.get_text(-1).ok()?;
    (!text.trim().is_empty()).then_some(text)
}

fn walk_subtree(
    walker: &UITreeWalker,
    root: &UIElement,
    depth: u32,
    visited: &mut u32,
) -> Option<String> {
    if depth >= MAX_WALK_DEPTH || *visited >= MAX_WALK_NODES {
        return None;
    }
    let mut child = walker.get_first_child(root).ok();
    while let Some(node) = child {
        *visited += 1;
        if *visited > MAX_WALK_NODES {
            return None;
        }
        if !node.is_password().unwrap_or(false) {
            if let Some(v) = value_pattern_text(&node) {
                return Some(v);
            }
            if let Some(t) = text_pattern_text(&node) {
                return Some(t);
            }
        }
        if let Some(found) = walk_subtree(walker, &node, depth + 1, visited) {
            return Some(found);
        }
        child = walker.get_next_sibling(&node).ok();
    }
    None
}
