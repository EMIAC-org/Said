//! Clipboard-and-paste helper.
//! - macOS: CGEvent to synthesise Cmd+V (requires Accessibility).
//! - Windows: `SendInput(KEYEVENTF_UNICODE)` for streaming typing + `OpenClipboard`/
//!   `SetClipboardData`/Ctrl+V for paste. AX-tree reads return `None` for v1
//!   (UIAutomation port is a follow-up).
//! - Other: clipboard-only stubs.

// Pure logic (UTF-16 encoding, surrogate pairs, CRLF translation) — compiled
// on every host so its tests run on macOS dev boxes and every CI runner.
pub mod win_paster;

#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::{CStr, CString, c_void};
    use std::io::Write;
    use std::process::Command;
    use std::thread;
    use std::time::Duration;

    const KEY_V: u16 = 9;
    const KEY_A: u16 = 0; // kVK_ANSI_A
    const KEY_C: u16 = 8; // kVK_ANSI_C
    const KEY_DELETE: u16 = 51; // kVK_Delete (backspace)
    const KEY_RIGHT: u16 = 124; // kVK_RightArrow
    const KEY_CMD: u16 = 55;

    const K_CG_HID_EVENT_TAP: u32 = 0;
    const K_CG_FLAG_COMMAND: u64 = 1 << 20;
    const K_CG_EVENT_SOURCE_STATE_COMBINED_SESSION: u32 = 1;

    /// kCFStringEncodingUTF8
    const CF_UTF8: u32 = 0x0800_0100;

    mod ffi {
        use std::ffi::c_void;
        unsafe extern "C" {
            // ── CG / paste ──────────────────────────────────────────────────────
            pub fn CGEventSourceCreate(state_id: u32) -> *mut c_void;
            pub fn CGEventCreateKeyboardEvent(
                source: *const c_void,
                keycode: u16,
                key_down: bool,
            ) -> *mut c_void;
            pub fn CGEventSetFlags(event: *mut c_void, flags: u64);
            pub fn CGEventPost(tap: u32, event: *mut c_void);
            /// Type arbitrary Unicode text via a synthetic keyboard event.
            /// `string_length` is the number of UTF-16 code units; `unicode_string`
            /// points to the array.  Works for any script including Devanagari.
            pub fn CGEventKeyboardSetUnicodeString(
                event: *mut c_void,
                string_length: u64,
                unicode_string: *const u16,
            );
            pub fn CFRelease(cf: *mut c_void);
            pub fn AXIsProcessTrusted() -> bool;
            pub fn AXIsProcessTrustedWithOptions(options: *const std::ffi::c_void) -> bool;

            // ── CoreFoundation strings ─────────────────────────────────────────
            pub fn CFStringCreateWithCString(
                alloc: *const c_void,
                c_str: *const i8,
                encoding: u32,
            ) -> *mut c_void;
            pub fn CFStringGetCString(
                the_string: *const c_void,
                buffer: *mut i8,
                buffer_size: i64,
                encoding: u32,
            ) -> bool;
            pub fn CFStringGetLength(the_string: *const c_void) -> i64;
            pub fn CFGetTypeID(cf: *const c_void) -> usize;
            pub fn CFStringGetTypeID() -> usize;

            // ── CoreFoundation booleans ────────────────────────────────────────
            /// kCFBooleanTrue — the canonical CF true value.
            /// Declared as a static pointer so we can pass it to AX set functions.
            pub static kCFBooleanTrue: *const c_void;
            pub static kCFTypeDictionaryKeyCallBacks: c_void;
            pub static kCFTypeDictionaryValueCallBacks: c_void;

            pub fn CFDictionaryCreate(
                allocator: *const c_void,
                keys: *const *const c_void,
                values: *const *const c_void,
                num_values: isize,
                key_callbacks: *const c_void,
                value_callbacks: *const c_void,
            ) -> *mut c_void;

            // ── CFNumber / CFArray (for diagnostics) ──────────────────────────
            pub fn CFNumberGetValue(
                number: *const c_void,
                the_type: i32,
                value_ptr: *mut c_void,
            ) -> bool;
            pub fn CFArrayGetCount(array: *const c_void) -> i64;
            pub fn CFArrayGetValueAtIndex(array: *const c_void, idx: i64) -> *const c_void;

            // ── Accessibility ─────────────────────────────────────────────────
            pub fn AXUIElementCreateSystemWide() -> *mut c_void;
            pub fn AXUIElementCreateApplication(pid: i32) -> *mut c_void;
            pub fn AXUIElementCopyAttributeValue(
                element: *const c_void,
                attribute: *const c_void,
                value: *mut *mut c_void,
            ) -> i32;
            /// Write an attribute on an AX element.
            /// Used to enable accessibility in Chrome (AXEnhancedUserInterface)
            /// and Electron (AXManualAccessibility).
            pub fn AXUIElementSetAttributeValue(
                element: *const c_void,
                attribute: *const c_void,
                value: *const c_void,
            ) -> i32;
            pub fn AXUIElementGetPid(element: *const c_void, pid: *mut i32) -> i32;
            pub fn AXUIElementCopyAttributeNames(
                element: *const c_void,
                out: *mut *mut c_void,
            ) -> i32;
            pub fn AXUIElementCopyParameterizedAttributeValue(
                element: *const c_void,
                attr: *const c_void,
                param: *const c_void,
                out: *mut *mut c_void,
            ) -> i32;
            pub fn AXValueCreate(the_type: u32, value_ptr: *const c_void) -> *mut c_void;

            pub fn CGPreflightListenEventAccess() -> bool;
            pub fn CGRequestListenEventAccess() -> bool;
        }
    }

    // CFRange struct for AXStringForRange
    #[repr(C)]
    struct CFRange {
        location: i64,
        length: i64,
    }
    const K_AX_VALUE_CF_RANGE_TYPE: u32 = 4;

    // ── Internal AX helpers ───────────────────────────────────────────────────

    /// Wrap a &str as a CFString (caller must CFRelease).
    unsafe fn cf_str(s: &str) -> *mut c_void {
        let cs = CString::new(s).unwrap_or_default();
        unsafe { ffi::CFStringCreateWithCString(std::ptr::null(), cs.as_ptr(), CF_UTF8) }
    }

    /// Convert a CFString to a Rust String (does NOT release).
    unsafe fn cfstring_to_rust(cf: *const c_void) -> Option<String> {
        if cf.is_null() {
            return None;
        }
        // Guard: only process CFString objects
        if unsafe { ffi::CFGetTypeID(cf) != ffi::CFStringGetTypeID() } {
            return None;
        }
        let char_len = unsafe { ffi::CFStringGetLength(cf) };
        if char_len < 0 {
            return None;
        }
        // Allocate: up to 4 bytes per UTF-16 code unit + NUL
        let buf_size = (char_len * 4 + 1) as usize;
        let mut buf: Vec<i8> = vec![0; buf_size];
        if unsafe { ffi::CFStringGetCString(cf, buf.as_mut_ptr(), buf_size as i64, CF_UTF8) } {
            Some(
                unsafe { CStr::from_ptr(buf.as_ptr()) }
                    .to_string_lossy()
                    .into_owned(),
            )
        } else {
            None
        }
    }

    /// Copy an AX attribute value. Returns an owned CF object (caller must CFRelease).
    unsafe fn ax_attr(element: *const c_void, attr: &str) -> Option<*mut c_void> {
        let key = unsafe { cf_str(attr) };
        // cf_str returns null if CFString allocation fails (OOM). CFRelease(null)
        // is undefined behavior, and the AX call needs a valid key — bail out.
        if key.is_null() {
            return None;
        }
        let mut value: *mut c_void = std::ptr::null_mut();
        let err = unsafe { ffi::AXUIElementCopyAttributeValue(element, key, &mut value) };
        unsafe { ffi::CFRelease(key) };
        if err == 0 && !value.is_null() {
            Some(value)
        } else {
            None
        }
    }

    /// Set an AX boolean attribute on an element (returns the AX error code).
    unsafe fn ax_set_bool(element: *const c_void, attr: &str) -> i32 {
        let key = unsafe { cf_str(attr) };
        // cf_str returns null if CFString allocation fails (OOM). CFRelease(null)
        // is undefined behavior — return a non-zero (failure) AX code instead.
        // Callers only treat 0 as success, so -1 reads as a benign failure.
        if key.is_null() {
            return -1;
        }
        // SAFETY: kCFBooleanTrue is a valid CFTypeRef (CFBooleanRef is toll-free bridged)
        let err = unsafe { ffi::AXUIElementSetAttributeValue(element, key, ffi::kCFBooleanTrue) };
        unsafe { ffi::CFRelease(key) };
        err
    }

    /// Try to unlock the AX tree for Chrome / Electron apps.
    ///
    /// Chrome uses `AXEnhancedUserInterface` (what VoiceOver sets when it activates).
    /// Electron uses `AXManualAccessibility`.
    /// Both are public attributes — setting them is the documented way to enable
    /// the accessibility tree in those runtimes.
    ///
    /// Returns true if at least one attribute was accepted (error == 0).
    unsafe fn ax_enable_ui(app: *const c_void) -> bool {
        let r1 = unsafe { ax_set_bool(app, "AXEnhancedUserInterface") };
        let r2 = unsafe { ax_set_bool(app, "AXManualAccessibility") };
        r1 == 0 || r2 == 0
    }

    fn frontmost_pid() -> Option<i32> {
        unsafe {
            use objc::runtime::{Class, Object};
            use objc::{msg_send, sel, sel_impl};

            let cls = Class::get("NSWorkspace")?;
            let workspace: *mut Object = msg_send![cls, sharedWorkspace];
            if workspace.is_null() {
                return None;
            }

            let app: *mut Object = msg_send![workspace, frontmostApplication];
            if app.is_null() {
                return None;
            }

            let pid: i32 = msg_send![app, processIdentifier];
            if pid > 0 { Some(pid) } else { None }
        }
    }

    fn unlock_app_by_pid(pid: i32) -> Option<i32> {
        if pid <= 0 {
            return None;
        }
        unsafe {
            if !ffi::AXIsProcessTrusted() {
                return None;
            }
            let app = ffi::AXUIElementCreateApplication(pid);
            if app.is_null() {
                return None;
            }
            ax_set_bool(app as *const _, "AXEnhancedUserInterface");
            ax_set_bool(app as *const _, "AXManualAccessibility");
            ffi::CFRelease(app);
            Some(pid)
        }
    }

    // ── Public AX pre-unlock ──────────────────────────────────────────────────

    /// Pre-unlock the accessibility tree for whatever app is currently focused.
    ///
    /// Call this BEFORE recording starts so Chrome/Electron has ~2 seconds to
    /// build their full AX tree while the user is dictating.  By the time we
    /// paste and start watching for edits, `AXValue` reads will succeed.
    ///
    /// Chrome requires `AXEnhancedUserInterface = true` on the app element.
    /// Electron requires `AXManualAccessibility  = true` on the app element.
    ///
    /// We set both — they are idempotent and safe to set on any app.
    ///
    /// Returns the PID of the unlocked app, or None if AX is unavailable.
    pub fn unlock_focused_app_now() -> Option<i32> {
        unsafe {
            if !ffi::AXIsProcessTrusted() {
                return None;
            }

            let sys = ffi::AXUIElementCreateSystemWide();
            if sys.is_null() {
                return None;
            }

            let app = ax_attr(sys as *const _, "AXFocusedApplication");
            ffi::CFRelease(sys);
            let app = app?;

            let mut pid: i32 = -1;
            ffi::AXUIElementGetPid(app as *const _, &mut pid);

            // Unlock Chrome (AXEnhancedUserInterface) and Electron (AXManualAccessibility).
            // These are idempotent — safe to call on any app.
            ax_set_bool(app as *const _, "AXEnhancedUserInterface");
            ax_set_bool(app as *const _, "AXManualAccessibility");
            ffi::CFRelease(app);

            if pid > 0 { Some(pid) } else { None }
        }
    }

    /// Capture the current frontmost app PID and unlock its AX tree.
    ///
    /// This is the OpenWhispr-style target lock. `NSWorkspace` tells us which
    /// app is frontmost before our own UI can disturb focus; then AX is used
    /// only to enable/read that specific process.
    pub fn lock_frontmost_app_now() -> Option<i32> {
        frontmost_pid()
            .and_then(unlock_app_by_pid)
            .or_else(unlock_focused_app_now)
    }

    // ── Public AX surface ─────────────────────────────────────────────────────

    /// Read the value of the currently-focused text element.
    ///
    /// Strategy (each step falls through to the next on failure):
    ///  1. Try `AXValue` directly — works for all native macOS apps.
    ///  2. If that returns nil, try to unlock the AX tree for Chrome / Electron
    ///     via `AXEnhancedUserInterface` + `AXManualAccessibility`, then retry
    ///     `AXValue`.  Chrome exposes the full text of <textarea> and contentEditable
    ///     fields once the accessibility tree is enabled.
    ///  3. Fall back to `AXSelectedText` — returns only the currently selected text
    ///     but is exposed by some Electron apps even without tree unlock.
    ///
    /// Returns None if accessibility is not granted or nothing readable is focused.
    ///
    /// Fast path only: reads the immediate `AXValue` from the focused element.
    /// It intentionally does not unlock Chrome/Electron accessibility or sleep,
    /// so hot polling loops can call it without blocking executor workers.
    pub fn read_focused_value_fast() -> Option<String> {
        unsafe {
            if !ffi::AXIsProcessTrusted() {
                return None;
            }

            let sys = ffi::AXUIElementCreateSystemWide();
            if sys.is_null() {
                return None;
            }

            // ── Get focused application element ───────────────────────────────
            let app = ax_attr(sys as *const _, "AXFocusedApplication");
            ffi::CFRelease(sys);
            let app = app?;

            // ── Get focused UI element ────────────────────────────────────────
            let el = match ax_attr(app as *const _, "AXFocusedUIElement") {
                Some(e) => e,
                None => {
                    ffi::CFRelease(app);
                    return None;
                }
            };

            if let Some(val_cf) = ax_attr(el as *const _, "AXValue") {
                let result = cfstring_to_rust(val_cf as *const _);
                ffi::CFRelease(val_cf);
                ffi::CFRelease(el);
                ffi::CFRelease(app);
                if result.is_some() {
                    return result;
                }
            }

            ffi::CFRelease(el);
            ffi::CFRelease(app);
            None
        }
    }

    /// Full focused value read used for first reads and one-shot callers.
    ///
    /// This preserves the old behavior: direct `AXValue`, Chrome/Electron AX
    /// unlock plus 200 ms cache rebuild wait, `AXSelectedText`, then bounded
    /// subtree BFS.
    pub fn read_focused_value_first() -> Option<String> {
        unsafe {
            if !ffi::AXIsProcessTrusted() {
                return None;
            }

            let sys = ffi::AXUIElementCreateSystemWide();
            if sys.is_null() {
                return None;
            }

            // ── Get focused application element ───────────────────────────────
            let app = ax_attr(sys as *const _, "AXFocusedApplication");
            ffi::CFRelease(sys);
            let app = app?;

            // ── Get focused UI element ────────────────────────────────────────
            let el = match ax_attr(app as *const _, "AXFocusedUIElement") {
                Some(e) => e,
                None => {
                    ffi::CFRelease(app);
                    return None;
                }
            };

            // ── Step 1: try AXValue directly ──────────────────────────────────
            if let Some(val_cf) = ax_attr(el as *const _, "AXValue") {
                let result = cfstring_to_rust(val_cf as *const _);
                ffi::CFRelease(val_cf);
                ffi::CFRelease(el);
                ffi::CFRelease(app);
                if result.is_some() {
                    return result;
                }
            }

            // ── Step 2: unlock Chrome / Electron AX tree, retry AXValue ──────
            // Chrome: AXEnhancedUserInterface (what VoiceOver sets on activation)
            // Electron: AXManualAccessibility
            let _unlocked = ax_enable_ui(app as *const _);

            // Chrome needs ~150-200 ms to populate its accessibility cache after
            // AXEnhancedUserInterface is set.  80 ms was consistently too short.
            thread::sleep(Duration::from_millis(200));

            // Re-fetch the focused element — the tree may have rebuilt.
            ffi::CFRelease(el);
            let el2 = match ax_attr(app as *const _, "AXFocusedUIElement") {
                Some(e) => e,
                None => {
                    ffi::CFRelease(app);
                    return None;
                }
            };

            if let Some(val_cf) = ax_attr(el2 as *const _, "AXValue") {
                let result = cfstring_to_rust(val_cf as *const _);
                ffi::CFRelease(val_cf);
                ffi::CFRelease(el2);
                ffi::CFRelease(app);
                if result.is_some() {
                    return result;
                }
            }

            // ── Step 3: AXSelectedText — works in some Electron apps ──────────
            if let Some(sel_cf) = ax_attr(el2 as *const _, "AXSelectedText") {
                let result = cfstring_to_rust(sel_cf as *const _);
                ffi::CFRelease(sel_cf);
                if result.is_some() {
                    ffi::CFRelease(el2);
                    ffi::CFRelease(app);
                    return result;
                }
            }

            // ── Step 4: BFS the focused element's subtree for a text-bearing
            //     descendant.  In modern Electron apps (Claude, Linear, certain
            //     Lark builds) AXFocusedUIElement returns a wrapper (the
            //     WebView itself) rather than the actual contenteditable; the
            //     real text lives one or two layers deep.
            //
            //     We bound the walk hard: max 64 elements visited, max depth 4.
            //     That's enough for typical Chromium AX trees but stays cheap
            //     enough to run inside the 30 ms edit-watch poll.
            let deep = read_text_in_subtree(el2 as *const _, 64, 4);
            ffi::CFRelease(el2);
            ffi::CFRelease(app);
            deep
        }
    }

    /// Back-compat shim for one-shot callers that expect the full read path.
    pub fn read_focused_value() -> Option<String> {
        read_focused_value_first()
    }

    /// Fast path for a locked target app.
    ///
    /// Unlike `read_focused_value_fast`, this does not ask the system-wide AX
    /// object for the current frontmost application. It reads the focused text
    /// element inside the app identified by `pid`, matching OpenWhispr's
    /// "capture target PID first, monitor that app later" model.
    pub fn read_focused_value_fast_for_pid(pid: i32) -> Option<String> {
        if pid <= 0 {
            return None;
        }
        unsafe {
            if !ffi::AXIsProcessTrusted() {
                return None;
            }

            let app = ffi::AXUIElementCreateApplication(pid);
            if app.is_null() {
                return None;
            }

            let el = match ax_attr(app as *const _, "AXFocusedUIElement") {
                Some(e) => e,
                None => {
                    ffi::CFRelease(app);
                    return None;
                }
            };

            if let Some(val_cf) = ax_attr(el as *const _, "AXValue") {
                let result = cfstring_to_rust(val_cf as *const _);
                ffi::CFRelease(val_cf);
                ffi::CFRelease(el);
                ffi::CFRelease(app);
                if result.is_some() {
                    return result;
                }
            }

            ffi::CFRelease(el);
            ffi::CFRelease(app);
            None
        }
    }

    /// Full read path for a locked target app.
    ///
    /// This mirrors `read_focused_value_first`, but starts from
    /// `AXUIElementCreateApplication(pid)` instead of the current
    /// `AXFocusedApplication`. That lets the edit watcher keep reading the
    /// originally dictated-into app even if our HUD or another app changes
    /// system focus.
    pub fn read_focused_value_first_for_pid(pid: i32) -> Option<String> {
        if pid <= 0 {
            return None;
        }
        unsafe {
            if !ffi::AXIsProcessTrusted() {
                return None;
            }

            let app = ffi::AXUIElementCreateApplication(pid);
            if app.is_null() {
                return None;
            }

            let el = match ax_attr(app as *const _, "AXFocusedUIElement") {
                Some(e) => e,
                None => {
                    ffi::CFRelease(app);
                    return None;
                }
            };

            if let Some(val_cf) = ax_attr(el as *const _, "AXValue") {
                let result = cfstring_to_rust(val_cf as *const _);
                ffi::CFRelease(val_cf);
                ffi::CFRelease(el);
                ffi::CFRelease(app);
                if result.is_some() {
                    return result;
                }
            }

            let _unlocked = ax_enable_ui(app as *const _);
            thread::sleep(Duration::from_millis(200));

            ffi::CFRelease(el);
            let el2 = match ax_attr(app as *const _, "AXFocusedUIElement") {
                Some(e) => e,
                None => {
                    ffi::CFRelease(app);
                    return None;
                }
            };

            if let Some(val_cf) = ax_attr(el2 as *const _, "AXValue") {
                let result = cfstring_to_rust(val_cf as *const _);
                ffi::CFRelease(val_cf);
                ffi::CFRelease(el2);
                ffi::CFRelease(app);
                if result.is_some() {
                    return result;
                }
            }

            if let Some(sel_cf) = ax_attr(el2 as *const _, "AXSelectedText") {
                let result = cfstring_to_rust(sel_cf as *const _);
                ffi::CFRelease(sel_cf);
                if result.is_some() {
                    ffi::CFRelease(el2);
                    ffi::CFRelease(app);
                    return result;
                }
            }

            let deep = read_text_in_subtree(el2 as *const _, 64, 4);
            ffi::CFRelease(el2);
            ffi::CFRelease(app);
            deep
        }
    }

    /// BFS a UI element's descendants looking for a non-empty CFString-typed
    /// AXValue.  Returns the first one found (closest to the root).
    ///
    /// The AX tree of a Chromium WebView can be wide (lots of toolbar/header
    /// children).  We cap the visit count and depth so this is bounded and
    /// safe to call from a hot polling loop.
    unsafe fn read_text_in_subtree(
        root: *const c_void,
        max_visits: usize,
        max_depth: usize,
    ) -> Option<String> {
        use std::collections::VecDeque;
        // Queue of (element, depth).  We DO NOT take ownership of `root`'s
        // refcount — caller still owns it.  Children obtained from AXChildren
        // are owned by us and must be CFReleased after we finish.
        let mut queue: VecDeque<(*const c_void, usize)> = VecDeque::new();
        let mut owned_for_release: Vec<*mut c_void> = Vec::new();
        let mut visits: usize = 0;
        queue.push_back((root, 0));

        let result: Option<String> = loop {
            let Some((el, depth)) = queue.pop_front() else {
                break None;
            };
            visits += 1;
            if visits > max_visits {
                break None;
            }

            // Try AXValue on this element.
            if let Some(val_cf) = unsafe { ax_attr(el, "AXValue") } {
                let s = unsafe { cfstring_to_rust(val_cf as *const _) };
                unsafe { ffi::CFRelease(val_cf) };
                if let Some(text) = s {
                    if !text.is_empty() {
                        break Some(text);
                    }
                }
            }

            // Stop descending past the depth cap.
            if depth >= max_depth {
                continue;
            }

            // Enqueue AXChildren if any.
            let Some(children_cf) = (unsafe { ax_attr(el, "AXChildren") }) else {
                continue;
            };
            // children_cf is a CFArrayRef (or compatible).  Verify type? Skip
            // — AX always returns CFArray for AXChildren when present.
            let count = unsafe { ffi::CFArrayGetCount(children_cf as *const _) };
            for i in 0..count.min(16) {
                // cap fan-out per node
                let child = unsafe { ffi::CFArrayGetValueAtIndex(children_cf as *const _, i) };
                if !child.is_null() {
                    queue.push_back((child, depth + 1));
                }
            }
            // children_cf was returned from AXUIElementCopyAttributeValue —
            // we own it.  Track for release after the loop.
            owned_for_release.push(children_cf);
        };

        // Release every CFArray we copied.  The element pointers inside are
        // owned by their parent; we don't release individual children.
        for cf in owned_for_release {
            unsafe { ffi::CFRelease(cf) };
        }
        result
    }

    /// Fallback for AX-blind apps (Electron, Chrome, web views).
    ///
    /// When `AXValue` returns nil, simulate Cmd+A → Cmd+C to read the full
    /// text of the focused field through the clipboard.  The original clipboard
    /// is restored afterwards.
    ///
    /// Returns None if the capture fails or the field appears empty.
    pub fn capture_focused_text_via_selection() -> Option<String> {
        // Save current clipboard so we can restore it afterwards.
        let original = pbpaste();

        // Clear clipboard so we can detect whether the copy actually landed.
        pbcopy("");
        thread::sleep(Duration::from_millis(50));

        unsafe {
            if !ffi::AXIsProcessTrusted() {
                pbcopy(&original);
                return None;
            }

            let source = ffi::CGEventSourceCreate(K_CG_EVENT_SOURCE_STATE_COMBINED_SESSION);
            if source.is_null() {
                pbcopy(&original);
                return None;
            }

            // Cmd+A — select all text in the focused field
            post_key(source, KEY_A, true, K_CG_FLAG_COMMAND);
            thread::sleep(Duration::from_millis(40));
            post_key(source, KEY_A, false, K_CG_FLAG_COMMAND);
            thread::sleep(Duration::from_millis(60));

            // Cmd+C — copy the selected text
            post_key(source, KEY_C, true, K_CG_FLAG_COMMAND);
            thread::sleep(Duration::from_millis(40));
            post_key(source, KEY_C, false, K_CG_FLAG_COMMAND);
            thread::sleep(Duration::from_millis(60));

            ffi::CFRelease(source);
        }

        // Give the target app time to write to the clipboard.
        thread::sleep(Duration::from_millis(150));
        let captured = pbpaste();

        // Restore the clipboard to what it was before.
        pbcopy(&original);

        if captured.is_empty() {
            None
        } else {
            Some(captured)
        }
    }

    /// Read only the currently selected text from the focused field.
    ///
    /// Strategy:
    ///   1. Try `kAXSelectedTextAttribute` via AX — zero-latency, non-destructive.
    ///   2. Fall back to Cmd+C (captures selection without first doing Cmd+A),
    ///      restoring the original clipboard afterwards.
    ///
    /// Returns `None` if nothing is selected or accessibility is not granted.
    pub fn read_selected_text() -> Option<String> {
        unsafe {
            if !ffi::AXIsProcessTrusted() {
                tracing::warn!("[paster] read_selected_text: AX not trusted — returning None");
                return None;
            }

            // ── Try AX kAXSelectedTextAttribute first ─────────────────────────
            let sys = ffi::AXUIElementCreateSystemWide();
            if !sys.is_null() {
                let focused_app_val = ax_attr(sys as *const _, "AXFocusedApplication");
                ffi::CFRelease(sys);

                if let Some(app_elem) = focused_app_val {
                    let focused_elem = ax_attr(app_elem as *const _, "AXFocusedUIElement");
                    ffi::CFRelease(app_elem);

                    if let Some(elem) = focused_elem {
                        let selected = ax_attr(elem as *const _, "AXSelectedText");
                        ffi::CFRelease(elem);

                        if let Some(sel_val) = selected {
                            let s = cfstring_to_rust(sel_val as *const _);
                            ffi::CFRelease(sel_val);
                            if let Some(text) = s {
                                if !text.trim().is_empty() {
                                    tracing::info!(
                                        "[paster] read_selected_text: AXSelectedText returned {} chars",
                                        text.len()
                                    );
                                    return Some(text);
                                }
                                tracing::debug!(
                                    "[paster] read_selected_text: AXSelectedText returned empty string"
                                );
                            }
                        } else {
                            tracing::debug!(
                                "[paster] read_selected_text: AXSelectedText attribute not available"
                            );
                        }
                    } else {
                        tracing::debug!("[paster] read_selected_text: no AXFocusedUIElement");
                    }
                } else {
                    tracing::debug!("[paster] read_selected_text: no AXFocusedApplication");
                }
            }
        }

        // ── Fallback: Cmd+C to copy the current selection ─────────────────────
        tracing::info!("[paster] read_selected_text: AX path failed — trying Cmd+C fallback");
        let original = pbpaste();
        pbcopy("");
        thread::sleep(Duration::from_millis(50));

        unsafe {
            let source = ffi::CGEventSourceCreate(K_CG_EVENT_SOURCE_STATE_COMBINED_SESSION);
            if source.is_null() {
                tracing::warn!("[paster] read_selected_text: CGEventSourceCreate failed");
                pbcopy(&original);
                return None;
            }
            post_key(source, KEY_C, true, K_CG_FLAG_COMMAND);
            thread::sleep(Duration::from_millis(40));
            post_key(source, KEY_C, false, K_CG_FLAG_COMMAND);
            thread::sleep(Duration::from_millis(100));
            ffi::CFRelease(source);
        }

        thread::sleep(Duration::from_millis(150));
        let captured = pbpaste();
        pbcopy(&original);

        if captured.is_empty() {
            tracing::warn!("[paster] read_selected_text: Cmd+C fallback also returned empty");
            None
        } else {
            tracing::info!(
                "[paster] read_selected_text: Cmd+C fallback got {} chars",
                captured.len()
            );
            Some(captured)
        }
    }

    /// Return the PID of the focused application, or None.
    pub fn focused_pid() -> Option<i32> {
        unsafe {
            if !ffi::AXIsProcessTrusted() {
                return None;
            }

            let sys = ffi::AXUIElementCreateSystemWide();
            if sys.is_null() {
                return None;
            }

            let app = ax_attr(sys as *const _, "AXFocusedApplication");
            ffi::CFRelease(sys);
            let app = app?;

            let mut pid: i32 = -1;
            let err = ffi::AXUIElementGetPid(app as *const _, &mut pid);
            ffi::CFRelease(app);
            if err == 0 && pid > 0 { Some(pid) } else { None }
        }
    }

    /// Result of one read attempt — either successful text or the failure reason.
    #[derive(Debug, Clone, serde::Serialize)]
    pub struct AxMethodResult {
        pub method: String, // "1_direct" / "2_unlock" / "3_selected" / "4_range" / "5_tree"
        pub label: String,  // human-readable description
        pub ok: bool,
        pub text: Option<String>,
        pub err: Option<String>,
    }

    /// Full diagnostic report for one focused field.
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

    /// Run all 5 reading strategies on whatever is currently focused.
    /// Used by the Tauri "diagnose_ax" command — Tauri already has Accessibility
    /// permission, so unlike a fresh standalone binary, this always works.
    pub fn diagnose_focused_field() -> AxDiagnostics {
        let mut report = AxDiagnostics {
            ax_trusted: false,
            app_name: None,
            app_pid: None,
            element_role: None,
            attributes: vec![],
            methods: vec![],
            clipboard: pbpaste(),
        };

        unsafe {
            report.ax_trusted = ffi::AXIsProcessTrusted();
            if !report.ax_trusted {
                report.methods.push(AxMethodResult {
                    method: "0_pre".into(),
                    label: "AX permission".into(),
                    ok: false,
                    text: None,
                    err: Some("AXIsProcessTrusted() = false".into()),
                });
                return report;
            }

            let sys = ffi::AXUIElementCreateSystemWide();
            let app_opt = ax_attr(sys as *const _, "AXFocusedApplication");
            ffi::CFRelease(sys);
            let Some(app) = app_opt else {
                report.methods.push(AxMethodResult {
                    method: "0_pre".into(),
                    label: "AXFocusedApplication".into(),
                    ok: false,
                    text: None,
                    err: Some("system-wide AXFocusedApplication returned nil".into()),
                });
                return report;
            };

            // App info
            let mut pid: i32 = -1;
            ffi::AXUIElementGetPid(app as *const _, &mut pid);
            report.app_pid = Some(pid);
            if let Some(t_cf) = ax_attr(app as *const _, "AXTitle") {
                report.app_name = cfstring_to_rust(t_cf as *const _);
                ffi::CFRelease(t_cf);
            }

            // Focused element
            let Some(el) = ax_attr(app as *const _, "AXFocusedUIElement") else {
                report.methods.push(AxMethodResult {
                    method: "0_pre".into(),
                    label: "AXFocusedUIElement".into(),
                    ok: false,
                    text: None,
                    err: Some("AXFocusedUIElement returned nil — no text field focused?".into()),
                });
                ffi::CFRelease(app);
                return report;
            };

            // Role
            if let Some(r_cf) = ax_attr(el as *const _, "AXRole") {
                report.element_role = cfstring_to_rust(r_cf as *const _);
                ffi::CFRelease(r_cf);
            }

            // Attribute names
            report.attributes = list_attribute_names(el as *const _);

            // ── Method 1: direct AXValue ──
            report.methods.push(run_method_1(el as *const _));

            // ── Method 2: unlock + AXValue ──
            report.methods.push(run_method_2(app as *const _));

            // Re-fetch element after unlock for methods 3-5
            let el_post = ax_attr(app as *const _, "AXFocusedUIElement").unwrap_or(el);

            // ── Method 3: AXSelectedText ──
            report.methods.push(run_method_3(el_post as *const _));

            // ── Method 4: AXNumberOfCharacters + AXStringForRange ──
            report.methods.push(run_method_4(el_post as *const _));

            // ── Method 5: tree traversal (concatenate all leaf text nodes) ──
            report
                .methods
                .push(run_method_5(app as *const _, el_post as *const _));

            // ── Method 6: Cmd+A + Cmd+C clipboard capture ──
            // This always works for contenteditable / AX-blind apps, but it is
            // disruptive (briefly selects all and clobbers clipboard).
            report.methods.push(run_method_6());

            if el_post != el {
                ffi::CFRelease(el_post);
            }
            ffi::CFRelease(el);
            ffi::CFRelease(app);
        }
        report
    }

    // ── Diagnostic helpers (each method) ─────────────────────────────────────

    unsafe fn list_attribute_names(el: *const c_void) -> Vec<String> {
        let mut out: *mut c_void = std::ptr::null_mut();
        let err = unsafe { ffi::AXUIElementCopyAttributeNames(el, &mut out) };
        if err != 0 || out.is_null() {
            return vec![];
        }
        let n = unsafe { ffi::CFArrayGetCount(out as *const _) };
        let mut names = Vec::with_capacity(n as usize);
        for i in 0..n {
            let item = unsafe { ffi::CFArrayGetValueAtIndex(out as *const _, i) };
            if let Some(s) = unsafe { cfstring_to_rust(item) } {
                names.push(s);
            }
        }
        unsafe { ffi::CFRelease(out) };
        names
    }

    unsafe fn run_method_1(el: *const c_void) -> AxMethodResult {
        let mut r = AxMethodResult {
            method: "1_direct".into(),
            label: "AXValue (direct)".into(),
            ok: false,
            text: None,
            err: None,
        };
        match unsafe { ax_attr(el, "AXValue") } {
            Some(cf) => {
                let s = unsafe { cfstring_to_rust(cf as *const _) };
                unsafe { ffi::CFRelease(cf) };
                match s {
                    Some(text) => {
                        r.ok = true;
                        r.text = Some(text);
                    }
                    None => {
                        r.err = Some("not a CFString".into());
                    }
                }
            }
            None => {
                r.err = Some("AXValue attribute missing/nil".into());
            }
        }
        r
    }

    unsafe fn run_method_2(app: *const c_void) -> AxMethodResult {
        let mut r = AxMethodResult {
            method: "2_unlock".into(),
            label: "AXEnhancedUserInterface + AXManualAccessibility unlock → AXValue".into(),
            ok: false,
            text: None,
            err: None,
        };
        let r1 = unsafe { ax_set_bool(app, "AXEnhancedUserInterface") };
        let r2 = unsafe { ax_set_bool(app, "AXManualAccessibility") };
        // Chrome needs ~150-200 ms to populate its AX cache after the unlock.
        thread::sleep(Duration::from_millis(200));

        let el2 = unsafe { ax_attr(app, "AXFocusedUIElement") };
        let Some(el2) = el2 else {
            r.err = Some(format!(
                "focused element nil after unlock (set errs: {r1}, {r2})"
            ));
            return r;
        };
        match unsafe { ax_attr(el2 as *const _, "AXValue") } {
            Some(cf) => {
                let s = unsafe { cfstring_to_rust(cf as *const _) };
                unsafe { ffi::CFRelease(cf) };
                match s {
                    Some(text) => {
                        r.ok = true;
                        r.text = Some(text);
                    }
                    None => {
                        r.err = Some("not a CFString".into());
                    }
                }
            }
            None => {
                r.err = Some(format!(
                    "AXValue still nil after unlock (set errs: {r1}, {r2})"
                ));
            }
        }
        unsafe { ffi::CFRelease(el2) };
        r
    }

    unsafe fn run_method_3(el: *const c_void) -> AxMethodResult {
        let mut r = AxMethodResult {
            method: "3_selected".into(),
            label: "AXSelectedText".into(),
            ok: false,
            text: None,
            err: None,
        };
        match unsafe { ax_attr(el, "AXSelectedText") } {
            Some(cf) => {
                let s = unsafe { cfstring_to_rust(cf as *const _) };
                unsafe { ffi::CFRelease(cf) };
                match s {
                    Some(text) => {
                        r.ok = true;
                        r.text = Some(text);
                    }
                    None => {
                        r.err = Some("not a CFString".into());
                    }
                }
            }
            None => {
                r.err = Some("AXSelectedText missing/nil".into());
            }
        }
        r
    }

    unsafe fn run_method_4(el: *const c_void) -> AxMethodResult {
        let mut r = AxMethodResult {
            method: "4_range".into(),
            label: "AXNumberOfCharacters + AXStringForRange".into(),
            ok: false,
            text: None,
            err: None,
        };
        let n_cf = match unsafe { ax_attr(el, "AXNumberOfCharacters") } {
            Some(c) => c,
            None => {
                r.err = Some("AXNumberOfCharacters missing".into());
                return r;
            }
        };
        let mut char_count: i64 = 0;
        // kCFNumberSInt64Type = 4
        let ok = unsafe {
            ffi::CFNumberGetValue(
                n_cf as *const _,
                4,
                &mut char_count as *mut i64 as *mut c_void,
            )
        };
        unsafe { ffi::CFRelease(n_cf) };
        if !ok || char_count <= 0 {
            r.err = Some(format!("AXNumberOfCharacters={char_count}"));
            return r;
        }

        let range = CFRange {
            location: 0,
            length: char_count,
        };
        let range_val = unsafe {
            ffi::AXValueCreate(
                K_AX_VALUE_CF_RANGE_TYPE,
                &range as *const _ as *const c_void,
            )
        };
        if range_val.is_null() {
            r.err = Some("AXValueCreate returned null".into());
            return r;
        }
        let attr = unsafe { cf_str("AXStringForRange") };
        let mut out: *mut c_void = std::ptr::null_mut();
        let err = unsafe {
            ffi::AXUIElementCopyParameterizedAttributeValue(el, attr, range_val, &mut out)
        };
        unsafe { ffi::CFRelease(attr) };
        unsafe { ffi::CFRelease(range_val) };
        if err != 0 || out.is_null() {
            r.err = Some(format!("AXStringForRange err={err}"));
            return r;
        }
        let s = unsafe { cfstring_to_rust(out as *const _) };
        unsafe { ffi::CFRelease(out) };
        match s {
            Some(text) => {
                r.ok = true;
                r.text = Some(text);
            }
            None => {
                r.err = Some("not a CFString".into());
            }
        }
        r
    }

    unsafe fn run_method_5(app: *const c_void, el: *const c_void) -> AxMethodResult {
        let mut r = AxMethodResult {
            method: "5_tree".into(),
            label: "Tree traversal — concatenate all leaf AXValues (depth 14)".into(),
            ok: false,
            text: None,
            err: None,
        };

        // Try from the focused element first (deep = 14).
        let mut parts: Vec<String> = Vec::new();
        unsafe { collect_all_text(el, 0, 14, &mut parts) };

        // If the focused element's subtree is empty, try from the focused window
        // (Chrome's contenteditable may be nested deeper in the DOM tree).
        if parts.is_empty() {
            if let Some(win) = unsafe { ax_attr(app, "AXFocusedWindow") } {
                unsafe { collect_all_text(win as *const _, 0, 12, &mut parts) };
                unsafe { ffi::CFRelease(win) };
            }
        }

        if parts.is_empty() {
            r.err = Some("no non-empty AXValue found in subtree".into());
        } else {
            r.ok = true;
            r.text = Some(parts.join(" "));
        }
        r
    }

    /// Recursively collect all non-empty `AXValue` strings from the subtree.
    /// Unlike the old first-match approach, this concatenates leaf text nodes —
    /// which is how Chrome exposes contenteditable content (as AXStaticText children).
    unsafe fn collect_all_text(
        el: *const c_void,
        depth: usize,
        max_depth: usize,
        out: &mut Vec<String>,
    ) {
        if depth > max_depth {
            return;
        }

        // Read AXValue on this node.
        if let Some(val) = unsafe { ax_attr(el, "AXValue") } {
            let s = unsafe { cfstring_to_rust(val as *const _) };
            unsafe { ffi::CFRelease(val) };
            if let Some(t) = s {
                if !t.trim().is_empty() {
                    out.push(t);
                    // Don't recurse into children if we already got text here —
                    // avoids double-counting parent + child nodes.
                    return;
                }
            }
        }

        // No text on this node — recurse into children.
        let Some(children) = (unsafe { ax_attr(el, "AXChildren") }) else {
            return;
        };
        let n = unsafe { ffi::CFArrayGetCount(children as *const _) };
        // Cap children to avoid O(n) blowup on deeply-branching trees.
        let limit = n.min(128);
        for i in 0..limit {
            let child = unsafe { ffi::CFArrayGetValueAtIndex(children as *const _, i) };
            // Some AX providers (Electron/Chromium) can return null elements in
            // the children array; recursing into one segfaults. Skip them.
            if child.is_null() {
                continue;
            }
            unsafe { collect_all_text(child, depth + 1, max_depth, out) };
        }
        unsafe { ffi::CFRelease(children) };
    }

    /// Method 6: Cmd+A → Cmd+C clipboard capture (always works, but disruptive).
    /// Returned as a diagnostic method so the UI can show whether this path works.
    fn run_method_6() -> AxMethodResult {
        let mut r = AxMethodResult {
            method: "6_clipboard".into(),
            label: "Cmd+A + Cmd+C clipboard capture (disruptive, always works)".into(),
            ok: false,
            text: None,
            err: None,
        };
        match capture_focused_text_via_selection() {
            Some(text) if !text.is_empty() => {
                r.ok = true;
                r.text = Some(text);
            }
            Some(_) => {
                r.err = Some("captured empty string".into());
            }
            None => {
                r.err = Some("capture returned None (AX not trusted or field empty)".into());
            }
        }
        r
    }

    /// Ensure AirNote appears in the Accessibility list and ask macOS to prompt.
    pub fn request_permission() {
        unsafe {
            let key = ffi::CFStringCreateWithCString(
                std::ptr::null(),
                c"AXTrustedCheckOptionPrompt".as_ptr(),
                CF_UTF8,
            );
            if key.is_null() {
                ffi::AXIsProcessTrustedWithOptions(std::ptr::null());
            } else {
                let keys = [key as *const c_void];
                let values = [ffi::kCFBooleanTrue];
                let options = ffi::CFDictionaryCreate(
                    std::ptr::null(),
                    keys.as_ptr(),
                    values.as_ptr(),
                    1,
                    &ffi::kCFTypeDictionaryKeyCallBacks as *const _ as *const c_void,
                    &ffi::kCFTypeDictionaryValueCallBacks as *const _ as *const c_void,
                );
                if options.is_null() {
                    ffi::AXIsProcessTrustedWithOptions(std::ptr::null());
                } else {
                    ffi::AXIsProcessTrustedWithOptions(options);
                    ffi::CFRelease(options);
                }
                ffi::CFRelease(key);
            }
        }
        let _ = std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
            .spawn();
    }

    /// Ask macOS for Input Monitoring, then open the matching System Settings pane.
    pub fn request_input_monitoring() {
        unsafe {
            if !ffi::CGPreflightListenEventAccess() {
                ffi::CGRequestListenEventAccess();
            }
        }
        let _ = std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent")
            .spawn();
    }

    fn post_key(source: *mut std::ffi::c_void, keycode: u16, key_down: bool, flags: u64) {
        unsafe {
            let event = ffi::CGEventCreateKeyboardEvent(source as *const _, keycode, key_down);
            if !event.is_null() {
                ffi::CGEventSetFlags(event, flags);
                ffi::CGEventPost(K_CG_HID_EVENT_TAP, event);
                ffi::CFRelease(event);
            }
        }
    }

    fn post_key_repeated(keycode: u16, count: usize, delay_ms: u64) -> Result<(), String> {
        if count == 0 {
            return Ok(());
        }
        unsafe {
            let source = ffi::CGEventSourceCreate(K_CG_EVENT_SOURCE_STATE_COMBINED_SESSION);
            if source.is_null() {
                return Err("CGEventSourceCreate failed".into());
            }
            for _ in 0..count {
                post_key(source, keycode, true, 0);
                thread::sleep(Duration::from_millis(delay_ms));
                post_key(source, keycode, false, 0);
                thread::sleep(Duration::from_millis(delay_ms));
            }
            ffi::CFRelease(source);
        }
        Ok(())
    }

    fn post_unicode_chunk(source: *mut std::ffi::c_void, text: &str) -> Result<(), String> {
        let utf16: Vec<u16> = text.encode_utf16().collect();
        let len = utf16.len() as u64;
        let ptr = utf16.as_ptr();

        unsafe {
            let dn = ffi::CGEventCreateKeyboardEvent(source, 0, true);
            if dn.is_null() {
                return Err("CGEventCreateKeyboardEvent keydown failed".into());
            }
            ffi::CGEventKeyboardSetUnicodeString(dn, len, ptr);
            ffi::CGEventPost(K_CG_HID_EVENT_TAP, dn);
            ffi::CFRelease(dn);

            // Give the HID stack time to dispatch keydown before keyup.
            thread::sleep(Duration::from_millis(6));

            let up = ffi::CGEventCreateKeyboardEvent(source, 0, false);
            if up.is_null() {
                return Err("CGEventCreateKeyboardEvent keyup failed".into());
            }
            ffi::CGEventKeyboardSetUnicodeString(up, len, ptr);
            ffi::CGEventPost(K_CG_HID_EVENT_TAP, up);
            ffi::CFRelease(up);

            // Pause after keyup so subsequent chunks cannot interleave.
            thread::sleep(Duration::from_millis(12));
        }

        Ok(())
    }

    fn pbcopy(text: &str) {
        if let Ok(mut child) = Command::new("pbcopy")
            .stdin(std::process::Stdio::piped())
            .spawn()
        {
            if let Some(ref mut stdin) = child.stdin {
                let _ = stdin.write_all(text.as_bytes());
            }
            let _ = child.wait();
        }
    }

    fn pbpaste() -> String {
        Command::new("pbpaste")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default()
    }

    pub fn is_accessibility_granted() -> bool {
        unsafe { ffi::AXIsProcessTrusted() }
    }

    /// Type `text` directly into the focused app using synthetic Unicode keyboard
    /// events — no clipboard involved, works for any script.
    ///
    /// Returns `Ok(true)` if text was actually typed, `Ok(false)` if Accessibility
    /// is not granted (caller should fall back to clipboard paste), or `Err` on
    /// a system-level failure.
    pub fn type_text(text: &str) -> Result<bool, String> {
        if text.is_empty() {
            return Ok(true);
        }
        unsafe {
            if !ffi::AXIsProcessTrusted() {
                return Ok(false);
            }
            let source = ffi::CGEventSourceCreate(K_CG_EVENT_SOURCE_STATE_COMBINED_SESSION);
            if source.is_null() {
                return Err("CGEventSourceCreate failed".into());
            }

            // CGEventKeyboardSetUnicodeString handles multi-character strings,
            // but chunking avoids very large final dictation payloads getting
            // dropped by the focused app while keeping the system clipboard clean.
            const MAX_CHUNK_CHARS: usize = 96;
            let mut start = 0;
            let mut count = 0;
            let mut result = Ok(());
            for (idx, _) in text.char_indices() {
                if count == MAX_CHUNK_CHARS {
                    result = post_unicode_chunk(source, &text[start..idx]);
                    if result.is_err() {
                        break;
                    }
                    start = idx;
                    count = 0;
                }
                count += 1;
            }
            if result.is_ok() && start < text.len() {
                result = post_unicode_chunk(source, &text[start..]);
            }

            if !source.is_null() {
                ffi::CFRelease(source);
            }
            result?;
        }
        Ok(true)
    }

    pub fn paste(text: &str) -> Result<(), String> {
        paste_inner(text, /* select_all_first = */ false)
    }

    /// Paste replacing whatever is currently in the focused field. Sends
    /// Cmd+A to select-all, then Cmd+V to replace. Use this when the
    /// caller knows it needs to overwrite existing content (the
    /// safety-paste path that fires when word-by-word typing partially
    /// failed or got reset by a draft-then-final LLM stream).
    pub fn paste_replacing(text: &str) -> Result<(), String> {
        paste_inner(text, /* select_all_first = */ true)
    }

    /// Replace only the suffix that AirNote typed during the current recording.
    /// This avoids Cmd+A in normal voice fallback paths, so existing user text
    /// in the field is not selected or overwritten.
    pub fn replace_typed_suffix(typed_text: &str, replacement: &str) -> Result<(), String> {
        let chars_to_delete = typed_text.chars().count();
        if chars_to_delete == 0 {
            return paste(replacement);
        }
        let ax_ok = unsafe { ffi::AXIsProcessTrusted() };
        tracing::info!(
            "[paste] replacing current typed suffix — delete_chars={} replacement_len={}",
            chars_to_delete,
            replacement.len()
        );
        if !ax_ok {
            return Err("Accessibility permission not granted — go to System Settings → Privacy → Accessibility and enable AirNote".into());
        }

        unsafe {
            let source = ffi::CGEventSourceCreate(K_CG_EVENT_SOURCE_STATE_COMBINED_SESSION);
            for _ in 0..chars_to_delete {
                post_key(source, KEY_DELETE, true, 0);
                thread::sleep(Duration::from_millis(2));
                post_key(source, KEY_DELETE, false, 0);
                thread::sleep(Duration::from_millis(2));
            }
            if !source.is_null() {
                ffi::CFRelease(source);
            }
        }
        thread::sleep(Duration::from_millis(40));
        match type_text(replacement) {
            Ok(true) => Ok(()),
            Ok(false) => paste(replacement),
            Err(e) => {
                tracing::warn!(
                    "[paste] direct replacement typing failed ({e}) — falling back to clipboard paste"
                );
                paste(replacement)
            }
        }
    }

    fn diff_single_span(old: &str, new: &str) -> (usize, usize, String, usize) {
        let old_chars: Vec<char> = old.chars().collect();
        let new_chars: Vec<char> = new.chars().collect();

        let mut prefix_len = 0;
        while prefix_len < old_chars.len()
            && prefix_len < new_chars.len()
            && old_chars[prefix_len] == new_chars[prefix_len]
        {
            prefix_len += 1;
        }

        let mut suffix_len = 0;
        while suffix_len < old_chars.len().saturating_sub(prefix_len)
            && suffix_len < new_chars.len().saturating_sub(prefix_len)
            && old_chars[old_chars.len() - 1 - suffix_len]
                == new_chars[new_chars.len() - 1 - suffix_len]
        {
            suffix_len += 1;
        }

        let old_mid_len = old_chars.len().saturating_sub(prefix_len + suffix_len);
        let new_mid: String = new_chars[prefix_len..new_chars.len().saturating_sub(suffix_len)]
            .iter()
            .collect();

        (prefix_len, old_mid_len, new_mid, suffix_len)
    }

    /// With the caret at the END of `typed_text`, compute the minimal tail rewrite
    /// that turns `typed_text` into `replacement`: how many BACKSPACES to delete
    /// from the end, and the string to retype. Pure (no I/O) so it can be unit
    /// tested. Invariants (locked in by `tail_rewrite_*` tests):
    ///   * `backspaces <= typed_text.chars().count()` — the deletion never reaches
    ///     past our own streamed text into the user's surrounding content.
    ///   * `(typed_text minus its last `backspaces` chars) + retype == replacement`.
    ///
    /// The safety invariant assumes `typed_text` matches what physically landed in
    /// the field (every streamed token typed successfully). The AX-range path
    /// verifies that against the live field value; this blind fallback trusts it.
    fn tail_rewrite(typed_text: &str, replacement: &str) -> (usize, String) {
        let (_prefix_len, old_mid_len, new_mid, suffix_len) =
            diff_single_span(typed_text, replacement);
        let typed_chars: Vec<char> = typed_text.chars().collect();
        let suffix_str: String = typed_chars[typed_chars.len().saturating_sub(suffix_len)..]
            .iter()
            .collect();
        (old_mid_len + suffix_len, format!("{new_mid}{suffix_str}"))
    }

    fn utf16_len_for_chars(text: &str, char_count: usize) -> i64 {
        text.chars()
            .take(char_count)
            .map(char::len_utf16)
            .sum::<usize>() as i64
    }

    fn find_current_recording_span(
        initial_text: Option<&str>,
        current_text: &str,
        typed_text: &str,
    ) -> Option<usize> {
        if typed_text.is_empty() {
            return None;
        }

        if let Some(initial) = initial_text {
            // Reliable anchor: the field is exactly the pre-existing text with our
            // streamed text inserted at a single point —
            // current == initial[..split] + typed + initial[split..].
            // Only edit when this holds exactly. If the field no longer matches our
            // model (length differs, content changed), do NOT guess a location:
            // return None so the caller leaves the streamed text untouched rather
            // than editing at a wrong offset.
            let initial_chars: Vec<char> = initial.chars().collect();
            let current_chars: Vec<char> = current_text.chars().collect();
            let typed_chars: Vec<char> = typed_text.chars().collect();
            if current_chars.len() == initial_chars.len() + typed_chars.len() {
                for split in 0..=initial_chars.len() {
                    if current_chars[..split] == initial_chars[..split]
                        && current_chars[split..split + typed_chars.len()] == typed_chars[..]
                        && current_chars[split + typed_chars.len()..] == initial_chars[split..]
                    {
                        return Some(split);
                    }
                }
            }
            return None;
        }

        // No reliable baseline (the field wasn't readable before dictation): accept
        // only a single, unambiguous occurrence of the streamed text. A non-unique
        // match — or no match — is too risky to edit, so return None and let the
        // caller keep the streamed text. The previous `ends_with` fallback is
        // intentionally removed: it assumed our text sat at the very end of the
        // field, which is precisely wrong when the user dictated into a field that
        // already had trailing content (the reported "too much content" bug).
        let mut matches = Vec::new();
        let mut search_from = 0;
        while let Some(relative) = current_text[search_from..].find(typed_text) {
            let byte_idx = search_from + relative;
            matches.push(current_text[..byte_idx].chars().count());
            search_from = byte_idx + typed_text.len();
        }
        if matches.len() == 1 {
            matches.into_iter().next()
        } else {
            None
        }
    }

    unsafe fn focused_ui_element() -> Option<*mut c_void> {
        let sys = unsafe { ffi::AXUIElementCreateSystemWide() };
        if sys.is_null() {
            return None;
        }
        let focused_app = unsafe { ax_attr(sys as *const _, "AXFocusedApplication") };
        unsafe { ffi::CFRelease(sys) };

        let app_elem = focused_app?;
        let focused_elem = unsafe { ax_attr(app_elem as *const _, "AXFocusedUIElement") };
        unsafe { ffi::CFRelease(app_elem) };
        focused_elem
    }

    unsafe fn set_selected_text_range(
        element: *const c_void,
        location: i64,
        length: i64,
    ) -> Result<(), String> {
        let range = CFRange { location, length };
        let range_val = unsafe {
            ffi::AXValueCreate(
                K_AX_VALUE_CF_RANGE_TYPE,
                &range as *const _ as *const c_void,
            )
        };
        if range_val.is_null() {
            return Err("AXValueCreate for selected range failed".into());
        }
        let attr = unsafe { cf_str("AXSelectedTextRange") };
        if attr.is_null() {
            unsafe { ffi::CFRelease(range_val) };
            return Err("CFStringCreate for AXSelectedTextRange failed".into());
        }
        let err = unsafe { ffi::AXUIElementSetAttributeValue(element, attr, range_val) };
        unsafe {
            ffi::CFRelease(attr);
            ffi::CFRelease(range_val);
        }
        if err != 0 {
            return Err(format!("AXSelectedTextRange set failed err={err}"));
        }
        Ok(())
    }

    fn type_or_paste_at_cursor(text: &str) -> Result<(), String> {
        if text.is_empty() {
            return Ok(());
        }
        match type_text(text) {
            Ok(true) => Ok(()),
            Ok(false) => paste(text),
            Err(e) => {
                tracing::warn!(
                    "[paste] direct reconciliation typing failed ({e}) — falling back to clipboard paste"
                );
                paste(text)
            }
        }
    }

    fn try_reconcile_by_ax_range(
        initial_text: Option<&str>,
        typed_text: &str,
        replacement: &str,
    ) -> Result<Option<bool>, String> {
        if !unsafe { ffi::AXIsProcessTrusted() } {
            return Err("Accessibility permission not granted — go to System Settings → Privacy → Accessibility and enable AirNote".into());
        }

        let elem = match unsafe { focused_ui_element() } {
            Some(elem) => elem,
            None => return Ok(None),
        };

        let current = unsafe { ax_attr(elem as *const _, "AXValue") }.and_then(|value| {
            let text = unsafe { cfstring_to_rust(value as *const _) };
            unsafe { ffi::CFRelease(value) };
            text
        });
        let Some(current_text) = current else {
            unsafe { ffi::CFRelease(elem) };
            return Ok(None);
        };

        let Some(span_start_chars) =
            find_current_recording_span(initial_text, &current_text, typed_text)
        else {
            unsafe { ffi::CFRelease(elem) };
            return Err("current recording span was not found in focused field".into());
        };

        let (prefix_len, old_mid_len, new_mid, suffix_len) =
            diff_single_span(typed_text, replacement);
        let selection_start_chars = span_start_chars + prefix_len;
        let selection_location = utf16_len_for_chars(&current_text, selection_start_chars);
        let old_mid: String = typed_text
            .chars()
            .skip(prefix_len)
            .take(old_mid_len)
            .collect();
        let selection_length = old_mid.encode_utf16().count() as i64;

        tracing::info!(
            "[paste] AX range reconcile — span_start_chars={} prefix_chars={} old_mid_chars={} new_mid_chars={} suffix_chars={}",
            span_start_chars,
            prefix_len,
            old_mid_len,
            new_mid.chars().count(),
            suffix_len,
        );

        unsafe { set_selected_text_range(elem as *const _, selection_location, selection_length)? };
        unsafe { ffi::CFRelease(elem) };

        if new_mid.is_empty() {
            // Pure deletion: a backspace deletes the selected old_mid range. Guard
            // on a non-empty selection so we never delete a char to the left of an
            // empty caret (only reachable if a future caller passes typed==replacement).
            if selection_length > 0 {
                post_key_repeated(KEY_DELETE, 1, 2)?;
            }
        } else {
            type_or_paste_at_cursor(&new_mid)?;
        }
        post_key_repeated(KEY_RIGHT, suffix_len, 1)?;
        Ok(Some(true))
    }

    /// Reconcile the text typed during the current recording by first locating
    /// that exact span in the focused field. If AX text is not readable, falls
    /// back to caret-based reconciliation.
    pub fn reconcile_current_recording(
        initial_text: Option<&str>,
        typed_text: &str,
        replacement: &str,
    ) -> Result<bool, String> {
        if typed_text == replacement {
            return Ok(false);
        }
        match try_reconcile_by_ax_range(initial_text, typed_text, replacement) {
            Ok(Some(changed)) => Ok(changed),
            Ok(None) => {
                tracing::warn!(
                    "[paste] AX range reconcile unavailable — falling back to caret-based reconciliation"
                );
                reconcile_typed_text(typed_text, replacement)
            }
            Err(e) => Err(e),
        }
    }

    /// Safely replace an existing focused-field span only when the exact text
    /// can be located through AX. This is used by repair/refine actions where
    /// we must not blindly type into the user's current app.
    pub fn replace_focused_text_exact(
        existing_text: &str,
        replacement: &str,
    ) -> Result<bool, String> {
        if existing_text == replacement {
            return Ok(false);
        }
        match try_reconcile_by_ax_range(None, existing_text, replacement) {
            Ok(Some(changed)) => Ok(changed),
            Ok(None) => Ok(false),
            Err(e) => {
                tracing::warn!("[paste] exact focused-text replacement skipped: {e}");
                Ok(false)
            }
        }
    }

    /// Reconcile the text typed during the current recording with the final
    /// backend output using the smallest single-span edit we can derive from
    /// common prefix/suffix. The caret is expected to be at the end of the text
    /// AirNote just streamed.
    pub fn reconcile_typed_text(typed_text: &str, replacement: &str) -> Result<bool, String> {
        if typed_text == replacement {
            return Ok(false);
        }
        if typed_text.is_empty() {
            paste(replacement)?;
            return Ok(true);
        }

        let ax_ok = unsafe { ffi::AXIsProcessTrusted() };
        if !ax_ok {
            return Err("Accessibility permission not granted — go to System Settings → Privacy → Accessibility and enable AirNote".into());
        }

        // The caret is at the END of the text AirNote just streamed. Rewrite only
        // our own trailing characters using BACKSPACES anchored at the caret —
        // never arrow-key navigation. Arrow nav assumes the field holds *only* our
        // typed text; when the field already had content (dictating into a
        // non-empty field, or mid-text), KEY_LEFT/KEY_RIGHT cross into the user's
        // surrounding text and the delete/retype lands at the wrong offset,
        // duplicating or garbling content. `tail_rewrite` only ever backspaces our
        // own typed tail (count <= typed_text length), then retypes the corrected
        // tail — so the user's surrounding text can never be touched.
        let (backspaces, retype) = tail_rewrite(typed_text, replacement);
        tracing::info!(
            "[paste] reconciling current typed text — backspaces={} retype_chars={}",
            backspaces,
            retype.chars().count(),
        );
        post_key_repeated(KEY_DELETE, backspaces, 2)?;
        type_or_paste_at_cursor(&retype)?;

        Ok(true)
    }

    fn paste_inner(text: &str, select_all_first: bool) -> Result<(), String> {
        let ax_ok = unsafe { ffi::AXIsProcessTrusted() };
        tracing::info!(
            "[paste] called — AXIsProcessTrusted={ax_ok}, text_len={}, select_all_first={}",
            text.len(),
            select_all_first,
        );

        if !ax_ok {
            tracing::warn!(
                "[paste] Accessibility NOT granted — cannot paste. \
                Grant AirNote in System Settings → Privacy & Security → Accessibility, then restart."
            );
            return Err("Accessibility permission not granted — go to System Settings → Privacy → Accessibility and enable AirNote".into());
        }

        // Copy text to clipboard, optionally select-all, send Cmd+V, then restore original clipboard
        let original = pbpaste();
        pbcopy(text);
        thread::sleep(Duration::from_millis(80));

        unsafe {
            let source = ffi::CGEventSourceCreate(K_CG_EVENT_SOURCE_STATE_COMBINED_SESSION);

            // Select-all first if requested (Cmd+A) so the paste REPLACES
            // existing content instead of appending.
            if select_all_first {
                tracing::info!("[paste] sending Cmd+A (select-all-then-replace)");
                post_key(source, KEY_CMD, true, 0);
                thread::sleep(Duration::from_millis(10));
                post_key(source, KEY_A, true, K_CG_FLAG_COMMAND);
                thread::sleep(Duration::from_millis(10));
                post_key(source, KEY_A, false, K_CG_FLAG_COMMAND);
                thread::sleep(Duration::from_millis(20));
            }

            tracing::debug!("[paste] sending Cmd+V keypress");
            if !select_all_first {
                post_key(source, KEY_CMD, true, 0);
                thread::sleep(Duration::from_millis(10));
            }
            post_key(source, KEY_V, true, K_CG_FLAG_COMMAND);
            thread::sleep(Duration::from_millis(10));
            post_key(source, KEY_V, false, K_CG_FLAG_COMMAND);
            thread::sleep(Duration::from_millis(10));
            post_key(source, KEY_CMD, false, 0);

            if !source.is_null() {
                ffi::CFRelease(source);
            }
        }

        thread::sleep(Duration::from_millis(400));
        pbcopy(&original);
        tracing::debug!("[paste] done — clipboard restored");
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::{diff_single_span, find_current_recording_span, tail_rewrite};

        #[test]
        fn diff_single_span_keeps_common_prefix_and_suffix() {
            let (prefix, old_mid_len, new_mid, suffix) =
                diff_single_span("monthly five dollar dena padega", "monthly $5 dena padega");
            assert_eq!(prefix, "monthly ".chars().count());
            assert_eq!(old_mid_len, "five dollar".chars().count());
            assert_eq!(new_mid, "$5");
            assert_eq!(suffix, " dena padega".chars().count());
        }

        #[test]
        fn current_recording_span_uses_initial_field_context() {
            let initial = "Hello  world";
            let typed = "monthly five dollar dena padega";
            let current = "Hello monthly five dollar dena padega world";
            assert_eq!(
                find_current_recording_span(Some(initial), current, typed),
                Some("Hello ".chars().count())
            );
        }

        #[test]
        fn current_recording_span_requires_unique_fallback_match() {
            let typed = "same";
            assert_eq!(
                find_current_recording_span(None, "before same after", typed),
                Some("before ".chars().count())
            );
            // Ambiguous: "same" occurs twice and we have no initial-field anchor,
            // so refuse to guess. The old code matched the trailing occurrence via
            // `ends_with` — exactly how dictating into a non-empty field got the
            // final output garbled. Returning None makes the caller keep the
            // already-correct streamed text instead.
            assert_eq!(
                find_current_recording_span(None, "same before same", typed),
                None
            );
        }

        #[test]
        fn current_recording_span_refuses_to_guess_when_field_changed() {
            // initial-field text is known but the field no longer matches
            // initial+typed insertion (the app mutated it) — refuse to edit at a
            // guessed offset rather than risk garbling existing content.
            assert_eq!(
                find_current_recording_span(
                    Some("Hello world"),
                    "wildly different content",
                    "spoken"
                ),
                None
            );
        }

        #[test]
        fn tail_rewrite_only_touches_our_tail_and_reconstructs_replacement() {
            // (streamed text, final replacement) covering: tail tweak, mid tweak,
            // word fix, append, deletion, pure append, partial/RESET (fragment ->
            // full), and multibyte + emoji.
            let cases = [
                ("5 pm", "5 PM"),
                ("5 pm world", "5 PM world"),
                ("helo world", "hello world"),
                ("main offic", "main office"),
                ("hello world", "hello"),
                ("hello", "hello world"),
                ("मैं", "Main office ja raha hoon"),
                ("cafe", "café ☕"),
            ];
            for (typed, repl) in cases {
                let (backspaces, retype) = tail_rewrite(typed, repl);
                let typed_len = typed.chars().count();
                // SAFETY: never delete more than we ourselves typed — the deletion
                // can never reach the user's surrounding text.
                assert!(
                    backspaces <= typed_len,
                    "backspaces {backspaces} > typed len {typed_len} for typed={typed:?}"
                );
                // CORRECTNESS: deleting our last `backspaces` streamed chars and
                // retyping `retype` reproduces the final output exactly.
                let kept: String = typed.chars().take(typed_len - backspaces).collect();
                assert_eq!(
                    format!("{kept}{retype}"),
                    repl,
                    "reconstruction failed for typed={typed:?} repl={repl:?}"
                );
            }
        }
    }
}

#[cfg(target_os = "windows")]
mod imp_windows;

#[cfg(target_os = "windows")]
mod uia;

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod imp_other;

#[cfg(target_os = "macos")]
pub use imp::*;

#[cfg(target_os = "windows")]
pub use imp_windows::*;

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub use imp_other::*;
