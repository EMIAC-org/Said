//! macOS CGEventTap hotkey listener.
//! `start_hold_listener` — fires `on_press` when the record key is held down, `on_release` when lifted.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordHotkey {
    CapsLock,
    RightOption,
    Function,
}

#[cfg(target_os = "macos")]
mod imp {
    use core_foundation::runloop::kCFRunLoopCommonModes;
    use std::sync::Arc;
    use std::time::Instant;

    mod ffi {
        use std::ffi::c_void;

        pub type CGEventRef = *mut c_void;
        pub type CGEventTapProxy = *mut c_void;
        pub type CFMachPortRef = *mut c_void;

        pub type CGEventTapCallBack = unsafe extern "C" fn(
            proxy: CGEventTapProxy,
            event_type: u32,
            event: CGEventRef,
            user_info: *mut c_void,
        ) -> CGEventRef;

        pub const K_CG_KEYBOARD_EVENT_KEYCODE: u32 = 9;
        pub const K_CG_EVENT_KEY_DOWN: u32 = 10;
        pub const K_CG_EVENT_LEFT_MOUSE_DOWN: u32 = 1;
        pub const K_CG_EVENT_RIGHT_MOUSE_DOWN: u32 = 3;
        pub const K_CG_EVENT_FLAGS_CHANGED: u32 = 12;
        pub const K_CG_EVENT_TAP_DISABLED_BY_TIMEOUT: u32 = 0xFFFFFFFE;
        pub const K_CG_EVENT_TAP_DISABLED_BY_USER_INPUT: u32 = 0xFFFFFFFF;

        /// kCGEventFlagMaskAlphaShift — set when Caps Lock is physically held
        pub const K_CG_FLAG_CAPS_LOCK: u64 = 0x0001_0000;
        /// kCGEventFlagMaskShift
        pub const K_CG_FLAG_SHIFT: u64 = 0x0002_0000;
        /// kCGEventFlagMaskControl
        pub const K_CG_FLAG_CONTROL: u64 = 0x0004_0000;
        /// kCGEventFlagMaskAlternate — Option key
        pub const K_CG_FLAG_ALT: u64 = 0x0008_0000;
        /// kCGEventFlagMaskCommand
        pub const K_CG_FLAG_COMMAND: u64 = 0x0010_0000;
        /// kCGEventFlagMaskSecondaryFn — Fn / Globe key
        pub const K_CG_FLAG_SECONDARY_FN: u64 = 0x0080_0000;
        /// Device-dependent left/right option bits from IOLLEvent.h
        pub const NX_DEVICELALTKEYMASK: u64 = 0x0000_0020;
        pub const NX_DEVICERALTKEYMASK: u64 = 0x0000_0040;

        // macOS virtual key codes for the number row
        pub const KC_1: i64 = 18;
        pub const KC_2: i64 = 19;
        pub const KC_3: i64 = 20;
        pub const KC_4: i64 = 21;
        pub const KC_5: i64 = 23;

        // macOS virtual key codes for special keys
        pub const KC_BACKSPACE: i64 = 51;
        pub const KC_DELETE: i64 = 117;
        pub const KC_LEFT: i64 = 123;
        pub const KC_RIGHT: i64 = 124;
        pub const KC_UP: i64 = 126;
        pub const KC_DOWN: i64 = 125;
        pub const KC_HOME: i64 = 115;
        pub const KC_END: i64 = 119;
        pub const KC_A: i64 = 0; // Cmd+A = select-all
        pub const KC_V: i64 = 9; // Ctrl+Cmd+V = paste-latest
        pub const KC_X: i64 = 7; // Cmd+X = cut
        pub const KC_Z: i64 = 6; // Cmd+Z = undo
        pub const KC_LEFT_OPTION: i64 = 58;
        pub const KC_RIGHT_OPTION: i64 = 61;
        pub const KC_FUNCTION: i64 = 63;

        unsafe extern "C" {
            pub fn CGEventTapCreate(
                tap: u32,
                place: u32,
                options: u32,
                events_of_interest: u64,
                callback: CGEventTapCallBack,
                user_info: *mut c_void,
            ) -> CFMachPortRef;

            pub fn CFMachPortCreateRunLoopSource(
                allocator: *const c_void,
                port: CFMachPortRef,
                order: i64,
            ) -> *mut c_void;

            pub fn CGEventGetIntegerValueField(event: CGEventRef, field: u32) -> i64;
            pub fn CGEventGetFlags(event: CGEventRef) -> u64;
            pub fn CGEventTapIsEnabled(tap: CFMachPortRef) -> bool;
            pub fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);

            pub fn CFRunLoopAddSource(rl: *mut c_void, source: *mut c_void, mode: *const c_void);
            pub fn CFRunLoopGetCurrent() -> *mut c_void;
            pub fn CFRunLoopRun();
            pub fn CFRelease(cf: *mut c_void);

            /// Read the Unicode character(s) produced by a keyboard event.
            pub fn CGEventKeyboardGetUnicodeString(
                event: CGEventRef,
                max_string_len: u32,
                actual_count: *mut u32,
                unicode_string: *mut u16,
            );

            /// macOS 10.15+ — returns true if this process has Input Monitoring TCC permission.
            /// This is the authoritative API; does NOT create any tap, no false positives.
            pub fn CGPreflightListenEventAccess() -> bool;

            /// macOS 10.15+ — prompt the user to grant Input Monitoring TCC permission.
            /// Shows the system dialog; returns true if permission is already granted.
            pub fn CGRequestListenEventAccess() -> bool;
        }
    }

    const CAPS_LOCK_KEYCODE: i64 = 57;
    const DEBOUNCE_MS: u128 = 300;

    // ── Keystroke buffer (for edit detection in AX-blind apps) ────────────────

    /// A compact, clonable keystroke event routed from the CGEventTap.
    #[derive(Clone, Debug)]
    pub enum KeyEvt {
        Char(char),    // printable character produced by this keypress
        Backspace,     // delete char before cursor (plain)
        Delete,        // delete char after cursor (plain)
        Left,          // move cursor one char left
        Right,         // move cursor one char right
        Home,          // jump to line/text start
        End,           // jump to line/text end
        WordLeft,      // Option+Left  — jump to previous word start
        WordRight,     // Option+Right — jump to next word end
        LineStart,     // Cmd+Left     — jump to line start
        LineEnd,       // Cmd+Right    — jump to line end
        WordBackspace, // Option+Backspace — delete word before cursor
        LineBackspace, // Cmd+Backspace    — delete to line start
        SelectAll,     // Cmd+A
        Cut,           // Cmd+X — marks reconstruction uncertain
        Undo,          // Cmd+Z — marks reconstruction uncertain
        MouseClick,    // mouse repositioned cursor — uncertain
        Other,         // ignored
    }

    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
    use std::sync::{Mutex, OnceLock};

    use crate::RecordHotkey;

    /// Timestamped key event stored in the ring buffer.
    pub struct TimedKeyEvt {
        pub when: Instant,
        pub evt: KeyEvt,
    }

    /// Global rolling buffer of the last ~2000 key events.
    static KEY_BUF: OnceLock<Arc<Mutex<VecDeque<TimedKeyEvt>>>> = OnceLock::new();

    fn key_buf() -> &'static Arc<Mutex<VecDeque<TimedKeyEvt>>> {
        KEY_BUF.get_or_init(|| Arc::new(Mutex::new(VecDeque::with_capacity(2048))))
    }

    /// Returns a reference to the global key event buffer.
    /// `watch_for_edit` notes `Instant::now()` before watching, then drains
    /// events timestamped after that instant.
    pub fn key_buffer() -> Arc<Mutex<VecDeque<TimedKeyEvt>>> {
        Arc::clone(key_buf())
    }

    // ── Option+1..5 tone shortcuts ────────────────────────────────────────────

    static SHORTCUT_CB: OnceLock<Arc<dyn Fn(u8) + Send + Sync>> = OnceLock::new();
    static RECORD_HOTKEY: AtomicU8 = AtomicU8::new(0);

    fn current_record_hotkey() -> RecordHotkey {
        match RECORD_HOTKEY.load(Ordering::Relaxed) {
            1 => RecordHotkey::RightOption,
            2 => RecordHotkey::Function,
            _ => RecordHotkey::CapsLock,
        }
    }

    pub fn set_record_hotkey(hotkey: RecordHotkey) {
        let encoded = match hotkey {
            RecordHotkey::CapsLock => 0,
            RecordHotkey::RightOption => 1,
            RecordHotkey::Function => 2,
        };
        RECORD_HOTKEY.store(encoded, Ordering::Relaxed);
        tracing::info!("[hotkey] record hotkey set to {:?}", hotkey);
    }

    /// Register a callback invoked when the user presses Option+1 through Option+5.
    /// The callback receives the digit (1–5). Must be called before the tap starts.
    pub fn register_shortcut_callback(cb: Arc<dyn Fn(u8) + Send + Sync>) {
        let _ = SHORTCUT_CB.set(cb);
    }

    /// Called inside a kCGEventKeyDown handler. Returns `true` if the event was
    /// an Option+1..5 shortcut (caller should return null to suppress the event).
    unsafe fn check_and_fire_shortcut(event: ffi::CGEventRef) -> bool {
        let flags = unsafe { ffi::CGEventGetFlags(event) };
        let alt = (flags & ffi::K_CG_FLAG_ALT) != 0;
        let left_alt = (flags & ffi::NX_DEVICELALTKEYMASK) != 0;
        let right_alt = (flags & ffi::NX_DEVICERALTKEYMASK) != 0;
        let cmd = (flags & ffi::K_CG_FLAG_COMMAND) != 0;
        let shift = (flags & ffi::K_CG_FLAG_SHIFT) != 0;
        let ctrl = (flags & ffi::K_CG_FLAG_CONTROL) != 0;
        let kc =
            unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_KEYBOARD_EVENT_KEYCODE) };

        if alt {
            tracing::trace!(
                "[hotkey] option keydown kc={kc} flags={flags:#010x} alt={alt} cmd={cmd} shift={shift} ctrl={ctrl}"
            );
        }

        // Only fire on bare Option (no other modifiers)
        if !alt || cmd || shift || ctrl {
            if alt {
                tracing::info!(
                    "[hotkey] option+{kc} rejected — other modifier held cmd={cmd} shift={shift} ctrl={ctrl}"
                );
            }
            return false;
        }

        if matches!(current_record_hotkey(), RecordHotkey::RightOption) {
            if !left_alt || right_alt {
                tracing::info!(
                    "[hotkey] option+{kc} ignored — Right Option reserved for hold-to-record, left_alt={left_alt} right_alt={right_alt}"
                );
                return false;
            }
        }

        let digit: Option<u8> = match kc {
            18 => Some(1),
            19 => Some(2),
            20 => Some(3),
            21 => Some(4),
            23 => Some(5),
            _ => None,
        };

        if let Some(n) = digit {
            tracing::info!("[hotkey] Option+{n} fired — calling tray polish callback");
            if let Some(cb) = SHORTCUT_CB.get() {
                cb(n);
            } else {
                tracing::warn!("[hotkey] Option+{n} fired but SHORTCUT_CB not registered!");
            }
            true
        } else {
            tracing::debug!("[hotkey] option+{kc} — not a tone shortcut key (bare Option)");
            false
        }
    }

    // ── Ctrl+Cmd+V — paste latest result ─────────────────────────────────────

    /// Callback fired when the user presses Ctrl+Cmd+V (our app-unique paste hotkey).
    static PASTE_CB: OnceLock<Arc<dyn Fn() + Send + Sync>> = OnceLock::new();

    /// Register a callback invoked when Ctrl+Cmd+V is pressed.
    /// The callback should paste whatever the app has stored as the latest polished result.
    pub fn register_paste_callback(cb: Arc<dyn Fn() + Send + Sync>) {
        let _ = PASTE_CB.set(cb);
    }

    /// Called inside a kCGEventKeyDown handler. Returns `true` if the event was
    /// Ctrl+Cmd+V (caller should return null to suppress the system paste action).
    unsafe fn check_and_fire_paste(event: ffi::CGEventRef) -> bool {
        let flags = unsafe { ffi::CGEventGetFlags(event) };
        let ctrl = (flags & ffi::K_CG_FLAG_CONTROL) != 0;
        let cmd = (flags & ffi::K_CG_FLAG_COMMAND) != 0;
        let alt = (flags & ffi::K_CG_FLAG_ALT) != 0;
        let shift = (flags & ffi::K_CG_FLAG_SHIFT) != 0;
        let kc =
            unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_KEYBOARD_EVENT_KEYCODE) };

        if ctrl || cmd {
            tracing::trace!(
                "[hotkey] keydown flags={flags:#010x} ctrl={ctrl} cmd={cmd} alt={alt} shift={shift} kc={kc}"
            );
        }

        // Exactly Ctrl+Cmd, no other modifiers
        if !ctrl || !cmd || alt || shift {
            return false;
        }
        if kc != ffi::KC_V {
            return false;
        }

        tracing::info!("[hotkey] Ctrl+Cmd+V detected — firing paste callback");
        if let Some(cb) = PASTE_CB.get() {
            cb();
        } else {
            tracing::warn!("[hotkey] Ctrl+Cmd+V fired but PASTE_CB not registered!");
        }
        true
    }

    // ── Input Monitoring permission tracking ──────────────────────────────────

    /// Set to true when CGEventTapCreate succeeds (Input Monitoring is granted).
    static TAP_CREATED: AtomicBool = AtomicBool::new(false);

    /// Stored hold-listener callbacks — saved so we can retry after permission grant.
    static HOLD_CALLBACKS: OnceLock<(Arc<dyn Fn() + Send + Sync>, Arc<dyn Fn() + Send + Sync>)> =
        OnceLock::new();

    /// Debounce: epoch-ms of last live permission check.
    static LAST_IM_CHECK_MS: AtomicU64 = AtomicU64::new(0);

    /// Last authoritative Input Monitoring result. Debounced snapshot reads
    /// must return this instead of manufacturing a temporary denial.
    static LAST_IM_GRANTED: AtomicBool = AtomicBool::new(false);

    /// Returns true if Input Monitoring TCC permission is granted.
    ///
    /// Uses `CGPreflightListenEventAccess()` — the authoritative macOS 10.15+ API.
    /// Never creates a tap, so it has zero false-positives even on macOS versions
    /// where `CGEventTapCreate` would succeed without real permission.
    ///
    /// Fast path: once our real tap has been created (TAP_CREATED flag), skip the
    /// syscall entirely — the permission can only be revoked with an app restart.
    /// Slow path: calls the API at most every 2 s (debounce).  When permission
    /// transitions from denied → granted after launch, automatically restarts the
    /// hold listener using the stored callbacks — but ONLY if `start_hold_listener`
    /// was already called and its initial tap creation failed.
    pub fn is_input_monitoring_granted() -> bool {
        // Fast path — tap is already running, permission can't be revoked at runtime.
        if TAP_CREATED.load(Ordering::Relaxed) {
            return true;
        }

        // Debounce: authoritative check at most every 2 seconds.
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let last = LAST_IM_CHECK_MS.load(Ordering::Relaxed);
        if now_ms.saturating_sub(last) < 2_000 {
            return LAST_IM_GRANTED.load(Ordering::Relaxed);
        }
        LAST_IM_CHECK_MS.store(now_ms, Ordering::Relaxed);

        // CGPreflightListenEventAccess() is the correct TCC API — no tap created,
        // no false positives, works on macOS 10.15+.
        let granted = unsafe { ffi::CGPreflightListenEventAccess() };
        LAST_IM_GRANTED.store(granted, Ordering::Relaxed);
        if !granted {
            return false;
        }

        // Permission IS granted.  But only restart the listener if
        // start_hold_listener was already called (HOLD_CALLBACKS is set) AND the
        // tap it spawned failed (TAP_CREATED is still false).  If the initial
        // start_hold_listener hasn't been called yet, just report the status —
        // don't spawn a tap, because start_hold_listener will do that itself.
        if HOLD_CALLBACKS.get().is_some() {
            // start_hold_listener was called earlier but its tap creation failed.
            // Now permission is granted — spawn a new tap.
            TAP_CREATED.store(true, Ordering::Relaxed);
            LAST_IM_GRANTED.store(true, Ordering::Relaxed);
            tracing::info!("[hotkey] Input Monitoring newly granted — restarting hold listener");

            let (on_press, on_release) = HOLD_CALLBACKS.get().unwrap();
            let p = Arc::clone(on_press);
            let r = Arc::clone(on_release);
            std::thread::spawn(move || unsafe {
                HOLD_STATE = Some(HoldState {
                    on_press: p,
                    on_release: r,
                    is_down: false,
                });
                run_tap(hold_tap_callback, &raw mut HOLD_TAP);
            });
        }
        true
    }

    /// Push a key event into the ring buffer (called from the CGEventTap callback).
    fn push_key(evt: KeyEvt) {
        let buf = key_buf();
        if let Ok(mut g) = buf.lock() {
            if g.len() >= 2048 {
                g.pop_front();
            }
            g.push_back(TimedKeyEvt {
                when: Instant::now(),
                evt,
            });
        }
    }

    /// Classify a kCGEventKeyDown event and push to the key buffer.
    unsafe fn handle_key_down(event: ffi::CGEventRef) {
        let keycode =
            unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_KEYBOARD_EVENT_KEYCODE) };
        let flags = unsafe { ffi::CGEventGetFlags(event) };
        let cmd = (flags & ffi::K_CG_FLAG_COMMAND) != 0;
        let alt = (flags & ffi::K_CG_FLAG_ALT) != 0;

        let evt = if cmd && alt {
            // Cmd+Option combos — treat as uncertain / ignore
            KeyEvt::Other
        } else if cmd {
            match keycode {
                ffi::KC_A => KeyEvt::SelectAll,
                ffi::KC_X => KeyEvt::Cut,
                ffi::KC_Z => KeyEvt::Undo,
                ffi::KC_LEFT => KeyEvt::LineStart,
                ffi::KC_RIGHT => KeyEvt::LineEnd,
                ffi::KC_UP => KeyEvt::Home,  // Cmd+Up → document start
                ffi::KC_DOWN => KeyEvt::End, // Cmd+Down → document end
                ffi::KC_BACKSPACE => KeyEvt::LineBackspace,
                _ => KeyEvt::Other,
            }
        } else if alt {
            match keycode {
                ffi::KC_LEFT => KeyEvt::WordLeft,
                ffi::KC_RIGHT => KeyEvt::WordRight,
                ffi::KC_BACKSPACE => KeyEvt::WordBackspace,
                _ => KeyEvt::Other,
            }
        } else {
            match keycode {
                ffi::KC_BACKSPACE => KeyEvt::Backspace,
                ffi::KC_DELETE => KeyEvt::Delete,
                ffi::KC_LEFT => KeyEvt::Left,
                ffi::KC_RIGHT => KeyEvt::Right,
                ffi::KC_UP => KeyEvt::Home,
                ffi::KC_DOWN => KeyEvt::End,
                ffi::KC_HOME => KeyEvt::Home,
                ffi::KC_END => KeyEvt::End,
                _ => {
                    // Read the Unicode character produced by this keypress.
                    let mut buf = [0u16; 4];
                    let mut n: u32 = 0;
                    unsafe {
                        ffi::CGEventKeyboardGetUnicodeString(event, 4, &mut n, buf.as_mut_ptr())
                    };
                    let s = String::from_utf16_lossy(&buf[..n as usize]);
                    let mut chars = s.chars();
                    match (chars.next(), chars.next()) {
                        (Some(c), None) if !c.is_control() => KeyEvt::Char(c),
                        _ => KeyEvt::Other,
                    }
                }
            }
        };
        push_key(evt);
    }

    // ── Hold-to-record listener ───────────────────────────────────────────────

    struct HoldState {
        on_press: Arc<dyn Fn() + Send + Sync>,
        on_release: Arc<dyn Fn() + Send + Sync>,
        is_down: bool,
    }

    static mut HOLD_STATE: Option<HoldState> = None;
    static mut HOLD_TAP: ffi::CFMachPortRef = std::ptr::null_mut();

    unsafe extern "C" fn hold_tap_callback(
        _proxy: ffi::CGEventTapProxy,
        event_type: u32,
        event: ffi::CGEventRef,
        _user_info: *mut std::ffi::c_void,
    ) -> ffi::CGEventRef {
        unsafe {
            rearm_if_disabled(event_type, HOLD_TAP);

            if event_type == ffi::K_CG_EVENT_KEY_DOWN {
                // Log keycode + flags for every keydown so we can confirm events arrive
                let _kc = ffi::CGEventGetIntegerValueField(event, ffi::K_CG_KEYBOARD_EVENT_KEYCODE);
                let _fl = ffi::CGEventGetFlags(event);
                tracing::trace!("[hotkey] HOLD tap keydown kc={_kc} flags={_fl:#010x}");

                if check_and_fire_paste(event) {
                    return std::ptr::null_mut(); // suppress Ctrl+Cmd+V system action
                }
                if check_and_fire_shortcut(event) {
                    return std::ptr::null_mut(); // suppress Option+N so it doesn't type a character
                }
                handle_key_down(event);
                return event;
            }
            if event_type == ffi::K_CG_EVENT_LEFT_MOUSE_DOWN
                || event_type == ffi::K_CG_EVENT_RIGHT_MOUSE_DOWN
            {
                push_key(KeyEvt::MouseClick);
                return event;
            }

            if event_type != ffi::K_CG_EVENT_FLAGS_CHANGED {
                return event;
            }

            let keycode = ffi::CGEventGetIntegerValueField(event, ffi::K_CG_KEYBOARD_EVENT_KEYCODE);
            let flags = ffi::CGEventGetFlags(event);

            if let Some(ref mut s) = HOLD_STATE {
                match current_record_hotkey() {
                    RecordHotkey::CapsLock => {
                        let caps_on = (flags & ffi::K_CG_FLAG_CAPS_LOCK) != 0;
                        if keycode == CAPS_LOCK_KEYCODE {
                            if caps_on && !s.is_down {
                                s.is_down = true;
                                tracing::info!("[hotkey] Caps Lock held → start recording");
                                (s.on_press)();
                            } else if !caps_on && s.is_down {
                                s.is_down = false;
                                tracing::info!("[hotkey] Caps Lock released → process");
                                (s.on_release)();
                            }
                        }
                    }
                    RecordHotkey::RightOption => {
                        let right_alt_on = (flags & ffi::NX_DEVICERALTKEYMASK) != 0;
                        if keycode == ffi::KC_RIGHT_OPTION {
                            if right_alt_on && !s.is_down {
                                s.is_down = true;
                                tracing::info!("[hotkey] Right Option held → start recording");
                                (s.on_press)();
                            } else if !right_alt_on && s.is_down {
                                s.is_down = false;
                                tracing::info!("[hotkey] Right Option released → process");
                                (s.on_release)();
                            }
                        }
                    }
                    RecordHotkey::Function => {
                        let fn_on = (flags & ffi::K_CG_FLAG_SECONDARY_FN) != 0;
                        if keycode == ffi::KC_FUNCTION {
                            if fn_on && !s.is_down {
                                s.is_down = true;
                                tracing::info!("[hotkey] Fn held → start recording");
                                (s.on_press)();
                            } else if !fn_on && s.is_down {
                                s.is_down = false;
                                tracing::info!("[hotkey] Fn released → process");
                                (s.on_release)();
                            }
                        }
                    }
                }
            }
            event
        }
    }

    /// Hold-to-record: `on_press` when key goes down, `on_release` when lifted.
    /// Both callbacks run on a background CFRunLoop thread — do blocking work in spawned threads.
    pub fn start_hold_listener(
        on_press: Arc<dyn Fn() + Send + Sync>,
        on_release: Arc<dyn Fn() + Send + Sync>,
    ) {
        // Save callbacks so we can restart the listener if permission is granted later.
        let _ = HOLD_CALLBACKS.set((Arc::clone(&on_press), Arc::clone(&on_release)));

        std::thread::spawn(move || {
            // SAFETY: called once from a dedicated thread; no concurrent access
            unsafe {
                HOLD_STATE = Some(HoldState {
                    on_press,
                    on_release,
                    is_down: false,
                });
                run_tap(hold_tap_callback, &raw mut HOLD_TAP);
            }
        });
    }

    // ── Shared tap helpers ────────────────────────────────────────────────────

    unsafe fn rearm_if_disabled(event_type: u32, tap: ffi::CFMachPortRef) {
        if event_type == ffi::K_CG_EVENT_TAP_DISABLED_BY_TIMEOUT
            || event_type == ffi::K_CG_EVENT_TAP_DISABLED_BY_USER_INPUT
        {
            if !tap.is_null() {
                // SAFETY: tap is a valid CFMachPortRef checked above
                unsafe { ffi::CGEventTapEnable(tap, true) };
                tracing::info!("[hotkey] tap re-enabled after macOS disabled it");
            }
        }
    }

    unsafe fn run_tap(callback: ffi::CGEventTapCallBack, tap_ptr: *mut ffi::CFMachPortRef) {
        let mask: u64 = (1u64 << ffi::K_CG_EVENT_FLAGS_CHANGED)
            | (1u64 << ffi::K_CG_EVENT_KEY_DOWN)
            | (1u64 << ffi::K_CG_EVENT_LEFT_MOUSE_DOWN)
            | (1u64 << ffi::K_CG_EVENT_RIGHT_MOUSE_DOWN);
        // SAFETY: CGEventTapCreate is called with valid arguments; null user_info is allowed
        let tap = unsafe { ffi::CGEventTapCreate(0, 0, 0, mask, callback, std::ptr::null_mut()) };

        if tap.is_null() {
            tracing::info!(
                "[hotkey] CGEventTapCreate failed — requesting Input Monitoring permission"
            );
            // Trigger the macOS TCC permission dialog for Input Monitoring.
            // NSInputMonitoringUsageDescription in Info.plist is required for this to work.
            unsafe { ffi::CGRequestListenEventAccess() };
            tracing::info!(
                "[hotkey] Restart the app after granting Input Monitoring in System Settings."
            );
            return;
        }

        // Record that Input Monitoring permission was granted (tap created OK).
        TAP_CREATED.store(true, Ordering::Relaxed);

        // SAFETY: tap_ptr is a valid mutable pointer owned by the caller's static
        unsafe { *tap_ptr = tap };

        // SAFETY: tap is a valid non-null CFMachPortRef
        if unsafe { !ffi::CGEventTapIsEnabled(tap) } {
            tracing::info!(
                "[hotkey] tap disabled — grant Input Monitoring permission, then restart"
            );
        } else {
            tracing::info!("[hotkey] CGEventTap active — listening for hold hotkey");
        }

        // SAFETY: tap is valid; null allocator uses default CF allocator
        let source = unsafe { ffi::CFMachPortCreateRunLoopSource(std::ptr::null(), tap, 0) };
        if source.is_null() {
            tracing::info!("[hotkey] RunLoop source creation failed");
            return;
        }

        unsafe {
            ffi::CFRunLoopAddSource(
                ffi::CFRunLoopGetCurrent(),
                source,
                kCFRunLoopCommonModes as *const _ as *const std::ffi::c_void,
            );
            ffi::CFRunLoopRun();
        }
    }
}

#[cfg(target_os = "macos")]
pub use imp::*;

#[cfg(not(target_os = "macos"))]
pub fn set_record_hotkey(_hotkey: RecordHotkey) {}

#[cfg(not(target_os = "macos"))]
pub fn is_input_monitoring_granted() -> bool {
    false
}
