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
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, VK_CONTROL, VK_LMENU, VK_MENU, VK_RMENU, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, HHOOK, KBDLLHOOKSTRUCT, MSG, SetWindowsHookExW,
    TranslateMessage, WH_KEYBOARD_LL,
};

use crate::HudShortcutAction;
use crate::RecordHotkey;
use crate::win_hotkey::{
    HookAction, ShortcutAction, WinModifiers, classify, classify_long_dictation, classify_shortcut,
    target_vk, wparam_to_kind,
};

// ── Pref: which key to watch ──────────────────────────────────────────────────
// 0 = CapsLock, 1 = RightOption (mapped to VK_RMENU on Windows),
// 2 = Function (no Windows analog — falls back to pass-through).
static RECORD_HOTKEY: AtomicU8 = AtomicU8::new(0);
// Backing store for the `Modifier { win_vk }` variant (kind == 3).
static RECORD_MOD_VK: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

pub fn set_record_hotkey(hotkey: RecordHotkey) {
    let encoded = match hotkey {
        RecordHotkey::CapsLock => 0,
        RecordHotkey::RightOption => 1,
        RecordHotkey::Function => 2,
        RecordHotkey::Modifier { win_vk, .. } => {
            RECORD_MOD_VK.store(win_vk, Ordering::Relaxed);
            3
        }
    };
    RECORD_HOTKEY.store(encoded, Ordering::Relaxed);
    tracing::info!("[hotkey] record hotkey set to {:?}", hotkey);
}

fn current_record_hotkey() -> RecordHotkey {
    match RECORD_HOTKEY.load(Ordering::Relaxed) {
        1 => RecordHotkey::RightOption,
        2 => RecordHotkey::Function,
        3 => RecordHotkey::Modifier {
            mac_keycode: 0,
            mac_mask: 0,
            win_vk: RECORD_MOD_VK.load(Ordering::Relaxed),
        },
        _ => RecordHotkey::CapsLock,
    }
}

// ── Permission gate ───────────────────────────────────────────────────────────
//
// There is no Windows equivalent to macOS TCC Input Monitoring — low-level
// keyboard hooks need no special grant for non-elevated targets, which is
// what AirNote records. Always granted.
pub fn is_input_monitoring_granted() -> bool {
    true
}

// ── State + callbacks ─────────────────────────────────────────────────────────

static ON_PRESS: OnceLock<Arc<dyn Fn() + Send + Sync>> = OnceLock::new();
static ON_RELEASE: OnceLock<Arc<dyn Fn() + Send + Sync>> = OnceLock::new();
static SHORTCUT_CB: OnceLock<Arc<dyn Fn(u8) + Send + Sync>> = OnceLock::new();
static PASTE_CB: OnceLock<Arc<dyn Fn() + Send + Sync>> = OnceLock::new();
static LONG_DICTATION_CB: OnceLock<Arc<dyn Fn() + Send + Sync>> = OnceLock::new();
static HUD_SHORTCUT_CB: OnceLock<Arc<dyn Fn(HudShortcutAction) + Send + Sync>> = OnceLock::new();
/// True while the target key is physically held. The low-level hook fires on
/// every autorepeat, so we de-duplicate via this flag.
static IS_DOWN: AtomicBool = AtomicBool::new(false);
/// Swallow the next Alt key-up after an Alt-based shortcut fires. Without this,
/// Windows can focus the menu bar after a global Alt shortcut even when the
/// actual shortcut key was suppressed.
static SUPPRESS_NEXT_ALT_UP: AtomicBool = AtomicBool::new(false);

fn vk_down(vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY) -> bool {
    unsafe { (GetKeyState(vk.0 as i32) as u16 & 0x8000) != 0 }
}

fn current_modifiers() -> WinModifiers {
    let left_alt = vk_down(VK_LMENU);
    let right_alt = vk_down(VK_RMENU);
    WinModifiers {
        ctrl: vk_down(VK_CONTROL),
        shift: vk_down(VK_SHIFT),
        alt: vk_down(VK_MENU) || left_alt || right_alt,
        left_alt,
        right_alt,
    }
}

fn is_alt_vk(vk: u32) -> bool {
    vk == VK_MENU.0 as u32 || vk == VK_LMENU.0 as u32 || vk == VK_RMENU.0 as u32
}

fn fire_shortcut(action: ShortcutAction) {
    match action {
        ShortcutAction::Tone(n) => {
            SUPPRESS_NEXT_ALT_UP.store(true, Ordering::Relaxed);
            tracing::info!("[hotkey] Alt+{n} fired — calling tray polish callback");
            if let Some(cb) = SHORTCUT_CB.get() {
                cb(n);
            } else {
                tracing::warn!("[hotkey] Alt+{n} fired but SHORTCUT_CB not registered!");
            }
        }
        ShortcutAction::PasteLatest => {
            SUPPRESS_NEXT_ALT_UP.store(true, Ordering::Relaxed);
            tracing::info!("[hotkey] Ctrl+Alt+V detected — firing paste callback");
            if let Some(cb) = PASTE_CB.get() {
                cb();
            } else {
                tracing::warn!("[hotkey] Ctrl+Alt+V fired but PASTE_CB not registered!");
            }
        }
        ShortcutAction::Hud(action) => {
            tracing::info!("[hotkey] Windows HUD shortcut detected — {action:?}");
            if let Some(cb) = HUD_SHORTCUT_CB.get() {
                cb(action);
            } else {
                tracing::warn!("[hotkey] HUD shortcut fired but HUD_SHORTCUT_CB not registered!");
            }
        }
    }
}

fn fire_long_dictation() {
    tracing::info!("[hotkey] Windows record-key+Space detected — locking long dictation");
    if let Some(cb) = LONG_DICTATION_CB.get() {
        cb();
    } else {
        tracing::warn!("[hotkey] record-key+Space fired but LONG_DICTATION_CB not registered!");
    }
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // Catch any panic before it unwinds across the Win32 kernel callback
    // boundary (undefined behavior / process abort). On panic, pass the event
    // through untouched — the safe default that never swallows a keystroke.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        hook_proc_inner(code, wparam, lparam)
    }));
    match result {
        Ok(ret) => ret,
        Err(_) => {
            tracing::error!("[hotkey] recovered panic inside WH_KEYBOARD_LL hook_proc");
            unsafe { CallNextHookEx(HHOOK::default(), code, wparam, lparam) }
        }
    }
}

unsafe fn hook_proc_inner(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code < 0 {
        return unsafe { CallNextHookEx(HHOOK::default(), code, wparam, lparam) };
    }

    // Defensive: injected/synthetic events (AutoHotKey, IMEs, accessibility
    // tools) can deliver a null KBDLLHOOKSTRUCT pointer. Dereferencing it is a
    // hard segfault, so pass through instead.
    if lparam.0 == 0 {
        return unsafe { CallNextHookEx(HHOOK::default(), code, wparam, lparam) };
    }

    let kb = unsafe { *(lparam.0 as *const KBDLLHOOKSTRUCT) };
    let vk = kb.vkCode;
    let kind = wparam_to_kind(wparam.0 as u32);

    if matches!(kind, crate::win_hotkey::EvtKind::KeyUp)
        && is_alt_vk(vk)
        && SUPPRESS_NEXT_ALT_UP.swap(false, Ordering::Relaxed)
    {
        return LRESULT(1);
    }

    let mods = current_modifiers();
    let record_hotkey = current_record_hotkey();

    // A key pressed while the record hotkey is held makes that press a
    // shortcut, never a tap (see `tap_latch`). Checked before the shortcut
    // and long-dictation handlers, which return early.
    if matches!(kind, crate::win_hotkey::EvtKind::KeyDown) && Some(vk) != target_vk(record_hotkey) {
        crate::tap_latch::with_latch(|latch| latch.other_key());
    }

    if classify_long_dictation(
        vk,
        kind,
        mods,
        record_hotkey,
        IS_DOWN.load(Ordering::Relaxed),
    ) {
        fire_long_dictation();
        return LRESULT(1);
    }

    if let Some(action) = classify_shortcut(vk, kind, mods, record_hotkey) {
        fire_shortcut(action);
        return LRESULT(1);
    }

    let target = target_vk(record_hotkey);
    let was_down = IS_DOWN.load(Ordering::Relaxed);

    match classify(vk, kind, target, was_down) {
        HookAction::PassThrough => unsafe {
            CallNextHookEx(HHOOK::default(), code, wparam, lparam)
        },
        HookAction::Swallow {
            fire_press,
            fire_release,
        } => {
            // Windows swallows every record hotkey, Caps Lock included, so
            // none of them toggle on their own; the tap latch gives all of
            // them tap-to-talk alongside hold-to-talk.
            let now = std::time::Instant::now();
            let mut action = crate::tap_latch::LatchAction::Ignore;
            if fire_press {
                IS_DOWN.store(true, Ordering::Relaxed);
                action = crate::tap_latch::with_latch(|latch| latch.press(now));
            }
            if fire_release {
                IS_DOWN.store(false, Ordering::Relaxed);
                action = crate::tap_latch::with_latch(|latch| latch.release(now));
            }
            match action {
                crate::tap_latch::LatchAction::Start => {
                    if let Some(cb) = ON_PRESS.get() {
                        cb();
                    }
                }
                crate::tap_latch::LatchAction::Finish => {
                    if let Some(cb) = ON_RELEASE.get() {
                        cb();
                    }
                }
                crate::tap_latch::LatchAction::Ignore => {}
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

// ── Global shortcut callbacks ────────────────────────────────────────────────

pub fn register_shortcut_callback(cb: Arc<dyn Fn(u8) + Send + Sync>) {
    let _ = SHORTCUT_CB.set(cb);
}

pub fn register_paste_callback(cb: Arc<dyn Fn() + Send + Sync>) {
    let _ = PASTE_CB.set(cb);
}

pub fn register_long_dictation_callback(cb: Arc<dyn Fn() + Send + Sync>) {
    let _ = LONG_DICTATION_CB.set(cb);
}

pub fn register_hud_shortcut_callback(cb: Arc<dyn Fn(HudShortcutAction) + Send + Sync>) {
    let _ = HUD_SHORTCUT_CB.set(cb);
}
