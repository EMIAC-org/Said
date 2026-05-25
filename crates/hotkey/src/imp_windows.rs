//! Windows low-level keyboard hook hotkey listener.
//!
//! Public API matches the macOS `imp` module (re-exported through
//! `crates/hotkey/src/lib.rs`). The hard logic lives in
//! [`crate::win_hotkey`] so it's covered by unit tests on every host.
//!
//! Caps Lock is the default hold-to-record key. The hook **suppresses** the
//! Caps Lock keydown and keyup, so the OS-level toggle state never fires —
//! same UX as macOS. If the user sets the record hotkey to Right Alt, that
//! key is held instead and the same suppression applies to VK_RMENU.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, OnceLock};

use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, HHOOK, KBDLLHOOKSTRUCT, MSG, SetWindowsHookExW,
    TranslateMessage, WH_KEYBOARD_LL,
};

use crate::HudShortcutAction;
use crate::RecordHotkey;
use crate::win_hotkey::{HookAction, classify, target_vk, wparam_to_kind};

// ── Pref: which key to watch ──────────────────────────────────────────────────
// 0 = CapsLock, 1 = RightOption (mapped to VK_RMENU on Windows),
// 2 = Function (no Windows analog — falls back to pass-through).
static RECORD_HOTKEY: AtomicU8 = AtomicU8::new(0);

pub fn set_record_hotkey(hotkey: RecordHotkey) {
    let encoded = match hotkey {
        RecordHotkey::CapsLock => 0,
        RecordHotkey::RightOption => 1,
        RecordHotkey::Function => 2,
    };
    RECORD_HOTKEY.store(encoded, Ordering::Relaxed);
    tracing::info!("[hotkey] record hotkey set to {:?}", hotkey);
}

fn current_record_hotkey() -> RecordHotkey {
    match RECORD_HOTKEY.load(Ordering::Relaxed) {
        1 => RecordHotkey::RightOption,
        2 => RecordHotkey::Function,
        _ => RecordHotkey::CapsLock,
    }
}

// ── Permission gate ───────────────────────────────────────────────────────────
//
// There is no Windows equivalent to macOS TCC Input Monitoring — low-level
// keyboard hooks need no special grant for non-elevated targets, which is
// what Said records. Always granted.
pub fn is_input_monitoring_granted() -> bool {
    true
}

// ── State + callbacks ─────────────────────────────────────────────────────────

static ON_PRESS: OnceLock<Arc<dyn Fn() + Send + Sync>> = OnceLock::new();
static ON_RELEASE: OnceLock<Arc<dyn Fn() + Send + Sync>> = OnceLock::new();
/// True while the target key is physically held. The low-level hook fires on
/// every autorepeat, so we de-duplicate via this flag.
static IS_DOWN: AtomicBool = AtomicBool::new(false);

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code < 0 {
        return unsafe { CallNextHookEx(HHOOK::default(), code, wparam, lparam) };
    }

    let kb = unsafe { *(lparam.0 as *const KBDLLHOOKSTRUCT) };
    let vk = kb.vkCode;
    let kind = wparam_to_kind(wparam.0 as u32);
    let target = target_vk(current_record_hotkey());
    let was_down = IS_DOWN.load(Ordering::Relaxed);

    match classify(vk, kind, target, was_down) {
        HookAction::PassThrough => unsafe {
            CallNextHookEx(HHOOK::default(), code, wparam, lparam)
        },
        HookAction::Swallow {
            fire_press,
            fire_release,
        } => {
            if fire_press {
                IS_DOWN.store(true, Ordering::Relaxed);
                if let Some(cb) = ON_PRESS.get() {
                    cb();
                }
            }
            if fire_release {
                IS_DOWN.store(false, Ordering::Relaxed);
                if let Some(cb) = ON_RELEASE.get() {
                    cb();
                }
            }
            LRESULT(1)
        }
    }
}

// ── Hold listener entry point ─────────────────────────────────────────────────

pub fn start_hold_listener(
    on_press: Arc<dyn Fn() + Send + Sync>,
    on_release: Arc<dyn Fn() + Send + Sync>,
) {
    let _ = ON_PRESS.set(on_press);
    let _ = ON_RELEASE.set(on_release);

    std::thread::spawn(move || unsafe {
        let hinstance: HINSTANCE = GetModuleHandleW(None)
            .map(HINSTANCE::from)
            .unwrap_or_default();

        let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), hinstance, 0);
        let hook = match hook {
            Ok(h) if !h.is_invalid() => h,
            Ok(_) => {
                tracing::error!("[hotkey] SetWindowsHookExW returned an invalid hook handle");
                return;
            }
            Err(e) => {
                tracing::error!("[hotkey] SetWindowsHookExW failed: {e}");
                return;
            }
        };
        tracing::info!("[hotkey] WH_KEYBOARD_LL installed — listening for hold hotkey");

        // Message pump: low-level hooks require the installing thread to
        // service messages or Windows silently removes the hook after a few
        // seconds.
        let mut msg = MSG::default();
        loop {
            let r = GetMessageW(&mut msg, None, 0, 0);
            if r.0 == 0 || r.0 == -1 {
                break;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        // Best-effort unhook on thread exit. In practice this thread runs
        // for the lifetime of the process; we only reach here on shutdown.
        let _ = windows::Win32::UI::WindowsAndMessaging::UnhookWindowsHookEx(hook);
    });
}

// ── Stubs: Option+1..5 and Ctrl+Cmd+V are not yet wired on Windows ────────────

pub fn register_shortcut_callback(_cb: Arc<dyn Fn(u8) + Send + Sync>) {
    tracing::debug!("[hotkey] register_shortcut_callback called on Windows — not yet wired");
}

pub fn register_paste_callback(_cb: Arc<dyn Fn() + Send + Sync>) {
    tracing::debug!("[hotkey] register_paste_callback called on Windows — not yet wired");
}

pub fn register_long_dictation_callback(_cb: Arc<dyn Fn() + Send + Sync>) {
    tracing::debug!(
        "[hotkey] register_long_dictation_callback called on Windows — no Fn key analog"
    );
}

pub fn register_hud_shortcut_callback(_cb: Arc<dyn Fn(HudShortcutAction) + Send + Sync>) {
    tracing::debug!("[hotkey] register_hud_shortcut_callback called on Windows — not yet wired");
}
