//! Windows hotkey listener — `WH_KEYBOARD_LL` + `WH_MOUSE_LL` low-level hooks.
//!
//! ## Threading model
//!
//! Two dedicated threads, communicating via a lock-free `ArrayQueue`:
//!
//!   * **`said-hotkey-pump`** — owns the hook installation and runs the
//!     Win32 message loop required by `WH_KEYBOARD_LL`. The hook callback
//!     fires on this thread and MUST return in <300 ms or Windows
//!     silently unhooks. We do the absolute minimum here: read the
//!     event struct, push a `RawEvent` to the queue, decide whether to
//!     suppress, then return.
//!
//!   * **`said-hotkey-dispatch`** — drains the queue, classifies events
//!     against the current `RecordHotkey` setting, fires user callbacks
//!     (`on_press` / `on_release` / shortcut / paste), and populates the
//!     cross-platform `KEY_BUF` for edit-watch reconstruction.
//!
//! ## Hotkey suppression
//!
//! The LL hook can swallow events by returning a non-zero `LRESULT`. We
//! only swallow the **record hotkey itself** (Right Ctrl by default), and
//! only for the Caps Lock variant where we need to neutralize the OS-side
//! caps state toggle. Other shortcuts (Option+1..5, Ctrl+Cmd+V on Mac)
//! map to Windows-specific bindings in P3+ — until then the callbacks
//! exist but the underlying chord isn't bound, so nothing is suppressed.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::time::{Duration, Instant};

use crossbeam_queue::ArrayQueue;
use once_cell::sync::OnceCell;
use parking_lot::Mutex;
use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetKeyboardLayout, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, ToUnicodeEx, VK_A,
    VK_BACK, VK_C, VK_CAPITAL, VK_CONTROL, VK_DELETE, VK_DOWN, VK_END, VK_F13, VK_HOME,
    VK_LCONTROL, VK_LEFT, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_MENU, VK_PAUSE, VK_RCONTROL, VK_RIGHT,
    VK_RMENU, VK_RSHIFT, VK_RWIN, VK_SHIFT, VK_UP, VK_V, VK_X, VK_Z, keybd_event,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, HHOOK, KBDLLHOOKSTRUCT, LLKHF_EXTENDED,
    LLKHF_INJECTED, MSG, MSLLHOOKSTRUCT, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx,
    WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_RBUTTONDOWN,
    WM_SYSKEYDOWN, WM_SYSKEYUP,
};

use crate::RecordHotkey;
use crate::shared::{KeyEvt, push_key};

// ── Inter-thread queue ────────────────────────────────────────────────────────

/// Capacity sized for a 1 ms-per-event sustained burst (1024 events/s).
/// Overflow drops the oldest events — acceptable for the edit-watch use
/// case where bursts are rare and short.
const QUEUE_CAPACITY: usize = 1024;

#[derive(Debug, Clone, Copy)]
enum RawEvent {
    KeyDown { vk: u32, scan: u32, flags: u32 },
    KeyUp { vk: u32, flags: u32 },
    LeftMouseDown,
    RightMouseDown,
}

static EVENT_QUEUE: OnceCell<Arc<ArrayQueue<RawEvent>>> = OnceCell::new();

fn queue() -> &'static Arc<ArrayQueue<RawEvent>> {
    EVENT_QUEUE.get_or_init(|| Arc::new(ArrayQueue::new(QUEUE_CAPACITY)))
}

// ── Configuration state ───────────────────────────────────────────────────────

/// Encoded `RecordHotkey` (defaults to `RightCtrl` on Windows).
static RECORD_HOTKEY: AtomicU8 = AtomicU8::new(ENC_RIGHT_CTRL);
const ENC_CAPS_LOCK: u8 = 0;
const ENC_RIGHT_OPTION: u8 = 1; // Right Alt on Windows
const ENC_FUNCTION: u8 = 2; // No-op on Windows (Fn isn't exposed)
const ENC_RIGHT_CTRL: u8 = 3;
const ENC_F13: u8 = 4;
const ENC_PAUSE: u8 = 5;

fn current_record_hotkey() -> RecordHotkey {
    match RECORD_HOTKEY.load(Ordering::Relaxed) {
        ENC_CAPS_LOCK => RecordHotkey::CapsLock,
        ENC_RIGHT_OPTION => RecordHotkey::RightOption,
        ENC_FUNCTION => RecordHotkey::Function,
        ENC_F13 => RecordHotkey::F13,
        ENC_PAUSE => RecordHotkey::Pause,
        _ => RecordHotkey::RightCtrl,
    }
}

pub fn set_record_hotkey(hotkey: RecordHotkey) {
    let enc = match hotkey {
        RecordHotkey::CapsLock => ENC_CAPS_LOCK,
        RecordHotkey::RightOption => ENC_RIGHT_OPTION,
        RecordHotkey::Function => ENC_FUNCTION,
        RecordHotkey::RightCtrl => ENC_RIGHT_CTRL,
        RecordHotkey::F13 => ENC_F13,
        RecordHotkey::Pause => ENC_PAUSE,
    };
    RECORD_HOTKEY.store(enc, Ordering::Relaxed);
    tracing::info!("[hotkey] record hotkey set to {hotkey:?}");
}

// ── Callback registration ─────────────────────────────────────────────────────

static SHORTCUT_CB: OnceCell<Arc<dyn Fn(u8) + Send + Sync>> = OnceCell::new();
static PASTE_CB: OnceCell<Arc<dyn Fn() + Send + Sync>> = OnceCell::new();
static TOGGLE_CB: OnceCell<Arc<dyn Fn() + Send + Sync>> = OnceCell::new();

pub fn register_shortcut_callback(cb: Arc<dyn Fn(u8) + Send + Sync>) {
    let _ = SHORTCUT_CB.set(cb);
}

pub fn register_paste_callback(cb: Arc<dyn Fn() + Send + Sync>) {
    let _ = PASTE_CB.set(cb);
}

// ── Hold-listener state ───────────────────────────────────────────────────────

struct HoldState {
    is_down: bool,
    on_press: Arc<dyn Fn() + Send + Sync>,
    on_release: Arc<dyn Fn() + Send + Sync>,
    /// Last Caps Lock toggle (for debouncing the OS-level toggle when
    /// using CapsLock as hold-to-talk).
    last_caps_toggle: Instant,
}

static HOLD_STATE: OnceCell<Mutex<HoldState>> = OnceCell::new();
static HOOKS_INSTALLED: AtomicBool = AtomicBool::new(false);

// ── Win32 permission stubs (no equivalent gate on Windows) ────────────────────

pub fn is_input_monitoring_granted() -> bool {
    true
}

// ── Hook callbacks (run on the pump thread; must be fast) ─────────────────────

/// Low-level keyboard hook. Pushes a `RawEvent` and decides suppression.
unsafe extern "system" fn ll_keyboard_callback(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if code < 0 {
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }

    // SAFETY: For code >= 0, lparam points to a KBDLLHOOKSTRUCT owned by
    // the OS for the duration of the callback. We only read fields.
    let kb = unsafe { *(lparam.0 as *const KBDLLHOOKSTRUCT) };
    let vk = kb.vkCode;
    let scan = kb.scanCode;
    let flags = kb.flags.0;

    // Skip events we injected ourselves (e.g. via `keybd_event` to neutralize
    // a Caps Lock toggle). Without this, we'd see our own injected events
    // and re-process them in an infinite loop.
    if (flags & LLKHF_INJECTED.0) != 0 {
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }

    let msg = wparam.0 as u32;
    let raw = match msg {
        WM_KEYDOWN | WM_SYSKEYDOWN => RawEvent::KeyDown { vk, scan, flags },
        WM_KEYUP | WM_SYSKEYUP => RawEvent::KeyUp { vk, flags },
        _ => return unsafe { CallNextHookEx(None, code, wparam, lparam) },
    };

    // Try-push: full queue means we drop the event (acceptable for our use case).
    let _ = queue().push(raw);

    if should_suppress(vk, flags, msg) {
        return LRESULT(1);
    }

    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Low-level mouse hook. Only used to populate `KEY_BUF` with click events
/// (mouse-driven cursor moves invalidate keystroke-based edit reconstruction).
unsafe extern "system" fn ll_mouse_callback(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code < 0 {
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }
    // SAFETY: same lifetime contract as keyboard hook.
    let ms = unsafe { *(lparam.0 as *const MSLLHOOKSTRUCT) };
    if (ms.flags & LLKHF_INJECTED.0) != 0 {
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }
    match wparam.0 as u32 {
        WM_LBUTTONDOWN => {
            let _ = queue().push(RawEvent::LeftMouseDown);
        }
        WM_RBUTTONDOWN => {
            let _ = queue().push(RawEvent::RightMouseDown);
        }
        _ => {}
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Decide whether the hook should swallow this event.
fn should_suppress(vk: u32, flags: u32, msg: u32) -> bool {
    // We only suppress when CapsLock is the chosen record hotkey AND the
    // event is a Caps Lock key press/release. Otherwise the OS-level
    // caps-lock toggle fires and the user's typing is mangled.
    let hotkey = current_record_hotkey();
    if matches!(hotkey, RecordHotkey::CapsLock) && vk == VK_CAPITAL.0 as u32 {
        // Suppress BOTH keydown and keyup; we'll synthesize state ourselves.
        let _ = (flags, msg);
        return true;
    }
    false
}

// ── Pump thread: installs hooks + runs the Win32 message loop ────────────────

fn pump_thread() {
    let h_instance = match unsafe { GetModuleHandleW(None) } {
        Ok(h) => HINSTANCE(h.0),
        Err(e) => {
            tracing::error!("[hotkey] GetModuleHandleW failed: {e:?}");
            return;
        }
    };

    let kb_hook =
        unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(ll_keyboard_callback), h_instance, 0) };
    let kb_hook: HHOOK = match kb_hook {
        Ok(h) => h,
        Err(e) => {
            tracing::error!("[hotkey] SetWindowsHookExW(WH_KEYBOARD_LL) failed: {e:?}");
            return;
        }
    };

    let mouse_hook =
        unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(ll_mouse_callback), h_instance, 0) };
    let mouse_hook: Option<HHOOK> = match mouse_hook {
        Ok(h) => Some(h),
        Err(e) => {
            tracing::warn!(
                "[hotkey] SetWindowsHookExW(WH_MOUSE_LL) failed: {e:?} — \
                 continuing without mouse click events"
            );
            None
        }
    };

    HOOKS_INSTALLED.store(true, Ordering::Release);
    tracing::info!("[hotkey] LL hooks installed on said-hotkey-pump thread");

    // Standard Win32 message loop. LL hooks require a message pump on the
    // thread that installed them, even if the app never posts WM_QUIT.
    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let _ = UnhookWindowsHookEx(kb_hook);
        if let Some(h) = mouse_hook {
            let _ = UnhookWindowsHookEx(h);
        }
    }
}

// ── Dispatch thread: drains queue, fires callbacks, populates KEY_BUF ────────

fn dispatch_thread(q: Arc<ArrayQueue<RawEvent>>) {
    loop {
        match q.pop() {
            Some(evt) => handle_event(evt),
            None => std::thread::sleep(Duration::from_millis(1)),
        }
    }
}

fn handle_event(evt: RawEvent) {
    match evt {
        RawEvent::KeyDown { vk, scan, flags } => {
            handle_hold_keydown(vk, flags);
            handle_shortcut_keydown(vk);
            handle_paste_keydown(vk);
            push_key(classify_keydown(vk, scan, flags));
        }
        RawEvent::KeyUp { vk, flags } => {
            handle_hold_keyup(vk, flags);
        }
        RawEvent::LeftMouseDown | RawEvent::RightMouseDown => {
            push_key(KeyEvt::MouseClick);
        }
    }
}

// ── Hold-hotkey state machine ────────────────────────────────────────────────

const CAPS_DEBOUNCE_MS: u128 = 300;

fn handle_hold_keydown(vk: u32, flags: u32) {
    let Some(state) = HOLD_STATE.get() else {
        return;
    };
    if !matches_record_hotkey(vk, flags) {
        return;
    }

    let mut s = state.lock();
    let hotkey = current_record_hotkey();

    // For CapsLock-as-PTT: the OS already toggled caps state before our hook
    // fired, even though we suppressed the event. Re-toggle to neutralize.
    if matches!(hotkey, RecordHotkey::CapsLock) {
        if s.last_caps_toggle.elapsed().as_millis() < CAPS_DEBOUNCE_MS {
            return;
        }
        s.last_caps_toggle = Instant::now();
        neutralize_caps_lock();
    }

    if !s.is_down {
        s.is_down = true;
        let cb = Arc::clone(&s.on_press);
        drop(s); // release mutex before invoking user callback
        tracing::info!("[hotkey] {hotkey:?} pressed → on_press");
        cb();
    }
}

fn handle_hold_keyup(vk: u32, flags: u32) {
    let Some(state) = HOLD_STATE.get() else {
        return;
    };
    if !matches_record_hotkey(vk, flags) {
        return;
    }

    let mut s = state.lock();
    if s.is_down {
        s.is_down = false;
        let cb = Arc::clone(&s.on_release);
        let hotkey = current_record_hotkey();
        drop(s);
        tracing::info!("[hotkey] {hotkey:?} released → on_release");
        cb();
    }
}

/// Does this (vk, flags) pair match the currently-selected record hotkey?
fn matches_record_hotkey(vk: u32, flags: u32) -> bool {
    let extended = (flags & LLKHF_EXTENDED.0) != 0;
    let hotkey = current_record_hotkey();
    match hotkey {
        RecordHotkey::CapsLock => vk == VK_CAPITAL.0 as u32,
        RecordHotkey::RightOption => {
            // Right Alt: either VK_RMENU directly, or VK_MENU with extended bit set.
            vk == VK_RMENU.0 as u32 || (vk == VK_MENU.0 as u32 && extended)
        }
        RecordHotkey::Function => false, // Fn isn't exposed to userspace on Windows
        RecordHotkey::RightCtrl => {
            // Right Ctrl: either VK_RCONTROL directly, or VK_CONTROL with extended bit set.
            vk == VK_RCONTROL.0 as u32 || (vk == VK_CONTROL.0 as u32 && extended)
        }
        RecordHotkey::F13 => vk == VK_F13.0 as u32,
        RecordHotkey::Pause => vk == VK_PAUSE.0 as u32,
    }
}

/// After suppressing a Caps Lock keydown, the OS may still have toggled the
/// caps state. Inject a counter-toggle to undo it.
fn neutralize_caps_lock() {
    // Send a synthetic VK_CAPITAL key-down + key-up; the LLKHF_INJECTED flag
    // on the resulting hook event ensures we don't recurse.
    unsafe {
        keybd_event(VK_CAPITAL.0 as u8, 0, KEYBD_EVENT_FLAGS(0), 0);
        keybd_event(VK_CAPITAL.0 as u8, 0, KEYEVENTF_KEYUP, 0);
    }
}

// ── Shortcut + paste callbacks (stubbed at the chord level until P3) ─────────

fn handle_shortcut_keydown(_vk: u32) {
    // Windows shortcut chord TBD in P3 — Mac uses Option+1..5, but on
    // Windows Win+1..5 is reserved by the OS for the taskbar.
    // Likely target: Ctrl+Alt+1..5. Until that's decided + bound, no-op.
}

fn handle_paste_keydown(_vk: u32) {
    // Windows paste-latest chord TBD in P3 — Mac uses Ctrl+Cmd+V (which is
    // free), but on Windows Ctrl+Shift+V is "paste as plain text" in many
    // apps. Likely target: Ctrl+Win+V or a configurable chord.
}

// ── KEY_BUF classifier (mirrors macOS handle_key_down) ───────────────────────

fn modifier_state() -> ModifierState {
    // SAFETY: GetAsyncKeyState is a stable Win32 API; the high bit of the
    // returned i16 indicates the key is currently down. Safe to call from
    // any thread without locking.
    unsafe {
        ModifierState {
            ctrl: down(GetAsyncKeyState(VK_CONTROL.0 as i32)),
            alt: down(GetAsyncKeyState(VK_MENU.0 as i32)),
            shift: down(GetAsyncKeyState(VK_SHIFT.0 as i32)),
            win: down(GetAsyncKeyState(VK_LWIN.0 as i32))
                || down(GetAsyncKeyState(VK_RWIN.0 as i32)),
        }
    }
}

fn down(state: i16) -> bool {
    (state as u16) & 0x8000 != 0
}

struct ModifierState {
    ctrl: bool,
    alt: bool,
    shift: bool,
    #[allow(dead_code)]
    win: bool,
}

fn classify_keydown(vk: u32, scan: u32, flags: u32) -> KeyEvt {
    let m = modifier_state();

    // Ignore pure modifier presses
    if is_modifier_vk(vk) {
        return KeyEvt::Other;
    }

    if m.ctrl && m.alt {
        // Ctrl+Alt combos (AltGr emits this on intl layouts) — too ambiguous
        // for edit-watch heuristics. Treat as Other.
        return KeyEvt::Other;
    }

    if m.ctrl {
        return match vk_to_letter(vk) {
            Some(b'A') => KeyEvt::SelectAll,
            Some(b'C') => KeyEvt::Other, // Ctrl+C — leaves text unchanged; ignore for edit-watch
            Some(b'X') => KeyEvt::Cut,
            Some(b'Z') => KeyEvt::Undo,
            _ => match vk {
                v if v == VK_BACK.0 as u32 => KeyEvt::WordBackspace,
                v if v == VK_LEFT.0 as u32 => KeyEvt::WordLeft,
                v if v == VK_RIGHT.0 as u32 => KeyEvt::WordRight,
                v if v == VK_HOME.0 as u32 => KeyEvt::Home, // Ctrl+Home = doc start
                v if v == VK_END.0 as u32 => KeyEvt::End,
                _ => KeyEvt::Other,
            },
        };
    }

    // Bare-key classification
    match vk {
        v if v == VK_BACK.0 as u32 => KeyEvt::Backspace,
        v if v == VK_DELETE.0 as u32 => KeyEvt::Delete,
        v if v == VK_LEFT.0 as u32 => KeyEvt::Left,
        v if v == VK_RIGHT.0 as u32 => KeyEvt::Right,
        v if v == VK_UP.0 as u32 => KeyEvt::Home, // matches macOS Up = doc start
        v if v == VK_DOWN.0 as u32 => KeyEvt::End,
        v if v == VK_HOME.0 as u32 => KeyEvt::LineStart,
        v if v == VK_END.0 as u32 => KeyEvt::LineEnd,
        _ => char_from_keydown(vk, scan, flags).map_or(KeyEvt::Other, KeyEvt::Char),
    }
}

fn is_modifier_vk(vk: u32) -> bool {
    matches!(
        vk,
        v if v == VK_CONTROL.0 as u32
            || v == VK_LCONTROL.0 as u32
            || v == VK_RCONTROL.0 as u32
            || v == VK_MENU.0 as u32
            || v == VK_LMENU.0 as u32
            || v == VK_RMENU.0 as u32
            || v == VK_SHIFT.0 as u32
            || v == VK_LSHIFT.0 as u32
            || v == VK_RSHIFT.0 as u32
            || v == VK_LWIN.0 as u32
            || v == VK_RWIN.0 as u32
            || v == VK_CAPITAL.0 as u32
    )
}

fn vk_to_letter(vk: u32) -> Option<u8> {
    if (VK_A.0 as u32..=VK_Z.0 as u32).contains(&vk) {
        Some(b'A' + (vk - VK_A.0 as u32) as u8)
    } else if vk == VK_V.0 as u32 {
        Some(b'V')
    } else if vk == VK_X.0 as u32 {
        Some(b'X')
    } else if vk == VK_C.0 as u32 {
        Some(b'C')
    } else if vk == VK_Z.0 as u32 {
        Some(b'Z')
    } else {
        None
    }
}

/// Best-effort: translate a vk+scan+flags into the Unicode character the
/// user actually typed, honoring the current keyboard layout and dead keys.
fn char_from_keydown(vk: u32, scan: u32, _flags: u32) -> Option<char> {
    // Snapshot the live modifier state at the moment of the keydown so
    // ToUnicodeEx honors Shift / AltGr.
    let mut state = [0u8; 256];
    unsafe {
        for (i, slot) in state.iter_mut().enumerate() {
            let s = GetAsyncKeyState(i as i32);
            if down(s) {
                *slot = 0x80;
            }
        }
        let layout = GetKeyboardLayout(0);
        let mut buf = [0u16; 4];
        // 0x04 flag: do NOT alter the kernel keyboard state (no dead-key
        // mutation). Without it, our peeks would corrupt the user's typing.
        let n = ToUnicodeEx(vk, scan, &state, buf.as_mut_slice(), 0x04, layout);
        if n <= 0 {
            return None;
        }
        let s = String::from_utf16_lossy(&buf[..n as usize]);
        let mut chars = s.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) if !c.is_control() => Some(c),
            _ => None,
        }
    }
}

// ── Public listeners ─────────────────────────────────────────────────────────

pub fn start_listener(callback: Arc<dyn Fn() + Send + Sync>) {
    let _ = TOGGLE_CB.set(callback);
    ensure_pump_thread();
}

pub fn start_hold_listener(
    on_press: Arc<dyn Fn() + Send + Sync>,
    on_release: Arc<dyn Fn() + Send + Sync>,
) {
    let _ = HOLD_STATE.set(Mutex::new(HoldState {
        is_down: false,
        on_press,
        on_release,
        last_caps_toggle: Instant::now() - Duration::from_secs(10),
    }));
    ensure_pump_thread();
}

/// Idempotent: starts the pump + dispatch threads once.
fn ensure_pump_thread() {
    static SPAWNED: AtomicBool = AtomicBool::new(false);
    if SPAWNED.swap(true, Ordering::AcqRel) {
        return;
    }

    let q = Arc::clone(queue());

    std::thread::Builder::new()
        .name("said-hotkey-pump".into())
        .spawn(pump_thread)
        .expect("failed to spawn said-hotkey-pump thread");

    std::thread::Builder::new()
        .name("said-hotkey-dispatch".into())
        .spawn(move || dispatch_thread(q))
        .expect("failed to spawn said-hotkey-dispatch thread");
}
